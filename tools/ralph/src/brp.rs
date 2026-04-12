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
}
