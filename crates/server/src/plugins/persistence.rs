//! Persistence plugin: opens the SQLite database, runs migrations,
//! and exposes a `Db` resource for other plugins to use.
//!
//! Autosave (every 5 minutes) is wired but not yet implemented — the
//! save systems will be added in later milestones as the world state
//! types are fleshed out.

use bevy::prelude::*;
use sqlx::{Pool, Sqlite, SqlitePool};
use std::path::PathBuf;

/// Bevy resource wrapping the live SQLite connection pool.
#[derive(Resource, Clone)]
pub struct Db(pub Pool<Sqlite>);
// Field read by future system plugins; suppress dead_code until they land.
#[allow(dead_code)]
impl Db {
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.0
    }
}

/// Path to the SQLite file, relative to the working directory.
const DB_PATH: &str = "fellytip.db";

/// Migrations embedded at compile time so the binary is self-contained.
static MIGRATOR: sqlx::migrate::Migrator =
    sqlx::migrate!("../../migrations");

pub struct PersistencePlugin;

impl Plugin for PersistencePlugin {
    fn build(&self, app: &mut App) {
        // Open and migrate the database synchronously before the ECS world
        // starts ticking. This keeps the plugin self-contained without
        // requiring a Bevy async executor.
        let db = open_and_migrate();
        app.insert_resource(db);
    }
}

fn open_and_migrate() -> Db {
    let path = PathBuf::from(DB_PATH);
    let url = format!("sqlite://{}?mode=rwc", path.display());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let pool = rt.block_on(async {
        let pool = SqlitePool::connect(&url)
            .await
            .unwrap_or_else(|e| panic!("Failed to open SQLite at {url}: {e}"));
        MIGRATOR
            .run(&pool)
            .await
            .unwrap_or_else(|e| panic!("Migration failed: {e}"));
        pool
    });

    tracing::info!("Database ready: {DB_PATH}");
    Db(pool)
}
