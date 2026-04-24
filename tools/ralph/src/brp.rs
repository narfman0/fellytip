//! Blocking BRP (Bevy Remote Protocol) HTTP client.
//!
//! Bevy 0.18 renamed all built-in BRP methods from `bevy/*` to `world.*`:
//!   bevy/query → world.query
//!   bevy/get   → world.get_components  (response wraps components in {"components":{…}})
//!   bevy/list  → world.list_resources  (used for ping)

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde_json::{Value, json};

pub struct BrpClient {
    base_url: String,
    client: Client,
}

impl BrpClient {
    pub fn server() -> Self {
        Self::new("http://localhost:15702")
    }

    #[allow(dead_code)]
    pub fn headless_client() -> Self {
        Self::new("http://localhost:15703")
    }

    fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_owned(),
            client: Client::new(),
        }
    }

    /// Call a BRP JSON-RPC method and return the `result` field.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "method":  method,
            "params":  params,
            "id":      1,
        });

        let resp: Value = self
            .client
            .post(&self.base_url)
            .json(&body)
            .send()
            .with_context(|| format!("POST {}", self.base_url))?
            .json()
            .context("parse JSON response")?;

        if let Some(err) = resp.get("error") {
            bail!("BRP error: {err}");
        }

        Ok(resp["result"].clone())
    }

    /// `world.query` — returns a list of matching entity objects.
    pub fn query(&self, components: &[&str]) -> Result<Vec<Value>> {
        let result = self.call(
            "world.query",
            json!({ "data": { "components": components } }),
        )?;
        match result {
            Value::Array(arr) => Ok(arr),
            Value::Null => Ok(vec![]),
            other => bail!("unexpected world.query result: {other}"),
        }
    }

    /// `world.get_components` — fetch specific components from a single entity.
    #[allow(dead_code)]
    /// Returns the component map directly (keyed by component path) so callers
    /// can use `result["ComponentPath"]["field"]` without unwrapping the envelope.
    pub fn get(&self, entity: u64, components: &[&str]) -> Result<Value> {
        let result = self.call(
            "world.get_components",
            json!({
                "entity": entity,
                "components": components,
            }),
        )?;
        // Bevy 0.18 wraps the components under a "components" key.
        Ok(result["components"].clone())
    }

    /// Check whether the server is reachable by listing resources.
    pub fn ping(&self) -> bool {
        self.call("world.list_resources", json!({})).is_ok()
    }

    /// `dm/spawn_npc` — spawn a faction NPC at the given position.
    /// Returns the spawned entity's bit-packed ID.
    pub fn dm_spawn_npc(&self, faction: &str, x: f32, y: f32, z: f32) -> Result<u64> {
        let result = self.call("dm/spawn_npc", json!({ "faction": faction, "x": x, "y": y, "z": z }))?;
        result["entity"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("dm/spawn_npc: missing 'entity' in response"))
    }

    /// `dm/kill` — despawn an entity by its bit-packed ID.
    #[allow(dead_code)]
    pub fn dm_kill(&self, entity: u64) -> Result<()> {
        self.call("dm/kill", json!({ "entity": entity }))?;
        Ok(())
    }

    /// `dm/teleport` — move an entity to a new world position.
    #[allow(dead_code)]
    pub fn dm_teleport(&self, entity: u64, x: f32, y: f32, z: f32) -> Result<()> {
        self.call("dm/teleport", json!({ "entity": entity, "x": x, "y": y, "z": z }))?;
        Ok(())
    }

    /// `dm/trigger_war_party` — force a war party to form immediately.
    /// Returns the number of warriors tagged.
    pub fn dm_trigger_war_party(&self, attacker_faction: &str, target_faction: &str) -> Result<u64> {
        let result = self.call(
            "dm/trigger_war_party",
            json!({ "attacker_faction": attacker_faction, "target_faction": target_faction }),
        )?;
        Ok(result["warriors_tagged"].as_u64().unwrap_or(0))
    }

    /// `dm/battle_history` — return up to `limit` newest-first battle records.
    pub fn dm_battle_history(&self, limit: Option<u32>) -> Result<Vec<Value>> {
        let params = match limit {
            Some(n) => json!({ "limit": n }),
            None    => json!({}),
        };
        let result = self.call("dm/battle_history", params)?;
        match result {
            Value::Array(arr) => Ok(arr),
            Value::Null       => Ok(vec![]),
            other             => bail!("unexpected dm/battle_history result: {other}"),
        }
    }

    /// `dm/underdark_pressure` — read the current pressure score and last raid tick.
    pub fn dm_underdark_pressure(&self) -> Result<(f64, u64)> {
        let result = self.call("dm/underdark_pressure", json!({}))?;
        let score = result["score"].as_f64().unwrap_or(0.0);
        let last_raid_tick = result["last_raid_tick"].as_u64().unwrap_or(0);
        Ok((score, last_raid_tick))
    }

    /// `dm/force_underdark_pressure` — force score to 1.0 so the next sim tick
    /// spawns a raid party immediately.
    pub fn dm_force_underdark_pressure(&self) -> Result<()> {
        self.call("dm/force_underdark_pressure", json!({}))?;
        Ok(())
    }
}
