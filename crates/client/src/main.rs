mod plugins;

use avian3d::prelude::*;
use bevy::prelude::*;
#[cfg(not(target_family = "wasm"))]
use bevy::remote::{BrpError, BrpResult, RemotePlugin, http::RemoteHttpPlugin};
#[cfg(not(target_family = "wasm"))]
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use fellytip_game::ServerGamePlugin;
use fellytip_shared::{
    PLAYER_SPEED, WORLD_SEED,
    bridge::{ClientFrameTimings, LocalPlayerInput},
    combat::types::CharacterClass,
    components::{EntityBounds, Experience, WorldPosition},
    inputs::ActionIntent,
    protocol::{ChooseClassMessage, FellytipProtocolPlugin},
    world::map::{is_passable_with_bounds, is_water_at, water_surface_at, smooth_surface_at, terrain_normal_at, find_surface_spawn, WorldMap, GRAVITY, JUMP_SPEED, DASH_SPEED, DASH_DURATION, LAND_SNAP, MAX_FALL_SPEED, STEP_HEIGHT, SWIM_BUOYANCY, SWIM_RISE_SPEED, MAP_WIDTH, MAP_HEIGHT, Z_SCALE},
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

// Absolute path to the assets directory baked in at compile time so the
// binary finds its assets regardless of the working directory it's launched from.
#[cfg(debug_assertions)]
const ASSET_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
#[cfg(not(debug_assertions))]
const ASSET_PATH: &str = "assets";

fn add_windowed_plugins(app: &mut App, visible: bool) {
    app.add_plugins(PhysicsPlugins::default())
        .add_plugins(avian3d::debug_render::PhysicsDebugPlugin)
        .insert_gizmo_config(
            avian3d::debug_render::PhysicsGizmos::default(),
            GizmoConfig { enabled: false, ..default() },
        )
        .add_plugins(
            DefaultPlugins.build()
                .set(AssetPlugin {
                    file_path: ASSET_PATH.into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Fellytip".into(),
                        visible,
                        ..default()
                    }),
                    ..default()
                }),
        )
    .add_systems(Update, track_frame_time)
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
    .add_plugins(plugins::ClassSelectionPlugin)
    .add_plugins(plugins::MapPlugin)
    .add_plugins(plugins::SettingsPlugin)
    .add_plugins(plugins::PauseMenuPlugin)
    .add_plugins(plugins::DebugConsolePlugin)
    .add_plugins(plugins::ZoneCachePlugin)
    .add_plugins(plugins::ZoneRendererPlugin)
    .add_plugins(plugins::PortalRendererPlugin)
    .add_plugins(plugins::ParticlesPlugin)
    .add_plugins(plugins::TargetSelectPlugin)
    .add_plugins(plugins::FloatingTextPlugin)
    .add_plugins(plugins::ActionMenuPlugin)
    .add_plugins(plugins::ClickToMovePlugin);
}

fn main() {
    let mut app = App::new();

    let args: Vec<String> = std::env::args().collect();
    let seed          = fellytip_shared::parse_arg(&args, "--seed",              WORLD_SEED);
    let map_width     = fellytip_shared::parse_arg(&args, "--map-width",         MAP_WIDTH);
    let map_height    = fellytip_shared::parse_arg(&args, "--map-height",        MAP_HEIGHT);
    let history_warp  = fellytip_shared::parse_arg(&args, "--history-warp-ticks", 0u64);
    let npcs_per_fac  = fellytip_shared::parse_arg(&args, "--npcs-per-faction",  3usize);

    #[cfg(not(target_family = "wasm"))]
    {
        let headless    = args.iter().any(|a| a == "--headless");
        let combat_test = args.iter().any(|a| a == "--combat-test");
        if headless {
            tracing_subscriber::fmt::init();
            add_windowed_plugins(&mut app, false);
            app.add_plugins(
                    RemotePlugin::default()
                        .with_method("dm/spawn_npc",         fellytip_server::plugins::dm::dm_spawn_npc)
                        .with_method("dm/kill",              fellytip_server::plugins::dm::dm_kill)
                        .with_method("dm/teleport",          fellytip_server::plugins::dm::dm_teleport)
                        .with_method("dm/set_faction",       fellytip_server::plugins::dm::dm_set_faction)
                        .with_method("dm/trigger_war_party", fellytip_server::plugins::dm::dm_trigger_war_party)
                        .with_method("dm/set_ecology",       fellytip_server::plugins::dm::dm_set_ecology)
                        .with_method("dm/battle_history",    fellytip_server::plugins::dm::dm_battle_history)
                        .with_method("dm/clear_battle_history", fellytip_server::plugins::dm::dm_clear_battle_history)
                        .with_method("dm/underground_pressure", fellytip_server::plugins::dm::dm_underground_pressure)
                        .with_method("dm/force_underground_pressure", fellytip_server::plugins::dm::dm_force_underground_pressure)
                        .with_method("dm/query_portals",     fellytip_server::plugins::dm::dm_query_portals)
                        .with_method("dm/spawn_wildlife",    fellytip_server::plugins::dm::dm_spawn_wildlife)
                        .with_method("dm/list_settlements",  fellytip_server::plugins::dm::dm_list_settlements)
                        .with_method("dm/spawn_raid",        fellytip_server::plugins::dm::dm_spawn_raid)
                        .with_method("dm/give_gold",         fellytip_server::plugins::dm::dm_give_gold)
                        .with_method("dm/spawn_bot",         fellytip_server::plugins::bot::dm_spawn_bot)
                        .with_method("dm/despawn_bot",       fellytip_server::plugins::bot::dm_despawn_bot)
                        .with_method("dm/list_bots",         fellytip_server::plugins::bot::dm_list_bots)
                        .with_method("dm/set_bot_action",    fellytip_server::plugins::bot::dm_set_bot_action)
                        .with_method("dm/set_portal_debug",  dm_set_portal_debug)
                        .with_method("dm/take_screenshot",        dm_take_screenshot)
                        .with_method("dm/set_camera_distance",    dm_set_camera_distance)
                        .with_method("dm/teleport_player",        dm_teleport_player)
                        .with_method("dm/set_character_debug",    dm_set_character_debug)
                        .with_method("dm/set_camera_free",        dm_set_camera_free)
                        .with_method("dm/choose_class",           dm_choose_class)
                        .with_method("dm/set_time_of_day",        dm_set_time_of_day)
                        .with_method("dm/enter_portal",           dm_enter_portal)
                        .with_method("dm/toggle_physics_debug",   dm_toggle_physics_debug)
                        .with_method("dm/move_entity",            fellytip_server::plugins::dm::dm_move_entity)
                )
                .add_plugins(RemoteHttpPlugin::default().with_port(BRP_PORT))
                .add_systems(Update, (headless_auto_attack, headless_auto_move));
        } else {
            add_windowed_plugins(&mut app, true);
            app.add_plugins(
                RemotePlugin::default()
                    .with_method("dm/spawn_npc",                  fellytip_server::plugins::dm::dm_spawn_npc)
                    .with_method("dm/kill",                       fellytip_server::plugins::dm::dm_kill)
                    .with_method("dm/teleport",                   fellytip_server::plugins::dm::dm_teleport)
                    .with_method("dm/set_faction",                fellytip_server::plugins::dm::dm_set_faction)
                    .with_method("dm/trigger_war_party",          fellytip_server::plugins::dm::dm_trigger_war_party)
                    .with_method("dm/set_ecology",                fellytip_server::plugins::dm::dm_set_ecology)
                    .with_method("dm/battle_history",             fellytip_server::plugins::dm::dm_battle_history)
                    .with_method("dm/clear_battle_history",       fellytip_server::plugins::dm::dm_clear_battle_history)
                    .with_method("dm/underground_pressure",       fellytip_server::plugins::dm::dm_underground_pressure)
                    .with_method("dm/force_underground_pressure", fellytip_server::plugins::dm::dm_force_underground_pressure)
                    .with_method("dm/query_portals",              fellytip_server::plugins::dm::dm_query_portals)
                    .with_method("dm/spawn_wildlife",             fellytip_server::plugins::dm::dm_spawn_wildlife)
                    .with_method("dm/list_settlements",           fellytip_server::plugins::dm::dm_list_settlements)
                    .with_method("dm/spawn_raid",                 fellytip_server::plugins::dm::dm_spawn_raid)
                    .with_method("dm/give_gold",                  fellytip_server::plugins::dm::dm_give_gold)
                    .with_method("dm/spawn_bot",                  fellytip_server::plugins::bot::dm_spawn_bot)
                    .with_method("dm/despawn_bot",                fellytip_server::plugins::bot::dm_despawn_bot)
                    .with_method("dm/list_bots",                  fellytip_server::plugins::bot::dm_list_bots)
                    .with_method("dm/set_bot_action",             fellytip_server::plugins::bot::dm_set_bot_action)
                    .with_method("dm/set_portal_debug",           dm_set_portal_debug)
                    .with_method("dm/take_screenshot",            dm_take_screenshot)
                    .with_method("dm/set_camera_distance",        dm_set_camera_distance)
                    .with_method("dm/teleport_player",            dm_teleport_player)
                    .with_method("dm/set_character_debug",        dm_set_character_debug)
                    .with_method("dm/set_camera_free",            dm_set_camera_free)
                    .with_method("dm/choose_class",               dm_choose_class)
                    .with_method("dm/set_time_of_day",            dm_set_time_of_day)
                    .with_method("dm/enter_portal",               dm_enter_portal)
                    .with_method("dm/toggle_physics_debug",       dm_toggle_physics_debug)
                    .with_method("dm/move_entity",                fellytip_server::plugins::dm::dm_move_entity)
            )
            .add_plugins(RemoteHttpPlugin::default().with_port(BRP_PORT));
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
        add_windowed_plugins(&mut app, true);
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
        )
        .add_systems(
            PostUpdate,
            sync_physics_to_pred,
        );

    app.run();
}

// ── Local-player tagging ──────────────────────────────────────────────────────

/// Insert `LocalPlayer`, `PredictedPosition`, and avian3d physics components on
/// the player entity once it exists. The capsule collider (half-height 0.9, radius
/// 0.35 ≈ 2.5 m tall) lives on a child entity offset upward so its bottom aligns
/// with the parent origin (feet). The parent Transform stays at feet level so that
/// the visual model and PredictedPosition.z always represent the ground contact point.
#[allow(clippy::type_complexity)]
fn tag_local_player(
    query: Query<
        (Entity, &WorldPosition),
        (
            With<Experience>,
            Without<LocalPlayer>,
            Without<fellytip_game::plugins::bot::BotController>,
        ),
    >,
    mut commands: Commands,
    map: Option<Res<WorldMap>>,
) {
    let Ok((entity, pos)) = query.single() else { return };
    // Snap z to the actual terrain surface using a high ceiling so that stale
    // DB values or the (0,0,0) spawn fallback never place the player below ground.
    let initial_z = map.as_deref()
        .and_then(|m| smooth_surface_at(m, pos.x, pos.y, Z_SCALE * 3.0))
        .unwrap_or(pos.z);
    // Bevy world space: x=east, y=up (vertical), z=south.
    // PredictedPosition:  x=east, y=south,        z=up (vertical).
    let initial_transform = Transform::from_xyz(pos.x, initial_z, pos.y);
    commands.entity(entity)
        .insert((
            LocalPlayer,
            PredictedPosition { x: pos.x, y: pos.y, z: initial_z, z_vel: 0.0, grounded: true, dash_timer: 0.0 },
            EntityBounds::PLAYER,
            initial_transform,
            // Dynamic so avian's contact solver pushes the capsule out of static
            // terrain.  Kinematic bodies have dominance=128 (same as Static), so
            // the constraint solver sees two zero-inv-mass bodies and produces no
            // push-back — the capsule passed straight through.
            // GravityScale(0) disables avian's built-in gravity; we drive it manually
            // via LinearVelocity.y so water buoyancy and jump logic stay unchanged.
            RigidBody::Dynamic,
            GravityScale(0.0),
            Friction::ZERO,
            LockedAxes::ROTATION_LOCKED,
            LinearVelocity::ZERO,
            // SweptCcd prevents tunnelling when fall speed reaches MAX_FALL_SPEED
            // (50 m/s → 0.8 m per 62.5 Hz step, enough to skip a 1-m terrain tile).
            SweptCcd::default(),
            // ShapeCaster probes just below the feet (parent origin = feet level).
            ShapeCaster::new(
                Collider::sphere(0.28),
                Vec3::new(0.0, -0.05, 0.0),
                Quat::IDENTITY,
                Dir3::NEG_Y,
            ).with_max_distance(0.15),
        ))
        .with_children(|parent| {
            // Bottom of capsule sits 0.05 above the parent origin (feet).
            // The small buffer prevents avian from tunnelling through thin trimesh seams
            // when the contact point is exactly at the edge of a terrain chunk.
            // half_height=0.9, radius=0.35 → center = 0.9+0.35+0.05 = 1.30 above feet.
            parent.spawn((
                Collider::capsule(0.9, 0.35),
                Transform::from_translation(Vec3::Y * (0.9 + 0.35 + 0.05)),
            ));
        });
    tracing::debug!("Tagged local player entity {entity:?} at z={initial_z:.2}");
}

// ── Host-mode frame-time monitoring ──────────────────────────────────────────

/// Push each Update delta into the shared `ClientFrameTimings` resource so the
/// server's `update_throttle_level` can bump the AI throttle when the render
/// thread is the bottleneck (host mode only — headless runs with `MinimalPlugins`
/// do not install this system because `add_windowed_plugins` is skipped).
fn track_frame_time(time: Res<Time>, mut timings: ResMut<ClientFrameTimings>) {
    timings.push(time.delta_secs());
}

// ── Position sync ─────────────────────────────────────────────────────────────

/// After avian3d resolves physics in FixedUpdate, read the updated `Transform`
/// back into `PredictedPosition` so the camera and WorldPosition sync use the
/// collision-corrected position. Only runs for physics-controlled players
/// (those with `LinearVelocity`). Runs in `PostUpdate`.
#[allow(clippy::type_complexity)]
fn sync_physics_to_pred(
    mut q: Query<(&Transform, &mut PredictedPosition), (With<LocalPlayer>, With<LinearVelocity>)>,
) {
    for (transform, mut pred) in &mut q {
        // Bevy world: x=east, y=up, z=south → PredictedPosition: x=east, y=south, z=up.
        pred.x = transform.translation.x;
        pred.y = transform.translation.z;
        pred.z = transform.translation.y;
    }
}

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
/// When `LinearVelocity` is present (windowed mode with avian3d physics), horizontal
/// and vertical velocity are written to `LinearVelocity` and avian integrates them,
/// resolving collisions against terrain trimesh colliders. `ShapeHits` (from the
/// downward `ShapeCaster` inserted in `tag_local_player`) drives `pred.grounded`.
///
/// When `LinearVelocity` is absent (headless mode), the original direct-mutation
/// logic runs unchanged, using `smooth_surface_at` for grounding.
///
/// MULTIPLAYER: restore MessageSender<PlayerInput> and send the full PlayerInput
/// struct over the network instead of writing to LocalPlayerInput.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn send_player_input(
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    egui_consumed: Option<Res<plugins::EguiPointerConsumed>>,
    hovered_target: Option<Res<plugins::target_select::HoveredTarget>>,
    camera_q: Query<&OrbitCamera>,
    mut pred_q: Query<(
        &mut Transform,
        &mut PredictedPosition,
        &EntityBounds,
        Option<&mut LinearVelocity>,
        Option<&ShapeHits>,
    ), With<LocalPlayer>>,
    map: Option<Res<WorldMap>>,
    time: Res<Time>,
    console: Option<Res<plugins::DebugConsole>>,
    pause_menu: Option<Res<plugins::pause_menu::PauseMenu>>,
    map_win: Option<Res<plugins::MapWindow>>,
    char_screen: Option<Res<plugins::CharScreen>>,
    class_sel: Option<Res<plugins::ClassSelectionState>>,
    mut local_input: ResMut<LocalPlayerInput>,
) {
    let Some(keyboard) = keyboard else { return };
    if console.is_some_and(|c| c.open)
        || pause_menu.is_some_and(|m| m.open)
        || map_win.is_some_and(|m| m.open)
        || char_screen.is_some_and(|s| s.open)
        || class_sel.is_some_and(|s| s.open)
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

    let Ok((mut transform, mut pred, bounds, mut linear_vel, shape_hits)) = pred_q.single_mut()
    else { return };
    let bounds = *bounds;
    let dt = time.delta_secs();
    let has_physics = linear_vel.is_some();

    // ── Ground detection (physics mode) ──────────────────────────────────────
    if has_physics
        && let Some(hits) = shape_hits {
            // normal1 is the outward normal on the cast sphere; for a downward cast
            // hitting a surface, normal1.y > 0 means the surface pushes us upward.
            pred.grounded = hits.iter().any(|h| h.normal1.y > 0.5);
        }

    // ── Jump ─────────────────────────────────────────────────────────────────
    if keyboard.just_pressed(KeyCode::Space) && pred.grounded {
        pred.z_vel = JUMP_SPEED;
        pred.grounded = false;
    }

    // ── Dash ─────────────────────────────────────────────────────────────────
    let dashing = pred.dash_timer > 0.0;
    if keyboard.just_pressed(KeyCode::ShiftLeft) && !dashing {
        pred.dash_timer = DASH_DURATION;
    }
    pred.dash_timer = (pred.dash_timer - dt).max(0.0);

    // ── Common speed parameters ───────────────────────────────────────────────
    let in_water = map.as_ref().is_some_and(|m| is_water_at(m, pred.x, pred.y));
    let speed_mul = if in_water { 0.5 } else { 1.0 };
    let speed = if pred.dash_timer > 0.0 { DASH_SPEED } else { PLAYER_SPEED };

    if has_physics {
        // ── Physics path ──────────────────────────────────────────────────────
        // Set horizontal velocity; avian integrates and resolves collisions.
        if let Some(ref mut lv) = linear_vel {
            lv.x = world_dx * speed * speed_mul;
            lv.z = world_dy * speed * speed_mul;
        }

        // Vertical velocity: manual gravity/buoyancy written into lv.y.
        if let Some(ref m) = map {
            let water_z = water_surface_at(m, pred.x, pred.y);
            if let Some(wz) = water_z {
                if pred.z > wz + LAND_SNAP {
                    pred.z_vel = (pred.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
                    pred.grounded = false;
                } else if pred.z < wz - LAND_SNAP {
                    pred.z_vel = (pred.z_vel + SWIM_BUOYANCY * dt).min(SWIM_RISE_SPEED);
                    pred.grounded = false;
                } else {
                    pred.z_vel = 0.0;
                    pred.grounded = true;
                }
            } else if pred.grounded {
                pred.z_vel = 0.0;
            } else {
                pred.z_vel = (pred.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
            }

            if let Some(ref mut lv) = linear_vel {
                lv.y = pred.z_vel;
            }

            // Safety floor: teleport via Transform (avian's position) when the
            // player falls through all geometry.
            // -15.0 is below any surface terrain (max Z_SCALE=26) but above cave depth 2
            // (-30), so it catches surface fall-through quickly without false-triggering
            // on depth-1 cave players (~-10).
            if pred.z < -15.0 {
                let (sx, sy, sz) = m.spawn_points.first().copied()
                    .unwrap_or_else(|| find_surface_spawn(m));
                pred.x = sx; pred.y = sy; pred.z = sz;
                pred.z_vel = 0.0; pred.grounded = true;
                transform.translation = Vec3::new(sx, sz, sy);
                if let Some(ref mut lv) = linear_vel {
                    lv.x = 0.0; lv.y = 0.0; lv.z = 0.0;
                }
                tracing::warn!("Player fell below floor — respawned at ({sx:.1}, {sy:.1}, {sz:.1})");
            }
        }
    } else {
        // ── No-physics path (headless / minimal plugins) ───────────────────────
        let new_x = pred.x + world_dx * speed * speed_mul * dt;
        let new_y = pred.y + world_dy * speed * speed_mul * dt;

        if let Some(ref m) = map {
            // Allow movement into water tiles so the player can swim.
            let can_xy = is_passable_with_bounds(m, new_x, new_y, pred.z, bounds) || is_water_at(m, new_x, new_y);
            let can_x  = is_passable_with_bounds(m, new_x, pred.y, pred.z, bounds) || is_water_at(m, new_x, pred.y);
            let can_y  = is_passable_with_bounds(m, pred.x, new_y, pred.z, bounds) || is_water_at(m, pred.x, new_y);
            if      can_xy { pred.x = new_x; pred.y = new_y; }
            else if can_x  { pred.x = new_x; }
            else if can_y  { pred.y = new_y; }

            // ── Vertical / gravity ────────────────────────────────────────────
            let terrain_z = smooth_surface_at(m, pred.x, pred.y, pred.z);
            let water_z   = water_surface_at(m, pred.x, pred.y);

            if let Some(wz) = water_z {
                if pred.z > wz + LAND_SNAP {
                    pred.z_vel = (pred.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
                    pred.z += pred.z_vel * dt;
                    pred.grounded = false;
                    if pred.z <= wz {
                        pred.z = wz; pred.z_vel = 0.0; pred.grounded = true;
                    }
                } else if pred.z < wz - LAND_SNAP {
                    pred.z_vel = (pred.z_vel + SWIM_BUOYANCY * dt).min(SWIM_RISE_SPEED);
                    pred.z += pred.z_vel * dt;
                    pred.grounded = false;
                    if pred.z >= wz {
                        pred.z = wz; pred.z_vel = 0.0; pred.grounded = true;
                    }
                } else {
                    pred.z = wz; pred.z_vel = 0.0; pred.grounded = true;
                }
            } else if pred.grounded {
                match terrain_z {
                    Some(tz) if tz >= pred.z - STEP_HEIGHT => {
                        pred.z = tz; pred.z_vel = 0.0;
                    }
                    _ => {
                        pred.grounded = false; pred.z_vel = 0.0;
                    }
                }
            } else {
                pred.z_vel = (pred.z_vel + GRAVITY * dt).max(MAX_FALL_SPEED);
                pred.z += pred.z_vel * dt;
                if let Some(tz) = terrain_z
                    && pred.z <= tz + LAND_SNAP {
                        pred.z = tz; pred.z_vel = 0.0; pred.grounded = true;
                    }
            }

            // ── Slope speed correction ────────────────────────────────────────
            if pred.grounded && !in_water && (world_dx != 0.0 || world_dy != 0.0) {
                let normal = terrain_normal_at(m, pred.x, pred.y, pred.z);
                let vel = Vec3::new(world_dx * speed, 0.0, world_dy * speed);
                let v_proj = vel - normal * vel.dot(normal);
                let delta = v_proj - vel;
                pred.x += delta.x * dt;
                pred.y += delta.z * dt;
                pred.z += v_proj.y * dt;
            }

            // Safety floor (same -15.0 threshold as the physics path above).
            if pred.z < -15.0 {
                let (sx, sy, sz) = m.spawn_points.first().copied()
                    .unwrap_or_else(|| find_surface_spawn(m));
                pred.x = sx; pred.y = sy; pred.z = sz;
                pred.z_vel = 0.0; pred.grounded = true;
                tracing::warn!("Player fell below floor — respawned at ({sx:.1}, {sy:.1}, {sz:.1})");
            }
        } else {
            pred.x = new_x;
            pred.y = new_y;
        }
    }

    // Queue combat action intents for server-side processing.
    // Left-click on a hovered enemy = targeted basic attack.
    // Left-click on empty space = attack nearest enemy (fallback).
    let egui_over = egui_consumed.as_ref().is_some_and(|e| e.0);
    let lmb_attack = !egui_over && mouse.as_ref().is_some_and(|m| m.just_pressed(MouseButton::Left));
    let action = if lmb_attack || keyboard.just_pressed(KeyCode::KeyQ) {
        Some(ActionIntent::BasicAttack)
    } else if keyboard.just_pressed(KeyCode::KeyE) {
        Some(ActionIntent::UseAbility(1))
    } else {
        None
    };
    if action.is_some() {
        let target_uuid = if lmb_attack {
            hovered_target.as_ref().and_then(|h| h.0.map(|(_, uuid)| uuid))
        } else {
            None
        };
        local_input.actions.push((action, target_uuid));
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

// ── dm/set_portal_debug ───────────────────────────────────────────────────────

/// Enable or disable the portal debug overlay (bright emissive highlight on all
/// portal meshes so they are impossible to miss in screenshots).
///
/// Params: `{ "enabled": bool }`
/// Returns `{ "ok": true, "enabled": bool }`.
#[cfg(not(target_family = "wasm"))]
fn dm_set_portal_debug(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    let enabled = params
        .as_ref()
        .and_then(|p| p.get("enabled"))
        .and_then(|v| v.as_bool())
        .ok_or_else(|| BrpError::internal("missing required param `enabled`"))?;

    let mut overlay = world.resource_mut::<plugins::portal_renderer::PortalDebugOverlay>();
    overlay.0 = enabled;
    tracing::info!(enabled, "DM set portal debug overlay");
    Ok(serde_json::json!({ "ok": true, "enabled": enabled }))
}

/// Trigger a screenshot save to a given path (or /tmp/fellytip_screenshot.png by default).
///
/// Params: `{ "path": "/tmp/out.png" }` (optional)
/// Returns `{ "ok": true, "path": "..." }`.
#[cfg(not(target_family = "wasm"))]
fn dm_take_screenshot(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    let path = params
        .as_ref()
        .and_then(|p| p.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("/tmp/fellytip_screenshot.png")
        .to_owned();

    world.commands().spawn(Screenshot::primary_window())
        .observe(save_to_disk(path.clone()));

    tracing::info!(%path, "DM screenshot requested");
    Ok(serde_json::json!({ "ok": true, "path": path }))
}

/// Teleport the local player by writing PredictedPosition (and WorldPosition),
/// bypassing the normal PredictedPosition→WorldPosition copy that makes
/// dm/teleport no-op for the local player.
///
/// Params: `{ "x": f32, "y": f32, "z": f32 }` (z optional, defaults to current)
/// Returns `{ "ok": true }`.
#[cfg(not(target_family = "wasm"))]
fn dm_teleport_player(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    let p = params.as_ref().ok_or_else(|| BrpError::internal("missing params"))?;
    let x = p.get("x").and_then(|v| v.as_f64()).ok_or_else(|| BrpError::internal("missing x"))? as f32;
    let y = p.get("y").and_then(|v| v.as_f64()).ok_or_else(|| BrpError::internal("missing y"))? as f32;

    let mut q = world.query_filtered::<(&mut Transform, &mut PredictedPosition, &mut WorldPosition), With<LocalPlayer>>();
    let (mut tf, mut pred, mut wpos) = q.single_mut(world)
        .map_err(|_| BrpError::internal("no local player found"))?;
    let z = p.get("z").and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(pred.z);
    // grounded=true lets the physics system snap z to terrain height next tick,
    // preventing the player from free-falling when terrain hasn't loaded yet.
    pred.x = x; pred.y = y; pred.z = z; pred.z_vel = 0.0; pred.grounded = true;
    wpos.x = x; wpos.y = y; wpos.z = z;
    // Also move the physics body so avian doesn't rubber-band back.
    tf.translation = Vec3::new(x, z, y);
    tracing::info!(x, y, z, "DM teleport player (PredictedPosition)");
    Ok(serde_json::json!({ "ok": true }))
}

/// Enable or disable the character debug overlay (gizmo sphere at every entity
/// with `WorldPosition` so NPCs are visible even when GLB meshes aren't loaded).
///
/// Params: `{ "enabled": bool }`
/// Returns `{ "ok": true, "enabled": bool }`.
#[cfg(not(target_family = "wasm"))]
fn dm_set_character_debug(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    let enabled = params
        .as_ref()
        .and_then(|p| p.get("enabled"))
        .and_then(|v| v.as_bool())
        .ok_or_else(|| BrpError::internal("missing required param `enabled`"))?;

    let mut overlay = world.resource_mut::<plugins::entity_renderer::CharacterDebugOverlay>();
    overlay.0 = enabled;
    tracing::info!(enabled, "DM set character debug overlay");
    Ok(serde_json::json!({ "ok": true, "enabled": enabled }))
}

/// Set the orbit camera distance (zoom).
///
/// Params: `{ "distance": f32 }` — clamped to [min_distance, max_distance].
/// Returns `{ "ok": true, "distance": f32 }`.
#[cfg(not(target_family = "wasm"))]
fn dm_set_camera_distance(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    let distance = params
        .as_ref()
        .and_then(|p| p.get("distance"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| BrpError::internal("missing required param `distance`"))? as f32;

    let mut q = world.query::<&mut plugins::camera::OrbitCamera>();
    let mut cam = q.single_mut(world)
        .map_err(|_| BrpError::internal("no OrbitCamera entity found"))?;
    let clamped = distance.clamp(cam.min_distance, cam.max_distance);
    cam.distance = clamped;
    tracing::info!(distance = clamped, "DM camera distance set");
    Ok(serde_json::json!({ "ok": true, "distance": clamped }))
}

/// Choose a character class for the local player, bypassing the class selection UI.
///
/// Params: `{ "class": "Warrior" | "Rogue" | "Mage" }` (defaults to "Warrior" if omitted)
/// Returns `{ "ok": true, "class": "..." }`.
#[cfg(not(target_family = "wasm"))]
fn dm_choose_class(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    let class_str = params
        .as_ref()
        .and_then(|p| p.get("class"))
        .and_then(|v| v.as_str())
        .unwrap_or("Warrior")
        .to_owned();

    let class = match class_str.as_str() {
        "Warrior"   => CharacterClass::Warrior,
        "Rogue"     => CharacterClass::Rogue,
        "Mage"      => CharacterClass::Mage,
        "Fighter"   => CharacterClass::Fighter,
        "Wizard"    => CharacterClass::Wizard,
        "Cleric"    => CharacterClass::Cleric,
        "Ranger"    => CharacterClass::Ranger,
        "Paladin"   => CharacterClass::Paladin,
        "Druid"     => CharacterClass::Druid,
        "Bard"      => CharacterClass::Bard,
        "Warlock"   => CharacterClass::Warlock,
        "Sorcerer"  => CharacterClass::Sorcerer,
        "Monk"      => CharacterClass::Monk,
        "Barbarian" => CharacterClass::Barbarian,
        _           => CharacterClass::Warrior,
    };

    world.write_message(ChooseClassMessage { class });
    // Also hide the class selection overlay so the UI dismisses.
    if let Some(mut state) = world.get_resource_mut::<plugins::class_selection::ClassSelectionState>() {
        state.open = false;
    }
    tracing::info!(%class_str, "DM choose class");
    Ok(serde_json::json!({ "ok": true, "class": class_str }))
}

/// Trigger portal traversal for the local player to the nearest portal.
///
/// Finds the portal trigger closest to the local player's current position and
/// immediately applies the zone transition as if the player had walked through it.
/// This bypasses the normal proximity check so the player doesn't need to be
/// standing on the portal anchor.
///
/// Optionally accepts `{ "portal_id": u32 }` to target a specific portal.
///
/// Returns `{ "ok": true, "portal_id": u32, "to_zone": u32 }`.
#[cfg(not(target_family = "wasm"))]
fn dm_enter_portal(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    use fellytip_game::plugins::portal::PlayerZoneTransition;
    use fellytip_shared::world::zone::{ZoneMembership, ZoneTopology};
    use fellytip_shared::components::WorldPosition;

    // Resolve the local player's entity and position.
    let mut player_q = world.query_filtered::<(Entity, &WorldPosition, Option<&ZoneMembership>), With<LocalPlayer>>();
    let (player_entity, player_pos, player_zone) =
        player_q.single(world)
            .map_err(|_| BrpError::internal("no local player found"))?;
    let player_pos = player_pos.clone();
    let player_zone_id = player_zone.map(|z| z.0);

    // Find portal by explicit ID or nearest anchor.
    let specific_id: Option<u32> = params
        .as_ref()
        .and_then(|p| p.get("portal_id"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);

    let portal_id = {
        let topology = world.get_resource::<ZoneTopology>()
            .ok_or_else(|| BrpError::internal("ZoneTopology resource not found"))?;

        if let Some(id) = specific_id {
            if topology.portals.iter().any(|p| p.id == id) {
                id
            } else {
                return Err(BrpError::internal(format!("portal {id} not found")));
            }
        } else {
            // Use portal system's PortalTrigger query to find the nearest one.
            let mut trigger_q = world.query::<(&fellytip_game::plugins::portal::PortalTrigger, &WorldPosition, Option<&ZoneMembership>)>();
            let nearest = trigger_q
                .iter(world)
                .filter(|(trigger, tpos, tzone)| {
                    // Prefer same-zone portals; if player has no zone, allow all.
                    let zone_match = player_zone_id
                        .zip(tzone.map(|z| z.0))
                        .map(|(pz, tz)| pz == tz)
                        .unwrap_or(true);
                    let _ = trigger;
                    let _ = tpos;
                    zone_match
                })
                .min_by(|(_, a_pos, _), (_, b_pos, _)| {
                    let da = (a_pos.x - player_pos.x).powi(2) + (a_pos.y - player_pos.y).powi(2);
                    let db = (b_pos.x - player_pos.x).powi(2) + (b_pos.y - player_pos.y).powi(2);
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(trigger, _, _)| trigger.portal_id);

            nearest.ok_or_else(|| BrpError::internal("no portal triggers found in current zone"))?
        }
    };

    // Look up to_zone for the return value before writing the message.
    let to_zone = {
        let topology = world.get_resource::<ZoneTopology>()
            .ok_or_else(|| BrpError::internal("ZoneTopology resource not found"))?;
        topology.portals.iter()
            .find(|p| p.id == portal_id)
            .map(|p| p.to_zone.0)
            .unwrap_or(0)
    };

    world.write_message(PlayerZoneTransition { entity: player_entity, portal_id });
    tracing::info!(portal_id, to_zone, "DM enter portal triggered for local player");
    Ok(serde_json::json!({ "ok": true, "portal_id": portal_id, "to_zone": to_zone }))
}

/// Set the time of day for the day-night cycle.
///
/// Params: `{ "time": f32 }` — 0.0/1.0 = midnight, 0.25 = dawn, 0.5 = noon, 0.75 = dusk.
/// Returns `{ "ok": true, "time": f32 }`.
#[cfg(not(target_family = "wasm"))]
fn dm_set_time_of_day(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    let time = params
        .as_ref()
        .and_then(|p| p.get("time"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| BrpError::internal("missing required param `time`"))? as f32;

    let time = time.fract().abs(); // wrap to [0.0, 1.0)
    world.resource_mut::<plugins::scene_lighting::TimeOfDay>().0 = time;
    tracing::info!(time, "DM set time of day");
    Ok(serde_json::json!({ "ok": true, "time": time }))
}

/// Toggle free-orbit mode on the camera.
///
/// Params: `{ "free": bool }` (defaults to `false` if omitted)
/// Returns `{ "free": bool }`.
#[cfg(not(target_family = "wasm"))]
fn dm_set_camera_free(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    let free = params
        .as_ref()
        .and_then(|p| p.get("free"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut q = world.query::<&mut plugins::camera::OrbitCamera>();
    let mut cam = q.single_mut(world)
        .map_err(|_| BrpError::internal("no OrbitCamera entity found"))?;
    cam.free_orbit = free;
    tracing::info!(free, "DM camera free_orbit set");
    Ok(serde_json::json!({ "free": free }))
}

/// Toggle avian physics debug rendering (collider wireframes).
///
/// Params: `{ "enabled": bool }` — omit to flip the current state.
/// Returns `{ "ok": true, "enabled": bool }`.
#[cfg(not(target_family = "wasm"))]
fn dm_toggle_physics_debug(
    In(params): In<Option<serde_json::Value>>,
    world: &mut World,
) -> BrpResult {
    let mut store = world.resource_mut::<GizmoConfigStore>();
    let (config, _) = store.config_mut::<avian3d::debug_render::PhysicsGizmos>();
    let enabled = params
        .as_ref()
        .and_then(|p| p.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(!config.enabled);
    config.enabled = enabled;
    tracing::info!(enabled, "DM physics debug rendering toggled");
    Ok(serde_json::json!({ "ok": true, "enabled": enabled }))
}
