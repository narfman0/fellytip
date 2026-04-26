//! NPC population management: spawn_faction_npcs, tick_population_system,
//! age_npcs_system, check_war_party_formation, seed_factions, init_population_state,
//! flush_factions_to_db, underground pressure/raid systems.

use bevy::ecs::message::{MessageReader, MessageWriter};
use bevy::prelude::*;
use fellytip_shared::{
    combat::{
        interrupt::InterruptStack,
        types::{CharacterClass, CombatantId},
    },
    components::{EntityKind, FactionBadge, GrowthStage, Health, NavPath, NavReplanTimer, PlayerStandings, WorldPosition},
    world::{
        civilization::Settlements,
        ecology::RegionId,
        faction::{
            Disposition, FactionId, FactionGoal, FactionResources, NpcRank,
            PlayerReputationMap, standing_tier, STANDING_NEUTRAL, STANDING_HOSTILE,
        },
        map::{MAP_HALF_WIDTH, MAP_HALF_HEIGHT},
        population::{
            tick_population, PopulationEffect, SettlementPopulation,
            WAR_PARTY_SIZE,
        },
        story::{StoryEvent, StoryEventKind, WriteStoryEvent},
    },
};
use smol_str::SmolStr;
use std::collections::HashMap;
use uuid::Uuid;

use crate::plugins::combat::{CombatParticipant, ExperienceReward};
use crate::plugins::interest::ChunkTemperature;
use crate::plugins::persistence::Db;
use crate::plugins::world_sim::WorldSimTick;

use super::{
    CurrentGoal, FactionMember,
    FactionNpcRank, FactionPopulationState, FactionRegistry, HomePosition, UndergroundPressure,
    WarPartyMember,
};

// ── FormWarPartyEvent ─────────────────────────────────────────────────────────

/// Emitted by `tick_population_system` when a faction is ready to dispatch
/// a war party. Consumed by `check_war_party_formation`.
#[derive(bevy::ecs::message::Message, Clone, Debug)]
pub struct FormWarPartyEvent {
    pub attacker_faction: FactionId,
    pub target_settlement_id: Uuid,
    pub target_x: f32,
    pub target_y: f32,
}

// ── Underground pressure constants ────────────────────────────────────────────

/// Minimum elapsed ticks since last raid before natural buildup kicks in.
const UNDERGROUND_NATURAL_BUILDUP_AFTER_TICKS: u64 = 300;
/// Decay multiplier applied each slow tick: `pressure *= DECAY`.
const UNDERGROUND_DECAY: f32 = 0.95;
/// Pressure boost while any war party is currently in the underground.
const UNDERGROUND_ACTIVE_BOOST: f32 = 0.1;
/// Natural buildup (when the last raid was long enough ago).
const UNDERGROUND_NATURAL_BOOST: f32 = 0.05;
/// Threshold bit layout for hysteresis tracking.
const UNDERGROUND_THRESHOLD_DISTANT_BIT: u8 = 1 << 0; // score >= 0.4
const UNDERGROUND_THRESHOLD_IMMINENT_BIT: u8 = 1 << 1; // score >= 0.7

/// Number of `WarPartyMember` entities spawned per underground raid.
const UNDERGROUND_RAID_PARTY_SIZE: u32 = 3;
/// Minimum pressure score before a raid is spawned.
const UNDERGROUND_RAID_THRESHOLD: f32 = 0.8;

// ── Population constants ──────────────────────────────────────────────────────

/// Military strength recovered per adult NPC per world-sim tick.
const MILITARY_REGEN_PER_ADULT: f32 = 0.05;
/// Hard ceiling on faction military strength.
const MILITARY_CAP: f32 = 50.0;
/// Fraction of prey count added to faction food per tick.
const PREY_TO_FOOD_RATE: f32 = 0.01;
/// Food upkeep per adult NPC per tick.
const FOOD_UPKEEP_PER_ADULT: f32 = 0.005;

// ── Spawn helpers ─────────────────────────────────────────────────────────────

/// Generate `n` spawn offsets (tile units) spread on a 3-wide grid so NPCs
/// aren't stacked on top of each other.
fn npc_spawn_offsets(n: usize) -> Vec<(f32, f32)> {
    (0..n).map(|i| ((i % 3) as f32 * 2.0, (i / 3) as f32 * 2.0)).collect()
}

/// Core ECS bundle for a faction NPC — shared by both `spawn_faction_npcs` and
/// `dm_spawn_npc` so the stat block stays in sync.
///
/// Callers are responsible for inserting any additional components they need:
/// - `spawn_faction_npcs` also inserts `NavPath`, `NavReplanTimer`, `FactionBadge`
/// - `dm_spawn_npc` also inserts `GrowthStage(1.0)` (instant adult)
///
/// `level` is clamped to `u32` at the call site (DM handler receives `u32`);
/// we store it as `u32` here matching `CombatParticipant`.
pub(crate) fn faction_npc_bundle(
    faction_id: FactionId,
    pos: WorldPosition,
    level: u32,
) -> impl Bundle {
    (
        pos.clone(),
        Health { current: 20, max: 20 },
        crate::plugins::combat::CombatParticipant {
            id: CombatantId(Uuid::new_v4()),
            interrupt_stack: InterruptStack::default(),
            class: CharacterClass::Warrior,
            level,
            // Leather armour, DEX 10 → AC 11 (SRD: 11 + DEX mod)
            armor_class: 11,
            strength: 10,
            dexterity: 10,
            constitution: 10,
        },
        // CR 1/4 = 50 XP (docs/dnd5e-srd-reference.md)
        crate::plugins::combat::ExperienceReward(50),
        FactionMember(faction_id.clone()),
        FactionNpcRank(NpcRank::Grunt),
        CurrentGoal(None),
        HomePosition(pos),
        EntityKind::FactionNpc,
    )
}

// ── Public startup systems ────────────────────────────────────────────────────

/// Seed the faction registry with four canonical factions.
///
/// Dispositions are set so wars naturally break out:
///   Iron Wolves ↔ Ash Covenant: Hostile
///   Merchant Guild ↔ Deep Tide: Hostile
pub fn seed_factions(mut registry: ResMut<FactionRegistry>) {
    let mut wolves_disp: HashMap<FactionId, Disposition> = HashMap::new();
    let mut guild_disp   = HashMap::new();
    let mut ash_disp     = HashMap::new();
    let mut tide_disp    = HashMap::new();

    // Iron Wolves ↔ Ash Covenant: mutually hostile.
    wolves_disp.insert(FactionId("ash_covenant".into()),  Disposition::Hostile);
    ash_disp.insert(FactionId("iron_wolves".into()),      Disposition::Hostile);
    // Merchant Guild ↔ Deep Tide: mutually hostile.
    guild_disp.insert(FactionId("deep_tide".into()),      Disposition::Hostile);
    tide_disp.insert(FactionId("merchant_guild".into()),  Disposition::Hostile);

    registry.factions = vec![
        fellytip_shared::world::faction::Faction {
            id: FactionId("iron_wolves".into()),
            name: SmolStr::new("Iron Wolves"),
            disposition: wolves_disp,
            goals: vec![FactionGoal::Survive, FactionGoal::RaidResource { resource_node_id: "mine_01".into() }],
            resources: FactionResources { food: 20.0, gold: 5.0, military_strength: 30.0 },
            territory: vec![RegionId("north".into())],
            is_aggressive: false,
            player_default_standing: STANDING_NEUTRAL,
        },
        fellytip_shared::world::faction::Faction {
            id: FactionId("merchant_guild".into()),
            name: SmolStr::new("Merchant Guild"),
            disposition: guild_disp,
            goals: vec![FactionGoal::FormAlliance { with: FactionId("iron_wolves".into()), min_trust: 0.5 }, FactionGoal::Survive],
            resources: FactionResources { food: 80.0, gold: 200.0, military_strength: 10.0 },
            territory: vec![RegionId("south".into())],
            is_aggressive: false,
            player_default_standing: STANDING_NEUTRAL,
        },
        fellytip_shared::world::faction::Faction {
            id: FactionId("ash_covenant".into()),
            name: SmolStr::new("Ash Covenant"),
            disposition: ash_disp,
            goals: vec![FactionGoal::Survive, FactionGoal::DefendSettlement { settlement_id: "ruins_01".into() }],
            resources: FactionResources { food: 15.0, gold: 0.0, military_strength: 40.0 },
            territory: vec![RegionId("east".into())],
            is_aggressive: true,
            player_default_standing: STANDING_HOSTILE,
        },
        fellytip_shared::world::faction::Faction {
            id: FactionId("deep_tide".into()),
            name: SmolStr::new("Deep Tide"),
            disposition: tide_disp,
            goals: vec![FactionGoal::Survive, FactionGoal::RaidResource { resource_node_id: "surface_01".into() }],
            resources: FactionResources { food: 10.0, gold: 0.0, military_strength: 35.0 },
            territory: vec![RegionId("underground".into())],
            is_aggressive: true,
            player_default_standing: STANDING_HOSTILE,
        },
    ];
}

/// Spawn guard NPCs for each faction at their nearest settlement.
/// Count is controlled by `MapGenConfig::npcs_per_faction` (default 3).
/// Runs at Startup after `seed_factions` and after `MapGenPlugin` inserts `Settlements`.
pub fn spawn_faction_npcs(
    registry: Res<FactionRegistry>,
    settlements: Res<Settlements>,
    config: Res<crate::plugins::map_gen::MapGenConfig>,
    mut commands: Commands,
) {
    if settlements.0.is_empty() {
        tracing::warn!("No settlements available; skipping faction NPC spawn");
        return;
    }

    let offsets = npc_spawn_offsets(config.npcs_per_faction);

    for (faction_idx, faction) in registry.factions.iter().enumerate() {
        // Assign each faction a home settlement by cycling through the list.
        let settlement = &settlements.0[faction_idx % settlements.0.len()];

        for (npc_idx, (ox, oy)) in offsets.iter().enumerate() {
            // settlement.x/y are tile-space (0..MAP_WIDTH); convert to world-space.
            let pos = WorldPosition {
                x: settlement.x - MAP_HALF_WIDTH as f32 + ox,
                y: settlement.y - MAP_HALF_HEIGHT as f32 + oy,
                z: settlement.z,
            };
            commands.spawn((
                faction_npc_bundle(faction.id.clone(), pos, 1),
                FactionBadge { faction_id: faction.id.0.to_string(), rank: NpcRank::Grunt },
                NavPath::default(),
                NavReplanTimer::default(),
            ));
            tracing::debug!(
                faction = %faction.name,
                settlement = %settlement.name,
                npc = npc_idx,
                "Faction NPC spawned"
            );
        }

        tracing::info!(
            faction = %faction.name,
            settlement = %settlement.name,
            count = offsets.len(),
            "Faction NPCs spawned"
        );
    }
}

/// Persist the current faction registry to SQLite so worldwatch can read it.
///
/// Runs at Startup after `seed_factions`. Uses the same block_on pattern as
/// `on_client_disconnected` in server/main.rs.
pub fn flush_factions_to_db(registry: Res<FactionRegistry>, db: Res<Db>) {
    let pool = db.pool().clone();
    let factions = registry.factions.clone();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for faction flush");

    rt.block_on(async move {
        for faction in &factions {
            let id = faction.id.0.as_str().to_owned();
            let name = faction.name.as_str().to_owned();
            let resources = serde_json::to_string(&faction.resources)
                .unwrap_or_else(|_| "{}".to_owned());
            let territory = serde_json::to_string(&faction.territory)
                .unwrap_or_else(|_| "[]".to_owned());
            let goals = serde_json::to_string(&faction.goals)
                .unwrap_or_else(|_| "[]".to_owned());

            let res = sqlx::query(
                "INSERT OR REPLACE INTO factions (id, name, resources, territory, goals) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(&name)
            .bind(&resources)
            .bind(&territory)
            .bind(&goals)
            .execute(&pool)
            .await;

            match res {
                Ok(_) => tracing::debug!(faction = %name, "Faction flushed to DB"),
                Err(e) => tracing::warn!(faction = %name, "Faction flush failed: {e}"),
            }
        }
        tracing::info!(count = factions.len(), "Factions persisted to SQLite");
    });
}

/// Seed one `SettlementPopulation` entry per settlement.
/// Runs after `spawn_faction_npcs` in the MapGenPlugin startup chain.
pub fn init_population_state(
    mut pop: ResMut<FactionPopulationState>,
    settlements: Res<Settlements>,
    registry: Res<FactionRegistry>,
    config: Res<crate::plugins::map_gen::MapGenConfig>,
) {
    if settlements.0.is_empty() {
        tracing::warn!("No settlements for population init");
        return;
    }
    for (faction_idx, faction) in registry.factions.iter().enumerate() {
        let settlement = &settlements.0[faction_idx % settlements.0.len()];
        let home_x = settlement.x - MAP_HALF_WIDTH as f32;
        let home_y = settlement.y - MAP_HALF_HEIGHT as f32;
        let military_strength = registry.factions
            .iter()
            .find(|f| f.id == faction.id)
            .map(|f| f.resources.military_strength)
            .unwrap_or(0.0);
        pop.settlements.insert(
            settlement.id,
            SettlementPopulation {
                settlement_id: settlement.id,
                faction_id: faction.id.clone(),
                world_id: fellytip_shared::world::zone::WORLD_SURFACE,
                birth_ticks: 0,
                adult_count: config.npcs_per_faction as u32,
                child_count: 0,
                home_x,
                home_y,
                home_z: settlement.z,
                war_party_cooldown: 0,
                military_strength,
            },
        );
    }
    tracing::info!(count = pop.settlements.len(), "Settlement population states seeded");
}

// ── WorldSim systems ──────────────────────────────────────────────────────────

/// Advance each settlement's population by one tick.
/// Spawns child NPCs and emits `FormWarPartyEvent` when threshold is reached.
pub fn tick_population_system(
    mut pop: ResMut<FactionPopulationState>,
    npc_query: Query<(&FactionMember, Option<&GrowthStage>, Has<WarPartyMember>)>,
    mut registry: ResMut<FactionRegistry>,
    ecology: Res<crate::plugins::ecology::EcologyState>,
    tick: Res<WorldSimTick>,
    mut commands: Commands,
    mut war_events: MessageWriter<FormWarPartyEvent>,
) {
    // Count live adults and children per faction.
    let mut faction_adults: HashMap<FactionId, u32>   = HashMap::new();
    let mut faction_children: HashMap<FactionId, u32> = HashMap::new();
    for (member, growth, is_war_party) in &npc_query {
        let is_child = growth.map(|g| g.0 < 1.0).unwrap_or(false);
        if is_child {
            *faction_children.entry(member.0.clone()).or_insert(0) += 1;
        } else if !is_war_party {
            *faction_adults.entry(member.0.clone()).or_insert(0) += 1;
        }
    }

    // ── Military strength recovery ────────────────────────────────────────────
    for faction in &mut registry.factions {
        let adults = *faction_adults.get(&faction.id).unwrap_or(&0);
        let regen = adults as f32 * MILITARY_REGEN_PER_ADULT;
        faction.resources.military_strength =
            (faction.resources.military_strength + regen).min(MILITARY_CAP);
    }

    // ── Ecology → food integration ────────────────────────────────────────────
    let total_prey: f64 = ecology.regions.iter().map(|r| r.prey.count).sum();
    let faction_count = registry.factions.len().max(1) as f64;
    let prey_per_faction = (total_prey / faction_count) as f32;

    for faction in &mut registry.factions {
        let adults = *faction_adults.get(&faction.id).unwrap_or(&0);
        let food_gain = prey_per_faction * PREY_TO_FOOD_RATE;
        let food_cost = adults as f32 * FOOD_UPKEEP_PER_ADULT;
        faction.resources.food = (faction.resources.food + food_gain - food_cost).clamp(0.0, 100.0);
    }

    // Build faction-id → hostile settlement positions map.
    let faction_hostile_targets: HashMap<FactionId, Vec<(Uuid, f32, f32, f32)>> = registry.factions
        .iter()
        .map(|f| {
            let hostile_faction_ids: Vec<&FactionId> = f.disposition
                .iter()
                .filter(|(_, d)| **d == Disposition::Hostile)
                .map(|(id, _)| id)
                .collect();
            let targets: Vec<(Uuid, f32, f32, f32)> = pop.settlements
                .values()
                .filter(|s| hostile_faction_ids.contains(&&s.faction_id))
                .map(|s| (s.settlement_id, s.home_x, s.home_y, s.home_z))
                .collect();
            (f.id.clone(), targets)
        })
        .collect();

    // Tick each settlement and apply effects.
    let settlement_ids: Vec<Uuid> = pop.settlements.keys().copied().collect();
    for sid in settlement_ids {
        let Some(mut state) = pop.settlements.remove(&sid) else { continue };
        state.adult_count        = *faction_adults.get(&state.faction_id).unwrap_or(&0);
        state.child_count        = *faction_children.get(&state.faction_id).unwrap_or(&0);
        state.military_strength  = registry.factions
            .iter()
            .find(|f| f.id == state.faction_id)
            .map(|f| f.resources.military_strength)
            .unwrap_or(0.0);
        let targets = faction_hostile_targets.get(&state.faction_id).map(|v| v.as_slice()).unwrap_or(&[]);
        let (next, effects) = tick_population(state, targets, None);

        for effect in effects {
            match effect {
                PopulationEffect::SpawnChild { x, y, z, .. } => {
                    // Use tick counter for deterministic jitter between factions.
                    let jitter = ((tick.0 as f32 * 0.37).sin() * 0.5, (tick.0 as f32 * 0.61).cos() * 0.5);
                    let pos = WorldPosition { x: x + jitter.0, y: y + jitter.1, z };
                    commands.spawn((
                        pos.clone(),
                        Health { current: 5, max: 5 },
                        CombatParticipant {
                            id: CombatantId(Uuid::new_v4()),
                            interrupt_stack: Default::default(),
                            class: CharacterClass::Warrior,
                            level: 1,
                            armor_class: 8,
                            strength: 8,
                            dexterity: 8,
                            constitution: 8,
                        },
                        ExperienceReward(5),
                        FactionMember(next.faction_id.clone()),
                        FactionNpcRank(NpcRank::Grunt),
                        FactionBadge { faction_id: next.faction_id.0.to_string(), rank: NpcRank::Grunt },
                        CurrentGoal(None),
                        HomePosition(pos),
                        EntityKind::FactionNpc,
                        GrowthStage(0.0),
                        NavPath::default(),
                        NavReplanTimer::default(),
                    ));
                    tracing::debug!(faction = %next.faction_id.0, "Child NPC spawned");
                }
                PopulationEffect::FormWarParty { attacker_faction, target_settlement_id, tx, ty } => {
                    war_events.write(FormWarPartyEvent {
                        attacker_faction,
                        target_settlement_id,
                        target_x: tx,
                        target_y: ty,
                    });
                }
            }
        }
        pop.settlements.insert(sid, next);
    }
}

/// Increment `GrowthStage` each tick, scaled by the NPC's zone speed.
pub fn age_npcs_system(
    mut query: Query<(&mut GrowthStage, &mut Health, &WorldPosition), With<FactionMember>>,
    temp: Res<ChunkTemperature>,
) {
    for (mut growth, mut health, pos) in &mut query {
        let speed = temp.speed_at_world(pos.x, pos.y);
        let prev = growth.0;
        growth.0 = (growth.0 + speed / 300.0).min(1.0);
        // On maturity: upgrade health to adult values.
        if prev < 1.0 && growth.0 >= 1.0 {
            health.max = 20;
            health.current = health.current.max(1);
            tracing::debug!("NPC matured to adult");
        }
    }
}

/// Tag `WAR_PARTY_SIZE` adult NPCs from the attacker faction as war-party members.
///
/// For aggressive factions, applies a 40 % chance to redirect the war party toward
/// a nearby hostile player instead of the target settlement.
#[allow(clippy::too_many_arguments)]
pub fn check_war_party_formation(
    mut events: MessageReader<FormWarPartyEvent>,
    npc_query: Query<(Entity, &FactionMember, Option<&GrowthStage>), Without<WarPartyMember>>,
    player_q: Query<(Entity, &WorldPosition, &CombatParticipant), With<PlayerStandings>>,
    rep: Res<PlayerReputationMap>,
    registry: Res<FactionRegistry>,
    pop: Res<FactionPopulationState>,
    tick: Res<WorldSimTick>,
    nav: Option<Res<crate::plugins::nav::NavGrid>>,
    mut flow_field: ResMut<crate::plugins::nav::FlowField>,
    mut story_events: MessageWriter<WriteStoryEvent>,
    mut commands: Commands,
) {
    for event in events.read() {
        // Resolve defender faction name for the story event.
        let defender_faction = pop.settlements
            .get(&event.target_settlement_id)
            .map(|s| s.faction_id.clone())
            .unwrap_or(FactionId("unknown".into()));

        story_events.write(WriteStoryEvent(StoryEvent {
            id: Uuid::new_v4(),
            tick: tick.0,
            world_day: (tick.0 / 300) as u32,
            kind: StoryEventKind::FactionWarDeclared {
                attacker: event.attacker_faction.clone(),
                defender: defender_faction,
            },
            participants: vec![],
            location: None,
            lore_tags: vec!["war".into()],
        }));

        // 40 % chance for aggressive factions to hunt a hostile player.
        let is_aggressive = registry.factions.iter()
            .find(|f| f.id == event.attacker_faction)
            .map(|f| f.is_aggressive)
            .unwrap_or(false);

        let player_target = if is_aggressive {
            // Deterministic roll: multiply tick by a prime, check low nibble.
            let roll = tick.0.wrapping_mul(2_654_435_761) % 10;
            if roll < 4 {
                // Nearest hostile player wins the raid target lottery.
                player_q.iter()
                    .filter(|(_, _, cp)| {
                        standing_tier(rep.score(cp.id.0, &event.attacker_faction)).is_aggressive()
                    })
                    .min_by(|(_, pa, _), (_, pb, _)| {
                        let da = (pa.x - event.target_x).powi(2) + (pa.y - event.target_y).powi(2);
                        let db = (pb.x - event.target_x).powi(2) + (pb.y - event.target_y).powi(2);
                        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(e, _, _)| e)
            } else {
                None
            }
        } else {
            None
        };

        if player_target.is_some() {
            tracing::info!(
                faction = %event.attacker_faction.0,
                "War party redirected to hunt hostile player"
            );
        }

        let mut tagged = 0u32;
        for (entity, member, growth) in &npc_query {
            if tagged >= WAR_PARTY_SIZE {
                break;
            }
            let is_adult = growth.map(|g| g.0 >= 1.0).unwrap_or(true);
            if member.0 == event.attacker_faction && is_adult {
                commands.entity(entity).insert(WarPartyMember {
                    target_settlement_id: event.target_settlement_id,
                    target_x: event.target_x,
                    target_y: event.target_y,
                    attacker_faction: event.attacker_faction.clone(),
                    player_target,
                    current_zone: fellytip_shared::world::zone::OVERWORLD_ZONE,
                    zone_route: Vec::new(),
                });
                tagged += 1;
            }
        }
        if tagged > 0 {
            tracing::info!(
                faction = %event.attacker_faction.0,
                warriors = tagged,
                target = %event.target_settlement_id,
                "War party formed"
            );

            // Pre-compute flow field for this target settlement.
            if let Some(ref nav_res) = nav {
                flow_field.get_or_compute(nav_res, event.target_x, event.target_y);
            }
        }
    }
}

// ── Underground pressure (UndergroundSimSchedule @ 0.1 Hz) ────────────────────

/// Tick the underground pressure score on `UndergroundSimSchedule` (0.1 Hz).
pub fn accumulate_underground_pressure(
    mut pressure: ResMut<UndergroundPressure>,
    tick: Res<WorldSimTick>,
    zone_registry: Option<Res<fellytip_shared::world::zone::ZoneRegistry>>,
    warriors: Query<&WarPartyMember>,
) {
    // Decay first so bumps accumulate on top of a lower floor.
    pressure.score *= UNDERGROUND_DECAY;

    // Check if any war party is currently in an underground zone.
    if let Some(registry) = zone_registry.as_ref() {
        let any_underground = warriors.iter().any(|wm| {
            registry
                .get(wm.current_zone)
                .map(|zone| matches!(
                    zone.kind,
                    fellytip_shared::world::zone::ZoneKind::Underground { .. }
                ))
                .unwrap_or(false)
        });
        if any_underground {
            pressure.score += UNDERGROUND_ACTIVE_BOOST;
        }
    }

    // Natural buildup if enough time has passed since the last raid.
    if tick.0.saturating_sub(pressure.last_raid_tick) > UNDERGROUND_NATURAL_BUILDUP_AFTER_TICKS {
        pressure.score += UNDERGROUND_NATURAL_BOOST;
    }

    pressure.score = pressure.score.clamp(0.0, 1.0);
}

/// Emit `StoryEvent::UndergroundThreat` when the pressure score crosses each
/// threshold (hysteresis: latched while >= threshold, cleared when < 0.4).
pub fn deliver_underground_signals(
    mut pressure: ResMut<UndergroundPressure>,
    tick: Res<WorldSimTick>,
    mut story_writer: MessageWriter<WriteStoryEvent>,
) {
    let score = pressure.score;

    // Distant signal at 0.4 (99 hops).
    if score >= 0.4 && (pressure.thresholds_crossed & UNDERGROUND_THRESHOLD_DISTANT_BIT) == 0 {
        pressure.thresholds_crossed |= UNDERGROUND_THRESHOLD_DISTANT_BIT;
        story_writer.write(WriteStoryEvent(StoryEvent {
            id: Uuid::new_v4(),
            tick: tick.0,
            world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
            kind: StoryEventKind::UndergroundThreat {
                faction_id: SmolStr::new("remnants"),
                hops_to_surface: 99,
            },
            participants: Vec::new(),
            location: None,
            lore_tags: vec!["underground".into(), "distant".into()],
        }));
        tracing::info!(score, "Underground distant signal fired");
    }

    // Imminent signal at 0.7 (2 hops).
    if score >= 0.7 && (pressure.thresholds_crossed & UNDERGROUND_THRESHOLD_IMMINENT_BIT) == 0 {
        pressure.thresholds_crossed |= UNDERGROUND_THRESHOLD_IMMINENT_BIT;
        story_writer.write(WriteStoryEvent(StoryEvent {
            id: Uuid::new_v4(),
            tick: tick.0,
            world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
            kind: StoryEventKind::UndergroundThreat {
                faction_id: SmolStr::new("remnants"),
                hops_to_surface: 2,
            },
            participants: Vec::new(),
            location: None,
            lore_tags: vec!["underground".into(), "imminent".into(), "fleeing".into()],
        }));
        tracing::info!(score, "Underground imminent signal fired");
    }

    // Hysteresis: reset all latched bits when pressure falls below 0.4.
    if score < 0.4 && pressure.thresholds_crossed != 0 {
        pressure.thresholds_crossed = 0;
    }
}

// ── Underground raid spawn (WorldSimSchedule) ────────────────────────────────

/// When pressure is high enough, spawn a `UNDERGROUND_RAID_PARTY_SIZE`-member
/// raid party in the deepest underground zone and route them to the overworld.
#[allow(clippy::too_many_arguments)]
pub fn spawn_underground_raid(
    mut commands: Commands,
    mut pressure: ResMut<UndergroundPressure>,
    zone_registry: Option<Res<fellytip_shared::world::zone::ZoneRegistry>>,
    zone_topology: Option<Res<fellytip_shared::world::zone::ZoneTopology>>,
    pop: Res<FactionPopulationState>,
    tick: Res<WorldSimTick>,
    warriors: Query<&WarPartyMember>,
) {
    if pressure.score < UNDERGROUND_RAID_THRESHOLD {
        return;
    }

    let Some(registry) = zone_registry else { return };
    let Some(topology) = zone_topology else { return };

    // Only one active underground raid at a time.
    let already_active = warriors.iter().any(|wm| {
        registry
            .get(wm.current_zone)
            .map(|z| matches!(z.kind, fellytip_shared::world::zone::ZoneKind::Underground { .. }))
            .unwrap_or(false)
            || wm.attacker_faction.0.as_str() == "remnants"
    });
    if already_active {
        return;
    }

    // Find the deepest underground zone (highest `depth`).
    let deepest = registry
        .zones
        .iter()
        .filter_map(|(id, zone)| match zone.kind {
            fellytip_shared::world::zone::ZoneKind::Underground { depth } => Some((*id, depth, zone)),
            _ => None,
        })
        .max_by_key(|(_, depth, _)| *depth);
    let Some((deepest_id, _, deepest_zone)) = deepest else {
        tracing::warn!("No underground zones in registry; skipping raid spawn");
        return;
    };

    // Find highest-population surface settlement (defender side).
    let target = pop
        .settlements
        .values()
        .filter(|s| s.world_id == fellytip_shared::world::zone::WORLD_SURFACE)
        .max_by_key(|s| s.adult_count)
        .map(|s| (s.settlement_id, s.home_x, s.home_y));
    let (target_sid, target_x, target_y) = match target {
        Some(t) => t,
        None => {
            tracing::warn!("No populated settlements for underground raid target; skipping spawn");
            return;
        }
    };

    // Compute zone route deepest → OVERWORLD_ZONE via BFS.
    let Some(zone_route) = topology.shortest_path(
        deepest_id,
        fellytip_shared::world::zone::OVERWORLD_ZONE,
    ) else {
        tracing::warn!(
            deepest = ?deepest_id,
            "No zone path from deepest underground to overworld; skipping raid spawn"
        );
        return;
    };

    let spawn_x = 0.0_f32;
    let spawn_y = 0.0_f32;
    let _ = deepest_zone; // kept for future anchor lookup

    let underground_fid = FactionId(SmolStr::new("remnants"));

    for i in 0..UNDERGROUND_RAID_PARTY_SIZE {
        let offset_x = (i % 3) as f32 * 1.0;
        let offset_y = (i / 3) as f32 * 1.0;
        let pos = WorldPosition {
            x: spawn_x + offset_x,
            y: spawn_y + offset_y,
            z: 0.0,
        };
        commands.spawn((
            pos.clone(),
            Health { current: 25, max: 25 },
            CombatParticipant {
                id: CombatantId(Uuid::new_v4()),
                interrupt_stack: InterruptStack::default(),
                class: CharacterClass::Warrior,
                level: 2,
                armor_class: 12,
                strength: 12,
                dexterity: 11,
                constitution: 12,
            },
            ExperienceReward(75),
            FactionBadge {
                faction_id: "remnants".to_string(),
                rank: NpcRank::Grunt,
            },
            FactionNpcRank(NpcRank::Grunt),
            EntityKind::FactionNpc,
            HomePosition(pos),
            WarPartyMember {
                target_settlement_id: target_sid,
                target_x,
                target_y,
                attacker_faction: underground_fid.clone(),
                player_target: None,
                current_zone: deepest_id,
                zone_route: zone_route.clone(),
            },
            fellytip_shared::world::zone::ZoneMembership(deepest_id),
        ));
    }

    pressure.score = 0.0;
    pressure.last_raid_tick = tick.0;
    pressure.thresholds_crossed = 0;
    tracing::info!(
        deepest = ?deepest_id,
        target = %target_sid,
        hops = zone_route.len(),
        "Underground raid party spawned"
    );
}
