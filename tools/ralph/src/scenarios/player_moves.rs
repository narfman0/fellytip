//! Scenario: assert that the local player's `WorldPosition` changes over ~4 s.
//!
//! Pre-conditions:
//!   `cargo run -p fellytip-server -- --no-idle-shutdown &`
//!   `cargo run -p fellytip-client -- --headless &`
//!
//! The headless client's `headless_auto_move` system walks right/left in 3 s
//! phases at `PLAYER_SPEED` (10 units/s), so after 4 s the player should have
//! moved at least ~10 units.

use crate::{Scenario, brp::BrpClient};
use anyhow::{Result, bail};
use std::{thread::sleep, time::{Duration, Instant}};

pub struct PlayerMoves;

const TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(250);
const WAIT_BETWEEN: Duration = Duration::from_secs(4);
const MOVE_EPSILON: f32 = 0.1;

const WORLD_POSITION: &str = "fellytip_shared::components::WorldPosition";
const EXPERIENCE: &str = "fellytip_shared::components::Experience";

impl Scenario for PlayerMoves {
    fn name(&self) -> &str {
        "player_moves"
    }

    fn run(&self) -> Result<()> {
        let server = BrpClient::server();

        // ── 1. Wait for server BRP ─────────────────────────────────────────
        let deadline = Instant::now() + TIMEOUT;
        tracing::info!("Waiting for server BRP at localhost:15702 …");
        loop {
            if server.ping() {
                break;
            }
            if Instant::now() > deadline {
                bail!("Server BRP not reachable within {TIMEOUT:?}");
            }
            sleep(POLL);
        }
        tracing::info!("Server reachable.");

        // ── 2. Wait for the player entity (WorldPosition + Experience) ──────
        // The player entity has Experience — this is the same discriminant that
        // tag_local_player uses on the client side.
        tracing::info!("Waiting for player entity …");
        let deadline = Instant::now() + TIMEOUT;
        let player = loop {
            let entities = server.query(&[WORLD_POSITION, EXPERIENCE])?;
            if let Some(e) = entities.into_iter().next() {
                break e;
            }
            if Instant::now() > deadline {
                bail!("Player entity (WorldPosition + Experience) never appeared within {TIMEOUT:?}");
            }
            sleep(POLL);
        };

        let entity_id = player["entity"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("entity id missing from query result"))?;
        let t1_x = player["components"][WORLD_POSITION]["x"]
            .as_f64()
            .unwrap_or(0.0) as f32;
        let t1_y = player["components"][WORLD_POSITION]["y"]
            .as_f64()
            .unwrap_or(0.0) as f32;
        tracing::info!(entity = entity_id, x = t1_x, y = t1_y, "T1 position sampled");

        // ── 3. Wait for the movement bot to walk ───────────────────────────
        sleep(WAIT_BETWEEN);

        // ── 4. Re-fetch position ───────────────────────────────────────────
        let t2_data = server.get(entity_id, &[WORLD_POSITION])?;
        let t2_x = t2_data[WORLD_POSITION]["x"].as_f64().unwrap_or(0.0) as f32;
        let t2_y = t2_data[WORLD_POSITION]["y"].as_f64().unwrap_or(0.0) as f32;
        tracing::info!(x = t2_x, y = t2_y, "T2 position sampled");

        // ── 5. Check displacement ──────────────────────────────────────────
        let dx = t2_x - t1_x;
        let dy = t2_y - t1_y;
        let dist = (dx * dx + dy * dy).sqrt();
        tracing::info!(dx, dy, dist, epsilon = MOVE_EPSILON, "Displacement measured");

        if dist < MOVE_EPSILON {
            bail!(
                "Player did not move: T1=({t1_x:.2},{t1_y:.2}) T2=({t2_x:.2},{t2_y:.2}) \
                 dist={dist:.4} < epsilon={MOVE_EPSILON}"
            );
        }

        tracing::info!(dist, "PASS: player moved");
        Ok(())
    }
}
