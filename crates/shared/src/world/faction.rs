//! Faction data, goals, dispositions, and the pure utility-scoring function.

use crate::world::ecology::RegionId;
use smol_str::SmolStr;
use std::collections::HashMap;

// ── Identifiers ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FactionId(pub SmolStr);

// ── Disposition ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Disposition {
    Allied,
    Friendly,
    Neutral,
    Hostile,
}

// ── Resources ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FactionResources {
    pub food: f32,
    pub gold: f32,
    pub military_strength: f32,
}

// ── Goals ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum FactionGoal {
    ExpandTerritory { target_region: RegionId },
    DefendSettlement { settlement_id: SmolStr },
    RaidResource { resource_node_id: SmolStr },
    FormAlliance { with: FactionId, min_trust: f32 },
    Survive,
}

// ── Faction ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Faction {
    pub id: FactionId,
    pub name: SmolStr,
    pub disposition: HashMap<FactionId, Disposition>,
    pub goals: Vec<FactionGoal>,
    pub resources: FactionResources,
    pub territory: Vec<RegionId>,
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
        }
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
