//! DM (Dungeon Master) BRP methods for live world manipulation.
//!
//! Registered on `RemotePlugin` in `server/main.rs` so the `worldwatch` tool
//! can call them at runtime for balance testing, debugging, and emergency
//! world control.
//!
//! All handlers run with exclusive `World` access — safe for spawns and
//! resource mutation without conflict.
//!
//! # Methods
//!
//! | Method                  | Action                                         |
//! |-------------------------|------------------------------------------------|
//! | `dm/spawn_npc`          | Spawn a full FactionNpc at a world position    |
//! | `dm/kill`               | Despawn any entity by ID                       |
//! | `dm/teleport`           | Move any entity to a new world position        |
//! | `dm/set_faction`        | Override a faction's food / gold / military    |
//! | `dm/trigger_war_party`  | Immediately form a war party for a faction     |
//! | `dm/set_ecology`        | Override prey / predator counts in a region    |
//! | `dm/battle_history`     | Read the rolling battle record history         |
//! | `dm/clear_battle_history` | Drop every queued BattleRecord (test helper) |
//! | `dm/underground_pressure` | Read the underground pressure resource snapshot |
//! | `dm/force_underground_pressure` | Force pressure score to 1.0 for tests |

use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use uuid::Uuid;

use fellytip_shared::{
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{EntityKind, GrowthStage, Health, WorldPosition},
    world::{
        faction::FactionId,
        population::WAR_PARTY_SIZE,
    },
};

use crate::plugins::{
    ai::{BattleHistory, CurrentGoal, FactionMember, FactionNpcRank, FactionPopulationState, FactionRegistry, HomePosition, UndergroundPressure, WarPartyMember},
    combat::{CombatParticipant, ExperienceReward},
    ecology::EcologyState,
};
use fellytip_shared::world::faction::NpcRank;

// ── Parameter helpers ─────────────────────────────────────────────────────────

/// Extract a required field from JSON params, returning a `BrpError` if absent
/// or if deserialization fails.
fn require<T: DeserializeOwned>(params: &Option<Value>, key: &str) -> Result<T, BrpError> {
    let v = params
        .as_ref()
        .and_then(|p| p.get(key))
        .ok_or_else(|| BrpError::internal(format!("missing required param `{key}`")))?;
    serde_json::from_value(v.clone())
        .map_err(|e| BrpError::internal(format!("invalid param `{key}`: {e}")))
}

/// Extract an optional field from JSON params, returning `None` if absent or
/// if deserialization fails silently.
fn opt<T: DeserializeOwned>(params: &Option<Value>, key: &str) -> Option<T> {
    params.as_ref()?.get(key).and_then(|v| serde_json::from_value(v.clone()).ok())
}

// ── dm/spawn_npc ──────────────────────────────────────────────────────────────

/// Spawn a full-stat faction NPC at the given world position.
///
/// Params: `{ faction: string, x: f32, y: f32, z: f32, level?: u32 }`
///
/// The interest manager will update the `Replicate` target within 1 s.
/// Returns `{ ok: true, entity: u64 }`.
pub fn dm_spawn_npc(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let faction: String = require(&params, "faction")?;
    let x: f32 = require(&params, "x")?;
    let y: f32 = require(&params, "y")?;
    let z: f32 = require(&params, "z")?;
    let level: u32 = opt(&params, "level").unwrap_or(1);

    let pos = WorldPosition { x, y, z };
    let entity = world.spawn((
        pos.clone(),
        Health { current: 20, max: 20 },
        CombatParticipant {
            id: CombatantId(Uuid::new_v4()),
            interrupt_stack: InterruptStack::default(),
            class: CharacterClass::Warrior,
            level,
            armor_class: 11,
            strength: 10,
            dexterity: 10,
            constitution: 10,
        },
        ExperienceReward(50),
        FactionMember(FactionId(faction.as_str().into())),
        FactionNpcRank(NpcRank::Grunt),
        CurrentGoal(None),
        HomePosition(pos),
        EntityKind::FactionNpc,
        GrowthStage(1.0),
    )).id();

    tracing::info!(faction = %faction, x, y, z, entity = ?entity, "DM spawned NPC");
    Ok(json!({ "ok": true, "entity": entity.to_bits() }))
}

// ── dm/kill ───────────────────────────────────────────────────────────────────

/// Despawn any entity by its bit-packed ID.
///
/// Params: `{ entity: u64 }` — use the `entity` field from `world.query`.
/// Returns `{ ok: true }`.
pub fn dm_kill(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let bits: u64 = require(&params, "entity")?;
    let entity = Entity::from_bits(bits);
    world.get_entity_mut(entity)
        .map_err(|_| BrpError::entity_not_found(entity))?
        .despawn();
    tracing::info!(entity = bits, "DM killed entity");
    Ok(json!({ "ok": true }))
}

// ── dm/teleport ───────────────────────────────────────────────────────────────

/// Move any entity to a new world position.
///
/// Params: `{ entity: u64, x: f32, y: f32, z: f32 }`
/// Returns `{ ok: true }`.
pub fn dm_teleport(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let bits: u64 = require(&params, "entity")?;
    let x: f32 = require(&params, "x")?;
    let y: f32 = require(&params, "y")?;
    let z: f32 = require(&params, "z")?;
    let entity = Entity::from_bits(bits);
    world.get_entity_mut(entity)
        .map_err(|_| BrpError::entity_not_found(entity))?
        .insert(WorldPosition { x, y, z });
    tracing::info!(entity = bits, x, y, z, "DM teleported entity");
    Ok(json!({ "ok": true }))
}

// ── dm/set_faction ────────────────────────────────────────────────────────────

/// Override a faction's resource values.
///
/// Params: `{ faction_id: string, food?: f32, gold?: f32, military?: f32 }`
/// Any omitted field is left unchanged.
/// Returns `{ ok: true }`.
pub fn dm_set_faction(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let faction_id: String  = require(&params, "faction_id")?;
    let food: Option<f32>     = opt(&params, "food");
    let gold: Option<f32>     = opt(&params, "gold");
    let military: Option<f32> = opt(&params, "military");

    let mut registry = world.resource_mut::<FactionRegistry>();
    let faction = registry.factions.iter_mut()
        .find(|f| f.id.0.as_str() == faction_id)
        .ok_or_else(|| BrpError::internal(format!("faction `{faction_id}` not found")))?;

    if let Some(v) = food     { faction.resources.food              = v.max(0.0); }
    if let Some(v) = gold     { faction.resources.gold              = v.max(0.0); }
    if let Some(v) = military { faction.resources.military_strength = v.clamp(0.0, 100.0); }

    tracing::info!(faction = %faction_id, ?food, ?gold, ?military, "DM updated faction resources");
    Ok(json!({ "ok": true }))
}

// ── dm/trigger_war_party ─────────────────────────────────────────────────────

/// Immediately form a war party for the attacker faction, targeting the nearest
/// settlement belonging to the target faction.
///
/// Params: `{ attacker_faction: string, target_faction: string }`
/// Returns `{ ok: true, warriors_tagged: usize }`.
pub fn dm_trigger_war_party(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let attacker_id: String = require(&params, "attacker_faction")?;
    let target_id: String   = require(&params, "target_faction")?;

    let attacker_fid = FactionId(attacker_id.as_str().into());

    // Find the target settlement position from FactionPopulationState.
    let target_pos = {
        let pop = world.resource::<FactionPopulationState>();
        pop.settlements.values()
            .find(|s| s.faction_id.0.as_str() == target_id)
            .map(|s| (s.settlement_id, s.home_x, s.home_y))
    };
    let (target_uuid, tx, ty) = target_pos
        .ok_or_else(|| BrpError::internal(format!("no settlement found for faction `{target_id}`")))?;

    // Collect eligible adult NPCs from the attacker faction.
    let candidates: Vec<Entity> = {
        let mut query = world.query_filtered::<
            (Entity, &FactionMember, Option<&GrowthStage>),
            Without<WarPartyMember>,
        >();
        query.iter(world)
            .filter(|(_, member, growth)| {
                member.0 == attacker_fid
                    && growth.map(|g| g.0 >= 1.0).unwrap_or(true)
            })
            .map(|(e, _, _)| e)
            .take(WAR_PARTY_SIZE as usize)
            .collect()
    };

    if candidates.is_empty() {
        return Err(BrpError::internal(format!(
            "no eligible adult NPCs in faction `{attacker_id}`"
        )));
    }

    let count = candidates.len();
    for entity in candidates {
        world.entity_mut(entity).insert(WarPartyMember {
            target_settlement_id: target_uuid,
            target_x: tx,
            target_y: ty,
            attacker_faction: attacker_fid.clone(),
            player_target: None,
            current_zone: fellytip_shared::world::zone::OVERWORLD_ZONE,
            zone_route: Vec::new(),
        });
    }

    tracing::info!(
        attacker = %attacker_id,
        target   = %target_id,
        warriors = count,
        "DM triggered war party"
    );
    Ok(json!({ "ok": true, "warriors_tagged": count }))
}

// ── dm/set_ecology ────────────────────────────────────────────────────────────

/// Override prey and/or predator population counts for an ecology region.
///
/// Params: `{ region: string, prey?: f64, predator?: f64 }`
/// Region IDs follow the pattern `macro_<rx>_<ry>` (e.g. `macro_0_0`).
/// Returns `{ ok: true }`.
pub fn dm_set_ecology(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let region: String    = require(&params, "region")?;
    let prey: Option<f64> = opt(&params, "prey");
    let pred: Option<f64> = opt(&params, "predator");

    let mut ecology = world.resource_mut::<EcologyState>();
    let eco = ecology.regions.iter_mut()
        .find(|r| r.region.0.as_str() == region)
        .ok_or_else(|| BrpError::internal(format!("region `{region}` not found")))?;

    if let Some(v) = prey { eco.prey.count     = v.max(0.0); }
    if let Some(v) = pred { eco.predator.count = v.max(0.0); }

    tracing::info!(region = %region, "DM updated ecology (prey={prey:?}, predator={pred:?})");
    Ok(json!({ "ok": true }))
}

// ── dm/battle_history ─────────────────────────────────────────────────────────

/// Return recent `BattleRecord` entries newest-first.
///
/// Params: `{ limit?: u32 }` — defaults to 100 (the resource cap).
/// Returns a JSON array of `{ winner_faction, loser_faction, target_settlement_id,
/// tick, attacker_casualties, defender_casualties }`.
pub fn dm_battle_history(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let limit = params
        .as_ref()
        .and_then(|p| p.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;
    let history = world.resource::<BattleHistory>();
    let records: Vec<_> = history.records.iter().rev().take(limit).collect();
    serde_json::to_value(&records)
        .map_err(|e| BrpError::internal(format!("serialize battle history: {e}")))
}

// ── dm/clear_battle_history ───────────────────────────────────────────────────

/// Drop every queued `BattleRecord` so subsequent `dm/battle_history`
/// calls return only battles that resolved after the clear.
///
/// Useful for end-to-end tests that trigger a war party and want the
/// first record returned to be the one they just caused, not a pre-sim
/// battle produced by `--history-warp-ticks`.
///
/// Params: `{}`  Returns `{ ok: true, cleared: usize }`.
pub fn dm_clear_battle_history(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let mut history = world.resource_mut::<BattleHistory>();
    let cleared = history.records.len();
    history.records.clear();
    tracing::info!(cleared, "DM cleared battle history");
    Ok(json!({ "ok": true, "cleared": cleared }))
}

// ── dm/underground_pressure ───────────────────────────────────────────────────

/// Return the current underground pressure resource snapshot.
///
/// Returns `{ score: f32, last_raid_tick: u64 }`.
pub fn dm_underground_pressure(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let pressure = world.resource::<UndergroundPressure>();
    Ok(json!({
        "score": pressure.score,
        "last_raid_tick": pressure.last_raid_tick,
    }))
}

// ── dm/force_underground_pressure ─────────────────────────────────────────────

/// Force the underground pressure score to 1.0 immediately so subsequent
/// systems trigger the raid path on the next tick. Intended for ralph e2e
/// scenarios that don't want to wait 10+ slow ticks for organic buildup.
///
/// Params: `{}`  Returns `{ ok: true }`.
pub fn dm_force_underground_pressure(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let mut pressure = world.resource_mut::<UndergroundPressure>();
    pressure.score = 1.0;
    tracing::info!("DM forced underground pressure to 1.0");
    Ok(json!({ "ok": true }))
}
