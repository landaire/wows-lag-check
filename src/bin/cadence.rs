// Characterize the cadence of client-side packets (Camera, GunMarker,
// PlayerNetStats) in WoWs replays so we can decide whether absence-of-these
// packets is a reliable signal for client stalls.
//
// usage: cargo run --release --bin cadence -- <replay> [<replay> ...]

use std::fs;
use wows_replays::ReplayFile;
use wows_replays::packet2::PacketTypeId;
use wows_replays::packet2::RawPacketIterator;

const MAX_REPLAY_CLOCK_S: f32 = 86_400.0;

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: cadence <replay> [<replay> ...]");
        std::process::exit(2);
    }
    for path in &paths {
        match analyze(path) {
            Ok(()) => {}
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
}

const TRACKED: &[(PacketTypeId, &str)] = &[
    (PacketTypeId::Camera, "Camera"),
    (PacketTypeId::GunMarker, "GunMarker"),
    (PacketTypeId::PlayerNetStats, "PlayerNetStats"),
    (PacketTypeId::ServerTick, "ServerTick"),
];

fn analyze(path: &str) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|e| format!("read: {e}"))?;
    let replay = ReplayFile::from_bytes(&bytes).map_err(|e| format!("parse: {e:?}"))?;
    let mut clocks: Vec<Vec<f32>> = vec![Vec::new(); TRACKED.len()];

    for packet in RawPacketIterator::new(&replay.packet_data) {
        let packet = packet.map_err(|e| format!("packet: {e:?}"))?;
        let clock = packet.clock.seconds();
        if !(0.0..MAX_REPLAY_CLOCK_S).contains(&clock) {
            continue;
        }
        if let Some(idx) = TRACKED.iter().position(|(t, _)| *t == packet.packet_type) {
            clocks[idx].push(clock);
        }
    }

    let duration = clocks
        .iter()
        .flat_map(|c| c.iter().copied())
        .fold(0.0_f32, f32::max);

    println!("=== {} ({}) ===", path, replay.meta.clientVersionFromExe);
    println!("replay duration: {duration:.2}s");
    println!(
        "{:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "packet", "count", "rate/s", "mean_dt", "p50_dt", "p95_dt", "max_dt"
    );
    for (i, (_, name)) in TRACKED.iter().enumerate() {
        report_one(name, &clocks[i], duration);
    }
    println!();

    // Client-side stall candidates: gaps in any client-side packet stream while
    // ServerTick continued at <0.5s spacing on both sides.
    let mut combined: Vec<f32> = clocks[0]
        .iter()
        .chain(clocks[1].iter())
        .chain(clocks[2].iter())
        .copied()
        .collect();
    combined.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let server = &clocks[3];

    println!("=== client-side gaps (>= 0.5s) with server alive ===");
    let mut any = false;
    for w in combined.windows(2) {
        let gap = w[1] - w[0];
        if gap < 0.5 {
            continue;
        }
        // Count server ticks inside the gap.
        let ticks_in_gap = server.iter().filter(|&&t| t > w[0] && t < w[1]).count();
        let server_rate = if gap > 0.0 {
            ticks_in_gap as f32 / gap
        } else {
            0.0
        };
        let server_alive = server_rate > 3.0;
        let tag = if server_alive { "CLIENT-ONLY" } else { "both" };
        println!(
            "  {:7.3}s gap @ {:7.3}s  server ticks in gap: {:3} ({:.1}/s)  [{}]",
            gap, w[0], ticks_in_gap, server_rate, tag,
        );
        any = true;
    }
    if !any {
        println!("  (none)");
    }
    println!();
    Ok(())
}

fn report_one(name: &str, clocks: &[f32], duration: f32) {
    if clocks.len() < 2 {
        println!("{name:<16} {:>8} (insufficient samples)", clocks.len());
        return;
    }
    let mut dts: Vec<f32> = clocks.windows(2).map(|w| w[1] - w[0]).collect();
    dts.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = dts.iter().sum::<f32>() / dts.len() as f32;
    let p50 = dts[dts.len() / 2];
    let p95 = dts[(dts.len() as f32 * 0.95) as usize];
    let max = *dts.last().unwrap();
    let rate = clocks.len() as f32 / duration.max(1.0);
    println!(
        "{name:<16} {:>8} {:>8.2} {:>8.4} {:>8.4} {:>8.4} {:>8.4}",
        clocks.len(),
        rate,
        mean,
        p50,
        p95,
        max
    );
}
