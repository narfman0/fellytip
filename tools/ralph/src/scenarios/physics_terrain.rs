//! Scenario: spawn a bot above the terrain in headless mode; assert that
//! gravity + the terrain trimesh collider catches it.
//!
//! Exercises Phase 1+2+5 of the physics work: PhysicsPlugins in headless,
//! per-chunk terrain trimesh, and the shared swept-shape-cast `step_kinematic`
//! that bots now drive movement through.
//!
//! Pre-conditions:
//!   `cargo run -p fellytip-client -- --headless &`

use crate::{Scenario, brp::BrpClient};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::{thread::sleep, time::{Duration, Instant}};

pub struct PhysicsTerrain;

const SETTLE_TIMEOUT: Duration = Duration::from_secs(8);
const SETTLE_POLL:    Duration = Duration::from_millis(250);

/// Spawn an idle bot at a chosen (x, y, z) and return its entity id.
fn spawn_bot(server: &BrpClient, x: f32, y: f32, z: f32) -> Result<u64> {
    let res = server.call(
        "dm/spawn_bot",
        json!({
            "class":      "Warrior",
            "x":          x,
            "y":          y,
            "z":          z,
            "policy":     "Idle",
            "move_speed": 0.0,
        }),
    )?;
    res["entity"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("dm/spawn_bot: missing 'entity' in response"))
}

/// Read the bot's current `WorldPosition` by entity id.
fn bot_z(server: &BrpClient, entity: u64) -> Result<f32> {
    let comps = server.get(entity, &["fellytip_shared::components::WorldPosition"])?;
    let z = comps["fellytip_shared::components::WorldPosition"]["z"]
        .as_f64()
        .context("WorldPosition.z missing")?;
    Ok(z as f32)
}

impl Scenario for PhysicsTerrain {
    fn name(&self) -> &str { "physics_terrain" }

    fn run(&self) -> Result<()> {
        let server = BrpClient::server();

        // ── 1. Wait for server ────────────────────────────────────────────
        let deadline = Instant::now() + Duration::from_secs(5);
        while !server.ping() {
            if Instant::now() > deadline { bail!("Server BRP not reachable within 5s"); }
            sleep(SETTLE_POLL);
        }

        // ── 2. Spawn a bot well above the terrain ─────────────────────────
        const SPAWN_Z: f32 = 50.0;
        let entity = spawn_bot(&server, 1.0, 1.0, SPAWN_Z)?;
        tracing::info!(entity, z = SPAWN_Z, "spawned idle bot above terrain");

        // ── 3. Poll until the bot stops moving (z stabilises) ─────────────
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        let mut last_z = SPAWN_Z;
        let mut stable_ticks = 0;
        let mut final_z = SPAWN_Z;
        while Instant::now() < deadline {
            sleep(SETTLE_POLL);
            let z = bot_z(&server, entity)?;
            if (z - last_z).abs() < 0.01 {
                stable_ticks += 1;
                if stable_ticks >= 3 {
                    final_z = z;
                    break;
                }
            } else {
                stable_ticks = 0;
            }
            last_z = z;
        }

        tracing::info!(final_z, "bot rest height after fall");

        // ── 4. Assertions ──────────────────────────────────────────────────
        // Must have fallen (NOT still at SPAWN_Z = 50).
        if final_z >= SPAWN_Z - 5.0 {
            bail!("bot did not fall: final_z={final_z:.2}, spawn_z={SPAWN_Z:.2}");
        }
        // Must have landed within plausible terrain z range. Surface terrain
        // ranges roughly 0..Z_SCALE (26); a tighter window catches fall-through.
        if !(0.0..=30.0).contains(&final_z) {
            bail!("bot landed outside plausible terrain range: final_z={final_z:.2}");
        }

        Ok(())
    }
}
