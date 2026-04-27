//! Scene lighting: directional sun + sky-blue ambient fill with a day-night cycle.
//! Underground (WORLD_SUNKEN_REALM) skips the day-night cycle and uses a static
//! dim blue ambient light instead.

use std::f32::consts::{PI, TAU};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use fellytip_shared::world::zone::{ZoneMembership, ZoneRegistry, WORLD_SUNKEN_REALM};
use crate::LocalPlayer;

pub struct SceneLightingPlugin;

/// Seconds for one full day-night cycle.
const DAY_DURATION: f32 = 300.0;

/// Underground ambient light color: dim blue, bright enough to see clearly.
const UNDERGROUND_AMBIENT_COLOR: Color = Color::srgb(0.15, 0.2, 0.35);
const UNDERGROUND_AMBIENT_BRIGHTNESS: f32 = 800.0;

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
            .add_systems(Update, (update_day_night, update_fog));
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
    player_q: Query<Option<&ZoneMembership>, With<LocalPlayer>>,
    zone_registry: Option<Res<ZoneRegistry>>,
) {
    // Determine if the player is currently in the Sunken Realm.
    let is_underground = if let Ok(zone_membership) = player_q.single() {
        if let (Some(registry), Some(membership)) = (&zone_registry, zone_membership) {
            registry.get(membership.0)
                .map(|z| z.world_id == WORLD_SUNKEN_REALM)
                .unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    };

    if is_underground {
        // Underground: disable the sun and apply static dim blue ambient.
        if let Ok((mut sun, _)) = sun_q.single_mut() {
            sun.illuminance = 0.0;
            sun.color = Color::BLACK;
        }
        if let Ok(mut amb) = amb_q.single_mut() {
            amb.color = UNDERGROUND_AMBIENT_COLOR;
            amb.brightness = UNDERGROUND_AMBIENT_BRIGHTNESS;
        }
        // Do not advance the time of day while underground.
        return;
    }

    // Surface: normal day-night cycle.
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
        // Soft ramp-up near horizon, peak at noon (~100,000 lux).
        // Dawn/dusk peak (elev≈0): ramp * 100_000 * ~0 ≈ a few thousand lux.
        // Full midday (elev=1): 100_000 lux.
        let ramp = if elev < 0.15 { elev / 0.15 } else { 1.0 };
        ramp * 100_000.0 * elev
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
        // Transition from deep moonlit-night to bright sky blue as sun rises.
        // Midnight: dark blue at brightness 50 (~10 lux equivalent).
        // Midday: sky blue at brightness ~800 (~100,000 lux, matched to sun illuminance).
        let day_frac = ((sun_elevation + 0.15) / 1.15).clamp(0.0, 1.0);
        amb.color = Color::srgb(
            0.05 + day_frac * 0.50,   // R: near-black night → warm day
            0.05 + day_frac * 0.60,   // G: near-black night → sky-blue
            0.15 + day_frac * 0.65,   // B: deep-blue moonlight → bright sky
        );
        // Night floor ~50 (moonlight), peak ~800 at solar zenith.
        amb.brightness = 50.0 + day_frac * 750.0;
    }
}

fn update_fog(
    mut fog_q: Query<&mut DistanceFog>,
    player_q: Query<Option<&ZoneMembership>, With<LocalPlayer>>,
    zone_registry: Option<Res<ZoneRegistry>>,
) {
    let is_underground = if let Ok(zone_membership) = player_q.single() {
        if let (Some(registry), Some(membership)) = (&zone_registry, zone_membership) {
            registry.get(membership.0)
                .map(|z| z.world_id == WORLD_SUNKEN_REALM)
                .unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    };

    for mut fog in fog_q.iter_mut() {
        if is_underground {
            fog.color = Color::srgba(0.02, 0.02, 0.05, 1.0);
            fog.falloff = FogFalloff::ExponentialSquared { density: 0.02 };
        } else {
            fog.color = Color::srgba(0.55, 0.65, 0.75, 1.0);
            fog.falloff = FogFalloff::ExponentialSquared { density: 0.008 };
        }
    }
}
