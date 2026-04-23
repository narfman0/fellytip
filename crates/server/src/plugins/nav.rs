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

impl NavGrid {
    /// A* from `start` to `goal` on the 256×256 nav grid.
    ///
    /// Returns a list of direction-change waypoints (not every cell).
    /// Returns `None` if no path exists.
    pub fn astar(&self, start: (usize, usize), goal: (usize, usize)) -> Option<Vec<(u16, u16)>> {
        use std::collections::BinaryHeap;
        use std::cmp::Reverse;

        if start == goal {
            return Some(vec![(goal.0 as u16, goal.1 as u16)]);
        }

        let cell_count = NAV_WIDTH * NAV_HEIGHT;
        let idx = |x: usize, y: usize| x + y * NAV_WIDTH;

        let mut g_score = vec![f32::MAX; cell_count];
        let mut came_from = vec![usize::MAX; cell_count];
        let mut open: BinaryHeap<Reverse<(u32, usize)>> = BinaryHeap::new();

        let start_idx = idx(start.0, start.1);
        g_score[start_idx] = 0.0;
        let h = heuristic(start, goal);
        open.push(Reverse((float_to_ord(h), start_idx)));

        while let Some(Reverse((_, cur_idx))) = open.pop() {
            let cur_x = cur_idx % NAV_WIDTH;
            let cur_y = cur_idx / NAV_WIDTH;

            if (cur_x, cur_y) == goal {
                return Some(reconstruct_path(&came_from, cur_idx, start_idx));
            }

            let cur_g = g_score[cur_idx];

            for (nx, ny) in neighbors(cur_x, cur_y) {
                let cell = self.cell(nx, ny);
                let cost = cell.movement_cost();
                if cost == f32::MAX {
                    continue;
                }
                let n_idx = idx(nx, ny);
                let tentative_g = cur_g + cost;
                if tentative_g < g_score[n_idx] {
                    g_score[n_idx] = tentative_g;
                    came_from[n_idx] = cur_idx;
                    let f = tentative_g + heuristic((nx, ny), goal);
                    open.push(Reverse((float_to_ord(f), n_idx)));
                }
            }
        }

        None
    }
}

fn heuristic(a: (usize, usize), b: (usize, usize)) -> f32 {
    let dx = (a.0 as i32 - b.0 as i32).abs();
    let dy = (a.1 as i32 - b.1 as i32).abs();
    (dx + dy) as f32
}

fn float_to_ord(f: f32) -> u32 {
    // Map f32 to a u32 that preserves ordering for non-negative values.
    f.to_bits()
}

fn neighbors(x: usize, y: usize) -> impl Iterator<Item = (usize, usize)> {
    let mut buf = [(0usize, 0usize); 4];
    let mut n = 0usize;
    if x + 1 < NAV_WIDTH  { buf[n] = (x + 1, y); n += 1; }
    if x > 0              { buf[n] = (x - 1, y); n += 1; }
    if y + 1 < NAV_HEIGHT { buf[n] = (x, y + 1); n += 1; }
    if y > 0              { buf[n] = (x, y - 1); n += 1; }
    buf.into_iter().take(n)
}

/// Reconstruct path from `came_from` table, keeping only direction-change waypoints.
fn reconstruct_path(came_from: &[usize], mut cur: usize, start: usize) -> Vec<(u16, u16)> {
    let mut full = Vec::new();
    while cur != start {
        full.push(cur);
        let prev = came_from[cur];
        if prev == usize::MAX { break; }
        cur = prev;
    }
    full.reverse();

    // Compress: keep waypoints where direction changes.
    let mut waypoints = Vec::new();
    let mut prev_dir: Option<(i32, i32)> = None;
    for &cell_idx in &full {
        let x = (cell_idx % NAV_WIDTH) as u16;
        let y = (cell_idx / NAV_WIDTH) as u16;
        // Compute direction from previous waypoint.
        if let Some(last) = waypoints.last().copied() {
            let (lx, ly): (u16, u16) = last;
            let dir = (x as i32 - lx as i32, y as i32 - ly as i32);
            if Some(dir) != prev_dir {
                waypoints.push((x, y));
                prev_dir = Some(dir);
            }
        } else {
            waypoints.push((x, y));
            prev_dir = None;
        }
    }
    waypoints
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
