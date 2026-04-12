//! Scenario: assert that at least one NPC takes damage within the timeout.
//!
//! Verifies the full combat pipeline end-to-end via live BRP queries:
//! interrupt stack → damage effect → Health component update.
//!
//! ## Recommended: combat-test mode (no headless client required)
//!
//! The server spawns an Iron Wolves brawler and a Merchant Guild guard that
//! fight automatically at 1 Hz.  No client needs to connect.
//!
//! ```bash
//! cargo run -p fellytip-server -- --combat-test &
//! cargo run -p ralph -- --scenario combat_resolves
//! ```
//!
//! ## Full game mode
//!
//! The headless client sends `BasicAttack` every 2 s; the server picks the
//! first entity with `ExperienceReward` as the target.
//!
//! ```bash
//! cargo run -p fellytip-server &
//! cargo run -p fellytip-client -- --headless &
//! cargo run -p ralph -- --scenario combat_resolves
//! ```

use crate::{Scenario, brp::BrpClient};
use anyhow::{Result, bail};
use std::{thread::sleep, time::{Duration, Instant}};

pub struct CombatResolves;

const TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(250);

const HEALTH: &str = "fellytip_shared::components::Health";

impl Scenario for CombatResolves {
    fn name(&self) -> &str {
        "combat_resolves"
    }

    fn run(&self) -> Result<()> {
        let server = BrpClient::server();

        // ── 1. Wait for server ────────────────────────────────────────────────
        let deadline = Instant::now() + TIMEOUT;
        tracing::info!("Waiting for server BRP at localhost:15702 …");
        loop {
            if server.ping() { break; }
            if Instant::now() > deadline { bail!("Server BRP not reachable within {TIMEOUT:?}"); }
            sleep(POLL);
        }

        // ── 2. Wait until some entity has taken damage (current < max) ────────
        // The player starts at full HP and NPCs don't attack back, so any entity
        // with current < max must be an NPC that was hit by the player.
        tracing::info!("Polling for any entity with Health.current < Health.max …");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let entities = server.query(&[HEALTH])?;
            for e in &entities {
                let max_hp  = e["components"][HEALTH]["max"].as_i64().unwrap_or(0);
                let current = e["components"][HEALTH]["current"].as_i64().unwrap_or(max_hp);
                if max_hp > 0 && current < max_hp {
                    tracing::info!(
                        entity = e["entity"].as_u64().unwrap_or(0),
                        current,
                        max_hp,
                        "PASS: combat damage confirmed"
                    );
                    return Ok(());
                }
            }
            if Instant::now() > deadline {
                bail!("No entity took damage within {TIMEOUT:?} — combat pipeline may be broken");
            }
            sleep(POLL);
        }
    }
}
