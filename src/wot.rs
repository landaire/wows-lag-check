//! WoT replay analysis. Shares the BigWorld container with WoWs but with a
//! different Blowfish key and renumbered packet ids; only PlayerNetStats and
//! the server tick are decoded.

use std::io::Read;

use blowfish::Blowfish;
use byteorder::BE;
use cipher::BlockDecrypt;
use cipher::KeyInit;
use cipher::generic_array::GenericArray;
use serde::Deserialize;

use crate::MAX_REPLAY_CLOCK_S;
use crate::analysis;
use crate::replay;

const REPLAY_MAGIC: u32 = 0x11343212;

const WOT_BLOWFISH_KEY: [u8; 16] = [
    0xDE, 0x72, 0xBE, 0xA0, 0xDE, 0x04, 0xBE, 0xB1, 0xDE, 0xFE, 0xBE, 0xEF, 0xDE, 0xAD, 0xBE, 0xEF,
];

/// WoT packet ids we care about. Other ids are walked but discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PacketKind {
    /// 4-byte (fps:8, ping:16, lag:1) at 10Hz.
    PlayerNetStats,
    /// 72-byte tick with monotonic counter at offset 12, 10Hz.
    ServerTick,
    Other,
}

impl PacketKind {
    fn from_raw(raw: u32) -> Self {
        match raw {
            0x1f => Self::PlayerNetStats,
            0x39 => Self::ServerTick,
            _ => Self::Other,
        }
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct WotMeta {
    clientVersionFromExe: String,
    #[serde(default)]
    clientVersionFromXml: String,
    #[serde(default)]
    mapName: String,
    #[serde(default)]
    mapDisplayName: String,
    #[serde(default)]
    dateTime: String,
    #[serde(default)]
    playerName: String,
    #[serde(default)]
    playerVehicle: String,
    #[serde(default)]
    serverName: Option<String>,
    #[serde(default)]
    battleType: Option<u32>,
    #[serde(default)]
    gameplayID: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ReplayInfo {
    pub client_version: String,
    pub client_version_long: String,
}

pub fn looks_like_wot(bytes: &[u8]) -> bool {
    let Ok(container) = Container::parse(bytes) else {
        return false;
    };
    let s = std::str::from_utf8(container.meta_block).unwrap_or("");
    s.contains("\"serverName\"") || s.contains("\"clientVersionFromXml\":\"World")
}

pub fn replay_info(bytes: &[u8]) -> Result<ReplayInfo, String> {
    let container = Container::parse(bytes)?;
    let meta = container.parse_meta()?;
    Ok(ReplayInfo {
        client_version: meta.clientVersionFromExe,
        client_version_long: meta.clientVersionFromXml,
    })
}

pub fn analyze(
    bytes: &[u8],
    threshold_ms: Option<u32>,
) -> Result<analysis::AnalysisResult, String> {
    let container = Container::parse(bytes)?;
    let meta = container.parse_meta()?;
    let packet_data = container.decrypt_packet_stream()?;
    let mut decoder = WotDecoder::default();
    decoder.walk(&packet_data)?;
    Ok(decoder.finish(&meta, threshold_ms))
}

/// BigWorld outer container: `[u32 magic][u32 block_count][block...][u32
/// decomp_size][u32 comp_size][encrypted body]`. Each block is `[u32 len][len
/// bytes]`. The first block is JSON metadata.
struct Container<'a> {
    meta_block: &'a [u8],
    encrypted: &'a [u8],
}

impl<'a> Container<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() < 12 {
            return Err("truncated replay header".into());
        }
        let magic = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        if magic != REPLAY_MAGIC {
            return Err(format!("bad magic 0x{magic:08x}"));
        }
        let block_count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if block_count == 0 {
            return Err("zero block count".into());
        }

        let mut off = 8usize;
        let mut meta_block: Option<&[u8]> = None;
        for i in 0..block_count {
            if bytes.len() < off + 4 {
                return Err("truncated block length".into());
            }
            let blen = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
            off += 4;
            if bytes.len() < off + blen {
                return Err("truncated block body".into());
            }
            if i == 0 {
                meta_block = Some(&bytes[off..off + blen]);
            }
            off += blen;
        }
        if bytes.len() < off + 8 {
            return Err("truncated size header".into());
        }
        // decomp_size and comp_size follow but aren't needed; the encrypted
        // body runs to end-of-file and may be 0-padded to a block boundary.
        Ok(Container {
            meta_block: meta_block.expect("block_count >= 1"),
            encrypted: &bytes[off + 8..],
        })
    }

    fn parse_meta(&self) -> Result<WotMeta, String> {
        serde_json::from_slice(self.meta_block).map_err(|e| format!("wot meta: {e}"))
    }

    fn decrypt_packet_stream(&self) -> Result<Vec<u8>, String> {
        let mut cipher = PlaintextFeedbackCipher::new(&WOT_BLOWFISH_KEY)?;
        let decrypted = cipher.decrypt(self.encrypted);
        let mut inflater = flate2::read::ZlibDecoder::new(decrypted.as_slice());
        let mut packet_data = Vec::new();
        inflater.read_to_end(&mut packet_data).map_err(|e| format!("wot inflate: {e}"))?;
        Ok(packet_data)
    }
}

/// Blowfish in plaintext-feedback mode (XOR-with-previous-plaintext), IV=0.
/// Matches `BW::Mercury::EncryptionFilter::decrypt`.
struct PlaintextFeedbackCipher {
    cipher: Blowfish<BE>,
    previous: [u8; 8],
}

impl PlaintextFeedbackCipher {
    fn new(key: &[u8; 16]) -> Result<Self, String> {
        let cipher = <Blowfish<BE>>::new_from_slice(key).map_err(|e| format!("blowfish key: {e}"))?;
        Ok(Self { cipher, previous: [0u8; 8] })
    }

    fn decrypt(&mut self, encrypted: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; encrypted.len()];
        for chunk_idx in 0..(encrypted.len() / 8) {
            let off = chunk_idx * 8;
            let mut block = GenericArray::clone_from_slice(&encrypted[off..off + 8]);
            self.cipher.decrypt_block(&mut block);
            for j in 0..8 {
                out[off + j] = block[j] ^ self.previous[j];
            }
            self.previous.copy_from_slice(&out[off..off + 8]);
        }
        out
    }
}

/// Walks the decrypted stream and feeds the spike detector.
#[derive(Default)]
struct WotDecoder {
    samples: Vec<analysis::NetStat>,
    tick_clocks: Vec<f32>,
    headers: Vec<analysis::PacketHeader>,
    corrupt_packet_clocks: Vec<f32>,
}

impl WotDecoder {
    fn walk(&mut self, packet_data: &[u8]) -> Result<(), String> {
        let mut last_valid_clock: f32 = 0.0;
        for packet in PacketIterator::new(packet_data) {
            let packet = packet?;
            let clock = packet.clock;
            if !(0.0..MAX_REPLAY_CLOCK_S).contains(&clock) {
                self.corrupt_packet_clocks.push(last_valid_clock);
                continue;
            }
            last_valid_clock = clock;
            self.headers.push(analysis::PacketHeader { clock, is_client_side: false });
            match PacketKind::from_raw(packet.packet_type) {
                PacketKind::PlayerNetStats => {
                    if let Some(ns) = NetStats::from_payload(packet.payload) {
                        self.samples.push(analysis::NetStat {
                            clock,
                            fps: ns.fps,
                            ping: ns.ping,
                            is_lagging: ns.is_lagging,
                        });
                    }
                }
                PacketKind::ServerTick => self.tick_clocks.push(clock),
                PacketKind::Other => {}
            }
        }
        Ok(())
    }

    fn finish(self, meta: &WotMeta, threshold_ms: Option<u32>) -> analysis::AnalysisResult {
        analysis::build_analysis(
            meta_out(meta),
            self.samples,
            self.tick_clocks,
            self.headers,
            Vec::new(),
            false,
            false,
            None,
            self.corrupt_packet_clocks,
            threshold_ms
                .map(|ms| analysis::SpikeThresholds { min_gap_s: ms as f32 / 1000.0 })
                .unwrap_or_default(),
        )
    }
}

fn meta_out(meta: &WotMeta) -> analysis::ReplayMetaOut {
    let game_type = meta
        .gameplayID
        .clone()
        .or_else(|| meta.battleType.map(|n| format!("battleType {n}")))
        .unwrap_or_default();
    analysis::ReplayMetaOut {
        map: meta.mapName.clone(),
        map_display_name: meta.mapDisplayName.clone(),
        date_time: meta.dateTime.clone(),
        player_name: meta.playerName.clone(),
        player_vehicle: meta.playerVehicle.clone(),
        client_version: meta.clientVersionFromExe.clone(),
        match_group: meta.serverName.clone().unwrap_or_default(),
        game_type,
        battle_duration_s: 0,
        replay_duration_s: 0,
        players_per_team: 0,
        arena_id: None,
        arena_id_hex: None,
        client_build: replay::build_from_client_version(&meta.clientVersionFromExe),
        region: meta.serverName.clone(),
    }
}

struct RawPacket<'a> {
    packet_type: u32,
    clock: f32,
    payload: &'a [u8],
}

/// Walks `[u32 size][u32 type][f32 clock][payload]` records.
struct PacketIterator<'a> {
    remaining: &'a [u8],
}

impl<'a> PacketIterator<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { remaining: data }
    }
}

impl<'a> Iterator for PacketIterator<'a> {
    type Item = Result<RawPacket<'a>, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        if self.remaining.len() < 12 {
            let truncated = std::mem::take(&mut self.remaining);
            return Some(Err(format!("truncated wot packet header: {} bytes", truncated.len())));
        }
        let size = u32::from_le_bytes(self.remaining[0..4].try_into().unwrap()) as usize;
        let packet_type = u32::from_le_bytes(self.remaining[4..8].try_into().unwrap());
        let clock = f32::from_le_bytes(self.remaining[8..12].try_into().unwrap());
        let total = 12 + size;
        if self.remaining.len() < total {
            self.remaining = &[];
            return Some(Err(format!("truncated wot packet body: {size}")));
        }
        let payload = &self.remaining[12..total];
        self.remaining = &self.remaining[total..];
        Some(Ok(RawPacket { packet_type, clock, payload }))
    }
}

struct NetStats {
    fps: u8,
    ping: u16,
    is_lagging: bool,
}

impl NetStats {
    fn from_payload(payload: &[u8]) -> Option<Self> {
        let bytes: [u8; 4] = payload.get(..4)?.try_into().ok()?;
        let packed = u32::from_le_bytes(bytes);
        Some(Self {
            fps: (packed & 0xff) as u8,
            ping: ((packed >> 8) & 0xffff) as u16,
            is_lagging: ((packed >> 24) & 1) != 0,
        })
    }
}
