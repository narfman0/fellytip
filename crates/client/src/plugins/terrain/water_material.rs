//! Animated water overlay material and mesh builder.
//!
//! Water and River tiles are rendered as a separate flat quad mesh over the
//! terrain, sharing a single `StandardMaterial` whose `base_color` is updated
//! each frame on a sine-wave so all water tiles pulse together.
//!
//! The water mesh is built at the same time as the terrain chunk mesh and
//! spawned as a child entity tagged with `WaterOverlay`.

use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    render::render_resource::PrimitiveTopology,
};
use fellytip_shared::world::map::{TileKind, WorldMap, CHUNK_TILES};

use super::chunk::vertex_height;

// ── Component ──────────────────────────────────────────────────────────────────

/// Marker on water overlay mesh entities so the animator system can find them.
#[derive(Component)]
pub struct WaterOverlay;

// ── Water mesh builder ─────────────────────────────────────────────────────────

/// Build a flat quad mesh covering all Water and River tiles in `coord`'s chunk.
///
/// Returns `None` if the chunk contains no water tiles (avoid spawning empty meshes).
/// The mesh sits at the nominal water height + a tiny offset (0.05 units) so it
/// renders slightly above the underlying terrain without z-fighting.
pub fn build_water_mesh(map: &WorldMap, cx: i32, cy: i32) -> Option<Mesh> {
    let half_w = (map.width  / 2) as i32;
    let half_h = (map.height / 2) as i32;

    let mut positions = Vec::<[f32; 3]>::new();
    let mut normals   = Vec::<[f32; 3]>::new();
    let mut colors    = Vec::<[f32; 4]>::new();
    let mut indices   = Vec::<u32>::new();

    for ty in 0..CHUNK_TILES {
        for tx in 0..CHUNK_TILES {
            let ix = (cx as usize * CHUNK_TILES + tx).min(map.width  - 1);
            let iy = (cy as usize * CHUNK_TILES + ty).min(map.height - 1);

            let col = map.column(ix, iy);
            let top_layer = col.layers.iter().rev().find(|l| l.kind != TileKind::Void);
            let Some(layer) = top_layer else { continue };

            if !matches!(layer.kind, TileKind::Water | TileKind::River | TileKind::CaveRiver) {
                continue;
            }

            // Place the water quad at terrain height + a small lift to avoid z-fighting.
            let y = vertex_height(map, ix, iy) + 0.05;

            let bx = ix as f32 - half_w as f32;
            let bz = iy as f32 - half_h as f32;

            // Base color: blue-green for Water, lighter for River.
            let color = match layer.kind {
                TileKind::River     => [0.22, 0.52, 0.88, 0.85],
                TileKind::CaveRiver => [0.05, 0.15, 0.60, 0.85],
                _                   => [0.15, 0.40, 0.75, 0.85],
            };

            let base = positions.len() as u32;
            positions.extend_from_slice(&[
                [bx,        y, bz       ],
                [bx + 1.0,  y, bz       ],
                [bx + 1.0,  y, bz + 1.0],
                [bx,        y, bz + 1.0],
            ]);
            normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
            colors.extend_from_slice(&[color; 4]);
            indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }

    if positions.is_empty() {
        return None;
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL,   normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR,    colors);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

// ── Shared water material ──────────────────────────────────────────────────────

/// Create the shared water `StandardMaterial`.
///
/// Semi-transparent blue with low roughness for a wet look.  `base_color` is
/// updated each frame by `animate_water` — the initial value here is irrelevant.
pub fn create_water_material(materials: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::srgba(0.15, 0.45, 0.80, 0.82),
        perceptual_roughness: 0.18,
        metallic: 0.05,
        reflectance: 0.6,
        alpha_mode: AlphaMode::Blend,
        double_sided: false,
        ..default()
    })
}

// ── Water animation system ─────────────────────────────────────────────────────

/// Animate the shared water material each frame using a sine wave so all water
/// tiles shimmer in sync.  Two color channels oscillate to mimic ripple depth.
pub fn animate_water(
    time: Res<Time>,
    water_mat_handle: Option<Res<WaterMaterialHandle>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(handle) = water_mat_handle else { return };
    let Some(mat) = materials.get_mut(&handle.0) else { return };

    let t = time.elapsed_secs();
    // Primary ripple: period ~2 s, amplitude ±0.08 on blue channel.
    let wave = (t * 3.14).sin() * 0.5 + 0.5; // 0..1
    // Secondary shimmer: slightly offset frequency, drives green.
    let shimmer = (t * 2.51 + 1.0).sin() * 0.5 + 0.5;

    let r = 0.12 + shimmer * 0.06;
    let g = 0.35 + shimmer * 0.12;
    let b = 0.70 + wave    * 0.15;
    let a = 0.78 + wave    * 0.07;

    mat.base_color = Color::srgba(r, g, b, a);
}

// ── Resource ───────────────────────────────────────────────────────────────────

/// Holds the shared handle for the animated water material.
#[derive(Resource)]
pub struct WaterMaterialHandle(pub Handle<StandardMaterial>);
