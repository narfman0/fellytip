//! Battle visualization plugin.
//!
//! Subscribes to `BattleStartMsg`, `BattleEndMsg`, and `BattleAttackMsg`
//! Bevy events emitted by the server-side AI plugin, and renders:
//!   - A pulsing translucent torus ring at each active battle site.
//!   - A "Battle Log" resource consumed by `hud.rs` to display recent events.
//!
//! MULTIPLAYER: restore MessageReceiver queries on Client entities for the
//! three battle message types and StoryMsg.

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use fellytip_shared::{
    protocol::{BattleAttackMsg, BattleEndMsg, BattleStartMsg, StoryMsg},
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

/// Rolling world-story event log shown in the HUD story panel.
#[derive(Resource, Default)]
pub struct ClientStoryLog {
    pub entries: Vec<String>,
}

impl ClientStoryLog {
    const MAX_ENTRIES: usize = 20;

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
            .init_resource::<ClientStoryLog>()
            .add_systems(Startup, setup_battle_assets)
            .add_systems(
                Update,
                (
                    on_battle_start,
                    on_battle_end,
                    on_battle_attack,
                    on_story_msg,
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
    mut events: MessageReader<BattleStartMsg>,
    assets: Option<Res<BattleAssets>>,
    mut commands: Commands,
    mut log: ResMut<BattleLog>,
) {
    let Some(assets) = assets else { return };
    for msg in events.read() {
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
        log.push(format!("⚔ {} attacks {}!", msg.attacker_faction, msg.defender_faction));
        tracing::debug!(
            attacker = %msg.attacker_faction,
            defender = %msg.defender_faction,
            "Battle started (client)"
        );
    }
}

fn on_battle_end(
    mut events: MessageReader<BattleEndMsg>,
    markers: Query<(Entity, &BattleSiteMarker)>,
    mut commands: Commands,
    mut log: ResMut<BattleLog>,
) {
    for msg in events.read() {
        for (entity, marker) in &markers {
            if marker.settlement_id == msg.settlement_id {
                commands.entity(entity).despawn();
            }
        }
        log.push(format!(
            "🏁 {} wins! ({} vs {} casualties)",
            msg.winner_faction, msg.attacker_casualties, msg.defender_casualties
        ));
    }
}

fn on_battle_attack(mut events: MessageReader<BattleAttackMsg>) {
    // Consumed to prevent buffer buildup; per-entity flash deferred to future milestone.
    for _ in events.read() {}
}

fn on_story_msg(
    mut events: MessageReader<StoryMsg>,
    mut log: ResMut<ClientStoryLog>,
) {
    for msg in events.read() {
        tracing::debug!(text = %msg.text, "Story event received");
        log.push(msg.text.clone());
    }
}

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
