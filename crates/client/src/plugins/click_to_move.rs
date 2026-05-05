//! Click-to-move: left-click terrain to path the player to that location.
//!
//! Sends `MoveToMessage` to the server, which computes A* and drives the player
//! via `follow_navigation_goal`. Also draws cyan path gizmos for any entity
//! (NPC or PC) that currently has a `NavigationGoal`.

use bevy::{ecs::message::MessageWriter, prelude::*};
use fellytip_shared::{
    components::{NavigationGoal, WorldPosition},
    protocol::MoveToMessage,
};
use super::{action_menu::EguiPointerConsumed, camera::OrbitCamera, target_select::HoveredTarget};
use crate::{LocalPlayer, PredictedPosition};

pub struct ClickToMovePlugin;

impl Plugin for ClickToMovePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (handle_terrain_click, draw_entity_paths));
    }
}

/// On left-click (no UI or enemy target), ray-cast onto the ground plane and
/// send `MoveToMessage` to the server.
fn handle_terrain_click(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    player_q: Query<&PredictedPosition, With<LocalPlayer>>,
    hovered: Option<Res<HoveredTarget>>,
    egui_consumed: Option<Res<EguiPointerConsumed>>,
    mut writer: MessageWriter<MoveToMessage>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    // Don't steal clicks consumed by egui or used for combat targeting.
    if egui_consumed.as_ref().is_some_and(|e| e.0) {
        return;
    }
    if hovered.as_ref().is_some_and(|h| h.0.is_some()) {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else { return };
    let Ok((camera, camera_transform)) = camera_q.single() else { return };
    let Ok(pred) = player_q.single() else { return };

    // Cast a ray from the camera through the cursor into Bevy world space.
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor_pos) else { return };

    // Intersect the ray with the horizontal plane at the player's elevation.
    // Bevy y = up; player elevation in Bevy y = PredictedPosition.z.
    let ground_y = pred.z;
    let denom = ray.direction.y;
    if denom.abs() < 1e-6 {
        return; // Ray is nearly horizontal — no valid ground intersection.
    }
    let t = (ground_y - ray.origin.y) / denom;
    if t < 0.0 {
        return; // Ground is behind the camera.
    }
    let hit = ray.origin + ray.direction * t;

    // Bevy world (x, y_up, z_south) → game world (x=east, y=north, z=elevation).
    let game_x = hit.x;
    let game_y = -hit.z; // Bevy z is south, game y is north.
    let game_z = pred.z;

    writer.write(MoveToMessage { x: game_x, y: game_y, z: game_z });
}

/// Draw cyan gizmo lines along the A* path and a yellow sphere at the destination
/// for every entity (PC or NPC) that currently has a `NavigationGoal`.
fn draw_entity_paths(
    goals: Query<(&NavigationGoal, &WorldPosition)>,
    mut gizmos: Gizmos,
) {
    for (goal, pos) in &goals {
        // Bevy world space: x=east, y=up, z=south.  Convert game (x, y) → Bevy (x, z=−y).
        let lift = 0.3; // Raise gizmos slightly above ground so they're visible.

        // Draw line from current entity position through each waypoint.
        let mut prev = Vec3::new(pos.x, pos.z + lift, pos.y);
        for &[wx, wy] in &goal.path_world {
            let next = Vec3::new(wx, pos.z + lift, wy);
            gizmos.line(prev, next, Color::srgb(0.2, 0.85, 1.0));
            prev = next;
        }

        // Destination marker sphere.
        let dest = Vec3::new(goal.target[0], goal.target[2] + lift, goal.target[1]);
        gizmos.sphere(
            Isometry3d::from_translation(dest),
            0.4,
            Color::srgb(1.0, 0.85, 0.1),
        );
    }
}
