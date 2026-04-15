//! Flat water mesh builder for `Water` and `River` tiles.
//!
//! Each water tile in a chunk becomes one independent flat quad sitting
//! `WATER_OFFSET` world units above the terrain surface.  Unlike the terrain
//! mesh, there is no vertex sharing or LOD stitching — flat quads at identical
//! Y produce no T-junction cracks.
//!
//! Vertex colour encoding:
//!  - RGB  — biome colour (Water: deep blue, River: lighter blue)
//!  - Alpha — **tile-type flag**: `1.0` = open Water, `0.0` = River.
//!    The water shader reads `in.color.a` and mixes between slow isotropic
//!    drift (ocean) and fast directional flow (river).

use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    render::render_resource::PrimitiveTopology,
};
use fellytip_shared::world::map::{TileKind, WorldMap};

use super::chunk::ChunkCoord;
use super::lod::{LodLevel, CHUNK_TILES};
use super::material::biome_color;

/// Y-axis offset applied to water quads to sit just above the terrain mesh
/// and prevent Z-fighting on flat water tiles.
const WATER_OFFSET: f32 = 0.02;

/// Build a flat water mesh for `coord` at the given LOD.
///
/// Returns `None` when the chunk contains no Water or River tiles — the caller
/// should skip spawning a water entity for that chunk.
///
/// Each sampled tile that is `Water` or `River` contributes one flat quad.
/// Quad size in world units equals `lod.step()` (coarser LOD → larger quads).
pub fn build_water_mesh(map: &WorldMap, coord: ChunkCoord, lod: LodLevel) -> Option<Mesh> {
    let step     = lod.step() as i32;
    let half_w   = (map.width  / 2) as i32;
    let half_h   = (map.height / 2) as i32;
    let tiles_per_side = CHUNK_TILES as i32 / step;

    let mut positions = Vec::<[f32; 3]>::new();
    let mut normals   = Vec::<[f32; 3]>::new();
    let mut colors    = Vec::<[f32; 4]>::new();
    let mut indices   = Vec::<u32>::new();

    for qy in 0..tiles_per_side {
        for qx in 0..tiles_per_side {
            let tx = (coord.cx * CHUNK_TILES as i32 + qx * step)
                .clamp(0, map.width  as i32 - 1) as usize;
            let ty = (coord.cy * CHUNK_TILES as i32 + qy * step)
                .clamp(0, map.height as i32 - 1) as usize;

            let col = map.column(tx, ty);
            let Some(layer) = col.layers.iter().rev().find(|l| l.is_surface_kind()) else {
                continue;
            };

            if !matches!(layer.kind, TileKind::Water | TileKind::River) {
                continue;
            }

            let y = layer.z_top + WATER_OFFSET;

            // Alpha channel encodes tile type for the shader:
            //   1.0 = open Water (slow isotropic drift)
            //   0.0 = River      (fast directional flow)
            let is_water_flag = if layer.kind == TileKind::Water { 1.0f32 } else { 0.0 };
            let rgb   = biome_color(layer.kind);
            let color = [rgb.x, rgb.y, rgb.z, is_water_flag];

            // World-space quad corners.
            let bx = tx as f32 - half_w as f32;
            let bz = ty as f32 - half_h as f32;
            let ex = bx + step as f32;
            let ez = bz + step as f32;

            // 4 vertices: TL, TR, BL, BR
            let base = positions.len() as u32;
            positions.extend_from_slice(&[
                [bx, y, bz], // TL
                [ex, y, bz], // TR
                [bx, y, ez], // BL
                [ex, y, ez], // BR
            ]);
            normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
            colors.extend_from_slice(&[color; 4]);

            // CCW winding viewed from +Y: TL, BL, TR | TR, BL, BR.
            indices.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
        }
    }

    if positions.is_empty() {
        return None;
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL,   normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR,    colors);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}
