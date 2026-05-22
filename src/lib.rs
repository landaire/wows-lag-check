pub mod analysis;
pub mod replay;
pub mod wot;
pub mod wows;

use wasm_bindgen::prelude::*;

/// Packets with a clock outside this range are dropped as corrupt.
pub(crate) const MAX_REPLAY_CLOCK_S: f32 = 86_400.0;

#[wasm_bindgen(start)]
pub fn _start() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

#[derive(serde::Serialize)]
struct ReplayInfoOut {
    game: &'static str,
    client_version: String,
    build: Option<u32>,
    dir_name: Option<String>,
}

#[wasm_bindgen(js_name = replayInfo)]
pub fn replay_info(bytes: &[u8]) -> Result<JsValue, JsError> {
    let out = if wot::looks_like_wot(bytes) {
        let info = wot::replay_info(bytes).map_err(|e| JsError::new(&e))?;
        ReplayInfoOut { game: "wot", client_version: info.client_version, build: None, dir_name: None }
    } else {
        let replay = wows_replays::ReplayFile::from_bytes(bytes)
            .map_err(|e| JsError::new(&format!("replay parse: {e}")))?;
        let info = wows::replay_info_from_meta(&replay.meta);
        ReplayInfoOut {
            game: "wows",
            client_version: info.client_version,
            build: info.build,
            dir_name: info.dir_name,
        }
    };
    serde_wasm_bindgen::to_value(&out).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

#[wasm_bindgen(js_name = analyzeReplay)]
pub fn analyze_replay(
    bytes: &[u8],
    defs_bundle: &[u8],
    game_params: &[u8],
    translations: &[u8],
    threshold_ms: Option<u32>,
) -> Result<JsValue, JsError> {
    let result = run_analysis(bytes, defs_bundle, game_params, translations, threshold_ms)
        .map_err(|e| JsError::new(&e))?;
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

/// WoT replays use the minimal path; the three game-data args are ignored.
pub fn run_analysis(
    bytes: &[u8],
    defs_bundle: &[u8],
    game_params: &[u8],
    translations: &[u8],
    threshold_ms: Option<u32>,
) -> Result<analysis::AnalysisResult, String> {
    if wot::looks_like_wot(bytes) {
        wot::analyze(bytes, threshold_ms)
    } else {
        wows::analyze(bytes, defs_bundle, game_params, translations, threshold_ms)
    }
}
