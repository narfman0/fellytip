# Zone Graph System

## Summary

The zone graph is the substrate for all interior traversal (buildings, dungeons) and the underground simulation (the Sunken Realm in lore). The overworld is zone 0; every interior space is a named child zone. Zones are nodes; portals are directed edges. The client prefetches 1-hop neighbors so transitions are effectively seamless.

Single-story village buildings use a cheaper **roof-cutaway** shortcut: the interior is still on zone 0, and the client just fades the roof mesh when the player walks under the building's AABB. A real child zone is only created when the space needs its own nav grid — multi-floor buildings (`Tavern`, `Barracks`, `Tower`, `Keep`), dungeons, and everything underground.

---

## Implementation Status

| Component | Location | Status | Gaps / TODOs |
|---|---|---|---|
| Zone data types (`Zone`, `ZoneKind`, `ZoneParent`, `InteriorTile`, `ZoneAnchor`, `Portal`, `PortalKind`) | `crates/shared/src/world/zone.rs` | ✅ Done | — |
| `ZoneTemplate` + content-hash dedupe (`ZoneTemplateId = u64`) | `crates/shared/src/world/zone.rs` | ✅ Done | — |
| `ZoneRegistry` + `ZoneTopology` resources | `crates/shared/src/world/zone.rs` | ✅ Done | — |
| `ZoneMembership` component | `crates/shared/src/world/zone.rs` | ✅ Done | — |
| `ZoneTopology::hop_distance` (BFS) | `crates/shared/src/world/zone.rs` | ✅ Done | `shortest_path` helper currently lives in `ai.rs` (`shortest_zone_path`); should move to `ZoneTopology` impl |
| `generate_zones(&buildings, seed)` pure fn | `crates/shared/src/world/zone.rs` | ✅ Done | Only multi-floor `BuildingKind`s produce zones; single-story buildings stay on overworld. Underground chain is currently a hard-coded 3 depths for testing. |
| `PortalPlugin` — spawns `PortalTrigger` entities, handles `PlayerZoneTransition` events | `crates/server/src/plugins/portal.rs` | ✅ Done | Anchor world positions are `(0,0)` placeholder — need building world-coord propagation so intra-zone triggers are placed correctly. |
| `ZoneNavGrids` resource + `build_zone_nav_grids` startup system | `crates/server/src/plugins/nav.rs` | ✅ Done (data container) | Zone-aware pathfinder consumption is pending; `nav.rs` still uses the flat 256×256 overworld `NavGrid` for AI. |
| `UndergroundSimSchedule` (0.1 Hz) + `UndergroundPressure` resource | `crates/server/src/plugins/world_sim.rs`, `plugins/ai.rs` | ✅ Done | See `docs/systems/underground.md`. |
| `advance_zone_parties` — zone-hopping for war-party members | `crates/server/src/plugins/ai.rs` | ✅ Done | Trigger-radius check compares to world origin until anchor world positions are wired. |
| `spawn_underground_raid` — pressure→raid party conversion | `crates/server/src/plugins/ai.rs` | ✅ Done | — |
| `dm/underground_pressure`, `dm/force_underground_pressure` BRP methods | `crates/server/src/plugins/dm.rs`, registered in `crates/client/src/main.rs` | ✅ Done | — |
| `ZoneTileMessage` server→client protocol + `ZoneCache` resource | `crates/shared/src/protocol.rs`, `crates/client/src/plugins/zone_cache.rs` | ✅ Done | Message should carry an explicit `ZoneKind` field instead of the client-side tile-shape heuristic in `zone_renderer::classify_zone`. |
| `ZoneRendererPlugin` — interior mesh spawn/despawn | `crates/client/src/plugins/zone_renderer.rs` | ✅ Done (scaffold) | One unlit quad per tile; no instancing, atlas, or lighting pass yet. Roof cutaway shader for zone-0 single-story buildings is **not** implemented. |
| `update_zone_visibility` — client-side per-zone entity culling | `crates/client/src/plugins/entity_renderer.rs` | ✅ Done (scaffold) | Lightyear interest management per-zone is **not** yet wired; this is local visibility only. |
| `underground_e2e` ralph scenario | `tools/ralph/src/scenarios/underground_e2e.rs` | ✅ Done | Best-effort battle assertion; would sharpen once `dm/story_events_by_tag` is added. |

### Follow-ups (known, documented, non-blocking)

1. Portal anchor world positions use `(0, 0)` as placeholder — building world-coords not yet propagated into the zone graph. `advance_zone_parties` and `setup_portal_triggers` both work around this today.
2. `ZoneTopology::shortest_path` should move from `plugins/ai.rs::shortest_zone_path` onto the impl itself.
3. `ZoneTileMessage` should carry an explicit `ZoneKind` instead of the client heuristics in `zone_renderer::classify_zone`.
4. `dm/story_events_by_tag` would let `underground_e2e` assert precisely on `UndergroundThreat` emission instead of polling raid spawns indirectly.
5. Lightyear interest management per zone group — the server-side infrastructure (one interest group per zone, subscribe to current + 1-hop neighbours) is not yet implemented. Client-side `update_zone_visibility` hides mismatched entities; this will be replaced when the lightyear side is wired.

---

## Data Model (actual shapes as implemented)

> Lives in `crates/shared/src/world/zone.rs`. No ECS here — pure types except for `Resource` / `Component` derives for Bevy wiring.

```rust
pub const OVERWORLD_ZONE: ZoneId = ZoneId(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub struct ZoneId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneKind {
    Overworld,
    BuildingFloor { floor: u8 },
    Dungeon { depth: u8 },
    Underground { depth: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneParent {
    Overworld,
    Settlement(Uuid),
    Dungeon,
    Underground,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InteriorTile {
    Floor,
    Wall,
    Void,
    Stair,
    Water,
    Pit,
    Balcony,   // sightline down; entities visible from floor below
    Window,    // jump-out portal visual marker
    Roof,      // exterior-facing, ambush spawn point
}

pub type ZoneTemplateId = u64; // content-hash of the tile array

pub struct ZoneTemplate {
    pub id: ZoneTemplateId,
    pub width: u16,
    pub height: u16,
    pub tiles: Vec<InteriorTile>,
    pub anchors: Vec<ZoneAnchor>,
}

/// Runtime zone record — lightweight metadata + reference to a shared template.
pub struct Zone {
    pub id: ZoneId,
    pub kind: ZoneKind,
    pub parent: ZoneParent,
    pub width: u16,
    pub height: u16,
    pub template_id: ZoneTemplateId,
    pub anchors: Vec<ZoneAnchor>,
}

pub struct Portal {
    pub id: u32,
    pub kind: PortalKind,
    pub from_zone: ZoneId,
    pub from_anchor: SmolStr,
    pub trigger_radius: f32,
    pub traversal_cost: f32,
    pub faction_permeable: bool,
    pub one_way: bool,
    pub to_zone: ZoneId,
    pub to_anchor: SmolStr,
}

pub enum PortalKind { Door, Staircase, Ladder, Trapdoor, CaveEntrance, SealRift }

#[derive(Resource, Default, Clone, Debug)]
pub struct ZoneRegistry {
    pub zones: HashMap<ZoneId, Zone>,
    pub templates: HashMap<ZoneTemplateId, ZoneTemplate>,
    pub next_id: u32,
}

#[derive(Resource, Default, Clone, Debug)]
pub struct ZoneTopology {
    pub portals: Vec<Portal>,
    pub adjacency: HashMap<ZoneId, SmallVec<[u32; 4]>>,
}

impl ZoneTopology {
    pub fn exits_from(&self, zone: ZoneId) -> impl Iterator<Item = &Portal>;
    pub fn neighbors(&self, zone: ZoneId) -> impl Iterator<Item = ZoneId>;
    pub fn hop_distance(&self, from: ZoneId, to: ZoneId) -> Option<usize>;
    // shortest_path currently lives in ai.rs as `shortest_zone_path` — see TODO above.
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub struct ZoneMembership(pub ZoneId);
```

Templates are **content-hashed** (`ZoneTemplate::compute_id(tiles)`) and deduped in the registry, so identical layouts (e.g. every level of the 3-floor underground chain) share storage.

---

## Zone Graph Topology (what `generate_zones` currently builds)

```
Overworld (ZoneId(0))
├── [Staircase pair] Tavern floor 0 ↔ floor 1            (per Tavern building)
├── [Staircase pair] Barracks floor 0 ↔ floor 1           (per Barracks building)
├── [Staircase pairs] Tower floors 0↔1, 1↔2, 2↔3          (per Tower building; floor 3 = 6×6 roof battlements)
├── [Staircase pairs] Keep floors 0↔1, 1↔2                (per Keep building; floor 2 = 10×10 battlements)
└── [CaveEntrance pair] → Underground depth 1
                          ├── [SealRift pair] → Underground depth 2
                          └── ... → Underground depth 3
```

Portals are **bidirectional by default** — `generate_zones()` emits one `Portal` record per direction. `Portal.one_way = true` if you want a one-way portal (e.g. trapdoor); `apply_zone_transitions` honours it.

Building floor layouts (per `zone.rs`):

- **Tavern**: 8×8 floors of `Floor` with a `Stair` at (3,3). Upper floor adds a `Balcony` at (5,5).
- **Barracks**: 8×8 floors. Upper floor's west wall replaced with `Window` tiles.
- **Tower**: 6×6 interior floors 0–2 with a `Stair` at (2,2); floor 3 = 6×6 roof battlements (perimeter `Roof` tiles).
- **Keep**: 6×6 interior floors 0–1; floor 2 = 10×10 battlements. (Entrance anchor on floor 0.)

---

## Client Prefetch (Seamless Transitions)

When a player's `ZoneMembership` changes:

1. `PortalPlugin::send_zone_tiles` emits one `ZoneTileMessage` for the destination zone and one for each 1-hop neighbour.
2. Client stores them in `ZoneCache` (see `crates/client/src/plugins/zone_cache.rs`).
3. `ZoneRendererPlugin::spawn_zone_meshes` wakes on the player's new zone and spawns interior meshes from the cached tiles; `despawn_stale_zone_meshes` clears previous zones.
4. By the time the player reaches a portal trigger radius, the destination zone's tiles are already in `ZoneCache` → transition is instant.

**Size budget:** BuildingFloor zones target 6×6–8×8 (Tavern/Barracks/Tower) or up to 10×10 (Keep battlements). Underground caverns are 16×16 in the current scaffold. These are tiny compared to the 1024×1024 overworld, so prefetching is cheap.

---

## Roof Cutaway (Single-Story Shortcut) — designed, not yet implemented

Buildings that never need stairs or a basement should use a visual shortcut instead of a portal:

- Interior stays on zone 0 — entities inside are overworld entities.
- A `BuildingRoof` entity (PBR mesh) with a shader that reads a `PlayerProximity` buffer.
- When any local player's position is inside the building's AABB (world-space footprint + margin), the roof's opacity lerps to 0.
- No zone transition occurs; nav grid, interest management, and replication are unchanged.
- Upgrade path: if a building later needs a basement, convert to a multi-floor `BuildingKind` — `generate_zones()` will emit child zones and portals automatically.

As of this milestone, `generate_zones()` already honours this by skipping single-story `BuildingKind`s (`TentDetailed`, `Fountain`, stalls, etc.). The shader/fade system is still TODO — no `BuildingRoof` component exists yet.

---

## Nav Grid Per Zone

Each `Zone` with non-empty tiles gets a `Grid<NavCell>` built by `build_zone_nav_grids` at startup and stored in the `ZoneNavGrids` resource (`HashMap<ZoneId, Grid<NavCell>>`).

Mapping is in `plugins/nav.rs::interior_tile_to_nav_cell`:

| Tile | NavCell |
|------|---------|
| Floor, Stair, Balcony | Passable |
| Water, Roof, Window | Slow |
| Wall, Void, Pit | Blocked |

The overworld `NavGrid` still uses the existing 256×256 downsampled tile grid — zone-aware A* / flow-field consumption of `ZoneNavGrids` is a follow-up. The container is in place so the algorithms can land next without a data migration.

War-party routing across zones: `advance_zone_parties` (WorldSimSchedule, 1 Hz) advances members one zone per tick based on their precomputed `zone_route: Vec<ZoneId>`. Intra-zone movement stays on `FixedUpdate` via `march_war_parties` once the party reaches the overworld.

---

## Simulation: Underground Raid Parties

See `docs/systems/underground.md` for the full loop. Short version:

1. `UndergroundPressure.score` accumulates on `UndergroundSimSchedule` (0.1 Hz).
2. At score ≥ 0.4 → distant `StoryEvent::UndergroundThreat` with `hops_to_surface = 99` (latched).
3. At score ≥ 0.7 → imminent `StoryEvent::UndergroundThreat` with `hops_to_surface = 2` (latched).
4. At score ≥ 0.8 → `spawn_underground_raid` spawns `UNDERGROUND_RAID_PARTY_SIZE` (3) WarPartyMembers in the deepest underground zone, routed BFS → `OVERWORLD_ZONE`.
5. `advance_zone_parties` hops them one zone per 1 Hz tick; on reaching `OVERWORLD_ZONE` the existing `march_war_parties` surface logic takes over.

---

## Interest Management

**Design:**
- Server maintains one interest group per zone.
- Player entity subscribes to: current zone + all 1-hop neighbours (pre-fetch group).
- Raid party entities in deep underground zones are **not** replicated to surface players — the `StoryEvent` is the signal.

**Current state:** the server-side lightyear group wiring is not yet implemented (see Implementation Status table). The client-side `update_zone_visibility` system in `entity_renderer.rs` hides mismatched entities locally as a stop-gap; a comment in that system calls out the missing server plumbing.

---

## Rendering

`ZoneRendererPlugin` (`crates/client/src/plugins/zone_renderer.rs`) spawns one quad per non-empty interior tile on zone entry and despawns them on zone exit. Zone kind drives tint:

| ZoneKind | Floor | Wall | Roof | Emissive |
|---|---|---|---|---|
| Overworld | not rendered (terrain handles it) | — | — | — |
| BuildingFloor | warm brown | dark brown | default | none |
| Dungeon | grey stone | grey stone | default | none |
| Underground | near-black | near-black | near-black | blue-green bioluminescent tint |

Tile handling: `Floor`/`Stair`/`Water` → floor quad; `Wall`/`Window` → vertical wall quad; `Balcony` → translucent elevated floor; `Roof` → floor quad at WALL_HEIGHT; `Void`/`Pit` → no mesh (empty space).

---

## Implementation Order (what's done vs remaining)

1. ✅ `crates/shared/src/world/zone.rs` — data types + `ZoneRegistry`/`ZoneTopology` + `generate_zones()`.
2. ✅ `generate_zones()` — produces child zones for `Tavern`/`Barracks`/`Tower`/`Keep` + hard-coded 3-depth underground chain.
3. ✅ `ZoneNavGrids` — per-zone `Grid<NavCell>` built on startup.
4. ✅ `PortalPlugin` — spawns trigger entities, emits `PlayerZoneTransition`, applies transitions, broadcasts `ZoneTileMessage` with neighbours.
5. ✅ Client zone prefetch — `ZoneCache` + `ZoneTileMessage` + `ZoneRendererPlugin`.
6. ⏳ Roof-cutaway shader for single-story buildings — deferred (no `BuildingRoof` component yet).
7. ✅ `advance_zone_parties` in `ai.rs` — zone-hopping at 1 Hz; converts to surface march on reaching overworld.
8. ✅ `StoryEvent::UndergroundThreat` + hop-distance gate (≤ 3 emits; 0.4/0.7 pressure thresholds emit with synthetic hop counts).
9. ⏳ Move `shortest_zone_path` from `ai.rs` onto `ZoneTopology` impl.
10. ⏳ Wire Lightyear interest groups per zone.
11. ⏳ Propagate building world-coords into portal anchors so `trigger_radius` checks are real.
12. ⏳ Give `ZoneTileMessage` an explicit `ZoneKind` field.
