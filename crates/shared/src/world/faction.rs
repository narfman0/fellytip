//! Faction data, goals, dispositions, and the pure utility-scoring function.

use crate::world::civilization::BuildingKind;
use crate::world::ecology::RegionId;
use bevy::prelude::{Reflect, Resource};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::collections::HashMap;
use uuid::Uuid;

// ── Identifiers ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FactionId(pub SmolStr);

// ── Disposition ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Disposition {
    Allied,
    Friendly,
    Neutral,
    Hostile,
}

// ── Resources ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FactionResources {
    pub food: f32,
    pub gold: f32,
    pub military_strength: f32,
}

// ── Goals ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FactionGoal {
    ExpandTerritory { target_region: RegionId },
    DefendSettlement { settlement_id: SmolStr },
    RaidResource { resource_node_id: SmolStr },
    FormAlliance { with: FactionId, min_trust: f32 },
    Survive,
}

// ── Standing tiers ────────────────────────────────────────────────────────────

pub const STANDING_EXALTED:    i32 =  750;
pub const STANDING_HONORED:    i32 =  500;
pub const STANDING_FRIENDLY:   i32 =  250;
pub const STANDING_NEUTRAL:    i32 =    0;
pub const STANDING_UNFRIENDLY: i32 = -250;
pub const STANDING_HOSTILE:    i32 = -500;

/// Player–faction reputation tier derived from a numeric score.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect, Serialize, Deserialize)]
pub enum StandingTier {
    Exalted,
    Honored,
    Friendly,
    Neutral,
    Unfriendly,
    Hostile,
    Hated,
}

impl StandingTier {
    /// True when the tier results in NPC aggression.
    pub fn is_aggressive(self) -> bool {
        matches!(self, StandingTier::Hostile | StandingTier::Hated)
    }
}

/// Map a numeric score to its `StandingTier`.
pub fn standing_tier(score: i32) -> StandingTier {
    if score >= STANDING_EXALTED         { StandingTier::Exalted    }
    else if score >= STANDING_HONORED    { StandingTier::Honored    }
    else if score >= STANDING_FRIENDLY   { StandingTier::Friendly   }
    else if score >= STANDING_NEUTRAL    { StandingTier::Neutral    }
    else if score >= STANDING_UNFRIENDLY { StandingTier::Unfriendly }
    else if score >= STANDING_HOSTILE    { StandingTier::Hostile    }
    else                                 { StandingTier::Hated      }
}

// ── NPC rank & kill penalties ─────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect, Serialize, Deserialize)]
pub enum NpcRank { Grunt, Named, Boss }

pub const KILL_PENALTY_GRUNT: i32 = -50;
pub const KILL_PENALTY_NAMED: i32 = -200;
pub const KILL_PENALTY_BOSS:  i32 = -500;

pub fn kill_standing_delta(rank: NpcRank) -> i32 {
    match rank {
        NpcRank::Grunt => KILL_PENALTY_GRUNT,
        NpcRank::Named => KILL_PENALTY_NAMED,
        NpcRank::Boss  => KILL_PENALTY_BOSS,
    }
}

/// Default player standing with a faction, used when no record exists.
pub fn default_standing(faction_id: &FactionId) -> i32 {
    match faction_id.0.as_str() {
        "ash_covenant" | "deep_tide" => STANDING_HOSTILE,
        _ => STANDING_NEUTRAL,
    }
}

// ── Faction-to-faction relations (issue #110) ─────────────────────────────────

/// Score range for faction-to-faction relations.
/// -100 = fully hostile, 0 = neutral, +100 = allied.
pub const RELATION_ALLIED: i32     =  60;
pub const RELATION_HOSTILE: i32    = -50;
pub const RELATION_TRADE_BONUS: i32 =   5;
pub const RELATION_RAID_PENALTY: i32 = -10;
pub const RELATION_BATTLE_PENALTY: i32 = -30;

/// Symmetric relation matrix between factions.
///
/// `get(a, b)` == `get(b, a)`.  Clamped to [-100, 100].
/// Initial values are set by `seed_faction_relations`.
#[derive(Debug, Default, Clone, Resource)]
pub struct FactionRelations(pub HashMap<(SmolStr, SmolStr), i32>);

impl FactionRelations {
    /// Canonical key — always stores (smaller, larger) alphabetically.
    fn key(a: &FactionId, b: &FactionId) -> (SmolStr, SmolStr) {
        if a.0.as_str() <= b.0.as_str() {
            (a.0.clone(), b.0.clone())
        } else {
            (b.0.clone(), a.0.clone())
        }
    }

    /// Current relation score between two factions (0 if unknown).
    pub fn get(&self, a: &FactionId, b: &FactionId) -> i32 {
        *self.0.get(&Self::key(a, b)).unwrap_or(&0)
    }

    /// Apply a delta and clamp to [-100, 100].
    pub fn apply_delta(&mut self, a: &FactionId, b: &FactionId, delta: i32) {
        let entry = self.0.entry(Self::key(a, b)).or_insert(0);
        *entry = (*entry + delta).clamp(-100, 100);
    }

    /// Set an absolute value (clamped).
    pub fn set(&mut self, a: &FactionId, b: &FactionId, score: i32) {
        *self.0.entry(Self::key(a, b)).or_insert(0) = score.clamp(-100, 100);
    }

    /// Returns true when factions are allied (relation >= RELATION_ALLIED).
    /// Allied factions do not attack each other's settlements.
    pub fn are_allied(&self, a: &FactionId, b: &FactionId) -> bool {
        self.get(a, b) >= RELATION_ALLIED
    }

    /// Returns true when factions are at war (relation < RELATION_HOSTILE).
    pub fn at_war(&self, a: &FactionId, b: &FactionId) -> bool {
        self.get(a, b) < RELATION_HOSTILE
    }

    /// Seed default starting relations per the design spec:
    /// - Iron Wolves hostile to Merchant Guild (-60)
    /// - Deep Tide neutral to all (0)
    /// - Ash Covenant hostile to Iron Wolves (-40)
    pub fn seed_defaults(&mut self) {
        let iron_wolves    = FactionId("iron_wolves".into());
        let merchant_guild = FactionId("merchant_guild".into());
        let ash_covenant   = FactionId("ash_covenant".into());
        let deep_tide      = FactionId("deep_tide".into());

        self.set(&iron_wolves,    &merchant_guild, -60);
        self.set(&ash_covenant,   &iron_wolves,    -40);
        // Deep Tide is explicitly neutral (0), which is the default.
        let _ = &deep_tide; // ensure it's referenced even if no explicit entry.
    }
}

// ── Player reputation map ─────────────────────────────────────────────────────

/// Per-player, per-faction standing scores.
///
/// Keyed by player UUID (same UUID as `CombatantId`).  Clamps scores to
/// [-999, 1000] on every mutation to prevent runaway values.
#[derive(Debug, Default, Clone, Resource)]
pub struct PlayerReputationMap(pub HashMap<Uuid, HashMap<FactionId, i32>>);

impl PlayerReputationMap {
    /// Current standing score.  Falls back to `default_standing` if no record
    /// exists for this player/faction combination.
    pub fn score(&self, player_id: Uuid, faction: &FactionId) -> i32 {
        self.0
            .get(&player_id)
            .and_then(|m| m.get(faction))
            .copied()
            .unwrap_or_else(|| default_standing(faction))
    }

    /// Apply a delta and clamp the result to [-999, 1000].
    pub fn apply_delta(&mut self, player_id: Uuid, faction: &FactionId, delta: i32) {
        let entry = self.0.entry(player_id).or_default()
            .entry(faction.clone()).or_insert_with(|| default_standing(faction));
        *entry = (*entry + delta).clamp(-999, 1000);
    }
}

// ── Faction ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Faction {
    pub id: FactionId,
    pub name: SmolStr,
    pub disposition: HashMap<FactionId, Disposition>,
    pub goals: Vec<FactionGoal>,
    pub resources: FactionResources,
    pub territory: Vec<RegionId>,
    /// Whether this faction attacks players on sight regardless of standing.
    pub is_aggressive: bool,
    /// Initial standing score for a player with no prior interaction.
    pub player_default_standing: i32,
}

// ── Pure goal-scoring function ────────────────────────────────────────────────

/// Score a goal for the given faction. Higher = higher priority.
///
/// Pure function — inject any world state needed via the faction snapshot.
pub fn score_goal(faction: &Faction, goal: &FactionGoal) -> f32 {
    match goal {
        FactionGoal::Survive => {
            // Always high priority when resources are critically low.
            if faction.resources.food < 10.0 || faction.resources.military_strength < 5.0 {
                100.0
            } else {
                10.0
            }
        }
        FactionGoal::DefendSettlement { .. } => {
            // More urgent when military is weak.
            50.0 - faction.resources.military_strength.min(40.0)
        }
        FactionGoal::ExpandTerritory { .. } => {
            // Only pursue when comfortable.
            if faction.resources.food > 50.0 && faction.resources.military_strength > 20.0 {
                30.0
            } else {
                0.0
            }
        }
        FactionGoal::RaidResource { .. } => {
            // Pursue when food is low but have military.
            if faction.resources.food < 30.0 && faction.resources.military_strength > 15.0 {
                40.0
            } else {
                5.0
            }
        }
        FactionGoal::FormAlliance { .. } => 15.0,
    }
}

/// Return the highest-scoring goal for this faction, if any.
pub fn pick_goal(faction: &Faction) -> Option<&FactionGoal> {
    faction
        .goals
        .iter()
        .max_by(|a, b| score_goal(faction, a).partial_cmp(&score_goal(faction, b)).unwrap())
}

// ── Faction archetype (issue #136) ───────────────────────────────────────────

/// Visual identity for a faction: which buildings it constructs and what
/// colour its procedural towers use.
pub struct FactionArchetype {
    /// Ordered pool of building kinds the faction places in its settlements.
    pub building_pool: &'static [BuildingKind],
    /// RGB base colour for tower wall panels.
    pub tower_wall_color: [f32; 3],
    /// RGB base colour for tower roof caps and floor slabs.
    pub tower_roof_color: [f32; 3],
}

/// Return the `FactionArchetype` for a well-known faction id, or a neutral
/// default for unknown factions.
pub fn faction_archetype(faction_id: &str) -> FactionArchetype {
    match faction_id {
        "iron_wolves" => FactionArchetype {
            building_pool: &[
                BuildingKind::Barracks,
                BuildingKind::Tower,
                BuildingKind::TentDetailed,
            ],
            tower_wall_color: [0.3, 0.3, 0.3],  // dark grey — rugged military
            tower_roof_color: [0.5, 0.2, 0.1],  // rust — battle-worn
        },
        "merchant_guild" => FactionArchetype {
            building_pool: &[
                BuildingKind::StallGreen,
                BuildingKind::StallRed,
                BuildingKind::Windmill,
                BuildingKind::Fountain,
            ],
            tower_wall_color: [0.7, 0.6, 0.4],  // warm stone — prosperous
            tower_roof_color: [0.6, 0.4, 0.1],  // amber — wealth
        },
        "ash_covenant" => FactionArchetype {
            building_pool: &[
                BuildingKind::Keep,
                BuildingKind::Tower,
                BuildingKind::Barracks,
            ],
            tower_wall_color: [0.2, 0.2, 0.2],  // charcoal — austere religious
            tower_roof_color: [0.4, 0.4, 0.4],  // ash — militant
        },
        "deep_tide" => FactionArchetype {
            building_pool: &[
                BuildingKind::Tower,
                BuildingKind::TentSmall,
                BuildingKind::CampfireStones,
            ],
            tower_wall_color: [0.2, 0.35, 0.5],  // ocean blue — nautical
            tower_roof_color: [0.1, 0.25, 0.3],  // dark teal — deep water
        },
        // Unknown faction: generic stone
        _ => FactionArchetype {
            building_pool: &[
                BuildingKind::TentDetailed,
                BuildingKind::TentSmall,
                BuildingKind::CampfireStones,
            ],
            tower_wall_color: [0.55, 0.50, 0.45],
            tower_roof_color: [0.30, 0.28, 0.25],
        },
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_faction(food: f32, military: f32) -> Faction {
        Faction {
            id: FactionId("test".into()),
            name: "Test Faction".into(),
            disposition: HashMap::new(),
            goals: vec![
                FactionGoal::Survive,
                FactionGoal::ExpandTerritory {
                    target_region: RegionId("north".into()),
                },
                FactionGoal::RaidResource {
                    resource_node_id: "forest_01".into(),
                },
            ],
            resources: FactionResources {
                food,
                military_strength: military,
                gold: 0.0,
            },
            territory: vec![],
            is_aggressive: false,
            player_default_standing: STANDING_NEUTRAL,
        }
    }

    #[test]
    fn standing_tier_exact_boundaries() {
        assert_eq!(standing_tier(STANDING_EXALTED),    StandingTier::Exalted);
        assert_eq!(standing_tier(STANDING_HONORED),    StandingTier::Honored);
        assert_eq!(standing_tier(STANDING_FRIENDLY),   StandingTier::Friendly);
        assert_eq!(standing_tier(STANDING_NEUTRAL),    StandingTier::Neutral);
        assert_eq!(standing_tier(STANDING_UNFRIENDLY), StandingTier::Unfriendly);
        assert_eq!(standing_tier(STANDING_HOSTILE),    StandingTier::Hostile);
        assert_eq!(standing_tier(STANDING_HOSTILE - 1), StandingTier::Hated);
        assert_eq!(standing_tier(-999),                StandingTier::Hated);
    }

    #[test]
    fn kill_penalty_ordering() {
        assert!(KILL_PENALTY_BOSS < KILL_PENALTY_NAMED);
        assert!(KILL_PENALTY_NAMED < KILL_PENALTY_GRUNT);
        assert!(KILL_PENALTY_GRUNT < 0);
    }

    #[test]
    fn aggressive_tiers() {
        assert!(StandingTier::Hostile.is_aggressive());
        assert!(StandingTier::Hated.is_aggressive());
        assert!(!StandingTier::Neutral.is_aggressive());
        assert!(!StandingTier::Friendly.is_aggressive());
    }

    #[test]
    fn faction_relations_seed_defaults() {
        let mut rel = FactionRelations::default();
        rel.seed_defaults();
        let iron_wolves    = FactionId("iron_wolves".into());
        let merchant_guild = FactionId("merchant_guild".into());
        let ash_covenant   = FactionId("ash_covenant".into());
        assert_eq!(rel.get(&iron_wolves, &merchant_guild), -60);
        assert_eq!(rel.get(&ash_covenant, &iron_wolves), -40);
    }

    #[test]
    fn faction_relations_symmetric() {
        let mut rel = FactionRelations::default();
        let a = FactionId("ash_covenant".into());
        let b = FactionId("iron_wolves".into());
        rel.set(&a, &b, -40);
        assert_eq!(rel.get(&a, &b), -40);
        assert_eq!(rel.get(&b, &a), -40);
    }

    #[test]
    fn faction_relations_allied_threshold() {
        let mut rel = FactionRelations::default();
        let a = FactionId("iron_wolves".into());
        let b = FactionId("merchant_guild".into());
        rel.set(&a, &b, RELATION_ALLIED);
        assert!(rel.are_allied(&a, &b));
        rel.set(&a, &b, RELATION_ALLIED - 1);
        assert!(!rel.are_allied(&a, &b));
    }

    #[test]
    fn faction_relations_clamped() {
        let mut rel = FactionRelations::default();
        let a = FactionId("deep_tide".into());
        let b = FactionId("ash_covenant".into());
        rel.apply_delta(&a, &b, -999);
        assert_eq!(rel.get(&a, &b), -100);
        rel.apply_delta(&a, &b, 9999);
        assert_eq!(rel.get(&a, &b), 100);
    }

    #[test]
    fn default_standing_hostile_factions() {
        assert_eq!(default_standing(&FactionId("ash_covenant".into())), STANDING_HOSTILE);
        assert_eq!(default_standing(&FactionId("deep_tide".into())),    STANDING_HOSTILE);
        assert_eq!(default_standing(&FactionId("iron_wolves".into())),  STANDING_NEUTRAL);
    }

    #[test]
    fn reputation_map_new_player_gets_default() {
        let rep = PlayerReputationMap::default();
        let id = Uuid::new_v4();
        assert_eq!(rep.score(id, &FactionId("iron_wolves".into())), STANDING_NEUTRAL);
        assert_eq!(rep.score(id, &FactionId("ash_covenant".into())), STANDING_HOSTILE);
    }

    #[test]
    fn reputation_map_delta_clamps() {
        let mut rep = PlayerReputationMap::default();
        let id = Uuid::new_v4();
        rep.apply_delta(id, &FactionId("iron_wolves".into()), -2000);
        assert!(rep.score(id, &FactionId("iron_wolves".into())) >= -999);
        rep.apply_delta(id, &FactionId("iron_wolves".into()), 5000);
        assert!(rep.score(id, &FactionId("iron_wolves".into())) <= 1000);
    }

    #[test]
    fn ten_grunt_kills_reach_hostile() {
        let mut rep = PlayerReputationMap::default();
        let id = Uuid::new_v4();
        let faction = FactionId("iron_wolves".into());
        for _ in 0..10 {
            rep.apply_delta(id, &faction, kill_standing_delta(NpcRank::Grunt));
        }
        let score = rep.score(id, &faction);
        assert_eq!(standing_tier(score), StandingTier::Hostile,
            "10 grunt kills should reach Hostile; score = {score}");
    }

    #[test]
    fn survive_tops_when_starving() {
        let f = make_faction(5.0, 50.0);
        assert!(matches!(pick_goal(&f), Some(FactionGoal::Survive)));
    }

    #[test]
    fn expand_when_comfortable() {
        let f = make_faction(100.0, 50.0);
        assert!(matches!(
            pick_goal(&f),
            Some(FactionGoal::ExpandTerritory { .. })
        ));
    }

    #[test]
    fn raid_when_hungry_and_strong() {
        let f = make_faction(20.0, 30.0);
        assert!(matches!(
            pick_goal(&f),
            Some(FactionGoal::RaidResource { .. })
        ));
    }
}
