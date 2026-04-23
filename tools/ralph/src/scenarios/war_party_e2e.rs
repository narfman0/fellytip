//! Scenario: drive a full war-party lifecycle and assert a battle winner.
//!
//! 1. Trigger a war party via `dm/trigger_war_party` (iron_wolves → ash_covenant).
//! 2. Poll `bevy/query` for `WarPartyMember` entities — require >= 10 within 15 s.
//! 3. Poll until the member count for the target settlement reaches 0 (<= 300 s).
//! 4. Fetch `dm/battle_history` and assert at least one record with a non-empty
//!    `winner_faction` (matching target settlement if available).
//!
//! ```bash
//! cargo run -p ralph -- --scenario war_party_e2e
//! ```

use crate::{Scenario, brp::BrpClient, harness::TestHarness};
use anyhow::{Result, bail};
use std::{thread::sleep, time::{Duration, Instant}};

pub struct WarPartyE2e;

const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);
const SPAWN_POLL:    Duration = Duration::from_secs(1);
const BATTLE_TIMEOUT: Duration = Duration::from_secs(300);
const BATTLE_POLL:    Duration = Duration::from_secs(2);
const HISTORY_TIMEOUT: Duration = Duration::from_secs(5);
const HISTORY_POLL:    Duration = Duration::from_millis(250);

const WAR_PARTY_MEMBER: &str = "fellytip_server::plugins::ai::WarPartyMember";

const ATTACKER: &str = "iron_wolves";
const DEFENDER: &str = "ash_covenant";
const MIN_SPAWNED: usize = 10;

impl Scenario for WarPartyE2e {
    fn name(&self) -> &str {
        "war_party_e2e"
    }

    fn run(&self) -> Result<()> {
        // ── 1. Start a fresh headless server ─────────────────────────────────
        let _harness = TestHarness::start(&[])?;
        let server = BrpClient::server();

        // ── 2. Trigger the war party via DM BRP ───────────────────────────────
        tracing::info!("Triggering war party: {ATTACKER} -> {DEFENDER}");
        let tagged = server.dm_trigger_war_party(ATTACKER, DEFENDER)?;
        tracing::info!(tagged, "dm/trigger_war_party accepted");
        if tagged < MIN_SPAWNED as u64 {
            bail!(
                "dm/trigger_war_party tagged only {tagged} warriors, expected >= {MIN_SPAWNED}"
            );
        }

        // ── 3. Verify WarPartyMember entities appear via bevy/query ──────────
        tracing::info!("Polling for WarPartyMember entities (need >= {MIN_SPAWNED}) …");
        let deadline = Instant::now() + SPAWN_TIMEOUT;
        let target_settlement_id = loop {
            let entities = server.query(&[WAR_PARTY_MEMBER])?;
            if entities.len() >= MIN_SPAWNED {
                let target = entities.first()
                    .and_then(|e| e["components"][WAR_PARTY_MEMBER]["target_settlement_id"].as_str())
                    .map(|s| s.to_owned());
                tracing::info!(
                    count = entities.len(),
                    target = ?target,
                    "War party spawned: {} members",
                    entities.len()
                );
                break target;
            }
            if Instant::now() > deadline {
                bail!(
                    "War party failed to spawn: only {} WarPartyMember entities within {:?}",
                    entities.len(),
                    SPAWN_TIMEOUT
                );
            }
            sleep(SPAWN_POLL);
        };

        // ── 4. Wait for the war party to resolve (member count drops to 0) ────
        let start = Instant::now();
        let deadline = start + BATTLE_TIMEOUT;
        tracing::info!("Waiting for battle resolution (timeout {BATTLE_TIMEOUT:?}) …");
        loop {
            let entities = server.query(&[WAR_PARTY_MEMBER])?;
            let matching = match &target_settlement_id {
                Some(sid) => entities.iter()
                    .filter(|e| {
                        e["components"][WAR_PARTY_MEMBER]["target_settlement_id"]
                            .as_str()
                            .is_some_and(|s| s == sid)
                    })
                    .count(),
                None => entities.len(),
            };
            if matching == 0 {
                tracing::info!(
                    elapsed_s = start.elapsed().as_secs(),
                    "War party resolved"
                );
                break;
            }
            if Instant::now() > deadline {
                bail!(
                    "War party did not resolve within {:?} (still {matching} members)",
                    BATTLE_TIMEOUT
                );
            }
            tracing::info!(
                remaining = matching,
                "Waiting for battle... {}s elapsed",
                start.elapsed().as_secs()
            );
            sleep(BATTLE_POLL);
        }

        // ── 5. Assert dm/battle_history captured the outcome ──────────────────
        tracing::info!("Fetching dm/battle_history …");
        let deadline = Instant::now() + HISTORY_TIMEOUT;
        loop {
            let records = server.dm_battle_history(None)?;
            let matching: Vec<&serde_json::Value> = records.iter()
                .filter(|r| {
                    r["winner_faction"].as_str().is_some_and(|s| !s.is_empty())
                        && target_settlement_id
                            .as_deref()
                            .is_none_or(|sid| r["target_settlement_id"].as_str() == Some(sid))
                })
                .collect();
            if let Some(record) = matching.first() {
                let winner = record["winner_faction"].as_str().unwrap_or("");
                let loser  = record["loser_faction"].as_str().unwrap_or("");
                tracing::info!(
                    winner,
                    loser,
                    atk_cas = record["attacker_casualties"].as_u64().unwrap_or(0),
                    def_cas = record["defender_casualties"].as_u64().unwrap_or(0),
                    "Battle resolved: {winner} defeated {loser}"
                );
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!(
                    "dm/battle_history had no matching record within {:?} (records: {})",
                    HISTORY_TIMEOUT,
                    records.len()
                );
            }
            sleep(HISTORY_POLL);
        }
        // _harness drops → server killed.
    }
}
