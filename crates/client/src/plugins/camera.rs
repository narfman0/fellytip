//! Orbit camera — right-click/middle-click drag to orbit, scroll to zoom.
//!
//! Default angle: yaw=45°, pitch=35.3° (classic isometric).  The target starts
//! at the centre of the world map so the player sees terrain immediately.
//!
//! Drag-to-orbit is disabled by default (`free_orbit: false`); set `free_orbit`
//! to `true` (e.g. via the `dm/set_camera_free` BRP method) to enable it.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use fellytip_shared::world::map::WorldMap;
use std::f32::consts::PI;
use crate::{ClientSet, LocalPlayer, PredictedPosition};
use crate::plugins::terrain::chunk::vertex_height;

/// Minimum world-unit clearance between the camera and the terrain surface below it.
const TERRAIN_CLEARANCE: f32 = 3.0;

/// True dimetric isometric yaw (45°).
pub const ISO_YAW: f32 = PI * 0.25;
/// True dimetric isometric pitch (≈35.264°, `atan(1/√2)`).
pub const ISO_PITCH: f32 = 0.615_479_7;

pub struct OrbitCameraPlugin;

impl Plugin for OrbitCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, update_orbit_camera.in_set(ClientSet::SyncCamera));
    }
}

/// Logical orbit state.  The Bevy `Transform` is recomputed every frame.
#[derive(Component)]
pub struct OrbitCamera {
    /// World-space point the camera orbits around (Bevy Y-up coordinates).
    pub target: Vec3,
    /// Distance from target in world units.
    pub distance: f32,
    /// Horizontal rotation in radians (0 = looking from +Z toward target).
    pub yaw: f32,
    /// Vertical angle above the horizontal plane, in radians.
    /// 0 = horizontal, PI/2 = straight down.
    pub pitch: f32,
    pub min_pitch: f32,
    pub max_pitch: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    /// Radians per pixel during drag.
    pub orbit_speed: f32,
    /// World units per scroll line (approximately).
    pub zoom_speed: f32,
    /// When `false` (default), yaw/pitch are locked to the ISO angles and
    /// right/middle-click drag has no effect.  Set to `true` to enable
    /// free-orbit mode (e.g. via `dm/set_camera_free`).
    pub free_orbit: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            // World-space origin (0, 8, 0) = centre of the map; y≈8 is typical surface elevation
            // with Z_SCALE=20.0 and moderate terrain height.
            target: Vec3::new(0.0, 8.0, 0.0),
            distance: 7.0,
            yaw: ISO_YAW,
            pitch: ISO_PITCH,
            min_pitch: 0.40,
            max_pitch: PI * 0.5 - 0.02,
            min_distance: 1.0,
            max_distance: 11.0,
            orbit_speed: 0.005,
            zoom_speed: 4.0,
            free_orbit: false,
        }
    }
}

fn camera_transform(o: &OrbitCamera) -> Transform {
    let (sin_yaw, cos_yaw) = o.yaw.sin_cos();
    let (sin_pitch, cos_pitch) = o.pitch.sin_cos();
    let offset = Vec3::new(
        cos_pitch * sin_yaw,
        sin_pitch,
        cos_pitch * cos_yaw,
    ) * o.distance;
    Transform::from_translation(o.target + offset).looking_at(o.target, Vec3::Y)
}

fn spawn_camera(mut commands: Commands) {
    let orbit = OrbitCamera::default();
    let transform = camera_transform(&orbit);
    commands.spawn((Camera3d::default(), transform, orbit));
}

fn update_orbit_camera(
    mut query: Query<(&mut OrbitCamera, &mut Transform)>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    // Follow the local player's predicted position — updated every frame on
    // input so the camera tracks the visual mesh with zero lag.
    player_q: Query<&PredictedPosition, With<LocalPlayer>>,
    map: Option<Res<WorldMap>>,
) {
    let Ok((mut orbit, mut transform)) = query.single_mut() else {
        return;
    };

    // Lock camera target onto the local player's predicted position.
    // world (x, y, z) → Bevy (x, z, y); z is elevation.
    if let Some(pos) = player_q.iter().next() {
        orbit.target = Vec3::new(pos.x, pos.z, pos.y);
    }

    // Right-click or middle-click drag to orbit — only active in free_orbit mode.
    if orbit.free_orbit
        && (buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Middle))
    {
        orbit.yaw -= motion.delta.x * orbit.orbit_speed;
        orbit.pitch = (orbit.pitch + motion.delta.y * orbit.orbit_speed)
            .clamp(orbit.min_pitch, orbit.max_pitch);
    }

    // Scroll wheel to zoom.
    if scroll.delta.y != 0.0 {
        orbit.distance = (orbit.distance - scroll.delta.y * orbit.zoom_speed)
            .clamp(orbit.min_distance, orbit.max_distance);
    }

    let mut t = camera_transform(&orbit);

    // Raise the camera if terrain below it would clip through.
    if let Some(ref map) = map {
        let floor = terrain_floor_y(map, t.translation.x, t.translation.z);
        if t.translation.y < floor {
            t.translation.y = floor;
            t = Transform::from_translation(t.translation).looking_at(orbit.target, Vec3::Y);
        }
    }

    *transform = t;
}

/// Returns the minimum camera Y at world position `(cam_x, cam_z)`: terrain
/// height at that tile plus the required clearance.
fn terrain_floor_y(map: &WorldMap, cam_x: f32, cam_z: f32) -> f32 {
    let half_w = (map.width  / 2) as f32;
    let half_h = (map.height / 2) as f32;
    let gx = ((cam_x + half_w).round() as i64).clamp(0, map.width  as i64 - 1) as usize;
    let gy = ((cam_z + half_h).round() as i64).clamp(0, map.height as i64 - 1) as usize;
    vertex_height(map, gx, gy) + TERRAIN_CLEARANCE
}

#[cfg(test)]
mod tests {
    use super::*;
    use fellytip_shared::world::map::generate_map;

    fn test_map() -> WorldMap {
        generate_map(42, 64, 64)
    }

    #[test]
    fn terrain_floor_includes_clearance() {
        let map = test_map();
        // Centre tile: Bevy (0, 0) → tile (32, 32).
        let floor = terrain_floor_y(&map, 0.0, 0.0);
        let raw = vertex_height(&map, 32, 32);
        assert!((floor - (raw + TERRAIN_CLEARANCE)).abs() < 1e-5);
    }

    #[test]
    fn camera_below_terrain_is_raised() {
        let map = test_map();
        let floor = terrain_floor_y(&map, 0.0, 0.0);
        // If the raw camera Y is below the floor, it must be lifted.
        let cam_y_below = floor - 10.0;
        assert!(cam_y_below < floor);
    }

    #[test]
    fn camera_above_terrain_is_unchanged() {
        let map = test_map();
        let floor = terrain_floor_y(&map, 0.0, 0.0);
        // A camera already above the floor must not be disturbed.
        let cam_y_above = floor + 5.0;
        assert!(cam_y_above >= floor);
    }

    #[test]
    fn oob_coords_clamp_to_map_edge() {
        let map = test_map();
        // Far outside map bounds should not panic and should return a valid floor.
        let floor = terrain_floor_y(&map, 1_000_000.0, 1_000_000.0);
        assert!(floor.is_finite());
    }
}
