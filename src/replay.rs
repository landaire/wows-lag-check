//! Minimal WoWs replay parser. Format reference: wows-replays crate
//! (https://github.com/landaire/wows-toolkit).
//!
//! Packet wire format is `[size:u32, type:u32, clock:f32, body:size]`. We
//! decode PlayerNetStats (0x1d), ServerTick (0x0e), and Map (0x28) bodies;
//! everything else gets skipped by `size` without needing entity defs.

use blowfish::Blowfish;
use byteorder::BE;
use cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
use flate2::read::ZlibDecoder;
use serde::Deserialize;
use std::io::Read;
use thiserror::Error;

const REPLAY_BLOWFISH_KEY: [u8; 16] = [
    0x29, 0xB7, 0xC9, 0x09, 0x38, 0x3F, 0x84, 0x88, 0xFA, 0x98, 0xEC, 0x4E, 0x13, 0x19, 0x79, 0xFB,
];

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("file too short")]
    TooShort,
    #[error("metadata JSON parse error: {0}")]
    MetaJson(#[from] serde_json::Error),
    #[error("metadata is not valid UTF-8")]
    MetaUtf8,
    #[error("replay payload is not a multiple of 8 bytes (blowfish block size)")]
    BadBlockAlignment,
    #[error("zlib decompression failed: {0}")]
    Inflate(#[from] std::io::Error),
    #[error("packet stream truncated at offset {0}")]
    Truncated(usize),
}

#[allow(non_snake_case)]
#[derive(Clone, Debug, Deserialize)]
pub struct VehicleInfo {
    pub shipId: i64,
    pub relation: u32,
    pub id: i64,
    pub name: String,
}

#[allow(non_snake_case)]
#[derive(Clone, Debug, Deserialize)]
pub struct ReplayMeta {
    pub clientVersionFromExe: String,
    pub mapDisplayName: String,
    pub mapName: String,
    pub dateTime: String,
    pub playerName: String,
    pub playerVehicle: String,
    pub matchGroup: String,
    pub gameType: String,
    pub battleDuration: u32,
    pub duration: u32,
    pub playersPerTeam: u32,
    #[serde(default)]
    pub vehicles: Vec<VehicleInfo>,
}

pub struct DecryptedReplay {
    pub meta: ReplayMeta,
    pub packet_data: Vec<u8>,
}

pub fn parse_replay_bytes(bytes: &[u8]) -> Result<DecryptedReplay, ReplayError> {
    let mut cur = 0usize;

    let read_u32 = |b: &[u8], at: usize| -> Result<u32, ReplayError> {
        b.get(at..at + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
            .ok_or(ReplayError::TooShort)
    };

    let _magic = read_u32(bytes, cur)?;
    cur += 4;
    let block_count = read_u32(bytes, cur)? as usize;
    cur += 4;

    let meta_len = read_u32(bytes, cur)? as usize;
    cur += 4;
    let meta_raw = bytes.get(cur..cur + meta_len).ok_or(ReplayError::TooShort)?;
    cur += meta_len;
    let meta_str = std::str::from_utf8(meta_raw).map_err(|_| ReplayError::MetaUtf8)?;
    let meta: ReplayMeta = serde_json::from_str(meta_str)?;

    for _ in 0..(block_count.saturating_sub(1)) {
        let block_size = read_u32(bytes, cur)? as usize;
        cur += 4 + block_size;
        if cur > bytes.len() {
            return Err(ReplayError::TooShort);
        }
    }

    let _decompressed_size = read_u32(bytes, cur)?;
    cur += 4;
    let _compressed_size = read_u32(bytes, cur)?;
    cur += 4;

    let encrypted = bytes.get(cur..).ok_or(ReplayError::TooShort)?;
    if encrypted.len() % 8 != 0 {
        return Err(ReplayError::BadBlockAlignment);
    }

    let cipher = <Blowfish<BE>>::new_from_slice(&REPLAY_BLOWFISH_KEY)
        .expect("16-byte key is valid for Blowfish");

    let mut decrypted = vec![0u8; encrypted.len()];
    let mut previous = [0u8; 8];
    for chunk_idx in 0..(encrypted.len() / 8) {
        let off = chunk_idx * 8;
        let mut block = GenericArray::clone_from_slice(&encrypted[off..off + 8]);
        cipher.decrypt_block(&mut block);
        for (j, b) in block.iter().enumerate() {
            decrypted[off + j] = *b ^ previous[j];
        }
        previous.copy_from_slice(&decrypted[off..off + 8]);
    }

    let mut deflater = ZlibDecoder::new(decrypted.as_slice());
    let mut packet_data = Vec::new();
    deflater.read_to_end(&mut packet_data)?;

    Ok(DecryptedReplay { meta, packet_data })
}

/// One observation from a `PlayerNetStats` (0x1d) packet.
#[derive(Debug, Clone, Copy)]
pub struct NetStat {
    pub clock: f32,
    pub fps: u8,
    pub ping: u16,
    pub is_lagging: bool,
}

/// Scans the decrypted packet bytes for pickle SHORT_BINSTRING tokens matching
/// known WoWs realm codes. The receivePlayerData blob stores each player's
/// realm as `U\x{len}{ascii}`, with the key name memoized once so it doesn't
/// recur. The most-common matching realm is the match's server.
pub fn detect_realm(packet_data: &[u8]) -> Option<&'static str> {
    let candidates: &[(&[u8], &str)] = &[
        (b"\x55\x02EU", "EU"),
        (b"\x55\x02NA", "NA"),
        (b"\x55\x04ASIA", "ASIA"),
        (b"\x55\x02RU", "RU"),
        (b"\x55\x02SG", "SG"),
        (b"\x55\x03CIS", "CIS"),
    ];
    let mut best: Option<(&'static str, usize)> = None;
    for (pat, name) in candidates {
        let mut count = 0usize;
        let mut i = 0;
        while let Some(off) = memmem(&packet_data[i..], pat) {
            count += 1;
            i += off + 1;
        }
        if count >= 3 && best.map(|(_, c)| count > c).unwrap_or(true) {
            best = Some((name, count));
        }
    }
    best.map(|(n, _)| n)
}

fn memmem(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub fn walk_packets<HV, NV, TV, MV>(
    packet_data: &[u8],
    mut visit_header: HV,
    mut visit_netstat: NV,
    mut visit_servertick: TV,
    mut visit_map: MV,
) -> Result<(), ReplayError>
where
    HV: FnMut(f32, u32),
    NV: FnMut(NetStat),
    TV: FnMut(f32),
    MV: FnMut(MapInfo),
{
    let mut offset = 0usize;
    while offset < packet_data.len() {
        if offset + 12 > packet_data.len() {
            return Err(ReplayError::Truncated(offset));
        }
        let size = u32::from_le_bytes(packet_data[offset..offset + 4].try_into().unwrap()) as usize;
        let ptype = u32::from_le_bytes(packet_data[offset + 4..offset + 8].try_into().unwrap());
        let clock = f32::from_le_bytes(packet_data[offset + 8..offset + 12].try_into().unwrap());
        let body_start = offset + 12;
        let body_end = body_start + size;
        if body_end > packet_data.len() {
            return Err(ReplayError::Truncated(offset));
        }
        let body = &packet_data[body_start..body_end];

        visit_header(clock, ptype);

        match ptype {
            0x1d if body.len() >= 4 => {
                let packed = u32::from_le_bytes(body[..4].try_into().unwrap());
                let fps = (packed & 0xff) as u8;
                let ping = ((packed >> 8) & 0xffff) as u16;
                let is_lagging = ((packed >> 24) & 1) != 0;
                visit_netstat(NetStat { clock, fps, ping, is_lagging });
            }
            0x0e if body.len() >= 8 => {
                let _tick_rate = f64::from_le_bytes(body[..8].try_into().unwrap());
                visit_servertick(clock);
            }
            0x28 if body.len() >= 12 => {
                let space_id = u32::from_le_bytes(body[0..4].try_into().unwrap());
                let arena_id = i64::from_le_bytes(body[4..12].try_into().unwrap());
                visit_map(MapInfo { space_id, arena_id });
            }
            _ => {}
        }

        offset = body_end;
    }
    Ok(())
}

/// `arena_id` is the server-side match identifier.
#[derive(Debug, Clone, Copy)]
pub struct MapInfo {
    pub space_id: u32,
    pub arena_id: i64,
}
