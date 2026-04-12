# System: World Simulation

The world simulation runs independently of player presence. Two separate schedules tick at different rates; neither waits for the other.

Exact tick rates and timing constants are defined in `crates/shared/src/lib.rs` (`TICK_HZ`) and `crates/server/src/plugins/world_sim.rs`.

## Tick rates

| Schedule | Rate | Systems |
|---|---|---|
| `FixedUpdate` | `TICK_HZ` (see `shared/src/lib.rs`) | Player input, movement, combat resolution |
| `WorldSimSchedule` | 1 Hz (real time) | Faction AI, ecology, story flush |

`WorldSimSchedule` is a custom Bevy schedule driven by a 1-second repeating timer in `Update`. It is not a `FixedUpdate` variant — it fires based on wall-clock time, not simulation ticks.

`WorldSimTick` is a monotonic counter incremented each time `WorldSimSchedule` fires. It is used as a timestamp for story events and as the basis for autosave intervals.

## History warp

On server startup, `MapGenPlugin` runs `WorldSimSchedule` synchronously for `HISTORY_WARP_TICKS` iterations (see `crates/server/src/plugins/map_gen.rs`) before accepting network connections. This pre-ages the world: factions have had time to expand and compete, ecology populations have stabilised or collapsed, and story events have accumulated. Players join a world with history, not a blank slate.

## Ecology (`world/ecology.rs`)

Each region tracks predator and prey population counts. Each world-sim tick, populations update using a discrete Lotka-Volterra model:

```
new_prey     = prey × (1 + r × (1 − prey/K)) − α × predator × prey
new_predator = predator × (β × α × prey − δ)
```

The model coefficients (`r`, `K`, `α`, `β`, `δ`) and the collapse threshold are defined in `ecology.rs`.

When a prey population falls below the collapse threshold, a `StoryEvent::EcologyCollapse` is emitted. This propagates to faction AI: food-scarce factions re-evaluate their goals and may shift toward raiding.

Populations are clamped to non-negative values. The model is pure — the ECS bridge passes current counts in and receives updated counts back.

## Faction AI (`world/faction.rs`)

Each faction holds a set of goals, scored by a utility function each world-sim tick. The goal with the highest utility becomes active. Goals include:

- **ExpandTerritory** — claim adjacent regions; scores high when food is plentiful and military is strong
- **DefendSettlement** — fortify when threatened; scores high when a settlement is contested
- **RaidResource** — send raiding parties; scores high when food is scarce and military is moderate
- **FormAlliance** — seek alliances; scores high when outnumbered by enemies
- **Survive** — fallback; always scores above 0

At equilibrium, a well-resourced faction expands; a food-stressed faction raids; a weak faction seeks alliances. Players can shift these balances by killing NPCs, depleting prey populations, or destroying settlements.

NPC entities wander toward their faction's active goal each world-sim tick. Individual NPC pathfinding runs in `FixedUpdate`.

## Settlement population (`world/population.rs` + `plugins/ai.rs`)

Each settlement has a `SettlementPopulation` entry in the `FactionPopulationState` resource. One `tick_population` call per settlement per world-sim tick drives:

- **Birth** — an integer tick counter (`birth_ticks`) advances by 1 each tick. When it reaches `BIRTH_PERIOD = 300` (5 minutes), a child NPC is spawned with `GrowthStage(0.0)` and `Health { current: 5, max: 5 }`. The counter resets to 0.
- **Growth** — `age_npcs_system` increments `GrowthStage` by `1/300` per tick. When `GrowthStage` crosses 1.0, health is upgraded to adult values (`max: 20`).
- **War party** — when `adult_count >= WAR_PARTY_THRESHOLD (15)` and the cooldown is zero, a `FormWarPartyEvent` is emitted. `check_war_party_formation` picks up to `WAR_PARTY_SIZE (10)` adults and tags them with `WarPartyMember`. A `WAR_PARTY_COOLDOWN (600)` tick pause prevents immediate re-formation.

Targeting is pure: `tick_population` receives a pre-filtered `hostile_targets: &[(Uuid, f32, f32)]` list from the caller, derived from `FactionRegistry` disposition maps.

## War parties and battles (`plugins/ai.rs`)

`march_war_parties` advances each `WarPartyMember` NPC `MARCH_SPEED (2.0)` tiles per world-sim tick toward its target settlement. When the first warrior arrives within `BATTLE_RADIUS (3.0)` tiles and no `ActiveBattle` entity exists for that settlement, an `ActiveBattle` is spawned and `BattleStartMsg` is broadcast to all clients via `CombatEventChannel`.

`run_battle_rounds` fires once per active battle per tick:
- Combatants within `BATTLE_RADIUS` of the battle site are collected as snapshots.
- Dice are drawn from `seeded_dice(settlement_id, tick)` — deterministic `ChaCha8Rng` seeded on `settlement_id XOR tick`.
- `tick_battle_round` wraps `resolve_round()` for each attacker-defender pair. `BattleAttackMsg` is sent per hit.
- When one side is eliminated, `ActiveBattle` is despawned, `BattleEndMsg` is broadcast, and the losing faction's `military_strength` is reduced.

## Faction NPC spawning (`plugins/ai.rs`)

On server startup — after `MapGenPlugin` inserts `Settlements` — `spawn_faction_npcs` runs once and creates three guard NPCs per faction at their assigned home settlement. Each faction is mapped to a settlement by index (`faction_idx % settlements.len()`), so factions distribute evenly across available settlements.

Each NPC spawns with:
- `WorldPosition` at the settlement centre, offset by a fixed tile-unit delta so they are not stacked
- `Health { current: 20, max: 20 }`
- `CombatParticipant` — Warrior, level 1, AC 11 (leather), STR 10, DEX 10, CON 10
- `ExperienceReward(50)` — CR 1/4 per the SRD CR→XP table
- `FactionMember`, `FactionNpcRank(Grunt)`, `CurrentGoal(None)`, `HomePosition` components

Aggression checks run at `FixedUpdate` (62.5 Hz) in `check_faction_aggression` (in `combat.rs`), not at the 1 Hz world-sim rate. This ensures NPC reactions are frame-accurate. See `docs/systems/factions.md` for full aggression rules.

NPCs are stationary until pathfinding is implemented. The `wander_npcs` WorldSimSchedule system is a placeholder that does nothing; it will be replaced with goal-directed movement in a later milestone.

## Wildlife entity spawning (`plugins/ecology.rs`)

`seed_ecology` runs once at startup (registered in `MapGenPlugin`'s Startup chain, between `generate_world` and `history_warp`). It divides the 512×512 tile map into a 4×4 grid of macro-regions and assigns Lotka-Volterra parameters to each based on its dominant biome:

| Biome group | prey_start | pred_start | r | K |
|---|---|---|---|---|
| Temperate forest, Grassland, Plains, Savanna | 100 | 20 | 0.5 | 200 |
| Tropical forest / Rainforest | 80 | 18 | 0.5 | 180 |
| Taiga (boreal) | 60 | 12 | 0.4 | 120 |
| Stone | 40 | 8 | 0.4 | 80 |
| Desert / Tundra / Polar | 20 | 4 | 0.3 | 50 |
| Water / Mountain / Underground | skipped | — | — | — |

After history warp runs, `sync_wildlife_entities` executes each WorldSimSchedule tick. It maintains a live pool of `WildlifeNpc` entities whose count tracks the simulated predator population:

- Desired entity count = `floor(predator.count / 20)` per region
- Spawning is capped at `MAX_SPAWNS_PER_TICK = 5` new entities per tick to prevent history-warp spikes
- When predator population falls below `SPAWN_THRESHOLD = 10.0`, all wildlife in that region are despawned

Each wildlife NPC has:
- `WildlifeNpc { region }` to track its home macro-region
- `WorldPosition` at the region's tile-space centre, z from `smooth_surface_at`
- `Health { current: 15, max: 15 }`
- `CombatParticipant` — Rogue, level 1, AC 10, STR 8, DEX 12, CON 10
- `ExperienceReward(25)` — CR 1/8 per the SRD CR→XP table

## Story log (`world/story.rs`)

Any server system can emit a `WriteStoryEvent` Bevy message. The story plugin collects these and appends them to the `StoryLog` resource — an in-memory ordered list of `StoryEvent` values indexed by lore tags.

A `StoryEvent` records:
- Unique ID and world-sim tick timestamp
- A `StoryEventKind` variant (faction war declared, ecology collapse, player killed named NPC, etc.)
- Participant `GameEntityId` list
- Optional tile-space location
- Lore tags for filtering (e.g. `"death"`, `"faction"`, `"ecology"`)

The log is flushed to SQLite periodically (autosave not yet wired; stub in place). It will be streamed to clients as UI events once the egui HUD is implemented.

Story events feed back into faction AI: a named NPC death may cause the victim's faction to spawn a `DefendSettlement` or retaliation goal.
