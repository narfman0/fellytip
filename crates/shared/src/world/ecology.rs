//! Discrete-time Lotka-Volterra population dynamics per region.
//!
//! Each world-sim tick (1 Hz) the server calls `tick_ecology` on every
//! `RegionEcology`. The function is pure — no RNG, no I/O — so it is
//! trivially unit-testable and proptest-fuzzable.

use smol_str::SmolStr;

// ── Identifiers ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpeciesId(pub SmolStr);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RegionId(pub SmolStr);

// ── Population state ──────────────────────────────────────────────────────────

/// Population of one species in one region.
#[derive(Clone, Debug, PartialEq)]
pub struct Population {
    pub species: SpeciesId,
    /// Current population count (clamped to ≥ 0).
    pub count: f64,
}

/// All species populations inside one region plus their interaction parameters.
#[derive(Clone, Debug)]
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

    // Discrete Lotka-Volterra step
    let new_p = (p * (1.0 + r * (1.0 - p / k)) - alpha * q * p).max(0.0);
    let new_q = (q * (beta * alpha * p - delta)).max(0.0);

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
