//! Static navigation grid for pathfinding.
//!
//! The 512×512 world tile map is downsampled 2:1 to a 256×256 `NavGrid`.
//! Each cell stores a `NavCell` passability class used by A* and flow-field systems.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use fellytip_shared::{
    PLAYER_SPEED,
    components::{NavPath, NavigationGoal, WorldPosition},
    world::{
        grid::Grid,
        map::{TileKind, WorldMap, MAP_HALF_HEIGHT, MAP_HALF_WIDTH},
    },
};
use crate::plugins::combat::LastPlayerInput;

pub const NAV_WIDTH: usize = 256;
pub const NAV_HEIGHT: usize = 256;
/// Downsample factor: MAP_WIDTH / NAV_WIDTH = 512 / 256 = 2
const DOWNSAMPLE: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Resource, Serialize, Deserialize)]
pub struct NavGrid {
    pub grid: Grid<NavCell>,
}

impl NavGrid {
    pub fn build(map: &WorldMap) -> Self {
        let mut cells = Vec::with_capacity(NAV_WIDTH * NAV_HEIGHT);
        for ny in 0..NAV_HEIGHT {
            for nx in 0..NAV_WIDTH {
                let tx = nx * DOWNSAMPLE;
                let ty = ny * DOWNSAMPLE;
                let col = map.column(tx.min(map.width - 1), ty.min(map.height - 1));
                let cell = col.layers.first()
                    .map(|l| {
                        if !l.walkable {
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

/// Compute an A* path between two world-space points and return the `NavPath`
/// and `NavigationGoal` components to insert on the entity.
///
/// Returns `None` if no path exists between the two points.
pub fn compute_nav_path(
    nav: &NavGrid,
    from: (f32, f32),
    to: (f32, f32, f32),
) -> Option<(NavPath, NavigationGoal)> {
    let start = world_to_nav(from.0, from.1);
    let goal = world_to_nav(to.0, to.1);
    let waypoints = nav.astar(start, goal)?;
    let path_world: Vec<[f32; 2]> = waypoints.iter()
        .map(|&(wx, wy)| {
            let (x, y) = nav_to_world(wx as usize, wy as usize);
            [x, y]
        })
        .collect();
    let nav_path = NavPath { waypoints, waypoint_index: 0 };
    let goal_comp = NavigationGoal { target: [to.0, to.1, to.2], path_world };
    Some((nav_path, goal_comp))
}

/// Move entities with an active `NavigationGoal` toward their A* waypoints.
///
/// Cancels on non-zero WASD input for player-controlled entities.
/// Removes both `NavPath` and `NavigationGoal` on arrival.
pub fn follow_navigation_goal(
    mut commands: Commands,
    mut query: Query<(Entity, &mut WorldPosition, &mut NavPath, Option<&LastPlayerInput>),
                     With<NavigationGoal>>,
    time: Res<Time<Fixed>>,
) {
    let step = PLAYER_SPEED * time.delta_secs();
    for (entity, mut pos, mut nav_path, last_input) in query.iter_mut() {
        if last_input.is_some_and(|i| i.move_dir != [0.0, 0.0]) {
            commands.entity(entity).remove::<(NavPath, NavigationGoal)>();
            continue;
        }
        if let Some((wx, wy)) = nav_path.next_waypoint() {
            let (target_x, target_y) = nav_to_world(wx as usize, wy as usize);
            let dx = target_x - pos.x;
            let dy = target_y - pos.y;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq <= step * step {
                pos.x = target_x;
                pos.y = target_y;
                nav_path.waypoint_index += 1;
            } else {
                let dist = dist_sq.sqrt();
                pos.x += (dx / dist) * step;
                pos.y += (dy / dist) * step;
            }
        }
        if nav_path.is_complete() {
            commands.entity(entity).remove::<(NavPath, NavigationGoal)>();
        }
    }
}

pub struct NavPlugin;

impl Plugin for NavPlugin {
    fn build(&self, app: &mut App) {
        // NavGrid is inserted as a resource by build_nav_grid, called from MapGenPlugin.
        app.add_systems(FixedUpdate, follow_navigation_goal);
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

/// Pure function: build a `NavGrid` from a `WorldMap`.  Called by both
/// `build_nav_grid` (when no cached resource exists) and `generate_world`
/// (when building the unified world gen cache).
pub fn build_nav_grid_from_map(map: &WorldMap) -> NavGrid {
    let nav = NavGrid::build(map);
    tracing::info!(
        cells = nav.grid.cells.len(),
        blocked = nav.grid.cells.iter().filter(|&&c| c == NavCell::Blocked).count(),
        slow = nav.grid.cells.iter().filter(|&&c| c == NavCell::Slow).count(),
        "NavGrid built"
    );
    nav
}

/// Startup system: build `NavGrid` from `WorldMap` and insert it as a resource.
/// Skips if `generate_world` already inserted a cached `NavGrid`.
pub fn build_nav_grid(map: Res<WorldMap>, existing: Option<Res<NavGrid>>, mut commands: Commands) {
    if existing.is_some() {
        tracing::info!("NavGrid: using cached value");
        return;
    }
    commands.insert_resource(build_nav_grid_from_map(&map));
}

// ── ZoneNavGrids ──────────────────────────────────────────────────────────────

use fellytip_shared::world::zone::{InteriorTile, ZoneId, ZoneRegistry};
use std::collections::HashMap;

/// Per-zone navigation grids, one `Grid<NavCell>` per known zone.
///
/// Built at startup from `ZoneRegistry`; consumed by zone-aware path planners.
#[derive(Resource, Default, Serialize, Deserialize)]
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

/// Pure function: build `ZoneNavGrids` from a `ZoneRegistry`.  Called by both
/// `build_zone_nav_grids` (when no cached resource exists) and `generate_world`
/// (when building the unified world gen cache).
pub fn build_zone_nav_grids_from_registry(registry: &ZoneRegistry) -> ZoneNavGrids {
    let mut zone_grids = ZoneNavGrids::default();
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
    zone_grids
}

/// Startup system: populate `ZoneNavGrids` from all zones in `ZoneRegistry`.
/// Skips if `generate_world` already inserted a cached `ZoneNavGrids`.
pub fn build_zone_nav_grids(
    registry: Option<Res<ZoneRegistry>>,
    mut zone_grids: ResMut<ZoneNavGrids>,
) {
    if !zone_grids.0.is_empty() {
        tracing::info!("ZoneNavGrids: using cached value");
        return;
    }
    let Some(registry) = registry else { return };
    *zone_grids = build_zone_nav_grids_from_registry(&registry);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use fellytip_shared::components::WorldPosition;
    use fellytip_shared::world::civilization::{Settlement, SettlementKind, Settlements};
    use smol_str::SmolStr;
    use std::time::Duration;
    use uuid::Uuid;

    /// Convert a settlement's tile-space coords to world-space `(x, y, z)`.
    fn settlement_world_pos(s: &Settlement) -> (f32, f32, f32) {
        (
            s.x - MAP_HALF_WIDTH as f32,
            s.y - MAP_HALF_HEIGHT as f32,
            s.z,
        )
    }

    /// Find the settlement closest to `from` in world-space.
    fn nearest_settlement(from: &WorldPosition, settlements: &Settlements) -> (f32, f32, f32) {
        settlements
            .0
            .iter()
            .map(|s| {
                let w = settlement_world_pos(s);
                let dx = w.0 - from.x;
                let dy = w.1 - from.y;
                (dx * dx + dy * dy, w)
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, w)| w)
            .expect("settlements list must not be empty")
    }

    fn dist2d(a: (f32, f32), b: (f32, f32)) -> f32 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    /// Spawn a player, find the nearest settlement, navigate toward it via the
    /// pathfinding/nav system, and assert that the player position has moved
    /// closer to the settlement than the spawn point.
    #[test]
    fn player_moves_toward_nearest_settlement() {
        // Build a fully-passable nav grid (skips the cost of generate_map).
        let nav = NavGrid {
            grid: Grid::from_cells(
                NAV_WIDTH,
                NAV_HEIGHT,
                vec![NavCell::Passable; NAV_WIDTH * NAV_HEIGHT],
            ),
        };

        // Spawn player at world origin.
        let spawn = WorldPosition { x: 0.0, y: 0.0, z: 0.0 };

        // Two settlements; the "Near" one is the one the player should walk to.
        // Settlement coordinates are tile-space, so add MAP_HALF_* to the
        // desired world-space position.
        let hw = MAP_HALF_WIDTH as f32;
        let hh = MAP_HALF_HEIGHT as f32;
        let settlements = Settlements(vec![
            Settlement {
                id: Uuid::new_v4(),
                name: SmolStr::new("Near"),
                kind: SettlementKind::Town,
                x: hw + 5.0,
                y: hh + 5.0,
                z: 0.0,
            },
            Settlement {
                id: Uuid::new_v4(),
                name: SmolStr::new("Far"),
                kind: SettlementKind::Capital,
                x: hw + 80.0,
                y: hh + 80.0,
                z: 0.0,
            },
        ]);

        // Find the nearest settlement and its world-space position.
        let target = nearest_settlement(&spawn, &settlements);
        let initial_dist = dist2d((spawn.x, spawn.y), (target.0, target.1));
        assert!(initial_dist > 1.0, "spawn must start meaningfully far from settlement");
        // Sanity: should pick the near one (~7.07), not the far one (~113).
        assert!(initial_dist < 20.0, "nearest_settlement should pick the close one");

        // Set up a minimal Bevy app with the navigation follower system.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(FixedUpdate, follow_navigation_goal);
        app.insert_resource(nav);
        app.insert_resource(settlements);

        // Compute the path from spawn to target via the public nav API.
        let nav_res = app.world().resource::<NavGrid>();
        let (nav_path, goal) = compute_nav_path(
            nav_res,
            (spawn.x, spawn.y),
            (target.0, target.1, target.2),
        )
        .expect("path should exist on a fully-passable nav grid");
        assert!(!nav_path.waypoints.is_empty(), "path should contain waypoints");

        // Spawn the player entity with WorldPosition + the nav components.
        let player = app
            .world_mut()
            .spawn((spawn.clone(), nav_path, goal))
            .id();

        // Drive FixedUpdate at a 1/64 s timestep until the path completes
        // (the system removes NavPath on arrival) or 8 simulated seconds pass.
        // PLAYER_SPEED = 2.5 u/s → 8 s covers ~20 units, well above the
        // ~7.07 unit start distance.
        let timestep = Duration::from_secs_f32(1.0 / 64.0);
        let max_ticks = 64 * 8;
        let mut ticks = 0;
        loop {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(timestep);
            app.world_mut().run_schedule(FixedUpdate);
            ticks += 1;
            // Stop early once the system has removed NavPath (arrived).
            if app.world().entity(player).get::<NavPath>().is_none() {
                break;
            }
            if ticks >= max_ticks {
                break;
            }
        }

        // Assert position changed and is now closer to the settlement.
        let final_pos = app
            .world()
            .entity(player)
            .get::<WorldPosition>()
            .expect("player must still have WorldPosition")
            .clone();

        assert!(
            (final_pos.x, final_pos.y) != (spawn.x, spawn.y),
            "player position must have changed from spawn ({}, {}) → ({}, {})",
            spawn.x, spawn.y, final_pos.x, final_pos.y,
        );

        let final_dist = dist2d((final_pos.x, final_pos.y), (target.0, target.1));
        assert!(
            final_dist < initial_dist,
            "player should be closer to settlement after pathing: \
             initial={initial_dist}, final={final_dist}",
        );
    }
}
