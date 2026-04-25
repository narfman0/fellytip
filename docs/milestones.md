# Fellytip — Milestones

Milestones are ordered by dependency. Each one builds on the previous.

## Status

| Milestone | Done when | Status |
|---|---|---|
| **0 — Bones** | Server and client binaries connect; `WorldPosition` replicates; WASD moves a sprite | ✅ Complete |
| **0b — Ralph** | BRP wired on server (15702) and headless client (15703); `ralph basic_movement` passes | ✅ Complete |
| **1 — Living World** | `WorldSimSchedule` at 1 Hz; factions, ecology, story log all tick; world survives restart | ✅ Complete |
| **2 — First Blood** | Player attacks NPC; NPC dies; XP awarded; story event emitted; proptest suite green | ✅ Complete (ralph `combat_resolves` scenario pending) |
| **3 — Party Play** | 4 simultaneous clients connect; party registry enforces cap; NPC interest management | ✅ Complete (party HUD still pending) |
| **World Gen** | fBm terrain, Whittaker biomes, rivers, settlements, territory, roads, history warp | ✅ Complete |
| **Living World ext.** | Settlement population growth; faction war parties; client battle visualizations | ✅ Complete |
| **Zone Graph / Interiors** | `ZoneRegistry`/`ZoneTopology` + `generate_zones()`; `PortalPlugin` with `PlayerZoneTransition`; `ZoneNavGrids`; `UndergroundSimSchedule` (0.1 Hz) + `UndergroundPressure` + raid spawn; client `ZoneRendererPlugin` + `ZoneCache`; `underground_e2e` ralph scenario green | ✅ Scaffold complete — see follow-ups in `docs/systems/zones.md` (portal anchor world-coords, `ZoneTopology::shortest_path`, `ZoneTileMessage::kind`, lightyear per-zone interest groups, roof-cutaway shader) |
| **4 — MVF** | 3 character classes with distinct abilities; dungeon room transitions; faction consequences visible in-game; ralph full suite green; 2-hour session stable | 🚧 Scaffold done — classes, abilities, full ralph suite remaining |

## Acceptance criteria per milestone

### Milestone 0
- `cargo run -p fellytip-client` starts without error.
- WASD inputs move the player entity and the `WorldPosition` component updates.

### Milestone 0b
- `ralph basic_movement` scenario passes against a running headless client (`--headless`).
- BRP `world.query` on `WorldPosition` returns correct data. (Bevy 0.18 renamed the built-in BRP methods from `bevy/*` to `world.*`.)

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
- NPC replication uses per-client zone-based interest management (Hot/Warm/Frozen chunks).

### World Gen
- `cargo run -p world_gen -- --seed 42` prints a recognisable ASCII world map.
- Same seed always produces the same map (determinism test in `cargo test -p fellytip-shared`).
- Server startup log shows settlement, road, and territory counts.

### Living World extension
- Settlement populations grow: child NPCs spawn every 300 ticks, scale with `GrowthStage`.
- When adult count ≥ 15, a war party of 10 marches toward a hostile-faction settlement.
- Battles resolve with seeded deterministic dice; `BattleStartMsg` / `BattleEndMsg` broadcast to clients.
- Client shows pulsing ring at battle sites and a Battle Log egui panel.
- **War-party pathfinding is complete**: 256×256 `NavGrid` (4:1 downsample of the 1024×1024 world, stored as `Grid<NavCell>`), A* for individual units, BFS/Dijkstra flow fields for parties targeting a shared settlement. See `docs/systems/pathfinding.md`.
- **Adaptive performance throttling is complete**: rolling tick-time samples derive a `ThrottleLevel` (`Full`/`Reduced`/`Minimal`/`Suspended`) with hysteresis. Host-mode client frame pressure bumps the level one step. See `docs/systems/perf.md`.

### Zone Graph / Interiors
- `generate_zones(&buildings, seed)` is pure and deterministic; emits one child zone per floor for multi-story building kinds (`Tavern`, `Barracks`, `Tower`, `Keep`) plus a hard-coded 3-depth underground chain.
- `PortalPlugin` spawns `PortalTrigger` entities for every portal, emits `PlayerZoneTransition` on proximity, applies transitions (respecting `one_way`), and broadcasts `ZoneTileMessage` for the destination zone + 1-hop neighbours.
- `UndergroundSimSchedule` runs at 0.1 Hz (one tick per 10 real seconds). `UndergroundPressure` accumulates on that schedule; thresholds at 0.4 / 0.7 emit `StoryEvent::UndergroundThreat` signals; threshold 0.8 spawns a 3-member raid party in the deepest underground zone that then zone-hops via `advance_zone_parties` on `WorldSimSchedule` (1 Hz) until it reaches `OVERWORLD_ZONE` and converts to a normal surface war party.
- `dm/underground_pressure` and `dm/force_underground_pressure` BRP methods drive the `underground_e2e` ralph scenario.
- Client side: `ZoneRendererPlugin` spawns interior meshes for the player's current zone; `update_zone_visibility` culls entities in other zones locally. See `docs/systems/zones.md` and `docs/systems/underground.md`.

### Milestone 4
- Three `CharacterClass` variants each have at least one distinct ability in the interrupt stack.
- Dungeon boss has phased abilities.
- A faction-war story event causes NPCs to patrol aggressively.
- `ralph --scenario all` passes.
- Server runs 2+ hours under a 4-client load test without panic or memory growth.

## What's next after Milestone 4

- SQLite autosave flush for story events and player state (faction reputation load/save)
- Party HUD (show party members' health bars)
- Ralph `combat_resolves` scenario
- Isometric rendering upgrade (feature flag already in place)
- Dungeon room transition system — zone graph scaffold is in place, still need surface `CaveEntrance` portals generated per biome and populated Dungeon tile layouts (currently only the underground chain generates content).
- NPC pathfinding (war-party march is tile-linear; goal-directed movement for guards)
- Zone-aware A* / flow-field pathfinding that consumes `ZoneNavGrids`
- Lightyear per-zone interest groups (replace client-only `update_zone_visibility` stop-gap)
