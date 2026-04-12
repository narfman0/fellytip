fn main() {
    println!("combat_sim: run `cargo test -p combat_sim` to execute proptest traces");
}

// ── Proptest: combat invariants ───────────────────────────────────────────────

#[cfg(test)]
mod combat_props {
    use fellytip_shared::combat::{
        rules::resolve_round,
        types::{
            CharacterClass, CombatState, CombatantId, CombatantSnapshot, CombatantState,
            CoreStats, Effect,
        },
    };
    use proptest::prelude::*;
    use uuid::Uuid;

    fn arb_snapshot(id: Uuid) -> impl Strategy<Value = CombatantSnapshot> {
        (1i32..=50, 10i32..=20, 1i32..=20).prop_map(move |(hp, armor_class, str_)| {
            CombatantSnapshot {
                id: CombatantId(id),
                faction: None,
                class: CharacterClass::Warrior,
                stats: CoreStats { strength: str_, ..CoreStats::default() },
                health_current: hp,
                health_max: hp,
                level: 1,
                armor_class,
            }
        })
    }

    fn arb_state() -> impl Strategy<Value = (CombatState, CombatantId, CombatantId)> {
        let aid = Uuid::new_v4();
        let did = Uuid::new_v4();
        (arb_snapshot(aid), arb_snapshot(did)).prop_map(move |(a, d)| {
            let state = CombatState {
                combatants: vec![CombatantState::new(a), CombatantState::new(d)],
                round: 0,
            };
            (state, CombatantId(aid), CombatantId(did))
        })
    }

    proptest! {
        #[test]
        fn health_never_negative(
            (state, aid, did) in arb_state(),
            attack_roll in 1i32..=20,
            dmg_roll in 1i32..=12,
        ) {
            let (next, _) = resolve_round(state, &aid, &did, attack_roll, dmg_roll);
            for c in &next.combatants {
                prop_assert!(c.health >= 0, "health went negative for {:?}", c.snapshot.id);
            }
        }

        #[test]
        fn miss_leaves_defender_health_unchanged(
            (state, aid, did) in arb_state(),
            dmg_roll in 1i32..=12,
        ) {
            let before_hp = state.get(&did).map(|c| c.health).unwrap_or(0);
            // Natural 1 is always a miss (SRD fumble rule).
            let (next, effects) = resolve_round(state, &aid, &did, 1, dmg_roll);
            let after_hp = next.get(&did).map(|c| c.health).unwrap_or(0);
            let has_damage = effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. }));
            if !has_damage {
                prop_assert_eq!(before_hp, after_hp);
            }
        }

        #[test]
        fn die_only_emitted_when_health_zero(
            (state, aid, did) in arb_state(),
            attack_roll in 1i32..=20,
            dmg_roll in 1i32..=12,
        ) {
            let (next, effects) = resolve_round(state, &aid, &did, attack_roll, dmg_roll);
            let die_emitted = effects.iter().any(|e| matches!(e, Effect::Die { target } if target == &did));
            let hp_zero = next.get(&did).map(|c| c.health == 0).unwrap_or(false);
            if die_emitted {
                prop_assert!(hp_zero, "Die emitted but health > 0");
            }
        }
    }
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
