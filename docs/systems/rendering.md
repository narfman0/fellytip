# System: Rendering

Rendering is client-only. The server has no rendering code. Simulation, networking, and combat are unaffected by the rendering mode.

## Current state

The client runs in two modes:

**Windowed** — default; uses `DefaultPlugins` with a 3D PBR render pipeline. An orbit camera, scene lighting, and a rolling tile mesh grid are active.

**Headless** — `cargo run -p fellytip-client -- --headless`. Uses `MinimalPlugins` with no window. Used for automated testing via BRP. The client still connects to the server, sends input, and receives replicated state — it just doesn't display anything.

## Camera

`OrbitCameraPlugin` (`crates/client/src/plugins/camera.rs`) spawns a single `Camera3d` with an `OrbitCamera` component.

| Control | Action |
|---|---|
| Right-click drag | Orbit (yaw + pitch) |
| Middle-click drag | Orbit (yaw + pitch) |
| Scroll wheel | Zoom in/out |

Default position: 45° yaw, 35.3° pitch — the classic isometric angle. Target starts at the centre of the 512×512 world map at elevation ≈3 world units.

Orbit state is stored in `OrbitCamera` (yaw, pitch, distance, target). The Bevy `Transform` is recomputed from those values every frame. Uses `AccumulatedMouseMotion` and `AccumulatedMouseScroll` resources (Bevy 0.18 input API).

## Lighting

`SceneLightingPlugin` (`crates/client/src/plugins/scene_lighting.rs`):

- **DirectionalLight** — 50 000 lux, warm white (1.0, 0.97, 0.88), angled 45° down with 30° yaw. Shadows disabled (can be toggled later).
- **AmbientLight** — 300 lux, sky-blue tint (0.55, 0.65, 0.80). Spawned as a Bevy entity component (Bevy 0.18 ambient light API).

## Tile rendering

`TileRendererPlugin` (`crates/client/src/plugins/tile_renderer.rs`) renders the world as flat PBR cuboids.

### World map on client

The client regenerates `WorldMap` locally at startup using `generate_map(WORLD_SEED)` — the same pure deterministic function the server uses. This avoids replicating ~3 MB of terrain data. `WORLD_SEED` is defined in `crates/shared/src/lib.rs` and shared by both binaries.

### Mesh

All tiles share one `Mesh` handle: `Cuboid::new(1.0, 0.2, 1.0)` — 1 world unit wide, 0.2 tall, 1 deep. The top face sits at Bevy Y = `z_top` (the tile's walking surface height); the center is at `y = z_top − 0.1`.

### Coordinate mapping

```
world (x, y, z_elevation) → Bevy (x, z_elevation, y)
```

Bevy is Y-up; the game's elevation axis is mapped to Bevy Y. World Y (north) becomes Bevy Z (depth).

### Materials

One `StandardMaterial` per `TileKind`. Same biome → same handle → Bevy automatic GPU instancing. Materials use `perceptual_roughness` and `metallic` for PBR. Water and River tiles use `AlphaMode::Blend`. `LuminousGrotto` has a teal `emissive` glow. All 21 variants are covered.

### Rolling window

A ±20-tile square around the orbit camera target (41×41 = 1 681 tiles max). The grid rebuilds only when the camera target crosses a tile boundary (integer IVec2 comparison each frame). Tiles that leave the window are despawned; tiles entering it are spawned.

Each frame renders the topmost surface layer of each visible column. Underground layers (Cavern, DeepRock, LuminousGrotto) are visible only if the camera descends below the surface.

## Upgrade path

### Textures

Replace `material_for(kind)` material definitions with `base_color_texture: Some(asset_server.load("tiles/grassland.png"))`. The mesh and instancing setup are unchanged.

### Player / entity rendering

Spawn a mesh entity with a `WorldPosition` observer that writes to `Transform`. The coordinate mapping `(x, z_top, y)` already exists in the tile renderer and can be extracted to a shared helper.

### Shadow maps

Set `shadows_enabled: true` on the `DirectionalLight` and add `ShadowMap` configuration to the `DirectionalLight` entity. No other changes needed.

### Isometric vs top-down

`camera_transform` in `camera.rs` computes position from `(yaw, pitch, distance)`. Switching between top-down (pitch=PI/2) and isometric (pitch≈0.615) is a runtime parameter change with no code modifications. The `isometric` cargo feature flag described in the original plan is no longer needed.

## Debug inspector

Debug builds include `bevy-inspector-egui` behind the `debug` feature flag:

```bash
cargo run -p fellytip-client --features debug
```

This opens the ECS world inspector, showing all entities, their components, and live component values (including `WorldPosition.z` updating in real time as a player moves over terrain).
