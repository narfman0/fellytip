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

All five are serializable (`serde`) and reflectable (`bevy::reflect`).

Players carry `WorldMeta` but not `EntityKind`. Absence of `EntityKind` on a replicated entity indicates a player.

## Messages

| Message | Direction | Channel |
|---|---|---|
| `PlayerInput` | Client → Server | Unordered unreliable UDP |
| `GreetMsg` | Server → Client | Ordered reliable |

`PlayerInput` carries `move_dir: [f32; 2]`, `pos: [f32; 3]` (client-computed position), an optional `ActionIntent`, and an optional target UUID.

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

The `ralph` tool uses these to assert live game state in end-to-end scenarios. Example: after pressing Space, ralph queries `Health` on the boss entity and asserts it decreased.

Debug builds of the client support `bevy-inspector-egui` behind the `debug` feature flag: `cargo run -p fellytip-client --features debug`.
