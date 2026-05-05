//! Interest management — zone-based simulation speed for NPCs.
//!
//! Tracks which map chunks are "hot" (near a player) or "warm" (within view
//! range) so that individual-NPC systems can scale their simulation speed:
//!
//! | Zone   | Chebyshev chunk radius         | Simulation speed |
//! |--------|-------------------------------|-----------------|
//! | Hot    | 0–`HOT_RADIUS`                 | 1.0× (real-time) |
//! | Warm   | `HOT_RADIUS+1`–`WARM_RADIUS`  | 0.25×            |
//! | Frozen | > `WARM_RADIUS`               | 0.05×            |
//!
//! MULTIPLAYER: restore per-client zone maps (HashMap<PeerId, …>) and
//! replication-target updates (update_npc_replication) when re-adding lightyear.

use std::collections::HashSet;

use bevy::prelude::*;

use fellytip_shared::{
    components::WorldPosition,
    world::map::{CHUNK_TILES, MAP_HALF_HEIGHT, MAP_HALF_WIDTH},
};

use crate::plugins::perf::ThrottleLevel;
use crate::plugins::world_sim::WorldSimSchedule;

// ── Zone radii ────────────────────────────────────────────────────────────────

const HOT_RADIUS:  i32 = 2;
const WARM_RADIUS: i32 = 8;

// ── Per-zone simulation speed multipliers ─────────────────────────────────────

pub const HOT_SPEED:    f32 = 1.0;
pub const WARM_SPEED:   f32 = 0.25;
pub const FROZEN_SPEED: f32 = 0.05;

// ── SimTier enum ──────────────────────────────────────────────────────────────

/// Simulation LOD tier for an entity based on proximity to the nearest player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimTier {
    Hot,
    Warm,
    Frozen,
}

impl SimTier {
    pub fn speed(self) -> f32 {
        match self {
            SimTier::Hot    => HOT_SPEED,
            SimTier::Warm   => WARM_SPEED,
            SimTier::Frozen => FROZEN_SPEED,
        }
    }
}

/// Return the LOD tier for an entity at the given world position.
pub fn entity_zone(pos: &WorldPosition, chunk_temp: &ChunkTemperature) -> SimTier {
    let chunk = world_to_chunk(pos.x, pos.y);
    if chunk_temp.hot.contains(&chunk) {
        SimTier::Hot
    } else if chunk_temp.warm.contains(&chunk) {
        SimTier::Warm
    } else {
        SimTier::Frozen
    }
}

/// Throttle-aware variant of `entity_zone`: downgrades the base tier
/// according to the current `AdaptiveScheduler` level.
///
/// | Base ＼ Throttle | Full | Reduced | Minimal  | Suspended |
/// |------------------|------|---------|----------|-----------|
/// | Hot              | Hot  | Warm    | Frozen   | Frozen    |
/// | Warm             | Warm | Warm    | Frozen   | Frozen    |
/// | Frozen           | Frozen | Frozen | Frozen | Frozen    |
pub fn effective_zone(
    pos: &WorldPosition,
    chunk_temp: &ChunkTemperature,
    throttle: ThrottleLevel,
) -> SimTier {
    let base = entity_zone(pos, chunk_temp);
    match throttle {
        ThrottleLevel::Full => base,
        ThrottleLevel::Reduced => match base {
            SimTier::Hot => SimTier::Warm,
            other => other,
        },
        ThrottleLevel::Minimal | ThrottleLevel::Suspended => SimTier::Frozen,
    }
}

// ── Resource ──────────────────────────────────────────────────────────────────

/// Chunk zone maps rebuilt every WorldSimSchedule tick (1 Hz).
///
/// Hot and Warm sets cover chunks within the respective radius of the local
/// player. Chunks not in either set are implicitly Frozen.
#[derive(Resource, Default)]
pub struct ChunkTemperature {
    pub hot:  HashSet<(i32, i32)>,
    pub warm: HashSet<(i32, i32)>,
}

impl ChunkTemperature {
    pub fn is_active(&self, chunk: (i32, i32)) -> bool {
        self.hot.contains(&chunk) || self.warm.contains(&chunk)
    }

    pub fn zone_speed(&self, chunk: (i32, i32)) -> f32 {
        if self.hot.contains(&chunk) {
            HOT_SPEED
        } else if self.warm.contains(&chunk) {
            WARM_SPEED
        } else {
            FROZEN_SPEED
        }
    }

    pub fn speed_at_world(&self, x: f32, y: f32) -> f32 {
        self.zone_speed(world_to_chunk(x, y))
    }
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct InterestPlugin;

impl Plugin for InterestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkTemperature>()
            .add_systems(WorldSimSchedule, update_chunk_temperature);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn world_to_chunk(x: f32, y: f32) -> (i32, i32) {
    let tile_x = (x + MAP_HALF_WIDTH as f32) as i32;
    let tile_y = (y + MAP_HALF_HEIGHT as f32) as i32;
    (
        tile_x.max(0) / CHUNK_TILES as i32,
        tile_y.max(0) / CHUNK_TILES as i32,
    )
}

// ── Systems ───────────────────────────────────────────────────────────────────

/// Rebuild zone maps from the local player's position (runs at 1 Hz).
///
/// Players are identified by the absence of ExperienceReward (NPCs have it).
/// MULTIPLAYER: iterate over all connected client positions instead.
fn update_chunk_temperature(
    players: Query<&WorldPosition, Without<super::combat::ExperienceReward>>,
    mut temp: ResMut<ChunkTemperature>,
) {
    temp.hot.clear();
    temp.warm.clear();

    for pos in &players {
        let (cx, cy) = world_to_chunk(pos.x, pos.y);

        for dy in -WARM_RADIUS..=WARM_RADIUS {
            for dx in -WARM_RADIUS..=WARM_RADIUS {
                let coord = (cx + dx, cy + dy);
                if dx.abs().max(dy.abs()) <= HOT_RADIUS {
                    temp.hot.insert(coord);
                } else {
                    temp.warm.insert(coord);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos_at_origin() -> WorldPosition {
        WorldPosition { x: 0.0, y: 0.0, z: 0.0 }
    }

    fn origin_chunk() -> (i32, i32) {
        world_to_chunk(0.0, 0.0)
    }

    fn hot_temp() -> ChunkTemperature {
        let mut t = ChunkTemperature::default();
        t.hot.insert(origin_chunk());
        t
    }

    fn warm_temp() -> ChunkTemperature {
        let mut t = ChunkTemperature::default();
        t.warm.insert(origin_chunk());
        t
    }

    #[test]
    fn full_throttle_passes_base_zone() {
        let p = pos_at_origin();
        assert_eq!(effective_zone(&p, &hot_temp(), ThrottleLevel::Full), SimTier::Hot);
        assert_eq!(effective_zone(&p, &warm_temp(), ThrottleLevel::Full), SimTier::Warm);
        assert_eq!(
            effective_zone(&p, &ChunkTemperature::default(), ThrottleLevel::Full),
            SimTier::Frozen
        );
    }

    #[test]
    fn reduced_demotes_hot_to_warm() {
        let p = pos_at_origin();
        assert_eq!(effective_zone(&p, &hot_temp(), ThrottleLevel::Reduced), SimTier::Warm);
        assert_eq!(effective_zone(&p, &warm_temp(), ThrottleLevel::Reduced), SimTier::Warm);
    }

    #[test]
    fn minimal_and_suspended_force_frozen() {
        let p = pos_at_origin();
        for level in [ThrottleLevel::Minimal, ThrottleLevel::Suspended] {
            assert_eq!(effective_zone(&p, &hot_temp(), level), SimTier::Frozen);
            assert_eq!(effective_zone(&p, &warm_temp(), level), SimTier::Frozen);
        }
    }
}
