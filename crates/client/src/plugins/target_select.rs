//! Screen-space entity picking: finds the nearest hostile entity under the cursor.
//!
//! Projects each enemy's world position to viewport coordinates each frame and
//! tracks the closest one within `PICK_RADIUS_PX` pixels of the cursor.
//! The result is exposed as the `HoveredTarget` resource consumed by the input
//! system (targeted left-click attack) and the action menu (context-aware
//! right-click menu).

use bevy::prelude::*;
use uuid::Uuid;
use fellytip_shared::bridge::{CombatParticipant, ExperienceReward};
use fellytip_shared::components::{Pacifist, WorldPosition};
use super::action_menu::EguiPointerConsumed;
use super::camera::OrbitCamera;

/// How close (in logical pixels) the cursor must be to an enemy's projected
/// screen position for that enemy to be considered hovered.
const PICK_RADIUS_PX: f32 = 60.0;

/// The hostile entity (if any) currently closest to the cursor, together with
/// its combat UUID (used to route the targeted attack to the server).
#[derive(Resource, Default)]
pub struct HoveredTarget(pub Option<(Entity, Uuid)>);

pub struct TargetSelectPlugin;

impl Plugin for TargetSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HoveredTarget>()
            .add_systems(Update, update_hovered_target);
    }
}

#[allow(clippy::type_complexity)]
fn update_hovered_target(
    camera_q: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    windows: Query<&Window>,
    enemies: Query<
        (Entity, &WorldPosition, &CombatParticipant),
        (With<ExperienceReward>, Without<Pacifist>),
    >,
    mut hovered: ResMut<HoveredTarget>,
    egui_consumed: Option<Res<EguiPointerConsumed>>,
) {
    hovered.0 = None;

    // Don't pick through egui overlays.
    if egui_consumed.as_ref().is_some_and(|e| e.0) {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else { return };
    let Ok((camera, camera_transform)) = camera_q.single() else { return };

    let mut closest: Option<(Entity, f32, Uuid)> = None;

    for (entity, world_pos, participant) in &enemies {
        // WorldPosition (x, y, z_elevation) → Bevy world (x, z_elevation, y).
        // Offset upward by ~0.9 units to hit the torso rather than the feet.
        let bevy_pos = Vec3::new(world_pos.x, world_pos.z + 0.9, world_pos.y);
        if let Ok(screen_pos) = camera.world_to_viewport(camera_transform, bevy_pos) {
            let dist = cursor_pos.distance(screen_pos);
            if dist < PICK_RADIUS_PX
                && (closest.is_none() || dist < closest.as_ref().unwrap().1)
            {
                closest = Some((entity, dist, participant.id.0));
            }
        }
    }

    hovered.0 = closest.map(|(e, _, uuid)| (e, uuid));
}
