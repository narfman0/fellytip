//! Tile renderer — spawns / despawns flat PBR cuboid meshes in a rolling
//! window around the camera target.
//!
//! # Coordinate mapping
//! World space (X=east, Y=north, Z=up) → Bevy space (X=east, Y=up, Z=south):
//! ```text
//! world (x, y, z_elevation) → bevy Vec3(x, z_elevation, y)
//! ```
//! A tile at grid cell `(ix, iy)` with surface height `z_top` occupies
//! Bevy positions `ix..ix+1` on X, `iy..iy+1` on Z, and has its top face
//! at Bevy Y = `z_top`.
//!
//! # Instancing
//! All tiles share one `Mesh` handle.  Tiles of the same biome share one
//! `StandardMaterial` handle, so Bevy's renderer can GPU-instance them in a
//! single draw call per biome.
//!
//! # Render window
//! A rolling ±[`RENDER_RADIUS`]-tile square around the camera's orbit target.
//! Tiles outside the window are despawned; tiles inside that are not yet
//! spawned are added.  The window only rebuilds when the camera crosses a
//! tile boundary (cheap integer-coordinate comparison each frame).

use std::collections::HashMap;
use bevy::prelude::*;
use fellytip_shared::{
    WORLD_SEED,
    world::map::{generate_map, TileKind, WorldMap, MAP_HEIGHT, MAP_WIDTH},
};

use crate::plugins::camera::OrbitCamera;

/// Half-width of the rolling render window in tiles.  41×41 = 1 681 max tiles.
const RENDER_RADIUS: i32 = 20;

pub struct TileRendererPlugin;

impl Plugin for TileRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_tile_assets)
            .add_systems(Update, update_tile_grid);
    }
}

// ── Resources ─────────────────────────────────────────────────────────────────

/// Shared GPU assets: one mesh + one material per biome kind.
#[derive(Resource)]
pub struct TileAssets {
    pub mesh: Handle<Mesh>,
    pub materials: HashMap<TileKind, Handle<StandardMaterial>>,
}

/// Tracks which tile grid cells have a spawned entity.
#[derive(Resource)]
struct TileGrid {
    /// `(ix, iy)` → spawned entity.
    spawned: HashMap<(i32, i32), Entity>,
    /// Centre the grid was last built around (forces rebuild when changed).
    last_center: IVec2,
}

impl Default for TileGrid {
    fn default() -> Self {
        Self {
            spawned: HashMap::new(),
            // Sentinel: ensures the first Update frame triggers a full build.
            last_center: IVec2::new(i32::MIN, i32::MIN),
        }
    }
}

// ── Startup ───────────────────────────────────────────────────────────────────

fn setup_tile_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Generate the world map deterministically — same as the server.
    // ~150 ms on first run; acceptable at startup.
    tracing::info!(seed = WORLD_SEED, "Client regenerating world map for rendering…");
    let map = generate_map(WORLD_SEED);
    tracing::info!("World map ready");

    // One shared flat cuboid: 1 world-unit wide, 0.2 tall, 1 deep.
    // The tile top face sits at the entity's Bevy Y origin + 0.1.
    let mesh = meshes.add(Cuboid::new(1.0, 0.2, 1.0));

    let mut mat_map = HashMap::new();
    for &kind in TileKind::ALL {
        mat_map.insert(kind, materials.add(material_for(kind)));
    }

    commands.insert_resource(TileAssets { mesh, materials: mat_map });
    commands.insert_resource(TileGrid::default());
    commands.insert_resource(map);
}

// ── Per-frame update ──────────────────────────────────────────────────────────

fn update_tile_grid(
    mut grid: ResMut<TileGrid>,
    map: Res<WorldMap>,
    assets: Res<TileAssets>,
    camera_q: Query<&OrbitCamera>,
    mut commands: Commands,
) {
    // Derive render centre from camera orbit target.
    // Bevy X → world X, Bevy Z → world Y.
    let target = camera_q.single().map(|o| o.target).unwrap_or_else(|_| {
        Vec3::new(MAP_WIDTH as f32 * 0.5, 3.0, MAP_HEIGHT as f32 * 0.5)
    });

    let cx = target.x as i32;
    let cy = target.z as i32; // Bevy Z corresponds to world Y

    let new_center = IVec2::new(cx, cy);
    if new_center == grid.last_center {
        return; // camera hasn't crossed a tile boundary
    }

    // ── Despawn tiles that have scrolled out of the window ────────────────────
    let to_remove: Vec<(i32, i32)> = grid
        .spawned
        .keys()
        .copied()
        .filter(|&(ix, iy)| {
            (ix - cx).abs() > RENDER_RADIUS || (iy - cy).abs() > RENDER_RADIUS
        })
        .collect();

    for key in to_remove {
        if let Some(entity) = grid.spawned.remove(&key) {
            commands.entity(entity).despawn();
        }
    }

    // ── Spawn tiles that have entered the window ──────────────────────────────
    for dy in -RENDER_RADIUS..=RENDER_RADIUS {
        for dx in -RENDER_RADIUS..=RENDER_RADIUS {
            let ix = cx + dx;
            let iy = cy + dy;

            // Bounds check.
            if ix < 0 || iy < 0 || ix >= MAP_WIDTH as i32 || iy >= MAP_HEIGHT as i32 {
                continue;
            }

            // Already spawned.
            if grid.spawned.contains_key(&(ix, iy)) {
                continue;
            }

            let col = map.column(ix as usize, iy as usize);

            // Pick the topmost surface layer; skip void / empty columns.
            let Some(layer) = col
                .layers
                .iter()
                .rev()
                .find(|l| l.is_surface_kind())
                .or_else(|| col.layers.last())
            else {
                continue;
            };

            if layer.kind == TileKind::Void {
                continue;
            }

            let Some(mat) = assets.materials.get(&layer.kind) else {
                continue;
            };

            // Position: top face at Bevy Y = z_top → centre at Y = z_top − 0.1.
            // Tile occupies ix..ix+1 on Bevy X, iy..iy+1 on Bevy Z.
            let bx = ix as f32 + 0.5;
            let by = layer.z_top - 0.1;
            let bz = iy as f32 + 0.5;

            let entity = commands
                .spawn((
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(mat.clone()),
                    Transform::from_translation(Vec3::new(bx, by, bz)),
                ))
                .id();

            grid.spawned.insert((ix, iy), entity);
        }
    }

    grid.last_center = new_center;
}

// ── Materials ─────────────────────────────────────────────────────────────────

fn material_for(kind: TileKind) -> StandardMaterial {
    match kind {
        // ── Surface biomes ────────────────────────────────────────────────────
        TileKind::Plains => StandardMaterial {
            base_color: Color::srgb(0.45, 0.65, 0.30),
            perceptual_roughness: 0.95,
            ..default()
        },
        TileKind::Grassland => StandardMaterial {
            base_color: Color::srgb(0.35, 0.72, 0.25),
            perceptual_roughness: 0.92,
            ..default()
        },
        TileKind::Forest => StandardMaterial {
            base_color: Color::srgb(0.12, 0.45, 0.12),
            perceptual_roughness: 0.95,
            ..default()
        },
        TileKind::TemperateForest => StandardMaterial {
            base_color: Color::srgb(0.18, 0.50, 0.18),
            perceptual_roughness: 0.95,
            ..default()
        },
        TileKind::TropicalForest => StandardMaterial {
            base_color: Color::srgb(0.08, 0.52, 0.20),
            perceptual_roughness: 0.90,
            ..default()
        },
        TileKind::TropicalRainforest => StandardMaterial {
            base_color: Color::srgb(0.04, 0.48, 0.15),
            perceptual_roughness: 0.88,
            ..default()
        },
        TileKind::Taiga => StandardMaterial {
            base_color: Color::srgb(0.22, 0.40, 0.22),
            perceptual_roughness: 0.93,
            ..default()
        },
        TileKind::Savanna => StandardMaterial {
            base_color: Color::srgb(0.76, 0.68, 0.30),
            perceptual_roughness: 0.90,
            ..default()
        },
        TileKind::Desert => StandardMaterial {
            base_color: Color::srgb(0.86, 0.76, 0.45),
            perceptual_roughness: 0.90,
            ..default()
        },
        TileKind::Tundra => StandardMaterial {
            base_color: Color::srgb(0.62, 0.68, 0.58),
            perceptual_roughness: 0.88,
            ..default()
        },
        TileKind::PolarDesert => StandardMaterial {
            base_color: Color::srgb(0.82, 0.87, 0.90),
            perceptual_roughness: 0.80,
            metallic: 0.02,
            ..default()
        },
        TileKind::Arctic => StandardMaterial {
            base_color: Color::srgb(0.92, 0.95, 0.98),
            perceptual_roughness: 0.72,
            metallic: 0.05,
            ..default()
        },
        TileKind::Mountain => StandardMaterial {
            base_color: Color::srgb(0.55, 0.50, 0.48),
            perceptual_roughness: 0.85,
            metallic: 0.05,
            ..default()
        },
        TileKind::Stone => StandardMaterial {
            base_color: Color::srgb(0.50, 0.48, 0.45),
            perceptual_roughness: 0.85,
            metallic: 0.02,
            ..default()
        },
        TileKind::Water => StandardMaterial {
            base_color: Color::srgba(0.15, 0.40, 0.75, 0.85),
            perceptual_roughness: 0.05,
            alpha_mode: AlphaMode::Blend,
            ..default()
        },
        TileKind::River => StandardMaterial {
            base_color: Color::srgba(0.22, 0.52, 0.88, 0.80),
            perceptual_roughness: 0.08,
            alpha_mode: AlphaMode::Blend,
            ..default()
        },
        // ── Underground ───────────────────────────────────────────────────────
        TileKind::Cavern => StandardMaterial {
            base_color: Color::srgb(0.25, 0.22, 0.20),
            perceptual_roughness: 0.95,
            ..default()
        },
        TileKind::DeepRock => StandardMaterial {
            base_color: Color::srgb(0.18, 0.16, 0.15),
            perceptual_roughness: 0.95,
            metallic: 0.02,
            ..default()
        },
        TileKind::LuminousGrotto => StandardMaterial {
            base_color: Color::srgb(0.10, 0.35, 0.32),
            perceptual_roughness: 0.75,
            emissive: LinearRgba::new(0.02, 0.15, 0.12, 0.0),
            ..default()
        },
        TileKind::Tunnel => StandardMaterial {
            base_color: Color::srgb(0.15, 0.15, 0.14),
            perceptual_roughness: 0.95,
            ..default()
        },
        // ── Meta ──────────────────────────────────────────────────────────────
        TileKind::Void => StandardMaterial {
            base_color: Color::srgb(0.02, 0.02, 0.02),
            perceptual_roughness: 1.0,
            ..default()
        },
    }
}
