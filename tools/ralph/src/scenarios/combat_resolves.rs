//! Scenario: assert that the dungeon boss takes damage after a player connects.
//!
//! The headless client sends `BasicAttack` automatically every 2 seconds.
//! This scenario verifies that the full combat pipeline (input → interrupt stack
//! → damage effect → Health component) is working end-to-end via live BRP queries.
//!
//! Pre-conditions:
//!   `cargo run -p fellytip-server &`
//!   `cargo run -p fellytip-client -- --headless &`

use crate::{Scenario, brp::BrpClient};
use anyhow::{Context, Result, bail};
use std::{thread::sleep, time::{Duration, Instant}};

pub struct CombatResolves;

const TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(250);

const HEALTH: &str = "fellytip_shared::components::Health";
const EXPERIENCE: &str = "fellytip_shared::components::Experience";

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

        // ── 2. Wait for player entity (has Health + Experience) ───────────────
        tracing::info!("Waiting for player entity (Health + Experience)…");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let entities = server.query(&[HEALTH, EXPERIENCE])?;
            if !entities.is_empty() {
                tracing::info!("Player entity found.");
                break;
            }
            if Instant::now() > deadline { bail!("No player entity within {TIMEOUT:?}"); }
            sleep(POLL);
        }

        // ── 3. Find boss entity (Health with max > 100) ───────────────────────
        tracing::info!("Waiting for boss entity (Health.max > 100)…");
        let deadline = Instant::now() + TIMEOUT;
        let (boss_entity, initial_hp) = 'find_boss: loop {
            let entities = server.query(&[HEALTH])?;
            for e in &entities {
                let max_hp = e["components"][HEALTH]["max"].as_i64().unwrap_or(0);
                if max_hp > 100 {
                    let id = e["entity"].as_u64().context("boss entity id")?;
                    let current = e["components"][HEALTH]["current"].as_i64().unwrap_or(max_hp);
                    tracing::info!(boss = id, hp = current, max = max_hp, "Boss found");
                    break 'find_boss (id, current);
                }
            }
            if Instant::now() > deadline { bail!("No boss entity with max HP > 100 within {TIMEOUT:?}"); }
            sleep(POLL);
        };

        // ── 4. Poll until boss HP decreases ──────────────────────────────────
        // The headless client fires BasicAttack every 2 s; attack reaches boss
        // regardless of distance (no range check in current combat rules).
        tracing::info!("Polling for boss HP decrease (initial={initial_hp})…");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let result = server.get(boss_entity, &[HEALTH])?;
            let current = result[HEALTH]["current"].as_i64().unwrap_or(initial_hp);
            if current < initial_hp {
                tracing::info!(
                    "PASS: boss HP decreased from {initial_hp} to {current}"
                );
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("Boss HP did not decrease within {TIMEOUT:?} (still {current}/{initial_hp})");
            }
            sleep(POLL);
        }
    }
}
