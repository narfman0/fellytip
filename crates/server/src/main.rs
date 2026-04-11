mod plugins;

use bevy::prelude::*;
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use core::time::Duration;
use fellytip_shared::{
    NET_PORT, PRIVATE_KEY, PROTOCOL_ID, TICK_HZ,
    components::WorldPosition,
    protocol::FellytipProtocolPlugin,
};
use lightyear::prelude::{server::*, *};
use std::net::SocketAddr;

/// BRP HTTP port for the server (used by ralph scenarios and tooling).
const BRP_PORT: u16 = 15702;

fn main() {
    tracing_subscriber::fmt::init();
    App::new()
        .add_plugins(MinimalPlugins)
        .add_plugins(RemotePlugin::default())
        .add_plugins(RemoteHttpPlugin::default().with_port(BRP_PORT))
        .add_plugins(ServerPlugins {
            tick_duration: Duration::from_secs_f64(1.0 / TICK_HZ),
        })
        .add_plugins(FellytipProtocolPlugin)
        .add_plugins(plugins::persistence::PersistencePlugin)
        .add_plugins(plugins::world_sim::WorldSimPlugin)
        .add_plugins(plugins::ecology::EcologyPlugin)
        .add_plugins(plugins::ai::AiPlugin)
        .add_plugins(plugins::story::StoryPlugin)
        .add_plugins(plugins::combat::CombatPlugin)
        .add_systems(Startup, plugins::ai::seed_factions)
        .add_systems(Startup, spawn_server)
        .add_observer(on_link_spawned)
        .add_observer(on_client_connected)
        .run();
}

fn spawn_server(mut commands: Commands) {
    let addr: SocketAddr = format!("0.0.0.0:{NET_PORT}").parse().unwrap();
    let e = commands
        .spawn((
            ServerUdpIo::default(),
            LocalAddr(addr),
            NetcodeServer::new(
                NetcodeConfig::default()
                    .with_protocol_id(PROTOCOL_ID)
                    .with_key(PRIVATE_KEY),
            ),
        ))
        .id();
    commands.entity(e).trigger(|entity| Start { entity });
    tracing::info!("Server listening on {addr}");
}

/// Every new client link gets a `ReplicationSender` so the server can push
/// entity updates to it.
fn on_link_spawned(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .insert(ReplicationSender::new(
            Duration::from_millis(50),
            SendUpdatesMode::SinceLastAck,
            false,
        ));
    tracing::debug!("Link spawned, added ReplicationSender: {:?}", trigger.entity);
}

/// When the netcode handshake completes, spawn a player entity for the client.
fn on_client_connected(trigger: On<Add, Connected>, query: Query<(), With<ClientOf>>, mut commands: Commands) {
    if query.get(trigger.entity).is_err() {
        return;
    }
    tracing::info!("Client connected: {:?}", trigger.entity);

    commands.spawn((
        WorldPosition { x: 0.0, y: 0.0 },
        Replicate::to_clients(NetworkTarget::All),
    ));
}
