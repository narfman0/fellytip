//! Zone-aware NPC movement: wander_npcs, march_war_parties, war_party_separation,
//! advance_zone_parties, update_war_party_player_targets, sync_player_standings.

use bevy::ecs::message::MessageWriter;
use bevy::prelude::*;
use fellytip_shared::{
    components::{NavPath, NavReplanTimer, NavigationGoal, PlayerStandings, WorldPosition},
    protocol::BattleStartMsg,
    world::{
        faction::PlayerReputationMap,
        population::{BATTLE_RADIUS, MARCH_SPEED},
        story::{StoryEvent, StoryEventKind, WriteStoryEvent},
    },
};
use std::collections::HashMap;
use uuid::Uuid;

use crate::plugins::combat::CombatParticipant;
use crate::plugins::interest::{effective_zone, ChunkTemperature, SimTier};
use crate::plugins::nav::{world_to_nav, nav_to_world, FlowField, NavGrid, ZoneNavGrids};
use crate::plugins::perf::AdaptiveScheduler;
use crate::plugins::world_sim::WorldSimTick;

use super::{
    ActiveBattle, FactionAlertState, FactionMember, FactionPopulationState, FactionRegistry,
    WarPartyMember,
};

/// Movement speed per tick for wandering NPCs (in world units).
const WANDER_STEP: f32 = 0.15;
/// Movement speed per tick for Frozen NPCs linear-marching to home.
const FROZEN_WANDER_STEP: f32 = 0.5;

/// Separation radius in world tiles — repulse members closer than this.
const SEPARATION_RADIUS: f32 = 1.5;
/// Repulsion gain: push apart by `(radius - dist) * gain` tiles.
const SEPARATION_GAIN: f32 = 0.5;
/// Circle radius for Frozen zone formation offsets.
const FROZEN_FORMATION_RADIUS: f32 = 1.5;

use super::HomePosition;

/// Move faction NPCs each world-sim tick using zone-gated A* pathfinding.
///
/// # Three-tier LOD behavior:
/// - **Hot** (chunks 0–2 from player): replan A* every 2 ticks, follow waypoints at full speed.
/// - **Warm** (chunks 3–8 from player): replan every 8 ticks, follow at 0.25× speed.
/// - **Frozen** (>8 chunks from player): skip A*, linear march toward home at 0.05× speed.
///
/// # Alert behavior (faction consequences)
/// When an NPC's faction is in `FactionAlertLevel::Alerted` state (set by `update_faction_alerts`
/// after a `BattleEndMsg`), the NPC patrols with:
/// - Double wander radius (7.0 tiles instead of 3.5)
/// - 1.5× movement speed
///
/// War party members are excluded — they march under `march_war_parties` instead.
#[allow(clippy::type_complexity)]
pub fn wander_npcs(
    mut query: Query<
        (
            Entity,
            &mut WorldPosition,
            &HomePosition,
            &FactionMember,
            &mut NavPath,
            &mut NavReplanTimer,
            Option<&fellytip_shared::world::zone::ZoneMembership>,
        ),
        (With<FactionMember>, Without<WarPartyMember>, Without<NavigationGoal>),
    >,
    temp: Res<ChunkTemperature>,
    scheduler: Res<AdaptiveScheduler>,
    nav: Option<Res<NavGrid>>,
    zone_nav_grids: Option<Res<ZoneNavGrids>>,
    tick: Res<WorldSimTick>,
    alerts: Res<FactionAlertState>,
) {
    let Some(nav) = nav else { return };

    for (entity, mut pos, home, faction_member, mut nav_path, mut replan_timer, zone_membership) in
        &mut query
    {
        let zone = effective_zone(&pos, &temp, scheduler.level);
        let zone_speed = zone.speed();
        let alerted = alerts.is_alerted(&faction_member.0);

        // Alerted NPCs move 50% faster during patrol.
        let speed_mul = if alerted { 1.5 } else { 1.0 };

        // Frozen: skip A*, linear march toward home position.
        if zone == SimTier::Frozen {
            let dx = home.0.x - pos.x;
            let dy = home.0.y - pos.y;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq > 0.01 {
                let dist = dist_sq.sqrt();
                pos.x += (dx / dist) * FROZEN_WANDER_STEP * zone_speed * speed_mul;
                pos.y += (dy / dist) * FROZEN_WANDER_STEP * zone_speed * speed_mul;
            }
            continue;
        }

        // Determine replan cadence from zone.
        let replan_every = if zone == SimTier::Hot { 2u32 } else { 8u32 };

        replan_timer.0 = replan_timer.0.saturating_add(1);

        // Replan A* when timer expires or path is exhausted.
        if replan_timer.0 >= replan_every || nav_path.is_complete() {
            replan_timer.0 = 0;

            // Wander radius: doubled when the faction is on alert.
            let patrol_radius = if alerted { 7.0_f32 } else { 3.5_f32 };

            // Wander goal: pick a position within `patrol_radius` tiles of home using entity seed.
            #[allow(clippy::cast_precision_loss)]
            let entity_seed = entity.to_bits() as f32 * 0.000_013_7;
            #[allow(clippy::cast_precision_loss)]
            let angle = (entity_seed + tick.0 as f32 * 0.07).sin() * std::f32::consts::TAU;
            let goal_x = home.0.x + angle.cos() * patrol_radius;
            let goal_y = home.0.y + angle.sin() * patrol_radius;

            // Zone-aware pathfinding: use the per-zone nav grid when the NPC
            // is inside a non-overworld zone (BuildingFloor, Dungeon, etc.).
            let npc_zone_id = zone_membership
                .map(|z| z.0)
                .unwrap_or(fellytip_shared::world::zone::OVERWORLD_ZONE);

            let planned = if npc_zone_id != fellytip_shared::world::zone::OVERWORLD_ZONE {
                // NPC is inside a zone interior — use zone tile-local A*.
                // Zone interior tiles are unit-sized; pos is already in tile units
                // relative to zone origin, so we cast directly to tile indices.
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let start_tile = (pos.x.max(0.0) as usize, pos.y.max(0.0) as usize);
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let goal_tile = (goal_x.max(0.0) as usize, goal_y.max(0.0) as usize);

                zone_nav_grids
                    .as_deref()
                    .and_then(|zg| zg.zone_astar(npc_zone_id, start_tile, goal_tile))
            } else {
                // Overworld NPC — existing 256×256 nav grid.
                let start = world_to_nav(pos.x, pos.y);
                let goal = world_to_nav(goal_x, goal_y);
                nav.astar(start, goal)
            };

            if let Some(waypoints) = planned {
                *nav_path = NavPath { waypoints, waypoint_index: 0 };
            }
        }

        // Follow current path: advance toward next waypoint.
        if let Some((wx, wy)) = nav_path.next_waypoint() {
            // Waypoint coordinate space matches the planning space used above.
            let (target_x, target_y) =
                if zone_membership.is_some_and(|z| z.0 != fellytip_shared::world::zone::OVERWORLD_ZONE) {
                    // Zone interior: tile coords are world units directly.
                    (wx as f32, wy as f32)
                } else {
                    nav_to_world(wx as usize, wy as usize)
                };
            let dx = target_x - pos.x;
            let dy = target_y - pos.y;
            let dist_sq = dx * dx + dy * dy;
            let step = WANDER_STEP * zone_speed * speed_mul;
            if dist_sq <= step * step {
                pos.x = target_x;
                pos.y = target_y;
                nav_path.waypoint_index += 1;
            } else {
                let dist = dist_sq.sqrt();
                pos.x += (dx / dist) * step;
                pos.y += (dy / dist) * step;
            }
        }
    }
}

/// Move war-party NPCs toward their target using flow-field pathfinding (Hot/Warm)
/// or linear march (Frozen). Spawn `ActiveBattle` when they arrive.
///
/// # Zone behavior:
/// - **Hot/Warm**: sample the cached flow field at the entity's nav cell,
///   apply direction × MARCH_SPEED × zone_speed.
/// - **Frozen**: keep existing linear march (unchanged behavior, macro-correct).
#[allow(clippy::too_many_arguments)]
pub fn march_war_parties(
    mut warriors: Query<(&WarPartyMember, &mut WorldPosition)>,
    battles: Query<&ActiveBattle>,
    pop: Res<FactionPopulationState>,
    temp: Res<ChunkTemperature>,
    scheduler: Res<AdaptiveScheduler>,
    flow_field: Res<FlowField>,
    mut commands: Commands,
    mut battle_start: MessageWriter<BattleStartMsg>,
) {
    // Dedupe ActiveBattle spawns within a single system run: multiple war-party
    // members can arrive on the same tick, and the `battles` query doesn't yet
    // see entities queued on `commands`, so without this set every arriving
    // member would spawn its own ActiveBattle (and BattleRecord on resolution).
    let mut spawned_this_tick: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    for (war_member, mut pos) in &mut warriors {
        // Guard: only advance surface-world war parties here. Underground or
        // cross-world parties are handled by `advance_zone_parties` until they
        // reach OVERWORLD_ZONE, at which point current_zone == OVERWORLD_ZONE
        // and the WORLD_SURFACE check passes.
        if war_member.current_zone != fellytip_shared::world::zone::OVERWORLD_ZONE {
            continue;
        }
        let zone = effective_zone(&pos, &temp, scheduler.level);
        let speed = zone.speed();
        let dx = war_member.target_x - pos.x;
        let dy = war_member.target_y - pos.y;
        let dist = (dx * dx + dy * dy).sqrt();

        if dist > 0.01 {
            if zone == SimTier::Frozen {
                // Frozen: linear march (macro-correct, skips expensive flow-field).
                let step = (MARCH_SPEED * speed / dist).min(1.0);
                pos.x += dx * step;
                pos.y += dy * step;
            } else {
                // Hot/Warm: use flow field direction vector.
                let (nx, ny) = world_to_nav(pos.x, pos.y);
                let dir = flow_field
                    .get(war_member.target_x, war_member.target_y)
                    .map(|ff| ff.dir_at(nx, ny))
                    .unwrap_or((0, 0));

                if dir != (0, 0) {
                    let move_x = dir.0 as f32 * MARCH_SPEED * speed;
                    let move_y = dir.1 as f32 * MARCH_SPEED * speed;
                    // Clamp so we don't overshoot the target.
                    let would_overshoot_x = move_x.abs() > dx.abs();
                    let would_overshoot_y = move_y.abs() > dy.abs();
                    pos.x += if would_overshoot_x { dx } else { move_x };
                    pos.y += if would_overshoot_y { dy } else { move_y };
                } else {
                    // No flow field entry (at target or unreachable): linear fallback.
                    let step = (MARCH_SPEED * speed / dist).min(1.0);
                    pos.x += dx * step;
                    pos.y += dy * step;
                }
            }
        }

        // Check if arrived and no battle already active for this settlement.
        if dist <= BATTLE_RADIUS {
            let already_active = battles.iter().any(|b| b.settlement_id == war_member.target_settlement_id)
                || spawned_this_tick.contains(&war_member.target_settlement_id);
            if !already_active {
                // Look up the defender faction from population state.
                let Some(target_pop) = pop.settlements.get(&war_member.target_settlement_id) else { continue };
                let defender_faction = target_pop.faction_id.clone();

                // Attacker faction is carried directly on the WarPartyMember component,
                // set either by `check_war_party_formation` or `dm/trigger_war_party`.
                let attacker_faction = war_member.attacker_faction.clone();
                spawned_this_tick.insert(war_member.target_settlement_id);

                let battle_entity = commands.spawn(ActiveBattle {
                    settlement_id: war_member.target_settlement_id,
                    attacker_faction: attacker_faction.clone(),
                    defender_faction: defender_faction.clone(),
                    battle_x: war_member.target_x,
                    battle_y: war_member.target_y,
                    attacker_casualties: 0,
                    defender_casualties: 0,
                    round_acc: 0.0,
                }).id();

                battle_start.write(BattleStartMsg {
                    settlement_id: war_member.target_settlement_id,
                    attacker_faction: attacker_faction.0.to_string(),
                    defender_faction: defender_faction.0.to_string(),
                    x: war_member.target_x,
                    y: war_member.target_y,
                    z: target_pop.home_z,
                });
                tracing::info!(
                    attacker = %attacker_faction.0,
                    defender = %defender_faction.0,
                    entity = ?battle_entity,
                    "Battle started"
                );
            }
        }
    }
}

/// WorldSimSchedule system (1 Hz): progress war parties along their zone route.
///
/// For each war party member with a non-empty `zone_route` and not on the
/// overworld: if within the trigger radius of their exit portal (looked up in
/// `ZoneTopology`) pop the next hop. If the route is exhausted and the member
/// is now on the overworld, the member resumes normal surface attack logic
/// via its existing `march_war_parties` path. Intra-zone movement stays on
/// `FixedUpdate` via `march_war_parties`.
///
/// Also emits `StoryEvent::UndergroundThreat` when the party's hop distance to
/// the overworld is ≤ 3.
#[allow(clippy::too_many_arguments)]
pub fn advance_zone_parties(
    mut warriors: Query<(
        &mut WarPartyMember,
        &WorldPosition,
        &mut fellytip_shared::world::zone::ZoneMembership,
    )>,
    topology: Option<Res<fellytip_shared::world::zone::ZoneTopology>>,
    mut story_writer: MessageWriter<WriteStoryEvent>,
    tick: Res<WorldSimTick>,
) {
    let Some(topology) = topology else { return };

    for (mut war_member, pos, mut membership) in &mut warriors {
        // Idle parties (no route, already overworld) — nothing to do.
        if war_member.zone_route.is_empty()
            && war_member.current_zone == fellytip_shared::world::zone::OVERWORLD_ZONE
        {
            continue;
        }

        // Emit UndergroundThreat when hops_to_surface <= 3.
        if war_member.current_zone != fellytip_shared::world::zone::OVERWORLD_ZONE
            && let Some(hops) = topology.hop_distance(
                war_member.current_zone,
                fellytip_shared::world::zone::OVERWORLD_ZONE,
            )
                && hops <= 3 {
                    story_writer.write(WriteStoryEvent(StoryEvent {
                        id: uuid::Uuid::new_v4(),
                        tick: tick.0,
                        world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
                        kind: StoryEventKind::UndergroundThreat {
                            faction_id: war_member.attacker_faction.0.clone(),
                            hops_to_surface: hops,
                        },
                        participants: Vec::new(),
                        location: None,
                        lore_tags: Vec::new(),
                    }));
                }

        // Find exit portal for the next hop in the route.
        let Some(&next_zone) = war_member.zone_route.first() else {
            continue;
        };
        let Some(portal) = topology
            .exits_from(war_member.current_zone)
            .find(|p| p.to_zone == next_zone)
        else {
            // No portal to the next zone — clear the route and bail so it
            // doesn't spin forever.
            war_member.zone_route.clear();
            continue;
        };

        // Within trigger radius? Use squared distance; the exit anchor world
        // position is not yet propagated (see portal.rs TODO) so we compare
        // against world origin as a placeholder. This system will become
        // load-bearing once anchors are wired to world coords.
        let dx = pos.x - 0.0;
        let dy = pos.y - 0.0;
        let r = portal.trigger_radius;
        if dx * dx + dy * dy <= r * r {
            // Pop next hop: advance zone_route and update current_zone.
            war_member.zone_route.remove(0);
            war_member.current_zone = next_zone;
            membership.0 = next_zone;
        }
    }
}

/// Apply lightweight pairwise repulsion within each war party group (Hot/Warm zones).
///
/// Frozen zone: instead of repulsion, hold each member at a fixed offset from
/// the group centroid arranged in a circle of radius `FROZEN_FORMATION_RADIUS`.
pub fn war_party_separation(
    mut warriors: Query<(Entity, &WarPartyMember, &mut WorldPosition)>,
    temp: Res<ChunkTemperature>,
    scheduler: Res<AdaptiveScheduler>,
) {
    // Collect (entity, settlement_id, pos) snapshot.
    let snapshot: Vec<(Entity, uuid::Uuid, f32, f32)> = warriors
        .iter()
        .map(|(e, w, pos)| (e, w.target_settlement_id, pos.x, pos.y))
        .collect();

    if snapshot.is_empty() {
        return;
    }

    // Group by target_settlement_id.
    let mut groups: HashMap<uuid::Uuid, Vec<usize>> = HashMap::new();
    for (i, (_, sid, _, _)) in snapshot.iter().enumerate() {
        groups.entry(*sid).or_default().push(i);
    }

    // Accumulate delta per entity.
    let mut deltas: HashMap<Entity, (f32, f32)> = HashMap::new();

    for indices in groups.values() {
        let zone = if indices.is_empty() {
            continue;
        } else {
            let (_, _, x, y) = snapshot[indices[0]];
            let dummy_pos = fellytip_shared::components::WorldPosition { x, y, z: 0.0 };
            effective_zone(&dummy_pos, &temp, scheduler.level)
        };

        if zone == SimTier::Frozen {
            // Frozen: arrange at fixed offsets from centroid.
            let n = indices.len();
            let cx: f32 = indices.iter().map(|&i| snapshot[i].2).sum::<f32>() / n as f32;
            let cy: f32 = indices.iter().map(|&i| snapshot[i].3).sum::<f32>() / n as f32;
            for (slot, &idx) in indices.iter().enumerate() {
                let (entity, _, _, _) = snapshot[idx];
                let angle = (slot as f32 / n as f32) * std::f32::consts::TAU;
                let target_x = cx + angle.cos() * FROZEN_FORMATION_RADIUS;
                let target_y = cy + angle.sin() * FROZEN_FORMATION_RADIUS;
                let (cur_x, cur_y) = (snapshot[idx].2, snapshot[idx].3);
                let e = deltas.entry(entity).or_insert((0.0, 0.0));
                e.0 += (target_x - cur_x) * 0.1;
                e.1 += (target_y - cur_y) * 0.1;
            }
        } else {
            // Hot/Warm: pairwise repulsion within SEPARATION_RADIUS.
            for i in 0..indices.len() {
                for j in (i + 1)..indices.len() {
                    let (ea, _, ax, ay) = snapshot[indices[i]];
                    let (eb, _, bx, by) = snapshot[indices[j]];
                    let dx = ax - bx;
                    let dy = ay - by;
                    let dist_sq = dx * dx + dy * dy;
                    if dist_sq > 0.0 && dist_sq < SEPARATION_RADIUS * SEPARATION_RADIUS {
                        let dist = dist_sq.sqrt();
                        let push = (SEPARATION_RADIUS - dist) * SEPARATION_GAIN;
                        let nx = dx / dist;
                        let ny = dy / dist;
                        let da = deltas.entry(ea).or_insert((0.0, 0.0));
                        da.0 += nx * push;
                        da.1 += ny * push;
                        let db = deltas.entry(eb).or_insert((0.0, 0.0));
                        db.0 -= nx * push;
                        db.1 -= ny * push;
                    }
                }
            }
        }
    }

    // Apply accumulated deltas.
    for (entity, _, mut pos) in &mut warriors {
        if let Some((dx, dy)) = deltas.get(&entity) {
            pos.x += dx;
            pos.y += dy;
        }
    }
}

/// Before marching, refresh the target coordinates for any war party hunting a player.
pub fn update_war_party_player_targets(
    player_q: Query<(Entity, &WorldPosition), With<PlayerStandings>>,
    mut warriors: Query<&mut WarPartyMember>,
) {
    for mut warrior in &mut warriors {
        let Some(target_entity) = warrior.player_target else { continue };
        if let Ok((_, pos)) = player_q.get(target_entity) {
            warrior.target_x = pos.x;
            warrior.target_y = pos.y;
        } else {
            // Player disconnected — fall back to settlement target.
            warrior.player_target = None;
        }
    }
}

/// Keep `PlayerStandings` components in sync with `PlayerReputationMap` every tick.
pub fn sync_player_standings(
    mut player_q: Query<(&CombatParticipant, &mut PlayerStandings), Without<FactionMember>>,
    rep: Res<PlayerReputationMap>,
    registry: Res<FactionRegistry>,
) {
    for (cp, mut standings) in &mut player_q {
        standings.standings = registry.factions.iter()
            .map(|f| (f.name.to_string(), rep.score(cp.id.0, &f.id)))
            .collect();
    }
}
