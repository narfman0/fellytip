//! Surface danger systems — issues #117, #118, #119, #120, #121.
//!
//! ## Systems
//! - `update_danger_levels`   (#120): recomputes `DangerLevel` per map region each tick.
//! - `spawn_bandit_groups`    (#117): periodically spawns bandit groups in wilderness tiles.
//! - `spawn_portal_horrors`   (#118): spawns horrors from Casino/Fungus portal locations.
//! - `resolve_warfront_events`(#119): processes WarfrontEvent for battles and aftermath.
//! - `update_threat_registry` (#121): tracks player kills and escalates bounties.
//!
//! All systems run on `WorldSimSchedule` (1 Hz) in the chain added by `AiPlugin`.

use bevy::ecs::message::MessageWriter;
use bevy::prelude::*;
use fellytip_shared::{
    combat::{
        interrupt::InterruptStack,
        types::{CharacterClass, CombatantId},
    },
    components::{AbilityModifiers, AbilityScores, EntityKind, FactionBadge, Health, HitDice, NavPath, NavReplanTimer, NpcClass, NpcLevel, PlayerStandings, SavingThrowProficiencies, WorldPosition},
    world::{
        civilization::Settlements,
        faction::{FactionId, NpcRank, PlayerReputationMap},
        map::{MAP_HALF_HEIGHT, MAP_HALF_WIDTH, WorldMap},
        story::{StoryEvent, StoryEventKind, WriteStoryEvent},
        zone::{PortalKind, WorldId, WORLD_DEVILS_CASINO, WORLD_HIVEMIND_FUNGUS, ZoneTopology},
    },
};
use smol_str::SmolStr;
use std::collections::HashMap;
use uuid::Uuid;

use crate::plugins::{
    combat::{CombatParticipant, ExperienceReward},
    world_sim::WorldSimTick,
};

use super::{
    CurrentGoal, FactionMember, FactionNpcRank, FactionPopulationState, FactionRegistry,
    HomePosition, WarPartyMember,
};

// ── Constants ──────────────────────────────────────────────────────────────────

/// Safe zone radius from nearest settlement (tiles).
pub const DANGER_SAFE_RADIUS: f32 = 30.0;
/// Moderate danger zone outer radius.
pub const DANGER_MODERATE_RADIUS: f32 = 80.0;

/// Ticks between bandit group spawns (per 50 wilderness tiles).
pub const BANDIT_SPAWN_INTERVAL: u64 = 100;
/// Bandit patrol wander radius in tiles.
pub const BANDIT_PATROL_RADIUS: f32 = 20.0;
/// Bandit attack detection radius in tiles.
pub const BANDIT_ATTACK_RADIUS: f32 = 6.0;
/// Members per bandit group.
pub const BANDIT_GROUP_SIZE_MIN: u32 = 2;
pub const BANDIT_GROUP_SIZE_MAX: u32 = 5;

/// Horror spawn timer (ticks). Also triggered by UndergroundPressure >= 0.8.
pub const HORROR_SPAWN_INTERVAL: u64 = 500;

/// Iron Wolves kill count thresholds for bounty escalation.
pub const BOUNTY_THRESHOLD_HUNTER: u32 = 3;
pub const BOUNTY_THRESHOLD_BOSS: u32 = 5;
pub const BOUNTY_THRESHOLD_WAR_PARTY: u32 = 10;
/// Ticks without a kill before bounty clears.
pub const BOUNTY_CLEAR_TICKS: u64 = 1000;

// ── Components ─────────────────────────────────────────────────────────────────

/// Attached to bandit group spawner entities.
#[derive(Component, Clone, Debug)]
pub struct BanditCamp {
    pub spawn_tile: (u32, u32),
    pub patrol_radius: f32,
}

/// Attached to horror entities that emerged from a portal.
#[derive(Component, Clone, Debug)]
pub struct PortalHorror {
    pub world_origin: WorldId,
}

/// Per-region danger classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DangerTier {
    Safe,
    Moderate,
    Dangerous,
}

/// Resource: danger tier for each map region quadrant (keyed by coarse grid cell).
#[derive(Resource, Default)]
pub struct DangerLevel {
    /// Map from coarse tile (x / 50, y / 50) to danger tier.
    pub grid: HashMap<(i32, i32), DangerTier>,
}

impl DangerLevel {
    /// Look up the danger tier for world-space `(wx, wy)` in tile coordinates.
    pub fn tier_at(&self, tile_x: f32, tile_y: f32) -> DangerTier {
        let gx = (tile_x / 50.0).floor() as i32;
        let gy = (tile_y / 50.0).floor() as i32;
        self.grid.get(&(gx, gy)).copied().unwrap_or(DangerTier::Moderate)
    }
}

// ── Resource: WarfrontEvent ────────────────────────────────────────────────────

/// Active warfront between two factions' settlements.
#[derive(Clone, Debug)]
pub struct WarfrontEvent {
    pub attacker_faction: FactionId,
    pub defender_faction: FactionId,
    pub target_settlement_id: Uuid,
    pub target_x: f32,
    pub target_y: f32,
    pub resolved: bool,
}

/// Resource tracking all live warfronts.
#[derive(Resource, Default)]
pub struct WarfrontRegistry {
    pub fronts: Vec<WarfrontEvent>,
}

// ── Resource: ThreatRegistry ────────────────────────────────────────────────────

/// Bounty level for a player against the Iron Wolves faction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BountyLevel {
    #[default]
    None,
    /// Named-rank Ranger bounty hunter dispatched.
    Hunter,
    /// Boss-rank Hunter dispatched.
    BossHunter,
    /// Full war party sent.
    WarParty,
}

/// Per-player faction kill tracking and bounty state.
#[derive(Clone, Debug, Default)]
pub struct PlayerThreat {
    /// Kill counts keyed by faction id string.
    pub faction_kills: HashMap<String, u32>,
    /// Tick of last Iron Wolves kill (for bounty expiry).
    pub last_iw_kill_tick: u64,
    /// Current bounty level with Iron Wolves.
    pub iw_bounty: BountyLevel,
}


/// Resource: tracks player kill counts and bounty states.
#[derive(Resource, Default)]
pub struct ThreatRegistry {
    /// Keyed by player `CombatantId` UUID.
    pub players: HashMap<Uuid, PlayerThreat>,
}

impl ThreatRegistry {
    /// Record a kill of an Iron Wolves NPC.
    pub fn record_iw_kill(&mut self, player_id: Uuid, tick: u64) {
        let entry = self.players.entry(player_id).or_default();
        *entry.faction_kills.entry("iron_wolves".to_string()).or_insert(0) += 1;
        entry.last_iw_kill_tick = tick;
        let kills = entry.faction_kills.get("iron_wolves").copied().unwrap_or(0);
        entry.iw_bounty = if kills >= BOUNTY_THRESHOLD_WAR_PARTY {
            BountyLevel::WarParty
        } else if kills >= BOUNTY_THRESHOLD_BOSS {
            BountyLevel::BossHunter
        } else if kills >= BOUNTY_THRESHOLD_HUNTER {
            BountyLevel::Hunter
        } else {
            BountyLevel::None
        };
    }

    /// Clear expired bounties (no IW kills for BOUNTY_CLEAR_TICKS).
    pub fn tick_expiry(&mut self, current_tick: u64) {
        for entry in self.players.values_mut() {
            if entry.iw_bounty != BountyLevel::None
                && current_tick.saturating_sub(entry.last_iw_kill_tick) >= BOUNTY_CLEAR_TICKS
            {
                entry.iw_bounty = BountyLevel::None;
                // Clear IW kills on expiry.
                entry.faction_kills.remove("iron_wolves");
            }
        }
    }
}

// ── Issue #120: Danger level computation ──────────────────────────────────────

/// Recompute `DangerLevel` grid each tick from settlement positions.
///
/// Divides the map into 50×50 coarse cells and classifies each as Safe /
/// Moderate / Dangerous based on the distance to the nearest settlement.
pub fn update_danger_levels(
    mut danger: ResMut<DangerLevel>,
    settlements: Option<Res<Settlements>>,
) {
    let Some(settlements) = settlements else { return };
    if settlements.0.is_empty() {
        return;
    }

    // We sample a coarse 50-tile grid over the 1024×1024 map.
    // Settlement coords are tile-space (0..MAP_WIDTH); we convert to world-space below.
    let settlement_positions: Vec<(f32, f32)> = settlements
        .0
        .iter()
        .map(|s| (s.x - MAP_HALF_WIDTH as f32, s.y - MAP_HALF_HEIGHT as f32))
        .collect();

    danger.grid.clear();

    // Sample every coarse cell (50 tiles wide) within the world bounds.
    let cells_x = (MAP_HALF_WIDTH as i32 * 2 / 50) + 1;
    let cells_y = (MAP_HALF_HEIGHT as i32 * 2 / 50) + 1;

    for gx in -(cells_x / 2)..=(cells_x / 2) {
        for gy in -(cells_y / 2)..=(cells_y / 2) {
            // Center of this coarse cell in world-space tile coords.
            let cx = gx as f32 * 50.0 + 25.0;
            let cy = gy as f32 * 50.0 + 25.0;

            let min_dist = settlement_positions
                .iter()
                .map(|&(sx, sy)| {
                    let dx = cx - sx;
                    let dy = cy - sy;
                    (dx * dx + dy * dy).sqrt()
                })
                .fold(f32::INFINITY, f32::min);

            let tier = if min_dist <= DANGER_SAFE_RADIUS {
                DangerTier::Safe
            } else if min_dist <= DANGER_MODERATE_RADIUS {
                DangerTier::Moderate
            } else {
                DangerTier::Dangerous
            };

            danger.grid.insert((gx, gy), tier);
        }
    }
}

// ── Issue #117: Bandit spawning ────────────────────────────────────────────────

/// Spawn bandit groups in wilderness tiles every BANDIT_SPAWN_INTERVAL ticks.
///
/// Rate: 1 group per 50 wilderness tiles per 100 ticks.
/// Spawns in Moderate or Dangerous zones only.
/// Dangerous zones spawn higher-CR bandits (Named rank).
pub fn spawn_bandit_groups(
    mut commands: Commands,
    tick: Res<WorldSimTick>,
    danger: Res<DangerLevel>,
    settlements: Option<Res<Settlements>>,
    existing_camps: Query<&BanditCamp>,
    world_map: Option<Res<WorldMap>>,
) {
    // Only fire every BANDIT_SPAWN_INTERVAL ticks.
    if !tick.0.is_multiple_of(BANDIT_SPAWN_INTERVAL) {
        return;
    }

    let Some(settlements) = settlements else { return };
    if settlements.0.is_empty() {
        return;
    }

    // Limit total bandit camps to prevent runaway spawning.
    let current_camps = existing_camps.iter().count();
    if current_camps >= 20 {
        return;
    }

    let iron_wolves = FactionId(SmolStr::new("iron_wolves"));

    // Pick candidate spawn positions scattered across wilderness (non-settlement) areas.
    // We use the tick as a deterministic seed to spread spawns across the map.
    let base_seed = tick.0.wrapping_mul(2_654_435_761);

    // Attempt a few candidate positions per tick.
    for attempt in 0..3u64 {
        let seed = base_seed.wrapping_add(attempt.wrapping_mul(1_234_567));

        // Sample a world-space position deterministically.
        let wx = (seed % (MAP_HALF_WIDTH as u64 * 2)) as f32 - MAP_HALF_WIDTH as f32;
        let wy = ((seed.wrapping_mul(987_654_321)) % (MAP_HALF_HEIGHT as u64 * 2)) as f32
            - MAP_HALF_HEIGHT as f32;

        // Check danger zone — skip Safe zones.
        let tier = danger.tier_at(wx, wy);
        if tier == DangerTier::Safe {
            continue;
        }

        // Skip if too close to any settlement (30 tiles).
        let near_settlement = settlements.0.iter().any(|s| {
            let sx = s.x - MAP_HALF_WIDTH as f32;
            let sy = s.y - MAP_HALF_HEIGHT as f32;
            let dx = wx - sx;
            let dy = wy - sy;
            dx * dx + dy * dy < DANGER_SAFE_RADIUS * DANGER_SAFE_RADIUS
        });
        if near_settlement {
            continue;
        }

        // Prefer road tiles near the candidate point (radius 5).
        let (spawn_wx, spawn_wy) = if let Some(ref map) = world_map {
            let mut road_tile = None;
            'road_search: for dy in -5i32..=5 {
                for dx in -5i32..=5 {
                    let cx = wx + dx as f32;
                    let cy = wy + dy as f32;
                    let ix = (cx + MAP_HALF_WIDTH as f32) as i64;
                    let iy = (cy + MAP_HALF_HEIGHT as f32) as i64;
                    if ix >= 0 && iy >= 0 && (ix as usize) < map.width && (iy as usize) < map.height {
                        let idx = ix as usize + iy as usize * map.width;
                        if map.road_tiles.get(idx).copied().unwrap_or(false) {
                            road_tile = Some((cx, cy));
                            break 'road_search;
                        }
                    }
                }
            }
            road_tile.unwrap_or((wx, wy))
        } else {
            (wx, wy)
        };

        // Determine group size and rank.
        let group_size = BANDIT_GROUP_SIZE_MIN
            + (seed % (BANDIT_GROUP_SIZE_MAX - BANDIT_GROUP_SIZE_MIN + 1) as u64) as u32;
        let rank = if tier == DangerTier::Dangerous {
            NpcRank::Named
        } else {
            NpcRank::Grunt
        };

        // Spawn bandit group members.
        let tile_x = (spawn_wx + MAP_HALF_WIDTH as f32) as u32;
        let tile_y = (spawn_wy + MAP_HALF_HEIGHT as f32) as u32;

        // Alternate Rogue / Barbarian per member.
        for i in 0..group_size {
            let class = if i % 2 == 0 {
                CharacterClass::Rogue
            } else {
                CharacterClass::Barbarian
            };
            let scores = AbilityScores::for_class(&class, rank);
            let mods = AbilityModifiers::from_scores(&scores);
            let level = if rank == NpcRank::Named { 3 } else { 1 };
            let (hp, ac, xp_reward) = match rank {
                NpcRank::Grunt => (20, 12, 50),
                NpcRank::Named => (35, 14, 200),
                NpcRank::Boss => (55, 16, 500),
            };
            let offset_x = (i % 3) as f32 * 1.5;
            let offset_y = (i / 3) as f32 * 1.5;
            let pos = WorldPosition { x: spawn_wx + offset_x, y: spawn_wy + offset_y, z: 0.0 };
            commands.spawn((
                pos.clone(),
                Health { current: hp, max: hp },
                CombatParticipant {
                    id: CombatantId(Uuid::new_v4()),
                    interrupt_stack: InterruptStack::default(),
                    class: class.clone(),
                    level,
                    armor_class: ac,
                    strength: scores.strength as i32,
                    dexterity: scores.dexterity as i32,
                    constitution: scores.constitution as i32,
                    intelligence: scores.intelligence as i32,
                    wisdom: scores.wisdom as i32,
                    charisma: scores.charisma as i32,
                },
                ExperienceReward(xp_reward),
                FactionMember(iron_wolves.clone()),
                FactionNpcRank(rank),
                FactionBadge {
                    faction_id: "iron_wolves".to_string(),
                    rank,
                },
                CurrentGoal(None),
                HomePosition(pos.clone()),
                EntityKind::FactionNpc,
                BanditCamp {
                    spawn_tile: (tile_x, tile_y),
                    patrol_radius: BANDIT_PATROL_RADIUS,
                },
                NavPath::default(),
                NavReplanTimer::default(),
                // Class/level/ability-score bundle (nested to stay within tuple Bundle limit).
                (
                    NpcClass(class.clone()),
                    NpcLevel(level),
                    scores,
                    mods,
                    HitDice::for_class_level(&class, level),
                    SavingThrowProficiencies::for_class(&class),
                ),
            ));
        }

        tracing::info!(
            wx = spawn_wx, wy = spawn_wy, group_size, ?tier, ?rank,
            "Bandit group spawned"
        );
        // One spawn per tick.
        break;
    }
}

// ── Issue #118: Portal horror spawning ────────────────────────────────────────

/// Spawn horror creatures from CasinoPortal and FungusPortal locations.
///
/// Fires when `UndergroundPressure >= 0.8` OR every HORROR_SPAWN_INTERVAL ticks.
pub fn spawn_portal_horrors(
    mut commands: Commands,
    tick: Res<WorldSimTick>,
    pressure: Res<super::UndergroundPressure>,
    topology: Option<Res<ZoneTopology>>,
) {
    let pressure_trigger = pressure.score >= 0.8;
    let timer_trigger = tick.0.is_multiple_of(HORROR_SPAWN_INTERVAL) && tick.0 > 0;

    if !pressure_trigger && !timer_trigger {
        return;
    }

    let Some(topology) = topology else { return };

    // Find Casino and Fungus portals by kind.
    let casino_portals: Vec<&fellytip_shared::world::zone::Portal> = topology
        .portals
        .iter()
        .filter(|p| p.kind == PortalKind::CasinoPortal)
        .collect();
    let fungus_portals: Vec<&fellytip_shared::world::zone::Portal> = topology
        .portals
        .iter()
        .filter(|p| p.kind == PortalKind::FungusPortal)
        .collect();

    // Compute horror count from pressure: 1 + (pressure * 2) capped at 4.
    let horror_count = (1 + (pressure.score * 2.0) as u32).min(4);

    // Casino horrors: Warlock "Void Gambler" CR 3 (HP 45, AC 13).
    for portal in &casino_portals {
        for horror_idx in 0..horror_count {
            // Deterministic jitter based on tick + portal id + index.
            let jitter_seed = tick.0.wrapping_add(portal.id as u64 * 31337).wrapping_add(horror_idx as u64 * 7);
            let jx = ((jitter_seed % 5) as f32) - 2.0;
            let jy = ((jitter_seed.wrapping_mul(17) % 5) as f32) - 2.0;
            let pos = WorldPosition { x: jx, y: jy, z: 0.0 };
            let warlock_scores = AbilityScores { strength: 10, dexterity: 14, constitution: 14, intelligence: 14, wisdom: 12, charisma: 18 };
            let warlock_mods = AbilityModifiers::from_scores(&warlock_scores);
            commands.spawn((
                pos.clone(),
                Health { current: 45, max: 45 },
                CombatParticipant {
                    id: CombatantId(Uuid::new_v4()),
                    interrupt_stack: InterruptStack::default(),
                    class: CharacterClass::Warlock,
                    level: 3,
                    armor_class: 13,
                    strength: warlock_scores.strength as i32,
                    dexterity: warlock_scores.dexterity as i32,
                    constitution: warlock_scores.constitution as i32,
                    intelligence: warlock_scores.intelligence as i32,
                    wisdom: warlock_scores.wisdom as i32,
                    charisma: warlock_scores.charisma as i32,
                },
                ExperienceReward(700),
                FactionNpcRank(NpcRank::Named),
                EntityKind::FactionNpc,
                HomePosition(pos.clone()),
                PortalHorror { world_origin: WORLD_DEVILS_CASINO },
                NavPath::default(),
                NavReplanTimer::default(),
                // Class/level/ability-score bundle (nested to stay within tuple Bundle limit).
                (
                    NpcClass(CharacterClass::Warlock),
                    NpcLevel(3),
                    warlock_scores,
                    warlock_mods,
                    HitDice::for_class_level(&CharacterClass::Warlock, 3),
                    SavingThrowProficiencies::for_class(&CharacterClass::Warlock),
                ),
            ));
            tracing::info!(portal_id = portal.id, horror_idx, horror_count, "Void Gambler spawned from CasinoPortal");
        }
    }

    // Fungus horrors: Druid "Spore Wraith" CR 2 (HP 33, AC 11).
    for portal in &fungus_portals {
        for horror_idx in 0..horror_count {
            let jitter_seed = tick.0.wrapping_add(portal.id as u64 * 42069).wrapping_add(horror_idx as u64 * 13);
            let jx = ((jitter_seed % 5) as f32) - 2.0;
            let jy = ((jitter_seed.wrapping_mul(23) % 5) as f32) - 2.0;
            let pos = WorldPosition { x: jx, y: jy, z: 0.0 };
            let druid_scores = AbilityScores { strength: 10, dexterity: 12, constitution: 14, intelligence: 12, wisdom: 16, charisma: 10 };
            let druid_mods = AbilityModifiers::from_scores(&druid_scores);
            commands.spawn((
                pos.clone(),
                Health { current: 33, max: 33 },
                CombatParticipant {
                    id: CombatantId(Uuid::new_v4()),
                    interrupt_stack: InterruptStack::default(),
                    class: CharacterClass::Druid,
                    level: 2,
                    armor_class: 11,
                    strength: druid_scores.strength as i32,
                    dexterity: druid_scores.dexterity as i32,
                    constitution: druid_scores.constitution as i32,
                    intelligence: druid_scores.intelligence as i32,
                    wisdom: druid_scores.wisdom as i32,
                    charisma: druid_scores.charisma as i32,
                },
                ExperienceReward(450),
                FactionNpcRank(NpcRank::Named),
                EntityKind::FactionNpc,
                HomePosition(pos.clone()),
                PortalHorror { world_origin: WORLD_HIVEMIND_FUNGUS },
                NavPath::default(),
                NavReplanTimer::default(),
                // Class/level/ability-score bundle (nested to stay within tuple Bundle limit).
                (
                    NpcClass(CharacterClass::Druid),
                    NpcLevel(2),
                    druid_scores,
                    druid_mods,
                    HitDice::for_class_level(&CharacterClass::Druid, 2),
                    SavingThrowProficiencies::for_class(&CharacterClass::Druid),
                ),
            ));
            tracing::info!(portal_id = portal.id, horror_idx, horror_count, "Spore Wraith spawned from FungusPortal");
        }
    }
}

// ── Issue #119: Warfare event system ──────────────────────────────────────────

/// Detect hostile faction pairs from settlements and create WarfrontEvents.
/// Also resolves active warfronts when war party arrives at target.
#[allow(clippy::too_many_arguments)]
pub fn resolve_warfront_events(
    mut warfront_registry: ResMut<WarfrontRegistry>,
    pop: Res<FactionPopulationState>,
    registry: Res<FactionRegistry>,
    tick: Res<WorldSimTick>,
    war_parties: Query<(&WarPartyMember, &CombatParticipant)>,
    mut story_writer: MessageWriter<WriteStoryEvent>,
) {
    use fellytip_shared::world::faction::Disposition;

    // Detect new wars: for each pair of settlements in hostile factions, create a front.
    for faction in &registry.factions {
        let hostile_fids: Vec<&FactionId> = faction
            .disposition
            .iter()
            .filter(|(_, d)| **d == Disposition::Hostile)
            .map(|(id, _)| id)
            .collect();

        for &hostile_fid in &hostile_fids {
            // Find a settlement of the hostile faction.
            let Some(hostile_settlement) = pop.settlements.values().find(|s| &s.faction_id == hostile_fid) else { continue };
            // Find a settlement of our faction.
            let Some(_our_settlement) = pop.settlements.values().find(|s| s.faction_id == faction.id) else { continue };

            let already_tracked = warfront_registry.fronts.iter().any(|f| {
                f.attacker_faction == faction.id && f.defender_faction == *hostile_fid && !f.resolved
            });
            if already_tracked {
                continue;
            }

            warfront_registry.fronts.push(WarfrontEvent {
                attacker_faction: faction.id.clone(),
                defender_faction: hostile_fid.clone(),
                target_settlement_id: hostile_settlement.settlement_id,
                target_x: hostile_settlement.home_x,
                target_y: hostile_settlement.home_y,
                resolved: false,
            });
            tracing::debug!(
                attacker = %faction.id.0,
                defender = %hostile_fid.0,
                "WarfrontEvent created"
            );
        }
    }

    // Resolve warfronts when a war party arrives at the target settlement.
    for front in warfront_registry.fronts.iter_mut() {
        if front.resolved {
            continue;
        }

        // Check if any attacker war party has reached the target.
        let arrived = war_parties.iter().any(|(wm, _)| {
            wm.attacker_faction == front.attacker_faction
                && wm.target_settlement_id == front.target_settlement_id
                && {
                    // Check if within BATTLE_RADIUS of target.
                    let dx = wm.target_x;
                    let dy = wm.target_y;
                    let _ = (dx, dy); // positions are on the WarPartyMember
                    true // simplified: mark resolved when any party has this target
                }
        });

        if !arrived {
            continue;
        }

        // Resolve battle: use seeded dice to determine winner.
        let seed = front.target_settlement_id.as_u128() as u64 ^ tick.0;
        let mut dice_iter = fellytip_shared::world::war::seeded_dice(front.target_settlement_id, tick.0);
        let attacker_roll: i32 = dice_iter.by_ref().take(3).sum();
        let defender_roll: i32 = dice_iter.take(3).sum();

        let (winner, loser, attacker_won) = if attacker_roll >= defender_roll {
            (front.attacker_faction.clone(), front.defender_faction.clone(), true)
        } else {
            (front.defender_faction.clone(), front.attacker_faction.clone(), false)
        };

        // Determine casualty percentages.
        let attacker_casualties_pct = if attacker_won { 0.10 + (seed % 11) as f32 * 0.01 } else { 0.20 + (seed % 21) as f32 * 0.01 };
        let defender_casualties_pct = if attacker_won { 0.20 + (seed % 21) as f32 * 0.01 } else { 0.10 + (seed % 11) as f32 * 0.01 };

        // Check for razed settlement (military ratio > 3:1 and attacker won).
        let attacker_ms = registry.factions.iter().find(|f| f.id == front.attacker_faction).map(|f| f.resources.military_strength).unwrap_or(0.0);
        let defender_ms = registry.factions.iter().find(|f| f.id == front.defender_faction).map(|f| f.resources.military_strength).unwrap_or(1.0);
        let is_razed = attacker_won && attacker_ms / defender_ms.max(0.001) > 3.0;

        let kind = if is_razed {
            StoryEventKind::SettlementRazed { by: winner.clone() }
        } else {
            StoryEventKind::FactionWarDeclared {
                attacker: winner.clone(),
                defender: loser.clone(),
            }
        };

        story_writer.write(WriteStoryEvent(StoryEvent {
            id: Uuid::new_v4(),
            tick: tick.0,
            world_day: (tick.0 / 300) as u32,
            kind,
            participants: vec![],
            location: None,
            lore_tags: vec![
                SmolStr::new("battle"),
                SmolStr::new(if is_razed { "razed" } else { "victory" }),
            ],
        }));

        tracing::info!(
            winner = %winner.0,
            loser = %loser.0,
            attacker_casualties_pct,
            defender_casualties_pct,
            is_razed,
            "WarfrontEvent resolved"
        );
        let _ = (attacker_casualties_pct, defender_casualties_pct);

        front.resolved = true;
    }

    // Prune resolved fronts older than 1000 entries.
    if warfront_registry.fronts.len() > 200 {
        warfront_registry.fronts.retain(|f| !f.resolved);
    }
}

// ── Issue #121: Dynamic bounty / threat level ──────────────────────────────────

/// Update ThreatRegistry each tick: expire old bounties and escalate bounty
/// hunters when kill thresholds are crossed.
#[allow(clippy::too_many_arguments)]
pub fn update_threat_registry(
    mut threat: ResMut<ThreatRegistry>,
    mut commands: Commands,
    tick: Res<WorldSimTick>,
    player_query: Query<(Entity, &CombatParticipant, &WorldPosition), With<PlayerStandings>>,
    rep: Res<PlayerReputationMap>,
) {
    use fellytip_shared::world::faction::standing_tier;

    // Expire stale bounties.
    threat.tick_expiry(tick.0);

    let iron_wolves_fid = FactionId(SmolStr::new("iron_wolves"));

    // For each player, check if they now qualify for a bounty escalation.
    for (_entity, cp, pos) in &player_query {
        let player_id = cp.id.0;
        let iw_score = rep.score(player_id, &iron_wolves_fid);
        let tier = standing_tier(iw_score);

        // Only track players hostile to Iron Wolves.
        if !tier.is_aggressive() {
            continue;
        }

        let Some(entry) = threat.players.get(&player_id) else { continue };
        let bounty = entry.iw_bounty;

        match bounty {
            BountyLevel::None => {}
            BountyLevel::Hunter => {
                // Spawn a Named-rank Ranger bounty hunter near the player.
                spawn_bounty_hunter(&mut commands, pos, NpcRank::Named, tick.0);
                tracing::info!(player_id = %player_id, "Named bounty hunter dispatched");
                // Only spawn once — reset so we don't re-trigger until kills increase.
                if let Some(e) = threat.players.get_mut(&player_id) {
                    e.iw_bounty = BountyLevel::None;
                }
            }
            BountyLevel::BossHunter => {
                spawn_bounty_hunter(&mut commands, pos, NpcRank::Boss, tick.0);
                tracing::info!(player_id = %player_id, "Boss bounty hunter dispatched");
                if let Some(e) = threat.players.get_mut(&player_id) {
                    e.iw_bounty = BountyLevel::None;
                }
            }
            BountyLevel::WarParty => {
                // Spawn a small war party (handled via existing FormWarPartyEvent in population.rs).
                // Here we just log and reset; the war-party formation picks up through normal channels.
                tracing::warn!(player_id = %player_id, "War party ordered against player by Iron Wolves");
                if let Some(e) = threat.players.get_mut(&player_id) {
                    e.iw_bounty = BountyLevel::None;
                }
            }
        }
    }
}

// ── Spawn helper: bounty hunter ────────────────────────────────────────────────

fn spawn_bounty_hunter(commands: &mut Commands, near: &WorldPosition, rank: NpcRank, tick: u64) {
    let iron_wolves = FactionId(SmolStr::new("iron_wolves"));
    let class = CharacterClass::Ranger;
    let scores = AbilityScores::for_class(&class, rank);
    let mods = AbilityModifiers::from_scores(&scores);
    let (hp, ac, level, xp_reward) = match rank {
        NpcRank::Grunt => (20, 13, 1u32, 50),
        NpcRank::Named => (40, 15, 5u32, 300),
        NpcRank::Boss => (65, 17, 8u32, 700),
    };
    // Spawn slightly offset from player position.
    let offset = (tick % 5) as f32 * 3.0 + 5.0;
    let pos = WorldPosition {
        x: near.x + offset,
        y: near.y + offset,
        z: near.z,
    };
    commands.spawn((
        pos.clone(),
        Health { current: hp, max: hp },
        CombatParticipant {
            id: CombatantId(Uuid::new_v4()),
            interrupt_stack: InterruptStack::default(),
            class: class.clone(),
            level,
            armor_class: ac,
            strength: scores.strength as i32,
            dexterity: scores.dexterity as i32,
            constitution: scores.constitution as i32,
            intelligence: scores.intelligence as i32,
            wisdom: scores.wisdom as i32,
            charisma: scores.charisma as i32,
        },
        ExperienceReward(xp_reward),
        FactionMember(iron_wolves.clone()),
        FactionNpcRank(rank),
        FactionBadge { faction_id: "iron_wolves".to_string(), rank },
        CurrentGoal(None),
        HomePosition(pos),
        EntityKind::FactionNpc,
        NavPath::default(),
        NavReplanTimer::default(),
        // Class/level/ability-score bundle (nested to stay within tuple Bundle limit).
        (
            NpcClass(class.clone()),
            NpcLevel(level),
            scores,
            mods,
            HitDice::for_class_level(&class, level),
            SavingThrowProficiencies::for_class(&class),
        ),
    ));
}

// ── Issue #97: Auto-raid generation ──────────────────────────────────────────

/// Generate warfront events from hostile faction military buildup.
///
/// Runs every 200 ticks. For each faction with military_strength > 20.0 and
/// is_aggressive == true, creates a new WarfrontEvent if no active warfront
/// exists for that faction. Max 3 active warfronts at a time.
pub fn auto_generate_raids(
    mut warfront_registry: ResMut<WarfrontRegistry>,
    registry: Res<super::FactionRegistry>,
    pop: Res<super::FactionPopulationState>,
    tick: Res<WorldSimTick>,
) {
    if !tick.0.is_multiple_of(200) || tick.0 == 0 {
        return;
    }

    // Count active (unresolved) warfronts.
    let active_count = warfront_registry.fronts.iter().filter(|f| !f.resolved).count();
    if active_count >= 3 {
        return;
    }

    for faction in &registry.factions {
        if !faction.is_aggressive || faction.resources.military_strength <= 20.0 {
            continue;
        }

        // Check if this faction already has an active warfront.
        let has_active = warfront_registry.fronts.iter().any(|f| {
            f.attacker_faction == faction.id && !f.resolved
        });
        if has_active {
            continue;
        }

        // Find the nearest hostile settlement (by territory centroid / settlement position).
        let attacker_centroid_x = pop.settlements.values()
            .filter(|s| s.faction_id == faction.id)
            .map(|s| s.home_x)
            .sum::<f32>()
            / (pop.settlements.values().filter(|s| s.faction_id == faction.id).count().max(1) as f32);
        let attacker_centroid_y = pop.settlements.values()
            .filter(|s| s.faction_id == faction.id)
            .map(|s| s.home_y)
            .sum::<f32>()
            / (pop.settlements.values().filter(|s| s.faction_id == faction.id).count().max(1) as f32);

        // Find nearest hostile settlement from other factions.
        let target = pop.settlements.values()
            .filter(|s| s.faction_id != faction.id && !s.collapsed)
            .min_by(|a, b| {
                let da = (a.home_x - attacker_centroid_x).powi(2) + (a.home_y - attacker_centroid_y).powi(2);
                let db = (b.home_x - attacker_centroid_x).powi(2) + (b.home_y - attacker_centroid_y).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });

        let Some(target) = target else { continue };

        // Recheck limit before inserting.
        let active_now = warfront_registry.fronts.iter().filter(|f| !f.resolved).count();
        if active_now >= 3 {
            break;
        }

        warfront_registry.fronts.push(WarfrontEvent {
            attacker_faction: faction.id.clone(),
            defender_faction: target.faction_id.clone(),
            target_settlement_id: target.settlement_id,
            target_x: target.home_x,
            target_y: target.home_y,
            resolved: false,
        });
        tracing::info!(
            faction = %faction.id.0,
            target = %target.settlement_id,
            military = faction.resources.military_strength,
            "auto_generate_raids: WarfrontEvent created from military buildup"
        );
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threat_registry_record_kill_escalates_bounty() {
        let mut reg = ThreatRegistry::default();
        let pid = Uuid::new_v4();

        // 0 kills: no bounty
        assert_eq!(reg.players.get(&pid).map(|e| e.iw_bounty), None);

        // 3 kills: Hunter
        for _ in 0..3 {
            reg.record_iw_kill(pid, 100);
        }
        assert_eq!(reg.players[&pid].iw_bounty, BountyLevel::Hunter);

        // 5 kills: BossHunter
        for _ in 0..2 {
            reg.record_iw_kill(pid, 100);
        }
        assert_eq!(reg.players[&pid].iw_bounty, BountyLevel::BossHunter);

        // 10 kills: WarParty
        for _ in 0..5 {
            reg.record_iw_kill(pid, 100);
        }
        assert_eq!(reg.players[&pid].iw_bounty, BountyLevel::WarParty);
    }

    #[test]
    fn threat_registry_expiry_clears_bounty() {
        let mut reg = ThreatRegistry::default();
        let pid = Uuid::new_v4();
        reg.record_iw_kill(pid, 0);
        reg.record_iw_kill(pid, 0);
        reg.record_iw_kill(pid, 0);
        assert_eq!(reg.players[&pid].iw_bounty, BountyLevel::Hunter);
        // Advance time past BOUNTY_CLEAR_TICKS.
        reg.tick_expiry(BOUNTY_CLEAR_TICKS + 1);
        assert_eq!(reg.players[&pid].iw_bounty, BountyLevel::None);
    }

    #[test]
    fn danger_level_default_tier_is_moderate() {
        let danger = DangerLevel::default();
        // No data in grid → defaults to Moderate.
        assert_eq!(danger.tier_at(999.0, 999.0), DangerTier::Moderate);
    }

    #[test]
    fn bounty_level_default_is_none() {
        let t = PlayerThreat::default();
        assert_eq!(t.iw_bounty, BountyLevel::None);
    }
}
