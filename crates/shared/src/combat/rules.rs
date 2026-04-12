//! Pure combat rule functions: fn(State, dice) -> (State, Vec<Effect>).
//!
//! Dice values are always **injected** — never rolled internally.
//! This makes every function trivially proptest-able and replay-able.

use crate::combat::types::{
    AttackRollResult, CharacterClass, CombatState, CombatantId, CombatantSnapshot, Effect,
    hit_die_for_class, proficiency_bonus,
};
use smol_str::SmolStr;

// ── Attack roll ───────────────────────────────────────────────────────────────

/// Determine hit/miss/crit from an injected d20 roll.
///
/// Uses SRD attack formula: `d20 + ability_mod + proficiency_bonus >= defender.armor_class`.
/// Natural 20 is always a critical hit; natural 1 is always a miss.
/// See `docs/dnd5e-srd-reference.md`.
///
/// `roll` — integer in [1, 20] provided by caller (test or ECS bridge).
pub fn resolve_attack_roll(
    attacker: &CombatantSnapshot,
    defender: &CombatantSnapshot,
    roll: i32,
) -> AttackRollResult {
    // Natural 20: critical hit regardless of AC (SRD §Attack Rolls).
    if roll == 20 {
        return AttackRollResult::CriticalHit;
    }
    // Natural 1: always a miss regardless of modifiers (SRD §Attack Rolls).
    if roll == 1 {
        return AttackRollResult::Miss;
    }
    // TODO: use DEX for Rogue finesse, INT for Mage spell attacks.
    let ability_mod = attacker.str_mod();
    let prof = proficiency_bonus(attacker.level);
    if roll + ability_mod + prof >= defender.armor_class {
        AttackRollResult::Hit
    } else {
        AttackRollResult::Miss
    }
}

// ── Damage resolution ─────────────────────────────────────────────────────────

/// Calculate damage effects from a successful roll.
///
/// `dmg_roll` — raw damage die value provided by caller.
pub fn resolve_damage(
    result: &AttackRollResult,
    attacker: &CombatantSnapshot,
    defender: &CombatantSnapshot,
    dmg_roll: i32,
) -> Vec<Effect> {
    match result {
        AttackRollResult::Miss => vec![],
        AttackRollResult::Hit => {
            // No flat damage reduction in 5e — AC already determined whether we hit.
            let amount = (dmg_roll + attacker.str_mod()).max(1);
            vec![Effect::TakeDamage {
                target: defender.id.clone(),
                amount,
            }]
        }
        AttackRollResult::CriticalHit => {
            // Crit doubles the die (not the modifier), ignores armour.
            let raw = (dmg_roll * 2 + attacker.str_mod()).max(1);
            vec![Effect::TakeDamage {
                target: defender.id.clone(),
                amount: raw,
            }]
        }
    }
}

// ── Effect application ────────────────────────────────────────────────────────

/// Apply a list of effects to `state`. Returns the updated state and any
/// secondary effects (e.g. `Die` when health reaches 0).
pub fn apply_effects(
    mut state: CombatState,
    effects: Vec<Effect>,
) -> (CombatState, Vec<Effect>) {
    let mut secondary = Vec::new();

    for effect in effects {
        match &effect {
            Effect::TakeDamage { target, amount } => {
                if let Some(c) = state.get_mut(target) {
                    c.health = (c.health - amount).max(0);
                    if c.health == 0 {
                        secondary.push(Effect::Die { target: target.clone() });
                    }
                }
            }
            Effect::HealDamage { target, amount } => {
                if let Some(c) = state.get_mut(target) {
                    c.health = (c.health + amount).min(c.snapshot.health_max);
                }
            }
            Effect::ApplyStatus { target, status } => {
                if let Some(c) = state.get_mut(target) {
                    if !c.statuses.contains(status) {
                        c.statuses.push(status.clone());
                    }
                }
            }
            Effect::Die { .. } => {
                // Die effect is just a marker; ECS bridge handles despawn.
            }
        }
    }

    (state, secondary)
}

// ── Convenience: full round ───────────────────────────────────────────────────

/// Resolve one attack round end-to-end.
///
/// Returns the updated state and all emitted effects (including Die).
pub fn resolve_round(
    state: CombatState,
    attacker_id: &CombatantId,
    defender_id: &CombatantId,
    attack_roll: i32,
    dmg_roll: i32,
) -> (CombatState, Vec<Effect>) {
    let attacker = match state.get(attacker_id) {
        Some(c) => c.snapshot.clone(),
        None => return (state, vec![]),
    };
    let defender = match state.get(defender_id) {
        Some(c) => c.snapshot.clone(),
        None => return (state, vec![]),
    };

    let roll_result = resolve_attack_roll(&attacker, &defender, attack_roll);
    let effects = resolve_damage(&roll_result, &attacker, &defender, dmg_roll);
    let all_effects = effects.clone();
    let (state, secondary) = apply_effects(state, effects);

    let mut all = all_effects;
    all.extend(secondary);
    (state, all)
}

// ── Ability resolution ────────────────────────────────────────────────────────

/// Resolve an activated ability.
///
/// `rolls` — pre-rolled dice injected by the ECS bridge (never rolled here).
/// Layout for ability 1 (StrongAttack): `[attack_d20, dmg_d8_1, dmg_d8_2]`.
/// Unknown `ability_id` values return an empty effect list.
pub fn resolve_ability(
    ability_id: u8,
    caster: &CombatantSnapshot,
    target: &CombatantSnapshot,
    rolls: &[i32],
) -> Vec<Effect> {
    match ability_id {
        1 => {
            // StrongAttack: 2×d8 damage + apply "weakened" status on hit.
            let attack_roll = rolls.first().copied().unwrap_or(1);
            let dmg1 = rolls.get(1).copied().unwrap_or(1);
            let dmg2 = rolls.get(2).copied().unwrap_or(1);
            let roll_result = resolve_attack_roll(caster, target, attack_roll);
            let mut effects = resolve_damage(&roll_result, caster, target, dmg1 + dmg2);
            if matches!(roll_result, AttackRollResult::Hit | AttackRollResult::CriticalHit) {
                effects.push(Effect::ApplyStatus {
                    target: target.id.clone(),
                    status: SmolStr::new("weakened"),
                });
            }
            effects
        }
        _ => vec![],
    }
}

// ── Progression helpers ───────────────────────────────────────────────────────

/// XP required to advance from `level` to `level + 1` (SRD table).
/// Returns `u32::MAX` for level 20+ (no further advancement).
/// See `docs/dnd5e-srd-reference.md`.
pub fn xp_to_next_level(level: u32) -> u32 {
    match level {
        1  =>     300,
        2  =>     600,
        3  =>   1_800,
        4  =>   3_800,
        5  =>   7_500,
        6  =>   9_000,
        7  =>  11_000,
        8  =>  14_000,
        9  =>  16_000,
        10 =>  21_000,
        11 =>  15_000,
        12 =>  20_000,
        13 =>  20_000,
        14 =>  25_000,
        15 =>  30_000,
        16 =>  30_000,
        17 =>  40_000,
        18 =>  40_000,
        19 =>  50_000,
        _  => u32::MAX, // level 20: no further advancement
    }
}

/// HP gained on level-up: roll the class hit die (provided via `rng`) + CON mod, min 1.
///
/// `rng` must yield values in `[1, hit_die_for_class(class)]`. The roll is clamped to
/// that range internally so out-of-bounds injected values cannot produce invalid results.
pub fn hp_on_level_up(
    class: &CharacterClass,
    con_mod: i32,
    rng: &mut impl Iterator<Item = i32>,
) -> i32 {
    let max_die = hit_die_for_class(class);
    let die_roll = rng.next().unwrap_or(1).clamp(1, max_die);
    (die_roll + con_mod).max(1)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::types::{CharacterClass, CombatantSnapshot, CombatantState, CoreStats};
    use uuid::Uuid;

    fn make_snapshot(id: Uuid, hp: i32, armor_class: i32) -> CombatantSnapshot {
        CombatantSnapshot {
            id: CombatantId(id),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: hp,
            health_max: hp,
            level: 1,
            armor_class,
        }
    }

    fn make_state(attacker_hp: i32, defender_hp: i32) -> (CombatState, CombatantId, CombatantId) {
        let aid = Uuid::new_v4();
        let did = Uuid::new_v4();
        let state = CombatState {
            combatants: vec![
                // Attacker: AC 10 (unarmored), defender: AC 12 (leather)
                CombatantState::new(make_snapshot(aid, attacker_hp, 10)),
                CombatantState::new(make_snapshot(did, defender_hp, 12)),
            ],
            round: 0,
        };
        (state, CombatantId(aid), CombatantId(did))
    }

    // ── Attack roll tests ─────────────────────────────────────────────────────

    #[test]
    fn natural_20_always_crits() {
        let (state, aid, did) = make_state(20, 20);
        let (_, effects) = resolve_round(state, &aid, &did, 20, 5);
        // Crit: 5*2 + str_mod(0) = 10 damage
        assert!(effects.iter().any(|e| matches!(e, Effect::TakeDamage { amount, .. } if *amount == 10)));
    }

    #[test]
    fn natural_1_always_misses() {
        let (state, aid, did) = make_state(20, 20);
        let (_, effects) = resolve_round(state, &aid, &did, 1, 5);
        assert!(!effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. })));
    }

    #[test]
    fn ac_check_with_proficiency() {
        // L1 attacker: STR 10 (mod 0), proficiency +2. Defender AC 12.
        // Roll 10: 10 + 0 + 2 = 12 >= 12 → hit.
        // Roll 9:  9 + 0 + 2 = 11 < 12  → miss.
        let aid = Uuid::new_v4();
        let did = Uuid::new_v4();
        let attacker = make_snapshot(aid, 20, 10);
        let defender = make_snapshot(did, 20, 12);
        assert_eq!(resolve_attack_roll(&attacker, &defender, 10), AttackRollResult::Hit);
        assert_eq!(resolve_attack_roll(&attacker, &defender, 9),  AttackRollResult::Miss);
    }

    #[test]
    fn no_dr_on_hit_damage_equals_roll_plus_mod() {
        let aid = Uuid::new_v4();
        let did = Uuid::new_v4();
        let attacker = make_snapshot(aid, 20, 10);
        // Defender has high AC but we pass a guaranteed-hit result directly
        let defender = make_snapshot(did, 50, 20);
        let effects = resolve_damage(&AttackRollResult::Hit, &attacker, &defender, 7);
        let damage: i32 = effects.iter()
            .filter_map(|e| if let Effect::TakeDamage { amount, .. } = e { Some(*amount) } else { None })
            .sum();
        // STR 10 → mod 0; expected = (7 + 0).max(1) = 7; no DR subtracted
        assert_eq!(damage, 7);
    }

    #[test]
    fn lethal_damage_emits_die() {
        let (state, aid, did) = make_state(20, 1);
        let (_, effects) = resolve_round(state, &aid, &did, 20, 1);
        assert!(effects.iter().any(|e| matches!(e, Effect::Die { .. })));
    }

    #[test]
    fn health_never_negative() {
        let (state, aid, did) = make_state(20, 5);
        let (next, _) = resolve_round(state, &aid, &did, 20, 100);
        let defender_health = next.combatants.iter()
            .find(|c| c.snapshot.id == did)
            .map(|c| c.health)
            .unwrap_or(0);
        assert_eq!(defender_health, 0);
    }

    // ── XP table tests ────────────────────────────────────────────────────────

    #[test]
    fn xp_table_spot_checks() {
        assert_eq!(xp_to_next_level(1),  300);
        assert_eq!(xp_to_next_level(4),  3_800);
        assert_eq!(xp_to_next_level(9),  16_000);
        assert_eq!(xp_to_next_level(19), 50_000);
        assert_eq!(xp_to_next_level(20), u32::MAX);
    }

    #[test]
    fn proficiency_bonus_table() {
        for level in 1..=4  { assert_eq!(proficiency_bonus(level), 2, "level {level}"); }
        for level in 5..=8  { assert_eq!(proficiency_bonus(level), 3, "level {level}"); }
        for level in 9..=12 { assert_eq!(proficiency_bonus(level), 4, "level {level}"); }
        for level in 13..=16 { assert_eq!(proficiency_bonus(level), 5, "level {level}"); }
        for level in 17..=20 { assert_eq!(proficiency_bonus(level), 6, "level {level}"); }
    }

    // ── HP on level-up tests ──────────────────────────────────────────────────

    #[test]
    fn hp_on_level_up_minimum_one() {
        // CON mod -5, roll 1: (1 + -5).max(1) = 1
        let mut rng = std::iter::once(1);
        let hp = hp_on_level_up(&CharacterClass::Warrior, -5, &mut rng);
        assert_eq!(hp, 1);
    }

    #[test]
    fn hp_warrior_vs_mage_same_roll() {
        // Roll 8: Warrior clamps to 8 (d10 max=10), Mage clamps to 6 (d6 max=6)
        let warrior_hp = hp_on_level_up(&CharacterClass::Warrior, 0, &mut std::iter::once(8));
        let mage_hp    = hp_on_level_up(&CharacterClass::Mage,    0, &mut std::iter::once(8));
        assert!(warrior_hp >= mage_hp, "warrior={warrior_hp}, mage={mage_hp}");
    }

    // ── Ability tests ─────────────────────────────────────────────────────────

    #[test]
    fn strong_attack_deals_more_damage_than_basic_on_same_roll() {
        let (state, caster_id, target_id) = make_state(20, 100);
        let caster = state.get(&caster_id).unwrap().snapshot.clone();
        let target = state.get(&target_id).unwrap().snapshot.clone();

        // Basic: 4 damage; Strong: 4+4=8 damage (both hit)
        let basic_effects  = resolve_damage(&AttackRollResult::Hit, &caster, &target, 4);
        let strong_effects = resolve_ability(1, &caster, &target, &[15, 4, 4]);

        let sum_dmg = |effects: &[Effect]| -> i32 {
            effects.iter()
                .filter_map(|e| if let Effect::TakeDamage { amount, .. } = e { Some(*amount) } else { None })
                .sum()
        };
        assert!(sum_dmg(&strong_effects) > sum_dmg(&basic_effects));
    }

    #[test]
    fn strong_attack_applies_weakened_status_on_hit() {
        let (state, aid, did) = make_state(20, 100);
        let caster = state.get(&aid).unwrap().snapshot.clone();
        let target = state.get(&did).unwrap().snapshot.clone();
        let effects = resolve_ability(1, &caster, &target, &[15, 4, 4]);
        assert!(effects.iter().any(|e| matches!(e,
            Effect::ApplyStatus { status, .. } if status == "weakened"
        )));
    }

    #[test]
    fn unknown_ability_id_returns_no_effects() {
        let (state, aid, did) = make_state(20, 100);
        let caster = state.get(&aid).unwrap().snapshot.clone();
        let target = state.get(&did).unwrap().snapshot.clone();
        let effects = resolve_ability(99, &caster, &target, &[15, 4, 4]);
        assert!(effects.is_empty());
    }
}
