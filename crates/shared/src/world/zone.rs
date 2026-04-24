//! Zone Graph — spatial hierarchy of worldspace, interiors, and the Underdark.
//!
//! A `Zone` is a self-contained tile grid (overworld region, building floor,
//! dungeon level, underdark cave). Zones connect via `Portal`s. Entities carry
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

/// What category of zone this is (overworld, building floor, dungeon, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneKind {
    Overworld,
    BuildingFloor { floor: u8 },
    Dungeon { depth: u8 },
    Underdark { depth: u8 },
}

/// Parent relationship — used for spatial ownership and cleanup semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneParent {
    Overworld,
    Settlement(Uuid),
    Dungeon,
    Underdark,
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
    UnderDarkRift,
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
}

// ── Resources (Bevy) ──────────────────────────────────────────────────────────

/// Central registry of all zones and templates.
#[derive(Resource, Default, Clone, Debug)]
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
#[derive(Resource, Default, Clone, Debug)]
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

use crate::world::civilization::{Building, BuildingKind};

/// Generate zone graph from a list of buildings plus a seeded underdark chain.
///
/// Each multi-story building produces N `BuildingFloor` zones with
/// `Staircase` portal pairs connecting adjacent floors. A small 3-level
/// underdark chain is generated unconditionally for testing; it attaches to
/// the overworld via a `CaveEntrance` portal.
pub fn generate_zones(
    buildings: &[Building],
    _seed: u64,
) -> (ZoneRegistry, ZoneTopology) {
    let mut registry = ZoneRegistry::default();
    let mut topology = ZoneTopology::default();
    let mut next_portal_id: u32 = 0;

    // Zone 0 — overworld.
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
        width: 0,
        height: 0,
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
                width: w,
                height: h,
                template_id,
                anchors,
            };
            registry.insert(zone, template);
            floor_zone_ids.push(zone_id);
        }

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
            });
            next_portal_id += 1;
        }
    }

    // Underdark chain: 3 zones depth 1→3. Overworld attaches via CaveEntrance
    // portal to depth 1; UnderDarkRift portals link deeper layers.
    let underdark_template_tiles: Vec<InteriorTile> = vec![InteriorTile::Floor; 16 * 16];
    let underdark_template_id = ZoneTemplate::compute_id(&underdark_template_tiles);
    let underdark_template = ZoneTemplate {
        id: underdark_template_id,
        width: 16,
        height: 16,
        tiles: underdark_template_tiles.clone(),
        anchors: vec![
            ZoneAnchor {
                name: SmolStr::new("up"),
                pos: Vec2::new(1.0, 1.0),
            },
            ZoneAnchor {
                name: SmolStr::new("down"),
                pos: Vec2::new(14.0, 14.0),
            },
        ],
    };

    let mut underdark_ids: Vec<ZoneId> = Vec::with_capacity(3);
    for depth in 1u8..=3 {
        let zone_id = registry.alloc_id();
        let zone = Zone {
            id: zone_id,
            kind: ZoneKind::Underdark { depth },
            parent: ZoneParent::Underdark,
            width: 16,
            height: 16,
            template_id: underdark_template_id,
            anchors: underdark_template.anchors.clone(),
        };
        // Reuse the same template across all three.
        registry.insert(zone, underdark_template.clone());
        underdark_ids.push(zone_id);
    }

    // Overworld → Underdark depth 1 via CaveEntrance (bidirectional).
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::CaveEntrance,
        from_zone: overworld_id,
        from_anchor: SmolStr::new("cave_entrance"),
        trigger_radius: 2.0,
        traversal_cost: 2.0,
        faction_permeable: true,
        one_way: false,
        to_zone: underdark_ids[0],
        to_anchor: SmolStr::new("up"),
    });
    next_portal_id += 1;
    topology.add_portal(Portal {
        id: next_portal_id,
        kind: PortalKind::CaveEntrance,
        from_zone: underdark_ids[0],
        from_anchor: SmolStr::new("up"),
        trigger_radius: 2.0,
        traversal_cost: 2.0,
        faction_permeable: true,
        one_way: false,
        to_zone: overworld_id,
        to_anchor: SmolStr::new("cave_entrance"),
    });
    next_portal_id += 1;

    // Deeper links via UnderDarkRift (bidirectional).
    for i in 0..(underdark_ids.len() - 1) {
        let upper = underdark_ids[i];
        let lower = underdark_ids[i + 1];
        topology.add_portal(Portal {
            id: next_portal_id,
            kind: PortalKind::UnderDarkRift,
            from_zone: upper,
            from_anchor: SmolStr::new("down"),
            trigger_radius: 1.5,
            traversal_cost: 1.0,
            faction_permeable: true,
            one_way: false,
            to_zone: lower,
            to_anchor: SmolStr::new("up"),
        });
        next_portal_id += 1;
        topology.add_portal(Portal {
            id: next_portal_id,
            kind: PortalKind::UnderDarkRift,
            from_zone: lower,
            from_anchor: SmolStr::new("up"),
            trigger_radius: 1.5,
            traversal_cost: 1.0,
            faction_permeable: true,
            one_way: false,
            to_zone: upper,
            to_anchor: SmolStr::new("down"),
        });
        next_portal_id += 1;
    }

    (registry, topology)
}

/// How many BuildingFloor zones does this kind produce. 0/1 = no interior zones.
fn building_floor_count(kind: BuildingKind) -> u8 {
    match kind {
        BuildingKind::Tavern | BuildingKind::Barracks => 2,
        BuildingKind::Tower => 4,
        BuildingKind::Keep => 3,
        _ => 0,
    }
}

/// Produce (width, height, tiles, anchors) for a given building floor.
fn build_floor_tiles(
    kind: BuildingKind,
    floor: u8,
) -> (u16, u16, Vec<InteriorTile>, Vec<ZoneAnchor>) {
    match kind {
        BuildingKind::Tavern => tavern_floor(floor),
        BuildingKind::Barracks => barracks_floor(floor),
        BuildingKind::Tower => tower_floor(floor, 6),
        BuildingKind::Keep => tower_floor(floor, 10),
        _ => (0, 0, Vec::new(), Vec::new()),
    }
}

fn tavern_floor(floor: u8) -> (u16, u16, Vec<InteriorTile>, Vec<ZoneAnchor>) {
    let w: u16 = 8;
    let h: u16 = 8;
    let mut tiles = vec![InteriorTile::Floor; (w * h) as usize];
    let mut anchors = Vec::new();
    let stair_pos = Vec2::new(3.0, 3.0);
    let stair_idx = (stair_pos.y as usize) * w as usize + (stair_pos.x as usize);
    tiles[stair_idx] = InteriorTile::Stair;

    match floor {
        0 => {
            anchors.push(ZoneAnchor {
                name: SmolStr::new("entrance"),
                pos: Vec2::new(0.0, 4.0),
            });
            anchors.push(ZoneAnchor {
                name: SmolStr::new("stair_up_0"),
                pos: stair_pos,
            });
        }
        _ => {
            // Upper floor: add a balcony tile + stair_down anchor.
            let balcony_idx = 5 * w as usize + 5;
            tiles[balcony_idx] = InteriorTile::Balcony;
            anchors.push(ZoneAnchor {
                name: SmolStr::new("stair_down_1"),
                pos: stair_pos,
            });
        }
    }

    (w, h, tiles, anchors)
}

fn barracks_floor(floor: u8) -> (u16, u16, Vec<InteriorTile>, Vec<ZoneAnchor>) {
    let w: u16 = 8;
    let h: u16 = 8;
    let mut tiles = vec![InteriorTile::Floor; (w * h) as usize];
    let stair_pos = Vec2::new(3.0, 3.0);
    let stair_idx = (stair_pos.y as usize) * w as usize + (stair_pos.x as usize);
    tiles[stair_idx] = InteriorTile::Stair;

    let mut anchors = Vec::new();
    match floor {
        0 => {
            anchors.push(ZoneAnchor {
                name: SmolStr::new("entrance"),
                pos: Vec2::new(0.0, 4.0),
            });
            anchors.push(ZoneAnchor {
                name: SmolStr::new("stair_up_0"),
                pos: stair_pos,
            });
        }
        _ => {
            // Upper floor: west-wall windows (x = 0, rows 1..h-1).
            for y in 1..(h as usize - 1) {
                tiles[y * w as usize] = InteriorTile::Window;
            }
            anchors.push(ZoneAnchor {
                name: SmolStr::new("stair_down_1"),
                pos: stair_pos,
            });
        }
    }
    (w, h, tiles, anchors)
}

/// Tower / Keep. `size` is side length (6 for Tower, 10 for Keep battlements).
fn tower_floor(floor: u8, size: u16) -> (u16, u16, Vec<InteriorTile>, Vec<ZoneAnchor>) {
    // Tower always uses 6×6 for interior floors; keep uses 6×6 for interior
    // floors and 10×10 for battlements — but the issue spec says
    // "Keep: same as Tower but floor 3 = battlements 10×10". So interior
    // floors are 6×6 for both; only the battlements dimension changes.
    let is_battlement = floor >= 3; // floors 0..=2 interior, floor 3 = battlements
    let (w, h): (u16, u16) = if is_battlement { (size, size) } else { (6, 6) };
    let mut tiles = vec![InteriorTile::Floor; (w * h) as usize];
    let mut anchors = Vec::new();

    let stair_pos = Vec2::new(2.0, 2.0);
    let stair_idx = (stair_pos.y as usize) * w as usize + (stair_pos.x as usize);
    if (stair_idx) < tiles.len() {
        tiles[stair_idx] = InteriorTile::Stair;
    }

    if floor == 0 {
        anchors.push(ZoneAnchor {
            name: SmolStr::new("entrance"),
            pos: Vec2::new(0.0, w as f32 / 2.0),
        });
    }

    if is_battlement {
        // Roof tiles around perimeter.
        for y in 0..h as usize {
            for x in 0..w as usize {
                if x == 0 || y == 0 || x == (w as usize - 1) || y == (h as usize - 1) {
                    tiles[y * w as usize + x] = InteriorTile::Roof;
                }
            }
        }
        anchors.push(ZoneAnchor {
            name: SmolStr::new(format!("stair_down_{floor}")),
            pos: stair_pos,
        });
    } else {
        // Regular interior floor — add both up and down stairs where applicable.
        if floor > 0 {
            anchors.push(ZoneAnchor {
                name: SmolStr::new(format!("stair_down_{floor}")),
                pos: stair_pos,
            });
        }
        anchors.push(ZoneAnchor {
            name: SmolStr::new(format!("stair_up_{floor}")),
            pos: stair_pos,
        });
    }

    (w, h, tiles, anchors)
}

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
        });
        assert_eq!(topo.hop_distance(ZoneId(0), ZoneId(2)), Some(2));
        assert_eq!(topo.hop_distance(ZoneId(0), ZoneId(0)), Some(0));
        assert_eq!(topo.hop_distance(ZoneId(2), ZoneId(0)), None);
    }
}
