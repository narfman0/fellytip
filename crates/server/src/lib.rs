pub mod plugins;

use bevy::prelude::*;
use core::time::Duration;
use fellytip_shared::{
    WORLD_SEED, NET_PORT, WS_PORT, PRIVATE_KEY, PROTOCOL_ID,
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{Experience, Health, PlayerStandings, WorldMeta, WorldPosition},
    protocol::{GreetMsg, WorldStateChannel},
    world::{
        faction::PlayerReputationMap,
        map::{find_surface_spawn, WorldMap, MAP_HEIGHT, MAP_WIDTH},
        story::{GameEntityId, StoryLog},
    },
};
use lightyear::prelude::{server::*, *};
use std::net::SocketAddr;
use uuid::Uuid;

use plugins::combat::{CombatParticipant, LastPlayerInput, PlayerEntity, PositionSanityTimer};
use plugins::ecology::EcologyState;
use plugins::persistence::Db;
pub use plugins::map_gen::MapGenConfig;

/// Parse `--flag value` from the arg list, returning `default` if not found.
pub fn parse_arg<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    args.windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(default)
}

#[derive(Resource, Default)]
pub struct ConnectedCount {
    pub current: u32,
    pub ever_connected: bool,
}

#[derive(Resource)]
pub struct IdleTimer(pub Timer);

/// Bundles all server-side game logic plugins and networking systems.
///
/// Does NOT add `ServerPlugins` or `FellytipProtocolPlugin` — callers are
/// responsible for those so that dedicated-server and host-mode can each
/// configure them appropriately.
pub struct ServerGamePlugin {
    pub seed: u64,
    pub width: usize,
    pub height: usize,
    pub history_warp_ticks: u64,
    pub npcs_per_faction: usize,
    /// `Some(secs)` enables idle-shutdown after all clients disconnect.
    /// `None` disables it (host mode: the local player keeps the app alive).
    pub idle_secs: Option<f32>,
}

impl Plugin for ServerGamePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(MapGenConfig {
                seed: self.seed,
                width: self.width,
                height: self.height,
                history_warp_ticks: self.history_warp_ticks,
                npcs_per_faction: self.npcs_per_faction,
            })
            .insert_resource(ConnectedCount::default())
            .add_plugins(plugins::persistence::PersistencePlugin)
            .add_plugins(plugins::world_sim::WorldSimPlugin)
            .add_plugins(plugins::story::StoryPlugin)
            .add_plugins(plugins::combat::CombatPlugin)
            .add_plugins(plugins::map_gen::MapGenPlugin)
            .add_plugins(plugins::ecology::EcologyPlugin)
            .add_plugins(plugins::ai::AiPlugin)
            .add_plugins(plugins::interest::InterestPlugin)
            .add_plugins(plugins::party::PartyPlugin)
            .add_plugins(plugins::dungeon::DungeonPlugin)
            .add_systems(Startup, plugins::ai::seed_factions)
            .add_systems(PostStartup, spawn_server)
            .add_systems(Update, send_greet_msg)
            .add_observer(on_link_spawned)
            .add_observer(on_client_connected)
            .add_observer(on_client_disconnected);

        if let Some(idle_secs) = self.idle_secs {
            app.insert_resource(IdleTimer(Timer::from_seconds(idle_secs, TimerMode::Once)))
               .add_systems(Update, idle_shutdown);
        }
    }
}

pub fn spawn_server(mut commands: Commands) {
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

fn on_client_disconnected(
    trigger:  On<Add, Disconnected>,
    client_q: Query<&PlayerEntity>,
    player_q: Query<(&CombatParticipant, &WorldPosition, &Health, &Experience)>,
    rep:      Res<PlayerReputationMap>,
    mut count: ResMut<ConnectedCount>,
    db:       Res<Db>,
) {
    count.current = count.current.saturating_sub(1);

    let Ok(PlayerEntity(player_entity)) = client_q.get(trigger.entity) else { return };
    let Ok((participant, pos, health, exp)) = player_q.get(*player_entity) else { return };

    let player_uuid = participant.id.0;
    let player_id   = player_uuid.to_string();
    let level       = exp.level as i64;
    let hp_current  = health.current as i64;
    let hp_max      = health.max as i64;
    let pos_x       = pos.x as f64;
    let pos_y       = pos.y as f64;
    let last_seen   = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let name  = player_id.clone();
    let class = "Warrior";

    let faction_standings: Vec<(String, i32)> = rep.0
        .get(&player_uuid)
        .map(|m| m.iter().map(|(k, &v)| (k.0.to_string(), v)).collect())
        .unwrap_or_default();

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
        .bind(&player_id).bind(&name).bind(class)
        .bind(level).bind(hp_current).bind(hp_max)
        .bind(pos_x).bind(pos_y).bind(last_seen)
        .execute(&pool)
        .await;

        match res {
            Ok(_)  => tracing::info!(player = %player_id, "Player state saved on disconnect"),
            Err(e) => tracing::warn!(player = %player_id, "Player save failed: {e}"),
        }

        for (faction_id, score) in &faction_standings {
            let res = sqlx::query(
                "INSERT OR REPLACE INTO player_faction_standing \
                 (player_id, faction_id, score) VALUES (?, ?, ?)",
            )
            .bind(&player_id).bind(faction_id).bind(*score)
            .execute(&pool)
            .await;
            if let Err(e) = res {
                tracing::warn!(player = %player_id, faction = %faction_id, "Standing save failed: {e}");
            }
        }
        if !faction_standings.is_empty() {
            tracing::info!(player = %player_id, count = faction_standings.len(), "Faction standings saved");
        }
    });
}

fn on_client_connected(
    trigger:    On<Add, Connected>,
    query:      Query<(), With<ClientOf>>,
    map:        Option<Res<WorldMap>>,
    map_config: Option<Res<MapGenConfig>>,
    mut count:  ResMut<ConnectedCount>,
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
            Health { current: 100, max: 100 },
            CombatParticipant {
                id: CombatantId(player_uuid),
                interrupt_stack: InterruptStack::default(),
                class: CharacterClass::Warrior,
                level: 1,
                armor_class: 13,
                strength: 12,
                dexterity: 14,
                constitution: 12,
            },
            GameEntityId(player_uuid),
            Experience::new(),
            PlayerStandings::default(),
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

type NewClientQuery<'w, 's> =
    Query<'w, 's, (&'static PlayerEntity, &'static RemoteId), (Added<PlayerEntity>, With<ClientOf>)>;

fn send_greet_msg(
    new_clients: NewClientQuery,
    player_q:   Query<&GameEntityId>,
    server:     Single<&Server>,
    mut msg_sender: ServerMultiMessageSender,
) {
    for (player_entity, remote_id) in &new_clients {
        let Ok(geid) = player_q.get(player_entity.0) else { continue };
        let _ = msg_sender.send::<GreetMsg, WorldStateChannel>(
            &GreetMsg { message: "Welcome!".into(), player_id: geid.0 },
            &server,
            &NetworkTarget::Single(remote_id.0),
        );
        tracing::info!("Sent GreetMsg to client {:?} with player UUID {}", remote_id.0, geid.0);
    }
}

fn idle_shutdown(
    time:      Res<Time>,
    count:     Res<ConnectedCount>,
    mut timer: ResMut<IdleTimer>,
    mut story: ResMut<StoryLog>,
    ecology:   Option<Res<EcologyState>>,
    db:        Res<Db>,
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
