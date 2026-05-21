// cargo run --release --bin smoke -- <replay.wowsreplay>

use std::fs;
use wows_lag_check::{analysis, replay};

fn main() {
    let path = std::env::args().nth(1).expect("usage: smoke <replay>");
    let bytes = fs::read(&path).expect("read replay");

    let decrypted = replay::parse_replay_bytes(&bytes).expect("parse");

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
    .expect("walk packets");

    let region = replay::detect_realm(&decrypted.packet_data);
    let arena_id = replay::build_from_client_version(&decrypted.meta.clientVersionFromExe)
        .and_then(|build| replay::detect_arena_id(&decrypted.packet_data, build));

    let result = analysis::build_analysis(
        decrypted.meta,
        samples,
        server_ticks,
        headers,
        map_info,
        arena_id,
        region,
        analysis::SpikeThresholds::default(),
    );

    println!("=== {} ({}) ===", result.meta.player_vehicle, result.meta.player_name);
    println!("Map: {}  Mode: {}", result.meta.map_display_name, result.meta.game_type);
    println!("Client: {}", result.meta.client_version);
    match (&result.meta.arena_id, &result.meta.arena_id_hex) {
        (Some(d), Some(h)) => println!("Arena ID: {d} (0x{h})"),
        _ => println!("Arena ID: (not found)"),
    }
    println!("Server:   {}", result.meta.region.as_deref().unwrap_or("(unknown)"));
    println!("Severity: {:?} ({})", result.severity.severity, result.severity.headline);
    println!(
        "Replay duration: {:.1}s  Battle duration: {}s",
        result.replay_duration_s, result.meta.battle_duration_s
    );
    println!();
    println!("PlayerNetStats samples: {}", result.samples_total);
    println!("ServerTick packets:    {}", result.server_ticks_total);
    println!(
        "Ping  min={}  max={}  mean={:.1}  p95={}",
        result.ping_stats.min_ms,
        result.ping_stats.max_ms,
        result.ping_stats.mean_ms,
        result.ping_stats.p95_ms
    );
    println!();
    println!("=== {} spikes (gap >= 500ms) ===", result.spikes.len());
    for s in &result.spikes {
        let bt_start = (s.gap_start_clock - result.battle_start_clock_approx_s).max(0.0);
        let kind = if s.client_present_during_gap { "server-only" } else { "client+server" };
        println!(
            "  {:7.3}s gap @ replay {:7.3}s (battle {:02}:{:06.3})  peak {}ms  [{}, {} client pkts in gap]",
            s.gap_seconds,
            s.gap_start_clock,
            (bt_start as u32) / 60,
            (bt_start as f64) % 60.0,
            s.peak_ping_ms,
            kind,
            s.client_packets_in_gap,
        );
    }
}
