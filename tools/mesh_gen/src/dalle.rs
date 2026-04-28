use anyhow::{bail, Result};
use reqwest::Client;
use serde::Deserialize;

const OPENAI_URL: &str = "https://api.openai.com/v1/images/generations";

pub struct DalleClient {
    client: Client,
    api_key: String,
}

#[derive(Deserialize)]
struct ImageResponse {
    data: Vec<ImageData>,
}

#[derive(Deserialize)]
struct ImageData {
    b64_json: Option<String>,
    #[allow(dead_code)]
    url: Option<String>,
}

impl DalleClient {
    pub fn new(api_key: String) -> Self {
        Self { client: Client::new(), api_key }
    }

    /// Generate a single reference image for `description`. Returns base64 data URL.
    pub async fn generate_reference_image(&self, description: &str) -> Result<String> {
        let prompt = format!(
            "3D game asset reference sheet: {description}. \
             Turntable views: front, 3/4, side, back on white background. \
             Fantasy RPG art style, clean silhouette, suitable for 3D reconstruction."
        );
        let body = serde_json::json!({
            "model": "dall-e-3",
            "prompt": prompt,
            "n": 1,
            "size": "1024x1024",
            "response_format": "b64_json"
        });
        let resp = self.client
            .post(OPENAI_URL)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await?;
            bail!("DALL-E failed: {text}");
        }
        let img_resp: ImageResponse = resp.json().await?;
        let b64 = img_resp.data.into_iter()
            .next()
            .and_then(|d| d.b64_json)
            .ok_or_else(|| anyhow::anyhow!("No image data in DALL-E response"))?;
        Ok(format!("data:image/png;base64,{b64}"))
    }
}
