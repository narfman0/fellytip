# Fellytip

Multiplayer action RPG in Rust/Bevy where the world simulates itself independent of player presence.

**Status:** Active development. Full D&D 5e SRD foundation (ability scores, hit dice, saving throws, XP thresholds 1-20), all 14 classes with class selection screen, click-to-attack input, visual quality pass (bloom/HDR, SSAO, fog, animated water, particle FX, procedural skybox, tree sway), in-game settings menu. Fully procedural world with fBm terrain, Whittaker biomes, rivers, settlements, territory, road networks, and 200-tick history pre-simulation before players join.

## Prerequisites

- Rust stable (edition 2024)
- A C linker (MSVC on Windows, `gcc`/`clang` on Linux/macOS)

## Run modes

```bash
# Single-player (default): client runs the game plugin in-process, no network.
cargo run -p fellytip-client

# Headless mode (no window) for ralph scenarios, bots, and external BRP tooling.
# Exposes BRP on port 15702 and registers all dm/* methods.
cargo run -p fellytip-client -- --headless

# Preview the generated world as ASCII art.
cargo run -p world_gen -- --seed 42

# Sprite + 3D mesh studio.
cargo run -p character_studio
```

See [`tools/character_studio/README.md`](tools/character_studio/README.md) for backend setup (OpenAI / Stability AI / Mock) and the `MESHY_API_KEY` env var for 3D mesh generation.

**Note:** Always run commands from the workspace root (`fellytip/`), not from inside a crate directory.

## Verify with ralph

`ralph` is an automated test driver that asserts live world state via the Bevy Remote Protocol (BRP).

```bash
# With a headless client already running:
cargo run -p ralph -- --scenario basic_movement
cargo run -p ralph -- --scenario all
```

## Tests

```bash
cargo test --workspace
cargo test -p fellytip-shared        # pure logic: protocol, components, sprite math
cargo test -p fellytip-world-types   # map gen, biomes, civilization, ecology, zones
cargo test -p fellytip-combat-rules  # combat rules, interrupts, spells
cargo test -p fellytip-game          # plugin-level tests
cargo test -p combat_sim             # proptest invariants for combat + ecology
cargo clippy --workspace -- -D warnings
```

## Crates

| Crate | Purpose |
|---|---|
| `crates/shared` | ECS components, replicated protocol, input intents, math/sprite utilities |
| `crates/combat-rules` | Pure combat logic — `CharacterClass`, `SpellSlots`, rules, interrupt stack |
| `crates/world-types` | Pure world data types — map, zone, faction, population, ecology, civilization, dungeon, cave, grid |
| `crates/game` | `ServerGamePlugin` + all 16 game simulation plugins (combat, AI, ecology, nav, portal, party, persistence, perf, story, world-sim, dungeon, bot, interest, character-persistence, map-gen, combat-test) |
| `crates/server` | Thin shim over `fellytip-game` + DM/RPC admin tools (`dm_spawn_npc`, `dm_kill`, `dm_teleport`, `dm_trigger_war_party`, `dm_set_ecology`, …) |
| `crates/client` | Bevy rendering + UI binary; hosts `ServerGamePlugin` in-process for both windowed and headless runs |
| `tools/ralph` | BRP HTTP test driver |
| `tools/combat_sim` | proptest harness for combat + ecology rules |
| `tools/world_gen` | ASCII world preview |
| `tools/worldwatch` | Live BRP + SQLite dashboard |
| `tools/character_studio` | AI sprite pipeline + egui desktop studio |

## Bots / fake players

The headless client exposes BRP methods for spawning server-side fake players that are indistinguishable from real players (same component bundle, same combat / aggro / persistence handling):

| Method | Effect |
|---|---|
| `dm/spawn_bot` | Spawn a bot at a world position with a `BotPolicy` (Idle / Wander / Aggressive) |
| `dm/despawn_bot` | Despawn a bot by entity id |
| `dm/list_bots` | List all live bots |
| `dm/set_bot_action` | Queue a one-shot `ActionIntent` on a bot |

See `docs/brp.md` for the full `dm/*` reference.

## What's implemented

| Area | State |
|---|---|
| Networking (Lightyear 0.26) | Server + client protocol; `WorldPosition {x,y,z}` replicated |
| BRP observability | Headless client port 15702 |
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
| **Bots / fake players** | `dm/spawn_bot` etc. spawn full-fidelity player entities with `BotController` + `BotPolicy` |

## ASCII map legend

```
~ water    ^ mountain   , grassland   f temperate forest   d desert
s savanna  T tropical   R rainforest  b taiga              _ tundra
p polar    * arctic     = river       + road
★ capital  • town       ⚑ underground city
```
