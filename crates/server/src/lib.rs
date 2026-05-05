//! `fellytip-server` is a thin shim around `fellytip-game`.
//!
//! The game logic lives in [`fellytip_game`]; this crate adds only the
//! server-side DM (Dungeon Master) BRP method handlers in [`plugins::dm`].
//!
//! Re-exports from `fellytip_game` are provided for backward compatibility so
//! existing callers (client, ralph scenarios, worldwatch) keep compiling.

pub mod plugins;

// Re-export the game plugin so existing callers (`fellytip_server::ServerGamePlugin`)
// continue to compile without churn.
pub use fellytip_game::{MapGenConfig, ServerGamePlugin};
