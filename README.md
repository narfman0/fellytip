# Fellytip

Multiplayer action RPG in Rust/Bevy where the world simulates itself independent of player presence.

**Status:** Active development. Full D&D 5e SRD foundation (ability scores, hit dice, saving throws, XP thresholds 1-20), all 14 classes with class selection screen, click-to-attack input, visual quality pass (bloom/HDR, SSAO, fog, animated water, particle FX, procedural skybox, tree sway), in-game settings menu. Fully procedural world with fBm terrain, Whittaker biomes, rivers, settlements, territory, road networks, and 200-tick history pre-simulation before players join.

## Prerequisites

- Rust stable (edition 2021)
- A C linker (MSVC on Windows, `gcc`/`clang` on Linux/macOS)

## Run

```bash
# Terminal 1 — game server (from the workspace root)
cargo run -p fellytip-server

# Terminal 2 — game client (windowed, connects to localhost automatically)
cargo run -p fellytip-client

# Terminal 2 alt — headless client (no window, for testing)
cargo run -p fellytip-client -- --headless

# Preview the generated world as ASCII art
cargo run -p world_gen -- --seed 42
cargo run -p world_gen -- --seed 42 --width 120 --height 50
```

**Note:** Always run commands from the workspace root (`fellytip/`), not from inside a crate directory. The server and client run in separate terminals and connect automatically on localhost.

## Verify with ralph

`ralph` is an automated test driver that asserts live world state via the Bevy Remote Protocol (BRP).

```bash
# With server + headless client already running:
cargo run -p ralph -- --scenario basic_movement
```

## Tests

```bash
cargo test --workspace               # 58 tests across all crates
cargo test -p fellytip-shared        # pure logic: map gen, biomes, civilization, combat (58 tests)
cargo test -p combat_sim             # proptest invariants for combat + ecology
cargo clippy --workspace -- -D warnings
```

## What's implemented

| Area | State |
|---|---|
| Networking (Lightyear 0.26) | Server + client connect; `WorldPosition {x,y,z}` replicated |
| BRP observability | Server port 15702, headless client port 15703 |
| SQLite persistence | Migrations run on startup; `Db` resource available |
| World sim (1 Hz) | `WorldSimSchedule` drives ecology, faction AI, story flush |
| **World map** | 512×512 tile grid, stacked layers, fBm terrain, 3D height (`z`) |
| **Biomes** | Whittaker diagram: 10 biome types from temperature × precipitation |
| **Rivers** | Steepest-descent flow accumulation; high-drainage tiles marked River |
| **Settlements** | Poisson-disk surface placement; BFS underground city siting |
| **Territory** | BFS flood-fill assigns every walkable tile to nearest settlement |
| **Roads** | Kruskal MST + Bresenham rasterization between settlements |
| **History warp** | 200 WorldSim ticks run at startup before clients connect |
| **Underground** | Shallow caves (CA 48%), deep voids (CA 30%), shaft connectors |
| Ecology | Discrete Lotka-Volterra per region; Collapse/Recovery events |
| Faction AI | Utility-scored goals; NPC wander each world-sim tick |
| Story log | `WriteStoryEvent` messages → `StoryLog` resource |
| Combat rules | Pure `fn(State, dice) -> (State, Vec<Effect>)`; proptest-covered |
| Combat ECS bridge | `FixedUpdate` interrupt stack; dice injected at boundary |
| Fluid movement | Z follows terrain via bilinear height interpolation + lerp |
| Party system | Up to 4 clients; `PartyRegistry` resource |
| Dungeon | Boss NPC spawned; `BossNpc` + `InDungeon` markers; `BossPhase` 3-phase combat |
| **Character classes** | All 14 D&D 5e SRD classes with scrollable selection screen; class-specific ability arrays, saving throw proficiencies, and hit dice |
| **D&D 5e SRD foundation** | `AbilityScores`, `AbilityModifiers`, `HitDice`, `SavingThrowProficiencies` ECS components; `roll_saving_throw()`; SRD XP thresholds (levels 1–20); proficiency bonus by level |
| **Click-to-attack** | Left-click = basic attack; right-click = action popup (Attack / Ability / Dodge / Cancel); Q/E keybindings preserved |
| **Visual quality** | Bloom/HDR, SSAO, distance fog, animated water shader, particle FX (campfire, lantern, combat hits, heals), procedural day-night skybox with stars, tree sway, windmill spin |
| **Settings menu** | In-game egui settings panel (Escape → Settings): bloom, SSAO, fog, water, tree sway, windmill, LOD, particles, skybox toggles + saturation slider; persisted as RON |
| **Faction alert system** | `FactionAlertState` raises NPC patrol radius 2× and speed 1.5× for 300 ticks after any battle |
| **Party HUD** | egui health bars for all party members; updates from replicated `Health` components |
| **Roof cutaway** | `RoofTile` + `update_roof_cutaway` hides interior roof tiles when player is in a `BuildingFloor` zone |
| **Zone-aware A*** | `find_path_zone_aware` in shared; `ZoneNavGrids::zone_astar` bridge; wired into NPC wander |

## Crates

| Crate | Purpose |
|---|---|
| `crates/shared` | Types, protocol, combat rules, world gen — no I/O |
| `crates/server` | Bevy server: networking, world sim, persistence, map gen |
| `crates/client` | Bevy client: networking, rendering (WIP) |
| `tools/ralph` | BRP test driver |
| `tools/combat_sim` | proptest harness for combat + ecology rules |
| `tools/world_gen` | ASCII world preview — `cargo run -p world_gen -- --seed N` |
| `tools/worldwatch` | Windows tray app: live BRP + SQLite dashboard |
| `tools/character_studio` | AI sprite pipeline (DALL-E 3) + egui desktop studio — `cargo run -p character_studio` |

## ASCII map legend

```
~ water    ^ mountain   , grassland   f temperate forest   d desert
s savanna  T tropical   R rainforest  b taiga              _ tundra
p polar    * arctic     = river       + road
★ capital  • town       ⚑ underground city
```
