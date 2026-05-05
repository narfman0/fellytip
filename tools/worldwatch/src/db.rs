//! Read-only SQLite queries against fellytip.db.
//!
//! Opens the DB in read-only mode so worldwatch can never corrupt live data.
//! The `factions`, `ecology_state`, and `story_events` tables are written by
//! the server and read here.

use anyhow::Result;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions, Row};

use crate::state::{FactionSnapshot, RegionSnapshot, StoryEntry};

const COLLAPSE_THRESHOLD: i64 = 5;

async fn open(path: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    Ok(SqlitePool::connect_with(opts).await?)
}

pub async fn fetch_factions(db_path: &str) -> Result<Vec<FactionSnapshot>> {
    let pool = open(db_path).await?;

    let rows = sqlx::query("SELECT id, name, resources, goals FROM factions")
        .fetch_all(&pool)
        .await?;

    let mut out = Vec::new();
    for row in rows {
        let id: String = row.get("id");
        let name: String = row.get("name");
        let resources_json: String = row.get("resources");
        let goals_json: String = row.get("goals");

        let resources: serde_json::Value =
            serde_json::from_str(&resources_json).unwrap_or_default();
        let goals: serde_json::Value =
            serde_json::from_str(&goals_json).unwrap_or(serde_json::Value::Array(vec![]));

        let food = resources["food"].as_f64().unwrap_or(0.0) as f32;
        let gold = resources["gold"].as_f64().unwrap_or(0.0) as f32;
        let military = resources["military_strength"].as_f64().unwrap_or(0.0) as f32;

        // Top goal: first variant name from the goals JSON array.
        let top_goal = goals
            .as_array()
            .and_then(|arr| arr.first())
            .map(goal_label)
            .unwrap_or_else(|| "None".to_owned());

        let _ = id; // not displayed in the UI
        out.push(FactionSnapshot { name, food, gold, military, top_goal });
    }
    Ok(out)
}

/// Extract a human-readable label from a serde-serialised FactionGoal.
///
/// Serialised form is either `"Survive"` (unit variant) or
/// `{"ExpandTerritory": {...}}` (struct variant).
fn goal_label(v: &serde_json::Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_owned();
    }
    if let Some(obj) = v.as_object()
        && let Some(key) = obj.keys().next()
    {
        return key.clone();
    }
    "Unknown".to_owned()
}

pub async fn fetch_ecology(db_path: &str) -> Result<Vec<RegionSnapshot>> {
    let pool = open(db_path).await?;

    // Group prey/predator rows by region_id.
    let rows = sqlx::query(
        "SELECT species_id, region_id, count FROM ecology_state ORDER BY region_id, species_id",
    )
    .fetch_all(&pool)
    .await?;

    // Build a map: region_id → (prey_row, predator_row).
    // Prey species = "deer", predator species = "wolf" (from seed_ecology).
    // Use alphabetical ordering as a stable heuristic: first species = prey, second = predator.
    let mut by_region: std::collections::BTreeMap<String, Vec<(String, i64)>> =
        std::collections::BTreeMap::new();

    for row in rows {
        let species: String = row.get("species_id");
        let region: String = row.get("region_id");
        let count: i64 = row.get("count");
        by_region.entry(region).or_default().push((species, count));
    }

    let mut out = Vec::new();
    for (region_id, mut species) in by_region {
        // Sort for deterministic ordering (alphabetical by species name).
        species.sort_by(|a, b| a.0.cmp(&b.0));

        let (prey_species, prey_count) = species
            .first()
            .cloned()
            .unwrap_or_else(|| ("?".to_owned(), 0));
        let (predator_species, predator_count) = species
            .get(1)
            .cloned()
            .unwrap_or_else(|| ("?".to_owned(), 0));

        out.push(RegionSnapshot {
            region_id,
            prey_species,
            prey_count,
            predator_species,
            predator_count,
            prey_collapsed: prey_count < COLLAPSE_THRESHOLD,
            predator_collapsed: predator_count < COLLAPSE_THRESHOLD,
        });
    }
    Ok(out)
}

pub async fn fetch_story(db_path: &str, limit: i64) -> Result<Vec<StoryEntry>> {
    let pool = open(db_path).await?;

    let rows = sqlx::query(
        "SELECT tick, world_day, kind, lore_tags \
         FROM story_events ORDER BY tick DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(&pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StoryEntry {
            tick: r.get("tick"),
            world_day: r.get("world_day"),
            kind: r.get("kind"),
            lore_tags: r.get("lore_tags"),
        })
        .collect())
}
