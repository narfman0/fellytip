//! War-party combat helpers.
//!
//! `seeded_dice` produces a deterministic sequence of attack and damage rolls
//! keyed on `(settlement_id, tick)` — same inputs always yield the same rolls.
//! `tick_battle_round` is a thin wrapper around the existing `resolve_round`
//! rule function so callers don't need to import the full combat module.

use crate::combat::{
    rules::resolve_round,
    types::{CombatState, CombatantId, Effect},
};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use uuid::Uuid;

// ── Seeded dice ───────────────────────────────────────────────────────────────

/// Returns a deterministic iterator of i32 dice values seeded on
/// `settlement_id XOR tick`.
///
/// Values alternate: attack roll [1, 20], damage roll [1, 8], …
/// The iterator is infinite — callers pull exactly as many values as needed.
pub fn seeded_dice(settlement_id: Uuid, tick: u64) -> impl Iterator<Item = i32> {
    let seed = settlement_id.as_u128() as u64 ^ tick;
    let rng = ChaCha8Rng::seed_from_u64(seed);
    DiceIter { rng, phase: 0 }
}

struct DiceIter {
    rng: ChaCha8Rng,
    /// 0 = attack phase (d20), 1 = damage phase (d8).
    phase: u8,
}

impl Iterator for DiceIter {
    type Item = i32;
    fn next(&mut self) -> Option<i32> {
        let val = if self.phase == 0 {
            (self.rng.random::<u32>() % 20 + 1) as i32 // [1, 20]
        } else {
            (self.rng.random::<u32>() % 8 + 1) as i32  // [1, 8]
        };
        self.phase = 1 - self.phase;
        Some(val)
    }
}

// ── Battle round wrapper ──────────────────────────────────────────────────────

/// Resolve one NPC-vs-NPC combat round using the existing `resolve_round` rule.
///
/// `dice` must yield at least two values: `[attack_roll, dmg_roll]`.
/// Callers obtain `dice` from `seeded_dice` (deterministic) or any iterator
/// (for tests).
pub fn tick_battle_round(
    state: CombatState,
    attacker_id: &CombatantId,
    defender_id: &CombatantId,
    dice: &mut impl Iterator<Item = i32>,
) -> (CombatState, Vec<Effect>) {
    let attack_roll = dice.next().unwrap_or(10);
    let dmg_roll = dice.next().unwrap_or(4);
    resolve_round(state, attacker_id, defender_id, attack_roll, dmg_roll)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_dice_deterministic() {
        let id = Uuid::nil();
        let v1: Vec<i32> = seeded_dice(id, 42).take(10).collect();
        let v2: Vec<i32> = seeded_dice(id, 42).take(10).collect();
        assert_eq!(v1, v2, "same seed must produce same sequence");
    }

    #[test]
    fn seeded_dice_varies_by_tick() {
        let id = Uuid::nil();
        let v1: Vec<i32> = seeded_dice(id, 1).take(10).collect();
        let v2: Vec<i32> = seeded_dice(id, 2).take(10).collect();
        assert_ne!(v1, v2, "different ticks should produce different sequences");
    }

    #[test]
    fn attack_rolls_in_range() {
        let id = Uuid::nil();
        // Take only the attack-phase values (even indices).
        for (i, val) in seeded_dice(id, 0).enumerate().take(20) {
            if i % 2 == 0 {
                assert!((1..=20).contains(&val), "attack roll out of [1,20]: {val}");
            }
        }
    }

    #[test]
    fn damage_rolls_in_range() {
        let id = Uuid::nil();
        // Take only the damage-phase values (odd indices).
        for (i, val) in seeded_dice(id, 0).enumerate().take(20) {
            if i % 2 == 1 {
                assert!((1..=8).contains(&val), "damage roll out of [1,8]: {val}");
            }
        }
    }

    #[test]
    fn tick_battle_round_delegates_to_resolve_round() {
        use crate::combat::types::{
            CharacterClass, CombatantSnapshot, CombatantState, CoreStats,
        };

        let aid = Uuid::new_v4();
        let did = Uuid::new_v4();
        let make_snap = |id: Uuid, hp: i32| CombatantSnapshot {
            id: CombatantId(id),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: hp,
            health_max: hp,
            level: 1,
            armor_class: 10,
        };
        let state = CombatState {
            combatants: vec![
                CombatantState::new(make_snap(aid, 20)),
                CombatantState::new(make_snap(did, 20)),
            ],
            round: 0,
        };

        // Use a forced hit (roll=20 critical) with known damage so we can verify effects.
        let mut dice = std::iter::once(20).chain(std::iter::once(4)).chain(std::iter::repeat(1));
        let (_, effects) = tick_battle_round(state, &CombatantId(aid), &CombatantId(did), &mut dice);
        // Natural 20 crit: damage = 4*2 + str_mod(0) = 8
        assert!(
            effects.iter().any(|e| matches!(e, Effect::TakeDamage { amount, .. } if *amount == 8)),
            "expected crit damage of 8, got: {effects:?}"
        );
    }
}
