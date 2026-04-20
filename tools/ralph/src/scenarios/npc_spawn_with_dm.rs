//! Scenario: self-contained NPC spawn + damage verification via DM BRP methods.
//!
//! Pattern: start → inject state via DM → verify outcome → teardown.
//!
//! ```bash
//! cargo run -p ralph -- --scenario npc_spawn_with_dm
//! ```
//!
//! The scenario launches its own headless server, uses `dm/spawn_npc` to place
//! an Iron Wolves NPC adjacent to the player, then waits for `headless_auto_attack`
//! (fires every 2 s) to damage it.  The harness kills the server on drop.

use crate::{Scenario, brp::BrpClient, harness::TestHarness};
use anyhow::{Result, bail};
use std::{thread::sleep, time::{Duration, Instant}};

pub struct NpcSpawnWithDm;

const TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(250);

const WORLD_POSITION: &str = "fellytip_shared::components::WorldPosition";
const EXPERIENCE:     &str = "fellytip_shared::components::Experience";
const HEALTH:         &str = "fellytip_shared::components::Health";

impl Scenario for NpcSpawnWithDm {
    fn name(&self) -> &str {
        "npc_spawn_with_dm"
    }

    fn run(&self) -> Result<()> {
        // ── 1. Start a fresh headless server ─────────────────────────────────
        let _harness = TestHarness::start(&[])?;
        let server = BrpClient::server();

        // ── 2. Poll for player entity (WorldPosition + Experience) ────────────
        tracing::info!("Polling for player entity (WorldPosition + Experience) …");
        let deadline = Instant::now() + TIMEOUT;
        let (px, py, pz) = loop {
            let entities = server.query(&[WORLD_POSITION, EXPERIENCE])?;
            if let Some(e) = entities.first() {
                let x = e["components"][WORLD_POSITION]["x"].as_f64().unwrap_or(0.0) as f32;
                let y = e["components"][WORLD_POSITION]["y"].as_f64().unwrap_or(0.0) as f32;
                let z = e["components"][WORLD_POSITION]["z"].as_f64().unwrap_or(0.0) as f32;
                tracing::info!(x, y, z, "Found player entity");
                break (x, y, z);
            }
            if Instant::now() > deadline {
                bail!("No player entity found within {TIMEOUT:?}");
            }
            sleep(POLL);
        };

        // ── 3. DM-spawn an NPC adjacent to the player ─────────────────────────
        let npc_entity = server.dm_spawn_npc("iron_wolves", px + 2.0, py, pz)?;
        tracing::info!(npc_entity, "DM spawned Iron Wolves NPC adjacent to player");

        // ── 4. Wait for the NPC to take damage ────────────────────────────────
        tracing::info!("Waiting for NPC to take damage (headless_auto_attack fires every 2 s) …");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let entities = server.query(&[HEALTH])?;
            for e in &entities {
                if e["entity"].as_u64().unwrap_or(0) != npc_entity { continue; }
                let max_hp  = e["components"][HEALTH]["max"].as_i64().unwrap_or(0);
                let current = e["components"][HEALTH]["current"].as_i64().unwrap_or(max_hp);
                if max_hp > 0 && current < max_hp {
                    tracing::info!(npc_entity, current, max_hp, "PASS: NPC took damage");
                    return Ok(());
                }
            }
            if Instant::now() > deadline {
                bail!("NPC did not take damage within {TIMEOUT:?}");
            }
            sleep(POLL);
        }
        // ── 5. _harness drops here → server killed ────────────────────────────
    }
}
