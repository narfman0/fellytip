# Plan: Configurable & Persisted Map Dimensions

## Context

Map width and height are currently compile-time constants (`MAP_WIDTH = 128`, `MAP_HEIGHT = 128`)
in `crates/shared/src/world/map.rs:14-15`. The `WorldMap` struct does not store dimensions; every
indexing call hardcodes the constants. This plan makes dimensions runtime-configurable via server
CLI args and persists them as part of the saved world.

**Key facts gathered before writing this plan:**

- `WorldMap` (`map.rs:201-226`) stores only `columns: Vec<TileColumn>`, `seed: u64`, and
  `road_tiles: Vec<bool>`. No width/height fields.
- `generate_map(seed: u64) -> WorldMap` — width/height not in signature.
- `generate_settlements` and `generate_roads` in `civilization.rs` import and use the constants
  directly.
- Map is bincode-serialized to `world_{seed}.bin`; the server falls back to regeneration on
  deserialization failure.
- The client calls `generate_map(WORLD_SEED)` locally in `tile_renderer.rs` — terrain is never
  replicated.
- `WORLD_SEED: u64 = 42` is a compile-time constant in `crates/shared/src/lib.rs:19`.

---

## Step 1 — Add `width` / `height` fields to `WorldMap`

**File:** `crates/shared/src/world/map.rs`

- Add `pub width: usize` and `pub height: usize` to the struct.
- Update `column()` → `&self.columns[ix + iy * self.width]`
- Update `column_at()` bounds check → `ix < self.width && iy < self.height`
- Keep `MAP_WIDTH` / `MAP_HEIGHT` constants as named defaults — do not remove them.

---

## Step 2 — Update `generate_map` signature

**File:** `crates/shared/src/world/map.rs`

Change signature to:

```rust
pub fn generate_map(seed: u64, width: usize, height: usize) -> WorldMap
```

Replace every internal `MAP_WIDTH` / `MAP_HEIGHT` reference with the `width` / `height`
parameters:

- fBm frequency calculation
- Height and moisture field generation loops
- Biome classification loops
- Shallow cave and Underdark cellular automata passes
- `road_tiles` vec allocation (`vec![false; width * height]`)

Store `width` and `height` in the returned `WorldMap`.

---

## Step 3 — Update civilization functions

**File:** `crates/shared/src/world/civilization.rs`

- `generate_settlements(&map, seed)` — replace `MAP_WIDTH` / `MAP_HEIGHT` with
  `map.width` / `map.height`.
- `generate_roads(&mut map, &settlements)` — same.
- Remove the `MAP_WIDTH` / `MAP_HEIGHT` imports from this file.

---

## Step 4 — Update persistence (cache filename encodes dimensions)

**File:** `crates/server/src/plugins/map_gen.rs`

Bincode is positional — adding fields to `WorldMap` silently breaks old `.bin` files. The server
already falls back to regeneration on load failure, so this is safe. Make the breakage explicit
and self-documenting by encoding dimensions in the filename:

- Change cache filename: `world_{seed}.bin` → `world_{seed}_{width}x{height}.bin`
- Update `get_map_file_path`, `set_map_file_path`, and `save_map_file` for the new scheme.
- Old files are ignored; the map regenerates deterministically from seed + dimensions.

---

## Step 5 — Server CLI args and `MapGenConfig` resource

**File:** `crates/server/src/main.rs`

Add CLI arguments:

- `--seed <N>` (currently `WORLD_SEED` is a compile-time constant — make it runtime)
- `--map-width <N>` (default: `MAP_WIDTH`)
- `--map-height <N>` (default: `MAP_HEIGHT`)

Insert a `MapGenConfig` Bevy resource before `MapGenPlugin` is added:

```rust
#[derive(Resource, Reflect)]
pub struct MapGenConfig {
    pub seed: u64,
    pub width: usize,
    pub height: usize,
}
```

**File:** `crates/server/src/plugins/map_gen.rs`

- `MapGenPlugin` reads `Res<MapGenConfig>` instead of the constants.
- Pass `config.seed`, `config.width`, `config.height` to `generate_map`.

---

## Step 6 — Replicate map metadata to client

The client regenerates the map locally from the seed; it needs `width` and `height` to produce
the same result as the server.

Add a small replicated resource in `crates/shared/src/components.rs` (or a new
`world_meta.rs`):

```rust
#[derive(Resource, Replicate, Serialize, Deserialize, Reflect, Clone)]
pub struct WorldMeta {
    pub seed: u64,
    pub width: usize,
    pub height: usize,
}
```

- Server inserts `WorldMeta` after map gen completes.
- Register in `FellytipProtocolPlugin`.
- Client's `tile_renderer` waits for `Res<WorldMeta>` before calling
  `generate_map(meta.seed, meta.width, meta.height)`.

**Deferral option:** If lightyear protocol complexity is not wanted yet, clients can fall back to
`MAP_WIDTH` / `MAP_HEIGHT` defaults. This is acceptable as long as only default-sized worlds are
used in practice. Flag as tech debt if deferred.

---

## Step 7 — Update `world_gen` tool

**File:** `tools/world_gen/src/main.rs`

- Add `--width <N>` and `--height <N>` CLI args alongside the existing `--seed` arg.
- Pass to `generate_map(seed, width, height)`.
- Update the statistics and downsampling logic that currently uses `MAP_WIDTH` / `MAP_HEIGHT`.

---

## Step 8 — Tests and verification

- Update all `generate_map(seed)` callsites in tests to `generate_map(seed, MAP_WIDTH, MAP_HEIGHT)`.
- Add a unit test in `map.rs`:
  ```rust
  let map = generate_map(42, 64, 32);
  assert_eq!(map.width, 64);
  assert_eq!(map.height, 32);
  assert_eq!(map.columns.len(), 64 * 32);
  assert_eq!(map.road_tiles.len(), 64 * 32);
  ```
- Run full verification sequence:
  ```bash
  cargo test --workspace
  cargo clippy --workspace -- -D warnings
  cargo build --workspace
  cargo run -p world_gen -- --seed 42 --width 64 --height 64
  ```

---

## File touch summary

| File | Change |
|---|---|
| `crates/shared/src/world/map.rs` | Add `width`/`height` to `WorldMap`; update `column()`/`column_at()`; change `generate_map` signature |
| `crates/shared/src/world/civilization.rs` | Replace `MAP_WIDTH`/`MAP_HEIGHT` with `map.width`/`map.height`; remove imports |
| `crates/server/src/main.rs` | Add `--seed`, `--map-width`, `--map-height` args; insert `MapGenConfig` resource |
| `crates/server/src/plugins/map_gen.rs` | Read `MapGenConfig`; update cache filename scheme |
| `crates/shared/src/components.rs` | Add `WorldMeta` replicated resource |
| `crates/shared/src/lib.rs` | Keep `WORLD_SEED`/`MAP_WIDTH`/`MAP_HEIGHT` as defaults only |
| `crates/client/src/plugins/tile_renderer.rs` | Wait for `WorldMeta`; pass dims to `generate_map` |
| `tools/world_gen/src/main.rs` | Add `--width`/`--height` args; update internal usages |
| Test callsites (shared + combat_sim) | Update `generate_map` calls to new signature |

---

## Execution order

Steps 1–3 are pure `crates/shared` changes and can be verified with `cargo test -p fellytip-shared`
before touching server or client code. Steps 4–5 are server-only. Step 6 crosses the network
boundary and should be done last, after the rest compiles cleanly.
