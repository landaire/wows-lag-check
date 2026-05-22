pub mod analysis;
pub mod replay;

use std::borrow::Cow;
use std::collections::HashMap;

use gettext::Catalog;
use wasm_bindgen::prelude::*;
use wows_replays::ReplayFile;
use wows_replays::analyzer::decoder::DecodedPacketPayload;
use wows_replays::analyzer::decoder::PacketDecoder;
use wows_replays::game_constants::GameConstants;
use wows_replays::packet2::PacketTypeId;
use wows_replays::packet2::Parser;
use wows_replays::packet2::PlayerNetStatsPacket;
use wows_replays::packet2::RawPacketIterator;
use wows_replays::types::EntityId;
use wowsunpack::data::DataFileWithCallback;
use wowsunpack::data::Version;
use wowsunpack::data::ship_config::parse_ship_config;
use wowsunpack::error::GameDataError;
use wowsunpack::game_params::types::Param;
use wowsunpack::game_params::types::Species;
use wowsunpack::game_types::GameParamId;
use wowsunpack::rpc::entitydefs::EntitySpec;
use wowsunpack::rpc::entitydefs::parse_scripts;
use wowsunpack::rpc::typedefs::ArgValue;

use analysis::GameEvent;

/// Packets whose clock falls outside this range carry a corrupt timestamp.
/// Some replays contain garbage f32 clock values (e.g. 1e32); such packets
/// are skipped during parsing so they can't poison durations or the chart.
const MAX_REPLAY_CLOCK_S: f32 = 86_400.0;

#[wasm_bindgen(start)]
pub fn _start() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

#[derive(serde::Serialize)]
struct ReplayInfo {
    client_version: String,
    build: Option<u32>,
    dir_name: Option<String>,
}

#[wasm_bindgen(js_name = replayInfo)]
pub fn replay_info(bytes: &[u8]) -> Result<JsValue, JsError> {
    let replay =
        ReplayFile::from_bytes(bytes).map_err(|e| JsError::new(&format!("replay parse: {e}")))?;
    let v = &replay.meta.clientVersionFromExe;
    let info = ReplayInfo {
        client_version: v.clone(),
        build: replay::build_from_client_version(v),
        dir_name: replay::version_dir_name(v),
    };
    serde_wasm_bindgen::to_value(&info).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

#[wasm_bindgen(js_name = analyzeReplay)]
pub fn analyze_replay(
    bytes: &[u8],
    defs_bundle: &[u8],
    game_params: &[u8],
    translations: &[u8],
) -> Result<JsValue, JsError> {
    let result = run_analysis(bytes, defs_bundle, game_params, translations)
        .map_err(|e| JsError::new(&e))?;
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

/// Parse a replay and run the spike analysis. With entity defs the replay goes
/// through the full decoder (arena ID, consumables, kills, and per-spike event
/// context); without them it falls back to the spec-free `RawPacketIterator`.
/// `game_params` (rkyv blob) and `translations` (.mo catalog) resolve ship and
/// camouflage display names; either may be empty.
pub fn run_analysis(
    bytes: &[u8],
    defs_bundle: &[u8],
    game_params: &[u8],
    translations: &[u8],
) -> Result<analysis::AnalysisResult, String> {
    let replay = ReplayFile::from_bytes(bytes).map_err(|e| format!("replay parse: {e}"))?;

    let build = replay::build_from_client_version(&replay.meta.clientVersionFromExe);
    let region = replay::detect_realm(&replay.packet_data);
    let specs = load_entity_specs(defs_bundle)?;
    let resolver = NameResolver::load(game_params, translations);

    let mut samples = Vec::new();
    let mut server_ticks = Vec::new();
    let mut headers = Vec::new();
    let mut events: Vec<GameEvent> = Vec::new();
    let mut arena_id: Option<i64> = None;
    let mut battle_start_clock: Option<f32> = None;
    let mut corrupt_packet_clocks: Vec<f32> = Vec::new();

    match &specs {
        Some(specs) => decode_with_specs(
            &replay,
            specs,
            &resolver,
            &mut samples,
            &mut server_ticks,
            &mut headers,
            &mut events,
            &mut arena_id,
            &mut battle_start_clock,
            &mut corrupt_packet_clocks,
        )?,
        None => {
            let mut last_valid_clock: f32 = 0.0;
            for packet in RawPacketIterator::new(&replay.packet_data) {
                let packet = packet.map_err(|e| format!("packet parse: {e:?}"))?;
                let clock = packet.clock.seconds();
                if !(0.0..MAX_REPLAY_CLOCK_S).contains(&clock) {
                    corrupt_packet_clocks.push(last_valid_clock);
                    continue;
                }
                last_valid_clock = clock;
                headers.push(analysis::PacketHeader {
                    clock,
                    ptype: packet.packet_type,
                });
                match packet.packet_type {
                    PacketTypeId::PlayerNetStats => {
                        if let Some(ns) = PlayerNetStatsPacket::from_payload(packet.payload) {
                            samples.push(net_stat(clock, &ns));
                        }
                    }
                    PacketTypeId::ServerTick => server_ticks.push(clock),
                    _ => {}
                }
            }
        }
    }

    Ok(analysis::build_analysis(
        replay.meta,
        samples,
        server_ticks,
        headers,
        events,
        arena_id,
        build,
        region,
        specs.is_some(),
        resolver.loaded(),
        battle_start_clock,
        corrupt_packet_clocks,
        analysis::SpikeThresholds::default(),
    ))
}

/// Full decode pass: ping samples, server ticks, arena ID, and the in-game
/// events (consumables, ship kills + death effects, spots) used for spike
/// context. Ship and camouflage names come from `resolver`.
fn decode_with_specs(
    replay: &ReplayFile,
    specs: &[EntitySpec],
    resolver: &NameResolver,
    samples: &mut Vec<analysis::NetStat>,
    server_ticks: &mut Vec<f32>,
    headers: &mut Vec<analysis::PacketHeader>,
    events: &mut Vec<GameEvent>,
    arena_id: &mut Option<i64>,
    battle_start_clock: &mut Option<f32>,
    corrupt_packet_clocks: &mut Vec<f32>,
) -> Result<(), String> {
    let version = Version::from_client_exe(&replay.meta.clientVersionFromExe);
    let game_constants = GameConstants::defaults();
    let decoder = PacketDecoder::builder()
        .version(version)
        .battle_constants(game_constants.battle())
        .common_constants(game_constants.common())
        .ships_constants(game_constants.ships())
        .build();

    // Ship entity -> equipped death effect name, harvested from shipConfig.
    let mut death_effects: HashMap<EntityId, &'static str> = HashMap::new();
    // Ship entity -> equipped camouflage name, harvested from shipConfig.
    let mut camos: HashMap<EntityId, String> = HashMap::new();
    // Ship entity -> last visibilityFlags. Bit 0 = visible to the recording
    // player; a 0 -> 1 transition is a "spotted" event.
    let mut visibility: HashMap<EntityId, i32> = HashMap::new();
    // Ship entity -> player name, ship param id, and ship name, from the
    // arena state. Used to label spotted/kill/consumable events.
    let mut ships: HashMap<EntityId, ShipInfo> = HashMap::new();

    let mut parser = Parser::new(specs);
    let mut remaining = &replay.packet_data[..];
    let mut last_valid_clock: f32 = 0.0;
    while !remaining.is_empty() {
        let packet = parser
            .parse_packet(&mut remaining)
            .map_err(|e| format!("packet parse: {e:?}"))?;
        let clock = packet.clock.seconds();
        if !(0.0..MAX_REPLAY_CLOCK_S).contains(&clock) {
            corrupt_packet_clocks.push(last_valid_clock);
            continue;
        }
        last_valid_clock = clock;
        headers.push(analysis::PacketHeader {
            clock,
            ptype: packet.packet_type,
        });

        let decoded = decoder.decode(&packet);
        match &decoded.payload {
            DecodedPacketPayload::PlayerNetStats(ns) => samples.push(net_stat(clock, ns)),
            DecodedPacketPayload::ServerTick(_) => server_ticks.push(clock),
            DecodedPacketPayload::OnArenaStateReceived {
                arena_id: aid,
                player_states,
                bot_states,
                ..
            } => {
                if arena_id.is_none() {
                    *arena_id = Some(*aid);
                }
                for p in player_states.iter().chain(bot_states.iter()) {
                    let ship_param_id = p.ship_params_id();
                    ships.insert(
                        p.entity_id(),
                        ShipInfo {
                            player: p.username().to_string(),
                            ship_param_id: ship_param_id.map(|g| g.raw()),
                            ship_name: ship_param_id.and_then(|id| resolver.ship_name(id)),
                        },
                    );
                }
            }
            DecodedPacketPayload::EntityCreate(ec) => {
                index_ship_config(
                    &ec.props,
                    ec.entity_id,
                    &version,
                    resolver,
                    &mut death_effects,
                    &mut camos,
                );
            }
            DecodedPacketPayload::CellPlayerCreate(cpc) => {
                index_ship_config(
                    &cpc.props,
                    cpc.entity_id,
                    &version,
                    resolver,
                    &mut death_effects,
                    &mut camos,
                );
            }
            DecodedPacketPayload::Consumable {
                entity, consumable, ..
            } => {
                events.push(GameEvent {
                    clock,
                    tick_offset: 0,
                    kind: "consumable".to_string(),
                    ships: vec![event_ship(&ships, &camos, *entity)],
                    detail: consumable_name(consumable),
                    death_effect: None,
                });
            }
            DecodedPacketPayload::ShipDestroyed {
                killer,
                victim,
                cause,
            } => {
                events.push(GameEvent {
                    clock,
                    tick_offset: 0,
                    kind: "kill".to_string(),
                    ships: vec![
                        event_ship(&ships, &camos, *victim),
                        event_ship(&ships, &camos, *killer),
                    ],
                    detail: death_cause_name(cause),
                    death_effect: death_effects.get(killer).map(|e| e.to_string()),
                });
            }
            DecodedPacketPayload::EntityProperty(ep) if ep.property == "battleStage" => {
                // BattleLogic.battleStage: raw 0 == BattleStage::Waiting, which is
                // the moment the pre-battle countdown ends and the match starts.
                // (See wows_replays BattleController for the same logic.)
                if battle_start_clock.is_none() && ep.value.as_i32() == Some(0) {
                    *battle_start_clock = Some(clock);
                }
            }
            DecodedPacketPayload::EntityProperty(ep) if ep.property == "visibilityFlags" => {
                let new = ep.value.as_i32().unwrap_or(0);
                let was_visible = visibility.get(&ep.entity_id).map(|v| v & 1 != 0);
                if was_visible == Some(false) && new & 1 != 0 {
                    events.push(GameEvent {
                        clock,
                        tick_offset: 0,
                        kind: "spotted".to_string(),
                        ships: vec![event_ship(&ships, &camos, ep.entity_id)],
                        detail: String::new(),
                        death_effect: None,
                    });
                }
                visibility.insert(ep.entity_id, new);
            }
            _ => {}
        }
    }
    Ok(())
}

fn net_stat(clock: f32, ns: &PlayerNetStatsPacket) -> analysis::NetStat {
    analysis::NetStat {
        clock,
        fps: ns.fps,
        ping: ns.ping,
        is_lagging: ns.is_lagging,
    }
}

/// Player name, ship param id, and ship display name for one ship entity.
struct ShipInfo {
    player: String,
    ship_param_id: Option<u64>,
    ship_name: Option<String>,
}

/// Resolve a ship entity into an `EventShip`, merging the arena-state info
/// (player, ship name/id) with the camouflage harvested from shipConfig.
/// `player` falls back to `entity <id>` when the ship is unknown.
fn event_ship(
    ships: &HashMap<EntityId, ShipInfo>,
    camos: &HashMap<EntityId, String>,
    eid: EntityId,
) -> analysis::EventShip {
    let info = ships.get(&eid);
    let player = match info {
        Some(s) if !s.player.is_empty() => s.player.clone(),
        _ => format!("entity {}", eid.raw()),
    };
    analysis::EventShip {
        entity_id: eid.raw(),
        player,
        ship_name: info.and_then(|s| s.ship_name.clone()),
        ship_param_id: info.and_then(|s| s.ship_param_id),
        camo: camos.get(&eid).cloned(),
    }
}

/// Parse a ship's `shipConfig` blob (from its entity-creation properties) and
/// record the equipped death effect and camouflage, resolved from its
/// exteriors list.
fn index_ship_config(
    props: &HashMap<&str, ArgValue>,
    entity_id: EntityId,
    version: &Version,
    resolver: &NameResolver,
    death_effects: &mut HashMap<EntityId, &'static str>,
    camos: &mut HashMap<EntityId, String>,
) {
    let Some(ArgValue::Blob(blob)) = props.get("shipConfig") else {
        return;
    };
    let Ok(config) = parse_ship_config(blob, version) else {
        return;
    };
    if let Some(effect) = config
        .exteriors()
        .iter()
        .find_map(|id| replay::death_effect_name(id.raw()))
    {
        death_effects.insert(entity_id, effect);
    }
    if let Some(camo) = resolver.camo_name(config.exteriors()) {
        camos.insert(entity_id, camo);
    }
}

fn consumable_name(
    c: &wowsunpack::recognized::Recognized<wowsunpack::game_types::Consumable>,
) -> String {
    match c.known() {
        Some(known) => format!("{known:?}"),
        None => "Consumable".to_string(),
    }
}

fn death_cause_name(
    c: &wowsunpack::recognized::Recognized<wowsunpack::game_types::DeathCause>,
) -> String {
    match c.known() {
        Some(known) => format!("{known:?}"),
        None => "unknown".to_string(),
    }
}

/// zstd-decompress a blob when it carries the zstd magic; otherwise return it
/// borrowed unchanged. The web client fetches the game-params rkyv and the .mo
/// catalog zstd-compressed; the smoke harness may pass either form.
fn inflate_if_zstd(data: &[u8]) -> Cow<'_, [u8]> {
    const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
    if data.len() < 4 || data[..4] != ZSTD_MAGIC {
        return Cow::Borrowed(data);
    }
    let mut out = Vec::new();
    let ok = ruzstd::decoding::StreamingDecoder::new(data)
        .ok()
        .and_then(|mut d| std::io::Read::read_to_end(&mut d, &mut out).ok())
        .is_some();
    Cow::Owned(if ok { out } else { Vec::new() })
}

/// Resolves ship and camouflage display names from the rkyv GameParams blob
/// and the gettext translation catalog.
struct NameResolver {
    by_id: HashMap<GameParamId, Param>,
    catalog: Option<Catalog>,
}

impl NameResolver {
    /// Load from the game-params blob and the .mo translation catalog. Each
    /// input may be a raw or zstd-compressed archive, and either may be empty,
    /// which disables the corresponding lookups.
    fn load(game_params: &[u8], translations: &[u8]) -> Self {
        let rkyv_bytes = inflate_if_zstd(game_params);
        let by_id = if rkyv_bytes.is_empty() {
            HashMap::new()
        } else {
            rkyv::from_bytes::<Vec<Param>, rkyv::rancor::Error>(&rkyv_bytes)
                .map(|params| params.into_iter().map(|p| (p.id(), p)).collect())
                .unwrap_or_default()
        };
        let mo_bytes = inflate_if_zstd(translations);
        let catalog = if mo_bytes.is_empty() {
            None
        } else {
            Catalog::parse(&mo_bytes[..]).ok()
        };
        Self { by_id, catalog }
    }

    /// True once the GameParams blob has been parsed.
    fn loaded(&self) -> bool {
        !self.by_id.is_empty()
    }

    /// Translate an `IDS_*` key. None when the catalog is absent or returns the
    /// key unchanged (gettext's signal for "not found").
    fn translate(&self, key: &str) -> Option<String> {
        let catalog = self.catalog.as_ref()?;
        let value = catalog.gettext(key);
        if value == key {
            None
        } else {
            Some(value.to_string())
        }
    }

    /// Display name for a ship param id, e.g. "Utrecht".
    fn ship_name(&self, id: GameParamId) -> Option<String> {
        let param = self.by_id.get(&id)?;
        self.translate(&format!("IDS_{}", param.index()))
    }

    /// Display name of the camouflage in a ship's exteriors list. The list also
    /// holds signal flags, ensigns, and the death effect, which are skipped; a
    /// ship carries at most one camouflage. The exterior's translation key is
    /// `IDS_{name}` uppercased (unlike ships, which key off the index).
    fn camo_name(&self, exteriors: &[GameParamId]) -> Option<String> {
        for id in exteriors {
            let Some(param) = self.by_id.get(id) else {
                continue;
            };
            let is_camo = matches!(
                param.species().and_then(|s| s.known()),
                Some(Species::Permoflage | Species::Camouflage | Species::Skin | Species::MSkin)
            );
            if is_camo
                && let Some(name) = self.translate(&format!("IDS_{}", param.name().to_uppercase()))
            {
                return Some(name);
            }
        }
        None
    }
}

fn load_entity_specs(defs_bundle: &[u8]) -> Result<Option<Vec<EntitySpec>>, String> {
    if defs_bundle.is_empty() {
        return Ok(None);
    }
    let files = replay::unpack_def_bundle(defs_bundle)
        .ok_or_else(|| "malformed entity-def bundle".to_string())?;
    let loader = DataFileWithCallback::new(move |path: &str| {
        files
            .get(path)
            .map(|v| Cow::Owned(v.clone()))
            .ok_or_else(|| {
                GameDataError::Io(std::io::Error::other(format!(
                    "missing bundled file: {path}"
                )))
            })
    });
    let specs = parse_scripts(&loader).map_err(|e| format!("entity defs: {e}"))?;
    Ok(Some(specs))
}
