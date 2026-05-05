//! Server-only plugin modules.
//!
//! The bulk of the game-logic plugins now live in `fellytip-game`. This module
//! contains only the DM (Dungeon Master) BRP handlers, which manipulate
//! server-only resources and remain in the server crate.
//!
//! Re-exports from `fellytip_game::plugins` are provided so existing call sites
//! such as `fellytip_server::plugins::bot::BotController` keep working.

pub mod dm;

pub use fellytip_game::plugins::{
    ai,
    bot,
    character_persistence,
    combat,
    combat_test,
    dungeon,
    ecology,
    interest,
    map_gen,
    nav,
    party,
    perf,
    persistence,
    portal,
    story,
    world_sim,
};
