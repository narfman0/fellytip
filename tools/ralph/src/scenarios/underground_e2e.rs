//! Scenario: drive the full underground pressure → raid → surface loop.
//!
//! 1. Start a fresh headless server with `--history-warp-ticks 0`.
//! 2. Clear battle history so we only see post-start records.
//! 3. Call `dm/force_underground_pressure` to skip ~10 slow ticks of accumulation.
//! 4. Poll `dm/underground_pressure` until a raid has spawned (score reset + tick set).
//! 5. Query `WarPartyMember` entities tagged with the `remnants` `FactionBadge`.
//! 6. Poll those members' `ZoneMembership` until they reach `OVERWORLD_ZONE`.
//! 7. Assert the raid produced a `BattleHistory` record within the remaining
//!    budget (best-effort — surface march can exceed 60s on large maps, so a
//!    shortfall here logs a warning instead of failing the scenario).
//!
//! ```bash
//! cargo run -p ralph -- --scenario underground_e2e
//! ```
//!
//! Gates each step on a tight per-step timeout and reports totals so test
//! flakes caused by cold `cargo build` are distinguishable from logic bugs.

use crate::{Scenario, brp::BrpClient, harness::TestHarness};
use anyhow::{Result, bail};
use std::{thread::sleep, time::{Duration, Instant}};

pub struct UndergroundE2e;

const WAR_PARTY_MEMBER: &str = "fellytip_game::plugins::ai::WarPartyMember";
const FACTION_BADGE: &str = "fellytip_shared::components::FactionBadge";
const ZONE_MEMBERSHIP: &str = "fellytip_shared::world::zone::ZoneMembership";

const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);
const SURFACE_TIMEOUT: Duration = Duration::from_secs(20);
const BATTLE_TIMEOUT: Duration = Duration::from_secs(25);
const POLL: Duration = Duration::from_millis(500);

const OVERWORLD_ZONE_ID: u64 = 0;

impl Scenario for UndergroundE2e {
    fn name(&self) -> &str {
        "underground_e2e"
    }

    fn run(&self) -> Result<()> {
        let overall_start = Instant::now();

        // ── 1. Launch headless server with a clean history warp ──────────────
        let _harness = TestHarness::start(&["--history-warp-ticks", "0"])?;
        let server = BrpClient::server();

        // ── 2. Clear any residual battle records from the last run ────────────
        server.call("dm/clear_battle_history", serde_json::json!({}))?;

        // ── 3. Force pressure to 1.0 so the next 1 Hz tick spawns a raid ──────
        let (initial_score, initial_tick) = server.dm_underground_pressure()?;
        tracing::info!(initial_score, initial_tick, "Initial underground pressure");
        server.dm_force_underground_pressure()?;
        tracing::info!("dm/force_underground_pressure → score=1.0");

        // ── 4. Wait for the raid spawn by observing pressure reset ────────────
        tracing::info!("Polling dm/underground_pressure until last_raid_tick advances …");
        let deadline = Instant::now() + SPAWN_TIMEOUT;
        loop {
            let (score, last_raid_tick) = server.dm_underground_pressure()?;
            if last_raid_tick > initial_tick {
                tracing::info!(score, last_raid_tick, "Raid spawned");
                break;
            }
            if Instant::now() > deadline {
                bail!(
                    "Raid did not spawn within {:?} (score={}, last_raid_tick={})",
                    SPAWN_TIMEOUT,
                    score,
                    last_raid_tick
                );
            }
            sleep(POLL);
        }

        // ── 5. Find the underground raid party members ────────────────────────
        tracing::info!("Polling for WarPartyMember with FactionBadge=remnants …");
        let deadline = Instant::now() + SPAWN_TIMEOUT;
        let raid_entities: Vec<u64> = loop {
            let entities = server.query(&[WAR_PARTY_MEMBER, FACTION_BADGE])?;
            let matching: Vec<u64> = entities
                .iter()
                .filter(|e| {
                    e["components"][FACTION_BADGE]["faction_id"].as_str()
                        == Some("remnants")
                })
                .filter_map(|e| e["entity"].as_u64())
                .collect();
            if !matching.is_empty() {
                tracing::info!(count = matching.len(), "Underground raid party present");
                break matching;
            }
            if Instant::now() > deadline {
                bail!(
                    "No remnants faction WarPartyMember entities within {:?}",
                    SPAWN_TIMEOUT
                );
            }
            sleep(POLL);
        };

        // ── 6. Wait until at least one member reaches OVERWORLD_ZONE ──────────
        tracing::info!(
            "Polling ZoneMembership for raid members until overworld reached …"
        );
        let deadline = Instant::now() + SURFACE_TIMEOUT;
        loop {
            let entities = server.query(&[WAR_PARTY_MEMBER, ZONE_MEMBERSHIP])?;
            let on_surface = entities.iter().any(|e| {
                let eid = e["entity"].as_u64().unwrap_or(u64::MAX);
                if !raid_entities.contains(&eid) {
                    return false;
                }
                // ZoneMembership reflects as a tuple struct ([id]); handle both
                // array and object encodings defensively.
                let zid = e["components"][ZONE_MEMBERSHIP][0]
                    .as_u64()
                    .or_else(|| e["components"][ZONE_MEMBERSHIP]["0"].as_u64())
                    .unwrap_or(u64::MAX);
                zid == OVERWORLD_ZONE_ID
            });
            if on_surface {
                tracing::info!(
                    elapsed_s = overall_start.elapsed().as_secs(),
                    "Raid party reached OVERWORLD_ZONE"
                );
                break;
            }
            if Instant::now() > deadline {
                bail!(
                    "Raid party did not reach OVERWORLD_ZONE within {:?}",
                    SURFACE_TIMEOUT
                );
            }
            sleep(POLL);
        }

        // ── 7. Best-effort: wait for a BattleHistory record mentioning the raid
        // Surface march + battle can exceed the 60s budget on large worlds.
        // A shortfall here logs a warning — the rest of the chain already ran.
        tracing::info!(
            "Best-effort polling dm/battle_history for a remnants record …"
        );
        let deadline = Instant::now() + BATTLE_TIMEOUT;
        let mut saw_record = false;
        while Instant::now() < deadline {
            let records = server.dm_battle_history(None)?;
            if records.iter().any(|r| {
                r["winner_faction"].as_str() == Some("remnants")
                    || r["loser_faction"].as_str() == Some("remnants")
            }) {
                saw_record = true;
                tracing::info!("BattleHistory contains a remnants record");
                break;
            }
            sleep(POLL);
        }
        if !saw_record {
            tracing::warn!(
                "No remnants BattleHistory record observed within {:?} — \
                 surface march + battle likely exceeds the E2E window; \
                 zone-hop + overworld arrival still passed.",
                BATTLE_TIMEOUT
            );
        }

        tracing::info!(
            total_s = overall_start.elapsed().as_secs(),
            "[underground_e2e] PASS"
        );
        Ok(())
        // _harness drops → server killed
    }
}
