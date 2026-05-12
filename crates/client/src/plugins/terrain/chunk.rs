//! Renderer-side wrapper around `fellytip_world_types::mesh::build_chunk_geometry`.
//!
//! The pure builder lives in `world-types` so the physics layer can consume the
//! same vertex/index buffers. This file wraps the buffers in a Bevy `Mesh`,
//! computing per-vertex normals, UVs and biome colours on top.

use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    render::render_resource::PrimitiveTopology,
};
use fellytip_shared::world::map::WorldMap;
use fellytip_shared::world::mesh::{build_chunk_geometry, ChunkGeometry};

// Re-export pure types so existing `super::chunk::ChunkCoord` etc. callers
// (camera, water_material, scene_decoration, manager) keep working.
pub use fellytip_shared::world::mesh::{vertex_height, ChunkCoord, EdgeTransitions};

use super::lod::{LodLevel, CHUNK_TILES};
use super::material::corner_biome_color;

// ── ChunkCoord convenience for renderer ───────────────────────────────────────

/// World-space centre of this chunk in Bevy coordinates (X east, Y up, Z south).
///
/// Renderer-only helper that adapts the pure-data `world_center_xz` to `Vec3`.
pub fn chunk_world_center(coord: ChunkCoord, map: &WorldMap) -> Vec3 {
    let (bx, bz) = coord.world_center_xz(map);
    Vec3::new(bx, 0.0, bz)
}

// ── Mesh builder ──────────────────────────────────────────────────────────────

/// Build a smooth `Mesh` for chunk `coord` at the given LOD.
///
/// Positions + indices are produced by `world-types::mesh::build_chunk_geometry`
/// (the same data the physics layer consumes). This wrapper computes the
/// renderer-only attributes (normals via central differences, biome-blended
/// vertex colours, planar UVs) and assembles the `Mesh`.
pub fn build_chunk_mesh(
    map:         &WorldMap,
    coord:       ChunkCoord,
    lod:         LodLevel,
    transitions: EdgeTransitions,
) -> Mesh {
    let ChunkGeometry { positions, indices, vps } =
        build_chunk_geometry(map, coord, lod.step(), transitions);

    let step  = lod.step() as i32;
    let half_w = (map.width  / 2) as i32;
    let n_verts = positions.len();

    // Reconstruct the height grid from positions (row-major, Y component).
    let h_grid: Vec<f32> = positions.iter().map(|p| p[1]).collect();

    // ── Per-vertex colours + UVs ──────────────────────────────────────────────
    const UV_SCALE: f32 = 0.25;

    let mut uvs    = Vec::<[f32; 2]>::with_capacity(n_verts);
    let mut colors = Vec::<[f32; 4]>::with_capacity(n_verts);

    for vy in 0..vps {
        for vx in 0..vps {
            let gx = (coord.cx * CHUNK_TILES as i32 + vx as i32 * step)
                .clamp(0, map.width  as i32 - 1) as usize;
            let gy = (coord.cy * CHUNK_TILES as i32 + vy as i32 * step)
                .clamp(0, map.height as i32 - 1) as usize;
            let bx = gx as f32 - half_w as f32;
            let bz = gy as f32 - (map.height as i32 / 2) as f32;
            uvs.push([bx * UV_SCALE, bz * UV_SCALE]);

            let h = h_grid[vy * vps + vx];
            let base = corner_biome_color(map, gx, gy);
            let tx = gx as i32;
            let tz = gy as i32;
            let height_factor = (0.85 + (h / 20.0).clamp(0.0, 1.0) * 0.30).clamp(0.85, 1.15);
            let r = (base[0] + tile_color_noise(tx, tz, 0)) * height_factor;
            let g = (base[1] + tile_color_noise(tx, tz, 1)) * height_factor;
            let b = (base[2] + tile_color_noise(tx, tz, 2)) * height_factor;
            colors.push([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), base[3]]);
        }
    }

    // ── Smooth normals (central differences over height grid) ─────────────────
    let max_v = vps - 1;
    let mut normals = Vec::<[f32; 3]>::with_capacity(n_verts);
    for vy in 0..vps {
        for vx in 0..vps {
            let vx_l = if vx > 0     { vx - 1 } else { vx };
            let vx_r = if vx < max_v { vx + 1 } else { vx };
            let vy_u = if vy > 0     { vy - 1 } else { vy };
            let vy_d = if vy < max_v { vy + 1 } else { vy };

            let h_l = h_grid[vy   * vps + vx_l];
            let h_r = h_grid[vy   * vps + vx_r];
            let h_u = h_grid[vy_u * vps + vx];
            let h_d = h_grid[vy_d * vps + vx];

            let span = if vx == 0 || vx == max_v || vy == 0 || vy == max_v {
                step as f32
            } else {
                2.0 * step as f32
            };
            let dx = (h_r - h_l) / span;
            let dz = (h_d - h_u) / span;
            let n  = Vec3::new(-dx, 1.0, -dz).normalize();
            normals.push([n.x, n.y, n.z]);
        }
    }

    // ── Assemble mesh ─────────────────────────────────────────────────────────
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL,   normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0,     uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR,    colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}


/// Deterministic per-vertex color noise based on tile coordinates and channel.
///
/// Returns a value in `[-0.08, +0.08]` — roughly ±8% brightness variation.
/// Uses only integer arithmetic until the final float conversion, so the result
/// is identical every time the same tile is processed (chunk rebuilds are stable).
fn tile_color_noise(tile_x: i32, tile_z: i32, channel: u32) -> f32 {
    let h = tile_x
        .wrapping_mul(127)
        .wrapping_add(tile_z.wrapping_mul(311))
        .wrapping_add(channel.wrapping_mul(17) as i32) as u32;
    let h = h ^ (h >> 16);
    let h = h.wrapping_mul(0x45d9f3b);
    let h = h ^ (h >> 16);
    (h & 0xFF) as f32 / 255.0 * 0.16 - 0.08
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;
    use fellytip_shared::world::map::generate_map;

    fn test_map() -> WorldMap {
        generate_map(42, 64, 64)
    }

    fn get_positions(mesh: &Mesh) -> &Vec<[f32; 3]> {
        match mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() {
            VertexAttributeValues::Float32x3(v) => v,
            _ => panic!("unexpected attribute type"),
        }
    }

    #[test]
    fn vertex_count_lod_full() {
        let map  = test_map();
        let mesh = build_chunk_mesh(&map, ChunkCoord { cx: 0, cy: 0 }, LodLevel::Full, EdgeTransitions::default());
        assert_eq!(get_positions(&mesh).len(), 33 * 33);
    }

    #[test]
    fn vertex_count_lod_half() {
        let map  = test_map();
        let mesh = build_chunk_mesh(&map, ChunkCoord { cx: 0, cy: 0 }, LodLevel::Half, EdgeTransitions::default());
        assert_eq!(get_positions(&mesh).len(), 17 * 17);
    }

    #[test]
    fn vertex_count_lod_quarter() {
        let map  = test_map();
        let mesh = build_chunk_mesh(&map, ChunkCoord { cx: 0, cy: 0 }, LodLevel::Quarter, EdgeTransitions::default());
        assert_eq!(get_positions(&mesh).len(), 9 * 9);
    }

    #[test]
    fn vertex_heights_match_corner_offsets() {
        let map  = test_map();
        let mesh = build_chunk_mesh(&map, ChunkCoord { cx: 0, cy: 0 }, LodLevel::Full, EdgeTransitions::default());
        let pos  = get_positions(&mesh);

        for vy in 0..33usize {
            for vx in 0..33usize {
                let expected = vertex_height(&map, vx, vy);
                let actual   = pos[vy * 33 + vx][1]; // Bevy Y = height
                assert!(
                    (actual - expected).abs() < 1e-5,
                    "vertex ({vx},{vy}) height: got {actual}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn adjacent_chunks_share_edge_heights() {
        let map = test_map();
        let a = build_chunk_mesh(&map, ChunkCoord { cx: 0, cy: 0 }, LodLevel::Full, EdgeTransitions::default());
        let b = build_chunk_mesh(&map, ChunkCoord { cx: 1, cy: 0 }, LodLevel::Full, EdgeTransitions::default());
        let pa = get_positions(&a);
        let pb = get_positions(&b);

        // Chunk A right edge (vx=32) == Chunk B left edge (vx=0) for all rows.
        for vy in 0..33usize {
            let y_a = pa[vy * 33 + 32][1];
            let y_b = pb[vy * 33 +  0][1];
            assert!(
                (y_a - y_b).abs() < 1e-5,
                "seam at vy={vy}: A_right={y_a} B_left={y_b}"
            );
        }
    }
}
