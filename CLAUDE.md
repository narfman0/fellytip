# Fellytip — Claude Guide

## What this project is

Multiplayer action RPG in Rust/Bevy. The world simulates itself independently of player presence. Pure simulation logic lives in `crates/shared`; Bevy ECS is a thin bridge.

See `docs/` for product documentation:
- `docs/requirements.md` — what the game must do
- `docs/architecture.md` — crate layout, design constraints, data flow
- `docs/milestones.md` — milestone definitions and status
- `docs/brp.md` — BRP endpoint reference (`dm/*` methods, ralph client API)
- `docs/systems/` — one file per major system (world-map, combat, civilization, world-sim, networking, persistence, rendering, pathfinding, perf, zones, underground)

## Crate map

| Crate | Role |
|---|---|
| `crates/shared` | Pure types, protocol, combat rules, world gen — no ECS, no I/O. Includes `world/zone.rs` (zone graph types + `generate_zones()`) and the generic `world/grid.rs` |
| `crates/server` | Bevy server logic (library): `WorldSimSchedule` (1 Hz) + `UndergroundSimSchedule` (0.1 Hz), AI, persistence, map gen, nav grid / pathfinding (`plugins/nav.rs` — refactored to use `Grid<T>`, with `ZoneNavGrids`), zone portals (`plugins/portal.rs`), adaptive performance throttling (`plugins/perf.rs`), DM BRP methods (`plugins/dm.rs`) |
| `crates/client` | Bevy client binary: rendering, egui HUD, input — also hosts the server plugins in-process today. Zone interior rendering lives in `plugins/zone_renderer.rs`; zone-scoped visibility culling is in `plugins/entity_renderer.rs::update_zone_visibility` |
| `tools/combat_sim` | proptest harness — runs combat rules with no ECS |
| `tools/ralph` | BRP HTTP test driver — asserts live world state via JSON-RPC |
| `tools/worldwatch` | Async egui dashboard — live world inspector + DM control panel over BRP |
| `tools/world_gen` | ASCII world preview: `cargo run -p world_gen -- --seed N` |
| `tools/character_studio` | Sprite atlas generator + desktop studio: `cargo run -p character_studio` — opens the egui GUI. Select an entity, choose Mock/OpenAI/Stability backend, generate 4 variants, approve one. |
| `tools/mesh_gen` | Rigged+animated 3D model pipeline: `cargo run -p mesh_gen -- --all` (mock) or `--backend live` (requires `MESHY_API_KEY`). Uses Meshy text-to-3d to produce animated GLBs. Outputs to `assets/models/`. Sprite generation is handled by `character_studio`. |

See `docs/systems/rendering.md` for bestiary / atlas details (`atlas_id_for_entity`, sprite generation, drift-guard test).

## Non-negotiable architecture rules

- **Pure rules, thin bridge.** Combat and world-sim logic goes in `crates/shared` as `fn(State) -> (State, Vec<Effect>)`. ECS systems only snapshot state → call rules → apply effects.
- **Never roll dice inside rules.** Always inject `rng: &mut impl Iterator<Item=i32>` so proptest can drive deterministic traces.
- **No wildcard `_` in interrupt stack `match`.** Every `InterruptFrame` variant must be handled explicitly — this is a lint-level guarantee against silent fallthrough bugs.
- **Three tick rates.** `FixedUpdate` at 62.5 Hz (combat/movement). `WorldSimSchedule` custom schedule at 1 Hz (factions/ecology/story/zone-hop). `UndergroundSimSchedule` custom schedule at 0.1 Hz (underground pressure accumulation + decay — slow background buildup). Never cross-schedule without a documented reason.
- **Isometric stays behind a feature flag.** Only `sync_transform` changes between `topdown` (default) and `isometric` features. Simulation and networking are untouched.
- **World gen is pure and deterministic.** `generate_map(seed)` and `generate_settlements(map, seed)` are pure functions in `crates/shared` — no ECS, no I/O. Same seed always produces the same world. The server calls them on startup via `MapGenPlugin`.
- **No circular module deps in world gen.** `world/civilization.rs` may import from `world/map.rs`. `world/map.rs` must NOT import from `civilization.rs`. Settlement generation happens after `generate_map` returns.

## Intellectual property guardrails

Fellytip is an original universe. Do NOT introduce content from Wizards of the Coast / TSR / Hasbro IP:

**Banned terms (WotC trademarks, not in SRD):**
- Underdark (use "underground" or "the Sunken Realm")
- Drow (use "deep-dwellers" or invent a name)
- Mind Flayer / Illithid
- Beholder
- Displacer Beast
- Githyanki / Githzerai
- Flumph (despite being SRD it's associated enough to avoid)
- Any named D&D setting: Forgotten Realms, Greyhawk, Eberron, etc.

**Permitted (SRD 5.1 CC BY 4.0 licensed):**
- Generic monster types in `assets/bestiary.toml`: goblin, kobold, orc, hobgoblin, bugbear, skeleton, zombie, ghoul, owlbear, troll, giant spider, giant rat, gelatinous cube, hill giant, young red dragon
- Generic fantasy concepts: elves, dwarves, dragons, trolls, giants (these are public domain)
- Aboleth (SRD)

**Lore direction:**
- The underground realm is called "the Sunken Realm" in lore, "underground" in code
- The ancient civilization is "the Kindled" (Auremn in their own tongue)
- The chaos entity is "the Unmaking"
- Surface factions are original: Ash Covenant, Deep Tide, Iron Wolves, Merchant Guild
- When in doubt: invent something new rather than borrowing from existing IP

## Key version pins (do not bump without checking compatibility)

- `bevy = "0.18"`, `lightyear = "0.26.4"`, `sqlx = "0.8"`, `bevy_egui = "0.39"`, `bevy-inspector-egui = "0.36"`

## Testing & verification

```bash
cargo test --workspace                 # 58 tests total (fast, no I/O)
cargo test -p fellytip-shared          # pure logic: map gen, biomes, civilization, combat
cargo test -p combat_sim               # 100k+ proptest traces
cargo clippy --workspace -- -D warnings
cargo run -p ralph -- --scenario all                # live end-to-end via BRP
cargo run -p ralph -- --scenario underground_e2e     # zone-graph raid pipeline end-to-end
cargo run -p world_gen -- --seed 42    # ASCII world preview (sanity check)
cargo run -p character_studio                      # open Character Studio desktop GUI
```

Run `cargo clippy` before considering any task done.

## Ralph loop (automated feedback)

`fellytip-client --headless` exposes BRP on port **15702**. Launch order:

```bash
cargo run -p fellytip-client -- --headless &
cargo run -p ralph -- --scenario all
```

Ralph scenarios are the acceptance criteria for each milestone. See **`docs/brp.md`** for the full `dm/*` method reference, built-in `world.*` endpoints, and the typed ralph BRP client API.

## Milestones & implementation order

See **`docs/milestones.md`** for the full milestone table and acceptance criteria. Work top-to-bottom. System docs in `docs/systems/` describe the current implementation of each major system.

## Style

- Prefer `thiserror` for error types, `anyhow` at call sites / main.
- Use `SmolStr` for interned string identifiers (faction names, lore tags, sprite keys).
- `GameEntityId(Uuid)` is the stable cross-session identity; Bevy `Entity` is ephemeral.
- Replicated components go in `crates/shared/src/components.rs` and must be registered in `FellytipProtocolPlugin`.
- Story events are emitted as Bevy events (`WriteStoryEvent`), collected by `story_writer`, and flushed to SQLite every 5 minutes.
- `WorldPosition` has three fields `{x, y, z}` — always include all three when constructing it.
- `TileKind` has 20 variants (5 legacy surface + 11 Whittaker biomes + River + 4 underground). When matching exhaustively include all or use a `_` only after documenting why.
- `WorldMap` is NOT replicated to clients (512×512 = too large). Clients get entity positions only; tile rendering samples locally on region-load.
- `generate_map` runs the full pipeline: fBm surface → moisture → biome classification → rivers → shallow caves → underground voids → shafts. Call order in server: `generate_map` → `generate_settlements` → `generate_roads` (mutates map).
- `Settlements` is a Bevy `Resource` (wraps `Vec<Settlement>`) inserted by `MapGenPlugin`.
- `BuildingKind` now includes `Tavern` (2-floor), `Barracks` (2-floor), `Tower` (4-floor incl. battlements) and `Keep` (3-floor + 10×10 battlements) — these are the building kinds that produce child zones via `generate_zones()`. All other `BuildingKind` variants stay on the overworld (no interior zone). See `docs/systems/zones.md`.
- Zone graph: `ZoneRegistry` + `ZoneTopology` are Bevy resources populated at startup. `OVERWORLD_ZONE = ZoneId(0)`. Zones are generated by pure `generate_zones(&buildings, seed)` in `crates/shared/src/world/zone.rs`. A fixed 3-depth underground chain (the Sunken Realm) is always generated for testing. See `docs/systems/zones.md` and `docs/systems/underground.md`.

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