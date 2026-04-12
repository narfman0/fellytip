//! Deterministic settlement population simulation.
//!
//! Each world-sim tick (1 Hz) the server calls `tick_population` on every
//! `SettlementPopulation`. The function is pure — no RNG, no ECS — and uses a
//! fractional accumulator for births (same technique as Lotka-Volterra in
//! `ecology.rs`) so no dice are needed.

use crate::world::faction::FactionId;
use uuid::Uuid;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Ticks between births at a settlement (300 ticks = 5 real minutes at 1 Hz).
pub const BIRTH_PERIOD: u32 = 300;

/// Adults required before a war party can be dispatched.
pub const WAR_PARTY_THRESHOLD: u32 = 15;

/// Number of warriors pulled from the adult pool per war party.
pub const WAR_PARTY_SIZE: u32 = 10;

/// Tiles moved per world-sim tick (1 Hz) while marching.
pub const MARCH_SPEED: f32 = 2.0;

/// Distance in tiles at which a war party triggers a battle at the target.
pub const BATTLE_RADIUS: f32 = 3.0;

/// Ticks to wait after dispatching a war party before another can form.
/// 600 ticks = 10 real minutes.
pub const WAR_PARTY_COOLDOWN: u32 = 600;

// ── State ─────────────────────────────────────────────────────────────────────

/// Per-settlement mutable population state (runtime, not world-gen output).
#[derive(Clone, Debug)]
pub struct SettlementPopulation {
    pub settlement_id: Uuid,
    pub faction_id: FactionId,
    /// Ticks elapsed since the last birth.  Fires a spawn when it reaches `BIRTH_PERIOD`.
    pub birth_ticks: u32,
    /// Live adult NPC count (updated from ECS before calling `tick_population`).
    pub adult_count: u32,
    /// Live child NPC count (updated from ECS before calling `tick_population`).
    pub child_count: u32,
    /// Home position in world-space (used as spawn origin for children).
    pub home_x: f32,
    pub home_y: f32,
    pub home_z: f32,
    /// Ticks remaining before another war party can form (0 = ready).
    pub war_party_cooldown: u32,
}

// ── Effects emitted by the tick ───────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum PopulationEffect {
    /// Server should spawn a child NPC near the given world position.
    SpawnChild {
        settlement_id: Uuid,
        x: f32,
        y: f32,
        z: f32,
    },
    /// Server should tag `WAR_PARTY_SIZE` adults as war-party members bound
    /// for `target_settlement_id` at world position `(tx, ty)`.
    FormWarParty {
        attacker_faction: FactionId,
        target_settlement_id: Uuid,
        tx: f32,
        ty: f32,
    },
}

// ── Pure tick function ────────────────────────────────────────────────────────

/// Advance one settlement's population by one tick.
///
/// `hostile_targets` — list of `(settlement_id, world_x, world_y)` belonging
/// to factions marked `Disposition::Hostile` toward this settlement's faction.
/// Supplied by the caller from ECS data so this function stays pure.
///
/// Returns the updated state and any effects to apply.
pub fn tick_population(
    mut state: SettlementPopulation,
    hostile_targets: &[(Uuid, f32, f32)],
) -> (SettlementPopulation, Vec<PopulationEffect>) {
    let mut effects = Vec::new();

    // ── Birth accumulation (integer counter — no floating-point drift) ────────
    state.birth_ticks += 1;
    if state.birth_ticks >= BIRTH_PERIOD {
        state.birth_ticks = 0;
        // Deterministic jitter without RNG: golden-ratio angular spread.
        let angle = 1.618_034_f32 * std::f32::consts::TAU;
        effects.push(PopulationEffect::SpawnChild {
            settlement_id: state.settlement_id,
            x: state.home_x + angle.cos(),
            y: state.home_y + angle.sin(),
            z: state.home_z,
        });
    }

    // ── War party formation ───────────────────────────────────────────────────
    if state.war_party_cooldown > 0 {
        state.war_party_cooldown -= 1;
    } else if state.adult_count >= WAR_PARTY_THRESHOLD {
        if let Some(&(target_id, tx, ty)) = nearest_target(state.home_x, state.home_y, hostile_targets) {
            effects.push(PopulationEffect::FormWarParty {
                attacker_faction: state.faction_id.clone(),
                target_settlement_id: target_id,
                tx,
                ty,
            });
            // Deduct war party from the adult count so the threshold isn't
            // re-triggered next tick before the ECS has a chance to mark them.
            state.adult_count = state.adult_count.saturating_sub(WAR_PARTY_SIZE);
            state.war_party_cooldown = WAR_PARTY_COOLDOWN;
        }
    }

    (state, effects)
}

fn nearest_target(hx: f32, hy: f32, targets: &[(Uuid, f32, f32)]) -> Option<&(Uuid, f32, f32)> {
    targets.iter().min_by(|a, b| {
        let da = (a.1 - hx).powi(2) + (a.2 - hy).powi(2);
        let db = (b.1 - hx).powi(2) + (b.2 - hy).powi(2);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pop(adult_count: u32) -> SettlementPopulation {
        SettlementPopulation {
            settlement_id: Uuid::nil(),
            faction_id: FactionId("test".into()),
            birth_ticks: 0,
            adult_count,
            child_count: 0,
            home_x: 0.0,
            home_y: 0.0,
            home_z: 0.0,
            war_party_cooldown: 0,
        }
    }

    #[test]
    fn birth_acc_accumulates_deterministically() {
        let mut state = make_pop(3);
        let mut spawn_count = 0u32;
        for _ in 0..300 {
            let (next, effects) = tick_population(state, &[]);
            state = next;
            spawn_count += effects.iter()
                .filter(|e| matches!(e, PopulationEffect::SpawnChild { .. }))
                .count() as u32;
        }
        assert_eq!(spawn_count, 1, "exactly one child born in 300 ticks");
    }

    #[test]
    fn birth_acc_remainder_carried() {
        let mut state = make_pop(3);
        for _ in 0..150 {
            let (next, effects) = tick_population(state, &[]);
            state = next;
            assert!(
                effects.iter().all(|e| !matches!(e, PopulationEffect::SpawnChild { .. })),
                "no birth in first 150 ticks"
            );
        }
        // After 150 ticks the counter should be at 150 (halfway to BIRTH_PERIOD).
        assert_eq!(state.birth_ticks, 150);
    }

    #[test]
    fn war_party_threshold_at_exactly_15() {
        let target = (Uuid::new_v4(), 100.0f32, 100.0f32);
        let state = make_pop(WAR_PARTY_THRESHOLD);
        let (_, effects) = tick_population(state, &[target]);
        assert!(
            effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
            "war party formed at threshold"
        );
    }

    #[test]
    fn war_party_not_formed_below_threshold() {
        let target = (Uuid::new_v4(), 100.0f32, 100.0f32);
        let state = make_pop(WAR_PARTY_THRESHOLD - 1);
        let (_, effects) = tick_population(state, &[target]);
        assert!(
            !effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
            "war party should not form below threshold"
        );
    }

    #[test]
    fn war_party_requires_hostile_target() {
        // No targets: war party never forms even if adults >= threshold.
        let state = make_pop(WAR_PARTY_THRESHOLD + 5);
        let (_, effects) = tick_population(state, &[]);
        assert!(
            !effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. }))
        );
    }

    #[test]
    fn war_party_cooldown_prevents_immediate_repeat() {
        let target = (Uuid::new_v4(), 100.0f32, 100.0f32);
        let state = make_pop(WAR_PARTY_THRESHOLD + WAR_PARTY_SIZE); // enough for two
        let (state, effects) = tick_population(state, &[target]);
        assert!(effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })));
        // Next tick: cooldown active, no second war party.
        let (_, effects2) = tick_population(state, &[target]);
        assert!(!effects2.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })));
    }

    #[test]
    fn deterministic_same_input() {
        let state1 = make_pop(5);
        let state2 = make_pop(5);
        let (r1, e1) = tick_population(state1, &[]);
        let (r2, e2) = tick_population(state2, &[]);
        assert_eq!(r1.birth_ticks, r2.birth_ticks);
        assert_eq!(e1, e2);
    }
}
