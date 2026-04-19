//! Game protocol: channel types and message types.
//!
//! In single-player mode these message types flow through Bevy's Message system
//! (MessageWriter → MessageReader) within the same process.
//!
//! MULTIPLAYER: restore lightyear channel/message registration; swap
//! MessageWriter/MessageReader for the lightyear network equivalents.

use bevy::ecs::message::Message;
use bevy::prelude::*;
use crate::components::{EntityKind, Experience, FactionBadge, GrowthStage, Health, PlayerStandings, WildlifeKind, WorldMeta, WorldPosition};
use crate::world::story::GameEntityId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Channel marker types ──────────────────────────────────────────────────────

pub struct WorldStateChannel;
pub struct PlayerInputChannel;
pub struct CombatEventChannel;

// ── Messages ──────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone, Message)]
pub struct GreetMsg {
    pub message: String,
    pub player_id: Uuid,
}

#[derive(Serialize, Deserialize, Debug, Clone, Message)]
pub struct BattleStartMsg {
    pub settlement_id: Uuid,
    pub attacker_faction: String,
    pub defender_faction: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Message)]
pub struct StoryMsg {
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Message)]
pub struct BattleEndMsg {
    pub settlement_id: Uuid,
    pub winner_faction: String,
    pub attacker_casualties: u32,
    pub defender_casualties: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Message)]
pub struct BattleAttackMsg {
    pub target_combatant_id: Uuid,
    pub damage: i32,
    pub is_kill: bool,
}

// ── Plugin ───────────────────────────────────────────────────────────────────

pub struct FellytipProtocolPlugin;

impl Plugin for FellytipProtocolPlugin {
    fn build(&self, app: &mut App) {
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
        // Messages are registered by the plugins that emit them (StoryPlugin, AiPlugin).
        // MULTIPLAYER: add_channel / register_message / register_component calls here.
    }
}
