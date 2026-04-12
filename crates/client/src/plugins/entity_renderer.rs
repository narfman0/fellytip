//! Renders replicated game entities (players, NPCs, settlements) as PBR meshes.
//!
//! Every entity that arrives from the server with a `WorldPosition` + `Replicated`
//! gets a mesh inserted directly.  Visual appearance is determined by `EntityKind`:
//!
//! | `EntityKind`  | Mesh     | Colour       |
//! |---------------|----------|--------------|
//! | absent        | capsule  | warm gold    | ← player
//! | `FactionNpc`  | capsule  | steel blue   |
//! | `Wildlife`    | capsule  | forest green |
//! | `Settlement`  | pillar   | bright white |
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
use crate::{LocalPlayer, PredictedPosition};
use fellytip_shared::components::{EntityKind, WorldPosition};
use lightyear::prelude::Replicated;

pub struct EntityRendererPlugin;

impl Plugin for EntityRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_entity_assets)
            .add_systems(
                Update,
                (spawn_entity_visuals, sync_remote_transforms, sync_local_player_transform),
            );
    }
}

// ── Assets ────────────────────────────────────────────────────────────────────

/// Pre-built mesh + material handles for all entity visual variants.
#[derive(Resource)]
struct EntityVisualAssets {
    capsule_mesh: Handle<Mesh>,
    /// Tall cylinder used for settlement markers.
    pillar_mesh: Handle<Mesh>,
    /// Warm gold — local and remote players.
    player_mat: Handle<StandardMaterial>,
    /// Steel blue — faction guard NPCs.
    faction_npc_mat: Handle<StandardMaterial>,
    /// Forest green — ecology wildlife.
    wildlife_mat: Handle<StandardMaterial>,
    /// Bright white — settlement markers.
    settlement_mat: Handle<StandardMaterial>,
}

fn setup_entity_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let capsule_mesh = meshes.add(Capsule3d::new(0.3, 0.4));
    // Radius 0.2, height 3.0 — thin pillar standing 3 units tall.
    let pillar_mesh = meshes.add(Cylinder::new(0.2, 3.0));

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
    let settlement_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.95, 0.95), // bright white
        perceptual_roughness: 0.30,
        emissive: LinearRgba::new(0.15, 0.15, 0.15, 1.0),
        ..default()
    });

    commands.insert_resource(EntityVisualAssets {
        capsule_mesh,
        pillar_mesh,
        player_mat,
        faction_npc_mat,
        wildlife_mat,
        settlement_mat,
    });
}

// ── Coordinate helpers ────────────────────────────────────────────────────────

/// Half-height offset for capsule entities so the visual base sits on terrain.
const CAPSULE_HALF_HEIGHT: f32 = 0.7; // half of total capsule height (~1.4 / 2)

/// Half-height offset for pillar entities (cylinder height 3.0 / 2).
const PILLAR_HALF_HEIGHT: f32 = 1.5;

fn capsule_translation(pos: &WorldPosition) -> Vec3 {
    Vec3::new(pos.x, pos.z + CAPSULE_HALF_HEIGHT, pos.y)
}

fn pillar_translation(pos: &WorldPosition) -> Vec3 {
    Vec3::new(pos.x, pos.z + PILLAR_HALF_HEIGHT, pos.y)
}

// ── Systems ───────────────────────────────────────────────────────────────────

type NewReplicatedPos = (Added<WorldPosition>, With<Replicated>);
type ChangedRemotePos = (Changed<WorldPosition>, With<Replicated>, Without<LocalPlayer>);
type ChangedPredictedPos = (Changed<PredictedPosition>, With<LocalPlayer>);

/// Fires once per entity the first time `WorldPosition` is added by replication.
/// Inserts the visual components directly onto the replicated entity.
fn spawn_entity_visuals(
    mut commands: Commands,
    assets: Res<EntityVisualAssets>,
    query: Query<(Entity, &WorldPosition, Option<&EntityKind>), NewReplicatedPos>,
) {
    for (entity, pos, kind) in &query {
        let (mesh, material, translation) = match kind {
            Some(EntityKind::FactionNpc) => (
                assets.capsule_mesh.clone(),
                assets.faction_npc_mat.clone(),
                capsule_translation(pos),
            ),
            Some(EntityKind::Wildlife) => (
                assets.capsule_mesh.clone(),
                assets.wildlife_mat.clone(),
                capsule_translation(pos),
            ),
            Some(EntityKind::Settlement) => (
                assets.pillar_mesh.clone(),
                assets.settlement_mat.clone(),
                pillar_translation(pos),
            ),
            None => (
                assets.capsule_mesh.clone(),
                assets.player_mat.clone(),
                capsule_translation(pos),
            ),
        };

        commands.entity(entity).insert((
            Transform::from_translation(translation),
            Mesh3d(mesh),
            MeshMaterial3d(material),
        ));
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
            Some(EntityKind::Settlement) => pillar_translation(pos),
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
