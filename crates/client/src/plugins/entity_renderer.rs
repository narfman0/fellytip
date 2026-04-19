//! Renders replicated game entities (players, NPCs, settlements) as PBR meshes.
//!
//! Every entity that arrives from the server with a `WorldPosition` + `Replicated`
//! gets a visual component inserted directly:
//!
//! | `EntityKind`  | Visual                                      |
//! |---------------|---------------------------------------------|
//! | absent        | Kenney `characterMedium` GLB                | ← player
//! | `FactionNpc`  | Kenney `characterLarge{Male/Female}`        |
//! | `Wildlife`    | Kenney Prototype Kit animal GLB (3 species) |
//! | `Settlement`  | Kenney Fantasy Town Kit GLB scene          |
//!
//! A separate system keeps the Bevy `Transform` in sync as the server pushes
//! position updates.
//!
//! # Coordinate mapping
//! World `(x, y, z_elevation)` → Bevy `(x, z_elevation, y)`.
//! Character models have origin at feet — no offset needed.
//! All entity origins are at feet — no offset needed for any entity type.
//!
//! # Local-player vs remote entities
//! The local player's transform tracks `PredictedPosition` (updated every frame
//! on input) for zero-latency visual response.  Remote entity transforms still
//! track the authoritative `WorldPosition` from replication.

use bevy::prelude::*;
use crate::{ClientSet, LocalPlayer, PredictedPosition};
use fellytip_shared::components::{EntityKind, FactionBadge, GrowthStage, WildlifeKind, WorldPosition};

use super::character_animation::{CharacterAnimState, CharacterAssets, CHARACTER_SCALE};

pub struct EntityRendererPlugin;

impl Plugin for EntityRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_entity_assets)
            .add_systems(
                Update,
                (
                    spawn_entity_visuals,
                    apply_faction_tint,
                    sync_remote_transforms,
                    sync_growth_stage_scale,
                    sync_local_player_transform.in_set(ClientSet::SyncVisuals),
                ),
            );
    }
}

// ── Assets ────────────────────────────────────────────────────────────────────

#[derive(Resource)]
struct EntityVisualAssets {
    /// `[bison, dog, horse]` — index matches `WildlifeKind` variant order.
    wildlife_scenes:   [Handle<Scene>; 3],
    settlement_scenes: Vec<Handle<Scene>>,
    // Per-faction tint materials applied to faction NPC child meshes.
    iron_wolves_mat:    Handle<StandardMaterial>,
    merchant_guild_mat: Handle<StandardMaterial>,
    ash_covenant_mat:   Handle<StandardMaterial>,
    deep_tide_mat:      Handle<StandardMaterial>,
}

impl EntityVisualAssets {
    /// Return the tint material for a faction NPC, or `None` for unknown factions.
    fn faction_tint(&self, badge: &FactionBadge) -> Option<Handle<StandardMaterial>> {
        match badge.faction_id.as_str() {
            "iron_wolves"    => Some(self.iron_wolves_mat.clone()),
            "merchant_guild" => Some(self.merchant_guild_mat.clone()),
            "ash_covenant"   => Some(self.ash_covenant_mat.clone()),
            "deep_tide"      => Some(self.deep_tide_mat.clone()),
            _                => None,
        }
    }
}

fn setup_entity_assets(
    mut commands:  Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server:  Res<AssetServer>,
) {
    let wildlife_scenes = [
        asset_server.load("nature/animal-bison.glb#Scene0"),
        asset_server.load("nature/animal-dog.glb#Scene0"),
        asset_server.load("nature/animal-horse.glb#Scene0"),
    ];

    let settlement_scenes = vec![
        asset_server.load("town/stall-green.glb#Scene0"),
        asset_server.load("town/stall-red.glb#Scene0"),
        asset_server.load("nature/tent_detailedClosed.glb#Scene0"),
        asset_server.load("town/fountain-round.glb#Scene0"),
        asset_server.load("town/windmill.glb#Scene0"),
    ];

    // Per-faction tint materials — applied to child `Mesh3d` entities spawned
    // under the faction NPC's GLB SceneRoot so each faction reads as a distinct
    // colour without replacing the full character model.
    let iron_wolves_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 0.50, 0.85), // steel blue
        perceptual_roughness: 0.55,
        ..default()
    });
    let merchant_guild_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.65, 0.10), // amber
        perceptual_roughness: 0.45,
        ..default()
    });
    let ash_covenant_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.80, 0.10, 0.10), // crimson
        perceptual_roughness: 0.60,
        ..default()
    });
    let deep_tide_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.05, 0.55, 0.60), // deep teal
        perceptual_roughness: 0.65,
        ..default()
    });

    commands.insert_resource(EntityVisualAssets {
        wildlife_scenes,
        settlement_scenes,
        iron_wolves_mat,
        merchant_guild_mat,
        ash_covenant_mat,
        deep_tide_mat,
    });
}

// ── Coordinate helpers ────────────────────────────────────────────────────────

/// World → Bevy coordinate mapping: `(x, z_elevation, y)`.
/// All entity GLB origins are at the model's feet, so no vertical offset is needed.
fn ground_translation(pos: &WorldPosition) -> Vec3 {
    Vec3::new(pos.x, pos.z, pos.y)
}

// ── Systems ───────────────────────────────────────────────────────────────────

// All entities with WorldPosition get visuals; local player is excluded from
// remote-position sync since its transform tracks PredictedPosition instead.
// MULTIPLAYER: restore With<Replicated> filters to limit to server-sent entities.
type NewReplicatedPos  = Added<WorldPosition>;
type ChangedRemotePos  = (Changed<WorldPosition>, Without<LocalPlayer>);
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
        Option<&'static FactionBadge>,
        Option<&'static WildlifeKind>,
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
    for (entity, pos, kind, growth, badge, wildlife_kind) in &query {
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

            // ── Wildlife — Kenney Prototype Kit animal GLB ────────────────────
            Some(EntityKind::Wildlife) => {
                let scene_idx = match wildlife_kind {
                    Some(WildlifeKind::Bison) | None => 0,
                    Some(WildlifeKind::Dog)           => 1,
                    Some(WildlifeKind::Horse)          => 2,
                };
                let scale = growth
                    .map(|g| 0.3 + 0.7 * g.0.clamp(0.0, 1.0))
                    .unwrap_or(1.0);
                commands.entity(entity).insert((
                    SceneRoot(assets.wildlife_scenes[scene_idx].clone()),
                    Transform::from_translation(ground_translation(pos))
                        .with_scale(Vec3::splat(scale)),
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

                // Store the faction tint handle as a component so the child-mesh
                // tinting system can apply it once the GLB scene finishes loading.
                let mut cmd = commands.entity(entity);
                cmd.insert((
                    SceneRoot(scene),
                    Transform::from_translation(ground_translation(pos))
                        .with_scale(Vec3::splat(scale)),
                    CharacterAnimState::default(),
                ));
                if let Some(tint) = badge.and_then(|b| assets.faction_tint(b)) {
                    cmd.insert(FactionTintHandle(tint));
                }
            }
        }
    }
}

/// Deferred material handle stored on faction NPCs while their GLB scene loads.
/// Consumed by `apply_faction_tint` once child mesh entities exist.
#[derive(Component)]
struct FactionTintHandle(Handle<StandardMaterial>);

/// Apply faction tint colours to child `Mesh3d` entities after the GLB scene loads.
///
/// Runs every frame until the `FactionTintHandle` is removed.  Child entities are
/// only present after `SceneRoot` finishes spawning, so we retry each frame.
fn apply_faction_tint(
    mut commands:  Commands,
    tinted:        Query<(Entity, &FactionTintHandle), With<CharacterAnimState>>,
    children_q:    Query<&Children>,
    mesh_entities: Query<Entity, With<Mesh3d>>,
) {
    for (root, tint_handle) in &tinted {
        // Collect all descendant mesh entities.
        let mut found_any = false;
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if mesh_entities.contains(e) {
                commands.entity(e).insert(MeshMaterial3d(tint_handle.0.clone()));
                found_any = true;
            }
            if let Ok(children) = children_q.get(e) {
                stack.extend(children.iter());
            }
        }
        // Once at least one mesh was found the scene is loaded; remove the handle.
        if found_any {
            commands.entity(root).remove::<FactionTintHandle>();
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
        let base = if char_anim.is_some() { CHARACTER_SCALE } else { 1.0 };
        transform.scale = Vec3::splat(base * growth);
    }
}

/// Keeps remote-entity transforms in sync with authoritative `WorldPosition`.
fn sync_remote_transforms(
    mut query: Query<RemotePosItems, ChangedRemotePos>,
) {
    for (pos, mut transform, _kind, _char_anim) in &mut query {
        transform.translation = ground_translation(pos);
    }
}

/// Keeps the local player's transform in sync with `PredictedPosition`.
fn sync_local_player_transform(
    mut query: Query<LocalPosItems, ChangedPredictedPos>,
) {
    for (pred, mut transform, _char_anim) in &mut query {
        transform.translation = Vec3::new(pred.x, pred.z, pred.y);
    }
}
