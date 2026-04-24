//! NPC AI plugin — re-evaluates faction goals and nudges NPC positions
//! each WorldSimSchedule tick (1 Hz).
//!
//! # Three-tier pathfinding LOD
//!
//! All pathfinding is gated on the entity's `SimTier` (see `interest::effective_zone`):
//!
//! | Tier   | Individual NPCs (wander_npcs)         | War parties (march_war_parties)        | Separation (war_party_separation) |
//! |--------|---------------------------------------|----------------------------------------|-----------------------------------|
//! | Hot    | A* replan every 2 ticks, full speed   | Flow-field sampling, full MARCH_SPEED  | Pairwise repulsion                |
//! | Warm   | A* replan every 8 ticks, 0.25× speed  | Flow-field sampling, 0.25× speed       | Pairwise repulsion                |
//! | Frozen | Linear march toward home, 0.05× speed | Linear march to target, 0.05× speed   | Fixed centroid-offset formation   |
//!
//! Frozen entities always reach their goal (macro-correct behavior) via linear march;
//! expensive A* and flow-field sampling are skipped entirely for out-of-range chunks.

use bevy::ecs::message::{MessageReader, MessageWriter};
use bevy::prelude::*;
use crate::plugins::interest::ChunkTemperature;
use crate::plugins::persistence::Db;
use fellytip_shared::protocol::{BattleAttackMsg, BattleEndMsg, BattleStartMsg};
use fellytip_shared::{
    combat::{
        interrupt::InterruptStack,
        types::{CharacterClass, CombatantId, CombatantSnapshot, CombatantState, CombatState, CoreStats, Effect},
    },
    components::{EntityKind, FactionBadge, GrowthStage, Health, NavPath, NavReplanTimer, PlayerStandings, WorldPosition},
    world::{
        civilization::Settlements,
        faction::{
            Disposition, Faction, FactionId, FactionResources, FactionGoal, NpcRank,
            PlayerReputationMap, standing_tier, STANDING_NEUTRAL, STANDING_HOSTILE, pick_goal,
        },
        ecology::RegionId,
        map::{MAP_HALF_WIDTH, MAP_HALF_HEIGHT},
        population::{
            tick_population, PopulationEffect, SettlementPopulation,
            BATTLE_RADIUS, MARCH_SPEED, WAR_PARTY_SIZE,
        },
        story::{StoryEvent, StoryEventKind, WriteStoryEvent},
        war::{seeded_dice, tick_battle_round},
    },
};
use smol_str::SmolStr;
use std::collections::{HashMap, VecDeque};
use uuid::Uuid;

use crate::plugins::combat::{CombatParticipant, ExperienceReward};
use crate::plugins::interest::{effective_zone, SimTier};
use crate::plugins::nav::{world_to_nav, nav_to_world, NavGrid, FlowField};
use crate::plugins::perf::AdaptiveScheduler;
use crate::plugins::world_sim::{UnderDarkSimSchedule, WorldSimSchedule, WorldSimTick};

/// Server-only component: which faction this NPC belongs to.
#[derive(Component)]
pub struct FactionMember(#[allow(dead_code)] pub FactionId);

/// Server-only component: current AI goal being pursued.
#[derive(Component)]
pub struct CurrentGoal(#[allow(dead_code)] pub Option<FactionGoal>);

/// Server-only component: home position used for bounded wander / future pathfinding.
#[derive(Component)]
pub struct HomePosition(pub WorldPosition);

/// Server-only component: NPC rank for kill-penalty calculation.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct FactionNpcRank(pub NpcRank);

/// Server-only resource: all live factions.
#[derive(Resource, Default)]
pub struct FactionRegistry {
    pub factions: Vec<Faction>,
}

/// Tags an NPC as part of an active war party marching toward a target.
/// Server-only — never replicated, but registered for reflection so ralph
/// scenarios can observe war-party membership via `bevy/query`.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct WarPartyMember {
    pub target_settlement_id: Uuid,
    pub target_x: f32,
    pub target_y: f32,
    /// Faction that dispatched this war party. Stored so the spawned
    /// `ActiveBattle` (and downstream `BattleRecord`) has the correct
    /// attacker — we cannot reliably recover it from `FactionMember` alone
    /// because `march_war_parties` doesn't query that component.
    ///
    /// `FactionId` doesn't derive `Reflect`, so it's skipped during reflection.
    #[reflect(ignore)]
    pub attacker_faction: FactionId,
    /// When set, this war party is hunting the given player entity rather than
    /// marching to the settlement.  `target_x`/`target_y` are updated each tick
    /// by `update_war_party_player_targets` to follow the player's position.
    pub player_target: Option<Entity>,
    /// Zone the war party member currently occupies. Defaults to the overworld.
    /// Reflect-skipped because ZoneId is in shared::world::zone and doesn't
    /// satisfy the reflection derive requirements downstream.
    #[reflect(ignore)]
    pub current_zone: fellytip_shared::world::zone::ZoneId,
    /// Pre-computed zone hop path this party is traversing. Empty = overworld-only.
    #[reflect(ignore)]
    pub zone_route: Vec<fellytip_shared::world::zone::ZoneId>,
}

/// Lives on a bookkeeping entity while a battle is ongoing at a settlement.
/// Despawned when one side is eliminated.
#[derive(Component)]
pub struct ActiveBattle {
    pub settlement_id: Uuid,
    pub attacker_faction: FactionId,
    pub defender_faction: FactionId,
    pub battle_x: f32,
    pub battle_y: f32,
    pub attacker_casualties: u32,
    pub defender_casualties: u32,
    /// Fractional round accumulator. Zone speed is added each tick; a round
    /// fires when the accumulator crosses 1.0. Battles near no player resolve
    /// at FROZEN_SPEED (0.05) rounds per tick = ~20 ticks per round.
    pub round_acc: f32,
}

/// Per-settlement mutable population state — one entry per settlement.
#[derive(Resource, Default)]
pub struct FactionPopulationState {
    pub settlements: HashMap<Uuid, SettlementPopulation>,
}

/// Persistent record of a resolved battle — appended when one side is eliminated.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BattleRecord {
    pub winner_faction: String,
    pub loser_faction: String,
    pub target_settlement_id: String,
    pub tick: u64,
    pub attacker_casualties: u32,
    pub defender_casualties: u32,
}

/// Background pressure score for the Underdark faction's surface raids.
///
/// Accumulates slowly on `UnderDarkSimSchedule` (0.1 Hz). When it crosses
/// configured thresholds it emits environmental signals (`StoryEvent`s); when
/// it peaks the raid spawn system converts it into a concrete war party.
///
/// * `score`: 0.0 = calm, 1.0 = imminent raid
/// * `last_raid_tick`: `WorldSimTick` when the last raid party spawned
/// * `thresholds_crossed`: bitmask of thresholds currently latched (bit 0 = 0.4
///   distant signal, bit 1 = 0.7 imminent signal). Uses hysteresis: bits are
///   set when crossed upward and cleared when the score drops back below 0.4.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct UnderDarkPressure {
    pub score: f32,
    pub last_raid_tick: u64,
    pub thresholds_crossed: u8,
}

/// Rolling history of resolved battles, capped at 100 entries.
#[derive(Resource, Default)]
pub struct BattleHistory {
    pub records: VecDeque<BattleRecord>,
}

impl BattleHistory {
    pub fn push(&mut self, record: BattleRecord) {
        if self.records.len() >= 100 {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }
}

/// Emitted by `tick_population_system` when a faction is ready to dispatch
/// a war party. Consumed by `check_war_party_formation`.
#[derive(bevy::ecs::message::Message, Clone, Debug)]
pub struct FormWarPartyEvent {
    pub attacker_faction: FactionId,
    pub target_settlement_id: Uuid,
    pub target_x: f32,
    pub target_y: f32,
}

/// Generate `n` spawn offsets (tile units) spread on a 3-wide grid so NPCs
/// aren't stacked on top of each other.
fn npc_spawn_offsets(n: usize) -> Vec<(f32, f32)> {
    (0..n).map(|i| ((i % 3) as f32 * 2.0, (i / 3) as f32 * 2.0)).collect()
}

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FactionRegistry>()
            .init_resource::<PlayerReputationMap>()
            .init_resource::<FactionPopulationState>()
            .init_resource::<FlowField>()
            .init_resource::<BattleHistory>()
            .init_resource::<UnderDarkPressure>()
            .add_message::<FormWarPartyEvent>()
            .register_type::<FactionNpcRank>()
            .register_type::<WarPartyMember>()
            .add_message::<BattleStartMsg>()
            .add_message::<BattleEndMsg>()
            .add_message::<BattleAttackMsg>();
        app.add_systems(
            WorldSimSchedule,
            (
                update_faction_goals,
                tick_population_system,
                age_npcs_system,
                check_war_party_formation,
                update_war_party_player_targets,
                advance_zone_parties,
                spawn_underdark_raid,
                march_war_parties,
                war_party_separation,
                run_battle_rounds,
                wander_npcs,
                sync_player_standings,
            ).chain(),
        );
        app.add_systems(
            UnderDarkSimSchedule,
            (
                accumulate_underdark_pressure,
                deliver_underdark_signals,
            ).chain(),
        );
        // spawn_faction_npcs, init_population_state, and flush_factions_to_db are
        // registered in MapGenPlugin's Startup chain so they run after
        // generate_world inserts the Settlements resource.
    }
}

/// Re-score and update the active goal for every faction.
fn update_faction_goals(mut registry: ResMut<FactionRegistry>) {
    for faction in &mut registry.factions {
        if let Some(top) = pick_goal(faction) {
            tracing::debug!(
                faction = %faction.name,
                goal = ?top,
                "Faction goal selected"
            );
        }
    }
}

/// Move faction NPCs each world-sim tick using zone-gated A* pathfinding.
///
/// # Three-tier LOD behavior:
/// - **Hot** (chunks 0–2 from player): replan A* every 2 ticks, follow waypoints at full speed.
/// - **Warm** (chunks 3–8 from player): replan every 8 ticks, follow at 0.25× speed.
/// - **Frozen** (>8 chunks from player): skip A*, linear march toward home at 0.05× speed.
///
/// War party members are excluded — they march under `march_war_parties` instead.
#[allow(clippy::type_complexity)]
fn wander_npcs(
    mut query: Query<
        (Entity, &mut WorldPosition, &HomePosition, &mut NavPath, &mut NavReplanTimer),
        (With<FactionMember>, Without<WarPartyMember>),
    >,
    temp: Res<ChunkTemperature>,
    scheduler: Res<AdaptiveScheduler>,
    nav: Option<Res<NavGrid>>,
    tick: Res<WorldSimTick>,
) {
    let Some(nav) = nav else { return };

    for (entity, mut pos, home, mut nav_path, mut replan_timer) in &mut query {
        let zone = effective_zone(&pos, &temp, scheduler.level);
        let zone_speed = zone.speed();

        // Frozen: skip A*, linear march toward home position.
        if zone == SimTier::Frozen {
            let dx = home.0.x - pos.x;
            let dy = home.0.y - pos.y;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq > 0.01 {
                let dist = dist_sq.sqrt();
                pos.x += (dx / dist) * FROZEN_WANDER_STEP * zone_speed;
                pos.y += (dy / dist) * FROZEN_WANDER_STEP * zone_speed;
            }
            continue;
        }

        // Determine replan cadence from zone.
        let replan_every = if zone == SimTier::Hot { 2u32 } else { 8u32 };

        replan_timer.0 = replan_timer.0.saturating_add(1);

        // Replan A* when timer expires or path is exhausted.
        if replan_timer.0 >= replan_every || nav_path.is_complete() {
            replan_timer.0 = 0;

            // Wander goal: pick a position within 4 tiles of home using entity seed.
            #[allow(clippy::cast_precision_loss)]
            let entity_seed = entity.to_bits() as f32 * 0.000_013_7;
            #[allow(clippy::cast_precision_loss)]
            let angle = (entity_seed + tick.0 as f32 * 0.07).sin() * std::f32::consts::TAU;
            let goal_x = home.0.x + angle.cos() * 3.5;
            let goal_y = home.0.y + angle.sin() * 3.5;

            let start = world_to_nav(pos.x, pos.y);
            let goal  = world_to_nav(goal_x, goal_y);

            if let Some(waypoints) = nav.astar(start, goal) {
                *nav_path = NavPath { waypoints, waypoint_index: 0 };
            }
        }

        // Follow current path: advance toward next waypoint.
        if let Some((wx, wy)) = nav_path.next_waypoint() {
            let (target_x, target_y) = nav_to_world(wx as usize, wy as usize);
            let dx = target_x - pos.x;
            let dy = target_y - pos.y;
            let dist_sq = dx * dx + dy * dy;
            let step = WANDER_STEP * zone_speed;
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

/// Movement speed per tick for wandering NPCs (in world units).
const WANDER_STEP: f32 = 0.15;
/// Movement speed per tick for Frozen NPCs linear-marching to home.
const FROZEN_WANDER_STEP: f32 = 0.5;

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
                pos.clone(),
                Health { current: 20, max: 20 },
                CombatParticipant {
                    id: CombatantId(Uuid::new_v4()),
                    interrupt_stack: InterruptStack::default(),
                    class: CharacterClass::Warrior,
                    level: 1,
                    // Leather armour, DEX 10 → AC 11 (SRD: 11 + DEX mod)
                    armor_class: 11,
                    strength: 10,
                    dexterity: 10,
                    constitution: 10,
                },
                // CR 1/4 = 50 XP (docs/dnd5e-srd-reference.md)
                ExperienceReward(50),
                FactionMember(faction.id.clone()),
                FactionNpcRank(NpcRank::Grunt),
                FactionBadge { faction_id: faction.id.0.to_string(), rank: NpcRank::Grunt },
                CurrentGoal(None),
                HomePosition(pos),
                EntityKind::FactionNpc,
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
        Faction {
            id: FactionId("iron_wolves".into()),
            name: SmolStr::new("Iron Wolves"),
            disposition: wolves_disp,
            goals: vec![FactionGoal::Survive, FactionGoal::RaidResource { resource_node_id: "mine_01".into() }],
            resources: FactionResources { food: 20.0, gold: 5.0, military_strength: 30.0 },
            territory: vec![RegionId("north".into())],
            is_aggressive: false,
            player_default_standing: STANDING_NEUTRAL,
        },
        Faction {
            id: FactionId("merchant_guild".into()),
            name: SmolStr::new("Merchant Guild"),
            disposition: guild_disp,
            goals: vec![FactionGoal::FormAlliance { with: FactionId("iron_wolves".into()), min_trust: 0.5 }, FactionGoal::Survive],
            resources: FactionResources { food: 80.0, gold: 200.0, military_strength: 10.0 },
            territory: vec![RegionId("south".into())],
            is_aggressive: false,
            player_default_standing: STANDING_NEUTRAL,
        },
        Faction {
            id: FactionId("ash_covenant".into()),
            name: SmolStr::new("Ash Covenant"),
            disposition: ash_disp,
            goals: vec![FactionGoal::Survive, FactionGoal::DefendSettlement { settlement_id: "ruins_01".into() }],
            resources: FactionResources { food: 15.0, gold: 0.0, military_strength: 40.0 },
            territory: vec![RegionId("east".into())],
            is_aggressive: true,
            player_default_standing: STANDING_HOSTILE,
        },
        Faction {
            id: FactionId("deep_tide".into()),
            name: SmolStr::new("Deep Tide"),
            disposition: tide_disp,
            goals: vec![FactionGoal::Survive, FactionGoal::RaidResource { resource_node_id: "surface_01".into() }],
            resources: FactionResources { food: 10.0, gold: 0.0, military_strength: 35.0 },
            territory: vec![RegionId("underdark".into())],
            is_aggressive: true,
            player_default_standing: STANDING_HOSTILE,
        },
    ];
}

// ── Population state init ─────────────────────────────────────────────────────

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

/// Military strength recovered per adult NPC per world-sim tick.
/// At 10 adults and 0 strength, recovery rate = 0.5/tick → 15.0 in 30 ticks (~30 s).
const MILITARY_REGEN_PER_ADULT: f32 = 0.05;
/// Hard ceiling on faction military strength.
const MILITARY_CAP: f32 = 50.0;
/// Fraction of prey count added to faction food per tick.
/// 100 prey × 0.01 = 1.0 food/tick.
const PREY_TO_FOOD_RATE: f32 = 0.01;
/// Food upkeep per adult NPC per tick.
/// 200 adults × 0.005 = 1.0 food/tick consumed (balanced against moderate prey).
const FOOD_UPKEEP_PER_ADULT: f32 = 0.005;

/// Advance each settlement's population by one tick.
/// Spawns child NPCs and emits `FormWarPartyEvent` when threshold is reached.
fn tick_population_system(
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
    // Each adult NPC contributes a small regen each tick, capped at MILITARY_CAP.
    for faction in &mut registry.factions {
        let adults = *faction_adults.get(&faction.id).unwrap_or(&0);
        let regen = adults as f32 * MILITARY_REGEN_PER_ADULT;
        faction.resources.military_strength =
            (faction.resources.military_strength + regen).min(MILITARY_CAP);
    }

    // ── Ecology → food integration ────────────────────────────────────────────
    // Sum prey across all ecology regions (global average; faction territories
    // use label IDs that don't map spatially to ecology macro-regions).
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
    let faction_hostile_targets: HashMap<FactionId, Vec<(Uuid, f32, f32)>> = registry.factions
        .iter()
        .map(|f| {
            let hostile_faction_ids: Vec<&FactionId> = f.disposition
                .iter()
                .filter(|(_, d)| **d == Disposition::Hostile)
                .map(|(id, _)| id)
                .collect();
            let targets: Vec<(Uuid, f32, f32)> = pop.settlements
                .values()
                .filter(|s| hostile_faction_ids.contains(&&s.faction_id))
                .map(|s| (s.settlement_id, s.home_x, s.home_y))
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
        let (next, effects) = tick_population(state, targets);

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
///
/// An NPC in the Hot zone (near a player) matures in 300 ticks (~5 min).
/// In the Warm zone (0.25×) it takes ~20 min; in Frozen (0.05×) ~100 min.
/// Aggregate systems (births, faction goals) are unaffected.
fn age_npcs_system(
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
fn check_war_party_formation(
    mut events: MessageReader<FormWarPartyEvent>,
    npc_query: Query<(Entity, &FactionMember, Option<&GrowthStage>), Without<WarPartyMember>>,
    player_q: Query<(Entity, &WorldPosition, &CombatParticipant), With<PlayerStandings>>,
    rep: Res<PlayerReputationMap>,
    registry: Res<FactionRegistry>,
    pop: Res<FactionPopulationState>,
    tick: Res<WorldSimTick>,
    nav: Option<Res<NavGrid>>,
    mut flow_field: ResMut<FlowField>,
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

/// Before marching, refresh the target coordinates for any war party hunting a player.
fn update_war_party_player_targets(
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
fn sync_player_standings(
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

/// Move war-party NPCs toward their target using flow-field pathfinding (Hot/Warm)
/// or linear march (Frozen). Spawn `ActiveBattle` when they arrive.
///
/// # Zone behavior:
/// - **Hot/Warm**: sample the cached flow field at the entity's nav cell,
///   apply direction × MARCH_SPEED × zone_speed.
/// - **Frozen**: keep existing linear march (unchanged behavior, macro-correct).
#[allow(clippy::too_many_arguments)]
fn march_war_parties(
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
/// Also emits `StoryEvent::UnderDarkThreat` when the party's hop distance to
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

        // Emit UnderDarkThreat when hops_to_surface <= 3.
        if war_member.current_zone != fellytip_shared::world::zone::OVERWORLD_ZONE {
            if let Some(hops) = topology.hop_distance(
                war_member.current_zone,
                fellytip_shared::world::zone::OVERWORLD_ZONE,
            ) {
                if hops <= 3 {
                    story_writer.write(WriteStoryEvent(StoryEvent {
                        id: Uuid::new_v4(),
                        tick: tick.0,
                        world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
                        kind: StoryEventKind::UnderDarkThreat {
                            faction_id: war_member.attacker_faction.0.clone(),
                            hops_to_surface: hops,
                        },
                        participants: Vec::new(),
                        location: None,
                        lore_tags: Vec::new(),
                    }));
                }
            }
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

type BattleNpcQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static CombatParticipant,
        &'static FactionMember,
        Option<&'static WarPartyMember>,
        &'static WorldPosition,
        Option<&'static HomePosition>,
    ),
>;

/// Run one combat round per attacker-defender pair for each active battle.
///
/// Battle pace is zone-gated: the `round_acc` accumulator on `ActiveBattle`
/// advances by zone speed each tick and a round only fires when it crosses 1.0.
/// Near a player (Hot) that is every tick; in Frozen it's roughly every 20 ticks.
#[allow(clippy::too_many_arguments)]
fn run_battle_rounds(
    mut battles: Query<(Entity, &mut ActiveBattle)>,
    all_npcs: BattleNpcQuery,
    mut health_query: Query<&mut Health>,
    tick: Res<WorldSimTick>,
    mut registry: ResMut<FactionRegistry>,
    mut commands: Commands,
    mut battle_end: MessageWriter<BattleEndMsg>,
    mut battle_attack: MessageWriter<BattleAttackMsg>,
    temp: Res<ChunkTemperature>,
    mut story_events: MessageWriter<WriteStoryEvent>,
    mut history: ResMut<BattleHistory>,
) {
    for (battle_entity, mut battle) in &mut battles {
        // Advance the fractional accumulator by this tick's zone speed.
        // A round only fires once the accumulator reaches a full tick's worth.
        let speed = temp.speed_at_world(battle.battle_x, battle.battle_y);
        battle.round_acc += speed;
        if battle.round_acc < 1.0 {
            continue;
        }
        battle.round_acc -= 1.0;

        let bx = battle.battle_x;
        let by = battle.battle_y;

        // Collect snapshots of attackers and defenders near the battle site.
        // attacker tuple: (entity, snap, current_hp, home_pos)
        let mut attacker_snaps: Vec<(Entity, CombatantSnapshot, i32, Option<WorldPosition>)> = Vec::new();
        let mut defender_snaps: Vec<(Entity, CombatantSnapshot, i32, Option<WorldPosition>)> = Vec::new();

        for (entity, cp, member, war_member, pos, home_pos) in &all_npcs {
            let dist = ((pos.x - bx).powi(2) + (pos.y - by).powi(2)).sqrt();
            if dist > BATTLE_RADIUS * 4.0 {
                continue;
            }
            let Ok(health) = health_query.get(entity) else { continue };
            let snap = CombatantSnapshot {
                id: cp.id.clone(),
                faction: None,
                class: cp.class.clone(),
                stats: CoreStats {
                    strength: cp.strength,
                    dexterity: cp.dexterity,
                    constitution: cp.constitution,
                    intellect: 10,
                },
                health_current: health.current,
                health_max: health.max,
                level: cp.level,
                armor_class: cp.armor_class,
            };
            let home = home_pos.map(|h| h.0.clone());
            if member.0 == battle.attacker_faction && war_member.is_some() {
                attacker_snaps.push((entity, snap, health.current, home));
            } else if member.0 == battle.defender_faction && war_member.is_none() {
                defender_snaps.push((entity, snap, health.current, home));
            }
        }

        // Battle ends when one side is eliminated.
        if attacker_snaps.is_empty() || defender_snaps.is_empty() {
            let (winner, loser) = if attacker_snaps.is_empty() {
                (
                    battle.defender_faction.0.as_str().to_owned(),
                    battle.attacker_faction.0.as_str().to_owned(),
                )
            } else {
                (
                    battle.attacker_faction.0.as_str().to_owned(),
                    battle.defender_faction.0.as_str().to_owned(),
                )
            };
            tracing::info!(
                winner = %winner,
                atk_cas = battle.attacker_casualties,
                def_cas = battle.defender_casualties,
                "Battle ended"
            );
            battle_end.write(BattleEndMsg {
                settlement_id: battle.settlement_id,
                winner_faction: winner.clone(),
                attacker_casualties: battle.attacker_casualties,
                defender_casualties: battle.defender_casualties,
            });
            history.push(BattleRecord {
                winner_faction: winner,
                loser_faction: loser,
                target_settlement_id: battle.settlement_id.to_string(),
                tick: tick.0,
                attacker_casualties: battle.attacker_casualties,
                defender_casualties: battle.defender_casualties,
            });

            // Emit a story event for settlement destruction when attackers win.
            if !attacker_snaps.is_empty() {
                story_events.write(WriteStoryEvent(StoryEvent {
                    id: Uuid::new_v4(),
                    tick: tick.0,
                    world_day: (tick.0 / 300) as u32,
                    kind: StoryEventKind::SettlementRazed { by: battle.attacker_faction.clone() },
                    participants: vec![],
                    location: Some(IVec2::new(battle.battle_x as i32, battle.battle_y as i32)),
                    lore_tags: vec!["settlement".into(), "war".into()],
                }));
            }

            // Update losing faction's military strength.
            if attacker_snaps.is_empty() {
                // Defenders won.
                if let Some(f) = registry.factions.iter_mut().find(|f| f.id == battle.attacker_faction) {
                    f.resources.military_strength = (f.resources.military_strength - battle.attacker_casualties.min(10) as f32).max(0.0);
                }
            } else {
                // Attackers won.
                if let Some(f) = registry.factions.iter_mut().find(|f| f.id == battle.defender_faction) {
                    f.resources.military_strength = (f.resources.military_strength - battle.defender_casualties.min(10) as f32).max(0.0);
                }
            }

            // Remove WarPartyMember from surviving attackers and teleport them home.
            for (entity, _, _, home) in &attacker_snaps {
                let mut cmd = commands.entity(*entity);
                cmd.remove::<WarPartyMember>();
                if let Some(home_pos) = home {
                    cmd.insert(home_pos.clone());
                }
            }
            commands.entity(battle_entity).despawn();
            continue;
        }

        // Build combined CombatState for this tick's rounds.
        let all_combatants: Vec<CombatantState> = attacker_snaps.iter().chain(defender_snaps.iter())
            .map(|(_, snap, hp, _)| CombatantState { snapshot: snap.clone(), health: *hp, statuses: vec![] })
            .collect();
        let mut state = CombatState { combatants: all_combatants, round: tick.0 as u32 };

        let mut dice = seeded_dice(battle.settlement_id, tick.0);

        // Each attacker targets a defender (seeded round-robin).
        let def_count = defender_snaps.len();
        for (atk_idx, (_, atk_snap, _, _)) in attacker_snaps.iter().enumerate() {
            let def_idx = (atk_idx + tick.0 as usize) % def_count;
            let (def_entity, def_snap, _, _) = &defender_snaps[def_idx];

            let (next_state, effects) = tick_battle_round(state.clone(), &atk_snap.id, &def_snap.id, &mut dice);
            state = next_state;

            for effect in &effects {
                match effect {
                    Effect::TakeDamage { target, amount } => {
                        // Find the entity matching this CombatantId.
                        let target_entity = attacker_snaps.iter().chain(defender_snaps.iter())
                            .find(|(_, s, _, _)| &s.id == target)
                            .map(|(e, _, _, _)| *e);
                        if let Some(entity) = target_entity {
                            let is_defender = entity == *def_entity;
                            let atk_msg = BattleAttackMsg {
                                target_combatant_id: target.0,
                                damage: *amount,
                                is_kill: false,
                            };
                            battle_attack.write(atk_msg);
                            let _ = is_defender; // casualties tracked on Die effect
                        }
                    }
                    Effect::Die { target } => {
                        let target_entity = attacker_snaps.iter().chain(defender_snaps.iter())
                            .find(|(_, s, _, _)| &s.id == target)
                            .map(|(e, _, _, _)| *e);
                        if let Some(entity) = target_entity {
                            let is_attacker = attacker_snaps.iter().any(|(e, _, _, _)| *e == entity);
                            if is_attacker {
                                battle.attacker_casualties += 1;
                            } else {
                                battle.defender_casualties += 1;
                            }
                            let kill_msg = BattleAttackMsg {
                                target_combatant_id: target.0,
                                damage: 0,
                                is_kill: true,
                            };
                            battle_attack.write(kill_msg);
                            commands.entity(entity).despawn();
                        }
                    }
                    _ => {}
                }
            }
        }

        // Sync health from the updated CombatState back to ECS Health components.
        for (entity, snap, _, _) in attacker_snaps.iter().chain(defender_snaps.iter()) {
            if let Some(cs) = state.get(&snap.id) {
                if let Ok(mut health) = health_query.get_mut(*entity) {
                    health.current = cs.health;
                }
            }
        }
    }
}

/// Separation radius in world tiles — repulse members closer than this.
const SEPARATION_RADIUS: f32 = 1.5;
/// Repulsion gain: push apart by `(radius - dist) * gain` tiles.
const SEPARATION_GAIN: f32 = 0.5;
/// Circle radius for Frozen zone formation offsets.
const FROZEN_FORMATION_RADIUS: f32 = 1.5;

/// Apply lightweight pairwise repulsion within each war party group (Hot/Warm zones).
///
/// Frozen zone: instead of repulsion, hold each member at a fixed offset from
/// the group centroid arranged in a circle of radius `FROZEN_FORMATION_RADIUS`.
fn war_party_separation(
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

// ── Underdark pressure (UnderDarkSimSchedule @ 0.1 Hz) ────────────────────────

/// Minimum elapsed ticks since last raid before natural buildup kicks in.
const UNDERDARK_NATURAL_BUILDUP_AFTER_TICKS: u64 = 300;
/// Decay multiplier applied each slow tick: `pressure *= DECAY`.
const UNDERDARK_DECAY: f32 = 0.95;
/// Pressure boost while any war party is currently in the Underdark.
const UNDERDARK_ACTIVE_BOOST: f32 = 0.1;
/// Natural buildup (when the last raid was long enough ago).
const UNDERDARK_NATURAL_BOOST: f32 = 0.05;
/// Threshold bit layout for hysteresis tracking.
const UNDERDARK_THRESHOLD_DISTANT_BIT: u8 = 1 << 0; // score >= 0.4
const UNDERDARK_THRESHOLD_IMMINENT_BIT: u8 = 1 << 1; // score >= 0.7

/// Tick the Underdark pressure score on `UnderDarkSimSchedule` (0.1 Hz).
///
/// - Decays toward 0 at `DECAY` each slow tick (~2 minutes to zero with no input)
/// - +`ACTIVE_BOOST` if any `WarPartyMember` currently occupies an Underdark zone
/// - +`NATURAL_BOOST` if it has been >= 300 WorldSim ticks since the last raid
/// - Clamps to [0.0, 1.0]
fn accumulate_underdark_pressure(
    mut pressure: ResMut<UnderDarkPressure>,
    tick: Res<WorldSimTick>,
    zone_registry: Option<Res<fellytip_shared::world::zone::ZoneRegistry>>,
    warriors: Query<&WarPartyMember>,
) {
    // Decay first so bumps accumulate on top of a lower floor.
    pressure.score *= UNDERDARK_DECAY;

    // Check if any war party is currently in an Underdark zone.
    if let Some(registry) = zone_registry.as_ref() {
        let any_in_underdark = warriors.iter().any(|wm| {
            registry
                .get(wm.current_zone)
                .map(|zone| matches!(
                    zone.kind,
                    fellytip_shared::world::zone::ZoneKind::Underdark { .. }
                ))
                .unwrap_or(false)
        });
        if any_in_underdark {
            pressure.score += UNDERDARK_ACTIVE_BOOST;
        }
    }

    // Natural buildup if enough time has passed since the last raid.
    if tick.0.saturating_sub(pressure.last_raid_tick) > UNDERDARK_NATURAL_BUILDUP_AFTER_TICKS {
        pressure.score += UNDERDARK_NATURAL_BOOST;
    }

    pressure.score = pressure.score.clamp(0.0, 1.0);
}

/// Emit `StoryEvent::UnderDarkThreat` when the pressure score crosses each
/// threshold (hysteresis: latched while >= threshold, cleared when < 0.4).
fn deliver_underdark_signals(
    mut pressure: ResMut<UnderDarkPressure>,
    tick: Res<WorldSimTick>,
    mut story_writer: MessageWriter<WriteStoryEvent>,
) {
    let score = pressure.score;

    // Distant signal at 0.4 (99 hops).
    if score >= 0.4 && (pressure.thresholds_crossed & UNDERDARK_THRESHOLD_DISTANT_BIT) == 0 {
        pressure.thresholds_crossed |= UNDERDARK_THRESHOLD_DISTANT_BIT;
        story_writer.write(WriteStoryEvent(StoryEvent {
            id: Uuid::new_v4(),
            tick: tick.0,
            world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
            kind: StoryEventKind::UnderDarkThreat {
                faction_id: SmolStr::new("underdark"),
                hops_to_surface: 99,
            },
            participants: Vec::new(),
            location: None,
            lore_tags: vec!["underdark".into(), "distant".into()],
        }));
        tracing::info!(score, "Underdark distant signal fired");
    }

    // Imminent signal at 0.7 (2 hops).
    if score >= 0.7 && (pressure.thresholds_crossed & UNDERDARK_THRESHOLD_IMMINENT_BIT) == 0 {
        pressure.thresholds_crossed |= UNDERDARK_THRESHOLD_IMMINENT_BIT;
        story_writer.write(WriteStoryEvent(StoryEvent {
            id: Uuid::new_v4(),
            tick: tick.0,
            world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
            kind: StoryEventKind::UnderDarkThreat {
                faction_id: SmolStr::new("underdark"),
                hops_to_surface: 2,
            },
            participants: Vec::new(),
            location: None,
            lore_tags: vec!["underdark".into(), "imminent".into(), "fleeing".into()],
        }));
        tracing::info!(score, "Underdark imminent signal fired");
    }

    // Hysteresis: reset all latched bits when pressure falls below 0.4.
    if score < 0.4 && pressure.thresholds_crossed != 0 {
        pressure.thresholds_crossed = 0;
    }
}

// ── Underdark raid spawn (WorldSimSchedule) ──────────────────────────────────

/// Number of `WarPartyMember` entities spawned per Underdark raid.
const UNDERDARK_RAID_PARTY_SIZE: u32 = 3;
/// Minimum pressure score before a raid is spawned.
const UNDERDARK_RAID_THRESHOLD: f32 = 0.8;

/// When pressure is high enough, spawn a `UNDERDARK_RAID_PARTY_SIZE`-member
/// raid party in the deepest Underdark zone and route them to the overworld.
///
/// Runs on `WorldSimSchedule` (1 Hz). Gated so only one raid is active at a
/// time; successful spawns reset `pressure.score` to zero and record the tick.
#[allow(clippy::too_many_arguments)]
fn spawn_underdark_raid(
    mut commands: Commands,
    mut pressure: ResMut<UnderDarkPressure>,
    zone_registry: Option<Res<fellytip_shared::world::zone::ZoneRegistry>>,
    zone_topology: Option<Res<fellytip_shared::world::zone::ZoneTopology>>,
    pop: Res<FactionPopulationState>,
    tick: Res<WorldSimTick>,
    warriors: Query<&WarPartyMember>,
) {
    if pressure.score < UNDERDARK_RAID_THRESHOLD {
        return;
    }

    let Some(registry) = zone_registry else { return };
    let Some(topology) = zone_topology else { return };

    // Only one active Underdark raid at a time.
    let already_active = warriors.iter().any(|wm| {
        registry
            .get(wm.current_zone)
            .map(|z| matches!(z.kind, fellytip_shared::world::zone::ZoneKind::Underdark { .. }))
            .unwrap_or(false)
            || wm.attacker_faction.0.as_str() == "underdark"
    });
    if already_active {
        return;
    }

    // Find the deepest Underdark zone (highest `depth`).
    let deepest = registry
        .zones
        .iter()
        .filter_map(|(id, zone)| match zone.kind {
            fellytip_shared::world::zone::ZoneKind::Underdark { depth } => Some((*id, depth, zone)),
            _ => None,
        })
        .max_by_key(|(_, depth, _)| *depth);
    let Some((deepest_id, _, deepest_zone)) = deepest else {
        tracing::warn!("No Underdark zones in registry; skipping raid spawn");
        return;
    };

    // Find highest-population surface settlement (defender side).
    let target = pop
        .settlements
        .values()
        .max_by_key(|s| s.adult_count)
        .map(|s| (s.settlement_id, s.home_x, s.home_y));
    let (target_sid, target_x, target_y) = match target {
        Some(t) => t,
        None => {
            tracing::warn!("No populated settlements for Underdark raid target; skipping spawn");
            return;
        }
    };

    // Compute zone route deepest → OVERWORLD_ZONE via BFS.
    let Some(zone_route) = shortest_zone_path(
        &topology,
        deepest_id,
        fellytip_shared::world::zone::OVERWORLD_ZONE,
    ) else {
        tracing::warn!(
            deepest = ?deepest_id,
            "No zone path from deepest Underdark to overworld; skipping raid spawn"
        );
        return;
    };

    // Spawn origin: center of deepest Underdark zone's tile grid (local coords).
    let spawn_x = deepest_zone.width as f32 * 0.5;
    let spawn_y = deepest_zone.height as f32 * 0.5;

    let underdark_fid = FactionId(SmolStr::new("underdark"));

    for i in 0..UNDERDARK_RAID_PARTY_SIZE {
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
                faction_id: "underdark".to_string(),
                rank: NpcRank::Grunt,
            },
            FactionNpcRank(NpcRank::Grunt),
            EntityKind::FactionNpc,
            HomePosition(pos),
            WarPartyMember {
                target_settlement_id: target_sid,
                target_x,
                target_y,
                attacker_faction: underdark_fid.clone(),
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
        "Underdark raid party spawned"
    );
}

/// BFS shortest zone-hop path from `from` to `to` over `ZoneTopology`.
/// Returns the list of zones to hop into (excluding `from`, including `to`),
/// or `None` if unreachable.
fn shortest_zone_path(
    topology: &fellytip_shared::world::zone::ZoneTopology,
    from: fellytip_shared::world::zone::ZoneId,
    to: fellytip_shared::world::zone::ZoneId,
) -> Option<Vec<fellytip_shared::world::zone::ZoneId>> {
    use std::collections::{HashMap, VecDeque};
    if from == to {
        return Some(Vec::new());
    }
    let mut parent: HashMap<
        fellytip_shared::world::zone::ZoneId,
        fellytip_shared::world::zone::ZoneId,
    > = HashMap::new();
    let mut queue: VecDeque<fellytip_shared::world::zone::ZoneId> = VecDeque::new();
    queue.push_back(from);
    parent.insert(from, from);
    while let Some(cur) = queue.pop_front() {
        for next in topology.neighbors(cur) {
            if parent.contains_key(&next) {
                continue;
            }
            parent.insert(next, cur);
            if next == to {
                // Reconstruct path.
                let mut path = Vec::new();
                let mut at = to;
                while at != from {
                    path.push(at);
                    at = *parent.get(&at)?;
                }
                path.reverse();
                return Some(path);
            }
            queue.push_back(next);
        }
    }
    None
}

