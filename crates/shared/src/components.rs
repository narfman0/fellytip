// Shared ECS components — replicated between server and client.

use crate::world::faction::NpcRank;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// 3-D world position (game units, not pixels).
///
/// This is the single canonical position component replicated
/// from server to every connected client.
/// `z` is the elevation — entities follow terrain height automatically.
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct WorldPosition {
    pub x: f32,
    pub y: f32,
    /// Elevation in world units. 0 = sea level.
    pub z: f32,
}

/// Current and maximum hit points — replicated so clients can render health bars.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct Health {
    pub current: i32,
    pub max: i32,
}

/// Player experience and level — replicated so clients can render the HUD.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct Experience {
    pub xp: u32,
    pub level: u32,
    /// XP required to reach the next level.
    pub xp_to_next: u32,
}

impl Experience {
    pub fn new() -> Self {
        Self { xp: 0, level: 1, xp_to_next: 300 }
    }
}

impl Default for Experience {
    fn default() -> Self {
        Self::new()
    }
}

/// Visual / gameplay kind for non-player entities — replicated so the client
/// can render each type with a distinct mesh and colour.
///
/// Players do **not** carry this component; its absence on a replicated entity
/// signals "local or remote player".
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub enum EntityKind {
    /// Faction-aligned guard or soldier NPC.
    FactionNpc,
    /// Ecology-driven predator or prey creature.
    Wildlife,
    /// Static settlement marker (capital or town).
    Settlement,
}

/// Species variant for wildlife entities — replicated so the client renders the correct model.
///
/// Absent on non-wildlife entities.
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub enum WildlifeKind {
    #[default]
    Bison,
    Dog,
    Horse,
}

/// Growth stage for entities — 0.0 = newborn, 1.0 = full adult.
///
/// Replicated so the client can scale entity visuals proportionally.
/// Absent on entities spawned as adults (treated as 1.0 by the renderer).
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct GrowthStage(pub f32);

/// Replicated faction badge — identifies which faction an NPC belongs to and its
/// rank so the client can tint capsule meshes per-faction.
///
/// Absent on player entities; only present on `EntityKind::FactionNpc` entities.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct FactionBadge {
    /// String form of `FactionId` — avoids pulling `SmolStr` into lightyear's registry.
    pub faction_id: String,
    pub rank: NpcRank,
}

/// Per-faction reputation scores for a player — replicated to the owning client
/// so the HUD can display standings without a server round-trip.
///
/// Updated every world-sim tick (1 Hz) from `PlayerReputationMap`.
/// Only present on player entities.
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct PlayerStandings {
    /// `(faction_name, score)` pairs for all known factions, sorted by name.
    pub standings: Vec<(String, i32)>,
}

/// World generation parameters — attached to the local player entity and
/// replicated to the client so the client can regenerate an identical
/// `WorldMap` for client-authoritative movement prediction.
///
/// Using `u32` (not `usize`) for stable cross-platform serialization.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct WorldMeta {
    pub seed:   u64,
    pub width:  u32,
    pub height: u32,
}

/// Axis-aligned bounding box for collision.
///
/// `half_w` is the horizontal radius checked in all four cardinal quadrants.
/// `height` is the entity's vertical extent (feet to crown), reserved for
/// ceiling-clearance checks in a future pass.
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Component)]
pub struct EntityBounds {
    pub half_w: f32,
    pub height: f32,
}

impl EntityBounds {
    /// Default human-sized player bounds.
    pub const PLAYER: Self = Self { half_w: 0.35, height: 1.8 };
    /// Point check — identical to the old single-point `is_walkable_at` behaviour.
    pub const POINT: Self = Self { half_w: 0.0, height: 0.0 };

    /// The four corners of the footprint in (dx, dy) offsets from the entity centre.
    #[inline]
    pub fn corners(self) -> [(f32, f32); 4] {
        let hw = self.half_w;
        [(-hw, -hw), (hw, -hw), (-hw, hw), (hw, hw)]
    }
}

impl Default for EntityBounds {
    fn default() -> Self { Self::PLAYER }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildlife_kind_default_is_bison() {
        assert_eq!(WildlifeKind::default(), WildlifeKind::Bison);
    }

    #[test]
    fn entity_bounds_corners_symmetric() {
        let b = EntityBounds { half_w: 0.5, height: 1.0 };
        let c = b.corners();
        assert_eq!(c, [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)]);
    }

    #[test]
    fn wildlife_kind_serde_round_trip() {
        for variant in [WildlifeKind::Bison, WildlifeKind::Dog, WildlifeKind::Horse] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: WildlifeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }
}
