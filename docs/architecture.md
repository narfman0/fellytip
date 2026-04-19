# Fellytip — Architecture

## Crate layout

| Crate | Responsibility |
|---|---|
| `crates/shared` | Pure types, combat rules, world generation, protocol — no ECS, no I/O |
| `crates/server` | Bevy ECS game logic lib: world sim, AI, persistence, combat, map gen. No binary. |
| `crates/client` | Single runnable binary: links server lib, adds rendering, input, egui HUD |
| `tools/ralph` | BRP HTTP test driver — automated end-to-end scenario assertions |
| `tools/combat_sim` | proptest harness for combat and ecology rules, no ECS |
| `tools/world_gen` | Standalone ASCII world preview: `cargo run -p world_gen -- --seed N` |
| `tools/worldwatch` | eframe desktop monitor: reads BRP + SQLite and displays live world state |

`crates/server` is a **lib-only** crate. There is no separate server process. All plugins (`WorldSimPlugin`, `AiPlugin`, `CombatPlugin`, etc.) run in-process inside the single `fellytip-client` binary. The crate boundary is preserved so a true server binary can be re-introduced behind a `multiplayer` feature flag later without major surgery.

## Design constraints

### Pure simulation, thin ECS bridge

All game logic lives in `crates/shared` as ordinary Rust functions: `fn(State) -> (State, Vec<Effect>)`. No Bevy types, no async, no I/O. ECS systems in `crates/server` snapshot component data into pure types, call the shared functions, and apply the returned effects back to the ECS world.

This constraint means:
- Logic is unit-testable without Bevy running.
- proptest can feed arbitrary inputs to combat and ecology.
- World generation can run in a standalone CLI tool.

### Dice injection at the boundary

Randomness is never generated inside pure logic. Every function that needs a random value takes `rng: &mut impl Iterator<Item=i32>` and reads from it. The ECS bridge feeds real dice; test harnesses feed deterministic values. This applies to combat (attack rolls, damage) and faction war battles (`seeded_dice` in `world/war.rs` uses `ChaCha8Rng` keyed on settlement ID + tick).

### Two tick rates

| Schedule | Rate | What runs |
|---|---|---|
| `FixedUpdate` | 62.5 Hz | Combat resolution, player movement, input application |
| `WorldSimSchedule` | 1 Hz | Faction AI, population, ecology, war parties, story event flush |

These schedules never share mutable state during a tick. If a system needs to cross the boundary it must document why.

### Exhaustive interrupt-stack matching

The `InterruptFrame` enum must be matched exhaustively — no `_` wildcard. This is enforced by convention and code review. Silent fallthrough bugs in combat reactions are a class of bug this eliminates.

## Data flow

```
Keyboard input (client Update, 60+ Hz)
  → send_player_input
      → moves PredictedPosition (client-side, zero-latency)
      → writes ActionIntent to LocalPlayerInput resource
  → sync_pred_to_world → copies PredictedPosition → WorldPosition (same frame)

WorldPosition update → process_player_input (FixedUpdate, 62.5 Hz)
  → reads LocalPlayerInput resource
  → queues PendingAttack if BasicAttack intent present
  → initiate_attacks
      → pushes InterruptFrame onto attacker's InterruptStack
  → resolve_interrupts
      → steps each stack (pure: InterruptStack::step)
      → applies Vec<Effect> to Health components
      → awards XP, emits WriteStoryEvent, despawns dead entities
```

```
World sim tick (1 Hz)
  → update_chunk_temperature → ChunkTemperature Hot/Warm zones (single player)
  → update_faction_goals     → utility scoring → active FactionGoal
  → tick_population_system   → birth counters, war party formation events
  → age_npcs_system          → GrowthStage += 1/300; adult health upgrade
  → check_war_party_formation→ tags adults as WarPartyMember
  → march_war_parties        → moves warriors; writes BattleStartMsg
  → run_battle_rounds        → seeded combat; writes BattleAttackMsg / BattleEndMsg
  → wander_npcs              → placeholder (non-war-party guards stationary)
  → EcologyPlugin            → Lotka-Volterra per region; StoryEvents on collapse
  → StoryPlugin              → collect_story_events: WriteStoryEvent → StoryLog + StoryMsg
                             → flush_story_log (every 300 ticks): StoryLog → SQLite
```

Messages (`BattleStartMsg`, `BattleEndMsg`, `BattleAttackMsg`, `StoryMsg`) flow through Bevy's native `MessageWriter` / `MessageReader` within the same process — no network hop.

## Key version pins

| Dependency | Version | Note |
|---|---|---|
| `bevy` | 0.18 | |
| `sqlx` | 0.8 | 0.9 is alpha; stay on 0.8 |
| `bevy_egui` | 0.39 | |
| `bevy-inspector-egui` | 0.36 | Behind `debug` feature flag |
| `rand` / `rand_chacha` | 0.10 / 0.10 | RngExt trait for `.random::<T>()` |

## Coordinate system

- `WorldPosition { x, y, z }` — `x` and `y` are tile-space coordinates (1 unit = 1 tile). `z` is elevation in world units (0 = sea level, positive = above ground).
- Bevy render space: world `(x, y, z_elevation)` → Bevy `(x, z_elevation, y)`. Bevy is Y-up; the world's Z elevation becomes Bevy's Y.
- Chunk coordinates: `chunk = ((tile_x) / CHUNK_TILES, (tile_y) / CHUNK_TILES)` where `tile_x = pos.x + MAP_HALF_WIDTH`.

## Entity identity

- `Bevy Entity` — ephemeral, local to one server session.
- `GameEntityId(Uuid)` — stable cross-session identity stored in SQLite. Used for story events and persistence. Player entities carry this as a `Component`; the invariant `CombatantId.0 == GameEntityId.0` holds for all player entities.
- `CombatantId(Uuid)` — identifies a combatant within the interrupt stack (can be player or NPC).

## Key server resources

| Resource | Description |
|---|---|
| `WorldMap` | Generated tile grid; not replicated to clients |
| `Settlements` | List of generated settlements; used for NPC spawn placement |
| `FactionRegistry` | All live `Faction` structs (including disposition maps); mutated by world-sim AI |
| `FactionPopulationState` | Per-settlement `SettlementPopulation` (birth ticks, adult/child counts, cooldowns) |
| `ChunkTemperature` | Hot/Warm zone chunk sets around the local player; rebuilt every WorldSim tick |
| `PlayerReputationMap` | Per-player, per-faction standing scores (`HashMap<Uuid, HashMap<FactionId, i32>>`); clamped to `[-999, 1000]`; persisted to `player_faction_standing` SQLite table |
| `EcologyState` | Per-region predator/prey population counts |
| `StoryLog` | In-memory ordered event log; flushed to SQLite periodically |
| `WorldSimTick` | Monotonic counter incremented each 1 Hz world-sim tick |
