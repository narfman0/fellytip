//! Stability AI (Stable Image Core) backend for `SpriteGenerator`.
//!
//! Reads config from env:
//!   STABILITY_API_KEY  — required
//!   STABILITY_ENDPOINT — defaults to https://api.stability.ai/v2beta/stable-image/generate/core
//!
//! Uses the v2beta multipart API; returns base64 JSON.

use crate::generator::{FrameRequest, SpriteGenerator};
use crate::seeding::frame_seed;
use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use image::{imageops::FilterType, RgbaImage};
use serde::Deserialize;

pub const DEFAULT_ENDPOINT: &str =
    "https://api.stability.ai/v2beta/stable-image/generate/core";

pub struct StabilityGenerator {
    client: reqwest::blocking::Client,
    api_key: String,
    endpoint: String,
}

impl StabilityGenerator {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("STABILITY_API_KEY").map_err(|_| {
            anyhow!(
                "STABILITY_API_KEY is not set. Export it or use a different backend."
            )
        })?;
        let endpoint = std::env::var("STABILITY_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("building HTTP client")?;

        Ok(Self { client, api_key, endpoint })
    }
}

#[derive(Deserialize)]
struct StabilityResponse {
    image: String,
}

impl SpriteGenerator for StabilityGenerator {
    fn generate(&self, req: FrameRequest<'_>) -> Result<RgbaImage> {
        let prompt = self.prompt_for(req, req.base_prompt, req.style);

        let form = reqwest::blocking::multipart::Form::new()
            .text("prompt", prompt)
            .text("output_format", "png")
            .text("aspect_ratio", "1:1");

        let resp = self
            .client
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Accept", "application/json")
            .multipart(form)
            .send()
            .context("sending request to Stability AI")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            return Err(anyhow!("Stability AI returned {status}: {text}"));
        }

        let parsed: StabilityResponse = resp.json().context("parsing Stability AI response")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&parsed.image)
            .context("decoding base64 image from Stability AI")?;

        let img = image::load_from_memory(&bytes).context("decoding image bytes")?;
        let resized = img.resize_exact(req.tile_size, req.tile_size, FilterType::Lanczos3);
        Ok(resized.to_rgba8())
    }

    fn prompt_for(&self, req: FrameRequest<'_>, base_prompt: &str, style: &str) -> String {
        let seed = frame_seed(req.entity_id, req.direction, req.frame);
        format!(
            "{base_prompt}, facing direction {}/{} (sprite-grid), frame {} of `{}`. \
             Seed={seed:016x}. Style: {style}. Isolated subject, transparent background.",
            req.direction, 7, req.frame, req.animation
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_api_key_is_a_clean_error() {
        // SAFETY: only this test removes the var; other tests don't set it.
        unsafe { std::env::remove_var("STABILITY_API_KEY"); }
        let result = StabilityGenerator::from_env();
        match result {
            Err(e) => assert!(e.to_string().contains("STABILITY_API_KEY")),
            Ok(_) => panic!("construction must fail without STABILITY_API_KEY"),
        }
    }
}
