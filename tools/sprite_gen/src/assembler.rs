//! Stitch per-frame images into one atlas PNG.

use crate::generator::{FrameRequest, SpriteGenerator};
use crate::layout::AtlasLayout;
use anyhow::Context;
use fellytip_shared::bestiary::BestiaryEntry;
use image::RgbaImage;

/// Run the generator for every (animation × direction × frame) cell and copy
/// each result into the correct atlas slot.  Pure in-memory; no file I/O.
pub fn assemble_atlas(
    generator: &dyn SpriteGenerator,
    entry: &BestiaryEntry,
    layout: &AtlasLayout,
) -> anyhow::Result<RgbaImage> {
    let mut atlas = RgbaImage::new(layout.image_width(), layout.image_height());
    for slot in &layout.animations {
        for dir in 0..layout.directions {
            let row = slot.row_start + dir;
            for frame in 0..slot.frames {
                let img = generator
                    .generate(FrameRequest {
                        entity_id: entry.id.as_str(),
                        animation: slot.name.as_str(),
                        direction: dir,
                        frame,
                        tile_size: layout.tile_size,
                    })
                    .with_context(|| {
                        format!(
                            "generating {}/{}/dir{}/frame{}",
                            entry.id, slot.name, dir, frame
                        )
                    })?;
                let (ox, oy) = layout.cell_origin(row, frame);
                copy_tile(&mut atlas, &img, ox, oy);
            }
        }
    }
    Ok(atlas)
}

fn copy_tile(dst: &mut RgbaImage, src: &RgbaImage, ox: u32, oy: u32) {
    for y in 0..src.height() {
        for x in 0..src.width() {
            dst.put_pixel(ox + x, oy + y, *src.get_pixel(x, y));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::MockGenerator;
    use fellytip_shared::bestiary::{AnimationDef, BestiaryEntry};

    fn small_entry() -> BestiaryEntry {
        BestiaryEntry {
            id: "tiny".into(),
            display_name: "Tiny".into(),
            directions: 4,
            ai_prompt_base: "p".into(),
            ai_style: "s".into(),
            palette_seed: "g".into(),
            animations: vec![AnimationDef {
                name: "idle".into(),
                frames: 2,
                fps: 1,
            }],
        }
    }

    #[test]
    fn atlas_dimensions_match_layout() {
        let e = small_entry();
        let l = AtlasLayout::from_entry(&e);
        let atlas = assemble_atlas(&MockGenerator, &e, &l).unwrap();
        assert_eq!(atlas.width(), l.image_width());
        assert_eq!(atlas.height(), l.image_height());
    }

    #[test]
    fn different_directions_produce_different_pixels() {
        let e = small_entry();
        let l = AtlasLayout::from_entry(&e);
        let atlas = assemble_atlas(&MockGenerator, &e, &l).unwrap();
        // Sample the top-left pixel of the dir=0 tile and the dir=1 tile.
        let (x0, y0) = l.cell_origin(0, 0); // anim 0, dir 0, frame 0
        let (x1, y1) = l.cell_origin(1, 0); // anim 0, dir 1, frame 0
        assert_ne!(
            atlas.get_pixel(x0, y0),
            atlas.get_pixel(x1, y1),
            "dir 0 and dir 1 should produce different colors"
        );
    }
}
