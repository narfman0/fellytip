# System: Civilization

The civilization system generates settlements, assigns territory, and connects settlements with roads. All generation is pure and deterministic from a seed; it runs after `generate_map` returns and before the server accepts player connections.

## Settlements

A settlement has a kind, a world-space position, a name, and a stable UUID.

### Settlement kinds

| Kind | Description |
|---|---|
| Capital | Major surface settlement; roughly 1-in-8 settlements are capitals |
| Town | Ordinary surface settlement |
| UndergroundCity | Located in a large Underdark cavern |

### Surface placement — Poisson-disk grid approximation

The 512×512 map is divided into 32×32 tile cells. Within each cell, the most habitable walkable tile is identified as a candidate. A candidate is accepted as a settlement only if no existing settlement lies within 30 tiles. This produces spacing similar to Poisson-disk sampling without the rejection-loop cost.

Habitability scores by biome:

| Biome | Score |
|---|---|
| Grassland, Plains | 1.0 |
| TemperateForest, Forest | 0.8 |
| Savanna, TropicalForest, TropicalRainforest | 0.7 |
| Taiga | 0.5 |
| Desert | 0.3 |
| Tundra | 0.2 |
| PolarDesert, Arctic | 0.1 |
| Water, River, Mountain, underground | 0.0 |

Cells with maximum habitability below 0.3 produce no settlement.

### Underground placement — connected-component analysis

All walkable `LuminousGrotto` tiles are labeled by BFS flood-fill into connected components. Each component with area ≥ 500 tiles receives one `UndergroundCity` placed at the component's centroid.

Because the Underdark is generated with 30% initial fill and loose rules, it tends to produce one very large connected region, so a typical 512×512 world has 1–3 underground cities.

## Territory assignment

Territory is assigned by BFS flood-fill from each settlement's tile position. Only walkable surface tiles are claimed. The flood spreads to 4-directional and diagonal neighbours. The result is a Voronoi-like partition of the walkable surface, with each tile assigned to its nearest reachable settlement.

Territory is stored as a flat array parallel to `WorldMap.columns`. It is used by faction AI to determine which settlements border each other and drive expansion goals.

## Road network

Surface settlements (not underground cities) are connected by a minimum spanning tree using Euclidean distances between settlement positions. Kruskal's algorithm builds the MST; each MST edge is then rasterised into map tiles using Bresenham's line algorithm. Rasterised tiles are flagged in `WorldMap.road_tiles`.

Roads are not yet enforced as preferred paths by AI or movement; the flags are available for rendering and future pathfinding cost weighting.

## Resources

The `Settlements` struct is a Bevy resource inserted by `MapGenPlugin` after generation. Other server systems access it with `Res<Settlements>`.

## Sequence

```
Startup (after seed_factions):
  generate_map(seed)
    → WorldMap resource
  generate_settlements(&map, seed)
    → Vec<Settlement>
  generate_roads(&mut map, &settlements)
    → populates map.road_tiles
  assign_territories(&map, &settlements)
    → TerritoryMap (used internally; not yet a persistent resource)
  insert WorldMap, Settlements as resources
  run WorldSimSchedule × 200  (history warp)
```
