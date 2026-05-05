# BRP (Bevy Remote Protocol) Reference

## Launch order

`fellytip-client --headless` runs both client and server logic in one process and exposes BRP on port **15702** (see `BRP_PORT` in `crates/client/src/main.rs`). There is no separate `fellytip-server` binary — `fellytip-server` is a library crate the client consumes.

```bash
cargo run -p fellytip-client -- --headless &
cargo run -p ralph -- --scenario all
cargo run -p ralph -- --scenario movement_e2e   # click-to-move / dm/move_entity
cargo run -p ralph -- --scenario underground_e2e # zone-graph raid pipeline
```

Ralph scenarios are the acceptance criteria for each milestone. A scenario passing = milestone shipped.

## Built-in endpoints

Bevy 0.18 renamed built-in BRP endpoints from `bevy/*` to `world.*`. Always use the `world.*` names:

| Method | Description |
|---|---|
| `world.query` | Query entities matching a component filter |
| `world.get_components` | Fetch specific components from a single entity |
| `world.list_resources` | List inserted resources (used for ping) |
| `world.spawn` | Spawn an entity with given components |

## DM methods (`dm/*`)

Registered by `crates/server/src/plugins/dm.rs` and exposed on the headless client. Used by ralph scenarios and `tools/worldwatch` for live testing.

| Method | Params | Effect |
|---|---|---|
| `dm/spawn_npc` | `{ faction, x, y, z, level? }` | Spawn a full-stat faction NPC |
| `dm/kill` | `{ entity }` | Despawn any entity by id |
| `dm/teleport` | `{ entity, x, y, z }` | Move an entity to a new position |
| `dm/set_faction` | `{ faction, food?, gold?, military? }` | Override faction resources |
| `dm/trigger_war_party` | `{ attacker_faction, target_faction }` | Immediately form a war party targeting the nearest hostile settlement |
| `dm/set_ecology` | `{ region, prey?, predator? }` | Override prey / predator counts in a macro region |
| `dm/battle_history` | `{ limit? }` | Read the rolling battle record history, newest-first |
| `dm/clear_battle_history` | `{}` | Drop every queued `BattleRecord` — test helper so scenarios can isolate their own battle |
| `dm/underground_pressure` | `{}` | Read the `UndergroundPressure` snapshot `{ score, last_raid_tick }` |
| `dm/force_underground_pressure` | `{}` | Force `UndergroundPressure.score = 1.0` so the next 1 Hz tick spawns a raid — used by `underground_e2e` |
| `dm/move_entity` | `{ entity, x, y, z }` | A* path any entity (PC or NPC) to a world target; inserts `NavPath` + `NavigationGoal` — used by `movement_e2e` |
| `dm/query_portals` | `{}` | List all portal trigger positions |
| `dm/spawn_wildlife` | `{ kind, x, y, z }` | Spawn a wildlife entity at a world position |
| `dm/list_settlements` | `{ kind?, name? }` | Return all settlement names + world-space coords |

## Client-side DM methods

Registered in `crates/client/src/main.rs` (windowed + headless modes):

| Method | Effect |
|---|---|
| `dm/set_portal_debug` | Toggle portal debug overlay |
| `dm/take_screenshot` | Capture a screenshot |
| `dm/set_camera_distance` | Override orbit camera distance |
| `dm/teleport_player` | Teleport the local player |
| `dm/set_character_debug` | Toggle character debug overlay |
| `dm/set_camera_free` | Enable/disable free-orbit camera |
| `dm/choose_class` | Force class selection (headless test helper) |
| `dm/set_time_of_day` | Override scene lighting time |
| `dm/enter_portal` | Force the player through a named portal |
| `dm/toggle_physics_debug` | Toggle avian3d physics debug rendering |

## Ralph BRP client

`tools/ralph/src/brp.rs` wraps JSON-RPC calls. Each `dm/*` method has a typed helper:

```rust
server.dm_spawn_npc("iron_wolves", x, y, z)  // → entity u64
server.dm_kill(entity)
server.dm_teleport(entity, x, y, z)
server.dm_trigger_war_party("iron_wolves", "ash_covenant")
server.dm_battle_history(Some(10))
server.dm_underground_pressure()              // → (score, last_raid_tick)
server.dm_force_underground_pressure()
server.dm_move_entity(entity, x, y, z)        // → waypoint count u32
server.query(&["ComponentType"])              // → Vec<Value>
server.get(entity, &["ComponentType"])        // → Value
```
