//! Character persistence: save/load player state to/from SQLite.
//!
//! On player spawn the server checks the DB for an existing character row;
//! if found, restores class/level/XP/HP/position instead of using the
//! class selected during class-selection.
//!
//! Autosave runs every ~60 seconds (counted in wall-clock seconds via `Time`).
//! On player despawn (`on_player_despawn`) a final save is performed.
//!
//! The schema lives in `migrations/001_initial.sql` (players table) extended
//! by `migrations/004_player_characters.sql` (xp / xp_to_next / faction_standings).

use bevy::prelude::*;
use fellytip_shared::{
    combat::types::CharacterClass,
    components::{Experience, Health, WorldPosition},
    world::story::GameEntityId,
};
use crate::plugins::{combat::CombatParticipant, persistence::Db};

/// How often (seconds) the autosave runs.
const AUTOSAVE_INTERVAL: f32 = 60.0;

/// Resource tracking time until next autosave.
#[derive(Resource)]
pub struct AutosaveTimer(pub f32);

impl Default for AutosaveTimer {
    fn default() -> Self {
        Self(AUTOSAVE_INTERVAL)
    }
}

pub struct CharacterPersistencePlugin;

impl Plugin for CharacterPersistencePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AutosaveTimer>()
            .add_systems(Update, autosave_players);
    }
}

// ── DB helpers ────────────────────────────────────────────────────────────────

pub fn class_to_str(class: &CharacterClass) -> &'static str {
    match class {
        CharacterClass::Warrior   => "Warrior",
        CharacterClass::Rogue     => "Rogue",
        CharacterClass::Mage      => "Mage",
        CharacterClass::Fighter   => "Fighter",
        CharacterClass::Wizard    => "Wizard",
        CharacterClass::Cleric    => "Cleric",
        CharacterClass::Ranger    => "Ranger",
        CharacterClass::Paladin   => "Paladin",
        CharacterClass::Druid     => "Druid",
        CharacterClass::Bard      => "Bard",
        CharacterClass::Warlock   => "Warlock",
        CharacterClass::Sorcerer  => "Sorcerer",
        CharacterClass::Monk      => "Monk",
        CharacterClass::Barbarian => "Barbarian",
    }
}

pub fn class_from_str(s: &str) -> CharacterClass {
    match s {
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
    }
}

/// Key used in `world_meta` to persist the local player UUID across restarts.
const META_KEY_LOCAL_PLAYER: &str = "local_player_uuid";

/// Load the persisted local player UUID from `world_meta`, if any.
pub fn load_local_player_uuid(db: &Db) -> Option<uuid::Uuid> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        let row: Option<(String,)> =
            sqlx::query_as::<_, (String,)>(
                "SELECT value FROM world_meta WHERE key = ?",
            )
            .bind(META_KEY_LOCAL_PLAYER)
            .fetch_optional(db.pool())
            .await
            .ok()?;
        let (val,) = row?;
        uuid::Uuid::parse_str(&val).ok()
    })
}

/// Persist the local player UUID in `world_meta`.
pub fn store_local_player_uuid(db: &Db, uuid: uuid::Uuid) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to build tokio runtime for store_local_player_uuid: {e}");
            return;
        }
    };
    let val = uuid.to_string();
    rt.block_on(async {
        let res = sqlx::query(
            "INSERT OR REPLACE INTO world_meta (key, value) VALUES (?, ?)",
        )
        .bind(META_KEY_LOCAL_PLAYER)
        .bind(&val)
        .execute(db.pool())
        .await;
        if let Err(e) = res {
            tracing::error!("Failed to store local player UUID: {e}");
        }
    });
}

/// Attempt to load an existing character row from the DB.
///
/// Returns `Some((class, level, xp, xp_to_next, hp_current, hp_max, x, y, z))`
/// if a row exists, `None` otherwise.
pub fn load_character(db: &Db, player_uuid: uuid::Uuid) -> Option<CharacterSaveRow> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;

    rt.block_on(async {
        let id_str = player_uuid.to_string();
        let row = sqlx::query(
            r#"SELECT class, level, xp, xp_to_next, health_current, health_max, pos_x, pos_y, pos_z
               FROM players WHERE id = ?"#,
        )
        .bind(&id_str)
        .fetch_optional(db.pool())
        .await
        .ok()??;

        use sqlx::Row;
        Some(CharacterSaveRow {
            class:          class_from_str(row.try_get::<&str, _>("class").ok()?),
            level:          row.try_get::<i64, _>("level").ok()? as u32,
            xp:             row.try_get::<i64, _>("xp").ok()? as u32,
            xp_to_next:     row.try_get::<i64, _>("xp_to_next").ok()? as u32,
            health_current: row.try_get::<i64, _>("health_current").ok()? as i32,
            health_max:     row.try_get::<i64, _>("health_max").ok()? as i32,
            pos_x:          row.try_get::<f64, _>("pos_x").ok()? as f32,
            pos_y:          row.try_get::<f64, _>("pos_y").ok()? as f32,
            pos_z:          row.try_get::<f64, _>("pos_z").ok()? as f32,
        })
    })
}

/// Save (upsert) a character row to the DB.
#[allow(clippy::too_many_arguments)]
pub fn save_character(
    db: &Db,
    player_uuid: uuid::Uuid,
    class: &CharacterClass,
    level: u32,
    xp: u32,
    xp_to_next: u32,
    health_current: i32,
    health_max: i32,
    pos_x: f32,
    pos_y: f32,
    pos_z: f32,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to build tokio runtime for save: {e}");
            return;
        }
    };

    let class_str = class_to_str(class).to_owned();
    let id_str = player_uuid.to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let level_i = level as i64;
    let xp_i = xp as i64;
    let xp_to_next_i = xp_to_next as i64;
    let hc_i = health_current as i64;
    let hm_i = health_max as i64;
    let px = pos_x as f64;
    let py = pos_y as f64;
    let pz = pos_z as f64;

    rt.block_on(async {
        let result = sqlx::query(
            r#"INSERT INTO players
                   (id, name, class, level, xp, xp_to_next, health_current, health_max, pos_x, pos_y, pos_z, last_seen)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
                   class          = excluded.class,
                   level          = excluded.level,
                   xp             = excluded.xp,
                   xp_to_next     = excluded.xp_to_next,
                   health_current = excluded.health_current,
                   health_max     = excluded.health_max,
                   pos_x          = excluded.pos_x,
                   pos_y          = excluded.pos_y,
                   pos_z          = excluded.pos_z,
                   last_seen      = excluded.last_seen"#,
        )
        .bind(&id_str)
        .bind(&id_str)   // name = id for now (no name input yet)
        .bind(&class_str)
        .bind(level_i)
        .bind(xp_i)
        .bind(xp_to_next_i)
        .bind(hc_i)
        .bind(hm_i)
        .bind(px)
        .bind(py)
        .bind(pz)
        .bind(now)
        .execute(db.pool())
        .await;

        match result {
            Ok(_)  => tracing::debug!(uuid = %player_uuid, "Character saved"),
            Err(e) => tracing::error!(uuid = %player_uuid, error = %e, "Character save failed"),
        }
    });
}

/// Data loaded from the `players` table.
pub struct CharacterSaveRow {
    pub class:          CharacterClass,
    pub level:          u32,
    pub xp:             u32,
    pub xp_to_next:     u32,
    pub health_current: i32,
    pub health_max:     i32,
    pub pos_x:          f32,
    pub pos_y:          f32,
    pub pos_z:          f32,
}

// ── Autosave system ───────────────────────────────────────────────────────────

type PlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static GameEntityId,
        &'static CombatParticipant,
        &'static Health,
        &'static Experience,
        &'static WorldPosition,
    ),
    Without<fellytip_shared::components::EntityKind>,
>;

fn autosave_players(
    time: Res<Time>,
    mut timer: ResMut<AutosaveTimer>,
    db: Res<Db>,
    players: PlayerQuery,
) {
    timer.0 -= time.delta_secs();
    if timer.0 > 0.0 {
        return;
    }
    timer.0 = AUTOSAVE_INTERVAL;

    for (gid, participant, health, exp, pos) in players.iter() {
        save_character(
            &db,
            gid.0,
            &participant.class,
            participant.level,
            exp.xp,
            exp.xp_to_next,
            health.current,
            health.max,
            pos.x,
            pos.y,
            pos.z,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_roundtrip_all_classes() {
        let classes = [
            CharacterClass::Warrior,
            CharacterClass::Rogue,
            CharacterClass::Mage,
            CharacterClass::Fighter,
            CharacterClass::Wizard,
            CharacterClass::Cleric,
            CharacterClass::Ranger,
            CharacterClass::Paladin,
            CharacterClass::Druid,
            CharacterClass::Bard,
            CharacterClass::Warlock,
            CharacterClass::Sorcerer,
            CharacterClass::Monk,
            CharacterClass::Barbarian,
        ];
        for class in &classes {
            let s = class_to_str(class);
            let back = class_from_str(s);
            assert_eq!(
                std::mem::discriminant(class),
                std::mem::discriminant(&back),
                "class_from_str({s:?}) did not round-trip back to the same variant"
            );
        }
    }

    #[test]
    fn class_from_str_unknown_defaults_to_warrior() {
        assert!(matches!(class_from_str("Barbarian Warrior"), CharacterClass::Warrior));
    }
}
