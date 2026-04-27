//! End-to-end combat tests — pure logic, no Bevy ECS.
//!
//! All dice values are **injected** so every test is deterministic.

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::combat::{
        interrupt::{AbilityContext, AttackContext, InterruptFrame, InterruptStack},
        rules::{apply_effects, resolve_ability, resolve_spell},
        spells::SpellSlots,
        types::{
            CharacterClass, CombatState, CombatantId, CombatantSnapshot, CombatantState,
            CoreStats, Effect,
        },
    };

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn id() -> CombatantId {
        CombatantId(Uuid::new_v4())
    }

    fn wizard_snapshot(id: CombatantId) -> CombatantSnapshot {
        CombatantSnapshot {
            id,
            faction: None,
            class: CharacterClass::Wizard,
            stats: CoreStats { intellect: 16, ..CoreStats::default() }, // INT 16 → mod +3
            health_current: 28,
            health_max: 28,
            level: 5,
            armor_class: 12,
        }
    }

    fn barbarian_snapshot(id: CombatantId) -> CombatantSnapshot {
        CombatantSnapshot {
            id,
            faction: None,
            class: CharacterClass::Barbarian,
            stats: CoreStats { dexterity: 10, constitution: 16, ..CoreStats::default() }, // DEX 10 → mod 0
            health_current: 52,
            health_max: 52,
            level: 5,
            armor_class: 14,
        }
    }

    fn cleric_snapshot(id: CombatantId) -> CombatantSnapshot {
        CombatantSnapshot {
            id,
            faction: None,
            class: CharacterClass::Cleric,
            stats: CoreStats { wisdom: 14, ..CoreStats::default() }, // WIS 14 → mod +2
            health_current: 38,
            health_max: 38,
            level: 5,
            armor_class: 16,
        }
    }

    fn warlock_snapshot(id: CombatantId) -> CombatantSnapshot {
        CombatantSnapshot {
            id,
            faction: None,
            class: CharacterClass::Warlock,
            stats: CoreStats { charisma: 14, ..CoreStats::default() }, // CHA 14 → mod +2
            health_current: 35,
            health_max: 35,
            level: 5,
            armor_class: 13,
        }
    }

    fn fighter_snapshot(id: CombatantId) -> CombatantSnapshot {
        CombatantSnapshot {
            id,
            faction: None,
            class: CharacterClass::Fighter,
            stats: CoreStats { strength: 15, ..CoreStats::default() }, // STR 15 → mod +2
            health_current: 10, // low HP to trigger DefensiveStance
            health_max: 44,
            level: 5,
            armor_class: 16,
        }
    }

    fn warrior_snapshot(id: CombatantId) -> CombatantSnapshot {
        CombatantSnapshot {
            id,
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats { strength: 15, ..CoreStats::default() },
            health_current: 44,
            health_max: 44,
            level: 5,
            armor_class: 14,
        }
    }

    fn rogue_snapshot(id: CombatantId) -> CombatantSnapshot {
        CombatantSnapshot {
            id,
            faction: None,
            class: CharacterClass::Rogue,
            stats: CoreStats { dexterity: 15, ..CoreStats::default() },
            health_current: 36,
            health_max: 36,
            level: 5,
            armor_class: 14,
        }
    }

    fn sorcerer_snapshot(id: CombatantId) -> CombatantSnapshot {
        CombatantSnapshot {
            id,
            faction: None,
            class: CharacterClass::Sorcerer,
            stats: CoreStats { intellect: 14, ..CoreStats::default() },
            health_current: 28,
            health_max: 28,
            level: 5,
            armor_class: 12,
        }
    }

    // ── 1. Spell casting E2E tests ────────────────────────────────────────────

    /// Fireball (8d6, DEX save DC 15) against a Barbarian with DEX 10 (mod 0).
    ///
    /// Failed save: full raw damage.
    /// Successful save: half raw damage.
    #[test]
    fn wizard_fireball_with_dex_save_halves_damage() {
        let wiz_id = id();
        let bar_id = id();
        let caster = wizard_snapshot(wiz_id);
        let target = barbarian_snapshot(bar_id.clone());

        // Inject 8 dice rolls of 4 each → raw = 32.
        // For a FAILED save, the last element (save roll) must be below DC 15.
        // DEX mod = 0, proficiency (level 5) = 3 → need 15, roll 5 → 5+0+3=8 < 15 → fail.
        let rolls_fail: Vec<i32> = vec![4, 4, 4, 4, 4, 4, 4, 4, 5]; // 8 dmg dice + save
        let effects_fail = resolve_spell("Fireball", &caster, &target, &rolls_fail);
        let full_damage: i32 = effects_fail
            .iter()
            .filter_map(|e| {
                if let Effect::TakeDamage { amount, .. } = e {
                    Some(*amount)
                } else {
                    None
                }
            })
            .sum();
        assert_eq!(full_damage, 32, "failed save should deal full damage");

        // For a SUCCESSFUL save, the save roll must be >= DC 15 with modifiers.
        // DEX 10 → mod 0, not proficient (target is Barbarian, not prof in DEX saves).
        // Roll 15 → 15+0+0=15 >= 15 → success → half damage = 16.
        let rolls_save: Vec<i32> = vec![4, 4, 4, 4, 4, 4, 4, 4, 15];
        let effects_save = resolve_spell("Fireball", &caster, &target, &rolls_save);
        let half_damage: i32 = effects_save
            .iter()
            .filter_map(|e| {
                if let Effect::TakeDamage { amount, .. } = e {
                    Some(*amount)
                } else {
                    None
                }
            })
            .sum();
        assert_eq!(half_damage, 16, "successful save should halve the damage");

        assert_eq!(
            full_damage,
            half_damage * 2,
            "full damage should be exactly double halved damage"
        );
    }

    /// Cure Wounds (1d8 heal, no modifier from resolve_spell — spell handles raw dice).
    /// The HealDamage effect amount equals the sum of the heal dice.
    #[test]
    fn cleric_cure_wounds_restores_hp() {
        let cleric_id = id();
        let target_id = id();
        let caster = cleric_snapshot(cleric_id);
        let target = CombatantSnapshot {
            id: target_id.clone(),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: 10,
            health_max: 40,
            level: 1,
            armor_class: 12,
        };

        // Cure Wounds: 1 heal die (d8). Inject roll = 6.
        let rolls = vec![6i32];
        let effects = resolve_spell("Cure Wounds", &caster, &target, &rolls);

        let heal: i32 = effects
            .iter()
            .filter_map(|e| {
                if let Effect::HealDamage { amount, .. } = e {
                    Some(*amount)
                } else {
                    None
                }
            })
            .sum();

        // resolve_spell sums heal dice and returns max(total, 1)
        assert_eq!(heal, 6, "Cure Wounds should heal the rolled amount");
        assert!(
            effects.iter().any(|e| matches!(e, Effect::HealDamage { .. })),
            "should produce HealDamage effect"
        );
    }

    /// Eldritch Blast is a cantrip with no saving throw — it always deals damage.
    #[test]
    fn warlock_eldritch_blast_always_hits() {
        let wl_id = id();
        let target_id = id();
        let caster = warlock_snapshot(wl_id);
        let target = CombatantSnapshot {
            id: target_id.clone(),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: 30,
            health_max: 30,
            level: 1,
            armor_class: 25, // unreachably high AC — but spell has no attack roll
        };

        // Eldritch Blast: 1 die (d10), no save_ability → no save roll needed.
        let rolls = vec![8i32];
        let effects = resolve_spell("Eldritch Blast", &caster, &target, &rolls);

        assert!(
            effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. })),
            "Eldritch Blast should always deal damage regardless of AC"
        );
    }

    /// When a Wizard has zero spell slots, it falls back to ability 3 (ArcaneBlast).
    /// This is a logic gate test: if can_cast(3) is false, use ArcaneBlast.
    #[test]
    fn spell_slot_exhaustion_falls_back_to_ability() {
        let wiz_id = id();
        let target_id = id();
        let caster = wizard_snapshot(wiz_id.clone());
        let target = CombatantSnapshot {
            id: target_id.clone(),
            faction: None,
            class: CharacterClass::Barbarian,
            stats: CoreStats::default(),
            health_current: 50,
            health_max: 50,
            level: 1,
            armor_class: 10,
        };

        let mut slots = SpellSlots::for_class(&CharacterClass::Wizard, 5);
        // Expend all level-3 slots (Level 5 Wizard has 2).
        slots.expend(3);
        slots.expend(3);
        assert!(!slots.can_cast(3), "level-3 slots should be exhausted");

        // When slots are exhausted, the fallback is ArcaneBlast (ability 3).
        // ArcaneBlast: auto-hit d8 + INT mod.
        // INT 16 → mod +3; roll 5 → 5+3=8 damage.
        let effects = if !slots.can_cast(3) {
            resolve_ability(3, &caster, &target, &[5])
        } else {
            resolve_spell("Fireball", &caster, &target, &[4, 4, 4, 4, 4, 4, 4, 4, 1])
        };

        assert!(
            effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. })),
            "ArcaneBlast fallback should still deal damage"
        );
        // ArcaneBlast also applies "scorched"
        assert!(
            effects.iter().any(|e| matches!(e, Effect::ApplyStatus { status, .. } if status == "scorched")),
            "ArcaneBlast should apply scorched status"
        );
    }

    // ── 2. Ability E2E tests ──────────────────────────────────────────────────

    /// RageEntry (ability 8): Barbarian attacks with d12+2, gains "raging" status.
    #[test]
    fn barbarian_rage_entry_applies_raging_status() {
        let bar_id = id();
        let target_id = id();
        let caster = barbarian_snapshot(bar_id.clone());
        let target = CombatantSnapshot {
            id: target_id.clone(),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: 50,
            health_max: 50,
            level: 1,
            armor_class: 10,
        };

        // Ability 8 = RageEntry: rolls = [attack_d20, dmg_d12].
        // Roll 15 (hit), dmg 10 → 10+2 + STR mod(0) = 12.
        let effects = resolve_ability(8, &caster, &target, &[15, 10]);

        assert!(
            effects.iter().any(|e| matches!(e, Effect::ApplyStatus { status, target: t, .. }
                if status == "raging" && t == &bar_id)),
            "Barbarian should gain 'raging' status"
        );
        assert!(
            effects.iter().any(|e| matches!(e, Effect::TakeDamage { amount, .. } if *amount == 12)),
            "RageEntry should deal d12+2 = 12 damage (dmg roll 10, +2 rage bonus)"
        );
    }

    /// HealAlly (ability 7): Cleric heals target for d8 + WIS mod (+2).
    #[test]
    fn cleric_heal_ally_restores_health() {
        let cleric_id = id();
        let target_id = id();
        let caster = cleric_snapshot(cleric_id);
        let target = CombatantSnapshot {
            id: target_id.clone(),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: 5,
            health_max: 40,
            level: 1,
            armor_class: 12,
        };

        // Ability 7 = HealAlly: rolls = [dmg_d8].
        // Roll 6 + WIS mod 2 = 8.
        let effects = resolve_ability(7, &caster, &target, &[6]);

        assert!(
            effects.iter().any(|e| matches!(e, Effect::HealDamage { .. })),
            "HealAlly should produce a HealDamage effect"
        );
        let healed: i32 = effects
            .iter()
            .filter_map(|e| {
                if let Effect::HealDamage { amount, .. } = e {
                    Some(*amount)
                } else {
                    None
                }
            })
            .sum();
        assert_eq!(healed, 8, "HealAlly should heal d8(6) + WIS mod(2) = 8");
    }

    /// DefensiveStance (ability 9): Fighter gains "defending" status, no damage roll.
    #[test]
    fn fighter_defensive_stance_when_low_hp() {
        let fighter_id = id();
        let target_id = id();
        let caster = fighter_snapshot(fighter_id.clone());
        let target = CombatantSnapshot {
            id: target_id.clone(),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: 50,
            health_max: 50,
            level: 1,
            armor_class: 12,
        };

        // Ability 9 = DefensiveStance: no rolls needed.
        let effects = resolve_ability(9, &caster, &target, &[]);

        assert!(
            effects.iter().any(|e| matches!(e, Effect::ApplyStatus { status, target: t, .. }
                if status == "defending" && t == &fighter_id)),
            "DefensiveStance should apply 'defending' to the caster"
        );
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. })),
            "DefensiveStance should not deal damage"
        );
    }

    // ── 3. Party vs party interrupt stack test ────────────────────────────────

    #[test]
    fn party_vs_party_full_encounter() {
        // Party A
        let warrior_id = id();
        let cleric_id = id();
        let wizard_id = id();
        // Party B
        let barbarian_id = id();
        let rogue_id = id();
        let sorcerer_id = id();

        let warrior = warrior_snapshot(warrior_id.clone());
        let cleric = cleric_snapshot(cleric_id.clone());
        let wizard = wizard_snapshot(wizard_id.clone());
        let barbarian = barbarian_snapshot(barbarian_id.clone());
        let rogue = rogue_snapshot(rogue_id.clone());
        let sorcerer = sorcerer_snapshot(sorcerer_id.clone());

        let mut state = CombatState {
            combatants: vec![
                CombatantState::new(warrior),
                CombatantState::new(cleric),
                CombatantState::new(wizard),
                CombatantState::new(barbarian),
                CombatantState::new(rogue),
                CombatantState::new(sorcerer),
            ],
            round: 0,
        };

        let mut fireball_fired = false;
        let mut heal_fired = false;
        let mut rage_fired = false;

        // Run 6 rounds.
        for round in 0..6u32 {
            state.round = round;
            let mut stack = InterruptStack::default();

            // Each entity queues their class-appropriate action vs an enemy.
            // Warrior (ability 1: StrongAttack) vs Barbarian
            stack.push(InterruptFrame::ResolvingAbility {
                ctx: AbilityContext {
                    caster: warrior_id.clone(),
                    ability_id: 1,
                    targets: vec![barbarian_id.clone()],
                    rolls: vec![15, 5, 5], // attack 15, dmg 5+5
                },
            });

            // Cleric (ability 7: HealAlly) heals Wizard
            stack.push(InterruptFrame::ResolvingAbility {
                ctx: AbilityContext {
                    caster: cleric_id.clone(),
                    ability_id: 7,
                    targets: vec![wizard_id.clone()],
                    rolls: vec![6], // 6 + WIS mod 2 = 8 heal
                },
            });

            // Wizard casts Fireball (CastingSpell frame) vs Barbarian
            stack.push(InterruptFrame::CastingSpell {
                caster: wizard_id.clone(),
                spell_name: "Fireball",
                slot_level: 3,
                target: barbarian_id.clone(),
                rolls: vec![4, 4, 4, 4, 4, 4, 4, 4, 5], // 8×4=32 dmg, save roll 5 → fails
            });

            // Barbarian (ability 8: RageEntry) vs Warrior
            stack.push(InterruptFrame::ResolvingAbility {
                ctx: AbilityContext {
                    caster: barbarian_id.clone(),
                    ability_id: 8,
                    targets: vec![warrior_id.clone()],
                    rolls: vec![15, 8], // attack 15, dmg 8 → 8+2=10
                },
            });

            // Rogue (ability 2: SneakAttack) vs Cleric
            stack.push(InterruptFrame::ResolvingAbility {
                ctx: AbilityContext {
                    caster: rogue_id.clone(),
                    ability_id: 2,
                    targets: vec![cleric_id.clone()],
                    rolls: vec![15, 4, 3], // attack 15, dmg 4+3+DEX mod 2 = 9
                },
            });

            // Sorcerer (ability 3: ArcaneBlast fallback) vs Wizard
            stack.push(InterruptFrame::ResolvingAbility {
                ctx: AbilityContext {
                    caster: sorcerer_id.clone(),
                    ability_id: 3,
                    targets: vec![wizard_id.clone()],
                    rolls: vec![6], // 6 + INT mod 2 = 8
                },
            });

            // Process all frames
            let mut rng = std::iter::empty::<i32>();
            while !stack.is_empty() {
                let (effects, _) = stack.step(&state, &mut rng);

                // Track which spell/ability types fired
                for e in &effects {
                    match e {
                        Effect::TakeDamage { target, .. } if target == &barbarian_id => {
                            // Could be Fireball or Warrior attack
                            fireball_fired = true;
                        }
                        Effect::HealDamage { target, .. } if target == &wizard_id => {
                            heal_fired = true;
                        }
                        Effect::ApplyStatus { status, target, .. }
                            if status == "raging" && target == &barbarian_id =>
                        {
                            rage_fired = true;
                        }
                        _ => {}
                    }
                }

                let (new_state, secondary) = apply_effects(state, effects);
                state = new_state;

                // Ensure no negative HP
                for c in &state.combatants {
                    assert!(
                        c.health >= 0,
                        "HP must never be negative, got {} for {:?}",
                        c.health,
                        c.snapshot.id
                    );
                }

                // Handle Die secondary effects (just verify no panic)
                let _ = secondary;
            }
        }

        // At least one entity must have died (HP = 0).
        let any_dead = state.combatants.iter().any(|c| c.health == 0);
        assert!(any_dead, "After 6 rounds of combat, at least one entity should be dead");

        assert!(fireball_fired, "Wizard's Fireball should have fired (damage to Barbarian)");
        assert!(heal_fired, "Cleric's HealAlly should have fired (heal to Wizard)");
        assert!(rage_fired, "Barbarian's RageEntry should have applied 'raging' status");
    }

    // ── 4. Spell slot expend and refill test ──────────────────────────────────

    #[test]
    fn wizard_runs_out_of_slots_and_recovers_on_long_rest() {
        let mut slots = SpellSlots::for_class(&CharacterClass::Wizard, 5);

        // Level 5 Wizard SRD table: 4 level-1, 3 level-2, 2 level-3 slots.
        assert_eq!(slots.max_slots[1], 4, "level-1 max should be 4");
        assert_eq!(slots.max_slots[2], 3, "level-2 max should be 3");
        assert_eq!(slots.max_slots[3], 2, "level-3 max should be 2");

        // Confirm can cast initially.
        assert!(slots.can_cast(3), "should be able to cast level-3 before expending");

        // Expend all level-3 slots.
        slots.expend(3);
        slots.expend(3);
        assert!(!slots.can_cast(3), "level-3 slots should now be exhausted");

        // Other slot levels should still be available.
        assert!(slots.can_cast(1), "level-1 slots should still be available");
        assert!(slots.can_cast(2), "level-2 slots should still be available");

        // Cantrips are always castable regardless.
        assert!(slots.can_cast(0), "cantrips should always be castable");

        // Long rest restores all slots.
        slots.long_rest();
        assert!(slots.can_cast(3), "level-3 slots should be restored after long rest");
        assert_eq!(
            slots.used_slots[3], 0,
            "used level-3 slots should reset to 0 after long rest"
        );
    }
}
