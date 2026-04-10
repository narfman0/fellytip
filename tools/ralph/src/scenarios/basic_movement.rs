//! Scenario: assert that at least one entity with `WorldPosition` exists on
//! the server after a client connects.
//!
//! Pre-conditions (set up externally before running ralph):
//!   `cargo run -p fellytip-server &`
//!   `cargo run -p fellytip-client -- --headless &`
//!
//! The scenario polls for up to 5 s to give the client time to connect and
//! trigger the server-side player spawn.

use crate::{Scenario, brp::BrpClient};
use anyhow::{Result, bail};
use std::{thread::sleep, time::{Duration, Instant}};

pub struct BasicMovement;

const TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

impl Scenario for BasicMovement {
    fn name(&self) -> &str {
        "basic_movement"
    }

    fn run(&self) -> Result<()> {
        let server = BrpClient::server();

        // ── 1. Wait for server to be reachable ─────────────────────────────
        let deadline = Instant::now() + TIMEOUT;
        tracing::info!("Waiting for server BRP at localhost:15702 …");
        loop {
            if server.ping() {
                break;
            }
            if Instant::now() > deadline {
                bail!("Server BRP not reachable within {TIMEOUT:?}");
            }
            sleep(POLL_INTERVAL);
        }
        tracing::info!("Server reachable.");

        // ── 2. Poll until a WorldPosition entity appears ────────────────────
        let component = "fellytip_shared::components::WorldPosition";
        tracing::info!("Polling for entities with {component} …");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let entities = server.query(&[component])?;
            if !entities.is_empty() {
                tracing::info!(
                    "PASS: found {} entity/entities with WorldPosition",
                    entities.len()
                );
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("No WorldPosition entity appeared within {TIMEOUT:?}");
            }
            sleep(POLL_INTERVAL);
        }
    }
}
