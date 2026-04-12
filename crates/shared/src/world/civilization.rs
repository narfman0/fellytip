//! Civilizations: settlements, territory, and road networks.
//!
//! All functions are pure (no ECS) and deterministic given a seed.
//! Call order expected by the server:
//! ```text
//! let map      = generate_map(seed);
//! let civs     = generate_settlements(&map, seed);
//! assign_territories(&map, &civs);
//! generate_roads(&mut map, &civs);
//! ```

use std::collections::VecDeque;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use uuid::Uuid;

use crate::world::map::{TileKind, WorldMap, UNDERDARK_Z};

// ── Types ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementKind {
    Capital,
    Town,
    UndergroundCity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settlement {
    pub id: Uuid,
    pub name: SmolStr,
    pub kind: SettlementKind,
    /// Tile-space center X (ix + 0.5, range 0..MAP_WIDTH).  Subtract
    /// `MAP_HALF_WIDTH` to convert to world-space.
    pub x: f32,
    /// Tile-space center Y (iy + 0.5, range 0..MAP_HEIGHT).  Subtract
    /// `MAP_HALF_HEIGHT` to convert to world-space.
    pub y: f32,
    /// World-space Z elevation at this location.
    pub z: f32,
}

/// Bevy resource holding all generated settlements.
#[derive(Resource, Default, Clone, Debug, Serialize, Deserialize)]
pub struct Settlements(pub Vec<Settlement>);

/// Territory map: one optional settlement-index per tile column (flat row-major).
///
/// `territory[ix + iy * MAP_WIDTH] = Some(settlement_idx)` means the tile
/// is "claimed" by that settlement.
pub type TerritoryMap = Vec<Option<usize>>;

// ── Habitability ───────────────────────────────────────────────────────────────

/// Surface habitability score in `[0.0, 1.0]`.
///
/// `0.0` — uninhabitable (water, river, mountain, underground).
pub fn habitability(kind: TileKind) -> f32 {
    match kind {
        TileKind::Grassland | TileKind::Plains           => 1.0,
        TileKind::TemperateForest | TileKind::Forest     => 0.8,
        TileKind::Savanna
        | TileKind::TropicalForest
        | TileKind::TropicalRainforest                   => 0.7,
        TileKind::Taiga                                  => 0.5,
        TileKind::Desert                                 => 0.3,
        TileKind::Tundra                                 => 0.2,
        TileKind::PolarDesert | TileKind::Arctic         => 0.1,
        _                                                => 0.0,
    }
}

// ── Settlement generation ─────────────────────────────────────────────────────

/// Minimum tile distance between any two settlements.
const MIN_SETTLEMENT_DIST: f32 = 30.0;
/// Grid-cell size for Poisson-disk approximation (one candidate per cell).
const GRID_CELL: usize = 32;
/// Fraction of cells that become Capitals (~1 in 8).
const CAPITAL_PROB: f32 = 0.12;

/// Generate all surface and underground settlements deterministically from `seed`.
///
/// Surface: Poisson-disk grid approximation — divides the map into
/// `GRID_CELL×GRID_CELL` cells, picks the most habitable walkable tile in each,
/// rejects candidates too close to existing settlements.
///
/// Underground: Connected-component BFS of [`TileKind::LuminousGrotto`] tiles;
/// one city per component exceeding [`MIN_UNDERGROUND_AREA`].
pub fn generate_settlements(map: &WorldMap, seed: u64) -> Vec<Settlement> {
    let mut out = surface_settlements(map, seed);
    out.extend(underground_settlements(map, seed.wrapping_add(1)));
    out
}

fn surface_settlements(map: &WorldMap, seed: u64) -> Vec<Settlement> {
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut placed: Vec<Settlement> = Vec::new();
    let mut idx = 0usize;

    let cells_x = map.width  / GRID_CELL;
    let cells_y = map.height / GRID_CELL;

    for cy in 0..cells_y {
        for cx in 0..cells_x {
            // Find highest-habitability walkable tile in this cell.
            let mut best_score = 0.0f32;
            let mut best_pos   = None;
            let mut best_z     = 0.0f32;

            for dy in 0..GRID_CELL {
                for dx in 0..GRID_CELL {
                    let ix = cx * GRID_CELL + dx;
                    let iy = cy * GRID_CELL + dy;
                    let col = map.column(ix, iy);
                    if let Some(layer) = col.layers.iter().find(|l| l.is_surface_kind() && l.walkable) {
                        let score = habitability(layer.kind);
                        if score > best_score {
                            best_score = score;
                            best_pos   = Some((ix, iy));
                            best_z     = layer.z_top;
                        }
                    }
                }
            }

            if best_score < 0.3 {
                continue;
            }
            let (ix, iy) = match best_pos { Some(p) => p, None => continue };

            // Poisson-disk rejection.
            let fx = ix as f32 + 0.5;
            let fy = iy as f32 + 0.5;
            let too_close = placed.iter().any(|s: &Settlement| {
                let ddx = s.x - fx;
                let ddy = s.y - fy;
                (ddx * ddx + ddy * ddy).sqrt() < MIN_SETTLEMENT_DIST
            });
            if too_close {
                continue;
            }

            let kind = if idx == 0 || rng.random::<f32>() < CAPITAL_PROB {
                SettlementKind::Capital
            } else {
                SettlementKind::Town
            };

            let id = deterministic_uuid(&mut rng);
            placed.push(Settlement {
                id,
                name: SmolStr::new(format!("Settlement_{idx}")),
                kind,
                x: fx,
                y: fy,
                z: best_z,
            });
            idx += 1;
        }
    }

    placed
}

/// Minimum Underdark connected-component area to qualify for a city.
const MIN_UNDERGROUND_AREA: usize = 500;

fn underground_settlements(map: &WorldMap, seed: u64) -> Vec<Settlement> {
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let n = map.width * map.height;

    // Mark walkable LuminousGrotto tiles.
    let is_grotto: Vec<bool> = (0..n)
        .map(|i| {
            map.columns[i]
                .layers
                .iter()
                .any(|l| l.kind == TileKind::LuminousGrotto && l.walkable)
        })
        .collect();

    // BFS connected-component labeling.
    let mut component: Vec<Option<usize>> = vec![None; n];
    let mut components: Vec<Vec<usize>> = Vec::new();

    for start in 0..n {
        if !is_grotto[start] || component[start].is_some() {
            continue;
        }
        let comp_id = components.len();
        components.push(Vec::new());
        let mut queue = VecDeque::new();
        queue.push_back(start);
        component[start] = Some(comp_id);

        while let Some(idx) = queue.pop_front() {
            components[comp_id].push(idx);
            let ix = idx % map.width;
            let iy = idx / map.width;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 { continue; }
                    let nx = ix as i32 + dx;
                    let ny = iy as i32 + dy;
                    if nx < 0 || ny < 0
                        || nx as usize >= map.width
                        || ny as usize >= map.height
                    { continue; }
                    let ni = nx as usize + ny as usize * map.width;
                    if is_grotto[ni] && component[ni].is_none() {
                        component[ni] = Some(comp_id);
                        queue.push_back(ni);
                    }
                }
            }
        }
    }

    // One city per large component (centroid placement).
    let mut out = Vec::new();
    for (city_idx, cells) in components.iter().enumerate() {
        if cells.len() < MIN_UNDERGROUND_AREA {
            continue;
        }
        let sum_x: usize = cells.iter().map(|&i| i % map.width).sum();
        let sum_y: usize = cells.iter().map(|&i| i / map.width).sum();
        let cx = sum_x / cells.len();
        let cy = sum_y / cells.len();

        out.push(Settlement {
            id:   deterministic_uuid(&mut rng),
            name: SmolStr::new(format!("Deepcity_{city_idx}")),
            kind: SettlementKind::UndergroundCity,
            x:    cx as f32 + 0.5,
            y:    cy as f32 + 0.5,
            z:    UNDERDARK_Z,
        });
    }

    out
}

// ── Territory assignment ───────────────────────────────────────────────────────

/// Assign each surface tile to the nearest settlement via BFS flood-fill.
///
/// Only walkable surface tiles are assigned.  The returned [`TerritoryMap`] has
/// the same flat row-major layout as `WorldMap::columns`.
pub fn assign_territories(map: &WorldMap, settlements: &[Settlement]) -> TerritoryMap {
    let n = map.width * map.height;
    let mut territory: TerritoryMap = vec![None; n];
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new(); // (tile_idx, settlement_idx)

    // Seed BFS from each settlement's tile.
    for (si, s) in settlements.iter().enumerate() {
        let ix = (s.x as usize).min(map.width  - 1);
        let iy = (s.y as usize).min(map.height - 1);
        let idx = ix + iy * map.width;
        if territory[idx].is_none() {
            territory[idx] = Some(si);
            queue.push_back((idx, si));
        }
    }

    while let Some((idx, si)) = queue.pop_front() {
        let ix = idx % map.width;
        let iy = idx / map.width;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx == 0 && dy == 0 { continue; }
                let nx = ix as i32 + dx;
                let ny = iy as i32 + dy;
                if nx < 0 || ny < 0
                    || nx as usize >= map.width
                    || ny as usize >= map.height
                { continue; }
                let ni = nx as usize + ny as usize * map.width;
                if territory[ni].is_some() { continue; }
                // Only cross walkable surface tiles.
                let col = &map.columns[ni];
                if col.layers.iter().any(|l| l.is_surface_kind() && l.walkable) {
                    territory[ni] = Some(si);
                    queue.push_back((ni, si));
                }
            }
        }
    }

    territory
}

// ── Road network ───────────────────────────────────────────────────────────────

/// Connect all settlements with a minimum spanning tree road network.
///
/// # Algorithm
/// 1. Build a complete graph of Euclidean distances between settlements.
/// 2. Kruskal's MST on that graph.
/// 3. For each MST edge, draw a road by straight-line Bresenham walk and mark
///    `map.road_tiles[ix + iy * MAP_WIDTH] = true`.
///
/// Only surface settlements are connected (underground cities are excluded since
/// they can't be reached by surface roads).
pub fn generate_roads(map: &mut WorldMap, settlements: &[Settlement]) {
    let surface: Vec<&Settlement> = settlements
        .iter()
        .filter(|s| !matches!(s.kind, SettlementKind::UndergroundCity))
        .collect();

    if surface.len() < 2 {
        return;
    }

    // ── Kruskal's MST ─────────────────────────────────────────────────────────
    // Build sorted edge list (Euclidean distance).
    let mut edges: Vec<(f32, usize, usize)> = Vec::new();
    for i in 0..surface.len() {
        for j in (i + 1)..surface.len() {
            let dx = surface[i].x - surface[j].x;
            let dy = surface[i].y - surface[j].y;
            edges.push(((dx * dx + dy * dy).sqrt(), i, j));
        }
    }
    edges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Union-Find.
    let mut parent: Vec<usize> = (0..surface.len()).collect();
    fn find(p: &mut Vec<usize>, x: usize) -> usize {
        if p[x] != x { p[x] = find(p, p[x]); }
        p[x]
    }

    let mut mst_edges: Vec<(usize, usize)> = Vec::new();
    for (_, u, v) in &edges {
        let pu = find(&mut parent, *u);
        let pv = find(&mut parent, *v);
        if pu != pv {
            parent[pu] = pv;
            mst_edges.push((*u, *v));
            if mst_edges.len() == surface.len() - 1 {
                break;
            }
        }
    }

    // ── Bresenham road drawing ─────────────────────────────────────────────────
    for (u, v) in mst_edges {
        let ax = surface[u].x as i32;
        let ay = surface[u].y as i32;
        let bx = surface[v].x as i32;
        let by = surface[v].y as i32;
        for (rx, ry) in bresenham(ax, ay, bx, by) {
            if rx >= 0 && ry >= 0
                && (rx as usize) < map.width
                && (ry as usize) < map.height
            {
                map.road_tiles[rx as usize + ry as usize * map.width] = true;
            }
        }
    }
}

/// Bresenham line rasteriser.  Returns all integer points from `(x0,y0)` to `(x1,y1)`.
fn bresenham(mut x0: i32, mut y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
    let mut pts = Vec::new();
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        pts.push((x0, y0));
        if x0 == x1 && y0 == y1 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x0 += sx; }
        if e2 <= dx { err += dx; y0 += sy; }
    }
    pts
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Generate a deterministic UUID using bytes from a seeded RNG.
fn deterministic_uuid(rng: &mut impl rand::RngExt) -> Uuid {
    let mut bytes = [0u8; 16];
    for b in bytes.iter_mut() {
        *b = rng.random::<u8>();
    }
    Uuid::from_bytes(bytes)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::map::{generate_map, MAP_WIDTH, MAP_HEIGHT};

    #[test]
    fn surface_settlements_are_generated() {
        let map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 42);
        let surface_count = settlements
            .iter()
            .filter(|s| !matches!(s.kind, SettlementKind::UndergroundCity))
            .count();
        assert!(surface_count >= 5, "expected ≥5 surface settlements, got {surface_count}");
    }

    #[test]
    fn underground_cities_are_generated() {
        let map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 42);
        let underground_count = settlements
            .iter()
            .filter(|s| matches!(s.kind, SettlementKind::UndergroundCity))
            .count();
        assert!(underground_count >= 1,
            "expected ≥1 underground city, got {underground_count}");
    }

    #[test]
    fn at_least_one_capital_generated() {
        let map = generate_map(0, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 0);
        let capital_count = settlements
            .iter()
            .filter(|s| matches!(s.kind, SettlementKind::Capital))
            .count();
        assert!(capital_count >= 1, "expected ≥1 capital, got {capital_count}");
    }

    #[test]
    fn settlements_are_deterministic() {
        let map = generate_map(7, MAP_WIDTH, MAP_HEIGHT);
        let a = generate_settlements(&map, 7);
        let b = generate_settlements(&map, 7);
        assert_eq!(a.len(), b.len(), "settlement count is not deterministic");
        for (sa, sb) in a.iter().zip(b.iter()) {
            assert!((sa.x - sb.x).abs() < 1e-6 && (sa.y - sb.y).abs() < 1e-6,
                "settlement positions are not deterministic");
        }
    }

    #[test]
    fn territory_covers_most_surface_tiles() {
        let map = generate_map(1, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 1);
        let territory = assign_territories(&map, &settlements);

        let walkable: usize = map.columns.iter()
            .filter(|c| c.layers.iter().any(|l| l.is_surface_kind() && l.walkable))
            .count();
        let assigned: usize = territory.iter().filter(|t| t.is_some()).count();

        // With enough settlements, most walkable tiles should be assigned.
        if walkable > 0 {
            let ratio = assigned as f32 / walkable as f32;
            assert!(ratio > 0.5,
                "territory covers only {:.0}% of walkable tiles", ratio * 100.0);
        }
    }

    #[test]
    fn roads_are_written_to_map() {
        let map = generate_map(5, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 5);
        let mut map = map;
        generate_roads(&mut map, &settlements);

        let road_count = map.road_tiles.iter().filter(|&&r| r).count();
        assert!(road_count > 0, "no road tiles written");
    }

    #[test]
    fn min_settlement_distance_respected() {
        let map = generate_map(3, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 3);
        for i in 0..settlements.len() {
            for j in (i + 1)..settlements.len() {
                let si = &settlements[i];
                let sj = &settlements[j];
                // Underground cities aren't subject to surface Poisson-disk.
                if matches!(si.kind, SettlementKind::UndergroundCity)
                    || matches!(sj.kind, SettlementKind::UndergroundCity)
                {
                    continue;
                }
                let dx = si.x - sj.x;
                let dy = si.y - sj.y;
                let dist = (dx * dx + dy * dy).sqrt();
                assert!(
                    dist >= MIN_SETTLEMENT_DIST,
                    "settlements {i} and {j} are too close: {dist:.1} < {MIN_SETTLEMENT_DIST}"
                );
            }
        }
    }

    #[test]
    fn habitability_water_is_zero() {
        assert_eq!(habitability(TileKind::Water), 0.0);
        assert_eq!(habitability(TileKind::River), 0.0);
        assert_eq!(habitability(TileKind::Mountain), 0.0);
    }

    #[test]
    fn habitability_grassland_is_max() {
        assert_eq!(habitability(TileKind::Grassland), 1.0);
    }

    #[test]
    fn territory_index_in_bounds() {
        let map = generate_map(2, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 2);
        let territory = assign_territories(&map, &settlements);
        for &t in &territory {
            if let Some(idx) = t {
                assert!(idx < settlements.len(),
                    "territory index {idx} ≥ settlements.len()={}", settlements.len());
            }
        }
    }

    #[test]
    fn bresenham_endpoints_included() {
        let pts = bresenham(0, 0, 4, 3);
        assert!(pts.contains(&(0, 0)), "start not in bresenham output");
        assert!(pts.contains(&(4, 3)), "end not in bresenham output");
    }

}
