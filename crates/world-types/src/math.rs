// Coordinate math and tile utilities.

/// World units per tile along one axis.
pub const TILE_SIZE: f32 = 1.0;

/// Pixel width of one tile (for rendering).
pub const TILE_W: f32 = 32.0;

/// Pixel height of one tile (for rendering — half of width for isometric).
pub const TILE_H: f32 = 16.0;

/// Convert a floating-point world coordinate to its integer tile index (floor,
/// clamped to 0 for negative inputs).
#[inline]
pub fn tile_index(v: f32) -> usize {
    v.floor().max(0.0) as usize
}

/// Fractional position within a tile in [0.0, 1.0).
///
/// For `v = 2.7` returns `0.7`; for `v = -0.3` returns `0.7` (position 0.7
/// of the way through tile `[-1, 0)`).  Uses `v - floor(v)` so the result
/// always matches the tile selected by `column_at` (which also uses `floor`).
#[inline]
pub fn tile_frac(v: f32) -> f32 {
    v - v.floor()
}

/// Bilinear interpolation over a unit square.
///
/// `corners = [top_left, top_right, bottom_left, bottom_right]`
/// `fx` — fraction across the cell left→right in `[0, 1]`.
/// `fy` — fraction down the cell top→bottom in `[0, 1]`.
///
/// All four corners are exact when `fx`/`fy` are 0 or 1.
#[inline]
pub fn bilerp(corners: [f32; 4], fx: f32, fy: f32) -> f32 {
    let top    = corners[0] + (corners[1] - corners[0]) * fx;
    let bottom = corners[2] + (corners[3] - corners[2]) * fx;
    top + (bottom - top) * fy
}

/// Isometric projection: `WorldPosition (x, y, z)` → screen `(sx, sy)`.
///
/// `z` shifts the sprite upward in screen space so elevated tiles appear
/// higher than ground-level tiles.
#[inline]
pub fn iso_project(x: f32, y: f32, z: f32) -> (f32, f32) {
    let sx = (x - y) * (TILE_W / 2.0);
    let sy = (x + y) * (TILE_H / 4.0) + z * (TILE_H / 2.0);
    (sx, sy)
}

/// Top-down projection: `WorldPosition (x, y, _z)` → screen `(sx, sy)`.
///
/// Elevation is ignored in the top-down view.
#[inline]
pub fn topdown_project(x: f32, y: f32, _z: f32) -> (f32, f32) {
    (x * TILE_W, y * TILE_H)
}

// ── Procedural noise ──────────────────────────────────────────────────────────

/// Ken Perlin's quintic smooth-step: `6t⁵ − 15t⁴ + 10t³`.
///
/// Produces zero first- and second-order derivatives at `t=0` and `t=1`,
/// eliminating the "grid" artefacts present in the classic cubic smooth-step.
#[inline]
pub fn smooth_step(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Deterministic pseudo-random value for a 2-D integer lattice point.
///
/// Uses an FNV-inspired integer hash then maps the result to `[0.0, 1.0)`.
/// Same inputs always produce the same output; no global state.
#[inline]
pub fn lattice_hash(ix: i32, iy: i32) -> f32 {
    let mut h = ix
        .wrapping_mul(374_761_393i32)
        .wrapping_add(iy.wrapping_mul(668_265_263i32));
    h = h.wrapping_add(h << 13);
    h ^= h >> 7;
    h = h.wrapping_add(h << 3);
    h ^= h >> 17;
    h = h.wrapping_add(h << 5);
    (h as u32 as f32) / (u32::MAX as f32)
}

/// Smooth value noise at `(x, y)`.
///
/// Samples the four surrounding integer lattice corners with [`lattice_hash`]
/// and bilinearly interpolates using the quintic [`smooth_step`] curve.
/// Returns a value in `[0.0, 1.0)`.
pub fn value_noise(x: f32, y: f32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    // Fractional part via subtraction (handles negatives correctly unlike `.fract()`).
    let fx = smooth_step(x - x.floor());
    let fy = smooth_step(y - y.floor());

    let v00 = lattice_hash(ix,     iy);
    let v10 = lattice_hash(ix + 1, iy);
    let v01 = lattice_hash(ix,     iy + 1);
    let v11 = lattice_hash(ix + 1, iy + 1);

    bilerp([v00, v10, v01, v11], fx, fy)
}

/// Fractional Brownian motion (fBm) at `(x, y)`.
///
/// Sums `octaves` layers of [`value_noise`] with doubling frequency
/// (`lacunarity`) and halving amplitude (`persistence`).  The result is
/// normalised by the maximum possible amplitude sum so it stays in `[0, 1]`.
///
/// Typical terrain settings: `octaves=6`, `persistence=0.5`, `lacunarity=2.0`.
pub fn fbm(x: f32, y: f32, octaves: u32, persistence: f32, lacunarity: f32) -> f32 {
    let mut value = 0.0f32;
    let mut amplitude = 1.0f32;
    let mut frequency = 1.0f32;
    let mut max_value = 0.0f32;

    for _ in 0..octaves {
        value += value_noise(x * frequency, y * frequency) * amplitude;
        max_value += amplitude;
        amplitude *= persistence;
        frequency *= lacunarity;
    }

    if max_value > 0.0 { value / max_value } else { 0.0 }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilerp_corners_are_exact() {
        let c = [1.0f32, 2.0, 3.0, 4.0];
        assert!((bilerp(c, 0.0, 0.0) - 1.0).abs() < 1e-6, "TL");
        assert!((bilerp(c, 1.0, 0.0) - 2.0).abs() < 1e-6, "TR");
        assert!((bilerp(c, 0.0, 1.0) - 3.0).abs() < 1e-6, "BL");
        assert!((bilerp(c, 1.0, 1.0) - 4.0).abs() < 1e-6, "BR");
    }

    #[test]
    fn bilerp_center_is_average() {
        let c = [1.0f32, 2.0, 3.0, 4.0];
        let center = bilerp(c, 0.5, 0.5);
        let expected = (1.0 + 2.0 + 3.0 + 4.0) / 4.0;
        assert!((center - expected).abs() < 1e-6);
    }

    #[test]
    fn bilerp_flat_surface() {
        let c = [5.0f32; 4];
        assert!((bilerp(c, 0.3, 0.7) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn tile_frac_positive() {
        assert!((tile_frac(2.7) - 0.7).abs() < 1e-6);
        assert!((tile_frac(0.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn tile_frac_negative_matches_floor() {
        // -0.3 is 0.7 of the way through tile [-1, 0) — same convention as column_at.
        assert!((tile_frac(-0.3) - 0.7).abs() < 1e-6);
        assert!((tile_frac(-1.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn tile_index_clamps_negative() {
        assert_eq!(tile_index(-1.5), 0);
        assert_eq!(tile_index(3.9), 3);
    }

    #[test]
    fn iso_project_zero_z_matches_flat() {
        // At z=0 the iso projection should give the standard formula.
        let (sx, sy) = iso_project(2.0, 3.0, 0.0);
        assert!((sx - (2.0 - 3.0) * (TILE_W / 2.0)).abs() < 1e-6);
        assert!((sy - (2.0 + 3.0) * (TILE_H / 4.0)).abs() < 1e-6);
    }

    #[test]
    fn iso_project_z_shifts_upward() {
        let (_, sy0) = iso_project(0.0, 0.0, 0.0);
        let (_, sy1) = iso_project(0.0, 0.0, 1.0);
        assert!(sy1 > sy0, "higher z should produce a larger screen y (upward in Bevy)");
    }

    // ── Noise tests ───────────────────────────────────────────────────────────

    #[test]
    fn smooth_step_endpoints() {
        assert!((smooth_step(0.0) - 0.0).abs() < 1e-6);
        assert!((smooth_step(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn smooth_step_midpoint_is_half() {
        assert!((smooth_step(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn lattice_hash_is_deterministic() {
        assert!((lattice_hash(3, 7) - lattice_hash(3, 7)).abs() < 1e-10);
        assert!((lattice_hash(-1, 5) - lattice_hash(-1, 5)).abs() < 1e-10);
    }

    #[test]
    fn lattice_hash_in_unit_range() {
        for ix in -5..=5 {
            for iy in -5..=5 {
                let v = lattice_hash(ix, iy);
                assert!((0.0..=1.0).contains(&v), "lattice_hash({ix},{iy})={v} out of [0,1]");
            }
        }
    }

    #[test]
    fn value_noise_in_unit_range() {
        // Sample a variety of positions including negative coords.
        for i in -10..=10 {
            let x = i as f32 * 0.37;
            let y = i as f32 * 0.53;
            let v = value_noise(x, y);
            assert!((0.0..=1.0).contains(&v), "value_noise({x},{y})={v} out of [0,1]");
        }
    }

    #[test]
    fn fbm_in_unit_range() {
        for i in 0..20 {
            let x = i as f32 * 0.13;
            let y = i as f32 * 0.17;
            let v = fbm(x, y, 6, 0.5, 2.0);
            assert!((0.0..=1.0).contains(&v), "fbm({x},{y})={v} out of [0,1]");
        }
    }

    #[test]
    fn fbm_is_deterministic() {
        let a = fbm(1.5, 2.3, 6, 0.5, 2.0);
        let b = fbm(1.5, 2.3, 6, 0.5, 2.0);
        assert!((a - b).abs() < 1e-10);
    }

    #[test]
    fn fbm_varies_across_positions() {
        let a = fbm(0.0, 0.0, 6, 0.5, 2.0);
        let b = fbm(10.0, 10.0, 6, 0.5, 2.0);
        let c = fbm(200.0, 300.0, 6, 0.5, 2.0);
        // Not all the same value — the function actually varies.
        assert!(!(a == b && b == c), "fbm is constant — something is wrong");
    }
}
