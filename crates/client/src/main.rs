mod plugins;

use bevy::prelude::*;
#[cfg(not(target_family = "wasm"))]
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use fellytip_server::{plugins::combat::LocalPlayerInput, ServerGamePlugin};
use fellytip_shared::{
    PLAYER_SPEED, WORLD_SEED,
    components::{Experience, WorldPosition},
    inputs::ActionIntent,
    protocol::FellytipProtocolPlugin,
    world::map::{is_walkable_at, is_water_at, water_surface_at, smooth_surface_at, terrain_normal_at, WorldMap, GRAVITY, JUMP_SPEED, DASH_SPEED, DASH_DURATION, LAND_SNAP, MAX_FALL_SPEED, STEP_HEIGHT, SWIM_BUOYANCY, SWIM_RISE_SPEED, MAP_WIDTH, MAP_HEIGHT},
};
use plugins::camera::OrbitCamera;

/// System-set ordering for client `Update` systems.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum ClientSet {
    Input,
    SyncVisuals,
    SyncCamera,
}

/// Marker component on the single local player entity.
#[derive(Component)]
pub struct LocalPlayer;

/// Client-only predicted world position updated immediately on input for
/// zero-latency visual response. Synced to `WorldPosition` each frame.
#[derive(Component, Default, Clone)]
pub struct PredictedPosition {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub z_vel: f32,
    /// True while the character is on a walkable surface (grounded-tracking
    /// mode). When grounded, z follows terrain directly without gravity so
    /// the character doesn't bounce on uneven slopes.
    pub grounded: bool,
    /// Remaining seconds in the current dash burst (0 = not dashing).
    pub dash_timer: f32,
}

/// BRP HTTP port for the client (used by ralph scenarios in headless mode).
#[cfg(not(target_family = "wasm"))]
const BRP_PORT: u16 = 15702;

fn add_windowed_plugins(app: &mut App) {
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
    .add_plugins(plugins::BillboardSpritePlugin)
    .add_plugins(plugins::BattleVisualsPlugin)
    .add_plugins(plugins::HudPlugin)
    .add_plugins(plugins::MapPlugin)
    .add_plugins(plugins::PauseMenuPlugin)
    .add_plugins(plugins::DebugConsolePlugin);
}

fn main() {
    let mut app = App::new();

    let args: Vec<String> = std::env::args().collect();
    let seed          = fellytip_server::parse_arg(&args, "--seed",              WORLD_SEED);
    let map_width     = fellytip_server::parse_arg(&args, "--map-width",         MAP_WIDTH);
    let map_height    = fellytip_server::parse_arg(&args, "--map-height",        MAP_HEIGHT);
    let history_warp  = fellytip_server::parse_arg(&args, "--history-warp-ticks", 0u64);
    let npcs_per_fac  = fellytip_server::parse_arg(&args, "--npcs-per-faction",  3usize);

    #[cfg(not(target_family = "wasm"))]
    {
        let headless    = args.iter().any(|a| a == "--headless");
        let combat_test = args.iter().any(|a| a == "--combat-test");
        if headless {
            tracing_subscriber::fmt::init();
            app.add_plugins(MinimalPlugins)
                .add_plugins(
                    RemotePlugin::default()
                        .with_method("dm/spawn_npc",         fellytip_server::plugins::dm::dm_spawn_npc)
                        .with_method("dm/kill",              fellytip_server::plugins::dm::dm_kill)
                        .with_method("dm/teleport",          fellytip_server::plugins::dm::dm_teleport)
                        .with_method("dm/set_faction",       fellytip_server::plugins::dm::dm_set_faction)
                        .with_method("dm/trigger_war_party", fellytip_server::plugins::dm::dm_trigger_war_party)
                        .with_method("dm/set_ecology",       fellytip_server::plugins::dm::dm_set_ecology)
                )
                .add_plugins(RemoteHttpPlugin::default().with_port(BRP_PORT))
                .add_systems(Update, (headless_auto_attack, headless_auto_move));
        } else {
            add_windowed_plugins(&mut app);
        }
        app.add_plugins(FellytipProtocolPlugin)
            .add_plugins(ServerGamePlugin {
                seed,
                width:               map_width,
                height:              map_height,
                history_warp_ticks:  history_warp,
                npcs_per_faction:    npcs_per_fac,
                combat_test,
            });
    }
    #[cfg(target_family = "wasm")]
    {
        add_windowed_plugins(&mut app);
        app.add_plugins(FellytipProtocolPlugin)
            .add_plugins(ServerGamePlugin {
                seed,
                width:               map_width,
                height:              map_height,
                history_warp_ticks:  history_warp,
                npcs_per_faction:    npcs_per_fac,
                combat_test: false,
            });
    }

    app.configure_sets(
            Update,
            (ClientSet::Input, ClientSet::SyncVisuals, ClientSet::SyncCamera).chain(),
        )
        .add_systems(
            Update,
            (
                tag_local_player,
                ApplyDeferred,
                send_player_input.in_set(ClientSet::Input),
            ).chain(),
        )
        .add_systems(
            Update,
            sync_pred_to_world.after(ClientSet::Input).before(ClientSet::SyncVisuals),
        );

    app.run();
}

// ── Local-player tagging ──────────────────────────────────────────────────────

/// Insert `LocalPlayer` and `PredictedPosition` on the player entity once it
/// exists. Runs every frame until tagged; safe to call repeatedly (Without<LocalPlayer>
/// guard prevents re-insertion).
#[allow(clippy::type_complexity)]
fn tag_local_player(
    query: Query<(Entity, &WorldPosition), (With<Experience>, Without<LocalPlayer>)>,
    mut commands: Commands,
) {
    let Ok((entity, pos)) = query.single() else { return };
    commands.entity(entity).insert((
        LocalPlayer,
        PredictedPosition { x: pos.x, y: pos.y, z: pos.z, z_vel: 0.0, grounded: true, dash_timer: 0.0 },
    ));
    tracing::debug!("Tagged local player entity {entity:?}");
}

// ── Position sync ─────────────────────────────────────────────────────────────

/// Copy PredictedPosition → WorldPosition for the local player each frame so
/// that server-side combat and AI systems see the current position.
///
/// MULTIPLAYER: remove this system; the server authoritative position will be
/// sent by the client via PlayerInput message and reconciled with WorldPosition.
fn sync_pred_to_world(
    mut q: Query<(&PredictedPosition, &mut WorldPosition), With<LocalPlayer>>,
) {
    for (pred, mut world) in &mut q {
        world.x = pred.x;
        world.y = pred.y;
        world.z = pred.z;
    }
}

// ── Input ──────────────────────────────────────────────────────────────────────

/// Read keyboard input, apply client-authoritative movement prediction to
/// `PredictedPosition` (for instant visual response), and push any action
/// intent into `LocalPlayerInput` for the server-side combat system to process.
///
/// MULTIPLAYER: restore MessageSender<PlayerInput> and send the full PlayerInput
/// struct over the network instead of writing to LocalPlayerInput.
#[allow(clippy::too_many_arguments)]
fn send_player_input(
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    camera_q: Query<&OrbitCamera>,
    mut pred_q: Query<&mut PredictedPosition, With<LocalPlayer>>,
    map: Option<Res<WorldMap>>,
    time: Res<Time>,
    console: Option<Res<plugins::DebugConsole>>,
    pause_menu: Option<Res<plugins::pause_menu::PauseMenu>>,
    map_win: Option<Res<plugins::MapWindow>>,
    char_screen: Option<Res<plugins::CharScreen>>,
    mut local_input: ResMut<LocalPlayerInput>,
) {
    let Some(keyboard) = keyboard else { return };
    if console.is_some_and(|c| c.open)
        || pause_menu.is_some_and(|m| m.open)
        || map_win.is_some_and(|m| m.open)
        || char_screen.is_some_and(|s| s.open)
    {
        return;
    }

    let mut raw_x = 0.0_f32;
    let mut raw_y = 0.0_f32;
    if keyboard.pressed(KeyCode::KeyW) || keyboard.pressed(KeyCode::ArrowUp)    { raw_y += 1.0; }
    if keyboard.pressed(KeyCode::KeyS) || keyboard.pressed(KeyCode::ArrowDown)  { raw_y -= 1.0; }
    if keyboard.pressed(KeyCode::KeyA) || keyboard.pressed(KeyCode::ArrowLeft)  { raw_x -= 1.0; }
    if keyboard.pressed(KeyCode::KeyD) || keyboard.pressed(KeyCode::ArrowRight) { raw_x += 1.0; }

    let len = (raw_x * raw_x + raw_y * raw_y).sqrt();
    if len > 0.0 { raw_x /= len; raw_y /= len; }

    let yaw = camera_q.iter().next().map(|c| c.yaw).unwrap_or(0.0);
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let world_dx =  cos_yaw * raw_x - sin_yaw * raw_y;
    let world_dy = -sin_yaw * raw_x - cos_yaw * raw_y;

    if let Ok(mut pred) = pred_q.single_mut() {
        let dt = time.delta_secs();

        // ── Jump ─────────────────────────────────────────────────────────────
        if keyboard.just_pressed(KeyCode::Space) && pred.grounded {
            pred.z_vel = JUMP_SPEED;
            pred.grounded = false;
        }

        // ── Dash ─────────────────────────────────────────────────────────────
        let dashing = pred.dash_timer > 0.0;
        if keyboard.just_pressed(KeyCode::ShiftLeft) && !dashing {
            pred.dash_timer = DASH_DURATION;
        }
        pred.dash_timer = (pred.dash_timer - dt).max(0.0);

        // ── Horizontal movement ───────────────────────────────────────────────
        let in_water = map.as_ref().is_some_and(|m| is_water_at(m, pred.x, pred.y));
        // Halve speed while swimming.
        let speed_mul = if in_water { 0.5 } else { 1.0 };
        let speed = if pred.dash_timer > 0.0 { DASH_SPEED } else { PLAYER_SPEED };
        let new_x = pred.x + world_dx * speed * speed_mul * dt;
        let new_y = pred.y + world_dy * speed * speed_mul * dt;

        if let Some(ref m) = map {
            // Allow movement into water tiles so the player can swim.
            let can_xy = is_walkable_at(m, new_x, new_y, pred.z) || is_water_at(m, new_x, new_y);
            let can_x  = is_walkable_at(m, new_x, pred.y, pred.z) || is_water_at(m, new_x, pred.y);
            let can_y  = is_walkable_at(m, pred.x, new_y, pred.z) || is_water_at(m, pred.x, new_y);
            if      can_xy { pred.x = new_x; pred.y = new_y; }
            else if can_x  { pred.x = new_x; }
            else if can_y  { pred.y = new_y; }

            // ── Vertical / gravity ────────────────────────────────────────────
            let terrain_z = smooth_surface_at(m, pred.x, pred.y, pred.z);
            let water_z   = water_surface_at(m, pred.x, pred.y);

            if let Some(wz) = water_z {
                // Over water: apply gravity above surface, buoyancy below.
                if pred.z > wz + LAND_SNAP {
                    // Falling toward water surface.
                    pred.z_vel = (pred.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
                    pred.z += pred.z_vel * dt;
                    pred.grounded = false;
                    if pred.z <= wz {
                        pred.z = wz;
                        pred.z_vel = 0.0;
                        pred.grounded = true;
                    }
                } else if pred.z < wz - LAND_SNAP {
                    // Submerged: buoyancy pushes up to surface.
                    pred.z_vel = (pred.z_vel + SWIM_BUOYANCY * dt).min(SWIM_RISE_SPEED);
                    pred.z += pred.z_vel * dt;
                    pred.grounded = false;
                    if pred.z >= wz {
                        pred.z = wz;
                        pred.z_vel = 0.0;
                        pred.grounded = true;
                    }
                } else {
                    // At surface: float.
                    pred.z = wz;
                    pred.z_vel = 0.0;
                    pred.grounded = true;
                }
            } else if pred.grounded {
                match terrain_z {
                    Some(tz) if tz >= pred.z - STEP_HEIGHT => {
                        // Ground-tracking: follow terrain height directly, no
                        // gravity, so the character never bounces on slopes.
                        pred.z = tz;
                        pred.z_vel = 0.0;
                    }
                    _ => {
                        // Walked off a ledge — enter airborne state.
                        pred.grounded = false;
                        pred.z_vel = 0.0;
                    }
                }
            } else {
                // Airborne: integrate gravity.
                pred.z_vel = (pred.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
                pred.z += pred.z_vel * dt;

                if let Some(tz) = terrain_z {
                    if pred.z <= tz + LAND_SNAP {
                        pred.z = tz;
                        pred.z_vel = 0.0;
                        pred.grounded = true;
                    }
                }
            }

            // ── Slope speed correction (land only) ───────────────────────────
            // Project velocity onto the terrain plane so uphill/downhill
            // movement stays at PLAYER_SPEED.  Skip in water (no terrain normal).
            if pred.grounded && !in_water && (world_dx != 0.0 || world_dy != 0.0) {
                let normal = terrain_normal_at(m, pred.x, pred.y, pred.z);
                // Horizontal world-space velocity (Bevy: x east, z south).
                let vel = Vec3::new(world_dx * speed, 0.0, world_dy * speed);
                // Project onto slope plane: v_proj = v - (v·n)n
                let v_proj = vel - normal * vel.dot(normal);
                let correction_dt = dt;
                // Apply the difference between flat and projected movement.
                let delta = v_proj - vel;
                pred.x += delta.x * correction_dt;
                pred.y += delta.z * correction_dt;
                // The y-component of v_proj gives the along-slope elevation delta.
                pred.z += v_proj.y * correction_dt;
            }
        } else {
            pred.x = new_x;
            pred.y = new_y;
        }
    }

    // Queue combat action intents for server-side processing.
    let action = if keyboard.just_pressed(KeyCode::KeyQ) {
        Some(ActionIntent::BasicAttack)
    } else if keyboard.just_pressed(KeyCode::KeyE) {
        Some(ActionIntent::UseAbility(1))
    } else {
        None
    };
    if action.is_some() {
        local_input.actions.push((action, None));
    }
}

// ── Headless automation (ralph test scenarios) ────────────────────────────────

/// Headless-mode: sends BasicAttack every 2 seconds via LocalPlayerInput.
#[cfg(not(target_family = "wasm"))]
fn headless_auto_attack(
    mut local_input: ResMut<LocalPlayerInput>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
) {
    *elapsed += time.delta_secs();
    if *elapsed < 2.0 { return; }
    *elapsed = 0.0;
    local_input.actions.push((Some(ActionIntent::BasicAttack), None));
    tracing::debug!("Headless: auto BasicAttack queued");
}

/// Headless-mode: walks the player right for 3 s then left for 3 s, repeating.
#[cfg(not(target_family = "wasm"))]
fn headless_auto_move(
    mut pred_q: Query<&mut PredictedPosition, With<LocalPlayer>>,
    time: Res<Time>,
    mut phase_elapsed: Local<f32>,
    mut phase_right: Local<bool>,
) {
    let Ok(mut pred) = pred_q.single_mut() else { return };
    *phase_elapsed += time.delta_secs();
    if *phase_elapsed >= 3.0 {
        *phase_elapsed = 0.0;
        *phase_right = !*phase_right;
    }
    let dir_x: f32 = if *phase_right { 1.0 } else { -1.0 };
    pred.x += dir_x * PLAYER_SPEED * time.delta_secs();
}
