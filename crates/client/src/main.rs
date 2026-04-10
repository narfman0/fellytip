use bevy::prelude::*;
use fellytip_shared::{NET_PORT, PRIVATE_KEY, PROTOCOL_ID, TICK_HZ, protocol::FellytipProtocolPlugin};
use lightyear::prelude::{client::*, *};
use std::{net::SocketAddr, time::Duration};

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
        .add_observer(on_connected)
        .add_observer(on_disconnected);
    if !headless {
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

fn on_connected(trigger: On<Add, Connected>) {
    tracing::info!("Connected to server (entity {:?})", trigger.entity);
}

fn on_disconnected(trigger: On<Add, Disconnected>, q: Query<&Disconnected>) {
    let reason = q
        .get(trigger.entity)
        .ok()
        .and_then(|d| d.reason.as_deref())
        .unwrap_or("none");
    tracing::info!("Disconnected (entity {:?}): {reason}", trigger.entity);
}
