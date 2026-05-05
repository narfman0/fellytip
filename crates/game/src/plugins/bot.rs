//! Bot / "fake player" plugin — spawns full-fidelity player entities driven
//! programmatically for testing combat, AI aggro, and load.
//!
//! Server-side, a bot is identical to a real player: same component bundle
//! (`CombatParticipant`, `Health`, `Experience`, `ActionBudget`, `SpellSlots`,
//! …) so all combat resolution, faction aggression, and persistence-adjacent
//! systems treat it the same. The only addition is a `BotController` driver
//! component used in lieu of `LocalPlayerInput` for action choice and motion.
//!
//! # BRP methods
//!
//! | Method               | Effect                                                |
//! |----------------------|-------------------------------------------------------|
//! | `dm/spawn_bot`       | Spawn a bot player at the given position              |
//! | `dm/despawn_bot`     | Despawn a bot by entity id                            |
//! | `dm/list_bots`       | Return all live bots                                  |
//! | `dm/set_bot_action`  | Queue a one-shot `ActionIntent` on a bot              |
//!
//! Bots also expose a `BotPolicy` (`Idle` / `Wander` / `Aggressive`) that
//! drives autonomous behaviour each `FixedUpdate` tick.

use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use uuid::Uuid;

use fellytip_shared::{
    WORLD_SEED,
    combat::{
        SpellSlots, Spellbook,
        interrupt::InterruptStack,
        types::{CharacterClass, CombatantId},
    },
    components::{
        AbilityModifiers, AbilityScores, ActionBudget, Experience, Health, HitDice,
        PlayerStandings, SavingThrowProficiencies, WorldMeta, WorldPosition,
    },
    inputs::ActionIntent,
    world::{
        map::{MAP_HEIGHT, MAP_WIDTH},
        story::GameEntityId,
        zone::{OVERWORLD_ZONE, ZoneMembership},
    },
};

use crate::MapGenConfig;
use crate::plugins::combat::{
    ActionCooldowns, CombatParticipant, ExperienceReward, LastPlayerInput, PendingAbility,
    PendingAttack, PositionSanityTimer,
};

// ── Components ────────────────────────────────────────────────────────────────

/// Behaviour policy for a fake player.
#[derive(Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BotPolicy {
    /// Stand still, queue no actions. Useful as a passive target dummy.
    #[default]
    Idle,
    /// Random walk only; no combat actions.
    Wander,
    /// Random walk plus periodic basic-attack on the nearest enemy NPC.
    Aggressive,
}

impl BotPolicy {
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "idle" => Some(Self::Idle),
            "wander" => Some(Self::Wander),
            "aggressive" | "aggro" => Some(Self::Aggressive),
            _ => None,
        }
    }
}

/// Driver state for a bot. Acts as the marker that distinguishes bot players
/// from the real local player throughout the codebase: spawn / input / tagging
/// systems exclude bots via `Without<BotController>`.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct BotController {
    pub policy: BotPolicy,
    /// Speed in tiles/sec for random walk (0.0 = stationary).
    pub move_speed: f32,
    /// Current normalized 2D move direction.
    pub move_dir: [f32; 2],
    /// Countdown before re-rolling `move_dir`.
    pub move_change_timer: f32,
    /// How often a new `move_dir` is rolled.
    pub move_change_secs: f32,
    /// Min seconds between two attempted basic attacks (Aggressive policy).
    pub attack_period_secs: f32,
    /// Countdown before the next attack attempt is permitted.
    pub attack_timer: f32,
    /// One-shot programmatic action — drained on the next tick.
    /// `ActionIntent` is not `Reflect` (lives in the protocol crate), so the
    /// field is opaque to the editor inspector.
    #[reflect(ignore)]
    pub pending_action: Option<(ActionIntent, Option<Uuid>)>,
}

impl Default for BotController {
    fn default() -> Self {
        Self {
            policy: BotPolicy::Aggressive,
            move_speed: 2.0,
            move_dir: [0.0, 0.0],
            move_change_timer: 0.0,
            move_change_secs: 2.5,
            attack_period_secs: 2.0,
            attack_timer: 1.0,
            pending_action: None,
        }
    }
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<BotController>();
        // Run bot driving before the player input pipeline so that any
        // PendingAttack markers we queue this tick are picked up by
        // `initiate_attacks` in the same frame.
        app.add_systems(
            FixedUpdate,
            drive_bots.before(crate::plugins::combat::process_player_input),
        );
    }
}

// ── Spawning ──────────────────────────────────────────────────────────────────

/// Stats derived from a chosen class — mirror of `class_stats` in `lib.rs`.
fn bot_class_stats(
    class: &CharacterClass,
) -> (
    i32,
    i32,
    i32,
    i32,
    i32,
    i32,
    i32,
    AbilityScores,
    SavingThrowProficiencies,
) {
    use fellytip_shared::world::faction::NpcRank;
    match class {
        CharacterClass::Warrior => {
            let ab = AbilityScores::warrior();
            let saves = SavingThrowProficiencies::warrior();
            (
                10 + AbilityScores::modifier(ab.constitution) as i32,
                ab.strength as i32,
                ab.dexterity as i32,
                ab.constitution as i32,
                ab.intelligence as i32,
                ab.wisdom as i32,
                ab.charisma as i32,
                ab,
                saves,
            )
        }
        CharacterClass::Rogue => {
            let ab = AbilityScores::rogue();
            let saves = SavingThrowProficiencies::rogue();
            (
                8 + AbilityScores::modifier(ab.constitution) as i32,
                ab.strength as i32,
                ab.dexterity as i32,
                ab.constitution as i32,
                ab.intelligence as i32,
                ab.wisdom as i32,
                ab.charisma as i32,
                ab,
                saves,
            )
        }
        CharacterClass::Mage => {
            let ab = AbilityScores::mage();
            let saves = SavingThrowProficiencies::mage();
            (
                6 + AbilityScores::modifier(ab.constitution) as i32,
                ab.strength as i32,
                ab.dexterity as i32,
                ab.constitution as i32,
                ab.intelligence as i32,
                ab.wisdom as i32,
                ab.charisma as i32,
                ab,
                saves,
            )
        }
        other => {
            use fellytip_shared::combat::types::hit_die_for_class;
            let ab = AbilityScores::for_class(other, NpcRank::Grunt);
            let saves = SavingThrowProficiencies::for_class(other);
            let hp_max =
                hit_die_for_class(other) + AbilityScores::modifier(ab.constitution) as i32;
            (
                hp_max,
                ab.strength as i32,
                ab.dexterity as i32,
                ab.constitution as i32,
                ab.intelligence as i32,
                ab.wisdom as i32,
                ab.charisma as i32,
                ab,
                saves,
            )
        }
    }
}

/// Pure spawn helper — used by both the BRP method and any future code paths
/// that want to introduce a bot directly. Returns the new entity id.
pub fn spawn_bot(world: &mut World, class: CharacterClass, pos: WorldPosition, controller: BotController) -> Entity {
    let bot_uuid = Uuid::new_v4();

    let world_meta = world
        .get_resource::<MapGenConfig>()
        .map(|cfg| WorldMeta {
            seed: cfg.seed,
            width: cfg.width as u32,
            height: cfg.height as u32,
        })
        .unwrap_or(WorldMeta {
            seed: WORLD_SEED,
            width: MAP_WIDTH as u32,
            height: MAP_HEIGHT as u32,
        });

    let (hp_max, str_v, dex_v, con_v, int_v, wis_v, cha_v, ability_scores, saves) =
        bot_class_stats(&class);

    let level: u32 = 1;
    let spell_slots = SpellSlots::for_class(&class, level as u8);
    let spellbook = Spellbook::for_class(&class);
    let hit_dice = HitDice::for_class_level(&class, level);
    let ability_modifiers = AbilityModifiers::from_scores(&ability_scores);

    world
        .spawn((
            WorldPosition { x: pos.x, y: pos.y, z: pos.z },
            ZoneMembership(OVERWORLD_ZONE),
            Health { current: hp_max, max: hp_max },
            CombatParticipant {
                id: CombatantId(bot_uuid),
                interrupt_stack: InterruptStack::default(),
                class,
                level,
                armor_class: 13,
                strength: str_v,
                dexterity: dex_v,
                constitution: con_v,
                intelligence: int_v,
                wisdom: wis_v,
                charisma: cha_v,
            },
            ability_modifiers,
            hit_dice,
            ability_scores,
            saves,
            GameEntityId(bot_uuid),
            Experience { xp: 0, level, xp_to_next: 300 },
            PlayerStandings::default(),
            LastPlayerInput::default(),
            PositionSanityTimer {
                last_valid_x: pos.x,
                last_valid_y: pos.y,
                last_valid_z: pos.z,
                ..default()
            },
            (world_meta, spell_slots, spellbook),
            (ActionBudget::default(), ActionCooldowns::default(), controller),
        ))
        .id()
}

// ── Driver system ─────────────────────────────────────────────────────────────

/// Drive bot players each `FixedUpdate` tick — random walk, attack-nearest,
/// and one-shot `pending_action` injection. Mirrors what `process_player_input`
/// does for the real local player but reads from `BotController` instead of
/// `LocalPlayerInput`.
#[allow(clippy::type_complexity)]
fn drive_bots(
    time: Res<Time>,
    mut bots: Query<(
        Entity,
        &mut BotController,
        &mut WorldPosition,
        &mut ActionBudget,
        &mut ActionCooldowns,
        &CombatParticipant,
    )>,
    enemies: Query<(Entity, &WorldPosition, &CombatParticipant), (With<ExperienceReward>, Without<BotController>)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (bot_entity, mut bot, mut pos, mut budget, mut cds, _participant) in bots.iter_mut() {
        // ── 1. Movement (random walk) ────────────────────────────────────────
        if matches!(bot.policy, BotPolicy::Wander | BotPolicy::Aggressive) {
            bot.move_change_timer -= dt;
            if bot.move_change_timer <= 0.0 {
                let angle = rand::random_range(0.0_f32..std::f32::consts::TAU);
                bot.move_dir = [angle.cos(), angle.sin()];
                bot.move_change_timer = bot.move_change_secs.max(0.1);
            }
            let step = bot.move_speed * dt;
            pos.x += bot.move_dir[0] * step;
            pos.y += bot.move_dir[1] * step;
        }

        // ── 2. One-shot programmatic action (always honoured) ───────────────
        let one_shot = bot.pending_action.take();
        if let Some((intent, target_uuid)) = one_shot {
            apply_bot_action(
                bot_entity,
                intent,
                target_uuid,
                &enemies,
                &mut budget,
                &mut cds,
                &mut commands,
            );
            // Reset attack timer so we don't immediately re-fire in the
            // Aggressive branch below.
            bot.attack_timer = bot.attack_period_secs;
            continue;
        }

        // ── 3. Aggressive policy: attack nearest enemy on cooldown ──────────
        if matches!(bot.policy, BotPolicy::Aggressive) {
            bot.attack_timer -= dt;
            if bot.attack_timer <= 0.0 {
                bot.attack_timer = bot.attack_period_secs.max(0.1);
                if let Some(target) = nearest_enemy(&pos, &enemies) {
                    apply_bot_action(
                        bot_entity,
                        ActionIntent::BasicAttack,
                        Some(target),
                        &enemies,
                        &mut budget,
                        &mut cds,
                        &mut commands,
                    );
                }
            }
        }
    }
}

fn nearest_enemy(
    from: &WorldPosition,
    enemies: &Query<(Entity, &WorldPosition, &CombatParticipant), (With<ExperienceReward>, Without<BotController>)>,
) -> Option<Uuid> {
    enemies
        .iter()
        .map(|(_, p, cp)| {
            let dx = p.x - from.x;
            let dy = p.y - from.y;
            (dx * dx + dy * dy, cp.id.0)
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, id)| id)
}

/// Mirror of the action-application branch in `process_player_input`, but
/// scoped to a single bot entity rather than the global `LocalPlayerInput`.
fn apply_bot_action(
    bot_entity: Entity,
    intent: ActionIntent,
    target_uuid: Option<Uuid>,
    enemies: &Query<(Entity, &WorldPosition, &CombatParticipant), (With<ExperienceReward>, Without<BotController>)>,
    budget: &mut ActionBudget,
    cds: &mut ActionCooldowns,
    commands: &mut Commands,
) {
    use fellytip_shared::components::ActionSlot;
    const ROUND_SECONDS: f32 = 6.0;

    match intent {
        ActionIntent::BasicAttack => {
            if !budget.consume(ActionSlot::Action) {
                return;
            }
            cds.action_cd = ROUND_SECONDS;
            let target_entity = if let Some(uuid) = target_uuid {
                enemies.iter().find(|(_, _, cp)| cp.id.0 == uuid).map(|(e, _, _)| e)
            } else {
                enemies.iter().next().map(|(e, _, _)| e)
            };
            if let Some(target_entity) = target_entity {
                commands.entity(bot_entity).insert(PendingAttack { target: target_entity });
            }
        }
        ActionIntent::UseAbility(ability_id) => {
            if !budget.consume(ActionSlot::Action) {
                return;
            }
            cds.action_cd = ROUND_SECONDS;
            let target_entity = if let Some(uuid) = target_uuid {
                enemies.iter().find(|(_, _, cp)| cp.id.0 == uuid).map(|(e, _, _)| e)
            } else {
                enemies.iter().next().map(|(e, _, _)| e)
            };
            if let Some(target_entity) = target_entity {
                commands
                    .entity(bot_entity)
                    .insert(PendingAbility { target: target_entity, ability_id });
            }
        }
        ActionIntent::Interact | ActionIntent::Dodge => {}
    }
}

// ── BRP methods ───────────────────────────────────────────────────────────────

fn require<T: DeserializeOwned>(params: &Option<Value>, key: &str) -> Result<T, BrpError> {
    let v = params
        .as_ref()
        .and_then(|p| p.get(key))
        .ok_or_else(|| BrpError::internal(format!("missing required param `{key}`")))?;
    serde_json::from_value(v.clone())
        .map_err(|e| BrpError::internal(format!("invalid param `{key}`: {e}")))
}

fn opt<T: DeserializeOwned>(params: &Option<Value>, key: &str) -> Option<T> {
    params
        .as_ref()?
        .get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn parse_class(s: &str) -> Option<CharacterClass> {
    match s {
        "Warrior" => Some(CharacterClass::Warrior),
        "Rogue" => Some(CharacterClass::Rogue),
        "Mage" => Some(CharacterClass::Mage),
        _ => None,
    }
}

/// Spawn a bot player at the given world position.
///
/// Params: `{ class?: "Warrior"|"Rogue"|"Mage", x?: f32, y?: f32, z?: f32,
///            policy?: "Idle"|"Wander"|"Aggressive", move_speed?: f32,
///            attack_period_secs?: f32 }`
///
/// Defaults: class=Warrior, position=(0,0,0), policy=Aggressive.
/// Returns: `{ ok: true, entity: u64, uuid: string }`.
pub fn dm_spawn_bot(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let class_str: String = opt(&params, "class").unwrap_or_else(|| "Warrior".to_string());
    let class = parse_class(&class_str).ok_or_else(|| {
        BrpError::internal(format!(
            "unknown class `{class_str}`; valid values: Warrior, Rogue, Mage"
        ))
    })?;

    let x: f32 = opt(&params, "x").unwrap_or(0.0);
    let y: f32 = opt(&params, "y").unwrap_or(0.0);
    let z: f32 = opt(&params, "z").unwrap_or(0.0);

    let policy_str: String = opt(&params, "policy").unwrap_or_else(|| "Aggressive".to_string());
    let policy = BotPolicy::parse(&policy_str).ok_or_else(|| {
        BrpError::internal(format!(
            "unknown policy `{policy_str}`; valid values: Idle, Wander, Aggressive"
        ))
    })?;

    let mut controller = BotController { policy, ..Default::default() };
    if let Some(v) = opt::<f32>(&params, "move_speed") {
        controller.move_speed = v.max(0.0);
    }
    if let Some(v) = opt::<f32>(&params, "attack_period_secs") {
        controller.attack_period_secs = v.max(0.1);
        controller.attack_timer = controller.attack_period_secs;
    }

    let entity = spawn_bot(world, class, WorldPosition { x, y, z }, controller);
    let uuid = world
        .get::<GameEntityId>(entity)
        .map(|g| g.0)
        .unwrap_or_default();

    tracing::info!(
        class = %class_str, policy = %policy_str, x, y, z, entity = ?entity, %uuid,
        "DM spawned bot"
    );
    Ok(json!({ "ok": true, "entity": entity.to_bits(), "uuid": uuid.to_string() }))
}

/// Despawn a bot by entity id.
///
/// Params: `{ entity: u64 }`. Errors if the entity is not a bot.
/// Returns: `{ ok: true }`.
pub fn dm_despawn_bot(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let bits: u64 = require(&params, "entity")?;
    let entity = Entity::from_bits(bits);
    let is_bot = world
        .get_entity(entity)
        .map(|e| e.contains::<BotController>())
        .unwrap_or(false);
    if !is_bot {
        return Err(BrpError::internal(format!(
            "entity {bits} is not a bot (no BotController component)"
        )));
    }
    world.despawn(entity);
    tracing::info!(entity = bits, "DM despawned bot");
    Ok(json!({ "ok": true }))
}

/// List all live bots with their basic state.
///
/// Params: `{}`
/// Returns: array of `{ entity, uuid, policy, hp, max_hp, x, y, z, class }`.
pub fn dm_list_bots(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let mut q = world.query::<(
        Entity,
        &BotController,
        &GameEntityId,
        &Health,
        &WorldPosition,
        &CombatParticipant,
    )>();
    let bots: Vec<Value> = q
        .iter(world)
        .map(|(e, ctrl, gid, health, pos, cp)| {
            let policy = match ctrl.policy {
                BotPolicy::Idle => "Idle",
                BotPolicy::Wander => "Wander",
                BotPolicy::Aggressive => "Aggressive",
            };
            json!({
                "entity": e.to_bits(),
                "uuid":   gid.0.to_string(),
                "policy": policy,
                "hp":     health.current,
                "max_hp": health.max,
                "x":      pos.x,
                "y":      pos.y,
                "z":      pos.z,
                "class":  format!("{:?}", cp.class),
            })
        })
        .collect();
    Ok(json!(bots))
}

/// Queue a one-shot `ActionIntent` on a specific bot. The bot's driver will
/// apply it on the next `FixedUpdate` tick, then clear the slot.
///
/// Params: `{ entity: u64, action: "BasicAttack"|"UseAbility", ability_id?: u8,
///            target_uuid?: string }`
/// Returns: `{ ok: true }`.
pub fn dm_set_bot_action(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let bits: u64 = require(&params, "entity")?;
    let action_str: String = require(&params, "action")?;
    let target_uuid_str: Option<String> = opt(&params, "target_uuid");
    let ability_id: Option<u8> = opt(&params, "ability_id");

    let target_uuid = target_uuid_str
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|e| BrpError::internal(format!("invalid target_uuid: {e}")))?;

    let intent = match action_str.as_str() {
        "BasicAttack" => ActionIntent::BasicAttack,
        "UseAbility" => {
            let id = ability_id.ok_or_else(|| {
                BrpError::internal("`ability_id` is required for UseAbility")
            })?;
            ActionIntent::UseAbility(id)
        }
        other => {
            return Err(BrpError::internal(format!(
                "unknown action `{other}`; valid: BasicAttack, UseAbility"
            )));
        }
    };

    let entity = Entity::from_bits(bits);
    let mut entity_mut = world
        .get_entity_mut(entity)
        .map_err(|_| BrpError::entity_not_found(entity))?;
    let mut ctrl = entity_mut
        .get_mut::<BotController>()
        .ok_or_else(|| BrpError::internal(format!("entity {bits} has no BotController")))?;
    ctrl.pending_action = Some((intent, target_uuid));

    tracing::info!(entity = bits, action = %action_str, "DM set bot action");
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_policy_strings() {
        assert_eq!(BotPolicy::parse("Idle"), Some(BotPolicy::Idle));
        assert_eq!(BotPolicy::parse("WANDER"), Some(BotPolicy::Wander));
        assert_eq!(BotPolicy::parse("aggressive"), Some(BotPolicy::Aggressive));
        assert_eq!(BotPolicy::parse("aggro"), Some(BotPolicy::Aggressive));
        assert_eq!(BotPolicy::parse("invalid"), None);
    }

    #[test]
    fn default_controller_is_aggressive() {
        let c = BotController::default();
        assert_eq!(c.policy, BotPolicy::Aggressive);
        assert!(c.move_speed > 0.0);
        assert!(c.attack_period_secs > 0.0);
    }

    #[test]
    fn parse_class_strings() {
        assert!(matches!(parse_class("Warrior"), Some(CharacterClass::Warrior)));
        assert!(matches!(parse_class("Rogue"), Some(CharacterClass::Rogue)));
        assert!(matches!(parse_class("Mage"), Some(CharacterClass::Mage)));
        assert!(parse_class("Druid").is_none());
    }
}
