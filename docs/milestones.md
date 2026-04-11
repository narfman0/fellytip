# Fellytip — Milestones

Milestones are ordered by dependency. Each one builds on the previous.

## Status

| Milestone | Done when | Status |
|---|---|---|
| **0 — Bones** | Server and client binaries connect; `WorldPosition` replicates; WASD moves a sprite | ✅ Complete |
| **0b — Ralph** | BRP wired on server (15702) and headless client (15703); `ralph basic_movement` passes | ✅ Complete |
| **1 — Living World** | `WorldSimSchedule` at 1 Hz; factions, ecology, story log all tick; world survives restart | ✅ Complete (egui log viewer and BRP custom methods still pending) |
| **2 — First Blood** | Player attacks NPC; NPC dies; XP awarded; story event emitted; proptest suite green | ✅ Complete (ralph `combat_resolves` scenario pending) |
| **3 — Party Play** | 4 simultaneous clients connect; party registry enforces cap; test suite passes | ✅ Complete (visibility culling and party HUD still pending) |
| **World Gen** | fBm terrain, Whittaker biomes, rivers, settlements, territory, roads, 200-tick history warp | ✅ Complete |
| **4 — MVF** | 3 character classes with distinct abilities; dungeon room transitions; faction consequences visible in-game; ralph full suite green; 2-hour session stable | 🚧 Scaffold done — classes, abilities, full ralph suite remaining |

## Acceptance criteria per milestone

### Milestone 0
- `cargo run -p fellytip-server` starts without error.
- `cargo run -p fellytip-client` connects to the server.
- WASD inputs move the player entity and the `WorldPosition` component updates on the client.

### Milestone 0b
- `ralph basic_movement` scenario passes against a live server + headless client.
- BRP `bevy/query` on `WorldPosition` returns correct data.

### Milestone 1
- `WorldSimSchedule` fires once per real second.
- Faction goals update each world-sim tick.
- Ecology populations update and emit `StoryEvent::EcologyCollapse` when below threshold.
- Story events survive a server restart (flushed to SQLite).

### Milestone 2
- Pressing Space (BasicAttack) damages the dungeon boss.
- Boss death triggers XP award and a `PlayerKilledNamed` story event.
- `cargo test -p combat_sim` runs 100k+ proptest traces with no failures.

### Milestone 3
- 4 clients can all connect simultaneously and move independently.
- Attempting a 5th connection is rejected.

### World Gen
- `cargo run -p world_gen -- --seed 42` prints a recognisable ASCII world map.
- Same seed always produces the same map (determinism test in `cargo test -p fellytip-shared`).
- Server startup log shows settlement, road, and territory counts.

### Milestone 4
- Three `CharacterClass` variants each have at least one distinct ability in the interrupt stack.
- Dungeon boss has phased abilities.
- A faction-war story event causes NPCs to patrol aggressively.
- `ralph --scenario all` passes.
- Server runs 2+ hours under a 4-client load test without panic or memory growth.

## What's next after Milestone 4

- Egui story log viewer and faction state HUD
- SQLite autosave flush for story events and player state
- Visibility culling — replicate only entities within a party's radius
- Isometric rendering upgrade (feature flag already in place)
- Dungeon room transition system
