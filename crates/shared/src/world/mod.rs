//! World data types + simulation systems.
//!
//! Pure data and procgen types live in the `fellytip-world-types` crate and
//! are re-exported here so existing `crate::world::...` paths keep working.
//! Modules that drive Bevy schedules / events (pathfinding, story, war,
//! schedule, art_direction) remain in `fellytip-shared`.

pub use fellytip_world_types::{
    cave,
    civilization,
    dungeon,
    ecology,
    faction,
    grid,
    map,
    population,
    zone,
};

pub mod art_direction;
pub mod pathfinding;
pub mod schedule;
pub mod story;
pub mod war;
