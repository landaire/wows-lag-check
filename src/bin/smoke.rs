// cargo run --release --bin smoke -- <replay.wowsreplay> [<build-dir>]
//
// <build-dir> is an optional wows-replay-data build directory (e.g.
// G:\wows_builds\15.4.0_12506899). When given, entity defs are loaded from its
// vfs/scripts/ tree, game_params.rkyv and translations/en/.../global.mo are
// loaded for ship/camo names, and the replay is parsed through the full parser.

use std::fs;
use std::path::Path;
use wows_lag_check::run_analysis;

fn main() {
    let mut args = std::env::args().skip(1);
    let replay_path = args.next().expect("usage: smoke <replay> [build-dir]");
    let build_dir = args.next();

    let bytes = fs::read(&replay_path).expect("read replay");
    let data = build_dir
        .as_deref()
        .map(|d| BuildData::load(Path::new(d)))
        .unwrap_or_default();

    let result = run_analysis(&bytes, &data.defs_bundle, &data.game_params, &data.translations)
        .expect("analysis");

    println!("=== {} ({}) ===", result.meta.player_vehicle, result.meta.player_name);
    println!("Map: {}  Mode: {}", result.meta.map_display_name, result.meta.game_type);
    println!("Client: {}", result.meta.client_version);
    println!("Entity defs loaded: {}", result.entity_defs_loaded);
    println!("Game params loaded: {}", result.game_params_loaded);
    match (&result.meta.arena_id, &result.meta.arena_id_hex) {
        (Some(d), Some(h)) => println!("Arena ID: {d} (0x{h})"),
        _ => println!("Arena ID: (not found)"),
    }
    println!("Server:   {}", result.meta.region.as_deref().unwrap_or("(unknown)"));
    println!("Battle start clock: {:.3}s", result.battle_start_clock_s);
    println!("Severity: {:?} ({})", result.severity.severity, result.severity.headline);
    println!();
    println!("PlayerNetStats samples: {}", result.samples_total);
    println!("ServerTick packets:    {}", result.server_ticks_total);
    println!("Replay duration:       {}s", result.replay_duration_s);
    if result.corrupt_packet_clocks.is_empty() {
        println!("Corrupt-clock packets: 0");
    } else {
        let cs: Vec<String> = result.corrupt_packet_clocks.iter().map(|c| format!("{c:.1}s")).collect();
        println!(
            "Corrupt-clock packets: {} (at {})",
            result.corrupt_packet_clocks.len(),
            cs.join(", "),
        );
    }
    println!();
    println!("=== {} spikes (gap >= 500ms) ===", result.spikes.len());
    for s in &result.spikes {
        let bt = (s.gap_start_clock - result.battle_start_clock_s).max(0.0);
        let kind = if s.client_present_during_gap { "server-only" } else { "client+server" };
        let burst = if s.burst_ticks > 1 {
            format!("  burst {} ticks", s.burst_ticks)
        } else {
            String::new()
        };
        println!(
            "  {:7.3}s gap @ replay {:7.3}s (battle {:02}:{:06.3})  peak {}ms  [{}]{}",
            s.gap_seconds,
            s.gap_start_clock,
            (bt as u32) / 60,
            (bt as f64) % 60.0,
            s.peak_ping_ms,
            kind,
            burst,
        );
        for ev in &s.preceding_events {
            let players: Vec<&str> = ev.ships.iter().map(|sh| sh.player.as_str()).collect();
            let effect = ev.death_effect.as_deref().map(|e| format!(" [{e}]")).unwrap_or_default();
            println!(
                "        -{:.2}s ({} ticks)  [{}] {} {}{}",
                s.gap_start_clock - ev.clock,
                ev.tick_offset,
                ev.kind,
                players.join(" / "),
                ev.detail,
                effect,
            );
            for sh in &ev.ships {
                println!(
                    "              {} | entity {} | ship {} | {} | camo {}",
                    sh.player,
                    sh.entity_id,
                    sh.ship_param_id.map(|i| i.to_string()).unwrap_or_else(|| "-".into()),
                    sh.ship_name.as_deref().unwrap_or("-"),
                    sh.camo.as_deref().unwrap_or("-"),
                );
            }
        }
    }
}

/// Per-build inputs loaded from a wows-replay-data build directory.
#[derive(Default)]
struct BuildData {
    defs_bundle: Vec<u8>,
    game_params: Vec<u8>,
    translations: Vec<u8>,
}

impl BuildData {
    fn load(dir: &Path) -> BuildData {
        // Prefer the zstd blobs (what the web client uses); fall back to raw.
        let game_params = fs::read(dir.join("game_params.rkyv.zst"))
            .or_else(|_| fs::read(dir.join("game_params.rkyv")))
            .unwrap_or_default();
        let mo = dir.join("translations/en/LC_MESSAGES");
        let translations = fs::read(mo.join("global.mo.zst"))
            .or_else(|_| fs::read(mo.join("global.mo")))
            .unwrap_or_default();
        BuildData {
            defs_bundle: bundle_entity_defs(dir),
            game_params,
            translations,
        }
    }
}

/// Walk `<build-dir>/vfs/scripts/` and pack every file into the bundle format
/// `unpack_def_bundle` expects, keyed by the path relative to `vfs/`.
fn bundle_entity_defs(build_dir: &Path) -> Vec<u8> {
    let scripts = build_dir.join("vfs").join("scripts");
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_files(&scripts, &build_dir.join("vfs"), &mut files);

    let mut bundle = Vec::new();
    bundle.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (path, content) in files {
        bundle.extend_from_slice(&(path.len() as u32).to_le_bytes());
        bundle.extend_from_slice(path.as_bytes());
        bundle.extend_from_slice(&(content.len() as u32).to_le_bytes());
        bundle.extend_from_slice(&content);
    }
    bundle
}

fn collect_files(dir: &Path, vfs_root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        // fs::metadata follows symlinks (the dump uses CAS symlinks).
        let Ok(meta) = fs::metadata(&path) else { continue };
        if meta.is_dir() {
            collect_files(&path, vfs_root, out);
        } else if let Ok(content) = fs::read(&path) {
            let key = path.strip_prefix(vfs_root).unwrap().to_string_lossy().replace('\\', "/");
            out.push((key, content));
        }
    }
}
