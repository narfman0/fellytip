//! Water material extension — `ExtendedMaterial<StandardMaterial, WaterExtension>`.
//!
//! A single shared `WaterMaterial` handle is used for all water entities.
//! Each frame the `update_water_time` system writes `Time::elapsed_secs()` into
//! the extension's uniform, driving shader animation across all water chunks
//! simultaneously via one material-asset mutation.

use bevy::{
    pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin},
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

// ── Material extension ────────────────────────────────────────────────────────

/// Uniform data supplied to the water fragment shader.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct WaterExtension {
    /// Elapsed seconds, updated each frame by `update_water_time`.
    #[uniform(100)]
    pub time: f32,
}

impl MaterialExtension for WaterExtension {
    fn fragment_shader() -> ShaderRef {
        // Resolved at runtime from `assets/shaders/water.wgsl`.
        "shaders/water.wgsl".into()
    }
}

/// Alias for the full extended material type used by water entities.
pub type WaterMaterial = ExtendedMaterial<StandardMaterial, WaterExtension>;

// ── Assets resource ───────────────────────────────────────────────────────────

/// Shared GPU handle inserted at startup.
#[derive(Resource)]
pub struct WaterAssets {
    pub material: Handle<WaterMaterial>,
}

// ── Plugin ────────────────────────────────────────────────────────────────────

/// Registers `WaterMaterial` with the Bevy render pipeline.
pub struct WaterMaterialPlugin;

impl Plugin for WaterMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WaterMaterial>::default());
    }
}

// ── Startup system ────────────────────────────────────────────────────────────

/// Creates and inserts the shared `WaterAssets` resource.
///
/// Called from `Startup` after `WaterMaterialPlugin` has registered the
/// material type.  All water chunk entities share this single handle — one
/// `time` mutation per frame updates every water surface simultaneously.
pub fn setup_water_assets(
    mut commands:  Commands,
    mut materials: ResMut<Assets<WaterMaterial>>,
) {
    let material = materials.add(WaterMaterial {
        base: StandardMaterial {
            // WHITE base_color so vertex colour passes through unmodified;
            // the shader overrides it with animated water colour anyway.
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.05,
            metallic: 0.0,
            reflectance: 0.6,
            double_sided: false,
            cull_mode: None,
            ..default()
        },
        extension: WaterExtension::default(),
    });
    commands.insert_resource(WaterAssets { material });
}
