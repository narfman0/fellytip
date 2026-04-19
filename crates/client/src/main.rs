mod plugins;

use bevy::prelude::*;
#[cfg(not(target_family = "wasm"))]
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use core::time::Duration;
use fellytip_shared::{
    PLAYER_SPEED, PRIVATE_KEY, PROTOCOL_ID, TICK_HZ,
    components::{Experience, WorldPosition},
    inputs::{ActionIntent, PlayerInput},
    protocol::{FellytipProtocolPlugin, GreetMsg, PlayerInputChannel},
    world::map::{is_walkable_at, smooth_surface_at, WorldMap, GRAVITY, LAND_SNAP, MAX_FALL_SPEED},
    world::story::GameEntityId,
};
use uuid::Uuid;
#[cfg(not(target_family = "wasm"))]
use fellytip_shared::NET_PORT;
use lightyear::prelude::{client::*, *};
use plugins::camera::OrbitCamera;
use std::net::SocketAddr;

/// System-set ordering for client `Update` systems.
///
/// Every frame must flow:  Input → SyncVisuals → SyncCamera
///
/// Without this guarantee, `send_player_input` can update `PredictedPosition`
/// *between* `sync_local_player_transform` and `update_orbit_camera`, leaving
/// the capsule transform and the camera target one frame out of phase and
/// producing visible jitter.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum ClientSet {
    /// Reads keyboard input and writes `PredictedPosition`.
    Input,
    /// Propagates `PredictedPosition` into Bevy `Transform` for rendered meshes.
    SyncVisuals,
    /// Updates the orbit-camera target from `PredictedPosition`.
    SyncCamera,
}

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
    /// Vertical velocity in world units/sec. Positive = up, negative = down.
    /// Accumulated each frame under gravity when the entity is airborne.
    pub z_vel: f32,
}

/// Stores the UUID of the local player entity, received from the server via
/// `GreetMsg`.  Used by `tag_local_player` to identify which replicated entity
/// belongs to this client (avoids tagging remote player entities as LocalPlayer).
#[derive(Resource, Default)]
struct LocalPlayerId(Option<Uuid>);

/// Seconds between client connection attempts.  Gives the server time to finish
/// startup before the first attempt and allows automatic reconnect on failure.
const CONNECT_RETRY_SECS: f32 = 2.0;

/// BRP HTTP port for the headless client (used by ralph scenarios).
#[cfg(not(target_family = "wasm"))]
const BRP_PORT_HEADLESS: u16 = 15703;

/// BRP HTTP port exposed by the server; used to probe for a running instance.
#[cfg(not(target_family = "wasm"))]
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
#[cfg(not(target_family = "wasm"))]
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

    // Forward all args except client-only flags to the server.
    let forward: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--headless" && a != "--auto-launch" && a != "--host")
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

fn add_windowed_plugins(app: &mut App) {
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
    .add_plugins(plugins::SkyboxPlugin)
    .add_plugins(plugins::TerrainPlugin)
    .add_plugins(plugins::SceneDecorationPlugin)
    .add_plugins(plugins::CharacterAnimationPlugin)
    .add_plugins(plugins::EntityRendererPlugin)
    .add_plugins(plugins::BattleVisualsPlugin)
    .add_plugins(plugins::HudPlugin);
}

fn main() {
    let mut app = App::new();

    #[cfg(not(target_family = "wasm"))]
    {
        let headless  = std::env::args().any(|a| a == "--headless");
        // --host means "I want to host and play": ensures the server binary is
        // auto-launched (identical to --auto-launch but semantically distinct).
        // The server process binds UDP :5000 for remote players to join.
        let auto_launch = cfg!(debug_assertions)
            || std::env::args().any(|a| a == "--auto-launch")
            || std::env::args().any(|a| a == "--host");
        let _server_child = if !headless && auto_launch {
            maybe_spawn_server()
        } else {
            None
        };

        if headless {
            // Headless: minimal plugins + BRP for ralph test scenarios.
            // Tracing is initialised manually since MinimalPlugins has no LogPlugin.
            tracing_subscriber::fmt::init();
            app.add_plugins(MinimalPlugins)
                .add_plugins(RemotePlugin::default())
                .add_plugins(RemoteHttpPlugin::default().with_port(BRP_PORT_HEADLESS))
                .add_systems(Update, (headless_auto_attack, headless_auto_move));
        } else {
            add_windowed_plugins(&mut app);
        }
    }
    #[cfg(target_family = "wasm")]
    add_windowed_plugins(&mut app);

    app.add_plugins(ClientPlugins {
            tick_duration: Duration::from_secs_f64(1.0 / TICK_HZ),
        })
        .add_plugins(FellytipProtocolPlugin)
        .init_resource::<LocalPlayerId>()
        .insert_resource(ConnectTimer(Timer::from_seconds(
            CONNECT_RETRY_SECS,
            TimerMode::Repeating,
        )))
        .configure_sets(
            Update,
            (ClientSet::Input, ClientSet::SyncVisuals, ClientSet::SyncCamera).chain(),
        )
        .add_systems(
            Update,
            (try_connect, log_replicated_positions, reconcile_prediction, receive_greet),
        )
        .add_systems(
            Update,
            // Chain ensures tag_local_player's deferred inserts (LocalPlayer,
            // PredictedPosition) are flushed by apply_deferred before
            // send_player_input runs, so the correct spawn position is sent on
            // the very first replication frame instead of pos=[0,0,0].
            (tag_local_player, ApplyDeferred, send_player_input.in_set(ClientSet::Input)).chain(),
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
    // Each session gets a random client_id so multiple instances do not conflict
    // on the server (netcode rejects duplicate IDs).
    let client_id: u64 = rand::random();

    #[cfg(not(target_family = "wasm"))]
    {
        let server_addr: SocketAddr = format!("127.0.0.1:{NET_PORT}").parse().unwrap();
        let local_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let e = commands
            .spawn((
                UdpIo::default(),
                LocalAddr(local_addr),
                NetcodeClient::new(
                    Authentication::Manual {
                        server_addr,
                        client_id,
                        private_key: PRIVATE_KEY,
                        protocol_id: PROTOCOL_ID,
                    },
                    NetcodeConfig::default(),
                )
                .expect("failed to build NetcodeClient"),
            ))
            .id();
        commands.entity(e).trigger(|entity| Connect { entity });
        tracing::info!("Connecting UDP to {server_addr} (client_id={client_id})");
    }
    #[cfg(target_family = "wasm")]
    {
        use fellytip_shared::WS_PORT;
        use lightyear::websocket::client::{WebSocketClientIo, WebSocketTarget};
        let server_addr: SocketAddr = format!("127.0.0.1:{WS_PORT}").parse().unwrap();
        let e = commands
            .spawn((
                WebSocketClientIo {
                    // ClientConfig on WASM is a private empty struct; use
                    // Default::default() — the compiler infers the type from
                    // the field definition.
                    config: Default::default(),
                    target: WebSocketTarget::Url(format!("ws://127.0.0.1:{WS_PORT}")),
                },
                NetcodeClient::new(
                    Authentication::Manual {
                        server_addr,
                        client_id,
                        private_key: PRIVATE_KEY,
                        protocol_id: PROTOCOL_ID,
                    },
                    NetcodeConfig::default(),
                )
                .expect("failed to build NetcodeClient"),
            ))
            .id();
        commands.entity(e).trigger(|entity| Connect { entity });
        tracing::info!("Connecting WebSocket to ws://127.0.0.1:{WS_PORT} (client_id={client_id})");
    }
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
/// Reads `PredictedPosition` so the server position stays at the correct spawn location
/// rather than being overwritten with [0,0,0].
#[cfg(not(target_family = "wasm"))]
fn headless_auto_attack(
    mut sender: Option<Single<&mut MessageSender<PlayerInput>>>,
    pred_q: Query<&PredictedPosition, With<LocalPlayer>>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
) {
    *elapsed += time.delta_secs();
    if *elapsed < 2.0 {
        return;
    }
    *elapsed = 0.0;
    let Some(ref mut s) = sender else { return };
    let pos = pred_q.single().map(|p| [p.x, p.y, p.z]).unwrap_or([0.0; 3]);
    s.send::<PlayerInputChannel>(PlayerInput {
        move_dir: [0.0, 0.0],
        pos,
        action: Some(ActionIntent::BasicAttack),
        target: None,
    });
    tracing::debug!("Headless: auto BasicAttack sent");
}

/// Headless-mode only: walks the player right for 3 s then left for 3 s, repeating.
/// Sends `PlayerInput` with movement every frame so ralph can observe position change.
/// No terrain checks — `WorldMap` is not loaded in headless mode (mirrors the fallback
/// branch in `send_player_input` when the map resource is absent).
#[cfg(not(target_family = "wasm"))]
fn headless_auto_move(
    mut sender: Option<Single<&mut MessageSender<PlayerInput>>>,
    mut pred_q: Query<&mut PredictedPosition, With<LocalPlayer>>,
    time: Res<Time>,
    mut phase_elapsed: Local<f32>,
    mut phase_right: Local<bool>,
) {
    let Some(ref mut s) = sender else { return };
    let Ok(mut pred) = pred_q.single_mut() else { return };

    *phase_elapsed += time.delta_secs();
    if *phase_elapsed >= 3.0 {
        *phase_elapsed = 0.0;
        *phase_right = !*phase_right;
    }

    let dir_x: f32 = if *phase_right { 1.0 } else { -1.0 };
    pred.x += dir_x * PLAYER_SPEED * time.delta_secs();

    s.send::<PlayerInputChannel>(PlayerInput {
        move_dir: [dir_x, 0.0],
        pos: [pred.x, pred.y, pred.z],
        action: None,
        target: None,
    });
}

/// Read keyboard/gamepad input, apply client-authoritative movement prediction
/// immediately, and send a `PlayerInput` message (with computed position) to
/// the server every frame.
///
/// Movement direction is rotated by the camera yaw so W always moves "into the
/// screen" regardless of camera orbit angle.
///
/// The client uses its local `WorldMap` (same deterministic generation as the
/// server) to enforce terrain walkability and Z elevation following locally,
/// so prediction is fully accurate.  The computed `pos` is sent to the server,
/// which accepts it as the authoritative position.
///
/// Uses `Option<Res<...>>` so headless mode (no window, no input plugin) skips
/// gracefully.
fn send_player_input(
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    mut sender: Option<Single<&mut MessageSender<PlayerInput>>>,
    camera_q: Query<&OrbitCamera>,
    mut pred_q: Query<&mut PredictedPosition, With<LocalPlayer>>,
    map: Option<Res<WorldMap>>,
    time: Res<Time>,
) {
    let Some(keyboard) = keyboard else { return };
    let Some(ref mut sender) = sender else { return };

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

    // Apply client-authoritative movement prediction with local terrain checks.
    // The local WorldMap is the same deterministic generation as the server's,
    // so walkability and Z elevation are accurate without a server round-trip.
    if let Ok(mut pred) = pred_q.single_mut() {
        let dt = time.delta_secs();
        let new_x = pred.x + world_dx * PLAYER_SPEED * dt;
        let new_y = pred.y + world_dy * PLAYER_SPEED * dt;

        if let Some(ref m) = map {
            // Wall-slide: try full diagonal, then axis-aligned fallbacks.
            let can_xy = is_walkable_at(m, new_x, new_y, pred.z);
            let can_x  = is_walkable_at(m, new_x, pred.y, pred.z);
            let can_y  = is_walkable_at(m, pred.x, new_y, pred.z);
            if      can_xy { pred.x = new_x; pred.y = new_y; }
            else if can_x  { pred.x = new_x; }
            else if can_y  { pred.y = new_y; }
            // else: fully blocked; position unchanged

            // Z physics — velocity-integrated gravity with terrain contact.
            if let Some(terrain_z) = smooth_surface_at(m, pred.x, pred.y, pred.z) {
                if pred.z <= terrain_z + LAND_SNAP {
                    // Grounded: snap to surface and kill vertical velocity.
                    pred.z = terrain_z;
                    pred.z_vel = 0.0;
                } else {
                    // Airborne above terrain: integrate gravity.
                    pred.z_vel = (pred.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
                    pred.z += pred.z_vel * dt;
                    // Clamp if we stepped through the surface in one frame.
                    if pred.z < terrain_z {
                        pred.z = terrain_z;
                        pred.z_vel = 0.0;
                    }
                }
            } else {
                // No walkable surface reachable (void / water / mountain edge).
                pred.z_vel = (pred.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
                pred.z += pred.z_vel * dt;
            }
        } else {
            // Map not yet loaded — apply movement without terrain checks.
            pred.x = new_x;
            pred.y = new_y;
        }
    }

    // Action: Space → BasicAttack, Q → UseAbility(1).  Only on just_pressed.
    let action = if keyboard.just_pressed(KeyCode::Space) {
        Some(ActionIntent::BasicAttack)
    } else if keyboard.just_pressed(KeyCode::KeyQ) {
        Some(ActionIntent::UseAbility(1))
    } else {
        None
    };

    // Only send once the local player is tagged; avoids sending pos=[0,0,0]
    // on the first replication frame (which would teleport the server player
    // to the map centre before PredictedPosition is initialised).
    let Ok(pred) = pred_q.single() else { return };
    let pos = [pred.x, pred.y, pred.z];
    sender.send::<PlayerInputChannel>(PlayerInput {
        move_dir: [world_dx, world_dy],
        pos,
        action,
        target: None,
    });
}

// ── Local-player identification ───────────────────────────────────────────────

/// Drains incoming `GreetMsg`s and stores the player UUID so `tag_local_player`
/// knows which replicated entity is ours.
fn receive_greet(
    mut receiver: Query<&mut MessageReceiver<GreetMsg>, With<Client>>,
    mut local_id: ResMut<LocalPlayerId>,
) {
    let Ok(mut recv) = receiver.single_mut() else { return };
    for msg in recv.receive() {
        tracing::info!("Received GreetMsg — local player UUID: {}", msg.player_id);
        local_id.0 = Some(msg.player_id);
    }
}

// ── Local-player tagging ──────────────────────────────────────────────────────

type UntaggedLocalPlayer = (With<Replicated>, With<Experience>, Without<LocalPlayer>);

/// Inserts `LocalPlayer` and `PredictedPosition` onto the replicated entity
/// whose `GameEntityId` matches the UUID received in `GreetMsg`.
///
/// Waits until `LocalPlayerId` is populated (i.e. the server greeting has
/// arrived) so that remote player entities are never mistakenly tagged.
fn tag_local_player(
    query: Query<(Entity, &WorldPosition, &GameEntityId), UntaggedLocalPlayer>,
    local_id: Res<LocalPlayerId>,
    mut commands: Commands,
) {
    let Some(my_id) = local_id.0 else { return };
    for (entity, pos, geid) in &query {
        if geid.0 != my_id { continue; }
        commands.entity(entity).insert((
            LocalPlayer,
            PredictedPosition { x: pos.x, y: pos.y, z: pos.z, z_vel: 0.0 },
        ));
        tracing::debug!("Tagged local player entity {entity:?}");
    }
}

// ── Server reconciliation ─────────────────────────────────────────────────────

type LocalPlayerChanged = (With<LocalPlayer>, Changed<WorldPosition>);

/// Reconciles `PredictedPosition` when the server sends a position correction.
///
/// Since the client is now authoritative for X/Y/Z (it predicts movement using
/// its local terrain map), the server's replicated `WorldPosition` should stay
/// very close to `PredictedPosition` under normal conditions.
///
/// The only case where the server diverges is a `PositionSanityTimer` override
/// (client position was non-walkable for > 10 s) or a server-side teleport.
/// In that case the gap will be > 100 units and we snap to the server value.
fn reconcile_prediction(
    mut query: Query<(&WorldPosition, &mut PredictedPosition), LocalPlayerChanged>,
) {
    for (server, mut pred) in &mut query {
        let dx = server.x - pred.x;
        let dy = server.y - pred.y;
        let dz = server.z - pred.z;
        // 100 units ≈ 10 seconds of full-speed movement.
        // Only reachable via a genuine server enforcement event.
        if dx * dx + dy * dy + dz * dz > 10_000.0 {
            pred.x = server.x;
            pred.y = server.y;
            pred.z = server.z;
            pred.z_vel = 0.0;
        }
        // else: prediction is authoritative — leave it alone.
    }
}
