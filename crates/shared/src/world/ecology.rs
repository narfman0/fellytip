//! Discrete-time Lotka-Volterra population dynamics per region.
//!
//! Each world-sim tick (1 Hz) the server calls `tick_ecology` on every
//! `RegionEcology`. The function is pure — no RNG, no I/O — so it is
//! trivially unit-testable and proptest-fuzzable.

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

// ── Identifiers ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpeciesId(pub SmolStr);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RegionId(pub SmolStr);

// ── Population state ──────────────────────────────────────────────────────────

/// Population of one species in one region.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Population {
    pub species: SpeciesId,
    /// Current population count (clamped to ≥ 0).
    pub count: f64,
}

/// All species populations inside one region plus their interaction parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegionEcology {
    pub region: RegionId,
    /// Prey population (index 0) and predator population (index 1).
    /// Extend to a Vec for multi-species regions in future milestones.
    pub prey: Population,
    pub predator: Population,
    /// Prey intrinsic growth rate.
    pub r: f64,
    /// Prey carrying capacity.
    pub k: f64,
    /// Predation rate coefficient (α).
    pub alpha: f64,
    /// Predator biomass conversion efficiency (β).
    pub beta: f64,
    /// Predator death rate (δ).
    pub delta: f64,
}

// ── Events emitted by the ecology tick ───────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum EcologyEvent {
    /// A species dropped below the collapse threshold.
    Collapse {
        species: SpeciesId,
        region: RegionId,
    },
    /// A species recovered above the recovery threshold after a collapse.
    Recovery {
        species: SpeciesId,
        region: RegionId,
    },
}

// ── Thresholds ────────────────────────────────────────────────────────────────

/// Population below this is considered collapsed.
pub const COLLAPSE_THRESHOLD: f64 = 5.0;
/// Population above this after a collapse is considered recovered.
pub const RECOVERY_THRESHOLD: f64 = 20.0;

// ── Pure tick function ────────────────────────────────────────────────────────

/// Advance one region's ecology by one tick.
///
/// Returns the updated `RegionEcology` and any events that occurred.
/// This function is pure: given the same inputs it always produces the same
/// output. All world-state mutation and event dispatch happens outside.
pub fn tick_ecology(state: RegionEcology) -> (RegionEcology, Vec<EcologyEvent>) {
    let RegionEcology {
        region,
        prey,
        predator,
        r,
        k,
        alpha,
        beta,
        delta,
    } = state;

    let p = prey.count;
    let q = predator.count;

    // Discrete Lotka-Volterra step.
    // Prey: logistic growth minus predation losses.
    // Predator: population grows by conversion of prey eaten, shrinks by death rate.
    // Both use the additive (1 + rate) form so populations can only go negative via
    // the .max(0.0) clamp, never by a multiplicative factor < 0.
    let new_p = (p + p * r * (1.0 - p / k) - alpha * q * p).max(0.0);
    let new_q = (q + q * (beta * alpha * p - delta)).max(0.0);

    let mut events = Vec::new();

    if new_p < COLLAPSE_THRESHOLD && p >= COLLAPSE_THRESHOLD {
        events.push(EcologyEvent::Collapse {
            species: prey.species.clone(),
            region: region.clone(),
        });
    } else if new_p >= RECOVERY_THRESHOLD && p < RECOVERY_THRESHOLD {
        events.push(EcologyEvent::Recovery {
            species: prey.species.clone(),
            region: region.clone(),
        });
    }

    if new_q < COLLAPSE_THRESHOLD && q >= COLLAPSE_THRESHOLD {
        events.push(EcologyEvent::Collapse {
            species: predator.species.clone(),
            region: region.clone(),
        });
    } else if new_q >= RECOVERY_THRESHOLD && q < RECOVERY_THRESHOLD {
        events.push(EcologyEvent::Recovery {
            species: predator.species.clone(),
            region: region.clone(),
        });
    }

    let next = RegionEcology {
        region,
        prey: Population { count: new_p, ..prey },
        predator: Population { count: new_q, ..predator },
        r,
        k,
        alpha,
        beta,
        delta,
    };
    (next, events)
}

// ── Flora ─────────────────────────────────────────────────────────────────────

/// What kind of flora an entity represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FloraKind {
    /// Full-grown tree that can spread seeds.
    Tree,
    /// Low-growing bush or shrub.
    Shrub,
    /// Farm-managed crop plant.
    Crop,
    /// Dead wood — decays over time.
    DeadWood,
}

/// Shared component for flora entities — attached to every tree/shrub/crop/deadwood.
///
/// `stage` follows the same `[0.0, 1.0]` convention as `GrowthStage`:
/// - 0.0 = seedling
/// - 0.5 = sapling
/// - 1.0 = mature / full-grown
///
/// Trees at `stage >= 0.9` have a per-tick chance to spawn seedlings on adjacent tiles.
/// DeadWood entities decay (`age_ticks` increments) and are removed at a threshold.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FloraState {
    pub kind: FloraKind,
    /// Growth stage [0.0, 1.0].
    pub stage: f32,
    /// Ticks this entity has existed.
    pub age_ticks: u32,
}

impl FloraState {
    pub fn new_seedling() -> Self {
        Self { kind: FloraKind::Tree, stage: 0.0, age_ticks: 0 }
    }

    pub fn new_shrub() -> Self {
        Self { kind: FloraKind::Shrub, stage: 0.5, age_ticks: 0 }
    }

    pub fn new_deadwood() -> Self {
        Self { kind: FloraKind::DeadWood, stage: 0.0, age_ticks: 0 }
    }

    pub fn mature_tree() -> Self {
        Self { kind: FloraKind::Tree, stage: 1.0, age_ticks: 0 }
    }
}

/// Growth rate per tick for a tree based on the biome tile kind it stands on.
///
/// Temperate and tropical biomes have fast growth; polar biomes are slow.
/// Returns growth increment per world-sim tick (1 Hz).
pub fn tree_growth_rate(kind: crate::world::map::TileKind) -> f32 {
    use crate::world::map::TileKind;
    match kind {
        TileKind::TemperateForest | TileKind::Forest
        | TileKind::TropicalForest | TileKind::TropicalRainforest => 1.0 / 200.0,
        TileKind::Grassland | TileKind::Plains | TileKind::Savanna => 1.0 / 300.0,
        TileKind::Taiga => 1.0 / 500.0,
        TileKind::Tundra | TileKind::PolarDesert | TileKind::Arctic => 1.0 / 800.0,
        _ => 1.0 / 400.0,
    }
}

// ── Farm plots ────────────────────────────────────────────────────────────────

/// Per-tick growth rate for farm crops (1/200 ticks → harvestable in ~3 min).
pub const CROP_GROWTH_RATE: f32 = 1.0 / 200.0;

/// State for one farm plot near a settlement.
///
/// `crop_stage`:
/// - 0.0 = fallow (just harvested or just established)
/// - 0.0–1.0 = growing
/// - 1.0 = harvestable
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FarmPlotState {
    /// Settlement UUID this farm belongs to.
    pub settlement_id: uuid::Uuid,
    /// Crop growth stage [0.0, 1.0].
    pub crop_stage: f32,
    /// World-sim tick when the last harvest occurred.
    pub last_harvest_tick: u32,
    /// How much food the last harvest yielded.
    pub yield_amount: u32,
}

impl FarmPlotState {
    pub fn new(settlement_id: uuid::Uuid) -> Self {
        Self { settlement_id, crop_stage: 0.0, last_harvest_tick: 0, yield_amount: 0 }
    }

    /// Advance crop growth by one tick.  Returns `true` if the crop is ready to harvest.
    pub fn tick_growth(&mut self) -> bool {
        if self.crop_stage < 1.0 {
            self.crop_stage = (self.crop_stage + CROP_GROWTH_RATE).min(1.0);
        }
        self.crop_stage >= 1.0
    }

    /// Harvest the crop: reset stage and compute yield (10–30 food units).
    pub fn harvest(&mut self, current_tick: u32) -> u32 {
        let yield_val = 10 + (self.crop_stage * 20.0) as u32;
        self.crop_stage = 0.0;
        self.last_harvest_tick = current_tick;
        self.yield_amount = yield_val;
        yield_val
    }
}

// ── Animal behavior ───────────────────────────────────────────────────────────

/// Ecological role of an animal entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EcologyRole {
    /// Prey animals: deer, rabbit — graze and flee predators.
    Prey,
    /// Predator animals: wolf, bear — hunt prey.
    Predator,
    /// Scavenger animals — opportunistically eat dead prey.
    Scavenger,
}

/// Current behavioral state of an animal.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AnimalState {
    /// Peacefully eating vegetation.
    Grazing,
    /// Running away from a threat.
    Fleeing,
    /// Actively pursuing and attacking a target.
    Hunting,
    /// Resting / inactive.
    Resting,
}

/// Loot dropped when an animal is killed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LootKind {
    Meat,
    Hide,
    Bone,
}

// ── Ecological balance helpers ────────────────────────────────────────────────

/// Cap multiplier applied to prey when there are no predators in the region.
pub const PREY_OVERPOPULATION_CAP: f64 = 2.0;

/// Fraction by which predator population is reduced when prey collapses.
pub const PREDATOR_STARVATION_HALVING: f64 = 0.5;

/// Apply spatial distribution logic to a region after the normal Lotka-Volterra step.
///
/// - If prey drops to 0, halve predator population (starvation).
/// - If predators drop to 0, cap prey at `normal_max * PREY_OVERPOPULATION_CAP`.
///
/// Returns any balance events that occurred.
pub fn apply_spatial_balance(
    ecology: &mut RegionEcology,
    normal_prey_max: f64,
) -> Vec<EcologyEvent> {
    let mut events = Vec::new();

    if ecology.predator.count < COLLAPSE_THRESHOLD {
        // No predators → prey overpopulates but is capped.
        let cap = normal_prey_max * PREY_OVERPOPULATION_CAP;
        if ecology.prey.count > cap {
            ecology.prey.count = cap;
        }
    }

    if ecology.prey.count < COLLAPSE_THRESHOLD {
        // No prey → predators starve.
        let old = ecology.predator.count;
        ecology.predator.count = (old * PREDATOR_STARVATION_HALVING).max(0.0);
        if old >= COLLAPSE_THRESHOLD && ecology.predator.count < COLLAPSE_THRESHOLD {
            events.push(EcologyEvent::Collapse {
                species: ecology.predator.species.clone(),
                region: ecology.region.clone(),
            });
        }
    }

    events
}

// ── Cave creature type constants ─────────────────────────────────────────────

/// Cave herbivore species id: slow-reproducing fungus-eater.
pub const CAVE_HERBIVORE: &str = "cave_grub";
/// Cave predator species id: cave predator.
pub const CAVE_PREDATOR: &str = "troglodyte";
/// Fungus-zone specific colonial organism species id.
pub const MYCONID_COLONY: &str = "myconid";

// ── Cave ecology ─────────────────────────────────────────────────────────────

/// Carrying capacity for prey species based on the dominant cave biome near a
/// settlement.  Callers sample nearby tiles and pass the most common open kind.
///
/// - `CaveFloor`   — moderate food/resource density → standard capacity.
/// - `CrystalCave` — rich bioluminescent resources → elevated capacity.
/// - `LavaFloor`   — hostile heat, sparse life → reduced capacity.
/// - anything else — treated as a sealed wall → minimal capacity.
pub fn cave_carrying_capacity(dominant_kind: crate::world::map::TileKind) -> f64 {
    use crate::world::map::TileKind;
    match dominant_kind {
        TileKind::CaveFloor  => 100.0,
        TileKind::CrystalCave => 150.0,
        TileKind::LavaFloor  => 40.0,
        _                    => 10.0,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_region(prey: f64, predator: f64) -> RegionEcology {
        RegionEcology {
            region: RegionId("test".into()),
            prey: Population { species: SpeciesId("rabbit".into()), count: prey },
            predator: Population { species: SpeciesId("fox".into()), count: predator },
            r: 0.5,
            k: 200.0,
            alpha: 0.01,
            beta: 0.5,
            delta: 0.1,
        }
    }

    #[test]
    fn populations_stay_non_negative() {
        // Extreme predator pressure should clamp prey to 0, not go negative.
        let state = make_region(1.0, 1_000.0);
        let (next, _) = tick_ecology(state);
        assert!(next.prey.count >= 0.0);
        assert!(next.predator.count >= 0.0);
    }

    #[test]
    fn prey_grows_without_predators() {
        let state = make_region(50.0, 0.0);
        let (next, _) = tick_ecology(state);
        assert!(next.prey.count > 50.0, "prey should grow with no predators");
    }

    #[test]
    fn collapse_event_emitted() {
        // Start prey just above threshold, force a big step down.
        let mut state = make_region(6.0, 500.0);
        state.alpha = 0.1; // very high predation
        let (_, events) = tick_ecology(state);
        assert!(
            events.iter().any(|e| matches!(e, EcologyEvent::Collapse { .. })),
            "expected a Collapse event"
        );
    }

    #[test]
    fn deterministic() {
        let s1 = make_region(100.0, 20.0);
        let s2 = make_region(100.0, 20.0);
        let (r1, e1) = tick_ecology(s1);
        let (r2, e2) = tick_ecology(s2);
        assert_eq!(r1.prey.count, r2.prey.count);
        assert_eq!(r1.predator.count, r2.predator.count);
        assert_eq!(e1, e2);
    }
}
