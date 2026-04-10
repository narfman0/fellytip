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
