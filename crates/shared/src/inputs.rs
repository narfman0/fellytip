//! Player input type sent over the PlayerInputChannel.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Intent the player can declare alongside movement.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum ActionIntent {
    BasicAttack,
    UseAbility(u8),
    Interact,
    Dodge,
}

/// One frame of player input sent from client → server.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct PlayerInput {
    /// Normalised movement direction; zero = standing still.
    pub move_dir: [f32; 2],
    /// Optional action to perform this frame.
    pub action: Option<ActionIntent>,
    /// Optional target entity (stable cross-session UUID).
    pub target: Option<Uuid>,
}
