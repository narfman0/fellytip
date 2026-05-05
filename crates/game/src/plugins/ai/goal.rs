//! Faction goal evaluation: update_faction_goals, update_faction_alerts.

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use fellytip_shared::{
    protocol::BattleEndMsg,
    world::faction::pick_goal,
};

use super::{FactionAlertState, FactionRegistry};

/// Re-score and update the active goal for every faction.
pub fn update_faction_goals(mut registry: ResMut<FactionRegistry>) {
    for faction in &mut registry.factions {
        if let Some(top) = pick_goal(faction) {
            tracing::debug!(
                faction = %faction.name,
                goal = ?top,
                "Faction goal selected"
            );
        }
    }
}

/// Read incoming `BattleEndMsg` events and raise both the winner and loser
/// factions to `Alerted` state so their NPCs patrol more aggressively.
///
/// Also ticks the decay counter each world-sim tick so alerts expire after
/// `FactionAlertState::ALERT_DECAY_TICKS` ticks.
pub fn update_faction_alerts(
    mut alerts: ResMut<FactionAlertState>,
    mut battle_end_msgs: MessageReader<BattleEndMsg>,
    registry: Res<FactionRegistry>,
) {
    // Decay existing alerts first so the freshest raise wins.
    alerts.tick_decay();

    for msg in battle_end_msgs.read() {
        // Raise alerts for both sides of the battle.
        for faction_id in registry.factions.iter().filter(|f| {
            f.id.0.as_str() == msg.winner_faction
                || registry.factions.iter().any(|other| {
                    other.id.0.as_str() != msg.winner_faction
                        && other.disposition.get(&f.id)
                            == Some(&fellytip_shared::world::faction::Disposition::Hostile)
                })
        }).map(|f| f.id.clone()).collect::<Vec<_>>() {
            alerts.raise(&faction_id);
            tracing::info!(
                faction = %faction_id.0,
                "Faction alerted after battle — NPCs patrol aggressively"
            );
        }

        // Always alert the winner directly by name.
        if let Some(winner) = registry.factions.iter().find(|f| f.id.0.as_str() == msg.winner_faction) {
            alerts.raise(&winner.id.clone());
        }
    }
}
