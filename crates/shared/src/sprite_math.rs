//! Pure sprite-direction math.  Lives in shared so both the client
//! renderer and any future dedicated-server prediction code can agree on
//! which atlas row a given velocity produces.

use std::f32::consts::{PI, TAU};

/// Map a world-space velocity to a sprite-atlas row index, assuming the
/// atlas lays rows out as `0 = facing camera` and rotating counter-clockwise
/// in screen space by `TAU / directions` per row.
///
/// Inputs are plain `f32`s (not glam `Vec2`) so the function can live in
/// `crates/shared` without pulling in Bevy/glam.
///
/// - `vx`, `vy`: world-space velocity on the ground plane.
/// - `camera_yaw`: current camera yaw in radians (0 = looking down +Z toward
///   the origin, matching the client's orbit camera).
/// - `directions`: number of atlas rows per animation (expected values are
///   4 or 8 — the bestiary validator guarantees this).
///
/// Returns an index in `[0, directions)`.  If `|velocity| < 1e-4`, returns
/// `0` (idle facing) rather than whatever atan2 does to the zero vector.
pub fn world_dir_to_sprite_row(vx: f32, vy: f32, camera_yaw: f32, directions: u32) -> u32 {
    debug_assert!(directions > 0);
    if vx.abs() + vy.abs() < 1e-4 {
        return 0;
    }
    // atan2 returns [-PI, PI]; shift so 0 = +Y in world (toward north),
    // then offset by camera yaw so the sprite tracks the view.
    let world_angle = vy.atan2(vx);
    let screen_angle = world_angle - camera_yaw;
    let normalized = (screen_angle + PI).rem_euclid(TAU) / TAU; // [0,1)
    let scaled = normalized * directions as f32;
    (scaled.round() as i64).rem_euclid(directions as i64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_for(vx: f32, vy: f32) -> u32 {
        world_dir_to_sprite_row(vx, vy, 0.0, 8)
    }

    #[test]
    fn zero_velocity_returns_zero() {
        assert_eq!(world_dir_to_sprite_row(0.0, 0.0, 0.0, 8), 0);
        assert_eq!(world_dir_to_sprite_row(0.0, 0.0, 1.234, 4), 0);
    }

    #[test]
    fn output_is_always_within_bounds() {
        for directions in [4u32, 8] {
            for i in 0..36 {
                let theta = i as f32 * (TAU / 36.0);
                let row = world_dir_to_sprite_row(theta.cos(), theta.sin(), 0.0, directions);
                assert!(row < directions, "row {row} out of bounds for {directions}");
            }
        }
    }

    /// Rotating the camera yaw by exactly TAU/N should shift every row by 1.
    #[test]
    fn full_rotation_step_shifts_by_one() {
        let dirs: u32 = 8;
        let step = TAU / dirs as f32;
        for i in 0..36 {
            let theta = i as f32 * (TAU / 36.0);
            let base = world_dir_to_sprite_row(theta.cos(), theta.sin(), 0.0, dirs);
            let shifted = world_dir_to_sprite_row(theta.cos(), theta.sin(), step, dirs);
            // Shifting the camera by +step rotates the world CCW relative to
            // screen → the sprite must face `base - 1` (mod dirs).
            let expected = (base + dirs - 1) % dirs;
            assert_eq!(shifted, expected, "theta={theta}");
        }
    }

    /// Opposite velocities produce rows that differ by exactly `directions / 2`.
    #[test]
    fn antipodal_velocities_are_half_apart() {
        let dirs: u32 = 8;
        for i in 0..36 {
            let theta = i as f32 * (TAU / 36.0);
            let a = world_dir_to_sprite_row(theta.cos(), theta.sin(), 0.0, dirs);
            let b = world_dir_to_sprite_row(-theta.cos(), -theta.sin(), 0.0, dirs);
            let diff = (a as i32 - b as i32).rem_euclid(dirs as i32);
            assert_eq!(diff as u32, dirs / 2, "theta={theta} a={a} b={b}");
        }
    }

    /// Smooth monotonic sweep — rotating the velocity CCW must either keep
    /// the row or increment it (no large jumps, no reverses).
    #[test]
    fn monotonic_ccw_sweep() {
        let dirs: u32 = 8;
        let mut prev = dir_for(1.0, 0.0);
        let mut wraps = 0;
        for i in 1..=dirs * 4 {
            let theta = i as f32 * (TAU / (dirs * 4) as f32);
            let cur = dir_for(theta.cos(), theta.sin());
            let step = (cur + dirs - prev) % dirs;
            assert!(step <= 1, "non-adjacent step prev={prev} cur={cur}");
            if cur < prev { wraps += 1; }
            prev = cur;
        }
        // One wrap-around over a full revolution.
        assert!((0..=2).contains(&wraps), "expected ~1 wrap, got {wraps}");
    }

    /// 4-direction atlases: cardinal directions should map to 4 distinct rows.
    #[test]
    fn four_direction_cardinal_distinctness() {
        let rows: Vec<u32> = [
            ( 1.0,  0.0), // east
            ( 0.0,  1.0), // north
            (-1.0,  0.0), // west
            ( 0.0, -1.0), // south
        ]
        .iter()
        .map(|(x, y)| world_dir_to_sprite_row(*x, *y, 0.0, 4))
        .collect();
        let uniq: std::collections::HashSet<_> = rows.iter().collect();
        assert_eq!(uniq.len(), 4, "cardinal dirs must hit all 4 rows, got {rows:?}");
    }
}
