//! Pure tile-generation helpers for building interiors and dungeon zones.
//!
//! These functions take zone dimensions, building kind, floor index, and/or a
//! seed, and return tile layouts (floor, wall, roof, stair placements) together
//! with named anchor positions. They have no Bevy/ECS dependencies and can be
//! called from both the server (zone generation at startup) and the client
//! (local preview, minimap rendering).
//!
//! # Entry points
//!
//! * [`building_floor_count`] — how many `BuildingFloor` zones a building kind produces.
//! * [`build_floor_tiles`] — dispatch to the per-kind tile builder.
//! * [`tavern_floor`], [`barracks_floor`], [`tower_floor`] — concrete layouts.

use glam::Vec2;
use smol_str::SmolStr;

use crate::world::{
    civilization::BuildingKind,
    zone::{InteriorTile, ZoneAnchor},
};

/// How many BuildingFloor zones does this building kind produce. 0/1 = no interior zones.
pub fn building_floor_count(kind: BuildingKind) -> u8 {
    match kind {
        BuildingKind::Tavern | BuildingKind::Barracks => 2,
        BuildingKind::Tower => 4,
        BuildingKind::Keep => 3,
        BuildingKind::CapitalTower => 5,
        _ => 0,
    }
}

/// Produce `(width, height, tiles, anchors)` for a given building floor.
pub fn build_floor_tiles(
    kind: BuildingKind,
    floor: u8,
) -> (u16, u16, Vec<InteriorTile>, Vec<ZoneAnchor>) {
    match kind {
        BuildingKind::Tavern => tavern_floor(floor),
        BuildingKind::Barracks => barracks_floor(floor),
        BuildingKind::Tower => tower_floor(floor, 6),
        BuildingKind::Keep => tower_floor(floor, 10),
        BuildingKind::CapitalTower => capital_tower_floor(floor),
        _ => (0, 0, Vec::new(), Vec::new()),
    }
}

/// Tile layout for a tavern floor (8×8 grid).
///
/// Floor 0 has an entrance anchor on the west wall and a stair-up at (3, 3).
/// Upper floors gain a balcony tile and a stair-down anchor.
pub fn tavern_floor(floor: u8) -> (u16, u16, Vec<InteriorTile>, Vec<ZoneAnchor>) {
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

/// Tile layout for a barracks floor (8×8 grid).
///
/// Floor 0: entrance + stair-up at (3, 3).
/// Upper floors: west-wall windows on rows 1..h-1, stair-down anchor.
pub fn barracks_floor(floor: u8) -> (u16, u16, Vec<InteriorTile>, Vec<ZoneAnchor>) {
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

/// Tile layout for a Tower or Keep floor. `size` is the battlement side length
/// (6 for Tower, 10 for Keep).
///
/// Floors 0–2 are 6×6 interior rooms with stair tiles; floor ≥ 3 is a battlement
/// of `size × size` with `Roof` tiles around the perimeter.
pub fn tower_floor(floor: u8, size: u16) -> (u16, u16, Vec<InteriorTile>, Vec<ZoneAnchor>) {
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
    if stair_idx < tiles.len() {
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

/// Tile layout for a Capital Tower floor (20×20 circular grid).
///
/// The tower occupies a 20×20 grid centered at `(10, 10)` with a circular
/// footprint (radius ~9.5). Interior tiles at distance < 8.5 are `Floor`;
/// perimeter tiles at 8.5 ≤ distance ≤ 9.5 are `Wall`; exterior tiles
/// (distance > 9.5) are `Void`.
///
/// A 3×3 stair column at center `(9..12, 9..12)` holds `Stair` tiles on
/// floors 0–3, plus `Stair` (down) on floor 4. Floor 0 has a doorway
/// entrance gap in the south wall at x=10. Floor 4 (roof/battlements) uses
/// `Roof` tiles for the outer ring.
pub fn capital_tower_floor(floor: u8) -> (u16, u16, Vec<InteriorTile>, Vec<ZoneAnchor>) {
    let w: u16 = 20;
    let h: u16 = 20;
    let cx = 10.0_f32;
    let cz = 10.0_f32;
    let is_roof = floor >= 4;

    let mut tiles = vec![InteriorTile::Void; (w * h) as usize];
    let mut anchors = Vec::new();

    for z in 0..h as usize {
        for x in 0..w as usize {
            let dx = x as f32 + 0.5 - cx;
            let dz = z as f32 + 0.5 - cz;
            let dist = (dx * dx + dz * dz).sqrt();

            let tile = if dist > 9.5 {
                InteriorTile::Void
            } else if dist >= 8.5 {
                // Perimeter wall — roof floor uses Roof tiles for battlements.
                if is_roof {
                    InteriorTile::Roof
                } else {
                    InteriorTile::Wall
                }
            } else {
                InteriorTile::Floor
            };

            tiles[z * w as usize + x] = tile;
        }
    }

    // Stair column at center (x: 9..12, z: 9..12).
    let stair_pos = Vec2::new(10.0, 10.0);
    for sz in 9..12usize {
        for sx in 9..12usize {
            let idx = sz * w as usize + sx;
            tiles[idx] = InteriorTile::Stair;
        }
    }

    // Floor 0: south wall entrance gap at x=10 (doorway at z=19 and z=18).
    if floor == 0 {
        for entrance_z in [18usize, 19usize] {
            let idx = entrance_z * w as usize + 10;
            tiles[idx] = InteriorTile::Floor;
        }
        anchors.push(ZoneAnchor {
            name: SmolStr::new("entrance"),
            pos: Vec2::new(10.0, 19.0),
        });
    }

    // Stair anchors.
    if floor < 4 {
        anchors.push(ZoneAnchor {
            name: SmolStr::new(format!("stair_up_{floor}")),
            pos: stair_pos,
        });
    }
    if floor > 0 {
        anchors.push(ZoneAnchor {
            name: SmolStr::new(format!("stair_down_{floor}")),
            pos: stair_pos,
        });
    }

    (w, h, tiles, anchors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capital_tower_has_five_floors() {
        assert_eq!(building_floor_count(BuildingKind::CapitalTower), 5);
    }

    #[test]
    fn capital_tower_floor0_returns_tiles() {
        let (w, h, tiles, anchors) = capital_tower_floor(0);
        assert_eq!(w, 20);
        assert_eq!(h, 20);
        assert_eq!(tiles.len(), 400);
        // Must have some Floor tiles and some Wall tiles.
        assert!(tiles.iter().any(|t| *t == InteriorTile::Floor));
        assert!(tiles.iter().any(|t| *t == InteriorTile::Wall));
        // Floor 0 must have an entrance anchor.
        assert!(anchors.iter().any(|a| a.name == "entrance"));
        assert!(anchors.iter().any(|a| a.name == "stair_up_0"));
    }

    #[test]
    fn capital_tower_stair_tiles_at_center() {
        let (w, _, tiles, _) = capital_tower_floor(0);
        // Stair column is at grid positions (9..12, 9..12).
        for sz in 9..12usize {
            for sx in 9..12usize {
                assert_eq!(
                    tiles[sz * w as usize + sx],
                    InteriorTile::Stair,
                    "expected Stair at ({sx}, {sz})"
                );
            }
        }
    }

    #[test]
    fn capital_tower_roof_floor_has_roof_tiles() {
        let (_, _, tiles, anchors) = capital_tower_floor(4);
        // Floor 4 is the roof — perimeter ring must be Roof tiles.
        assert!(tiles.iter().any(|t| *t == InteriorTile::Roof));
        // No entrance anchor on roof floor.
        assert!(!anchors.iter().any(|a| a.name == "entrance"));
        // Stair-down anchor must exist.
        assert!(anchors.iter().any(|a| a.name == "stair_down_4"));
    }

    #[test]
    fn capital_tower_all_floors_build_via_dispatch() {
        for floor in 0..5u8 {
            let (w, h, tiles, _) = build_floor_tiles(BuildingKind::CapitalTower, floor);
            assert_eq!(w, 20, "floor {floor} width");
            assert_eq!(h, 20, "floor {floor} height");
            assert!(!tiles.is_empty(), "floor {floor} tiles should not be empty");
        }
    }

    #[test]
    fn tavern_has_two_floors() {
        assert_eq!(building_floor_count(BuildingKind::Tavern), 2);
    }

    #[test]
    fn tower_has_four_floors() {
        assert_eq!(building_floor_count(BuildingKind::Tower), 4);
    }

    #[test]
    fn keep_has_three_floors() {
        assert_eq!(building_floor_count(BuildingKind::Keep), 3);
    }

    #[test]
    fn tavern_floor0_has_entrance_anchor() {
        let (_, _, _, anchors) = tavern_floor(0);
        assert!(anchors.iter().any(|a| a.name == "entrance"));
        assert!(anchors.iter().any(|a| a.name == "stair_up_0"));
    }

    #[test]
    fn tavern_floor1_has_balcony() {
        let (w, _, tiles, anchors) = tavern_floor(1);
        let balcony_idx = 5 * w as usize + 5;
        assert_eq!(tiles[balcony_idx], InteriorTile::Balcony);
        assert!(anchors.iter().any(|a| a.name == "stair_down_1"));
    }

    #[test]
    fn barracks_upper_floor_has_windows() {
        let (w, _, tiles, _) = barracks_floor(1);
        // West wall (x=0) rows 1..h-1 should be Window.
        assert_eq!(tiles[1 * w as usize], InteriorTile::Window);
    }

    #[test]
    fn tower_battlement_has_roof_perimeter() {
        let (w, h, tiles, _) = tower_floor(3, 6);
        // All perimeter tiles should be Roof.
        for x in 0..w as usize {
            assert_eq!(tiles[x], InteriorTile::Roof, "top row x={x}");
            assert_eq!(tiles[(h as usize - 1) * w as usize + x], InteriorTile::Roof, "bottom row x={x}");
        }
        for y in 0..h as usize {
            assert_eq!(tiles[y * w as usize], InteriorTile::Roof, "left col y={y}");
            assert_eq!(tiles[y * w as usize + (w as usize - 1)], InteriorTile::Roof, "right col y={y}");
        }
    }
}
