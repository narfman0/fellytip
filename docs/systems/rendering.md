# System: Rendering

Rendering is client-only. The server has no rendering code. Simulation, networking, and combat are unaffected by the rendering mode.

Specific numeric values (angles, lux values, render radius, mesh dimensions) are defined in the source files listed below — those are the authority.

## Current state

The client runs in two modes:

**Windowed** — default; uses `DefaultPlugins` with a 3D PBR render pipeline. An orbit camera, scene lighting, and a rolling tile mesh grid are active.

**Headless** — `cargo run -p fellytip-client -- --headless`. Uses `MinimalPlugins` with no window. Used for automated testing via BRP. The client still connects to the server, sends input, and receives replicated state — it just doesn't display anything.

## Camera (`crates/client/src/plugins/camera.rs`)

`OrbitCameraPlugin` spawns a single `Camera3d` with an `OrbitCamera` component. The default angles give the classic isometric look; the target starts at the centre of the world map.

| Control | Action |
|---|---|
| Right-click drag | Orbit (yaw + pitch) |
| Middle-click drag | Orbit (yaw + pitch) |
| Scroll wheel | Zoom in/out |

Orbit state (yaw, pitch, distance, target) lives in `OrbitCamera`. The Bevy `Transform` is recomputed from those values every frame. Input is read from `AccumulatedMouseMotion` and `AccumulatedMouseScroll` resources (Bevy 0.18 input API).

## Lighting (`crates/client/src/plugins/scene_lighting.rs`)

`SceneLightingPlugin` spawns two light sources:
- **DirectionalLight** — bright warm-white sun, angled down from upper-left. Shadows disabled (can be toggled later).
- **AmbientLight** — dim sky-blue fill to keep unlit faces readable. Spawned as a component entity (Bevy 0.18 ambient light API).

## Tile rendering (`crates/client/src/plugins/tile_renderer.rs`)

`TileRendererPlugin` renders the world as flat PBR cuboids.

### World map on client

The client regenerates `WorldMap` locally at startup using `generate_map(WORLD_SEED)` — the same pure deterministic function the server uses. This avoids replicating the full terrain over the network. `WORLD_SEED` is defined in `crates/shared/src/lib.rs`.

### Mesh

All tiles share one `Mesh` handle (a flat cuboid). The top face sits at Bevy Y = `z_top`; the mesh center is slightly below that. Exact dimensions are in `setup_tile_assets`.

### Coordinate mapping

```
world (x, y, z_elevation) → Bevy (x, z_elevation, y)
```

Bevy is Y-up; the game's elevation axis maps to Bevy Y. World Y (north) becomes Bevy Z (depth).

### Materials

One `StandardMaterial` per `TileKind` (see `material_for` in `tile_renderer.rs`). Same biome → same handle → Bevy automatic GPU instancing. Water and River tiles use `AlphaMode::Blend`. `LuminousGrotto` has a teal emissive glow.

### Rolling window

A rolling square of radius `RENDER_RADIUS` (defined in `tile_renderer.rs`) around the orbit camera target. The grid rebuilds only when the camera target crosses a tile boundary. Tiles leaving the window are despawned; tiles entering it are spawned. The topmost surface layer of each column is rendered; underground layers become visible only when the camera descends below the surface.

## Upgrade path

### Textures

Replace `material_for(kind)` definitions with `base_color_texture: Some(asset_server.load(...))`. The mesh and instancing setup are unchanged.

### Player / entity rendering

Spawn a mesh entity with a `WorldPosition` observer that writes to `Transform` using the same coordinate mapping as tile rendering.

### Shadow maps

Set `shadows_enabled: true` on the `DirectionalLight` spawn in `scene_lighting.rs`. No other changes needed.

### Isometric vs top-down

`camera_transform` in `camera.rs` computes position from `(yaw, pitch, distance)`. Switching between top-down and isometric is a runtime `pitch` change with no code modifications.

## Debug inspector

Debug builds include `bevy-inspector-egui` behind the `debug` feature flag:

```bash
cargo run -p fellytip-client --features debug
```

This opens the ECS world inspector, showing all entities, their components, and live component values (including `WorldPosition.z` updating in real time as a player moves over terrain).
