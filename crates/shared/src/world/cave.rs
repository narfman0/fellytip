//! Procedural cave layer generation.
//!
//! Caves live below the surface in the same (ix, iy) grid. Each `depth`
//! adds walkable TileLayers at negative z values via WorldMap::add_stair_layer
//! (or the equivalent insertion helper). Use fBm noise (the same noise
//! infrastructure used for biome generation in this crate) to carve open
//! cave space out of solid rock.

use crate::math::{fbm, lattice_hash};
use crate::world::map::{TileKind, TileLayer, WorldMap};

pub const CAVE_DEPTH_SPACING: f32 = 20.0;

pub fn cave_z(depth: u32) -> f32 {
    -(10.0 + depth.saturating_sub(1) as f32 * CAVE_DEPTH_SPACING)
}

fn insert_cave_layer(
    map: &mut WorldMap,
    ix: usize,
    iy: usize,
    z_top: f32,
    kind: TileKind,
    walkable: bool,
) {
    if ix >= map.width || iy >= map.height {
        return;
    }
    let idx = ix + iy * map.width;
    let col = &mut map.columns[idx];
    if col.layers.iter().any(|l| (l.z_top - z_top).abs() < 0.1) {
        return;
    }
    let new_layer = TileLayer {
        z_base: z_top - 0.5,
        z_top,
        kind,
        walkable,
        corner_offsets: [0.0; 4],
    };
    let pos = col.layers.partition_point(|l| l.z_base < new_layer.z_base);
    col.layers.insert(pos, new_layer);
}

pub fn generate_cave_layer(map: &mut WorldMap, seed: u64, depth: u32) {
    let width = map.width;
    let height = map.height;
    let z = cave_z(depth);

    let depth_seed = seed
        .wrapping_add((depth as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let ox = (depth_seed.wrapping_mul(2_654_435_761) % 100_000) as f32;
    let oy = (depth_seed.wrapping_mul(805_459_861) % 100_000) as f32;
    let rox = (depth_seed.wrapping_mul(1_234_567_891) % 100_000) as f32;
    let roy = (depth_seed.wrapping_mul(987_654_321) % 100_000) as f32;

    let freq: f32 = 8.0 / width as f32;
    let depth_offset = (depth as i32).wrapping_mul(1_000_003);

    for iy in 0..height {
        for ix in 0..width {
            let n = fbm(
                (ix as f32 + ox) * freq,
                (iy as f32 + oy) * freq,
                4,
                0.5,
                2.0,
            );
            let open = n > 0.48;

            let kind = if open {
                let river_n = fbm(
                    (ix as f32 + rox) * freq,
                    (iy as f32 + roy) * freq,
                    3,
                    0.5,
                    2.0,
                );
                let h = lattice_hash(
                    ix as i32 + depth_offset,
                    iy as i32 + depth_offset.wrapping_add(31),
                );
                if (river_n - 0.5).abs() < 0.015 {
                    TileKind::CaveRiver
                } else if depth >= 3 && h < 0.01 {
                    TileKind::LavaFloor
                } else if h < 0.03 {
                    TileKind::CrystalCave
                } else {
                    TileKind::CaveFloor
                }
            } else {
                TileKind::CaveWall
            };

            let walkable = !matches!(kind, TileKind::CaveWall);
            insert_cave_layer(map, ix, iy, z, kind, walkable);
        }
    }
}

/// Find a suitable capital site within the cave layer at `depth`.
///
/// Scans all open cave floor tiles and returns the tile closest to the
/// centroid of all open tiles (a cheap "center of mass" approximation),
/// seeded for tie-breaking. Returns `None` if no open tiles exist.
pub fn find_cave_capital_site(map: &WorldMap, seed: u64, depth: u32) -> Option<(usize, usize)> {
    let mut open: Vec<(usize, usize)> = Vec::new();
    for iy in 0..map.height {
        for ix in 0..map.width {
            if is_cave_open(map, ix, iy, depth) {
                open.push((ix, iy));
            }
        }
    }
    if open.is_empty() {
        return None;
    }
    let sum_x: u64 = open.iter().map(|&(x, _)| x as u64).sum();
    let sum_y: u64 = open.iter().map(|&(_, y)| y as u64).sum();
    let cx = (sum_x / open.len() as u64) as usize;
    let cy = (sum_y / open.len() as u64) as usize;
    let tiebreak = seed.wrapping_mul(0x9E3779B97F4A7C15);
    open.iter()
        .enumerate()
        .min_by_key(|&(i, &(ix, iy))| {
            let dx = ix as i64 - cx as i64;
            let dy = iy as i64 - cy as i64;
            let dist_sq = dx * dx + dy * dy;
            (dist_sq, (i as u64).wrapping_mul(tiebreak) as i64)
        })
        .map(|(_, &pos)| pos)
}

/// Place `count` portal tiles distributed across the cave layer at `depth`.
///
/// Subdivides the map into a grid of cells and picks one open cave floor tile
/// per cell (up to `count` cells). Sets the cave layer tile and the
/// corresponding surface tile at the same (ix, iy) to `TileKind::CavePortal`.
pub fn place_cave_portals(map: &mut WorldMap, seed: u64, depth: u32, count: usize) {
    if count == 0 {
        return;
    }
    let width = map.width;
    let height = map.height;
    let cave_z_val = cave_z(depth);

    let cells_per_axis = (count as f64).sqrt().ceil() as usize + 1;
    let cell_w = width.div_ceil(cells_per_axis);
    let cell_h = height.div_ceil(cells_per_axis);

    let portal_seed = seed.wrapping_add(0xFEED_FACE_CAFE_BABE);
    let mut placed = 0usize;

    'outer: for cy in 0..cells_per_axis {
        for cx in 0..cells_per_axis {
            if placed >= count {
                break 'outer;
            }
            let x0 = cx * cell_w;
            let y0 = cy * cell_h;
            let x1 = (x0 + cell_w).min(width);
            let y1 = (y0 + cell_h).min(height);

            let cell_idx = cy * cells_per_axis + cx;
            let cell_seed = portal_seed.wrapping_add((cell_idx as u64).wrapping_mul(0x9E3779B97F4A7C15));
            let rand_off_x = (cell_seed.wrapping_mul(2654435761) >> 32) as usize;
            let rand_off_y = (cell_seed.wrapping_mul(805459861) >> 32) as usize;
            let span_x = if x1 > x0 { x1 - x0 } else { 1 };
            let span_y = if y1 > y0 { y1 - y0 } else { 1 };
            let start_x = x0 + rand_off_x % span_x;
            let start_y = y0 + rand_off_y % span_y;

            'cell: for dy in 0..span_y {
                for dx in 0..span_x {
                    let ix = x0 + (start_x - x0 + dx) % span_x;
                    let iy = y0 + (start_y - y0 + dy) % span_y;
                    if !is_cave_open(map, ix, iy, depth) {
                        continue;
                    }
                    let idx = ix + iy * width;
                    if let Some(layer) = map.columns[idx]
                        .layers
                        .iter_mut()
                        .find(|l| (l.z_top - cave_z_val).abs() < 0.1 && l.walkable)
                    {
                        layer.kind = TileKind::CavePortal;
                    }
                    if let Some(surface_layer) = map.columns[idx]
                        .layers
                        .iter_mut()
                        .find(|l| l.is_surface_kind() && l.walkable)
                    {
                        surface_layer.kind = TileKind::CavePortal;
                    }
                    placed += 1;
                    break 'cell;
                }
            }
        }
    }
}

/// Return all `(ix, iy)` positions that have a `CavePortal` layer at `cave_z(depth)`.
pub fn find_portal_tiles(map: &WorldMap, depth: u32) -> Vec<(usize, usize)> {
    let z = cave_z(depth);
    let mut portals = Vec::new();
    for iy in 0..map.height {
        for ix in 0..map.width {
            let col = &map.columns[ix + iy * map.width];
            if col.layers.iter().any(|l| l.kind == TileKind::CavePortal && (l.z_top - z).abs() < 0.1) {
                portals.push((ix, iy));
            }
        }
    }
    portals
}

pub fn is_cave_open(map: &WorldMap, ix: usize, iy: usize, depth: u32) -> bool {
    if ix >= map.width || iy >= map.height {
        return false;
    }
    let z = cave_z(depth);
    let col = &map.columns[ix + iy * map.width];
    col.layers.iter().any(|l| {
        (l.z_top - z).abs() < 0.1
            && matches!(
                l.kind,
                TileKind::CaveFloor
                    | TileKind::CrystalCave
                    | TileKind::LavaFloor
                    | TileKind::CaveRiver
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::map::TileColumn;

    fn empty_map(width: usize, height: usize) -> WorldMap {
        WorldMap {
            columns: vec![TileColumn::default(); width * height],
            width,
            height,
            seed: 0,
            road_tiles: vec![false; width * height],
            spawn_points: Vec::new(),
            buildings_stamped: false,
        }
    }

    #[test]
    fn cave_z_depth_one_is_negative_ten() {
        assert!((cave_z(1) - (-10.0)).abs() < 1e-6);
    }

    #[test]
    fn cave_z_depth_two_is_negative_thirty() {
        assert!((cave_z(2) - (-30.0)).abs() < 1e-6);
    }

    #[test]
    fn generate_cave_layer_adds_layers_at_correct_z() {
        let mut map = empty_map(32, 32);
        generate_cave_layer(&mut map, 42, 1);
        let z = cave_z(1);
        for iy in 0..map.height {
            for ix in 0..map.width {
                let col = &map.columns[ix + iy * map.width];
                assert!(
                    col.layers.iter().any(|l| (l.z_top - z).abs() < 0.1),
                    "cell ({ix},{iy}) missing cave layer at z={z}"
                );
            }
        }
    }

    #[test]
    fn generate_cave_layer_produces_mix_of_open_and_walls() {
        let mut map = empty_map(32, 32);
        generate_cave_layer(&mut map, 7, 1);
        let mut open = 0usize;
        let mut walls = 0usize;
        for iy in 0..map.height {
            for ix in 0..map.width {
                if is_cave_open(&map, ix, iy, 1) {
                    open += 1;
                } else {
                    let col = &map.columns[ix + iy * map.width];
                    if col.layers.iter().any(|l| l.kind == TileKind::CaveWall) {
                        walls += 1;
                    }
                }
            }
        }
        assert!(open > 0, "expected some open cave tiles, got {open}");
        assert!(walls > 0, "expected some cave wall tiles, got {walls}");
    }

    #[test]
    fn is_cave_open_true_for_floor_false_for_wall() {
        let mut map = empty_map(4, 4);
        let z = cave_z(1);
        insert_cave_layer(&mut map, 0, 0, z, TileKind::CaveFloor, true);
        insert_cave_layer(&mut map, 1, 0, z, TileKind::CaveWall, false);
        insert_cave_layer(&mut map, 2, 0, z, TileKind::CrystalCave, true);
        insert_cave_layer(&mut map, 3, 0, z, TileKind::CaveRiver, true);

        assert!(is_cave_open(&map, 0, 0, 1));
        assert!(!is_cave_open(&map, 1, 0, 1));
        assert!(is_cave_open(&map, 2, 0, 1));
        assert!(is_cave_open(&map, 3, 0, 1));
    }

    #[test]
    fn place_cave_portals_creates_portal_tiles() {
        let mut map = empty_map(64, 64);
        generate_cave_layer(&mut map, 42, 1);
        place_cave_portals(&mut map, 42, 1, 3);
        let z = cave_z(1);
        let portal_count = map.columns.iter().flat_map(|c| c.layers.iter()).filter(|l| {
            l.kind == TileKind::CavePortal && (l.z_top - z).abs() < 0.1
        }).count();
        assert!(portal_count >= 1, "expected >=1 cave portal tiles at cave z, got {portal_count}");
    }

    #[test]
    fn portal_exists_on_surface_and_underground() {
        use crate::world::map::{MAP_WIDTH, MAP_HEIGHT, generate_map};
        let mut map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        generate_cave_layer(&mut map, 42, 1);
        place_cave_portals(&mut map, 42, 1, 3);
        let cave_z_val = cave_z(1);
        let mut found_pair = false;
        for i in 0..map.columns.len() {
            let col = &map.columns[i];
            let has_cave_portal = col.layers.iter().any(|l| {
                l.kind == TileKind::CavePortal && (l.z_top - cave_z_val).abs() < 0.1
            });
            let has_surface_portal = col.layers.iter().any(|l| {
                l.kind == TileKind::CavePortal && l.z_top >= 0.0
            });
            if has_cave_portal && has_surface_portal {
                found_pair = true;
                break;
            }
        }
        assert!(found_pair, "expected at least one (ix,iy) with CavePortal at both z=0 and z=cave_z");
    }

    #[test]
    fn different_depths_produce_different_layouts() {
        let seed = 99;
        let mut a = empty_map(32, 32);
        generate_cave_layer(&mut a, seed, 1);
        let mut b = empty_map(32, 32);
        generate_cave_layer(&mut b, seed, 2);

        let mut differences = 0usize;
        for iy in 0..a.height {
            for ix in 0..a.width {
                if is_cave_open(&a, ix, iy, 1) != is_cave_open(&b, ix, iy, 2) {
                    differences += 1;
                }
            }
        }
        assert!(
            differences > 0,
            "expected different layouts at different depths, got {differences} differences"
        );
    }
}
