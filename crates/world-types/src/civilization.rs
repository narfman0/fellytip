//! Civilizations: settlements, territory, road networks, and building layouts.
//!
//! All functions are pure (no ECS) and deterministic given a seed.
//! Call order expected by the server:
//! ```text
//! let map        = generate_map(seed);
//! let civs       = generate_settlements_full(&mut map, seed);
//! assign_territories(&map, &civs);
//! generate_roads(&mut map, &civs);
//! let buildings  = generate_buildings(&civs, &map, seed);
//! apply_building_tiles(&buildings, &mut map);   // marks tiles non-walkable
//! map.spawn_points = generate_spawn_points(&map); // must run AFTER apply_building_tiles
//! ```

use std::collections::VecDeque;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use uuid::Uuid;

use crate::cave::{cave_z, find_cave_capital_site, generate_cave_layer, place_cave_portals};
use crate::faction::faction_archetype;
use crate::map::{TileKind, WorldMap};

// ── Types ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, bevy::prelude::Component, bevy::prelude::Reflect)]
#[reflect(Component)]
pub enum SettlementKind {
    Capital,
    Town,
    /// Peaceful underground sanctuary — sparse zen towers, depth-2 cave.
    /// Populated by the "sanctuary" faction (Japanese-inspired hidden dwellers).
    PeacefulSanctuary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settlement {
    pub id: Uuid,
    pub name: SmolStr,
    pub kind: SettlementKind,
    /// Tile-space center X (ix + 0.5, range 0..MAP_WIDTH).  Subtract
    /// `MAP_HALF_WIDTH` to convert to world-space.
    pub x: f32,
    /// Tile-space center Y (iy + 0.5, range 0..MAP_HEIGHT).  Subtract
    /// `MAP_HALF_HEIGHT` to convert to world-space.
    pub y: f32,
    /// World-space Z elevation at this location.
    pub z: f32,
}

/// Bevy resource holding all generated settlements.
#[derive(Resource, Default, Clone, Debug, Serialize, Deserialize)]
pub struct Settlements(pub Vec<Settlement>);

/// Marker component placed on a settlement entity when its population reaches 0
/// (issue #111). The entity remains in the world so it can be rebuilt.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component)]
pub struct AbandonedSettlement {
    /// Tick at which the settlement collapsed.
    pub collapsed_at_tick: u64,
    /// Faction that owned the settlement when it collapsed.
    /// Stored as `String` (not `SmolStr`) so Reflect can be derived on this component.
    pub original_faction_id: String,
}

/// Tile marker indicating the tile was once a settlement site (issue #109).
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component)]
pub struct RuinsTile;

// ── Building types ─────────────────────────────────────────────────────────────

/// Visual/semantic category for a procedurally-placed settlement building.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub enum BuildingKind {
    TentDetailed,   // nature/tent_detailedClosed.glb  — town camps
    TentSmall,      // nature/tent_smallClosed.glb      — town camps
    CampfireStones, // nature/campfire_stones.glb       — town center
    Windmill,       // town/windmill.glb                — capital landmark
    Stall,          // town/stall.glb                   — capital market (plain)
    StallBench,     // town/stall-bench.glb             — capital market (with bench)
    StallGreen,     // town/stall-green.glb             — capital market
    StallRed,       // town/stall-red.glb               — capital market
    Fountain,       // town/fountain-round.glb          — capital center
    Lantern,        // town/lantern.glb                 — capital street lighting
    Tavern,         // 2-floor interior zone (taproom + rooms)
    Barracks,       // 2-floor interior zone (hall + bunks)
    Tower,          // 4-floor interior zone (3 interior + battlements)
    Keep,           // 3-floor interior zone + 10×10 battlements floor
    CapitalTower,   // 5-floor interior zone (20×20 circular, capital landmark)
}

impl BuildingKind {
    /// Approximate axis-aligned half-extents of this kind's exterior at the
    /// renderer's standard `Vec3::splat(2.0)` GLB scale. Returns `(hx, hy, hz)`
    /// where `hy` is the half-height (so a Tower of total height 10 returns
    /// `hy = 5.0`).
    ///
    /// Used by `PhysicsWorldPlugin` to spawn cuboid colliders without needing
    /// to load the GLB scene (so colliders exist in headless mode too).
    /// Numbers are intentionally rough — they're collision proxies, not visual
    /// truth. Refine when a kind needs tighter collision.
    pub fn approx_half_extents(self) -> (f32, f32, f32) {
        match self {
            BuildingKind::TentDetailed   => (1.25, 1.0,  1.25),
            BuildingKind::TentSmall      => (1.0,  0.75, 1.0),
            BuildingKind::CampfireStones => (0.75, 0.25, 0.75),
            BuildingKind::Windmill       => (1.5,  3.0,  1.5),
            BuildingKind::Stall
            | BuildingKind::StallBench
            | BuildingKind::StallGreen
            | BuildingKind::StallRed     => (1.0,  1.25, 1.0),
            BuildingKind::Fountain       => (1.0,  0.5,  1.0),
            BuildingKind::Lantern        => (0.25, 1.5,  0.25),
            BuildingKind::Tavern
            | BuildingKind::Barracks     => (2.0,  2.5,  2.0),
            BuildingKind::Tower          => (1.5,  5.0,  1.5),
            BuildingKind::Keep           => (2.5,  3.75, 2.5),
            BuildingKind::CapitalTower   => (2.5,  6.25, 2.5),
        }
    }
}

/// A single procedurally-placed building belonging to a settlement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Building {
    pub id: Uuid,
    pub settlement_id: Uuid,
    pub kind: BuildingKind,
    /// Tile-space X (same convention as `Settlement.x`: range 0..MAP_WIDTH).
    pub tx: u32,
    /// Tile-space Y.
    pub ty: u32,
    /// World-space Z elevation (tile surface height at this position).
    pub z: f32,
    /// Rotation in 90-degree increments (0–3 maps to 0°, 90°, 180°, 270°).
    pub rotation: u8,
    /// Owning faction id string (e.g. "iron_wolves"). `None` for unaffiliated buildings.
    /// Populated deterministically by `generate_buildings` so clients can apply
    /// per-faction art direction without server queries.
    #[serde(default)]
    pub faction_id: Option<String>,
    /// Visual variant index (0–9) used by the renderer to pick among Synty preset
    /// house models. Derived deterministically from the building UUID.
    #[serde(default)]
    pub style_variant: u8,
}

/// Bevy resource holding all procedurally generated buildings.
#[derive(Resource, Default, Clone, Debug, Serialize, Deserialize)]
pub struct Buildings(pub Vec<Building>);

/// A procedurally generated civilization grouping one or more settlements.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Civilization {
    pub name: SmolStr,
    pub settlements: Vec<Settlement>,
}

/// Territory map: one optional settlement-index per tile column (flat row-major).
///
/// `territory[ix + iy * MAP_WIDTH] = Some(settlement_idx)` means the tile
/// is "claimed" by that settlement.
pub type TerritoryMap = Vec<Option<usize>>;

// ── Habitability ───────────────────────────────────────────────────────────────

/// Surface habitability score in `[0.0, 1.0]`.
///
/// `0.0` — uninhabitable (water, river, mountain).
pub fn habitability(kind: TileKind) -> f32 {
    match kind {
        TileKind::Grassland | TileKind::Plains           => 1.0,
        TileKind::TemperateForest | TileKind::Forest     => 0.8,
        TileKind::Savanna
        | TileKind::TropicalForest
        | TileKind::TropicalRainforest                   => 0.7,
        TileKind::Taiga                                  => 0.5,
        TileKind::Desert                                 => 0.3,
        TileKind::Tundra                                 => 0.2,
        TileKind::PolarDesert | TileKind::Arctic         => 0.1,
        _                                                => 0.0,
    }
}

// ── Settlement generation ─────────────────────────────────────────────────────

/// Minimum tile distance between any two settlements.
const MIN_SETTLEMENT_DIST: f32 = 60.0;
/// Grid-cell size for Poisson-disk approximation (one candidate per cell).
const GRID_CELL: usize = 64;
/// Fraction of cells that become Capitals (~1 in 8).
const CAPITAL_PROB: f32 = 0.12;

/// Generate all surface settlements deterministically from `seed`.
///
/// Uses a Poisson-disk grid approximation: divides the map into
/// `GRID_CELL×GRID_CELL` cells, picks the most habitable walkable tile in each,
/// rejects candidates too close to existing settlements.
pub fn generate_settlements(map: &WorldMap, seed: u64) -> Vec<Settlement> {
    surface_settlements(map, seed)
}

/// Generate surface and underground settlements, stamping cave layers into `map`.
///
/// Extends the surface settlement list with exactly one underground capital (depth 1)
/// and one depth-2 peaceful sanctuary.
/// The map is mutated to add cave layers required for cave building placement.
pub fn generate_settlements_full(map: &mut WorldMap, seed: u64) -> Vec<Settlement> {
    let mut settlements = surface_settlements(map, seed);
    if let Some(civ) = generate_underground_civilization(map, seed) {
        settlements.extend(civ.settlements);
    }
    if let Some(civ) = generate_sanctuary_civilization(map, seed) {
        settlements.extend(civ.settlements);
    }
    settlements
}

fn surface_settlements(map: &WorldMap, seed: u64) -> Vec<Settlement> {
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut placed: Vec<Settlement> = Vec::new();
    let mut idx = 0usize;

    let cells_x = map.width  / GRID_CELL;
    let cells_y = map.height / GRID_CELL;

    for cy in 0..cells_y {
        for cx in 0..cells_x {
            // Find highest-habitability walkable tile in this cell.
            let mut best_score = 0.0f32;
            let mut best_pos   = None;
            let mut best_z     = 0.0f32;

            // Collect all habitable tiles in this cell, pick one randomly to break grid.
            let mut candidates: Vec<(usize, usize, f32, f32)> = Vec::new();
            for dy in 0..GRID_CELL {
                for dx in 0..GRID_CELL {
                    let ix = cx * GRID_CELL + dx;
                    let iy = cy * GRID_CELL + dy;
                    let col = map.column(ix, iy);
                    if let Some(layer) = col.layers.iter().find(|l| l.is_surface_kind() && l.walkable) {
                        let score = habitability(layer.kind);
                        if score >= 0.6 {
                            candidates.push((ix, iy, score, layer.z_top));
                        } else if score > best_score {
                            best_score = score;
                            best_pos   = Some((ix, iy));
                            best_z     = layer.z_top;
                        }
                    }
                }
            }
            if !candidates.is_empty() {
                let pick = rng.random_range(0..candidates.len());
                let (ix, iy, score, z) = candidates[pick];
                best_score = score;
                best_pos   = Some((ix, iy));
                best_z     = z;
            }

            if best_score < 0.3 {
                continue;
            }
            let (ix, iy) = match best_pos { Some(p) => p, None => continue };

            // Poisson-disk rejection.
            let fx = ix as f32 + 0.5;
            let fy = iy as f32 + 0.5;
            let too_close = placed.iter().any(|s: &Settlement| {
                let ddx = s.x - fx;
                let ddy = s.y - fy;
                (ddx * ddx + ddy * ddy).sqrt() < MIN_SETTLEMENT_DIST
            });
            if too_close {
                continue;
            }

            let kind = if idx == 0 || rng.random::<f32>() < CAPITAL_PROB {
                SettlementKind::Capital
            } else {
                SettlementKind::Town
            };

            let id = deterministic_uuid(&mut rng);
            placed.push(Settlement {
                id,
                name: SmolStr::new(format!("Settlement_{idx}")),
                kind,
                x: fx,
                y: fy,
                z: best_z,
            });
            idx += 1;
        }
    }

    placed
}

/// Generate one underground civilization with a capital settlement at depth 1.
pub fn generate_underground_civilization(map: &mut WorldMap, seed: u64) -> Option<Civilization> {
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    const DEPTH: u32 = 1;
    generate_cave_layer(map, seed, DEPTH);
    place_cave_portals(map, seed, DEPTH, 3);

    let (ix, iy) = find_cave_capital_site(map, seed, DEPTH)?;

    let z = cave_z(DEPTH);
    let mut rng = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0xCAFE_BABE_DEAD_BEEFu64));
    let id = deterministic_uuid(&mut rng);

    let name_idx = seed % 4;
    let civ_name = match name_idx {
        0 => SmolStr::new("Underdark Realm"),
        1 => SmolStr::new("The Deep Dominion"),
        2 => SmolStr::new("Subterran Empire"),
        _ => SmolStr::new("Cavern Sovereignty"),
    };

    let capital = Settlement {
        id,
        name: SmolStr::new("Underground Capital"),
        kind: SettlementKind::Capital,
        x: ix as f32 + 0.5,
        y: iy as f32 + 0.5,
        z,
    };

    Some(Civilization {
        name: civ_name,
        settlements: vec![capital],
    })
}

/// Generate one depth-2 sanctuary civilization — the hidden, peaceful underground dwellers.
///
/// Uses cave depth 2 (z = -30). The single settlement is a `PeacefulSanctuary` and
/// belongs to the "sanctuary" faction.
pub fn generate_sanctuary_civilization(map: &mut WorldMap, seed: u64) -> Option<Civilization> {
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    const DEPTH: u32 = 2;
    generate_cave_layer(map, seed, DEPTH);
    place_cave_portals(map, seed, DEPTH, 1);

    let (ix, iy) = find_cave_capital_site(map, seed.wrapping_add(0x5A0C_7A0D_1234_5678), DEPTH)?;

    let z = cave_z(DEPTH);
    let mut rng = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0xBEAD_CAFE_1234_ABCD));
    let id = deterministic_uuid(&mut rng);

    let sanctuary = Settlement {
        id,
        name: SmolStr::new("Hidden Sanctuary"),
        kind: SettlementKind::PeacefulSanctuary,
        x: ix as f32 + 0.5,
        y: iy as f32 + 0.5,
        z,
    };

    Some(Civilization {
        name: SmolStr::new("The Reclusive Ones"),
        settlements: vec![sanctuary],
    })
}

// ── Territory assignment ───────────────────────────────────────────────────────

/// Assign each surface tile to the nearest settlement via BFS flood-fill.
///
/// Only walkable surface tiles are assigned.  The returned [`TerritoryMap`] has
/// the same flat row-major layout as `WorldMap::columns`.
pub fn assign_territories(map: &WorldMap, settlements: &[Settlement]) -> TerritoryMap {
    let n = map.width * map.height;
    let mut territory: TerritoryMap = vec![None; n];
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new(); // (tile_idx, settlement_idx)

    // Seed BFS from each settlement's tile.
    for (si, s) in settlements.iter().enumerate() {
        let ix = (s.x as usize).min(map.width  - 1);
        let iy = (s.y as usize).min(map.height - 1);
        let idx = ix + iy * map.width;
        if territory[idx].is_none() {
            territory[idx] = Some(si);
            queue.push_back((idx, si));
        }
    }

    while let Some((idx, si)) = queue.pop_front() {
        let ix = idx % map.width;
        let iy = idx / map.width;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx == 0 && dy == 0 { continue; }
                let nx = ix as i32 + dx;
                let ny = iy as i32 + dy;
                if nx < 0 || ny < 0
                    || nx as usize >= map.width
                    || ny as usize >= map.height
                { continue; }
                let ni = nx as usize + ny as usize * map.width;
                if territory[ni].is_some() { continue; }
                // Only cross walkable surface tiles.
                let col = &map.columns[ni];
                if col.layers.iter().any(|l| l.is_surface_kind() && l.walkable) {
                    territory[ni] = Some(si);
                    queue.push_back((ni, si));
                }
            }
        }
    }

    territory
}

// ── Road network ───────────────────────────────────────────────────────────────

/// Connect all settlements with a minimum spanning tree road network.
///
/// # Algorithm
/// 1. Build a complete graph of Euclidean distances between settlements.
/// 2. Kruskal's MST on that graph.
/// 3. For each MST edge, draw a road by straight-line Bresenham walk and mark
///    `map.road_tiles[ix + iy * MAP_WIDTH] = true`.
pub fn generate_roads(map: &mut WorldMap, settlements: &[Settlement]) {
    let surface: Vec<&Settlement> = settlements.iter().collect();

    if surface.len() < 2 {
        return;
    }

    // ── Kruskal's MST ─────────────────────────────────────────────────────────
    // Build sorted edge list (Euclidean distance).
    let mut edges: Vec<(f32, usize, usize)> = Vec::new();
    for i in 0..surface.len() {
        for j in (i + 1)..surface.len() {
            let dx = surface[i].x - surface[j].x;
            let dy = surface[i].y - surface[j].y;
            edges.push(((dx * dx + dy * dy).sqrt(), i, j));
        }
    }
    edges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Union-Find.
    let mut parent: Vec<usize> = (0..surface.len()).collect();
    fn find(p: &mut Vec<usize>, x: usize) -> usize {
        if p[x] != x { p[x] = find(p, p[x]); }
        p[x]
    }

    let mut mst_edges: Vec<(usize, usize)> = Vec::new();
    for (_, u, v) in &edges {
        let pu = find(&mut parent, *u);
        let pv = find(&mut parent, *v);
        if pu != pv {
            parent[pu] = pv;
            mst_edges.push((*u, *v));
            if mst_edges.len() == surface.len() - 1 {
                break;
            }
        }
    }

    // ── Bresenham road drawing ─────────────────────────────────────────────────
    for (u, v) in mst_edges {
        let ax = surface[u].x as i32;
        let ay = surface[u].y as i32;
        let bx = surface[v].x as i32;
        let by = surface[v].y as i32;
        for (rx, ry) in bresenham(ax, ay, bx, by) {
            if rx >= 0 && ry >= 0
                && (rx as usize) < map.width
                && (ry as usize) < map.height
            {
                map.road_tiles[rx as usize + ry as usize * map.width] = true;
            }
        }
    }
}

// ── Building generation ────────────────────────────────────────────────────────

/// RNG seed offset so building placement doesn't produce the same stream as
/// settlement placement (which also uses the raw `seed`).
const BUILDING_SEED_OFFSET: u64 = 0xB411_D1A0;

/// Spiral of 8 (dx, dy) offsets around the tower center, going clockwise.
const STAIR_SPIRAL: [(i32, i32); 8] = [
    (0, 1), (1, 1), (1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1),
];

/// Z height increment per spiral step — smaller than STEP_HEIGHT=2.0 so the
/// movement system can always reach the next step.
const STAIR_STEP: f32 = 1.4;

/// Height of each tower floor in world units.
const TOWER_FLOOR_H: f32 = 3.0;

fn tower_floor_count(kind: BuildingKind) -> u32 {
    match kind {
        BuildingKind::CapitalTower => 7,
        BuildingKind::Tower        => 4,
        BuildingKind::Keep         => 3,
        BuildingKind::Tavern | BuildingKind::Barracks => 2,
        _ => 0,
    }
}

fn apply_tower_stair_tiles(b: &Building, map: &mut WorldMap) {
    let floors = tower_floor_count(b.kind);
    if floors == 0 { return; }

    let cx = b.tx as i32;
    let cy = b.ty as i32;
    let base_z = b.z;
    let total_height = floors as f32 * TOWER_FLOOR_H;

    // Center tile: a flat floor layer at each storey.
    for k in 0..=floors {
        let z = base_z + k as f32 * TOWER_FLOOR_H;
        map.add_stair_layer(cx as usize, cy as usize, z, crate::map::TileKind::Stone);
    }

    // Stair spiral: rising by STAIR_STEP per tile.
    let total_steps = ((total_height / STAIR_STEP).ceil() as u32 + 8).min(256);
    for step in 0..total_steps {
        let z = base_z + step as f32 * STAIR_STEP;
        if z > base_z + total_height + TOWER_FLOOR_H { break; }
        let (dx, dy) = STAIR_SPIRAL[(step as usize) % 8];
        let tx = cx + dx;
        let ty = cy + dy;
        if tx >= 0 && ty >= 0
            && (tx as usize) < map.width
            && (ty as usize) < map.height
        {
            map.add_stair_layer(tx as usize, ty as usize, z, crate::map::TileKind::Stone);
        }
    }
}

/// Sanctuary buildings: sparse zen towers for the hidden depth-2 dwellers.
const SANCTUARY_POOL: &[BuildingKind] = &[
    BuildingKind::Tower,
    BuildingKind::Keep,
    BuildingKind::Tower,
];

/// Town buildings: camp-style structures around a central campfire.
const TOWN_CENTER: BuildingKind = BuildingKind::CampfireStones;
const TOWN_POOL: &[BuildingKind] = &[
    BuildingKind::TentDetailed,
    BuildingKind::TentSmall,
    BuildingKind::TentDetailed,  // weight detailed tents slightly higher
];
/// Capital buildings: civic and market structures.  Fountain is always placed
/// at the center; lanterns fill outer rings as street lighting.
const CAPITAL_CENTER: BuildingKind = BuildingKind::Fountain;
const CAPITAL_POOL: &[BuildingKind] = &[
    BuildingKind::Windmill,
    BuildingKind::StallGreen,
    BuildingKind::StallRed,
    BuildingKind::Stall,
    BuildingKind::StallBench,
    BuildingKind::Lantern,
    BuildingKind::TentSmall,
];
/// Underground cyberpunk capital buildings: data spires, corporate blocks, security posts.
/// Used for underground Capital settlements in the Sunken Realm (world_id 1).
const CYBERPUNK_POOL: &[BuildingKind] = &[
    BuildingKind::Tower,   // tall data spires
    BuildingKind::Keep,    // corporate blocks
    BuildingKind::Barracks, // security posts
    BuildingKind::Tower,   // weighted double
    BuildingKind::Keep,    // weighted double
];

/// Returns `true` for building kinds that generate multi-floor interior zones
/// and stair-tile spirals. These must be excluded from ring building pools to
/// prevent stair layers being stamped on top of adjacent GLB building tiles.
fn is_tower_kind(kind: BuildingKind) -> bool {
    matches!(
        kind,
        BuildingKind::CapitalTower
            | BuildingKind::Tower
            | BuildingKind::Keep
            | BuildingKind::Tavern
            | BuildingKind::Barracks
    )
}

/// Well-known faction ids in cycling order used for deterministic assignment.
const FACTION_CYCLE: &[&str] = &["iron_wolves", "merchant_guild", "ash_covenant", "deep_tide"];

/// Deterministically assign a faction to a settlement by index.
/// Underground settlements (z < 0) always belong to "deep_tide".
pub fn faction_for_settlement(settlement_index: usize, z: f32) -> &'static str {
    if z < 0.0 {
        "deep_tide"
    } else {
        FACTION_CYCLE[settlement_index % FACTION_CYCLE.len()]
    }
}

/// Ring radii (in tiles) used when placing Town buildings.
const TOWN_RADII: &[u32] = &[2, 3, 4];
/// Ring radii (in tiles) used when placing Capital buildings (excluding the
/// center fountain which is placed at the settlement tile itself).
const CAPITAL_RADII: &[u32] = &[2, 3, 4, 5, 6];

/// Pure, deterministic building layout for all settlements.
///
/// Does **not** mutate the map; call [`apply_building_tiles`] afterwards to
/// mark occupied tiles as impassable.
pub fn generate_buildings(settlements: &[Settlement], map: &WorldMap, seed: u64) -> Vec<Building> {
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;
    use std::collections::HashSet;
    use std::f32::consts::TAU;

    let mut rng = ChaCha8Rng::seed_from_u64(seed.wrapping_add(BUILDING_SEED_OFFSET));
    let mut all: Vec<Building> = Vec::new();

    for (settlement_idx, settlement) in settlements.iter().enumerate() {
        let cx = settlement.x as u32;
        let cy = settlement.y as u32;
        let is_underground = settlement.z < 0.0;

        // Deterministic faction assignment (issue #136).
        let faction_id_str = faction_for_settlement(settlement_idx, settlement.z);
        let archetype = faction_archetype(faction_id_str);

        // Track occupied tiles across this settlement to prevent overlap.
        let mut occupied: HashSet<(u32, u32)> = HashSet::new();
        // Reserve the settlement center tile.
        occupied.insert((cx, cy));

        let buildable = |map: &WorldMap, tx: u32, ty: u32| {
            if is_underground {
                is_cave_buildable(map, tx, ty)
            } else {
                is_tile_buildable(map, tx, ty)
            }
        };
        let make = |rng: &mut _, sid, kind, tx, ty, map: &WorldMap| {
            let mut b = if is_underground {
                make_cave_building(rng, sid, kind, tx, ty, map)
            } else {
                make_building(rng, sid, kind, tx, ty, map)
            };
            b.faction_id = Some(faction_id_str.to_string());
            b
        };

        match settlement.kind {
            SettlementKind::Capital => {
                // Always place a fountain at the exact center.
                if buildable(map, cx, cy) {
                    all.push(make(&mut rng, settlement.id, CAPITAL_CENTER, cx, cy, map));
                }

                // Place a CapitalTower offset north-west from the settlement center
                // (8 tiles north + 8 tiles west) to avoid overlapping the fountain and
                // market stalls placed in the inner rings.
                let tower_tx = cx.saturating_sub(8);
                let tower_ty = cy.saturating_sub(8);
                if !occupied.contains(&(tower_tx, tower_ty)) && buildable(map, tower_tx, tower_ty) {
                    occupied.insert((tower_tx, tower_ty));
                    all.push(make(
                        &mut rng,
                        settlement.id,
                        BuildingKind::CapitalTower,
                        tower_tx,
                        tower_ty,
                        map,
                    ));
                }

                // Underground Capitals owned by deep_tide (or any underground z < -1)
                // use the cyberpunk building pool (data spires, security posts).
                // Otherwise, use faction-specific pool when available; fall back to CAPITAL_POOL.
                // Tower-type buildings (Tower, Keep, Tavern, Barracks, CapitalTower) are
                // excluded from ring placement — they are only placed as the centre
                // landmark above. Ring buildings must be GLB-backed to avoid stair-layer
                // collisions with adjacent non-tower buildings.
                let raw_pool = if is_underground && (faction_id_str == "deep_tide" || settlement.z < -1.0) {
                    CYBERPUNK_POOL
                } else if !archetype.building_pool.is_empty() {
                    archetype.building_pool
                } else {
                    CAPITAL_POOL
                };
                let filtered: Vec<BuildingKind> = raw_pool.iter().copied()
                    .filter(|k| !is_tower_kind(*k))
                    .collect();
                let pool: &[BuildingKind] = if filtered.is_empty() { CAPITAL_POOL } else { &filtered };
                let count = 7 + (rng.random::<u32>() % 6) as usize; // 7–12
                place_ring_buildings_ex_with_faction(
                    &mut rng,
                    &mut all,
                    &mut occupied,
                    settlement,
                    map,
                    pool,
                    CAPITAL_RADII,
                    count,
                    is_underground,
                    faction_id_str,
                );
            }

            SettlementKind::Town => {
                // Always place a campfire at the center.
                if buildable(map, cx, cy) {
                    all.push(make(&mut rng, settlement.id, TOWN_CENTER, cx, cy, map));
                }

                // Use faction-specific pool when available; fall back to TOWN_POOL.
                // Tower-type buildings excluded from ring placement (see Capital branch).
                let raw_pool = if !archetype.building_pool.is_empty() {
                    archetype.building_pool
                } else {
                    TOWN_POOL
                };
                let filtered: Vec<BuildingKind> = raw_pool.iter().copied()
                    .filter(|k| !is_tower_kind(*k))
                    .collect();
                let pool: &[BuildingKind] = if filtered.is_empty() { TOWN_POOL } else { &filtered };
                let count = 3 + (rng.random::<u32>() % 3) as usize; // 3–5
                place_ring_buildings_ex_with_faction(
                    &mut rng,
                    &mut all,
                    &mut occupied,
                    settlement,
                    map,
                    pool,
                    TOWN_RADII,
                    count,
                    is_underground,
                    faction_id_str,
                );
            }

            SettlementKind::PeacefulSanctuary => {
                // Sanctuary: sparse towers only, low count (2–3), no center landmark.
                let count = 2 + (rng.random::<u32>() % 2) as usize; // 2–3
                place_ring_buildings_ex_with_faction(
                    &mut rng,
                    &mut all,
                    &mut occupied,
                    settlement,
                    map,
                    SANCTUARY_POOL,
                    TOWN_RADII,
                    count,
                    is_underground,
                    faction_id_str,
                );
            }
        }

        // Suppress unused import warning from TAU if loop body doesn't use it.
        let _ = TAU;
    }

    all
}

/// Place up to `count` buildings in concentric rings around the settlement center.
#[allow(clippy::too_many_arguments, dead_code)]
fn place_ring_buildings(
    rng: &mut impl rand::RngExt,
    output: &mut Vec<Building>,
    occupied: &mut std::collections::HashSet<(u32, u32)>,
    settlement: &Settlement,
    map: &WorldMap,
    pool: &[BuildingKind],
    radii: &[u32],
    count: usize,
) {
    place_ring_buildings_ex(rng, output, occupied, settlement, map, pool, radii, count, false);
}

/// Extended ring-building placement supporting both surface and underground settlements.
#[allow(clippy::too_many_arguments)]
fn place_ring_buildings_ex(
    rng: &mut impl rand::RngExt,
    output: &mut Vec<Building>,
    occupied: &mut std::collections::HashSet<(u32, u32)>,
    settlement: &Settlement,
    map: &WorldMap,
    pool: &[BuildingKind],
    radii: &[u32],
    count: usize,
    is_underground: bool,
) {
    use std::f32::consts::TAU;

    let cx = settlement.x as u32;
    let cy = settlement.y as u32;

    // Generate all candidate positions in the rings, then shuffle and pick.
    let mut candidates: Vec<(u32, u32)> = Vec::new();
    for &r in radii {
        let steps = (r * 8).max(8) as usize; // ~8 positions per unit radius
        for step in 0..steps {
            let angle = TAU * step as f32 / steps as f32;
            let tx = cx as i32 + (r as f32 * angle.cos()).round() as i32;
            let ty = cy as i32 + (r as f32 * angle.sin()).round() as i32;
            if tx >= 0 && ty >= 0 {
                candidates.push((tx as u32, ty as u32));
            }
        }
    }

    // Fisher-Yates shuffle.
    for i in (1..candidates.len()).rev() {
        let j = (rng.random::<u32>() as usize) % (i + 1);
        candidates.swap(i, j);
    }

    let mut placed = 0usize;
    let mut pool_idx = 0usize;
    for (tx, ty) in candidates {
        if placed >= count { break; }
        if occupied.contains(&(tx, ty)) { continue; }
        let ok = if is_underground {
            is_cave_buildable(map, tx, ty)
        } else {
            is_tile_buildable(map, tx, ty)
        };
        if !ok { continue; }
        occupied.insert((tx, ty));
        let kind = pool[pool_idx % pool.len()];
        pool_idx += 1;
        let b = if is_underground {
            make_cave_building(rng, settlement.id, kind, tx, ty, map)
        } else {
            make_building(rng, settlement.id, kind, tx, ty, map)
        };
        output.push(b);
        placed += 1;
    }
}

/// Extended ring-building placement supporting both surface and underground settlements,
/// with explicit faction id stamped on each generated building (issue #136).
#[allow(clippy::too_many_arguments)]
fn place_ring_buildings_ex_with_faction(
    rng: &mut impl rand::RngExt,
    output: &mut Vec<Building>,
    occupied: &mut std::collections::HashSet<(u32, u32)>,
    settlement: &Settlement,
    map: &WorldMap,
    pool: &[BuildingKind],
    radii: &[u32],
    count: usize,
    is_underground: bool,
    faction_id_str: &str,
) {
    use std::f32::consts::TAU;

    let cx = settlement.x as u32;
    let cy = settlement.y as u32;

    let mut candidates: Vec<(u32, u32)> = Vec::new();
    for &r in radii {
        let steps = (r * 8).max(8) as usize;
        for step in 0..steps {
            let angle = TAU * step as f32 / steps as f32;
            let tx = cx as i32 + (r as f32 * angle.cos()).round() as i32;
            let ty = cy as i32 + (r as f32 * angle.sin()).round() as i32;
            if tx >= 0 && ty >= 0 {
                candidates.push((tx as u32, ty as u32));
            }
        }
    }

    for i in (1..candidates.len()).rev() {
        let j = (rng.random::<u32>() as usize) % (i + 1);
        candidates.swap(i, j);
    }

    let mut placed = 0usize;
    let mut pool_idx = 0usize;
    for (tx, ty) in candidates {
        if placed >= count { break; }
        if occupied.contains(&(tx, ty)) { continue; }
        let ok = if is_underground {
            is_cave_buildable(map, tx, ty)
        } else {
            is_tile_buildable(map, tx, ty)
        };
        if !ok { continue; }
        occupied.insert((tx, ty));
        let kind = pool[pool_idx % pool.len()];
        pool_idx += 1;
        let mut b = if is_underground {
            make_cave_building(rng, settlement.id, kind, tx, ty, map)
        } else {
            make_building(rng, settlement.id, kind, tx, ty, map)
        };
        b.faction_id = Some(faction_id_str.to_string());
        output.push(b);
        placed += 1;
    }
}

/// Returns `true` if tile `(tx, ty)` exists and has a walkable surface layer.
fn is_tile_buildable(map: &WorldMap, tx: u32, ty: u32) -> bool {
    if tx as usize >= map.width || ty as usize >= map.height { return false; }
    let col = map.column(tx as usize, ty as usize);
    col.layers.iter().any(|l| l.is_surface_kind() && l.walkable)
}

/// Returns `true` if tile `(tx, ty)` has a walkable CaveFloor (or CrystalCave) layer.
/// LavaFloor and CaveRiver are excluded — buildings cannot be placed on them.
fn is_cave_buildable(map: &WorldMap, tx: u32, ty: u32) -> bool {
    if tx as usize >= map.width || ty as usize >= map.height { return false; }
    let col = map.column(tx as usize, ty as usize);
    col.layers.iter().any(|l| {
        l.walkable && matches!(l.kind, TileKind::CaveFloor | TileKind::CrystalCave)
    })
}

/// Construct a [`Building`] at the given tile position, sampling the surface Z.
fn make_building(
    rng: &mut impl rand::RngExt,
    settlement_id: Uuid,
    kind: BuildingKind,
    tx: u32,
    ty: u32,
    map: &WorldMap,
) -> Building {
    let z = map
        .column(tx as usize, ty as usize)
        .layers
        .iter()
        .filter(|l| l.is_surface_kind() && l.walkable)
        .map(|l| l.z_top)
        .fold(f32::NEG_INFINITY, f32::max);
    let id = deterministic_uuid(rng);
    let style_variant = (id.as_u128() % 10) as u8;
    Building {
        id,
        settlement_id,
        kind,
        tx,
        ty,
        z,
        rotation: (rng.random::<u8>() % 4),
        faction_id: None,
        style_variant,
    }
}

/// Construct a [`Building`] at the given tile position, sampling the cave layer Z.
fn make_cave_building(
    rng: &mut impl rand::RngExt,
    settlement_id: Uuid,
    kind: BuildingKind,
    tx: u32,
    ty: u32,
    map: &WorldMap,
) -> Building {
    let z = map
        .column(tx as usize, ty as usize)
        .layers
        .iter()
        .filter(|l| l.walkable && matches!(l.kind, TileKind::CaveFloor | TileKind::CrystalCave))
        .map(|l| l.z_top)
        .fold(f32::NEG_INFINITY, f32::max);
    let id = deterministic_uuid(rng);
    let style_variant = (id.as_u128() % 10) as u8;
    Building {
        id,
        settlement_id,
        kind,
        tx,
        ty,
        z,
        rotation: (rng.random::<u8>() % 4),
        faction_id: None,
        style_variant,
    }
}

/// Mark each building's tile as non-walkable in the map so movement systems
/// treat buildings as solid obstacles.
///
/// Sets `map.buildings_stamped = true` so cached `.bin` files that predate
/// this feature are automatically invalidated and regenerated.
pub fn apply_building_tiles(buildings: &[Building], map: &mut WorldMap) {
    for b in buildings {
        match b.kind {
            BuildingKind::CapitalTower
            | BuildingKind::Tower
            | BuildingKind::Keep
            | BuildingKind::Tavern
            | BuildingKind::Barracks => {
                apply_tower_stair_tiles(b, map);
            }
            _ => {
                map.mark_impassable(b.tx as usize, b.ty as usize);
            }
        }
    }
    map.buildings_stamped = true;
}

/// Bresenham line rasteriser.  Returns all integer points from `(x0,y0)` to `(x1,y1)`.
fn bresenham(mut x0: i32, mut y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
    let mut pts = Vec::new();
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        pts.push((x0, y0));
        if x0 == x1 && y0 == y1 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x0 += sx; }
        if e2 <= dx { err += dx; y0 += sy; }
    }
    pts
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Generate a deterministic UUID using bytes from a seeded RNG.
fn deterministic_uuid(rng: &mut impl rand::RngExt) -> Uuid {
    let mut bytes = [0u8; 16];
    for b in bytes.iter_mut() {
        *b = rng.random::<u8>();
    }
    Uuid::from_bytes(bytes)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{generate_map, MAP_WIDTH, MAP_HEIGHT};

    #[test]
    fn surface_settlements_are_generated() {
        let map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 42);
        assert!(settlements.len() >= 5, "expected ≥5 settlements, got {}", settlements.len());
    }

    #[test]
    fn at_least_one_capital_generated() {
        let map = generate_map(0, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 0);
        let capital_count = settlements
            .iter()
            .filter(|s| matches!(s.kind, SettlementKind::Capital))
            .count();
        assert!(capital_count >= 1, "expected ≥1 capital, got {capital_count}");
    }

    #[test]
    fn settlements_are_deterministic() {
        let map = generate_map(7, MAP_WIDTH, MAP_HEIGHT);
        let a = generate_settlements(&map, 7);
        let b = generate_settlements(&map, 7);
        assert_eq!(a.len(), b.len(), "settlement count is not deterministic");
        for (sa, sb) in a.iter().zip(b.iter()) {
            assert!((sa.x - sb.x).abs() < 1e-6 && (sa.y - sb.y).abs() < 1e-6,
                "settlement positions are not deterministic");
        }
    }

    #[test]
    fn territory_covers_most_surface_tiles() {
        let map = generate_map(1, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 1);
        let territory = assign_territories(&map, &settlements);

        let walkable: usize = map.columns.iter()
            .filter(|c| c.layers.iter().any(|l| l.is_surface_kind() && l.walkable))
            .count();
        let assigned: usize = territory.iter().filter(|t| t.is_some()).count();

        // With enough settlements, most walkable tiles should be assigned.
        if walkable > 0 {
            let ratio = assigned as f32 / walkable as f32;
            assert!(ratio > 0.5,
                "territory covers only {:.0}% of walkable tiles", ratio * 100.0);
        }
    }

    #[test]
    fn roads_are_written_to_map() {
        let mut map = generate_map(5, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 5);
        generate_roads(&mut map, &settlements);

        let road_count = map.road_tiles.iter().filter(|&&r| r).count();
        assert!(road_count > 0, "no road tiles written");
    }

    #[test]
    fn min_settlement_distance_respected() {
        let map = generate_map(3, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 3);
        let surface: Vec<_> = settlements.iter().filter(|s| s.z >= 0.0).collect();
        for i in 0..surface.len() {
            for j in (i + 1)..surface.len() {
                let si = surface[i];
                let sj = surface[j];
                let dx = si.x - sj.x;
                let dy = si.y - sj.y;
                let dist = (dx * dx + dy * dy).sqrt();
                assert!(
                    dist >= MIN_SETTLEMENT_DIST,
                    "settlements {i} and {j} are too close: {dist:.1} < {MIN_SETTLEMENT_DIST}"
                );
            }
        }
    }

    #[test]
    fn habitability_water_is_zero() {
        assert_eq!(habitability(TileKind::Water), 0.0);
        assert_eq!(habitability(TileKind::River), 0.0);
        assert_eq!(habitability(TileKind::Mountain), 0.0);
    }

    #[test]
    fn habitability_grassland_is_max() {
        assert_eq!(habitability(TileKind::Grassland), 1.0);
    }

    #[test]
    fn territory_index_in_bounds() {
        let map = generate_map(2, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 2);
        let territory = assign_territories(&map, &settlements);
        for &t in &territory {
            if let Some(idx) = t {
                assert!(idx < settlements.len(),
                    "territory index {idx} ≥ settlements.len()={}", settlements.len());
            }
        }
    }

    #[test]
    fn bresenham_endpoints_included() {
        let pts = bresenham(0, 0, 4, 3);
        assert!(pts.contains(&(0, 0)), "start not in bresenham output");
        assert!(pts.contains(&(4, 3)), "end not in bresenham output");
    }

    #[test]
    fn settlements_are_not_on_perfect_grid() {
        // With random jitter, settlements should NOT all land at GRID_CELL-aligned positions.
        let map = generate_map(99, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 99);
        assert!(!settlements.is_empty());
        // Check that at least one settlement's tile coordinates are not exact cell-center multiples.
        let has_jitter = settlements.iter().any(|s| {
            let cell_x = (s.x as usize) / GRID_CELL;
            let cell_y = (s.y as usize) / GRID_CELL;
            // A perfectly grid-aligned settlement would be at cell_x*GRID_CELL + GRID_CELL/2.
            let grid_cx = (cell_x * GRID_CELL + GRID_CELL / 2) as f32;
            let grid_cy = (cell_y * GRID_CELL + GRID_CELL / 2) as f32;
            (s.x - grid_cx).abs() > 1.0 || (s.y - grid_cy).abs() > 1.0
        });
        assert!(has_jitter, "all settlements are on perfect grid positions — jitter not working");
    }

    #[test]
    fn settlements_within_expected_density() {
        // With GRID_CELL=64 and MIN_SETTLEMENT_DIST=60 we expect far fewer settlements
        // than the old configuration (GRID_CELL=32, MIN_SETTLEMENT_DIST=30).
        // Upper bound: at most cells_x * cells_y = (MAP_WIDTH/64)^2.
        let map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 42);
        let max_cells = (MAP_WIDTH / GRID_CELL) * (MAP_HEIGHT / GRID_CELL);
        assert!(
            settlements.len() <= max_cells,
            "too many settlements: {} > max cells {}", settlements.len(), max_cells
        );
    }

    #[test]
    fn generate_buildings_town_count_in_range() {
        let map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 42);
        let buildings = generate_buildings(&settlements, &map, 42);

        for s in settlements.iter().filter(|s| matches!(s.kind, SettlementKind::Town)) {
            let count = buildings.iter().filter(|b| b.settlement_id == s.id).count();
            // 1 campfire center + 3–5 tents = 4–6 total
            assert!(
                (4..=6).contains(&count),
                "Town '{}' has {count} buildings, expected 4–6", s.name
            );
        }
    }

    #[test]
    fn generate_buildings_capital_count_in_range() {
        let map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 42);
        let buildings = generate_buildings(&settlements, &map, 42);

        for s in settlements.iter().filter(|s| matches!(s.kind, SettlementKind::Capital) && s.z >= 0.0) {
            let count = buildings.iter().filter(|b| b.settlement_id == s.id).count();
            // +1 for the center fountain, +1 for the CapitalTower = 9–14 total
            assert!(
                (9..=14).contains(&count),
                "Capital '{}' has {count} buildings, expected 9–14", s.name
            );
        }
    }

    #[test]
    fn generate_buildings_no_tile_collision() {
        let map = generate_map(7, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 7);
        let buildings = generate_buildings(&settlements, &map, 7);

        // Per-settlement uniqueness (buildings from different settlements may
        // overlap tiles only in extreme edge cases, but within one settlement
        // there must be no duplicates).
        for s in &settlements {
            let mut seen = std::collections::HashSet::new();
            for b in buildings.iter().filter(|b| b.settlement_id == s.id) {
                let pos = (b.tx, b.ty);
                assert!(
                    seen.insert(pos),
                    "Settlement '{}' has two buildings at tile ({}, {})", s.name, b.tx, b.ty
                );
            }
        }
    }

    #[test]
    fn generate_buildings_deterministic() {
        let map = generate_map(13, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 13);
        let a = generate_buildings(&settlements, &map, 13);
        let b = generate_buildings(&settlements, &map, 13);
        assert_eq!(a.len(), b.len(), "building count is not deterministic");
        for (ba, bb) in a.iter().zip(b.iter()) {
            assert_eq!(ba.tx, bb.tx);
            assert_eq!(ba.ty, bb.ty);
            assert_eq!(ba.kind, bb.kind);
        }
    }

    #[test]
    fn apply_building_tiles_marks_impassable() {
        let mut map = generate_map(5, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 5);
        let buildings = generate_buildings(&settlements, &map, 5);
        apply_building_tiles(&buildings, &mut map);

        assert!(map.buildings_stamped, "buildings_stamped must be true after apply");
        for b in &buildings {
            match b.kind {
                BuildingKind::CapitalTower
                | BuildingKind::Tower
                | BuildingKind::Keep
                | BuildingKind::Tavern
                | BuildingKind::Barracks => {
                    let col = map.column(b.tx as usize, b.ty as usize);
                    let walkable_count = col.layers.iter().filter(|l| l.walkable).count();
                    assert!(walkable_count >= 1,
                        "tower tile ({},{}) should have walkable stair layers", b.tx, b.ty);
                }
                _ => {
                    let col = map.column(b.tx as usize, b.ty as usize);
                    let walkable = col.layers.iter().any(|l| l.is_surface_kind() && l.walkable);
                    assert!(!walkable,
                        "tile ({},{}) should be impassable after apply_building_tiles", b.tx, b.ty);
                }
            }
        }
    }

    #[test]
    fn tower_stair_layers_are_reachable() {
        use crate::map::STEP_HEIGHT;
        let mut map = generate_map(5, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements(&map, 5);
        let buildings = generate_buildings(&settlements, &map, 5);
        apply_building_tiles(&buildings, &mut map);

        let tower = buildings.iter().find(|b| b.kind == BuildingKind::CapitalTower);
        if let Some(b) = tower {
            let cx = b.tx as usize;
            let cy = b.ty as usize;
            let col = map.column(cx, cy);
            let walkable_count = col.layers.iter().filter(|l| l.walkable).count();
            assert!(walkable_count >= 2,
                "tower center at ({cx},{cy}) should have ≥2 walkable layers, got {walkable_count}");
            let mut z = b.z;
            for layer in col.layers.iter().filter(|l| l.walkable) {
                let reached = col.surface_layer(z, STEP_HEIGHT);
                assert!(reached.is_some(),
                    "layer at z={} should be reachable from z={z}", layer.z_top);
                z = layer.z_top;
            }
        }
    }

    #[test]
    fn underground_civilization_has_capital() {
        let mut map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements_full(&mut map, 42);
        let underground_capitals: Vec<_> = settlements
            .iter()
            .filter(|s| matches!(s.kind, SettlementKind::Capital) && s.z < 0.0)
            .collect();
        assert_eq!(
            underground_capitals.len(), 1,
            "expected exactly 1 underground capital, got {}", underground_capitals.len()
        );
    }

    #[test]
    fn underground_capital_is_at_negative_z() {
        let mut map = generate_map(7, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements_full(&mut map, 7);
        let cap = settlements
            .iter()
            .find(|s| matches!(s.kind, SettlementKind::Capital) && s.z < 0.0)
            .expect("should have an underground capital");
        assert!(cap.z < 0.0, "underground capital z={} should be negative", cap.z);
        assert!(
            (cap.z - crate::cave::cave_z(1)).abs() < 0.5,
            "underground capital z={} should be near cave depth 1 ({})", cap.z, crate::cave::cave_z(1)
        );
    }

    #[test]
    fn underground_capital_has_buildings() {
        let mut map = generate_map(42, MAP_WIDTH, MAP_HEIGHT);
        let settlements = generate_settlements_full(&mut map, 42);
        let buildings = generate_buildings(&settlements, &map, 42);
        let cap = settlements
            .iter()
            .find(|s| matches!(s.kind, SettlementKind::Capital) && s.z < 0.0)
            .expect("should have an underground capital");
        let count = buildings.iter().filter(|b| b.settlement_id == cap.id).count();
        assert!(count >= 1, "underground capital should have at least 1 building, got {count}");
    }

}
