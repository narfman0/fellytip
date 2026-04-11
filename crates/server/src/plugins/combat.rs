//! ECS combat bridge: thin layer between ECS state and pure combat rules.
//!
//! Each FixedUpdate tick:
//!   1. Read PlayerInput messages from clients → PendingAttack markers
//!   2. Snapshot ECS component data into `CombatantSnapshot`
//!   3. Call `InterruptStack::step()` with injected dice
//!   4. Apply returned `Vec<Effect>` back to ECS; award XP; emit story events

use std::collections::HashMap;

use bevy::{ecs::message::MessageWriter, prelude::*};
use fellytip_shared::{
    TICK_HZ,
    combat::{
        interrupt::{AttackContext, InterruptFrame, InterruptStack},
        types::{
            CharacterClass, CombatState, CombatantId, CombatantSnapshot, CombatantState,
            CoreStats, Effect,
        },
    },
    components::{Experience, Health, WorldPosition},
    inputs::{ActionIntent, PlayerInput},
    world::story::{GameEntityId, StoryEvent, StoryEventKind, WriteStoryEvent},
};
use lightyear::prelude::{server::ClientOf, MessageReceiver};
use smol_str::SmolStr;
use uuid::Uuid;

// ── Server-only combat components ─────────────────────────────────────────────

/// Links a `ClientOf` entity to its spawned player entity.
#[derive(Component)]
pub struct PlayerEntity(pub Entity);

/// Server-only combat participant tracking.
#[derive(Component)]
pub struct CombatParticipant {
    pub id: CombatantId,
    pub interrupt_stack: InterruptStack,
    pub armor: i32,
    pub strength: i32,
}

/// XP granted to the killer when this entity dies. Server-only (NPCs/bosses).
#[derive(Component)]
pub struct ExperienceReward(pub u32);

/// Marker: this entity has a pending attack against `target`.
#[derive(Component)]
pub struct PendingAttack {
    pub target: Entity,
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (process_player_input, initiate_attacks, resolve_interrupts).chain(),
        );
    }
}

// ── Input processing ──────────────────────────────────────────────────────────

/// Read `PlayerInput` messages arriving from clients, apply movement, and
/// queue `PendingAttack` markers on player entities.
fn process_player_input(
    mut clients: Query<(&mut MessageReceiver<PlayerInput>, &PlayerEntity), With<ClientOf>>,
    mut positions: Query<&mut WorldPosition>,
    enemies: Query<(Entity, &CombatParticipant), With<ExperienceReward>>,
    mut commands: Commands,
) {
    let dt = (1.0 / TICK_HZ) as f32;
    const SPEED: f32 = 5.0;

    for (mut receiver, player_entity) in clients.iter_mut() {
        for input in receiver.receive() {
            // Apply movement
            let [dx, dy] = input.move_dir;
            if dx != 0.0 || dy != 0.0 {
                if let Ok(mut pos) = positions.get_mut(player_entity.0) {
                    pos.x += dx * SPEED * dt;
                    pos.y += dy * SPEED * dt;
                }
            }

            // Handle combat action
            if let Some(ActionIntent::BasicAttack) = input.action {
                let target = if let Some(uuid) = input.target {
                    enemies.iter().find(|(_, p)| p.id.0 == uuid).map(|(e, _)| e)
                } else {
                    enemies.iter().next().map(|(e, _)| e)
                };
                if let Some(target_entity) = target {
                    commands
                        .entity(player_entity.0)
                        .insert(PendingAttack { target: target_entity });
                    tracing::debug!(
                        player = ?player_entity.0,
                        target = ?target_entity,
                        "BasicAttack queued"
                    );
                }
            }
        }
    }
}

// ── Attack initiation ─────────────────────────────────────────────────────────

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

// ── Interrupt resolution ──────────────────────────────────────────────────────

type ParticipantQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut CombatParticipant,
        &'static mut Health,
        Option<&'static mut Experience>,
        Option<&'static ExperienceReward>,
    ),
>;

/// Step each active interrupt stack; apply effects; award XP on kills.
fn resolve_interrupts(
    mut participants: ParticipantQuery,
    mut story_writer: MessageWriter<WriteStoryEvent>,
    tick: Res<crate::plugins::world_sim::WorldSimTick>,
    mut commands: Commands,
) {
    // ── Phase 1: build lookup maps (immutable passes) ────────────────────────
    let id_to_entity: HashMap<CombatantId, Entity> = participants
        .iter()
        .map(|(e, p, ..)| (p.id.clone(), e))
        .collect();

    if id_to_entity.is_empty() {
        return;
    }

    let xp_rewards: HashMap<Entity, u32> = participants
        .iter()
        .filter_map(|(e, _, _, _, reward)| reward.map(|r| (e, r.0)))
        .collect();

    // ── Phase 2: build CombatState snapshot for rule calls ───────────────────
    let combat_state = CombatState {
        combatants: participants
            .iter()
            .map(|(_, p, h, ..)| CombatantState {
                snapshot: CombatantSnapshot {
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
                health: h.current,
                statuses: vec![],
            })
            .collect(),
        round: 0,
    };

    // ── Phase 3: step each non-empty interrupt stack ─────────────────────────
    let mut all_effects: Vec<(CombatantId, Entity, Vec<Effect>)> = Vec::new();
    for (entity, mut participant, ..) in participants.iter_mut() {
        if participant.interrupt_stack.is_empty() {
            continue;
        }
        let mut rng_iter = std::iter::from_fn(|| Some(rand::random_range(1..=20)));
        let (effects, _done) = participant.interrupt_stack.step(&combat_state, &mut rng_iter);
        if !effects.is_empty() {
            all_effects.push((participant.id.clone(), entity, effects));
        }
    }
    // All iter_mut() borrows are dropped here.

    // ── Phase 4: apply effects via get_mut() ────────────────────────────────
    let mut xp_awards: Vec<(Entity, u32)> = Vec::new();
    let mut despawn_list: Vec<Entity> = Vec::new();
    let mut story_events: Vec<StoryEvent> = Vec::new();

    for (_attacker_id, attacker_entity, effects) in &all_effects {
        for effect in effects {
            match effect {
                Effect::TakeDamage { target, amount } => {
                    if let Some(&target_entity) = id_to_entity.get(target) {
                        if let Ok((_, _, mut health, ..)) = participants.get_mut(target_entity) {
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
                    if let Some(&target_entity) = id_to_entity.get(target) {
                        if let Ok((_, _, mut health, ..)) = participants.get_mut(target_entity) {
                            health.current = (health.current + amount).min(health.max);
                        }
                    }
                }
                Effect::Die { target } => {
                    if let Some(&target_entity) = id_to_entity.get(target) {
                        if let Some(&xp) = xp_rewards.get(&target_entity) {
                            xp_awards.push((*attacker_entity, xp));
                        }
                        story_events.push(StoryEvent {
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
                        });
                        tracing::info!(target = ?target.0, "Combatant died");
                        despawn_list.push(target_entity);
                    }
                }
                Effect::ApplyStatus { .. } => {
                    // Status application — ECS status components in Step 13.
                }
            }
        }
    }

    // ── Phase 5: award XP ────────────────────────────────────────────────────
    for (attacker_entity, xp) in xp_awards {
        if let Ok((_, _, _, Some(mut exp), _)) = participants.get_mut(attacker_entity) {
            exp.xp += xp;
            tracing::info!(xp, total = exp.xp, level = exp.level, "XP awarded");
            while exp.xp >= exp.xp_to_next {
                exp.xp -= exp.xp_to_next;
                exp.level += 1;
                exp.xp_to_next = xp_to_next_level(exp.level);
                tracing::info!(level = exp.level, "Level up!");
            }
        }
    }

    // ── Phase 6: emit story events ───────────────────────────────────────────
    for event in story_events {
        story_writer.write(WriteStoryEvent(event));
    }

    // ── Phase 7: despawn dead entities ───────────────────────────────────────
    for entity in despawn_list {
        commands.entity(entity).despawn();
    }
}

fn xp_to_next_level(level: u32) -> u32 {
    100 * level
}
