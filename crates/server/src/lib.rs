pub mod plugins;

use bevy::prelude::*;
use fellytip_shared::{
    WORLD_SEED,
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{Experience, Health, PlayerStandings, WorldMeta, WorldPosition},
    world::{
        map::{find_surface_spawn, WorldMap, MAP_HEIGHT, MAP_WIDTH},
        story::GameEntityId,
    },
};
use uuid::Uuid;

use plugins::combat::{CombatParticipant, LastPlayerInput, PositionSanityTimer};
pub use plugins::map_gen::MapGenConfig;

/// Parse `--flag value` from the arg list, returning `default` if not found.
pub fn parse_arg<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    args.windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(default)
}

/// Bundles all server-side game logic plugins.
///
/// Does NOT add networking (ServerPlugins/ClientPlugins/FellytipProtocolPlugin)
/// — callers add those separately for multiplayer builds.
///
/// When `combat_test` is true, skips map gen, ecology, AI, and dungeon plugins
/// and adds `CombatTestPlugin` instead for a minimal two-entity combat world.
///
/// MULTIPLAYER: restore ServerPlugins, spawn_server, on_client_connected,
/// on_client_disconnected, on_link_spawned, send_greet_msg, and idle_shutdown.
pub struct ServerGamePlugin {
    pub seed: u64,
    pub width: usize,
    pub height: usize,
    pub history_warp_ticks: u64,
    pub npcs_per_faction: usize,
    pub combat_test: bool,
}

impl Plugin for ServerGamePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(MapGenConfig {
                seed: self.seed,
                width: self.width,
                height: self.height,
                history_warp_ticks: self.history_warp_ticks,
                npcs_per_faction: self.npcs_per_faction,
            })
            .add_plugins(plugins::persistence::PersistencePlugin)
            .add_plugins(plugins::world_sim::WorldSimPlugin)
            .add_plugins(plugins::story::StoryPlugin)
            .add_plugins(plugins::combat::CombatPlugin)
            .add_plugins(plugins::interest::InterestPlugin)
            .add_plugins(plugins::party::PartyPlugin);

        if self.combat_test {
            app.add_plugins(plugins::combat_test::CombatTestPlugin);
        } else {
            app.add_plugins(plugins::map_gen::MapGenPlugin)
                .add_plugins(plugins::ecology::EcologyPlugin)
                .add_plugins(plugins::ai::AiPlugin)
                .add_plugins(plugins::dungeon::DungeonPlugin)
                .add_systems(Startup, plugins::ai::seed_factions)
                .add_systems(PostStartup, spawn_local_player);
        }
    }
}

/// Spawn the local player entity after map generation completes.
///
/// Uses precomputed spawn points from the WorldMap (set in PostStartup, after
/// MapGenPlugin's Startup chain finishes). Falls back to `find_surface_spawn`
/// or the origin if no spawn points exist.
///
/// MULTIPLAYER: replace with on_client_connected observer that spawns one
/// entity per connecting client and sends GreetMsg with the player UUID.
fn spawn_local_player(
    map: Option<Res<WorldMap>>,
    map_config: Option<Res<MapGenConfig>>,
    mut commands: Commands,
) {
    let (spawn_x, spawn_y, spawn_z) = map
        .as_deref()
        .and_then(|m| {
            if m.spawn_points.is_empty() { return None; }
            Some(m.spawn_points[0])
        })
        .or_else(|| map.as_deref().map(find_surface_spawn))
        .unwrap_or((0.0, 0.0, 0.0));

    let world_meta = map_config.as_deref().map(|cfg| WorldMeta {
        seed:   cfg.seed,
        width:  cfg.width  as u32,
        height: cfg.height as u32,
    }).unwrap_or(WorldMeta {
        seed:   WORLD_SEED,
        width:  MAP_WIDTH  as u32,
        height: MAP_HEIGHT as u32,
    });

    let player_uuid = Uuid::new_v4();
    commands.spawn((
        WorldPosition { x: spawn_x, y: spawn_y, z: spawn_z },
        Health { current: 100, max: 100 },
        CombatParticipant {
            id: CombatantId(player_uuid),
            interrupt_stack: InterruptStack::default(),
            class: CharacterClass::Warrior,
            level: 1,
            armor_class: 13,
            strength: 12,
            dexterity: 14,
            constitution: 12,
        },
        GameEntityId(player_uuid),
        Experience::new(),
        PlayerStandings::default(),
        LastPlayerInput::default(),
        PositionSanityTimer {
            last_valid_x: spawn_x,
            last_valid_y: spawn_y,
            last_valid_z: spawn_z,
            ..default()
        },
        world_meta,
    ));
    tracing::info!(uuid = %player_uuid, x = spawn_x, y = spawn_y, z = spawn_z, "Local player spawned");
}
