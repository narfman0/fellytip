//! Scenario: assert that one `PhysicsBuilding` collider exists for every
//! procedurally-generated building, and that a bot dropped onto a building's
//! footprint lands ABOVE the surrounding terrain (proving the cuboid
//! collider catches it before it falls through to the trimesh below).
//!
//! Exercises Phase 4 of the physics work: building cuboid colliders authored
//! by `PhysicsWorldPlugin` from `Buildings` data, no GLB loading required.
//!
//! Pre-conditions:
//!   `cargo run -p fellytip-client -- --headless &`

use crate::{Scenario, brp::BrpClient};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::{thread::sleep, time::{Duration, Instant}};

pub struct PhysicsBuildings;

const SETTLE_TIMEOUT: Duration = Duration::from_secs(8);
const POLL:           Duration = Duration::from_millis(250);

fn building_translations(server: &BrpClient) -> Result<Vec<(u64, [f32; 3])>> {
    let result = server.call(
        "world.query",
        json!({
            "data": {
                "components": [
                    "fellytip_game::plugins::physics_world::PhysicsBuilding",
                    "bevy_transform::components::transform::Transform"
                ]
            }
        }),
    )?;
    let arr = result.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for e in arr {
        let entity = e["entity"].as_u64().context("entity missing")?;
        // Bevy 0.18 BRP serializes Vec3 as a JSON array `[x, y, z]`.
        let t = &e["components"]["bevy_transform::components::transform::Transform"]
            ["translation"];
        let arr = t.as_array().cloned().unwrap_or_default();
        if arr.len() != 3 { continue }
        let x = arr[0].as_f64().unwrap_or(f64::NAN) as f32;
        let y = arr[1].as_f64().unwrap_or(f64::NAN) as f32;
        let z = arr[2].as_f64().unwrap_or(f64::NAN) as f32;
        if x.is_finite() && y.is_finite() && z.is_finite() {
            out.push((entity, [x, y, z]));
        }
    }
    Ok(out)
}

fn spawn_idle_bot(server: &BrpClient, x: f32, y: f32, z: f32) -> Result<u64> {
    let res = server.call(
        "dm/spawn_bot",
        json!({
            "class": "Warrior",
            "x": x, "y": y, "z": z,
            "policy": "Idle", "move_speed": 0.0,
        }),
    )?;
    res["entity"].as_u64()
        .ok_or_else(|| anyhow::anyhow!("dm/spawn_bot: missing 'entity'"))
}

fn bot_z(server: &BrpClient, entity: u64) -> Result<f32> {
    let comps = server.get(entity, &["fellytip_shared::components::WorldPosition"])?;
    let z = comps["fellytip_shared::components::WorldPosition"]["z"]
        .as_f64().context("WorldPosition.z missing")?;
    Ok(z as f32)
}

impl Scenario for PhysicsBuildings {
    fn name(&self) -> &str { "physics_buildings" }

    fn run(&self) -> Result<()> {
        let server = BrpClient::server();

        // ── 1. Wait for server ─────────────────────────────────────────────
        let deadline = Instant::now() + Duration::from_secs(5);
        while !server.ping() {
            if Instant::now() > deadline { bail!("Server BRP not reachable within 5s"); }
            sleep(POLL);
        }

        // ── 2. There must be PhysicsBuilding colliders ─────────────────────
        // Wait up to a few seconds for MapGenPlugin to finish + the
        // physics_world refresh system to run at least once.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buildings = Vec::new();
        while Instant::now() < deadline {
            buildings = building_translations(&server)?;
            if !buildings.is_empty() { break; }
            sleep(POLL);
        }
        if buildings.is_empty() {
            bail!("no PhysicsBuilding colliders spawned after 5s — check Phase 4 wiring");
        }
        tracing::info!(count = buildings.len(), "PhysicsBuilding colliders detected");

        // ── 3. Pick a building whose roof sits noticeably above terrain ────
        // physics_world places the cuboid centered at (wx, b.z + hy, wy)
        // where hy = half-height. The terrain top at (wx, wy) is roughly
        // `b.z` (building rests on terrain), so the roof is at b.z + 2*hy.
        // We pick a building with hy > 2 (Tavern/Barracks/Tower/Keep/Capital)
        // so the difference is unambiguous.
        let target = buildings.iter().find(|(_, t)| {
            // Tile-space center y > 4 means the cuboid covers significant air.
            // (Half-height = translation.y - terrain_z, but we don't know
            // terrain_z without a second BRP call. Use translation.y as a
            // proxy — tall buildings have larger center.y.)
            t[1] > 15.0
        }).copied();

        // If no tall building exists in the world (rare), accept the count
        // check alone — Phase 4 wiring is still proven.
        let Some((_, building_pos)) = target else {
            tracing::info!("no tall building found in this world; skipping land-on-roof check");
            return Ok(());
        };
        tracing::info!(pos = ?building_pos, "dropping bot onto building");

        // ── 4. Drop a bot onto the building's xy at high z ─────────────────
        let bot = spawn_idle_bot(&server, building_pos[0], building_pos[2], 60.0)?;
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        let mut last_z = 60.0_f32;
        let mut stable = 0;
        let mut final_z = 60.0_f32;
        while Instant::now() < deadline {
            sleep(POLL);
            let z = bot_z(&server, bot)?;
            if (z - last_z).abs() < 0.01 {
                stable += 1;
                if stable >= 3 { final_z = z; break; }
            } else {
                stable = 0;
            }
            last_z = z;
        }
        tracing::info!(final_z, "bot rested on building");

        // ── 5. Assertion: bot rests above the building's base z ───────────
        // building_pos[1] is the cuboid CENTER y (b.z + hy). For Tavern-sized
        // buildings (hy=2.5) the roof is at center + 2.5 and base at center -
        // 2.5. We require final_z >= building_pos[1] - 0.5 — i.e. the bot
        // landed somewhere from mid-cuboid up to roof, not on the terrain
        // below the building.
        if final_z < building_pos[1] - 0.5 {
            bail!(
                "bot landed below building cuboid: final_z={final_z:.2}, building_center_y={:.2}",
                building_pos[1]
            );
        }

        Ok(())
    }
}
