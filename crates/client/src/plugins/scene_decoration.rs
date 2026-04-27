//! Scatter biome-appropriate 3D decorations (trees, rocks, vegetation) across
//! terrain chunks using Kenney Nature Kit GLB assets.
//!
//! # Instancing design
//!
//! Instead of `SceneRoot` (one scene hierarchy per decoration), we load each GLB
//! as a raw `Handle<Gltf>`, extract the `(Handle<Mesh>, Handle<StandardMaterial>)`
//! pairs for every primitive once the asset is ready, and then spawn decoration
//! entities with `Mesh3d` + `MeshMaterial3d` directly.
//!
//! Because every instance of `tree_default` shares the **exact same**
//! `Handle<Mesh>` and `Handle<StandardMaterial>`, Bevy's render batcher folds
//! them into a single draw call regardless of instance count.  This allows high
//! density (original 25–30% biome coverage) without GPU overhead.
//!
//! Multi-primitive models (e.g., trunk + foliage) are handled as a parent entity
//! (transform only) with one child entity per primitive.  Despawning the parent
//! automatically cascades to all children in Bevy 0.18's hierarchy system.
//!
//! # Load sequencing
//!
//! GLTF assets are async.  `finalize_decoration_assets` runs every frame until
//! all variant handles have been extracted; only then does the pending-chunk queue
//! start draining.  GLB files are small, so this typically resolves in < 1 s.
//!
//! # Density distribution
//!
//! Placement uses two-octave value noise so forest patches have dense cores
//! (both coarse and fine noise are low) that thin out toward edges (coarse is
//! low but fine flips high for many tiles).  A secondary isolated-tree pass
//! scatters a handful of lone trees into adjacent open biome tiles.
//!
//! # Tree growth
//!
//! Newly-spawned tree entities (not rocks/bushes) receive a `TreeGrowth`
//! component.  Each tree is born at a deterministic age fraction derived from
//! its tile hash so the world looks populated on first load.  `grow_trees`
//! advances age each frame and removes the component once the tree is mature,
//! stopping all per-frame work.  Future civilisation harvesting can despawn
//! the parent entity; a tile→entity reverse map is the planned extension point.
//!
//! # Coordinate convention
//! Tile `(gx, gy)` → Bevy `(gx − half_w, terrain_height, gy − half_h)`.

use std::collections::{HashMap, VecDeque};

use bevy::gltf::{Gltf, GltfMesh};
use bevy::prelude::*;
use fellytip_shared::world::map::{TileKind, WorldMap};

use super::terrain::chunk::{vertex_height, ChunkCoord};
use super::terrain::lod::CHUNK_TILES;
use super::terrain::{manager::ChunkManager, ChunkLifecycle};

pub struct SceneDecorationPlugin;

// ── Tuning constants ──────────────────────────────────────────────────────────

/// Chebyshev chunk distance within which decorations are placed.
const DECORATION_RADIUS: i32 = 10;

/// Maximum decoration entities per chunk.  Safety ceiling; density constants
/// below are the primary control.
const MAX_PER_CHUNK: usize = 64;

/// Pending chunks processed per frame — keeps frame time stable.
const CHUNKS_PER_FRAME: usize = 3;

/// Real-time seconds for a tree to grow from seedling to full size.
/// Varies ±1 min per individual tree (hash-derived).
const TREE_MATURE_SECS_BASE: f32 = 60.0;
const TREE_MATURE_SECS_RANGE: f32 = 120.0;

impl Plugin for SceneDecorationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_decoration_assets)
            .add_systems(
                Update,
                (finalize_decoration_assets, apply_decorations, grow_trees, sway_trees).chain(),
            );
    }
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// One GLB model with (optionally) extracted per-primitive mesh+material handles.
struct DecorationVariant {
    gltf: Handle<Gltf>,
    /// One entry per GLTF primitive.  Empty until `finalize_decoration_assets`
    /// has run and the asset has loaded.
    primitives: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
}

impl DecorationVariant {
    fn new(gltf: Handle<Gltf>) -> Self {
        Self { gltf, primitives: Vec::new() }
    }
    fn is_ready(&self) -> bool {
        !self.primitives.is_empty()
    }
}

/// Drives visual growth for a tree decoration.  Removed once the tree is mature.
#[derive(Component)]
struct TreeGrowth {
    target_scale: f32,
    mature_secs:  f32,
    age_secs:     f32,
}

/// Marker component placed on tree parent entities so `sway_trees` can animate them.
#[derive(Component)]
pub struct TreeSway;

// ── Resources ─────────────────────────────────────────────────────────────────

/// All decoration variants grouped by biome category.
#[derive(Resource)]
struct DecorationAssets {
    broadleaf: Vec<DecorationVariant>,
    conifer:   Vec<DecorationVariant>,
    desert:    Vec<DecorationVariant>,
    tropical:  Vec<DecorationVariant>,
    rocks:     Vec<DecorationVariant>,
    bushes:    Vec<DecorationVariant>,
    /// True once every variant has had its primitives extracted.
    all_ready: bool,
}

impl DecorationAssets {
    /// Flat mutable iterator over every variant across all categories.
    fn all_variants_mut(&mut self) -> impl Iterator<Item = &mut DecorationVariant> {
        self.broadleaf.iter_mut()
            .chain(self.conifer.iter_mut())
            .chain(self.desert.iter_mut())
            .chain(self.tropical.iter_mut())
            .chain(self.rocks.iter_mut())
            .chain(self.bushes.iter_mut())
    }
}

/// Per-chunk spawn tracking + pending queue.
#[derive(Resource, Default)]
struct DecorationState {
    /// Root decoration entity per chunk (parent of all primitives in that chunk).
    spawned: HashMap<ChunkCoord, Vec<Entity>>,
    /// Chunks awaiting decoration; drained at `CHUNKS_PER_FRAME` per frame.
    pending: VecDeque<ChunkCoord>,
}

// ── Startup ───────────────────────────────────────────────────────────────────

fn setup_decoration_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    let v = |path: &str| DecorationVariant::new(asset_server.load(format!("nature/{path}")));

    commands.insert_resource(DecorationAssets {
        broadleaf: vec![
            v("tree_default.glb"),
            v("tree_oak.glb"),
            v("tree_tall.glb"),
            v("tree_detailed.glb"),
        ],
        conifer: vec![
            v("tree_pineDefaultA.glb"),
            v("tree_pineDefaultB.glb"),
            v("tree_pineTallA.glb"),
        ],
        desert: vec![
            v("cactus_tall.glb"),
            v("cactus_short.glb"),
        ],
        tropical: vec![
            v("tree_palm.glb"),
            v("tree_palmTall.glb"),
        ],
        rocks: vec![
            v("rock_largeA.glb"),
            v("rock_largeB.glb"),
            v("rock_largeC.glb"),
            v("rock_tallA.glb"),
            v("rock_tallB.glb"),
            v("rock_tallC.glb"),
        ],
        bushes: vec![
            v("plant_bush.glb"),
            v("plant_bushLarge.glb"),
            v("grass.glb"),
            v("grass_large.glb"),
        ],
        all_ready: false,
    });
    commands.init_resource::<DecorationState>();
}

// ── Finalization system ───────────────────────────────────────────────────────

/// Runs every frame until all GLTF assets are loaded, extracting per-primitive
/// mesh and material handles.  No-ops instantly once `all_ready` is set.
fn finalize_decoration_assets(
    mut deco:          ResMut<DecorationAssets>,
    gltf_assets:       Res<Assets<Gltf>>,
    gltf_mesh_assets:  Res<Assets<GltfMesh>>,
) {
    if deco.all_ready { return; }

    let mut all_ready = true;
    for variant in deco.all_variants_mut() {
        if variant.is_ready() { continue; }

        let Some(gltf) = gltf_assets.get(&variant.gltf) else {
            all_ready = false;
            continue;
        };
        let Some(gltf_mesh_handle) = gltf.meshes.first() else { continue };
        let Some(gltf_mesh) = gltf_mesh_assets.get(gltf_mesh_handle) else {
            all_ready = false;
            continue;
        };

        for prim in &gltf_mesh.primitives {
            // Skip primitives without a material (shouldn't happen for Kenney assets).
            let Some(mat) = prim.material.clone() else { continue };
            variant.primitives.push((prim.mesh.clone(), mat));
        }

        // If the mesh had no valid primitives, retry next frame.
        if variant.primitives.is_empty() {
            all_ready = false;
        }
    }
    deco.all_ready = all_ready;
}

// ── Decoration system ─────────────────────────────────────────────────────────

fn apply_decorations(
    mut commands:  Commands,
    mut state:     ResMut<DecorationState>,
    mut lifecycle: ResMut<ChunkLifecycle>,
    deco:          Res<DecorationAssets>,
    map:           Res<WorldMap>,
    mgr:           Res<ChunkManager>,
) {
    // ── Despawn hidden chunks (always immediate) ───────────────────────────────

    for (coord, _) in lifecycle.newly_hidden.iter() {
        if let Some(entities) = state.spawned.remove(coord) {
            for ent in entities {
                commands.entity(ent).despawn();
            }
        }
        state.pending.retain(|c| c != coord);
    }

    // ── Enqueue newly visible chunks (radius-filtered) ─────────────────────────

    let cam_chunk = mgr.last_cam_chunk;
    for (coord, _) in lifecycle.newly_visible.iter().copied() {
        if state.spawned.contains_key(&coord) || state.pending.contains(&coord) {
            continue;
        }
        if let Some(cc) = cam_chunk {
            let dist = (coord.cx - cc.cx).abs().max((coord.cy - cc.cy).abs());
            if dist > DECORATION_RADIUS { continue; }
        }
        state.pending.push_back(coord);
    }

    lifecycle.newly_visible.clear();
    lifecycle.newly_hidden.clear();

    // ── Process pending queue (only when all assets are ready) ─────────────────

    if !deco.all_ready { return; }

    let half_w = (map.width  / 2) as i32;
    let half_h = (map.height / 2) as i32;

    for _ in 0..CHUNKS_PER_FRAME {
        let Some(coord) = state.pending.pop_front() else { break };

        if state.spawned.contains_key(&coord) { continue; }

        // Drop chunks that drifted outside the radius while queued.
        if let Some(cc) = cam_chunk {
            let dist = (coord.cx - cc.cx).abs().max((coord.cy - cc.cy).abs());
            if dist > DECORATION_RADIUS { continue; }
        }

        let mut chunk_roots: Vec<Entity> = Vec::new();

        let base_x = coord.cx * CHUNK_TILES as i32;
        let base_y = coord.cy * CHUNK_TILES as i32;

        'tile: for dy in 0..CHUNK_TILES as i32 {
            for dx in 0..CHUNK_TILES as i32 {
                if chunk_roots.len() >= MAX_PER_CHUNK { break 'tile; }

                let gx = (base_x + dx).clamp(0, map.width  as i32 - 1) as usize;
                let gy = (base_y + dy).clamp(0, map.height as i32 - 1) as usize;

                let kind = map.column(gx, gy).layers
                    .iter().rev()
                    .find(|l| l.is_surface_kind())
                    .or_else(|| map.column(gx, gy).layers.last())
                    .map(|l| l.kind)
                    .unwrap_or(TileKind::Void);

                let Some((variants, density, scale_base, grows)) =
                    decoration_for_biome(kind, &deco) else { continue };

                // ── Density gate: two-octave patch noise ──────────────────────
                //
                // The coarse octave (11 tiles) defines where clusters exist.
                // The fine octave (4 tiles) adds internal variation so cluster
                // cores are denser and edges thin out naturally.
                let noise = patch_noise(map.seed, gx, gy);
                let passed_main = noise < density as f32 / 256.0;

                // ── Isolated-tree secondary pass ──────────────────────────────
                //
                // A small per-biome probability lets lone trees appear even
                // outside the main cluster zone, giving an ecotone feel.
                let passed_isolated = if !passed_main {
                    let iso_threshold: u32 = match kind {
                        TileKind::Forest | TileKind::TemperateForest        =>  8, // ~3%
                        TileKind::Taiga                                      =>  8,
                        TileKind::TropicalForest | TileKind::TropicalRainforest => 10, // ~4%
                        TileKind::Plains | TileKind::Grassland               =>  4, // ~1.5%
                        _ => 0,
                    };
                    if iso_threshold == 0 {
                        false
                    } else {
                        let iso_h = tile_hash(map.seed ^ 0xFACE_CAFE_DEAD_BEEF, gx, gy);
                        ((iso_h & 0xFF) as u32) < iso_threshold
                    }
                } else {
                    false
                };

                if !passed_main && !passed_isolated { continue; }

                let h = tile_hash(map.seed, gx, gy);

                // Pick variant; skip if primitives not yet extracted.
                let idx     = (h as usize) % variants.len();
                let variant = &variants[idx];
                if !variant.is_ready() { continue; }

                let yaw       = (((h >>  8) & 0xFF) as f32 / 255.0) * std::f32::consts::TAU;
                let scale_var = 0.9 + (((h >> 16) & 0xFF) as f32 / 255.0) * 0.2;
                let target_scale = scale_base * scale_var;

                let bx = gx as f32 - half_w as f32;
                let bz = gy as f32 - half_h as f32;
                let by = vertex_height(&map, gx, gy);

                // ── Initial scale for growing trees ───────────────────────────
                //
                // Bits 32–39: age fraction 0%–90% so trees start at varied stages.
                // Bits 40–47: mature time jitter.
                let (spawn_scale, growth) = if grows {
                    let age_frac   = (((h >> 32) & 0xFF) as f32 / 255.0) * 0.9;
                    let mature_secs = TREE_MATURE_SECS_BASE
                        + ((h >> 40) & 0xFF) as f32 / 255.0 * TREE_MATURE_SECS_RANGE;
                    let age_secs   = age_frac * mature_secs;
                    // Smoothstep so sapling sizes are visually distinct.
                    let t0 = age_frac * age_frac * (3.0 - 2.0 * age_frac);
                    let initial_scale = (target_scale * t0).max(target_scale * 0.05);
                    let g = TreeGrowth { target_scale, mature_secs, age_secs };
                    (initial_scale, Some(g))
                } else {
                    (target_scale, None)
                };

                let parent_transform = Transform::from_xyz(bx, by, bz)
                    .with_rotation(Quat::from_rotation_y(yaw))
                    .with_scale(Vec3::splat(spawn_scale));

                let primitives: Vec<_> = variant.primitives.clone();

                let mut entity_cmds = commands.spawn((parent_transform, Visibility::Visible));
                if let Some(g) = growth {
                    entity_cmds.insert(g);
                }
                // Tag trees (and other growing plants) for wind sway animation.
                if grows {
                    entity_cmds.insert(TreeSway);
                }
                let parent = entity_cmds
                    .with_children(|p| {
                        for (mesh, mat) in &primitives {
                            p.spawn((
                                Mesh3d(mesh.clone()),
                                MeshMaterial3d(mat.clone()),
                                Transform::default(),
                            ));
                        }
                    })
                    .id();

                chunk_roots.push(parent);
            }
        }

        state.spawned.insert(coord, chunk_roots);
    }
}

// ── Growth system ─────────────────────────────────────────────────────────────

/// Advances tree ages each frame and updates their Transform scale.
/// The component is removed once the tree reaches full size, ending all updates.
fn grow_trees(
    time:     Res<Time>,
    mut cmds: Commands,
    mut q:    Query<(Entity, &mut Transform, &mut TreeGrowth)>,
) {
    let dt = time.delta_secs();
    for (entity, mut tf, mut g) in &mut q {
        g.age_secs += dt;
        let t = (g.age_secs / g.mature_secs).min(1.0);
        // Smoothstep easing — fast early growth, decelerates near maturity.
        let eased = t * t * (3.0 - 2.0 * t);
        tf.scale = Vec3::splat((g.target_scale * eased).max(g.target_scale * 0.05));
        if t >= 1.0 {
            cmds.entity(entity).remove::<TreeGrowth>();
        }
    }
}

// ── Sway system ──────────────────────────────────────────────────────────────

/// Applies a subtle sin-wave rotation oscillation to all tree entities tagged
/// with `TreeSway`.  Uses world position as a phase offset so adjacent trees
/// don't all sway in lockstep.
fn sway_trees(
    time: Res<Time>,
    mut q: Query<(&mut Transform, &GlobalTransform), With<TreeSway>>,
) {
    let t = time.elapsed_secs();
    for (mut transform, global) in &mut q {
        let pos = global.translation();
        let phase = (pos.x + pos.z) * 0.1;
        let sway = (t * 0.8 + phase).sin() * 0.02; // ~1 degree
        transform.rotation = Quat::from_rotation_z(sway);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `(variants, density_out_of_256, scale, grows)` for the biome,
/// or `None` for water / river / void (no decorations).
/// `grows` is true for trees/cacti/palms that participate in the growth system.
fn decoration_for_biome(
    kind: TileKind,
    deco: &DecorationAssets,
) -> Option<(&Vec<DecorationVariant>, u32, f32, bool)> {
    match kind {
        TileKind::Forest | TileKind::TemperateForest =>
            Some((&deco.broadleaf, 64, 1.0, true)),       // ~25%
        TileKind::TropicalForest | TileKind::TropicalRainforest =>
            Some((&deco.tropical, 77, 1.2, true)),         // ~30%
        TileKind::Taiga =>
            Some((&deco.conifer,  64, 1.0, true)),         // ~25%
        TileKind::Mountain | TileKind::Stone =>
            Some((&deco.rocks,    38, 0.8, false)),        // ~15%
        TileKind::Tundra | TileKind::Arctic | TileKind::PolarDesert =>
            Some((&deco.rocks,    20, 0.6, false)),        // ~8%
        TileKind::Desert =>
            Some((&deco.desert,   20, 1.0, true)),         // ~8%
        TileKind::Savanna =>
            Some((&deco.bushes,   20, 0.7, false)),        // ~8%
        TileKind::Plains | TileKind::Grassland =>
            Some((&deco.bushes,   13, 0.6, false)),        // ~5%
        TileKind::Water | TileKind::River | TileKind::Void
        | TileKind::CaveFloor | TileKind::CaveWall | TileKind::CrystalCave
        | TileKind::LavaFloor | TileKind::CaveRiver | TileKind::CavePortal => None,
    }
}

/// Single-octave bilinearly-interpolated value noise.
///
/// Returns `[0, 1]`.  Adjacent tiles within `patch_tiles` are correlated.
/// The corner lattice is hashed with the given `seed`.
fn value_noise(seed: u64, gx: usize, gy: usize, patch_tiles: f32) -> f32 {
    let fx = gx as f32 / patch_tiles;
    let fy = gy as f32 / patch_tiles;
    let ix = fx.floor() as u32;
    let iy = fy.floor() as u32;
    let tx = fx - ix as f32;
    let ty = fy - iy as f32;

    // Smoothstep — softens patch edges for gradual density transitions.
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);

    let corner = |px: u32, py: u32| -> f32 {
        let h = tile_hash(seed, px as usize, py as usize);
        (h >> 32) as f32 / u32::MAX as f32
    };

    let v00 = corner(ix,     iy);
    let v10 = corner(ix + 1, iy);
    let v01 = corner(ix,     iy + 1);
    let v11 = corner(ix + 1, iy + 1);

    let v0 = v00 + sx * (v10 - v00);
    let v1 = v01 + sx * (v11 - v01);
    v0 + sy * (v1 - v0)
}

/// Two-octave noise producing cluster cores with dense centres and sparse edges.
///
/// Coarse octave (11 tiles) — determines whether this area is a cluster at all.
/// Fine octave (4 tiles) — adds internal variation within the cluster.
/// Tiles near a cluster core score low on both → always decorated.
/// Tiles near a cluster edge score low on coarse but variable on fine → thinned out.
fn patch_noise(seed: u64, gx: usize, gy: usize) -> f32 {
    let coarse = value_noise(seed ^ 0xA5A5_A5A5_A5A5_A5A5, gx, gy, 11.0);
    let fine   = value_noise(seed ^ 0x1234_5678_ABCD_EF01, gx, gy,  4.0);
    coarse * 0.65 + fine * 0.35
}

/// Deterministic tile hash seeded by world seed + tile position.
fn tile_hash(seed: u64, gx: usize, gy: usize) -> u64 {
    let v = seed
        .wrapping_add((gx as u64).wrapping_mul(2654435761))
        .wrapping_add((gy as u64).wrapping_mul(805459861));
    let v = v ^ (v >> 33);
    let v = v.wrapping_mul(0xff51afd7ed558ccd);
    let v = v ^ (v >> 33);
    let v = v.wrapping_mul(0xc4ceb9fe1a85ec53);
    v ^ (v >> 33)
}
