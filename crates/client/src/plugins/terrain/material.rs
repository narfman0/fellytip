//! Terrain material helpers: biome colour lookup, vertex-colour averaging,
//! and per-biome ground texture selection.
//!
//! Each terrain chunk samples the dominant `TileKind` at its centre and is
//! assigned a `StandardMaterial` whose `base_color_texture` matches the biome
//! (grass, sand, mud, etc.).  Vertex colours from `corner_biome_color` still
//! tint the texture at each corner, providing smooth cross-biome blending.

use std::collections::HashMap;

use bevy::math::Vec3;
use bevy::prelude::*;
use fellytip_shared::world::map::{TileKind, WorldMap};

// ── Biome regions ─────────────────────────────────────────────────────────────

/// Coarse grouping of `TileKind` variants that share the same ground texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BiomeRegion {
    /// Grassland, plains, forests, taiga — green grass texture.
    Temperate,
    /// Tropical and rainforest biomes — darker green grass texture.
    Tropical,
    /// Desert — sand texture.
    Desert,
    /// Savanna — lighter sand texture.
    Savanna,
    /// Mountain, stone, rivers — mud/dirt texture for rocky ground.
    Stone,
    /// Tundra, polar desert, arctic — lightest grass texture tinted near-white
    /// by vertex colours.
    Snow,
    /// Cave tiles — dark mud texture (vertex colours carry most of the signal).
    Cave,
}

/// Map a `TileKind` to its coarse `BiomeRegion` for texture selection.
pub fn tilekind_to_biome_region(kind: TileKind) -> BiomeRegion {
    match kind {
        TileKind::Plains
        | TileKind::Grassland
        | TileKind::Forest
        | TileKind::TemperateForest
        | TileKind::Taiga => BiomeRegion::Temperate,
        TileKind::TropicalForest | TileKind::TropicalRainforest => BiomeRegion::Tropical,
        TileKind::Desert => BiomeRegion::Desert,
        TileKind::Savanna => BiomeRegion::Savanna,
        TileKind::Mountain | TileKind::Stone | TileKind::River => BiomeRegion::Stone,
        TileKind::Tundra | TileKind::PolarDesert | TileKind::Arctic => BiomeRegion::Snow,
        TileKind::Water => BiomeRegion::Temperate,
        TileKind::CaveFloor
        | TileKind::CaveWall
        | TileKind::CrystalCave
        | TileKind::LavaFloor
        | TileKind::CaveRiver
        | TileKind::CavePortal
        | TileKind::Void => BiomeRegion::Cave,
    }
}

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
        TileKind::CaveFloor           => Vec3::new(0.25, 0.25, 0.25),
        TileKind::CaveWall            => Vec3::new(0.10, 0.10, 0.12),
        TileKind::CrystalCave         => Vec3::new(0.20, 0.70, 0.80),
        TileKind::LavaFloor           => Vec3::new(0.90, 0.30, 0.05),
        TileKind::CaveRiver           => Vec3::new(0.05, 0.15, 0.60),
        TileKind::CavePortal          => Vec3::new(0.80, 0.10, 0.90),
        TileKind::Void                => Vec3::ZERO,
    }
}

/// Apply an art-direction tint multiplier to a biome colour.
///
/// `tint` is a `[f32; 3]` RGB multiplier where `[1.0, 1.0, 1.0]` is neutral.
/// Useful for per-world colour grading (e.g. Sunken Realm blue shift).
/// Called by terrain chunk generation when a `WorldArtDirection` tint is active.
#[allow(dead_code)]
pub fn biome_color_tinted(kind: TileKind, tint: [f32; 3]) -> Vec3 {
    let base = biome_color(kind);
    Vec3::new(base.x * tint[0], base.y * tint[1], base.z * tint[2])
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
            if let Some(layer) = col.layers.iter().rev().find(|l| l.kind != TileKind::Void) {
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

fn make_terrain_material(
    texture_path: &str,
    normal_path: Option<&str>,
    materials: &mut Assets<StandardMaterial>,
    asset_server: &AssetServer,
) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(asset_server.load(texture_path.to_string())),
        normal_map_texture: normal_path
            .map(|p| asset_server.load(p.to_string())),
        // Synty normal maps are authored for Unity (DirectX convention: Y flipped).
        flip_normal_map_y: true,
        perceptual_roughness: 0.88,
        metallic: 0.0,
        reflectance: 0.3,
        ..default()
    })
}

/// Create one `StandardMaterial` per `BiomeRegion`, each using the appropriate
/// Synty ground texture and matching normal map.  UVs are world-space planar
/// (computed per-vertex in the mesh), so no `uv_transform` is needed here.
pub fn create_terrain_materials(
    materials: &mut Assets<StandardMaterial>,
    asset_server: &AssetServer,
) -> HashMap<BiomeRegion, Handle<StandardMaterial>> {
    let mut map = HashMap::new();

    // (base texture, normal map, region)
    let entries: &[(&str, Option<&str>, BiomeRegion)] = &[
        (
            "synty/textures/PFK_Texture_Ground_Grass_01.png",
            Some("synty/textures/PFK_Texture_Ground_Base_Normals.png"),
            BiomeRegion::Temperate,
        ),
        (
            "synty/textures/PFK_Texture_Ground_Grass_01_Dark.png",
            Some("synty/textures/PFK_Texture_Ground_Grass_02_Normal.png"),
            BiomeRegion::Tropical,
        ),
        (
            "synty/textures/PFK_Texture_Ground_Sand_01.png",
            Some("synty/textures/PFK_Texture_Ground_Sand_02_Normal.png"),
            BiomeRegion::Desert,
        ),
        (
            "synty/textures/PFK_Texture_Ground_Sand_02.png",
            Some("synty/textures/PFK_Texture_Ground_Sand_02_Normal.png"),
            BiomeRegion::Savanna,
        ),
        (
            "synty/textures/PFK_Texture_Ground_Mud_01.png",
            Some("synty/textures/PFK_Texture_Ground_Base_Normals.png"),
            BiomeRegion::Stone,
        ),
        (
            "synty/textures/PFK_Texture_Ground_Grass_03.png",
            Some("synty/textures/PFK_Texture_Ground_Grass_03_Normals.png"),
            BiomeRegion::Snow,
        ),
        (
            "synty/textures/PFK_Texture_Ground_Mud_02.png",
            Some("synty/textures/PFK_Texture_Ground_Base_Normals.png"),
            BiomeRegion::Cave,
        ),
    ];

    for (base, normal, region) in entries {
        map.insert(*region, make_terrain_material(base, *normal, materials, asset_server));
    }

    map
}
