//! Scenario: assert the Lotka-Volterra ecology system is actually integrating
//! populations across `WorldSimSchedule` ticks — not just logging "Ecology
//! population snapshot" lines that never change.
//!
//! Takes two snapshots ~10s apart via `dm/ecology_snapshot`, asserts a
//! meaningful fraction of regions show population movement, and that at
//! least one region is below the collapse threshold (proving the
//! collapse/recovery branch is reachable in normal operation).
//!
//! Pre-conditions:
//!   `cargo run -p fellytip-client -- --headless &`

use crate::{Scenario, brp::BrpClient};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{collections::HashMap, thread::sleep, time::{Duration, Instant}};

pub struct EcologyDynamics;

const POLL: Duration = Duration::from_millis(250);
/// Population threshold below which a species is considered collapsed
/// (matches `fellytip_world_types::ecology::COLLAPSE_THRESHOLD`).
const COLLAPSE_THRESHOLD: f64 = 5.0;
/// Min fraction of regions that must show non-trivial movement between
/// snapshots to call the simulation "alive".
const MIN_MOVING_FRACTION: f32 = 0.10;
/// Per-region population delta considered "non-trivial".
const MOVE_EPSILON: f64 = 0.01;
/// Total wait between the two ecology snapshots. WorldSimSchedule fires at
/// 1 Hz, so 10s gives ~10 integration steps.
const SETTLE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy)]
struct RegionPop { prey: f64, predator: f64 }

fn parse_snapshot(v: &Value) -> Result<HashMap<String, RegionPop>> {
    let regions = v["regions"].as_array()
        .ok_or_else(|| anyhow::anyhow!("snapshot missing `regions` array"))?;
    let mut out = HashMap::new();
    for r in regions {
        let region = r["region"].as_str()
            .ok_or_else(|| anyhow::anyhow!("region.region not a string"))?
            .to_string();
        let prey = r["prey"].as_f64().unwrap_or(0.0);
        let predator = r["predator"].as_f64().unwrap_or(0.0);
        out.insert(region, RegionPop { prey, predator });
    }
    Ok(out)
}

impl Scenario for EcologyDynamics {
    fn name(&self) -> &str { "ecology_dynamics" }

    fn run(&self) -> Result<()> {
        let server = BrpClient::server();

        // ── 1. Wait for server ────────────────────────────────────────────
        let deadline = Instant::now() + Duration::from_secs(5);
        while !server.ping() {
            if Instant::now() > deadline { bail!("Server BRP not reachable within 5s"); }
            sleep(POLL);
        }

        // ── 2. First snapshot ─────────────────────────────────────────────
        let r1 = server.call("dm/ecology_snapshot", json!({}))?;
        let a = parse_snapshot(&r1)?;
        if a.is_empty() { bail!("no ecology regions seeded — EcologyState not populated"); }
        tracing::info!(regions = a.len(), "seeded ecology snapshot taken");

        // ── 3. Wait for the sim to tick a few times ──────────────────────
        sleep(SETTLE);

        // ── 4. Second snapshot ───────────────────────────────────────────
        let r2 = server.call("dm/ecology_snapshot", json!({}))?;
        let b = parse_snapshot(&r2)?;

        // ── 5. Movement assertion ────────────────────────────────────────
        let mut moved = 0;
        for (k, pre) in &a {
            if let Some(post) = b.get(k)
                && ((post.prey - pre.prey).abs() > MOVE_EPSILON
                    || (post.predator - pre.predator).abs() > MOVE_EPSILON)
            {
                moved += 1;
            }
        }
        let frac = moved as f32 / a.len() as f32;
        tracing::info!(moving_regions = moved, total = a.len(), fraction = frac,
            "ecology movement after {SETTLE:?}");
        if frac < MIN_MOVING_FRACTION {
            bail!(
                "only {moved}/{} regions moved (< {:.0}% threshold) — Lotka-Volterra integration appears dead",
                a.len(),
                MIN_MOVING_FRACTION * 100.0,
            );
        }

        // ── 6. Collapse-reachable assertion ──────────────────────────────
        let collapsed: Vec<&String> = b.iter()
            .filter(|(_, pop)| pop.prey < COLLAPSE_THRESHOLD || pop.predator < COLLAPSE_THRESHOLD)
            .map(|(k, _)| k)
            .collect();
        tracing::info!(collapsed = collapsed.len(),
            "regions below COLLAPSE_THRESHOLD ({COLLAPSE_THRESHOLD})");
        if collapsed.is_empty() {
            bail!(
                "no region below COLLAPSE_THRESHOLD={COLLAPSE_THRESHOLD} — \
                 either the collapse branch is unreachable or all populations are healthy beyond the test window"
            );
        }

        Ok(())
    }
}
