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
use bevy::gizmos::config::GizmoConfigStore;
use super::settings::WindmillSpinEnabled;
use crate::{ClientSet, LocalPlayer, PredictedPosition};
use fellytip_shared::components::{EntityKind, FactionBadge, GrowthStage, WildlifeKind, WorldPosition};
use fellytip_shared::world::art_direction::WorldArtDirection;
use fellytip_shared::world::civilization::{BuildingKind, Buildings, SettlementKind};
use fellytip_shared::world::map::{MAP_HALF_HEIGHT, MAP_HALF_WIDTH};
use fellytip_shared::world::zone::{ZoneMembership, ZoneRegistry, OVERWORLD_ZONE, WORLD_SUNKEN_REALM};

use super::billboard_sprite::{atlas_id_for_entity, BillboardSprites};
use super::character_animation::{CharacterAnimState, CharacterAssets, CHARACTER_SCALE};
use super::particles::{EmitterKind, ParticleEmitter};

/// When `true`, draw a gizmo sphere at every entity with `WorldPosition` so NPCs
/// are visible even when their GLB meshes haven't loaded yet.
/// Toggle via the `dm/set_character_debug` BRP method.
#[derive(Resource, Default)]
pub struct CharacterDebugOverlay(pub bool);

/// Custom gizmo group for character debug spheres so they render always-on-top
/// (depth_bias = -1.0) without affecting other gizmos.
#[derive(GizmoConfigGroup, Default, Reflect)]
pub struct CharacterDebugGizmos;

/// Marker placed on the `SceneRoot` entity for a spawned windmill building.
/// Used by `tag_windmill_children` to locate and tag the blade child node.
#[derive(Component)]
struct WindmillBuilding;

/// Marker placed on whichever entity should spin — either the whole windmill
/// or a named child node ("Blades", "Rotor", etc.) if one is found.
#[derive(Component)]
pub struct WindmillSpin;

pub struct EntityRendererPlugin;

impl Plugin for EntityRendererPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldArtDirection>()
            .init_resource::<CharacterDebugOverlay>()
            .init_gizmo_group::<CharacterDebugGizmos>()
            .add_systems(Startup, (configure_debug_gizmos, setup_entity_assets, setup_building_assets))
            .add_systems(
                Update,
                (
                    spawn_entity_visuals.in_set(ClientSet::EntityVisualSpawn),
                    spawn_building_visuals,
                    apply_faction_tint,
                    tag_windmill_children,
                    spin_windmills,
                    flicker_lantern_lights,
                    sync_remote_transforms,
                    sync_growth_stage_scale,
                    update_zone_visibility,
                    update_building_visibility,
                    sync_local_player_transform.in_set(ClientSet::SyncVisuals),
                    draw_character_debug_overlay,
                ),
            );
    }
}

fn configure_debug_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<CharacterDebugGizmos>();
    config.depth_bias = -1.0;
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

// Building physics colliders are authored by `fellytip_game::plugins::physics_world`
// from the pure `Buildings` resource — no GLB loading required, so colliders
// exist in headless mode too. Renderer is purely cosmetic for buildings now.

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
            | BuildingKind::Keep
            | BuildingKind::CapitalTower => self.tent_detailed.clone(),
            // Castle perimeter pieces (procedural ring around Capitals). No
            // Synty Fantasy Kingdom GLBs are loaded yet — physics is correct
            // via approx_half_extents, visuals use the tent placeholder until
            // Synty assets are wired up.
            BuildingKind::CastleWall
            | BuildingKind::CastleTower
            | BuildingKind::CastleGate => self.tent_detailed.clone(),
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
    mut commands:   Commands,
    buildings:      Res<Buildings>,
    assets:         Res<BuildingAssets>,
    existing:       Query<Entity, With<BuildingVisual>>,
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

        let mut ecmds = commands.spawn((
            SceneRoot(assets.scene_for(b.kind)),
            Transform::from_translation(translation)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(2.0)),
            BuildingVisual,
        ));
        if b.kind == BuildingKind::Windmill {
            ecmds.insert(WindmillBuilding);
        }
        if b.kind == BuildingKind::CampfireStones {
            ecmds.insert(ParticleEmitter {
                kind: EmitterKind::Campfire,
                timer: Timer::from_seconds(0.15, TimerMode::Repeating),
            });
        }

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
            commands.spawn((
                ParticleEmitter {
                    kind: EmitterKind::Lantern,
                    timer: Timer::from_seconds(0.2, TimerMode::Repeating),
                },
                Transform::from_translation(Vec3::new(wx, b.z + 2.5, wy)),
                BuildingVisual,
            ));
        }
    }
}

/// After a windmill GLB scene finishes loading, walk its children looking for a
/// node named "Blades", "Rotor", or similar.  Tag it (or the root itself as a
/// fallback) with `WindmillSpin` so the rotation system picks it up.
fn tag_windmill_children(
    mut commands: Commands,
    windmill_q:   Query<Entity, Added<WindmillBuilding>>,
    children_q:   Query<&Children>,
    names_q:      Query<&Name>,
) {
    for root in &windmill_q {
        // BFS over the scene hierarchy.
        let mut stack = vec![root];
        let mut blade_entity: Option<Entity> = None;
        while let Some(e) = stack.pop() {
            if let Ok(name) = names_q.get(e) {
                let n = name.as_str().to_ascii_lowercase();
                if n.contains("blade") || n.contains("rotor") || n.contains("fan") || n.contains("sail") {
                    blade_entity = Some(e);
                    break;
                }
            }
            if let Ok(children) = children_q.get(e) {
                stack.extend(children.iter());
            }
        }
        // Tag the specific blade child, or fall back to the whole scene root.
        let target = blade_entity.unwrap_or(root);
        commands.entity(target).insert(WindmillSpin);
    }
}

/// Rotate all entities tagged with `WindmillSpin` around their local Y axis.
fn spin_windmills(
    time: Res<Time>,
    mut q: Query<&mut Transform, With<WindmillSpin>>,
    windmill_spin_enabled: Option<Res<WindmillSpinEnabled>>,
) {
    // Respect the global windmill-spin toggle if the resource exists.
    if let Some(ref enabled) = windmill_spin_enabled
        && !enabled.0 {
            return;
        }
    for mut transform in &mut q {
        transform.rotate_local_y(time.delta_secs() * 0.8);
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
        if let Some(ref id) = atlas_id_for_entity(kind, badge, wildlife_kind)
            && sprites.atlases.contains_key(id.as_str()) {
                continue;
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
                // Player entities (no EntityKind) are 3× the base scale.
                let player_mul = if kind.is_none() { 3.0_f32 } else { 1.0 };
                let scale = CHARACTER_SCALE * growth_factor * player_mul;

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

type DebugOverlayItems<'a> = (
    &'a WorldPosition,
    Option<&'a EntityKind>,
    Option<&'a FactionBadge>,
    Option<&'a crate::LocalPlayer>,
);

/// Draw a gizmo sphere at every entity with `WorldPosition` when the
/// `CharacterDebugOverlay` resource is enabled. Colour encodes entity type:
/// - Local player → cyan
/// - FactionNpc (Iron Wolves) → steel blue
/// - FactionNpc (Merchant Guild) → amber
/// - FactionNpc (Ash Covenant) → crimson
/// - FactionNpc (Deep Tide) → teal
/// - FactionNpc (unknown faction) → white
/// - Wildlife (Bison / Dog / Horse) → green (all three variants share one color)
/// - Settlement → skipped (building meshes provide their own visual)
/// - BossNpc → gray fallback (server-only component, not replicated to client;
///   the boss entity has `WorldPosition` but no `EntityKind`, so it falls here)
/// - Everything else (remote players, unrecognised creatures) → gray
fn draw_character_debug_overlay(
    overlay: Res<CharacterDebugOverlay>,
    mut gizmos: Gizmos<CharacterDebugGizmos>,
    query: Query<DebugOverlayItems>,
) {
    if !overlay.0 {
        return;
    }
    for (pos, kind, badge, local_player) in &query {
        let center = Vec3::new(pos.x, pos.z + 0.5, pos.y);

        // Settlement entities have building mesh visuals — skip the debug sphere.
        if matches!(kind, Some(EntityKind::Settlement)) {
            continue;
        }

        let color = if local_player.is_some() {
            Color::srgb(0.0, 1.0, 1.0) // cyan
        } else {
            match kind {
                Some(EntityKind::FactionNpc) => match badge.map(|b| b.faction_id.as_str()) {
                    Some("iron_wolves")    => Color::srgb(0.29, 0.5, 0.65),
                    Some("merchant_guild") => Color::srgb(0.83, 0.63, 0.09),
                    Some("ash_covenant")   => Color::srgb(0.55, 0.1, 0.1),
                    Some("deep_tide")      => Color::srgb(0.1, 0.55, 0.55),
                    _                      => Color::WHITE,
                },
                // All three WildlifeKind variants (Bison, Dog, Horse) share green.
                Some(EntityKind::Wildlife) => Color::srgb(0.0, 0.8, 0.0),
                // Settlement is handled above with `continue`.
                Some(EntityKind::Settlement) => unreachable!(),
                // Gray fallback: remote players (no EntityKind), BossNpc entities
                // (server-only component, falls through without an EntityKind tag),
                // and any future entity kinds not yet handled here.
                _ => Color::srgb(0.5, 0.5, 0.5),
            }
        };
        gizmos.sphere(Isometry3d::from_translation(center), 0.5, color);
    }
}

/// Hides all [`BuildingVisual`] entities when the local player is in the Sunken
/// Realm (WORLD_SUNKEN_REALM). Uses world_id from ZoneMembership → ZoneRegistry
/// rather than a z-coordinate check, so the underground is a truly separate world.
fn update_building_visibility(
    player_q: Query<(&PredictedPosition, Option<&ZoneMembership>), With<LocalPlayer>>,
    zone_registry: Option<Res<ZoneRegistry>>,
    mut buildings: Query<&mut Visibility, With<BuildingVisual>>,
) {
    let Ok((pos, zone_membership)) = player_q.single() else { return };

    // Determine if the player is underground using world_id, falling back to z-check.
    let is_underground = if let (Some(registry), Some(membership)) = (&zone_registry, zone_membership) {
        registry.get(membership.0)
            .map(|z| z.world_id == WORLD_SUNKEN_REALM)
            .unwrap_or(false)
    } else {
        pos.z < -1.0
    };

    let desired = if is_underground { Visibility::Hidden } else { Visibility::Inherited };
    for mut vis in &mut buildings {
        if *vis != desired { *vis = desired; }
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
