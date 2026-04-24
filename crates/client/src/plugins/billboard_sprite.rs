//! Billboard sprite renderer — reads `assets/bestiary.toml`, loads each
//! entity's atlas PNG from `crates/client/assets/sprites/`, and slices it
//! into per-cell textures so individual frames can be selected at runtime.
//!
//! Scope (this PR — see issue #19):
//! - Loads bestiary + atlas textures at startup; gracefully skips entities
//!   whose PNG is not yet on disk.  `sprite_gen` is the generator (#17/#18).
//! - Spawns a billboard quad alongside (not replacing) the existing PBR
//!   mesh for faction NPCs, wildlife and the local player.  The additive
//!   behaviour is intentional — so this plugin can land without regressing
//!   the current client until every bestiary entry has real atlases.
//! - Uses `fellytip_shared::sprite_math::world_dir_to_sprite_row` to pick
//!   the sprite row from velocity and camera yaw.
//! - Cycles animation frames on a per-entity timer.
//!
//! Deferred (follow-up PRs):
//! - Suppressing the underlying PBR mesh when a sprite atlas is present.
//! - Distinct atlas per `EntityKind` / per faction (currently every
//!   animated entity maps to the only bestiary entry that exists).
//! - Custom `AssetLoader` for the RON manifest; today we derive the grid
//!   directly from the bestiary TOML.

use bevy::{
    asset::RenderAssetUsages,
    image::{Image, ImageSampler},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use fellytip_shared::{
    bestiary::{load_bestiary, AnimationDef, BestiaryEntry},
    components::{EntityKind, FactionBadge, WildlifeKind, WorldPosition},
    sprite_math::world_dir_to_sprite_row,
};
use smol_str::SmolStr;
use std::collections::HashMap;

use crate::{LocalPlayer, PredictedPosition};
use crate::plugins::camera::OrbitCamera;

/// Returns the bestiary atlas id for an entity based on its components, or
/// `None` for `Settlement` entities (which use PBR only) and cases where a
/// required component is absent (e.g. `Wildlife` with no `WildlifeKind`).
pub fn atlas_id_for_entity(
    kind: Option<&EntityKind>,
    badge: Option<&FactionBadge>,
    wildlife: Option<&WildlifeKind>,
) -> Option<SmolStr> {
    match kind {
        None => Some("hero".into()),
        Some(EntityKind::Settlement) => None,
        Some(EntityKind::FactionNpc) => {
            badge.map(|b| format!("{}_npc", b.faction_id).into())
        }
        Some(EntityKind::Wildlife) => match wildlife {
            Some(WildlifeKind::Bison) => Some("bison".into()),
            Some(WildlifeKind::Dog)   => Some("dog".into()),
            Some(WildlifeKind::Horse) => Some("horse".into()),
            None => None,
        },
    }
}

/// World-space edge length of the billboard quad.  Roughly player-sized.
const BILLBOARD_EDGE: f32 = 2.0;

pub struct BillboardSpritePlugin;

impl Plugin for BillboardSpritePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BillboardSprites>()
            .add_systems(Startup, load_atlases)
            .add_systems(
                Update,
                (
                    spawn_billboards,
                    face_camera,
                    update_direction,
                    advance_animation,
                    swap_cell_material,
                )
                    .chain(),
            );
    }
}

// ── Resources ─────────────────────────────────────────────────────────────────

/// Atlas registry — one entry per bestiary id whose PNG loaded successfully.
#[derive(Resource, Default)]
pub struct BillboardSprites {
    pub atlases: HashMap<SmolStr, AtlasAssets>,
    /// Shared quad mesh used by every billboard.
    pub quad: Option<Handle<Mesh>>,
}

pub struct AtlasAssets {
    pub entry: BestiaryEntry,
    /// Per-cell materials, indexed via [`Self::cell_index`].
    pub cell_materials: Vec<Handle<StandardMaterial>>,
    pub columns: u32,
    /// Total number of rows (`animations.len() * directions`).  Kept for
    /// validation and potential future use — it's redundant with
    /// `cell_materials.len() / columns`.
    #[allow(dead_code)]
    pub rows: u32,
}

impl AtlasAssets {
    /// Flat index for cell at `(row, col)`.
    pub fn cell_index(&self, row: u32, col: u32) -> usize {
        (row * self.columns + col) as usize
    }

    /// Total frames in animation `i` (used for wrap-around frame advance).
    pub fn animation(&self, i: usize) -> Option<&AnimationDef> {
        self.entry.animations.get(i)
    }

    pub fn row_start(&self, anim_index: usize) -> u32 {
        anim_index as u32 * self.entry.directions as u32
    }
}

// ── Components ────────────────────────────────────────────────────────────────

#[derive(Component, Debug)]
pub struct BillboardSprite {
    pub atlas_id: SmolStr,
    pub animation_index: usize,
    pub direction: u32,
    pub frame: u32,
    pub frame_timer: f32,
    /// Previous (x, y) in world space — used to derive velocity for
    /// direction quantisation.
    pub last_xy: (f32, f32),
}

// ── Startup ───────────────────────────────────────────────────────────────────

fn load_atlases(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut registry = BillboardSprites {
        quad: Some(meshes.add(billboard_quad())),
        ..Default::default()
    };

    let bestiary_path = bestiary_path();
    let entries = match load_bestiary(&bestiary_path) {
        Ok(e) => e,
        Err(e) => {
            warn!("billboard: no bestiary loaded ({e:#}); plugin will be a no-op");
            commands.insert_resource(registry);
            return;
        }
    };

    for entry in entries {
        match try_load_atlas(&entry, &mut images, &mut materials) {
            Ok(assets) => {
                info!(
                    "billboard: loaded atlas for `{}` ({} cells)",
                    entry.id,
                    assets.cell_materials.len(),
                );
                registry.atlases.insert(entry.id.clone(), assets);
            }
            Err(e) => {
                // Missing PNG is the common case right now — log at debug
                // so it's not noisy for every bestiary entry.
                debug!("billboard: skipping `{}` — {e:#}", entry.id);
            }
        }
    }

    commands.insert_resource(registry);
}

fn try_load_atlas(
    entry: &BestiaryEntry,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> anyhow::Result<AtlasAssets> {
    use anyhow::Context;
    let png_path = sprites_dir().join(format!("{}.png", entry.id));
    let bytes = std::fs::read(&png_path)
        .with_context(|| format!("reading {}", png_path.display()))?;
    let atlas_img = image::load_from_memory(&bytes)
        .with_context(|| format!("decoding {}", png_path.display()))?
        .to_rgba8();

    let tile_size = atlas_img.height() / (entry.animations.len() as u32 * entry.directions as u32);
    let columns = atlas_img.width() / tile_size;
    let rows = atlas_img.height() / tile_size;
    if tile_size == 0 || columns == 0 || rows == 0 {
        anyhow::bail!(
            "atlas {:?} has unusable dims {}×{}",
            png_path,
            atlas_img.width(),
            atlas_img.height()
        );
    }

    let mut cell_materials = Vec::with_capacity((rows * columns) as usize);
    for row in 0..rows {
        for col in 0..columns {
            let cell = image::imageops::crop_imm(
                &atlas_img,
                col * tile_size,
                row * tile_size,
                tile_size,
                tile_size,
            )
            .to_image();
            let img = rgba_to_bevy_image(&cell);
            let img_handle = images.add(img);
            let mat = materials.add(StandardMaterial {
                base_color_texture: Some(img_handle),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            });
            cell_materials.push(mat);
        }
    }

    Ok(AtlasAssets {
        entry: entry.clone(),
        cell_materials,
        columns,
        rows,
    })
}

fn rgba_to_bevy_image(img: &image::RgbaImage) -> Image {
    let (w, h) = img.dimensions();
    let mut out = Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        img.as_raw().clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    out.sampler = ImageSampler::nearest();
    out
}

fn billboard_quad() -> Mesh {
    let half = BILLBOARD_EDGE * 0.5;
    let positions = vec![
        [-half, 0.0, 0.0],
        [ half, 0.0, 0.0],
        [ half, BILLBOARD_EDGE, 0.0],
        [-half, BILLBOARD_EDGE, 0.0],
    ];
    let normals = vec![[0.0, 0.0, 1.0]; 4];
    let uvs = vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let indices = vec![0, 1, 2, 0, 2, 3];

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn bestiary_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/bestiary.toml")
}

fn sprites_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/sprites")
}

// ── Spawn ─────────────────────────────────────────────────────────────────────

/// Entities that just received a `WorldPosition` get a billboard sibling if
/// their bestiary id is loaded.
///
/// Resolution rules (mirrors `bestiary::REQUIRED_BESTIARY_IDS` docs):
/// - No `EntityKind` → `"hero"` (local player)
/// - `FactionNpc` + `FactionBadge` → `"{faction_id}_npc"`
/// - `Wildlife` + `WildlifeKind` → lowercase variant (`"bison"`, `"dog"`, `"horse"`)
/// - `Settlement` → no billboard
#[allow(clippy::type_complexity)]
fn spawn_billboards(
    mut commands: Commands,
    registry: Res<BillboardSprites>,
    new_entities: Query<
        (
            Entity,
            &WorldPosition,
            Option<&EntityKind>,
            Option<&FactionBadge>,
            Option<&WildlifeKind>,
        ),
        Added<WorldPosition>,
    >,
) {
    let Some(quad) = registry.quad.clone() else { return; };

    for (entity, pos, kind, badge, wildlife) in &new_entities {
        let Some(atlas_id) = atlas_id_for_entity(kind, badge, wildlife) else { continue };

        let Some(atlas) = registry.atlases.get(&atlas_id) else {
            debug!("billboard: no atlas for `{atlas_id}` — skipping {entity:?}");
            continue;
        };
        let Some(mat) = atlas.cell_materials.first().cloned() else { continue; };

        commands.entity(entity).with_children(|children| {
            children.spawn((
                Mesh3d(quad.clone()),
                MeshMaterial3d(mat),
                Transform::IDENTITY,
                BillboardSprite {
                    atlas_id,
                    animation_index: 0,
                    direction: 0,
                    frame: 0,
                    frame_timer: 0.0,
                    last_xy: (pos.x, pos.y),
                },
            ));
        });
    }
}

// ── Per-frame systems ─────────────────────────────────────────────────────────

fn face_camera(
    camera: Query<&OrbitCamera, Without<BillboardSprite>>,
    mut sprites: Query<&mut Transform, With<BillboardSprite>>,
) {
    let Ok(cam) = camera.single() else { return; };
    // Billboards use local space relative to their parent; rotate so the
    // quad always faces the camera's yaw on the XZ plane.
    let rotation = Quat::from_rotation_y(cam.yaw);
    for mut t in &mut sprites {
        t.rotation = rotation;
    }
}

fn update_direction(
    registry: Res<BillboardSprites>,
    camera: Query<&OrbitCamera>,
    mut sprites: Query<(&mut BillboardSprite, &ChildOf)>,
    world_pos: Query<&WorldPosition>,
    predicted: Query<(&PredictedPosition, &LocalPlayer)>,
) {
    let Ok(cam) = camera.single() else { return; };

    for (mut sprite, parent) in &mut sprites {
        let parent_entity = parent.parent();
        // Local player uses PredictedPosition; remote entities use WorldPosition.
        let (nx, ny) = if let Ok((p, _)) = predicted.get(parent_entity) {
            (p.x, p.y)
        } else if let Ok(p) = world_pos.get(parent_entity) {
            (p.x, p.y)
        } else {
            continue;
        };
        let (vx, vy) = (nx - sprite.last_xy.0, ny - sprite.last_xy.1);
        sprite.last_xy = (nx, ny);

        let Some(atlas) = registry.atlases.get(&sprite.atlas_id) else { continue; };
        let dirs = atlas.entry.directions as u32;
        sprite.direction = world_dir_to_sprite_row(vx, vy, cam.yaw, dirs);
    }
}

fn advance_animation(
    time: Res<Time>,
    registry: Res<BillboardSprites>,
    mut sprites: Query<&mut BillboardSprite>,
) {
    for mut sprite in &mut sprites {
        let Some(atlas) = registry.atlases.get(&sprite.atlas_id) else { continue; };
        let Some(anim) = atlas.animation(sprite.animation_index) else { continue; };
        let frame_duration = 1.0 / anim.fps.max(1) as f32;
        sprite.frame_timer += time.delta_secs();
        while sprite.frame_timer >= frame_duration {
            sprite.frame_timer -= frame_duration;
            sprite.frame = (sprite.frame + 1) % anim.frames.max(1) as u32;
        }
    }
}

fn swap_cell_material(
    registry: Res<BillboardSprites>,
    mut sprites: Query<(&BillboardSprite, &mut MeshMaterial3d<StandardMaterial>), Changed<BillboardSprite>>,
) {
    for (sprite, mut mat) in &mut sprites {
        let Some(atlas) = registry.atlases.get(&sprite.atlas_id) else { continue; };
        let row = atlas.row_start(sprite.animation_index) + sprite.direction;
        let idx = atlas.cell_index(row, sprite.frame);
        if let Some(handle) = atlas.cell_materials.get(idx) {
            mat.0 = handle.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fellytip_shared::bestiary::{AnimationDef, BestiaryEntry};

    fn mk_atlas(directions: u8, anim_frames: &[u16]) -> AtlasAssets {
        let entry = BestiaryEntry {
            id: "t".into(),
            display_name: "t".into(),
            directions,
            ai_prompt_base: "".into(),
            ai_style: "".into(),
            palette_seed: "".into(),
            animations: anim_frames
                .iter()
                .enumerate()
                .map(|(i, f)| AnimationDef {
                    name: format!("a{i}").into(),
                    frames: *f,
                    fps: 10,
                })
                .collect(),
        };
        let cols = *anim_frames.iter().max().unwrap_or(&1) as u32;
        let rows = anim_frames.len() as u32 * directions as u32;
        AtlasAssets {
            entry,
            cell_materials: vec![Handle::<StandardMaterial>::default(); (rows * cols) as usize],
            columns: cols,
            rows,
        }
    }

    #[test]
    fn cell_index_respects_columns() {
        let a = mk_atlas(8, &[4, 6, 5]);
        // row 0 col 0 → 0; row 0 col 3 → 3.
        assert_eq!(a.cell_index(0, 0), 0);
        assert_eq!(a.cell_index(0, 3), 3);
        // row 1 col 0 → columns (6).
        assert_eq!(a.cell_index(1, 0), 6);
        // row 7 col 5 → 7*6 + 5 = 47.
        assert_eq!(a.cell_index(7, 5), 47);
    }

    #[test]
    fn row_start_depends_on_animation_index() {
        let a = mk_atlas(8, &[4, 6, 5]);
        assert_eq!(a.row_start(0),  0);
        assert_eq!(a.row_start(1),  8);
        assert_eq!(a.row_start(2), 16);
    }

    #[test]
    fn animation_lookup_returns_defs_in_order() {
        let a = mk_atlas(4, &[2, 3]);
        assert_eq!(a.animation(0).unwrap().frames, 2);
        assert_eq!(a.animation(1).unwrap().frames, 3);
        assert!(a.animation(2).is_none());
    }
}
