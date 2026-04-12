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

/// Multiplier from normalised height `[0, 1]` to world-unit Z for surface terrain.
const Z_SCALE: f32 = 6.0;

// ── Underground depth levels (world units, negative = below ground) ───────────

/// Floor of the shallow cave network: small tunnels, dungeon entrances.
pub const SHALLOW_CAVE_Z: f32  = -15.0;
/// Base of the shallow cave walls (gives them ~7 units of thickness above the floor).
pub const SHALLOW_CAVE_BASE: f32 = -22.0;

/// Floor of the mid-level cave tier: larger passages, underground rivers.
pub const MID_CAVE_Z: f32  = -38.0;
pub const MID_CAVE_BASE: f32 = -46.0;

/// Floor of the Underdark: vast open caverns, bioluminescent fungi, deep cities.
pub const UNDERDARK_Z: f32  = -65.0;
pub const UNDERDARK_BASE: f32 = -80.0;

/// Maximum fall speed in world units per second.
/// Replaces the old hard-coded `2.0 * dt` cap so entities traverse deep shafts
/// in a reasonable time (~1.5 s from surface to Underdark floor).
pub const FALL_SPEED: f32 = 40.0;

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
    // ── Underground ──────────────────────────────────────────────────────────
    /// Generic cave floor — shallow dungeon tier.
    Cavern,
    /// Mid-level cave: larger passages, underground rivers.
    DeepRock,
    /// Underdark floor: vast caverns, bioluminescent fungi, civilization tier.
    LuminousGrotto,
    /// Vertical shaft tile — connects surface to underground levels.
    Tunnel,
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
        Self::Cavern,
        Self::DeepRock,
        Self::LuminousGrotto,
        Self::Tunnel,
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

    /// True for underground-generated kinds (Cavern, DeepRock, LuminousGrotto, Tunnel).
    pub fn is_underground_kind(&self) -> bool {
        matches!(
            self.kind,
            TileKind::Cavern | TileKind::DeepRock | TileKind::LuminousGrotto | TileKind::Tunnel
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
    /// Flat row-major road flag array. `true` = road tile (same indexing as `columns`).
    /// Populated after `generate_map` by [`crate::world::civilization::generate_roads`].
    #[serde(default)]
    pub road_tiles: Vec<bool>,
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

/// Returns `true` if `(x, y)` has a walkable surface (or underground) layer
/// reachable from `current_z` within one step. Returns `false` over Water,
/// River, Mountain, void columns, and out-of-bounds positions.
///
/// Used by the movement system to prevent entities from walking into impassable
/// terrain. See `docs/systems/world-map.md` for the wall-slide behaviour.
pub fn is_walkable_at(map: &WorldMap, x: f32, y: f32, current_z: f32) -> bool {
    smooth_surface_at(map, x, y, current_z).is_some()
}

// ── Biome classification ──────────────────────────────────────────────────────

/// Classify a surface tile using the Whittaker diagram.
///
/// `temperature` — `0.0` (tropical/equator) to `1.0` (polar/cold).  Derived
/// from latitude + altitude penalty.
/// `moisture` — `0.0` (arid) to `1.0` (wet).  From a separate fBm pass.
///
/// High-elevation tiles (h ≥ 0.72) and water tiles (h < 0.25) are handled
/// before this function is called and map directly to `Mountain` / `Water`.
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

/// Generate a world map deterministically from `seed`.
///
/// # Passes
/// 1. **Surface** — fBm terrain noise (6 octaves, continent-scale base frequency)
///    → classify (Water/Plains/Forest/Mountain) → corner-averaged heights for
///    smooth bilinear slopes.
/// 2. **Shallow caves** (Z ≈ -15 to -22) — cellular automata with 48 % initial fill,
///    5 smoothing steps.  Produces winding passages 3–8 tiles wide, similar to
///    dungeon crawl level 1.
/// 3. **Underdark** (Z ≈ -65 to -80) — CA with 30 % initial fill, 3 steps.
///    Produces vast, mostly-open caverns with scattered pillars — city-scale voids
///    suitable for underground civilizations.
pub fn generate_map(seed: u64) -> WorldMap {
    use crate::math::fbm;

    // ── Surface pass ──────────────────────────────────────────────────────────
    // Derive large coordinate offsets so different seeds sample entirely
    // different regions of the infinite fBm noise field.
    let ox  = ((seed.wrapping_mul(2_654_435_761)) % 100_000) as f32;
    let oy  = ((seed.wrapping_mul(805_459_861))   % 100_000) as f32;
    // Separate offset for precipitation noise.
    let mox = ((seed.wrapping_mul(1_234_567_891)) % 100_000) as f32;
    let moy = ((seed.wrapping_mul(987_654_321))   % 100_000) as f32;

    // Base frequency: ~4 cycles across the 512-tile map → continent-scale features.
    const BASE_FREQ: f32 = 4.0 / MAP_WIDTH as f32;
    // Precipitation varies at a finer scale than elevation.
    const MOISTURE_FREQ: f32 = 6.0 / MAP_WIDTH as f32;

    let heights: Vec<f32> = (0..MAP_WIDTH * MAP_HEIGHT)
        .map(|idx| {
            let ix = (idx % MAP_WIDTH) as f32;
            let iy = (idx / MAP_WIDTH) as f32;
            fbm((ix + ox) * BASE_FREQ, (iy + oy) * BASE_FREQ, 6, 0.5, 2.0)
        })
        .collect();

    // Precipitation field: independent fBm pass.
    let moisture: Vec<f32> = (0..MAP_WIDTH * MAP_HEIGHT)
        .map(|idx| {
            let ix = (idx % MAP_WIDTH) as f32;
            let iy = (idx / MAP_WIDTH) as f32;
            fbm((ix + mox) * MOISTURE_FREQ, (iy + moy) * MOISTURE_FREQ, 4, 0.5, 2.0)
        })
        .collect();

    let z_tops: Vec<f32> = heights
        .iter()
        .map(|&h| if h < 0.25 { 0.0 } else { h * Z_SCALE })
        .collect();

    let z_at = |ix: usize, iy: usize| {
        z_tops[ix.min(MAP_WIDTH - 1) + iy.min(MAP_HEIGHT - 1) * MAP_WIDTH]
    };

    let mut columns = Vec::with_capacity(MAP_WIDTH * MAP_HEIGHT);

    for iy in 0..MAP_HEIGHT {
        for ix in 0..MAP_WIDTH {
            let h = heights[ix + iy * MAP_WIDTH];
            let m = moisture[ix + iy * MAP_WIDTH];
            let z_top = z_tops[ix + iy * MAP_WIDTH];

            // Temperature: increases toward equator (center of map), decreases
            // with altitude.  Latitude factor: 0 at equator, 1 at poles.
            let lat = (iy as f32 / MAP_HEIGHT as f32 - 0.5).abs() * 2.0;
            let alt_penalty = (h - 0.45).max(0.0) * 0.6;
            let temperature = (lat * 0.7 + alt_penalty).clamp(0.0, 1.0);

            let (kind, walkable) = if h < 0.25 {
                (TileKind::Water, false)
            } else if h >= 0.72 {
                (TileKind::Mountain, false)
            } else {
                (classify_biome(temperature, m), true)
            };

            let ix_m = ix.saturating_sub(1);
            let ix_p = (ix + 1).min(MAP_WIDTH - 1);
            let iy_m = iy.saturating_sub(1);
            let iy_p = (iy + 1).min(MAP_HEIGHT - 1);

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

    river_pass(&mut columns, &heights);

    // ── Shallow cave pass ─────────────────────────────────────────────────────

    cave_pass(
        &mut columns,
        seed.wrapping_add(1),
        CaveParams {
            fill_chance: 0.48,
            ca_steps: 5,
            solid_threshold: 5, // standard cave rule
            floor_z: SHALLOW_CAVE_Z,
            base_z: SHALLOW_CAVE_BASE,
            kind: TileKind::Cavern,
        },
    );

    // ── Underdark pass ────────────────────────────────────────────────────────
    // 30 % initial fill + loose rule = vast open spaces with scattered pillars.

    cave_pass(
        &mut columns,
        seed.wrapping_add(2),
        CaveParams {
            fill_chance: 0.30,
            ca_steps: 3,
            solid_threshold: 6, // looser: only close off when 6+ of 8 are solid
            floor_z: UNDERDARK_Z,
            base_z: UNDERDARK_BASE,
            kind: TileKind::LuminousGrotto,
        },
    );

    // ── Shaft pass ────────────────────────────────────────────────────────────

    shaft_pass(&mut columns, seed.wrapping_add(3));

    WorldMap { columns, seed, road_tiles: vec![false; MAP_WIDTH * MAP_HEIGHT] }
}

/// Parameters for one underground generation pass.
struct CaveParams {
    /// Initial probability that a cell starts as solid rock (0.0–1.0).
    fill_chance: f32,
    /// Number of cellular automata smoothing steps.
    ca_steps: usize,
    /// Number of solid 8-neighbours required for a cell to remain solid.
    solid_threshold: u32,
    /// z_top of the walkable floor layer added to void cells.
    floor_z: f32,
    /// z_base of the walkable floor layer (thickness = floor_z - base_z).
    base_z: f32,
    /// [`TileKind`] assigned to the generated layers.
    kind: TileKind,
}

/// Cellular-automata cave generation.
///
/// Initialises a boolean solid/void grid with seeded noise, runs `ca_steps`
/// smoothing passes, then appends a walkable [`TileLayer`] at `floor_z` to
/// every void cell in `columns`.
fn cave_pass(columns: &mut [TileColumn], seed: u64, p: CaveParams) {
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    // true = solid rock, false = open void
    let mut solid: Vec<bool> = (0..MAP_WIDTH * MAP_HEIGHT)
        .map(|_| rng.random::<f32>() < p.fill_chance)
        .collect();

    for _ in 0..p.ca_steps {
        let prev = solid.clone();
        for iy in 0..MAP_HEIGHT {
            for ix in 0..MAP_WIDTH {
                let mut neighbors: u32 = 0;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        if dx == 0 && dy == 0 { continue; }
                        let nx = ix as i32 + dx;
                        let ny = iy as i32 + dy;
                        if nx < 0 || ny < 0 || nx >= MAP_WIDTH as i32 || ny >= MAP_HEIGHT as i32 {
                            neighbors += 1; // out-of-bounds counts as solid wall
                        } else {
                            neighbors += prev[nx as usize + ny as usize * MAP_WIDTH] as u32;
                        }
                    }
                }
                solid[ix + iy * MAP_WIDTH] = neighbors >= p.solid_threshold;
            }
        }
    }

    // Add a walkable layer to every open cell.
    for iy in 0..MAP_HEIGHT {
        for ix in 0..MAP_WIDTH {
            if !solid[ix + iy * MAP_WIDTH] {
                let layer = TileLayer {
                    z_base: p.base_z,
                    z_top: p.floor_z,
                    kind: p.kind,
                    walkable: true,
                    corner_offsets: [0.0; 4], // underground floors are flat
                };
                columns[ix + iy * MAP_WIDTH].layers.push(layer);
                // Keep sorted ascending by z_base (deepest first).
                columns[ix + iy * MAP_WIDTH]
                    .layers
                    .sort_by(|a, b| a.z_base.partial_cmp(&b.z_base).unwrap());
            }
        }
    }
}

/// Generate vertical shafts that connect walkable surface tiles to underground levels.
///
/// # Algorithm
/// 1. Collect all columns that have BOTH a walkable surface layer AND at least one
///    underground layer (Cavern or LuminousGrotto).  These are candidate shaft sites.
/// 2. Thin the candidates: keep only every `SHAFT_SPACING`-th candidate (via the
///    seeded RNG) so shafts appear periodically rather than everywhere.
/// 3. For each selected shaft column, add a `Tunnel` layer between the surface
///    floor and the shallowest underground layer.  The tunnel is walkable so the
///    movement system treats it as a sloped transition.
///
/// The shaft layer bridges the Z gap: `z_base = underground_floor`, `z_top =
/// surface_floor`.  Entities entering the shaft column smoothly lerp down to the
/// underground floor because that is now the highest reachable walkable layer
/// within `STEP_HEIGHT` of the surface (the surface layer is not removed — the
/// shaft sits beside it, and the game can use trigger volumes to enter/exit).
///
/// **No surface layer is removed**: shafts are additive.  A shaft column has both
/// the original surface layer and the tunnel layer; which one the entity stands on
/// depends on their current Z.
fn shaft_pass(columns: &mut [TileColumn], seed: u64) {
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    /// Roughly 1 shaft per SHAFT_SPACING columns (on average).
    const SHAFT_SPACING: usize = 40;

    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    for col in columns.iter_mut() {
        // Decide whether to place a shaft here (~1/SHAFT_SPACING chance).
        if (rng.random::<f32>() * SHAFT_SPACING as f32) >= 1.0 {
            continue;
        }

        // Must have a walkable surface layer.
        let surface_z = match col.layers.iter().find(|l| l.is_surface_kind() && l.walkable) {
            Some(l) => l.z_top,
            None => continue,
        };

        // Must have at least one underground walkable layer.
        let underground_z = match col
            .layers
            .iter()
            .filter(|l| l.is_underground_kind() && l.walkable)
            .map(|l| l.z_top)
            // Pick the shallowest underground floor (highest negative Z).
            .reduce(f32::max)
        {
            Some(z) => z,
            None => continue,
        };

        // Add the tunnel layer bridging surface → underground.
        let shaft = TileLayer {
            z_base: underground_z,
            z_top: surface_z,
            kind: TileKind::Tunnel,
            walkable: true,
            corner_offsets: [0.0; 4],
        };
        col.layers.push(shaft);
        // Re-sort to maintain ascending z_base order.
        col.layers.sort_by(|a, b| a.z_base.partial_cmp(&b.z_base).unwrap());
    }
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
fn river_pass(columns: &mut [TileColumn], heights: &[f32]) {
    const RIVER_THRESHOLD: u32 = 800; // tiles feeding into a cell for it to become a river

    let n = MAP_WIDTH * MAP_HEIGHT;

    // ── Build flow-direction map ──────────────────────────────────────────────
    let mut flow_dir = vec![None::<usize>; n];
    for iy in 0..MAP_HEIGHT {
        for ix in 0..MAP_WIDTH {
            let h = heights[ix + iy * MAP_WIDTH];
            let mut best_h = h;
            let mut best_idx = None;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 { continue; }
                    let nx = ix as i32 + dx;
                    let ny = iy as i32 + dy;
                    if nx < 0 || ny < 0
                        || nx as usize >= MAP_WIDTH
                        || ny as usize >= MAP_HEIGHT
                    {
                        continue;
                    }
                    let nh = heights[nx as usize + ny as usize * MAP_WIDTH];
                    if nh < best_h {
                        best_h = nh;
                        best_idx = Some(nx as usize + ny as usize * MAP_WIDTH);
                    }
                }
            }
            flow_dir[ix + iy * MAP_WIDTH] = best_idx;
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

    // ── Convert high-drainage walkable tiles to River ─────────────────────────
    for idx in 0..n {
        if flow[idx] < RIVER_THRESHOLD {
            continue;
        }
        let col = &mut columns[idx];
        if let Some(layer) = col.layers.iter_mut().find(|l| {
            l.is_surface_kind()
                && l.walkable
                && !matches!(l.kind, TileKind::Mountain | TileKind::Water)
        }) {
            layer.kind = TileKind::River;
            layer.walkable = false;
        }
    }
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
    fn surface_layers_have_non_negative_z_top() {
        // Surface-generated kinds (Plains, Forest, etc.) must always be at Z ≥ 0.
        // Underground kinds (Cavern, Tunnel, …) are legitimately negative.
        let m = generate_map(7);
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
    fn depth_constants_are_ordered() {
        // Verify that depth constants form a coherent descending stack.
        assert!(SHALLOW_CAVE_Z  > MID_CAVE_Z,  "shallow must be above mid");
        assert!(MID_CAVE_Z      > UNDERDARK_Z, "mid must be above underdark");
        assert!(SHALLOW_CAVE_Z  > SHALLOW_CAVE_BASE);
        assert!(MID_CAVE_Z      > MID_CAVE_BASE);
        assert!(UNDERDARK_Z     > UNDERDARK_BASE);
    }

    #[test]
    fn all_surface_kinds_present() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0u64..5 {
            let m = generate_map(seed);
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
        let m = generate_map(42);
        let river_count = m.columns.iter()
            .flat_map(|c| c.layers.iter())
            .filter(|l| l.kind == TileKind::River)
            .count();
        assert!(river_count > 100,
            "expected >100 River tiles, got {river_count}");
    }

    #[test]
    fn rivers_are_not_walkable() {
        let m = generate_map(1);
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
    fn underground_layers_generated() {
        let m = generate_map(0);

        let (mut cavern_count, mut grotto_count) = (0usize, 0usize);
        for col in &m.columns {
            for layer in &col.layers {
                match layer.kind {
                    TileKind::Cavern        => cavern_count += 1,
                    TileKind::LuminousGrotto => grotto_count += 1,
                    _ => {}
                }
            }
        }

        // With 512×512 and ~52% void for shallow caves and ~70% for Underdark,
        // we expect a very large number of each kind.
        assert!(cavern_count > 50_000,
            "expected many Cavern tiles, got {cavern_count}");
        assert!(grotto_count > 100_000,
            "expected many LuminousGrotto tiles, got {grotto_count}");
    }

    #[test]
    fn underground_layers_have_negative_z_top() {
        let m = generate_map(5);
        let mut found_cavern = false;
        let mut found_grotto = false;
        for col in &m.columns {
            for layer in &col.layers {
                if layer.kind == TileKind::Cavern {
                    assert!(layer.z_top < 0.0,
                        "Cavern z_top should be negative, got {}", layer.z_top);
                    found_cavern = true;
                }
                if layer.kind == TileKind::LuminousGrotto {
                    assert!(layer.z_top < 0.0,
                        "LuminousGrotto z_top should be negative, got {}", layer.z_top);
                    assert!(layer.z_top <= SHALLOW_CAVE_Z,
                        "Underdark should be deeper than shallow caves");
                    found_grotto = true;
                }
            }
        }
        assert!(found_cavern, "no Cavern layers in generated map");
        assert!(found_grotto, "no LuminousGrotto layers in generated map");
    }

    #[test]
    fn columns_sorted_ascending_by_z_base() {
        let m = generate_map(3);
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
    fn surface_and_underground_can_coexist_in_column() {
        let m = generate_map(0);
        let has_both = m.columns.iter().any(|col| {
            col.layers.iter().any(|l| l.is_surface_kind())
                && col.layers.iter().any(|l| l.is_underground_kind())
        });
        assert!(has_both, "no column has both surface and underground layers");
    }

    #[test]
    fn shafts_are_generated() {
        let m = generate_map(0);
        let shaft_count = m.columns.iter()
            .flat_map(|c| c.layers.iter())
            .filter(|l| l.kind == TileKind::Tunnel)
            .count();
        // With ~1/40 chance per eligible column, expect many thousands of shafts.
        assert!(shaft_count > 1_000,
            "expected >1000 shaft layers, got {shaft_count}");
    }

    #[test]
    fn shaft_z_top_equals_surface_z() {
        // Every Tunnel layer's z_top should equal a walkable surface layer's z_top
        // in the same column, and z_base should equal an underground layer's z_top.
        let m = generate_map(2);
        for col in &m.columns {
            for shaft in col.layers.iter().filter(|l| l.kind == TileKind::Tunnel) {
                let surface_match = col.layers.iter()
                    .any(|l| l.is_surface_kind() && l.walkable
                        && (l.z_top - shaft.z_top).abs() < 0.01);
                let underground_match = col.layers.iter()
                    .any(|l| l.is_underground_kind() && l.walkable
                        && (l.z_top - shaft.z_base).abs() < 0.01);
                assert!(surface_match,
                    "shaft z_top={} has no matching surface layer", shaft.z_top);
                assert!(underground_match,
                    "shaft z_base={} has no matching underground layer", shaft.z_base);
            }
        }
    }

    #[test]
    fn shaft_column_remains_sorted() {
        // Shaft pass must not break z_base sort order.
        let m = generate_map(7);
        for (i, col) in m.columns.iter().enumerate() {
            for pair in col.layers.windows(2) {
                assert!(
                    pair[0].z_base <= pair[1].z_base,
                    "column {i} unsorted after shaft pass: {} > {}",
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
        let mut map = WorldMap { columns: vec![TileColumn::default(); MAP_WIDTH * MAP_HEIGHT], seed: 0, road_tiles: vec![] };
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

    #[test]
    fn is_walkable_at_false_over_water() {
        let map = generate_map(0);
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
            assert!(!is_walkable_at(&map, wx, wy, 0.0), "Water should not be walkable");
            assert!(!is_walkable_at(&map, wx, wy, 100.0), "Water should not be walkable from above");
        }
    }

    #[test]
    fn is_walkable_at_false_out_of_bounds() {
        let map = generate_map(0);
        assert!(!is_walkable_at(&map, -1.0, 0.0, 0.0));
        assert!(!is_walkable_at(&map, MAP_WIDTH as f32 + 1.0, 0.0, 0.0));
    }
}
