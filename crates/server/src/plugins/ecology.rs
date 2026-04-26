//! Ecology plugin: seeds region ecologies from the world map, runs Lotka-Volterra
//! population dynamics each WorldSimSchedule tick, and syncs wildlife entity counts
//! to the simulated predator populations.

use std::collections::HashMap;

use bevy::prelude::*;
use crate::plugins::ai::HomePosition;
use crate::plugins::interest::ChunkTemperature;
use crate::plugins::persistence::Db;
use fellytip_shared::{
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{EntityKind, GrowthStage, Health, WildlifeKind, WorldPosition},
    world::{
        ecology::{EcologyEvent, Population, RegionEcology, RegionId, SpeciesId, tick_ecology},
        map::{smooth_surface_at, TileKind, WorldMap, MAP_WIDTH, MAP_HALF_WIDTH, MAP_HALF_HEIGHT, CHUNK_TILES},
    },
};
use uuid::Uuid;

use crate::plugins::combat::{CombatParticipant, ExperienceReward};
use crate::plugins::world_sim::{WorldSimSchedule, WorldSimTick};

/// Bevy resource holding all region ecologies.
#[derive(Resource, Default)]
pub struct EcologyState {
    pub regions: Vec<RegionEcology>,
}

/// Server-only marker for wildlife entities spawned from ecology populations.
#[derive(Component)]
pub struct WildlifeNpc {
    pub region: RegionId,
}

/// Grid size used for macro-region division (4×4 grid → 16 regions).
const MACRO_GRID: usize = 4;
/// Tile width/height of each macro-region (MAP_WIDTH / MACRO_GRID).
const MACRO_REGION_SIZE: usize = MAP_WIDTH / MACRO_GRID;

/// Predator population threshold below which no wildlife entities are spawned.
const SPAWN_THRESHOLD: f64 = 10.0;
/// Maximum wildlife NPC spawns per WorldSim tick (prevents history-warp spikes).
const MAX_SPAWNS_PER_TICK: usize = 5;

pub struct EcologyPlugin;

impl Plugin for EcologyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EcologyState>();
        app.add_systems(
            WorldSimSchedule,
            (run_ecology_tick, sync_wildlife_entities, age_wildlife_system, wander_wildlife, flush_ecology_to_db).chain(),
        );
    }
}

/// Seed `EcologyState` from the world map by classifying each 128×128 macro-region
/// and assigning Lotka-Volterra parameters matching the dominant biome.
///
/// Must run after `generate_world` inserts the `WorldMap` resource.
/// Registered in `MapGenPlugin` between `generate_world` and `history_warp`.
pub fn seed_ecology(map: Res<WorldMap>, mut state: ResMut<EcologyState>) {
    for ry in 0..MACRO_GRID {
        for rx in 0..MACRO_GRID {
            // Sample the center tile of the macro-region to determine its dominant biome.
            // Convert tile indices to world-space (map centered on (0,0)).
            let cx = (rx * MACRO_REGION_SIZE + MACRO_REGION_SIZE / 2) as f32 - MAP_HALF_WIDTH as f32;
            let cy = (ry * MACRO_REGION_SIZE + MACRO_REGION_SIZE / 2) as f32 - MAP_HALF_WIDTH as f32;

            let Some(col) = map.column_at(cx, cy) else { continue };
            // Find the topmost walkable layer at this sample point.
            // surface_layer(z, step) restricts by height; we want the dominant biome
            // regardless of elevation, so we iterate directly.
            let Some(surface) = col.layers.iter().rev().find(|l| l.walkable) else { continue };

            // Assign prey/predator starting counts and Lotka-Volterra coefficients
            // based on the biome's resource richness.
            // Parameters: (prey_start, pred_start, r, k, alpha, beta, delta)
            let params: Option<(f64, f64, f64, f64, f64, f64, f64)> = match surface.kind {
                // Rich temperate biomes
                TileKind::TemperateForest
                | TileKind::Grassland
                | TileKind::Plains
                | TileKind::Forest
                | TileKind::Savanna => Some((100.0, 20.0, 0.5, 200.0, 0.01, 0.5, 0.1)),
                // Tropical dense biomes
                TileKind::TropicalForest | TileKind::TropicalRainforest => {
                    Some((80.0, 18.0, 0.5, 180.0, 0.01, 0.5, 0.1))
                }
                // Boreal cold forests
                TileKind::Taiga => Some((60.0, 12.0, 0.4, 120.0, 0.01, 0.5, 0.12)),
                // Rocky terrain
                TileKind::Stone => Some((40.0, 8.0, 0.4, 80.0, 0.015, 0.45, 0.12)),
                // Harsh arid/cold biomes
                TileKind::Desert
                | TileKind::Tundra
                | TileKind::PolarDesert
                | TileKind::Arctic => Some((20.0, 4.0, 0.3, 50.0, 0.02, 0.4, 0.15)),
                // Non-viable: water, impassable terrain, cave/underground
                TileKind::Water
                | TileKind::Mountain
                | TileKind::River
                | TileKind::CaveFloor
                | TileKind::CaveWall
                | TileKind::CrystalCave
                | TileKind::LavaFloor
                | TileKind::CaveRiver
                | TileKind::CavePortal
                | TileKind::Void => None,
            };

            let Some((prey_start, pred_start, r, k, alpha, beta, delta)) = params else {
                continue;
            };

            let region_id = RegionId(smol_str::SmolStr::new(format!("macro_{rx}_{ry}")));
            state.regions.push(RegionEcology {
                region: region_id,
                prey: Population {
                    species: SpeciesId(smol_str::SmolStr::new("deer")),
                    count: prey_start,
                },
                predator: Population {
                    species: SpeciesId(smol_str::SmolStr::new("wolf")),
                    count: pred_start,
                },
                r,
                k,
                alpha,
                beta,
                delta,
            });
        }
    }

    tracing::info!(regions = state.regions.len(), "Ecology seeded from world map");
}

fn run_ecology_tick(
    mut state: ResMut<EcologyState>,
    tick: Res<crate::plugins::world_sim::WorldSimTick>,
) {
    let regions = std::mem::take(&mut state.regions);
    state.regions = regions
        .into_iter()
        .flat_map(|region| {
            let (next, events) = tick_ecology(region);
            for ev in events {
                match ev {
                    EcologyEvent::Collapse { species, region } => {
                        tracing::warn!(
                            tick = tick.0,
                            "Ecology collapse: {:?} in {:?}",
                            species.0,
                            region.0
                        );
                    }
                    EcologyEvent::Recovery { species, region } => {
                        tracing::info!(
                            tick = tick.0,
                            "Ecology recovery: {:?} in {:?}",
                            species.0,
                            region.0
                        );
                    }
                }
            }
            Some(next)
        })
        .collect();
}

/// Maintain wildlife NPC entities whose count tracks simulated predator populations.
///
/// For each ecology region: if predator population > `SPAWN_THRESHOLD`, ensure
/// `floor(predator_count / 20)` wildlife entities exist. Despawn excess entities
/// when populations collapse below the threshold. Creatures are scattered within
/// ±40 tiles of the region center so they don't all stack at one point.
/// Rarely, a baby (`GrowthStage(0.0)`) is spawned instead of an adult.
fn sync_wildlife_entities(
    state: Res<EcologyState>,
    wildlife_query: Query<(Entity, &WildlifeNpc)>,
    map: Option<Res<WorldMap>>,
    tick: Res<WorldSimTick>,
    mut commands: Commands,
) {
    // Build a per-region map of (count, entity_list) for existing wildlife.
    let mut region_counts: HashMap<String, (usize, Vec<Entity>)> = HashMap::new();
    for (entity, npc) in wildlife_query.iter() {
        let entry = region_counts
            .entry(npc.region.0.to_string())
            .or_insert((0, Vec::new()));
        entry.0 += 1;
        entry.1.push(entity);
    }

    let mut spawns_this_tick = 0usize;

    for ecology in &state.regions {
        let region_key = ecology.region.0.to_string();

        if ecology.predator.count < SPAWN_THRESHOLD {
            // Population collapsed — despawn all wildlife in this region.
            if let Some((_, entities)) = region_counts.get(&region_key) {
                for &entity in entities {
                    commands.entity(entity).despawn();
                }
            }
            continue;
        }

        let desired = (ecology.predator.count / 20.0).floor() as usize;
        let current = region_counts.get(&region_key).map(|(c, _)| *c).unwrap_or(0);

        // Despawn if over the desired count (can happen after a population drop).
        if current > desired {
            let excess = current - desired;
            if let Some((_, entities)) = region_counts.get(&region_key) {
                for &entity in entities.iter().take(excess) {
                    commands.entity(entity).despawn();
                }
            }
            continue;
        }

        // Spawn up to MAX_SPAWNS_PER_TICK new wildlife per tick.
        if current < desired && spawns_this_tick < MAX_SPAWNS_PER_TICK {
            let (spawn_x, spawn_y) = region_center_from_id(&ecology.region);

            // Scatter each creature within ±40 tiles of region center using
            // deterministic jitter based on slot index (same style as tick_population_system).
            #[allow(clippy::cast_precision_loss)]
            let spawn_index = current as f32;
            #[allow(clippy::cast_precision_loss)]
            let region_len = ecology.region.0.len() as f32;
            let seed_a = (region_len + spawn_index) * 1234.5;
            let seed_b = region_len * 7.3 + spawn_index * 3.1;
            let jitter_x = seed_a.sin() * 40.0;
            let jitter_y = seed_b.cos() * 40.0;
            let pos = WorldPosition {
                x: spawn_x + jitter_x,
                y: spawn_y + jitter_y,
                z: map
                    .as_ref()
                    .and_then(|m| smooth_surface_at(m, spawn_x + jitter_x, spawn_y + jitter_y, 0.0))
                    .unwrap_or(0.0),
            };

            let wildlife_kind = match ecology.region.0.len() % 3 {
                0 => WildlifeKind::Bison,
                1 => WildlifeKind::Dog,
                _ => WildlifeKind::Horse,
            };

            // Rarely spawn a baby instead of an adult when the population is healthy.
            // Period is staggered per region so babies don't all appear simultaneously.
            let region_index = ecology.region.0.len() as u64;
            let baby_period = (region_index * 300 + 600).max(600);
            let spawn_baby = ecology.predator.count > 30.0 && tick.0.is_multiple_of(baby_period);

            if spawn_baby {
                commands.spawn((
                    pos.clone(),
                    Health { current: 5, max: 5 },
                    CombatParticipant {
                        id: CombatantId(Uuid::new_v4()),
                        interrupt_stack: InterruptStack::default(),
                        class: CharacterClass::Rogue,
                        level: 1,
                        armor_class: 10,
                        strength: 6,
                        dexterity: 12,
                        constitution: 8,
                    },
                    ExperienceReward(5),
                    WildlifeNpc { region: ecology.region.clone() },
                    EntityKind::Wildlife,
                    wildlife_kind,
                    HomePosition(pos),
                    GrowthStage(0.0),
                ));
            } else {
                commands.spawn((
                    pos.clone(),
                    Health { current: 15, max: 15 },
                    CombatParticipant {
                        id: CombatantId(Uuid::new_v4()),
                        interrupt_stack: InterruptStack::default(),
                        class: CharacterClass::Rogue,
                        level: 1,
                        armor_class: 10,
                        strength: 8,
                        dexterity: 12,
                        constitution: 10,
                    },
                    // CR 1/8 = 25 XP (docs/dnd5e-srd-reference.md)
                    ExperienceReward(25),
                    WildlifeNpc { region: ecology.region.clone() },
                    EntityKind::Wildlife,
                    wildlife_kind,
                    HomePosition(pos),
                ));
            }
            spawns_this_tick += 1;
        }
    }
}

/// Increment `GrowthStage` for baby wildlife each tick, scaled by zone speed.
/// Babies in the Hot zone mature in ~300 ticks (5 min). Absent on adults.
fn age_wildlife_system(
    mut query: Query<(&mut GrowthStage, &mut Health, &WorldPosition), With<WildlifeNpc>>,
    temp: Res<ChunkTemperature>,
) {
    for (mut growth, mut health, pos) in &mut query {
        let speed = temp.speed_at_world(pos.x, pos.y);
        let prev = growth.0;
        growth.0 = (growth.0 + speed / 300.0).min(1.0);
        if prev < 1.0 && growth.0 >= 1.0 {
            health.max = 15;
            health.current = health.current.max(1);
        }
    }
}

/// Move each wildlife entity a small amount per tick using deterministic bounded
/// wandering. Frozen chunks are skipped to avoid wasting budget on unobserved entities.
fn wander_wildlife(
    mut query: Query<(Entity, &mut WorldPosition, &HomePosition), With<WildlifeNpc>>,
    temp: Res<ChunkTemperature>,
    tick: Res<WorldSimTick>,
) {
    for (entity, mut pos, home) in &mut query {
        let tile_x = (pos.x + MAP_HALF_WIDTH as f32) as i32;
        let tile_y = (pos.y + MAP_HALF_HEIGHT as f32) as i32;
        let chunk = (tile_x.max(0) / CHUNK_TILES as i32, tile_y.max(0) / CHUNK_TILES as i32);

        if !temp.is_active(chunk) {
            continue; // Frozen — skip simulation
        }

        let zone_speed = temp.zone_speed(chunk);

        // Slowly-rotating deterministic angle, unique per entity.
        // entity.to_bits() cast to f32 is intentionally lossy — we only need rough variation.
        #[allow(clippy::cast_precision_loss)]
        let entity_seed = entity.to_bits() as f32 * 0.000001;
        #[allow(clippy::cast_precision_loss)]
        let angle = (entity_seed + tick.0 as f32 * 0.05).sin() * std::f32::consts::TAU;
        let step = 0.2 * zone_speed;

        // Bounded wander: pull back toward HomePosition when beyond 20 tiles.
        let home_dx = home.0.x - pos.x;
        let home_dy = home.0.y - pos.y;
        let dist_sq = home_dx * home_dx + home_dy * home_dy;
        if dist_sq > 20.0f32.powi(2) {
            let dist = dist_sq.sqrt();
            let pull = 0.1 * zone_speed;
            pos.x += (home_dx / dist) * pull;
            pos.y += (home_dy / dist) * pull;
        } else {
            pos.x += angle.cos() * step;
            pos.y += angle.sin() * step;
        }
    }
}

/// How many world-sim ticks between ecology SQLite flushes (30 s at 1 Hz).
const ECOLOGY_FLUSH_INTERVAL: u64 = 30;

/// Flush current ecology population counts to SQLite immediately (blocking).
///
/// Called by the timed flush system and by the graceful shutdown hook so that
/// both code paths share the same SQL logic.
pub fn flush_ecology_now(state: &EcologyState, db: &Db) {
    let pool = db.pool().clone();
    let regions = state.regions.clone();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for ecology flush");

    rt.block_on(async move {
        for ecology in &regions {
            let region_id = ecology.region.0.as_str().to_owned();

            let res_prey = sqlx::query(
                "INSERT OR REPLACE INTO ecology_state (species_id, region_id, count) \
                 VALUES (?, ?, ?)",
            )
            .bind(ecology.prey.species.0.as_str())
            .bind(&region_id)
            .bind(ecology.prey.count as i64)
            .execute(&pool)
            .await;

            let res_pred = sqlx::query(
                "INSERT OR REPLACE INTO ecology_state (species_id, region_id, count) \
                 VALUES (?, ?, ?)",
            )
            .bind(ecology.predator.species.0.as_str())
            .bind(&region_id)
            .bind(ecology.predator.count as i64)
            .execute(&pool)
            .await;

            if let Err(e) = res_prey {
                tracing::warn!(region = %region_id, "Prey flush failed: {e}");
            }
            if let Err(e) = res_pred {
                tracing::warn!(region = %region_id, "Predator flush failed: {e}");
            }
        }
        tracing::debug!("Ecology state flushed to SQLite");
    });
}

/// Persist current ecology population counts to SQLite every
/// `ECOLOGY_FLUSH_INTERVAL` world-sim ticks.
///
/// Worldwatch reads `ecology_state` directly from the DB. Follows the same
/// block_on flush pattern as `flush_story_log` in story.rs.
fn flush_ecology_to_db(
    state: Res<EcologyState>,
    tick: Res<crate::plugins::world_sim::WorldSimTick>,
    db: Res<Db>,
) {
    if tick.0 == 0 || !tick.0.is_multiple_of(ECOLOGY_FLUSH_INTERVAL) {
        return;
    }
    flush_ecology_now(&state, &db);
}

/// Extract (x, y) tile-space center for a macro-region ID of the form "macro_{rx}_{ry}".
/// Returns (0, 0) for unrecognised IDs.
fn region_center_from_id(id: &RegionId) -> (f32, f32) {
    let s = match id.0.strip_prefix("macro_") {
        Some(s) => s,
        None => return (0.0, 0.0),
    };
    let mut parts = s.splitn(2, '_');
    let rx: usize = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let ry: usize = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    // Convert tile-space center to world-space (map centered on (0,0)).
    let cx = (rx * MACRO_REGION_SIZE + MACRO_REGION_SIZE / 2) as f32 - MAP_HALF_WIDTH as f32;
    let cy = (ry * MACRO_REGION_SIZE + MACRO_REGION_SIZE / 2) as f32 - MAP_HALF_WIDTH as f32;
    (cx, cy)
}
