//! Atlas assembler: stitches per-frame `DynamicImage`s into a single sprite sheet.
//!
//! Atlas layout:
//!   Rows   = directions (e.g. 8 for 8-way facing)
//!   Cols   = total animation frames across all clips (idle + walk + attack + …)
//!
//! Frame order within a row:
//!   [idle_0 … idle_N | walk_0 … walk_M | attack_0 … attack_K | …]
//!
//! All frames must be the same pixel dimensions.

use anyhow::{bail, Result};
use image::{DynamicImage, RgbaImage, Rgba};

/// Build the final atlas image.
///
/// `frames[dir][frame_index]` — outer vec is direction (row), inner is column.
/// All images must share the same `frame_w × frame_h` dimensions.
pub fn assemble_atlas(
    frames: &[Vec<DynamicImage>],
    frame_w: u32,
    frame_h: u32,
) -> Result<DynamicImage> {
    if frames.is_empty() {
        bail!("No directions provided to assemble_atlas");
    }
    let n_dirs = frames.len() as u32;
    let n_cols = frames[0].len() as u32;
    if n_cols == 0 {
        bail!("Direction 0 has no frames");
    }

    let atlas_w = n_cols * frame_w;
    let atlas_h = n_dirs * frame_h;
    let mut atlas = RgbaImage::new(atlas_w, atlas_h);

    // Fill with transparent pixels (RgbaImage default is zero = transparent black).
    for pixel in atlas.pixels_mut() {
        *pixel = Rgba([0, 0, 0, 0]);
    }

    for (dir, dir_frames) in frames.iter().enumerate() {
        let y_off = dir as u32 * frame_h;
        for (col, frame) in dir_frames.iter().enumerate() {
            let x_off = col as u32 * frame_w;
            let rgba = frame.to_rgba8();
            for py in 0..frame_h {
                for px in 0..frame_w {
                    if px < rgba.width() && py < rgba.height() {
                        atlas.put_pixel(x_off + px, y_off + py, *rgba.get_pixel(px, py));
                    }
                }
            }
        }
    }

    Ok(DynamicImage::ImageRgba8(atlas))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(r: u8, g: u8, b: u8, w: u32, h: u32) -> DynamicImage {
        let mut img = RgbaImage::new(w, h);
        for pixel in img.pixels_mut() {
            *pixel = Rgba([r, g, b, 255]);
        }
        DynamicImage::ImageRgba8(img)
    }

    #[test]
    fn atlas_dimensions_correct() {
        // 2 directions, 3 frames each, 4×4 frames → 12×8 atlas
        let frames = vec![
            vec![solid(255, 0, 0, 4, 4), solid(0, 255, 0, 4, 4), solid(0, 0, 255, 4, 4)],
            vec![solid(128, 0, 0, 4, 4), solid(0, 128, 0, 4, 4), solid(0, 0, 128, 4, 4)],
        ];
        let atlas = assemble_atlas(&frames, 4, 4).unwrap();
        assert_eq!(atlas.width(), 12);
        assert_eq!(atlas.height(), 8);
    }

    #[test]
    fn atlas_pixels_placed_correctly() {
        let red   = solid(255, 0, 0, 2, 2);
        let green = solid(0, 255, 0, 2, 2);
        let frames = vec![vec![red, green]];
        let atlas = assemble_atlas(&frames, 2, 2).unwrap().to_rgba8();

        // First frame (red) at (0,0)
        assert_eq!(atlas.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
        assert_eq!(atlas.get_pixel(1, 1), &Rgba([255, 0, 0, 255]));
        // Second frame (green) at (2,0)
        assert_eq!(atlas.get_pixel(2, 0), &Rgba([0, 255, 0, 255]));
        assert_eq!(atlas.get_pixel(3, 1), &Rgba([0, 255, 0, 255]));
    }

    #[test]
    fn empty_directions_errors() {
        let result = assemble_atlas(&[], 64, 64);
        assert!(result.is_err());
    }
}
