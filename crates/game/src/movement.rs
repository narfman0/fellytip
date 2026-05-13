//! Shared kinematic step used by both the local player (windowed input path)
//! and server-side bots. Replaces direct `WorldPosition` mutation with a swept
//! capsule shape-cast against the avian static colliders authored by
//! `PhysicsWorldPlugin` (terrain chunks + zone interiors), plus a tile-grid
//! passability gate for things the trimesh doesn't represent (e.g. forest
//! tiles that have no per-tree collider).
//!
//! The function is intentionally parameter-heavy and stateless — callers own
//! the `WorldPosition` (or `PredictedPosition`) and a small `KinematicState`
//! that holds persistent `z_vel` / `grounded` fields across frames.

use avian3d::prelude::{Collider, ShapeCastConfig, SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;
use fellytip_shared::components::EntityBounds;
use fellytip_shared::world::map::{
    is_passable_with_bounds, is_water_at, smooth_surface_at, water_surface_at,
    WorldMap, GRAVITY, LAND_SNAP, MAX_FALL_SPEED, SWIM_BUOYANCY, SWIM_RISE_SPEED,
};

/// Persistent per-entity kinematic state held alongside `WorldPosition`.
/// Used by `step_kinematic` to integrate gravity and detect grounded state.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct KinematicState {
    pub z_vel: f32,
    pub grounded: bool,
}

impl Default for KinematicState {
    fn default() -> Self {
        // Start grounded so the first tick snaps to terrain via the tile
        // fallback rather than freefalling from spawn.
        Self { z_vel: 0.0, grounded: true }
    }
}

/// One physics step for a kinematic entity (no rigid body).
///
/// Caller owns the position; this function returns the new `(x, y, z)`. The
/// `z_vel` and `grounded` fields on `state` are updated in place.
///
/// Inputs:
/// - `(x, y, z)`: current position. `z` is up.
/// - `state`: persistent vertical-motion state.
/// - `desired_horiz`: desired horizontal velocity vector in **world** axes
///   (x → world east, y → world north), units per second.
/// - `bounds`: capsule dimensions for the shape-cast.
/// - `dt`: timestep in seconds.
/// - `spatial`: avian `SpatialQuery` (no hits when no colliders are loaded —
///   tile fallback handles that case).
/// - `map`: optional `WorldMap` for tile passability + heightmap fallback.
///   When `None`, the move is purely the desired horizontal velocity.
#[allow(clippy::too_many_arguments)]
pub fn step_kinematic(
    x: f32, y: f32, z: f32,
    state: &mut KinematicState,
    desired_horiz: Vec2,
    bounds: EntityBounds,
    dt: f32,
    spatial: &SpatialQuery,
    map: Option<&WorldMap>,
) -> (f32, f32, f32) {
    let new_x = x + desired_horiz.x * dt;
    let new_y = y + desired_horiz.y * dt;

    let Some(m) = map else {
        return (new_x, new_y, z);
    };

    // ── 1. Tile passability gate (cliffs, trees, etc. lacking colliders) ────
    let can_x = is_passable_with_bounds(m, new_x, y, z, bounds) || is_water_at(m, new_x, y);
    let can_y = is_passable_with_bounds(m, x, new_y, z, bounds) || is_water_at(m, x, new_y);
    let horiz_dx = if can_x { new_x - x } else { 0.0 };
    let horiz_dy = if can_y { new_y - y } else { 0.0 };

    // ── 2. Vertical velocity update ──────────────────────────────────────────
    let water_z = water_surface_at(m, x, y);
    if let Some(wz) = water_z {
        if z > wz + LAND_SNAP {
            state.z_vel = (state.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
        } else if z < wz - LAND_SNAP {
            state.z_vel = (state.z_vel + SWIM_BUOYANCY * dt).min(SWIM_RISE_SPEED);
        } else {
            state.z_vel = 0.0;
        }
    } else if state.grounded && state.z_vel <= 0.0 {
        state.z_vel = -0.5;
    } else {
        state.z_vel = (state.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
    }

    // ── 3. Swept capsule shape-cast with slide ───────────────────────────────
    let half_h  = bounds.height * 0.5;
    let cyl_len = (bounds.height - 2.0 * bounds.half_w).max(0.01);
    let capsule = Collider::capsule(bounds.half_w, cyl_len);
    let mut origin    = Vec3::new(x, z + half_h, y);
    let mut remaining = Vec3::new(horiz_dx, state.z_vel * dt, horiz_dy);
    let mut grounded_new = false;
    let mut hit_any   = false;
    const SKIN: f32 = 0.02;

    for _ in 0..3 {
        let len = remaining.length();
        if len < 1e-5 { break; }
        let Ok(dir) = Dir3::new(remaining / len) else { break };
        // Without `ignore_origin_penetration`, a grounded capsule whose bottom
        // hemisphere rests on the trimesh reports every horizontal sweep as an
        // immediate hit at distance=0, making the entity sticky to the floor.
        let cfg = ShapeCastConfig {
            max_distance: len,
            ignore_origin_penetration: true,
            ..ShapeCastConfig::DEFAULT
        };
        match spatial.cast_shape(
            &capsule, origin, Quat::IDENTITY, dir, &cfg, &SpatialQueryFilter::default(),
        ) {
            Some(hit) => {
                hit_any = true;
                let travel = (hit.distance - SKIN).max(0.0);
                origin += dir.as_vec3() * travel;
                let n: Vec3 = hit.normal1;
                if n.y > 0.7 {
                    grounded_new = true;
                    if state.z_vel < 0.0 { state.z_vel = 0.0; }
                } else if n.y < -0.7 && state.z_vel > 0.0 {
                    state.z_vel = 0.0;
                }
                let leftover = remaining - dir.as_vec3() * travel;
                remaining = leftover - n * leftover.dot(n);
            }
            None => {
                origin += remaining;
                break;
            }
        }
    }

    let nx = origin.x;
    let mut nz = origin.y - half_h;
    let ny = origin.z;
    state.grounded = grounded_new;

    // ── 4. Water surface snap ────────────────────────────────────────────────
    if let Some(wz) = water_z
        && (nz - wz).abs() < LAND_SNAP {
            nz = wz;
            state.z_vel = 0.0;
            state.grounded = true;
        }

    // ── 5. Tile fallback for chunk-streaming gap / out-of-collider regions ──
    if !hit_any && !state.grounded
        && let Some(tz) = smooth_surface_at(m, nx, ny, nz)
        && nz <= tz + LAND_SNAP {
            nz = tz;
            state.z_vel = 0.0;
            state.grounded = true;
        }

    (nx, ny, nz)
}
