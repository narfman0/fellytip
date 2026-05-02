# Plan: Unified Asset Pipeline in Character Studio

## Goal

`tools/character_studio` is the single entry point for the full entity asset pipeline.
`tools/mesh_gen` is removed — all Meshy API code lives in character_studio.

---

## Pipeline Stages

Each entity moves through stages sequentially. Files on disk determine the actual stage shown at startup.

```
Stage 1: Draft     — entity in bestiary, nothing generated
Stage 2: Sprite    — 2D AI generation in progress
Stage 3: Approved  — base.png saved
Stage 4: Mesh      — Meshy text-to-3d (preview + refine, 10k target)
Stage 5: Rigged    — Meshy rigging API (humanoids only)
Stage 6: Animated  — Meshy animation API (per-clip, parallel)
Stage 7: Merged    — gltf-transform merge → {id}.glb
Stage 8: LODs      — gltf-transform simplify (optional, LOD1/2/3)
```

### Stage badges in entity list

| Badge | Stage         |
|-------|---------------|
| (none)| Draft         |
| ⏳    | generating    |
| 🖼    | Approved (sprite done, no mesh) |
| 🔷    | Mesh done     |
| 🦴    | Rigged        |
| 🎬    | Animated/Merged |
| ✅    | LODs complete |

### Files on disk per entity

```
assets/sprites/{id}/base.png              ← Stage 3
assets/models/{id}/{id}_mesh.glb          ← Stage 4
assets/models/{id}/{id}_rigged.glb        ← Stage 5
assets/models/{id}/{anim_name}.glb        ← Stage 6 (one per clip)
assets/models/{id}/{id}.glb              ← Stage 7 (merged, LOD0)
assets/models/{id}/{id}_lod1.glb         ← Stage 8
assets/models/{id}/{id}_lod2.glb         ← Stage 8
assets/models/{id}/{id}_lod3.glb         ← Stage 8
```

Each stage is idempotent: if the output file exists, the stage is skipped. Delete the file to re-run that stage.

### Pipeline sidecar: `pipeline_state.json`

Saved at `assets/models/{id}/pipeline_state.json`. Persists Meshy task IDs so in-flight jobs survive studio restarts.

```json
{
  "preview_task_id": "task_abc123",
  "refine_task_id":  "task_def456",
  "rig_task_id":     "task_ghi789",
  "animation_task_ids": {
    "idle":   "task_anim_0",
    "walk":   "task_anim_1",
    "attack": "task_anim_4",
    "death":  "task_anim_8",
    "run":    "task_anim_14",
    "behit":  "task_anim_7"
  }
}
```

On startup: load `pipeline_state.json` for each entity. If a task ID exists but its output file does not, resume polling that existing task instead of resubmitting. Written to disk immediately after each Meshy submit call returns a task ID.

---

## Meshy API Endpoints Used

Base URL: `https://api.meshy.ai/openapi`
Auth: `Authorization: Bearer {MESHY_API_KEY}`

| Stage | Endpoint | Key params |
|-------|----------|------------|
| 4 | `POST /v2/text-to-3d` (mode: preview) | `prompt`, `target_polycount: 10000`, `should_remesh: true`, `pose_mode: "t-pose"` |
| 4 | `POST /v2/text-to-3d` (mode: refine) | `preview_task_id`, `enable_pbr: true` |
| 5 | `POST /v1/rigging` | `input_task_id` (refine task), `height_meters: 1.7` |
| 6 | `POST /v1/animations` | `rig_task_id`, `action_id` (integer) |
| poll | `GET /v2/text-to-3d/{id}` or `/v1/rigging/{id}` or `/v1/animations/{id}` | — |

### Default animation set (action IDs)

```
0   = Idle
1   = Walk
4   = Attack
8   = Dead
14  = Run
7   = BeHit (hit reaction)
```

The bestiary `[[entity]]` block should gain an optional `animation_ids` field to override defaults per entity (e.g., `112 = Monster_Walk` for undead).

### Meshy test key

`msy_dummy_api_key_for_test_mode_12345678` — works on all endpoints, returns sample data, consumes no credits. Used for mock backend in studio.

---

## Local Tool Steps (gltf-transform)

Requires Node.js + `npx @gltf-transform/cli` (no install needed with npx).

### Stage 7: Merge animation clips

```bash
npx @gltf-transform/cli merge \
  assets/models/{id}/{id}_rigged.glb \
  assets/models/{id}/idle.glb \
  assets/models/{id}/walk.glb \
  assets/models/{id}/attack.glb \
  assets/models/{id}/death.glb \
  assets/models/{id}/{id}.glb
```

### Stage 8: Generate LODs

```bash
npx @gltf-transform/cli simplify \
  --ratio 0.5 assets/models/{id}/{id}.glb assets/models/{id}/{id}_lod1.glb

npx @gltf-transform/cli simplify \
  --ratio 0.25 assets/models/{id}/{id}.glb assets/models/{id}/{id}_lod2.glb

npx @gltf-transform/cli simplify \
  --ratio 0.1 assets/models/{id}/{id}.glb assets/models/{id}/{id}_lod3.glb
```

LOD generation preserves skinning weights (meshopt-based simplification).

---

## Bestiary Schema Changes

Add optional fields to `BestiaryEntry`:

```toml
[[entity]]
id             = "goblin"
display_name   = "Goblin"
ai_prompt_base = "..."
ai_style       = "standard"
palette_seed   = "goblin_green"
animation_ids  = [0, 1, 4, 8, 14, 7]   # optional, defaults applied if absent
mesh_prompt    = "..."                   # optional, overrides ai_prompt_base for 3D gen
```

---

## State Model (character_studio/src/studio/app.rs)

### New fields on StudioApp

```rust
entity_mesh: HashMap<usize, MeshGenState>,
entity_stage: HashMap<usize, PipelineStage>,
meshy_available: bool,   // MESHY_API_KEY set
models_dir: PathBuf,     // assets/models/
```

### PipelineStage

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipelineStage {
    Draft,
    Approved,      // base.png exists
    MeshDone,      // _mesh.glb exists
    Rigged,        // _rigged.glb exists
    Animated,      // all clip glbs exist
    Merged,        // {id}.glb exists
    LodsComplete,  // all lod glbs exist
}
```

### MeshGenState (per entity)

```rust
struct MeshGenState {
    stage: MeshSubStage,
    status: String,
    progress: u8,
    receiver: Option<Receiver<MeshGenEvent>>,
}

enum MeshSubStage {
    Idle,
    TextTo3dPreview,
    TextTo3dRefine,
    Rigging,
    Animating { completed: Vec<String>, total: usize },
    Merging,
    GeneratingLods,
}

enum MeshGenEvent {
    Progress(u8, String),   // percent + status label
    Done,
    Failed(String),
}
```

---

## UI Layout

### Left panel

Entity list with stage badge. On startup, scan disk to hydrate `entity_stage`.

### Right panel — Generation Panel

**Section A: Sprite** (unchanged, collapses to summary once approved)

**Section B: 3D Mesh** — unlocked when Stage 3 reached

- Mesh prompt text field (pre-filled `"{display_name} — {ai_prompt_base}"`, editable)
- Backend: Mock / Live Meshy (greyed if `MESHY_API_KEY` not set)
- `[Generate 3D Mesh]` button
- Progress bar + status label while running
- When done: "✅ Mesh: assets/models/{id}/{id}_mesh.glb (10k)"

**Section C: Rig + Animate** — unlocked when Stage 4 reached

- Animation list: checkboxes for each clip (Idle ✓, Walk ✓, Attack ✓, Dead ✓, Run ✓, BeHit ✓)
- `[Rig & Animate]` button — runs rigging then animation calls in parallel
- Per-clip progress indicators while generating
- Merge runs automatically after all clips complete
- When done: "✅ Merged: {id}.glb (10k, 6 animations)"

**Section D: LODs** — unlocked when Stage 7 reached, optional

- Checkboxes: LOD1 (5k) ✓, LOD2 (2.5k) ✓, LOD3 (1k) ✓
- `[Generate LODs]` button — runs gltf-transform locally
- When done: "✅ LODs: lod1/lod2/lod3"

---

## Implementation Sequence

1. **Delete `tools/mesh_gen`** — remove from workspace Cargo.toml
2. **Bestiary schema** — add `animation_ids` + `mesh_prompt` optional fields (with serde defaults)
3. **Meshy HTTP client** — new `src/meshy.rs` in character_studio: text-to-3d, rigging, animation endpoints + SSE poll loop
4. **Disk scan on startup** — hydrate `entity_stage` from file existence
5. **PipelineStage + MeshGenState** — add to StudioApp
6. **Entity list badges** — update label logic
7. **Section B UI** — mesh generation panel
8. **Section C UI** — rig + animate panel with per-clip progress
9. **Section D UI** — LOD generation panel (spawns `npx gltf-transform` subprocess)
10. **Wire background threads** — all Meshy calls in `std::thread::spawn` + tokio runtime, mpsc channel back to UI
11. **Compile + test with mock key** (`msy_dummy_api_key_for_test_mode_12345678`)
