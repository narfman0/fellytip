# Fellytip — Claude Guide

## What this project is

Multiplayer action RPG in Rust/Bevy. The world simulates itself independently of player presence. Pure simulation logic lives in `crates/shared`; Bevy ECS is a thin bridge.

See `docs/` for product documentation:
- `docs/requirements.md` — what the game must do
- `docs/architecture.md` — crate layout, design constraints, data flow
- `docs/milestones.md` — milestone definitions and status
- `docs/systems/` — one file per major system (world-map, combat, civilization, world-sim, networking, persistence, rendering)

## Crate map

| Crate | Role |
|---|---|
| `crates/shared` | Pure types, protocol, combat rules, world gen — no ECS, no I/O |
| `crates/server` | Bevy server: lightyear, WorldSimSchedule (1 Hz), AI, persistence, map gen |
| `crates/client` | Bevy client: lightyear, rendering, egui HUD, input |
| `tools/combat_sim` | proptest harness — runs combat rules with no ECS |
| `tools/ralph` | BRP HTTP test driver — asserts live world state via JSON-RPC |
| `tools/world_gen` | ASCII world preview: `cargo run -p world_gen -- --seed N` |

## Non-negotiable architecture rules

- **Pure rules, thin bridge.** Combat and world-sim logic goes in `crates/shared` as `fn(State) -> (State, Vec<Effect>)`. ECS systems only snapshot state → call rules → apply effects.
- **Never roll dice inside rules.** Always inject `rng: &mut impl Iterator<Item=i32>` so proptest can drive deterministic traces.
- **No wildcard `_` in interrupt stack `match`.** Every `InterruptFrame` variant must be handled explicitly — this is a lint-level guarantee against silent fallthrough bugs.
- **Two tick rates.** `FixedUpdate` at 62.5 Hz (combat/movement). `WorldSimSchedule` custom schedule at 1 Hz (factions/ecology/story). Never cross-schedule without a documented reason.
- **Isometric stays behind a feature flag.** Only `sync_transform` changes between `topdown` (default) and `isometric` features. Simulation and networking are untouched.
- **World gen is pure and deterministic.** `generate_map(seed)` and `generate_settlements(map, seed)` are pure functions in `crates/shared` — no ECS, no I/O. Same seed always produces the same world. The server calls them on startup via `MapGenPlugin`.
- **No circular module deps in world gen.** `world/civilization.rs` may import from `world/map.rs`. `world/map.rs` must NOT import from `civilization.rs`. Settlement generation happens after `generate_map` returns.

## Key version pins (do not bump without checking compatibility)

- `bevy = "0.18"`, `lightyear = "0.26.4"`, `sqlx = "0.8"`, `bevy_egui = "0.39"`, `bevy-inspector-egui = "0.36"`

## Testing & verification

```bash
cargo test --workspace                 # 58 tests total (fast, no I/O)
cargo test -p fellytip-shared          # pure logic: map gen, biomes, civilization, combat
cargo test -p combat_sim               # 100k+ proptest traces
cargo clippy --workspace -- -D warnings
cargo run -p ralph -- --scenario all   # live end-to-end via BRP
cargo run -p world_gen -- --seed 42    # ASCII world preview (sanity check)
```

Run `cargo clippy` before considering any task done.

## Ralph loop (automated feedback)

Server BRP on port **15702**, headless client on **15703**. Launch order:

```bash
cargo run -p fellytip-server &
cargo run -p fellytip-client -- --headless &
cargo run -p ralph -- --scenario all
```

Ralph scenarios are the acceptance criteria for each milestone. A scenario passing = milestone shipped.

## Milestones (current target: work top-to-bottom)

| # | Done when |
|---|---|
| **0 – Bones** | Server + client connect; `WorldPosition` replicated; WASD moves sprite |
| **0b – Ralph** | ralph `basic_movement` scenario passes via BRP |
| **1 – Living World** | Factions, ecology, story log in egui; world survives restart |
| **2 – First Blood** | Player attacks NPC; NPC death → story log; proptest green |
| **3 – Party Play** | 4 simultaneous clients; party HUD; visibility culling |
| **4 – MVF** | 3 classes, 1 dungeon, faction consequences; 2-hour session stable |

## Implementation order

Follow the milestone sequence in `docs/milestones.md`. Each milestone's acceptance criteria define what "done" means. System docs in `docs/systems/` describe the current implementation.

## Style

- Prefer `thiserror` for error types, `anyhow` at call sites / main.
- Use `SmolStr` for interned string identifiers (faction names, lore tags, sprite keys).
- `GameEntityId(Uuid)` is the stable cross-session identity; Bevy `Entity` is ephemeral.
- Replicated components go in `crates/shared/src/components.rs` and must be registered in `FellytipProtocolPlugin`.
- Story events are emitted as Bevy events (`WriteStoryEvent`), collected by `story_writer`, and flushed to SQLite every 5 minutes.
- `WorldPosition` has three fields `{x, y, z}` — always include all three when constructing it.
- `TileKind` has 20 variants (5 legacy surface + 11 Whittaker biomes + River + 4 underground). When matching exhaustively include all or use a `_` only after documenting why.
- `WorldMap` is NOT replicated to clients (512×512 = too large). Clients get entity positions only; tile rendering samples locally on region-load.
- `generate_map` runs the full pipeline: fBm surface → moisture → biome classification → rivers → shallow caves → Underdark → shafts. Call order in server: `generate_map` → `generate_settlements` → `generate_roads` (mutates map).
- `Settlements` is a Bevy `Resource` (wraps `Vec<Settlement>`) inserted by `MapGenPlugin`.

## Change workflow

For every atomic change, follow this sequence before moving on:

1. **Write unit tests** for any new pure logic (functions in `crates/shared`).
2. **Pass all tests** — `cargo test --workspace` must be green.
3. **Pass clippy** — `cargo clippy --workspace -- -D warnings` must be clean.
4. **Smoke test** — build both binaries (`cargo build --workspace`); run ralph if the change touches networking or combat.
5. **Update docs** — edit the relevant file in `docs/systems/` to reflect the current behaviour.
6. **Commit** with a descriptive message.
7. **Push** to origin.

Never leave a step half-done and move to the next feature. Small, complete, verified increments.

## Do

- **Write unit tests for all pure logic.**
- **Use `#[derive(Reflect)]` on new components and resources** so they appear in the ECS inspector.
- **One system per concern.** Prefer Bevy ECS patterns: components, resources, events, schedules.

## Don't

- **Don't use `cd` in bash commands.** Always pass the path explicitly.
- **For git, ALWAYS use `git -C <project_path> <command>`.** Never `cd` first.
- **Don't leave dead code or unused imports.**