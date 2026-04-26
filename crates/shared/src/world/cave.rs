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
