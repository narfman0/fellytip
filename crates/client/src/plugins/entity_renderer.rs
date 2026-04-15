//! Renders replicated game entities (players, NPCs, settlements) as PBR meshes.
//!
//! Every entity that arrives from the server with a `WorldPosition` + `Replicated`
//! gets a visual component inserted directly:
//!
//! | `EntityKind`  | Visual                              |
//! |---------------|-------------------------------------|
//! | absent        | Kenney `characterMedium` GLB        | ← player
//! | `FactionNpc`  | Kenney `characterLarge{Male/Female}`|
//! | `Wildlife`    | procedural capsule (forest green)   |
//! | `Settlement`  | Kenney Fantasy Town Kit GLB scene   |
//!
//! A separate system keeps the Bevy `Transform` in sync as the server pushes
//! position updates.
//!
//! # Coordinate mapping
//! World `(x, y, z_elevation)` → Bevy `(x, z_elevation, y)`.
//! Character models have origin at feet — no offset needed.
//! Wildlife capsule centre is `CAPSULE_HALF_HEIGHT` above the tile surface.
//!
//! # Local-player vs remote entities
//! The local player's transform tracks `PredictedPosition` (updated every frame
//! on input) for zero-latency visual response.  Remote entity transforms still
//! track the authoritative `WorldPosition` from replication.

use bevy::prelude::*;
use crate::{ClientSet, LocalPlayer, PredictedPosition};
use fellytip_shared::components::{EntityKind, GrowthStage, WorldPosition};
use lightyear::prelude::Replicated;

use super::character_animation::{CharacterAnimState, CharacterAssets, CHARACTER_SCALE};

pub struct EntityRendererPlugin;

impl Plugin for EntityRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_entity_assets)
            .add_systems(
                Update,
                (
                    spawn_entity_visuals,
                    sync_remote_transforms,
                    sync_growth_stage_scale,
                    sync_local_player_transform.in_set(ClientSet::SyncVisuals),
                ),
            );
    }
}

// ── Assets ────────────────────────────────────────────────────────────────────

/// Procedural mesh + material handles used only for entities that don't
/// use a GLB character model (wildlife, fallback).
#[derive(Resource)]
struct EntityVisualAssets {
    capsule_mesh:    Handle<Mesh>,
    wildlife_mat:    Handle<StandardMaterial>,
    settlement_scenes: Vec<Handle<Scene>>,
}

fn setup_entity_assets(
    mut commands:  Commands,
    mut meshes:    ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server:  Res<AssetServer>,
) {
    let capsule_mesh = meshes.add(Capsule3d::new(0.3, 0.4));

    let wildlife_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.20, 0.65, 0.25), // forest green
        perceptual_roughness: 0.70,
        ..default()
    });

    let settlement_scenes = vec![
        asset_server.load("town/stall-green.glb#Scene0"),
        asset_server.load("town/stall-red.glb#Scene0"),
        asset_server.load("town/tent_detailedClosed.glb#Scene0"),
        asset_server.load("town/fountain-round.glb#Scene0"),
        asset_server.load("town/windmill.glb#Scene0"),
    ];

    commands.insert_resource(EntityVisualAssets {
        capsule_mesh,
        wildlife_mat,
        settlement_scenes,
    });
}

// ── Coordinate helpers ────────────────────────────────────────────────────────

/// Half-height offset for capsule entities so the visual base sits on terrain.
const CAPSULE_HALF_HEIGHT: f32 = 0.7;

/// Ground-level translation for models whose origin is at their feet.
fn ground_translation(pos: &WorldPosition) -> Vec3 {
    Vec3::new(pos.x, pos.z, pos.y)
}

fn capsule_translation(pos: &WorldPosition) -> Vec3 {
    Vec3::new(pos.x, pos.z + CAPSULE_HALF_HEIGHT, pos.y)
}

// ── Systems ───────────────────────────────────────────────────────────────────

type NewReplicatedPos  = (Added<WorldPosition>,   With<Replicated>);
type ChangedRemotePos  = (Changed<WorldPosition>,  With<Replicated>, Without<LocalPlayer>);
type ChangedPredictedPos = (Changed<PredictedPosition>, With<LocalPlayer>);
type RemotePosItems<'a> = (
    &'a WorldPosition,
    &'a mut Transform,
    Option<&'a EntityKind>,
    Option<&'a CharacterAnimState>,
);
type LocalPosItems<'a> = (
    &'a PredictedPosition,
    &'a mut Transform,
    Option<&'a CharacterAnimState>,
);
type SpawnVisualQuery<'w, 's> = Query<
    'w, 's,
    (
        Entity,
        &'static WorldPosition,
        Option<&'static EntityKind>,
        Option<&'static GrowthStage>,
    ),
    NewReplicatedPos,
>;

/// Fires once per entity the first time `WorldPosition` is added by replication.
fn spawn_entity_visuals(
    mut commands:   Commands,
    assets:         Res<EntityVisualAssets>,
    char_assets:    Res<CharacterAssets>,
    query:          SpawnVisualQuery,
) {
    for (entity, pos, kind, growth) in &query {
        match kind {
            // ── Settlement ────────────────────────────────────────────────────
            Some(EntityKind::Settlement) => {
                let idx   = (entity.to_bits() as usize) % assets.settlement_scenes.len();
                let scene = assets.settlement_scenes[idx].clone();
                commands.entity(entity).insert((
                    SceneRoot(scene),
                    Transform::from_translation(ground_translation(pos))
                        .with_scale(Vec3::splat(2.0)),
                ));
            }

            // ── Wildlife — procedural capsule (animals, not humanoids) ────────
            Some(EntityKind::Wildlife) => {
                let scale = growth
                    .map(|g| 0.3 + 0.7 * g.0.clamp(0.0, 1.0))
                    .unwrap_or(1.0);
                commands.entity(entity).insert((
                    Transform::from_translation(capsule_translation(pos))
                        .with_scale(Vec3::splat(scale)),
                    Mesh3d(assets.capsule_mesh.clone()),
                    MeshMaterial3d(assets.wildlife_mat.clone()),
                ));
            }

            // ── Player (no EntityKind) + FactionNpc — 3D character model ──────
            _ => {
                let growth_factor = growth
                    .map(|g| 0.3 + 0.7 * g.0.clamp(0.0, 1.0))
                    .unwrap_or(1.0);
                let scale = CHARACTER_SCALE * growth_factor;

                // Faction NPCs alternate between large male/female for visual variety.
                let scene = match kind {
                    Some(EntityKind::FactionNpc) => {
                        if entity.to_bits() % 2 == 0 {
                            char_assets.large_male_scene.clone()
                        } else {
                            char_assets.large_female_scene.clone()
                        }
                    }
                    _ => char_assets.medium_scene.clone(),
                };

                commands.entity(entity).insert((
                    SceneRoot(scene),
                    Transform::from_translation(ground_translation(pos))
                        .with_scale(Vec3::splat(scale)),
                    CharacterAnimState::default(),
                ));
            }
        }
    }
}

/// Update entity scale whenever `GrowthStage` changes (NPC child → adult).
fn sync_growth_stage_scale(
    mut query: Query<
        (&GrowthStage, &mut Transform, Option<&CharacterAnimState>),
        Changed<GrowthStage>,
    >,
) {
    for (gs, mut transform, char_anim) in &mut query {
        let growth = 0.3 + 0.7 * gs.0.clamp(0.0, 1.0);
        // Character models use CHARACTER_SCALE as their base; capsules use 1.0.
        let base = if char_anim.is_some() { CHARACTER_SCALE } else { 1.0 };
        transform.scale = Vec3::splat(base * growth);
    }
}

/// Keeps remote-entity transforms in sync with authoritative `WorldPosition`.
fn sync_remote_transforms(
    mut query: Query<RemotePosItems, ChangedRemotePos>,
) {
    for (pos, mut transform, kind, char_anim) in &mut query {
        transform.translation = if char_anim.is_some() {
            ground_translation(pos)
        } else {
            match kind {
                Some(EntityKind::Settlement) => ground_translation(pos),
                _                            => capsule_translation(pos),
            }
        };
    }
}

/// Keeps the local player's transform in sync with `PredictedPosition`.
fn sync_local_player_transform(
    mut query: Query<LocalPosItems, ChangedPredictedPos>,
) {
    for (pred, mut transform, char_anim) in &mut query {
        transform.translation = if char_anim.is_some() {
            Vec3::new(pred.x, pred.z, pred.y)
        } else {
            Vec3::new(pred.x, pred.z + CAPSULE_HALF_HEIGHT, pred.y)
        };
    }
}
