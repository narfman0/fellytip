//! Renders replicated game entities (players, NPCs, settlements) as PBR meshes.
//!
//! Every entity that arrives from the server with a `WorldPosition` + `Replicated`
//! gets a visual component inserted directly:
//!
//! | `EntityKind`  | Visual                                           |
//! |---------------|--------------------------------------------------|
//! | absent        | Kenney `characterMedium` GLB                     | ← player
//! | `FactionNpc`  | Kenney `characterLarge{Male/Female}`             |
//! | `Wildlife`    | Kenney Prototype Kit animal GLB (3 species)      |
//! | `Building`    | Synty Polygon Fantasy Kingdom preset GLBs        |
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
use super::settings::{BuildingLodSettings, WindmillSpinEnabled};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::asset::RenderAssetUsages;
use crate::{ClientSet, LocalPlayer, PredictedPosition};
use fellytip_shared::components::{EntityKind, FactionBadge, GrowthStage, WildlifeKind, WorldPosition};
use fellytip_shared::world::art_direction::WorldArtDirection;
use fellytip_shared::world::civilization::{BuildingKind, Buildings, SettlementKind};
use fellytip_shared::world::faction::faction_archetype;
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

/// LOD component attached to tower-kind building parent entities.
/// Hides `full_entity` (the procedural mesh group) and shows `simple_entity`
/// (a flat-coloured cuboid) when the camera is beyond `distance_threshold`.
#[derive(Component)]
pub struct BuildingLod {
    pub full_entity:        Entity,
    pub simple_entity:      Entity,
    pub distance_threshold: f32,
}

pub struct EntityRendererPlugin;

impl Plugin for EntityRendererPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldArtDirection>()
            .init_resource::<CharacterDebugOverlay>()
            .init_gizmo_group::<CharacterDebugGizmos>()
            .add_systems(Startup, (configure_debug_gizmos, setup_entity_assets, setup_building_assets, setup_tower_assets))
            .add_systems(
                Update,
                (
                    spawn_entity_visuals,
                    spawn_building_visuals,
                    apply_faction_tint,
                    tag_windmill_children,
                    spin_windmills,
                    update_building_lod,
                    flicker_lantern_lights,
                    occlude_fade_system,
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

/// Marker for wall panels that should fade when they occlude the player's view.
#[derive(Component)]
struct OccludeFade;

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

#[derive(Resource)]
struct TowerMaterials {
    wall_mat: Handle<StandardMaterial>,
    wall_fade_mat: Handle<StandardMaterial>,
    roof_mat: Handle<StandardMaterial>,
}

/// GLB scene handles for building visuals. Houses 01-10 are Synty preset models
/// selected by building-ID hash for settlement variety.
#[derive(Resource)]
struct BuildingAssets {
    // Synty Polygon Fantasy Kingdom — 10 preset house variants.
    synty_houses:     [Handle<Scene>; 10],
    // Synty specialty buildings.
    synty_tavern:     Handle<Scene>,
    synty_blacksmith: Handle<Scene>,
    synty_church:     Handle<Scene>,
    synty_stables:    Handle<Scene>,
    synty_huts:       [Handle<Scene>; 2],
    // Synty windmill — base and blades are separate GLBs spawned as siblings.
    windmill_base:    Handle<Scene>,
    windmill_blades:  Handle<Scene>,
    // Kenney campfire (no Synty equivalent; used for campfire particle anchor).
    campfire_stones:  Handle<Scene>,
}

fn generate_stone_texture(images: &mut Assets<Image>) -> Handle<Image> {
    const W: u32 = 64;
    const H: u32 = 64;
    let mut data = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let row = y / 8;
            let col_offset: u32 = if row % 2 == 0 { 0 } else { 8 };
            let bx = (x + col_offset) % 16;
            let by = y % 8;
            let is_mortar = bx == 0 || by == 0;
            let h1 = x.wrapping_mul(2654435761_u32).wrapping_add(y.wrapping_mul(2246822519_u32));
            let noise = (h1 & 0x1F) as u8;
            let (r, g, b) = if is_mortar {
                (
                    160_u8.saturating_add(noise / 4),
                    155_u8.saturating_add(noise / 4),
                    150_u8.saturating_add(noise / 4),
                )
            } else {
                (
                    110_u8.saturating_add(noise),
                    90_u8.saturating_add(noise / 2),
                    75_u8.saturating_add(noise / 2),
                )
            };
            data.extend_from_slice(&[r, g, b, 255]);
        }
    }
    let mut image = Image::new(
        Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::Descriptor(
        bevy::image::ImageSamplerDescriptor {
            address_mode_u: bevy::image::ImageAddressMode::Repeat,
            address_mode_v: bevy::image::ImageAddressMode::Repeat,
            mag_filter: bevy::image::ImageFilterMode::Linear,
            min_filter: bevy::image::ImageFilterMode::Linear,
            ..Default::default()
        },
    );
    images.add(image)
}

fn setup_building_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    let s = |p: &str| asset_server.load(format!("synty/buildings/{p}#Scene0"));
    commands.insert_resource(BuildingAssets {
        synty_houses: [
            s("house_01.glb"), s("house_02.glb"), s("house_03.glb"),
            s("house_04.glb"), s("house_05.glb"), s("house_06.glb"),
            s("house_07.glb"), s("house_08.glb"), s("house_09.glb"),
            s("house_10.glb"),
        ],
        synty_tavern:     s("tavern.glb"),
        synty_blacksmith: s("blacksmith.glb"),
        synty_church:     s("church.glb"),
        synty_stables:    s("stables.glb"),
        synty_huts:       [s("hut_01.glb"), s("hut_02.glb")],
        windmill_base:    s("windmill_base.glb"),
        windmill_blades:  s("windmill_blades.glb"),
        campfire_stones:  asset_server.load("nature/campfire_stones.glb#Scene0"),
    });
}

fn setup_tower_assets(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    art_dir: Res<WorldArtDirection>,
) {
    // Default materials use the Surface world art style (WorldId 0).
    let surface = art_dir.get(0);
    let [wr, wg, wb] = surface.building_wall_color;
    let [rr, rg, rb] = surface.building_roof_color;

    let stone_tex = generate_stone_texture(&mut images);
    let wall_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(wr, wg, wb),
        base_color_texture: Some(stone_tex),
        perceptual_roughness: 0.9,
        metallic: 0.0,
        ..default()
    });
    let stone_fade_tex = generate_stone_texture(&mut images);
    let wall_fade_mat = materials.add(StandardMaterial {
        base_color_texture: Some(stone_fade_tex),
        base_color: Color::srgba(1.0, 1.0, 1.0, 0.15),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.9,
        metallic: 0.0,
        cull_mode: None,
        double_sided: true,
        ..default()
    });
    let roof_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(rr, rg, rb),
        perceptual_roughness: 0.85,
        ..default()
    });
    commands.insert_resource(TowerMaterials { wall_mat, wall_fade_mat, roof_mat });
}

fn tower_params(kind: BuildingKind, id_bytes: &[u8; 16]) -> (u32, f32) {
    let seed = u64::from_le_bytes(id_bytes[0..8].try_into().unwrap());
    match kind {
        BuildingKind::CapitalTower => {
            let floors = 7 + (seed % 4) as u32;
            let base_width = 3.0 + (seed >> 8 & 0x3) as f32 * 0.4;
            (floors, base_width)
        }
        BuildingKind::Tower => {
            let floors = 4 + (seed % 3) as u32;
            let base_width = 2.0 + (seed >> 8 & 0x1) as f32 * 0.5;
            (floors, base_width)
        }
        BuildingKind::Keep => (3, 2.5),
        BuildingKind::Tavern | BuildingKind::Barracks => (2, 2.0),
        _ => (1, 1.0),
    }
}

/// Build a box mesh with UV coordinates tiled based on physical dimensions.
/// All four vertical faces map U=horizontal/tile_size, V=vertical/tile_size (V=0 at top).
/// tile_size controls how many world units equal one full texture repetition.
fn build_tiled_box_mesh(w: f32, h: f32, d: f32, tile_size: f32) -> Mesh {
    let hw = w * 0.5;
    let hh = h * 0.5;
    let hd = d * 0.5;

    let uw = w / tile_size;
    let uh = h / tile_size;
    let ud = d / tile_size;

    // 24 vertices: 4 per face × 6 faces.
    // Vertex order per face: [bottom-left, bottom-right, top-right, top-left]
    // when viewed from outside (CCW winding from outside → correct backface culling).
    //
    // Coordinate conventions (right-hand, Y-up):
    //   Front (+z): screen-right = +x
    //   Back  (-z): screen-right = -x
    //   Right (+x): screen-right = -z
    //   Left  (-x): screen-right = +z
    let positions: Vec<[f32; 3]> = vec![
        // Front (+z normal): BL, BR, TR, TL
        [-hw, -hh,  hd], [ hw, -hh,  hd], [ hw,  hh,  hd], [-hw,  hh,  hd],
        // Back  (-z normal): BL(+x side), BR(-x side), TR, TL
        [ hw, -hh, -hd], [-hw, -hh, -hd], [-hw,  hh, -hd], [ hw,  hh, -hd],
        // Right (+x normal): BL(+z side), BR(-z side), TR, TL
        [ hw, -hh,  hd], [ hw, -hh, -hd], [ hw,  hh, -hd], [ hw,  hh,  hd],
        // Left  (-x normal): BL(-z side), BR(+z side), TR, TL
        [-hw, -hh, -hd], [-hw, -hh,  hd], [-hw,  hh,  hd], [-hw,  hh, -hd],
        // Top   (+y normal): BL(-x,+z), BR(+x,+z), TR(+x,-z), TL(-x,-z)
        [-hw,  hh,  hd], [ hw,  hh,  hd], [ hw,  hh, -hd], [-hw,  hh, -hd],
        // Bottom(-y normal): BL(-x,-z), BR(+x,-z), TR(+x,+z), TL(-x,+z)
        [-hw, -hh, -hd], [ hw, -hh, -hd], [ hw, -hh,  hd], [-hw, -hh,  hd],
    ];

    let normals: Vec<[f32; 3]> = vec![
        [ 0., 0., 1.], [ 0., 0., 1.], [ 0., 0., 1.], [ 0., 0., 1.], // front
        [ 0., 0.,-1.], [ 0., 0.,-1.], [ 0., 0.,-1.], [ 0., 0.,-1.], // back
        [ 1., 0., 0.], [ 1., 0., 0.], [ 1., 0., 0.], [ 1., 0., 0.], // right
        [-1., 0., 0.], [-1., 0., 0.], [-1., 0., 0.], [-1., 0., 0.], // left
        [ 0., 1., 0.], [ 0., 1., 0.], [ 0., 1., 0.], [ 0., 1., 0.], // top
        [ 0.,-1., 0.], [ 0.,-1., 0.], [ 0.,-1., 0.], [ 0.,-1., 0.], // bottom
    ];

    // For all vertical faces: U = horizontal screen distance / tile_size, V = height from top / tile_size.
    // BL→[0, uh], BR→[face_width/ts, uh], TR→[face_width/ts, 0], TL→[0, 0]
    // For top/bottom faces: U = width/ts, V = depth/ts.
    let uvs: Vec<[f32; 2]> = vec![
        // Front (+z): horizontal = x, width = w
        [0., uh], [uw, uh], [uw, 0.], [0., 0.],
        // Back  (-z): horizontal = -x, width = w (same tiling)
        [0., uh], [uw, uh], [uw, 0.], [0., 0.],
        // Right (+x): horizontal = -z (BL is at +z, BR at -z), width = d
        [0., uh], [ud, uh], [ud, 0.], [0., 0.],
        // Left  (-x): horizontal = +z (BL is at -z, BR at +z), width = d
        [0., uh], [ud, uh], [ud, 0.], [0., 0.],
        // Top: U = x width, V = z depth
        [0., ud], [uw, ud], [uw, 0.], [0., 0.],
        // Bottom
        [0., ud], [uw, ud], [uw, 0.], [0., 0.],
    ];

    // Two triangles per face: (BL,BR,TR) and (BL,TR,TL) — CCW from outside.
    let indices: Vec<u32> = (0..6u32)
        .flat_map(|f| {
            let b = f * 4;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect();

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Spawn a hollow procedural tower and return `(full_group_entity, simple_box_entity,
/// lod_anchor_entity)` so the caller can attach a `BuildingLod` component.
///
/// * `full_group_entity`  — invisible transform parent containing all wall/roof meshes.
/// * `simple_box_entity`  — flat-coloured cuboid shown at distance.
/// * `lod_anchor_entity`  — zero-size entity at the tower base used to measure
///   camera distance; holds the `BuildingLod` component.
#[allow(clippy::too_many_arguments)]
fn spawn_hollow_tower(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    wx: f32,
    wy: f32,
    base_z: f32,
    kind: BuildingKind,
    id_bytes: &[u8; 16],
    tower_mats: &TowerMaterials,
    wall_color_override: Option<[f32; 3]>,
    roof_color_override: Option<[f32; 3]>,
) {
    let (floors, base_width) = tower_params(kind, id_bytes);
    const TAPER: f32 = 0.06;
    const FLOOR_H: f32 = 3.0;
    const TILE_SIZE: f32 = 2.0;
    const WALL_T: f32 = 0.2;

    // Build per-tower wall/roof materials when color overrides are requested;
    // otherwise reuse the shared default materials from TowerMaterials.
    let wall_mat_handle = if let Some([r, g, b]) = wall_color_override {
        materials.add(StandardMaterial {
            base_color: Color::srgb(r, g, b),
            perceptual_roughness: 0.9,
            metallic: 0.0,
            ..default()
        })
    } else {
        tower_mats.wall_mat.clone()
    };
    let roof_mat_handle = if let Some([r, g, b]) = roof_color_override {
        materials.add(StandardMaterial {
            base_color: Color::srgb(r, g, b),
            perceptual_roughness: 0.85,
            ..default()
        })
    } else {
        tower_mats.roof_mat.clone()
    };

    // The "simple" LOD box colour matches the wall colour (or a neutral stone grey).
    let lod_color = wall_color_override.unwrap_or([0.55, 0.50, 0.45]);
    let simple_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(lod_color[0], lod_color[1], lod_color[2]),
        perceptual_roughness: 0.95,
        metallic: 0.0,
        ..default()
    });

    let total_h = floors as f32 * FLOOR_H;
    let simple_mesh = meshes.add(Cuboid::new(base_width, total_h, base_width));
    let simple_center_y = base_z + total_h * 0.5;

    // Spawn the full detail group (transform-only parent; children added below).
    let full_group = commands.spawn((
        Transform::default(),
        Visibility::Inherited,
        BuildingVisual,
    )).id();

    // Spawn the simple LOD box (hidden by default; shown at distance).
    let simple_entity = commands.spawn((
        Mesh3d(simple_mesh),
        MeshMaterial3d(simple_mat),
        Transform::from_translation(Vec3::new(wx, simple_center_y, wy)),
        Visibility::Hidden,
        BuildingVisual,
    )).id();

    // LOD anchor entity placed at the tower base; holds the BuildingLod component
    // so `update_building_lod` can measure camera distance from it.
    let lod_threshold = match kind {
        BuildingKind::CapitalTower => 120.0,
        BuildingKind::Tower | BuildingKind::Keep => 100.0,
        _ => 80.0,
    };
    commands.spawn((
        Transform::from_translation(Vec3::new(wx, base_z, wy)),
        GlobalTransform::default(),
        BuildingLod {
            full_entity: full_group,
            simple_entity,
            distance_threshold: lod_threshold,
        },
        BuildingVisual,
    ));

    // Spawn wall panels as children of the full_group.
    for floor in 0..floors {
        let w = (base_width * (1.0 - TAPER * floor as f32)).max(1.0);
        let hw = w * 0.5;
        let center_y = base_z + floor as f32 * FLOOR_H + FLOOR_H * 0.5;

        // North (+Z), South (-Z, entrance on floor 0), East (+X), West (-X)
        let panel_specs: [(f32, f32, bool); 4] = [
            (0.0,   hw,  false),
            (0.0,  -hw,  true ),  // south = entrance
            ( hw,  0.0,  false),
            (-hw,  0.0,  false),
        ];

        for (dx, dz, is_south) in panel_specs {
            if is_south && floor == 0 { continue; }
            let (mesh_w, mesh_d) = if dz.abs() > dx.abs() {
                (w, WALL_T)
            } else {
                (WALL_T, w)
            };
            let mesh = meshes.add(build_tiled_box_mesh(mesh_w, FLOOR_H, mesh_d, TILE_SIZE));
            let panel = commands.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(wall_mat_handle.clone()),
                Transform::from_translation(Vec3::new(wx + dx, center_y, wy + dz)),
                BuildingVisual,
                OccludeFade,
            )).id();
            commands.entity(full_group).add_child(panel);
        }

        if floor > 0 {
            let slab_y = base_z + floor as f32 * FLOOR_H;
            let slab_mesh = meshes.add(build_tiled_box_mesh(
                (w - WALL_T * 2.0).max(0.2),
                0.15,
                (w - WALL_T * 2.0).max(0.2),
                TILE_SIZE,
            ));
            let slab = commands.spawn((
                Mesh3d(slab_mesh),
                MeshMaterial3d(roof_mat_handle.clone()),
                Transform::from_translation(Vec3::new(wx, slab_y, wy)),
                BuildingVisual,
            )).id();
            commands.entity(full_group).add_child(slab);
        }
    }

    let top_w = (base_width * (1.0 - TAPER * floors as f32) * 1.15).max(1.2);
    let cap_y = base_z + floors as f32 * FLOOR_H + 0.4;
    let cap_mesh = meshes.add(build_tiled_box_mesh(top_w, 0.8, top_w, TILE_SIZE));
    let cap = commands.spawn((
        Mesh3d(cap_mesh),
        MeshMaterial3d(roof_mat_handle.clone()),
        Transform::from_translation(Vec3::new(wx, cap_y, wy)),
        BuildingVisual,
    )).id();
    commands.entity(full_group).add_child(cap);
}

/// Spawns (or respawns) local building entities whenever the `Buildings` resource changes.
///
/// Building entities are purely client-side; they are not replicated.
#[allow(clippy::too_many_arguments)]
fn spawn_building_visuals(
    mut commands:   Commands,
    mut meshes:     ResMut<Assets<Mesh>>,
    mut materials:  ResMut<Assets<StandardMaterial>>,
    buildings:      Res<Buildings>,
    assets:         Res<BuildingAssets>,
    tower_mats:     Res<TowerMaterials>,
    art_dir:        Res<WorldArtDirection>,
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

        // Procedural towers for multi-floor interior buildings (Tavern now uses Synty GLB).
        if matches!(b.kind, BuildingKind::CapitalTower | BuildingKind::Tower | BuildingKind::Keep | BuildingKind::Barracks) {
            let world_id = if b.z < 0.0 { 1u32 } else { 0u32 };
            let art = art_dir.get(world_id);
            let (wall_color, roof_color) = if let Some(ref fid) = b.faction_id {
                let arch = faction_archetype(fid.as_str());
                (Some(arch.tower_wall_color), Some(arch.tower_roof_color))
            } else {
                (Some(art.building_wall_color), Some(art.building_roof_color))
            };
            spawn_hollow_tower(
                &mut commands,
                &mut meshes,
                &mut materials,
                wx, wy, b.z, b.kind,
                b.id.as_bytes(),
                &tower_mats,
                wall_color,
                roof_color,
            );
            continue;
        }

        // Pick a Synty GLB scene based on building kind and the deterministic style_variant.
        let id_hash = b.style_variant as usize;
        let scene: Option<Handle<Scene>> = match b.kind {
            // Camps and outposts → small huts.
            BuildingKind::TentSmall | BuildingKind::TentDetailed
                => Some(assets.synty_huts[id_hash % 2].clone()),
            // Craft / trade buildings — each stall type gets a fitting Synty preset.
            BuildingKind::Stall | BuildingKind::StallGreen
                => Some(assets.synty_stables.clone()),
            BuildingKind::StallBench | BuildingKind::StallRed
                => Some(assets.synty_blacksmith.clone()),
            // Capital center landmark → church / large civic building.
            BuildingKind::Fountain
                => Some(assets.synty_church.clone()),
            // Faction tavern → Synty tavern preset.
            BuildingKind::Tavern
                => Some(assets.synty_tavern.clone()),
            // Windmill base (blades spawned as a separate sibling below).
            BuildingKind::Windmill
                => Some(assets.windmill_base.clone()),
            // Campfire → Kenney model kept for the particle effect anchor.
            BuildingKind::CampfireStones
                => Some(assets.campfire_stones.clone()),
            // Lantern → no mesh; only the PointLight + particle emitter below.
            BuildingKind::Lantern
                => None,
            // Generic fallback — houses 06-10 (upper half of set).
            _   => Some(assets.synty_houses[5 + id_hash % 5].clone()),
        };

        if let Some(scene_handle) = scene {
            let mut ecmds = commands.spawn((
                SceneRoot(scene_handle),
                Transform::from_translation(translation)
                    .with_rotation(rotation)
                    .with_scale(Vec3::splat(1.0)),
                BuildingVisual,
            ));
            if b.kind == BuildingKind::CampfireStones {
                ecmds.insert(ParticleEmitter {
                    kind: EmitterKind::Campfire,
                    timer: Timer::from_seconds(0.15, TimerMode::Repeating),
                });
            }
        }

        if b.kind == BuildingKind::Windmill {
            // Blades are a separate Synty GLB at the same origin; tag directly with WindmillSpin.
            commands.spawn((
                SceneRoot(assets.windmill_blades.clone()),
                Transform::from_translation(translation)
                    .with_rotation(rotation)
                    .with_scale(Vec3::splat(1.0)),
                WindmillSpin,
                BuildingVisual,
            ));
        }

        if b.kind == BuildingKind::Lantern {
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
    if let Some(ref enabled) = windmill_spin_enabled {
        if !enabled.0 {
            return;
        }
    }
    for mut transform in &mut q {
        transform.rotate_local_y(time.delta_secs() * 0.8);
    }
}

/// Toggle visibility between the full procedural tower mesh group and a simple
/// coloured cuboid based on camera distance.
fn update_building_lod(
    camera_q:             Query<&GlobalTransform, With<Camera>>,
    lod_q:                Query<(&BuildingLod, &GlobalTransform)>,
    mut visibility:       Query<&mut Visibility>,
    lod_settings:         Option<Res<BuildingLodSettings>>,
) {
    let Ok(cam_gt) = camera_q.single() else { return };
    let cam_pos = cam_gt.translation();

    // If LOD is disabled, always show the full-detail mesh.
    let lod_enabled = lod_settings.as_ref().map(|s| s.enabled).unwrap_or(true);

    for (lod, transform) in &lod_q {
        let show_full = if lod_enabled {
            let threshold = lod_settings.as_ref()
                .map(|s| s.distance)
                .unwrap_or(lod.distance_threshold);
            let dist = cam_pos.distance(transform.translation());
            dist < threshold
        } else {
            true
        };
        if let Ok(mut vis) = visibility.get_mut(lod.full_entity) {
            let desired = if show_full { Visibility::Inherited } else { Visibility::Hidden };
            if *vis != desired { *vis = desired; }
        }
        if let Ok(mut vis) = visibility.get_mut(lod.simple_entity) {
            let desired = if show_full { Visibility::Hidden } else { Visibility::Inherited };
            if *vis != desired { *vis = desired; }
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

fn occlude_fade_system(
    camera_q: Query<&Transform, With<crate::plugins::camera::OrbitCamera>>,
    player_q: Query<&Transform, With<crate::LocalPlayer>>,
    mut wall_q: Query<(&Transform, &mut MeshMaterial3d<StandardMaterial>), With<OccludeFade>>,
    tower_mats: Res<TowerMaterials>,
) {
    let Ok(cam_t) = camera_q.single() else { return };
    let Ok(player_t) = player_q.single() else { return };

    let cam_pos = cam_t.translation;
    let player_pos = player_t.translation;
    let seg = player_pos - cam_pos;
    let seg_len_sq = seg.length_squared();

    for (wall_t, mut mat) in &mut wall_q {
        let to_wall = wall_t.translation - cam_pos;
        let t = if seg_len_sq > 0.001 {
            to_wall.dot(seg) / seg_len_sq
        } else {
            0.0
        };
        let proj = cam_pos + seg * t.clamp(0.0, 1.0);
        let perp_dist = (wall_t.translation - proj).length();
        let should_fade = t > 0.05 && t < 0.98 && perp_dist < 3.0;
        let target = if should_fade {
            tower_mats.wall_fade_mat.clone()
        } else {
            tower_mats.wall_mat.clone()
        };
        if mat.0 != target {
            mat.0 = target;
        }
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
