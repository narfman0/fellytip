//! Post-processing steps applied to generated sprite frames before display/save.

use image::{Rgba, RgbaImage};
use std::collections::VecDeque;

/// Erase the background of a sprite by flood-filling inward from each corner.
///
/// Samples the color at all four corners, then BFS-expands from each corner
/// making pixels transparent when their Euclidean RGB distance to the seed
/// color is within `tolerance`.  Pixels already transparent are skipped.
pub fn remove_background(img: &mut RgbaImage, tolerance: u8) {
    let w = img.width();
    let h = img.height();
    if w == 0 || h == 0 {
        return;
    }

    let corners = [
        (0u32, 0u32),
        (w - 1, 0),
        (0, h - 1),
        (w - 1, h - 1),
    ];

    let tol_sq = (tolerance as u32) * (tolerance as u32);
    let mut visited = vec![false; (w * h) as usize];

    for (cx, cy) in corners {
        let seed = *img.get_pixel(cx, cy);
        if seed[3] == 0 {
            continue; // corner is already transparent; nothing to flood
        }

        let mut queue: VecDeque<(u32, u32)> = VecDeque::new();
        let idx = (cy * w + cx) as usize;
        if visited[idx] {
            continue;
        }
        visited[idx] = true;
        queue.push_back((cx, cy));

        while let Some((x, y)) = queue.pop_front() {
            let px = *img.get_pixel(x, y);
            if px[3] == 0 || color_dist_sq(px, seed) <= tol_sq {
                img.put_pixel(x, y, Rgba([0, 0, 0, 0]));
                for (nx, ny) in neighbours(x, y, w, h).into_iter().flatten() {
                    let ni = (ny * w + nx) as usize;
                    if !visited[ni] {
                        visited[ni] = true;
                        queue.push_back((nx, ny));
                    }
                }
            }
        }
    }
}

fn color_dist_sq(a: Rgba<u8>, b: Rgba<u8>) -> u32 {
    let dr = a[0].abs_diff(b[0]) as u32;
    let dg = a[1].abs_diff(b[1]) as u32;
    let db = a[2].abs_diff(b[2]) as u32;
    dr * dr + dg * dg + db * db
}

fn neighbours(x: u32, y: u32, w: u32, h: u32) -> [Option<(u32, u32)>; 4] {
    [
        if x + 1 < w { Some((x + 1, y)) } else { None },
        if y + 1 < h { Some((x, y + 1)) } else { None },
        if x > 0     { Some((x - 1, y)) } else { None },
        if y > 0     { Some((x, y - 1)) } else { None },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(r: u8, g: u8, b: u8, w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([r, g, b, 255]))
    }

    #[test]
    fn solid_image_becomes_fully_transparent() {
        let mut img = solid(200, 200, 200, 8, 8);
        remove_background(&mut img, 40);
        assert!(img.pixels().all(|p| p[3] == 0));
    }

    #[test]
    fn center_pixel_survives_when_different_from_corners() {
        let mut img = solid(200, 200, 200, 9, 9);
        // Paint the center pixel a very different color
        img.put_pixel(4, 4, Rgba([0, 128, 255, 255]));
        remove_background(&mut img, 40);
        // Corners should be transparent
        assert_eq!(img.get_pixel(0, 0)[3], 0);
        // Center should survive
        assert_eq!(img.get_pixel(4, 4)[3], 255);
    }

    #[test]
    fn already_transparent_pixels_unchanged() {
        let mut img = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0]));
        remove_background(&mut img, 40);
        assert!(img.pixels().all(|p| p[3] == 0));
    }
}
