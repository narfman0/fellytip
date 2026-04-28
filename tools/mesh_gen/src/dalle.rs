use anyhow::{bail, Result};
use base64::Engine as _;
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

    /// Generate a billboard sprite for `description`. Returns raw PNG bytes.
    pub async fn generate_billboard_sprite(&self, description: &str) -> Result<Vec<u8>> {
        let prompt = format!(
            "2D billboard sprite for a fantasy RPG game: {description}. \
             Front-facing character portrait on a plain white background, \
             clean silhouette, painterly game-art style."
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
        let bytes = base64::engine::general_purpose::STANDARD.decode(&b64)?;
        Ok(bytes)
    }
}
