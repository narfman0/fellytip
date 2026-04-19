use bevy::prelude::*;
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use core::time::Duration;
use fellytip_server::{parse_arg, plugins, ServerGamePlugin};
use fellytip_shared::{
    TICK_HZ, WORLD_SEED,
    protocol::FellytipProtocolPlugin,
    world::map::{MAP_HEIGHT, MAP_WIDTH},
};
use lightyear::prelude::server::*;

/// BRP HTTP port for the server (used by ralph scenarios and tooling).
const BRP_PORT: u16 = 15702;

/// Subset of startup constants that can be overridden per-developer via
/// `server.local.toml` (gitignored). CLI flags take precedence over this file;
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

    let local: LocalConfig = std::fs::read_to_string("server.local.toml")
        .ok()
        .and_then(|s| match toml::from_str(&s) {
            Ok(cfg) => { tracing::info!("Loaded server.local.toml"); Some(cfg) }
            Err(e)  => { tracing::warn!("server.local.toml parse error: {e}"); None }
        })
        .unwrap_or_default();

    let map_seed           = parse_arg(&args, "--seed",               local.map_seed.unwrap_or(WORLD_SEED));
    let map_width          = parse_arg(&args, "--map-width",          local.map_width.unwrap_or(MAP_WIDTH));
    let map_height         = parse_arg(&args, "--map-height",         local.map_height.unwrap_or(MAP_HEIGHT));
    let idle_secs: f32     = parse_arg(&args, "--idle-secs",          local.idle_secs.unwrap_or(300.0));
    let history_warp_ticks = parse_arg(&args, "--history-warp-ticks", local.history_warp_ticks.unwrap_or(10));
    let npcs_per_faction   = parse_arg(&args, "--npcs-per-faction",   local.npcs_per_faction.unwrap_or(3));

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
        .add_plugins(FellytipProtocolPlugin);

    if combat_test {
        // Minimal test world: two hostile NPCs, no map gen, no lightyear socket.
        // ralph `combat_resolves` scenario passes without a headless client.
        tracing::info!("Starting in combat-test mode — minimal world, no clients required");
        app.add_plugins(plugins::persistence::PersistencePlugin)
           .add_plugins(plugins::world_sim::WorldSimPlugin)
           .add_plugins(plugins::story::StoryPlugin)
           .add_plugins(plugins::combat::CombatPlugin)
           .add_plugins(plugins::combat_test::CombatTestPlugin);
    } else {
        app.add_plugins(ServerGamePlugin {
            seed:              map_seed,
            width:             map_width,
            height:            map_height,
            history_warp_ticks,
            npcs_per_faction,
            idle_secs: if no_idle_shutdown { None } else { Some(idle_secs) },
        });
        if no_idle_shutdown {
            tracing::info!("Idle-shutdown disabled (--no-idle-shutdown)");
        } else {
            tracing::info!(idle_secs, "Idle-shutdown enabled");
        }
    }

    app.run();
}
