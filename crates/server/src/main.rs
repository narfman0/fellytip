mod plugins;

use bevy::prelude::*;
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use core::time::Duration;
use fellytip_shared::{
    WORLD_SEED, NET_PORT, PRIVATE_KEY, PROTOCOL_ID, TICK_HZ,
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{Experience, Health, WorldPosition},
    protocol::FellytipProtocolPlugin,
    world::{
        map::{find_surface_spawn, WorldMap, MAP_WIDTH, MAP_HEIGHT},
        story::GameEntityId,
    },
};

use plugins::map_gen::MapGenConfig;
use lightyear::prelude::{server::*, *};
use std::net::SocketAddr;
use uuid::Uuid;

use plugins::combat::{CombatParticipant, PlayerEntity};
use plugins::persistence::Db;

/// BRP HTTP port for the server (used by ralph scenarios and tooling).
const BRP_PORT: u16 = 15702;

fn main() {
    tracing_subscriber::fmt::init();
    let args: Vec<String> = std::env::args().collect();
    let combat_test = args.iter().any(|a| a == "--combat-test");

    // Parse map gen CLI args (only meaningful outside combat-test mode).
    let map_seed   = parse_arg(&args, "--seed",       WORLD_SEED);
    let map_width  = parse_arg(&args, "--map-width",  MAP_WIDTH);
    let map_height = parse_arg(&args, "--map-height", MAP_HEIGHT);

    // Plugins shared by all run modes.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RemotePlugin::default())
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
        app.insert_resource(MapGenConfig { seed: map_seed, width: map_width, height: map_height })
            .add_plugins(plugins::map_gen::MapGenPlugin)
            .add_plugins(plugins::ecology::EcologyPlugin)
            .add_plugins(plugins::ai::AiPlugin)
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

/// When a client disconnects, save its player's current state to SQLite.
fn on_client_disconnected(
    trigger: On<Add, Disconnected>,
    client_q: Query<&PlayerEntity>,
    player_q: Query<(&CombatParticipant, &WorldPosition, &Health, &Experience)>,
    db: Res<Db>,
) {
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
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_err() {
        return;
    }
    let (spawn_x, spawn_y, spawn_z) = map
        .as_deref()
        .map(find_surface_spawn)
        .unwrap_or((0.0, 0.0, 0.0));
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
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    commands.entity(trigger.entity).insert(PlayerEntity(player));
    tracing::info!("Client {:?} connected → player entity {:?}", trigger.entity, player);
}
