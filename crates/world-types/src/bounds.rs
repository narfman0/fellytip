//! Entity bounds — physical footprint used by passability checks.
//!
//! Lives in `world-types` (not `components`) so that `world/map.rs` can use it
//! without forcing `world-types` to depend on `fellytip-shared`.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

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
