//! WoWs replay analysis.
//!
//! Walks the decrypted packet stream and feeds PlayerNetStats and ServerTick
//! into the spike detector. With entity defs we also get arena id, kills,
//! consumables, and spotted events; without them we fall back to the spec-free
//! iterator plus a pickle byte-scan for the realm.

use std::borrow::Cow;
use std::collections::HashMap;

use gettext::Catalog;
use wows_replays::ReplayFile;
use wows_replays::ReplayMeta;
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

use crate::MAX_REPLAY_CLOCK_S;
use crate::analysis;
use crate::analysis::EventKind;
use crate::analysis::GameEvent;
use crate::replay;

fn is_client_side_packet(ptype: PacketTypeId) -> bool {
    matches!(
        ptype,
        PacketTypeId::Camera | PacketTypeId::GunMarker | PacketTypeId::PlayerNetStats
    )
}

#[derive(serde::Serialize)]
pub struct ReplayInfo {
    pub client_version: String,
    pub build: Option<u32>,
    pub dir_name: Option<String>,
}

pub fn replay_info_from_meta(meta: &ReplayMeta) -> ReplayInfo {
    let v = &meta.clientVersionFromExe;
    ReplayInfo {
        client_version: v.clone(),
        build: replay::build_from_client_version(v),
        dir_name: replay::version_dir_name(v),
    }
}

pub fn analyze(
    bytes: &[u8],
    defs_bundle: &[u8],
    game_params: &[u8],
    translations: &[u8],
    threshold_ms: Option<u32>,
) -> Result<analysis::AnalysisResult, String> {
    let replay = ReplayFile::from_bytes(bytes).map_err(|e| format!("replay parse: {e}"))?;
    let specs = load_entity_specs(defs_bundle)?;
    let resolver = NameResolver::load(game_params, translations);

    let mut decoder = WowsDecoder::new(&replay, &resolver);
    match &specs {
        Some(specs) => decoder.decode_with_specs(specs)?,
        None => decoder.decode_spec_free()?,
    }
    Ok(decoder.finish(specs.is_some(), threshold_ms))
}

/// Per-replay decode state. Mutates a single `WowsDecoder` instance instead of
/// shuffling 11 `&mut` locals through a free function.
struct WowsDecoder<'a> {
    replay: &'a ReplayFile,
    resolver: &'a NameResolver,
    samples: Vec<analysis::NetStat>,
    server_ticks: Vec<f32>,
    headers: Vec<analysis::PacketHeader>,
    events: Vec<GameEvent>,
    arena_id: Option<i64>,
    battle_start_clock: Option<f32>,
    realm: Option<String>,
    corrupt_packet_clocks: Vec<f32>,
    death_effects: HashMap<EntityId, String>,
    camos: HashMap<EntityId, String>,
    visibility: HashMap<EntityId, i32>,
    ships: HashMap<EntityId, ShipInfo>,
}

impl<'a> WowsDecoder<'a> {
    fn new(replay: &'a ReplayFile, resolver: &'a NameResolver) -> Self {
        Self {
            replay,
            resolver,
            samples: Vec::new(),
            server_ticks: Vec::new(),
            headers: Vec::new(),
            events: Vec::new(),
            arena_id: None,
            battle_start_clock: None,
            realm: None,
            corrupt_packet_clocks: Vec::new(),
            death_effects: HashMap::new(),
            camos: HashMap::new(),
            visibility: HashMap::new(),
            ships: HashMap::new(),
        }
    }

    fn decode_spec_free(&mut self) -> Result<(), String> {
        let mut last_valid_clock: f32 = 0.0;
        for packet in RawPacketIterator::new(&self.replay.packet_data) {
            let packet = packet.map_err(|e| format!("packet parse: {e:?}"))?;
            let clock = packet.clock.seconds();
            if !(0.0..MAX_REPLAY_CLOCK_S).contains(&clock) {
                self.corrupt_packet_clocks.push(last_valid_clock);
                continue;
            }
            last_valid_clock = clock;
            self.headers.push(analysis::PacketHeader {
                clock,
                is_client_side: is_client_side_packet(packet.packet_type),
            });
            match packet.packet_type {
                PacketTypeId::PlayerNetStats => {
                    if let Some(ns) = PlayerNetStatsPacket::from_payload(packet.payload) {
                        self.samples.push(net_stat(clock, &ns));
                    }
                }
                PacketTypeId::ServerTick => self.server_ticks.push(clock),
                _ => {}
            }
        }
        self.realm = replay::detect_realm(&self.replay.packet_data).map(str::to_string);
        Ok(())
    }

    fn decode_with_specs(&mut self, specs: &[EntitySpec]) -> Result<(), String> {
        let version = Version::from_client_exe(&self.replay.meta.clientVersionFromExe);
        let game_constants = GameConstants::defaults();
        let decoder = PacketDecoder::builder()
            .version(version)
            .battle_constants(game_constants.battle())
            .common_constants(game_constants.common())
            .ships_constants(game_constants.ships())
            .build();

        let mut parser = Parser::new(specs);
        let mut remaining = &self.replay.packet_data[..];
        let mut last_valid_clock: f32 = 0.0;
        while !remaining.is_empty() {
            let packet = parser.parse_packet(&mut remaining).map_err(|e| format!("packet parse: {e:?}"))?;
            let clock = packet.clock.seconds();
            if !(0.0..MAX_REPLAY_CLOCK_S).contains(&clock) {
                self.corrupt_packet_clocks.push(last_valid_clock);
                continue;
            }
            last_valid_clock = clock;
            self.headers.push(analysis::PacketHeader {
                clock,
                is_client_side: is_client_side_packet(packet.packet_type),
            });

            let decoded = decoder.decode(&packet);
            self.handle_decoded(&version, clock, &decoded.payload);
        }
        Ok(())
    }

    fn handle_decoded(&mut self, version: &Version, clock: f32, payload: &DecodedPacketPayload<'_, '_, '_>) {
        match payload {
            DecodedPacketPayload::PlayerNetStats(ns) => self.samples.push(net_stat(clock, ns)),
            DecodedPacketPayload::ServerTick(_) => self.server_ticks.push(clock),
            DecodedPacketPayload::OnArenaStateReceived { arena_id, player_states, bot_states, .. } => {
                if self.arena_id.is_none() {
                    self.arena_id = Some(*arena_id);
                }
                if self.realm.is_none() {
                    let self_realm = player_states
                        .iter()
                        .find(|p| p.username() == self.replay.meta.playerName)
                        .and_then(|p| p.realm());
                    self.realm = self_realm
                        .or_else(|| player_states.iter().find_map(|p| p.realm()))
                        .map(str::to_string);
                }
                for p in player_states.iter().chain(bot_states.iter()) {
                    let ship_param_id = p.ship_params_id();
                    self.ships.insert(
                        p.entity_id(),
                        ShipInfo {
                            player: p.username().to_string(),
                            ship_param_id: ship_param_id.map(|g| g.raw()),
                            ship_name: ship_param_id.and_then(|id| self.resolver.ship_name(id)),
                        },
                    );
                }
            }
            DecodedPacketPayload::EntityCreate(ec) => self.index_ship_config(version, &ec.props, ec.entity_id),
            DecodedPacketPayload::CellPlayerCreate(cpc) => self.index_ship_config(version, &cpc.props, cpc.entity_id),
            DecodedPacketPayload::Consumable { entity, consumable, .. } => {
                let ship = event_ship(&self.ships, &self.camos, *entity);
                self.events.push(GameEvent {
                    clock,
                    tick_offset: 0,
                    kind: EventKind::Consumable,
                    ships: vec![ship],
                    detail: consumable_name(consumable),
                    death_effect: None,
                });
            }
            DecodedPacketPayload::ShipDestroyed { killer, victim, cause } => {
                let victim_ship = event_ship(&self.ships, &self.camos, *victim);
                let killer_ship = event_ship(&self.ships, &self.camos, *killer);
                self.events.push(GameEvent {
                    clock,
                    tick_offset: 0,
                    kind: EventKind::Kill,
                    ships: vec![victim_ship, killer_ship],
                    detail: death_cause_name(cause),
                    death_effect: self.death_effects.get(killer).cloned(),
                });
            }
            DecodedPacketPayload::EntityProperty(ep) if ep.property == "battleStage" => {
                // BattleStage::Waiting (raw 0) fires when the pre-battle countdown ends.
                if self.battle_start_clock.is_none() && ep.value.as_i32() == Some(0) {
                    self.battle_start_clock = Some(clock);
                }
            }
            DecodedPacketPayload::EntityProperty(ep) if ep.property == "visibilityFlags" => {
                let new = ep.value.as_i32().unwrap_or(0);
                let was_visible = self.visibility.get(&ep.entity_id).map(|v| v & 1 != 0);
                if was_visible == Some(false) && new & 1 != 0 {
                    let ship = event_ship(&self.ships, &self.camos, ep.entity_id);
                    self.events.push(GameEvent {
                        clock,
                        tick_offset: 0,
                        kind: EventKind::Spotted,
                        ships: vec![ship],
                        detail: String::new(),
                        death_effect: None,
                    });
                }
                self.visibility.insert(ep.entity_id, new);
            }
            _ => {}
        }
    }

    fn index_ship_config(
        &mut self,
        version: &Version,
        props: &HashMap<&str, ArgValue>,
        entity_id: EntityId,
    ) {
        let Some(ArgValue::Blob(blob)) = props.get("shipConfig") else {
            return;
        };
        let Ok(config) = parse_ship_config(blob, version) else {
            return;
        };
        let effect = self.resolver.death_effect_name(config.exteriors()).or_else(|| {
            config
                .exteriors()
                .iter()
                .find_map(|id| replay::death_effect_name(id.raw()))
                .map(str::to_string)
        });
        if let Some(effect) = effect {
            self.death_effects.insert(entity_id, effect);
        }
        if let Some(camo) = self.resolver.camo_name(config.exteriors()) {
            self.camos.insert(entity_id, camo);
        }
    }

    fn finish(self, entity_defs_loaded: bool, threshold_ms: Option<u32>) -> analysis::AnalysisResult {
        let build = replay::build_from_client_version(&self.replay.meta.clientVersionFromExe);
        let meta = meta_out(&self.replay.meta, self.arena_id, build, self.realm.clone());
        analysis::build_analysis(
            meta,
            self.samples,
            self.server_ticks,
            self.headers,
            self.events,
            entity_defs_loaded,
            self.resolver.loaded(),
            self.battle_start_clock,
            self.corrupt_packet_clocks,
            threshold_ms
                .map(|ms| analysis::SpikeThresholds { min_gap_s: ms as f32 / 1000.0 })
                .unwrap_or_default(),
        )
    }
}

fn meta_out(
    meta: &ReplayMeta,
    arena_id: Option<i64>,
    client_build: Option<u32>,
    region: Option<String>,
) -> analysis::ReplayMetaOut {
    analysis::ReplayMetaOut {
        map: meta.mapName.clone(),
        map_display_name: meta.mapDisplayName.clone(),
        date_time: meta.dateTime.clone(),
        player_name: meta.playerName.clone(),
        player_vehicle: meta.playerVehicle.clone(),
        client_version: meta.clientVersionFromExe.clone(),
        match_group: meta.matchGroup.clone(),
        game_type: meta.gameType.clone(),
        battle_duration_s: meta.battleDuration,
        replay_duration_s: meta.duration,
        players_per_team: meta.playersPerTeam,
        arena_id: arena_id.map(|a| a.to_string()),
        arena_id_hex: arena_id.map(|a| format!("{:016x}", a as u64)),
        client_build,
        region,
    }
}

fn net_stat(clock: f32, ns: &PlayerNetStatsPacket) -> analysis::NetStat {
    analysis::NetStat { clock, fps: ns.fps, ping: ns.ping, is_lagging: ns.is_lagging }
}

struct ShipInfo {
    player: String,
    ship_param_id: Option<u64>,
    ship_name: Option<String>,
}

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

/// Ship and camo display names from the rkyv GameParams blob + .mo catalog.
struct NameResolver {
    by_id: HashMap<GameParamId, Param>,
    catalog: Option<Catalog>,
}

impl NameResolver {
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
        let catalog = if mo_bytes.is_empty() { None } else { Catalog::parse(&mo_bytes[..]).ok() };
        Self { by_id, catalog }
    }

    fn loaded(&self) -> bool {
        !self.by_id.is_empty()
    }

    fn translate(&self, key: &str) -> Option<String> {
        let catalog = self.catalog.as_ref()?;
        let value = catalog.gettext(key);
        if value == key { None } else { Some(value.to_string()) }
    }

    fn ship_name(&self, id: GameParamId) -> Option<String> {
        let param = self.by_id.get(&id)?;
        self.translate(&format!("IDS_{}", param.index()))
    }

    fn camo_name(&self, exteriors: &[GameParamId]) -> Option<String> {
        for id in exteriors {
            let Some(param) = self.by_id.get(id) else { continue };
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

    fn death_effect_name(&self, exteriors: &[GameParamId]) -> Option<String> {
        for id in exteriors {
            let Some(param) = self.by_id.get(id) else { continue };
            let is_death_effect = param
                .species()
                .and_then(|s| s.unknown())
                .map(|raw| raw == "ShipDestruction")
                .unwrap_or(false);
            if is_death_effect
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
            .ok_or_else(|| GameDataError::Io(std::io::Error::other(format!("missing bundled file: {path}"))))
    });
    let specs = parse_scripts(&loader).map_err(|e| format!("entity defs: {e}"))?;
    Ok(Some(specs))
}
