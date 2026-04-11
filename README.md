# Fellytip

Multiplayer action RPG in Rust/Bevy where the world simulates itself independent of player presence.

**Status:** All 13 implementation steps complete through Milestone 4 scaffold. Core systems are running: networking, world simulation, ecology, factions, story log, combat rules + ECS bridge, party system, and dungeon boss spawn.

## Prerequisites

- Rust stable (edition 2021)
- A C linker (MSVC on Windows, `gcc`/`clang` on Linux/macOS)

## Run

```bash
# Terminal 1 — game server
cargo run -p fellytip-server

# Terminal 2 — headless client (connects automatically)
cargo run -p fellytip-client -- --headless
```

## Verify with ralph

`ralph` is an automated test driver that asserts live world state via the Bevy Remote Protocol (BRP).

```bash
# With server + headless client already running:
cargo run -p ralph -- --scenario basic_movement
```

## Tests

```bash
cargo test -p fellytip-shared   # pure logic: ecology, faction, combat (13 tests)
cargo test -p combat_sim        # proptest invariants for combat + ecology
cargo test -p fellytip-server   # party system tests
cargo clippy --workspace -- -D warnings
```

## What's implemented

| Area | State |
|---|---|
| Networking (Lightyear 0.26) | Server + client connect; `WorldPosition` replicated |
| BRP observability | Server port 15702, headless client port 15703 |
| SQLite persistence | Migrations run on startup; `Db` resource available |
| World sim (1 Hz) | `WorldSimSchedule` drives ecology, faction AI, story flush |
| Ecology | Discrete Lotka-Volterra per region; Collapse/Recovery events |
| Faction AI | Utility-scored goals; NPC wander each world-sim tick |
| Story log | `WriteStoryEvent` messages → `StoryLog` resource |
| Combat rules | Pure `fn(State, dice) -> (State, Vec<Effect>)`; proptest-covered |
| Combat ECS bridge | `FixedUpdate` interrupt stack; dice injected at boundary |
| Party system | Up to 4 clients; `PartyRegistry` resource |
| Dungeon | Boss NPC spawned; `BossNpc` + `InDungeon` markers |

## Crates

| Crate | Purpose |
|---|---|
| `crates/shared` | Types, protocol, combat rules — no I/O |
| `crates/server` | Bevy server: networking, world sim, persistence |
| `crates/client` | Bevy client: networking, rendering (WIP) |
| `tools/ralph` | BRP test driver |
| `tools/combat_sim` | proptest harness for combat + ecology rules |
