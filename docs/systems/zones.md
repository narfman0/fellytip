# Zone Graph System

## Summary

The zone graph is the substrate for all interior traversal (buildings, dungeons) and the Underdark simulation. The overworld is zone 0; every interior space is a named child zone. Zones are nodes; portals are directed edges. The client prefetches 1-hop neighbors so transitions are effectively seamless.

Single-story village buildings use a cheaper **roof-cutaway** shortcut: the interior is still on zone 0, and the client just fades the roof mesh when the player walks under the building's AABB. A real child zone is only created when the space needs its own nav grid — multi-floor buildings, dungeons, and everything underground.

---

## Data Model (Rust sketch)

> Lives in `crates/shared/src/world/zone.rs`. No ECS here — pure types.

```rust
pub type ZoneId = Uuid;

/// Structural kind — drives generation, rendering, and simulation rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneKind {
    /// Zone 0: the existing 1024×1024 world map.
    Overworld,
    /// Any floor of an above-ground settlement building.
    BuildingFloor { settlement_id: Uuid, building_index: u16, floor: u8 },
    /// Sub-surface lit dungeon (caves, crypts, mine shafts).
    Dungeon { depth: u8 },
    /// True underdark — permanently dark, hostile faction home territory.
    Underdark { depth: u8 },
}

/// A traversable region — the graph node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Zone {
    pub id: ZoneId,
    pub kind: ZoneKind,
    pub width: u16,
    pub height: u16,
    /// Flat tile grid, row-major: index = y * width + x.
    pub tiles: Vec<InteriorTile>,
    /// Named arrival/departure points referenced by portals.
    pub anchors: Vec<ZoneAnchor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InteriorTile {
    Floor,
    Wall,
    Void,   // outside room boundary — impassable
    Stair,  // visual marker only; traversal is on the Portal
    Water,
    Pit,
}

/// Named point inside a zone — portals reference these by name.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZoneAnchor {
    pub name: SmolStr, // e.g. "entrance", "upper_stair_top", "cellar_hatch"
    pub pos: Vec2,     // local tile coords (0,0 = top-left)
}

/// Directed edge connecting two zones.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Portal {
    pub id: Uuid,
    pub kind: PortalKind,
    pub from_zone: ZoneId,
    pub from_anchor: SmolStr, // trigger point in from_zone
    pub trigger_radius: f32,
    pub to_zone: ZoneId,
    pub to_anchor: SmolStr, // arrival point in to_zone
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortalKind {
    Door,
    Staircase,
    Ladder,
    Trapdoor,
    CaveEntrance,
    UnderDarkRift, // ancient/magical passage to the deep
}

/// The full zone graph — one `Resource` on the server, replicated as needed.
#[derive(Resource, Default, Debug, Serialize, Deserialize)]
pub struct ZoneGraph {
    pub zones: HashMap<ZoneId, Zone>,
    pub portals: Vec<Portal>,
}

impl ZoneGraph {
    pub fn exits_from(&self, zone: ZoneId) -> impl Iterator<Item = &Portal> {
        self.portals.iter().filter(move |p| p.from_zone == zone)
    }

    pub fn neighbors(&self, zone: ZoneId) -> impl Iterator<Item = ZoneId> + '_ {
        self.exits_from(zone).map(|p| p.to_zone)
    }

    /// Dijkstra over hop-count — used by raid-party routing.
    pub fn shortest_path(&self, from: ZoneId, to: ZoneId) -> Option<Vec<ZoneId>>;

    /// Hop count between zones — used for the "looming threat" threshold.
    pub fn hop_distance(&self, from: ZoneId, to: ZoneId) -> Option<usize>;
}
```

---

## Zone Graph Topology

```
Overworld (zone 0, id = derive(WORLD_SEED))
├── [Door] → TavernGroundFloor
│              └── [Staircase] → TavernUpperFloor
├── [Door] → BlacksmithGroundFloor
├── [CaveEntrance] → Dungeon:depth=1
│                     └── [CaveEntrance] → Dungeon:depth=2
│                                           └── [UnderDarkRift] → Underdark:depth=1
│                                                                   └── ... depth N
└── (roof-cutaway buildings: no child zone, interior stays on zone 0)
```

Portals are **bidirectional by default** — one `Portal` record per direction. The server generates the reverse portal automatically during zone construction.

---

## Client Prefetch (Seamless Transitions)

When a player's current zone changes:

1. Server sends the new zone's `Zone` data + entity snapshot (existing replication).
2. Client immediately requests all 1-hop neighbor zones (by `ZoneId` list from the graph).
3. Neighbor zones are small (≤ 64×64 tiles), so they arrive in the same frame or next.
4. By the time the player reaches a portal trigger radius, the destination zone is already in memory → transition is instant (no loading screen).

**Size budget:** BuildingFloor zones target 16×16–64×64 tiles. Dungeon floors 64×128. Underdark caverns 128×256. These are tiny compared to the 1024×1024 overworld, so prefetching is cheap.

---

## Roof Cutaway (Single-Story Shortcut)

Buildings that never need stairs or a basement use a visual shortcut instead of a portal:

- Interior stays on zone 0 — entities inside are overworld entities.
- A `BuildingRoof` entity (PBR mesh) has a material shader that reads from a `PlayerProximity` buffer.
- When any local player's position is inside the building's AABB (world-space footprint + small margin), the roof's opacity lerps to 0.
- No zone transition occurs; nav grid, interest management, and replication are unchanged.
- Upgrade path: if a building later needs a basement, create a child zone and add a trapdoor portal — no other systems change.

---

## Nav Grid Per Zone

Each `Zone` gets a `ZoneNavGrid` computed from `InteriorTile`:

| Tile | NavCell |
|------|---------|
| Floor, Stair | Passable |
| Water | Slow |
| Wall, Void, Pit | Blocked |

Algorithm is identical to `nav.rs` (A* for individuals, Dijkstra flow fields for groups). Zone nav grids are lazily computed on first use and cached in `ZoneGraph`.

War-party routing across zones: parties path to the exit portal anchor within their current zone, then hop to the next zone via the portal. `WorldSimSchedule` advances one zone-hop per 1 Hz tick.

---

## Simulation: Underdark Raid Parties

```
Underdark:depth=N  →  ... → Dungeon:depth=1 → Overworld
```

Each raid party carries:
- `current_zone: ZoneId`
- `within_zone_pos: Vec2`
- `route: Vec<ZoneId>` — precomputed by `ZoneGraph::shortest_path` to target settlement

`advance_raid_parties` (WorldSimSchedule, 1 Hz):
1. Move party toward exit portal anchor using zone's flow field.
2. When within `trigger_radius`, hop: set `current_zone` to next zone in route.
3. Emit `StoryEvent::UnderDarkThreat { hops_to_surface }` whenever `hop_distance(current_zone, overworld_id) ≤ 3`.
4. When party reaches zone 0, convert to a surface `WarParty` using existing `trigger_war_party` logic.

Players intercept by being in a zone the route passes through — they'll see the raid party entities via interest management.

---

## Interest Management

- Server maintains one **interest group per zone**.
- Player entity subscribes to: their current zone + all 1-hop neighbors (pre-fetch group, entities only — not tiles).
- Portal entities are replicated to both `from_zone` and `to_zone` groups.
- Raid party entities in deep Underdark zones are **not** replicated to surface players (too far, no threat yet). The `StoryEvent` is the signal.

---

## Rendering

**Above-ground interiors**
- Floor: one `StandardMaterial` quad per room (instanced by tile count, one draw call per material type).
- Walls: oriented billboard quads facing the camera, per-wall texture atlas.
- Props: existing sprite billboards (unchanged pipeline).
- Lighting: ambient matches exterior time-of-day; interior point lights from candles, fireplaces.

**Underground / Underdark**
- Same mesh pipeline; ambient = near-black.
- Point lights from torches, bioluminescent fungi (`PointLight` entities, low radius).
- Underdark: no ambient at all — total darkness outside light radii.

**Roof cutaway** (zone 0 buildings)
- `BuildingRoof` mesh has custom wgsl that samples a `proximity_buffer` (vec of player world-positions).
- Opacity = `smoothstep(inner_r, outer_r, min_dist_to_any_player)`.
- No depth-sort issues because the roof is opaque when far and transparent when near.

---

## Implementation Order

1. `crates/shared/src/world/zone.rs` — data types + `ZoneGraph` (no ECS).
2. `generate_zones()` pure fn — produces child zones for each `Building` that needs one, plus cave entrances.
3. `ZoneNavPlugin` — builds `ZoneNavGrid` per zone on startup.
4. `PortalPlugin` (server) — spawns trigger volumes, handles `PlayerZoneTransition` events.
5. Client zone prefetch — on zone change, request neighbor zone data via BRP.
6. Roof cutaway shader + `BuildingRoof` component.
7. `advance_raid_parties` in `ai.rs` — replace overworld-only march with zone-hopping.
8. `StoryEvent::UnderDarkThreat` + hop-distance gate.
