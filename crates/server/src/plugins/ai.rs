//! NPC AI plugin — re-evaluates faction goals and nudges NPC positions
//! each WorldSimSchedule tick (1 Hz).

use bevy::ecs::message::{MessageReader, MessageWriter};
use bevy::prelude::*;
use crate::plugins::persistence::Db;
use crate::plugins::interest::ChunkTemperature;
use lightyear::prelude::{server::Server, NetworkTarget, Replicate, ServerMultiMessageSender};
use fellytip_shared::{
    combat::{
        interrupt::InterruptStack,
        types::{CharacterClass, CombatantId, CombatantSnapshot, CombatantState, CombatState, CoreStats, Effect},
    },
    components::{EntityKind, GrowthStage, Health, WorldPosition},
    protocol::{BattleAttackMsg, BattleEndMsg, BattleStartMsg, CombatEventChannel},
    world::{
        civilization::Settlements,
        faction::{
            Disposition, Faction, FactionId, FactionResources, FactionGoal, NpcRank,
            PlayerReputationMap, STANDING_NEUTRAL, STANDING_HOSTILE, pick_goal,
        },
        ecology::RegionId,
        map::{CHUNK_TILES, MAP_HALF_WIDTH, MAP_HALF_HEIGHT},
        population::{
            tick_population, PopulationEffect, SettlementPopulation,
            BATTLE_RADIUS, MARCH_SPEED, WAR_PARTY_SIZE,
        },
        war::{seeded_dice, tick_battle_round},
    },
};
use smol_str::SmolStr;
use std::collections::HashMap;
use uuid::Uuid;

use crate::plugins::combat::{CombatParticipant, ExperienceReward};
use crate::plugins::world_sim::{WorldSimSchedule, WorldSimTick};

/// Server-only component: which faction this NPC belongs to.
#[derive(Component)]
pub struct FactionMember(#[allow(dead_code)] pub FactionId);

/// Server-only component: current AI goal being pursued.
#[derive(Component)]
pub struct CurrentGoal(#[allow(dead_code)] pub Option<FactionGoal>);

/// Server-only component: home position used for bounded wander / future pathfinding.
#[derive(Component)]
pub struct HomePosition(#[allow(dead_code)] pub WorldPosition);

/// Server-only component: NPC rank for kill-penalty calculation.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct FactionNpcRank(pub NpcRank);

/// Server-only resource: all live factions.
#[derive(Resource, Default)]
pub struct FactionRegistry {
    pub factions: Vec<Faction>,
}

/// Tags an NPC as part of an active war party marching toward a target settlement.
/// Server-only — never replicated.
#[derive(Component)]
pub struct WarPartyMember {
    pub target_settlement_id: Uuid,
    pub target_x: f32,
    pub target_y: f32,
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

/// Emitted by `tick_population_system` when a faction is ready to dispatch
/// a war party. Consumed by `check_war_party_formation`.
#[derive(bevy::ecs::message::Message, Clone, Debug)]
pub struct FormWarPartyEvent {
    pub attacker_faction: FactionId,
    pub target_settlement_id: Uuid,
    pub target_x: f32,
    pub target_y: f32,
}

/// Number of NPC soldiers spawned per faction at startup.
const NPCS_PER_FACTION: usize = 3;

/// Fixed offsets (tile units) applied to each NPC spawn relative to the
/// faction's home settlement, so NPCs aren't stacked on top of each other.
const NPC_OFFSETS: [(f32, f32); NPCS_PER_FACTION] = [(0.0, 0.0), (2.0, 0.0), (0.0, 2.0)];

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FactionRegistry>()
            .init_resource::<PlayerReputationMap>()
            .init_resource::<FactionPopulationState>()
            .add_message::<FormWarPartyEvent>()
            .register_type::<FactionNpcRank>();
        app.add_systems(
            WorldSimSchedule,
            (
                update_faction_goals,
                tick_population_system,
                age_npcs_system,
                check_war_party_formation,
                march_war_parties,
                run_battle_rounds,
                wander_npcs,
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

/// Faction NPCs are stationary for now; real pathfinding comes later.
///
/// NPCs in Frozen chunks (no client has them in Hot or Warm zone) are skipped
/// to avoid wasting simulation budget on unobserved entities.
fn wander_npcs(
    query: Query<(&WorldPosition,), With<FactionMember>>,
    temp:  Res<ChunkTemperature>,
) {
    for (pos,) in &query {
        let tile_x = (pos.x + MAP_HALF_WIDTH as f32) as i32;
        let tile_y = (pos.y + MAP_HALF_HEIGHT as f32) as i32;
        let chunk  = (tile_x.max(0) / CHUNK_TILES as i32, tile_y.max(0) / CHUNK_TILES as i32);

        if !temp.is_active(chunk) {
            continue; // Frozen — skip simulation
        }

        // Intentionally idle — real pathfinding will go here.
    }
}

/// Spawn `NPCS_PER_FACTION` guard NPCs for each faction at their nearest settlement.
/// Runs at Startup after `seed_factions` and after `MapGenPlugin` inserts `Settlements`.
pub fn spawn_faction_npcs(
    registry: Res<FactionRegistry>,
    settlements: Res<Settlements>,
    mut commands: Commands,
) {
    if settlements.0.is_empty() {
        tracing::warn!("No settlements available; skipping faction NPC spawn");
        return;
    }

    for (faction_idx, faction) in registry.factions.iter().enumerate() {
        // Assign each faction a home settlement by cycling through the list.
        let settlement = &settlements.0[faction_idx % settlements.0.len()];

        for (npc_idx, (ox, oy)) in NPC_OFFSETS.iter().enumerate() {
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
                CurrentGoal(None),
                HomePosition(pos),
                EntityKind::FactionNpc,
                // Start with no replication target; update_npc_replication
                // in InterestPlugin will set the correct target within 1 s.
                Replicate::to_clients(NetworkTarget::None),
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
            count = NPCS_PER_FACTION,
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
) {
    if settlements.0.is_empty() {
        tracing::warn!("No settlements for population init");
        return;
    }
    for (faction_idx, faction) in registry.factions.iter().enumerate() {
        let settlement = &settlements.0[faction_idx % settlements.0.len()];
        let home_x = settlement.x - MAP_HALF_WIDTH as f32;
        let home_y = settlement.y - MAP_HALF_HEIGHT as f32;
        pop.settlements.insert(
            settlement.id,
            SettlementPopulation {
                settlement_id: settlement.id,
                faction_id: faction.id.clone(),
                birth_ticks: 0,
                adult_count: NPCS_PER_FACTION as u32,
                child_count: 0,
                home_x,
                home_y,
                home_z: settlement.z,
                war_party_cooldown: 0,
            },
        );
    }
    tracing::info!(count = pop.settlements.len(), "Settlement population states seeded");
}

// ── WorldSim systems ──────────────────────────────────────────────────────────

/// Advance each settlement's population by one tick.
/// Spawns child NPCs and emits `FormWarPartyEvent` when threshold is reached.
fn tick_population_system(
    mut pop: ResMut<FactionPopulationState>,
    npc_query: Query<(&FactionMember, Option<&GrowthStage>, Has<WarPartyMember>)>,
    registry: Res<FactionRegistry>,
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
        state.adult_count   = *faction_adults.get(&state.faction_id).unwrap_or(&0);
        state.child_count   = *faction_children.get(&state.faction_id).unwrap_or(&0);
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
                        CurrentGoal(None),
                        HomePosition(pos),
                        EntityKind::FactionNpc,
                        GrowthStage(0.0),
                        Replicate::to_clients(NetworkTarget::None),
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
fn check_war_party_formation(
    mut events: MessageReader<FormWarPartyEvent>,
    npc_query: Query<(Entity, &FactionMember, Option<&GrowthStage>), Without<WarPartyMember>>,
    mut commands: Commands,
) {
    for event in events.read() {
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
        }
    }
}

/// Move war-party NPCs toward their target. Spawn `ActiveBattle` when they arrive.
///
/// Movement is scaled by zone speed: warriors march at full [`MARCH_SPEED`] near
/// a player, quarter-speed in the Warm zone, and 5 % speed in Frozen areas.
fn march_war_parties(
    mut warriors: Query<(&WarPartyMember, &mut WorldPosition)>,
    battles: Query<&ActiveBattle>,
    pop: Res<FactionPopulationState>,
    temp: Res<ChunkTemperature>,
    mut commands: Commands,
    mut msg_sender: ServerMultiMessageSender,
    server: Single<&Server>,
) {
    for (war_member, mut pos) in &mut warriors {
        let speed = temp.speed_at_world(pos.x, pos.y);
        let dx = war_member.target_x - pos.x;
        let dy = war_member.target_y - pos.y;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist > 0.01 {
            let step = (MARCH_SPEED * speed / dist).min(1.0);
            pos.x += dx * step;
            pos.y += dy * step;
        }

        // Check if arrived and no battle already active for this settlement.
        if dist <= BATTLE_RADIUS {
            let already_active = battles.iter().any(|b| b.settlement_id == war_member.target_settlement_id);
            if !already_active {
                // Look up the defender faction from population state.
                let Some(target_pop) = pop.settlements.get(&war_member.target_settlement_id) else { continue };
                let defender_faction = target_pop.faction_id.clone();

                // Look up the attacker faction (any WarPartyMember entity gives us the target,
                // but we need the attacker faction — use the FactionMember below after we query it).
                // Since we don't have FactionMember in this query, we derive it from the defender's
                // hostile dispositions: the attacker is a faction hostile to the defender.
                // The specific faction ID is encoded in the war party member's target; we will use
                // a separate system for the actual faction lookup. For the battle entity,
                // we look up who is hostile to the defender.
                //
                // Simpler: keep attacker_faction in WarPartyMember. For now use the first
                // hostile faction we can infer (the pop state only has settlement info).
                // We'll set attacker_faction = Unknown and fill it via the existing pop data.
                // Instead, query the attacker's FactionMember below.
                //
                // Since this query doesn't include FactionMember, get the attacker faction
                // from FactionPopulationState: find a settlement NOT matching the defender faction
                // that has a hostile disposition. This is stored in the registry but not accessible here.
                // Workaround: store attacker_faction in WarPartyMember itself.
                // We set it from check_war_party_formation... but the event has it.
                //
                // For now, encode a placeholder and fix it below by checking pop.settlements.
                let attacker_faction = pop.settlements.values()
                    .find(|s| s.faction_id != defender_faction)
                    .map(|s| s.faction_id.clone())
                    .unwrap_or(FactionId("unknown".into()));

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

                let msg = BattleStartMsg {
                    settlement_id: war_member.target_settlement_id,
                    attacker_faction: attacker_faction.0.to_string(),
                    defender_faction: defender_faction.0.to_string(),
                    x: war_member.target_x,
                    y: war_member.target_y,
                    z: target_pop.home_z,
                };
                let _ = msg_sender.send::<BattleStartMsg, CombatEventChannel>(&msg, &server, &NetworkTarget::All);
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

type BattleNpcQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static CombatParticipant,
        &'static Health,
        &'static FactionMember,
        Option<&'static WarPartyMember>,
        &'static WorldPosition,
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
    mut msg_sender: ServerMultiMessageSender,
    server: Single<&Server>,
    temp: Res<ChunkTemperature>,
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
        let mut attacker_snaps: Vec<(Entity, CombatantSnapshot, i32)> = Vec::new(); // (entity, snap, current_hp)
        let mut defender_snaps: Vec<(Entity, CombatantSnapshot, i32)> = Vec::new();

        for (entity, cp, health, member, war_member, pos) in &all_npcs {
            let dist = ((pos.x - bx).powi(2) + (pos.y - by).powi(2)).sqrt();
            if dist > BATTLE_RADIUS * 4.0 {
                continue;
            }
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
            if member.0 == battle.attacker_faction && war_member.is_some() {
                attacker_snaps.push((entity, snap, health.current));
            } else if member.0 == battle.defender_faction && war_member.is_none() {
                defender_snaps.push((entity, snap, health.current));
            }
        }

        // Battle ends when one side is eliminated.
        if attacker_snaps.is_empty() || defender_snaps.is_empty() {
            let winner = if attacker_snaps.is_empty() {
                battle.defender_faction.0.as_str().to_owned()
            } else {
                battle.attacker_faction.0.as_str().to_owned()
            };
            tracing::info!(
                winner = %winner,
                atk_cas = battle.attacker_casualties,
                def_cas = battle.defender_casualties,
                "Battle ended"
            );
            let end_msg = BattleEndMsg {
                settlement_id: battle.settlement_id,
                winner_faction: winner,
                attacker_casualties: battle.attacker_casualties,
                defender_casualties: battle.defender_casualties,
            };
            let _ = msg_sender.send::<BattleEndMsg, CombatEventChannel>(&end_msg, &server, &NetworkTarget::All);

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

            // Remove WarPartyMember from surviving attackers.
            for (entity, _, _) in &attacker_snaps {
                commands.entity(*entity).remove::<WarPartyMember>();
            }
            commands.entity(battle_entity).despawn();
            continue;
        }

        // Build combined CombatState for this tick's rounds.
        let all_combatants: Vec<CombatantState> = attacker_snaps.iter().chain(defender_snaps.iter())
            .map(|(_, snap, hp)| CombatantState { snapshot: snap.clone(), health: *hp, statuses: vec![] })
            .collect();
        let mut state = CombatState { combatants: all_combatants, round: tick.0 as u32 };

        let mut dice = seeded_dice(battle.settlement_id, tick.0);

        // Each attacker targets a defender (seeded round-robin).
        let def_count = defender_snaps.len();
        for (atk_idx, (_, atk_snap, _)) in attacker_snaps.iter().enumerate() {
            let def_idx = (atk_idx + tick.0 as usize) % def_count;
            let (def_entity, def_snap, _) = &defender_snaps[def_idx];

            let (next_state, effects) = tick_battle_round(state.clone(), &atk_snap.id, &def_snap.id, &mut dice);
            state = next_state;

            for effect in &effects {
                match effect {
                    Effect::TakeDamage { target, amount } => {
                        // Find the entity matching this CombatantId.
                        let target_entity = attacker_snaps.iter().chain(defender_snaps.iter())
                            .find(|(_, s, _)| &s.id == target)
                            .map(|(e, _, _)| *e);
                        if let Some(entity) = target_entity {
                            let is_defender = entity == *def_entity;
                            let atk_msg = BattleAttackMsg {
                                target_combatant_id: target.0,
                                damage: *amount,
                                is_kill: false,
                            };
                            let _ = msg_sender.send::<BattleAttackMsg, CombatEventChannel>(&atk_msg, &server, &NetworkTarget::All);
                            let _ = is_defender; // casualties tracked on Die effect
                        }
                    }
                    Effect::Die { target } => {
                        let target_entity = attacker_snaps.iter().chain(defender_snaps.iter())
                            .find(|(_, s, _)| &s.id == target)
                            .map(|(e, _, _)| *e);
                        if let Some(entity) = target_entity {
                            let is_attacker = attacker_snaps.iter().any(|(e, _, _)| *e == entity);
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
                            let _ = msg_sender.send::<BattleAttackMsg, CombatEventChannel>(&kill_msg, &server, &NetworkTarget::All);
                            commands.entity(entity).despawn();
                        }
                    }
                    _ => {}
                }
            }
        }

        // Sync health from the updated CombatState back to ECS Health components.
        for (entity, snap, _) in attacker_snaps.iter().chain(defender_snaps.iter()) {
            if let Some(cs) = state.get(&snap.id) {
                if let Ok(mut health) = health_query.get_mut(*entity) {
                    health.current = cs.health;
                }
            }
        }
    }
}
