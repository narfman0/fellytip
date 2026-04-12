# System: Rendering

Rendering is client-only. The server has no rendering code. Simulation, networking, and combat are unaffected by the rendering mode.

Specific numeric values (angles, lux values, render radius, mesh dimensions) are defined in the source files listed below — those are the authority.

## Current state

The client runs in two modes:

**Windowed** — default; uses `DefaultPlugins` with a 3D PBR render pipeline. An orbit camera, scene lighting, and a smooth chunked terrain mesh are active.

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
- **DirectionalLight** — bright warm-white sun, angled down from upper-left. Shadow maps enabled.
- **AmbientLight** — sky-blue fill to keep unlit faces readable. Spawned as a component entity (Bevy 0.18 ambient light API).

## Terrain rendering (`crates/client/src/plugins/terrain/`)

`TerrainPlugin` renders the world as a smooth chunked terrain using PBR `Mesh` entities with vertex colors. It replaces the old flat-cuboid `TileRendererPlugin`.

### World map on client

The client regenerates `WorldMap` locally at startup using the same `generate_map(WORLD_SEED, MAP_WIDTH, MAP_HEIGHT)` call as the server — pure and deterministic, so no network transfer needed. The map is stored as a Bevy `Resource`.

### Chunk architecture

The world is divided into 32×32-tile **chunks** (`CHUNK_TILES = 32` in `lod.rs`). Each chunk is one `Mesh3d` entity. All chunk vertices are in world space; chunk entities use `Transform::IDENTITY`.

A single `StandardMaterial` (vertex colors, `base_color = WHITE`) is shared across all chunks, so biome color variation comes entirely from `Mesh::ATTRIBUTE_COLOR` — Bevy's PBR pipeline applies vertex colors automatically.

### Smooth heights

Every `TileLayer` stores `corner_offsets: [TL, TR, BL, BR]`, where each value is the average `z_top` contribution of the four tile centers sharing that corner. Because the formula is symmetric, **all four tiles touching a corner compute the same offset** — guaranteeing seamless heights across chunk boundaries.

The vertex at tile-grid position `(gx, gy)` uses `layer.z_top + layer.corner_offsets[0]` (TL corner of tile `(gx, gy)`). Per-vertex normals are computed via central differences over the height grid.

### Vertex colors

`corner_biome_color(map, gx, gy)` averages the biome colors of the four tiles sharing corner `(gx, gy)`. Colors for each `TileKind` are hardcoded linear-sRGB values in `material.rs` (matching the old `material_for()` table). No textures; all variation is vertex-color blending.

### LOD levels (`lod.rs`)

| `LodLevel` | Step | Vertices/side | Distance threshold |
|---|---|---|---|
| `Full`    | 1 | 33 | < 80 units   |
| `Half`    | 2 | 17 | < 192 units  |
| `Quarter` | 4 |  9 | ≥ 192 units  |

Distance thresholds are tuned so that at the maximum camera zoom (~400 units) the entire visible area fits within `render_radius = 13` chunks.

LOD transitions are **constrained to ±1 level** between neighbors (BFS clamping in `update_chunk_visibility`). Where a fine chunk borders a coarser one, T-junction stitching eliminates visible cracks: odd-indexed edge vertices are removed (whole triangles filtered) and replaced with T-collapse triangles.

### Coordinate mapping

```
tile grid (gx, gy, height) → Bevy (gx − half_w, height, gy − half_h)
```

Bevy is Y-up; terrain height maps to Bevy Y. Tile column index maps to Bevy X (east); tile row index maps to Bevy Z (south). The origin is the center of the map.

### Chunk lifecycle (`manager.rs`)

Three systems run in order every `Update` frame:

1. **`update_chunk_visibility`** — reads `OrbitCamera.target`, computes which chunks are within `render_radius`, selects LOD per chunk by distance, runs BFS LOD clamping, marks changed chunks as dirty, records out-of-range chunks for despawn.
2. **`rebuild_dirty_chunks`** — calls `build_chunk_mesh` for each dirty chunk coord + LOD, inserts the new `Mesh` into `Assets<Mesh>`, caches the handle.
3. **`apply_chunk_meshes`** — spawns new chunk entities, despawns out-of-range ones, swaps `Mesh3d` handles on entities whose LOD changed.

## Entity rendering (`crates/client/src/plugins/entity_renderer.rs`)

`EntityRendererPlugin` spawns a PBR mesh for each replicated entity that carries `WorldPosition`. Visual appearance is determined by the optional `EntityKind` component:

| `EntityKind`  | Mesh     | Colour       |
|---------------|----------|--------------|
| absent        | capsule  | warm gold    | ← player
| `FactionNpc`  | capsule  | steel blue   |
| `Wildlife`    | capsule  | forest green |
| `Settlement`  | pillar (Cylinder3d, 3 units tall) | bright white |

A system (`sync_remote_transforms`) updates the Bevy `Transform` every frame from `WorldPosition`, using the same coordinate mapping as the terrain. The local player's transform is driven by `PredictedPosition` instead for zero-latency movement.

`GrowthStage` (replicated `f32` in [0.0, 1.0]) drives capsule scale: `scale = 0.3 + 0.7 × GrowthStage`. Newborn NPCs appear at 30% capsule size and grow to full size over 300 world-sim ticks (~5 minutes). `sync_growth_stage_scale` updates the `Transform` on each change.

## Battle visualizations (`crates/client/src/plugins/battle.rs`)

`BattleVisualsPlugin` (windowed only) subscribes to server battle messages and renders:

- **Battle ring** — a `Torus` mesh at the battle site using translucent red `AlphaMode::Blend` material. Its alpha pulses via `0.25 + 0.25 × sin(phase)` at 2 rad/s. One ring per active battle; despawned when `BattleEndMsg` arrives.
- **Battle Log** — a rolling 50-entry `BattleLog` resource of human-readable event strings, written by `on_battle_start` and `on_battle_end`.

Messages are received via the lightyear client `MessageReceiver<T>` pattern on the `Client` entity.

## HUD (`crates/client/src/plugins/hud.rs`)

`HudPlugin` draws two egui windows via `bevy_egui`. Only added in windowed mode.

- **Bottom-left panel** (`##stats`): HP bar (`Health.current / Health.max`) + XP progress bar (`Experience.xp / xp_to_next`) + level label. Populated from the first replicated entity that has both `Replicated` and `Experience` (the local player). Shows "Connecting…" before the player entity arrives.
- **Top-right panel** (`Battle Log`): last 20 entries from `BattleLog`, newest first. Shows active faction battles.
- Controls: Space → BasicAttack; Q → StrongAttack (ability 1).

## Upgrade path

### Textures

Replace the hardcoded `biome_color(kind)` values in `material.rs` with `base_color_texture` handles. The mesh and LOD setup are unchanged.

### Isometric vs top-down

`camera_transform` in `camera.rs` computes position from `(yaw, pitch, distance)`. Switching between top-down and isometric is a runtime `pitch` change with no code modifications.

## Debug inspector

Debug builds include `bevy-inspector-egui` behind the `debug` feature flag:

```bash
cargo run -p fellytip-client --features debug
```

This opens the ECS world inspector, showing all entities, their components, and live component values (including `WorldPosition.z` updating in real time as a player moves over terrain).
