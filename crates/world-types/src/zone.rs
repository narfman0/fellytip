//! Zone Graph — spatial hierarchy of worldspace, interiors, and the Sunken Realm.
//!
//! A `Zone` is a self-contained tile grid (overworld region, building floor,
//! dungeon level, underground cave). Zones connect via `Portal`s. Entities carry
//! a `ZoneMembership` component pointing at the zone they currently occupy.
//!
//! The `ZoneRegistry` resource owns all zones and templates; `ZoneTopology`
//! owns the portal graph. Both are populated by `generate_zones()` at startup
//! from the civilization `Building` list.

use bevy::prelude::{Component, Reflect, Resource};
use glam::Vec2;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use smol_str::SmolStr;
use std::collections::HashMap;
use uuid::Uuid;

// ── IDs and enums ─────────────────────────────────────────────────────────────

/// Canonical ID of the overworld zone — always 0.
pub const OVERWORLD_ZONE: ZoneId = ZoneId(0);

/// Opaque zone identifier. `ZoneId(0)` is reserved for the overworld.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect,
)]
pub struct ZoneId(pub u32);

/// Identifies a distinct coordinate universe. Zones in the same WorldId share
/// a coordinate space; zones in different WorldIds do not.
///
/// WorldId(0) = The Surface (main world)
/// WorldId(1) = The Sunken Realm (underground)
/// WorldId(2) = The Mycelium (extra-cosmological fungi world, separate universe)
/// WorldId(3) = Devil's Casino Realm (placeholder)
/// WorldId(4) = Hivemind Fungus World (placeholder)
/// WorldId(5+) = Dynamically allocated player/procedural worlds
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub struct WorldId(pub u32);

pub const WORLD_SURFACE: WorldId = WorldId(0);
pub const WORLD_SUNKEN_REALM: WorldId = WorldId(1);
pub const WORLD_MYCELIUM: WorldId = WorldId(2);
/// Devil's Casino Realm — placeholder world, no terrain generation yet.
pub const WORLD_DEVILS_CASINO: WorldId = WorldId(3);
/// Hivemind Fungus World — placeholder world, no terrain generation yet.
pub const WORLD_HIVEMIND_FUNGUS: WorldId = WorldId(4);

/// First dynamically allocated world ID. IDs below this are reserved for
/// well-known worlds (Surface, Sunken Realm, Mycelium, Devil's Casino,
/// Hivemind Fungus).
pub const WORLD_DYNAMIC_START: u32 = 5;

/// Registry for world IDs — tracks the next available dynamic world ID so
/// player-owned or procedurally generated worlds can be allocated at runtime
/// without colliding with the reserved well-known IDs.
#[derive(Resource, Default, Clone, Debug)]
pub struct WorldRegistry {
    /// Next ID to hand out when `alloc_world_id()` is called. Starts at
    /// `WORLD_DYNAMIC_START` so reserved IDs (0-2) are never reused.
    pub next_dynamic_id: u32,
}

impl WorldRegistry {
    pub fn new() -> Self {
        Self {
            next_dynamic_id: WORLD_DYNAMIC_START,
        }
    }

    /// Allocate a new unique world ID. IDs are monotonically increasing and
    /// never reuse a previously allocated value.
    pub fn alloc_world_id(&mut self) -> WorldId {
        let id = WorldId(self.next_dynamic_id);
        self.next_dynamic_id += 1;
        id
    }

    /// Returns `true` if the given WorldId is one of the reserved well-known worlds.
    pub fn is_reserved(id: WorldId) -> bool {
        id.0 < WORLD_DYNAMIC_START
    }
}

// ── Placeholder zone stubs for reserved worlds ────────────────────────────────

/// Stub zone definition for a placeholder world that has no terrain generation yet.
#[derive(Clone, Debug)]
pub struct PlaceholderWorldZone {
    pub world_id: WorldId,
    pub name: &'static str,
}

pub const PLACEHOLDER_WORLDS: &[PlaceholderWorldZone] = &[
    PlaceholderWorldZone {
        world_id: WORLD_DEVILS_CASINO,
        name: "Devil's Casino Realm",
    },
    PlaceholderWorldZone {
        world_id: WORLD_HIVEMIND_FUNGUS,
        name: "Hivemind Fungus World",
    },
];

/// What category of zone this is (overworld, building floor, dungeon, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneKind {
    Overworld,
    BuildingFloor { floor: u8 },
    Dungeon { depth: u8 },
    Underground { depth: u8 },
}

/// Parent relationship — used for spatial ownership and cleanup semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneParent {
    Overworld,
    Settlement(Uuid),
    Dungeon,
    Underground,
}

/// Tile kinds inside an interior or subterranean zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InteriorTile {
    Floor,
    Wall,
    Void,
    Stair,
    Water,
    Pit,
    /// Sightline down; entities visible from floor below.
    Balcony,
    /// Jump-out portal visual marker.
    Window,
    /// Exterior-facing, ambush spawn point.
    Roof,
}

// ── Zone templates ────────────────────────────────────────────────────────────

/// Named point inside a zone — used as portal endpoint or spawn location.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZoneAnchor {
    pub name: SmolStr,
    pub pos: Vec2,
}

/// Content hash of a zone's tile array. Identical templates share one entry
/// in `ZoneRegistry::templates`.
pub type ZoneTemplateId = u64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZoneTemplate {
    pub id: ZoneTemplateId,
    pub width: u16,
    pub height: u16,
    pub tiles: Vec<InteriorTile>,
    pub anchors: Vec<ZoneAnchor>,
}

impl ZoneTemplate {
    /// Stable content-hash of the tile array. Anchors and dimensions are
    /// intentionally excluded so identical layouts with different labels
    /// collide (caller is expected to dedupe on tile shape).
    pub fn compute_id(tiles: &[InteriorTile]) -> ZoneTemplateId {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        tiles.hash(&mut h);
        h.finish()
    }
}

/// Runtime zone record — lightweight metadata + reference to a shared template.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Zone {
    pub id: ZoneId,
    pub kind: ZoneKind,
    pub parent: ZoneParent,
    pub world_id: WorldId,
    pub width: u16,
    pub height: u16,
    pub template_id: ZoneTemplateId,
    pub anchors: Vec<ZoneAnchor>,
}

// ── Portals ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortalKind {
    Door,
    Staircase,
    Ladder,
    Trapdoor,
    CaveEntrance,
    SealRift,
    /// Portal to the Devil's Casino Realm (placeholder).
    CasinoPortal,
    /// Portal to the Hivemind Fungus World (placeholder).
    FungusPortal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Custom portal shape vertices. `None` means use a default rectangle
    /// sized `trigger_radius × trigger_radius * 2` (width × height).
    #[serde(default)]
    pub shape: Option<Vec<Vec2>>,
}

// ── Resources (Bevy) ──────────────────────────────────────────────────────────

/// Central registry of all zones and templates.
#[derive(Resource, Default, Clone, Debug, Serialize, Deserialize)]
pub struct ZoneRegistry {
    pub zones: HashMap<ZoneId, Zone>,
    pub templates: HashMap<ZoneTemplateId, ZoneTemplate>,
    pub next_id: u32,
}

impl ZoneRegistry {
    pub fn alloc_id(&mut self) -> ZoneId {
        let id = ZoneId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn insert(&mut self, zone: Zone, template: ZoneTemplate) {
        self.templates.entry(template.id).or_insert(template);
        self.zones.insert(zone.id, zone);
    }

    pub fn get(&self, id: ZoneId) -> Option<&Zone> {
        self.zones.get(&id)
    }

    pub fn tiles(&self, zone: &Zone) -> Option<&[InteriorTile]> {
        self.templates
            .get(&zone.template_id)
            .map(|t| t.tiles.as_slice())
    }
}

/// Portal graph over zones.
#[derive(Resource, Default, Clone, Debug, Serialize, Deserialize)]
pub struct ZoneTopology {
    pub portals: Vec<Portal>,
    /// For each zone, the list of portal ids that exit from it.
    pub adjacency: HashMap<ZoneId, SmallVec<[u32; 4]>>,
}

impl ZoneTopology {
    pub fn add_portal(&mut self, portal: Portal) {
        let pid = portal.id;
        self.adjacency.entry(portal.from_zone).or_default().push(pid);
        self.portals.push(portal);
    }

    pub fn exits_from(&self, zone: ZoneId) -> impl Iterator<Item = &Portal> {
        self.adjacency
            .get(&zone)
            .into_iter()
            .flat_map(|ids| {
                ids.iter()
                    .filter_map(|id| self.portals.iter().find(|p| p.id == *id))
            })
    }

    pub fn neighbors(&self, zone: ZoneId) -> impl Iterator<Item = ZoneId> + '_ {
        self.exits_from(zone).map(|p| p.to_zone)
    }

    /// Returns `true` if `portal` crosses a world boundary (i.e. `from_zone` and
    /// `to_zone` belong to different `WorldId`s).
    pub fn is_world_crossing(&self, portal: &Portal, registry: &ZoneRegistry) -> bool {
        let from_world = registry.get(portal.from_zone).map(|z| z.world_id);
        let to_world = registry.get(portal.to_zone).map(|z| z.world_id);
        from_world != to_world
    }

    /// BFS shortest zone-hop path from `from` to `to`.
    /// Returns the list of zones to hop into (excluding `from`, including `to`),
    /// or `None` if unreachable. Returns `Some(vec![])` if `from == to`.
    pub fn shortest_path(&self, from: ZoneId, to: ZoneId) -> Option<Vec<ZoneId>> {
        use std::collections::{HashMap, VecDeque};
        if from == to {
            return Some(Vec::new());
        }
        let mut parent: HashMap<ZoneId, ZoneId> = HashMap::new();
        let mut queue: VecDeque<ZoneId> = VecDeque::new();
        queue.push_back(from);
        parent.insert(from, from);
        while let Some(cur) = queue.pop_front() {
            for next in self.neighbors(cur) {
                if parent.contains_key(&next) {
                    continue;
                }
                parent.insert(next, cur);
                if next == to {
                    // Reconstruct path.
                    let mut path = Vec::new();
                    let mut at = to;
                    while at != from {
                        path.push(at);
                        at = *parent.get(&at)?;
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(next);
            }
        }
        None
    }

    /// BFS hop count between zones. `Some(0)` if from == to. `None` if unreachable.
    pub fn hop_distance(&self, from: ZoneId, to: ZoneId) -> Option<usize> {
        if from == to {
            return Some(0);
        }
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((from, 0usize));
        while let Some((cur, dist)) = queue.pop_front() {
            if !visited.insert(cur) {
                continue;
            }
            for next in self.neighbors(cur) {
                if next == to {
                    return Some(dist + 1);
                }
                queue.push_back((next, dist + 1));
            }
        }
        None
    }
}

/// ECS component — zone an entity currently occupies.
#[derive(
    Component, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect,
)]
pub struct ZoneMembership(pub ZoneId);

// ── Zone generation ───────────────────────────────────────────────────────────

use crate::civilization::Building;

/// Generate zone graph from a list of buildings plus a seeded underground chain.
///
/// Each multi-story building produces N `BuildingFloor` zones with
/// `Staircase` portal pairs connecting adjacent floors. A small 3-level
/// underground chain (the Sunken Realm) is generated unconditionally for
/// testing; it attaches to the overworld via a `CaveEntrance` portal.
pub fn generate_zones(
    buildings: &[Building],
    _seed: u64,
) -> (ZoneRegistry, ZoneTopology, HashMap<Uuid, ZoneId>) {
    let mut registry = ZoneRegistry::default();
    let mut topology = ZoneTopology::default();
    let mut next_portal_id: u32 = 0;
    let mut building_to_floor0: HashMap<Uuid, ZoneId> = HashMap::new();

    // Zone 0 — overworld. Width/height reflect the world tile dimensions
    // (1024×1024) so clients can reason about the surface bounds. The tile
    // array is intentionally empty: the overworld's tiles live in `WorldMap`,
    // not in the zone template.
    let overworld_id = registry.alloc_id();
    debug_assert_eq!(overworld_id, OVERWORLD_ZONE);
    let overworld_template = ZoneTemplate {
        id: ZoneTemplate::compute_id(&[]),
        width: 0,
        height: 0,
        tiles: Vec::new(),
        anchors: Vec::new(),
    };
    let overworld_zone = Zone {
        id: overworld_id,
        kind: ZoneKind::Overworld,
        parent: ZoneParent::Overworld,
        world_id: WORLD_SURFACE,
        width: 1024,
        height: 1024,
        template_id: overworld_template.id,
        anchors: Vec::new(),
    };
    registry.insert(overworld_zone, overworld_template);

    // Per-building multi-story generation.
    for building in buildings {
        let floor_count = building_floor_count(building.kind);
        if floor_count < 2 {
            continue;
        }

        let mut floor_zone_ids: Vec<ZoneId> = Vec::with_capacity(floor_count as usize);

        for floor in 0..floor_count {
            let (w, h, tiles, anchors) = build_floor_tiles(building.kind, floor);
            let template_id = ZoneTemplate::compute_id(&tiles);
            let template = ZoneTemplate {
                id: template_id,
                width: w,
                height: h,
                tiles,
                anchors: anchors.clone(),
            };
            let zone_id = registry.alloc_id();
            let zone = Zone {
                id: zone_id,
                kind: ZoneKind::BuildingFloor { floor },
                parent: ZoneParent::Settlement(building.settlement_id),
                world_id: WORLD_SURFACE,
                width: w,
                height: h,
                template_id,
                anchors,
            };
            registry.insert(zone, template);
            floor_zone_ids.push(zone_id);
        }

        building_to_floor0.insert(building.id, floor_zone_ids[0]);

        // Staircase portal pairs between adjacent floors.
        for i in 0..(floor_zone_ids.len() - 1) {
            let lower = floor_zone_ids[i];
            let upper = floor_zone_ids[i + 1];
            let up_anchor = SmolStr::new(format!("stair_up_{i}"));
            let down_anchor = SmolStr::new(format!("stair_down_{}", i + 1));

            // Lower → upper.
            topology.add_portal(Portal {
                id: next_portal_id,
                kind: PortalKind::Staircase,
                from_zone: lower,
                from_anchor: up_anchor.clone(),
                trigger_radius: 1.0,
                traversal_cost: 1.0,
                faction_permeable: true,
                one_way: false,
                to_zone: upper,
                to_anchor: down_anchor.clone(),
                shape: None,
            });
            next_portal_id += 1;

            // Upper → lower (reverse).
            topology.add_portal(Portal {
                id: next_portal_id,
                kind: PortalKind::Staircase,
                from_zone: upper,
                from_anchor: down_anchor,
                trigger_radius: 1.0,
                traversal_cost: 1.0,
                faction_permeable: true,
                one_way: false,
                to_zone: lower,
                to_anchor: up_anchor,
                shape: None,
            });
            next_portal_id += 1;
        }
    }

    // Underground chain (the Sunken Realm): 2 zones — depth 1 (shallow) and
    // depth 2 (deep). Each depth gets a procedurally generated cave room layout
    // seeded by depth + world seed so layouts differ between levels.
    let mut underground_ids: Vec<ZoneId> = Vec::with_capacity(2);
    for depth in 1u8..=2 {
        let (w, h, tiles, anchors) = cave_zone_tiles(depth, _seed);
        let template_id = ZoneTemplate::compute_id(&tiles);
        let template = ZoneTemplate {
            id: template_id,
            width: w,
            height: h,
            tiles,
            anchors: anchors.clone(),
        };
        let zone_id = registry.alloc_id();
        let zone = Zone {
            id: zone_id,
            kind: ZoneKind::Underground { depth },
            parent: ZoneParent::Underground,
            world_id: WORLD_SUNKEN_REALM,
            width: w,
            height: h,
            template_id,
            anchors,
        };
        registry.insert(zone, template);
        underground_ids.push(zone_id);
    }

    // Overworld → depth 1 via CaveEntrance (bidirectional).
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::CaveEntrance,
        from_zone: overworld_id,
        from_anchor: SmolStr::new("cave_entrance"),
        trigger_radius: 2.0,
        traversal_cost: 2.0,
        faction_permeable: true,
        one_way: false,
        to_zone: underground_ids[0],
        to_anchor: SmolStr::new("up"),
        shape: None,
    });
    next_portal_id += 1;
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::CaveEntrance,
        from_zone: underground_ids[0],
        from_anchor: SmolStr::new("up"),
        trigger_radius: 2.0,
        traversal_cost: 2.0,
        faction_permeable: true,
        one_way: false,
        to_zone: overworld_id,
        to_anchor: SmolStr::new("cave_entrance"),
        shape: None,
    });
    next_portal_id += 1;

    // Depth 1 → depth 2 via SealRift (bidirectional).
    let upper = underground_ids[0];
    let lower = underground_ids[1];
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::SealRift,
        from_zone: upper,
        from_anchor: SmolStr::new("down"),
        trigger_radius: 1.5,
        traversal_cost: 1.0,
        faction_permeable: true,
        one_way: false,
        to_zone: lower,
        to_anchor: SmolStr::new("up"),
        shape: None,
    });
    next_portal_id += 1;
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::SealRift,
        from_zone: lower,
        from_anchor: SmolStr::new("up"),
        trigger_radius: 1.5,
        traversal_cost: 1.0,
        faction_permeable: true,
        one_way: false,
        to_zone: upper,
        to_anchor: SmolStr::new("down"),
        shape: None,
    });
    next_portal_id += 1;

    // ── Placeholder worlds — stub entry zones ─────────────────────────────────
    //
    // Devil's Casino Realm (WORLD_DEVILS_CASINO = WorldId(3)) and
    // Hivemind Fungus World (WORLD_HIVEMIND_FUNGUS = WorldId(4)) each get a
    // single stub Underground zone (depth 0 used as a lobby/placeholder).
    // The surface overworld connects to them via CasinoPortal / FungusPortal.

    let stub_tiles: Vec<InteriorTile> = vec![InteriorTile::Floor; 8 * 8];
    let stub_template_id = ZoneTemplate::compute_id(&stub_tiles);
    let stub_template = ZoneTemplate {
        id: stub_template_id,
        width: 8,
        height: 8,
        tiles: stub_tiles,
        anchors: vec![ZoneAnchor {
            name: SmolStr::new("entrance"),
            pos: Vec2::new(4.0, 4.0),
        }],
    };
    registry.templates.entry(stub_template_id).or_insert(stub_template.clone());

    // Devil's Casino stub zone.
    let casino_zone_id = registry.alloc_id();
    registry.zones.insert(
        casino_zone_id,
        Zone {
            id: casino_zone_id,
            kind: ZoneKind::Underground { depth: 0 },
            parent: ZoneParent::Underground,
            world_id: WORLD_DEVILS_CASINO,
            width: 8,
            height: 8,
            template_id: stub_template_id,
            anchors: stub_template.anchors.clone(),
        },
    );
    // Surface → Casino (one-way stub portal from overworld).
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::CasinoPortal,
        from_zone: overworld_id,
        from_anchor: SmolStr::new("casino_portal"),
        trigger_radius: 2.0,
        traversal_cost: 5.0,
        faction_permeable: true,
        one_way: false,
        to_zone: casino_zone_id,
        to_anchor: SmolStr::new("entrance"),
        shape: None,
    });
    next_portal_id += 1;
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::CasinoPortal,
        from_zone: casino_zone_id,
        from_anchor: SmolStr::new("entrance"),
        trigger_radius: 2.0,
        traversal_cost: 5.0,
        faction_permeable: true,
        one_way: false,
        to_zone: overworld_id,
        to_anchor: SmolStr::new("casino_portal"),
        shape: None,
    });
    next_portal_id += 1;

    // Hivemind Fungus stub zone.
    let fungus_zone_id = registry.alloc_id();
    registry.zones.insert(
        fungus_zone_id,
        Zone {
            id: fungus_zone_id,
            kind: ZoneKind::Underground { depth: 0 },
            parent: ZoneParent::Underground,
            world_id: WORLD_HIVEMIND_FUNGUS,
            width: 8,
            height: 8,
            template_id: stub_template_id,
            anchors: stub_template.anchors.clone(),
        },
    );
    // Surface → Fungus (bidirectional stub portal from overworld).
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::FungusPortal,
        from_zone: overworld_id,
        from_anchor: SmolStr::new("fungus_portal"),
        trigger_radius: 2.0,
        traversal_cost: 5.0,
        faction_permeable: true,
        one_way: false,
        to_zone: fungus_zone_id,
        to_anchor: SmolStr::new("entrance"),
        shape: None,
    });
    next_portal_id += 1;
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::FungusPortal,
        from_zone: fungus_zone_id,
        from_anchor: SmolStr::new("entrance"),
        trigger_radius: 2.0,
        traversal_cost: 5.0,
        faction_permeable: true,
        one_way: false,
        to_zone: overworld_id,
        to_anchor: SmolStr::new("fungus_portal"),
        shape: None,
    });
    #[allow(unused_assignments)]
    { next_portal_id += 1; }

    (registry, topology, building_to_floor0)
}

// Tile generation helpers live in `crate::dungeon`.
use crate::dungeon::{build_floor_tiles, building_floor_count, cave_zone_tiles};

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overworld_constant() {
        assert_eq!(OVERWORLD_ZONE, ZoneId(0));
    }

    #[test]
    fn test_topology_exits() {
        let mut topo = ZoneTopology::default();
        topo.add_portal(Portal {
            id: 0,
            kind: PortalKind::Door,
            from_zone: ZoneId(1),
            from_anchor: SmolStr::new("a"),
            trigger_radius: 1.0,
            traversal_cost: 1.0,
            faction_permeable: true,
            one_way: false,
            to_zone: ZoneId(2),
            to_anchor: SmolStr::new("b"),
            shape: None,
        });
        let exits: Vec<&Portal> = topo.exits_from(ZoneId(1)).collect();
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].to_zone, ZoneId(2));
        assert_eq!(topo.exits_from(ZoneId(2)).count(), 0);
    }

    #[test]
    fn test_hop_distance() {
        let mut topo = ZoneTopology::default();
        // Chain 0 → 1 → 2
        topo.add_portal(Portal {
            id: 0,
            kind: PortalKind::Door,
            from_zone: ZoneId(0),
            from_anchor: SmolStr::new("a"),
            trigger_radius: 1.0,
            traversal_cost: 1.0,
            faction_permeable: true,
            one_way: false,
            to_zone: ZoneId(1),
            to_anchor: SmolStr::new("b"),
            shape: None,
        });
        topo.add_portal(Portal {
            id: 1,
            kind: PortalKind::Door,
            from_zone: ZoneId(1),
            from_anchor: SmolStr::new("a"),
            trigger_radius: 1.0,
            traversal_cost: 1.0,
            faction_permeable: true,
            one_way: false,
            to_zone: ZoneId(2),
            to_anchor: SmolStr::new("b"),
            shape: None,
        });
        assert_eq!(topo.hop_distance(ZoneId(0), ZoneId(2)), Some(2));
        assert_eq!(topo.hop_distance(ZoneId(0), ZoneId(0)), Some(0));
        assert_eq!(topo.hop_distance(ZoneId(2), ZoneId(0)), None);
    }
}
