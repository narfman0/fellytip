//! Renders replicated game entities (players, NPCs, settlements) as PBR meshes.
//!
//! Every entity that arrives from the server with a `WorldPosition` + `Replicated`
//! gets a mesh inserted directly.  Visual appearance is determined by `EntityKind`:
//!
//! | `EntityKind`  | Mesh              | Appearance          |
//! |---------------|-------------------|---------------------|
//! | absent        | capsule           | warm gold           | ← player
//! | `FactionNpc`  | capsule           | steel blue          |
//! | `Wildlife`    | capsule           | forest green        |
//! | `Settlement`  | GLB scene (Kenney Fantasy Town Kit) | 3D building |
//!
//! A separate system keeps the Bevy `Transform` in sync as the server pushes
//! position updates.
//!
//! # Coordinate mapping
//! Same convention as `tile_renderer`: world (x, y, z_elevation) → Bevy (x, z_elevation, y).
//! Capsule centre is placed `CAPSULE_HALF_HEIGHT` above the tile surface; pillar
//! centre is placed `PILLAR_HALF_HEIGHT` above.
//!
//! # Local-player vs remote entities
//! The local player's transform tracks `PredictedPosition` (updated every frame
//! on input) for zero-latency visual response.  Remote entity transforms still
//! track the authoritative `WorldPosition` from replication.

use bevy::prelude::*;
use crate::{ClientSet, LocalPlayer, PredictedPosition};
use fellytip_shared::components::{EntityKind, GrowthStage, WorldPosition};
use lightyear::prelude::Replicated;

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

/// Pre-built mesh + material handles for all entity visual variants.
///
/// Settlement entities use GLB scene handles instead of procedural meshes.
#[derive(Resource)]
struct EntityVisualAssets {
    capsule_mesh: Handle<Mesh>,
    /// Warm gold — local and remote players.
    player_mat: Handle<StandardMaterial>,
    /// Steel blue — faction guard NPCs.
    faction_npc_mat: Handle<StandardMaterial>,
    /// Forest green — ecology wildlife.
    wildlife_mat: Handle<StandardMaterial>,
    /// Fantasy Town Kit GLB scenes — one is picked per settlement based on entity id.
    settlement_scenes: Vec<Handle<Scene>>,
}

fn setup_entity_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
) {
    let capsule_mesh = meshes.add(Capsule3d::new(0.3, 0.4));

    let player_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.75, 0.20), // warm gold
        perceptual_roughness: 0.55,
        ..default()
    });
    let faction_npc_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 0.50, 0.85), // steel blue
        perceptual_roughness: 0.55,
        ..default()
    });
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
        player_mat,
        faction_npc_mat,
        wildlife_mat,
        settlement_scenes,
    });
}

// ── Coordinate helpers ────────────────────────────────────────────────────────

/// Half-height offset for capsule entities so the visual base sits on terrain.
const CAPSULE_HALF_HEIGHT: f32 = 0.7; // half of total capsule height (~1.4 / 2)

fn capsule_translation(pos: &WorldPosition) -> Vec3 {
    Vec3::new(pos.x, pos.z + CAPSULE_HALF_HEIGHT, pos.y)
}

fn settlement_translation(pos: &WorldPosition) -> Vec3 {
    // GLB models have their origin at the base; place directly on terrain surface.
    Vec3::new(pos.x, pos.z, pos.y)
}

// ── Systems ───────────────────────────────────────────────────────────────────

type NewReplicatedPos = (Added<WorldPosition>, With<Replicated>);
type ChangedRemotePos = (Changed<WorldPosition>, With<Replicated>, Without<LocalPlayer>);
type ChangedPredictedPos = (Changed<PredictedPosition>, With<LocalPlayer>);
type SpawnVisualQuery<'w, 's> =
    Query<'w, 's, (Entity, &'static WorldPosition, Option<&'static EntityKind>, Option<&'static GrowthStage>), NewReplicatedPos>;

/// Fires once per entity the first time `WorldPosition` is added by replication.
/// Inserts the visual components directly onto the replicated entity.
fn spawn_entity_visuals(
    mut commands: Commands,
    assets: Res<EntityVisualAssets>,
    query: SpawnVisualQuery,
) {
    for (entity, pos, kind, growth) in &query {
        match kind {
            Some(EntityKind::Settlement) => {
                // Hash the entity's generation+index bits to pick a stable variant.
                let idx = (entity.to_bits() as usize) % assets.settlement_scenes.len();
                let scene = assets.settlement_scenes[idx].clone();
                let translation = settlement_translation(pos);
                commands.entity(entity).insert((
                    SceneRoot(scene),
                    Transform::from_translation(translation).with_scale(Vec3::splat(2.0)),
                ));
            }
            _ => {
                let (mesh, material) = match kind {
                    Some(EntityKind::FactionNpc) => (
                        assets.capsule_mesh.clone(),
                        assets.faction_npc_mat.clone(),
                    ),
                    Some(EntityKind::Wildlife) => (
                        assets.capsule_mesh.clone(),
                        assets.wildlife_mat.clone(),
                    ),
                    _ => (
                        assets.capsule_mesh.clone(),
                        assets.player_mat.clone(),
                    ),
                };
                let translation = capsule_translation(pos);

                // Apply initial scale from GrowthStage (0.0 = newborn, 1.0 = adult).
                let scale = growth
                    .map(|g| 0.3 + 0.7 * g.0.clamp(0.0, 1.0))
                    .unwrap_or(1.0);

                commands.entity(entity).insert((
                    Transform::from_translation(translation).with_scale(Vec3::splat(scale)),
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                ));
            }
        }
    }
}

/// Update capsule scale whenever `GrowthStage` changes (fractional growth each tick).
fn sync_growth_stage_scale(
    mut query: Query<(&GrowthStage, &mut Transform), Changed<GrowthStage>>,
) {
    for (gs, mut transform) in &mut query {
        let scale = 0.3 + 0.7 * gs.0.clamp(0.0, 1.0);
        transform.scale = Vec3::splat(scale);
    }
}

/// Keeps remote-entity transforms in sync with authoritative `WorldPosition`.
///
/// Excludes the local player: its transform is driven by `PredictedPosition`
/// in `sync_local_player_transform` for zero-latency visual feedback.
fn sync_remote_transforms(
    mut query: Query<(&WorldPosition, &mut Transform, Option<&EntityKind>), ChangedRemotePos>,
) {
    for (pos, mut transform, kind) in &mut query {
        transform.translation = match kind {
            Some(EntityKind::Settlement) => settlement_translation(pos),
            _ => capsule_translation(pos),
        };
    }
}

/// Keeps the local player's transform in sync with `PredictedPosition` so
/// visual movement is immediate — no 50 ms server round-trip required.
fn sync_local_player_transform(
    mut query: Query<(&PredictedPosition, &mut Transform), ChangedPredictedPos>,
) {
    for (pred, mut transform) in &mut query {
        transform.translation = Vec3::new(pred.x, pred.z + CAPSULE_HALF_HEIGHT, pred.y);
    }
}
