//! Chunk coordinate type and the pure mesh-builder that converts `WorldMap`
//! tile data into a smooth `Mesh`.
//!
//! # Vertex derivation
//!
//! Every `TileLayer` stores `corner_offsets: [TL, TR, BL, BR]`.  Each corner
//! height is the average of the four tile-center `z_top` values that share
//! that corner — computed symmetrically, so all four tiles touching a corner
//! arrive at the **same** value.
//!
//! The vertex at tile-grid position `(gx, gy)` is the TL corner of tile
//! `(gx, gy)`, so its height = `layer.z_top + layer.corner_offsets[0]`.
//!
//! The right-edge of chunk A and the left-edge of chunk B both read the TL
//! corner of the same shared column → identical heights → seamless mesh.

use std::collections::HashSet;

use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    render::render_resource::PrimitiveTopology,
};
use fellytip_shared::world::map::{TileKind, WorldMap};

use super::lod::{EdgeTransitions, LodLevel, CHUNK_TILES};
use super::material::{biome_color, corner_biome_color};

// ── Chunk coordinate ──────────────────────────────────────────────────────────

/// Integer chunk address in the chunk grid.
///
/// Tile column `ix = cx * CHUNK_TILES`, tile row `iy = cy * CHUNK_TILES`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChunkCoord {
    pub cx: i32,
    pub cy: i32,
}

impl ChunkCoord {
    /// World-space centre of this chunk in Bevy coordinates (X east, Y up, Z south).
    pub fn world_center(self, map: &WorldMap) -> Vec3 {
        let half_w = (map.width  / 2) as f32;
        let half_h = (map.height / 2) as f32;
        let bx = self.cx as f32 * CHUNK_TILES as f32 + CHUNK_TILES as f32 * 0.5 - half_w;
        let bz = self.cy as f32 * CHUNK_TILES as f32 + CHUNK_TILES as f32 * 0.5 - half_h;
        Vec3::new(bx, 0.0, bz)
    }

    /// Chunk coord containing tile `(ix, iy)`.
    pub fn from_tile(ix: i32, iy: i32) -> Self {
        Self {
            cx: ix.div_euclid(CHUNK_TILES as i32),
            cy: iy.div_euclid(CHUNK_TILES as i32),
        }
    }
}

// ── Mesh builder ──────────────────────────────────────────────────────────────

/// Build a smooth `Mesh` for chunk `coord` at the given LOD.
///
/// Heights come from `TileLayer::corner_offsets[0]` (TL corner of each tile).
/// Normals are computed via central differences.  Vertex colours are the
/// 4-tile biome-colour average at each corner — same averaging as heights.
///
/// `transitions` marks edges that face a neighbour at one coarser LOD level.
/// Those edges receive T-collapse stitching that eliminates visible cracks.
pub fn build_chunk_mesh(
    map:         &WorldMap,
    coord:       ChunkCoord,
    lod:         LodLevel,
    transitions: EdgeTransitions,
) -> Mesh {
    let step  = lod.step() as i32;
    let vps   = lod.verts_per_side(); // vertices per side
    let half_w = (map.width  / 2) as i32;
    let half_h = (map.height / 2) as i32;

    // ── Vertex positions, colours, and cached heights for normal computation ──

    let n_verts = vps * vps;
    let mut positions = Vec::<[f32; 3]>::with_capacity(n_verts);
    let mut colors    = Vec::<[f32; 4]>::with_capacity(n_verts);
    let mut h_grid    = vec![0.0f32; n_verts];

    for vy in 0..vps {
        for vx in 0..vps {
            let gx = (coord.cx * CHUNK_TILES as i32 + vx as i32 * step)
                .clamp(0, map.width  as i32 - 1) as usize;
            let gy = (coord.cy * CHUNK_TILES as i32 + vy as i32 * step)
                .clamp(0, map.height as i32 - 1) as usize;

            let h = vertex_height(map, gx, gy);
            h_grid[vy * vps + vx] = h;

            let bx = gx as i32 as f32 - half_w as f32;
            let bz = gy as i32 as f32 - half_h as f32;
            positions.push([bx, h, bz]);
            colors.push(corner_biome_color(map, gx, gy));
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

            // Tile-space distance between samples (in world units = step tiles).
            let span = if vx == 0 || vx == max_v || vy == 0 || vy == max_v {
                step as f32         // edge: one-sided difference, span = step
            } else {
                2.0 * step as f32   // interior: central difference, span = 2*step
            };

            let dx = (h_r - h_l) / span;
            let dz = (h_d - h_u) / span; // +vy → +Bevy Z (south, descending iy)
            let n  = Vec3::new(-dx, 1.0, -dz).normalize();
            normals.push([n.x, n.y, n.z]);
        }
    }

    // ── Index buffer — standard CCW quad triangulation ────────────────────────

    let mut indices = Vec::<u32>::with_capacity((vps - 1) * (vps - 1) * 6);

    for vy in 0..(vps - 1) {
        for vx in 0..(vps - 1) {
            let i00 = (vy       * vps + vx    ) as u32;
            let i10 = (vy       * vps + vx + 1) as u32;
            let i01 = ((vy + 1) * vps + vx    ) as u32;
            let i11 = ((vy + 1) * vps + vx + 1) as u32;
            // CCW winding viewed from +Y (above). Proof: for a flat quad at z=0,
            // edge1 = i01-i00 = (0,0,+step) and edge2 = i10-i00 = (+step,0,0),
            // so cross = (+Z)×(+X) = (0,+1,0) → normal points up. ✓
            indices.extend_from_slice(&[i00, i01, i10, i10, i01, i11]);
        }
    }

    // ── Edge stitching — T-collapse for LOD-level transitions ─────────────────

    // Convention: "north" = vy=0 (−Z edge), "south" = vy=max (+Z edge),
    //             "west"  = vx=0 (−X edge), "east"  = vx=max (+X edge).
    if transitions.north { stitch_row(&mut indices, vps, 0,       false); }
    if transitions.south { stitch_row(&mut indices, vps, vps - 1, true ); }
    if transitions.west  { stitch_col(&mut indices, vps, 0,       false); }
    if transitions.east  { stitch_col(&mut indices, vps, vps - 1, true ); }

    // ── Assemble mesh ─────────────────────────────────────────────────────────

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL,   normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR,    colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

// ── Height helper ─────────────────────────────────────────────────────────────

/// Height of the terrain vertex at tile-grid position `(gx, gy)`.
///
/// A vertex at `(gx, gy)` is the TL corner of tile `(gx, gy)`,
/// stored as `corner_offsets[0]`.
fn vertex_height(map: &WorldMap, gx: usize, gy: usize) -> f32 {
    let col = map.column(gx, gy);
    if let Some(layer) = col.layers.iter().rev().find(|l| l.is_surface_kind()) {
        layer.z_top + layer.corner_offsets[0]
    } else if let Some(layer) = col.layers.last() {
        layer.z_top + layer.corner_offsets[0]
    } else {
        0.0
    }
}

// ── Underground mesh builder ─────────────────────────────────────────────────

/// Build a flat mesh for underground tier `kind` in chunk `coord`.
///
/// Underground floors have `corner_offsets = [0.0; 4]`, so no height
/// interpolation or LOD stitching is needed — each open cell is a flat quad
/// at the layer's fixed `z_top`.  Solid cells (no walkable layer of `kind`)
/// are skipped, producing the natural cave-wall gaps.
pub fn build_underground_chunk_mesh(map: &WorldMap, coord: ChunkCoord, kind: TileKind) -> Mesh {
    let half_w = (map.width  / 2) as i32;
    let half_h = (map.height / 2) as i32;
    let c = biome_color(kind);
    let col_arr = [c.x, c.y, c.z, 1.0];

    let mut positions = Vec::<[f32; 3]>::new();
    let mut normals   = Vec::<[f32; 3]>::new();
    let mut colors    = Vec::<[f32; 4]>::new();
    let mut indices   = Vec::<u32>::new();

    for dy in 0..CHUNK_TILES {
        for dx in 0..CHUNK_TILES {
            let ix = (coord.cx as usize * CHUNK_TILES + dx).min(map.width  - 1);
            let iy = (coord.cy as usize * CHUNK_TILES + dy).min(map.height - 1);
            let Some(layer) = map.column(ix, iy)
                .layers.iter().find(|l| l.kind == kind && l.walkable)
            else {
                continue;
            };

            let y  = layer.z_top;
            let bx = ix as f32 - half_w as f32;
            let bz = iy as f32 - half_h as f32;
            let b  = positions.len() as u32;

            positions.extend_from_slice(&[
                [bx,       y, bz      ],   // TL
                [bx + 1.0, y, bz      ],   // TR
                [bx,       y, bz + 1.0],   // BL
                [bx + 1.0, y, bz + 1.0],   // BR
            ]);
            normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
            colors.extend_from_slice(&[col_arr; 4]);
            // CCW from +Y: TL→BL→TR, TR→BL→BR
            indices.extend_from_slice(&[b, b + 2, b + 1,  b + 1, b + 2, b + 3]);
        }
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL,   normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR,    colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

// ── Edge stitching ────────────────────────────────────────────────────────────
//
// When an adjacent chunk is at LOD N+1 (one coarser level), its shared edge
// has half as many vertices. Without stitching, T-junctions appear as cracks.
//
// Fix: remove triangles that contain an odd-indexed vertex on the fine edge
// and add three replacement triangles that collapse each adjacent pair of fine
// vertices to the position of the corresponding coarse vertex.
//
// LOD transitions are constrained to ±1 level (enforced by the chunk manager),
// so only a 2:1 ratio ever occurs.

/// Remove and re-triangulate the row at `vy_edge` with T-collapse stitching.
///
/// Triangles are processed in groups of 3 indices (whole triangles).
fn stitch_row(indices: &mut Vec<u32>, vps: usize, vy_edge: usize, is_south: bool) {
    let vps = vps as u32;
    let vy_e = vy_edge as u32;

    // Odd-vx vertices on the edge row — these do not exist on the coarse neighbour.
    let odd_edge: HashSet<u32> = (1..vps)
        .step_by(2)
        .map(|vx| vy_e * vps + vx)
        .collect();

    // Remove entire triangles that reference any odd edge vertex.
    filter_triangles(indices, &odd_edge);

    // Inner row for stitching triangles.
    let vy_inner = if is_south { vy_e - 1 } else { vy_e + 1 };

    // For each even pair (k, k+2) on the edge, emit 3 stitching triangles
    // that merge via the interior neighbours.
    let mut k = 0u32;
    while k + 2 <= vps - 1 {
        let e0 = vy_e     * vps + k;
        let e2 = vy_e     * vps + k + 2;
        let m0 = vy_inner * vps + k;
        let m1 = vy_inner * vps + k + 1;
        let m2 = vy_inner * vps + k + 2;

        // All triangles must be CCW from +Y (same rule as main quads).
        // North: vy_inner > vy_e (larger Z, south of edge).
        //   [e0,m1,e2]: (1,+1)×(2,0) → b·c−a·d = 2 > 0 ✓
        //   [e0,m0,m1]: (0,+1)×(1,+1) → 1 > 0 ✓
        //   [e2,m1,m2]: (−1,+1)×(0,+1) → 1 > 0 ✓
        // South: vy_inner < vy_e (smaller Z, north of edge).
        //   [e0,e2,m1]: (2,0)×(1,−1) → 2 > 0 ✓
        //   [e0,m1,m0]: (1,−1)×(0,−1) → 1 > 0 ✓
        //   [e2,m2,m1]: (0,−1)×(−1,−1) → 1 > 0 ✓
        if is_south {
            indices.extend_from_slice(&[e0, e2, m1,  e0, m1, m0,  e2, m2, m1]);
        } else {
            indices.extend_from_slice(&[e0, m1, e2,  e0, m0, m1,  e2, m1, m2]);
        }
        k += 2;
    }
}

/// Remove and re-triangulate the column at `vx_edge` with T-collapse stitching.
fn stitch_col(indices: &mut Vec<u32>, vps: usize, vx_edge: usize, is_east: bool) {
    let vps = vps as u32;
    let vx_e = vx_edge as u32;

    let odd_edge: HashSet<u32> = (1..vps)
        .step_by(2)
        .map(|vy| vy * vps + vx_e)
        .collect();

    filter_triangles(indices, &odd_edge);

    let vx_inner = if is_east { vx_e - 1 } else { vx_e + 1 };

    let mut k = 0u32;
    while k + 2 <= vps - 1 {
        let e0 = k       * vps + vx_e;
        let e2 = (k + 2) * vps + vx_e;
        let m0 = k       * vps + vx_inner;
        let m1 = (k + 1) * vps + vx_inner;
        let m2 = (k + 2) * vps + vx_inner;

        // East: vx_inner < vx_e (west of edge, smaller X).
        //   [e0,m1,e2]: (−1,+1)×(0,+2) → 2 > 0 ✓
        //   [e0,m0,m1]: (−1,0)×(−1,+1) → 1 > 0 ✓
        //   [e2,m1,m2]: (−1,−1)×(−1,0) → 1 > 0 ✓
        // West: vx_inner > vx_e (east of edge, larger X).
        //   [e0,e2,m1]: (0,+2)×(+1,+1) → 2 > 0 ✓
        //   [e0,m1,m0]: (+1,+1)×(+1,0) → 1 > 0 ✓
        //   [e2,m2,m1]: (+1,0)×(+1,−1) → 1 > 0 ✓
        if is_east {
            indices.extend_from_slice(&[e0, m1, e2,  e0, m0, m1,  e2, m1, m2]);
        } else {
            indices.extend_from_slice(&[e0, e2, m1,  e0, m1, m0,  e2, m2, m1]);
        }
        k += 2;
    }
}

/// Remove entire triangles (groups of 3 indices) that contain any index in `bad`.
fn filter_triangles(indices: &mut Vec<u32>, bad: &HashSet<u32>) {
    let kept: Vec<u32> = indices
        .chunks_exact(3)
        .filter(|tri| !tri.iter().any(|i| bad.contains(i)))
        .flatten()
        .copied()
        .collect();
    *indices = kept;
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
