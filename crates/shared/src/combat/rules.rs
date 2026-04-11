//! Pure combat rule functions: fn(State, dice) -> (State, Vec<Effect>).
//!
//! Dice values are always **injected** — never rolled internally.
//! This makes every function trivially proptest-able and replay-able.

use crate::combat::types::{
    AttackRollResult, CombatState, CombatantId, CombatantSnapshot, Effect,
};
use smol_str::SmolStr;

/// Attack threshold (to-hit) — roll must beat this (after modifiers) to land.
const BASE_ATTACK_DC: i32 = 10;
/// Roll required (before modifiers) for a critical hit.
const CRIT_THRESHOLD: i32 = 20;

// ── Attack roll ───────────────────────────────────────────────────────────────

/// Determine hit/miss/crit from an injected d20 roll.
///
/// `roll` — integer in [1, 20] provided by caller (test or ECS bridge).
pub fn resolve_attack_roll(
    attacker: &CombatantSnapshot,
    _defender: &CombatantSnapshot,
    roll: i32,
) -> AttackRollResult {
    if roll >= CRIT_THRESHOLD {
        return AttackRollResult::CriticalHit;
    }
    let total = roll + attacker.str_mod();
    if total >= BASE_ATTACK_DC {
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
            let raw = (dmg_roll + attacker.str_mod()).max(1);
            let reduced = (raw - defender.armor).max(0);
            vec![Effect::TakeDamage {
                target: defender.id.clone(),
                amount: reduced,
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

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::types::{CharacterClass, CombatantSnapshot, CombatantState, CoreStats};
    use uuid::Uuid;

    fn make_snapshot(id: Uuid, hp: i32, armor: i32) -> CombatantSnapshot {
        CombatantSnapshot {
            id: CombatantId(id),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: hp,
            health_max: hp,
            level: 1,
            armor,
        }
    }

    fn make_state(attacker_hp: i32, defender_hp: i32) -> (CombatState, CombatantId, CombatantId) {
        let aid = Uuid::new_v4();
        let did = Uuid::new_v4();
        let state = CombatState {
            combatants: vec![
                CombatantState::new(make_snapshot(aid, attacker_hp, 0)),
                CombatantState::new(make_snapshot(did, defender_hp, 2)),
            ],
            round: 0,
        };
        (state, CombatantId(aid), CombatantId(did))
    }

    #[test]
    fn crit_always_hits_ignores_armor() {
        let (state, aid, did) = make_state(20, 20);
        let (_, effects) = resolve_round(state, &aid, &did, 20, 5);
        assert!(effects.iter().any(|e| matches!(e, Effect::TakeDamage { amount, .. } if *amount > 5)));
    }

    #[test]
    fn miss_deals_no_damage() {
        let (state, aid, did) = make_state(20, 20);
        let (_, effects) = resolve_round(state, &aid, &did, 1, 5);
        assert!(!effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. })));
    }

    #[test]
    fn lethal_damage_emits_die() {
        let (state, aid, did) = make_state(20, 1);
        // High roll + high dmg should kill defender
        let (_, effects) = resolve_round(state, &aid, &did, 20, 10);
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

    #[test]
    fn strong_attack_deals_more_damage_than_basic_on_same_roll() {
        let (state, caster_id, target_id) = make_state(20, 100);
        let caster = state.get(&caster_id).unwrap().snapshot.clone();
        let target = state.get(&target_id).unwrap().snapshot.clone();

        // Same attack roll (guaranteed hit), dmg roll = 4 for basic; 4+4 for strong.
        let basic_effects  = resolve_damage(&AttackRollResult::Hit, &caster, &target, 4);
        let strong_effects = resolve_ability(1, &caster, &target, &[15, 4, 4]);

        let basic_dmg: i32 = basic_effects.iter()
            .filter_map(|e| if let Effect::TakeDamage { amount, .. } = e { Some(*amount) } else { None })
            .sum();
        let strong_dmg: i32 = strong_effects.iter()
            .filter_map(|e| if let Effect::TakeDamage { amount, .. } = e { Some(*amount) } else { None })
            .sum();

        assert!(strong_dmg > basic_dmg, "strong_dmg={strong_dmg}, basic_dmg={basic_dmg}");
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
