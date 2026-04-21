//! Scene lighting: directional sun + sky-blue ambient fill with a day-night cycle.

use std::f32::consts::{PI, TAU};
use bevy::prelude::*;

pub struct SceneLightingPlugin;

/// Seconds for one full day-night cycle.
const DAY_DURATION: f32 = 300.0;

/// Current time of day as a fraction [0.0, 1.0).
/// 0.0/1.0 = midnight, 0.25 = dawn, 0.5 = noon, 0.75 = dusk.
#[derive(Resource)]
pub struct TimeOfDay(pub f32);

impl Default for TimeOfDay {
    fn default() -> Self {
        // Start at mid-morning so the world is already lit on first load.
        Self(0.35)
    }
}

/// Marker on the directional sun entity.
#[derive(Component)]
struct SunLight;

impl Plugin for SceneLightingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TimeOfDay>()
            .add_systems(Startup, spawn_lights)
            .add_systems(Update, update_day_night);
    }
}

fn spawn_lights(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.97, 0.88),
            illuminance: 32_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(
            EulerRot::YXZ,
            PI * 0.167,
            -PI * 0.25,
            0.0,
        )),
        SunLight,
    ));

    commands.spawn(AmbientLight {
        color: Color::srgb(0.55, 0.65, 0.80),
        brightness: 80.0,
        ..default()
    });
}

fn update_day_night(
    time: Res<Time>,
    mut tod: ResMut<TimeOfDay>,
    mut sun_q: Query<(&mut DirectionalLight, &mut Transform), With<SunLight>>,
    mut amb_q: Query<&mut AmbientLight>,
) {
    tod.0 = (tod.0 + time.delta_secs() / DAY_DURATION).fract();
    let t = tod.0;

    // Sun elevation: -1 at midnight, 0 at dawn/dusk, +1 at noon.
    let sun_elevation = -(t * TAU).cos();
    let elev = sun_elevation.clamp(0.0, 1.0);

    // Pitch: 0 at horizon, -PI/2 when directly overhead.
    // Yaw sweeps east (dawn) to west (dusk) over the day.
    let pitch = -elev * PI * 0.48;
    let yaw = (t - 0.5) * PI;

    let illuminance = if sun_elevation > 0.0 {
        // Soft ramp-up near horizon, peak at noon.
        let ramp = if elev < 0.15 { elev / 0.15 } else { 1.0 };
        ramp * 80_000.0 * elev
    } else {
        0.0
    };

    // Sun colour: orange near horizon, warm white at zenith.
    let sun_color = if sun_elevation > 0.0 {
        let horizon_t = (elev * 6.0).clamp(0.0, 1.0); // quick transition off horizon
        Color::srgb(
            1.0,
            0.45 + horizon_t * 0.52,
            0.10 + horizon_t * 0.78,
        )
    } else {
        Color::BLACK
    };

    if let Ok((mut sun, mut sun_tf)) = sun_q.single_mut() {
        sun.illuminance = illuminance;
        sun.color = sun_color;
        sun_tf.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    }

    if let Ok(mut amb) = amb_q.single_mut() {
        // Transition from deep-night blue to sky blue as sun rises.
        let day_frac = ((sun_elevation + 0.2) / 1.2).clamp(0.0, 1.0);
        amb.color = Color::srgb(
            0.08 + day_frac * 0.47,
            0.10 + day_frac * 0.55,
            0.22 + day_frac * 0.58,
        );
        amb.brightness = 12.0 + day_frac * 68.0;
    }
}
