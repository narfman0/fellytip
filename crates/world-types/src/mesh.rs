//! Pure chunk-geometry builder, consumed by both the renderer (wraps the
//! result in a Bevy `Mesh`) and the physics layer (wraps it in a trimesh
//! `Collider`). No Bevy types — just `WorldMap` data in, vertex/index buffers
//! out.
//!
//! # Vertex derivation
//!
//! The vertex at tile-grid position `(gx, gy)` is the TL corner of tile
//! `(gx, gy)`. Its height is `TileLayer::z_top + corner_offsets[0]`. All four
//! tiles touching a corner agree on this value, so adjacent chunks share an
//! identical edge → seamless mesh.
//!
//! # Coordinate convention
//!
//! Output positions are in **physics-world coordinates** (X east, Y up,
//! Z south). World-tile `(gx, gy)` maps to `position[0] = gx - half_w` and
//! `position[2] = gy - half_h`; `position[1]` is the height.

use crate::map::{WorldMap, CHUNK_TILES};

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
    /// Chunk coord containing tile `(ix, iy)`.
    pub fn from_tile(ix: i32, iy: i32) -> Self {
        Self {
            cx: ix.div_euclid(CHUNK_TILES as i32),
            cy: iy.div_euclid(CHUNK_TILES as i32),
        }
    }

    /// Physics-world center of the chunk (Y = 0 plane, Y axis is height).
    pub fn world_center_xz(self, map: &WorldMap) -> (f32, f32) {
        let half_w = (map.width  / 2) as f32;
        let half_h = (map.height / 2) as f32;
        let bx = self.cx as f32 * CHUNK_TILES as f32 + CHUNK_TILES as f32 * 0.5 - half_w;
        let bz = self.cy as f32 * CHUNK_TILES as f32 + CHUNK_TILES as f32 * 0.5 - half_h;
        (bx, bz)
    }
}

// ── Edge transitions ──────────────────────────────────────────────────────────

/// Which edges of this chunk border a neighbour at a coarser LOD.
///
/// When a flag is set, the builder replaces the outer triangle strip on that
/// edge with T-collapse stitching triangles that close the cracks where two
/// meshes meet at a 2:1 vertex ratio.
///
/// `north` = `-Z` edge (smaller iy), `south` = `+Z` edge (larger iy),
/// `west` = `-X` edge (smaller ix), `east` = `+X` edge (larger ix).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct EdgeTransitions {
    pub north: bool,
    pub south: bool,
    pub east:  bool,
    pub west:  bool,
}

// ── Output ────────────────────────────────────────────────────────────────────

/// Raw vertex/index data for a chunk. Consumer wraps in `Mesh` or `Collider`.
pub struct ChunkGeometry {
    pub positions: Vec<[f32; 3]>,
    pub indices:   Vec<u32>,
    /// Vertices per side. Useful for callers computing per-vertex data (e.g.
    /// the renderer building normals / colors / UVs in parallel).
    pub vps: usize,
}

// ── Height helper ─────────────────────────────────────────────────────────────

/// Height of the terrain vertex at tile-grid position `(gx, gy)`.
///
/// A vertex at `(gx, gy)` is the TL corner of tile `(gx, gy)`, stored as
/// `corner_offsets[0]` on the topmost surface layer of that column.
pub fn vertex_height(map: &WorldMap, gx: usize, gy: usize) -> f32 {
    let col = map.column(gx, gy);
    if let Some(layer) = col.layers.iter().rev().find(|l| l.is_surface_kind()) {
        layer.z_top + layer.corner_offsets[0]
    } else if let Some(layer) = col.layers.last() {
        layer.z_top + layer.corner_offsets[0]
    } else {
        0.0
    }
}

// ── Mesh builder ──────────────────────────────────────────────────────────────

/// Build a chunk's vertex+index buffers at the given LOD step.
///
/// `step` controls vertex density: `1` samples every tile (LOD 0 / "Full"),
/// `2` samples every other tile, etc. Output vertex count per side is
/// `CHUNK_TILES / step + 1`.
///
/// `edges` marks edges that face a neighbour at one coarser LOD; those edges
/// get T-collapse stitching. Pass `EdgeTransitions::default()` for a uniform
/// chunk (no stitching) — that's what physics colliders always want.
pub fn build_chunk_geometry(
    map: &WorldMap,
    coord: ChunkCoord,
    step: usize,
    edges: EdgeTransitions,
) -> ChunkGeometry {
    let step_i = step as i32;
    let vps = CHUNK_TILES / step + 1;
    let half_w = (map.width  / 2) as i32;
    let half_h = (map.height / 2) as i32;

    let n_verts = vps * vps;
    let mut positions = Vec::<[f32; 3]>::with_capacity(n_verts);
    let mut h_grid    = vec![0.0f32; n_verts];

    for vy in 0..vps {
        for vx in 0..vps {
            let gx = (coord.cx * CHUNK_TILES as i32 + vx as i32 * step_i)
                .clamp(0, map.width  as i32 - 1) as usize;
            let gy = (coord.cy * CHUNK_TILES as i32 + vy as i32 * step_i)
                .clamp(0, map.height as i32 - 1) as usize;

            let h = vertex_height(map, gx, gy);
            h_grid[vy * vps + vx] = h;
            let bx = gx as f32 - half_w as f32;
            let bz = gy as f32 - half_h as f32;
            positions.push([bx, h, bz]);
        }
    }

    // ── Best-diagonal CCW triangulation ──────────────────────────────────────
    let mut indices = Vec::<u32>::with_capacity((vps - 1) * (vps - 1) * 6);
    for vy in 0..(vps - 1) {
        for vx in 0..(vps - 1) {
            let i00 = (vy       * vps + vx    ) as u32;
            let i10 = (vy       * vps + vx + 1) as u32;
            let i01 = ((vy + 1) * vps + vx    ) as u32;
            let i11 = ((vy + 1) * vps + vx + 1) as u32;

            let h00 = h_grid[vy       * vps + vx    ];
            let h10 = h_grid[vy       * vps + vx + 1];
            let h01 = h_grid[(vy + 1) * vps + vx    ];
            let h11 = h_grid[(vy + 1) * vps + vx + 1];

            if (h00 - h11).abs() <= (h10 - h01).abs() {
                indices.extend_from_slice(&[i00, i01, i11, i00, i11, i10]);
            } else {
                indices.extend_from_slice(&[i00, i01, i10, i10, i01, i11]);
            }
        }
    }

    // ── Edge stitching ───────────────────────────────────────────────────────
    if edges.north { stitch_row(&mut indices, vps, 0,       false); }
    if edges.south { stitch_row(&mut indices, vps, vps - 1, true ); }
    if edges.west  { stitch_col(&mut indices, vps, 0,       false); }
    if edges.east  { stitch_col(&mut indices, vps, vps - 1, true ); }

    ChunkGeometry { positions, indices, vps }
}

// ── Edge stitching (T-collapse for LOD seams) ────────────────────────────────

fn stitch_row(indices: &mut Vec<u32>, vps: usize, vy_edge: usize, is_south: bool) {
    let vps_u = vps as u32;
    let vy_e = vy_edge as u32;

    let odd_edge: std::collections::HashSet<u32> = (1..vps_u)
        .step_by(2)
        .map(|vx| vy_e * vps_u + vx)
        .collect();
    filter_triangles(indices, &odd_edge);

    let vy_inner = if is_south { vy_e - 1 } else { vy_e + 1 };

    let mut k = 0u32;
    while k + 2 < vps_u {
        let e0 = vy_e     * vps_u + k;
        let e2 = vy_e     * vps_u + k + 2;
        let m0 = vy_inner * vps_u + k;
        let m1 = vy_inner * vps_u + k + 1;
        let m2 = vy_inner * vps_u + k + 2;
        if is_south {
            indices.extend_from_slice(&[e0, e2, m1,  e0, m1, m0,  e2, m2, m1]);
        } else {
            indices.extend_from_slice(&[e0, m1, e2,  e0, m0, m1,  e2, m1, m2]);
        }
        k += 2;
    }
}

fn stitch_col(indices: &mut Vec<u32>, vps: usize, vx_edge: usize, is_east: bool) {
    let vps_u = vps as u32;
    let vx_e = vx_edge as u32;

    let odd_edge: std::collections::HashSet<u32> = (1..vps_u)
        .step_by(2)
        .map(|vy| vy * vps_u + vx_e)
        .collect();
    filter_triangles(indices, &odd_edge);

    let vx_inner = if is_east { vx_e - 1 } else { vx_e + 1 };

    let mut k = 0u32;
    while k + 2 < vps_u {
        let e0 = k       * vps_u + vx_e;
        let e2 = (k + 2) * vps_u + vx_e;
        let m0 = k       * vps_u + vx_inner;
        let m1 = (k + 1) * vps_u + vx_inner;
        let m2 = (k + 2) * vps_u + vx_inner;
        if is_east {
            indices.extend_from_slice(&[e0, m1, e2,  e0, m0, m1,  e2, m1, m2]);
        } else {
            indices.extend_from_slice(&[e0, e2, m1,  e0, m1, m0,  e2, m2, m1]);
        }
        k += 2;
    }
}

fn filter_triangles(indices: &mut Vec<u32>, bad: &std::collections::HashSet<u32>) {
    let kept: Vec<u32> = indices
        .chunks_exact(3)
        .filter(|tri| !tri.iter().any(|i| bad.contains(i)))
        .flatten()
        .copied()
        .collect();
    *indices = kept;
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::generate_map;

    fn test_map() -> WorldMap {
        generate_map(42, 64, 64)
    }

    #[test]
    fn vertex_count_step_1() {
        let map = test_map();
        let geom = build_chunk_geometry(&map, ChunkCoord { cx: 0, cy: 0 }, 1, EdgeTransitions::default());
        assert_eq!(geom.positions.len(), 33 * 33);
        assert_eq!(geom.vps, 33);
    }

    #[test]
    fn vertex_count_step_2() {
        let map = test_map();
        let geom = build_chunk_geometry(&map, ChunkCoord { cx: 0, cy: 0 }, 2, EdgeTransitions::default());
        assert_eq!(geom.positions.len(), 17 * 17);
        assert_eq!(geom.vps, 17);
    }

    #[test]
    fn vertex_heights_match_corner_offsets() {
        let map = test_map();
        let geom = build_chunk_geometry(&map, ChunkCoord { cx: 0, cy: 0 }, 1, EdgeTransitions::default());
        for vy in 0..33usize {
            for vx in 0..33usize {
                let expected = vertex_height(&map, vx, vy);
                let actual = geom.positions[vy * 33 + vx][1];
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
        let a = build_chunk_geometry(&map, ChunkCoord { cx: 0, cy: 0 }, 1, EdgeTransitions::default());
        let b = build_chunk_geometry(&map, ChunkCoord { cx: 1, cy: 0 }, 1, EdgeTransitions::default());
        for vy in 0..33usize {
            let y_a = a.positions[vy * 33 + 32][1];
            let y_b = b.positions[vy * 33 +  0][1];
            assert!(
                (y_a - y_b).abs() < 1e-5,
                "seam at vy={vy}: A_right={y_a} B_left={y_b}"
            );
        }
    }

    #[test]
    fn deterministic_rebuild() {
        let map = test_map();
        let a = build_chunk_geometry(&map, ChunkCoord { cx: 1, cy: 1 }, 1, EdgeTransitions::default());
        let b = build_chunk_geometry(&map, ChunkCoord { cx: 1, cy: 1 }, 1, EdgeTransitions::default());
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.indices,   b.indices);
    }

    #[test]
    fn triangulation_is_in_groups_of_three() {
        let map = test_map();
        let geom = build_chunk_geometry(&map, ChunkCoord { cx: 0, cy: 0 }, 1, EdgeTransitions::default());
        assert_eq!(geom.indices.len() % 3, 0);
        for &i in &geom.indices {
            assert!((i as usize) < geom.positions.len(), "index out of range");
        }
    }
}
