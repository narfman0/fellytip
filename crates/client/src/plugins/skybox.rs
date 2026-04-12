//! Procedural skybox — a tiny 4×4 cubemap generated at startup.
//!
//! Face colours: deep blue zenith (+Y), earthy brown nadir (-Y),
//! and a top-to-horizon gradient on all four side faces.

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::Skybox,
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
    },
};

pub struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        // PostStartup guarantees the camera entity (spawned in Startup) exists.
        app.add_systems(PostStartup, attach_skybox);
    }
}

fn attach_skybox(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    camera_q: Query<Entity, With<Camera3d>>,
) {
    let handle = images.add(build_sky_cubemap());

    for entity in &camera_q {
        commands.entity(entity).insert(Skybox {
            image: handle.clone(),
            brightness: 2_000.0,
            rotation: Quat::IDENTITY,
        });
    }
}

/// Build a 4×4 per-face cubemap with a sky-blue palette.
///
/// Cubemap face order (Vulkan / WebGPU): +X, −X, +Y, −Y, +Z, −Z.
/// Face 2 (+Y) = top/zenith; face 3 (−Y) = bottom/nadir.
fn build_sky_cubemap() -> Image {
    const SIZE: u32 = 4;
    // sRGB RGBA colours
    let zenith:  [u8; 4] = [40,  90,  175, 255]; // deep sky blue
    let horizon: [u8; 4] = [130, 190, 240, 255]; // pale sky blue
    let nadir:   [u8; 4] = [85,  75,   55, 255]; // earthy brown

    let bytes_per_face = (SIZE * SIZE * 4) as usize;
    let mut data = vec![0u8; bytes_per_face * 6];

    for face in 0..6usize {
        for row in 0..SIZE as usize {
            // row 0 = top of face, row SIZE−1 = bottom; sides blend zenith→horizon.
            let t = row as f32 / (SIZE - 1) as f32;
            for col in 0..SIZE as usize {
                let offset = face * bytes_per_face + (row * SIZE as usize + col) * 4;
                let color: [u8; 4] = match face {
                    2 => zenith,
                    3 => nadir,
                    _ => lerp_rgba(zenith, horizon, t),
                };
                data[offset..offset + 4].copy_from_slice(&color);
            }
        }
    }

    let mut image = Image::new(
        Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 6 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image
}

#[inline]
fn lerp_rgba(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
        255,
    ]
}
