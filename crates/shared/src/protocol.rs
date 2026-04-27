//! Game protocol: channel types and message types.
//!
//! In single-player mode these message types flow through Bevy's Message system
//! (MessageWriter → MessageReader) within the same process.
//!
//! MULTIPLAYER: restore lightyear channel/message registration; swap
//! MessageWriter/MessageReader for the lightyear network equivalents.

use bevy::ecs::message::Message;
use bevy::prelude::*;
use crate::combat::types::CharacterClass;
use crate::components::{EntityBounds, EntityKind, Experience, FactionBadge, GrowthStage, Health, PlayerStandings, WildlifeKind, WorldMeta, WorldPosition};
use crate::world::story::GameEntityId;
use crate::world::zone::{InteriorTile, Portal, WorldId, ZoneAnchor, ZoneId, ZoneKind};
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

/// Client → server: player has selected a class and wants to spawn.
///
/// Sent once on first join (no saved character). The server will look up
/// the DB for an existing character; if found it ignores the class field and
/// restores saved state instead.
#[derive(Serialize, Deserialize, Debug, Clone, Message)]
pub struct ChooseClassMessage {
    pub class: CharacterClass,
}

/// Server → client message carrying a single zone's tile map + anchors.
///
/// In single-player mode this flows via Bevy's Message system; MULTIPLAYER
/// will register it with `MessageRegistry` and route it across the network.
#[derive(Serialize, Deserialize, Debug, Clone, Message)]
pub struct ZoneTileMessage {
    pub zone_id: ZoneId,
    pub zone_kind: ZoneKind,
    pub width: u16,
    pub height: u16,
    pub tiles: Vec<InteriorTile>,
    pub anchors: Vec<ZoneAnchor>,
}

/// A single portal record as seen by the client, including which hop distance
/// the destination zone is from the player's current zone.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ClientPortalEntry {
    pub portal: Portal,
    /// Hop distance from the player's current zone to `portal.from_zone`.
    /// 0 = portal is in the current zone, 1 = one hop away, etc.
    pub from_hop: u8,
    /// World-space position of the portal's from-anchor (tile coords for
    /// interior zones, which share the global coordinate space).
    pub from_world_pos: glam::Vec3,
    /// World-space position of the portal's to-anchor in the destination zone.
    pub to_world_pos: glam::Vec3,
}

/// Server → client message carrying the portal graph for the player's current
/// zone plus all reachable zones within 2 hops.
///
/// Sent alongside `ZoneTileMessage` on every `PlayerZoneTransition`.
#[derive(Serialize, Deserialize, Debug, Clone, Message)]
pub struct ZoneNeighborMessage {
    /// The zone the player just entered (hop 0).
    pub current_zone: ZoneId,
    /// All portals reachable within 2 hops, with hop annotation.
    pub portals: Vec<ClientPortalEntry>,
    /// All zone IDs reachable within 2 hops and their hop distance.
    pub zone_hops: Vec<(ZoneId, u8)>,
}

// ── Plugin ───────────────────────────────────────────────────────────────────

pub struct FellytipProtocolPlugin;

impl Plugin for FellytipProtocolPlugin {
    fn build(&self, app: &mut App) {
        // Register types with Bevy's AppTypeRegistry (required for BRP inspection).
        app.register_type::<WorldId>();
        app.register_type::<WorldPosition>();
        app.register_type::<Health>();
        app.register_type::<Experience>();
        app.register_type::<EntityKind>();
        app.register_type::<WildlifeKind>();
        app.register_type::<WorldMeta>();
        app.register_type::<GrowthStage>();
        app.register_type::<FactionBadge>();
        app.register_type::<PlayerStandings>();
        app.register_type::<EntityBounds>();
        app.register_type::<GameEntityId>();
        // ChooseClassMessage flows client→server; must be registered before
        // MessageWriter/MessageReader can be used.
        app.add_message::<ChooseClassMessage>();
        // Messages are registered by the plugins that emit them (StoryPlugin, AiPlugin).
        // MULTIPLAYER: add_channel / register_message / register_component calls here.
    }
}
