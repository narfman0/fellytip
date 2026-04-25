//! Client-side cache of received zone tiles and neighbor topology.
//!
//! The server broadcasts `ZoneTileMessage` for the current + 1-hop neighbor
//! zones on every `PlayerZoneTransition`. The client keeps the latest copy
//! per zone in `ZoneCache` so renderers / navigation can sample without a
//! round-trip.
//!
//! `ZoneNeighborCache` stores the latest `ZoneNeighborMessage` (portals +
//! hop-distance map for zones within 2 hops of the player's current zone).
//!
//! In single-player (embedded server) this is a pure ECS loop; MULTIPLAYER
//! will receive messages over the wire.

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use std::collections::HashMap;

use fellytip_shared::{
    protocol::{ZoneNeighborMessage, ZoneTileMessage},
    world::zone::ZoneId,
};

#[derive(Resource, Default)]
pub struct ZoneCache(pub HashMap<ZoneId, ZoneTileMessage>);

/// Cached portal topology for zones near the local player.
/// Updated every time a `ZoneNeighborMessage` arrives.
#[derive(Resource, Default)]
pub struct ZoneNeighborCache(pub Option<ZoneNeighborMessage>);

pub struct ZoneCachePlugin;

impl Plugin for ZoneCachePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneCache>()
            .init_resource::<ZoneNeighborCache>()
            .add_systems(Update, (ingest_zone_tiles, ingest_zone_neighbors));
    }
}

fn ingest_zone_tiles(mut reader: MessageReader<ZoneTileMessage>, mut cache: ResMut<ZoneCache>) {
    for msg in reader.read() {
        cache.0.insert(msg.zone_id, msg.clone());
    }
}

fn ingest_zone_neighbors(
    mut reader: MessageReader<ZoneNeighborMessage>,
    mut cache: ResMut<ZoneNeighborCache>,
) {
    for msg in reader.read() {
        cache.0 = Some(msg.clone());
    }
}
