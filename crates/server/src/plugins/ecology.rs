//! Ecology plugin: seeds region ecologies from the world map, runs Lotka-Volterra
//! population dynamics each WorldSimSchedule tick, and syncs wildlife entity counts
//! to the simulated predator populations.
//!
//! Issues implemented here:
//! - #112: Tree growth lifecycle (FloraEntity, seedling dispersal, deadwood decay)
//! - #113: Farm crop production cycles (FarmPlot, settlement food supply)
//! - #114: Animal AI (AnimalBehavior — grazing, fleeing, hunting)
//! - #115: Hunting mechanic (Loot drops, Ranger NPC hunt_prey system)
//! - #116: Ecological balance / spatial distribution (prey overpop, predator starvation)

use std::collections::HashMap;

use bevy::ecs::message::MessageWriter;
use bevy::prelude::*;
use crate::plugins::ai::HomePosition;
use crate::plugins::interest::ChunkTemperature;
use crate::plugins::persistence::Db;
use fellytip_shared::{
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{EntityKind, GrowthStage, Health, WildlifeKind, WorldPosition},
    world::{
        ecology::{
            AnimalState, apply_spatial_balance, cave_carrying_capacity, EcologyEvent, EcologyRole,
            FarmPlotState, FloraKind, FloraState, LootKind,
            Population, RegionEcology, RegionId, SpeciesId, tick_ecology,
            tree_growth_rate, CAVE_HERBIVORE, CAVE_PREDATOR,
        },
        map::{smooth_surface_at, TileKind, WorldMap, MAP_WIDTH, MAP_HALF_WIDTH, MAP_HALF_HEIGHT, CHUNK_TILES},
        story::{StoryEvent, StoryEventKind, WriteStoryEvent},
    },
};
use smol_str::SmolStr;
use uuid::Uuid;

use crate::plugins::combat::{CombatParticipant, ExperienceReward};
use crate::plugins::world_sim::{WorldSimSchedule, WorldSimTick};

// ── Resources ─────────────────────────────────────────────────────────────────

/// Bevy resource holding all region ecologies.
#[derive(Resource, Default)]
pub struct EcologyState {
    pub regions: Vec<RegionEcology>,
}

/// Bevy resource holding all active farm plots (indexed by settlement UUID string).
#[derive(Resource, Default)]
pub struct FarmState {
    /// Settlement UUID (string) → farm plot state.
    pub plots: HashMap<String, FarmPlotState>,
    /// Settlement UUID → accumulated food supply.
    pub food_supply: HashMap<String, f32>,
}

// ── Components ────────────────────────────────────────────────────────────────

/// Server-only marker for wildlife entities spawned from ecology populations.
#[derive(Component)]
pub struct WildlifeNpc {
    pub region: RegionId,
}

/// Server-only component for flora entities (trees, shrubs, crops, deadwood).
#[derive(Component)]
pub struct FloraEntity {
    pub state: FloraState,
    /// Tile-space tile kind at this flora entity's position (for growth rate).
    pub biome: TileKind,
}

/// Server-only component tracking animal behavioral state.
#[derive(Component)]
pub struct AnimalBehavior {
    pub role: EcologyRole,
    pub state: AnimalState,
    /// Current chase/flee target entity (if any).
    pub target: Option<Entity>,
}

/// Server-only component: loot item dropped on kill.
#[derive(Component)]
pub struct Loot {
    pub kind: LootKind,
    pub quantity: u8,
}

/// Marker component: this NPC is a Ranger — actively hunts nearby prey.
#[derive(Component)]
pub struct RangerNpc;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Grid size used for macro-region division (4×4 grid → 16 regions).
const MACRO_GRID: usize = 4;
/// Tile width/height of each macro-region (MAP_WIDTH / MACRO_GRID).
const MACRO_REGION_SIZE: usize = MAP_WIDTH / MACRO_GRID;

/// Predator population threshold below which no wildlife entities are spawned.
const SPAWN_THRESHOLD: f64 = 10.0;
/// Maximum wildlife NPC spawns per WorldSim tick (prevents history-warp spikes).
const MAX_SPAWNS_PER_TICK: usize = 5;

/// Ticks between new tree seedling spawns (to prevent flooding the world).
const SEEDLING_COOLDOWN_TICKS: u64 = 10;
/// Max seedling spawns per WorldSim tick.
const MAX_SEEDLING_SPAWNS_PER_TICK: usize = 3;
/// Chance per tick that a mature tree disperses a seed (1 in N).
const SEED_DISPERSAL_CHANCE: u64 = 50;
/// Ticks before deadwood decays and is removed.
const DEADWOOD_DECAY_TICKS: u32 = 600;

/// Range in tiles at which prey detects predators and flees.
const FLEE_RANGE_SQ: f32 = 8.0 * 8.0;
/// Range in tiles at which predators start hunting prey.
const HUNT_RANGE_SQ: f32 = 12.0 * 12.0;
/// Range for a predator to land an attack (adjacent).
const ATTACK_RANGE_SQ: f32 = 1.5 * 1.5;
/// Range for a Ranger NPC to initiate a hunt.
const RANGER_HUNT_RANGE_SQ: f32 = 20.0 * 20.0;

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct EcologyPlugin;

impl Plugin for EcologyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EcologyState>();
        app.init_resource::<FarmState>();
        app.add_systems(
            WorldSimSchedule,
            (
                apply_pressure_to_cave_ecology,
                run_ecology_tick,
                apply_balance_dynamics,
                sync_wildlife_entities,
                age_wildlife_system,
                wander_wildlife,
                update_animal_behavior,
                grow_flora,
                tick_farm_plots,
                hunt_prey_ranger,
                flush_ecology_to_db,
            ).chain(),
        );
    }
}

// ── World-seeding ─────────────────────────────────────────────────────────────

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
            let Some(surface) = col.layers.iter().rev().find(|l| l.walkable) else { continue };

            // Assign prey/predator starting counts and Lotka-Volterra coefficients
            // based on the biome's resource richness.
            // Parameters: (prey_start, pred_start, r, k, alpha, beta, delta)
            // Tuned for stable oscillation with the corrected additive update formula
            // (new_q = q + q*(beta*alpha*p - delta)).
            // Equilibrium: prey* = delta/(beta*alpha), pred* = r/alpha*(1-prey*/k).
            // Starting populations are placed at (or near) the fixed point so the
            // system is immediately stable rather than collapsing on tick 2.
            let params: Option<(f64, f64, f64, f64, f64, f64, f64)> = match surface.kind {
                // Rich temperate biomes — prey*=100, pred*=20 (exact fixed point)
                TileKind::TemperateForest
                | TileKind::Grassland
                | TileKind::Plains
                | TileKind::Forest
                | TileKind::Savanna => Some((100.0, 20.0, 0.1, 250.0, 0.003, 0.1, 0.03)),
                // Tropical dense biomes — prey*=100, pred*=18
                TileKind::TropicalForest | TileKind::TropicalRainforest => {
                    Some((100.0, 18.0, 0.1, 280.0, 0.003, 0.1, 0.03))
                }
                // Boreal cold forests — prey*=80, pred*=12
                TileKind::Taiga => Some((80.0, 12.0, 0.08, 220.0, 0.003, 0.1, 0.024)),
                // Rocky terrain — prey*=60, pred*=8
                TileKind::Stone => Some((60.0, 8.0, 0.07, 180.0, 0.003, 0.1, 0.018)),
                // Harsh arid/cold biomes — prey*=30, pred*=4
                TileKind::Desert
                | TileKind::Tundra
                | TileKind::PolarDesert
                | TileKind::Arctic => Some((30.0, 4.0, 0.06, 120.0, 0.003, 0.1, 0.009)),
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

    // ── Underground / cave regions ────────────────────────────────────────────
    // Seed cave ecology for each macro-region that has a cave tile as its
    // dominant walkable surface. Cave species use halved birth rates (slower
    // reproduction in resource-scarce cave environments).
    for ry in 0..MACRO_GRID {
        for rx in 0..MACRO_GRID {
            let cx = (rx * MACRO_REGION_SIZE + MACRO_REGION_SIZE / 2) as f32 - MAP_HALF_WIDTH as f32;
            let cy = (ry * MACRO_REGION_SIZE + MACRO_REGION_SIZE / 2) as f32 - MAP_HALF_WIDTH as f32;

            let Some(col) = map.column_at(cx, cy) else { continue };
            // Look for a cave layer (non-walkable surface layers are skipped by the main loop).
            let cave_layer = col.layers.iter().find(|l| {
                matches!(l.kind, TileKind::CaveFloor | TileKind::CrystalCave | TileKind::LavaFloor)
            });
            let Some(cave_layer) = cave_layer else { continue };

            let base_k = cave_carrying_capacity(cave_layer.kind);
            // Cave birth rate is half the standard surface rate (slower reproduction).
            let cave_r = 0.05; // half of typical surface r ≈ 0.1

            let region_id = RegionId(smol_str::SmolStr::new(format!("cave_{rx}_{ry}")));
            state.regions.push(RegionEcology {
                region: region_id,
                prey: Population {
                    species: SpeciesId(smol_str::SmolStr::new(CAVE_HERBIVORE)),
                    count: base_k * 0.4, // start at ~40% of carrying capacity
                },
                predator: Population {
                    species: SpeciesId(smol_str::SmolStr::new(CAVE_PREDATOR)),
                    count: (base_k * 0.05).max(1.0),
                },
                r: cave_r,
                k: base_k,
                alpha: 0.003,
                beta: 0.1,
                delta: 0.024,
            });
        }
    }

    tracing::info!(regions = state.regions.len(), "Ecology seeded from world map");
}

/// Seed the initial farm plots near the first few settlements.
///
/// Called from MapGenPlugin after `generate_world`. Settlements are extracted
/// from the map's spawn_points list as a proxy (actual settlement positions are
/// registered in the civilization resource).  We create one farm plot per region
/// that contains a settlement, associated to a synthetic settlement UUID.
pub fn seed_farm_plots(mut farm_state: ResMut<FarmState>) {
    // Seed a handful of farm plots with synthetic settlement IDs.
    // In a full implementation these would be linked to actual `Settlements`.
    for i in 0..4u8 {
        let sid = Uuid::from_bytes([i, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let sid_str = sid.to_string();
        farm_state.plots.insert(sid_str.clone(), FarmPlotState::new(sid));
        farm_state.food_supply.insert(sid_str, 0.0);
    }
    tracing::info!(plots = farm_state.plots.len(), "Farm plots seeded");
}

// ── Core ecology tick ─────────────────────────────────────────────────────────

fn run_ecology_tick(
    mut state: ResMut<EcologyState>,
    tick: Res<crate::plugins::world_sim::WorldSimTick>,
    mut story_writer: MessageWriter<WriteStoryEvent>,
) {
    let regions = std::mem::take(&mut state.regions);
    state.regions = regions
        .into_iter()
        .flat_map(|region| {
            let (next, events) = tick_ecology(region);

            // Debug: log wolf population at key ticks for the first region only.
            if matches!(tick.0, 1 | 2 | 5 | 10) && next.region.0.ends_with("0_0") {
                tracing::info!(
                    tick = tick.0,
                    region = %next.region.0,
                    prey = next.prey.count,
                    predators = next.predator.count,
                    "Ecology population snapshot"
                );
            }
            for ev in &events {
                match ev {
                    EcologyEvent::Collapse { species, region } => {
                        tracing::warn!(
                            tick = tick.0,
                            "Ecology collapse: {:?} in {:?}",
                            species.0,
                            region.0
                        );
                        story_writer.write(WriteStoryEvent(StoryEvent {
                            id: Uuid::new_v4(),
                            tick: tick.0,
                            world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
                            kind: StoryEventKind::EcologyCollapse {
                                species: species.clone(),
                                region: region.clone(),
                            },
                            participants: vec![],
                            location: None,
                            lore_tags: vec!["ecology".into(), "collapse".into()],
                        }));
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

            // #116: Emit prey scarcity / predator extinction story events.
            if next.prey.count < 10.0 && next.prey.count > 0.0 {
                tracing::warn!(
                    tick = tick.0,
                    region = %next.region.0,
                    prey = next.prey.count,
                    "Prey scarcity in region"
                );
                story_writer.write(WriteStoryEvent(StoryEvent {
                    id: Uuid::new_v4(),
                    tick: tick.0,
                    world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
                    kind: StoryEventKind::PreyScarcity {
                        species: next.prey.species.clone(),
                        region: next.region.clone(),
                    },
                    participants: vec![],
                    location: None,
                    lore_tags: vec!["ecology".into(), "scarcity".into()],
                }));
            }
            if next.predator.count == 0.0 {
                tracing::warn!(
                    tick = tick.0,
                    region = %next.region.0,
                    "Predator extinction in region"
                );
                story_writer.write(WriteStoryEvent(StoryEvent {
                    id: Uuid::new_v4(),
                    tick: tick.0,
                    world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
                    kind: StoryEventKind::PredatorExtinction {
                        species: next.predator.species.clone(),
                        region: next.region.clone(),
                    },
                    participants: vec![],
                    location: None,
                    lore_tags: vec!["ecology".into(), "extinction".into()],
                }));
            }

            Some(next)
        })
        .collect();
}

// ── #116: Spatial balance — prey overpop / predator starvation ────────────────

/// Apply spatial distribution balance dynamics after the main Lotka-Volterra step.
///
/// - When a region's prey drops to 0, predators starve (population halved).
/// - When a region's predators drop to 0, prey overpopulates (capped at 2× normal max).
fn apply_balance_dynamics(
    mut state: ResMut<EcologyState>,
    tick: Res<WorldSimTick>,
    mut story_writer: MessageWriter<WriteStoryEvent>,
) {
    for ecology in &mut state.regions {
        // The "normal max" for prey is the carrying capacity k.
        let normal_max = ecology.k;
        let extra_events = apply_spatial_balance(ecology, normal_max);

        for ev in extra_events {
            match &ev {
                EcologyEvent::Collapse { species, region } => {
                    tracing::warn!(
                        tick = tick.0,
                        "Predator starvation: {:?} in {:?}",
                        species.0, region.0
                    );
                    story_writer.write(WriteStoryEvent(StoryEvent {
                        id: Uuid::new_v4(),
                        tick: tick.0,
                        world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
                        kind: StoryEventKind::PredatorStarved {
                            species: species.clone(),
                            region: region.clone(),
                        },
                        participants: vec![],
                        location: None,
                        lore_tags: vec!["ecology".into(), "starvation".into()],
                    }));
                }
                EcologyEvent::Recovery { .. } => {} // not used by balance fn
            }
        }

        // Also emit a prey overpopulation event if predators are absent.
        if ecology.predator.count < fellytip_shared::world::ecology::COLLAPSE_THRESHOLD
            && ecology.prey.count > ecology.k
        {
            // Only emit once — skip if already near the cap.
            if ecology.prey.count >= ecology.k * 1.9 {
                story_writer.write(WriteStoryEvent(StoryEvent {
                    id: Uuid::new_v4(),
                    tick: tick.0,
                    world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
                    kind: StoryEventKind::PreyOverpopulated {
                        species: ecology.prey.species.clone(),
                        region: ecology.region.clone(),
                    },
                    participants: vec![],
                    location: None,
                    lore_tags: vec!["ecology".into(), "overpopulation".into()],
                }));
            }
        }
    }
}

// ── Wildlife entity sync ──────────────────────────────────────────────────────

/// Maintain wildlife NPC entities whose count tracks simulated predator populations.
///
/// For each ecology region: if predator population > `SPAWN_THRESHOLD`, ensure
/// `floor(predator_count / 20)` wildlife entities exist. Despawn excess entities
/// when populations collapse below the threshold. Creatures are scattered within
/// ±40 tiles of the region center so they don't all stack at one point.
/// Rarely, a baby (`GrowthStage(0.0)`) is spawned instead of an adult.
/// All wildlife entities now get an `AnimalBehavior` component (prey/predator role).
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
            // deterministic jitter based on slot index.
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

            // Determine ecology role: alternate prey/predator per spawn index.
            let role = if (spawn_index as usize).is_multiple_of(3) {
                EcologyRole::Predator
            } else {
                EcologyRole::Prey
            };

            // Rarely spawn a baby instead of an adult when the population is healthy.
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
                        intelligence: 2,
                        wisdom: 10,
                        charisma: 3,
                    },
                    ExperienceReward(5),
                    WildlifeNpc { region: ecology.region.clone() },
                    EntityKind::Wildlife,
                    wildlife_kind,
                    HomePosition(pos),
                    GrowthStage(0.0),
                    AnimalBehavior { role, state: AnimalState::Grazing, target: None },
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
                        intelligence: 2,
                        wisdom: 10,
                        charisma: 3,
                    },
                    // CR 1/8 = 25 XP (docs/dnd5e-srd-reference.md)
                    ExperienceReward(25),
                    WildlifeNpc { region: ecology.region.clone() },
                    EntityKind::Wildlife,
                    wildlife_kind,
                    HomePosition(pos),
                    AnimalBehavior { role, state: AnimalState::Grazing, target: None },
                    // Loot component: killing this animal drops random loot
                    Loot {
                        kind: if role == EcologyRole::Prey { LootKind::Meat } else { LootKind::Hide },
                        quantity: 1 + (spawn_index as u8 % 3),
                    },
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
    mut query: Query<(Entity, &mut WorldPosition, &HomePosition, &AnimalBehavior), With<WildlifeNpc>>,
    temp: Res<ChunkTemperature>,
    tick: Res<WorldSimTick>,
) {
    for (entity, mut pos, home, behavior) in &mut query {
        // Only wander when grazing or resting — behavior system handles flee/hunt movement.
        if !matches!(behavior.state, AnimalState::Grazing | AnimalState::Resting) {
            continue;
        }

        let tile_x = (pos.x + MAP_HALF_WIDTH as f32) as i32;
        let tile_y = (pos.y + MAP_HALF_HEIGHT as f32) as i32;
        let chunk = (tile_x.max(0) / CHUNK_TILES as i32, tile_y.max(0) / CHUNK_TILES as i32);

        if !temp.is_active(chunk) {
            continue; // Frozen — skip simulation
        }

        let zone_speed = temp.zone_speed(chunk);

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

// ── #114: Animal AI — grazing, fleeing, predator hunting ─────────────────────

/// Update animal behavioral states and move accordingly.
///
/// - Prey: switch to Fleeing if a predator is within `FLEE_RANGE_SQ` tiles.
/// - Predators: switch to Hunting if prey is within `HUNT_RANGE_SQ`; attack when adjacent.
fn update_animal_behavior(
    mut animals: Query<(Entity, &mut WorldPosition, &mut AnimalBehavior), With<WildlifeNpc>>,
    tick: Res<WorldSimTick>,
) {
    // Collect positions snapshot for distance queries (avoid borrow conflicts).
    let snapshot: Vec<(Entity, [f32; 2], EcologyRole)> = animals
        .iter()
        .map(|(e, pos, b)| (e, [pos.x, pos.y], b.role))
        .collect();

    // Determine flee/hunt targets — pure math pass, no mutations yet.
    let mut new_targets: HashMap<Entity, (AnimalState, Option<Entity>)> = HashMap::new();

    for &(self_entity, self_pos, self_role) in &snapshot {
        match self_role {
            EcologyRole::Prey => {
                // Look for nearest predator within flee range.
                let nearest_pred = snapshot.iter()
                    .filter(|(e, _, role)| *e != self_entity && *role == EcologyRole::Predator)
                    .min_by(|(_, pa, _), (_, pb, _)| {
                        let da = dist_sq(self_pos, *pa);
                        let db = dist_sq(self_pos, *pb);
                        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                    });

                if let Some(&(pred_entity, pred_pos, _)) = nearest_pred
                    && dist_sq(self_pos, pred_pos) <= FLEE_RANGE_SQ {
                        new_targets.insert(self_entity, (AnimalState::Fleeing, Some(pred_entity)));
                        continue;
                    }
                // Alternate resting/grazing every 100 ticks.
                let entity_bits = self_entity.to_bits();
                let state = if (tick.0.wrapping_add(entity_bits) % 100) < 80 {
                    AnimalState::Grazing
                } else {
                    AnimalState::Resting
                };
                new_targets.insert(self_entity, (state, None));
            }
            EcologyRole::Predator => {
                // Look for nearest prey within hunt range.
                let nearest_prey = snapshot.iter()
                    .filter(|(e, _, role)| *e != self_entity && *role == EcologyRole::Prey)
                    .min_by(|(_, pa, _), (_, pb, _)| {
                        let da = dist_sq(self_pos, *pa);
                        let db = dist_sq(self_pos, *pb);
                        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                    });

                if let Some(&(prey_entity, prey_pos, _)) = nearest_prey
                    && dist_sq(self_pos, prey_pos) <= HUNT_RANGE_SQ {
                        new_targets.insert(self_entity, (AnimalState::Hunting, Some(prey_entity)));
                        continue;
                    }
                new_targets.insert(self_entity, (AnimalState::Grazing, None));
            }
            EcologyRole::Scavenger => {
                new_targets.insert(self_entity, (AnimalState::Grazing, None));
            }
        }
    }

    // Build a quick position lookup for movement.
    let pos_lookup: HashMap<Entity, [f32; 2]> = snapshot.iter().map(|(e, p, _)| (*e, *p)).collect();

    // Apply new states and move toward/away targets.
    for (entity, mut pos, mut behavior) in &mut animals {
        if let Some((new_state, new_target)) = new_targets.remove(&entity) {
            behavior.state = new_state;
            behavior.target = new_target;
        }

        match &behavior.state {
            AnimalState::Fleeing => {
                if let Some(threat_entity) = behavior.target
                    && let Some(&threat_pos) = pos_lookup.get(&threat_entity) {
                        let dx = pos.x - threat_pos[0];
                        let dy = pos.y - threat_pos[1];
                        let len = (dx * dx + dy * dy).sqrt().max(0.001);
                        // Run away at double wander speed.
                        pos.x += (dx / len) * 0.4;
                        pos.y += (dy / len) * 0.4;
                    }
            }
            AnimalState::Hunting => {
                if let Some(prey_entity) = behavior.target
                    && let Some(&prey_pos) = pos_lookup.get(&prey_entity) {
                        let dx = prey_pos[0] - pos.x;
                        let dy = prey_pos[1] - pos.y;
                        let d_sq = dx * dx + dy * dy;
                        if d_sq > ATTACK_RANGE_SQ {
                            let len = d_sq.sqrt().max(0.001);
                            pos.x += (dx / len) * 0.35;
                            pos.y += (dy / len) * 0.35;
                        }
                        // Attack is handled below only when adjacent — health damage
                        // not applied here to avoid borrow conflicts; a dedicated
                        // combat system would handle that.
                    }
            }
            AnimalState::Grazing | AnimalState::Resting => {}
        }
    }
}

#[inline]
fn dist_sq(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

// ── #112: Tree growth lifecycle ───────────────────────────────────────────────

/// Advance growth for all flora entities, handle seed dispersal, and decay deadwood.
fn grow_flora(
    mut flora_query: Query<(Entity, &mut FloraEntity, &WorldPosition)>,
    map: Option<Res<WorldMap>>,
    tick: Res<WorldSimTick>,
    mut commands: Commands,
) {
    let mut seedlings_this_tick = 0usize;

    for (entity, mut flora, pos) in &mut flora_query {
        flora.state.age_ticks += 1;

        match flora.state.kind {
            FloraKind::DeadWood => {
                // DeadWood decays and is removed after DEADWOOD_DECAY_TICKS.
                if flora.state.age_ticks >= DEADWOOD_DECAY_TICKS {
                    commands.entity(entity).despawn();
                }
            }
            FloraKind::Tree | FloraKind::Shrub => {
                // Grow toward maturity.
                if flora.state.stage < 1.0 {
                    let rate = tree_growth_rate(flora.biome);
                    flora.state.stage = (flora.state.stage + rate).min(1.0);
                }

                // Mature tree seed dispersal.
                if flora.state.kind == FloraKind::Tree
                    && flora.state.stage >= 0.9
                    && seedlings_this_tick < MAX_SEEDLING_SPAWNS_PER_TICK
                    && tick.0.is_multiple_of(SEEDLING_COOLDOWN_TICKS)
                {
                    // Deterministic dispersal chance per entity per tick.
                    let entity_bits = entity.to_bits();
                    let roll = tick.0.wrapping_add(entity_bits) % SEED_DISPERSAL_CHANCE;
                    if roll == 0 {
                        // Pick an adjacent tile offset deterministically.
                        let offsets: [(f32, f32); 4] = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)];
                        let offset_idx = (tick.0.wrapping_mul(entity_bits) % 4) as usize;
                        let (ox, oy) = offsets[offset_idx];
                        let sx = pos.x + ox;
                        let sy = pos.y + oy;
                        let sz = map.as_ref()
                            .and_then(|m| smooth_surface_at(m, sx, sy, pos.z))
                            .unwrap_or(pos.z);

                        // Determine biome at spawn position.
                        let biome = map.as_ref()
                            .and_then(|m| m.column_at(sx, sy))
                            .and_then(|col| col.layers.iter().rev().find(|l| l.walkable))
                            .map(|l| l.kind)
                            .unwrap_or(TileKind::Plains);

                        commands.spawn((
                            WorldPosition { x: sx, y: sy, z: sz },
                            FloraEntity {
                                state: FloraState::new_seedling(),
                                biome,
                            },
                        ));
                        seedlings_this_tick += 1;
                    }
                }
            }
            FloraKind::Crop => {
                // Crops are managed by tick_farm_plots — skip here.
            }
        }
    }
}

// ── #113: Farm crop production cycles ────────────────────────────────────────

/// Advance all farm plot crop stages.  When a crop reaches 1.0 it is harvested
/// and its yield is added to the associated settlement's food supply.
fn tick_farm_plots(
    mut farm: ResMut<FarmState>,
    tick: Res<WorldSimTick>,
    mut story_writer: MessageWriter<WriteStoryEvent>,
) {
    let plot_keys: Vec<String> = farm.plots.keys().cloned().collect();
    for sid_str in plot_keys {
        let Some(plot) = farm.plots.get_mut(&sid_str) else { continue };
        let ready = plot.tick_growth();
        if ready {
            #[allow(clippy::cast_possible_truncation)]
            let current_tick = (tick.0 % u32::MAX as u64) as u32;
            let yield_val = plot.harvest(current_tick);
            *farm.food_supply.entry(sid_str.clone()).or_insert(0.0) += yield_val as f32;
            tracing::debug!(settlement = %sid_str, yield_val, "Farm harvested");

            story_writer.write(WriteStoryEvent(StoryEvent {
                id: Uuid::new_v4(),
                tick: tick.0,
                world_day: (tick.0 / 86_400).min(u32::MAX as u64) as u32,
                kind: StoryEventKind::FarmHarvested {
                    settlement_id: SmolStr::new(&sid_str),
                    yield_amount: yield_val,
                },
                participants: vec![],
                location: None,
                lore_tags: vec!["farm".into(), "harvest".into()],
            }));
        }
    }
}

// ── #115: Ranger NPC hunting system ──────────────────────────────────────────

/// Ranger-class NPCs actively hunt the nearest prey animal within their territory.
///
/// Each tick: find the nearest `WildlifeNpc` with `EcologyRole::Prey` within
/// `RANGER_HUNT_RANGE_SQ` tiles, then move toward it.
fn hunt_prey_ranger(
    mut rangers: Query<(&mut WorldPosition,), With<RangerNpc>>,
    prey_query: Query<&WorldPosition, (With<WildlifeNpc>, Without<RangerNpc>)>,
) {
    for (mut ranger_pos,) in &mut rangers {
        // Find nearest prey.
        let nearest = prey_query.iter()
            .filter(|prey_pos| {
                let dx = prey_pos.x - ranger_pos.x;
                let dy = prey_pos.y - ranger_pos.y;
                dx * dx + dy * dy <= RANGER_HUNT_RANGE_SQ
            })
            .min_by(|a, b| {
                let da = {
                    let dx = a.x - ranger_pos.x;
                    let dy = a.y - ranger_pos.y;
                    dx * dx + dy * dy
                };
                let db = {
                    let dx = b.x - ranger_pos.x;
                    let dy = b.y - ranger_pos.y;
                    dx * dx + dy * dy
                };
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });

        if let Some(target_pos) = nearest {
            let dx = target_pos.x - ranger_pos.x;
            let dy = target_pos.y - ranger_pos.y;
            let len = (dx * dx + dy * dy).sqrt().max(0.001);
            // Move at half wander speed toward target.
            ranger_pos.x += (dx / len) * 0.3;
            ranger_pos.y += (dy / len) * 0.3;
        }
    }
}

// ── DB persistence ────────────────────────────────────────────────────────────

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

// ── Helpers ───────────────────────────────────────────────────────────────────

// ── Issue #84: Pressure-driven cave carrying capacity reduction ───────────────

/// Apply underground pressure to reduce carrying capacity of cave ecology regions.
///
/// Each tick, cave regions' effective carrying capacity is reduced proportional
/// to underground pressure: pressure_factor = 1.0 - (pressure * 0.3).min(0.5).
/// At max pressure (1.0) this gives a 30% cap reduction (not exceeding 50%).
pub fn apply_pressure_to_cave_ecology(
    mut ecology: ResMut<EcologyState>,
    pressure: Res<crate::plugins::ai::UndergroundPressure>,
    map: Option<Res<WorldMap>>,
) {
    let Some(map) = map else { return };

    let pressure_factor = 1.0 - (pressure.score * 0.3).min(0.5) as f64;

    for region in ecology.regions.iter_mut() {
        // Only apply to cave regions (prefixed with "cave_").
        if !region.region.0.starts_with("cave_") {
            continue;
        }

        // Determine base carrying capacity from tile kind.
        // Parse region coordinates from the id to sample the map.
        let s = match region.region.0.strip_prefix("cave_") {
            Some(s) => s,
            None => continue,
        };
        let mut parts = s.splitn(2, '_');
        let rx: usize = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let ry: usize = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let cx = (rx * MACRO_REGION_SIZE + MACRO_REGION_SIZE / 2) as f32 - MAP_HALF_WIDTH as f32;
        let cy = (ry * MACRO_REGION_SIZE + MACRO_REGION_SIZE / 2) as f32 - MAP_HALF_WIDTH as f32;

        let Some(col) = map.column_at(cx, cy) else { continue };
        let cave_layer = col.layers.iter().find(|l| {
            matches!(l.kind, TileKind::CaveFloor | TileKind::CrystalCave | TileKind::LavaFloor)
        });
        let Some(cave_layer) = cave_layer else { continue };

        let base_k = cave_carrying_capacity(cave_layer.kind);
        region.k = base_k * pressure_factor;
    }
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
