//! Pure world-data types extracted from `fellytip-shared`.
//!
//! This crate holds the deterministic procgen + topology types (maps, zones,
//! factions, ecology, civilizations, dungeons, caves, grids, populations)
//! plus the small math/noise utilities they share. No ECS schedules, no I/O.

pub mod bounds;
pub mod cave;
pub mod civilization;
pub mod dungeon;
pub mod ecology;
pub mod faction;
pub mod grid;
pub mod map;
pub mod math;
pub mod mesh;
pub mod population;
pub mod zone;
