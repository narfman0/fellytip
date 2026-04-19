//! Lightyear protocol: channel + component + message registration.
//! Must be added AFTER `ServerPlugins`/`ClientPlugins` but BEFORE any
//! `Server`/`Client` entity is spawned.

use crate::components::{EntityKind, Experience, FactionBadge, GrowthStage, Health, PlayerStandings, WildlifeKind, WorldMeta, WorldPosition};
use crate::inputs::PlayerInput;
use crate::world::story::GameEntityId;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Channels ─────────────────────────────────────────────────────────────────

/// Ordered-reliable server→client stream (world state, story events).
pub struct WorldStateChannel;

/// Unordered-unreliable client→server stream (player inputs).
pub struct PlayerInputChannel;

/// Sequenced-reliable server→client stream (combat events).
pub struct CombatEventChannel;

// ── Messages ──────────────────────────────────────────────────────────────────

/// Sent by the server when a client first connects; carries the UUID of the
/// player entity that was spawned for this client.  The client uses it to
/// identify which replicated entity is "theirs" via `tag_local_player`.
#[derive(Serialize, Deserialize, Debug, Clone, Event)]
pub struct GreetMsg {
    pub message: String,
    /// UUID matching the `GameEntityId` on the spawned player entity.
    pub player_id: Uuid,
}

/// Sent by the server when a faction war party arrives at a rival settlement
/// and a battle begins.
#[derive(Serialize, Deserialize, Debug, Clone, Event)]
pub struct BattleStartMsg {
    pub settlement_id: Uuid,
    pub attacker_faction: String,
    pub defender_faction: String,
    /// World-space X of the battle site.
    pub x: f32,
    /// World-space Y of the battle site.
    pub y: f32,
    /// Elevation of the battle site.
    pub z: f32,
}

/// Sent by the server when a significant world story event occurs.
/// Displayed in the client's story panel (bottom-right HUD).
#[derive(Serialize, Deserialize, Debug, Clone, Event)]
pub struct StoryMsg {
    pub text: String,
}

/// Sent by the server when a battle concludes (one side eliminated).
#[derive(Serialize, Deserialize, Debug, Clone, Event)]
pub struct BattleEndMsg {
    pub settlement_id: Uuid,
    pub winner_faction: String,
    pub attacker_casualties: u32,
    pub defender_casualties: u32,
}

/// Sent by the server for each attack during a battle — drives NPC damage
/// flash on the client.
#[derive(Serialize, Deserialize, Debug, Clone, Event)]
pub struct BattleAttackMsg {
    /// `CombatantId.0` of the entity that was hit.
    pub target_combatant_id: Uuid,
    pub damage: i32,
    pub is_kill: bool,
}

// ── Plugin ───────────────────────────────────────────────────────────────────

pub struct FellytipProtocolPlugin;

impl Plugin for FellytipProtocolPlugin {
    fn build(&self, app: &mut App) {
        // Channels
        app.add_channel::<WorldStateChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            send_frequency: Duration::from_millis(50), // 20 Hz replication
            priority: 1.0,
        })
        .add_direction(NetworkDirection::ServerToClient);

        app.add_channel::<PlayerInputChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            send_frequency: Duration::ZERO,
            priority: 5.0,
        })
        .add_direction(NetworkDirection::ClientToServer);

        app.add_channel::<CombatEventChannel>(ChannelSettings {
            mode: ChannelMode::SequencedReliable(ReliableSettings::default()),
            send_frequency: Duration::ZERO,
            priority: 2.0,
        })
        .add_direction(NetworkDirection::ServerToClient);

        // Register types with Bevy's AppTypeRegistry (required for BRP inspection).
        app.register_type::<WorldPosition>();
        app.register_type::<Health>();
        app.register_type::<Experience>();
        app.register_type::<EntityKind>();
        app.register_type::<WildlifeKind>();
        app.register_type::<WorldMeta>();
        app.register_type::<GrowthStage>();
        app.register_type::<FactionBadge>();
        app.register_type::<PlayerStandings>();
        app.register_type::<GameEntityId>();

        // Register components with lightyear for network replication.
        app.register_component::<WorldPosition>();
        app.register_component::<Health>();
        app.register_component::<Experience>();
        app.register_component::<EntityKind>();
        app.register_component::<WildlifeKind>();
        app.register_component::<WorldMeta>();
        app.register_component::<GrowthStage>();
        app.register_component::<FactionBadge>();
        app.register_component::<PlayerStandings>();
        app.register_component::<GameEntityId>();

        // Messages
        app.register_message::<GreetMsg>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<PlayerInput>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<StoryMsg>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<BattleStartMsg>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<BattleEndMsg>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<BattleAttackMsg>()
            .add_direction(NetworkDirection::ServerToClient);
    }
}
