# System: Networking

> **Current state:** single-process, no network layer. All game logic (server plugins + client plugins) runs in one `fellytip-client` binary. Lightyear has been removed. The architecture is structured so a true multiplayer server can be re-introduced behind a `multiplayer` feature flag without a rewrite.

## Single-binary topology

```
fellytip-client (one process)
  ├── ServerGamePlugin  ← crates/server lib (world sim, AI, combat, persistence)
  ├── FellytipProtocolPlugin ← type registration only
  ├── Input / rendering / HUD plugins
  └── HTTP BRP_PORT (15702) — BRP JSON-RPC (observability, ralph)
```

There is no separate server process. Port 15702 is the only network socket (BRP only).

## In-process messaging

Server → client events use Bevy's native `MessageWriter<T>` / `MessageReader<T>` within the same process:

| Message | Writer | Reader | Notes |
|---|---|---|---|
| `BattleStartMsg` | `AiPlugin` (`march_war_parties`) | `BattleVisualsPlugin` | Triggers ring spawn + battle log entry |
| `BattleEndMsg` | `AiPlugin` (`run_battle_rounds`) | `BattleVisualsPlugin` | Despawns ring; logs winner |
| `BattleAttackMsg` | `AiPlugin` (`run_battle_rounds`) | `BattleVisualsPlugin` | Consumed; per-entity flash deferred |
| `StoryMsg` | `StoryPlugin` (`collect_story_events`) | `BattleVisualsPlugin` | Appended to `ClientStoryLog` for HUD |
| `WriteStoryEvent` | Various plugins | `StoryPlugin` | Triggers story log + SQLite flush |

All message types are registered with `app.add_message::<T>()` by the plugin that writes them. Registration is idempotent so readers can also register defensively.

## Player input

There is no `PlayerInput` network message. Input flows through a resource:

```
Keyboard → send_player_input (client Update)
         → PredictedPosition (movement, immediate)
         → LocalPlayerInput.actions (combat intents)

sync_pred_to_world → WorldPosition ← same frame

process_player_input (FixedUpdate, reads LocalPlayerInput)
  → queues PendingAttack, clears LocalPlayerInput.actions
```

`LocalPlayerInput` is defined in `crates/server/src/plugins/combat.rs` and inserted as a `Resource` by `CombatPlugin`.

## Player lifecycle

The local player entity is spawned by `spawn_local_player` in `PostStartup` (after `MapGenPlugin`'s `Startup` chain finishes). It receives:

- `WorldPosition` — initial position from `WorldMap::spawn_points[0]` or `find_surface_spawn`
- `Health`, `CombatParticipant`, `Experience`, `PlayerStandings`
- `GameEntityId(Uuid)` — stable cross-session identity
- `LastPlayerInput`, `PositionSanityTimer`

`tag_local_player` runs every `Update` frame until it finds an entity with `With<Experience>, Without<LocalPlayer>` and attaches the `LocalPlayer` marker and `PredictedPosition`.

## Interest management (`crates/server/src/plugins/interest.rs`)

`ChunkTemperature` tracks Hot and Warm chunk sets centred on the single local player:

| Zone | Chebyshev chunk radius | Per-NPC sim speed |
|---|---|---|
| Hot | 0–2 | 1.0× (full speed) |
| Warm | 3–8 | 0.25× (quarter speed) |
| Frozen | > 8 | 0.05× (~20× slower) |

Aggregate systems (birth counters, ecology, faction goals) always run at full speed. Per-NPC systems (aging, movement, battle rounds) are zone-gated via `ChunkTemperature`.

`update_npc_replication` is removed (no replication targets). Zone gating is still used to throttle expensive per-NPC work.

## Client-side movement

`send_player_input` reads WASD/arrow keys, normalises the direction, rotates by camera yaw, and applies movement to `PredictedPosition` using the local `WorldMap` (same deterministic generation as the server-side map). Terrain walkability (`is_walkable_at`) and Z elevation (`smooth_surface_at`) are computed locally each frame.

`sync_pred_to_world` copies `PredictedPosition` → `WorldPosition` every `Update` frame so server-side systems (combat, AI) see current position.

## BRP observability

The client exposes Bevy Remote Protocol (BRP) on port **15702** (Bevy 0.18 renamed all built-in methods from `bevy/*` to `world.*`):

- `world.query` — query entities by component
- `world.get` — read components on a specific entity
- `world.insert` / `world.spawn` / `world.destroy` — mutate ECS

Custom `dm/*` methods (`dm/spawn_npc`, `dm/kill`, `dm/teleport`, `dm/set_faction`, `dm/trigger_war_party`, `dm/set_ecology`, `dm/battle_history`, `dm/clear_battle_history`) are registered in `crates/client/src/main.rs` for live world manipulation — see `CLAUDE.md` for the parameter list and `crates/server/src/plugins/dm.rs` for handler source.

`ralph` uses these endpoints to assert live game state. Headless mode (`--headless`) skips rendering but keeps BRP active.

## Headless automation

`--headless` skips `DefaultPlugins` and windowed plugins. Two automation systems replace keyboard input:

- **`headless_auto_attack`** — queues `BasicAttack` via `LocalPlayerInput` every 2 s
- **`headless_auto_move`** — writes directly to `PredictedPosition`, alternating right/left every 3 s

## Multiplayer re-introduction path

The codebase is structured to minimise the effort of adding Lightyear back:

1. Re-introduce `lightyear` behind `feature = "multiplayer"` in workspace `Cargo.toml`.
2. Re-enable `FellytipProtocolPlugin::build()` — channel and replication registration.
3. Restore `crates/server` as a binary process (the lib is already there).
4. Re-wrap `LocalPlayerInput` as `MessageSender<PlayerInput>` behind `#[cfg(feature = "multiplayer")]`.
5. Restore `on_client_connected` hook to spawn one entity per client.
6. Restore `Replicate` on server entities for `WorldPosition`, `Health`, etc.

Key architectural invariants that make this feasible without a rewrite:
- Pure logic in `crates/shared` (no ECS, no network types)
- `WorldSimSchedule` two-tick-rate design
- `GameEntityId(Uuid)` stable identity
- `PartyPlugin` already structured for multi-client
