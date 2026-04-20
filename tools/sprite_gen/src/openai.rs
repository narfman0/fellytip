//! OpenAI DALL-E 3 backend for `SpriteGenerator`.
//!
//! Reads config from env:
//!   SPRITE_GEN_API_KEY  — required
//!   SPRITE_GEN_ENDPOINT — defaults to https://api.openai.com/v1/images/generations
//!   SPRITE_GEN_MODEL    — defaults to dall-e-3
//!
//! The backend cannot be constructed without an API key, which keeps the
//! default (`MockGenerator`) safe for CI and unit tests.

use crate::generator::{FrameRequest, SpriteGenerator};
use crate::seeding::frame_seed;
use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use image::{imageops::FilterType, RgbaImage};
use serde::{Deserialize, Serialize};

pub const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1/images/generations";
pub const DEFAULT_MODEL: &str = "dall-e-3";
pub const DALLE_SIZE: &str = "1024x1024";

pub struct OpenAiDalleGenerator {
    client: reqwest::blocking::Client,
    api_key: String,
    endpoint: String,
    model: String,
}

impl OpenAiDalleGenerator {
    /// Construct a backend from env vars.  Returns an error (not a panic) if
    /// `SPRITE_GEN_API_KEY` is missing so callers can fall back or exit
    /// cleanly.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("SPRITE_GEN_API_KEY").map_err(|_| {
            anyhow!(
                "SPRITE_GEN_API_KEY is not set. Either export it, pass --dry-run, \
                 or keep the default --backend mock."
            )
        })?;
        let endpoint = std::env::var("SPRITE_GEN_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
        let model = std::env::var("SPRITE_GEN_MODEL")
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("building HTTP client")?;

        Ok(Self { client, api_key, endpoint, model })
    }
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    prompt: &'a str,
    n: u8,
    size: &'a str,
    response_format: &'a str,
}

#[derive(Deserialize)]
struct Response {
    data: Vec<Datum>,
}

#[derive(Deserialize)]
struct Datum {
    b64_json: String,
}

impl SpriteGenerator for OpenAiDalleGenerator {
    fn generate(&self, req: FrameRequest<'_>) -> Result<RgbaImage> {
        // Prompt includes a pseudo-seed for reproducibility citation.
        let seed = frame_seed(req.entity_id, req.direction, req.frame);
        let prompt = format!(
            "{}, facing direction {}/{} (sprite-grid), frame {} of `{}`. \
             Seed={seed:016x}. Isolated subject, transparent background, \
             centered, no UI, no text, no watermark.",
            req.entity_id, req.direction, 7, req.frame, req.animation
        );

        let body = Request {
            model: &self.model,
            prompt: &prompt,
            n: 1,
            size: DALLE_SIZE,
            response_format: "b64_json",
        };

        let resp = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .context("sending request to image API")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            return Err(anyhow!("image API returned {status}: {text}"));
        }

        let parsed: Response = resp.json().context("parsing image API response")?;
        let datum = parsed
            .data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("image API returned no data"))?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&datum.b64_json)
            .context("decoding base64 image")?;
        let img = image::load_from_memory(&bytes).context("decoding PNG/JPEG bytes")?;

        // Resize to tile_size with a clean filter.
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

    /// When the env var is missing, construction fails with a clear message —
    /// callers rely on this to keep mock-default behavior deterministic.
    #[test]
    fn missing_api_key_is_a_clean_error() {
        // Scrub any ambient value for this thread.
        // SAFETY: only this test removes the var; other tests don't set it.
        unsafe { std::env::remove_var("SPRITE_GEN_API_KEY"); }
        let result = OpenAiDalleGenerator::from_env();
        match result {
            Err(e) => assert!(e.to_string().contains("SPRITE_GEN_API_KEY")),
            Ok(_) => panic!("construction must fail without SPRITE_GEN_API_KEY"),
        }
    }
}
