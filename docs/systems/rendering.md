# System: Rendering

Rendering is client-only. The server has no rendering code. Simulation, networking, and combat are unaffected by the rendering mode.

Specific numeric values (angles, lux values, render radius, mesh dimensions) are defined in the source files listed below — those are the authority.

## Current state

The client runs in two modes:

**Windowed** — default; uses `DefaultPlugins` with a 3D PBR render pipeline. An orbit camera, scene lighting, and a smooth chunked terrain mesh are active.

**Headless** — `cargo run -p fellytip-client -- --headless`. Uses `MinimalPlugins` with no window. Used for automated testing via BRP. All game logic still runs (world sim, combat, AI); it just doesn't display anything.

## Camera (`crates/client/src/plugins/camera.rs`)

`OrbitCameraPlugin` spawns a single `Camera3d` with an `OrbitCamera` component. The default angles give the classic isometric look; the target starts at the centre of the world map.

| Control | Action |
|---|---|
| Right-click drag | Orbit (yaw + pitch) — **disabled when `locked_iso` is enabled** |
| Middle-click drag | Orbit (yaw + pitch) — **disabled when `locked_iso` is enabled** |
| Scroll wheel | Zoom in/out |

Orbit state (yaw, pitch, distance, target) lives in `OrbitCamera`. The Bevy `Transform` is recomputed from those values every frame. Input is read from `AccumulatedMouseMotion` and `AccumulatedMouseScroll` resources (Bevy 0.18 input API).

### `locked_iso` feature flag

Cargo feature `locked_iso` on the `fellytip-client` crate locks the camera to a fixed-orientation dimetric isometric view — `yaw = ISO_YAW (45°)`, `pitch = ISO_PITCH (≈35.264°, atan(1/√2))`. Drag handlers are compiled out; only scroll zoom remains. This is the intended mode for the billboard-sprite art pipeline (see issue #13). Default builds keep the orbit handlers so top-down debugging via drag still works.

```bash
cargo run -p fellytip-client --features locked_iso
```

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

## Bestiary (`assets/bestiary.toml`, `crates/shared/src/bestiary.rs`)

`assets/bestiary.toml` is the single source of truth for all entity sprite definitions. `crates/shared/src/bestiary.rs` provides pure-data types (`Bestiary`, `BestiaryEntry`, `AnimationDef`) — no ECS, no I/O — so `sprite_gen` can load it without Bevy.

### TOML schema

Each `[[entity]]` block:

| Field | Type | Description |
|---|---|---|
| `id` | string | Stable lookup key; used as the directory name under `crates/client/assets/sprites/` |
| `display_name` | string | Human-readable name used in AI prompts |
| `directions` | int | Facing directions: `4` or `8` |
| `ai_prompt_base` | string | Subject description passed to the AI image backend |
| `ai_style` | string | Style suffix appended to every frame prompt |
| `palette_seed` | string | Named seed for colour quantisation (keeps colours consistent across frames) |
| `animations` | array | Ordered list of `AnimationDef` |

Each `AnimationDef`:

| Field | Type | Description |
|---|---|---|
| `name` | string | `idle`, `walk`, `attack`, `death` |
| `frames` | int | Frame count for this clip |
| `fps` | int | Playback speed |

**Adding a new creature:**
1. Add one `[[entity]]` block to `assets/bestiary.toml`.
2. Run: `cargo run -p sprite_gen -- --entity <id>`
3. Commit the generated `atlas.png` + `manifest.json`.

### sprite_gen tool (`tools/sprite_gen/`)

Reads bestiary, calls an AI backend per frame, stitches frames into an atlas, writes a JSON manifest.

```bash
# Mock backend — instant coloured placeholders, no API key needed
cargo run -p sprite_gen -- --all

# DALL-E 3 — real art
cargo run -p sprite_gen -- --all --backend dalle --api-key sk-... --workers 4

# Incremental — skip entities whose atlas is newer than bestiary.toml
cargo run -p sprite_gen -- --all --incremental

# Single entity
cargo run -p sprite_gen -- --entity player

# Dry-run — print prompts only, generate nothing
cargo run -p sprite_gen -- --all --dry-run
```

Outputs per entity (written to `crates/client/assets/sprites/{id}/`):
- `atlas.png` — `directions`-row × N-col sprite sheet
- `manifest.json` — frame layout consumed by `BillboardSpritePlugin`

## Billboard sprites (`crates/client/src/plugins/billboard_sprite.rs`)

`BillboardSpritePlugin` reads `assets/bestiary.toml`, loads each entity's atlas PNG from `crates/client/assets/sprites/`, slices it into per-cell Bevy `Image`s + per-cell `StandardMaterial`s (unlit, alpha-blended), and spawns a billboard quad child for every replicated `WorldPosition` entity whose kind has a loaded atlas.

Atlas selection is `EntityKind`-aware. `atlas_id_for_entity(kind, badge, wildlife)` in `billboard_sprite.rs` returns:

- `"hero"` when no `EntityKind` is present (the player).
- `"{faction_id}_npc"` for `EntityKind::FactionNpc` — composed from the replicated `FactionBadge`.
- `"bison"` / `"dog"` / `"horse"` for `EntityKind::Wildlife` (keyed on `WildlifeKind`).
- `None` for `EntityKind::Settlement` (buildings stay on the PBR pipeline).

`EntityRendererPlugin::spawn_entity_visuals` consults the same helper and **skips PBR mesh insertion** when `BillboardSprites` already has an atlas loaded for that entity kind — so billboard and PBR renderers no longer stack on the same entity. Entities whose atlas is missing (e.g. a new bestiary entry without generated art yet) still fall through to the PBR path.

Per frame:
- `face_camera` rotates each sprite's local transform to `Quat::from_rotation_y(camera.yaw)`.
- `update_direction` uses `fellytip_shared::sprite_math::world_dir_to_sprite_row(velocity, camera.yaw, directions)` to pick the atlas row.
- `advance_animation` ticks a per-entity frame timer at the animation's declared fps and wraps on overflow.
- `swap_cell_material` points the entity's `MeshMaterial3d` at the cell material for the current `(row, frame)`.

Direction math lives in `crates/shared/src/sprite_math.rs` so it's ECS-free and covered by unit tests (cardinal distinctness, antipodal symmetry, monotonic CCW sweep, camera-yaw rotation invariance).

### Bestiary coverage

`assets/bestiary.toml` declares one entry per in-game entity kind. The drift-guard test `fellytip_shared::bestiary::bestiary_covers_all_entity_kinds` fails if a new `EntityKind`/`WildlifeKind`/faction variant is added without a matching `[[entity]]` block.

| Bestiary id | Renderer maps from | Prompt theme |
|---|---|---|
| `hero` | player (no `EntityKind`) | adventurer hero, sword and shield |
| `iron_wolves_npc` | `FactionNpc` + `faction_id="iron_wolves"` | steel-blue clan warrior |
| `merchant_guild_npc` | `FactionNpc` + `faction_id="merchant_guild"` | amber caravaneer |
| `ash_covenant_npc` | `FactionNpc` + `faction_id="ash_covenant"` | crimson zealot |
| `deep_tide_npc` | `FactionNpc` + `faction_id="deep_tide"` | teal coastal mariner |
| `bison` | `Wildlife` + `WildlifeKind::Bison` | plains bison |
| `dog` | `Wildlife` + `WildlifeKind::Dog` | wild dog |
| `horse` | `Wildlife` + `WildlifeKind::Horse` | chestnut riding horse |

In addition to the above, `assets/bestiary.toml` now declares 15 D&D SRD Tier 1 monsters (Goblin, Kobold, Orc, Hobgoblin, Bugbear, Skeleton, Zombie, Ghoul, Owlbear, Troll, Giant Spider, Giant Rat, Gelatinous Cube, Hill Giant, Young Red Dragon) so the atlas is already generated before gameplay code hooks each one up. The old `goblin_scout` placeholder has been removed.

`Settlement` entities stay on the PBR pipeline — static buildings don't need billboard animation.

## Entity rendering (`crates/client/src/plugins/entity_renderer.rs`)

`EntityRendererPlugin` spawns a PBR mesh for each replicated entity that carries `WorldPosition`. Visual appearance is determined by `EntityKind` and the new `FactionBadge` component:

| Entity type | Mesh | Colour |
|---|---|---|
| Player (no `EntityKind`) | Kenney `characterMedium` GLB | — |
| `FactionNpc` | Kenney character GLBs, tinted per faction | per faction |
| `Wildlife` | Kenney animal GLBs (bison / dog / horse) | — |
| `Settlement` (Town) | Kenney tent GLBs (`tent_detailedClosed`, `tent_smallClosed`) | — |
| `Settlement` (Capital) | Kenney town GLBs (`windmill`, `stall-green`, `stall-red`) | — |

Settlement visual selection is keyed on the `SettlementKind` component (now also a Bevy `Component`) attached to each settlement marker entity at spawn. Capitals use larger windmill/stall assets; towns use tent assets. The specific scene is chosen deterministically from the entity id hash.

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

`HudPlugin` draws egui windows via `bevy_egui`. Only added in windowed mode.

| Panel | Trigger | Anchor | Contents |
|---|---|---|---|
| `##stats` | always | Bottom-left | HP bar + XP progress bar + level |
| `Battle Log` | always | Top-left | Last 20 battle events from `BattleLog` |
| `World Events` | always | Bottom-right | Last 10 story events from `ClientStoryLog` |
| `Character` | `C` key toggle | Centre | Detailed stats grid + faction standings |

**Character screen** (`C` key): shows a centred overlay with level, HP, XP, and per-faction reputation scores with tier colours. Blocks movement input while open. Ignored when the debug console or pause menu is open.

**Faction standings** were moved from the always-visible top-left panel into the `Character` screen.

**World Events**: hidden until at least one story event has been received.

Controls: Space → BasicAttack; Q → StrongAttack (ability 1); C → Character screen.

## Minimap (`crates/client/src/plugins/map.rs`)

An always-visible 180×180 px minimap anchors to the top-right corner. It rotates so the player's forward direction is always at the top.

- **Terrain**: rendered as a rotated textured quad mesh using computed UV coordinates per canvas corner (inverse rotation by camera yaw). The 512×512 terrain texture is generated once from `WorldMap` on startup.
- **Settlement dots**: Capital (gold, 4 px) and Town (white, 3 px) dots are drawn at rotated canvas positions.
- **Player dot**: red circle at canvas centre.
- **Forward arrow**: white line pointing straight up (the map rotates, so "up" is always forward).
- **North indicator**: small "N" label at the rotated north direction.
- **Coordinates + nearby settlement**: shown below the canvas.

The `M` / `Tab` keys open a full 512×512 pan-zoom map window.

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
