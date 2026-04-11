# Fellytip — Architecture

## Crate layout

| Crate | Responsibility |
|---|---|
| `crates/shared` | Pure types, combat rules, world generation, protocol — no ECS, no I/O |
| `crates/server` | Bevy ECS server: networking, world sim scheduling, persistence, map gen |
| `crates/client` | Bevy ECS client: networking, rendering, input |
| `tools/ralph` | BRP HTTP test driver — automated end-to-end scenario assertions |
| `tools/combat_sim` | proptest harness for combat and ecology rules, no ECS |
| `tools/world_gen` | Standalone ASCII world preview: `cargo run -p world_gen -- --seed N` |

## Design constraints

### Pure simulation, thin ECS bridge

All game logic lives in `crates/shared` as ordinary Rust functions: `fn(State) -> (State, Vec<Effect>)`. No Bevy types, no async, no I/O. ECS systems in `crates/server` snapshot component data into pure types, call the shared functions, and apply the returned effects back to the ECS world.

This constraint means:
- Logic is unit-testable without Bevy running.
- proptest can feed arbitrary inputs to combat and ecology.
- World generation can run in a standalone CLI tool.

### Dice injection at the boundary

Randomness is never generated inside pure logic. Every function that needs a random value takes `rng: &mut impl Iterator<Item=i32>` and reads from it. The ECS bridge feeds real dice; test harnesses feed deterministic values. This applies to combat (attack rolls, damage) and world generation (fBm is deterministic from seed; only procedural cave/shaft placement uses an RNG passed in).

### Two tick rates

| Schedule | Rate | What runs |
|---|---|---|
| `FixedUpdate` | 62.5 Hz | Combat resolution, player movement, input application |
| `WorldSimSchedule` | 1 Hz | Faction AI, ecology, story event flush |

These schedules never share mutable state during a tick. If a system needs to cross the boundary it must document why.

### Exhaustive interrupt-stack matching

The `InterruptFrame` enum must be matched exhaustively — no `_` wildcard. This is enforced by convention and code review. Silent fallthrough bugs in combat reactions are a class of bug this eliminates.

## Data flow

```
Client keyboard input
  → PlayerInput message (UDP, unreliable)
  → Server MessageReceiver<PlayerInput>
  → process_player_input (FixedUpdate)
      → moves WorldPosition
      → queues PendingAttack if BasicAttack
  → initiate_attacks
      → pushes InterruptFrame onto attacker's InterruptStack
  → resolve_interrupts
      → steps each stack (pure: InterruptStack::step)
      → applies Vec<Effect> to Health components
      → awards XP, emits WriteStoryEvent, despawns dead entities
  → Replication (50 ms interval)
      → WorldPosition + Health replicated to all clients
```

```
World sim tick (1 Hz)
  → EcologyPlugin  → Lotka-Volterra per region → emits StoryEvents on collapse
  → AiPlugin       → faction utility scoring → updates faction goals
  → StoryPlugin    → flushes WriteStoryEvent messages → StoryLog resource + SQLite
```

## Key version pins

| Dependency | Version | Note |
|---|---|---|
| `bevy` | 0.18 | Do not bump without checking Lightyear compatibility |
| `lightyear` | 0.26.4 | Targets Bevy 0.18 specifically |
| `sqlx` | 0.8 | 0.9 is alpha; stay on 0.8 |
| `bevy_egui` | 0.39 | |
| `bevy-inspector-egui` | 0.36 | Behind `debug` feature flag |
| `rand` / `rand_chacha` | 0.10 / 0.10 | RngExt trait for `.random::<T>()` |

## Coordinate system

- `WorldPosition { x, y, z }` — `x` and `y` are tile-space coordinates (1 unit = 1 tile). `z` is elevation in world units (0 = sea level, positive = above ground, negative = underground).
- `TILE_W = 32`, `TILE_H = 16` pixels — used only in rendering projection.
- Top-down projection: `(x * TILE_W, y * TILE_H)`.
- Isometric projection: `((x - y) * TILE_W/2, (x + y) * TILE_H/4 + z * TILE_H/2)` — available via `iso_project()` in `crates/shared/src/math.rs`, gated behind `isometric` feature flag.

## Entity identity

- `Bevy Entity` — ephemeral, local to one server session.
- `GameEntityId(Uuid)` — stable cross-session identity stored in SQLite. Used for story events and persistence.
- `CombatantId(Uuid)` — identifies a combatant within the interrupt stack (can be player or NPC).
