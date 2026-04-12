//! Terrain material helpers: biome colour lookup and vertex-colour averaging.
//!
//! All colours are sourced from the original `material_for()` in `tile_renderer.rs`.
//! The terrain system shares a single `StandardMaterial { vertex_colors: true }` so
//! every chunk uses one draw-call regardless of how many biomes it spans.

use bevy::math::Vec3;
use bevy::prelude::*;
use fellytip_shared::world::map::{TileKind, WorldMap};

/// Linear-sRGB base colour for each `TileKind`, matching `tile_renderer::material_for`.
pub fn biome_color(kind: TileKind) -> Vec3 {
    match kind {
        TileKind::Plains              => Vec3::new(0.45, 0.65, 0.30),
        TileKind::Grassland           => Vec3::new(0.35, 0.72, 0.25),
        TileKind::Forest              => Vec3::new(0.12, 0.45, 0.12),
        TileKind::TemperateForest     => Vec3::new(0.18, 0.50, 0.18),
        TileKind::TropicalForest      => Vec3::new(0.08, 0.52, 0.20),
        TileKind::TropicalRainforest  => Vec3::new(0.04, 0.48, 0.15),
        TileKind::Taiga               => Vec3::new(0.22, 0.40, 0.22),
        TileKind::Savanna             => Vec3::new(0.76, 0.68, 0.30),
        TileKind::Desert              => Vec3::new(0.86, 0.76, 0.45),
        TileKind::Tundra              => Vec3::new(0.62, 0.68, 0.58),
        TileKind::PolarDesert         => Vec3::new(0.82, 0.87, 0.90),
        TileKind::Arctic              => Vec3::new(0.92, 0.95, 0.98),
        TileKind::Mountain            => Vec3::new(0.55, 0.50, 0.48),
        TileKind::Stone               => Vec3::new(0.50, 0.48, 0.45),
        TileKind::Water               => Vec3::new(0.15, 0.40, 0.75),
        TileKind::River               => Vec3::new(0.22, 0.52, 0.88),
        TileKind::Cavern              => Vec3::new(0.25, 0.22, 0.20),
        TileKind::DeepRock            => Vec3::new(0.18, 0.16, 0.15),
        TileKind::LuminousGrotto      => Vec3::new(0.10, 0.35, 0.32),
        TileKind::Tunnel              => Vec3::new(0.15, 0.15, 0.14),
        TileKind::Void                => Vec3::ZERO,
    }
}

/// Blended vertex colour at tile-grid corner `(gx, gy)`.
///
/// A corner is shared by the four tiles `(gx-1,gy-1)`, `(gx,gy-1)`,
/// `(gx-1,gy)`, `(gx,gy)`. Their biome colours are averaged with equal weight,
/// producing smooth colour gradients at biome boundaries — the same averaging
/// logic that already smooths heights via `corner_offsets`.
pub fn corner_biome_color(map: &WorldMap, gx: usize, gy: usize) -> [f32; 4] {
    let mut sum = Vec3::ZERO;
    let mut count = 0u32;

    for (dx, dy) in [(-1i32, -1i32), (0, -1), (-1, 0), (0, 0)] {
        let ix = gx as i32 + dx;
        let iy = gy as i32 + dy;
        if ix >= 0 && iy >= 0 && (ix as usize) < map.width && (iy as usize) < map.height {
            let col = map.column(ix as usize, iy as usize);
            if let Some(layer) = col.layers.iter().rev().find(|l| l.is_surface_kind()) {
                sum += biome_color(layer.kind);
                count += 1;
            }
        }
    }

    if count > 0 {
        sum /= count as f32;
    }
    [sum.x, sum.y, sum.z, 1.0]
}

/// Create the one shared terrain material.
///
/// Bevy 0.18 applies `ATTRIBUTE_COLOR` vertex data automatically when the mesh
/// carries that attribute — no field needed on `StandardMaterial`.  Setting
/// `base_color = WHITE` makes the PBR base-colour a neutral identity so the
/// vertex colour passes through unmodified.
pub fn create_terrain_material(
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.88,
        metallic: 0.0,
        reflectance: 0.3,
        ..default()
    })
}
