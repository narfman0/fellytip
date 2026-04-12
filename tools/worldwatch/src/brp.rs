//! Async BRP (Bevy Remote Protocol) HTTP client.
//!
//! Mirrors tools/ralph/src/brp.rs but uses reqwest's async client since
//! this crate runs inside a tokio runtime.

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::{Value, json};

pub struct BrpClient {
    base_url: String,
    client: Client,
}

impl BrpClient {
    pub fn new() -> Self {
        Self {
            base_url: "http://localhost:15702".to_owned(),
            client: Client::new(),
        }
    }

    /// Call a BRP JSON-RPC method and return the `result` field.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
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
            .await
            .with_context(|| format!("POST {}", self.base_url))?
            .json()
            .await
            .context("parse JSON response")?;

        if let Some(err) = resp.get("error") {
            bail!("BRP error: {err}");
        }

        Ok(resp["result"].clone())
    }

    /// `world.query` — returns matching entities with their component values.
    pub async fn query(&self, components: &[&str]) -> Result<Vec<Value>> {
        let result = self
            .call("world.query", json!({ "data": { "components": components } }))
            .await?;
        match result {
            Value::Array(arr) => Ok(arr),
            Value::Null => Ok(vec![]),
            other => bail!("unexpected world.query result: {other}"),
        }
    }

    /// `world.get_resource` — fetch a resource by its full type path.
    ///
    /// Requires the resource to have `#[derive(Reflect)]` and be registered
    /// with `app.register_type::<T>()` on the server.
    pub async fn get_resource(&self, resource_path: &str) -> Result<Value> {
        self.call(
            "world.get_resource",
            json!({ "resource": resource_path }),
        )
        .await
    }

    /// Check whether the server is reachable.
    pub async fn ping(&self) -> bool {
        self.call("world.list_resources", json!({})).await.is_ok()
    }
}
