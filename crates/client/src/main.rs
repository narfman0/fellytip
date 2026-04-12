mod plugins;

use bevy::prelude::*;
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use core::time::Duration;
use fellytip_shared::{
    NET_PORT, PRIVATE_KEY, PROTOCOL_ID, TICK_HZ,
    components::WorldPosition,
    inputs::{ActionIntent, PlayerInput},
    protocol::{FellytipProtocolPlugin, PlayerInputChannel},
};
use lightyear::prelude::{client::*, *};
use std::net::SocketAddr;

/// Seconds between client connection attempts.  Gives the server time to finish
/// startup before the first attempt and allows automatic reconnect on failure.
const CONNECT_RETRY_SECS: f32 = 2.0;

/// BRP HTTP port for the headless client (used by ralph scenarios).
const BRP_PORT_HEADLESS: u16 = 15703;

fn main() {
    let headless = std::env::args().any(|a| a == "--headless");
    let mut app = App::new();

    if headless {
        // Headless: minimal plugins + BRP for ralph test scenarios.
        // Tracing is initialised manually since MinimalPlugins has no LogPlugin.
        tracing_subscriber::fmt::init();
        app.add_plugins(MinimalPlugins)
            .add_plugins(RemotePlugin::default())
            .add_plugins(RemoteHttpPlugin::default().with_port(BRP_PORT_HEADLESS))
            .add_systems(Update, headless_auto_attack);
    } else {
        // Windowed: full render stack.  DefaultPlugins includes LogPlugin so we
        // do NOT call tracing_subscriber::fmt::init() to avoid a double-init.
        app.add_plugins(
            DefaultPlugins.build().set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Fellytip".into(),
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins(plugins::SceneLightingPlugin)
        .add_plugins(plugins::OrbitCameraPlugin)
        .add_plugins(plugins::TileRendererPlugin)
        .add_plugins(plugins::EntityRendererPlugin)
        .add_plugins(plugins::HudPlugin);
    }

    app.add_plugins(ClientPlugins {
            tick_duration: Duration::from_secs_f64(1.0 / TICK_HZ),
        })
        .add_plugins(FellytipProtocolPlugin)
        .insert_resource(ConnectTimer(Timer::from_seconds(
            CONNECT_RETRY_SECS,
            TimerMode::Repeating,
        )))
        .add_systems(Update, (try_connect, log_replicated_positions, send_player_input))
        .add_observer(on_connected)
        .add_observer(on_disconnected);

    app.run();
}

/// Drives periodic connection attempts so the client can start before the
/// server is ready and reconnects automatically after a dropped connection.
#[derive(Resource)]
struct ConnectTimer(Timer);

/// Attempt to connect (or reconnect) on each timer tick.
///
/// States:
/// - No `NetcodeClient` entity   → spawn one and start the handshake.
/// - Entity present, `Connected` → already live, do nothing.
/// - Entity present, `Disconnected` → clean it up then let the next tick retry.
/// - Entity present, neither      → handshake in flight, wait.
fn try_connect(
    time: Res<Time>,
    mut timer: ResMut<ConnectTimer>,
    clients: Query<(Entity, Has<Connected>, Has<Disconnected>), With<NetcodeClient>>,
    mut commands: Commands,
) {
    if !timer.0.tick(time.delta()).just_finished() {
        return;
    }

    let mut has_live = false;
    let mut has_pending = false;
    for (entity, connected, disconnected) in &clients {
        if connected {
            has_live = true;
        } else if disconnected {
            // Clean up the stale entity so we can spawn a fresh one next tick.
            commands.entity(entity).despawn();
        } else {
            // Handshake still in flight — wait for it to resolve.
            has_pending = true;
        }
    }
    if has_live || has_pending {
        return;
    }

    // No live or in-flight client: attempt a fresh connection.
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

/// Headless-mode only: sends `BasicAttack` every 2 seconds for ralph integration tests.
/// Allows `combat_resolves` to assert that damage lands without a real keyboard.
fn headless_auto_attack(
    mut sender: Option<Single<&mut MessageSender<PlayerInput>>>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
) {
    *elapsed += time.delta_secs();
    if *elapsed < 2.0 {
        return;
    }
    *elapsed = 0.0;
    let Some(ref mut s) = sender else { return };
    s.send::<PlayerInputChannel>(PlayerInput {
        move_dir: [0.0, 0.0],
        action: Some(ActionIntent::BasicAttack),
        target: None,
    });
    tracing::debug!("Headless: auto BasicAttack sent");
}

/// Read keyboard/gamepad input and send a `PlayerInput` message to the server.
/// Runs every Update frame; only sends when there is actual input.
/// Uses `Option<Res<...>>` so headless mode (no window, no input plugin) skips gracefully.
fn send_player_input(
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    mut sender: Single<&mut MessageSender<PlayerInput>>,
) {
    let Some(keyboard) = keyboard else { return };

    // Movement: WASD or arrow keys
    let mut dx = 0.0_f32;
    let mut dy = 0.0_f32;
    if keyboard.pressed(KeyCode::KeyW) || keyboard.pressed(KeyCode::ArrowUp) {
        dy += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) || keyboard.pressed(KeyCode::ArrowDown) {
        dy -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) || keyboard.pressed(KeyCode::ArrowLeft) {
        dx -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) || keyboard.pressed(KeyCode::ArrowRight) {
        dx += 1.0;
    }

    // Normalise diagonal movement
    let len = (dx * dx + dy * dy).sqrt();
    if len > 0.0 {
        dx /= len;
        dy /= len;
    }

    // Action: Space → BasicAttack, Q → StrongAttack (ability 1)
    let action = if keyboard.just_pressed(KeyCode::Space) {
        Some(ActionIntent::BasicAttack)
    } else if keyboard.just_pressed(KeyCode::KeyQ) {
        Some(ActionIntent::UseAbility(1))
    } else {
        None
    };

    // Only send when there is meaningful input
    if dx != 0.0 || dy != 0.0 || action.is_some() {
        sender.send::<PlayerInputChannel>(PlayerInput {
            move_dir: [dx, dy],
            action,
            target: None,
        });
    }
}
