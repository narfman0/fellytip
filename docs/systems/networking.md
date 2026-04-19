# System: Networking

Networking is handled by [Lightyear 0.26.4](https://github.com/cBournhonesque/lightyear), which targets Bevy 0.18. The transport layer is UDP (netcode).

Port numbers and the protocol constants (`PROTOCOL_ID`, `PRIVATE_KEY`, `NET_PORT`) are defined in `crates/shared/src/lib.rs`. BRP port constants are defined in the server and client `main.rs` files respectively.

## Topology

```
Server (fellytip-server)
  ├── UDP NET_PORT          — game traffic (Lightyear netcode)
  └── HTTP BRP_PORT         — BRP JSON-RPC (observability, ralph)

Client (fellytip-client)
  ├── connects to server UDP NET_PORT
  └── HTTP BRP_PORT_HEADLESS — BRP JSON-RPC (headless client observability)
```

Up to 4 clients may connect simultaneously. A 5th connection is rejected by `PartyPlugin`.

## Replicated components

These components in `crates/shared/src/components.rs` are registered in `FellytipProtocolPlugin` and replicated from server to all clients:

| Component | Sync mode | Notes |
|---|---|---|
| `WorldPosition { x, y, z }` | Client-authoritative; server echoes back | Client predicts with local terrain; server only overrides after 10 s walkability violation |
| `Health { current, max }` | Simple interpolation | Client renders health bars |
| `Experience { xp, level, xp_to_next }` | Simple interpolation | Client renders level/XP |
| `EntityKind` | Simple | Enum: FactionNpc / Wildlife / Settlement — drives visual differentiation on client |
| `WorldMeta { seed, width, height }` | Server → client (once on connect) | Client regenerates local `WorldMap` so terrain walkability matches the server exactly |
| `GrowthStage(f32)` | Simple | 0.0 = newborn, 1.0 = adult; drives NPC capsule scale (0.3 → 1.0) on client |
| `FactionBadge { faction_id, rank }` | Simple | Only on `FactionNpc` entities; drives per-faction capsule colour on client |
| `PlayerStandings { standings }` | Simple | Only on player entities; list of `(faction_name, score)` — drives HUD reputation display |

All are serializable (`serde`) and reflectable (`bevy::reflect`).

Players carry `WorldMeta`, `PlayerStandings`, but not `EntityKind`. Absence of `EntityKind` on a replicated entity indicates a player.

## Messages

| Message | Direction | Channel | Notes |
|---|---|---|---|
| `PlayerInput` | Client → Server | Unordered unreliable UDP | Movement + action intent every frame |
| `GreetMsg` | Server → Client | Ordered reliable | Sent on connect to verify channel |
| `StoryMsg { text }` | Server → Client | Ordered reliable | Significant world-story events; displayed in client story panel |
| `BattleStartMsg` | Server → Client | Sequenced reliable | Broadcast when war party arrives at target settlement |
| `BattleEndMsg` | Server → Client | Sequenced reliable | Broadcast when one side is eliminated |
| `BattleAttackMsg` | Server → Client | Sequenced reliable | Broadcast per hit during a battle |

`PlayerInput` carries `move_dir: [f32; 2]`, `pos: [f32; 3]` (client-computed position), an optional `ActionIntent`, and an optional target UUID.

## Interest management (`crates/server/src/plugins/interest.rs`)

Each connected client has a two-tier active zone centred on its player entity:

| Zone | Chebyshev chunk radius | Replication | Individual NPC sim speed |
|---|---|---|---|
| Hot | 0–2 | Yes | 1.0× (full speed) |
| Warm | 3–8 | Yes | 0.25× (quarter speed) |
| Frozen | > 8 | No | 0.05× (~20× slower) |

Aggregate systems (birth counters, ecology, faction goals) always run at full speed — only per-NPC systems (aging, movement, battle rounds) are zone-gated.

`update_chunk_temperature` rebuilds zone maps once per `WorldSimSchedule` tick (1 Hz) from current player positions. `update_npc_replication` then re-targets each NPC's `Replicate` component so only clients near the NPC receive its replication traffic.

## Client-side input and movement

`send_player_input` runs in `Update` on the client. It reads WASD/arrow keys, normalises the direction, rotates by camera yaw, and applies movement to `PredictedPosition` using the local `WorldMap` (same deterministic generation as the server). Terrain walkability (`is_walkable_at`) and Z elevation (`smooth_surface_at`) are computed locally — no server round-trip needed.

The computed `pos: [f32; 3]` is included in every `PlayerInput` message sent to the server.

`PredictedPosition` drives the local player's Bevy `Transform` for zero-latency visual feedback. Remote entities use the replicated `WorldPosition`.

## Server-side input processing

`process_player_input` reads `MessageReceiver<PlayerInput>` on each `ClientOf` entity. It accepts the client's sent `pos` directly as the new `WorldPosition` (client is authoritative for XY and Z). A `PositionSanityTimer` component tracks how long the position has been in non-walkable terrain; after 10 continuous seconds it snaps the player back to the last valid position.

## Connection lifecycle

On `Add<Connected>` (netcode handshake complete), the server spawns a player entity with `WorldPosition`, `Health`, `CombatParticipant`, `Experience`, `WorldMeta`, `PositionSanityTimer`, and `Replicate::to_clients(NetworkTarget::All)`, then attaches `PlayerEntity(player)` to the `ClientOf` entity.

The client's `TerrainPlugin` watches for `WorldMeta` arriving on the replicated player entity. If the seed or dimensions differ from the default, it regenerates `WorldMap` and marks all terrain chunks dirty for re-render.

On `Add<LinkOf>`, a `ReplicationSender` is added to the link entity so the server can push updates to that specific client.

## BRP observability

Both the server and headless client expose the Bevy Remote Protocol (BRP) — a JSON-RPC HTTP API built into Bevy.

Built-in endpoints:
- `bevy/query` — query entities by component
- `bevy/get` — read components on a specific entity
- `bevy/insert` / `bevy/spawn` / `bevy/destroy` — mutate ECS

The `ralph` tool uses these to assert live game state in end-to-end scenarios. Scenarios:

| Scenario | What it checks |
|---|---|
| `basic_movement` | At least one `WorldPosition` entity exists after a client connects |
| `combat_resolves` | At least one entity has `Health.current < Health.max` (damage landed) |
| `player_moves` | The player's `WorldPosition` changes by > 0.1 units over ~4 s |

## Headless automation

The headless client (`--headless`) runs two automation systems instead of keyboard input:

- **`headless_auto_attack`** — sends `BasicAttack` every 2 s (drives `combat_resolves`)
- **`headless_auto_move`** — walks right 3 s / left 3 s repeating at `PLAYER_SPEED` (drives `player_moves`); no terrain checks since `WorldMap` is not loaded in headless mode

Both systems read `PredictedPosition` (seeded from the first replicated `WorldPosition`) to keep the sent `pos` accurate.

## Smoke-test script

`scripts/smoke_test.sh` builds the workspace, starts server + headless client, runs all ralph scenarios, dumps logs on failure, and exits with ralph's exit code:

```bash
bash scripts/smoke_test.sh
```

Debug builds of the client support `bevy-inspector-egui` behind the `debug` feature flag: `cargo run -p fellytip-client --features debug`.
