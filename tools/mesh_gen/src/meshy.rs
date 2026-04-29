use anyhow::{bail, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;

const MESHY_BASE: &str = "https://api.meshy.ai";

pub struct MeshyClient {
    client: Client,
    api_key: String,
}

#[derive(Deserialize)]
struct TaskResponse {
    result: String, // task_id
}

#[derive(Deserialize)]
struct PollResponse {
    status: String,
    progress: Option<u32>,
    model_urls: Option<ModelUrls>,
}

#[derive(Deserialize)]
struct ModelUrls {
    glb: Option<String>,
}

impl MeshyClient {
    pub fn new(api_key: String) -> Self {
        Self { client: Client::new(), api_key }
    }

    /// Submit an image-to-3D task. `image_data_url` is a base64 data URL.
    pub async fn submit_image_to_3d(&self, image_data_url: &str) -> Result<String> {
        let body = serde_json::json!({
            "image_url": image_data_url,
            "enable_pbr": true
        });
        let resp = self.client
            .post(format!("{MESHY_BASE}/v1/image-to-3d"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await?;
            bail!("Meshy image-to-3d submit failed: {text}");
        }
        let task: TaskResponse = resp.json().await?;
        Ok(task.result)
    }

    /// Submit a text-to-3D task with rigging and animation enabled (Meshy v2).
    pub async fn submit_text_to_animated_3d(&self, prompt: &str) -> Result<String> {
        let body = serde_json::json!({
            "mode": "preview",
            "prompt": prompt,
            "art_style": "realistic",
            "should_remesh": true,
            "should_animate": true,
        });
        let resp = self.client
            .post(format!("{MESHY_BASE}/v2/text-to-3d"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await?;
            bail!("Meshy text-to-3d submit failed: {text}");
        }
        let task: TaskResponse = resp.json().await?;
        Ok(task.result)
    }

    /// Poll image-to-3D task until complete. Returns GLB bytes.
    pub async fn wait_and_download_image_3d(&self, task_id: &str) -> Result<Vec<u8>> {
        self.poll_until_done(format!("{MESHY_BASE}/v1/image-to-3d/{task_id}"), task_id).await
    }

    /// Poll text-to-3D task until complete. Returns animated GLB bytes.
    pub async fn wait_and_download_text_3d(&self, task_id: &str) -> Result<Vec<u8>> {
        self.poll_until_done(format!("{MESHY_BASE}/v2/text-to-3d/{task_id}"), task_id).await
    }

    async fn poll_until_done(&self, url: String, task_id: &str) -> Result<Vec<u8>> {
        loop {
            sleep(Duration::from_secs(5)).await;
            let resp = self.client
                .get(&url)
                .bearer_auth(&self.api_key)
                .send()
                .await?
                .json::<PollResponse>()
                .await?;
            let progress = resp.progress.unwrap_or(0);
            tracing::info!("Task {task_id}: {} ({progress}%)", resp.status);
            match resp.status.as_str() {
                "SUCCEEDED" => {
                    let glb_url = resp.model_urls
                        .and_then(|u| u.glb)
                        .ok_or_else(|| anyhow::anyhow!("No GLB URL in response"))?;
                    let bytes = self.client.get(&glb_url).send().await?.bytes().await?;
                    return Ok(bytes.to_vec());
                }
                "FAILED" => bail!("Meshy task {task_id} failed"),
                _ => {} // IN_PROGRESS, keep polling
            }
        }
    }
}

pub struct MockMeshyClient;

impl MockMeshyClient {
    /// Returns minimal valid GLB bytes (12-byte GLB header + empty JSON chunk).
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

    /// Returns minimal valid PNG bytes (1×1 white pixel).
    pub fn mock_png() -> Vec<u8> {
        // Minimal 1x1 white pixel PNG (well-known byte sequence)
        vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
            0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, // IHDR length=13
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // width=1, height=1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, // 8bpc RGB, CRC start
            0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, // CRC end, IDAT length=12
            0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00, // IDAT type + data
            0x00, 0x00, 0x02, 0x00, 0x01, 0xe2, 0x21, 0xbc, // IDAT data + CRC
            0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, // IEND length=0 + type
            0x44, 0xae, 0x42, 0x60, 0x82,                   // IEND CRC
        ]
    }
}
