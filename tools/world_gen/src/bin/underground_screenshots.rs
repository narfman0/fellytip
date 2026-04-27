//! Generates a series of PNG screenshots tracing the path from the
//! underground capital centre up to the surface cave portals (seed 42).
//!
//! Outputs:
//!   /tmp/fellytip_underground/00_overview.png        — full world overview (surface)
//!   /tmp/fellytip_underground/01_surface_portals.png — surface zoomed in on portal region
//!   /tmp/fellytip_underground/02_underground_full.png— full cave layer map
//!   /tmp/fellytip_underground/03_capital_centre.png  — zoomed in on underground capital
//!   /tmp/fellytip_underground/04_path_to_surface.png — overlay: path from capital → portals
//!
//! Usage:  cargo run -p world_gen --bin underground_screenshots -- [--seed N]

use std::path::Path;
use image::{Rgb, RgbImage};
use fellytip_shared::world::{
    civilization::{generate_settlements_full, generate_roads, SettlementKind},
    map::{generate_map, TileKind, MAP_WIDTH, MAP_HEIGHT},
    cave::{cave_z, find_portal_tiles},
};

const SEED: u64 = 42;
const CAVE_DEPTH: u32 = 1;

// Pixel colours per tile kind
fn tile_color(kind: TileKind) -> Rgb<u8> {
    match kind {
        TileKind::Plains          => Rgb([180, 200, 120]),
        TileKind::Forest          => Rgb([34,  110,  34]),
        TileKind::Mountain        => Rgb([160, 160, 160]),
        TileKind::Water           => Rgb([30,  100, 200]),
        TileKind::Stone           => Rgb([140, 140, 120]),
        TileKind::Desert          => Rgb([230, 210, 130]),
        TileKind::Savanna         => Rgb([200, 180,  80]),
        TileKind::TropicalForest  => Rgb([20,  140,  60]),
        TileKind::TropicalRainforest => Rgb([0, 100,  40]),
        TileKind::Grassland       => Rgb([140, 200, 100]),
        TileKind::TemperateForest => Rgb([60,  140,  60]),
        TileKind::Taiga           => Rgb([80,  130, 100]),
        TileKind::Tundra          => Rgb([200, 210, 220]),
        TileKind::PolarDesert     => Rgb([240, 240, 250]),
        TileKind::Arctic          => Rgb([255, 255, 255]),
        TileKind::River           => Rgb([60,  140, 220]),
        TileKind::CaveFloor       => Rgb([60,   60,  80]),
        TileKind::CaveWall        => Rgb([30,   30,  40]),
        TileKind::CrystalCave     => Rgb([80,  180, 220]),
        TileKind::LavaFloor       => Rgb([220,  60,  20]),
        TileKind::CaveRiver       => Rgb([40,  100, 180]),
        TileKind::CavePortal      => Rgb([220, 180,  20]),  // gold
        TileKind::Void            => Rgb([0,    0,    0]),
    }
}

fn save_png(img: &RgbImage, path: &str) {
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    img.save(p).expect("failed to save PNG");
    println!("Saved: {path}");
}

/// Render a rectangular region of the world's surface into an RgbImage.
/// `scale` is the number of output pixels per world tile.
fn render_surface_region(
    map: &fellytip_shared::world::map::WorldMap,
    x0: usize, y0: usize,
    region_w: usize, region_h: usize,
    scale: u32,
) -> RgbImage {
    let pw = (region_w as u32) * scale;
    let ph = (region_h as u32) * scale;
    let mut img = RgbImage::new(pw, ph);
    for dy in 0..region_h {
        for dx in 0..region_w {
            let ix = x0 + dx;
            let iy = y0 + dy;
            let kind = if ix < map.width && iy < map.height {
                let col = map.column(ix, iy);
                // Surface layer: prefer CavePortal if present, else topmost surface
                col.layers.iter()
                    .rfind(|l| l.kind == TileKind::CavePortal && l.z_top >= -0.5)
                    .or_else(|| col.layers.iter().rfind(|l| l.is_surface_kind()))
                    .map(|l| l.kind)
                    .unwrap_or(TileKind::Void)
            } else {
                TileKind::Void
            };
            let color = tile_color(kind);
            for sy in 0..scale {
                for sx in 0..scale {
                    let px = (dx as u32) * scale + sx;
                    let py = (dy as u32) * scale + sy;
                    img.put_pixel(px, py, color);
                }
            }
        }
    }
    img
}

/// Render a rectangular region of the cave layer into an RgbImage.
fn render_cave_region(
    map: &fellytip_shared::world::map::WorldMap,
    depth: u32,
    x0: usize, y0: usize,
    region_w: usize, region_h: usize,
    scale: u32,
) -> RgbImage {
    let z = cave_z(depth);
    let pw = (region_w as u32) * scale;
    let ph = (region_h as u32) * scale;
    let mut img = RgbImage::new(pw, ph);
    for dy in 0..region_h {
        for dx in 0..region_w {
            let ix = x0 + dx;
            let iy = y0 + dy;
            let kind = if ix < map.width && iy < map.height {
                let col = map.column(ix, iy);
                col.layers.iter()
                    .find(|l| (l.z_top - z).abs() < 0.5)
                    .map(|l| l.kind)
                    .unwrap_or(TileKind::Void)
            } else {
                TileKind::Void
            };
            let color = tile_color(kind);
            for sy in 0..scale {
                for sx in 0..scale {
                    let px = (dx as u32) * scale + sx;
                    let py = (dy as u32) * scale + sy;
                    img.put_pixel(px, py, color);
                }
            }
        }
    }
    img
}

/// Draw a filled rectangle (marker) on an image.
fn draw_marker(img: &mut RgbImage, cx: u32, cy: u32, half: u32, color: Rgb<u8>) {
    let x0 = cx.saturating_sub(half);
    let y0 = cy.saturating_sub(half);
    let x1 = (cx + half).min(img.width() - 1);
    let y1 = (cy + half).min(img.height() - 1);
    for py in y0..=y1 {
        for px in x0..=x1 {
            img.put_pixel(px, py, color);
        }
    }
}

/// Draw a simple line between two points (Bresenham).
fn draw_line(img: &mut RgbImage, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgb<u8>) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        if x >= 0 && y >= 0 && x < img.width() as i32 && y < img.height() as i32 {
            img.put_pixel(x as u32, y as u32, color);
        }
        if x == x1 && y == y1 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x += sx; }
        if e2 <= dx { err += dx; y += sy; }
    }
}

/// Draw a label (crude: print tile blocks spelling ASCII letters) — just a
/// thick dot-line border around a rectangle for visual clarity.
fn draw_border(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, color: Rgb<u8>, thickness: u32) {
    for t in 0..thickness {
        let x0t = x0.saturating_sub(t);
        let y0t = y0.saturating_sub(t);
        let x1t = (x1 + t).min(img.width() - 1);
        let y1t = (y1 + t).min(img.height() - 1);
        for px in x0t..=x1t {
            if y0t < img.height() { img.put_pixel(px, y0t, color); }
            if y1t < img.height() { img.put_pixel(px, y1t, color); }
        }
        for py in y0t..=y1t {
            if x0t < img.width() { img.put_pixel(x0t, py, color); }
            if x1t < img.width() { img.put_pixel(x1t, py, color); }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed = args.windows(2)
        .find(|w| w[0] == "--seed")
        .and_then(|w| w[1].parse::<u64>().ok())
        .unwrap_or(SEED);

    eprintln!("=== Fellytip Underground Screenshot Generator (seed={seed}) ===");

    eprintln!("Generating world map…");
    let mut map = generate_map(seed, MAP_WIDTH, MAP_HEIGHT);

    eprintln!("Generating settlements…");
    let settlements = generate_settlements_full(&mut map, seed);
    generate_roads(&mut map, &settlements);

    // Find the underground capital
    let underground_capital = settlements.iter()
        .find(|s| matches!(s.kind, SettlementKind::Capital) && s.z < 0.0)
        .expect("no underground capital found");

    let cap_x = underground_capital.x as usize;
    let cap_y = underground_capital.y as usize;
    eprintln!("Underground capital at tile ({cap_x}, {cap_y}), z={}", underground_capital.z);

    // Find surface cave portals
    let portal_tiles = find_portal_tiles(&map, CAVE_DEPTH);
    eprintln!("Found {} cave portal tiles at depth {CAVE_DEPTH}", portal_tiles.len());
    for (px, py) in &portal_tiles {
        eprintln!("  Portal at ({px}, {py})");
    }

    // Find a surface capital (for overview labelling)
    let surface_capitals: Vec<_> = settlements.iter()
        .filter(|s| matches!(s.kind, SettlementKind::Capital) && s.z >= 0.0)
        .collect();
    eprintln!("Surface capitals: {}", surface_capitals.len());

    // ── Screenshot 1: Full world surface overview (downsampled 4× → 256×256 px) ──
    eprintln!("\n[1/5] Rendering full surface overview…");
    {
        // Render at 1 pixel per 4 tiles = 256×256
        let _scale = 1u32;
        let step = 4usize;
        let disp_w = MAP_WIDTH / step;
        let disp_h = MAP_HEIGHT / step;
        let mut img = RgbImage::new(disp_w as u32, disp_h as u32);
        for dy in 0..disp_h {
            for dx in 0..disp_w {
                let ix = dx * step;
                let iy = dy * step;
                let col = map.column(ix, iy);
                let kind = col.layers.iter()
                    .rfind(|l| l.kind == TileKind::CavePortal && l.z_top >= -0.5)
                    .or_else(|| col.layers.iter().rfind(|l| l.is_surface_kind()))
                    .map(|l| l.kind)
                    .unwrap_or(TileKind::Void);
                img.put_pixel(dx as u32, dy as u32, tile_color(kind));
            }
        }
        // Mark surface capitals
        for sc in &surface_capitals {
            let px = sc.x as u32 / step as u32;
            let py = sc.y as u32 / step as u32;
            draw_marker(&mut img, px, py, 3, Rgb([255, 50, 50]));
        }
        // Mark portal locations
        for (ptx, pty) in &portal_tiles {
            let px = *ptx as u32 / step as u32;
            let py = *pty as u32 / step as u32;
            draw_marker(&mut img, px, py, 2, Rgb([255, 220, 0]));
        }
        save_png(&img, "/tmp/fellytip_underground/00_overview.png");
    }

    // ── Screenshot 2: Surface zoomed into the portal region ──────────────────
    eprintln!("[2/5] Rendering surface portal region…");
    {
        if !portal_tiles.is_empty() {
            let avg_px = portal_tiles.iter().map(|(x, _)| *x).sum::<usize>() / portal_tiles.len();
            let avg_py = portal_tiles.iter().map(|(_, y)| *y).sum::<usize>() / portal_tiles.len();
            let half = 128usize;
            let x0 = avg_px.saturating_sub(half);
            let y0 = avg_py.saturating_sub(half);
            let rw = (half * 2).min(MAP_WIDTH - x0);
            let rh = (half * 2).min(MAP_HEIGHT - y0);
            let scale = 4u32;
            let mut img = render_surface_region(&map, x0, y0, rw, rh, scale);
            // Mark portals
            let (iw1, ih1) = (img.width(), img.height());
            for (ptx, pty) in &portal_tiles {
                if *ptx >= x0 && *pty >= y0 {
                    let px = ((*ptx - x0) as u32) * scale + scale / 2;
                    let py = ((*pty - y0) as u32) * scale + scale / 2;
                    draw_marker(&mut img, px, py, 6, Rgb([255, 220, 0]));
                    draw_border(&mut img, px.saturating_sub(8), py.saturating_sub(8),
                        (px + 8).min(iw1 - 1), (py + 8).min(ih1 - 1),
                        Rgb([255, 255, 255]), 2);
                }
            }
            save_png(&img, "/tmp/fellytip_underground/01_surface_portals.png");
        } else {
            eprintln!("  No portals to render for screenshot 2");
        }
    }

    // ── Screenshot 3: Full underground cave layer ─────────────────────────────
    eprintln!("[3/5] Rendering full underground cave layer…");
    {
        let step = 4usize;
        let disp_w = MAP_WIDTH / step;
        let disp_h = MAP_HEIGHT / step;
        let z = cave_z(CAVE_DEPTH);
        let mut img = RgbImage::new(disp_w as u32, disp_h as u32);
        for dy in 0..disp_h {
            for dx in 0..disp_w {
                let ix = dx * step;
                let iy = dy * step;
                let col = map.column(ix, iy);
                let kind = col.layers.iter()
                    .find(|l| (l.z_top - z).abs() < 0.5)
                    .map(|l| l.kind)
                    .unwrap_or(TileKind::Void);
                img.put_pixel(dx as u32, dy as u32, tile_color(kind));
            }
        }
        // Mark underground capital
        let px = cap_x as u32 / step as u32;
        let py = cap_y as u32 / step as u32;
        draw_marker(&mut img, px, py, 4, Rgb([255, 50, 50]));
        // Mark underground portal positions
        for (ptx, pty) in &portal_tiles {
            let px2 = *ptx as u32 / step as u32;
            let py2 = *pty as u32 / step as u32;
            draw_marker(&mut img, px2, py2, 3, Rgb([255, 220, 0]));
        }
        save_png(&img, "/tmp/fellytip_underground/02_underground_full.png");
    }

    // ── Screenshot 4: Zoomed in on underground capital centre ─────────────────
    eprintln!("[4/5] Rendering underground capital (zoomed)…");
    {
        let half = 80usize;
        let x0 = cap_x.saturating_sub(half);
        let y0 = cap_y.saturating_sub(half);
        let rw = (half * 2).min(MAP_WIDTH - x0);
        let rh = (half * 2).min(MAP_HEIGHT - y0);
        let scale = 4u32;
        let mut img = render_cave_region(&map, CAVE_DEPTH, x0, y0, rw, rh, scale);
        // Mark capital centre (red dot)
        let cpx = ((cap_x - x0) as u32) * scale + scale / 2;
        let cpy = ((cap_y - y0) as u32) * scale + scale / 2;
        draw_marker(&mut img, cpx, cpy, 8, Rgb([255, 50, 50]));
        let (iw, ih) = (img.width(), img.height());
        draw_border(&mut img, cpx.saturating_sub(12), cpy.saturating_sub(12),
            (cpx + 12).min(iw - 1), (cpy + 12).min(ih - 1),
            Rgb([255, 255, 255]), 2);
        // Mark any portals visible in this region
        for (ptx, pty) in &portal_tiles {
            if *ptx >= x0 && *ptx < x0 + rw && *pty >= y0 && *pty < y0 + rh {
                let ppx = ((*ptx - x0) as u32) * scale + scale / 2;
                let ppy = ((*pty - y0) as u32) * scale + scale / 2;
                draw_marker(&mut img, ppx, ppy, 6, Rgb([255, 220, 0]));
            }
        }
        save_png(&img, "/tmp/fellytip_underground/03_capital_centre.png");
    }

    // ── Screenshot 5: Path overlay from capital to nearest portal ─────────────
    eprintln!("[5/5] Rendering path from underground capital to surface portal…");
    {
        if portal_tiles.is_empty() {
            eprintln!("  No portals — skipping path screenshot");
        } else {
            // Pick the nearest portal to the capital
            let nearest_portal = portal_tiles.iter().min_by_key(|(px, py)| {
                let dx = *px as i64 - cap_x as i64;
                let dy = *py as i64 - cap_y as i64;
                dx * dx + dy * dy
            }).copied().unwrap();

            // Determine bounding box to show both capital and portal
            let all_x = [cap_x, nearest_portal.0];
            let all_y = [cap_y, nearest_portal.1];
            let min_x = *all_x.iter().min().unwrap();
            let max_x = *all_x.iter().max().unwrap();
            let min_y = *all_y.iter().min().unwrap();
            let max_y = *all_y.iter().max().unwrap();
            let pad = 80usize;
            let x0 = min_x.saturating_sub(pad);
            let y0 = min_y.saturating_sub(pad);
            let x1 = (max_x + pad).min(MAP_WIDTH - 1);
            let y1 = (max_y + pad).min(MAP_HEIGHT - 1);
            let rw = x1 - x0 + 1;
            let rh = y1 - y0 + 1;
            let scale = 2u32;

            // Render cave layer base
            let mut img = render_cave_region(&map, CAVE_DEPTH, x0, y0, rw, rh, scale);

            // Overlay surface portal tile colours in top portion
            // (show a blended strip: top half surface, bottom half cave)
            // Draw surface layer in a band around portal position
            let portal_band_half = 40usize;
            let pb_x0 = nearest_portal.0.saturating_sub(portal_band_half).max(x0);
            let pb_x1 = (nearest_portal.0 + portal_band_half).min(x0 + rw - 1);
            let pb_y0 = nearest_portal.1.saturating_sub(portal_band_half).max(y0);
            let pb_y1 = (nearest_portal.1 + portal_band_half).min(y0 + rh - 1);
            for iy in pb_y0..=pb_y1 {
                for ix in pb_x0..=pb_x1 {
                    if ix < map.width && iy < map.height {
                        let col = map.column(ix, iy);
                        let kind = col.layers.iter()
                            .rfind(|l| l.is_surface_kind() || l.kind == TileKind::CavePortal)
                            .map(|l| l.kind)
                            .unwrap_or(TileKind::Void);
                        let color = tile_color(kind);
                        // Blend: slightly brightened to indicate "surface zone"
                        let blended = Rgb([
                            ((color[0] as u16 + 60).min(255)) as u8,
                            ((color[1] as u16 + 60).min(255)) as u8,
                            ((color[2] as u16 + 30).min(255)) as u8,
                        ]);
                        let px = ((ix - x0) as u32) * scale;
                        let py = ((iy - y0) as u32) * scale;
                        for sy in 0..scale {
                            for sx in 0..scale {
                                img.put_pixel(px + sx, py + sy, blended);
                            }
                        }
                    }
                }
            }

            // Draw path line: capital → nearest portal
            let cap_px  = ((cap_x - x0) as i32) * scale as i32 + scale as i32 / 2;
            let cap_py  = ((cap_y - y0) as i32) * scale as i32 + scale as i32 / 2;
            let port_px = ((nearest_portal.0 - x0) as i32) * scale as i32 + scale as i32 / 2;
            let port_py = ((nearest_portal.1 - y0) as i32) * scale as i32 + scale as i32 / 2;

            // Draw thick path (3 px width)
            for off in -1i32..=1 {
                draw_line(&mut img, cap_px + off, cap_py, port_px + off, port_py, Rgb([255, 80, 255]));
                draw_line(&mut img, cap_px, cap_py + off, port_px, port_py + off, Rgb([255, 80, 255]));
            }

            // Mark capital (red)
            let (iw2, ih2) = (img.width() as i32, img.height() as i32);
            draw_marker(&mut img, cap_px as u32, cap_py as u32, 8, Rgb([255, 50, 50]));
            draw_border(&mut img, (cap_px - 12).max(0) as u32, (cap_py - 12).max(0) as u32,
                (cap_px + 12).min(iw2 - 1) as u32,
                (cap_py + 12).min(ih2 - 1) as u32,
                Rgb([255, 255, 255]), 2);

            // Mark portal (gold)
            draw_marker(&mut img, port_px as u32, port_py as u32, 8, Rgb([255, 220, 0]));
            draw_border(&mut img, (port_px - 12).max(0) as u32, (port_py - 12).max(0) as u32,
                (port_px + 12).min(iw2 - 1) as u32,
                (port_py + 12).min(ih2 - 1) as u32,
                Rgb([255, 255, 255]), 2);

            // Mark all portals in view
            for (ptx, pty) in &portal_tiles {
                if *ptx >= x0 && *ptx < x0 + rw && *pty >= y0 && *pty < y0 + rh {
                    let ppx = ((*ptx - x0) as u32) * scale + scale / 2;
                    let ppy = ((*pty - y0) as u32) * scale + scale / 2;
                    if (*ptx, *pty) != nearest_portal {
                        draw_marker(&mut img, ppx, ppy, 5, Rgb([255, 200, 0]));
                    }
                }
            }

            save_png(&img, "/tmp/fellytip_underground/04_path_to_surface.png");

            eprintln!("  Capital: ({cap_x}, {cap_y})  →  Nearest portal: ({}, {})",
                nearest_portal.0, nearest_portal.1);
            eprintln!("  Distance: {} tiles",
                (((nearest_portal.0 as i64 - cap_x as i64).pow(2)
                + (nearest_portal.1 as i64 - cap_y as i64).pow(2)) as f64).sqrt() as u64);
        }
    }

    eprintln!("\n=== Done. Screenshots in /tmp/fellytip_underground/ ===");
}
