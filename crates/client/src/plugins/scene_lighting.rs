//! Scene lighting: a single directional sun + soft sky-blue ambient fill.

use std::f32::consts::PI;
use bevy::prelude::*;

pub struct SceneLightingPlugin;

impl Plugin for SceneLightingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_lights);
    }
}

fn spawn_lights(mut commands: Commands) {
    // Directional "sun" — bright, angled from upper-left.
    // Pitch: -45° (down). Yaw: +30° off the X axis.
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.97, 0.88), // slightly warm white
            illuminance: 50_000.0,
            shadows_enabled: false, // enable when shadow maps are needed
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(
            EulerRot::YXZ,
            PI * 0.167, // 30° yaw
            -PI * 0.25, // 45° down
            0.0,
        )),
    ));

    // Ambient sky fill — cool blue keeps unlit faces readable.
    // In Bevy 0.18, AmbientLight is a component spawned on a dedicated entity.
    commands.spawn(AmbientLight {
        color: Color::srgb(0.55, 0.65, 0.80),
        brightness: 300.0,
        ..default()
    });
}
