//! Portal plugin — spawns trigger entities for every Portal in `ZoneTopology`
//! and handles per-tick proximity checks + zone transitions.
//!
//! Anchor world positions are currently `Vec2::ZERO` for all portals (see
//! TODOs below) — they will be wired properly once building world-space
//! coordinates are propagated into the zone graph.

use bevy::ecs::message::{Message, MessageReader, MessageWriter};
use bevy::prelude::*;

use fellytip_shared::{
    components::WorldPosition,
    world::zone::{ZoneMembership, ZoneTopology},
};

// ── Components ────────────────────────────────────────────────────────────────

/// Marker spawned on each `Portal` — its `WorldPosition` is the portal's
/// from-anchor world-space position and its `ZoneMembership` is `from_zone`.
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalTrigger {
    pub portal_id: u32,
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Emitted when an entity enters a portal's trigger radius. The `apply_zone_transitions`
/// system consumes these and performs the actual zone swap.
#[derive(Message, Clone, Copy, Debug)]
pub struct PlayerZoneTransition {
    pub entity: Entity,
    pub portal_id: u32,
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct PortalPlugin;

impl Plugin for PortalPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<PlayerZoneTransition>()
            .add_systems(Startup, setup_portal_triggers)
            .add_systems(
                FixedUpdate,
                (check_portal_triggers, apply_zone_transitions).chain(),
            );
    }
}

// ── Startup ───────────────────────────────────────────────────────────────────

/// Spawn one `PortalTrigger` entity per Portal.
///
/// TODO: replace `Vec2::ZERO` anchor positions once building world-space
/// coords are propagated into the zone graph (anchor.pos is currently
/// zone-local tile coordinates).
fn setup_portal_triggers(mut commands: Commands, topology: Option<Res<ZoneTopology>>) {
    let Some(topology) = topology else { return };

    for portal in &topology.portals {
        // TODO: compute the world-space position of `portal.from_anchor`
        // inside `portal.from_zone`. For now, all portal triggers live at
        // world origin — zone transitions will still work because the
        // check system also needs world positions to be correct first.
        commands.spawn((
            PortalTrigger { portal_id: portal.id },
            WorldPosition { x: 0.0, y: 0.0, z: 0.0 },
            ZoneMembership(portal.from_zone),
        ));
    }
}

// ── Tick systems ──────────────────────────────────────────────────────────────

/// For each non-trigger entity with a `ZoneMembership`, emit a
/// `PlayerZoneTransition` when it enters any same-zone `PortalTrigger`'s
/// radius.
fn check_portal_triggers(
    movers: Query<(Entity, &WorldPosition, &ZoneMembership), Without<PortalTrigger>>,
    triggers: Query<(&PortalTrigger, &WorldPosition, &ZoneMembership)>,
    topology: Option<Res<ZoneTopology>>,
    mut out: MessageWriter<PlayerZoneTransition>,
) {
    let Some(topology) = topology else { return };

    for (entity, pos, zone) in &movers {
        for (trigger, tpos, tzone) in &triggers {
            if tzone.0 != zone.0 {
                continue;
            }
            // Look up portal to get trigger_radius.
            let Some(portal) = topology.portals.iter().find(|p| p.id == trigger.portal_id)
            else {
                continue;
            };
            let dx = tpos.x - pos.x;
            let dy = tpos.y - pos.y;
            if dx * dx + dy * dy <= portal.trigger_radius * portal.trigger_radius {
                out.write(PlayerZoneTransition {
                    entity,
                    portal_id: portal.id,
                });
            }
        }
    }
}

/// Apply queued zone transitions: update `ZoneMembership` and move the
/// entity to the destination anchor.
///
/// TODO: replace `Vec2::ZERO` destination position with the world-space
/// position of `portal.to_anchor` inside `portal.to_zone`.
fn apply_zone_transitions(
    mut events: MessageReader<PlayerZoneTransition>,
    topology: Option<Res<ZoneTopology>>,
    mut q: Query<(&mut WorldPosition, &mut ZoneMembership)>,
) {
    let Some(topology) = topology else { return };

    for ev in events.read() {
        let Some(portal) = topology.portals.iter().find(|p| p.id == ev.portal_id) else {
            continue;
        };
        let Ok((mut pos, mut zone)) = q.get_mut(ev.entity) else {
            continue;
        };

        // Respect one_way: only traverse from `from_zone`.
        if portal.one_way && zone.0 != portal.from_zone {
            continue;
        }

        zone.0 = portal.to_zone;
        // TODO: move to actual to_anchor world position; for now just zero.
        pos.x = 0.0;
        pos.y = 0.0;
    }
}
