//! Zone-aware pathfinding helpers.
//!
//! Provides a pure `find_path_zone_aware` function that selects the correct
//! nav grid based on whether start and end are within the same non-overworld
//! zone. All A* logic is delegated to the existing grid-level algorithms to
//! keep this file to pure dispatch logic.
//!
//! ## Design decisions
//!
//! * Zone interiors use tile-local coordinates `(i32, i32)`. Conversion from
//!   world-space to a zone-local cell is the caller's responsibility, since the
//!   server `nav.rs` already owns the `world_to_nav` helpers and the zone
//!   interior does not share the overworld coordinate system.
//!
//! * Cross-zone paths (start zone ≠ end zone) return `None`. Zone transitions
//!   are handled by the portal system; intra-zone A* only needs to navigate
//!   within a single zone's grid.
//!
//! * If the zone has no registered `ZoneNavGrids` entry the function falls back
//!   to returning `None` so callers can gracefully degrade.
//!
//! None of this file touches Bevy ECS — the types used (`Grid`, `ZoneId`) are
//! plain Rust structs.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::HashMap;

use crate::world::grid::Grid;
use crate::world::zone::{ZoneId, OVERWORLD_ZONE};

// ── Cost type used by interior grids ─────────────────────────────────────────

/// Movement cost class for a single cell in a zone nav grid.
///
/// Mirrors `NavCell` from `crates/server/src/plugins/nav.rs` but lives in
/// `shared` so the pure pathfinder function can reference it without depending
/// on the server crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ZoneNavCell {
    #[default]
    Passable,
    Slow,
    Blocked,
}

impl ZoneNavCell {
    #[inline]
    pub fn movement_cost(self) -> f32 {
        match self {
            ZoneNavCell::Passable => 1.0,
            ZoneNavCell::Slow => 2.0,
            ZoneNavCell::Blocked => f32::MAX,
        }
    }
}

// ── Outcome ───────────────────────────────────────────────────────────────────

/// Result of a zone-aware path query.
#[derive(Debug, PartialEq, Eq)]
pub enum PathResult {
    /// A path was found; the vec holds `(x, y)` waypoints in zone-local tile
    /// coordinates, direction-change compressed (same semantics as `NavGrid::astar`).
    Found(Vec<(i32, i32)>),
    /// No path exists (blocked or start == goal but trivially reachable).
    NoPath,
    /// The query crossed zone boundaries — the portal system should handle it.
    ZoneCrossing,
    /// One or both zones have no registered nav grid.
    NoNavGrid,
    /// Both endpoints are on the overworld; use the overworld `NavGrid` instead.
    UseOverworld,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Find a path inside a single zone using the zone's `Grid<ZoneNavCell>`.
///
/// # Parameters
/// * `start` / `end` — zone-local tile coordinates `(x, y)`.
/// * `start_zone` / `end_zone` — the zones the start and end points belong to.
/// * `grids` — per-zone nav grids (keyed by `ZoneId`).
///
/// # Returns
/// A `PathResult` indicating whether a path was found or why one could not be
/// computed.
pub fn find_path_zone_aware(
    start: (i32, i32),
    end: (i32, i32),
    start_zone: ZoneId,
    end_zone: ZoneId,
    grids: &HashMap<ZoneId, Grid<ZoneNavCell>>,
) -> PathResult {
    // Both on overworld — delegate to the overworld A*.
    if start_zone == OVERWORLD_ZONE && end_zone == OVERWORLD_ZONE {
        return PathResult::UseOverworld;
    }

    // Different non-overworld zones — portal system handles it.
    if start_zone != end_zone {
        return PathResult::ZoneCrossing;
    }

    // Same non-overworld zone.
    let Some(grid) = grids.get(&start_zone) else {
        return PathResult::NoNavGrid;
    };

    match astar_on_grid(grid, start, end) {
        Some(path) => PathResult::Found(path),
        None => PathResult::NoPath,
    }
}

// ── Internal A* ──────────────────────────────────────────────────────────────

/// A* on a `Grid<ZoneNavCell>`. Returns direction-change-compressed waypoints
/// as `(i32, i32)` zone-local tile coordinates, or `None` if unreachable.
fn astar_on_grid(
    grid: &Grid<ZoneNavCell>,
    start: (i32, i32),
    end: (i32, i32),
) -> Option<Vec<(i32, i32)>> {
    let w = grid.w as i32;
    let h = grid.h as i32;

    // Clamp inputs to grid bounds.
    let clamp = |(x, y): (i32, i32)| {
        (x.clamp(0, w - 1) as usize, y.clamp(0, h - 1) as usize)
    };
    let (sx, sy) = clamp(start);
    let (ex, ey) = clamp(end);

    if (sx, sy) == (ex, ey) {
        return Some(vec![(ex as i32, ey as i32)]);
    }

    let cell_count = grid.w * grid.h;
    let idx = |x: usize, y: usize| x + y * grid.w;

    let mut g_score = vec![f32::MAX; cell_count];
    let mut came_from = vec![usize::MAX; cell_count];
    let mut open: BinaryHeap<Reverse<(u32, usize)>> = BinaryHeap::new();

    let start_idx = idx(sx, sy);
    g_score[start_idx] = 0.0;
    open.push(Reverse((f32_to_ord(heuristic((sx, sy), (ex, ey))), start_idx)));

    while let Some(Reverse((_, cur_idx))) = open.pop() {
        let cur_x = cur_idx % grid.w;
        let cur_y = cur_idx / grid.w;

        if (cur_x, cur_y) == (ex, ey) {
            return Some(reconstruct_compressed(&came_from, cur_idx, start_idx, grid.w));
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
                let f = tentative_g + heuristic((nx, ny), (ex, ey));
                open.push(Reverse((f32_to_ord(f), n_idx)));
            }
        }
    }

    None
}

fn heuristic(a: (usize, usize), b: (usize, usize)) -> f32 {
    let dx = (a.0 as i32 - b.0 as i32).unsigned_abs();
    let dy = (a.1 as i32 - b.1 as i32).unsigned_abs();
    (dx + dy) as f32
}

fn f32_to_ord(f: f32) -> u32 {
    f.to_bits()
}

/// Reconstruct path from `came_from`, keeping only direction-change waypoints.
/// The goal cell is always included as the final waypoint.
fn reconstruct_compressed(
    came_from: &[usize],
    mut cur: usize,
    start: usize,
    w: usize,
) -> Vec<(i32, i32)> {
    let goal_idx = cur;
    let mut full: Vec<usize> = Vec::new();
    while cur != start {
        full.push(cur);
        let prev = came_from[cur];
        if prev == usize::MAX {
            break;
        }
        cur = prev;
    }
    full.reverse();

    let mut waypoints: Vec<(i32, i32)> = Vec::new();
    let mut prev_dir: Option<(i32, i32)> = None;
    for &cell_idx in &full {
        let x = (cell_idx % w) as i32;
        let y = (cell_idx / w) as i32;
        if let Some(&(lx, ly)) = waypoints.last() {
            let dir = (x - lx, y - ly);
            if Some(dir) != prev_dir {
                waypoints.push((x, y));
                prev_dir = Some(dir);
            }
        } else {
            waypoints.push((x, y));
            prev_dir = None;
        }
    }

    // Always ensure the goal is the final waypoint.
    let goal_x = (goal_idx % w) as i32;
    let goal_y = (goal_idx / w) as i32;
    if waypoints.last().copied() != Some((goal_x, goal_y)) {
        waypoints.push((goal_x, goal_y));
    }

    waypoints
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_grid(w: usize, h: usize, blocked: &[(usize, usize)]) -> Grid<ZoneNavCell> {
        let mut cells = vec![ZoneNavCell::Passable; w * h];
        for &(bx, by) in blocked {
            cells[by * w + bx] = ZoneNavCell::Blocked;
        }
        Grid::from_cells(w, h, cells)
    }

    fn grids_with(zone: ZoneId, grid: Grid<ZoneNavCell>) -> HashMap<ZoneId, Grid<ZoneNavCell>> {
        let mut m = HashMap::new();
        m.insert(zone, grid);
        m
    }

    // Helper zone id.
    const ZONE_A: ZoneId = ZoneId(1);
    const ZONE_B: ZoneId = ZoneId(2);

    #[test]
    fn both_overworld_returns_use_overworld() {
        let grids: HashMap<ZoneId, Grid<ZoneNavCell>> = HashMap::new();
        let result = find_path_zone_aware(
            (0, 0), (3, 3), OVERWORLD_ZONE, OVERWORLD_ZONE, &grids,
        );
        assert_eq!(result, PathResult::UseOverworld);
    }

    #[test]
    fn different_zones_returns_zone_crossing() {
        let grids: HashMap<ZoneId, Grid<ZoneNavCell>> = HashMap::new();
        let result = find_path_zone_aware((0, 0), (3, 3), ZONE_A, ZONE_B, &grids);
        assert_eq!(result, PathResult::ZoneCrossing);
    }

    #[test]
    fn missing_nav_grid_returns_no_nav_grid() {
        let grids: HashMap<ZoneId, Grid<ZoneNavCell>> = HashMap::new();
        let result = find_path_zone_aware((0, 0), (3, 3), ZONE_A, ZONE_A, &grids);
        assert_eq!(result, PathResult::NoNavGrid);
    }

    #[test]
    fn simple_path_found() {
        // 4×4 fully passable grid, path from (0,0) to (3,3).
        let grid = make_grid(4, 4, &[]);
        let grids = grids_with(ZONE_A, grid);
        let result = find_path_zone_aware((0, 0), (3, 3), ZONE_A, ZONE_A, &grids);
        match result {
            PathResult::Found(waypoints) => {
                assert!(!waypoints.is_empty());
                let last = *waypoints.last().unwrap();
                assert_eq!(last, (3, 3));
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn blocked_path_returns_no_path() {
        // 4×4 grid with a complete horizontal wall at y=1 (blocks (0,1) through (3,1)).
        let grid = make_grid(4, 4, &[(0, 1), (1, 1), (2, 1), (3, 1)]);
        let grids = grids_with(ZONE_A, grid);
        let result = find_path_zone_aware((0, 0), (0, 3), ZONE_A, ZONE_A, &grids);
        assert_eq!(result, PathResult::NoPath);
    }

    #[test]
    fn path_around_obstacle() {
        // 4×4 grid with a partial wall: blocks (1,1), (1,2) but (1,3) is open.
        let grid = make_grid(4, 4, &[(1, 1), (1, 2)]);
        let grids = grids_with(ZONE_A, grid);
        let result = find_path_zone_aware((0, 0), (2, 2), ZONE_A, ZONE_A, &grids);
        assert!(matches!(result, PathResult::Found(_)));
    }

    #[test]
    fn start_equals_end_returns_single_waypoint() {
        let grid = make_grid(4, 4, &[]);
        let grids = grids_with(ZONE_A, grid);
        let result = find_path_zone_aware((2, 2), (2, 2), ZONE_A, ZONE_A, &grids);
        match result {
            PathResult::Found(wps) => {
                assert_eq!(wps.len(), 1);
                assert_eq!(wps[0], (2, 2));
            }
            other => panic!("expected Found with single waypoint, got {other:?}"),
        }
    }

    #[test]
    fn slow_tiles_traversed() {
        // 3×1 grid: (0,0)=Passable, (1,0)=Slow, (2,0)=Passable.
        let cells = vec![ZoneNavCell::Passable, ZoneNavCell::Slow, ZoneNavCell::Passable];
        let grid = Grid::from_cells(3, 1, cells);
        let grids = grids_with(ZONE_A, grid);
        let result = find_path_zone_aware((0, 0), (2, 0), ZONE_A, ZONE_A, &grids);
        assert!(matches!(result, PathResult::Found(_)));
    }
}
