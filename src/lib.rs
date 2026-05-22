pub mod analysis;
pub mod replay;

use std::borrow::Cow;
use std::collections::HashMap;

use wasm_bindgen::prelude::*;
use wows_replays::ReplayFile;
use wows_replays::analyzer::decoder::{DecodedPacketPayload, PacketDecoder};
use wows_replays::game_constants::GameConstants;
use wows_replays::packet2::{PacketTypeId, Parser, PlayerNetStatsPacket, RawPacketIterator};
use wows_replays::types::EntityId;
use wowsunpack::data::Version;
use wowsunpack::data::DataFileWithCallback;
use wowsunpack::data::ship_config::parse_ship_config;
use wowsunpack::error::GameDataError;
use wowsunpack::rpc::entitydefs::{EntitySpec, parse_scripts};
use wowsunpack::rpc::typedefs::ArgValue;

use analysis::GameEvent;

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
    let replay = ReplayFile::from_bytes(bytes).map_err(|e| JsError::new(&format!("replay parse: {e}")))?;
    let v = &replay.meta.clientVersionFromExe;
    let info = ReplayInfo {
        client_version: v.clone(),
        build: replay::build_from_client_version(v),
        dir_name: replay::version_dir_name(v),
    };
    serde_wasm_bindgen::to_value(&info).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

#[wasm_bindgen(js_name = analyzeReplay)]
pub fn analyze_replay(bytes: &[u8], defs_bundle: &[u8]) -> Result<JsValue, JsError> {
    let result = run_analysis(bytes, defs_bundle).map_err(|e| JsError::new(&e))?;
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

/// Parse a replay and run the spike analysis. With entity defs the replay goes
/// through the full decoder (arena ID, consumables, kills, and per-spike event
/// context); without them it falls back to the spec-free `RawPacketIterator`.
pub fn run_analysis(bytes: &[u8], defs_bundle: &[u8]) -> Result<analysis::AnalysisResult, String> {
    let replay = ReplayFile::from_bytes(bytes).map_err(|e| format!("replay parse: {e}"))?;

    let build = replay::build_from_client_version(&replay.meta.clientVersionFromExe);
    let region = replay::detect_realm(&replay.packet_data);
    let specs = load_entity_specs(defs_bundle)?;

    let mut samples = Vec::new();
    let mut server_ticks = Vec::new();
    let mut headers = Vec::new();
    let mut events: Vec<GameEvent> = Vec::new();
    let mut arena_id: Option<i64> = None;

    match &specs {
        Some(specs) => decode_with_specs(
            &replay,
            specs,
            &mut samples,
            &mut server_ticks,
            &mut headers,
            &mut events,
            &mut arena_id,
        )?,
        None => {
            for packet in RawPacketIterator::new(&replay.packet_data) {
                let packet = packet.map_err(|e| format!("packet parse: {e:?}"))?;
                let clock = packet.clock.seconds();
                headers.push(analysis::PacketHeader { clock, ptype: packet.packet_type });
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
        analysis::SpikeThresholds::default(),
    ))
}

/// Full decode pass: ping samples, server ticks, arena ID, and the in-game
/// events (consumables, ship kills + death effects) used for spike context.
fn decode_with_specs(
    replay: &ReplayFile,
    specs: &[EntitySpec],
    samples: &mut Vec<analysis::NetStat>,
    server_ticks: &mut Vec<f32>,
    headers: &mut Vec<analysis::PacketHeader>,
    events: &mut Vec<GameEvent>,
    arena_id: &mut Option<i64>,
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
    // Ship entity -> last visibilityFlags. Bit 0 = visible to the recording
    // player; a 0 -> 1 transition is a "spotted" event.
    let mut visibility: HashMap<EntityId, i32> = HashMap::new();
    // Ship entity -> player name and numeric ship param id, from the arena
    // state. Used to label spotted/kill/consumable events.
    let mut ships: HashMap<EntityId, ShipInfo> = HashMap::new();

    let mut parser = Parser::new(specs);
    let mut remaining = &replay.packet_data[..];
    while !remaining.is_empty() {
        let packet = parser
            .parse_packet(&mut remaining)
            .map_err(|e| format!("packet parse: {e:?}"))?;
        let clock = packet.clock.seconds();
        headers.push(analysis::PacketHeader { clock, ptype: packet.packet_type });

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
                    ships.insert(
                        p.entity_id(),
                        ShipInfo {
                            name: p.username().to_string(),
                            ship_param_id: p.ship_params_id().map(|g| g.raw()),
                        },
                    );
                }
            }
            DecodedPacketPayload::EntityCreate(ec) => {
                if let Some(effect) = death_effect_from_props(&ec.props, &version) {
                    death_effects.insert(ec.entity_id, effect);
                }
            }
            DecodedPacketPayload::CellPlayerCreate(cpc) => {
                if let Some(effect) = death_effect_from_props(&cpc.props, &version) {
                    death_effects.insert(cpc.entity_id, effect);
                }
            }
            DecodedPacketPayload::Consumable { entity, consumable, .. } => {
                events.push(GameEvent {
                    clock,
                    tick_offset: 0,
                    kind: "consumable".to_string(),
                    text: format!("{} used {}", ship_label(&ships, *entity), consumable_name(consumable)),
                    entity_id: Some(entity.raw()),
                    ship_param_id: ships.get(entity).and_then(|s| s.ship_param_id),
                });
            }
            DecodedPacketPayload::ShipDestroyed { killer, victim, cause } => {
                let mut text = format!(
                    "{} destroyed by {} ({})",
                    ship_label(&ships, *victim),
                    ship_label(&ships, *killer),
                    death_cause_name(cause),
                );
                if let Some(effect) = death_effects.get(killer) {
                    text.push_str(&format!("; killer's death effect: {effect}"));
                }
                events.push(GameEvent {
                    clock,
                    tick_offset: 0,
                    kind: "kill".to_string(),
                    text,
                    entity_id: Some(victim.raw()),
                    ship_param_id: ships.get(victim).and_then(|s| s.ship_param_id),
                });
            }
            DecodedPacketPayload::EntityProperty(ep) if ep.property == "visibilityFlags" => {
                let new = ep.value.as_i32().unwrap_or(0);
                let was_visible = visibility.get(&ep.entity_id).map(|v| v & 1 != 0);
                if was_visible == Some(false) && new & 1 != 0 {
                    events.push(GameEvent {
                        clock,
                        tick_offset: 0,
                        kind: "spotted".to_string(),
                        text: format!("Spotted {}", ship_label(&ships, ep.entity_id)),
                        entity_id: Some(ep.entity_id.raw()),
                        ship_param_id: ships.get(&ep.entity_id).and_then(|s| s.ship_param_id),
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
    analysis::NetStat { clock, fps: ns.fps, ping: ns.ping, is_lagging: ns.is_lagging }
}

/// Player name and numeric ship param id for one ship entity, from the arena
/// state packet.
struct ShipInfo {
    name: String,
    ship_param_id: Option<u64>,
}

/// Human-friendly label for a ship entity: the owning player's name, or
/// `entity <id>` when the entity was not in the arena state.
fn ship_label(ships: &HashMap<EntityId, ShipInfo>, eid: EntityId) -> String {
    match ships.get(&eid) {
        Some(s) if !s.name.is_empty() => s.name.clone(),
        _ => format!("entity {}", eid.raw()),
    }
}

/// Pull the equipped death effect (if any) out of a ship's `shipConfig` blob,
/// found in its entity-creation properties.
fn death_effect_from_props(
    props: &HashMap<&str, ArgValue>,
    version: &Version,
) -> Option<&'static str> {
    let ArgValue::Blob(blob) = props.get("shipConfig")? else {
        return None;
    };
    let config = parse_ship_config(blob, version).ok()?;
    config
        .exteriors()
        .iter()
        .find_map(|id| replay::death_effect_name(id.raw()))
}

fn consumable_name(c: &wowsunpack::recognized::Recognized<wowsunpack::game_types::Consumable>) -> String {
    match c.known() {
        Some(known) => format!("{known:?}"),
        None => "Consumable".to_string(),
    }
}

fn death_cause_name(c: &wowsunpack::recognized::Recognized<wowsunpack::game_types::DeathCause>) -> String {
    match c.known() {
        Some(known) => format!("{known:?}"),
        None => "unknown".to_string(),
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
            .ok_or_else(|| GameDataError::Io(std::io::Error::other(format!("missing bundled file: {path}"))))
    });
    let specs = parse_scripts(&loader).map_err(|e| format!("entity defs: {e}"))?;
    Ok(Some(specs))
}
