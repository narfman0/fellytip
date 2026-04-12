//! Orbit camera — right-click/middle-click drag to orbit, scroll to zoom.
//!
//! Default angle: yaw=45°, pitch=35.3° (classic isometric).  The target starts
//! at the centre of the world map so the player sees terrain immediately.

use std::f32::consts::PI;
use bevy::{
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    prelude::*,
};
use crate::{LocalPlayer, PredictedPosition};

pub struct OrbitCameraPlugin;

impl Plugin for OrbitCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, update_orbit_camera);
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
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            // World-space origin (0, 3, 0) = centre of the map; y≈3 is typical surface elevation.
            target: Vec3::new(0.0, 3.0, 0.0),
            distance: 60.0,
            yaw: PI * 0.25,   // 45° diagonal — isometric
            pitch: 0.615,     // ~35.3° — classic isometric elevation
            min_pitch: 0.05,
            max_pitch: PI * 0.5 - 0.02,
            min_distance: 5.0,
            max_distance: 400.0,
            orbit_speed: 0.005,
            zoom_speed: 4.0,
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
) {
    let Ok((mut orbit, mut transform)) = query.single_mut() else {
        return;
    };

    // Lock camera target onto the local player's predicted position.
    // world (x, y, z) → Bevy (x, z, y); z is elevation.
    if let Some(pos) = player_q.iter().next() {
        orbit.target = Vec3::new(pos.x, pos.z, pos.y);
    }

    // Right-click or middle-click drag to orbit.
    if buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Middle) {
        orbit.yaw -= motion.delta.x * orbit.orbit_speed;
        orbit.pitch = (orbit.pitch + motion.delta.y * orbit.orbit_speed)
            .clamp(orbit.min_pitch, orbit.max_pitch);
    }

    // Scroll wheel to zoom.
    if scroll.delta.y != 0.0 {
        orbit.distance = (orbit.distance - scroll.delta.y * orbit.zoom_speed)
            .clamp(orbit.min_distance, orbit.max_distance);
    }

    *transform = camera_transform(&orbit);
}
