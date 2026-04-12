mod plugins;

use bevy::prelude::*;
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use core::time::Duration;
use fellytip_shared::{
    NET_PORT, PLAYER_SPEED, PRIVATE_KEY, PROTOCOL_ID, TICK_HZ,
    components::{Experience, WorldPosition},
    inputs::{ActionIntent, PlayerInput},
    protocol::{FellytipProtocolPlugin, PlayerInputChannel},
};
use lightyear::prelude::{client::*, *};
use plugins::camera::OrbitCamera;
use std::net::SocketAddr;

/// Marker component inserted on the single entity that belongs to this client.
///
/// Used to route client-side prediction: the local player's visual transform
/// tracks `PredictedPosition`; remote entities track server `WorldPosition`.
#[derive(Component)]
pub struct LocalPlayer;

/// Client-only predicted world position updated immediately on input, before
/// the server round-trip.  Reconciled toward the authoritative `WorldPosition`
/// whenever the server sends an update.
#[derive(Component, Default, Clone)]
pub struct PredictedPosition {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Seconds between client connection attempts.  Gives the server time to finish
/// startup before the first attempt and allows automatic reconnect on failure.
const CONNECT_RETRY_SECS: f32 = 2.0;

/// BRP HTTP port for the headless client (used by ralph scenarios).
const BRP_PORT_HEADLESS: u16 = 15703;

/// BRP HTTP port exposed by the server; used to probe for a running instance.
const SERVER_BRP_PORT: u16 = 15702;

/// If no server is reachable on `SERVER_BRP_PORT`, locate `fellytip-server`
/// next to this binary and spawn it as a child process.  Blocks (up to 10 s)
/// until the server's BRP port becomes reachable, then returns.
///
/// Returns the spawned `Child` handle, or `None` when:
/// - a server is already reachable (don't launch a duplicate), or
/// - the server binary is not found next to the client (remote-server scenario).
///
/// Dropping the returned `Child` does **not** kill the server process — the
/// server manages its own lifetime via its idle-shutdown timer.
fn maybe_spawn_server() -> Option<std::process::Child> {
    let probe_addr: std::net::SocketAddr =
        format!("127.0.0.1:{SERVER_BRP_PORT}").parse().unwrap();

    // Already running?
    if std::net::TcpStream::connect_timeout(
        &probe_addr,
        std::time::Duration::from_millis(200),
    )
    .is_ok()
    {
        return None; // Existing server detected — don't spawn a duplicate.
    }

    // Locate the server binary alongside this binary.
    let server_bin = {
        let mut path = std::env::current_exe().ok()?.parent()?.to_path_buf();
        path.push(format!("fellytip-server{}", std::env::consts::EXE_SUFFIX));
        if !path.exists() {
            return None; // Not found — user may be connecting to a remote server.
        }
        path
    };

    // Forward all args except the binary name and --headless to the server.
    let forward: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--headless")
        .collect();

    let child = std::process::Command::new(&server_bin)
        .args(&forward)
        .spawn()
        .ok()?;

    // Poll until the server's BRP port is reachable (up to 10 s).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if std::net::TcpStream::connect_timeout(
            &probe_addr,
            std::time::Duration::from_millis(200),
        )
        .is_ok()
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            break; // Proceed anyway; the connect retry loop will handle it.
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    Some(child)
}

fn main() {
    let headless = std::env::args().any(|a| a == "--headless");

    // In windowed mode, ensure a local server is running before the Bevy app
    // starts.  We hold the Child handle for the duration of main() so the handle
    // isn't released prematurely (dropping it does not kill the server process).
    let _server_child = if !headless { maybe_spawn_server() } else { None };

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
        .add_plugins(plugins::TerrainPlugin)
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
        .add_systems(
            Update,
            (
                try_connect,
                log_replicated_positions,
                tag_local_player,
                reconcile_prediction,
                send_player_input,
            ),
        )
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

/// Read keyboard/gamepad input, apply client-side prediction immediately, and
/// send a `PlayerInput` message to the server every frame.
///
/// Movement direction is rotated by the camera yaw so W always moves "into the
/// screen" from the player's point of view regardless of camera orbit angle.
///
/// The direction is sent **every frame** including the zero-vector when no keys
/// are held so the server always receives an explicit stop signal even under
/// packet loss.
///
/// Uses `Option<Res<...>>` so headless mode (no window, no input plugin) skips
/// gracefully.
fn send_player_input(
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    mut sender: Single<&mut MessageSender<PlayerInput>>,
    camera_q: Query<&OrbitCamera>,
    mut pred_q: Query<&mut PredictedPosition, With<LocalPlayer>>,
    time: Res<Time>,
) {
    let Some(keyboard) = keyboard else { return };

    // Raw WASD input on screen axes (before camera rotation).
    let mut raw_x = 0.0_f32; // A/D strafe
    let mut raw_y = 0.0_f32; // W/S forward/back
    if keyboard.pressed(KeyCode::KeyW) || keyboard.pressed(KeyCode::ArrowUp) {
        raw_y += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) || keyboard.pressed(KeyCode::ArrowDown) {
        raw_y -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) || keyboard.pressed(KeyCode::ArrowLeft) {
        raw_x -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) || keyboard.pressed(KeyCode::ArrowRight) {
        raw_x += 1.0;
    }

    // Normalise diagonal movement before rotating.
    let len = (raw_x * raw_x + raw_y * raw_y).sqrt();
    if len > 0.0 {
        raw_x /= len;
        raw_y /= len;
    }

    // Rotate raw input by camera yaw so W moves toward the camera's forward
    // direction in world space.
    //
    // Camera offset: (cos_pitch*sin_yaw, sin_pitch, cos_pitch*cos_yaw) in Bevy
    // coords.  Projecting onto the horizontal world plane gives:
    //   forward = (-sin_yaw, -cos_yaw)  [world x, world y]
    //   right   = ( cos_yaw, -sin_yaw)
    //
    // Combined: world_x = cos(yaw)*raw_x - sin(yaw)*raw_y
    //           world_y = -sin(yaw)*raw_x - cos(yaw)*raw_y
    let yaw = camera_q.iter().next().map(|c| c.yaw).unwrap_or(0.0);
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let world_dx =  cos_yaw * raw_x - sin_yaw * raw_y;
    let world_dy = -sin_yaw * raw_x - cos_yaw * raw_y;

    // Apply client-side prediction: move the local visual immediately so the
    // player feels instant response while waiting for server confirmation.
    if let Ok(mut pred) = pred_q.single_mut() {
        let dt = time.delta_secs();
        pred.x += world_dx * PLAYER_SPEED * dt;
        pred.y += world_dy * PLAYER_SPEED * dt;
    }

    // Action: Space → BasicAttack, Q → UseAbility(1).  Only on just_pressed.
    let action = if keyboard.just_pressed(KeyCode::Space) {
        Some(ActionIntent::BasicAttack)
    } else if keyboard.just_pressed(KeyCode::KeyQ) {
        Some(ActionIntent::UseAbility(1))
    } else {
        None
    };

    // Always send, including zero-vector, so the server receives an explicit
    // stop when all keys are released.  Skip only if there is no action and
    // the direction is zero (avoids spamming the server when standing still).
    if world_dx != 0.0 || world_dy != 0.0 || action.is_some() {
        sender.send::<PlayerInputChannel>(PlayerInput {
            move_dir: [world_dx, world_dy],
            action,
            target: None,
        });
    } else {
        // Send the explicit stop once so the server can clear LastPlayerInput.
        // `sender.send` is fire-and-forget on an unreliable channel, so sending
        // it every frame when idle is harmless but unnecessary; we send on every
        // Update frame when movement is non-zero and send this zero-vector when
        // not moving.
        sender.send::<PlayerInputChannel>(PlayerInput {
            move_dir: [0.0, 0.0],
            action: None,
            target: None,
        });
    }
}

// ── Local-player tagging ──────────────────────────────────────────────────────

type UntaggedLocalPlayer = (With<Replicated>, With<Experience>, Without<LocalPlayer>);

/// Inserts `LocalPlayer` and `PredictedPosition` onto the first replicated
/// entity that has an `Experience` component (i.e. our own player).
///
/// Runs once: the `Without<LocalPlayer>` filter prevents re-insertion.
fn tag_local_player(
    query: Query<(Entity, &WorldPosition), UntaggedLocalPlayer>,
    mut commands: Commands,
) {
    for (entity, pos) in &query {
        commands.entity(entity).insert((
            LocalPlayer,
            PredictedPosition { x: pos.x, y: pos.y, z: pos.z },
        ));
        tracing::debug!("Tagged local player entity {entity:?}");
    }
}

// ── Server reconciliation ─────────────────────────────────────────────────────

type LocalPlayerChanged = (With<LocalPlayer>, Changed<WorldPosition>);

/// Reconciles `PredictedPosition` toward the authoritative `WorldPosition`
/// whenever the server pushes a position update for the local player.
///
/// Strategy: trust the prediction completely during normal movement.
/// XY is only overwritten when the server disagrees by > 15 units — this only
/// happens for genuine server-side corrections (teleports, anti-cheat, severe
/// collision rejection).  Small drift (< 15 units) is left alone; the client
/// and server stay within ~1 unit of each other under normal latency so this
/// threshold is never triggered during regular play.
///
/// Z is always taken from the server because the client has no terrain map and
/// cannot predict elevation changes on its own.
fn reconcile_prediction(
    mut query: Query<(&WorldPosition, &mut PredictedPosition), LocalPlayerChanged>,
) {
    for (server, mut pred) in &mut query {
        // Always accept server elevation; client cannot predict terrain height.
        pred.z = server.z;

        let dx = server.x - pred.x;
        let dy = server.y - pred.y;
        // 15 units ≈ 1.5 seconds of full-speed movement — only reachable via a
        // true server correction (teleport, wall push-back, anti-cheat).
        // Never triggered by normal network lag or dt accumulation.
        if dx * dx + dy * dy > 225.0 {
            pred.x = server.x;
            pred.y = server.y;
        }
        // else: prediction is close enough — leave it alone.
    }
}
