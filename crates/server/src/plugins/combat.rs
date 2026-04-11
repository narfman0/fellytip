//! ECS combat bridge: thin layer between ECS state and pure combat rules.
//!
//! Each FixedUpdate tick:
//!   1. Snapshot ECS component data into `CombatantSnapshot`
//!   2. Call `InterruptStack::step()` with injected dice
//!   3. Apply returned `Vec<Effect>` back to ECS; emit story events for deaths

use bevy::prelude::*;
use fellytip_shared::{
    combat::{
        interrupt::{AttackContext, InterruptFrame, InterruptStack},
        types::{
            CharacterClass, CombatState, CombatantId, CombatantSnapshot, CombatantState,
            CoreStats, Effect,
        },
    },
    world::story::{GameEntityId, StoryEvent, StoryEventKind, WriteStoryEvent},
};
use smol_str::SmolStr;
use uuid::Uuid;

// ── Server-only combat components ─────────────────────────────────────────────

/// Health component (server-authoritative; replicated to clients separately).
#[derive(Component, Clone, Debug)]
pub struct Health {
    pub current: i32,
    pub max: i32,
}

/// Server-only combat participant tracking.
#[derive(Component)]
pub struct CombatParticipant {
    pub id: CombatantId,
    pub interrupt_stack: InterruptStack,
    pub armor: i32,
    pub strength: i32,
}

/// Marker: this entity has a pending attack against `target`.
#[derive(Component)]
pub struct PendingAttack {
    pub target: Entity,
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (initiate_attacks, resolve_interrupts).chain());
    }
}

/// Convert pending attacks into interrupt frames, then clear the marker.
fn initiate_attacks(
    mut attacker_query: Query<(Entity, &PendingAttack, &mut CombatParticipant)>,
    defender_query: Query<&CombatParticipant, Without<PendingAttack>>,
    mut commands: Commands,
) {
    for (entity, attack, mut participant) in attacker_query.iter_mut() {
        let attacker_id = participant.id.clone();
        if let Ok(defender) = defender_query.get(attack.target) {
            let frame = InterruptFrame::ResolvingAttack {
                ctx: AttackContext {
                    attacker: attacker_id,
                    defender: defender.id.clone(),
                    attack_roll: rand::random_range(1..=20),
                    dmg_roll: rand::random_range(1..=8),
                },
            };
            participant.interrupt_stack.push(frame);
        }
        commands.entity(entity).remove::<PendingAttack>();
    }
}

/// Step each active interrupt stack; apply effects back to ECS.
fn resolve_interrupts(
    mut participants: Query<(Entity, &mut CombatParticipant, &mut Health)>,
    mut story_writer: MessageWriter<WriteStoryEvent>,
    tick: Res<crate::plugins::world_sim::WorldSimTick>,
    mut commands: Commands,
) {
    // Build a lightweight CombatState snapshot for rule calls.
    let snapshots: Vec<(Entity, CombatantSnapshot, i32)> = participants
        .iter()
        .map(|(e, p, h)| {
            (
                e,
                CombatantSnapshot {
                    id: p.id.clone(),
                    faction: None,
                    class: CharacterClass::Warrior,
                    stats: CoreStats {
                        strength: p.strength,
                        ..CoreStats::default()
                    },
                    health_current: h.current,
                    health_max: h.max,
                    level: 1,
                    armor: p.armor,
                },
                h.current,
            )
        })
        .collect();

    let combat_state = CombatState {
        combatants: snapshots
            .iter()
            .map(|(_, snap, hp)| CombatantState {
                snapshot: snap.clone(),
                health: *hp,
                statuses: vec![],
            })
            .collect(),
        round: 0,
    };

    // Step each participant's stack, collecting effects.
    let mut all_effects: Vec<(CombatantId, Vec<Effect>)> = Vec::new();
    for (_, mut participant, _) in participants.iter_mut() {
        if participant.interrupt_stack.is_empty() {
            continue;
        }
        let mut rng_iter = std::iter::from_fn(|| Some(rand::random_range(1..=20)));
        let (effects, _done) = participant.interrupt_stack.step(&combat_state, &mut rng_iter);
        if !effects.is_empty() {
            all_effects.push((participant.id.clone(), effects));
        }
    }

    // Apply effects back to ECS health components.
    for (_, effects) in all_effects {
        for effect in effects {
            match effect {
                Effect::TakeDamage { target, amount } => {
                    for (_, participant, mut health) in participants.iter_mut() {
                        if participant.id == target {
                            health.current = (health.current - amount).max(0);
                            tracing::debug!(
                                target = ?target.0,
                                amount,
                                remaining = health.current,
                                "Damage applied"
                            );
                        }
                    }
                }
                Effect::HealDamage { target, amount } => {
                    for (_, participant, mut health) in participants.iter_mut() {
                        if participant.id == target {
                            health.current = (health.current + amount).min(health.max);
                        }
                    }
                }
                Effect::Die { target } => {
                    // Find the entity and despawn it; emit story event.
                    for (entity, participant, _) in participants.iter() {
                        if participant.id == target {
                            tracing::info!(target = ?target.0, "Combatant died");
                            story_writer.write(WriteStoryEvent(StoryEvent {
                                id: Uuid::new_v4(),
                                tick: tick.0,
                                world_day: (tick.0 / 86400) as u32,
                                kind: StoryEventKind::PlayerKilledNamed {
                                    victim: GameEntityId(target.0),
                                    killer: GameEntityId(Uuid::nil()),
                                },
                                participants: vec![GameEntityId(target.0)],
                                location: None,
                                lore_tags: vec![SmolStr::new("death")],
                            }));
                            commands.entity(entity).despawn();
                            break;
                        }
                    }
                }
                Effect::ApplyStatus { .. } => {
                    // Status application — ECS status components in Step 13.
                }
            }
        }
    }
}
