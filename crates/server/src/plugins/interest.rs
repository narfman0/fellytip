//! Interest management — per-client chunk temperature and NPC visibility culling.
//!
//! Each connected client has a two-tier active zone centred on its player:
//!
//! | Zone   | Chebyshev chunk radius | Effect                                   |
//! |--------|------------------------|------------------------------------------|
//! | Hot    | 0–`HOT_RADIUS`         | Replicated + fully simulated             |
//! | Warm   | `HOT_RADIUS+1`–`WARM_RADIUS` | Replicated, stub/reduced simulation |
//! | Frozen | > `WARM_RADIUS`        | Not replicated, simulation skipped       |
//!
//! [`update_chunk_temperature`] rebuilds the zone maps once per
//! [`WorldSimSchedule`] tick (1 Hz).  [`update_npc_replication`] re-targets
//! each NPC's [`Replicate`] component so that only clients near the NPC
//! receive its replication traffic.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use lightyear::prelude::{server::*, *};

use fellytip_shared::{
    components::WorldPosition,
    world::map::{CHUNK_TILES, MAP_HALF_HEIGHT, MAP_HALF_WIDTH},
};

use crate::plugins::{
    ai::FactionMember,
    combat::PlayerEntity,
    world_sim::WorldSimSchedule,
};

// ── Zone radii ────────────────────────────────────────────────────────────────

/// Chebyshev chunk radius for the Hot zone (fully simulated + replicated).
const HOT_RADIUS: i32 = 2;

/// Chebyshev chunk radius for the Warm zone (replicated, stub simulation).
const WARM_RADIUS: i32 = 8;

// ── Per-zone simulation speed multipliers ──────────────────────────────��──────
//
// Applied to individual NPC systems (aging, marching, battle rounds).
// Aggregate systems (births, ecology, faction goals) are not zone-gated.

/// Full simulation speed: applied when any player has the NPC's chunk in Hot zone.
pub const HOT_SPEED: f32 = 1.0;
/// Quarter speed: applied when the nearest player has the chunk in Warm zone only.
pub const WARM_SPEED: f32 = 0.25;
/// 5 % speed: applied when no player has the chunk in Hot or Warm zone (Frozen).
/// An NPC that matures in 300 ticks near a player takes 6000 ticks (~100 min) here.
pub const FROZEN_SPEED: f32 = 0.05;

// ── Resource ──────────────────────────────────────────────────────────────────

/// Per-client chunk zone maps, rebuilt every [`WorldSimSchedule`] tick.
///
/// Chunks beyond `WARM_RADIUS` are implicitly Frozen and are not stored here.
#[derive(Resource, Default)]
pub struct ChunkTemperature {
    /// Chunks within `HOT_RADIUS` of each peer's player (Chebyshev distance).
    pub hot:  HashMap<PeerId, HashSet<(i32, i32)>>,
    /// Chunks within `WARM_RADIUS` but outside `HOT_RADIUS`.
    pub warm: HashMap<PeerId, HashSet<(i32, i32)>>,
}

impl ChunkTemperature {
    /// Returns `true` if any peer has the given chunk in Hot or Warm zone.
    pub fn is_active(&self, chunk: (i32, i32)) -> bool {
        self.hot.values().any(|s| s.contains(&chunk))
            || self.warm.values().any(|s| s.contains(&chunk))
    }

    /// Simulation speed multiplier [0.05, 1.0] for a chunk.
    ///
    /// - Hot (any player within `HOT_RADIUS`)   → [`HOT_SPEED`]   (1.0)
    /// - Warm (nearest player in `WARM_RADIUS`) → [`WARM_SPEED`]  (0.25)
    /// - Frozen (no player nearby)              → [`FROZEN_SPEED`] (0.05)
    ///
    /// Used by individual-NPC systems (`age_npcs`, `march_war_parties`,
    /// `run_battle_rounds`) to slow down unobserved simulation without
    /// affecting aggregate systems (births, ecology, faction goals).
    pub fn zone_speed(&self, chunk: (i32, i32)) -> f32 {
        if self.hot.values().any(|s| s.contains(&chunk)) {
            HOT_SPEED
        } else if self.warm.values().any(|s| s.contains(&chunk)) {
            WARM_SPEED
        } else {
            FROZEN_SPEED
        }
    }

    /// Convenience wrapper: converts world-space (x, y) to a chunk coordinate
    /// and returns the zone speed for that chunk.
    pub fn speed_at_world(&self, x: f32, y: f32) -> f32 {
        self.zone_speed(world_to_chunk(x, y))
    }

    /// Returns all peers that have the chunk in Hot or Warm zone.
    pub fn visible_to(&self, chunk: (i32, i32)) -> Vec<PeerId> {
        let mut peers = Vec::new();
        for (peer, chunks) in &self.hot {
            if chunks.contains(&chunk) {
                peers.push(*peer);
            }
        }
        for (peer, chunks) in &self.warm {
            if chunks.contains(&chunk) && !peers.contains(peer) {
                peers.push(*peer);
            }
        }
        peers
    }
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct InterestPlugin;

impl Plugin for InterestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkTemperature>()
            .add_systems(
                WorldSimSchedule,
                (update_chunk_temperature, update_npc_replication).chain(),
            );
    }
}

// ── Systems ───────────────────────────────────────────────────────────────────

/// Convert a world-space XY position to a chunk grid coordinate.
///
/// `WorldPosition.x/y` are centred on the origin; tile indices are
/// `ix = x + MAP_HALF_WIDTH`, `iy = y + MAP_HALF_HEIGHT`.
fn world_to_chunk(x: f32, y: f32) -> (i32, i32) {
    let tile_x = (x + MAP_HALF_WIDTH as f32) as i32;
    let tile_y = (y + MAP_HALF_HEIGHT as f32) as i32;
    (
        tile_x.max(0) / CHUNK_TILES as i32,
        tile_y.max(0) / CHUNK_TILES as i32,
    )
}

/// Rebuild zone maps from current player positions (runs at 1 Hz).
fn update_chunk_temperature(
    clients: Query<(&RemoteId, &PlayerEntity), With<ClientOf>>,
    players: Query<&WorldPosition>,
    mut temp: ResMut<ChunkTemperature>,
) {
    temp.hot.clear();
    temp.warm.clear();

    for (remote_id, player_entity) in &clients {
        let Ok(pos) = players.get(player_entity.0) else { continue };
        let (cx, cy) = world_to_chunk(pos.x, pos.y);
        let peer = remote_id.0;

        let mut hot  = HashSet::new();
        let mut warm = HashSet::new();

        for dy in -WARM_RADIUS..=WARM_RADIUS {
            for dx in -WARM_RADIUS..=WARM_RADIUS {
                let coord = (cx + dx, cy + dy);
                if dx.abs().max(dy.abs()) <= HOT_RADIUS {
                    hot.insert(coord);
                } else {
                    warm.insert(coord);
                }
            }
        }

        temp.hot.insert(peer, hot);
        temp.warm.insert(peer, warm);
    }
}

/// Update each NPC's replication target based on which clients have its chunk active.
///
/// Replaces the `Replicate` component so lightyear's on_replace hook adjusts
/// the sender list automatically.
fn update_npc_replication(
    npcs:     Query<(Entity, &WorldPosition), With<FactionMember>>,
    temp:     Res<ChunkTemperature>,
    mut cmds: Commands,
) {
    for (entity, pos) in &npcs {
        let chunk = world_to_chunk(pos.x, pos.y);
        let peers = temp.visible_to(chunk);
        cmds.entity(entity)
            .insert(Replicate::to_clients(NetworkTarget::from(peers)));
    }
}
