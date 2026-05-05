//! World generation preview tool.
//!
//! Generates a world map from a seed and renders a downsampled ASCII overview
//! to stdout, followed by settlement and civilization statistics.
//!
//! Usage:
//!   cargo run -p world_gen -- [--seed <N>] [--map-width <W>] [--map-height <H>] [--width <disp_w>] [--height <disp_h>]
//!
//! Defaults: seed=42, map 1024×1024, display size=80x40

use fellytip_shared::world::{
    civilization::{generate_settlements_full, generate_roads, SettlementKind},
    map::{generate_map, TileKind, MAP_HEIGHT, MAP_WIDTH},
};

fn main() {
    // ── Parse args ────────────────────────────────────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    let seed       = parse_flag(&args, "--seed",       42u64);
    let map_width  = parse_flag(&args, "--map-width",  MAP_WIDTH);
    let map_height = parse_flag(&args, "--map-height", MAP_HEIGHT);
    let disp_w     = parse_flag(&args, "--width",      80usize);
    let disp_h     = parse_flag(&args, "--height",     40usize);

    eprintln!("Generating map (seed={seed}, map={map_width}×{map_height}, display={disp_w}×{disp_h})…");

    // ── Generate world ────────────────────────────────────────────────────────
    let mut map = generate_map(seed, map_width, map_height);
    let settlements = generate_settlements_full(&mut map, seed);
    generate_roads(&mut map, &settlements);

    // ── Tile statistics ────────────────────────────────────────────────────────
    let mut counts = std::collections::HashMap::<TileKind, usize>::new();
    for col in &map.columns {
        for layer in &col.layers {
            *counts.entry(layer.kind).or_insert(0) += 1;
        }
    }
    let total_tiles = map_width * map_height;

    // ── Collect road/settlement markers for overlay ────────────────────────────
    // One character per display cell (downsampled from map_width × map_height).
    let cell_w = map_width  / disp_w;
    let cell_h = map_height / disp_h;

    // ── Render ASCII grid ─────────────────────────────────────────────────────
    println!();
    println!("  World Map — seed {seed}  ({map_width}×{map_height} tiles, displayed {disp_w}×{disp_h})");
    println!();

    for dy in 0..disp_h {
        print!("  ");
        for dx in 0..disp_w {
            let base_ix = dx * cell_w;
            let base_iy = dy * cell_h;

            // Check if any settlement is in this cell.
            let has_settlement = settlements.iter().any(|s| {
                let sx = s.x as usize / cell_w;
                let sy = s.y as usize / cell_h;
                sx == dx && sy == dy
            });

            // Check if any road tile is in this cell.
            let has_road = (0..cell_w).any(|ddx| {
                (0..cell_h).any(|ddy| {
                    let ix = base_ix + ddx;
                    let iy = base_iy + ddy;
                    if ix < map_width && iy < map_height {
                        map.road_tiles[ix + iy * map_width]
                    } else {
                        false
                    }
                })
            });

            // Find dominant surface tile kind in this cell.
            let dominant = dominant_kind(&map, base_ix, base_iy, cell_w, cell_h);

            let ch = if has_settlement {
                match settlements.iter().find(|s| {
                    s.x as usize / cell_w == dx && s.y as usize / cell_h == dy
                }) {
                    Some(s) if matches!(s.kind, SettlementKind::Capital) => '★',
                    _                                                       => '•',
                }
            } else if has_road {
                '+'
            } else {
                tile_char(dominant)
            };
            print!("{ch}");
        }
        println!();
    }
    println!();

    // ── Statistics ─────────────────────────────────────────────────────────────
    println!("  Tile composition ({total_tiles} surface columns):");
    let mut sorted_counts: Vec<_> = counts.iter().collect();
    sorted_counts.sort_by_key(|&(_, &v)| std::cmp::Reverse(v));
    for (kind, count) in &sorted_counts {
        let pct = **count as f32 / total_tiles as f32 * 100.0;
        println!("    {:20} {:6}  ({:.1}%)", format!("{kind:?}"), count, pct);
    }

    println!();
    println!("  Settlements ({}):", settlements.len());
    let capitals  = settlements.iter().filter(|s| matches!(s.kind, SettlementKind::Capital)).count();
    let towns     = settlements.iter().filter(|s| matches!(s.kind, SettlementKind::Town)).count();
    println!("    Capitals:         {capitals}  (★)");
    println!("    Towns:            {towns}  (•)");

    let road_tiles = map.road_tiles.iter().filter(|&&r| r).count();
    println!();
    println!("  Road tiles: {road_tiles} (+)");
    println!();
}

/// Find the most common surface [`TileKind`] in a cell.
fn dominant_kind(
    map: &fellytip_shared::world::map::WorldMap,
    base_ix: usize,
    base_iy: usize,
    cell_w: usize,
    cell_h: usize,
) -> TileKind {
    let mut counts = std::collections::HashMap::<TileKind, usize>::new();
    for ddy in 0..cell_h {
        for ddx in 0..cell_w {
            let ix = base_ix + ddx;
            let iy = base_iy + ddy;
            if ix >= map.width || iy >= map.height { continue; }
            let col = map.column(ix, iy);
            // Surface layer only (topmost).
            if let Some(layer) = col.layers.iter().rfind(|l| l.is_surface_kind()) {
                *counts.entry(layer.kind).or_insert(0) += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, v)| *v)
        .map(|(k, _)| k)
        .unwrap_or(TileKind::Void)
}

/// Map a [`TileKind`] to a printable ASCII character.
fn tile_char(kind: TileKind) -> char {
    match kind {
        TileKind::Water           => '~',
        TileKind::Mountain        => '^',
        TileKind::Plains          => '.',
        TileKind::Forest          => 't',
        TileKind::Stone           => '#',
        TileKind::Desert          => 'd',
        TileKind::Savanna         => 's',
        TileKind::TropicalForest  => 'T',
        TileKind::TropicalRainforest => 'R',
        TileKind::Grassland       => ',',
        TileKind::TemperateForest => 'f',
        TileKind::Taiga           => 'b',
        TileKind::Tundra          => '_',
        TileKind::PolarDesert     => 'p',
        TileKind::Arctic          => '*',
        TileKind::River           => '=',
        TileKind::CaveFloor       => 'c',
        TileKind::CaveWall        => 'w',
        TileKind::CrystalCave     => 'C',
        TileKind::LavaFloor       => 'L',
        TileKind::CaveRiver       => 'r',
        TileKind::CavePortal      => 'P',
        TileKind::Void            => ' ',
    }
}

// ── CLI helpers ────────────────────────────────────────────────────────────────

fn parse_flag<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    args.windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(default)
}
