//! Static navigation grid for pathfinding.
//!
//! The 1024×1024 world tile map is downsampled 4:1 to a 256×256 `NavGrid`.
//! Each cell stores a `NavCell` passability class used by A* and flow-field systems.

use bevy::prelude::*;
use fellytip_shared::world::{
    grid::Grid,
    map::{TileKind, WorldMap, MAP_HALF_HEIGHT, MAP_HALF_WIDTH},
};

pub const NAV_WIDTH: usize = 256;
pub const NAV_HEIGHT: usize = 256;
/// Downsample factor: 1024 / 256 = 4
const DOWNSAMPLE: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NavCell {
    #[default]
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
    pub grid: Grid<NavCell>,
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
        NavGrid {
            grid: Grid::from_cells(NAV_WIDTH, NAV_HEIGHT, cells),
        }
    }

    /// Back-compat view of the underlying cell slice.
    #[inline]
    pub fn cells(&self) -> &[NavCell] {
        &self.grid.cells
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
        if nx >= self.grid.w || ny >= self.grid.h {
            return NavCell::Blocked;
        }
        *self.grid.get(nx, ny)
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
    /// A* from `start` to `goal` on the nav grid.
    ///
    /// Returns a list of direction-change waypoints (not every cell).
    /// Returns `None` if no path exists.
    pub fn astar(&self, start: (usize, usize), goal: (usize, usize)) -> Option<Vec<(u16, u16)>> {
        use std::collections::BinaryHeap;
        use std::cmp::Reverse;

        if start == goal {
            return Some(vec![(goal.0 as u16, goal.1 as u16)]);
        }

        let w = self.grid.w;
        let h = self.grid.h;
        let cell_count = w * h;
        let idx = |x: usize, y: usize| x + y * w;

        let mut g_score = vec![f32::MAX; cell_count];
        let mut came_from = vec![usize::MAX; cell_count];
        let mut open: BinaryHeap<Reverse<(u32, usize)>> = BinaryHeap::new();

        let start_idx = idx(start.0, start.1);
        g_score[start_idx] = 0.0;
        let h_score = heuristic(start, goal);
        open.push(Reverse((float_to_ord(h_score), start_idx)));

        while let Some(Reverse((_, cur_idx))) = open.pop() {
            let cur_x = cur_idx % w;
            let cur_y = cur_idx / w;

            if (cur_x, cur_y) == goal {
                return Some(reconstruct_path(&came_from, cur_idx, start_idx, w));
            }

            let cur_g = g_score[cur_idx];

            for (nx, ny) in self.grid.neighbors_4(cur_x, cur_y) {
                let cell = *self.grid.get(nx, ny);
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
            // Suppress unused-bound warning when h == 0.
            let _ = h;
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

/// Reconstruct path from `came_from` table, keeping only direction-change waypoints.
fn reconstruct_path(came_from: &[usize], mut cur: usize, start: usize, w: usize) -> Vec<(u16, u16)> {
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
        let x = (cell_idx % w) as u16;
        let y = (cell_idx / w) as u16;
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

// ── Flow field ────────────────────────────────────────────────────────────────

/// Pre-computed Dijkstra/BFS flow field from a target settlement outward.
///
/// Each cell holds an `(i8, i8)` direction vector pointing toward the target.
pub struct FlowFieldData {
    /// Flat row-major array of direction vectors, indexed `[nx + ny * w]`.
    pub dirs: Vec<(i8, i8)>,
    pub w: usize,
    pub h: usize,
}

impl FlowFieldData {
    /// Compute a flow field by BFS from the target nav-grid cell outward.
    pub fn compute(nav: &NavGrid, target: (usize, usize)) -> Self {
        use std::collections::VecDeque;

        let w = nav.grid.w;
        let h = nav.grid.h;
        let n = w * h;
        let mut dirs: Vec<(i8, i8)> = vec![(0, 0); n];
        let mut visited = vec![false; n];

        let idx = |x: usize, y: usize| x + y * w;
        let mut queue = VecDeque::new();

        let start_idx = idx(target.0, target.1);
        visited[start_idx] = true;
        queue.push_back((target.0, target.1));

        while let Some((cx, cy)) = queue.pop_front() {
            for (nx, ny) in nav.grid.neighbors_4(cx, cy) {
                if nav.cell(nx, ny).movement_cost() == f32::MAX {
                    continue; // Blocked
                }
                let ni = idx(nx, ny);
                if !visited[ni] {
                    visited[ni] = true;
                    // Direction from (nx,ny) toward (cx,cy) (i.e., toward the target).
                    #[allow(clippy::cast_possible_truncation)]
                    let dir = (
                        (cx as i32 - nx as i32).clamp(-1, 1) as i8,
                        (cy as i32 - ny as i32).clamp(-1, 1) as i8,
                    );
                    dirs[ni] = dir;
                    queue.push_back((nx, ny));
                }
            }
        }

        FlowFieldData { dirs, w, h }
    }

    /// Sample the direction vector at nav-grid cell `(nx, ny)`.
    #[inline]
    pub fn dir_at(&self, nx: usize, ny: usize) -> (i8, i8) {
        if nx >= self.w || ny >= self.h {
            return (0, 0);
        }
        self.dirs[nx + ny * self.w]
    }
}

/// Cached flow fields keyed by target settlement chunk coordinates (u32, u32).
///
/// Multiple war parties targeting the same settlement reuse the same field.
#[derive(Resource, Default)]
pub struct FlowField {
    pub fields: std::collections::HashMap<(u32, u32), FlowFieldData>,
}

impl FlowField {
    /// Get or compute the flow field for a settlement at world position `(wx, wy)`.
    pub fn get_or_compute(&mut self, nav: &NavGrid, wx: f32, wy: f32) -> &FlowFieldData {
        let (nx, ny) = world_to_nav(wx, wy);
        // Key by nav-grid cell as settlement chunk coords.
        let key = (nx as u32, ny as u32);
        self.fields.entry(key).or_insert_with(|| FlowFieldData::compute(nav, (nx, ny)))
    }

    pub fn get(&self, wx: f32, wy: f32) -> Option<&FlowFieldData> {
        let (nx, ny) = world_to_nav(wx, wy);
        self.fields.get(&(nx as u32, ny as u32))
    }
}

/// Build the NavGrid from the WorldMap and insert it as a resource.
/// Must run after generate_world inserts the WorldMap.
pub fn build_nav_grid(map: Res<WorldMap>, mut commands: Commands) {
    let nav = NavGrid::build(&map);
    tracing::info!(
        cells = nav.grid.cells.len(),
        blocked = nav.grid.cells.iter().filter(|&&c| c == NavCell::Blocked).count(),
        slow = nav.grid.cells.iter().filter(|&&c| c == NavCell::Slow).count(),
        "NavGrid built"
    );
    commands.insert_resource(nav);
}

// ── ZoneNavGrids ──────────────────────────────────────────────────────────────

use fellytip_shared::world::zone::{InteriorTile, ZoneId, ZoneRegistry};
use std::collections::HashMap;

/// Per-zone navigation grids, one `Grid<NavCell>` per known zone.
///
/// Built at startup from `ZoneRegistry`; consumed by zone-aware path planners.
#[derive(Resource, Default)]
pub struct ZoneNavGrids(pub HashMap<ZoneId, Grid<NavCell>>);

impl ZoneNavGrids {
    pub fn get(&self, zone: ZoneId) -> Option<&Grid<NavCell>> {
        self.0.get(&zone)
    }

    pub fn insert(&mut self, zone: ZoneId, grid: Grid<NavCell>) {
        self.0.insert(zone, grid);
    }

    /// A* within a single zone's nav grid.
    ///
    /// `start` and `end` are zone-local tile coordinates `(x, y)`. Returns
    /// direction-change-compressed waypoints as `(u16, u16)`, or `None` if the
    /// zone has no nav grid or no path exists.
    pub fn zone_astar(
        &self,
        zone: ZoneId,
        start: (usize, usize),
        goal: (usize, usize),
    ) -> Option<Vec<(u16, u16)>> {
        use std::collections::BinaryHeap;
        use std::cmp::Reverse;

        let grid = self.0.get(&zone)?;

        if start == goal {
            return Some(vec![(goal.0 as u16, goal.1 as u16)]);
        }

        let w = grid.w;
        let h = grid.h;
        let cell_count = w * h;
        let idx = |x: usize, y: usize| x + y * w;

        let mut g_score = vec![f32::MAX; cell_count];
        let mut came_from = vec![usize::MAX; cell_count];
        let mut open: BinaryHeap<Reverse<(u32, usize)>> = BinaryHeap::new();

        // Clamp inputs to grid bounds.
        let sx = start.0.min(w - 1);
        let sy = start.1.min(h - 1);
        let ex = goal.0.min(w - 1);
        let ey = goal.1.min(h - 1);

        let start_idx = idx(sx, sy);
        let h_score = zone_heuristic((sx, sy), (ex, ey));
        g_score[start_idx] = 0.0;
        open.push(Reverse((float_to_ord(h_score), start_idx)));

        while let Some(Reverse((_, cur_idx))) = open.pop() {
            let cur_x = cur_idx % w;
            let cur_y = cur_idx / w;

            if (cur_x, cur_y) == (ex, ey) {
                return Some(reconstruct_path(&came_from, cur_idx, start_idx, w));
            }

            let cur_g = g_score[cur_idx];

            for (nx, ny) in grid.neighbors_4(cur_x, cur_y) {
                let cell = *grid.get(nx, ny);
                let cost = cell.movement_cost();
                if cost == f32::MAX {
                    continue;
                }
                let n_idx = idx(nx, ny);
                let tentative_g = cur_g + cost;
                if tentative_g < g_score[n_idx] {
                    g_score[n_idx] = tentative_g;
                    came_from[n_idx] = cur_idx;
                    let f = tentative_g + zone_heuristic((nx, ny), (ex, ey));
                    open.push(Reverse((float_to_ord(f), n_idx)));
                }
            }
        }

        None
    }
}

fn zone_heuristic(a: (usize, usize), b: (usize, usize)) -> f32 {
    let dx = (a.0 as i32 - b.0 as i32).abs();
    let dy = (a.1 as i32 - b.1 as i32).abs();
    (dx + dy) as f32
}

/// Convert an `InteriorTile` to its navigation cost class.
pub fn interior_tile_to_nav_cell(tile: InteriorTile) -> NavCell {
    match tile {
        InteriorTile::Floor | InteriorTile::Stair | InteriorTile::Balcony => NavCell::Passable,
        InteriorTile::Window | InteriorTile::Roof => NavCell::Slow,
        InteriorTile::Wall | InteriorTile::Void | InteriorTile::Water | InteriorTile::Pit => {
            NavCell::Blocked
        }
    }
}

/// Startup system: populate `ZoneNavGrids` from all zones in `ZoneRegistry`.
pub fn build_zone_nav_grids(
    registry: Option<Res<ZoneRegistry>>,
    mut zone_grids: ResMut<ZoneNavGrids>,
) {
    let Some(registry) = registry else { return };
    for zone in registry.zones.values() {
        let Some(tiles) = registry.tiles(zone) else { continue };
        if tiles.is_empty() {
            continue;
        }
        let cells: Vec<NavCell> = tiles.iter().copied().map(interior_tile_to_nav_cell).collect();
        let grid = Grid::from_cells(zone.width as usize, zone.height as usize, cells);
        zone_grids.insert(zone.id, grid);
    }
    tracing::info!(count = zone_grids.0.len(), "ZoneNavGrids built");
}
