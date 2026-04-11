# System: Rendering

Rendering is client-only. The server has no rendering code. Simulation, networking, and combat are unaffected by the rendering mode.

## Current state

The client can run in two modes:

**Windowed** — default; renders with Bevy's `DefaultPlugins`. Tile and sprite rendering is a work in progress; the coordinate system and projection functions are in place.

**Headless** — `cargo run -p fellytip-client -- --headless`. Uses `MinimalPlugins` with no window. Used for automated testing via BRP. The client still connects to the server, sends input, and receives replicated state — it just doesn't display anything.

## Coordinate system

World-space coordinates map to screen pixels via one of two projection functions in `crates/shared/src/math.rs`. These functions have no Bevy dependency and can be tested independently.

**Top-down** (default):
```
screen_x = world_x × TILE_W
screen_y = world_y × TILE_H
```

**Isometric** (behind `isometric` feature flag):
```
screen_x = (world_x − world_y) × (TILE_W / 2)
screen_y = (world_x + world_y) × (TILE_H / 4) + world_z × (TILE_H / 2)
```

`TILE_W = 32`, `TILE_H = 16` pixels.

The isometric projection includes the `z` elevation offset so entities at different heights appear at different vertical positions on screen. A character in the Underdark would appear far below a surface character at the same `(x, y)` tile.

## Upgrade path

Switching from top-down to isometric requires changing only the `sync_transform` system on the client — the function that copies `WorldPosition` into Bevy's `Transform`. Everything else (simulation, networking, combat, world gen) is untouched. The `isometric` cargo feature gates the swap.

## Tile rendering

Not yet implemented. The `WorldMap` resource is available on the server; once a client-side tile cache is added, the client will request tile data for the visible region on spawn and re-request on region transitions. The `WorldMap` is not replicated in bulk — it is too large (~3 MB uncompressed for 512×512).

## Debug inspector

Debug builds include `bevy-inspector-egui` behind the `debug` feature flag:

```bash
cargo run -p fellytip-client --features debug
```

This opens the ECS world inspector, showing all entities, their components, and live component values (including `WorldPosition.z` updating in real time as a player moves over terrain).
