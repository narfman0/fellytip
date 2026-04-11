// Shared ECS components — replicated between server and client.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// 2-D world position (game units, not pixels).
///
/// This is the single canonical position component replicated
/// from server to every connected client.
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct WorldPosition {
    pub x: f32,
    pub y: f32,
}

/// Current and maximum hit points — replicated so clients can render health bars.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct Health {
    pub current: i32,
    pub max: i32,
}

/// Player experience and level — replicated so clients can render the HUD.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct Experience {
    pub xp: u32,
    pub level: u32,
    /// XP required to reach the next level.
    pub xp_to_next: u32,
}

impl Experience {
    pub fn new() -> Self {
        Self { xp: 0, level: 1, xp_to_next: 100 }
    }
}

impl Default for Experience {
    fn default() -> Self {
        Self::new()
    }
}
