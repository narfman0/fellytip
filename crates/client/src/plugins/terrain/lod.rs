//! LOD levels and edge-transition flags for the chunk terrain system.

/// Side length of one chunk in tiles.
pub const CHUNK_TILES: usize = 32;

/// Four levels of detail, selected by distance from the camera to chunk centre.
///
/// | Level   | Step | Verts/side | Distance threshold  |
/// |---------|------|------------|---------------------|
/// | Full    | 1    | 33         | < 80 world units    |
/// | Half    | 2    | 17         | 80–192 world units  |
/// | Quarter | 4    | 9          | 192–320 world units |
/// | Eighth  | 8    | 5          | ≥ 320 world units   |
///
/// T-collapse stitching handles any 2:1 vertex ratio, so `Eighth` works without
/// changes to the stitching code.  The BFS LOD clamping (±1 constraint) ensures
/// no Full chunk is ever adjacent to a Quarter chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LodLevel {
    #[default]
    Full    = 0,
    Half    = 1,
    Quarter = 2,
    Eighth  = 3,
}

impl LodLevel {
    /// Tile-grid step between sampled vertices (1 = every tile, 2 = every other, …).
    pub fn step(self) -> usize {
        match self {
            LodLevel::Full    => 1,
            LodLevel::Half    => 2,
            LodLevel::Quarter => 4,
            LodLevel::Eighth  => 8,
        }
    }

    /// Number of vertices along one side of the chunk mesh at this LOD.
    pub fn verts_per_side(self) -> usize {
        CHUNK_TILES / self.step() + 1
    }

    /// Choose LOD based on world-unit distance from camera to chunk centre.
    pub fn from_distance(dist: f32) -> Self {
        if      dist < 80.0  { LodLevel::Full    }
        else if dist < 192.0 { LodLevel::Half    }
        else if dist < 320.0 { LodLevel::Quarter }
        else                 { LodLevel::Eighth  }
    }

    /// Next coarser level (saturates at Eighth).
    pub fn coarser(self) -> Self {
        match self {
            LodLevel::Full    => LodLevel::Half,
            LodLevel::Half    => LodLevel::Quarter,
            LodLevel::Quarter => LodLevel::Eighth,
            LodLevel::Eighth  => LodLevel::Eighth,
        }
    }
}

/// Which edges of this chunk border a neighbour at a coarser LOD.
///
/// When a flag is set, `build_chunk_mesh` replaces the outermost triangle strip
/// on that edge with T-collapse stitching triangles that eliminate cracks where
/// the two meshes meet at a 2:1 vertex ratio.
///
/// "North" = −Z (tile iy decreasing), "South" = +Z (iy increasing),
/// "West"  = −X (tile ix decreasing), "East"  = +X (ix increasing).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct EdgeTransitions {
    pub north: bool,
    pub south: bool,
    pub east:  bool,
    pub west:  bool,
}
