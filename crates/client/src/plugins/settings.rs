//! In-game graphics settings: resource, persistence (RON), egui UI, and camera-component apply.

use bevy::core_pipeline::Skybox;
use bevy::pbr::{DistanceFog, FogFalloff, ScreenSpaceAmbientOcclusion};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::view::{ColorGrading, ColorGradingGlobal};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use serde::{Deserialize, Serialize};

// ── Feature-gated resources inserted by apply_graphics ───────────────────────

/// Global toggle for continuous particle emitters.
/// `tick_emitters` in particles.rs reads this and skips emission when false.
#[derive(Resource)]
pub struct ParticlesEnabled(pub bool);

/// Global toggle for the tree wind-sway animation.
#[derive(Resource)]
pub struct TreeSwayEnabled(pub bool);

/// Global toggle for the windmill spin animation.
#[derive(Resource)]
pub struct WindmillSpinEnabled(pub bool);

// ── Settings open/closed state ────────────────────────────────────────────────

/// Newtype resource that controls whether the settings window is visible.
#[derive(Resource, Default)]
pub struct SettingsMenuOpen(pub bool);

// ── GraphicsSettings resource ─────────────────────────────────────────────────

#[derive(Resource, Reflect, Serialize, Deserialize, Clone)]
#[reflect(Resource)]
pub struct GraphicsSettings {
    pub bloom: bool,
    /// Bloom intensity in [0.0, 1.0].
    pub bloom_intensity: f32,
    pub ssao: bool,
    pub fog: bool,
    /// Exponential-squared fog density in [0.001, 0.05].
    pub fog_density: f32,
    pub animated_water: bool,
    pub tree_sway: bool,
    pub windmill_spin: bool,
    pub particles: bool,
    pub skybox: bool,
    /// Post-process colour saturation in [0.5, 1.5].
    pub post_saturation: f32,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            bloom: true,
            bloom_intensity: 0.15,
            ssao: true,
            fog: true,
            fog_density: 0.008,
            animated_water: true,
            tree_sway: true,
            windmill_spin: true,
            particles: true,
            skybox: true,
            post_saturation: 1.1,
        }
    }
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GraphicsSettings>()
            .init_resource::<SettingsMenuOpen>()
            .insert_resource(ParticlesEnabled(true))
            .insert_resource(TreeSwayEnabled(true))
            .insert_resource(WindmillSpinEnabled(true))
            .register_type::<GraphicsSettings>()
            .add_systems(Startup, load_settings)
            .add_systems(
                Update,
                (save_on_change, apply_graphics).chain(),
            )
            .add_systems(EguiPrimaryContextPass, draw_settings_window);
    }
}

// ── Persistence helpers ───────────────────────────────────────────────────────

fn settings_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|mut p| {
        p.push("fellytip");
        p.push("settings.ron");
        p
    })
}

fn load_settings(mut settings: ResMut<GraphicsSettings>) {
    let Some(path) = settings_path() else { return };
    let Ok(content) = std::fs::read_to_string(&path) else { return };
    match ron::from_str::<GraphicsSettings>(&content) {
        Ok(loaded) => *settings = loaded,
        Err(e) => warn!("Failed to parse settings file {path:?}: {e}"),
    }
}

fn save_on_change(settings: Res<GraphicsSettings>) {
    if !settings.is_changed() {
        return;
    }
    let Some(path) = settings_path() else { return };
    // Ensure the directory exists.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match ron::to_string(settings.as_ref()) {
        Ok(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                warn!("Failed to save settings to {path:?}: {e}");
            }
        }
        Err(e) => warn!("Failed to serialize settings: {e}"),
    }
}

// ── apply_graphics ────────────────────────────────────────────────────────────

fn apply_graphics(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    camera_q: Query<Entity, With<Camera3d>>,
    mut particles_enabled: ResMut<ParticlesEnabled>,
    mut tree_sway_enabled: ResMut<TreeSwayEnabled>,
    mut windmill_spin_enabled: ResMut<WindmillSpinEnabled>,
) {
    if !settings.is_changed() {
        return;
    }

    // Propagate to sub-system resources.
    particles_enabled.0 = settings.particles;
    tree_sway_enabled.0 = settings.tree_sway;
    windmill_spin_enabled.0 = settings.windmill_spin;

    for entity in &camera_q {
        let mut ecmds = commands.entity(entity);

        // Bloom
        if settings.bloom {
            ecmds.insert(Bloom {
                intensity: settings.bloom_intensity,
                ..default()
            });
        } else {
            ecmds.remove::<Bloom>();
        }

        // SSAO
        if settings.ssao {
            ecmds.insert(ScreenSpaceAmbientOcclusion::default());
        } else {
            ecmds.remove::<ScreenSpaceAmbientOcclusion>();
        }

        // Distance fog
        if settings.fog {
            ecmds.insert(DistanceFog {
                color: Color::srgba(0.55, 0.65, 0.75, 1.0),
                falloff: FogFalloff::ExponentialSquared { density: settings.fog_density },
                directional_light_color: Color::srgba(1.0, 0.95, 0.85, 0.5),
                directional_light_exponent: 30.0,
            });
        } else {
            ecmds.remove::<DistanceFog>();
        }

        // Skybox — toggling requires removing/re-attaching the component added by SkyboxPlugin.
        // TODO: re-attach Skybox on enable by wiring into SkyboxPlugin's attach_skybox system.
        if !settings.skybox {
            ecmds.remove::<Skybox>();
        }

        // Post-saturation (ColorGrading is always applied so toggling skybox doesn't break other
        // post-process settings).
        ecmds.insert(ColorGrading {
            global: ColorGradingGlobal {
                post_saturation: settings.post_saturation,
                ..default()
            },
            ..default()
        });
    }
}

// ── draw_settings_window ──────────────────────────────────────────────────────

fn draw_settings_window(
    mut ctx: EguiContexts,
    mut settings: ResMut<GraphicsSettings>,
    mut open: ResMut<SettingsMenuOpen>,
) -> Result {
    let egui_ctx = ctx.ctx_mut()?;

    if !open.0 {
        return Ok(());
    }

    egui::Window::new("Settings")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .collapsible(false)
        .show(egui_ctx, |ui| {
            ui.set_min_width(320.0);

            egui::CollapsingHeader::new("Graphics")
                .default_open(true)
                .show(ui, |ui| {
                    // Bloom
                    ui.checkbox(&mut settings.bloom, "Bloom");
                    if settings.bloom {
                        ui.add(
                            egui::Slider::new(&mut settings.bloom_intensity, 0.0..=1.0)
                                .text("Bloom intensity"),
                        );
                    }

                    // SSAO
                    ui.checkbox(&mut settings.ssao, "Ambient occlusion (SSAO)");

                    // Fog
                    ui.checkbox(&mut settings.fog, "Distance fog");
                    if settings.fog {
                        ui.add(
                            egui::Slider::new(&mut settings.fog_density, 0.001..=0.05)
                                .text("Fog density"),
                        );
                    }

                    // Animated water
                    ui.checkbox(&mut settings.animated_water, "Animated water");

                    // Tree sway
                    ui.checkbox(&mut settings.tree_sway, "Tree wind sway");

                    // Windmill animation
                    ui.checkbox(&mut settings.windmill_spin, "Windmill animation");

                    // Particles
                    ui.checkbox(&mut settings.particles, "Particle effects");

                    // Skybox
                    ui.checkbox(&mut settings.skybox, "Skybox");

                    // Post saturation
                    ui.add(
                        egui::Slider::new(&mut settings.post_saturation, 0.5..=1.5)
                            .text("Post saturation"),
                    );
                });

            egui::CollapsingHeader::new("Controls")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label("Key rebinding coming soon.");
                });

            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                if ui.button("Close").clicked() {
                    open.0 = false;
                }
            });
        });

    Ok(())
}
