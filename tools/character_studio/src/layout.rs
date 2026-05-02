//! Atlas layout math — pure, deterministic, no I/O.
//!
//! A single entity's atlas is a grid of `(animations × directions)` rows and
//! `max_frames` columns.  Row-major:
//!
//!   row(anim_index, dir) = anim_index * directions + dir
//!   col(frame)           = frame
//!
//! For animations shorter than `max_frames` the trailing cells are unused
//! (transparent).  This wastes a few pixels but keeps one PNG per entity and
//! lets the billboard renderer use a single `TextureAtlasLayout::from_grid`.

use fellytip_shared::bestiary::BestiaryEntry;

/// Size in pixels of one atlas cell (per frame image).
pub const TILE_SIZE: u32 = 128;

/// Layout of one entity's atlas.  Pure data derived from a [`BestiaryEntry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasLayout {
    pub tile_size: u32,
    pub columns: u32,
    pub rows: u32,
    pub directions: u32,
    pub animations: Vec<AnimationSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimationSlot {
    pub name: smol_str::SmolStr,
    /// First row used by this animation (direction 0 sits at `row_start`,
    /// direction `d` at `row_start + d`).
    pub row_start: u32,
    pub frames: u32,
    pub fps: u32,
}

impl AtlasLayout {
    pub fn from_entry(entry: &BestiaryEntry) -> Self {
        let directions = entry.directions as u32;
        let columns = entry
            .animations
            .iter()
            .map(|a| a.frames as u32)
            .max()
            .unwrap_or(1);

        let mut animations = Vec::with_capacity(entry.animations.len());
        let mut cursor = 0u32;
        for a in &entry.animations {
            animations.push(AnimationSlot {
                name: a.name.clone(),
                row_start: cursor,
                frames: a.frames as u32,
                fps: a.fps as u32,
            });
            cursor += directions;
        }

        AtlasLayout {
            tile_size: TILE_SIZE,
            columns,
            rows: cursor,
            directions,
            animations,
        }
    }

    pub fn image_width(&self) -> u32 {
        self.columns * self.tile_size
    }

    pub fn image_height(&self) -> u32 {
        self.rows * self.tile_size
    }

    /// Pixel origin for the cell at `(row, col)`.
    pub fn cell_origin(&self, row: u32, col: u32) -> (u32, u32) {
        (col * self.tile_size, row * self.tile_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fellytip_shared::bestiary::AnimationDef;

    fn entry(directions: u8, frames: &[u16]) -> BestiaryEntry {
        BestiaryEntry {
            id: "test".into(),
            display_name: "Test".into(),
            directions,
            ai_prompt_base: "x".into(),
            ai_style: "y".into(),
            palette_seed: "z".into(),
            animations: frames
                .iter()
                .enumerate()
                .map(|(i, f)| AnimationDef {
                    name: format!("a{i}").into(),
                    frames: *f,
                    fps: 10,
                })
                .collect(),
            animation_ids: vec![],
            mesh_prompt: String::new(),
        }
    }

    #[test]
    fn row_col_math_matches_spec() {
        // 8 directions, 3 animations with 4, 8, 5 frames.
        let e = entry(8, &[4, 8, 5]);
        let l = AtlasLayout::from_entry(&e);
        assert_eq!(l.directions, 8);
        assert_eq!(l.columns, 8);
        assert_eq!(l.rows, 24); // 3 animations × 8 directions
        assert_eq!(l.animations[0].row_start, 0);
        assert_eq!(l.animations[1].row_start, 8);
        assert_eq!(l.animations[2].row_start, 16);
    }

    #[test]
    fn image_dimensions_are_grid_times_tile() {
        let l = AtlasLayout::from_entry(&entry(4, &[3, 7]));
        assert_eq!(l.tile_size, TILE_SIZE);
        assert_eq!(l.columns, 7);
        assert_eq!(l.rows, 8); // 2 animations × 4 directions
        assert_eq!(l.image_width(), 7 * TILE_SIZE);
        assert_eq!(l.image_height(), 8 * TILE_SIZE);
    }

    #[test]
    fn cell_origin_is_top_left_corner() {
        let l = AtlasLayout::from_entry(&entry(8, &[4]));
        assert_eq!(l.cell_origin(0, 0), (0, 0));
        assert_eq!(l.cell_origin(3, 2), (2 * TILE_SIZE, 3 * TILE_SIZE));
    }
}
