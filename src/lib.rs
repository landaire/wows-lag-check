pub mod analysis;
pub mod replay;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn _start() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

#[wasm_bindgen(js_name = analyzeReplay)]
pub fn analyze_replay(bytes: &[u8]) -> Result<JsValue, JsError> {
    let decrypted =
        replay::parse_replay_bytes(bytes).map_err(|e| JsError::new(&format!("replay parse: {e}")))?;

    let mut samples = Vec::new();
    let mut server_ticks = Vec::new();
    let mut headers = Vec::new();
    let mut map_info: Option<replay::MapInfo> = None;

    replay::walk_packets(
        &decrypted.packet_data,
        |clock, ptype| headers.push(analysis::PacketHeader { clock, ptype }),
        |s| samples.push(s),
        |c| server_ticks.push(c),
        |m| {
            if map_info.is_none() {
                map_info = Some(m);
            }
        },
    )
    .map_err(|e| JsError::new(&format!("packet walk: {e}")))?;

    let region = replay::detect_realm(&decrypted.packet_data);

    let result = analysis::build_analysis(
        decrypted.meta,
        samples,
        server_ticks,
        headers,
        map_info,
        region,
        analysis::SpikeThresholds::default(),
    );

    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&format!("serialize: {e}")))
}
