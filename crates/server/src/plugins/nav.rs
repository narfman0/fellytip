//! Static navigation grid for pathfinding.
//!
//! The 1024×1024 world tile map is downsampled 4:1 to a 256×256 `NavGrid`.
//! Each cell stores a `NavCell` passability class used by A* and flow-field systems.

use bevy::prelude::*;
use fellytip_shared::world::map::{TileKind, WorldMap, MAP_HALF_HEIGHT, MAP_HALF_WIDTH};

pub const NAV_WIDTH: usize = 256;
pub const NAV_HEIGHT: usize = 256;
/// Downsample factor: 1024 / 256 = 4
const DOWNSAMPLE: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavCell {
    Passable,
    Slow,
    Blocked,
}

impl NavCell {
    pub fn movement_cost(self) -> f32 {
        match self {
            NavCell::Passable => 1.0,
            NavCell::Slow => 2.0,
            NavCell::Blocked => f32::MAX,
        }
    }
}

fn tile_kind_to_nav_cell(kind: TileKind) -> NavCell {
    match kind {
        TileKind::Water | TileKind::Mountain | TileKind::River => NavCell::Blocked,
        TileKind::Forest | TileKind::TropicalForest | TileKind::TemperateForest | TileKind::Taiga => NavCell::Slow,
        _ => NavCell::Passable,
    }
}

/// 256×256 navigation grid built from the world tile map.
#[derive(Resource)]
pub struct NavGrid {
    pub cells: Vec<NavCell>,
}

impl NavGrid {
    pub fn build(map: &WorldMap) -> Self {
        let mut cells = Vec::with_capacity(NAV_WIDTH * NAV_HEIGHT);
        for ny in 0..NAV_HEIGHT {
            for nx in 0..NAV_WIDTH {
                // Sample the first tile of each 4×4 block (majority would be more accurate
                // but single-sample is cheap and sufficient for AI pathfinding).
                let tx = nx * DOWNSAMPLE;
                let ty = ny * DOWNSAMPLE;
                let col = map.column(tx.min(map.width - 1), ty.min(map.height - 1));
                let cell = col.layers.first()
                    .map(|l| {
                        if !l.walkable {
                            // Non-walkable includes Water, Mountain, River, impassable buildings.
                            NavCell::Blocked
                        } else {
                            tile_kind_to_nav_cell(l.kind)
                        }
                    })
                    .unwrap_or(NavCell::Blocked);
                cells.push(cell);
            }
        }
        NavGrid { cells }
    }

    #[inline]
    pub fn nav_cell_at(&self, world_x: f32, world_y: f32) -> NavCell {
        let (nx, ny) = world_to_nav(world_x, world_y);
        self.cell(nx, ny)
    }

    #[inline]
    pub fn passability_at(&self, world_x: f32, world_y: f32) -> f32 {
        self.nav_cell_at(world_x, world_y).movement_cost()
    }

    #[inline]
    pub fn cell(&self, nx: usize, ny: usize) -> NavCell {
        if nx >= NAV_WIDTH || ny >= NAV_HEIGHT {
            return NavCell::Blocked;
        }
        self.cells[nx + ny * NAV_WIDTH]
    }
}

/// Convert world-space coordinates to nav-grid cell indices.
#[inline]
pub fn world_to_nav(world_x: f32, world_y: f32) -> (usize, usize) {
    let tile_x = (world_x + MAP_HALF_WIDTH as f32) as i32;
    let tile_y = (world_y + MAP_HALF_HEIGHT as f32) as i32;
    let nx = (tile_x / DOWNSAMPLE as i32).clamp(0, NAV_WIDTH as i32 - 1) as usize;
    let ny = (tile_y / DOWNSAMPLE as i32).clamp(0, NAV_HEIGHT as i32 - 1) as usize;
    (nx, ny)
}

/// Convert nav-grid cell indices to world-space centre coordinates.
#[inline]
pub fn nav_to_world(nx: usize, ny: usize) -> (f32, f32) {
    let tile_x = nx * DOWNSAMPLE + DOWNSAMPLE / 2;
    let tile_y = ny * DOWNSAMPLE + DOWNSAMPLE / 2;
    (
        tile_x as f32 - MAP_HALF_WIDTH as f32,
        tile_y as f32 - MAP_HALF_HEIGHT as f32,
    )
}

pub struct NavPlugin;

impl Plugin for NavPlugin {
    fn build(&self, _app: &mut App) {
        // NavGrid is inserted as a resource by build_nav_grid, called from MapGenPlugin.
    }
}

/// Build the NavGrid from the WorldMap and insert it as a resource.
/// Must run after generate_world inserts the WorldMap.
pub fn build_nav_grid(map: Res<WorldMap>, mut commands: Commands) {
    let nav = NavGrid::build(&map);
    tracing::info!(
        cells = nav.cells.len(),
        blocked = nav.cells.iter().filter(|&&c| c == NavCell::Blocked).count(),
        slow = nav.cells.iter().filter(|&&c| c == NavCell::Slow).count(),
        "NavGrid built"
    );
    commands.insert_resource(nav);
}
