use serde::Serialize;

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
    /// >1 means the server fired repeated ticks at the same clock before freezing.
    pub burst_ticks: u32,
    pub seconds_since_previous_spike: Option<f32>,
    pub preceding_events: Vec<GameEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventShip {
    pub entity_id: u32,
    pub player: String,
    pub ship_name: Option<String>,
    pub ship_param_id: Option<u64>,
    pub camo: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Consumable,
    Kill,
    Spotted,
}

/// `ships` is by-position (Spotted: [ship]; Kill: [victim, killer]; Consumable: [user]).
#[derive(Debug, Clone, Serialize)]
pub struct GameEvent {
    pub clock: f32,
    pub tick_offset: i32,
    pub kind: EventKind,
    pub ships: Vec<EventShip>,
    pub detail: String,
    pub death_effect: Option<String>,
}

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
    pub battle_start_clock_s: f32,
    /// True when battle_start_clock_s came from BattleLogic.battleStage; false
    /// when it's the 30.94s fallback.
    pub battle_start_clock_exact: bool,
    pub samples: Vec<PingSample>,
    pub server_tick_clocks: Vec<f32>,
    pub spikes: Vec<Spike>,
    pub ping_stats: PingStats,
    pub samples_total: u32,
    pub server_ticks_total: u32,
    pub replay_duration_s: f32,
    pub spike_threshold_ms: u32,
    pub severity: SeveritySummary,
    pub entity_defs_loaded: bool,
    pub game_params_loaded: bool,
    pub corrupt_packet_clocks: Vec<f32>,
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

/// `is_client_side` flags packets that flow client-to-server independently of
/// the server tick (WoWs: Camera, GunMarker, PlayerNetStats).
pub struct PacketHeader {
    pub clock: f32,
    pub is_client_side: bool,
}

pub fn build_analysis(
    meta: ReplayMetaOut,
    samples: Vec<NetStat>,
    server_ticks: Vec<f32>,
    headers: Vec<PacketHeader>,
    mut events: Vec<GameEvent>,
    entity_defs_loaded: bool,
    game_params_loaded: bool,
    battle_start_clock: Option<f32>,
    corrupt_packet_clocks: Vec<f32>,
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
    events.sort_by(|a, b| {
        a.clock
            .partial_cmp(&b.clock)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut spikes = detect_spikes(&server_ticks, &samples, &headers, thresholds);
    let mut prev_end: Option<f32> = None;
    for spike in &mut spikes {
        spike.seconds_since_previous_spike =
            prev_end.map(|end| (spike.gap_start_clock - end).max(0.0));
        prev_end = Some(spike.gap_end_clock);
    }
    let mut distinct_ticks = server_ticks.clone();
    distinct_ticks.dedup();

    for spike in &mut spikes {
        let lo = spike.gap_start_clock - EVENT_WINDOW_S;
        let hi = spike.gap_start_clock;
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

    let replay_duration_s = headers.iter().map(|h| h.clock).fold(0.0_f32, f32::max);
    let battle_start_clock_exact = battle_start_clock.is_some();
    let battle_start_clock_s = battle_start_clock.unwrap_or(30.94_f32);
    let severity = classify_severity(&spikes, battle_start_clock_s);

    AnalysisResult {
        meta,
        battle_start_clock_s,
        battle_start_clock_exact,
        samples: samples_out,
        server_tick_clocks: server_ticks.clone(),
        spikes,
        ping_stats,
        samples_total: samples.len() as u32,
        server_ticks_total: server_ticks.len() as u32,
        replay_duration_s,
        spike_threshold_ms: (thresholds.min_gap_s * 1000.0).round() as u32,
        severity,
        entity_defs_loaded,
        game_params_loaded,
        corrupt_packet_clocks,
    }
}

fn classify_severity(spikes: &[Spike], battle_start: f32) -> SeveritySummary {
    let spike_count = spikes.len() as u32;
    let total_stalled_s: f32 = spikes.iter().map(|s| s.gap_seconds).sum();
    let worst = spikes.iter().max_by(|a, b| {
        a.gap_seconds
            .partial_cmp(&b.gap_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let worst_gap_s = worst.map(|s| s.gap_seconds).unwrap_or(0.0);
    let worst_gap_battle_s = worst
        .map(|s| s.gap_start_clock - battle_start)
        .unwrap_or(0.0);

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

    SeveritySummary {
        severity,
        spike_count,
        total_stalled_s,
        worst_gap_s,
        worst_gap_battle_s,
        headline,
    }
}

fn compute_ping_stats(samples: &[NetStat]) -> PingStats {
    if samples.is_empty() {
        return PingStats {
            min_ms: 0,
            max_ms: 0,
            mean_ms: 0.0,
            p95_ms: 0,
        };
    }
    let mut pings: Vec<u16> = samples.iter().map(|s| s.ping).collect();
    pings.sort_unstable();
    let min_ms = *pings.first().unwrap();
    let max_ms = *pings.last().unwrap();
    let sum: u64 = pings.iter().map(|&p| p as u64).sum();
    let mean_ms = (sum as f64 / pings.len() as f64) as f32;
    let idx = ((pings.len() as f64) * 0.95).floor() as usize;
    let p95_ms = pings[idx.min(pings.len() - 1)];
    PingStats {
        min_ms,
        max_ms,
        mean_ms,
        p95_ms,
    }
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

        let burst_ticks = server_ticks[..=idx]
            .iter()
            .rev()
            .take_while(|&&t| t == prev)
            .count() as u32;

        let client_packets_in_gap = headers
            .iter()
            .filter(|h| h.clock > prev && h.clock < cur && h.is_client_side)
            .count() as u32;

        let (peak_ping_ms, peak_ping_clock) = samples
            .iter()
            .filter(|s| s.clock >= prev && s.clock <= (cur + 2.0))
            .map(|s| (s.ping, s.clock))
            .max_by_key(|(p, _)| *p)
            .unwrap_or((0, prev));

        let client_rate_hz = if gap > 0.0 {
            client_packets_in_gap as f32 / gap
        } else {
            0.0
        };
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
            seconds_since_previous_spike: None,
            preceding_events: Vec::new(),
        });
    }
    spikes
}
