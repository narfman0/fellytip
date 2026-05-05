use bevy::prelude::*;
use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};
use fellytip_game::{MapGenConfig, ServerGamePlugin};
use fellytip_shared::{utils::parse_arg, protocol::FellytipProtocolPlugin};
use fellytip_server::plugins;
use fellytip_game::plugins::bot;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: u64    = parse_arg(&args, "--seed",               42);
    let width: usize = parse_arg(&args, "--map-width",          256);
    let height: usize= parse_arg(&args, "--map-height",         256);
    let warp: u64    = parse_arg(&args, "--history-warp-ticks", 0);
    let npcs: usize  = parse_arg(&args, "--npcs-per-faction",   10);
    let port: u16    = parse_arg(&args, "--port",               15702);

    App::new()
        .add_plugins(MinimalPlugins)
        .add_plugins(bevy::log::LogPlugin::default())
        .add_plugins(FellytipProtocolPlugin)
        .add_plugins(
            RemotePlugin::default()
                .with_method("dm/spawn_npc",               plugins::dm::dm_spawn_npc)
                .with_method("dm/spawn_wildlife",          plugins::dm::dm_spawn_wildlife)
                .with_method("dm/kill",                    plugins::dm::dm_kill)
                .with_method("dm/teleport",                plugins::dm::dm_teleport)
                .with_method("dm/set_faction",             plugins::dm::dm_set_faction)
                .with_method("dm/trigger_war_party",       plugins::dm::dm_trigger_war_party)
                .with_method("dm/set_ecology",             plugins::dm::dm_set_ecology)
                .with_method("dm/battle_history",          plugins::dm::dm_battle_history)
                .with_method("dm/clear_battle_history",    plugins::dm::dm_clear_battle_history)
                .with_method("dm/underground_pressure",    plugins::dm::dm_underground_pressure)
                .with_method("dm/force_pressure",         plugins::dm::dm_force_underground_pressure)
                .with_method("dm/query_portals",           plugins::dm::dm_query_portals)
                .with_method("dm/list_settlements",        plugins::dm::dm_list_settlements)
                .with_method("dm/move_entity",             plugins::dm::dm_move_entity)
                .with_method("dm/spawn_raid",              plugins::dm::dm_spawn_raid)
                .with_method("dm/give_gold",               plugins::dm::dm_give_gold)
                .with_method("dm/spawn_bot",               bot::dm_spawn_bot)
                .with_method("dm/despawn_bot",             bot::dm_despawn_bot)
                .with_method("dm/list_bots",               bot::dm_list_bots)
                .with_method("dm/set_bot_action",          bot::dm_set_bot_action)
        )
        .add_plugins(RemoteHttpPlugin::default().with_port(port))
        .add_plugins(ServerGamePlugin {
            seed,
            width,
            height,
            history_warp_ticks: warp,
            npcs_per_faction: npcs,
            combat_test: false,
        })
        .run();
}
