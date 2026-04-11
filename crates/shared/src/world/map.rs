//! Tile map: columnar stacked layers with height data and smooth surface queries.
//!
//! Each grid cell `(ix, iy)` is a [`TileColumn`] — a sorted `Vec<TileLayer>`.
//! Multiple layers can coexist vertically (ground + bridge + cave ceiling).
//!
//! Height queries are pure functions with no ECS dependency so they can be
//! called from both server movement systems and the proptest harness.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

// ── Constants ─────────────────────────────────────────────────────────────────

pub const MAP_WIDTH: usize  = 512;
pub const MAP_HEIGHT: usize = 512;

/// How far above `current_z` an entity can step up in one tick.
pub const STEP_HEIGHT: f32 = 0.6;

/// Z-lerp rate: world units per second the entity's elevation closes toward
/// the terrain surface.
pub const Z_FOLLOW_RATE: f32 = 12.0;

/// Multiplier from normalised height `[0, 1]` to world-unit Z.
const Z_SCALE: f32 = 4.0;

// ── Tile kind ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum TileKind {
    Plains,
    Forest,
    Mountain,
    Water,
    Stone,
    Void,
}

// ── Tile layer ────────────────────────────────────────────────────────────────

/// A single horizontal slice within a tile column.
///
/// Multiple layers may occupy the same `(ix, iy)` grid cell at different
/// heights — e.g. ground at Z=1, elevated platform at Z=4.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect)]
pub struct TileLayer {
    /// Bottom of this layer in world units.
    pub z_base: f32,
    /// Top surface — the Z height where entities stand.
    pub z_top: f32,
    pub kind: TileKind,
    /// Whether entities can walk on `z_top` of this layer.
    pub walkable: bool,
    /// Per-corner height offsets from `z_top`: `[TL, TR, BL, BR]`.
    ///
    /// Computed as the average of the four tile-center heights that share
    /// each corner, minus `z_top`. This gives smooth bilinear slopes between
    /// adjacent tiles without abrupt height steps.
    pub corner_offsets: [f32; 4],
}

impl TileLayer {
    /// Absolute corner heights: `z_top + corner_offsets[i]`.
    pub fn corner_heights(&self) -> [f32; 4] {
        [
            self.z_top + self.corner_offsets[0],
            self.z_top + self.corner_offsets[1],
            self.z_top + self.corner_offsets[2],
            self.z_top + self.corner_offsets[3],
        ]
    }
}

// ── Tile column ───────────────────────────────────────────────────────────────

/// Vertical stack of tile layers at one grid position.
/// Layers are sorted ascending by `z_base`. May be empty (void column).
#[derive(Clone, Debug, Default, Serialize, Deserialize, Reflect)]
pub struct TileColumn {
    pub layers: Vec<TileLayer>,
}

impl TileColumn {
    /// Return the highest walkable layer whose `z_top` is at or below
    /// `current_z + step_height`. Returns `None` if no walkable layer
    /// is reachable.
    pub fn surface_layer(&self, current_z: f32, step_height: f32) -> Option<&TileLayer> {
        self.layers
            .iter()
            .filter(|l| l.walkable && l.z_top <= current_z + step_height)
            .max_by(|a, b| a.z_top.partial_cmp(&b.z_top).unwrap())
    }
}

// ── World map ─────────────────────────────────────────────────────────────────

/// The authoritative tile map, stored as a server-side `Resource`.
///
/// Not replicated to clients (too large). Clients receive entity positions
/// only; terrain rendering will sample this on spawn/region-load later.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect, Resource)]
pub struct WorldMap {
    /// Flat row-major array: index with `ix + iy * MAP_WIDTH`.
    pub columns: Vec<TileColumn>,
    pub seed: u64,
}

impl WorldMap {
    pub fn column(&self, ix: usize, iy: usize) -> &TileColumn {
        &self.columns[ix + iy * MAP_WIDTH]
    }

    /// Returns `None` if `(x, y)` is outside the map bounds.
    pub fn column_at(&self, x: f32, y: f32) -> Option<&TileColumn> {
        let ix = x.floor() as i64;
        let iy = y.floor() as i64;
        if ix < 0 || iy < 0 || ix as usize >= MAP_WIDTH || iy as usize >= MAP_HEIGHT {
            return None;
        }
        Some(self.column(ix as usize, iy as usize))
    }
}

// ── Height query functions ────────────────────────────────────────────────────

/// Exact (non-interpolated) surface height at the tile containing `(x, y)`.
///
/// Returns the `z_top` of the highest walkable layer reachable from
/// `current_z` (within [`STEP_HEIGHT`]). Returns `None` over void or water.
pub fn surface_height_at(map: &WorldMap, x: f32, y: f32, current_z: f32) -> Option<f32> {
    map.column_at(x, y)?
        .surface_layer(current_z, STEP_HEIGHT)
        .map(|l| l.z_top)
}

/// Bilinearly interpolated surface height for fluid terrain traversal.
///
/// Reads the current tile's pre-computed corner heights (each corner is the
/// average of the four adjacent tile-centers that share it, so the height
/// field is continuous across tile boundaries) and bilinearly interpolates
/// based on the sub-tile fractional position `(fx, fy)`.
///
/// Returns `None` when no walkable surface is reachable from `current_z`.
pub fn smooth_surface_at(map: &WorldMap, x: f32, y: f32, current_z: f32) -> Option<f32> {
    let col = map.column_at(x, y)?;
    let layer = col.surface_layer(current_z, STEP_HEIGHT)?;
    let corners = layer.corner_heights();
    let fx = crate::math::tile_frac(x);
    let fy = crate::math::tile_frac(y);
    Some(crate::math::bilerp(corners, fx, fy))
}

// ── Map generation ────────────────────────────────────────────────────────────

/// Generate a world map deterministically from `seed`.
///
/// # Algorithm
/// 1. Fill 64×64 height buffer with uniform random values in `[0, 1]` using
///    a seeded `ChaCha8Rng` (reproducible across platforms).
/// 2. One-pass cardinal box-blur to smooth out point noise into continent shapes.
/// 3. Classify by height threshold:
///    - `< 0.25` → Water (z_top = 0, non-walkable)
///    - `< 0.55` → Plains
///    - `< 0.72` → Forest
///    - `≥ 0.72` → Mountain (non-walkable)
/// 4. Scale walkable tile heights: `z_top = height * Z_SCALE`.
/// 5. Compute per-corner offsets for smooth slopes: each corner is the
///    average of the four surrounding tile z_tops minus this tile's z_top.
pub fn generate_map(seed: u64) -> WorldMap {
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    // Pass 1: raw noise heights
    let mut heights = vec![0.0f32; MAP_WIDTH * MAP_HEIGHT];
    for h in heights.iter_mut() {
        *h = rng.random::<f32>();
    }

    // Pass 2: cardinal box-blur
    let orig = heights.clone();
    for iy in 0..MAP_HEIGHT {
        for ix in 0..MAP_WIDTH {
            let mut sum = orig[ix + iy * MAP_WIDTH];
            let mut count = 1.0f32;
            if ix > 0 {
                sum += orig[(ix - 1) + iy * MAP_WIDTH];
                count += 1.0;
            }
            if ix + 1 < MAP_WIDTH {
                sum += orig[(ix + 1) + iy * MAP_WIDTH];
                count += 1.0;
            }
            if iy > 0 {
                sum += orig[ix + (iy - 1) * MAP_WIDTH];
                count += 1.0;
            }
            if iy + 1 < MAP_HEIGHT {
                sum += orig[ix + (iy + 1) * MAP_WIDTH];
                count += 1.0;
            }
            heights[ix + iy * MAP_WIDTH] = sum / count;
        }
    }

    // Pass 3: compute z_top for each cell
    let z_tops: Vec<f32> = heights
        .iter()
        .map(|&h| if h < 0.25 { 0.0 } else { h * Z_SCALE })
        .collect();

    // Helper: z_top at (ix, iy) clamped to map bounds.
    let z_at = |ix: usize, iy: usize| z_tops[ix.min(MAP_WIDTH - 1) + iy.min(MAP_HEIGHT - 1) * MAP_WIDTH];

    // Pass 4: build columns with corner offsets
    let mut columns = Vec::with_capacity(MAP_WIDTH * MAP_HEIGHT);

    for iy in 0..MAP_HEIGHT {
        for ix in 0..MAP_WIDTH {
            let h = heights[ix + iy * MAP_WIDTH];
            let z_top = z_tops[ix + iy * MAP_WIDTH];

            let (kind, walkable) = if h < 0.25 {
                (TileKind::Water, false)
            } else if h < 0.55 {
                (TileKind::Plains, true)
            } else if h < 0.72 {
                (TileKind::Forest, true)
            } else {
                (TileKind::Mountain, false)
            };

            // Corner heights: average of the 4 tile-centers sharing each corner.
            // Shared corners produce continuous terrain across tile boundaries.
            let ix_m = ix.saturating_sub(1);
            let ix_p = (ix + 1).min(MAP_WIDTH - 1);
            let iy_m = iy.saturating_sub(1);
            let iy_p = (iy + 1).min(MAP_HEIGHT - 1);

            let c_tl = (z_at(ix_m, iy_m) + z_at(ix, iy_m) + z_at(ix_m, iy) + z_at(ix, iy)) / 4.0;
            let c_tr = (z_at(ix, iy_m) + z_at(ix_p, iy_m) + z_at(ix, iy) + z_at(ix_p, iy)) / 4.0;
            let c_bl = (z_at(ix_m, iy) + z_at(ix, iy) + z_at(ix_m, iy_p) + z_at(ix, iy_p)) / 4.0;
            let c_br = (z_at(ix, iy) + z_at(ix_p, iy) + z_at(ix, iy_p) + z_at(ix_p, iy_p)) / 4.0;

            let layer = TileLayer {
                z_base: (z_top - 0.5).max(0.0),
                z_top,
                kind,
                walkable,
                corner_offsets: [c_tl - z_top, c_tr - z_top, c_bl - z_top, c_br - z_top],
            };

            columns.push(TileColumn { layers: vec![layer] });
        }
    }

    WorldMap { columns, seed }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_is_deterministic() {
        let a = generate_map(42);
        let b = generate_map(42);
        assert_eq!(a.columns.len(), b.columns.len());
        for (ca, cb) in a.columns.iter().zip(b.columns.iter()) {
            assert_eq!(ca.layers.len(), cb.layers.len());
            for (la, lb) in ca.layers.iter().zip(cb.layers.iter()) {
                assert_eq!(la.kind, lb.kind);
                assert!((la.z_top - lb.z_top).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn map_column_count() {
        let m = generate_map(1);
        assert_eq!(m.columns.len(), MAP_WIDTH * MAP_HEIGHT);
    }

    #[test]
    fn no_negative_z_top() {
        let m = generate_map(7);
        for col in &m.columns {
            for layer in &col.layers {
                assert!(layer.z_top >= 0.0, "z_top must be non-negative");
            }
        }
    }

    #[test]
    fn all_tile_kinds_present() {
        // 3 seeds × 512×512 = 786k tiles; all surface kinds should appear.
        let mut seen = std::collections::HashSet::new();
        for seed in 0u64..3 {
            let m = generate_map(seed);
            for col in &m.columns {
                for layer in &col.layers {
                    seen.insert(layer.kind);
                }
            }
        }
        assert!(seen.contains(&TileKind::Water),    "no Water tiles");
        assert!(seen.contains(&TileKind::Plains),   "no Plains tiles");
        assert!(seen.contains(&TileKind::Forest),   "no Forest tiles");
        assert!(seen.contains(&TileKind::Mountain), "no Mountain tiles");
    }

    #[test]
    fn smooth_surface_between_tile_heights() {
        // Build a minimal 2×1 map manually: left tile z=1.0, right tile z=3.0.
        let make_layer = |z_top: f32| TileLayer {
            z_base: 0.0,
            z_top,
            kind: TileKind::Plains,
            walkable: true,
            corner_offsets: [0.0; 4],
        };
        let mut map = WorldMap { columns: vec![TileColumn::default(); MAP_WIDTH * MAP_HEIGHT], seed: 0 };
        map.columns[0] = TileColumn { layers: vec![make_layer(1.0)] };
        map.columns[1] = TileColumn { layers: vec![make_layer(3.0)] };

        // At x=0.5 (middle of tile 0), z should equal tile 0's z_top (no offsets).
        let h = smooth_surface_at(&map, 0.5, 0.0, 5.0).unwrap();
        assert!((h - 1.0).abs() < 1e-6, "Expected 1.0, got {h}");
    }

    #[test]
    fn stacked_layers_selectable() {
        let ground = TileLayer {
            z_base: 0.0, z_top: 1.0, kind: TileKind::Plains,
            walkable: true, corner_offsets: [0.0; 4],
        };
        let bridge = TileLayer {
            z_base: 3.0, z_top: 3.5, kind: TileKind::Stone,
            walkable: true, corner_offsets: [0.0; 4],
        };
        let col = TileColumn { layers: vec![ground, bridge] };

        // From ground level (z ≈ 0.8) we can only reach the ground layer.
        let at_ground = col.surface_layer(0.8, STEP_HEIGHT).unwrap();
        assert_eq!(at_ground.kind, TileKind::Plains);

        // From bridge height (z ≈ 3.2) we get the bridge.
        let at_bridge = col.surface_layer(3.2, STEP_HEIGHT).unwrap();
        assert_eq!(at_bridge.kind, TileKind::Stone);
    }

    #[test]
    fn surface_height_at_returns_none_over_water() {
        let map = generate_map(0);
        // Find a water tile and confirm surface_height_at returns None.
        let water_pos = map.columns.iter().enumerate().find_map(|(i, col)| {
            if col.layers.first().map(|l| l.kind == TileKind::Water).unwrap_or(false) {
                let ix = i % MAP_WIDTH;
                let iy = i / MAP_WIDTH;
                Some((ix as f32 + 0.5, iy as f32 + 0.5))
            } else {
                None
            }
        });
        if let Some((wx, wy)) = water_pos {
            assert!(
                surface_height_at(&map, wx, wy, 0.0).is_none(),
                "Water tiles should not be walkable"
            );
        }
        // If the seed has no water tiles, the test passes vacuously.
    }
}
