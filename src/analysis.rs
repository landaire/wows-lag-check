use serde::Serialize;
use wows_replays::packet2::PacketTypeId;

/// One PlayerNetStats observation, with the clock pulled from the packet header.
#[derive(Debug, Clone, Copy)]
pub struct NetStat {
    pub clock: f32,
    pub fps: u8,
    pub ping: u16,
    pub is_lagging: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PingSample {
    pub clock: f32,
    pub ping_ms: u16,
    pub fps: u8,
    pub is_lagging: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Spike {
    pub gap_start_clock: f32,
    pub gap_end_clock: f32,
    pub gap_seconds: f32,
    pub peak_ping_ms: u16,
    pub peak_ping_clock: f32,
    pub client_packets_in_gap: u32,
    pub client_rate_hz: f32,
    pub client_present_during_gap: bool,
    /// ServerTick packets stamped with the gap-start clock. A value above 1
    /// means the server fired repeated ticks without advancing the game clock
    /// just before freezing: a stutter burst.
    pub burst_ticks: u32,
    /// In-game events in the 2 s window before the spike started.
    pub preceding_events: Vec<GameEvent>,
}

/// An in-game event used to give context to a spike. `kind` is one of
/// "consumable", "kill", "spotted"; `text` is a human-readable summary.
#[derive(Debug, Clone, Serialize)]
pub struct GameEvent {
    pub clock: f32,
    /// Distinct server-tick instants between this event and the spike it
    /// precedes (negative = before the spike). Filled in when the event is
    /// attached to a spike.
    pub tick_offset: i32,
    pub kind: String,
    pub text: String,
    /// Entity ID of the ship the event concerns (spotted ship / kill victim).
    pub entity_id: Option<u32>,
    /// Numeric ship GameParamId of that ship, when known from the arena state.
    /// The textual index (e.g. "PNSC010") needs GameParams and is not resolved.
    pub ship_param_id: Option<u64>,
}

/// How far before a spike to collect context events.
pub const EVENT_WINDOW_S: f32 = 2.0;

#[derive(Debug, Clone, Serialize)]
pub struct ReplayMetaOut {
    pub map: String,
    pub map_display_name: String,
    pub date_time: String,
    pub player_name: String,
    pub player_vehicle: String,
    pub client_version: String,
    pub match_group: String,
    pub game_type: String,
    pub battle_duration_s: u32,
    pub replay_duration_s: u32,
    pub players_per_team: u32,
    pub arena_id: Option<String>,
    pub arena_id_hex: Option<String>,
    pub client_build: Option<u32>,
    /// Server/region extracted from the receivePlayerData pickle blob. None
    /// when no recognisable realm string was found.
    pub region: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PingStats {
    pub min_ms: u16,
    pub max_ms: u16,
    pub mean_ms: f32,
    pub p95_ms: u16,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Clean,
    Minor,
    Moderate,
    Severe,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeveritySummary {
    pub severity: Severity,
    pub spike_count: u32,
    pub total_stalled_s: f32,
    pub worst_gap_s: f32,
    pub worst_gap_battle_s: f32,
    pub headline: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisResult {
    pub meta: ReplayMetaOut,
    /// Replay-clock seconds at which the battle timer reads 20:00. 30s pre-battle
    /// countdown for Random/Co-op/Ranked/Brawl; Operations differs.
    pub battle_start_clock_approx_s: f32,
    pub samples: Vec<PingSample>,
    pub server_tick_clocks: Vec<f32>,
    pub spikes: Vec<Spike>,
    pub ping_stats: PingStats,
    pub samples_total: u32,
    pub server_ticks_total: u32,
    pub replay_duration_s: f32,
    pub severity: SeveritySummary,
    /// True when entity definitions were loaded and the replay was parsed
    /// through the full parser (arena ID available).
    pub entity_defs_loaded: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SpikeThresholds {
    pub min_gap_s: f32,
}

impl Default for SpikeThresholds {
    fn default() -> Self {
        Self { min_gap_s: 0.5 }
    }
}

pub struct PacketHeader {
    pub clock: f32,
    pub ptype: PacketTypeId,
}

/// Camera, GunMarker, and PlayerNetStats are each emitted on a 10 Hz client
/// timer (~30 Hz combined), independent of the 7 Hz server tick.
pub fn is_client_side_packet(ptype: PacketTypeId) -> bool {
    matches!(
        ptype,
        PacketTypeId::Camera | PacketTypeId::GunMarker | PacketTypeId::PlayerNetStats
    )
}

pub fn build_analysis(
    meta: wows_replays::ReplayMeta,
    samples: Vec<NetStat>,
    server_ticks: Vec<f32>,
    headers: Vec<PacketHeader>,
    mut events: Vec<GameEvent>,
    arena_id: Option<i64>,
    client_build: Option<u32>,
    region: Option<&'static str>,
    entity_defs_loaded: bool,
    thresholds: SpikeThresholds,
) -> AnalysisResult {
    let samples_out: Vec<PingSample> = samples
        .iter()
        .map(|s| PingSample {
            clock: s.clock,
            ping_ms: s.ping,
            fps: s.fps,
            is_lagging: s.is_lagging,
        })
        .collect();

    let ping_stats = compute_ping_stats(&samples);
    events.sort_by(|a, b| a.clock.partial_cmp(&b.clock).unwrap_or(std::cmp::Ordering::Equal));
    let mut spikes = detect_spikes(&server_ticks, &samples, &headers, thresholds);
    // The server can emit a burst of ticks all stamped with the same clock
    // (a stutter just before a freeze). Collapse those so a tick offset
    // counts distinct game-clock instants, not raw packets.
    let mut distinct_ticks = server_ticks.clone();
    distinct_ticks.dedup();

    for spike in &mut spikes {
        let lo = spike.gap_start_clock - EVENT_WINDOW_S;
        let hi = spike.gap_start_clock;
        // distinct_ticks is monotonic, so partition_point counts ticks at or
        // before a clock. The offset is event-tick-count minus spike-tick-count.
        let gap_ticks = distinct_ticks.partition_point(|&t| t <= spike.gap_start_clock) as i32;
        spike.preceding_events = events
            .iter()
            .filter(|e| e.clock >= lo && e.clock <= hi)
            .map(|e| {
                let mut e = e.clone();
                let event_ticks = distinct_ticks.partition_point(|&t| t <= e.clock) as i32;
                e.tick_offset = event_ticks - gap_ticks;
                e
            })
            .collect();
    }

    let replay_duration_s = headers
        .iter()
        .map(|h| h.clock)
        .fold(0.0_f32, f32::max);

    let battle_start_clock_approx_s = 30.94_f32;
    let severity = classify_severity(&spikes, battle_start_clock_approx_s);

    AnalysisResult {
        meta: ReplayMetaOut {
            map: meta.mapName,
            map_display_name: meta.mapDisplayName,
            date_time: meta.dateTime,
            player_name: meta.playerName,
            player_vehicle: meta.playerVehicle,
            client_version: meta.clientVersionFromExe,
            match_group: meta.matchGroup,
            game_type: meta.gameType,
            battle_duration_s: meta.battleDuration,
            replay_duration_s: meta.duration,
            players_per_team: meta.playersPerTeam,
            arena_id: arena_id.map(|a| a.to_string()),
            arena_id_hex: arena_id.map(|a| format!("{:016x}", a as u64)),
            client_build,
            region: region.map(|r| r.to_string()),
        },
        battle_start_clock_approx_s,
        samples: samples_out,
        server_tick_clocks: server_ticks.clone(),
        spikes,
        ping_stats,
        samples_total: samples.len() as u32,
        server_ticks_total: server_ticks.len() as u32,
        replay_duration_s,
        severity,
        entity_defs_loaded,
    }
}

fn classify_severity(spikes: &[Spike], battle_start: f32) -> SeveritySummary {
    let spike_count = spikes.len() as u32;
    let total_stalled_s: f32 = spikes.iter().map(|s| s.gap_seconds).sum();
    let worst = spikes
        .iter()
        .max_by(|a, b| a.gap_seconds.partial_cmp(&b.gap_seconds).unwrap_or(std::cmp::Ordering::Equal));
    let worst_gap_s = worst.map(|s| s.gap_seconds).unwrap_or(0.0);
    let worst_gap_battle_s = worst.map(|s| s.gap_start_clock - battle_start).unwrap_or(0.0);

    let severity = if spike_count == 0 {
        Severity::Clean
    } else if worst_gap_s >= 5.0 || total_stalled_s >= 10.0 || spike_count >= 8 {
        Severity::Severe
    } else if worst_gap_s >= 2.0 || total_stalled_s >= 4.0 || spike_count >= 4 {
        Severity::Moderate
    } else {
        Severity::Minor
    };

    let headline = match severity {
        Severity::Clean => "No server stalls detected.".to_string(),
        _ => format!(
            "{} spike{}, {:.1}s total stalled, worst {:.1}s",
            spike_count,
            if spike_count == 1 { "" } else { "s" },
            total_stalled_s,
            worst_gap_s
        ),
    };

    SeveritySummary { severity, spike_count, total_stalled_s, worst_gap_s, worst_gap_battle_s, headline }
}

fn compute_ping_stats(samples: &[NetStat]) -> PingStats {
    if samples.is_empty() {
        return PingStats { min_ms: 0, max_ms: 0, mean_ms: 0.0, p95_ms: 0 };
    }
    let mut pings: Vec<u16> = samples.iter().map(|s| s.ping).collect();
    pings.sort_unstable();
    let min_ms = *pings.first().unwrap();
    let max_ms = *pings.last().unwrap();
    let sum: u64 = pings.iter().map(|&p| p as u64).sum();
    let mean_ms = (sum as f64 / pings.len() as f64) as f32;
    let idx = ((pings.len() as f64) * 0.95).floor() as usize;
    let p95_ms = pings[idx.min(pings.len() - 1)];
    PingStats { min_ms, max_ms, mean_ms, p95_ms }
}

fn detect_spikes(
    server_ticks: &[f32],
    samples: &[NetStat],
    headers: &[PacketHeader],
    thresholds: SpikeThresholds,
) -> Vec<Spike> {
    let mut spikes = Vec::new();
    for (idx, window) in server_ticks.windows(2).enumerate() {
        let prev = window[0];
        let cur = window[1];
        let gap = cur - prev;
        if gap < thresholds.min_gap_s {
            continue;
        }

        // server_ticks is sorted, so ticks sharing the gap-start clock form a
        // run ending at idx. Counting it backwards gives the burst size.
        let burst_ticks = server_ticks[..=idx]
            .iter()
            .rev()
            .take_while(|&&t| t == prev)
            .count() as u32;

        let client_packets_in_gap = headers
            .iter()
            .filter(|h| h.clock > prev && h.clock < cur && is_client_side_packet(h.ptype))
            .count() as u32;

        let (peak_ping_ms, peak_ping_clock) = samples
            .iter()
            .filter(|s| s.clock >= prev && s.clock <= (cur + 2.0))
            .map(|s| (s.ping, s.clock))
            .max_by_key(|(p, _)| *p)
            .unwrap_or((0, prev));

        let client_rate_hz = if gap > 0.0 { client_packets_in_gap as f32 / gap } else { 0.0 };
        let client_present_during_gap = client_rate_hz > 5.0;

        spikes.push(Spike {
            gap_start_clock: prev,
            gap_end_clock: cur,
            gap_seconds: gap,
            peak_ping_ms,
            peak_ping_clock,
            client_packets_in_gap,
            client_rate_hz,
            client_present_during_gap,
            burst_ticks,
            preceding_events: Vec::new(),
        });
    }
    spikes
}
