# System: World Map

The world map is a 512×512 grid of tile columns. It is generated once from a seed on server startup, never replicated to clients (it is too large), and used server-side for movement height queries and AI pathfinding.

## Tile structure

Each grid cell `(ix, iy)` holds a `TileColumn` — a sorted list of `TileLayer` values ordered by ascending `z_base`. Multiple layers can occupy the same column at different heights, enabling caves beneath terrain, bridges above ground, and the Underdark beneath everything.

A `TileLayer` has:
- `z_base` / `z_top` — vertical extent in world units
- `kind` — the `TileKind` variant
- `walkable` — whether entities can stand on this layer
- `corner_offsets` — four per-corner height adjustments from `z_top`, used for smooth bilinear slopes between adjacent tiles

## Tile kinds

### Surface biomes
| Kind | Walkable | Notes |
|---|---|---|
| Grassland | yes | Temperate, low-medium rainfall |
| Plains | yes | Legacy; used as fallback |
| TemperateForest | yes | Temperate, high rainfall |
| Forest | yes | Legacy; used as fallback |
| Savanna | yes | Warm, moderate rainfall |
| TropicalForest | yes | Hot, high rainfall |
| TropicalRainforest | yes | Hot, very high rainfall |
| Taiga | yes | Cool, moderate-high rainfall |
| Desert | yes | Hot, arid |
| Tundra | yes | Cold, sparse |
| PolarDesert | yes | Very cold, dry |
| Arctic | yes | Very cold, wet |
| Mountain | no | High elevation; impassable |
| Water | no | Ocean/lake; impassable |
| River | no | High drainage area; impassable (crossing requires bridge) |
| Stone | yes | Bare rock surface |

### Underground
| Kind | Walkable | Depth |
|---|---|---|
| Cavern | yes | Shallow (~−15 m) |
| DeepRock | yes | Mid-level (~−38 m) |
| LuminousGrotto | yes | Underdark (~−65 m) |
| Tunnel | yes | Shaft connector (surface to underground) |

### Meta
| Kind | Walkable | Notes |
|---|---|---|
| Void | no | Empty column; no tile present |

## Height system

`WorldPosition.z` is the entity's current elevation. The server movement system queries `smooth_surface_at(map, x, y, current_z)` after each horizontal move and lerps `z` toward the result at `Z_FOLLOW_RATE = 12.0` world units/second. Descent is capped at `FALL_SPEED = 40.0` units/second; ascent is capped at `STEP_HEIGHT = 0.6` units per tick to prevent teleporting through thin floors.

`smooth_surface_at` returns a bilinearly interpolated height using the current tile's pre-computed corner offsets. The four corners are the averages of the four adjacent tile centers that share each corner, giving a continuous height field with no visible seams.

`surface_layer(current_z, step_height)` selects the highest walkable layer whose `z_top ≤ current_z + step_height`. This means an entity standing at ground level cannot suddenly snap up to a bridge 3 m above, and an entity in the Underdark cannot snap up to the surface above.

## Generation pipeline

All generation is deterministic: same seed always produces the same map. The pipeline runs once on server startup in `MapGenPlugin`.

### 1. Surface terrain
Two independent fBm (fractional Brownian motion) passes:
- **Elevation**: 6 octaves, base frequency ≈ 4 cycles/512 tiles (continent scale). Produces smooth height values [0, 1].
- **Moisture**: 4 octaves, slightly finer frequency. Produces independent precipitation values [0, 1].

Both passes derive large coordinate offsets from the seed so different seeds sample distinct regions of the infinite noise field.

### 2. Biome classification
Each walkable tile (elevation 0.25–0.72) is classified by the Whittaker diagram:
- **Temperature** = latitude factor (0 at equator, 1 at poles) × 0.7 + altitude penalty × 0.3
- **Moisture** = value from the fBm moisture pass
- `classify_biome(temperature, moisture)` returns the appropriate `TileKind`

Tiles below elevation 0.25 become non-walkable Water. Tiles above 0.72 become non-walkable Mountain.

### 3. Rivers
Flow direction: for each tile, the steepest downhill cardinal/diagonal neighbour.
Drainage area: accumulated by processing tiles from highest to lowest, adding each tile's count to its downhill neighbour.
Tiles with drainage area ≥ 800 that are currently walkable become non-walkable River tiles.

### 4. Shallow caves (Z ≈ −15 m)
Cellular automata with 48% initial fill, 5 smoothing steps, solid threshold of 5 neighbours. Produces winding 3–8 tile-wide passages similar to a dungeon crawl. Open cells get a walkable `Cavern` layer at `z_top = −15`.

### 5. Underdark (Z ≈ −65 m)
Same algorithm with 30% initial fill, 3 steps, threshold of 6 neighbours. Produces vast, mostly-open caverns with scattered pillars — city-scale voids. Open cells get a walkable `LuminousGrotto` layer.

### 6. Shaft connectors
Roughly 1-in-40 columns that have both a walkable surface layer and at least one walkable underground layer get a `Tunnel` layer bridging the two. Tunnels enable vertical travel.

## Constants

| Constant | Value | Meaning |
|---|---|---|
| MAP_WIDTH / MAP_HEIGHT | 512 | Grid dimensions in tiles |
| STEP_HEIGHT | 0.6 | Max upward snap per movement tick (world units) |
| Z_FOLLOW_RATE | 12.0 | Z lerp speed toward terrain surface (units/s) |
| FALL_SPEED | 40.0 | Maximum fall speed (units/s) |
| SHALLOW_CAVE_Z | −15.0 | Shallow cave floor elevation |
| UNDERDARK_Z | −65.0 | Underdark floor elevation |
