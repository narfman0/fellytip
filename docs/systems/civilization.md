# System: Civilization

The civilization system generates settlements, assigns territory, and connects settlements with roads. All generation is pure and deterministic from a seed; it runs after `generate_map` returns and before the server accepts player connections.

Exact placement parameters (grid cell size, minimum spacing, habitability thresholds, component size cutoffs) live in `crates/shared/src/world/civilization.rs` — that file is the authority.

## Settlements

A settlement has a kind, a world-space position, a name, and a stable UUID. The `Settlement` and `SettlementKind` types are defined in `civilization.rs`.

### Settlement kinds

| Kind | Description |
|---|---|
| `Capital` | Major surface settlement |
| `Town` | Ordinary surface settlement |
| `UndergroundCity` | Located in a large Underdark cavern |

### Surface placement — Poisson-disk grid approximation

The map is divided into fixed-size cells. Within each cell, the most habitable walkable tile is identified as a candidate. A candidate is accepted only if no existing settlement lies within the minimum spacing distance. This produces spacing similar to Poisson-disk sampling without the rejection-loop cost.

Habitability scores per biome are defined in the `habitability()` function in `civilization.rs`. Cells whose best tile falls below the minimum habitability threshold produce no settlement.

### Underground placement — connected-component analysis

All walkable `LuminousGrotto` tiles are labeled by BFS flood-fill into connected components. Each component large enough (area threshold in `civilization.rs`) receives one `UndergroundCity` at the component's centroid.

## Territory assignment

Territory is assigned by BFS flood-fill from each settlement's tile position. Only walkable surface tiles are claimed; the flood spreads to 4-directional and diagonal neighbours. The result is a Voronoi-like partition of the walkable surface with each tile assigned to its nearest reachable settlement.

Territory is stored as a flat array parallel to `WorldMap.columns`. It is used by faction AI to determine which settlements border each other and drive expansion goals.

## Road network

Surface settlements are connected by a minimum spanning tree (Kruskal's algorithm, Euclidean distances). Each MST edge is rasterised into map tiles using Bresenham's line algorithm. Rasterised tiles are flagged in `WorldMap.road_tiles`.

Roads are not yet enforced as preferred paths by AI or movement; the flags are available for rendering and future pathfinding cost weighting.

## Resources

`Settlements` is a Bevy resource inserted by `MapGenPlugin` after generation. Other server systems access it with `Res<Settlements>`.

## Startup sequence

```
Startup (after seed_factions):
  generate_map(seed)           → WorldMap resource
  generate_settlements(&map, seed) → Vec<Settlement>
  generate_roads(&mut map, &settlements) → populates map.road_tiles
  assign_territories(&map, &settlements) → TerritoryMap
  insert WorldMap, Settlements as Bevy resources
  run WorldSimSchedule × HISTORY_WARP_TICKS  (see map_gen.rs)
```
