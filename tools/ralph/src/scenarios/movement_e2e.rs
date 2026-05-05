//! Scenario: drive the player and an NPC to specific positions via `dm/move_entity`.
//!
//! Pre-conditions:
//!   `cargo run -p fellytip-client -- --headless &`
//!
//! Steps:
//!   1. Wait for server BRP and the player entity.
//!   2. Use `dm/move_entity` to path the player 10 units north-east.
//!   3. Poll until the player arrives within ARRIVE_EPSILON or times out.
//!   4. Spawn an NPC and path it to a nearby point, assert it arrives.

use crate::{Scenario, brp::BrpClient};
use anyhow::{Result, bail};
use std::{thread::sleep, time::{Duration, Instant}};

pub struct MovementE2e;

const TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(300);
const ARRIVE_EPSILON: f32 = 2.0;

const WORLD_POSITION: &str = "fellytip_shared::components::WorldPosition";
const EXPERIENCE: &str = "fellytip_shared::components::Experience";

impl Scenario for MovementE2e {
    fn name(&self) -> &str {
        "movement_e2e"
    }

    fn run(&self) -> Result<()> {
        let server = BrpClient::server();

        // ── 1. Wait for server BRP ─────────────────────────────────────────
        let deadline = Instant::now() + TIMEOUT;
        tracing::info!("Waiting for server BRP …");
        loop {
            if server.ping() { break; }
            if Instant::now() > deadline {
                bail!("Server BRP not reachable within {TIMEOUT:?}");
            }
            sleep(POLL);
        }

        // ── 2. Wait for player entity ──────────────────────────────────────
        tracing::info!("Waiting for player entity …");
        let deadline = Instant::now() + TIMEOUT;
        let player = loop {
            let entities = server.query(&[WORLD_POSITION, EXPERIENCE])?;
            if let Some(e) = entities.into_iter().next() { break e; }
            if Instant::now() > deadline {
                bail!("Player entity never appeared within {TIMEOUT:?}");
            }
            sleep(POLL);
        };

        let player_id = player["entity"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("entity id missing"))?;
        let start_x = player["components"][WORLD_POSITION]["x"].as_f64().unwrap_or(0.0) as f32;
        let start_y = player["components"][WORLD_POSITION]["y"].as_f64().unwrap_or(0.0) as f32;
        let start_z = player["components"][WORLD_POSITION]["z"].as_f64().unwrap_or(0.0) as f32;
        tracing::info!(entity = player_id, x = start_x, y = start_y, "Player start position");

        // ── 3. Path player 10 units north-east ────────────────────────────
        let target_x = start_x + 10.0;
        let target_y = start_y + 10.0;
        let n = server.dm_move_entity(player_id, target_x, target_y, start_z)?;
        tracing::info!(waypoints = n, target_x, target_y, "Player navigation goal set");

        if n == 0 {
            bail!("dm/move_entity returned 0 waypoints — destination may be blocked");
        }

        // ── 4. Poll until player arrives ──────────────────────────────────
        tracing::info!("Polling for player arrival …");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let data = server.get(player_id, &[WORLD_POSITION])?;
            let px = data[WORLD_POSITION]["x"].as_f64().unwrap_or(0.0) as f32;
            let py = data[WORLD_POSITION]["y"].as_f64().unwrap_or(0.0) as f32;
            let dx = px - target_x;
            let dy = py - target_y;
            let dist = (dx * dx + dy * dy).sqrt();
            tracing::debug!(px, py, dist, "Player position");
            if dist <= ARRIVE_EPSILON {
                tracing::info!(dist, "Player arrived at destination");
                break;
            }
            if Instant::now() > deadline {
                bail!(
                    "Player did not arrive within {TIMEOUT:?}: \
                     target=({target_x:.1},{target_y:.1}) current=({px:.1},{py:.1}) dist={dist:.2}"
                );
            }
            sleep(POLL);
        }

        // ── 5. NPC destination test ────────────────────────────────────────
        tracing::info!("Spawning NPC and testing dm/move_entity for NPCs …");
        let npc_id = server.dm_spawn_npc("iron_wolves", start_x + 2.0, start_y, start_z)?;
        let npc_target_x = start_x + 8.0;
        let npc_target_y = start_y + 4.0;
        let n = server.dm_move_entity(npc_id, npc_target_x, npc_target_y, start_z)?;
        tracing::info!(entity = npc_id, waypoints = n, "NPC navigation goal set");

        let deadline = Instant::now() + TIMEOUT;
        loop {
            let data = server.get(npc_id, &[WORLD_POSITION])?;
            let nx = data[WORLD_POSITION]["x"].as_f64().unwrap_or(0.0) as f32;
            let ny = data[WORLD_POSITION]["y"].as_f64().unwrap_or(0.0) as f32;
            let dx = nx - npc_target_x;
            let dy = ny - npc_target_y;
            let dist = (dx * dx + dy * dy).sqrt();
            tracing::debug!(nx, ny, dist, "NPC position");
            if dist <= ARRIVE_EPSILON {
                tracing::info!(dist, "NPC arrived at destination");
                break;
            }
            if Instant::now() > deadline {
                bail!(
                    "NPC did not arrive within {TIMEOUT:?}: \
                     target=({npc_target_x:.1},{npc_target_y:.1}) current=({nx:.1},{ny:.1}) dist={dist:.2}"
                );
            }
            sleep(POLL);
        }

        server.dm_kill(npc_id)?;
        tracing::info!("PASS: dm/move_entity works for both player and NPCs");
        Ok(())
    }
}
