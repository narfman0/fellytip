mod plugins;

use bevy::prelude::*;
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use core::time::Duration;
use fellytip_shared::{
    WORLD_SEED, NET_PORT, WS_PORT, PRIVATE_KEY, PROTOCOL_ID, TICK_HZ,
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{Experience, Health, WorldMeta, WorldPosition},
    protocol::FellytipProtocolPlugin,
    world::{
        map::{find_surface_spawn, WorldMap, MAP_WIDTH, MAP_HEIGHT},
        story::{GameEntityId, StoryLog},
    },
};

use plugins::ecology::EcologyState;
use plugins::map_gen::MapGenConfig;
use lightyear::prelude::{server::*, *};
use std::net::SocketAddr;
use uuid::Uuid;

use plugins::combat::{CombatParticipant, LastPlayerInput, PlayerEntity, PositionSanityTimer};
use plugins::persistence::Db;

/// BRP HTTP port for the server (used by ralph scenarios and tooling).
const BRP_PORT: u16 = 15702;

/// Tracks how many clients are currently connected and whether any have ever
/// connected.  Used by the idle-shutdown system.
#[derive(Resource, Default)]
struct ConnectedCount {
    /// Number of currently-connected clients.
    current: u32,
    /// True once the first client has connected since server start.
    /// Prevents idle-shutdown from firing on a server that nobody ever joined.
    ever_connected: bool,
}

/// One-shot timer driving the idle-shutdown grace period.
/// Ticks only while `ConnectedCount::current == 0` and `ever_connected == true`.
/// Reset to zero whenever a client is online.
#[derive(Resource)]
struct IdleTimer(Timer);

/// Subset of startup constants that can be overridden per-developer via
/// `server.local.toml` (gitignored).  CLI flags take precedence over this file;
/// this file takes precedence over the hardcoded defaults.
#[derive(serde::Deserialize, Default)]
struct LocalConfig {
    history_warp_ticks: Option<u64>,
    npcs_per_faction:   Option<usize>,
    map_seed:           Option<u64>,
    map_width:          Option<usize>,
    map_height:         Option<usize>,
    idle_secs:          Option<f32>,
}

fn main() {
    tracing_subscriber::fmt::init();
    let args: Vec<String> = std::env::args().collect();
    let combat_test      = args.iter().any(|a| a == "--combat-test");
    let no_idle_shutdown = args.iter().any(|a| a == "--no-idle-shutdown");

    // Load optional local dev overrides (gitignored; CLI flags still win).
    let local: LocalConfig = std::fs::read_to_string("server.local.toml")
        .ok()
        .and_then(|s| match toml::from_str(&s) {
            Ok(cfg) => { tracing::info!("Loaded server.local.toml"); Some(cfg) }
            Err(e)  => { tracing::warn!("server.local.toml parse error: {e}"); None }
        })
        .unwrap_or_default();

    // Parse map gen CLI args (only meaningful outside combat-test mode).
    // Priority: hardcoded default < server.local.toml < CLI flag.
    let map_seed           = parse_arg(&args, "--seed",                local.map_seed.unwrap_or(WORLD_SEED));
    let map_width          = parse_arg(&args, "--map-width",           local.map_width.unwrap_or(MAP_WIDTH));
    let map_height         = parse_arg(&args, "--map-height",          local.map_height.unwrap_or(MAP_HEIGHT));
    let idle_secs: f32     = parse_arg(&args, "--idle-secs",           local.idle_secs.unwrap_or(300.0));
    let history_warp_ticks = parse_arg(&args, "--history-warp-ticks",  local.history_warp_ticks.unwrap_or(10));
    let npcs_per_faction   = parse_arg(&args, "--npcs-per-faction",    local.npcs_per_faction.unwrap_or(3));

    // Plugins shared by all run modes.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(
            RemotePlugin::default()
                .with_method("dm/spawn_npc",         plugins::dm::dm_spawn_npc)
                .with_method("dm/kill",              plugins::dm::dm_kill)
                .with_method("dm/teleport",          plugins::dm::dm_teleport)
                .with_method("dm/set_faction",       plugins::dm::dm_set_faction)
                .with_method("dm/trigger_war_party", plugins::dm::dm_trigger_war_party)
                .with_method("dm/set_ecology",       plugins::dm::dm_set_ecology),
        )
        .add_plugins(RemoteHttpPlugin::default().with_port(BRP_PORT))
        .add_plugins(ServerPlugins {
            tick_duration: Duration::from_secs_f64(1.0 / TICK_HZ),
        })
        .add_plugins(FellytipProtocolPlugin)
        .add_plugins(plugins::persistence::PersistencePlugin)
        .add_plugins(plugins::world_sim::WorldSimPlugin)
        .add_plugins(plugins::story::StoryPlugin)
        .add_plugins(plugins::combat::CombatPlugin);

    if combat_test {
        // Minimal test world: two hostile NPCs, no map gen, no lightyear socket.
        // ralph `combat_resolves` scenario passes without a headless client.
        tracing::info!("Starting in combat-test mode — minimal world, no clients required");
        app.add_plugins(plugins::combat_test::CombatTestPlugin);
    } else {
        // Full game world with map gen, ecology, factions, and live networking.
        // Insert MapGenConfig before MapGenPlugin so it can read it.
        app.insert_resource(MapGenConfig {
                seed: map_seed, width: map_width, height: map_height,
                history_warp_ticks, npcs_per_faction,
            })
            .insert_resource(ConnectedCount::default())
            .insert_resource(IdleTimer(Timer::from_seconds(idle_secs, TimerMode::Once)))
            .add_plugins(plugins::map_gen::MapGenPlugin)
            .add_plugins(plugins::ecology::EcologyPlugin)
            .add_plugins(plugins::ai::AiPlugin)
            .add_plugins(plugins::interest::InterestPlugin)
            .add_plugins(plugins::party::PartyPlugin)
            .add_plugins(plugins::dungeon::DungeonPlugin)
            .add_systems(Startup, plugins::ai::seed_factions)
            // spawn_server runs in PostStartup so its deferred command application
            // (which triggers the lightyear UDP observer) cannot share an ApplyDeferred
            // sync point with the map-gen chain.  If the observer panics (e.g. port in
            // use), it no longer corrupts generate_world's Commands flush and the
            // WorldMap resource is always present for seed_ecology.
            .add_systems(PostStartup, spawn_server)
            .add_observer(on_link_spawned)
            .add_observer(on_client_connected)
            .add_observer(on_client_disconnected);

        if !no_idle_shutdown {
            app.add_systems(Update, idle_shutdown);
            tracing::info!(idle_secs, "Idle-shutdown enabled");
        } else {
            tracing::info!("Idle-shutdown disabled (--no-idle-shutdown)");
        }
    }

    app.run();
}

/// Parse `--flag value` from the arg list, returning `default` if not found.
fn parse_arg<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    args.windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(default)
}

fn spawn_server(mut commands: Commands) {
    // UDP socket for native clients.
    let udp_addr: SocketAddr = format!("0.0.0.0:{NET_PORT}").parse().unwrap();
    let udp_e = commands
        .spawn((
            ServerUdpIo::default(),
            LocalAddr(udp_addr),
            NetcodeServer::new(
                NetcodeConfig::default()
                    .with_protocol_id(PROTOCOL_ID)
                    .with_key(PRIVATE_KEY),
            ),
        ))
        .id();
    commands.entity(udp_e).trigger(|entity| Start { entity });
    tracing::info!("Server UDP listening on {udp_addr}");

    // WebSocket socket for browser (WASM) clients.
    let ws_addr: SocketAddr = format!("0.0.0.0:{WS_PORT}").parse().unwrap();
    let ws_e = commands
        .spawn((
            WebSocketServerIo {
                config: ServerConfig::builder()
                    .with_bind_default(WS_PORT)
                    .with_no_encryption(),
            },
            LocalAddr(ws_addr),
            NetcodeServer::new(
                NetcodeConfig::default()
                    .with_protocol_id(PROTOCOL_ID)
                    .with_key(PRIVATE_KEY),
            ),
        ))
        .id();
    commands.entity(ws_e).trigger(|entity| Start { entity });
    tracing::info!("Server WebSocket listening on {ws_addr}");
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

/// When a client disconnects, save its player's current state to SQLite.
fn on_client_disconnected(
    trigger:  On<Add, Disconnected>,
    client_q: Query<&PlayerEntity>,
    player_q: Query<(&CombatParticipant, &WorldPosition, &Health, &Experience)>,
    mut count: ResMut<ConnectedCount>,
    db: Res<Db>,
) {
    count.current = count.current.saturating_sub(1);

    let Ok(PlayerEntity(player_entity)) = client_q.get(trigger.entity) else {
        return;
    };
    let Ok((participant, pos, health, exp)) = player_q.get(*player_entity) else {
        return;
    };

    let player_id  = participant.id.0.to_string();
    let level      = exp.level as i64;
    let hp_current = health.current as i64;
    let hp_max     = health.max as i64;
    let pos_x      = pos.x as f64;
    let pos_y      = pos.y as f64;
    let last_seen  = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Use the UUID as a placeholder name until the name system is implemented.
    let name  = player_id.clone();
    let class = "Warrior";

    let pool = db.pool().clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for player save");

    rt.block_on(async move {
        let res = sqlx::query(
            "INSERT OR REPLACE INTO players \
             (id, name, class, level, health_current, health_max, pos_x, pos_y, last_seen) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&player_id)
        .bind(&name)
        .bind(class)
        .bind(level)
        .bind(hp_current)
        .bind(hp_max)
        .bind(pos_x)
        .bind(pos_y)
        .bind(last_seen)
        .execute(&pool)
        .await;

        match res {
            Ok(_) => tracing::info!(player = %player_id, "Player state saved on disconnect"),
            Err(e) => tracing::warn!(player = %player_id, "Player save failed: {e}"),
        }
    });
}

/// When the netcode handshake completes, spawn a player entity and link it to
/// the `ClientOf` entity so the input system can find it.
fn on_client_connected(
    trigger: On<Add, Connected>,
    query: Query<(), With<ClientOf>>,
    map: Option<Res<WorldMap>>,
    map_config: Option<Res<MapGenConfig>>,
    mut count: ResMut<ConnectedCount>,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_err() {
        return;
    }
    count.current += 1;
    count.ever_connected = true;

    let (spawn_x, spawn_y, spawn_z) = map
        .as_deref()
        .map(find_surface_spawn)
        .unwrap_or((0.0, 0.0, 0.0));

    // WorldMeta tells the client which seed/dimensions were used so it can
    // regenerate an identical local WorldMap for client-authoritative movement.
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
    let player = commands
        .spawn((
            WorldPosition { x: spawn_x, y: spawn_y, z: spawn_z },
            // Starting HP is generous (100) rather than strict SRD (d10+CON mod)
            // to give players a comfortable introduction to combat.
            Health { current: 100, max: 100 },
            CombatParticipant {
                id: CombatantId(player_uuid),
                interrupt_stack: InterruptStack::default(),
                class: CharacterClass::Warrior,
                level: 1,
                // Leather armour + DEX 14 (+2) = AC 13 (SRD leather: 11 + DEX mod)
                armor_class: 13,
                strength: 12,
                dexterity: 14,
                constitution: 12,
            },
            GameEntityId(player_uuid),
            Experience::new(),
            LastPlayerInput::default(),
            PositionSanityTimer {
                last_valid_x: spawn_x,
                last_valid_y: spawn_y,
                last_valid_z: spawn_z,
                ..default()
            },
            world_meta,
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    commands.entity(trigger.entity).insert(PlayerEntity(player));
    tracing::info!("Client {:?} connected → player entity {:?}", trigger.entity, player);
}

// ── Idle shutdown ─────────────────────────────────────────────────────────────

/// Flush server state and terminate after all clients have been gone for the
/// configured idle period.
///
/// The countdown only begins once at least one client has ever connected, so a
/// bare dedicated server (without `--no-idle-shutdown`) doesn't self-terminate
/// if nobody ever joins.  Resets to zero whenever a client is online.
///
/// Flushes story events and ecology state before terminating so no data is
/// lost.  On the idle-shutdown path all clients will already have disconnected
/// (triggering `on_client_disconnected`), so there are no online players to
/// save.  Uses `std::process::exit` to avoid Bevy/lightyear event-API
/// compatibility issues.
fn idle_shutdown(
    time:          Res<Time>,
    count:         Res<ConnectedCount>,
    mut timer:     ResMut<IdleTimer>,
    mut story:     ResMut<StoryLog>,
    ecology:       Option<Res<EcologyState>>,
    db:            Res<Db>,
) {
    if count.current > 0 {
        timer.0.reset();
        return;
    }
    if !count.ever_connected {
        return;
    }
    if timer.0.tick(time.delta()).just_finished() {
        tracing::info!("All players gone for idle period — flushing and shutting down");
        plugins::story::flush_story_now(&mut story, &db);
        if let Some(eco) = ecology {
            plugins::ecology::flush_ecology_now(&eco, &db);
        }
        tracing::info!("Server state flushed — goodbye");
        std::process::exit(0);
    }
}
