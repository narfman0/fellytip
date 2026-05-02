//! Standalone entity preview tool.
//!
//! Usage: preview <entity_id>
//!
//! Finds the entity's GLB file by walking up from cwd looking for
//! `assets/models/{id}/{id}.glb` (or `{id}_mesh.glb`), then displays it in an
//! isometric 3D viewport with a green ground plane, simple sky clear-color,
//! and scroll-to-zoom / right-click-drag-to-orbit.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use std::f32::consts::PI;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: preview <entity_id>");
        std::process::exit(1);
    }
    let entity_id = args[1].clone();

    // Find assets directory and GLB path.
    let Some(assets_dir) = find_assets_dir() else {
        eprintln!("Error: could not find 'assets/' directory by walking up from cwd");
        std::process::exit(1);
    };

    // Try primary path first, then _mesh fallback.
    let glb_rel = find_glb_rel(&assets_dir, &entity_id);
    let Some(glb_rel) = glb_rel else {
        eprintln!(
            "Error: could not find GLB for entity '{}' in {}",
            entity_id,
            assets_dir.display()
        );
        std::process::exit(1);
    };

    let title = format!("Preview \u{2014} {}", entity_id);

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: title.clone(),
                        resolution: (900_u32, 700_u32).into(),
                        ..default()
                    }),
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: assets_dir.to_string_lossy().to_string(),
                    ..default()
                }),
        )
        .insert_resource(ClearColor(Color::srgb(0.4, 0.6, 0.9)))
        .insert_resource(PreviewConfig {
            glb_rel,
            entity_id,
        })
        .add_systems(Startup, setup)
        .add_systems(Update, update_camera)
        .run();
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct PreviewConfig {
    glb_rel: String,
    entity_id: String,
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

#[derive(Component)]
struct OrbitCamera {
    target: Vec3,
    distance: f32,
    yaw: f32,
    pitch: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 5.0,
            yaw: PI * 0.25,
            pitch: 0.615_479_7, // isometric ≈35.3°
        }
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    config: Res<PreviewConfig>,
) {
    // Ambient light.
    commands.spawn(AmbientLight {
        color: Color::WHITE,
        brightness: 500.0,
        ..default()
    });

    // Spawn GLB scene.
    let scene_path = format!("{}#Scene0", config.glb_rel);
    commands.spawn(SceneRoot(asset_server.load(scene_path)));

    // Green ground plane.
    let green_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.5, 0.2),
        perceptual_roughness: 0.9,
        ..default()
    });
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(10.0)))),
        MeshMaterial3d(green_mat),
        Transform::default(),
    ));

    // Directional sunlight.
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -PI / 4.0, PI / 6.0, 0.0)),
    ));

    // Orbit camera.
    let orbit = OrbitCamera::default();
    let transform = camera_transform(&orbit);
    commands.spawn((Camera3d::default(), Msaa::Sample4, transform, orbit));
}

fn update_camera(
    mut query: Query<(&mut OrbitCamera, &mut Transform)>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
) {
    let Ok((mut cam, mut transform)) = query.single_mut() else {
        return;
    };

    // Scroll to zoom.
    if scroll.delta.y != 0.0 {
        cam.distance = (cam.distance - scroll.delta.y * 0.5).clamp(1.5, 20.0);
    }

    // Right-click drag to orbit.
    if buttons.pressed(MouseButton::Right) {
        cam.yaw += motion.delta.x * 0.005;
        cam.pitch = (cam.pitch + motion.delta.y * 0.005).clamp(0.1, 1.5);
    }

    *transform = camera_transform(&cam);
}

fn camera_transform(cam: &OrbitCamera) -> Transform {
    let eye = cam.target
        + Vec3::new(
            cam.distance * cam.pitch.cos() * cam.yaw.sin(),
            cam.distance * cam.pitch.sin(),
            cam.distance * cam.pitch.cos() * cam.yaw.cos(),
        );
    Transform::from_translation(eye).looking_at(cam.target, Vec3::Y)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Walk up from cwd until we find an `assets/` directory.
fn find_assets_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("assets");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Return the GLB path relative to `assets_dir`, or `None` if neither candidate
/// exists on disk.
fn find_glb_rel(assets_dir: &std::path::Path, id: &str) -> Option<String> {
    let primary = format!("models/{id}/{id}.glb");
    if assets_dir.join(&primary).exists() {
        return Some(primary);
    }
    let fallback = format!("models/{id}/{id}_mesh.glb");
    if assets_dir.join(&fallback).exists() {
        return Some(fallback);
    }
    None
}
