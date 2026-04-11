fn main() {
    println!("combat_sim: run `cargo test -p combat_sim` to execute proptest traces");
}

// ── Proptest: ecology invariants ───────────────────────────────────────────────

#[cfg(test)]
mod ecology_props {
    use fellytip_shared::world::ecology::{
        EcologyEvent, Population, RegionEcology, RegionId, SpeciesId, tick_ecology,
    };
    use proptest::prelude::*;

    fn arb_region() -> impl Strategy<Value = RegionEcology> {
        (
            0.0f64..=500.0, // prey count
            0.0f64..=200.0, // predator count
            0.01f64..=1.0,  // r
            50.0f64..=500.0, // k
            0.001f64..=0.05, // alpha
            0.1f64..=0.9,   // beta
            0.01f64..=0.5,  // delta
        )
            .prop_map(|(prey, pred, r, k, alpha, beta, delta)| RegionEcology {
                region: RegionId("prop".into()),
                prey: Population {
                    species: SpeciesId("rabbit".into()),
                    count: prey,
                },
                predator: Population {
                    species: SpeciesId("fox".into()),
                    count: pred,
                },
                r,
                k,
                alpha,
                beta,
                delta,
            })
    }

    proptest! {
        #[test]
        fn populations_never_negative(state in arb_region()) {
            let (next, _) = tick_ecology(state);
            prop_assert!(next.prey.count >= 0.0, "prey went negative: {}", next.prey.count);
            prop_assert!(next.predator.count >= 0.0, "predator went negative: {}", next.predator.count);
        }

        #[test]
        fn tick_is_deterministic(state in arb_region()) {
            let (r1, e1) = tick_ecology(state.clone());
            let (r2, e2) = tick_ecology(state);
            prop_assert_eq!(r1.prey.count, r2.prey.count);
            prop_assert_eq!(r1.predator.count, r2.predator.count);
            prop_assert_eq!(e1, e2);
        }

        #[test]
        fn collapse_only_when_crossing_threshold(state in arb_region()) {
            use fellytip_shared::world::ecology::COLLAPSE_THRESHOLD;
            let (next, events) = tick_ecology(state.clone());
            let had_prey_collapse = events.iter().any(|e| {
                matches!(e, EcologyEvent::Collapse { species, .. } if species.0 == "rabbit")
            });
            if had_prey_collapse {
                // If we emitted a collapse, prey must be below threshold now
                // and was above (or at) threshold before.
                prop_assert!(
                    next.prey.count < COLLAPSE_THRESHOLD,
                    "Collapse emitted but next count = {}",
                    next.prey.count
                );
                prop_assert!(
                    state.prey.count >= COLLAPSE_THRESHOLD,
                    "Collapse emitted but prior count was already below threshold = {}",
                    state.prey.count
                );
            }
        }
    }
}
