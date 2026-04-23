//! Static navigation grid for A* and flow-field pathfinding.
//!
//! Built once at world-gen time by downsampling the full 1024×1024 tile map 4:1.
//! Each 4×4 tile block collapses to one [`NavCell`] (worst-case wins).
//! Fits in 64 KB (256×256 × 1 byte) and is fully static post-gen.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::map::{TileKind, WorldMap};

pub const NAV_GRID_WIDTH: usize = 256;
pub const NAV_GRID_HEIGHT: usize = 256;
/// Tile-to-nav-cell downsample factor (4 tiles → 1 nav cell on each axis).
pub const NAV_DOWNSAMPLE: usize = 4;

/// Pathfinding cost of one nav grid cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum NavCell {
    /// Traversable at normal speed.
    #[default]
    Passable,
    /// Traversable at reduced speed (dense vegetation, etc.).
    Slow,
    /// Impassable (water, mountain, river, void, buildings).
    Blocked,
}

impl NavCell {
    /// Pessimistic merge: Blocked > Slow > Passable.
    ///
    /// Used to collapse a 4×4 tile block: if any tile is impassable the whole
    /// cell becomes Blocked; if any is slow but none are blocked it becomes Slow.
    pub fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Blocked, _) | (_, Self::Blocked) => Self::Blocked,
            (Self::Slow, _) | (_, Self::Slow) => Self::Slow,
            _ => Self::Passable,
        }
    }
}

impl TileKind {
    /// Nav cost for this tile kind, ignoring per-tile walkability flags.
    ///
    /// Callers should check walkability separately and return `NavCell::Blocked`
    /// for non-walkable tiles regardless of kind (e.g. buildings on forest).
    pub fn nav_cell(self) -> NavCell {
        match self {
            TileKind::Mountain | TileKind::Water | TileKind::River | TileKind::Void => {
                NavCell::Blocked
            }
            TileKind::Forest
            | TileKind::TropicalForest
            | TileKind::TropicalRainforest
            | TileKind::TemperateForest
            | TileKind::Taiga => NavCell::Slow,
            TileKind::Plains
            | TileKind::Stone
            | TileKind::Desert
            | TileKind::Savanna
            | TileKind::Grassland
            | TileKind::Tundra
            | TileKind::PolarDesert
            | TileKind::Arctic => NavCell::Passable,
        }
    }
}

/// Downsampled 256×256 navigation grid built from the world tile map.
///
/// Each cell covers `NAV_DOWNSAMPLE × NAV_DOWNSAMPLE` world tiles.  The grid
/// is fully static after world-gen — terrain does not change post-generation.
/// Building footprints are baked in because [`NavGrid::build_from`] must be
/// called after `apply_building_tiles`.
///
/// Query terrain cost with [`NavGrid::nav_cell_at`].
#[derive(Clone, Debug, Serialize, Deserialize, Reflect, Resource)]
#[reflect(Resource)]
pub struct NavGrid {
    /// Flat row-major array: `NAV_GRID_WIDTH * NAV_GRID_HEIGHT` entries.
    /// Index: `nx + ny * NAV_GRID_WIDTH`.
    cells: Vec<NavCell>,
    /// World-map half-width (tiles), stored for coordinate conversion.
    map_half_width: i64,
    /// World-map half-height (tiles), stored for coordinate conversion.
    map_half_height: i64,
}

impl NavGrid {
    /// Build a [`NavGrid`] from a fully-stamped [`WorldMap`].
    ///
    /// Must be called **after** `apply_building_tiles` so building footprints
    /// are baked into the grid as `Blocked` cells.
    pub fn build_from(map: &WorldMap) -> Self {
        let map_half_width = (map.width / 2) as i64;
        let map_half_height = (map.height / 2) as i64;

        let mut cells = Vec::with_capacity(NAV_GRID_WIDTH * NAV_GRID_HEIGHT);
        for ny in 0..NAV_GRID_HEIGHT {
            for nx in 0..NAV_GRID_WIDTH {
                cells.push(sample_block(map, nx, ny));
            }
        }

        Self { cells, map_half_width, map_half_height }
    }

    /// Return the [`NavCell`] whose block contains world-space `(world_x, world_y)`.
    ///
    /// Returns [`NavCell::Blocked`] for out-of-bounds coordinates.
    pub fn nav_cell_at(&self, world_x: f32, world_y: f32) -> NavCell {
        let nx = (world_x.floor() as i64 + self.map_half_width) / NAV_DOWNSAMPLE as i64;
        let ny = (world_y.floor() as i64 + self.map_half_height) / NAV_DOWNSAMPLE as i64;
        if nx < 0 || ny < 0 || nx as usize >= NAV_GRID_WIDTH || ny as usize >= NAV_GRID_HEIGHT {
            return NavCell::Blocked;
        }
        self.cells[nx as usize + ny as usize * NAV_GRID_WIDTH]
    }
}

/// Collapse a 4×4 tile block starting at nav cell `(nx, ny)` to a single [`NavCell`].
///
/// Worst-case wins: Blocked > Slow > Passable.  Non-walkable tiles (water,
/// mountain, river, buildings) always contribute Blocked regardless of kind.
/// Out-of-bounds tile positions are treated as Blocked.
fn sample_block(map: &WorldMap, nx: usize, ny: usize) -> NavCell {
    let mut cell = NavCell::Passable;
    'outer: for dy in 0..NAV_DOWNSAMPLE {
        for dx in 0..NAV_DOWNSAMPLE {
            let ix = nx * NAV_DOWNSAMPLE + dx;
            let iy = ny * NAV_DOWNSAMPLE + dy;
            if ix >= map.width || iy >= map.height {
                return NavCell::Blocked;
            }
            let col = map.column(ix, iy);
            let tile_cell = match col.layers.last() {
                None => NavCell::Blocked,
                Some(layer) if !layer.walkable => NavCell::Blocked,
                Some(layer) => layer.kind.nav_cell(),
            };
            cell = cell.merge(tile_cell);
            if cell == NavCell::Blocked {
                break 'outer;
            }
        }
    }
    cell
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::map::{TileColumn, TileLayer, WorldMap};

    fn make_map(width: usize, height: usize, kind: TileKind, walkable: bool) -> WorldMap {
        let layer = TileLayer {
            z_base: 0.0,
            z_top: 1.0,
            kind,
            walkable,
            corner_offsets: [0.0; 4],
        };
        let column = TileColumn { layers: vec![layer] };
        WorldMap {
            columns: vec![column; width * height],
            width,
            height,
            seed: 0,
            road_tiles: vec![false; width * height],
            spawn_points: Vec::new(),
            buildings_stamped: false,
        }
    }

    #[test]
    fn tile_kind_nav_cell_blocked() {
        assert_eq!(TileKind::Mountain.nav_cell(), NavCell::Blocked);
        assert_eq!(TileKind::Water.nav_cell(), NavCell::Blocked);
        assert_eq!(TileKind::River.nav_cell(), NavCell::Blocked);
        assert_eq!(TileKind::Void.nav_cell(), NavCell::Blocked);
    }

    #[test]
    fn tile_kind_nav_cell_slow() {
        assert_eq!(TileKind::Forest.nav_cell(), NavCell::Slow);
        assert_eq!(TileKind::TropicalForest.nav_cell(), NavCell::Slow);
        assert_eq!(TileKind::TropicalRainforest.nav_cell(), NavCell::Slow);
        assert_eq!(TileKind::TemperateForest.nav_cell(), NavCell::Slow);
        assert_eq!(TileKind::Taiga.nav_cell(), NavCell::Slow);
    }

    #[test]
    fn tile_kind_nav_cell_passable() {
        assert_eq!(TileKind::Plains.nav_cell(), NavCell::Passable);
        assert_eq!(TileKind::Stone.nav_cell(), NavCell::Passable);
        assert_eq!(TileKind::Desert.nav_cell(), NavCell::Passable);
        assert_eq!(TileKind::Savanna.nav_cell(), NavCell::Passable);
        assert_eq!(TileKind::Grassland.nav_cell(), NavCell::Passable);
        assert_eq!(TileKind::Tundra.nav_cell(), NavCell::Passable);
        assert_eq!(TileKind::PolarDesert.nav_cell(), NavCell::Passable);
        assert_eq!(TileKind::Arctic.nav_cell(), NavCell::Passable);
    }

    #[test]
    fn nav_cell_covers_all_tile_kinds() {
        for &kind in TileKind::ALL {
            let _ = kind.nav_cell();
        }
    }

    #[test]
    fn nav_grid_passable_plains_map() {
        let map = make_map(1024, 1024, TileKind::Plains, true);
        let grid = NavGrid::build_from(&map);
        assert_eq!(grid.nav_cell_at(0.0, 0.0), NavCell::Passable);
        assert_eq!(grid.nav_cell_at(-511.0, -511.0), NavCell::Passable);
        assert_eq!(grid.nav_cell_at(511.0, 511.0), NavCell::Passable);
    }

    #[test]
    fn nav_grid_blocked_water_map() {
        let map = make_map(1024, 1024, TileKind::Water, false);
        let grid = NavGrid::build_from(&map);
        assert_eq!(grid.nav_cell_at(0.0, 0.0), NavCell::Blocked);
    }

    #[test]
    fn nav_grid_slow_forest_map() {
        let map = make_map(1024, 1024, TileKind::Forest, true);
        let grid = NavGrid::build_from(&map);
        assert_eq!(grid.nav_cell_at(0.0, 0.0), NavCell::Slow);
    }

    #[test]
    fn nav_grid_out_of_bounds_is_blocked() {
        let map = make_map(1024, 1024, TileKind::Plains, true);
        let grid = NavGrid::build_from(&map);
        assert_eq!(grid.nav_cell_at(9999.0, 9999.0), NavCell::Blocked);
        assert_eq!(grid.nav_cell_at(-9999.0, -9999.0), NavCell::Blocked);
    }

    #[test]
    fn nav_grid_non_walkable_tile_is_blocked() {
        // walkable=false overrides the tile kind (e.g. building on plains).
        let map = make_map(1024, 1024, TileKind::Plains, false);
        let grid = NavGrid::build_from(&map);
        assert_eq!(grid.nav_cell_at(0.0, 0.0), NavCell::Blocked);
    }

    #[test]
    fn nav_cell_merge() {
        assert_eq!(NavCell::Passable.merge(NavCell::Slow), NavCell::Slow);
        assert_eq!(NavCell::Slow.merge(NavCell::Blocked), NavCell::Blocked);
        assert_eq!(NavCell::Passable.merge(NavCell::Blocked), NavCell::Blocked);
        assert_eq!(NavCell::Passable.merge(NavCell::Passable), NavCell::Passable);
    }
}
