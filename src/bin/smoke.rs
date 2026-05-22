// cargo run --release --bin smoke -- <replay.wowsreplay> [<build-dir>]
//
// <build-dir> is an optional wows-replay-data build directory (e.g.
// G:\wows_builds\15.4.0_12506899). When given, entity defs are loaded from its
// vfs/scripts/ tree and the replay is parsed through the full parser.

use std::fs;
use std::path::Path;
use wows_lag_check::run_analysis;

fn main() {
    let mut args = std::env::args().skip(1);
    let replay_path = args.next().expect("usage: smoke <replay> [build-dir]");
    let build_dir = args.next();

    let bytes = fs::read(&replay_path).expect("read replay");
    let defs_bundle = build_dir
        .as_deref()
        .map(|d| bundle_entity_defs(Path::new(d)))
        .unwrap_or_default();

    let result = run_analysis(&bytes, &defs_bundle).expect("analysis");

    println!("=== {} ({}) ===", result.meta.player_vehicle, result.meta.player_name);
    println!("Map: {}  Mode: {}", result.meta.map_display_name, result.meta.game_type);
    println!("Client: {}", result.meta.client_version);
    println!("Entity defs loaded: {}", result.entity_defs_loaded);
    match (&result.meta.arena_id, &result.meta.arena_id_hex) {
        (Some(d), Some(h)) => println!("Arena ID: {d} (0x{h})"),
        _ => println!("Arena ID: (not found)"),
    }
    println!("Server:   {}", result.meta.region.as_deref().unwrap_or("(unknown)"));
    println!("Severity: {:?} ({})", result.severity.severity, result.severity.headline);
    println!();
    println!("PlayerNetStats samples: {}", result.samples_total);
    println!("ServerTick packets:    {}", result.server_ticks_total);
    println!();
    println!("=== {} spikes (gap >= 500ms) ===", result.spikes.len());
    for s in &result.spikes {
        let bt = (s.gap_start_clock - result.battle_start_clock_approx_s).max(0.0);
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
            let mut detail = String::new();
            if let Some(eid) = ev.entity_id {
                detail.push_str(&format!("  entity {eid}"));
            }
            if let Some(sid) = ev.ship_param_id {
                detail.push_str(&format!("  ship {sid}"));
            }
            println!(
                "        -{:.2}s ({} ticks)  [{}] {}{}",
                s.gap_start_clock - ev.clock,
                ev.tick_offset,
                ev.kind,
                ev.text,
                detail,
            );
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
