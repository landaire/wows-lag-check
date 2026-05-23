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

/// Which packet stream the gap was detected in.
///
/// `Server` spikes come from gaps in the 7Hz ServerTick stream and historically
/// catch network/server lag. `Client` spikes come from gaps in the 10Hz
/// Camera/GunMarker/PlayerNetStats stream and catch game-thread freezes: when
/// the client stalls, incoming server packets get buffered and written with the
/// pre-freeze clock, but outgoing client-side packets simply pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SpikeSource {
    Server,
    Client,
}

#[derive(Debug, Clone, Serialize)]
pub struct Spike {
    pub source: SpikeSource,
    pub gap_start_clock: f32,
    pub gap_end_clock: f32,
    pub gap_seconds: f32,
    pub peak_ping_ms: u16,
    pub peak_ping_clock: f32,
    pub client_packets_in_gap: u32,
    pub client_rate_hz: f32,
    pub client_present_during_gap: bool,
    /// Total ServerTick records observed in the gap window. During a client
    /// freeze, many ticks get recorded at the same pre-freeze clock value, so
    /// this can be high even when [`server_distinct_clocks_in_gap`] is zero.
    pub server_packets_in_gap: u32,
    /// Count of distinct clock values among server ticks inside the gap. Near
    /// the normal 7Hz means the server was actually running during the gap;
    /// zero means everything stalled together (or got buffered).
    pub server_distinct_clocks_in_gap: u32,
    pub server_rate_hz: f32,
    pub server_present_during_gap: bool,
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
    /// Clocks of the lockstep 10Hz client-side packet stream (one entry per
    /// observed Camera packet). A gap here indicates the game thread froze.
    pub client_tick_clocks: Vec<f32>,
    /// Server-tick gaps, preserved for compatibility. See [`Spike::source`].
    pub spikes: Vec<Spike>,
    /// Client-tick gaps. Often overlap with server spikes during a freeze
    /// (same event seen from two angles), but provide finer temporal
    /// resolution and a cleaner signature for client-side stalls.
    pub client_spikes: Vec<Spike>,
    pub ping_stats: PingStats,
    pub samples_total: u32,
    pub server_ticks_total: u32,
    pub client_ticks_total: u32,
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
    client_ticks: Vec<f32>,
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

    let mut spikes = detect_spikes(
        SpikeSource::Server,
        &server_ticks,
        &server_ticks,
        &samples,
        &headers,
        thresholds,
    );
    let mut client_spikes = detect_spikes(
        SpikeSource::Client,
        &client_ticks,
        &server_ticks,
        &samples,
        &headers,
        thresholds,
    );

    let replay_duration_s = headers.iter().map(|h| h.clock).fold(0.0_f32, f32::max);
    let battle_start_clock_exact = battle_start_clock.is_some();
    let battle_start_clock_s = battle_start_clock.unwrap_or(30.94_f32);

    // Drop pure client-stream gaps that finish before the player is in
    // battle. The 10Hz Camera cadence doesn't begin until the user has
    // clicked through the loading screen, so anything earlier registers as a
    // "client freeze" that's actually just init. Server-stream gaps from the
    // same window are kept — they represent both-streams-silent events that
    // the existing analyzer has always reported.
    client_spikes.retain(|s| s.gap_end_clock >= battle_start_clock_s);

    annotate_spike_chain(&mut spikes);
    annotate_spike_chain(&mut client_spikes);

    let mut distinct_ticks = server_ticks.clone();
    distinct_ticks.dedup();
    annotate_preceding_events(&mut spikes, &events, &distinct_ticks);
    annotate_preceding_events(&mut client_spikes, &events, &distinct_ticks);

    let severity = classify_severity(&spikes, &client_spikes, battle_start_clock_s);

    AnalysisResult {
        meta,
        battle_start_clock_s,
        battle_start_clock_exact,
        samples: samples_out,
        server_tick_clocks: server_ticks.clone(),
        client_tick_clocks: client_ticks.clone(),
        spikes,
        client_spikes,
        ping_stats,
        samples_total: samples.len() as u32,
        server_ticks_total: server_ticks.len() as u32,
        client_ticks_total: client_ticks.len() as u32,
        replay_duration_s,
        spike_threshold_ms: (thresholds.min_gap_s * 1000.0).round() as u32,
        severity,
        entity_defs_loaded,
        game_params_loaded,
        corrupt_packet_clocks,
    }
}

fn annotate_spike_chain(spikes: &mut [Spike]) {
    let mut prev_end: Option<f32> = None;
    for spike in spikes.iter_mut() {
        spike.seconds_since_previous_spike =
            prev_end.map(|end| (spike.gap_start_clock - end).max(0.0));
        prev_end = Some(spike.gap_end_clock);
    }
}

fn annotate_preceding_events(spikes: &mut [Spike], events: &[GameEvent], distinct_ticks: &[f32]) {
    for spike in spikes.iter_mut() {
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
}

/// Severity considers both server-stream and client-stream spikes. The two
/// streams usually report the same freezes (server-tick burst at frozen clock,
/// client-tick clean gap), so deduping by overlap avoids double-counting.
fn classify_severity(
    server_spikes: &[Spike],
    client_spikes: &[Spike],
    battle_start: f32,
) -> SeveritySummary {
    let merged = merge_overlapping_spikes(server_spikes, client_spikes);
    let spike_count = merged.len() as u32;
    let total_stalled_s: f32 = merged.iter().map(|s| s.2).sum();
    let worst = merged
        .iter()
        .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
    let worst_gap_s = worst.map(|s| s.2).unwrap_or(0.0);
    let worst_gap_battle_s = worst.map(|s| s.0 - battle_start).unwrap_or(0.0);

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
        Severity::Clean => "No stalls detected.".to_string(),
        _ => format!(
            "{} stall{}, {:.1}s total stalled, worst {:.1}s",
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

/// Walks the union of server- and client-stream spikes in clock order and
/// collapses overlapping windows into a single (start, end, length) tuple. The
/// gap_seconds field of the synthesized spike covers the merged extent rather
/// than the sum of contributors.
fn merge_overlapping_spikes(server: &[Spike], client: &[Spike]) -> Vec<(f32, f32, f32)> {
    let mut windows: Vec<(f32, f32)> = server
        .iter()
        .chain(client.iter())
        .map(|s| (s.gap_start_clock, s.gap_end_clock))
        .collect();
    windows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<(f32, f32, f32)> = Vec::new();
    for (start, end) in windows {
        if let Some(last) = out.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
            last.2 = last.1 - last.0;
            continue;
        }
        out.push((start, end, end - start));
    }
    out
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
    source: SpikeSource,
    primary_ticks: &[f32],
    server_ticks: &[f32],
    samples: &[NetStat],
    headers: &[PacketHeader],
    thresholds: SpikeThresholds,
) -> Vec<Spike> {
    let mut spikes = Vec::new();
    for (idx, window) in primary_ticks.windows(2).enumerate() {
        let prev = window[0];
        let cur = window[1];
        let gap = cur - prev;
        if gap < thresholds.min_gap_s {
            continue;
        }

        // For server-stream gaps, burst_ticks is the classic "server fired N
        // repeated ticks at the same clock before freezing" signal. For
        // client-stream gaps it tends to be 1, since client-side packets pause
        // cleanly rather than bunching.
        let burst_ticks = primary_ticks[..=idx]
            .iter()
            .rev()
            .take_while(|&&t| t == prev)
            .count() as u32;

        let client_packets_in_gap = headers
            .iter()
            .filter(|h| h.clock > prev && h.clock < cur && h.is_client_side)
            .count() as u32;

        let server_total_in_gap = server_ticks
            .iter()
            .filter(|&&t| t > prev && t < cur)
            .count() as u32;
        let mut server_in_gap_clocks: Vec<f32> = server_ticks
            .iter()
            .copied()
            .filter(|&t| t > prev && t < cur)
            .collect();
        server_in_gap_clocks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        server_in_gap_clocks.dedup();
        let server_distinct_in_gap = server_in_gap_clocks.len() as u32;

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

        let server_rate_hz = if gap > 0.0 {
            server_distinct_in_gap as f32 / gap
        } else {
            0.0
        };
        // Normal server tick cadence is 7Hz; >3Hz means the server was
        // actually running (not just buffering) during the gap.
        let server_present_during_gap = server_rate_hz > 3.0;

        spikes.push(Spike {
            source,
            gap_start_clock: prev,
            gap_end_clock: cur,
            gap_seconds: gap,
            peak_ping_ms,
            peak_ping_clock,
            client_packets_in_gap,
            client_rate_hz,
            client_present_during_gap,
            server_packets_in_gap: server_total_in_gap,
            server_distinct_clocks_in_gap: server_distinct_in_gap,
            server_rate_hz,
            server_present_during_gap,
            burst_ticks,
            seconds_since_previous_spike: None,
            preceding_events: Vec::new(),
        });
    }
    spikes
}
