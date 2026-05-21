pub mod analysis;
pub mod replay;

use wasm_bindgen::prelude::*;
use wows_replays::ReplayFile;
use wows_replays::packet2::{PacketTypeId, PlayerNetStatsPacket, RawPacketIterator};

#[wasm_bindgen(start)]
pub fn _start() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

#[wasm_bindgen(js_name = analyzeReplay)]
pub fn analyze_replay(bytes: &[u8]) -> Result<JsValue, JsError> {
    let result = run_analysis(bytes).map_err(|e| JsError::new(&e))?;
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

/// Parse a replay and run the spike analysis. Shared by the WASM entry point
/// and the native smoke-test binary.
pub fn run_analysis(bytes: &[u8]) -> Result<analysis::AnalysisResult, String> {
    let replay = ReplayFile::from_bytes(bytes).map_err(|e| format!("replay parse: {e}"))?;

    let build = replay::build_from_client_version(&replay.meta.clientVersionFromExe);
    let arena_method_id = build.and_then(replay::arena_state_method_id);
    let region = replay::detect_realm(&replay.packet_data);

    let mut samples = Vec::new();
    let mut server_ticks = Vec::new();
    let mut headers = Vec::new();
    let mut arena_id: Option<i64> = None;

    for packet in RawPacketIterator::new(&replay.packet_data) {
        let packet = packet.map_err(|e| format!("packet parse: {e:?}"))?;
        let clock = packet.clock.seconds();
        headers.push(analysis::PacketHeader { clock, ptype: packet.packet_type });

        match packet.packet_type {
            PacketTypeId::PlayerNetStats => {
                if let Some(ns) = PlayerNetStatsPacket::from_payload(packet.payload) {
                    samples.push(analysis::NetStat {
                        clock,
                        fps: ns.fps,
                        ping: ns.ping,
                        is_lagging: ns.is_lagging,
                    });
                }
            }
            PacketTypeId::ServerTick => server_ticks.push(clock),
            // onArenaStateReceived is an EntityMethod at clock=0. We can't
            // decode it without entity specs, but the raw body carries the
            // arena_id at a fixed offset; the method_id table confirms it.
            PacketTypeId::EntityMethod if arena_id.is_none() && clock == 0.0 => {
                if let Some(mid) = arena_method_id {
                    arena_id = replay::arena_id_from_packet_body(packet.payload, mid);
                }
            }
            _ => {}
        }
    }

    Ok(analysis::build_analysis(
        replay.meta,
        samples,
        server_ticks,
        headers,
        arena_id,
        build,
        region,
        analysis::SpikeThresholds::default(),
    ))
}
