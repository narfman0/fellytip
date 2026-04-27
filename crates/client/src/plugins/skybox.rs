//! Procedural skybox — gradient cubemap driven by the day-night cycle.
//!
//! The cubemap is regenerated each frame from the current `TimeOfDay` so the
//! sky smoothly transitions through night → dawn → noon → dusk → night.
//!
//! Stars are baked into the nadir/side faces at night and faded out by day.

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::Skybox,
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
    },
};

use crate::plugins::scene_lighting::TimeOfDay;

// ── Sky colour constants ───────────────────────────────────────────────────────

const NIGHT_ZENITH:  [f32; 3] = [0.02, 0.02, 0.08];
const NIGHT_HORIZON: [f32; 3] = [0.04, 0.04, 0.14];

const DAWN_ZENITH:  [f32; 3] = [0.40, 0.25, 0.55];
const DAWN_HORIZON: [f32; 3] = [0.85, 0.45, 0.18];

const NOON_ZENITH:  [f32; 3] = [0.28, 0.55, 0.95];
const NOON_HORIZON: [f32; 3] = [0.65, 0.82, 1.00];

const DUSK_ZENITH:  [f32; 3] = [0.35, 0.20, 0.45];
const DUSK_HORIZON: [f32; 3] = [0.90, 0.40, 0.15];

const NADIR_DAY:   [f32; 3] = [0.35, 0.30, 0.22]; // earthy brown
const NADIR_NIGHT: [f32; 3] = [0.02, 0.02, 0.05]; // nearly black

// ── Plugin ─────────────────────────────────────────────────────────────────────

pub struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        // PostStartup guarantees the camera entity (spawned in Startup) exists.
        app.add_systems(PostStartup, attach_skybox)
            .add_systems(Update, update_skybox);
    }
}

// ── Components / Resources ─────────────────────────────────────────────────────

/// Holds the handle to the mutable sky cubemap image.
#[derive(Resource)]
struct SkyImageHandle(Handle<Image>);

// ── Startup system ─────────────────────────────────────────────────────────────

fn attach_skybox(
    mut commands: Commands,
    mut images:   ResMut<Assets<Image>>,
    camera_q:     Query<Entity, With<Camera3d>>,
    tod:          Res<TimeOfDay>,
) {
    let image = build_sky_cubemap(tod.0);
    let handle = images.add(image);
    commands.insert_resource(SkyImageHandle(handle.clone()));

    for entity in &camera_q {
        commands.entity(entity).insert(Skybox {
            image:      handle.clone(),
            brightness: 2_000.0,
            rotation:   Quat::IDENTITY,
        });
    }
}

// ── Update system ──────────────────────────────────────────────────────────────

/// Regenerate the sky cubemap each frame from the current `TimeOfDay`.
fn update_skybox(
    tod:        Res<TimeOfDay>,
    sky_handle: Option<Res<SkyImageHandle>>,
    mut images: ResMut<Assets<Image>>,
    mut clear:  ResMut<ClearColor>,
) {
    let Some(sky_handle) = sky_handle else { return };
    let Some(image) = images.get_mut(&sky_handle.0) else { return };

    let (zenith, horizon) = sky_colors(tod.0);

    // Refresh cubemap pixel data in-place.
    if let Some(ref mut data) = image.data {
        write_sky_cubemap(data, zenith, horizon, tod.0);
    }

    // Also update ClearColor so the background (seen through gaps / before sky
    // renders) roughly matches the horizon hue.
    clear.0 = Color::srgb(horizon[0], horizon[1], horizon[2]);
}

// ── Sky colour interpolation ───────────────────────────────────────────────────

/// Return (zenith, horizon) RGB as `[f32; 3]` for the given time-of-day fraction.
///
/// Time mapping:
/// - 0.00 / 1.00 = midnight
/// - 0.25 = dawn
/// - 0.50 = noon
/// - 0.75 = dusk
fn sky_colors(time: f32) -> ([f32; 3], [f32; 3]) {
    if time < 0.20 {
        // midnight → pre-dawn  (0.00 → 0.20)
        let t = time / 0.20;
        (lerp3(NIGHT_ZENITH, NIGHT_ZENITH, t), lerp3(NIGHT_HORIZON, NIGHT_HORIZON, t))
    } else if time < 0.30 {
        // pre-dawn → dawn     (0.20 → 0.30)
        let t = (time - 0.20) / 0.10;
        (lerp3(NIGHT_ZENITH, DAWN_ZENITH, t), lerp3(NIGHT_HORIZON, DAWN_HORIZON, t))
    } else if time < 0.50 {
        // dawn → noon         (0.30 → 0.50)
        let t = (time - 0.30) / 0.20;
        (lerp3(DAWN_ZENITH, NOON_ZENITH, t), lerp3(DAWN_HORIZON, NOON_HORIZON, t))
    } else if time < 0.70 {
        // noon → dusk         (0.50 → 0.70)
        let t = (time - 0.50) / 0.20;
        (lerp3(NOON_ZENITH, DUSK_ZENITH, t), lerp3(NOON_HORIZON, DUSK_HORIZON, t))
    } else if time < 0.80 {
        // dusk → night        (0.70 → 0.80)
        let t = (time - 0.70) / 0.10;
        (lerp3(DUSK_ZENITH, NIGHT_ZENITH, t), lerp3(DUSK_HORIZON, NIGHT_HORIZON, t))
    } else {
        // deep night          (0.80 → 1.00)
        (NIGHT_ZENITH, NIGHT_HORIZON)
    }
}

// ── Cubemap helpers ────────────────────────────────────────────────────────────

/// Resolution of each cubemap face.  Higher = smoother gradient.
const SIZE: u32 = 16;

/// Allocate a fresh sky cubemap for the given time-of-day.
fn build_sky_cubemap(time: f32) -> Image {
    let (zenith, horizon) = sky_colors(time);
    let bytes_per_face = (SIZE * SIZE * 4) as usize;
    let mut data = vec![0u8; bytes_per_face * 6];
    write_sky_cubemap(&mut data, zenith, horizon, time);

    let mut image = Image::new(
        Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 6 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image
}

/// Fill `data` (6 × SIZE × SIZE × 4 bytes) with gradient sky colours.
///
/// Cubemap face order (Vulkan / WebGPU): +X, −X, +Y, −Y, +Z, −Z.
/// Face 2 (+Y) = top/zenith; face 3 (−Y) = bottom/nadir.
/// Side faces (0,1,4,5) blend from zenith (row 0) to horizon (row SIZE−1).
fn write_sky_cubemap(data: &mut [u8], zenith: [f32; 3], horizon: [f32; 3], time: f32) {
    // Nadir transitions from day-earth to near-black at night.
    let night_amt = night_amount(time);
    let nadir = lerp3(NADIR_DAY, NADIR_NIGHT, night_amt);

    // Star brightness: visible in deep night, fade to 0 at dawn/dusk.
    let star_brightness = (night_amt - 0.4).max(0.0) / 0.6; // 0 during day, 1 at night

    let bytes_per_face = (SIZE * SIZE * 4) as usize;

    for face in 0..6usize {
        for row in 0..SIZE as usize {
            let t_row = row as f32 / (SIZE - 1) as f32; // 0 = top, 1 = bottom of face

            for col in 0..SIZE as usize {
                let offset = face * bytes_per_face + (row * SIZE as usize + col) * 4;
                let rgb: [f32; 3] = match face {
                    2 => zenith,                      // +Y: pure zenith
                    3 => nadir,                       // −Y: ground/nadir
                    _ => lerp3(zenith, horizon, t_row), // sides: zenith→horizon
                };

                // Add procedural stars to the +Y face and side faces when it's night.
                // Stars are placed at deterministic positions using a simple hash.
                let star = if star_brightness > 0.0 && face != 3 {
                    let star_hash = star_hash(face as u32, row as u32, col as u32);
                    let is_star = star_hash > 250u8; // sparse: ~2% of pixels
                    if is_star {
                        // Vary star brightness slightly.
                        let twinkle = (star_hash as f32 / 255.0) * star_brightness;
                        twinkle * 0.9
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };

                let r = (rgb[0] + star).clamp(0.0, 1.0);
                let g = (rgb[1] + star).clamp(0.0, 1.0);
                let b = (rgb[2] + star).clamp(0.0, 1.0);

                data[offset    ] = linear_to_srgb_u8(r);
                data[offset + 1] = linear_to_srgb_u8(g);
                data[offset + 2] = linear_to_srgb_u8(b);
                data[offset + 3] = 255;
            }
        }
    }
}

/// How "night" it is right now: 1.0 at midnight, 0.0 at noon.
fn night_amount(time: f32) -> f32 {
    // Sun elevation as a cosine: -1 at midnight, +1 at noon.
    let sun_elev = -(time * std::f32::consts::TAU).cos();
    // Map [−1, +1] → [1, 0] and clamp.
    ((1.0 - sun_elev) * 0.5).clamp(0.0, 1.0)
}

// ── Colour math ───────────────────────────────────────────────────────────────

#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Simple per-pixel hash for deterministic star placement.
#[inline]
fn star_hash(face: u32, row: u32, col: u32) -> u8 {
    let mut h = face.wrapping_mul(7919)
        .wrapping_add(row.wrapping_mul(6271))
        .wrapping_add(col.wrapping_mul(3779));
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    (h & 0xFF) as u8
}

/// Approximate linear-to-sRGB conversion (gamma ~2.2).
#[inline]
fn linear_to_srgb_u8(linear: f32) -> u8 {
    // Simple gamma 2.2 approximation; good enough for a skybox gradient.
    let srgb = linear.clamp(0.0, 1.0).powf(1.0 / 2.2);
    (srgb * 255.0) as u8
}
