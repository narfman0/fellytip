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

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::prelude::*;
use crate::{ClientSet, LocalPlayer, PredictedPosition};
use fellytip_shared::components::{EntityKind, FactionBadge, GrowthStage, WildlifeKind, WorldPosition};
use fellytip_shared::world::civilization::{BuildingKind, Buildings, SettlementKind};
use fellytip_shared::world::map::{MAP_HALF_HEIGHT, MAP_HALF_WIDTH};
use fellytip_shared::world::zone::{ZoneMembership, OVERWORLD_ZONE};

use super::billboard_sprite::{atlas_id_for_entity, BillboardSprites};
use super::character_animation::{CharacterAnimState, CharacterAssets, CHARACTER_SCALE};

pub struct EntityRendererPlugin;

impl Plugin for EntityRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (setup_entity_assets, setup_building_assets))
            .add_systems(
                Update,
                (
                    spawn_entity_visuals,
                    spawn_building_visuals,
                    apply_faction_tint,
                    flicker_lantern_lights,
                    sync_remote_transforms,
                    sync_growth_stage_scale,
                    update_zone_visibility,
                    sync_local_player_transform.in_set(ClientSet::SyncVisuals),
                ),
            );
    }
}

/// Per-lantern flicker state: unique phase offset so each lantern flickers independently.
#[derive(Component)]
struct LanternFlicker {
    phase: f32,
}

// ── Assets ────────────────────────────────────────────────────────────────────

#[derive(Resource)]
struct EntityVisualAssets {
    /// `[bison, dog, horse]` — index matches `WildlifeKind` variant order.
    wildlife_scenes:   [Handle<Scene>; 3],
    /// Scenes used for Town-kind settlements (small camps, tents).
    town_scenes:       Vec<Handle<Scene>>,
    /// Scenes used for Capital-kind settlements (larger buildings).
    capital_scenes:    Vec<Handle<Scene>>,
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

// ── Building assets ───────────────────────────────────────────────────────────

/// Marker component on locally-spawned building visual entities.
#[derive(Component)]
struct BuildingVisual;

/// GLB scene handles for all [`BuildingKind`] variants.
#[derive(Resource)]
struct BuildingAssets {
    tent_detailed:   Handle<Scene>,
    tent_small:      Handle<Scene>,
    campfire_stones: Handle<Scene>,
    windmill:        Handle<Scene>,
    stall:           Handle<Scene>,
    stall_bench:     Handle<Scene>,
    stall_green:     Handle<Scene>,
    stall_red:       Handle<Scene>,
    fountain:        Handle<Scene>,
    lantern:         Handle<Scene>,
}

impl BuildingAssets {
    fn scene_for(&self, kind: BuildingKind) -> Handle<Scene> {
        match kind {
            BuildingKind::TentDetailed   => self.tent_detailed.clone(),
            BuildingKind::TentSmall      => self.tent_small.clone(),
            BuildingKind::CampfireStones => self.campfire_stones.clone(),
            BuildingKind::Windmill       => self.windmill.clone(),
            BuildingKind::Stall          => self.stall.clone(),
            BuildingKind::StallBench     => self.stall_bench.clone(),
            BuildingKind::StallGreen     => self.stall_green.clone(),
            BuildingKind::StallRed       => self.stall_red.clone(),
            BuildingKind::Fountain       => self.fountain.clone(),
            BuildingKind::Lantern        => self.lantern.clone(),
            // Multi-story interior building kinds have no GLB scene yet —
            // they are purely zone-graph metadata. Fall back to a generic asset.
            BuildingKind::Tavern
            | BuildingKind::Barracks
            | BuildingKind::Tower
            | BuildingKind::Keep         => self.tent_detailed.clone(),
        }
    }
}

fn setup_building_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(BuildingAssets {
        tent_detailed:   asset_server.load("nature/tent_detailedClosed.glb#Scene0"),
        tent_small:      asset_server.load("nature/tent_smallClosed.glb#Scene0"),
        campfire_stones: asset_server.load("nature/campfire_stones.glb#Scene0"),
        windmill:        asset_server.load("town/windmill.glb#Scene0"),
        stall:           asset_server.load("town/stall.glb#Scene0"),
        stall_bench:     asset_server.load("town/stall-bench.glb#Scene0"),
        stall_green:     asset_server.load("town/stall-green.glb#Scene0"),
        stall_red:       asset_server.load("town/stall-red.glb#Scene0"),
        fountain:        asset_server.load("town/fountain-round.glb#Scene0"),
        lantern:         asset_server.load("town/lantern.glb#Scene0"),
    });
}

/// Spawns (or respawns) local building entities whenever the `Buildings` resource changes.
///
/// Building entities are purely client-side; they are not replicated.
fn spawn_building_visuals(
    mut commands:  Commands,
    buildings:     Res<Buildings>,
    assets:        Res<BuildingAssets>,
    existing:      Query<Entity, With<BuildingVisual>>,
) {
    if !buildings.is_changed() { return; }

    // Despawn old visuals before respawning (handles seed change via apply_world_meta).
    for e in &existing {
        commands.entity(e).despawn();
    }

    for b in &buildings.0 {
        let wx = b.tx as f32 - MAP_HALF_WIDTH as f32 + 0.5;
        let wy = b.ty as f32 - MAP_HALF_HEIGHT as f32 + 0.5;
        let translation = Vec3::new(wx, b.z, wy);
        let rotation = Quat::from_rotation_y(b.rotation as f32 * FRAC_PI_2);

        commands.spawn((
            SceneRoot(assets.scene_for(b.kind)),
            Transform::from_translation(translation)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(2.0)),
            BuildingVisual,
        ));

        if b.kind == BuildingKind::Lantern {
            // Unique phase per lantern so each flickers independently.
            let phase = (b.tx as f32 * 7.3 + b.ty as f32 * 3.7).rem_euclid(TAU);
            commands.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.72, 0.25),
                    intensity: 1_200.0,
                    radius: 0.3,
                    range: 10.0,
                    shadows_enabled: false,
                    ..default()
                },
                // Place the light at the lantern flame height (model top ~ 2 units tall at scale 2).
                Transform::from_translation(Vec3::new(wx, b.z + 2.5, wy)),
                LanternFlicker { phase },
                BuildingVisual,
            ));
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

    let town_scenes = vec![
        asset_server.load("nature/tent_detailedClosed.glb#Scene0"),
        asset_server.load("nature/tent_smallClosed.glb#Scene0"),
    ];

    let capital_scenes = vec![
        asset_server.load("town/windmill.glb#Scene0"),
        asset_server.load("town/stall-green.glb#Scene0"),
        asset_server.load("town/stall-red.glb#Scene0"),
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
        town_scenes,
        capital_scenes,
        iron_wolves_mat,
        merchant_guild_mat,
        ash_covenant_mat,
        deep_tide_mat,
    });
}

fn flicker_lantern_lights(
    time: Res<Time>,
    mut q: Query<(&mut PointLight, &LanternFlicker)>,
) {
    let t = time.elapsed_secs();
    for (mut light, flicker) in &mut q {
        let slow = f32::sin(t * 2.3 + flicker.phase) * 0.12;
        let fast = f32::sin(t * 17.1 + flicker.phase * 1.9) * 0.06;
        let scale = (1.0 + slow + fast).clamp(0.7, 1.3);
        light.intensity = 1_200.0 * scale;
    }
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
//
// Using a structural filter instead of Added<WorldPosition> so that entities
// spawned during Startup/PostStartup are caught — Added<T> misses them because
// their added_tick precedes the first Update run of this system.
type NewReplicatedPos  = (With<WorldPosition>, Without<SceneRoot>);
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
        Option<&'static SettlementKind>,
    ),
    NewReplicatedPos,
>;

/// Fires once per entity the first time `WorldPosition` is added by replication.
/// Skips PBR scene insertion for entities whose billboard atlas is already loaded
/// — the billboard renderer handles those.
fn spawn_entity_visuals(
    mut commands:   Commands,
    assets:         Res<EntityVisualAssets>,
    char_assets:    Res<CharacterAssets>,
    sprites:        Res<BillboardSprites>,
    query:          SpawnVisualQuery,
) {
    for (entity, pos, kind, growth, badge, wildlife_kind, settlement_kind) in &query {
        // Skip PBR if a billboard atlas is loaded for this entity kind.
        if let Some(ref id) = atlas_id_for_entity(kind, badge, wildlife_kind) {
            if sprites.atlases.contains_key(id.as_str()) {
                continue;
            }
        }
        match kind {
            // ── Settlement ────────────────────────────────────────────────────
            // Spawn only the center-point marker; surrounding buildings are
            // spawned separately by spawn_building_visuals from Buildings resource.
            Some(EntityKind::Settlement) => {
                let scene = match settlement_kind {
                    Some(SettlementKind::Capital) => assets.capital_scenes[0].clone(), // windmill
                    _ => assets.town_scenes[0].clone(), // detailed tent
                };
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

// ZONE VISIBILITY: client-side only. Server still replicates all entities.
// Do NOT "fix" this by filtering on the server without a proper Lightyear
// interest management implementation. See docs/systems/zones.md.
//
// Entities with `ZoneMembership` are shown only when the local player shares
// their zone. Entities without `ZoneMembership` are treated as always visible
// (overworld-only ambient entities).
fn update_zone_visibility(
    player_zone_q: Query<Option<&ZoneMembership>, With<LocalPlayer>>,
    mut entities: Query<(&ZoneMembership, &mut Visibility), Without<LocalPlayer>>,
) {
    let player_zone = player_zone_q
        .single()
        .ok()
        .and_then(|opt| opt.copied())
        .map(|z| z.0)
        .unwrap_or(OVERWORLD_ZONE);
    for (membership, mut visibility) in &mut entities {
        let desired = if membership.0 == player_zone {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *visibility != desired {
            *visibility = desired;
        }
    }
}
