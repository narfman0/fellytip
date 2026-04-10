use bevy::prelude::*;
use fellytip_shared::{NET_PORT, PRIVATE_KEY, PROTOCOL_ID, TICK_HZ, protocol::FellytipProtocolPlugin};
use lightyear::prelude::{server::*, *};
use std::{net::SocketAddr, time::Duration};

fn main() {
    tracing_subscriber::fmt::init();
    App::new()
        .add_plugins(MinimalPlugins)
        .add_plugins(ServerPlugins {
            tick_duration: Duration::from_secs_f64(1.0 / TICK_HZ),
        })
        .add_plugins(FellytipProtocolPlugin)
        .add_systems(Startup, spawn_server)
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

fn on_client_connected(trigger: On<Add, Connected>, query: Query<(), With<ClientOf>>) {
    if query.get(trigger.entity).is_ok() {
        tracing::info!("Client connected: {:?}", trigger.entity);
    }
}
