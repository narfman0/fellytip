//! Client-side cache of received zone tiles.
//!
//! The server broadcasts `ZoneTileMessage` for the current + 1-hop neighbor
//! zones on every `PlayerZoneTransition`. The client keeps the latest copy
//! per zone in `ZoneCache` so renderers / navigation can sample without a
//! round-trip.
//!
//! In single-player (embedded server) this is a pure ECS loop; MULTIPLAYER
//! will receive messages over the wire.

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use std::collections::HashMap;

use fellytip_shared::{protocol::ZoneTileMessage, world::zone::ZoneId};

#[derive(Resource, Default)]
pub struct ZoneCache(pub HashMap<ZoneId, ZoneTileMessage>);

pub struct ZoneCachePlugin;

impl Plugin for ZoneCachePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneCache>()
            .add_systems(Update, ingest_zone_tiles);
    }
}

fn ingest_zone_tiles(mut reader: MessageReader<ZoneTileMessage>, mut cache: ResMut<ZoneCache>) {
    for msg in reader.read() {
        cache.0.insert(msg.zone_id, msg.clone());
    }
}
