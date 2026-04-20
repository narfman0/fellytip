//! 2.5D billboard sprite renderer.
//!
//! For each game entity that has a generated sprite sheet, this plugin spawns a
//! flat quad mesh facing the camera.  The 3D terrain and settlement meshes are
//! left untouched; only character/wildlife entities use sprites.
//!
//! # Data flow
//! 1. `load_sprite_registry` (Startup) — scans `assets/sprites/*/manifest.json`
//!    and populates `SpriteRegistry`.
//! 2. `spawn_billboard_visuals` (PreUpdate) — for each new entity with
//!    `WorldPosition` that has a manifest, marks it with `HasSpriteSheet` and
//!    spawns a companion billboard mesh entity.
//! 3. `EntityRendererPlugin::spawn_entity_visuals` (Update) — skips any entity
//!    that already carries `HasSpriteSheet`.
//! 4. `update_billboard` (Update) — advances animation frames and reorients
//!    the quad to face the camera each frame.
//!
//! # Graceful degradation
//! When `assets/sprites/` is empty or absent (no sprite sheets generated yet),
//! `SpriteRegistry` stays empty and all entities continue to render as 3D GLB
//! models via `EntityRendererPlugin`.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use bevy::prelude::*;
use fellytip_shared::bestiary::SpriteManifest;
use fellytip_shared::components::{EntityKind, WildlifeKind, WorldPosition};

use crate::{LocalPlayer, PredictedPosition};
use crate::plugins::camera::OrbitCamera;

pub struct BillboardSpritePlugin;

impl Plugin for BillboardSpritePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpriteRegistry>()
            .add_systems(Startup, load_sprite_registry)
            .add_systems(PreUpdate, spawn_billboard_visuals)
            .add_systems(
                Update,
                (update_billboard, cleanup_orphaned_billboards),
            );
    }
}

// ── Resources ─────────────────────────────────────────────────────────────────

/// Maps entity-id string → loaded sprite manifest.
/// Populated at startup by scanning `assets/sprites/`.
#[derive(Resource, Default)]
pub struct SpriteRegistry {
    pub manifests: HashMap<String, SpriteManifest>,
}

// ── Marker components ─────────────────────────────────────────────────────────

/// Added to a game entity when `BillboardSpritePlugin` claims it.
/// `EntityRendererPlugin` skips entities that carry this marker.
#[derive(Component)]
pub struct HasSpriteSheet;

// ── State components ──────────────────────────────────────────────────────────

/// Animation and direction state for a billboard-rendered entity.
#[derive(Component)]
pub struct SpriteBillboard {
    /// Matches a key in `SpriteRegistry`.
    pub entity_id: String,
    /// Current facing direction (0 = south, clockwise, 0–7).
    pub direction: u8,
    /// Index into `manifest.animations`.
    pub anim_idx: usize,
    /// Current frame within the animation.
    pub frame: u32,
    /// Seconds elapsed since the last frame advance.
    pub frame_timer: f32,
    /// Previous translation used to compute velocity for direction.
    pub last_translation: Vec3,
    /// Handle to the quad's material — updated each frame for UV selection.
    pub material: Handle<StandardMaterial>,
    /// The companion billboard mesh entity.
    pub billboard_entity: Entity,
}

/// Marker on a billboard quad entity; references its owning game entity.
#[derive(Component)]
struct BillboardOf(Entity);

// ── Systems ───────────────────────────────────────────────────────────────────

/// Scan `assets/sprites/` for available manifests at game startup.
///
/// Uses the compile-time `CARGO_MANIFEST_DIR` path — reliable in development
/// builds; adjust `SPRITE_DIRS` for deployment.
fn load_sprite_registry(mut registry: ResMut<SpriteRegistry>) {
    // Try both the package-relative path and a cwd-relative fallback.
    let candidate_dirs: &[&str] = &[
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/sprites"),
        "crates/client/assets/sprites",
        "assets/sprites",
    ];

    for dir_str in candidate_dirs {
        let dir = std::path::Path::new(dir_str);
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut count = 0usize;
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("manifest.json");
            let Ok(json) = std::fs::read_to_string(&manifest_path) else {
                continue;
            };
            match serde_json::from_str::<SpriteManifest>(&json) {
                Ok(manifest) => {
                    tracing::info!(
                        "Loaded sprite manifest: '{}' ({} dirs × {} cols)",
                        manifest.entity_id,
                        manifest.atlas_rows,
                        manifest.atlas_cols
                    );
                    registry.manifests.insert(manifest.entity_id.clone(), manifest);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!("Bad manifest at {}: {e}", manifest_path.display());
                }
            }
        }
        if count > 0 {
            tracing::info!("Sprite registry: {count} manifests loaded from {dir_str}");
            return; // Found the sprites directory; don't search further.
        }
    }
    tracing::info!("Sprite registry: no manifests found — all entities use 3D models");
}

/// Maps entity components to the bestiary entity_id used as the sprite key.
fn entity_kind_to_id(kind: Option<&EntityKind>, wildlife: Option<&WildlifeKind>) -> Option<&'static str> {
    match kind {
        None                          => Some("player"),
        Some(EntityKind::FactionNpc)  => Some("faction_npc"),
        Some(EntityKind::Settlement)  => None, // Settlements stay as 3D props.
        Some(EntityKind::Wildlife)    => match wildlife {
            Some(WildlifeKind::Bison) | None => Some("wildlife_bison"),
            Some(WildlifeKind::Dog)           => Some("wildlife_dog"),
            Some(WildlifeKind::Horse)         => Some("wildlife_horse"),
        },
    }
}

/// For entities that have a sprite manifest, add `HasSpriteSheet` and spawn a
/// companion billboard mesh.  Runs in `PreUpdate` so `EntityRendererPlugin` in
/// `Update` sees `HasSpriteSheet` and skips those entities.
fn spawn_billboard_visuals(
    mut commands:   Commands,
    mut meshes:     ResMut<Assets<Mesh>>,
    mut materials:  ResMut<Assets<StandardMaterial>>,
    asset_server:   Res<AssetServer>,
    registry:       Res<SpriteRegistry>,
    new_entities:   Query<
        (Entity, &WorldPosition, Option<&EntityKind>, Option<&WildlifeKind>),
        Added<WorldPosition>,
    >,
) {
    for (entity, pos, kind, wildlife) in &new_entities {
        let Some(entity_id) = entity_kind_to_id(kind, wildlife) else {
            continue;
        };
        let Some(manifest) = registry.manifests.get(entity_id) else {
            continue; // No sprite sheet → entity_renderer handles it.
        };

        let atlas_path = format!("sprites/{entity_id}/atlas.png");
        let texture: Handle<Image> = asset_server.load(&atlas_path);

        let frame_w = manifest.frame_width as f32;
        let frame_h = manifest.frame_height as f32;
        let scale_x = 1.0 / manifest.atlas_cols as f32;
        let scale_y = 1.0 / manifest.atlas_rows as f32;

        let material = materials.add(StandardMaterial {
            base_color_texture: Some(texture),
            alpha_mode: AlphaMode::Mask(0.5),
            unlit: true,
            double_sided: true,
            cull_mode: None,
            uv_transform: Affine2::from_scale_angle_translation(
                Vec2::new(scale_x, scale_y),
                0.0,
                Vec2::ZERO,
            ),
            ..default()
        });

        // Sprite quad sized to one frame, pivoted at the bottom edge.
        let quad_h = frame_h / frame_w; // Aspect-correct; normalised to width=1 world unit.
        let mesh = meshes.add(Rectangle::new(1.0, quad_h));

        let world_translation = Vec3::new(pos.x, pos.z, pos.y);

        // Billboard mesh entity: separate from game entity (no parent) so
        // transform rotation doesn't fight with the game entity's transform.
        let billboard_entity = commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                Transform::from_translation(world_translation + Vec3::Y * (quad_h * 0.5)),
                BillboardOf(entity),
            ))
            .id();

        commands.entity(entity).insert((
            HasSpriteSheet,
            Transform::from_translation(world_translation),
            SpriteBillboard {
                entity_id: entity_id.to_owned(),
                direction: 0,
                anim_idx: 0,
                frame: 0,
                frame_timer: 0.0,
                last_translation: world_translation,
                material,
                billboard_entity,
            },
        ));
    }
}

/// Advance animation frames and orient each billboard quad toward the camera.
fn update_billboard(
    time:         Res<Time>,
    registry:     Res<SpriteRegistry>,
    mut mats:     ResMut<Assets<StandardMaterial>>,
    camera_q:     Query<(&OrbitCamera, &Transform), With<Camera3d>>,
    player_q:     Query<&PredictedPosition, With<LocalPlayer>>,
    mut game_q:   Query<(&Transform, &mut SpriteBillboard)>,
    mut mesh_q:   Query<&mut Transform, (With<BillboardOf>, Without<SpriteBillboard>)>,
) {
    let Ok((orbit, cam_transform)) = camera_q.single() else { return };
    let dt = time.delta_secs();

    for (game_tf, mut sprite) in &mut game_q {
        let Some(manifest) = registry.manifests.get(&sprite.entity_id) else {
            continue;
        };

        // -- Direction from velocity ---------------------------------------
        let current_pos = game_tf.translation;
        let delta = current_pos - sprite.last_translation;
        sprite.last_translation = current_pos;

        if delta.length_squared() > 1e-6 {
            // Project world velocity into screen space (subtract camera yaw).
            let vx = delta.x;
            let vz = delta.z;
            let yaw = orbit.yaw;
            let (sin_y, cos_y) = yaw.sin_cos();
            let screen_x = cos_y * vx + sin_y * vz;
            let screen_y = -sin_y * vx + cos_y * vz;
            let angle = screen_y.atan2(screen_x);
            // Map to 0–7: angle 0 = east → dir 6; south → dir 0.
            let adjusted = (angle - PI * 0.5).rem_euclid(TAU);
            sprite.direction = ((adjusted / TAU * 8.0).round() as u8) % 8;

            // Switch to walk if not already.
            if sprite.anim_idx == 0 {
                if let Some(walk_idx) = manifest.animations.iter().position(|a| a.name == "walk") {
                    sprite.anim_idx = walk_idx;
                    sprite.frame = 0;
                    sprite.frame_timer = 0.0;
                }
            }
        } else {
            // Standing still → idle.
            if sprite.anim_idx != 0 {
                sprite.anim_idx = 0;
                sprite.frame = 0;
                sprite.frame_timer = 0.0;
            }
        }

        // -- Advance frame -------------------------------------------------
        let anim = &manifest.animations[sprite.anim_idx];
        let frame_dur = if anim.fps > 0 { 1.0 / anim.fps as f32 } else { 0.25 };
        sprite.frame_timer += dt;
        if sprite.frame_timer >= frame_dur {
            sprite.frame_timer -= frame_dur;
            sprite.frame = (sprite.frame + 1) % anim.frames;
        }

        // -- Update material UV -------------------------------------------
        let col = anim.start_col + sprite.frame;
        let row = sprite.direction as u32;
        let scale_x = 1.0 / manifest.atlas_cols as f32;
        let scale_y = 1.0 / manifest.atlas_rows as f32;

        if let Some(mat) = mats.get_mut(&sprite.material) {
            mat.uv_transform = Affine2::from_scale_angle_translation(
                Vec2::new(scale_x, scale_y),
                0.0,
                Vec2::new(col as f32 * scale_x, row as f32 * scale_y),
            );
        }

        // -- Orient billboard to face camera -------------------------------
        if let Ok(mut billboard_tf) = mesh_q.get_mut(sprite.billboard_entity) {
            let entity_pos = current_pos;
            let to_cam = (cam_transform.translation - entity_pos).normalize_or_zero();
            if to_cam.length_squared() > 1e-6 {
                billboard_tf.rotation = Quat::from_rotation_arc(Vec3::Z, to_cam);
            }
            billboard_tf.translation = entity_pos + Vec3::Y * 0.5;
        }
    }

    // Sync non-SpriteBillboard entity positions for local player (uses PredictedPosition).
    if let Ok(pred) = player_q.single() {
        // The player's Transform is driven by `sync_local_player_transform` in
        // entity_renderer; we only need to update the billboard mesh position here
        // by checking BillboardOf → parent entity.  The update loop above handles
        // it via game_q which queries the Transform that entity_renderer already set.
        let _ = pred; // Already handled via game_q above.
    }
}

/// Despawn billboard mesh entities whose parent game entity no longer exists.
fn cleanup_orphaned_billboards(
    mut commands: Commands,
    billboards:   Query<(Entity, &BillboardOf)>,
    game_entities: Query<Entity, With<SpriteBillboard>>,
) {
    for (billboard_entity, BillboardOf(parent)) in &billboards {
        if game_entities.get(*parent).is_err() {
            commands.entity(billboard_entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_kind_to_id_player() {
        assert_eq!(entity_kind_to_id(None, None), Some("player"));
    }

    #[test]
    fn entity_kind_to_id_faction_npc() {
        assert_eq!(entity_kind_to_id(Some(&EntityKind::FactionNpc), None), Some("faction_npc"));
    }

    #[test]
    fn entity_kind_to_id_bison() {
        assert_eq!(
            entity_kind_to_id(Some(&EntityKind::Wildlife), Some(&WildlifeKind::Bison)),
            Some("wildlife_bison")
        );
    }

    #[test]
    fn entity_kind_to_id_settlement_is_none() {
        assert_eq!(entity_kind_to_id(Some(&EntityKind::Settlement), None), None);
    }

    #[test]
    fn direction_quantization_south() {
        use std::f32::consts::PI;
        // Moving "down" in screen space (south) → dir 0
        let yaw = PI * 0.25; // 45° camera
        let vx = -1.0_f32;
        let vz = -1.0_f32;
        let (sin_y, cos_y) = yaw.sin_cos();
        let screen_x = cos_y * vx + sin_y * vz;
        let screen_y = -sin_y * vx + cos_y * vz;
        let angle = screen_y.atan2(screen_x);
        let adjusted = (angle - PI * 0.5).rem_euclid(TAU);
        let dir = ((adjusted / TAU * 8.0).round() as u8) % 8;
        assert_eq!(dir, 0); // south
    }
}
