//! Meshy API HTTP client (blocking / non-async).

use anyhow::{bail, Result};
use serde::Deserialize;

const MESHY_BASE: &str = "https://api.meshy.ai/openapi";

pub struct MeshyClient {
    client: reqwest::blocking::Client,
    api_key: String,
}

#[derive(Deserialize)]
struct TaskResponse {
    result: String,
}

#[derive(Deserialize)]
struct PollResponse {
    status: String,
    #[serde(default)]
    progress: u8,
    model_urls: Option<ModelUrls>,
}

#[derive(Deserialize)]
struct ModelUrls {
    glb: Option<String>,
}

pub struct PollResult {
    pub status: String,
    pub progress: u8,
    pub model_url: Option<String>,
}

impl MeshyClient {
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            api_key,
        }
    }

    /// POST /v2/text-to-3d (mode: preview). Returns task_id.
    pub fn submit_preview(&self, prompt: &str) -> Result<String> {
        let body = serde_json::json!({
            "mode": "preview",
            "prompt": prompt,
            "target_polycount": 10000,
            "should_remesh": true,
            "pose_mode": "t-pose"
        });
        let resp = self
            .client
            .post(format!("{MESHY_BASE}/v2/text-to-3d"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()?;
        if !resp.status().is_success() {
            let text = resp.text()?;
            bail!("Meshy preview submit failed: {text}");
        }
        let task: TaskResponse = resp.json()?;
        Ok(task.result)
    }

    /// POST /v2/text-to-3d (mode: refine). Returns task_id.
    pub fn submit_refine(&self, preview_task_id: &str) -> Result<String> {
        let body = serde_json::json!({
            "mode": "refine",
            "preview_task_id": preview_task_id,
            "enable_pbr": true
        });
        let resp = self
            .client
            .post(format!("{MESHY_BASE}/v2/text-to-3d"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()?;
        if !resp.status().is_success() {
            let text = resp.text()?;
            bail!("Meshy refine submit failed: {text}");
        }
        let task: TaskResponse = resp.json()?;
        Ok(task.result)
    }

    /// POST /v1/rigging. Returns task_id.
    pub fn submit_rig(&self, refine_task_id: &str) -> Result<String> {
        let body = serde_json::json!({
            "input_task_id": refine_task_id,
            "height_meters": 1.7
        });
        let resp = self
            .client
            .post(format!("{MESHY_BASE}/v1/rigging"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()?;
        if !resp.status().is_success() {
            let text = resp.text()?;
            bail!("Meshy rig submit failed: {text}");
        }
        let task: TaskResponse = resp.json()?;
        Ok(task.result)
    }

    /// POST /v1/animations. Returns task_id.
    pub fn submit_animation(&self, rig_task_id: &str, action_id: u32) -> Result<String> {
        let body = serde_json::json!({
            "rig_task_id": rig_task_id,
            "action_id": action_id
        });
        let resp = self
            .client
            .post(format!("{MESHY_BASE}/v1/animations"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()?;
        if !resp.status().is_success() {
            let text = resp.text()?;
            bail!("Meshy animation submit failed: {text}");
        }
        let task: TaskResponse = resp.json()?;
        Ok(task.result)
    }

    /// POST /v1/text-to-texture. Returns task_id.
    pub fn submit_texture(
        &self,
        refine_task_id: &str,
        object_prompt: &str,
        style_prompt: &str,
    ) -> Result<String> {
        let body = serde_json::json!({
            "input_task_id": refine_task_id,
            "object_prompt": object_prompt,
            "style_prompt": style_prompt,
            "enable_original_uv": true,
            "resolution": "1024"
        });
        let resp = self
            .client
            .post(format!("{MESHY_BASE}/v1/text-to-texture"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()?;
        if !resp.status().is_success() {
            let text = resp.text()?;
            bail!("Meshy texture submit failed: {text}");
        }
        let task: TaskResponse = resp.json()?;
        Ok(task.result)
    }

    /// GET /{path}/{task_id} — poll a task for status.
    pub fn poll(&self, path: &str, task_id: &str) -> Result<PollResult> {
        let url = format!("{MESHY_BASE}/{path}/{task_id}");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()?;
        if !resp.status().is_success() {
            let text = resp.text()?;
            bail!("Meshy poll failed: {text}");
        }
        let pr: PollResponse = resp.json()?;
        Ok(PollResult {
            status: pr.status,
            progress: pr.progress,
            model_url: pr.model_urls.and_then(|u| u.glb),
        })
    }

    /// Download raw bytes from a URL.
    pub fn download(&self, url: &str) -> Result<Vec<u8>> {
        let bytes = self.client.get(url).send()?.bytes()?;
        Ok(bytes.to_vec())
    }
}

/// Returns minimal valid GLB bytes (12-byte header + empty JSON chunk).
pub fn mock_glb() -> Vec<u8> {
    let json = b"{}";
    let json_padded_len = (json.len() + 3) & !3;
    let mut json_padded = json.to_vec();
    json_padded.resize(json_padded_len, 0x20);
    let total_len = 12 + 8 + json_padded_len as u32;
    let mut glb = Vec::new();
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&total_len.to_le_bytes());
    glb.extend_from_slice(&(json_padded_len as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // JSON chunk type
    glb.extend_from_slice(&json_padded);
    glb
}
