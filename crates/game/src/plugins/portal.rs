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
    protocol::{ClientPortalEntry, ZoneNeighborMessage, ZoneTileMessage},
    world::zone::{ZoneId, ZoneMembership, ZoneRegistry, ZoneTopology},
};

use crate::plugins::nav::{build_zone_nav_grids, ZoneNavGrids};

// ── Components ────────────────────────────────────────────────────────────────

/// Marker spawned on each `Portal` — its `WorldPosition` is the portal's
/// from-anchor world-space position and its `ZoneMembership` is `from_zone`.
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalTrigger {
    pub portal_id: u32,
}

/// Added to an entity after a portal traversal to prevent immediate re-triggering.
/// Counts down in seconds; the entity is immune to portal triggers while > 0.
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalCooldown(pub f32);

/// Cooldown duration in seconds after a zone transition before triggers re-arm.
const PORTAL_COOLDOWN_SECS: f32 = 2.0;

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
        app.init_resource::<ZoneRegistry>()
            .init_resource::<ZoneTopology>()
            .init_resource::<ZoneNavGrids>()
            .add_message::<PlayerZoneTransition>()
            .add_message::<ZoneTileMessage>()
            .add_message::<ZoneNeighborMessage>()
            .add_systems(
                Startup,
                (build_zone_nav_grids, setup_portal_triggers)
                    .chain()
                    .after(crate::plugins::map_gen::populate_zones),
            )
            .add_systems(
                FixedUpdate,
                (
                    tick_portal_cooldowns,
                    check_portal_triggers,
                    apply_zone_transitions,
                    send_zone_tiles,
                    send_zone_neighbors,
                ).chain(),
            );
    }
}

// ── Startup ───────────────────────────────────────────────────────────────────

/// Spawn one `PortalTrigger` entity per Portal.
///
/// For overworld-facing portals (e.g. building entrance doors) the
/// from-anchor's world-space position is looked up from `ZoneRegistry` and
/// used directly. For intra-zone portals (e.g. staircases inside a building)
/// the anchor still lives in zone-local tile coordinates; those triggers are
/// placed at the local coords for now — proximity checks against entities in
/// the same zone only become meaningful once in-zone entities track their
/// zone-local positions separately.
fn setup_portal_triggers(
    mut commands: Commands,
    topology: Option<Res<ZoneTopology>>,
    registry: Option<Res<ZoneRegistry>>,
) {
    let Some(topology) = topology else { return };
    let Some(registry) = registry else { return };

    for portal in &topology.portals {
        let world_pos = registry
            .get(portal.from_zone)
            .and_then(|zone| {
                zone.anchors
                    .iter()
                    .find(|a| a.name == portal.from_anchor)
                    .map(|a| (a.pos.x, a.pos.y))
            })
            .unwrap_or((0.0, 0.0));

        commands.spawn((
            PortalTrigger { portal_id: portal.id },
            WorldPosition { x: world_pos.0, y: world_pos.1, z: 0.0 },
            ZoneMembership(portal.from_zone),
        ));
    }
}

// ── Tick systems ──────────────────────────────────────────────────────────────

/// Tick down `PortalCooldown` timers and remove the component when expired.
fn tick_portal_cooldowns(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut PortalCooldown)>,
) {
    for (entity, mut cooldown) in &mut q {
        cooldown.0 -= time.delta_secs();
        if cooldown.0 <= 0.0 {
            commands.entity(entity).remove::<PortalCooldown>();
        }
    }
}

/// For each non-trigger entity with a `ZoneMembership`, emit a
/// `PlayerZoneTransition` when it enters any same-zone `PortalTrigger`'s
/// radius. Entities with an active `PortalCooldown` are skipped.
type MoverQuery<'w, 's> = Query<
    'w, 's,
    (Entity, &'static WorldPosition, &'static ZoneMembership),
    (Without<PortalTrigger>, Without<PortalCooldown>),
>;

fn check_portal_triggers(
    movers: MoverQuery,
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
/// entity to the destination anchor (as stored in `ZoneRegistry`).
///
/// On world-crossing portals (e.g. surface ↔ Sunken Realm), the entity's z
/// coordinate is reset to 0.0 so it enters the flat destination world at
/// ground level rather than retaining an elevation from the source world.
fn apply_zone_transitions(
    mut commands: Commands,
    mut events: MessageReader<PlayerZoneTransition>,
    topology: Option<Res<ZoneTopology>>,
    registry: Option<Res<ZoneRegistry>>,
    mut q: Query<(&mut WorldPosition, &mut ZoneMembership)>,
) {
    let Some(topology) = topology else { return };
    let Some(registry) = registry else { return };

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

        // Detect world crossing before updating zone membership.
        let crosses_world = topology.is_world_crossing(portal, &registry);

        zone.0 = portal.to_zone;

        // Move to the destination anchor's stored position. For overworld
        // anchors this is already world-space; for intra-zone anchors it's
        // zone-local tile coordinates (kept for now).
        if let Some(anchor_pos) = registry.get(portal.to_zone).and_then(|zone| {
            zone.anchors
                .iter()
                .find(|a| a.name == portal.to_anchor)
                .map(|a| (a.pos.x, a.pos.y))
        }) {
            pos.x = anchor_pos.0;
            pos.y = anchor_pos.1;
        } else {
            pos.x = 0.0;
            pos.y = 0.0;
        }

        // On world crossings, reset elevation so the entity enters the
        // destination world at ground level (z = 0.0 in the new coordinate
        // space).
        if crosses_world {
            pos.z = 0.0;
            tracing::info!(
                entity = ?ev.entity,
                portal_id = portal.id,
                to_zone = ?portal.to_zone,
                "World-crossing portal traversal — z reset to 0.0"
            );
        }

        // Apply a cooldown so the entity cannot immediately re-trigger a
        // portal at the destination anchor position.
        commands.entity(ev.entity).insert(PortalCooldown(PORTAL_COOLDOWN_SECS));
    }
}

/// Broadcast the destination zone + all 1-hop neighbor zones to clients for
/// each `PlayerZoneTransition`. In single-player this simply writes to the
/// local `ZoneTileMessage` event stream; MULTIPLAYER will filter per-client.
fn send_zone_tiles(
    mut events: MessageReader<PlayerZoneTransition>,
    topology: Option<Res<ZoneTopology>>,
    registry: Option<Res<ZoneRegistry>>,
    mut writer: MessageWriter<ZoneTileMessage>,
) {
    let (Some(topology), Some(registry)) = (topology, registry) else {
        return;
    };

    let mut seen: std::collections::HashSet<ZoneId> = std::collections::HashSet::new();
    for ev in events.read() {
        let Some(portal) = topology.portals.iter().find(|p| p.id == ev.portal_id) else {
            continue;
        };
        let target = portal.to_zone;

        // Destination zone + all 1-hop neighbors.
        let mut zones_to_send: Vec<ZoneId> = vec![target];
        for neighbor in topology.neighbors(target) {
            zones_to_send.push(neighbor);
        }

        for zid in zones_to_send {
            if !seen.insert(zid) {
                continue;
            }
            let Some(zone) = registry.get(zid) else { continue };
            let Some(tiles) = registry.tiles(zone) else { continue };
            writer.write(ZoneTileMessage {
                zone_id: zid,
                zone_kind: zone.kind,
                width: zone.width,
                height: zone.height,
                tiles: tiles.to_vec(),
                anchors: zone.anchors.clone(),
            });
        }
    }
}

/// Look up a portal anchor's world-space position from the zone registry.
///
/// For overworld-facing portals, the anchor pos is already in world-space.
/// For intra-zone portals (staircases, etc.) the anchor pos is zone-local tile
/// coordinates. Returns `Vec3::ZERO` when the anchor is not found.
fn portal_anchor_world_pos(
    zone_id: ZoneId,
    anchor_name: &str,
    registry: &ZoneRegistry,
) -> Vec3 {
    registry
        .get(zone_id)
        .and_then(|zone| {
            zone.anchors
                .iter()
                .find(|a| a.name == anchor_name)
                .map(|a| Vec3::new(a.pos.x, 0.0, a.pos.y))
        })
        .unwrap_or(Vec3::ZERO)
}

/// Broadcast portal/topology information for the current zone and all
/// zones within 2 hops for each `PlayerZoneTransition`.
///
/// The `from_world_pos` and `to_world_pos` fields in `ClientPortalEntry` are
/// now populated from `ZoneRegistry` anchor positions so the portal renderer
/// can place portal meshes at correct world-space locations.
fn send_zone_neighbors(
    mut events: MessageReader<PlayerZoneTransition>,
    topology: Option<Res<ZoneTopology>>,
    registry: Option<Res<ZoneRegistry>>,
    mut writer: MessageWriter<ZoneNeighborMessage>,
) {
    let Some(topology) = topology else { return };
    let Some(registry) = registry else { return };

    for ev in events.read() {
        let Some(transit_portal) = topology.portals.iter().find(|p| p.id == ev.portal_id) else {
            continue;
        };
        let current_zone = transit_portal.to_zone;

        // Collect all zones within 2 hops and their hop distances.
        let mut zone_hops: Vec<(ZoneId, u8)> = vec![(current_zone, 0)];
        let mut portals: Vec<ClientPortalEntry> = Vec::new();

        // Hop 0: portals in current zone.
        for portal in topology.exits_from(current_zone) {
            let from_world_pos = portal_anchor_world_pos(
                portal.from_zone,
                &portal.from_anchor,
                &registry,
            );
            let to_world_pos = portal_anchor_world_pos(
                portal.to_zone,
                &portal.to_anchor,
                &registry,
            );
            portals.push(ClientPortalEntry {
                portal: portal.clone(),
                from_hop: 0,
                from_world_pos,
                to_world_pos,
            });
        }

        // Hop 1: 1-hop neighbor zones.
        let hop1_zones: Vec<ZoneId> = topology.neighbors(current_zone).collect();
        for &zone1 in &hop1_zones {
            zone_hops.push((zone1, 1));
            for portal in topology.exits_from(zone1) {
                // Avoid duplicating portals already in the list.
                if !portals.iter().any(|e| e.portal.id == portal.id) {
                    let from_world_pos = portal_anchor_world_pos(
                        portal.from_zone,
                        &portal.from_anchor,
                        &registry,
                    );
                    let to_world_pos = portal_anchor_world_pos(
                        portal.to_zone,
                        &portal.to_anchor,
                        &registry,
                    );
                    portals.push(ClientPortalEntry {
                        portal: portal.clone(),
                        from_hop: 1,
                        from_world_pos,
                        to_world_pos,
                    });
                }
            }
        }

        // Hop 2: 2-hop neighbor zones.
        for zone1 in hop1_zones {
            for zone2 in topology.neighbors(zone1) {
                // Skip zones already tracked (current or hop-1).
                if zone_hops.iter().any(|(z, _)| *z == zone2) {
                    continue;
                }
                zone_hops.push((zone2, 2));
                for portal in topology.exits_from(zone2) {
                    if !portals.iter().any(|e| e.portal.id == portal.id) {
                        let from_world_pos = portal_anchor_world_pos(
                            portal.from_zone,
                            &portal.from_anchor,
                            &registry,
                        );
                        let to_world_pos = portal_anchor_world_pos(
                            portal.to_zone,
                            &portal.to_anchor,
                            &registry,
                        );
                        portals.push(ClientPortalEntry {
                            portal: portal.clone(),
                            from_hop: 2,
                            from_world_pos,
                            to_world_pos,
                        });
                    }
                }
            }
        }

        writer.write(ZoneNeighborMessage {
            current_zone,
            portals,
            zone_hops,
        });
    }
}
