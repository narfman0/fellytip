//! WorldSnapshot and the background polling loop.
//!
//! A tokio task calls `poll_snapshot` every 2 seconds and stores the result
//! in an `Arc<Mutex<WorldSnapshot>>` that the eframe `App` reads each frame.

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::Result;

use crate::brp::BrpClient;
use crate::db;

// ── Snapshot types ────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct WorldSnapshot {
    pub server_online: bool,
    pub overview: OverviewSnapshot,
    pub factions: Vec<FactionSnapshot>,
    pub ecology: Vec<RegionSnapshot>,
    pub story: Vec<StoryEntry>,
}

#[derive(Clone, Default)]
pub struct OverviewSnapshot {
    pub total_entities: usize,
    pub player_count: usize,
    pub npc_count: usize,
    pub world_tick: u64,
}

#[derive(Clone, Default)]
pub struct FactionSnapshot {
    pub name: String,
    pub food: f32,
    pub gold: f32,
    pub military: f32,
    pub top_goal: String,
}

#[derive(Clone, Default)]
pub struct RegionSnapshot {
    pub region_id: String,
    pub prey_species: String,
    pub prey_count: i64,
    pub predator_species: String,
    pub predator_count: i64,
    pub prey_collapsed: bool,
    pub predator_collapsed: bool,
}

#[derive(Clone, Default)]
pub struct StoryEntry {
    pub tick: i64,
    pub world_day: i64,
    pub kind: String,
    pub lore_tags: String,
}

// ── Polling loop ──────────────────────────────────────────────────────────────

/// Background task: polls BRP + SQLite every 2 seconds and updates the shared
/// snapshot. Also serves freeform BRP queries and DM commands sent via channels.
pub async fn polling_loop(
    snapshot: Arc<Mutex<WorldSnapshot>>,
    query_rx: mpsc::Receiver<String>,
    result_tx: mpsc::Sender<String>,
    dm_rx: mpsc::Receiver<(String, serde_json::Value)>,
    dm_result_tx: mpsc::Sender<String>,
    db_path: String,
) {
    let brp = BrpClient::new();

    loop {
        // Check for a pending freeform BRP query (non-blocking).
        if let Ok(component_path) = query_rx.try_recv() {
            let result = run_freeform_query(&brp, &component_path).await;
            let _ = result_tx.send(result);
        }

        // Check for a pending DM command (non-blocking).
        if let Ok((method, params)) = dm_rx.try_recv() {
            let result = run_dm_command(&brp, &method, params).await;
            let _ = dm_result_tx.send(result);
        }

        let new_snap = fetch_snapshot(&brp, &db_path).await;
        {
            let mut guard = snapshot.lock().unwrap();
            *guard = new_snap;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn fetch_snapshot(brp: &BrpClient, db_path: &str) -> WorldSnapshot {
    if !brp.ping().await {
        return WorldSnapshot::default();
    }

    let overview = fetch_overview(brp).await.unwrap_or_default();
    let factions = db::fetch_factions(db_path).await.unwrap_or_default();
    let ecology = db::fetch_ecology(db_path).await.unwrap_or_default();
    let story = db::fetch_story(db_path, 50).await.unwrap_or_default();

    WorldSnapshot {
        server_online: true,
        overview,
        factions,
        ecology,
        story,
    }
}

async fn fetch_overview(brp: &BrpClient) -> Result<OverviewSnapshot> {
    // Total entities: everything with a WorldPosition.
    let all = brp
        .query(&["fellytip_shared::components::WorldPosition"])
        .await
        .unwrap_or_default();

    // Players: entities that also have an Experience component.
    let players = brp
        .query(&["fellytip_shared::components::Experience"])
        .await
        .unwrap_or_default();

    let total_entities = all.len();
    let player_count = players.len();
    let npc_count = total_entities.saturating_sub(player_count);

    // World tick: from the reflected WorldSimTick resource.
    let world_tick = brp
        .get_resource("fellytip_server::plugins::world_sim::WorldSimTick")
        .await
        .ok()
        .and_then(|v| v["0"].as_u64())
        .unwrap_or(0);

    Ok(OverviewSnapshot {
        total_entities,
        player_count,
        npc_count,
        world_tick,
    })
}

async fn run_freeform_query(brp: &BrpClient, component_path: &str) -> String {
    match brp.query(&[component_path]).await {
        Ok(results) => serde_json::to_string_pretty(&results).unwrap_or_else(|e| e.to_string()),
        Err(e) => format!("Error: {e}"),
    }
}

async fn run_dm_command(brp: &BrpClient, method: &str, params: serde_json::Value) -> String {
    match brp.call(method, params).await {
        Ok(v) => format!("✓ {}", serde_json::to_string_pretty(&v).unwrap_or_else(|e| e.to_string())),
        Err(e) => format!("✗ {e}"),
    }
}
