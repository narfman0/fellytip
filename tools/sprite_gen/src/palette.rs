//! Palette quantization — post-process generated frames so all of an entity's
//! sprites share a tight color palette, hiding small stylistic drift between
//! AI outputs.  Pure, deterministic; no I/O.

use color_quant::NeuQuant;
use image::{Rgba, RgbaImage};

/// Number of palette entries to lock an entity's sprites to.  16 is
/// aggressive enough to enforce a consistent look without killing detail.
pub const PALETTE_COLORS: usize = 16;

/// Re-colour `img` to its own `PALETTE_COLORS`-entry quantised palette.
/// Fully transparent pixels are left untouched.
pub fn quantise_in_place(img: &mut RgbaImage) {
    let pixels: Vec<u8> = img.as_raw().clone();
    let nq = NeuQuant::new(10, PALETTE_COLORS, &pixels);

    for px in img.pixels_mut() {
        let [r, g, b, a] = px.0;
        if a == 0 {
            continue;
        }
        let ix = nq.index_of(&[r, g, b, a]);
        let palette = nq.color_map_rgba();
        let base = ix * 4;
        *px = Rgba([palette[base], palette[base + 1], palette[base + 2], a]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantise_is_deterministic() {
        let mut a = RgbaImage::from_pixel(16, 16, Rgba([123, 200, 50, 255]));
        let mut b = a.clone();
        // Scribble a gradient so the quantiser has variety to work with.
        for (i, px) in a.pixels_mut().enumerate() {
            px.0 = [(i % 256) as u8, (i * 2 % 256) as u8, 80, 255];
        }
        for (i, px) in b.pixels_mut().enumerate() {
            px.0 = [(i % 256) as u8, (i * 2 % 256) as u8, 80, 255];
        }
        quantise_in_place(&mut a);
        quantise_in_place(&mut b);
        assert_eq!(a.as_raw(), b.as_raw());
    }

    #[test]
    fn transparent_pixels_are_preserved() {
        let mut img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 0]));
        // A handful of opaque pixels.
        img.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 1, Rgba([0, 255, 0, 255]));
        quantise_in_place(&mut img);
        assert_eq!(img.get_pixel(3, 3).0[3], 0);
    }

    #[test]
    fn output_uses_at_most_palette_colors() {
        let mut img = RgbaImage::new(32, 32);
        for (i, px) in img.pixels_mut().enumerate() {
            let v = ((i * 7) % 256) as u8;
            px.0 = [v, 255 - v, (v / 2) + 32, 255];
        }
        quantise_in_place(&mut img);
        let mut distinct = std::collections::HashSet::new();
        for px in img.pixels() {
            distinct.insert(px.0);
        }
        assert!(
            distinct.len() <= PALETTE_COLORS,
            "expected ≤ {} distinct colors, got {}",
            PALETTE_COLORS,
            distinct.len()
        );
    }
}
