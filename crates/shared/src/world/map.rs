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

pub const MAP_WIDTH: usize  = 1024;
pub const MAP_HEIGHT: usize = 1024;

/// Half the map width/height, used to convert between world coords (centered on
/// (0,0)) and tile indices (0..MAP_WIDTH / 0..MAP_HEIGHT).
pub const MAP_HALF_WIDTH:  i64 = (MAP_WIDTH  / 2) as i64;
pub const MAP_HALF_HEIGHT: i64 = (MAP_HEIGHT / 2) as i64;

/// Side length of one chunk in tiles.  Matches the client terrain renderer —
/// shared here so server-side interest management uses the same partition.
pub const CHUNK_TILES: usize = 32;

/// How far above `current_z` an entity can step up in one tick.
/// Set high enough to handle steep slopes at the new Z_SCALE=20 terrain.
pub const STEP_HEIGHT: f32 = 2.0;

/// Gravitational acceleration in world units/sec² (downward = negative).
pub const GRAVITY: f32 = -20.0;

/// Terminal velocity cap in world units/sec (downward). Safety clamp so
/// a single large drop cannot teleport an entity through thin floors.
pub const MAX_FALL_SPEED: f32 = -50.0;

/// Distance from terrain surface within which the entity is considered
/// grounded and snaps to terrain. Prevents floating-point oscillation on
/// flat tiles where bilinear interpolation may jitter by a small epsilon.
pub const LAND_SNAP: f32 = 0.05;

/// Vertical speed imparted on jump (world units/sec, upward = positive).
pub const JUMP_SPEED: f32 = 9.0;

/// Horizontal speed multiplier applied during a dash burst.
pub const DASH_SPEED: f32 = 12.0;

/// Duration of the dash burst in seconds.
pub const DASH_DURATION: f32 = 0.18;

/// Upward acceleration applied to entities submerged below the water surface.
/// Strong enough to quickly overcome gravity and bring the player up.
pub const SWIM_BUOYANCY: f32 = 35.0;

/// Maximum upward speed while swimming to the surface (world units/sec).
pub const SWIM_RISE_SPEED: f32 = 6.0;

/// Multiplier from normalised height `[0, 1]` to world-unit Z for surface terrain.
/// At 20.0, mountain peaks reach ~20 world units above sea level.
pub const Z_SCALE: f32 = 20.0;

// ── Tile kind ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum TileKind {
    // ── Surface biomes ────────────────────────────────────────────────────────
    /// Legacy / fallback plains (hot-temperate grassland).
    Plains,
    /// Legacy / fallback forest.
    Forest,
    /// High-altitude impassable peaks.
    Mountain,
    /// Open water — non-walkable.
    Water,
    /// Bare rocky surface.
    Stone,
    // ── Whittaker biomes ──────────────────────────────────────────────────────
    Desert,
    Savanna,
    TropicalForest,
    TropicalRainforest,
    Grassland,
    TemperateForest,
    Taiga,
    Tundra,
    PolarDesert,
    Arctic,
    /// Surface river tile (flow accumulation ≥ river_threshold).
    River,
    // ── Meta ─────────────────────────────────────────────────────────────────
    Void,
}

impl TileKind {
    /// Every variant in declaration order.  Used by the client renderer to
    /// pre-build the material map without duplicating the list.
    pub const ALL: &'static [Self] = &[
        Self::Plains,
        Self::Forest,
        Self::Mountain,
        Self::Water,
        Self::Stone,
        Self::Desert,
        Self::Savanna,
        Self::TropicalForest,
        Self::TropicalRainforest,
        Self::Grassland,
        Self::TemperateForest,
        Self::Taiga,
        Self::Tundra,
        Self::PolarDesert,
        Self::Arctic,
        Self::River,
        Self::Void,
    ];

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

    /// True for surface-generated kinds (Plains, Forest, Mountain, Water, Stone,
    /// and all Whittaker biomes + River).
    pub fn is_surface_kind(&self) -> bool {
        matches!(
            self.kind,
            TileKind::Plains | TileKind::Forest | TileKind::Mountain
                | TileKind::Water | TileKind::Stone
                | TileKind::Desert | TileKind::Savanna
                | TileKind::TropicalForest | TileKind::TropicalRainforest
                | TileKind::Grassland | TileKind::TemperateForest
                | TileKind::Taiga | TileKind::Tundra
                | TileKind::PolarDesert | TileKind::Arctic | TileKind::River
        )
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
    /// Return the highest walkable layer reachable from `current_z`.
    ///
    /// For single-walkable-layer columns (all normal terrain) the layer is
    /// always returned regardless of height — the movement system snaps the
    /// entity's z to the surface after horizontal movement.
    ///
    /// For multi-layer columns (bridges, future cave ceilings) `step_height`
    /// is used to select the correct level: only layers whose `z_top` is at
    /// or below `current_z + step_height` are considered.
    pub fn surface_layer(&self, current_z: f32, step_height: f32) -> Option<&TileLayer> {
        let walkable_count = self.layers.iter().filter(|l| l.walkable).count();
        if walkable_count == 0 {
            return None;
        }
        if walkable_count == 1 {
            return self.layers.iter().find(|l| l.walkable);
        }
        // Multi-layer: use step_height to choose the right level.
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
    /// Flat row-major array: index with `ix + iy * self.width`.
    pub columns: Vec<TileColumn>,
    pub width: usize,
    pub height: usize,
    pub seed: u64,
    /// Flat row-major road flag array. `true` = road tile (same indexing as `columns`).
    /// Populated after `generate_map` by [`crate::world::civilization::generate_roads`].
    #[serde(default)]
    pub road_tiles: Vec<bool>,
    /// Precomputed world-space (x, y, z) spawn positions with verified open neighbors.
    /// Populated by `generate_spawn_points` during world gen, after roads are stamped.
    #[serde(default)]
    pub spawn_points: Vec<(f32, f32, f32)>,
    /// Set to `true` after `apply_building_tiles` runs. Used as a cache-invalidation
    /// sentinel: old `.bin` files default to `false` and are regenerated automatically.
    #[serde(default)]
    pub buildings_stamped: bool,
}

impl WorldMap {
    pub fn column(&self, ix: usize, iy: usize) -> &TileColumn {
        &self.columns[ix + iy * self.width]
    }

    /// Mark the surface layer at tile `(ix, iy)` as non-walkable so buildings
    /// act as solid obstacles for the existing `is_walkable_at` movement check.
    pub fn mark_impassable(&mut self, ix: usize, iy: usize) {
        let idx = ix + iy * self.width;
        if let Some(col) = self.columns.get_mut(idx) {
            if let Some(layer) = col.layers.iter_mut().find(|l| l.is_surface_kind()) {
                layer.walkable = false;
            }
        }
    }

    /// Add a walkable stair/floor layer at the given tile position and z height.
    /// Skips silently if a walkable layer already exists within 0.1 world units of z_top.
    /// Inserts the new layer in sorted order by z_base.
    pub fn add_stair_layer(&mut self, ix: usize, iy: usize, z_top: f32, kind: TileKind) {
        if ix >= self.width || iy >= self.height { return; }
        let idx = ix + iy * self.width;
        let col = &mut self.columns[idx];
        if col.layers.iter().any(|l| l.walkable && (l.z_top - z_top).abs() < 0.1) { return; }
        let new_layer = TileLayer {
            z_base: (z_top - 0.5).max(0.0),
            z_top,
            kind,
            walkable: true,
            corner_offsets: [0.0; 4],
        };
        let pos = col.layers.partition_point(|l| l.z_base < new_layer.z_base);
        col.layers.insert(pos, new_layer);
    }

    /// Returns `None` if `(x, y)` is outside the map bounds.
    ///
    /// `x` and `y` are world-space coordinates centered on (0, 0): the map
    /// spans [-width/2, width/2) × [-height/2, height/2).
    pub fn column_at(&self, x: f32, y: f32) -> Option<&TileColumn> {
        let half_w = (self.width / 2) as i64;
        let half_h = (self.height / 2) as i64;
        let ix = x.floor() as i64 + half_w;
        let iy = y.floor() as i64 + half_h;
        if ix < 0 || iy < 0 || ix as usize >= self.width || iy as usize >= self.height {
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

/// Returns `true` if `(x, y)` has a walkable surface (or underground) layer
/// reachable from `current_z` within one step. Returns `false` over Water,
/// River, Mountain, void columns, and out-of-bounds positions.
///
/// Used by the movement system to prevent entities from walking into impassable
/// terrain. See `docs/systems/world-map.md` for the wall-slide behaviour.
pub fn is_walkable_at(map: &WorldMap, x: f32, y: f32, current_z: f32) -> bool {
    smooth_surface_at(map, x, y, current_z).is_some()
}

/// Returns `true` if an entity whose floor is at `entity_z` can pass through
/// `col` horizontally.
///
/// Passage is allowed when:
/// 1. A walkable surface exists reachable from `entity_z` (normal ground movement).
/// 2. `entity_z` is at or above the highest layer top in the column (aerial clearance).
///    Void columns (no layers) are always impassable.
fn column_is_clear(col: &TileColumn, entity_z: f32) -> bool {
    if col.surface_layer(entity_z, STEP_HEIGHT).is_some() {
        return true;
    }
    let max_z_top = col.layers.iter().map(|l| l.z_top).fold(f32::NEG_INFINITY, f32::max);
    if max_z_top == f32::NEG_INFINITY {
        return false; // void column
    }
    // Strict greater-than: entity must be meaningfully above the obstacle top.
    // At exactly z_top the entity is at the obstacle surface, not above it.
    entity_z > max_z_top
}

/// Passability check with an entity bounding box.
///
/// Checks all four corners of the entity's footprint at `(cx, cy)`. A corner
/// that falls outside the map boundary is treated as impassable.
///
/// Replaces [`is_walkable_at`] in the movement loop. Passing
/// [`crate::components::EntityBounds::POINT`] reproduces the legacy
/// single-point behaviour exactly.
pub fn is_passable_with_bounds(
    map: &WorldMap,
    cx: f32,
    cy: f32,
    entity_z: f32,
    bounds: crate::components::EntityBounds,
) -> bool {
    for (dx, dy) in bounds.corners() {
        match map.column_at(cx + dx, cy + dy) {
            None => return false,
            Some(col) => {
                if !column_is_clear(col, entity_z) {
                    return false;
                }
            }
        }
    }
    true
}

/// Returns `true` if the tile at `(x, y)` is a Water or River tile.
///
/// Unlike [`is_walkable_at`], this ignores walkability — Water/River tiles
/// are never walkable but an entity can still swim on them.
pub fn is_water_at(map: &WorldMap, x: f32, y: f32) -> bool {
    map.column_at(x, y)
        .and_then(|col| col.layers.last())
        .map(|l| matches!(l.kind, TileKind::Water | TileKind::River))
        .unwrap_or(false)
}

/// Returns the surface Z of the Water or River layer at `(x, y)`, or `None`
/// if the tile is not water.  Used to float entities at the water surface.
pub fn water_surface_at(map: &WorldMap, x: f32, y: f32) -> Option<f32> {
    map.column_at(x, y)?
        .layers
        .iter()
        .find(|l| matches!(l.kind, TileKind::Water | TileKind::River))
        .map(|l| l.z_top)
}

/// Approximate terrain normal at `(x, y)` via finite differences over
/// `smooth_surface_at`. Used to project movement velocity onto the slope so
/// the character neither speeds up going downhill nor stalls going uphill.
///
/// Returns `Vec3::Y` (flat-ground normal) when any sample is out of range.
pub fn terrain_normal_at(map: &WorldMap, x: f32, y: f32, current_z: f32) -> Vec3 {
    const STEP: f32 = 0.5;
    let h = |dx: f32, dy: f32| {
        smooth_surface_at(map, x + dx, y + dy, current_z).unwrap_or(current_z)
    };
    let dzdx = (h(STEP, 0.0) - h(-STEP, 0.0)) / (2.0 * STEP);
    let dzdy = (h(0.0, STEP) - h(0.0, -STEP)) / (2.0 * STEP);
    Vec3::new(-dzdx, 1.0, -dzdy).normalize()
}

/// Find a walkable surface spawn point near the world origin (map centre).
///
/// Tries tiles in an expanding ring around `(0, 0)` until it finds one with a
/// walkable surface layer.  Uses `Z_SCALE * 2` as the z-ceiling so the query
/// succeeds regardless of terrain height — no existing `current_z` is needed.
///
/// Returns world-space `(x, y, z)` for the first hit.  Falls back to
/// `(0.0, 0.0, 0.0)` only if the entire map has no walkable surface (should
/// never happen on a valid generated map).
pub fn find_surface_spawn(map: &WorldMap) -> (f32, f32, f32) {
    // Any walkable surface tile has z_top in [0, Z_SCALE].  Using Z_SCALE * 2
    // as the ceiling guarantees surface_layer sees all surface layers while
    // still returning the topmost one (underground layers are negative).
    const Z_CEIL: f32 = Z_SCALE * 2.0;
    for radius in 0..(map.width.min(map.height) / 2) {
        let r = radius as i64;
        for dy in -r..=r {
            for dx in -r..=r {
                // Only test the outer ring of the current radius.
                if dx.abs() != r && dy.abs() != r {
                    continue;
                }
                // Tile centres in world space (world is centered on (0,0)).
                let x = dx as f32 + 0.5;
                let y = dy as f32 + 0.5;
                if let Some(z) = smooth_surface_at(map, x, y, Z_CEIL) {
                    return (x, y, z);
                }
            }
        }
    }
    (0.0, 0.0, 0.0)
}

/// Precompute up to 3 well-spaced spawn points that each have ≥ 2 open cardinal
/// neighbours, so players never land in a walled valley they cannot escape.
pub fn generate_spawn_points(map: &WorldMap) -> Vec<(f32, f32, f32)> {
    const COUNT: usize = 3;
    const Z_CEIL: f32 = Z_SCALE * 2.0;
    const MIN_SPACING_SQ: f32 = 50.0 * 50.0;

    let mut points: Vec<(f32, f32, f32)> = Vec::new();

    'outer: for radius in 0..(map.width.min(map.height) / 2) {
        let r = radius as i64;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r { continue; }
                let x = dx as f32 + 0.5;
                let y = dy as f32 + 0.5;
                let Some(z) = smooth_surface_at(map, x, y, Z_CEIL) else { continue };

                let open = [(x + 1., y), (x - 1., y), (x, y + 1.), (x, y - 1.)]
                    .iter()
                    .filter(|&&(nx, ny)| is_walkable_at(map, nx, ny, z))
                    .count();
                if open < 2 { continue; }

                if points.iter().any(|&(px, py, _)| {
                    (x - px).powi(2) + (y - py).powi(2) < MIN_SPACING_SQ
                }) { continue; }

                points.push((x, y, z));
                if points.len() >= COUNT { break 'outer; }
            }
        }
    }
    points
}

// ── Biome classification ──────────────────────────────────────────────────────

/// Normalised-height threshold above which a tile becomes an impassable cliff.
const MOUNTAIN_THRESHOLD: f32 = 0.85;

/// Normalised-height threshold above which a tile becomes a walkable snowfield,
/// creating a visible transition zone between normal biomes and impassable cliffs.
const ARCTIC_THRESHOLD: f32 = 0.75;

/// Classify a surface tile using the Whittaker diagram.
///
/// `temperature` — `0.0` (tropical/equator) to `1.0` (polar/cold).  Derived
/// from latitude + altitude penalty.
/// `moisture` — `0.0` (arid) to `1.0` (wet).  From a separate fBm pass.
///
/// High-elevation tiles (h ≥ [`MOUNTAIN_THRESHOLD`]) and water tiles (h < 0.25)
/// are handled before this function is called and map to `Mountain` / `Water`.
/// Tiles with h ≥ [`ARCTIC_THRESHOLD`] become `Arctic` snowfields regardless of
/// temperature — the altitude override fires before this function is called.
pub fn classify_biome(temperature: f32, moisture: f32) -> TileKind {
    if temperature < 0.2 {
        // Tropical band
        if moisture < 0.30 { TileKind::Desert }
        else if moisture < 0.55 { TileKind::Savanna }
        else if moisture < 0.75 { TileKind::TropicalForest }
        else { TileKind::TropicalRainforest }
    } else if temperature < 0.45 {
        // Subtropical / warm temperate
        if moisture < 0.25 { TileKind::Savanna }
        else if moisture < 0.60 { TileKind::Grassland }
        else { TileKind::TemperateForest }
    } else if temperature < 0.65 {
        // Cool temperate
        if moisture < 0.25 { TileKind::Grassland }
        else { TileKind::TemperateForest }
    } else if temperature < 0.82 {
        // Boreal / subarctic
        if moisture < 0.35 { TileKind::Tundra }
        else { TileKind::Taiga }
    } else {
        // Polar
        if moisture < 0.50 { TileKind::PolarDesert }
        else { TileKind::Arctic }
    }
}

// ── Map generation ────────────────────────────────────────────────────────────

/// Generate a world map deterministically from `seed` with the given `width` and `height`.
///
/// Pass [`MAP_WIDTH`] / [`MAP_HEIGHT`] for the defaults.
///
/// # Passes
/// 1. **Surface** — domain-warped fBm terrain noise → biome classification with
///    elevation overrides (Arctic snowfields, impassable Mountain peaks) →
///    corner-averaged heights for smooth bilinear slopes.
/// 2. **Rivers** — steepest-descent drainage accumulation → River tiles + valley
///    carving → corner offset recomputation.
pub fn generate_map(seed: u64, width: usize, height: usize) -> WorldMap {
    use crate::math::fbm;

    // ── Surface pass ──────────────────────────────────────────────────────────
    // Derive large coordinate offsets so different seeds sample entirely
    // different regions of the infinite fBm noise field.
    let ox  = ((seed.wrapping_mul(2_654_435_761)) % 100_000) as f32;
    let oy  = ((seed.wrapping_mul(805_459_861))   % 100_000) as f32;
    // Separate offset for precipitation noise.
    let mox = ((seed.wrapping_mul(1_234_567_891)) % 100_000) as f32;
    let moy = ((seed.wrapping_mul(987_654_321))   % 100_000) as f32;
    // Offsets for the two domain-warp displacement fields.
    let wx_ox = ((seed.wrapping_mul(3_141_592_653)) % 100_000) as f32;
    let wx_oy = ((seed.wrapping_mul(2_718_281_828)) % 100_000) as f32;
    let wy_ox = ((seed.wrapping_mul(1_414_213_562)) % 100_000) as f32;
    let wy_oy = ((seed.wrapping_mul(1_618_033_988)) % 100_000) as f32;

    // Base frequency: ~4 cycles across the map → continent-scale features.
    let base_freq: f32 = 4.0 / width as f32;
    // Warp samples at 2× base frequency for mid-scale displacement.
    let warp_freq: f32 = base_freq * 2.0;
    // Amplitude of the domain warp in tile units — creates organic ridges.
    let warp_amp: f32 = 80.0;
    // Precipitation varies at a finer scale than elevation.
    let moisture_freq: f32 = 6.0 / width as f32;

    let heights: Vec<f32> = (0..width * height)
        .map(|idx| {
            let ix = (idx % width) as f32;
            let iy = (idx / width) as f32;
            // Sample two warp displacement fields.
            let wx = fbm((ix + wx_ox) * warp_freq, (iy + wx_oy) * warp_freq, 4, 0.5, 2.0);
            let wy = fbm((ix + wy_ox) * warp_freq, (iy + wy_oy) * warp_freq, 4, 0.5, 2.0);
            // Displace the sample coordinates before the main height fBm.
            let warped_x = (ix + ox) + wx * warp_amp;
            let warped_y = (iy + oy) + wy * warp_amp;
            fbm(warped_x * base_freq, warped_y * base_freq, 6, 0.5, 2.0)
        })
        .collect();

    // Precipitation field: independent fBm pass.
    let moisture: Vec<f32> = (0..width * height)
        .map(|idx| {
            let ix = (idx % width) as f32;
            let iy = (idx / width) as f32;
            fbm((ix + mox) * moisture_freq, (iy + moy) * moisture_freq, 4, 0.5, 2.0)
        })
        .collect();

    let z_tops: Vec<f32> = heights
        .iter()
        .map(|&h| if h < 0.25 { 0.0 } else { h * Z_SCALE })
        .collect();

    let z_at = |ix: usize, iy: usize| {
        z_tops[ix.min(width - 1) + iy.min(height - 1) * width]
    };

    let mut columns = Vec::with_capacity(width * height);

    for iy in 0..height {
        for ix in 0..width {
            let h = heights[ix + iy * width];
            let m = moisture[ix + iy * width];
            let z_top = z_tops[ix + iy * width];

            // Temperature: increases toward equator (center of map), decreases
            // with altitude.  Latitude factor: 0 at equator, 1 at poles.
            let lat = (iy as f32 / height as f32 - 0.5).abs() * 2.0;
            let alt_penalty = (h - 0.45).max(0.0) * 0.6;
            let temperature = (lat * 0.7 + alt_penalty).clamp(0.0, 1.0);

            let (kind, walkable) = if h < 0.25 {
                (TileKind::Water, false)
            } else if h >= MOUNTAIN_THRESHOLD {
                // Impassable cliffs above the snowfield threshold.
                (TileKind::Mountain, false)
            } else if h >= ARCTIC_THRESHOLD {
                // Walkable snowfield — altitude override regardless of temperature.
                (TileKind::Arctic, true)
            } else {
                (classify_biome(temperature, m), true)
            };

            let ix_m = ix.saturating_sub(1);
            let ix_p = (ix + 1).min(width - 1);
            let iy_m = iy.saturating_sub(1);
            let iy_p = (iy + 1).min(height - 1);

            let c_tl = (z_at(ix_m,iy_m)+z_at(ix,iy_m)+z_at(ix_m,iy)+z_at(ix,iy)) / 4.0;
            let c_tr = (z_at(ix,iy_m)+z_at(ix_p,iy_m)+z_at(ix,iy)+z_at(ix_p,iy)) / 4.0;
            let c_bl = (z_at(ix_m,iy)+z_at(ix,iy)+z_at(ix_m,iy_p)+z_at(ix,iy_p)) / 4.0;
            let c_br = (z_at(ix,iy)+z_at(ix_p,iy)+z_at(ix,iy_p)+z_at(ix_p,iy_p)) / 4.0;

            let layer = TileLayer {
                z_base: (z_top - 0.5).max(0.0),
                z_top,
                kind,
                walkable,
                corner_offsets: [c_tl-z_top, c_tr-z_top, c_bl-z_top, c_br-z_top],
            };
            columns.push(TileColumn { layers: vec![layer] });
        }
    }

    // ── River pass ────────────────────────────────────────────────────────────

    river_pass(&mut columns, &heights, width, height);

    WorldMap { columns, width, height, seed, road_tiles: vec![false; width * height], spawn_points: Vec::new(), buildings_stamped: false }
}

/// Steepest-descent river generation.
///
/// # Algorithm
/// 1. For every tile, find the steepest cardinal + diagonal downhill neighbour.
/// 2. Sort tiles height-descending (highlands first).
/// 3. Accumulate drainage area: each tile contributes its count to its
///    downhill neighbour.
/// 4. Walkable surface tiles whose drainage area exceeds [`RIVER_THRESHOLD`]
///    are reclassified as [`TileKind::River`] (non-walkable; requires a boat
///    or bridge to cross).
///
/// Water and Mountain tiles are never converted.
fn river_pass(columns: &mut [TileColumn], heights: &[f32], width: usize, height: usize) {
    // Scale threshold proportionally to map area so rivers appear at any map size.
    // At 512×512 this equals 800 tiles (~0.3% of total); shrinks for smaller maps.
    let river_threshold: u32 =
        ((width * height) as u64 * 800 / (512 * 512)).max(2) as u32;

    let n = width * height;

    // ── Build flow-direction map ──────────────────────────────────────────────
    let mut flow_dir = vec![None::<usize>; n];
    for iy in 0..height {
        for ix in 0..width {
            let h = heights[ix + iy * width];
            let mut best_h = h;
            let mut best_idx = None;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 { continue; }
                    let nx = ix as i32 + dx;
                    let ny = iy as i32 + dy;
                    if nx < 0 || ny < 0
                        || nx as usize >= width
                        || ny as usize >= height
                    {
                        continue;
                    }
                    let nh = heights[nx as usize + ny as usize * width];
                    if nh < best_h {
                        best_h = nh;
                        best_idx = Some(nx as usize + ny as usize * width);
                    }
                }
            }
            flow_dir[ix + iy * width] = best_idx;
        }
    }

    // ── Accumulate drainage area (process highest tiles first) ───────────────
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_unstable_by(|&a, &b| heights[b].partial_cmp(&heights[a]).unwrap_or(std::cmp::Ordering::Equal));

    let mut flow = vec![1u32; n];
    for &idx in &order {
        if let Some(dst) = flow_dir[idx] {
            flow[dst] = flow[dst].saturating_add(flow[idx]);
        }
    }

    // ── Convert high-drainage walkable tiles to River and carve valleys ─────────
    for idx in 0..n {
        if flow[idx] < river_threshold {
            continue;
        }
        let col = &mut columns[idx];
        if let Some(layer) = col.layers.iter_mut().find(|l| {
            l.is_surface_kind()
                && l.walkable
                && !matches!(l.kind, TileKind::Mountain | TileKind::Water)
        }) {
            // Carve a valley: lower z_top proportional to drainage flow so large
            // rivers cut deeper gorges than narrow headwater streams.
            let depth = (flow[idx] as f32 / river_threshold as f32).sqrt().min(3.0) * 1.5;
            layer.z_top  = (layer.z_top - depth).max(0.0);
            layer.z_base = (layer.z_top - 0.5).max(0.0);
            layer.kind = TileKind::River;
            layer.walkable = false;
        }
    }

    // Recompute corner offsets after valley carving modifies z_top values.
    // The surface-pass snapshot is stale; this reads directly from column data.
    recompute_corner_offsets(columns, width, height);
}

/// Re-average each surface layer's `corner_offsets` from the current `z_top` values.
///
/// Must be called after any pass that modifies `z_top` in-place.  The original
/// surface pass computes offsets from a `z_tops` snapshot; this function corrects
/// them by reading the current column state, ensuring seamless mesh edges after
/// river valley carving.
fn recompute_corner_offsets(columns: &mut [TileColumn], width: usize, height: usize) {
    // Snapshot current surface z_top values; void columns default to 0.0.
    let z_tops: Vec<f32> = (0..width * height)
        .map(|idx| {
            columns[idx]
                .layers
                .iter()
                .filter(|l| l.is_surface_kind())
                .map(|l| l.z_top)
                .fold(f32::NEG_INFINITY, f32::max)
                .max(0.0)
        })
        .collect();

    let z_at = |ix: usize, iy: usize| z_tops[ix.min(width - 1) + iy.min(height - 1) * width];

    for iy in 0..height {
        for ix in 0..width {
            let z_top = z_tops[ix + iy * width];
            let ix_m = ix.saturating_sub(1);
            let ix_p = (ix + 1).min(width - 1);
            let iy_m = iy.saturating_sub(1);
            let iy_p = (iy + 1).min(height - 1);

            let c_tl = (z_at(ix_m,iy_m)+z_at(ix,iy_m)+z_at(ix_m,iy)+z_at(ix,iy)) / 4.0;
            let c_tr = (z_at(ix,iy_m)+z_at(ix_p,iy_m)+z_at(ix,iy)+z_at(ix_p,iy)) / 4.0;
            let c_bl = (z_at(ix_m,iy)+z_at(ix,iy)+z_at(ix_m,iy_p)+z_at(ix,iy_p)) / 4.0;
            let c_br = (z_at(ix,iy)+z_at(ix_p,iy)+z_at(ix,iy_p)+z_at(ix_p,iy_p)) / 4.0;

            let col = &mut columns[ix + iy * width];
            if let Some(layer) = col.layers.iter_mut().find(|l| l.is_surface_kind()) {
                layer.corner_offsets = [c_tl-z_top, c_tr-z_top, c_bl-z_top, c_br-z_top];
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_is_deterministic() {
        let a = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let b = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
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
        let m = generate_map(1, MAP_WIDTH, MAP_HEIGHT);
        assert_eq!(m.columns.len(), MAP_WIDTH * MAP_HEIGHT);
    }

    #[test]
    fn surface_layers_have_non_negative_z_top() {
        // Surface-generated kinds (Plains, Forest, etc.) must always be at Z ≥ 0.
        let m = generate_map(7, MAP_WIDTH, MAP_HEIGHT);
        for col in &m.columns {
            for layer in &col.layers {
                if layer.is_surface_kind() {
                    assert!(
                        layer.z_top >= 0.0,
                        "surface layer {:?} has negative z_top {}",
                        layer.kind, layer.z_top
                    );
                }
            }
        }
    }

    #[test]
    fn all_surface_kinds_present() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0u64..5 {
            let m = generate_map(seed, MAP_WIDTH, MAP_HEIGHT);
            for col in &m.columns {
                for layer in &col.layers {
                    seen.insert(layer.kind);
                }
            }
        }
        // Water and Mountain are always present at height extremes.
        assert!(seen.contains(&TileKind::Water),    "no Water tiles");
        assert!(seen.contains(&TileKind::Mountain), "no Mountain tiles");
        // Biome tiles should be present (walkable height range 0.25–0.72).
        let biome_kinds = [
            TileKind::Desert, TileKind::Savanna, TileKind::TropicalForest,
            TileKind::TropicalRainforest, TileKind::Grassland, TileKind::TemperateForest,
            TileKind::Taiga, TileKind::Tundra, TileKind::PolarDesert, TileKind::Arctic,
        ];
        let biome_count = biome_kinds.iter().filter(|k| seen.contains(k)).count();
        assert!(biome_count >= 5,
            "expected ≥5 distinct biome kinds across seeds 0–4, got {biome_count}; seen: {seen:?}");
    }

    #[test]
    fn rivers_are_generated() {
        let m = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let river_count = m.columns.iter()
            .flat_map(|c| c.layers.iter())
            .filter(|l| l.kind == TileKind::River)
            .count();
        // Minimum expected rivers scales with map area (conservative: ~0.02% of tiles).
        let min_rivers = (MAP_WIDTH * MAP_HEIGHT / 5_000).max(5);
        assert!(river_count > min_rivers,
            "expected >{min_rivers} River tiles, got {river_count}");
    }

    #[test]
    fn rivers_are_not_walkable() {
        let m = generate_map(1, MAP_WIDTH, MAP_HEIGHT);
        for col in &m.columns {
            for layer in &col.layers {
                if layer.kind == TileKind::River {
                    assert!(!layer.walkable, "River tile should not be walkable");
                }
            }
        }
    }

    #[test]
    fn classify_biome_covers_all_quadrants() {
        // Verify the biome function returns a non-Mountain, non-Water, walkable kind
        // for all temp/moisture combinations and that multiple biomes are possible.
        let mut seen = std::collections::HashSet::new();
        for ti in 0..=10 {
            for mi in 0..=10 {
                let t = ti as f32 / 10.0;
                let m = mi as f32 / 10.0;
                let k = classify_biome(t, m);
                seen.insert(k);
                // Must be a walkable biome kind (not water or mountain).
                assert!(
                    !matches!(k, TileKind::Water | TileKind::Mountain | TileKind::Void),
                    "classify_biome({t},{m}) returned non-surface kind {k:?}"
                );
            }
        }
        assert!(seen.len() >= 8, "expected ≥8 distinct biomes, got {}", seen.len());
    }

    #[test]
    fn columns_sorted_ascending_by_z_base() {
        let m = generate_map(3, MAP_WIDTH, MAP_HEIGHT);
        for (i, col) in m.columns.iter().enumerate() {
            for pair in col.layers.windows(2) {
                assert!(
                    pair[0].z_base <= pair[1].z_base,
                    "column {i}: layers not sorted — {} > {}",
                    pair[0].z_base, pair[1].z_base
                );
            }
        }
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
        let mut map = WorldMap { columns: vec![TileColumn::default(); MAP_WIDTH * MAP_HEIGHT], width: MAP_WIDTH, height: MAP_HEIGHT, seed: 0, road_tiles: vec![], spawn_points: vec![], buildings_stamped: false };
        map.columns[0] = TileColumn { layers: vec![make_layer(1.0)] };
        map.columns[1] = TileColumn { layers: vec![make_layer(3.0)] };

        // Tile (0,0) center in world-space: x = -(MAP_HALF_WIDTH) + 0.5
        let h = smooth_surface_at(
            &map,
            -(MAP_HALF_WIDTH as f32) + 0.5,
            -(MAP_HALF_HEIGHT as f32) + 0.5,
            5.0,
        ).unwrap();
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
    fn single_layer_terrain_always_reachable() {
        // Forest at z=10.0 with player approaching from z=7.0 (height diff > STEP_HEIGHT).
        // With the old code this would return None; it must now return the forest layer.
        let forest = TileLayer {
            z_base: 8.0, z_top: 10.0, kind: TileKind::TemperateForest,
            walkable: true, corner_offsets: [0.0; 4],
        };
        let col = TileColumn { layers: vec![forest] };
        let layer = col.surface_layer(7.0, STEP_HEIGHT);
        assert!(layer.is_some(), "single-layer forest must be reachable from any entity_z");
        assert_eq!(layer.unwrap().kind, TileKind::TemperateForest);

        // Building (walkable=false) must still be impassable.
        let building = TileLayer {
            z_base: 0.0, z_top: 3.0, kind: TileKind::Plains,
            walkable: false, corner_offsets: [0.0; 4],
        };
        let blocked = TileColumn { layers: vec![building] };
        assert!(blocked.surface_layer(0.0, STEP_HEIGHT).is_none(), "non-walkable tile must block");
    }

    #[test]
    fn surface_height_at_returns_none_over_water() {
        let map = generate_map(0, MAP_WIDTH, MAP_HEIGHT);
        // Find a water tile and confirm surface_height_at returns None.
        // Convert tile indices to world-space (map centered on (0,0)).
        let water_pos = map.columns.iter().enumerate().find_map(|(i, col)| {
            if col.layers.first().map(|l| l.kind == TileKind::Water).unwrap_or(false) {
                let ix = i % MAP_WIDTH;
                let iy = i / MAP_WIDTH;
                Some((
                    ix as f32 + 0.5 - MAP_HALF_WIDTH as f32,
                    iy as f32 + 0.5 - MAP_HALF_HEIGHT as f32,
                ))
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

    #[test]
    fn is_water_at_true_over_water() {
        let map = generate_map(0, MAP_WIDTH, MAP_HEIGHT);
        let water_pos = map.columns.iter().enumerate().find_map(|(i, col)| {
            if col.layers.first().map(|l| l.kind == TileKind::Water).unwrap_or(false) {
                let ix = i % MAP_WIDTH;
                let iy = i / MAP_WIDTH;
                Some((
                    ix as f32 + 0.5 - MAP_HALF_WIDTH as f32,
                    iy as f32 + 0.5 - MAP_HALF_HEIGHT as f32,
                ))
            } else { None }
        });
        if let Some((wx, wy)) = water_pos {
            assert!(is_water_at(&map, wx, wy), "Water tile should report is_water_at=true");
            assert!(water_surface_at(&map, wx, wy).is_some(), "Water tile should have a surface z");
        }
    }

    #[test]
    fn is_water_at_false_over_land() {
        let map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let (x, y, _) = find_surface_spawn(&map);
        assert!(!is_water_at(&map, x, y), "Spawn tile should not be water");
        assert!(water_surface_at(&map, x, y).is_none(), "Spawn tile should have no water surface");
    }

    #[test]
    fn is_walkable_at_false_over_water() {
        let map = generate_map(0, MAP_WIDTH, MAP_HEIGHT);
        // Convert tile indices to world-space (map centered on (0,0)).
        let water_pos = map.columns.iter().enumerate().find_map(|(i, col)| {
            if col.layers.first().map(|l| l.kind == TileKind::Water).unwrap_or(false) {
                let ix = i % MAP_WIDTH;
                let iy = i / MAP_WIDTH;
                Some((
                    ix as f32 + 0.5 - MAP_HALF_WIDTH as f32,
                    iy as f32 + 0.5 - MAP_HALF_HEIGHT as f32,
                ))
            } else {
                None
            }
        });
        if let Some((wx, wy)) = water_pos {
            assert!(!is_walkable_at(&map, wx, wy, 0.0), "Water should not be walkable");
            assert!(!is_walkable_at(&map, wx, wy, 100.0), "Water should not be walkable from above");
        }
    }

    #[test]
    fn is_walkable_at_false_out_of_bounds() {
        let map = generate_map(0, MAP_WIDTH, MAP_HEIGHT);
        // Map spans [-MAP_HALF_WIDTH, MAP_HALF_WIDTH) in world-space.
        assert!(!is_walkable_at(&map, -(MAP_HALF_WIDTH as f32) - 1.0, 0.0, 0.0));
        assert!(!is_walkable_at(&map,   MAP_HALF_WIDTH as f32  + 1.0, 0.0, 0.0));
    }

    #[test]
    fn find_surface_spawn_is_walkable_at_spawn_z() {
        let map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let (x, y, z) = find_surface_spawn(&map);
        // The returned z must be walkable from itself.
        assert!(
            is_walkable_at(&map, x, y, z),
            "spawn ({x:.2}, {y:.2}, {z:.2}) must be walkable from its own z"
        );
        // z must match what smooth_surface_at gives with a high ceiling.
        let expected_z = smooth_surface_at(&map, x, y, Z_SCALE * 2.0).unwrap();
        assert!(
            (z - expected_z).abs() < 1e-3,
            "spawn z {z:.4} should match surface z {expected_z:.4}"
        );
    }

    #[test]
    fn custom_dimensions_stored_correctly() {
        let map = generate_map(42, 64, 32);
        assert_eq!(map.width, 64);
        assert_eq!(map.height, 32);
        assert_eq!(map.columns.len(), 64 * 32);
        assert_eq!(map.road_tiles.len(), 64 * 32);
    }

    // ── Physics constant sanity checks ────────────────────────────────────────

    #[test]
    fn gravity_reaches_terminal_velocity() {
        // After enough ticks, z_vel should clamp to MAX_FALL_SPEED.
        let mut z_vel = 0.0_f32;
        let dt = 1.0 / 62.5;
        for _ in 0..500 {
            z_vel = (z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
        }
        assert!(
            (z_vel - MAX_FALL_SPEED).abs() < 0.01,
            "expected terminal velocity {MAX_FALL_SPEED}, got {z_vel}"
        );
    }

    #[test]
    fn fall_covers_expected_distance() {
        // From Z_SCALE height above terrain (worst-case drop), should land in < 200 ticks (~3.2 s).
        let mut z = Z_SCALE;
        let mut z_vel = 0.0_f32;
        let terrain_z = 0.0_f32;
        let dt = 1.0 / 62.5;
        let mut frames = 0u32;
        while z > terrain_z && frames < 500 {
            z_vel = (z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
            z += z_vel * dt;
            frames += 1;
        }
        assert!(
            frames < 200,
            "expected landing in < 200 ticks from height {Z_SCALE}, took {frames}"
        );
    }

    #[test]
    fn spawn_points_are_walkable_and_open() {
        let mut map = generate_map(42, 64, 64);
        map.spawn_points = generate_spawn_points(&map);
        assert!(!map.spawn_points.is_empty(), "must have at least one spawn point");
        for &(x, y, z) in &map.spawn_points {
            assert!(is_walkable_at(&map, x, y, z), "spawn ({x},{y}) must be walkable");
            let open = [(x + 1., y), (x - 1., y), (x, y + 1.), (x, y - 1.)]
                .iter()
                .filter(|&&(nx, ny)| is_walkable_at(&map, nx, ny, z))
                .count();
            assert!(open >= 2, "spawn at ({x},{y}) must have >= 2 open neighbours, got {open}");
        }
    }

    #[test]
    fn spawn_tile_passable_with_point_bounds() {
        use crate::components::EntityBounds;
        let map = generate_map(42, 64, 64);
        let (sx, sy, sz) = find_surface_spawn(&map);
        assert!(
            is_passable_with_bounds(&map, sx, sy, sz, EntityBounds::POINT),
            "spawn tile must be passable with point bounds"
        );
    }

    #[test]
    fn mountain_tile_blocked_below_peak() {
        use crate::components::EntityBounds;
        let map = generate_map(42, 64, 64);
        let mountain = map.columns.iter().enumerate().find(|(_, col)| {
            col.layers.iter().any(|l| l.kind == TileKind::Mountain)
        });
        if let Some((idx, col)) = mountain {
            let ix = idx % map.width;
            let iy = idx / map.width;
            let x = ix as f32 - (map.width / 2) as f32 + 0.5;
            let y = iy as f32 - (map.height / 2) as f32 + 0.5;
            let z_top = col.layers.iter().map(|l| l.z_top).fold(f32::NEG_INFINITY, f32::max);
            // At the mountain surface level, entity is not above it — blocked.
            assert!(
                !is_passable_with_bounds(&map, x, y, z_top, EntityBounds::POINT),
                "entity at mountain z_top should still be blocked"
            );
        }
    }

    #[test]
    fn entity_above_obstacle_can_pass() {
        use crate::components::EntityBounds;
        let map = generate_map(42, 64, 64);
        let mountain = map.columns.iter().enumerate().find(|(_, col)| {
            col.layers.iter().any(|l| l.kind == TileKind::Mountain)
        });
        if let Some((idx, col)) = mountain {
            let ix = idx % map.width;
            let iy = idx / map.width;
            let x = ix as f32 - (map.width / 2) as f32 + 0.5;
            let y = iy as f32 - (map.height / 2) as f32 + 0.5;
            let z_top = col.layers.iter().map(|l| l.z_top).fold(f32::NEG_INFINITY, f32::max);
            // At ground level: blocked.
            assert!(!is_passable_with_bounds(&map, x, y, 0.0, EntityBounds::POINT));
            // Above mountain top: clear.
            assert!(is_passable_with_bounds(&map, x, y, z_top + 0.1, EntityBounds::POINT));
        }
    }

    #[test]
    fn wide_bounds_blocked_by_adjacent_obstacle() {
        use crate::components::EntityBounds;
        let map = generate_map(42, 64, 64);
        'outer: for iy in 1..(map.height - 1) {
            for ix in 1..(map.width - 1) {
                // Find a walkable tile at its own surface z.
                let col = map.column(ix, iy);
                let surface_z = match col.layers.iter().filter(|l| l.walkable).map(|l| l.z_top).reduce(f32::max) {
                    Some(z) => z,
                    None => continue,
                };
                // Find a non-walkable right neighbour whose max z_top >= surface_z
                // (so aerial clearance does NOT apply when entity is at surface_z).
                let right = map.column(ix + 1, iy);
                let right_max_z = right.layers.iter().map(|l| l.z_top).fold(f32::NEG_INFINITY, f32::max);
                if right_max_z < surface_z { continue; } // aerial clearance would wrongly allow passage
                if right.surface_layer(surface_z, STEP_HEIGHT).is_some() { continue; } // right is walkable — no test
                let cx = ix as f32 - (map.width / 2) as f32 + 0.5;
                let cy = iy as f32 - (map.height / 2) as f32 + 0.5;
                assert!(
                    is_passable_with_bounds(&map, cx, cy, surface_z, EntityBounds::POINT),
                    "point check should pass on walkable centre"
                );
                let wide = EntityBounds { half_w: 0.6, height: 1.8 };
                // Shift centre right so right corners land on the obstacle tile.
                assert!(
                    !is_passable_with_bounds(&map, cx + 0.45, cy, surface_z, wide),
                    "wide bounds should be blocked by adjacent obstacle"
                );
                break 'outer;
            }
        }
    }
}
