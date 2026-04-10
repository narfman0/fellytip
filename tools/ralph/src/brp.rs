//! Blocking BRP (Bevy Remote Protocol) HTTP client.

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

    /// `bevy/query` — returns a list of matching entity objects.
    pub fn query(&self, components: &[&str]) -> Result<Vec<Value>> {
        let result = self.call(
            "bevy/query",
            json!({ "data": { "components": components } }),
        )?;
        match result {
            Value::Array(arr) => Ok(arr),
            Value::Null => Ok(vec![]),
            other => bail!("unexpected bevy/query result: {other}"),
        }
    }

    /// Check whether the server is reachable.
    pub fn ping(&self) -> bool {
        self.call("bevy/list", json!({})).is_ok()
    }
}
