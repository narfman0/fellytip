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
//! | `dm/query_portals`              | List all portal trigger positions      |
//! | `dm/spawn_wildlife`             | Spawn a wildlife entity at a world position |
//! | `dm/list_settlements`           | Return all settlement names + world-space coords |
//! | `dm/spawn_raid`                 | Spawn an underground raid party directly        |
//! | `dm/give_gold`                  | Award a gold delta to a faction                 |

use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use fellytip_shared::{
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{EntityKind, Experience, GrowthStage, Health, WildlifeKind, WorldPosition},
    world::{
        civilization::Settlements,
        ecology::RegionId,
        faction::FactionId,
        population::{UNDERGROUND_RAID_PARTY_SIZE, WAR_PARTY_SIZE},
        zone::ZoneMembership,
    },
};
use uuid::Uuid;

use fellytip_game::plugins::{
    ai::{BattleHistory, FactionMember, FactionPopulationState, FactionRegistry, UndergroundPressure, WarPartyMember},
    ai::population::faction_npc_bundle,
    combat::{CombatParticipant, ExperienceReward},
    ecology::{EcologyState, WildlifeNpc},
    portal::PortalTrigger,
};

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
        faction_npc_bundle(FactionId(faction.as_str().into()), pos, level),
        // Mark as instant adult so the NPC is immediately combat-ready.
        GrowthStage(1.0),
    )).id();

    tracing::info!(faction = %faction, x, y, z, entity = ?entity, "DM spawned NPC");
    Ok(json!({ "ok": true, "entity": entity.to_bits() }))
}

// ── dm/spawn_wildlife ─────────────────────────────────────────────────────────

/// Spawn a wildlife entity at the given world position.
///
/// Params: `{ x: f64, y: f64, z?: f64, kind?: "Bison"|"Horse"|"Dog" }`
/// Defaults: `z = 10.0`, `kind = "Horse"`.
///
/// Returns `{ ok: true, entity: u64 }`.
pub fn dm_spawn_wildlife(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let x: f64 = require(&params, "x")?;
    let y: f64 = require(&params, "y")?;
    let z: f64 = opt(&params, "z").unwrap_or(10.0);
    let kind_str: String = opt(&params, "kind").unwrap_or_else(|| "Horse".to_string());

    let wildlife_kind = match kind_str.as_str() {
        "Bison" => WildlifeKind::Bison,
        "Dog"   => WildlifeKind::Dog,
        "Horse" => WildlifeKind::Horse,
        other   => return Err(BrpError::internal(format!(
            "unknown wildlife kind `{other}`; valid values: Bison, Horse, Dog"
        ))),
    };

    #[allow(clippy::cast_possible_truncation)]
    let pos = WorldPosition { x: x as f32, y: y as f32, z: z as f32 };

    let entity = world.spawn((
        pos,
        Health { current: 15, max: 15 },
        CombatParticipant {
            id: CombatantId(Uuid::new_v4()),
            interrupt_stack: InterruptStack::default(),
            class: CharacterClass::Rogue,
            level: 1,
            armor_class: 10,
            strength: 8,
            dexterity: 12,
            constitution: 10,
            intelligence: 2,
            wisdom: 10,
            charisma: 5,
        },
        ExperienceReward(25),
        WildlifeNpc { region: RegionId("dm_spawned".into()) },
        EntityKind::Wildlife,
        wildlife_kind,
    )).id();

    tracing::info!(kind = %kind_str, x, y, z, entity = ?entity, "DM spawned wildlife");
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

// ── dm/query_portals ──────────────────────────────────────────────────────────

/// Query all `PortalTrigger` entities and return their world positions and zone
/// membership.
///
/// Params: `{}`
/// Returns a JSON array of `{ "portal_id": u32, "x": f32, "y": f32, "z": f32,
/// "zone_id": u32 }`.
pub fn dm_query_portals(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let mut query = world.query::<(&PortalTrigger, &WorldPosition, Option<&ZoneMembership>)>();
    let portals: Vec<Value> = query
        .iter(world)
        .map(|(trigger, pos, zone)| {
            json!({
                "portal_id": trigger.portal_id,
                "x": pos.x,
                "y": pos.y,
                "z": pos.z,
                "zone_id": zone.map(|z| z.0.0).unwrap_or(0u32),
            })
        })
        .collect();
    tracing::debug!(count = portals.len(), "DM queried portals");
    Ok(json!(portals))
}

// ── dm/list_settlements ───────────────────────────────────────────────────────

/// List all generated settlements with their world-space coordinates.
///
/// Params: `{}` (none required); optional `{ "kind": "Capital" | "Town" }` to filter.
/// Returns a JSON array of `{ "name": str, "kind": str, "x": f32, "y": f32, "z": f32 }`.
/// `x`/`y` are already in Bevy world-space (tile_x − MAP_HALF_WIDTH + 0.5).
pub fn dm_list_settlements(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    use fellytip_shared::world::civilization::SettlementKind;
    use fellytip_shared::world::map::{MAP_HALF_WIDTH, MAP_HALF_HEIGHT};

    let kind_filter: Option<String> = opt(&params, "kind");

    let settlements = world
        .get_resource::<Settlements>()
        .ok_or_else(|| BrpError::internal("Settlements resource not found"))?;

    let list: Vec<Value> = settlements
        .0
        .iter()
        .filter(|s| {
            kind_filter.as_deref().is_none_or(|k| {
                let label = match s.kind {
                    SettlementKind::Capital => "Capital",
                    SettlementKind::Town    => "Town",
                    _                        => "Other",
                };
                label.eq_ignore_ascii_case(k)
            })
        })
        .map(|s| {
            let world_x = s.x - MAP_HALF_WIDTH as f32;
            let world_y = s.y - MAP_HALF_HEIGHT as f32;
            let kind_str = match s.kind {
                SettlementKind::Capital => "Capital",
                SettlementKind::Town    => "Town",
                _                        => "Other",
            };
            json!({
                "name":  s.name,
                "kind":  kind_str,
                "x":     world_x,
                "y":     world_y,
                "z":     s.z,
            })
        })
        .collect();

    tracing::debug!(count = list.len(), "DM listed settlements");
    Ok(json!(list))
}

// ── dm/move_entity ────────────────────────────────────────────────────────────

/// Path any entity (PC or NPC) to a world-space target using A*.
///
/// Params: `{ entity: u64, x: f32, y: f32, z: f32 }`
///
/// Inserts `NavPath` + `NavigationGoal` on the entity; the `follow_navigation_goal`
/// system drives movement from there. Returns `{ ok: true, waypoints: N }`.
pub fn dm_move_entity(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let entity_bits: u64 = require(&params, "entity")?;
    let x: f32 = require(&params, "x")?;
    let y: f32 = require(&params, "y")?;
    let z: f32 = require(&params, "z")?;

    let entity = Entity::from_bits(entity_bits);

    let from = world.get::<WorldPosition>(entity)
        .ok_or_else(|| BrpError::internal("entity has no WorldPosition"))
        .map(|p| (p.x, p.y))?;

    let (nav_path, goal) = {
        let nav = world.get_resource::<fellytip_game::plugins::nav::NavGrid>()
            .ok_or_else(|| BrpError::internal("NavGrid not ready"))?;
        fellytip_game::plugins::nav::compute_nav_path(nav, from, (x, y, z))
            .ok_or_else(|| BrpError::internal("no path found to destination"))?
    };

    let n = nav_path.waypoints.len();
    world.entity_mut(entity).insert((nav_path, goal));

    tracing::info!(?entity, x, y, z, waypoints = n, "DM set entity destination");
    Ok(json!({ "ok": true, "waypoints": n }))
}

// ── dm/spawn_raid ─────────────────────────────────────────────────────────────

/// Spawn an underground raid party directly, bypassing pressure buildup.
///
/// Params: `{ faction: string, target_faction: string }`
/// Returns `{ ok: true, raiders_spawned: usize }`.
pub fn dm_spawn_raid(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let faction_id: String      = require(&params, "faction")?;
    let target_id: String       = require(&params, "target_faction")?;

    let attacker_fid = FactionId(faction_id.as_str().into());

    let target_pos = {
        let pop = world.resource::<FactionPopulationState>();
        pop.settlements.values()
            .find(|s| s.faction_id.0.as_str() == target_id)
            .map(|s| (s.settlement_id, s.home_x, s.home_y))
    };
    let (target_uuid, tx, ty) = target_pos
        .ok_or_else(|| BrpError::internal(format!("no settlement found for faction `{target_id}`")))?;

    let mut raiders_spawned = 0usize;
    for _ in 0..UNDERGROUND_RAID_PARTY_SIZE {
        let pos = WorldPosition { x: 0.0, y: 0.0, z: -50.0 };
        let entity = world.spawn((
            faction_npc_bundle(attacker_fid.clone(), pos, 1),
            GrowthStage(1.0),
        )).id();
        world.entity_mut(entity).insert(WarPartyMember {
            target_settlement_id: target_uuid,
            target_x: tx,
            target_y: ty,
            attacker_faction: attacker_fid.clone(),
            player_target: None,
            current_zone: fellytip_shared::world::zone::OVERWORLD_ZONE,
            zone_route: Vec::new(),
        });
        raiders_spawned += 1;
    }

    tracing::info!(
        faction   = %faction_id,
        target    = %target_id,
        spawned   = raiders_spawned,
        "DM spawned raid party"
    );
    Ok(json!({ "ok": true, "raiders_spawned": raiders_spawned }))
}

// ── dm/give_gold ──────────────────────────────────────────────────────────────

/// Award a gold delta to a faction (delta-based, clamped to >= 0).
///
/// Params: `{ faction_id: string, amount: f32 }`
/// Returns `{ ok: true, new_gold: f32 }`.
pub fn dm_give_gold(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let faction_id: String = require(&params, "faction_id")?;
    let amount: f32        = require(&params, "amount")?;

    let mut registry = world.resource_mut::<FactionRegistry>();
    let faction = registry.factions.iter_mut()
        .find(|f| f.id.0.as_str() == faction_id)
        .ok_or_else(|| BrpError::internal(format!("faction `{faction_id}` not found")))?;

    faction.resources.gold = (faction.resources.gold + amount).max(0.0);
    let new_gold = faction.resources.gold;

    tracing::info!(faction = %faction_id, amount, new_gold, "DM gave gold to faction");
    Ok(json!({ "ok": true, "new_gold": new_gold }))
}

/// Award XP directly to a player entity (identified by having `Experience`).
///
/// Params: `{ amount: u32 }`
/// Returns `{ ok: true, xp: u32, level: u32, leveled_up: bool }`.
pub fn dm_give_xp(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let amount: u32 = require(&params, "amount")?;

    let mut q = world.query::<&mut Experience>();
    let mut exp = q.iter_mut(world).next()
        .ok_or_else(|| BrpError::internal("no Experience entity found"))?;

    let old_level = exp.level;
    exp.xp += amount;
    while exp.xp >= exp.xp_to_next {
        exp.xp -= exp.xp_to_next;
        exp.level += 1;
        exp.xp_to_next = Experience::xp_to_next_level(exp.level);
    }
    let leveled_up = exp.level > old_level;
    let (xp, level) = (exp.xp, exp.level);

    tracing::info!(amount, level, xp, leveled_up, "DM gave XP");
    Ok(json!({ "ok": true, "xp": xp, "level": level, "leveled_up": leveled_up }))
}
