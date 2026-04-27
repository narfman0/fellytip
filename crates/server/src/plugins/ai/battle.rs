//! Battle round execution: run_battle_rounds, BattleRecord, BattleHistory.

use bevy::ecs::message::MessageWriter;
use bevy::prelude::*;
use fellytip_shared::{
    combat::types::{CombatState, CombatantSnapshot, CombatantState, CoreStats, Effect},
    components::{Health, WorldPosition},
    protocol::{BattleAttackMsg, BattleEndMsg},
    world::{
        population::BATTLE_RADIUS,
        story::{StoryEvent, StoryEventKind, WriteStoryEvent},
        war::{seeded_dice, tick_battle_round},
    },
};
use std::collections::VecDeque;
use uuid::Uuid;

use crate::plugins::combat::CombatParticipant;
use crate::plugins::world_sim::WorldSimTick;

use super::{FactionMember, FactionRegistry, HomePosition, WarPartyMember};

/// Persistent record of a resolved battle — appended when one side is eliminated.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BattleRecord {
    pub winner_faction: String,
    pub loser_faction: String,
    pub target_settlement_id: String,
    pub tick: u64,
    pub attacker_casualties: u32,
    pub defender_casualties: u32,
}

/// Rolling history of resolved battles, capped at 100 entries.
#[derive(Resource, Default)]
pub struct BattleHistory {
    pub records: VecDeque<BattleRecord>,
}

impl BattleHistory {
    pub fn push(&mut self, record: BattleRecord) {
        if self.records.len() >= 100 {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }
}

/// Lives on a bookkeeping entity while a battle is ongoing at a settlement.
/// Despawned when one side is eliminated.
#[derive(Component)]
pub struct ActiveBattle {
    pub settlement_id: Uuid,
    pub attacker_faction: fellytip_shared::world::faction::FactionId,
    pub defender_faction: fellytip_shared::world::faction::FactionId,
    pub battle_x: f32,
    pub battle_y: f32,
    pub attacker_casualties: u32,
    pub defender_casualties: u32,
    /// Fractional round accumulator. Zone speed is added each tick; a round
    /// fires when the accumulator crosses 1.0. Battles near no player resolve
    /// at FROZEN_SPEED (0.05) rounds per tick = ~20 ticks per round.
    pub round_acc: f32,
}

type BattleNpcQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static CombatParticipant,
        &'static FactionMember,
        Option<&'static WarPartyMember>,
        &'static WorldPosition,
        Option<&'static HomePosition>,
    ),
>;

/// Run one combat round per attacker-defender pair for each active battle.
///
/// Battle pace is zone-gated: the `round_acc` accumulator on `ActiveBattle`
/// advances by zone speed each tick and a round only fires when it crosses 1.0.
/// Near a player (Hot) that is every tick; in Frozen it's roughly every 20 ticks.
#[allow(clippy::too_many_arguments)]
pub fn run_battle_rounds(
    mut battles: Query<(Entity, &mut ActiveBattle)>,
    all_npcs: BattleNpcQuery,
    mut health_query: Query<&mut Health>,
    tick: Res<WorldSimTick>,
    mut registry: ResMut<FactionRegistry>,
    mut commands: Commands,
    mut battle_end: MessageWriter<BattleEndMsg>,
    mut battle_attack: MessageWriter<BattleAttackMsg>,
    temp: Res<crate::plugins::interest::ChunkTemperature>,
    mut story_events: MessageWriter<WriteStoryEvent>,
    mut history: ResMut<BattleHistory>,
) {
    for (battle_entity, mut battle) in &mut battles {
        // Advance the fractional accumulator by this tick's zone speed.
        // A round only fires once the accumulator reaches a full tick's worth.
        let speed = temp.speed_at_world(battle.battle_x, battle.battle_y);
        battle.round_acc += speed;
        if battle.round_acc < 1.0 {
            continue;
        }
        battle.round_acc -= 1.0;

        let bx = battle.battle_x;
        let by = battle.battle_y;

        // Collect snapshots of attackers and defenders near the battle site.
        // attacker tuple: (entity, snap, current_hp, home_pos)
        let mut attacker_snaps: Vec<(Entity, CombatantSnapshot, i32, Option<WorldPosition>)> = Vec::new();
        let mut defender_snaps: Vec<(Entity, CombatantSnapshot, i32, Option<WorldPosition>)> = Vec::new();

        for (entity, cp, member, war_member, pos, home_pos) in &all_npcs {
            let dist = ((pos.x - bx).powi(2) + (pos.y - by).powi(2)).sqrt();
            if dist > BATTLE_RADIUS * 4.0 {
                continue;
            }
            let Ok(health) = health_query.get(entity) else { continue };
            let snap = CombatantSnapshot {
                id: cp.id.clone(),
                faction: None,
                class: cp.class.clone(),
                stats: CoreStats {
                    strength: cp.strength,
                    dexterity: cp.dexterity,
                    constitution: cp.constitution,
                    intellect: cp.intelligence,
                    wisdom: cp.wisdom,
                    charisma: cp.charisma,
                },
                health_current: health.current,
                health_max: health.max,
                level: cp.level,
                armor_class: cp.armor_class,
            };
            let home = home_pos.map(|h| h.0.clone());
            if member.0 == battle.attacker_faction && war_member.is_some() {
                attacker_snaps.push((entity, snap, health.current, home));
            } else if member.0 == battle.defender_faction && war_member.is_none() {
                defender_snaps.push((entity, snap, health.current, home));
            }
        }

        // Battle ends when one side is eliminated.
        if attacker_snaps.is_empty() || defender_snaps.is_empty() {
            let (winner, loser) = if attacker_snaps.is_empty() {
                (
                    battle.defender_faction.0.as_str().to_owned(),
                    battle.attacker_faction.0.as_str().to_owned(),
                )
            } else {
                (
                    battle.attacker_faction.0.as_str().to_owned(),
                    battle.defender_faction.0.as_str().to_owned(),
                )
            };
            tracing::info!(
                winner = %winner,
                atk_cas = battle.attacker_casualties,
                def_cas = battle.defender_casualties,
                "Battle ended"
            );
            battle_end.write(BattleEndMsg {
                settlement_id: battle.settlement_id,
                winner_faction: winner.clone(),
                attacker_casualties: battle.attacker_casualties,
                defender_casualties: battle.defender_casualties,
            });
            history.push(BattleRecord {
                winner_faction: winner,
                loser_faction: loser,
                target_settlement_id: battle.settlement_id.to_string(),
                tick: tick.0,
                attacker_casualties: battle.attacker_casualties,
                defender_casualties: battle.defender_casualties,
            });

            // Emit a story event for settlement destruction when attackers win.
            if !attacker_snaps.is_empty() {
                story_events.write(WriteStoryEvent(StoryEvent {
                    id: Uuid::new_v4(),
                    tick: tick.0,
                    world_day: (tick.0 / 300) as u32,
                    kind: StoryEventKind::SettlementRazed { by: battle.attacker_faction.clone() },
                    participants: vec![],
                    location: Some(IVec2::new(battle.battle_x as i32, battle.battle_y as i32)),
                    lore_tags: vec!["settlement".into(), "war".into()],
                }));
            }

            // Update losing faction's military strength.
            if attacker_snaps.is_empty() {
                // Defenders won.
                if let Some(f) = registry.factions.iter_mut().find(|f| f.id == battle.attacker_faction) {
                    f.resources.military_strength = (f.resources.military_strength - battle.attacker_casualties.min(10) as f32).max(0.0);
                }
            } else {
                // Attackers won.
                if let Some(f) = registry.factions.iter_mut().find(|f| f.id == battle.defender_faction) {
                    f.resources.military_strength = (f.resources.military_strength - battle.defender_casualties.min(10) as f32).max(0.0);
                }
            }

            // Remove WarPartyMember from surviving attackers and teleport them home.
            for (entity, _, _, home) in &attacker_snaps {
                let mut cmd = commands.entity(*entity);
                cmd.remove::<WarPartyMember>();
                if let Some(home_pos) = home {
                    cmd.insert(home_pos.clone());
                }
            }
            commands.entity(battle_entity).despawn();
            continue;
        }

        // Build combined CombatState for this tick's rounds.
        let all_combatants: Vec<CombatantState> = attacker_snaps.iter().chain(defender_snaps.iter())
            .map(|(_, snap, hp, _)| CombatantState { snapshot: snap.clone(), health: *hp, statuses: vec![] })
            .collect();
        let mut state = CombatState { combatants: all_combatants, round: tick.0 as u32 };

        let mut dice = seeded_dice(battle.settlement_id, tick.0);

        // Each attacker targets a defender (seeded round-robin).
        let def_count = defender_snaps.len();
        for (atk_idx, (_, atk_snap, _, _)) in attacker_snaps.iter().enumerate() {
            let def_idx = (atk_idx + tick.0 as usize) % def_count;
            let (def_entity, def_snap, _, _) = &defender_snaps[def_idx];

            let (next_state, effects) = tick_battle_round(state.clone(), &atk_snap.id, &def_snap.id, &mut dice);
            state = next_state;

            for effect in &effects {
                match effect {
                    Effect::TakeDamage { target, amount } => {
                        // Find the entity matching this CombatantId.
                        let target_entity = attacker_snaps.iter().chain(defender_snaps.iter())
                            .find(|(_, s, _, _)| &s.id == target)
                            .map(|(e, _, _, _)| *e);
                        if let Some(entity) = target_entity {
                            let is_defender = entity == *def_entity;
                            let atk_msg = BattleAttackMsg {
                                target_combatant_id: target.0,
                                damage: *amount,
                                is_kill: false,
                            };
                            battle_attack.write(atk_msg);
                            let _ = is_defender; // casualties tracked on Die effect
                        }
                    }
                    Effect::Die { target } => {
                        let target_entity = attacker_snaps.iter().chain(defender_snaps.iter())
                            .find(|(_, s, _, _)| &s.id == target)
                            .map(|(e, _, _, _)| *e);
                        if let Some(entity) = target_entity {
                            let is_attacker = attacker_snaps.iter().any(|(e, _, _, _)| *e == entity);
                            if is_attacker {
                                battle.attacker_casualties += 1;
                            } else {
                                battle.defender_casualties += 1;
                            }
                            let kill_msg = BattleAttackMsg {
                                target_combatant_id: target.0,
                                damage: 0,
                                is_kill: true,
                            };
                            battle_attack.write(kill_msg);
                            commands.entity(entity).despawn();
                        }
                    }
                    _ => {}
                }
            }
        }

        // Sync health from the updated CombatState back to ECS Health components.
        for (entity, snap, _, _) in attacker_snaps.iter().chain(defender_snaps.iter()) {
            if let Some(cs) = state.get(&snap.id) {
                if let Ok(mut health) = health_query.get_mut(*entity) {
                    health.current = cs.health;
                }
            }
        }
    }
}
