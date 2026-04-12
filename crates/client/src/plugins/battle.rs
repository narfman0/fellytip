//! Battle visualization plugin.
//!
//! Subscribes to `BattleStartMsg`, `BattleEndMsg`, and `BattleAttackMsg`
//! from the server and renders:
//!   - A pulsing translucent torus ring at each active battle site.
//!   - A "Battle Log" resource consumed by `hud.rs` to display recent events.

use bevy::prelude::*;
use lightyear::prelude::{client::Client, MessageReceiver};
use fellytip_shared::{
    protocol::{BattleAttackMsg, BattleEndMsg, BattleStartMsg},
    world::population::BATTLE_RADIUS,
};
use uuid::Uuid;

// ── Resources ─────────────────────────────────────────────────────────────────

/// Rolling battle event log shown in the HUD.
#[derive(Resource, Default)]
pub struct BattleLog {
    pub entries: Vec<String>,
}

impl BattleLog {
    const MAX_ENTRIES: usize = 50;

    pub fn push(&mut self, entry: String) {
        if self.entries.len() >= Self::MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }
}

// ── Components ────────────────────────────────────────────────────────────────

/// Marks the pulsing ring entity rendered at an active battle site.
#[derive(Component)]
pub struct BattleSiteMarker {
    pub settlement_id: Uuid,
    pub pulse_phase: f32,
}

// ── Battle ring visual assets ─────────────────────────────────────────────────

#[derive(Resource)]
struct BattleAssets {
    ring_mesh: Handle<Mesh>,
    ring_mat:  Handle<StandardMaterial>,
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct BattleVisualsPlugin;

impl Plugin for BattleVisualsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BattleLog>()
            .add_systems(Startup, setup_battle_assets)
            .add_systems(
                Update,
                (
                    on_battle_start,
                    on_battle_end,
                    on_battle_attack,
                    animate_battle_rings,
                ),
            );
    }
}

fn setup_battle_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Torus ring: major radius ≈ BATTLE_RADIUS, minor radius 0.15 (thin tube).
    let ring_mesh = meshes.add(Torus::new(BATTLE_RADIUS, 0.15));
    let ring_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.15, 0.05, 0.4),
        emissive: LinearRgba::new(1.0, 0.1, 0.0, 0.0),
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    commands.insert_resource(BattleAssets { ring_mesh, ring_mat });
}

// ── Systems ───────────────────────────────────────────────────────────────────

fn on_battle_start(
    mut receiver: Query<&mut MessageReceiver<BattleStartMsg>, With<Client>>,
    assets: Option<Res<BattleAssets>>,
    mut commands: Commands,
    mut log: ResMut<BattleLog>,
) {
    let Some(assets) = assets else { return };
    let Ok(mut recv) = receiver.single_mut() else { return };
    for msg in recv.receive() {
        // Spawn the ring at the battle site. Coordinate mapping: (world_x, z_elev, world_y).
        let translation = Vec3::new(msg.x, msg.z, msg.y);
        commands.spawn((
            BattleSiteMarker {
                settlement_id: msg.settlement_id,
                pulse_phase: 0.0,
            },
            Transform::from_translation(translation),
            Mesh3d(assets.ring_mesh.clone()),
            MeshMaterial3d(assets.ring_mat.clone()),
        ));
        let entry = format!(
            "⚔ {} attacks {}!",
            msg.attacker_faction, msg.defender_faction
        );
        log.push(entry);
        tracing::debug!(
            attacker = %msg.attacker_faction,
            defender = %msg.defender_faction,
            "Battle started (client)"
        );
    }
}

fn on_battle_end(
    mut receiver: Query<&mut MessageReceiver<BattleEndMsg>, With<Client>>,
    markers: Query<(Entity, &BattleSiteMarker)>,
    mut commands: Commands,
    mut log: ResMut<BattleLog>,
) {
    let Ok(mut recv) = receiver.single_mut() else { return };
    for msg in recv.receive() {
        // Despawn the matching ring.
        for (entity, marker) in &markers {
            if marker.settlement_id == msg.settlement_id {
                commands.entity(entity).despawn();
            }
        }
        let entry = format!(
            "🏁 {} wins! ({} vs {} casualties)",
            msg.winner_faction, msg.attacker_casualties, msg.defender_casualties
        );
        log.push(entry);
    }
}

/// Log attack events for visibility; no per-entity flash (client lacks CombatantId).
fn on_battle_attack(
    mut receiver: Query<&mut MessageReceiver<BattleAttackMsg>, With<Client>>,
) {
    let Ok(mut recv) = receiver.single_mut() else { return };
    for _msg in recv.receive() {
        // Consumed to prevent buffer buildup; detailed flash deferred to future milestone.
    }
}

/// Pulse the ring opacity using a sine wave.
fn animate_battle_rings(
    time: Res<Time>,
    mut rings: Query<(&mut BattleSiteMarker, &MeshMaterial3d<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (mut marker, mat_handle) in &mut rings {
        marker.pulse_phase += time.delta_secs() * 2.0;
        if let Some(mat) = materials.get_mut(&mat_handle.0) {
            let alpha = 0.25 + 0.25 * marker.pulse_phase.sin();
            mat.base_color = Color::srgba(1.0, 0.15, 0.05, alpha);
        }
    }
}
