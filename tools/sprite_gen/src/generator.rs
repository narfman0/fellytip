//! Trait-based AI sprite generation backends.
//!
//! Add new backends by implementing `SpriteGenerator`.  The trait is object-safe
//! so callers can hold a `Box<dyn SpriteGenerator>`.

use anyhow::{Context, Result};
use image::{DynamicImage, RgbaImage, Rgba};

/// Synchronous sprite generation contract.
///
/// Implementations must be `Send + Sync` so the tool can fan out work across
/// `tokio::task::spawn_blocking` threads.
pub trait SpriteGenerator: Send + Sync {
    /// Generate one sprite frame from `prompt`.
    ///
    /// `seed` must produce identical output on repeated calls (idempotency for
    /// `--incremental` mode).  Implementations that don't support seeding
    /// (e.g. DALL-E 3) should use `seed` only for cache-key derivation.
    fn generate_blocking(&self, prompt: &str, seed: u64, frame_size: u32) -> Result<DynamicImage>;
}

// ── Mock backend ──────────────────────────────────────────────────────────────

/// Returns a solid-color placeholder frame with a cross overlay.
/// Used for `--dry-run`, CI, and offline development.
pub struct MockGenerator;

impl SpriteGenerator for MockGenerator {
    fn generate_blocking(&self, _prompt: &str, seed: u64, size: u32) -> Result<DynamicImage> {
        let r = ((seed.wrapping_mul(7).wrapping_add(100)) % 200 + 55) as u8;
        let g = ((seed.wrapping_mul(13).wrapping_add(50)) % 200 + 55) as u8;
        let b = ((seed.wrapping_mul(17).wrapping_add(80)) % 200 + 55) as u8;

        let mut img = RgbaImage::new(size, size);
        for pixel in img.pixels_mut() {
            *pixel = Rgba([r, g, b, 255]);
        }
        // Draw a thin cross so each frame is visually distinct.
        let mid = size / 2;
        for i in 0..size {
            img.put_pixel(i, mid, Rgba([0, 0, 0, 255]));
            img.put_pixel(mid, i, Rgba([0, 0, 0, 255]));
        }
        Ok(DynamicImage::ImageRgba8(img))
    }
}

// ── DALL-E 3 backend ──────────────────────────────────────────────────────────

/// Calls the OpenAI DALL-E 3 API (`POST /v1/images/generations`).
///
/// Requires an API key in `SPRITE_GEN_API_KEY` env var or passed via `--api-key`.
/// DALL-E 3 only supports `n=1`; the tool handles retries at the caller level.
pub struct DalleGenerator {
    pub api_key: String,
    pub model: String,
}

impl DalleGenerator {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: "dall-e-3".into(),
        }
    }
}

impl SpriteGenerator for DalleGenerator {
    fn generate_blocking(&self, prompt: &str, _seed: u64, frame_size: u32) -> Result<DynamicImage> {
        let client = reqwest::blocking::Client::new();
        let body = serde_json::json!({
            "model":           self.model,
            "prompt":          prompt,
            "n":               1,
            "size":            "1024x1024",
            "response_format": "b64_json",
        });

        let response = client
            .post("https://api.openai.com/v1/images/generations")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .context("HTTP request to OpenAI failed")?;

        let status = response.status();
        let json: serde_json::Value = response.json().context("Failed to parse OpenAI response")?;

        if !status.is_success() {
            let msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            anyhow::bail!("OpenAI API error {status}: {msg}");
        }

        let b64 = json["data"][0]["b64_json"]
            .as_str()
            .context("Missing b64_json in OpenAI response")?;

        let bytes = base64::Engine::decode(&base64::prelude::BASE64_STANDARD, b64)
            .context("Failed to base64-decode image")?;

        let img = image::load_from_memory(&bytes).context("Failed to decode image bytes")?;
        // Resize from 1024×1024 → target frame size with high-quality downsampling.
        let resized = img.resize_exact(frame_size, frame_size, image::imageops::FilterType::Lanczos3);
        Ok(resized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_generator_returns_correct_size() {
        let gen = MockGenerator;
        let img = gen.generate_blocking("test", 42, 64).unwrap();
        assert_eq!(img.width(), 64);
        assert_eq!(img.height(), 64);
    }

    #[test]
    fn mock_generator_is_deterministic() {
        let gen = MockGenerator;
        let a = gen.generate_blocking("any prompt", 7, 32).unwrap().to_rgba8();
        let b = gen.generate_blocking("any prompt", 7, 32).unwrap().to_rgba8();
        assert_eq!(a.as_raw(), b.as_raw());
    }

    #[test]
    fn mock_generator_differs_by_seed() {
        let gen = MockGenerator;
        let a = gen.generate_blocking("p", 0, 32).unwrap().to_rgba8();
        let b = gen.generate_blocking("p", 999, 32).unwrap().to_rgba8();
        assert_ne!(a.as_raw(), b.as_raw());
    }
}
