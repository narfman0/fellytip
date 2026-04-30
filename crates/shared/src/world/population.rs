//! Deterministic settlement population simulation.
//!
//! Each world-sim tick (1 Hz) the server calls `tick_population` on every
//! `SettlementPopulation`. The function is pure — no RNG, no ECS — and uses a
//! fractional accumulator for births (same technique as Lotka-Volterra in
//! `ecology.rs`) so no dice are needed.
//!
//! # Economy (issue #107)
//!
//! Each settlement also carries a [`SettlementEconomy`] that tracks food supply,
//! consumption, gold, and trade income. Food surplus drives growth (+1 NPC every
//! 200 ticks); famine causes starvation (-1 NPC every 50 ticks). When population
//! reaches 0 the settlement is considered collapsed.

use crate::world::faction::FactionId;
use crate::world::map::{TileKind, WorldMap};
use crate::world::zone::WorldId;
use uuid::Uuid;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Ticks between births at a settlement (300 ticks = 5 real minutes at 1 Hz).
pub const BIRTH_PERIOD: u32 = 300;

/// Adults required before a war party can be dispatched.
pub const WAR_PARTY_THRESHOLD: u32 = 15;

/// Number of warriors pulled from the adult pool per war party.
pub const WAR_PARTY_SIZE: u32 = 10;

/// Tiles moved per world-sim tick (1 Hz) while marching.
pub const MARCH_SPEED: f32 = 2.0;

/// Distance in tiles at which a war party triggers a battle at the target.
pub const BATTLE_RADIUS: f32 = 3.0;

/// Ticks to wait after dispatching a war party before another can form.
/// 600 ticks = 10 real minutes.
pub const WAR_PARTY_COOLDOWN: u32 = 600;

/// Hard ceiling on NPCs per settlement (adults + children combined).
/// Prevents unbounded growth when war casualties don't keep up with births.
pub const MAX_SETTLEMENT_POP: u32 = 30;

/// Minimum military strength required before a war party can be dispatched.
pub const WAR_PARTY_MILITARY_MIN: f32 = 15.0;

/// Ticks between fed-population growth events (+1 NPC per surplus food tick).
pub const ECONOMY_GROWTH_PERIOD: u32 = 200;

/// Ticks between starvation events (-1 NPC per deficit food tick).
pub const ECONOMY_STARVE_PERIOD: u32 = 50;

/// Food consumed per NPC per tick.
pub const FOOD_CONSUMPTION_PER_NPC: f32 = 0.5;

/// Food produced per tick by a single hunter NPC.
pub const HUNTER_FOOD_PER_TICK: f32 = 0.8;

/// Trade income added to gold per tick per merchant NPC on a road.
pub const MERCHANT_TRADE_PER_TICK: f32 = 1.5;

/// Food bonus added per tick per farmer NPC working a farm tile.
pub const FARMER_FOOD_PER_TICK: f32 = 1.0;

// ── Economy state ─────────────────────────────────────────────────────────────

/// Per-settlement economic state (issue #107).
///
/// `food_supply` and `food_consumption` are updated each tick;
/// `gold` and `trade_income` track economic activity.
#[derive(Clone, Debug, Default)]
pub struct SettlementEconomy {
    /// Current food stockpile.
    pub food_supply: f32,
    /// Food consumed per tick (= population * FOOD_CONSUMPTION_PER_NPC).
    pub food_consumption: f32,
    /// Gold reserve.
    pub gold: f32,
    /// Trade income added to gold per tick (from merchants + trade routes).
    pub trade_income: f32,
    /// Ticks elapsed since last growth event (counted when fed).
    pub growth_ticks: u32,
    /// Ticks elapsed since last starvation event (counted when starving).
    pub starve_ticks: u32,
    /// Ticks an enemy war party has been besieging this settlement (issue #109).
    pub siege_ticks: u32,
}

// ── Economic role (issue #108) ────────────────────────────────────────────────

/// Economic role an NPC fills within its settlement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NpcRole {
    Farmer,
    Hunter,
    Merchant,
    Guard,
    Soldier,
    Idle,
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Per-settlement mutable population state (runtime, not world-gen output).
#[derive(Clone, Debug)]
pub struct SettlementPopulation {
    pub settlement_id: Uuid,
    pub faction_id: FactionId,
    /// Coordinate universe this settlement belongs to. Defaults to the surface world.
    pub world_id: WorldId,
    /// Ticks elapsed since the last birth.  Fires a spawn when it reaches `BIRTH_PERIOD`.
    pub birth_ticks: u32,
    /// Live adult NPC count (updated from ECS before calling `tick_population`).
    pub adult_count: u32,
    /// Live child NPC count (updated from ECS before calling `tick_population`).
    pub child_count: u32,
    /// Home position in world-space (used as spawn origin for children).
    pub home_x: f32,
    pub home_y: f32,
    pub home_z: f32,
    /// Ticks remaining before another war party can form (0 = ready).
    pub war_party_cooldown: u32,
    /// Mirror of `FactionResources.military_strength` — synced by the ECS caller
    /// before each `tick_population` call so the pure function can gate war parties.
    pub military_strength: f32,
    /// Settlement economic state (issue #107).
    pub economy: SettlementEconomy,
    /// Whether this settlement has been marked collapsed (population == 0).
    pub collapsed: bool,
    /// Tick at which this settlement was founded (for history tracking, issue #111).
    pub founded_tick: u64,
}

// ── Effects emitted by the tick ───────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum PopulationEffect {
    /// Server should spawn a child NPC near the given world position.
    SpawnChild {
        settlement_id: Uuid,
        x: f32,
        y: f32,
        z: f32,
    },
    /// Server should tag `WAR_PARTY_SIZE` adults as war-party members bound
    /// for `target_settlement_id` at world position `(tx, ty)`.
    FormWarParty {
        attacker_faction: FactionId,
        target_settlement_id: Uuid,
        tx: f32,
        ty: f32,
    },
    /// Economy-driven extra birth (food surplus, issue #107).
    EconomyGrowth { settlement_id: Uuid, x: f32, y: f32, z: f32 },
    /// Economy-driven NPC death from starvation (issue #107).
    Starvation { settlement_id: Uuid },
    /// Settlement population reached zero — it has collapsed (issue #111).
    SettlementCollapsed { settlement_id: Uuid, faction_id: FactionId, x: f32, y: f32, z: f32 },
}

// ── Pure tick function ────────────────────────────────────────────────────────

/// Advance one settlement's population by one tick.
///
/// `hostile_targets` — list of `(settlement_id, world_x, world_y, world_z)` belonging
/// to factions marked `Disposition::Hostile` toward this settlement's faction.
/// Supplied by the caller from ECS data so this function stays pure.
///
/// `map` — optional reference to the world map used to compute a cave growth
/// modifier for underground settlements. Pass `None` to skip the modifier.
///
/// `current_tick` — world-sim tick counter, used to seed economy growth/starvation
/// timing. Pass `0` if not relevant (e.g., in tests that don't exercise economy).
///
/// Returns the updated state and any effects to apply.
pub fn tick_population(
    mut state: SettlementPopulation,
    hostile_targets: &[(Uuid, f32, f32, f32)],
    map: Option<&WorldMap>,
) -> (SettlementPopulation, Vec<PopulationEffect>) {
    let mut effects = Vec::new();

    // ── Birth accumulation (integer counter — no floating-point drift) ────────
    // Underground settlements use a modified birth period based on cave biome.
    let effective_period = if state.home_z < 0.0 {
        if let Some(m) = map {
            let modifier = cave_growth_modifier(m, state.home_x, state.home_y);
            ((BIRTH_PERIOD as f32) / (1.0 + modifier)).round().max(1.0) as u32
        } else {
            BIRTH_PERIOD
        }
    } else {
        BIRTH_PERIOD
    };

    state.birth_ticks += 1;
    if state.birth_ticks >= effective_period {
        state.birth_ticks = 0;
        // Only spawn if the settlement is below its population cap.
        if state.adult_count + state.child_count < MAX_SETTLEMENT_POP {
            // Deterministic jitter without RNG: golden-ratio angular spread.
            let angle = 1.618_034_f32 * std::f32::consts::TAU;
            effects.push(PopulationEffect::SpawnChild {
                settlement_id: state.settlement_id,
                x: state.home_x + angle.cos(),
                y: state.home_y + angle.sin(),
                z: state.home_z,
            });
        }
    }

    // ── Economy tick (issue #107) ─────────────────────────────────────────────
    // Skip if already collapsed.
    if !state.collapsed {
        let total_pop = state.adult_count + state.child_count;
        // Siege doubles consumption (issue #109).
        let siege_multiplier = if state.economy.siege_ticks >= 5 { 2.0 } else { 1.0 };
        state.economy.food_consumption =
            total_pop as f32 * FOOD_CONSUMPTION_PER_NPC * siege_multiplier;

        // Apply food consumption.
        let net = state.economy.food_supply - state.economy.food_consumption;
        state.economy.food_supply = (state.economy.food_supply - state.economy.food_consumption).max(0.0);

        // Apply trade income.
        state.economy.gold += state.economy.trade_income;

        if net >= 0.0 && total_pop < MAX_SETTLEMENT_POP {
            // Fed: count toward a growth event (+1 NPC every ECONOMY_GROWTH_PERIOD fed ticks).
            state.economy.growth_ticks += 1;
            state.economy.starve_ticks = 0;
            if state.economy.growth_ticks >= ECONOMY_GROWTH_PERIOD {
                state.economy.growth_ticks = 0;
                effects.push(PopulationEffect::EconomyGrowth {
                    settlement_id: state.settlement_id,
                    x: state.home_x,
                    y: state.home_y,
                    z: state.home_z,
                });
            }
        } else if net < 0.0 {
            // Starving: count toward a starvation event (-1 NPC every ECONOMY_STARVE_PERIOD ticks).
            state.economy.starve_ticks += 1;
            state.economy.growth_ticks = 0;
            if state.economy.starve_ticks >= ECONOMY_STARVE_PERIOD {
                state.economy.starve_ticks = 0;
                if total_pop > 0 {
                    effects.push(PopulationEffect::Starvation { settlement_id: state.settlement_id });
                    // Decrement adult count immediately so war-party gate sees the hit.
                    state.adult_count = state.adult_count.saturating_sub(1);
                }
                // Check collapse condition (issue #111).
                let total_after = state.adult_count + state.child_count;
                if total_after == 0 {
                    state.collapsed = true;
                    effects.push(PopulationEffect::SettlementCollapsed {
                        settlement_id: state.settlement_id,
                        faction_id: state.faction_id.clone(),
                        x: state.home_x,
                        y: state.home_y,
                        z: state.home_z,
                    });
                }
            }
        }
    }

    // ── War party formation ───────────────────────────────────────────────────
    if state.war_party_cooldown > 0 {
        state.war_party_cooldown -= 1;
    } else if state.adult_count >= WAR_PARTY_THRESHOLD
        && state.military_strength >= WAR_PARTY_MILITARY_MIN
    {
        if let Some(&(target_id, tx, ty, _tz)) =
            nearest_target(state.home_x, state.home_y, state.home_z, hostile_targets)
        {
            effects.push(PopulationEffect::FormWarParty {
                attacker_faction: state.faction_id.clone(),
                target_settlement_id: target_id,
                tx,
                ty,
            });
            // Deduct war party from the adult count so the threshold isn't
            // re-triggered next tick before the ECS has a chance to mark them.
            state.adult_count = state.adult_count.saturating_sub(WAR_PARTY_SIZE);
            state.war_party_cooldown = WAR_PARTY_COOLDOWN;
        }
    }

    (state, effects)
}

/// Returns a growth rate modifier `[-0.03, +0.05]` based on adjacent cave tiles.
///
/// Samples a 3×3 neighbourhood around the settlement home tile (clamped to the
/// cave layer at depth 1). CrystalCave tiles add +5% each; LavaFloor tiles
/// subtract 3% each. The contributions are clamped to `[-0.2, +0.2]` to
/// prevent runaway values from dense biomes.
pub fn cave_growth_modifier(map: &WorldMap, hx: f32, hy: f32) -> f32 {
    let cx = hx as usize;
    let cy = hy as usize;
    let mut modifier = 0.0f32;
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if nx < 0 || ny < 0 || nx as usize >= map.width || ny as usize >= map.height {
                continue;
            }
            let col = &map.columns[nx as usize + ny as usize * map.width];
            for layer in &col.layers {
                if layer.z_top >= 0.0 {
                    continue;
                }
                match layer.kind {
                    TileKind::CrystalCave => modifier += 0.05,
                    TileKind::LavaFloor   => modifier -= 0.03,
                    _ => {}
                }
            }
        }
    }
    modifier.clamp(-0.2, 0.2)
}

fn nearest_target(
    hx: f32,
    hy: f32,
    hz: f32,
    targets: &[(Uuid, f32, f32, f32)],
) -> Option<&(Uuid, f32, f32, f32)> {
    targets.iter().min_by(|a, b| {
        let z_penalty_a = (a.3 - hz).abs() / 10.0;
        let z_penalty_b = (b.3 - hz).abs() / 10.0;
        let da = ((a.1 - hx).powi(2) + (a.2 - hy).powi(2)).sqrt() + z_penalty_a;
        let db = ((b.1 - hx).powi(2) + (b.2 - hy).powi(2)).sqrt() + z_penalty_b;
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::map::{TileColumn, TileLayer, TileKind};
    use crate::world::zone::WORLD_SURFACE;

    fn make_pop(adult_count: u32) -> SettlementPopulation {
        SettlementPopulation {
            settlement_id: Uuid::nil(),
            faction_id: FactionId("test".into()),
            world_id: WORLD_SURFACE,
            birth_ticks: 0,
            adult_count,
            child_count: 0,
            home_x: 0.0,
            home_y: 0.0,
            home_z: 0.0,
            war_party_cooldown: 0,
            military_strength: WAR_PARTY_MILITARY_MIN,
            economy: SettlementEconomy { food_supply: 9999.0, ..Default::default() },
            collapsed: false,
            founded_tick: 0,
        }
    }

    fn make_pop_underground(adult_count: u32) -> SettlementPopulation {
        SettlementPopulation {
            settlement_id: Uuid::nil(),
            faction_id: FactionId("test_cave".into()),
            world_id: WORLD_SURFACE,
            birth_ticks: 0,
            adult_count,
            child_count: 0,
            home_x: 5.0,
            home_y: 5.0,
            home_z: -10.0,
            war_party_cooldown: 0,
            military_strength: WAR_PARTY_MILITARY_MIN,
            economy: SettlementEconomy { food_supply: 9999.0, ..Default::default() },
            collapsed: false,
            founded_tick: 0,
        }
    }

    fn empty_map(width: usize, height: usize) -> WorldMap {
        WorldMap {
            columns: vec![TileColumn::default(); width * height],
            width,
            height,
            seed: 0,
            road_tiles: vec![false; width * height],
            spawn_points: Vec::new(),
            buildings_stamped: false,
        }
    }

    #[test]
    fn birth_acc_accumulates_deterministically() {
        let mut state = make_pop(3);
        let mut spawn_count = 0u32;
        for _ in 0..300 {
            let (next, effects) = tick_population(state, &[], None);
            state = next;
            spawn_count += effects.iter()
                .filter(|e| matches!(e, PopulationEffect::SpawnChild { .. }))
                .count() as u32;
        }
        assert_eq!(spawn_count, 1, "exactly one child born in 300 ticks");
    }

    #[test]
    fn birth_acc_remainder_carried() {
        let mut state = make_pop(3);
        for _ in 0..150 {
            let (next, effects) = tick_population(state, &[], None);
            state = next;
            assert!(
                effects.iter().all(|e| !matches!(e, PopulationEffect::SpawnChild { .. })),
                "no birth in first 150 ticks"
            );
        }
        // After 150 ticks the counter should be at 150 (halfway to BIRTH_PERIOD).
        assert_eq!(state.birth_ticks, 150);
    }

    #[test]
    fn war_party_threshold_at_exactly_15() {
        let target = (Uuid::new_v4(), 100.0f32, 100.0f32, 0.0f32);
        let state = make_pop(WAR_PARTY_THRESHOLD);
        let (_, effects) = tick_population(state, &[target], None);
        assert!(
            effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
            "war party formed at threshold"
        );
    }

    #[test]
    fn war_party_not_formed_below_threshold() {
        let target = (Uuid::new_v4(), 100.0f32, 100.0f32, 0.0f32);
        let state = make_pop(WAR_PARTY_THRESHOLD - 1);
        let (_, effects) = tick_population(state, &[target], None);
        assert!(
            !effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
            "war party should not form below threshold"
        );
    }

    #[test]
    fn war_party_requires_hostile_target() {
        // No targets: war party never forms even if adults >= threshold.
        let state = make_pop(WAR_PARTY_THRESHOLD + 5);
        let (_, effects) = tick_population(state, &[], None);
        assert!(
            !effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. }))
        );
    }

    #[test]
    fn war_party_cooldown_prevents_immediate_repeat() {
        let target = (Uuid::new_v4(), 100.0f32, 100.0f32, 0.0f32);
        let state = make_pop(WAR_PARTY_THRESHOLD + WAR_PARTY_SIZE); // enough for two
        let (state, effects) = tick_population(state, &[target], None);
        assert!(effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })));
        // Next tick: cooldown active, no second war party.
        let (_, effects2) = tick_population(state, &[target], None);
        assert!(!effects2.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })));
    }

    #[test]
    fn deterministic_same_input() {
        let state1 = make_pop(5);
        let state2 = make_pop(5);
        let (r1, e1) = tick_population(state1, &[], None);
        let (r2, e2) = tick_population(state2, &[], None);
        assert_eq!(r1.birth_ticks, r2.birth_ticks);
        assert_eq!(e1, e2);
    }

    #[test]
    fn birth_capped_at_max_pop() {
        // Settlement already at MAX_SETTLEMENT_POP: no births should occur.
        let mut state = make_pop(MAX_SETTLEMENT_POP / 2);
        state.child_count = MAX_SETTLEMENT_POP - state.adult_count;
        let mut spawn_count = 0u32;
        for _ in 0..300 {
            let (next, effects) = tick_population(state, &[], None);
            state = next;
            spawn_count += effects.iter()
                .filter(|e| matches!(e, PopulationEffect::SpawnChild { .. }))
                .count() as u32;
        }
        assert_eq!(spawn_count, 0, "no birth when at population cap");
    }

    #[test]
    fn birth_allowed_below_cap() {
        // One slot below the cap: exactly one birth in 300 ticks.
        let mut state = make_pop(MAX_SETTLEMENT_POP / 2);
        state.child_count = MAX_SETTLEMENT_POP - state.adult_count - 1;
        let mut spawn_count = 0u32;
        for _ in 0..300 {
            let (next, effects) = tick_population(state, &[], None);
            state = next;
            spawn_count += effects.iter()
                .filter(|e| matches!(e, PopulationEffect::SpawnChild { .. }))
                .count() as u32;
        }
        assert_eq!(spawn_count, 1, "exactly one birth when one slot below cap");
    }

    #[test]
    fn war_party_requires_military_strength() {
        let target = (Uuid::new_v4(), 100.0f32, 100.0f32, 0.0f32);

        // Below military threshold: no war party.
        let mut low_mil = make_pop(WAR_PARTY_THRESHOLD);
        low_mil.military_strength = WAR_PARTY_MILITARY_MIN - 1.0;
        let (_, effects) = tick_population(low_mil, &[target], None);
        assert!(
            !effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
            "war party should not form below military threshold"
        );

        // At military threshold: war party forms.
        let high_mil = make_pop(WAR_PARTY_THRESHOLD);
        let (_, effects) = tick_population(high_mil, &[target], None);
        assert!(
            effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
            "war party should form at military threshold"
        );
    }

    #[test]
    fn underground_population_grows() {
        let mut state = make_pop_underground(3);
        let mut spawn_count = 0u32;
        for _ in 0..300 {
            let (next, effects) = tick_population(state, &[], None);
            state = next;
            spawn_count += effects.iter()
                .filter(|e| matches!(e, PopulationEffect::SpawnChild { .. }))
                .count() as u32;
        }
        assert!(spawn_count >= 1, "underground settlement should produce at least one birth in 300 ticks");
    }

    #[test]
    fn crystal_cave_gives_growth_bonus() {
        let mut map = empty_map(16, 16);
        let z = crate::world::cave::cave_z(1);
        let cx = 5usize;
        let cy = 5usize;
        // Place CrystalCave tile at the settlement position.
        let idx = cx + cy * map.width;
        map.columns[idx].layers.push(TileLayer {
            z_base: z - 0.5,
            z_top: z,
            kind: TileKind::CrystalCave,
            walkable: true,
            corner_offsets: [0.0; 4],
        });
        let modifier = cave_growth_modifier(&map, cx as f32 + 0.5, cy as f32 + 0.5);
        assert!(modifier > 0.0, "CrystalCave adjacent tile should give positive growth modifier, got {modifier}");
    }

    #[test]
    fn underground_civ_can_raid_surface() {
        // Underground settlement at z=-10, surface target at z=0.
        let mut state = make_pop_underground(WAR_PARTY_THRESHOLD + WAR_PARTY_SIZE);
        state.home_x = 50.0;
        state.home_y = 50.0;
        // Surface target: nearby in x/y but different z layer.
        let surface_target = (Uuid::new_v4(), 60.0f32, 60.0f32, 0.0f32);
        let (_, effects) = tick_population(state, &[surface_target], None);
        assert!(
            effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
            "underground civ should be able to raid surface settlement"
        );
    }

    // ── Economy tests (issue #107) ─────────────────────────────────────────────

    #[test]
    fn economy_growth_fires_after_200_fed_ticks() {
        let mut state = make_pop(5);
        // Plenty of food — should never starve, growth event every 200 ticks.
        state.economy.food_supply = 99999.0;
        let mut growth_count = 0u32;
        for _ in 0..200 {
            let (next, effects) = tick_population(state, &[], None);
            state = next;
            growth_count += effects.iter()
                .filter(|e| matches!(e, PopulationEffect::EconomyGrowth { .. }))
                .count() as u32;
        }
        assert_eq!(growth_count, 1, "exactly one economy growth event in 200 fed ticks");
    }

    #[test]
    fn starvation_fires_after_50_deficit_ticks() {
        let mut state = make_pop(5);
        // No food supply at all — starvation starts immediately.
        state.economy.food_supply = 0.0;
        let mut starve_count = 0u32;
        for _ in 0..50 {
            let (next, effects) = tick_population(state, &[], None);
            state = next;
            starve_count += effects.iter()
                .filter(|e| matches!(e, PopulationEffect::Starvation { .. }))
                .count() as u32;
        }
        assert_eq!(starve_count, 1, "exactly one starvation event in 50 deficit ticks");
    }

    #[test]
    fn settlement_collapses_when_all_starve() {
        // One adult, no food — should collapse after 50 starvation ticks.
        let mut state = make_pop(1);
        state.economy.food_supply = 0.0;
        let mut collapsed = false;
        for _ in 0..100 {
            let (next, effects) = tick_population(state, &[], None);
            state = next;
            if effects.iter().any(|e| matches!(e, PopulationEffect::SettlementCollapsed { .. })) {
                collapsed = true;
                break;
            }
        }
        assert!(collapsed, "settlement with 1 adult and no food should collapse");
        assert!(state.collapsed, "collapsed flag should be set");
    }

    #[test]
    fn siege_doubles_consumption() {
        let mut state = make_pop(4);
        state.economy.siege_ticks = 5;
        state.economy.food_supply = 100.0;
        // One tick: consumption = 4 adults * 0.5 * 2.0 = 4.0
        let (next, _) = tick_population(state, &[], None);
        assert!(
            (next.economy.food_consumption - 4.0).abs() < 0.01,
            "siege should double consumption: expected 4.0, got {}", next.economy.food_consumption
        );
    }

    #[test]
    fn food_surplus_does_not_grow_beyond_cap() {
        let mut state = make_pop(MAX_SETTLEMENT_POP);
        state.economy.food_supply = 99999.0;
        let mut growth_count = 0u32;
        for _ in 0..200 {
            let (next, effects) = tick_population(state, &[], None);
            state = next;
            growth_count += effects.iter()
                .filter(|e| matches!(e, PopulationEffect::EconomyGrowth { .. }))
                .count() as u32;
        }
        assert_eq!(growth_count, 0, "no economy growth when already at MAX_SETTLEMENT_POP");
    }
}
