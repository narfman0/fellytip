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
    combat::{
        hit_die_for_class, hp_on_level_up, xp_to_next_level,
        interrupt::{AbilityContext, AttackContext, InterruptFrame, InterruptStack},
        types::{
            CharacterClass, CombatState, CombatantId, CombatantSnapshot, CombatantState,
            CoreStats, Effect,
        },
    },
    components::{Experience, Health, WorldPosition},
    inputs::ActionIntent,
    world::{
        faction::{kill_standing_delta, standing_tier, PlayerReputationMap},
        story::{GameEntityId, StoryEvent, StoryEventKind, WriteStoryEvent},
    },
};

// ── Class-appropriate NPC ability selection ───────────────────────────────────

/// Choose the ability_id an NPC should use based on its class and current HP %.
///
/// Issue #129 — class-appropriate NPC combat actions:
/// - Fighter/Warrior: StrongAttack (1) when HP > 50 %, DefensiveStance (9) when ≤ 50 %
/// - Rogue/Ranger/Monk: SneakAttack (2)
/// - Mage/Wizard/Sorcerer: ArcaneBlast (3)
/// - Barbarian: RageEntry (8)
/// - Cleric/Druid: HealAlly (7) — targets self (ECS bridge heals lowest-HP ally)
/// - Paladin: StrongAttack (1)
/// - Warlock/Bard: ArcaneBlast (3) — CHA-based blast flavoured as Eldritch Blast / Bardic Magic
fn npc_ability_id(class: &CharacterClass, hp_pct: f32) -> u8 {
    match class {
        CharacterClass::Warrior | CharacterClass::Fighter | CharacterClass::Paladin => {
            if hp_pct > 0.5 { 1 } else { 9 }
        }
        CharacterClass::Rogue | CharacterClass::Ranger | CharacterClass::Monk => 2,
        CharacterClass::Mage | CharacterClass::Wizard | CharacterClass::Sorcerer
        | CharacterClass::Warlock | CharacterClass::Bard => 3,
        CharacterClass::Barbarian => 8,
        CharacterClass::Cleric | CharacterClass::Druid => 7,
    }
}

use crate::plugins::ai::{FactionMember, FactionNpcRank, FactionRegistry};
use smol_str::SmolStr;
use uuid::Uuid;

// ── Local player input buffer ─────────────────────────────────────────────────

/// Actions queued by the client input system for the current frame.
///
/// The client pushes to this resource from `send_player_input` in Update;
/// `process_player_input` drains it in FixedUpdate.
///
/// MULTIPLAYER: replace with MessageReceiver<PlayerInput> on ClientOf entities.
#[derive(Resource, Default)]
pub struct LocalPlayerInput {
    pub actions: Vec<(Option<ActionIntent>, Option<uuid::Uuid>)>,
}

/// Server-only combat participant tracking.
#[derive(Component)]
pub struct CombatParticipant {
    pub id: CombatantId,
    pub interrupt_stack: InterruptStack,
    pub class: CharacterClass,
    pub level: u32,
    /// Armour Class — threshold an attack roll must meet or beat to hit.
    /// See `docs/dnd5e-srd-reference.md`.
    pub armor_class: i32,
    pub strength: i32,
    pub dexterity: i32,
    pub constitution: i32,
    pub intelligence: i32,
    pub wisdom: i32,
    pub charisma: i32,
}

/// XP granted to the killer when this entity dies. Server-only (NPCs/bosses).
#[derive(Component)]
pub struct ExperienceReward(pub u32);

/// Stores the most-recently-received movement direction for a player entity.
///
/// No longer used to drive position (the client sends its computed position
/// directly).  Kept for potential future use (e.g. AI context, aggression
/// range prediction).
#[derive(Component, Default)]
pub struct LastPlayerInput {
    pub move_dir: [f32; 2],
}

/// Tracks how long the client's sent position has been outside a walkable tile.
///
/// Resets on each valid position.  After 10 s continuously in non-walkable
/// terrain the server snaps the player back to the last known valid position.
/// This is the only server-side position enforcement in the client-authoritative
/// movement model.
#[derive(Component, Default)]
pub struct PositionSanityTimer {
    pub excess_secs:  f32,
    pub last_valid_x: f32,
    pub last_valid_y: f32,
    pub last_valid_z: f32,
}

/// Marker: this entity has a pending attack against `target`.
#[derive(Component)]
pub struct PendingAttack {
    pub target: Entity,
}

/// Marker: this entity has a pending ability use against `target`.
#[derive(Component)]
pub struct PendingAbility {
    pub target: Entity,
    pub ability_id: u8,
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalPlayerInput>();
        app.add_systems(
            FixedUpdate,
            check_faction_aggression
                .before(process_player_input),
        );
        app.add_systems(
            FixedUpdate,
            (process_player_input, initiate_attacks, initiate_abilities, resolve_interrupts).chain(),
        );
    }
}

// ── Faction aggression check ──────────────────────────────────────────────────

type NpcAggroQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static FactionMember, &'static WorldPosition, &'static CombatParticipant, &'static Health),
    (With<ExperienceReward>, Without<PendingAttack>, Without<PendingAbility>),
>;

/// Check whether any faction NPC should initiate combat with a nearby player.
///
/// Triggers when:
/// 1. The NPC's faction has `is_aggressive = true`, OR
/// 2. The player's standing with that faction is Hostile or Hated.
///
/// Range: 10 tiles (squared distance ≤ 100).  Runs at FixedUpdate (62.5 Hz).
///
/// Issue #129: NPCs queue a class-appropriate ability instead of a plain attack.
fn check_faction_aggression(
    npc_query: NpcAggroQuery,
    player_query: Query<
        (Entity, &WorldPosition, &GameEntityId),
        Without<ExperienceReward>,
    >,
    reputation: Res<PlayerReputationMap>,
    registry: Res<FactionRegistry>,
    mut commands: Commands,
) {
    const AGGRO_RANGE_SQ: f32 = 100.0; // 10 tiles²
    for (npc_entity, fm, npc_pos, cp, health) in npc_query.iter() {
        let Some(faction) = registry.factions.iter().find(|f| f.id == fm.0) else {
            continue;
        };
        for (player_entity, player_pos, gid) in player_query.iter() {
            let dx = npc_pos.x - player_pos.x;
            let dy = npc_pos.y - player_pos.y;
            if dx * dx + dy * dy > AGGRO_RANGE_SQ {
                continue;
            }
            let tier = standing_tier(reputation.score(gid.0, &fm.0));
            if faction.is_aggressive || tier.is_aggressive() {
                // Use class-appropriate ability (#129).
                let hp_pct = if health.max > 0 {
                    health.current as f32 / health.max as f32
                } else {
                    0.0
                };
                let ability_id = npc_ability_id(&cp.class, hp_pct);
                commands.entity(npc_entity).insert(PendingAbility {
                    target: player_entity,
                    ability_id,
                });
                break;
            }
        }
    }
}

// ── Input processing ──────────────────────────────────────────────────────────

/// Drain `LocalPlayerInput` actions and queue combat markers on the player entity.
///
/// Movement is handled by `sync_pred_to_world` in the client (Update); this
/// system only processes the action intents accumulated since the last tick.
///
/// MULTIPLAYER: restore MessageReceiver<PlayerInput> iteration over ClientOf
/// entities and re-add the position-acceptance + sanity-timer logic.
fn process_player_input(
    mut local_input: ResMut<LocalPlayerInput>,
    player_q: Query<Entity, Without<ExperienceReward>>,
    enemies: Query<(Entity, &CombatParticipant), With<ExperienceReward>>,
    mut commands: Commands,
) {
    let Ok(player_entity) = player_q.single() else { return };
    let pending_actions: Vec<(Option<ActionIntent>, Option<uuid::Uuid>)> =
        local_input.actions.drain(..).collect();

    for (action, target_uuid) in pending_actions {
        match action {
            Some(ActionIntent::BasicAttack) => {
                let target = if let Some(uuid) = target_uuid {
                    enemies.iter().find(|(_, p)| p.id.0 == uuid).map(|(e, _)| e)
                } else {
                    enemies.iter().next().map(|(e, _)| e)
                };
                if let Some(target_entity) = target {
                    commands
                        .entity(player_entity)
                        .insert(PendingAttack { target: target_entity });
                    tracing::debug!(target = ?target_entity, "BasicAttack queued");
                }
            }
            Some(ActionIntent::UseAbility(ability_id)) => {
                let target = if let Some(uuid) = target_uuid {
                    enemies.iter().find(|(_, p)| p.id.0 == uuid).map(|(e, _)| e)
                } else {
                    enemies.iter().next().map(|(e, _)| e)
                };
                if let Some(target_entity) = target {
                    commands
                        .entity(player_entity)
                        .insert(PendingAbility { target: target_entity, ability_id });
                    tracing::debug!(target = ?target_entity, ability_id, "UseAbility queued");
                }
            }
            Some(ActionIntent::Interact) | Some(ActionIntent::Dodge) | None => {}
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

// ── Ability initiation ────────────────────────────────────────────────────────

/// Convert pending abilities into interrupt frames, then clear the marker.
fn initiate_abilities(
    mut caster_query: Query<(Entity, &PendingAbility, &mut CombatParticipant)>,
    defender_query: Query<&CombatParticipant, Without<PendingAbility>>,
    mut commands: Commands,
) {
    for (entity, pending, mut participant) in caster_query.iter_mut() {
        let caster_id = participant.id.clone();
        if let Ok(defender) = defender_query.get(pending.target) {
            let frame = InterruptFrame::ResolvingAbility {
                ctx: AbilityContext {
                    caster: caster_id,
                    ability_id: pending.ability_id,
                    targets: vec![defender.id.clone()],
                    rolls: vec![
                        rand::random_range(1..=20), // attack d20
                        rand::random_range(1..=8),  // dmg d8 #1
                        rand::random_range(1..=8),  // dmg d8 #2
                    ],
                },
            };
            participant.interrupt_stack.push(frame);
        }
        commands.entity(entity).remove::<PendingAbility>();
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
        Option<&'static GameEntityId>,
        Option<&'static FactionMember>,
        Option<&'static FactionNpcRank>,
    ),
>;

/// Step each active interrupt stack; apply effects; award XP on kills.
fn resolve_interrupts(
    mut participants: ParticipantQuery,
    mut story_writer: MessageWriter<WriteStoryEvent>,
    mut reputation: ResMut<PlayerReputationMap>,
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
        .filter_map(|(e, _, _, _, reward, ..)| reward.map(|r| (e, r.0)))
        .collect();

    // Map Entity → player GameEntityId (only entities that have one — i.e. players).
    let entity_to_game_id: HashMap<Entity, uuid::Uuid> = participants
        .iter()
        .filter_map(|(e, _, _, _, _, gid, ..)| gid.map(|g| (e, g.0)))
        .collect();

    // ── Phase 2: build CombatState snapshot for rule calls ───────────────────
    let combat_state = CombatState {
        combatants: participants
            .iter()
            .map(|(_, p, h, _, _, _, fm, _)| CombatantState {
                snapshot: CombatantSnapshot {
                    id: p.id.clone(),
                    faction: fm.map(|m| m.0.clone()),
                    class: p.class.clone(),
                    stats: CoreStats {
                        strength: p.strength,
                        dexterity: p.dexterity,
                        constitution: p.constitution,
                        intellect: p.intelligence,
                        wisdom: p.wisdom,
                        charisma: p.charisma,
                    },
                    health_current: h.current,
                    health_max: h.max,
                    level: p.level,
                    armor_class: p.armor_class,
                },
                health: h.current,
                statuses: vec![],
            })
            .collect(),
        round: 0,
    };

    // ── Phase 3: step each non-empty interrupt stack ─────────────────────────
    let mut all_effects: Vec<(CombatantId, Entity, Vec<Effect>)> = Vec::new();
    for (entity, mut participant, _, _, _, _, _, _) in participants.iter_mut() {
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
    // (killer_uuid, target_entity) pairs for reputation penalty application.
    let mut reputation_kills: Vec<(uuid::Uuid, Entity)> = Vec::new();

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
                        // Resolve killer UUID from attacker's GameEntityId (players have one).
                        let killer_uuid = entity_to_game_id
                            .get(attacker_entity)
                            .copied()
                            .unwrap_or(Uuid::nil());
                        story_events.push(StoryEvent {
                            id: Uuid::new_v4(),
                            tick: tick.0,
                            world_day: (tick.0 / 86400) as u32,
                            kind: StoryEventKind::PlayerKilledNamed {
                                victim: GameEntityId(target.0),
                                killer: GameEntityId(killer_uuid),
                            },
                            participants: vec![GameEntityId(target.0)],
                            location: None,
                            lore_tags: vec![SmolStr::new("death")],
                        });
                        if killer_uuid != Uuid::nil() {
                            reputation_kills.push((killer_uuid, target_entity));
                        }
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

    // ── Phase 4b: apply reputation deltas for kills ──────────────────────────
    for (killer_uuid, target_entity) in &reputation_kills {
        if let Ok((_, _, _, _, _, _, Some(fm), rank)) = participants.get(*target_entity) {
            let rank = rank.map(|r| r.0).unwrap_or(fellytip_shared::world::faction::NpcRank::Grunt);
            reputation.apply_delta(*killer_uuid, &fm.0, kill_standing_delta(rank));
            let new_score = reputation.score(*killer_uuid, &fm.0);
            tracing::debug!(
                killer = %killer_uuid,
                faction = %fm.0.0,
                delta = kill_standing_delta(rank),
                score = new_score,
                tier = ?standing_tier(new_score),
                "Kill reputation applied"
            );
        }
    }

    // ── Phase 5: award XP and apply level-up ────────────────────────────────
    for (attacker_entity, xp) in xp_awards {
        if let Ok((_, mut participant, mut health, Some(mut exp), _, _, _, _)) =
            participants.get_mut(attacker_entity)
        {
            exp.xp += xp;
            tracing::info!(xp, total = exp.xp, level = exp.level, "XP awarded");
            while exp.xp >= exp.xp_to_next {
                exp.xp -= exp.xp_to_next;
                exp.level += 1;
                exp.xp_to_next = xp_to_next_level(exp.level);
                participant.level = exp.level;
                // HP gain on level-up: roll hit die + CON mod, min 1 (SRD §Level Advancement).
                let hit_die = hit_die_for_class(&participant.class);
                let roll = rand::random_range(1..=hit_die);
                let con_mod = (participant.constitution - 10) / 2;
                let gain = hp_on_level_up(&participant.class, con_mod, &mut std::iter::once(roll));
                health.max += gain;
                health.current = health.max; // full heal on level-up
                tracing::info!(level = exp.level, hp_gain = gain, "Level up!");
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

