//! Stack-based interrupt chain state machine.
//!
//! `InterruptStack::step()` drives one frame of resolution per call.
//! Every `InterruptFrame` variant must be handled explicitly — no `_` wildcard.

use crate::combat::rules::{resolve_ability, resolve_attack_roll, resolve_damage, resolve_spell};
use crate::combat::types::{CombatState, CombatantId, Effect};

// ── Context types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AttackContext {
    pub attacker: CombatantId,
    pub defender: CombatantId,
    /// Injected attack roll value [1..=20].
    pub attack_roll: i32,
    /// Injected damage roll value.
    pub dmg_roll: i32,
}

#[derive(Clone, Debug)]
pub struct DamageContext {
    pub target: CombatantId,
    pub amount: i32,
    pub source: CombatantId,
}

#[derive(Clone, Debug)]
pub struct AbilityContext {
    pub caster: CombatantId,
    pub ability_id: u8,
    pub targets: Vec<CombatantId>,
    /// Pre-rolled dice injected by the ECS bridge: [attack_d20, dmg_d8_1, dmg_d8_2, …]
    pub rolls: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct MovementContext {
    pub mover: CombatantId,
    pub destination: (i32, i32),
}

// ── Frame variants ────────────────────────────────────────────────────────────

/// One entry on the interrupt stack.
///
/// **INVARIANT**: Every variant must appear in every `match` — no `_` wildcard.
/// This is enforced at compile-time by clippy's `wildcard_enum_match_arm` (see
/// CLAUDE.md).
#[derive(Clone, Debug)]
pub enum InterruptFrame {
    ResolvingAttack   { ctx: AttackContext },
    ResolvingDamage   { ctx: DamageContext },
    ResolvingAbility  { ctx: AbilityContext },
    ResolvingMovement { ctx: MovementContext },
    /// A spellcast being resolved. `rolls` holds injected dice for damage/save.
    /// Layout: `[dmg_die_1, dmg_die_2, …, save_d20]` (save d20 last, if any).
    CastingSpell {
        caster: CombatantId,
        spell_name: &'static str,
        slot_level: u8,
        target: CombatantId,
        /// Pre-rolled dice injected by the ECS bridge.
        rolls: Vec<i32>,
    },
}

// ── Stack ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct InterruptStack(pub Vec<InterruptFrame>);

impl InterruptStack {
    pub fn push(&mut self, frame: InterruptFrame) {
        self.0.push(frame);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Process the top frame of the stack.
    ///
    /// `rng` — iterator yielding injected dice values; never rolled internally.
    ///
    /// Returns `(effects_to_apply, is_done)`.
    /// `is_done` is true when the stack is empty after this step.
    pub fn step(
        &mut self,
        state: &CombatState,
        rng: &mut impl Iterator<Item = i32>,
    ) -> (Vec<Effect>, bool) {
        let Some(frame) = self.0.pop() else {
            return (vec![], true);
        };

        let effects = match frame {
            InterruptFrame::ResolvingAttack { ctx } => {
                let Some(attacker) = state.get(&ctx.attacker).map(|c| c.snapshot.clone()) else {
                    return (vec![], self.0.is_empty());
                };
                let Some(defender) = state.get(&ctx.defender).map(|c| c.snapshot.clone()) else {
                    return (vec![], self.0.is_empty());
                };
                let roll_result = resolve_attack_roll(&attacker, &defender, ctx.attack_roll);
                resolve_damage(&roll_result, &attacker, &defender, ctx.dmg_roll)
            }

            InterruptFrame::ResolvingDamage { ctx } => {
                vec![Effect::TakeDamage {
                    target: ctx.target,
                    amount: ctx.amount,
                }]
            }

            InterruptFrame::ResolvingAbility { ctx } => {
                let _ = rng; // dice are pre-rolled into ctx.rolls
                let Some(caster) = state.get(&ctx.caster).map(|c| c.snapshot.clone()) else {
                    return (vec![], self.0.is_empty());
                };
                let Some(target_id) = ctx.targets.first().cloned() else {
                    return (vec![], self.0.is_empty());
                };
                let Some(target) = state.get(&target_id).map(|c| c.snapshot.clone()) else {
                    return (vec![], self.0.is_empty());
                };
                resolve_ability(ctx.ability_id, &caster, &target, &ctx.rolls)
            }

            InterruptFrame::ResolvingMovement { ctx } => {
                // Movement resolution placeholder — collision/range checks in Step 8+.
                let _ = ctx;
                vec![]
            }

            InterruptFrame::CastingSpell { caster, spell_name, slot_level, target, rolls } => {
                let _ = slot_level; // slot already expended by the ECS bridge before queuing
                let Some(caster_state) = state.get(&caster).map(|c| c.snapshot.clone()) else {
                    return (vec![], self.0.is_empty());
                };
                let Some(target_state) = state.get(&target).map(|c| c.snapshot.clone()) else {
                    return (vec![], self.0.is_empty());
                };
                resolve_spell(spell_name, &caster_state, &target_state, &rolls)
            }
        };

        (effects, self.0.is_empty())
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::types::{
        CharacterClass, CombatState, CombatantSnapshot, CombatantState, CoreStats,
    };
    use uuid::Uuid;

    fn make_state() -> (CombatState, CombatantId, CombatantId) {
        let aid = CombatantId(Uuid::new_v4());
        let did = CombatantId(Uuid::new_v4());
        let attacker = CombatantSnapshot {
            id: aid.clone(),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: 20,
            health_max: 20,
            level: 1,
            armor_class: 10,
        };
        let defender = CombatantSnapshot {
            id: did.clone(),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: 10,
            health_max: 10,
            level: 1,
            armor_class: 10,
        };
        let state = CombatState {
            combatants: vec![
                CombatantState::new(attacker),
                CombatantState::new(defender),
            ],
            round: 0,
        };
        (state, aid, did)
    }

    #[test]
    fn attack_frame_produces_damage_on_hit() {
        let (state, aid, did) = make_state();
        let mut stack = InterruptStack::default();
        stack.push(InterruptFrame::ResolvingAttack {
            ctx: AttackContext {
                attacker: aid,
                defender: did,
                attack_roll: 15, // guaranteed hit
                dmg_roll: 5,
            },
        });

        let mut rng = std::iter::empty::<i32>();
        let (effects, done) = stack.step(&state, &mut rng);
        assert!(done);
        assert!(effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. })));
    }

    #[test]
    fn empty_stack_is_done() {
        let (state, _, _) = make_state();
        let mut stack = InterruptStack::default();
        let mut rng = std::iter::empty::<i32>();
        let (effects, done) = stack.step(&state, &mut rng);
        assert!(done);
        assert!(effects.is_empty());
    }
}
