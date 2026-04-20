//! Image-source abstraction for sprite frames.
//!
//! `#18` wires a real AI backend; this file ships a deterministic mock so the
//! end-to-end pipeline (layout → image → PNG + RON) can be exercised offline.

use image::{Rgba, RgbaImage};

/// Parameters describing a single frame request.
#[derive(Debug, Clone, Copy)]
pub struct FrameRequest<'a> {
    pub entity_id: &'a str,
    pub animation: &'a str,
    pub direction: u32,
    pub frame: u32,
    pub tile_size: u32,
}

pub trait SpriteGenerator {
    /// Produce a single frame as an RGBA tile_size × tile_size image.
    fn generate(&self, req: FrameRequest<'_>) -> anyhow::Result<RgbaImage>;

    /// Prompt that would be sent to the backend for this frame.  Used by
    /// `--dry-run` so CI and reviewers can eyeball the prompts without cost.
    fn prompt_for(&self, _req: FrameRequest<'_>, base_prompt: &str, style: &str) -> String {
        format!("{base_prompt} | {style}")
    }
}

/// Deterministic placeholder generator — emits a solid-color frame keyed by
/// `(entity_id, direction, frame)` with a small progress stripe at the bottom
/// so animations aren't visually identical.  Fast, no network, no secrets.
pub struct MockGenerator;

impl SpriteGenerator for MockGenerator {
    fn generate(&self, req: FrameRequest<'_>) -> anyhow::Result<RgbaImage> {
        let [r, g, b] = mock_color(req.entity_id, req.direction);
        let mut img = RgbaImage::from_pixel(req.tile_size, req.tile_size, Rgba([r, g, b, 255]));

        // Progress stripe: proportional to `frame` (darker = earlier).
        let stripe_h = (req.tile_size / 8).max(2);
        let progress = req.frame.saturating_add(1) as f32
            / (req.frame.saturating_add(2)) as f32; // 1/2, 2/3, 3/4, ...
        let filled = (req.tile_size as f32 * progress) as u32;
        for y in (req.tile_size - stripe_h)..req.tile_size {
            for x in 0..filled.min(req.tile_size) {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        Ok(img)
    }

    fn prompt_for(&self, req: FrameRequest<'_>, base_prompt: &str, style: &str) -> String {
        // Dry-run output matches what a real backend would receive.
        format!(
            "{base_prompt}, facing direction {}/{max}, frame {} of anim `{}` | style: {style}",
            req.direction,
            req.frame,
            req.animation,
            max = 7,
        )
    }
}

/// FNV-1a hash → RGB color.  Deterministic, no external randomness.
fn mock_color(entity_id: &str, direction: u32) -> [u8; 3] {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64  = 0x100000001b3;
    let mut h: u64 = FNV_OFFSET;
    for b in entity_id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h ^= direction as u64;
    h = h.wrapping_mul(FNV_PRIME);

    [
        ((h >> 16) & 0xff) as u8,
        ((h >> 8)  & 0xff) as u8,
        ( h        & 0xff) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_colors_are_stable() {
        let a = mock_color("goblin_scout", 0);
        let b = mock_color("goblin_scout", 0);
        assert_eq!(a, b);
    }

    #[test]
    fn mock_colors_differ_by_direction() {
        let a = mock_color("goblin_scout", 0);
        let b = mock_color("goblin_scout", 1);
        assert_ne!(a, b);
    }

    #[test]
    fn mock_colors_differ_by_entity() {
        let a = mock_color("goblin_scout", 0);
        let b = mock_color("orc_grunt", 0);
        assert_ne!(a, b);
    }

    #[test]
    fn mock_generate_produces_expected_dimensions() {
        let g = MockGenerator;
        let img = g
            .generate(FrameRequest {
                entity_id: "x",
                animation: "idle",
                direction: 0,
                frame: 0,
                tile_size: 64,
            })
            .unwrap();
        assert_eq!(img.width(), 64);
        assert_eq!(img.height(), 64);
    }
}
