pub mod plugins;

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use fellytip_shared::{
    WORLD_SEED,
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}, SpellSlots, Spellbook},
    components::{AbilityModifiers, AbilityScores, ActionBudget, Experience, Health, HitDice, PlayerStandings, SavingThrowProficiencies, WorldMeta, WorldPosition},
    protocol::ChooseClassMessage,
    world::{
        map::{find_surface_spawn, WorldMap, MAP_HEIGHT, MAP_WIDTH},
        story::GameEntityId,
        zone::{ZoneMembership, OVERWORLD_ZONE},
    },
};
use uuid::Uuid;

use plugins::character_persistence::{load_character, load_local_player_uuid, save_character, store_local_player_uuid};
use plugins::combat::{ActionCooldowns, CombatParticipant, LastPlayerInput, PositionSanityTimer};
use plugins::persistence::Db;
pub use plugins::map_gen::MapGenConfig;

/// Parse `--flag value` from the arg list, returning `default` if not found.
pub fn parse_arg<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    args.windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(default)
}

/// Bundles all server-side game logic plugins.
///
/// Does NOT add networking (ServerPlugins/ClientPlugins/FellytipProtocolPlugin)
/// — callers add those separately for multiplayer builds.
///
/// When `combat_test` is true, skips map gen, ecology, AI, and dungeon plugins
/// and adds `CombatTestPlugin` instead for a minimal two-entity combat world.
///
/// MULTIPLAYER: restore ServerPlugins, spawn_server, on_client_connected,
/// on_client_disconnected, on_link_spawned, send_greet_msg, and idle_shutdown.
pub struct ServerGamePlugin {
    pub seed: u64,
    pub width: usize,
    pub height: usize,
    pub history_warp_ticks: u64,
    pub npcs_per_faction: usize,
    pub combat_test: bool,
}

impl Plugin for ServerGamePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<fellytip_shared::components::HitDice>()
           .register_type::<fellytip_shared::components::AbilityModifiers>();

        app.insert_resource(MapGenConfig {
                seed: self.seed,
                width: self.width,
                height: self.height,
                history_warp_ticks: self.history_warp_ticks,
                npcs_per_faction: self.npcs_per_faction,
            })
            .add_plugins(plugins::persistence::PersistencePlugin)
            .add_plugins(plugins::character_persistence::CharacterPersistencePlugin)
            .add_plugins(plugins::world_sim::WorldSimPlugin)
            .add_plugins(plugins::perf::PerfPlugin)
            .add_plugins(plugins::story::StoryPlugin)
            .add_plugins(plugins::combat::CombatPlugin)
            .add_plugins(plugins::interest::InterestPlugin)
            .add_plugins(plugins::party::PartyPlugin)
            .add_plugins(plugins::portal::PortalPlugin)
            .add_systems(Update, fellytip_shared::components::sync_ability_modifiers);

        if self.combat_test {
            app.add_plugins(plugins::combat_test::CombatTestPlugin);
        } else {
            app.add_plugins(plugins::map_gen::MapGenPlugin)
                .add_plugins(plugins::ecology::EcologyPlugin)
                .add_plugins(plugins::ai::AiPlugin)
                .add_plugins(plugins::dungeon::DungeonPlugin)
                .add_systems(Startup, (plugins::ai::seed_factions, plugins::ai::population::seed_faction_relations).chain())
                // Player is now spawned in response to ChooseClassMessage (Update),
                // not at PostStartup, so the class selection screen is respected.
                .add_systems(Update, spawn_player_on_class_choice);
        }
    }
}

/// Stats derived from a chosen class (SRD standard array).
///
/// Player classes (Warrior / Rogue / Mage) use curated standard arrays.
/// NPC-only classes fall back to `AbilityScores::for_class` with Grunt rank,
/// which should not normally be reached for players but is handled for safety.
fn class_stats(class: &CharacterClass) -> (i32, i32, i32, i32, i32, i32, i32, AbilityScores, SavingThrowProficiencies) {
    use fellytip_shared::world::faction::NpcRank;
    // Returns (hp_max, str, dex, con, int, wis, cha, ability_scores, saves)
    match class {
        CharacterClass::Warrior => {
            let ab = AbilityScores::warrior();
            let saves = SavingThrowProficiencies::warrior();
            (10 + AbilityScores::modifier(ab.constitution) as i32,
             ab.strength as i32, ab.dexterity as i32, ab.constitution as i32,
             ab.intelligence as i32, ab.wisdom as i32, ab.charisma as i32,
             ab, saves)
        }
        CharacterClass::Rogue => {
            let ab = AbilityScores::rogue();
            let saves = SavingThrowProficiencies::rogue();
            (8 + AbilityScores::modifier(ab.constitution) as i32,
             ab.strength as i32, ab.dexterity as i32, ab.constitution as i32,
             ab.intelligence as i32, ab.wisdom as i32, ab.charisma as i32,
             ab, saves)
        }
        CharacterClass::Mage => {
            let ab = AbilityScores::mage();
            let saves = SavingThrowProficiencies::mage();
            (6 + AbilityScores::modifier(ab.constitution) as i32,
             ab.strength as i32, ab.dexterity as i32, ab.constitution as i32,
             ab.intelligence as i32, ab.wisdom as i32, ab.charisma as i32,
             ab, saves)
        }
        // NPC classes — not normally used for players but handled for completeness.
        other => {
            use fellytip_shared::combat::types::hit_die_for_class;
            let ab = AbilityScores::for_class(other, NpcRank::Grunt);
            let saves = SavingThrowProficiencies::for_class(other);
            let hp_max = hit_die_for_class(other) + AbilityScores::modifier(ab.constitution) as i32;
            (hp_max,
             ab.strength as i32, ab.dexterity as i32, ab.constitution as i32,
             ab.intelligence as i32, ab.wisdom as i32, ab.charisma as i32,
             ab, saves)
        }
    }
}

/// Listen for `ChooseClassMessage` from the client and spawn the player entity.
///
/// If a saved character exists in the DB, class/level/HP/XP/position are
/// restored from the DB row and the chosen class is ignored.
///
/// Runs every Update frame but does nothing until the message arrives.
/// Guard: only spawns if no player entity exists yet (Without<Experience>).
fn spawn_player_on_class_choice(
    mut reader: MessageReader<ChooseClassMessage>,
    map: Option<Res<WorldMap>>,
    map_config: Option<Res<MapGenConfig>>,
    db: Res<Db>,
    // Only allow spawning when there is no player entity yet.
    existing_players: Query<Entity, With<Experience>>,
    mut commands: Commands,
) {
    let msg = reader.read().next();
    let Some(msg) = msg else { return };

    // Prevent double-spawning if the message fires again somehow.
    if !existing_players.is_empty() {
        tracing::warn!("ChooseClassMessage received but player already exists — ignoring");
        return;
    }

    let (spawn_x, spawn_y, spawn_z) = map
        .as_deref()
        .and_then(|m| {
            if m.spawn_points.is_empty() { return None; }
            Some(m.spawn_points[0])
        })
        .or_else(|| map.as_deref().map(find_surface_spawn))
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

    // Look up the persisted local player UUID from world_meta so that the same
    // character is reloaded across server restarts.  If none exists, generate a
    // new one and store it.
    let player_uuid = load_local_player_uuid(&db)
        .unwrap_or_else(|| {
            let uuid = Uuid::new_v4();
            store_local_player_uuid(&db, uuid);
            uuid
        });

    // Try to restore from DB first.
    let saved = load_character(&db, player_uuid);

    let (class, level, xp_val, xp_to_next_val, hp_current, hp_max, px, py, pz) =
        if let Some(row) = saved {
            tracing::info!(uuid = %player_uuid, class = ?row.class, "Restoring player from DB");
            (row.class, row.level, row.xp, row.xp_to_next,
             row.health_current, row.health_max, row.pos_x, row.pos_y, row.pos_z)
        } else {
            let chosen_class = msg.class.clone();
            let (hp_max, _, _, _, _, _, _, _, _) = class_stats(&chosen_class);
            tracing::info!(uuid = %player_uuid, class = ?chosen_class, "Spawning new player");
            // Insert initial DB row right away so future restarts restore state.
            save_character(&db, player_uuid, &chosen_class, 1, 0, 300,
                           hp_max, hp_max, spawn_x, spawn_y, spawn_z);
            (chosen_class, 1u32, 0u32, 300u32, hp_max, hp_max, spawn_x, spawn_y, spawn_z)
        };

    let (_, str_val, dex_val, con_val, int_val, wis_val, cha_val, ability_scores, saves) =
        class_stats(&class);

    let spell_slots = SpellSlots::for_class(&class, level as u8);
    let spellbook   = Spellbook::for_class(&class);
    let hit_dice    = HitDice::for_class_level(&class, level);
    let ability_modifiers = AbilityModifiers::from_scores(&ability_scores);

    commands.spawn((
        WorldPosition { x: px, y: py, z: pz },
        ZoneMembership(OVERWORLD_ZONE),
        Health { current: hp_current, max: hp_max },
        CombatParticipant {
            id: CombatantId(player_uuid),
            interrupt_stack: InterruptStack::default(),
            class,
            level,
            armor_class: 13,
            strength:     str_val,
            dexterity:    dex_val,
            constitution: con_val,
            intelligence: int_val,
            wisdom:       wis_val,
            charisma:     cha_val,
        },
        ability_modifiers,
        hit_dice,
        ability_scores,
        saves,
        GameEntityId(player_uuid),
        Experience { xp: xp_val, level, xp_to_next: xp_to_next_val },
        PlayerStandings::default(),
        LastPlayerInput::default(),
        PositionSanityTimer {
            last_valid_x: px,
            last_valid_y: py,
            last_valid_z: pz,
            ..default()
        },
        (world_meta, spell_slots, spellbook),
        (ActionBudget::default(), ActionCooldowns::default()),
    ));
    tracing::info!(uuid = %player_uuid, x = px, y = py, z = pz, "Player spawned after class selection");
}
