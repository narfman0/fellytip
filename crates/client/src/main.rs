use bevy::prelude::*;
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use core::time::Duration;
use fellytip_shared::{
    NET_PORT, PRIVATE_KEY, PROTOCOL_ID, TICK_HZ,
    components::WorldPosition,
    protocol::FellytipProtocolPlugin,
};
use lightyear::prelude::{client::*, *};
use std::net::SocketAddr;

/// BRP HTTP port for the headless client (used by ralph scenarios).
const BRP_PORT_HEADLESS: u16 = 15703;

fn main() {
    tracing_subscriber::fmt::init();
    let headless = std::env::args().any(|a| a == "--headless");
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(ClientPlugins {
            tick_duration: Duration::from_secs_f64(1.0 / TICK_HZ),
        })
        .add_plugins(FellytipProtocolPlugin)
        .add_systems(Startup, spawn_client)
        .add_systems(Update, log_replicated_positions)
        .add_observer(on_connected)
        .add_observer(on_disconnected);
    if headless {
        app.add_plugins(RemotePlugin::default())
            .add_plugins(RemoteHttpPlugin::default().with_port(BRP_PORT_HEADLESS));
    } else {
        // Rendering plugins added later (milestone 0).
    }
    app.run();
}

fn spawn_client(mut commands: Commands) {
    let server_addr: SocketAddr = format!("127.0.0.1:{NET_PORT}").parse().unwrap();
    let local_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
    let e = commands
        .spawn((
            UdpIo::default(),
            LocalAddr(local_addr),
            NetcodeClient::new(
                Authentication::Manual {
                    server_addr,
                    client_id: 1,
                    private_key: PRIVATE_KEY,
                    protocol_id: PROTOCOL_ID,
                },
                NetcodeConfig::default(),
            )
            .expect("failed to build NetcodeClient"),
        ))
        .id();
    commands.entity(e).trigger(|entity| Connect { entity });
    tracing::info!("Connecting to {server_addr}");
}

/// When the connection is established, attach a `ReplicationReceiver` so that
/// replicated entities are applied to this world.
fn on_connected(trigger: On<Add, Connected>, mut commands: Commands) {
    tracing::info!("Connected to server (entity {:?})", trigger.entity);
    commands
        .entity(trigger.entity)
        .insert(ReplicationReceiver::default());
}

fn on_disconnected(trigger: On<Add, Disconnected>, q: Query<&Disconnected>) {
    let reason = q
        .get(trigger.entity)
        .ok()
        .and_then(|d| d.reason.as_deref())
        .unwrap_or("none");
    tracing::info!("Disconnected (entity {:?}): {reason}", trigger.entity);
}

/// Log all entities that have a replicated `WorldPosition`.
fn log_replicated_positions(query: Query<(Entity, &WorldPosition), With<Replicated>>) {
    for (entity, pos) in &query {
        tracing::debug!("Replicated WorldPosition {entity:?}: ({}, {})", pos.x, pos.y);
    }
}
