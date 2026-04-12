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
| `WorldPosition { x, y, z }` | Full prediction + linear interpolation | Predicted on client; authoritative on server |
| `Health { current, max }` | Simple interpolation | Client renders health bars |
| `Experience { xp, level, xp_to_next }` | Simple interpolation | Client renders level/XP |
| `EntityKind` | Simple | Enum: FactionNpc / Wildlife / Settlement — drives visual differentiation on client |

All four are serializable (`serde`) and reflectable (`bevy::reflect`).

Players do **not** carry `EntityKind`. Its absence on a replicated entity indicates a player.

## Messages

| Message | Direction | Channel |
|---|---|---|
| `PlayerInput` | Client → Server | Unordered unreliable UDP |
| `GreetMsg` | Server → Client | Ordered reliable |

`PlayerInput` carries `move_dir: [f32; 2]`, an optional `ActionIntent`, and an optional target UUID.

## Client-side input

`send_player_input` runs in `FixedUpdate` on the client. It reads keyboard state (WASD / arrows for movement, Space for BasicAttack), normalises the direction vector, and sends a `PlayerInput` only when there is actual input. The `MessageSender<PlayerInput>` component is automatically added to the `Client` entity by Lightyear's registration.

## Server-side input processing

`process_player_input` reads `MessageReceiver<PlayerInput>` on each `ClientOf` entity, applies movement to the linked player entity's `WorldPosition`, and queues attacks. The `PlayerEntity(Entity)` component on the `ClientOf` entity provides the link.

Replication is pushed to all clients at the interval configured in `SendUpdatesMode` (see `crates/server/src/plugins/combat.rs`).

## Connection lifecycle

On `Add<Connected>` (netcode handshake complete), the server spawns a player entity with `WorldPosition`, `Health`, `CombatParticipant`, `Experience`, and `Replicate::to_clients(NetworkTarget::All)`, then attaches `PlayerEntity(player)` to the `ClientOf` entity.

On `Add<LinkOf>`, a `ReplicationSender` is added to the link entity so the server can push updates to that specific client.

## BRP observability

Both the server and headless client expose the Bevy Remote Protocol (BRP) — a JSON-RPC HTTP API built into Bevy.

Built-in endpoints:
- `bevy/query` — query entities by component
- `bevy/get` — read components on a specific entity
- `bevy/insert` / `bevy/spawn` / `bevy/destroy` — mutate ECS

The `ralph` tool uses these to assert live game state in end-to-end scenarios. Example: after pressing Space, ralph queries `Health` on the boss entity and asserts it decreased.

Debug builds of the client support `bevy-inspector-egui` behind the `debug` feature flag: `cargo run -p fellytip-client --features debug`.
