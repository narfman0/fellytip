//! Stitch per-frame images into one atlas PNG.

use crate::generator::{FrameRequest, SpriteGenerator};
use crate::layout::AtlasLayout;
use crate::parallel::parallel_map;
use fellytip_shared::bestiary::BestiaryEntry;
use image::{Rgba, RgbaImage};

/// One cell request — captured up-front so the generator work can run in
/// parallel while atlas composition stays single-threaded.
struct Cell {
    row: u32,
    col: u32,
    slot_name: smol_str::SmolStr,
    direction: u32,
    frame: u32,
}

/// Result of one cell — `None` means the generator failed; that cell is left
/// transparent and logged so a later `--incremental` run can retry it.
#[derive(Default)]
struct CellResult {
    image: Option<RgbaImage>,
}

/// Run the generator for every (animation × direction × frame) cell, each in
/// its own worker, and copy each result into the correct atlas slot.
///
/// A single-frame failure is **logged and skipped** — the cell stays
/// transparent so a later `--incremental` rerun can retry it.
pub fn assemble_atlas(
    generator: &(dyn SpriteGenerator + Sync),
    entry: &BestiaryEntry,
    layout: &AtlasLayout,
    workers: usize,
) -> anyhow::Result<RgbaImage> {
    let cells = collect_cells(layout);
    let tile_size = layout.tile_size;
    let entity_id = entry.id.clone();

    let cells_owned: Vec<Cell> = cells;
    let generator_ref: &(dyn SpriteGenerator + Sync) = generator;

    let results = parallel_map(cells_owned, workers, |c| {
        let req = FrameRequest {
            entity_id: entity_id.as_str(),
            animation: c.slot_name.as_str(),
            direction: c.direction,
            frame: c.frame,
            tile_size,
        };
        match generator_ref.generate(req) {
            Ok(image) => CellResult { image: Some(image) },
            Err(e) => {
                tracing::warn!(
                    entity = %entity_id,
                    anim = %c.slot_name,
                    dir = c.direction,
                    frame = c.frame,
                    "sprite generate failed: {e:#}",
                );
                CellResult { image: None }
            }
        }
    });

    let mut atlas = RgbaImage::from_pixel(
        layout.image_width(),
        layout.image_height(),
        Rgba([0, 0, 0, 0]),
    );
    let mut failures = 0usize;
    // Re-iterate cells in the same order to know where to paste each result.
    for (idx, c) in collect_cells(layout).into_iter().enumerate() {
        let res = &results[idx];
        if let Some(img) = &res.image {
            let (ox, oy) = layout.cell_origin(c.row, c.col);
            copy_tile(&mut atlas, img, ox, oy);
        } else {
            failures += 1;
        }
    }
    if failures > 0 {
        tracing::warn!(entity = %entry.id, failures, "frames skipped; rerun with --incremental to retry");
    }

    Ok(atlas)
}

fn collect_cells(layout: &AtlasLayout) -> Vec<Cell> {
    let mut cells = Vec::new();
    for slot in &layout.animations {
        for dir in 0..layout.directions {
            for frame in 0..slot.frames {
                cells.push(Cell {
                    row: slot.row_start + dir,
                    col: frame,
                    slot_name: slot.name.clone(),
                    direction: dir,
                    frame,
                });
            }
        }
    }
    cells
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
        let atlas = assemble_atlas(&MockGenerator, &e, &l, 1).unwrap();
        assert_eq!(atlas.width(), l.image_width());
        assert_eq!(atlas.height(), l.image_height());
    }

    #[test]
    fn different_directions_produce_different_pixels() {
        let e = small_entry();
        let l = AtlasLayout::from_entry(&e);
        let atlas = assemble_atlas(&MockGenerator, &e, &l, 1).unwrap();
        let (x0, y0) = l.cell_origin(0, 0);
        let (x1, y1) = l.cell_origin(1, 0);
        assert_ne!(atlas.get_pixel(x0, y0), atlas.get_pixel(x1, y1));
    }

    #[test]
    fn worker_count_does_not_change_output() {
        let e = small_entry();
        let l = AtlasLayout::from_entry(&e);
        let serial = assemble_atlas(&MockGenerator, &e, &l, 1).unwrap();
        let parallel = assemble_atlas(&MockGenerator, &e, &l, 4).unwrap();
        assert_eq!(serial.as_raw(), parallel.as_raw());
    }

    /// A generator whose `generate` always fails — simulates an API outage.
    /// The assembler must produce an atlas (not propagate the error) and
    /// leave the failed cells transparent.
    struct AlwaysFail;
    impl SpriteGenerator for AlwaysFail {
        fn generate(&self, _req: FrameRequest<'_>) -> anyhow::Result<RgbaImage> {
            Err(anyhow::anyhow!("simulated outage"))
        }
    }

    #[test]
    fn failed_frames_become_transparent_not_fatal() {
        let e = small_entry();
        let l = AtlasLayout::from_entry(&e);
        let atlas = assemble_atlas(&AlwaysFail, &e, &l, 1).unwrap();
        // Every pixel should be fully transparent.
        assert!(atlas.pixels().all(|p| p.0[3] == 0));
    }
}
