# System: Pathfinding (navigation grid + flow fields)

Navigation lives in `crates/server/src/plugins/nav.rs`. It provides a single static grid for AI pathfinding and two algorithms layered on top — A* for individual movement and BFS/Dijkstra flow fields for war parties that share a destination.

## Storage: `Grid<T>`

Both the overworld `NavGrid` and the per-zone `ZoneNavGrids` share a generic row-major 2D container:

```rust
// crates/shared/src/world/grid.rs
pub struct Grid<T> {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<T>,
}
```

`Grid<T>` provides `new(w, h)` (default-fill), `from_cells(w, h, cells)`, `get`/`get_mut`, `in_bounds`, and a `neighbors_4` iterator. The nav types below are both thin wrappers around `Grid<NavCell>`.

## NavGrid (overworld)

`NavGrid` wraps `Grid<NavCell>` with dimensions `256 × 256`. It is built once from `WorldMap` on startup (after `generate_map` inserts it) — a 4:1 downsample of the 1024×1024 world tile map, one nav cell per 4×4 tile block.

Each cell is a `NavCell` classifying passability:

| `NavCell` | Movement cost | Source tiles |
|---|---|---|
| `Passable` | `1.0` | Default walkable terrain |
| `Slow` | `2.0` | `Forest`, `TropicalForest`, `TemperateForest`, `Taiga` |
| `Blocked` | `f32::MAX` | `Water`, `Mountain`, `River`, any non-walkable layer (impassable buildings, etc.) |

Construction samples the first tile of each 4×4 block — cheap and sufficient for AI pathfinding. `build_nav_grid` runs once from `MapGenPlugin`'s startup chain and logs cell counts.

Coordinate helpers:

- `world_to_nav(wx, wy) -> (nx, ny)` — world coords to grid cell, clamped.
- `nav_to_world(nx, ny) -> (wx, wy)` — grid cell to world-space centre.
- `NavGrid::nav_cell_at(wx, wy)` / `passability_at` — direct lookups by world position.

## A*

`NavGrid::astar(start, goal)` runs a textbook A* with Manhattan heuristic and 4-connected neighbours. The open set is a min-heap keyed on `f = g + h` (via `Reverse<(u32, usize)>`, where the `u32` is the bit-cast of the f-score).

Returns a compressed list of `(u16, u16)` waypoints — `reconstruct_path` keeps only cells where the direction changes, so straight runs collapse to two endpoints. This keeps the per-NPC follow-waypoint memory tiny.

Individual NPCs (e.g. guards) run A* at a cadence determined by their zone (see below).

## Flow fields

`FlowFieldData` is a pre-computed Dijkstra/BFS flow field from a target settlement outward. Each cell stores an `(i8, i8)` direction vector pointing toward the target. Blocked cells are skipped; every reachable cell gets a valid direction in one pass.

`FlowField` is a `Resource` that caches computed fields keyed by the target settlement's nav-grid cell `(u32, u32)`. Multiple war parties targeting the same settlement share one field — `get_or_compute(nav, wx, wy)` builds it lazily the first time a party needs it.

Flow fields are the right fit for war parties because the target is shared and the cost of a single BFS (O(N) cells) amortises over every follower.

## Zone-gated LOD

War-party and NPC movement pay for pathfinding proportional to their `ChunkTemperature` zone (Hot = near a player, Warm = mid-range, Frozen = far):

| Zone | Speed multiplier | A* / flow-field policy |
|---|---|---|
| `Hot` | 1.0 | A* replan every 2 ticks; flow-field sampling every tick at full march speed |
| `Warm` | 0.25 | A* replan every 8 ticks; flow-field sampling at 0.25× speed |
| `Frozen` | 0.05 | Skip A* and flow fields entirely; linear march toward the target at 0.05× speed |

Frozen-zone NPCs always reach their goal — linear march is deliberately macro-correct. It trades locally-realistic routing for near-zero CPU cost when no player can see the party. See `crates/server/src/plugins/ai.rs` for the movement systems that consume `NavGrid` and `FlowField`.

## `ZoneNavGrids` — per-zone grids for interiors

`ZoneNavGrids(pub HashMap<ZoneId, Grid<NavCell>>)` is a Bevy resource populated at startup by `build_zone_nav_grids` (in `plugins/nav.rs`) from the `ZoneRegistry`. For every zone with a non-empty tile array it builds a `Grid<NavCell>` sized to `zone.width × zone.height`.

`InteriorTile` → `NavCell` mapping (see `interior_tile_to_nav_cell`):

| `InteriorTile` | `NavCell` |
|---|---|
| `Floor`, `Stair`, `Balcony` | `Passable` |
| `Water`, `Roof`, `Window` | `Slow` |
| `Wall`, `Void`, `Pit` | `Blocked` |

The overworld zone (`OVERWORLD_ZONE = ZoneId(0)`) has an empty tile array in the registry and is deliberately skipped — its pathfinding still uses the flat 256×256 `NavGrid` above. `ZoneNavGrids` is currently a data container; zone-aware A* / flow-field pathfinding is a follow-up. See `docs/systems/zones.md` for the registry side and the overall status table.

## Interaction with adaptive throttling

The `AdaptiveScheduler` throttle level (see `docs/systems/perf.md`) further gates how often the movement systems tick — under pressure, replan cadence is stretched and Warm/Frozen updates are skipped more aggressively. Passability data itself is static, so nothing needs to be recomputed when the throttle escalates.
