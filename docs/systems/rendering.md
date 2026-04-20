# System: Rendering

Rendering is client-only. The server has no rendering code. Simulation, networking, and combat are unaffected by the rendering mode.

Specific numeric values (angles, lux values, render radius, mesh dimensions) are defined in the source files listed below — those are the authority.

## Current state

The client runs in two modes:

**Windowed** — default; uses `DefaultPlugins` with a 3D PBR render pipeline. An orbit camera, scene lighting, and a smooth chunked terrain mesh are active.

**Headless** — `cargo run -p fellytip-client -- --headless`. Uses `MinimalPlugins` with no window. Used for automated testing via BRP. All game logic still runs (world sim, combat, AI); it just doesn't display anything.

## Camera (`crates/client/src/plugins/camera.rs`)

`OrbitCameraPlugin` spawns a single `Camera3d` with an `OrbitCamera` component locked to classic isometric angles (yaw=45°, pitch=35.3°). The target starts at the centre of the world map.

| Control | Action |
|---|---|
| Scroll wheel | Zoom in/out |
| Right/Middle-click drag | Orbit (yaw + pitch) — only when `orbit_locked = false` |

`OrbitCamera.orbit_locked` is `true` by default, giving a fixed isometric perspective. Set it to `false` at runtime (e.g. in a debug menu) to restore free orbit for development.

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
| `Full`    | 1 | 33 | < 80 units    |
| `Half`    | 2 | 17 | 80–192 units  |
| `Quarter` | 4 |  9 | 192–320 units |
| `Eighth`  | 8 |  5 | ≥ 320 units   |

Distance thresholds are tuned so that at the maximum camera zoom the entire visible area fits within `render_radius = 20` chunks.

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

## Billboard sprite renderer (`crates/client/src/plugins/billboard_sprite.rs`)

`BillboardSpritePlugin` provides 2.5D character rendering: flat quad sprites in the 3D world, always facing the camera. Designed for AI-generated 8-direction animated sprite sheets.

**Data pipeline:**
1. `assets/bestiary.toml` defines every entity type with prompts, directions, and animation clips.
2. `cargo run -p sprite_gen -- --all` generates 8-direction sprite sheets:
   - `crates/client/assets/sprites/{entity_id}/atlas.png` — full atlas (8 rows × N cols).
   - `crates/client/assets/sprites/{entity_id}/manifest.json` — layout for the renderer.
3. At game startup, `load_sprite_registry` scans for manifests and populates `SpriteRegistry`.
4. For each new entity whose kind matches a registry entry, `spawn_billboard_visuals` (in `PreUpdate`) adds `HasSpriteSheet` and spawns a companion `Mesh3d` quad entity.
5. `update_billboard` (in `Update`) selects the atlas frame via `StandardMaterial.uv_transform` (scale + translate into the atlas UV space) and reorients the quad to face the camera each frame.

**Atlas layout:** Rows = facing directions (0 = south, clockwise). Columns = concatenated animation frames (idle | walk | attack | death …).

**Graceful degradation:** If no sprite sheets are present, `SpriteRegistry` is empty. `HasSpriteSheet` is never added to any entity, and `EntityRendererPlugin` renders everything as 3D GLB models as before.

**Direction quantization:**
- Compute velocity from frame-to-frame translation delta.
- Rotate velocity into screen space (subtract camera yaw).
- Snap to nearest of 8 octants.
- 0 = south, 1 = SW, 2 = W, 3 = NW, 4 = N, 5 = NE, 6 = E, 7 = SE.

**Generating sprites:**
```bash
# Mock backend (instant, coloured placeholders)
cargo run -p sprite_gen -- --all --output crates/client/assets/sprites/

# DALL-E 3 (requires OpenAI API key)
cargo run -p sprite_gen -- --all --backend dalle --api-key sk-... --workers 4

# Incremental: skip up-to-date entities
cargo run -p sprite_gen -- --all --incremental
```

## Entity rendering (`crates/client/src/plugins/entity_renderer.rs`)

`EntityRendererPlugin` spawns a PBR mesh for each replicated entity that carries `WorldPosition`. Visual appearance is determined by `EntityKind` and the new `FactionBadge` component:

| Entity type | Mesh | Colour |
|---|---|---|
| Player (no `EntityKind`) | capsule | warm gold |
| `FactionNpc` with `FactionBadge` (iron_wolves) | capsule | steel blue |
| `FactionNpc` with `FactionBadge` (merchant_guild) | capsule | amber |
| `FactionNpc` with `FactionBadge` (ash_covenant) | capsule | crimson |
| `FactionNpc` with `FactionBadge` (deep_tide) | capsule | deep teal |
| `FactionNpc` without badge (fallback) | capsule | steel blue |
| `Wildlife` | capsule | forest green |
| `Settlement` | pillar (Cylinder3d, 3 units tall) | bright white |

`FactionBadge { faction_id: String, rank: NpcRank }` is replicated from the server so the client can select the faction-specific material at spawn time.

A system (`sync_remote_transforms`) updates the Bevy `Transform` every frame from `WorldPosition`, using the same coordinate mapping as the terrain. The local player's transform is driven by `PredictedPosition` instead for zero-latency movement.

`GrowthStage` (replicated `f32` in [0.0, 1.0]) drives capsule scale: `scale = 0.3 + 0.7 × GrowthStage`. Newborn NPCs appear at 30% capsule size and grow to full size over 300 world-sim ticks (~5 minutes). `sync_growth_stage_scale` updates the `Transform` on each change.

## Battle visualizations (`crates/client/src/plugins/battle.rs`)

`BattleVisualsPlugin` (windowed only) subscribes to server battle messages and renders:

- **Battle ring** — a `Torus` mesh at the battle site using translucent red `AlphaMode::Blend` material. Its alpha pulses via `0.25 + 0.25 × sin(phase)` at 2 rad/s. One ring per active battle; despawned when `BattleEndMsg` arrives.
- **Battle Log** — a rolling 50-entry `BattleLog` resource of human-readable event strings, written by `on_battle_start` and `on_battle_end`.
- **Client Story Log** — a rolling 20-entry `ClientStoryLog` resource populated by `on_story_msg` when `StoryMsg` broadcasts arrive from the server.

Messages are received via the lightyear client `MessageReceiver<T>` pattern on the `Client` entity.

## HUD (`crates/client/src/plugins/hud.rs`)

`HudPlugin` draws four egui windows via `bevy_egui`. Only added in windowed mode.

| Panel | Anchor | Contents |
|---|---|---|
| `##stats` | Bottom-left | HP bar + XP progress bar + level |
| `Faction Standing` | Top-left | Per-faction reputation score + tier, colour-coded |
| `Battle Log` | Top-right | Last 20 battle events from `BattleLog` |
| `World Events` | Bottom-right | Last 10 story events from `ClientStoryLog` |

**Faction Standing**: reads `PlayerStandings` from the local player entity (the component is replicated from the server and refreshed every world-sim tick). Tier colours: green (Friendly+), grey (Neutral), orange (Unfriendly), red (Hostile/Hated). Hidden until at least one standing is available.

**World Events**: hidden until at least one story event has been received.

Controls: Space → BasicAttack; Q → StrongAttack (ability 1).

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
