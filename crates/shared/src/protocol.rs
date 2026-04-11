//! Lightyear protocol: channel + component + message registration.
//! Must be added AFTER `ServerPlugins`/`ClientPlugins` but BEFORE any
//! `Server`/`Client` entity is spawned.

use crate::components::{Experience, Health, WorldPosition};
use crate::inputs::PlayerInput;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// ── Channels ─────────────────────────────────────────────────────────────────

/// Ordered-reliable server→client stream (world state, story events).
pub struct WorldStateChannel;

/// Unordered-unreliable client→server stream (player inputs).
pub struct PlayerInputChannel;

/// Sequenced-reliable server→client stream (combat events).
pub struct CombatEventChannel;

// ── Messages ──────────────────────────────────────────────────────────────────

/// Sent by the server when a client first connects; verifies the channel.
#[derive(Serialize, Deserialize, Debug, Clone, Event)]
pub struct GreetMsg {
    pub message: String,
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

        // Register components with lightyear for network replication.
        app.register_component::<WorldPosition>();
        app.register_component::<Health>();
        app.register_component::<Experience>();

        // Messages
        app.register_message::<GreetMsg>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<PlayerInput>()
            .add_direction(NetworkDirection::ClientToServer);
    }
}
