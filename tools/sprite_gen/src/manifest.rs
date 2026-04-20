//! RON sidecar describing the atlas grid for the billboard renderer.
//!
//! Bevy's `TextureAtlasLayout` isn't directly serializable, so we ship the
//! grid parameters instead and let the renderer reconstruct the layout at
//! runtime via `TextureAtlasLayout::from_grid`.

use crate::layout::AtlasLayout;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtlasManifest {
    pub tile_size: u32,
    pub columns: u32,
    pub rows: u32,
    pub directions: u32,
    pub animations: Vec<AnimationManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationManifest {
    pub name: SmolStr,
    pub row_start: u32,
    pub frames: u32,
    pub fps: u32,
}

impl From<&AtlasLayout> for AtlasManifest {
    fn from(l: &AtlasLayout) -> Self {
        AtlasManifest {
            tile_size: l.tile_size,
            columns: l.columns,
            rows: l.rows,
            directions: l.directions,
            animations: l
                .animations
                .iter()
                .map(|a| AnimationManifest {
                    name: a.name.clone(),
                    row_start: a.row_start,
                    frames: a.frames,
                    fps: a.fps,
                })
                .collect(),
        }
    }
}

pub fn to_ron(m: &AtlasManifest) -> Result<String, ron::Error> {
    ron::ser::to_string_pretty(m, ron::ser::PrettyConfig::default())
}

pub fn from_ron(s: &str) -> Result<AtlasManifest, ron::de::SpannedError> {
    ron::from_str(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::AtlasLayout;
    use fellytip_shared::bestiary::{AnimationDef, BestiaryEntry};

    fn entry() -> BestiaryEntry {
        BestiaryEntry {
            id: "goblin_scout".into(),
            display_name: "Goblin Scout".into(),
            directions: 8,
            ai_prompt_base: "p".into(),
            ai_style: "s".into(),
            palette_seed: "g".into(),
            animations: vec![
                AnimationDef { name: "idle".into(),  frames: 4, fps: 4 },
                AnimationDef { name: "walk".into(),  frames: 8, fps: 12 },
            ],
        }
    }

    #[test]
    fn manifest_mirrors_layout() {
        let e = entry();
        let l = AtlasLayout::from_entry(&e);
        let m = AtlasManifest::from(&l);
        assert_eq!(m.tile_size, l.tile_size);
        assert_eq!(m.columns, l.columns);
        assert_eq!(m.rows, l.rows);
        assert_eq!(m.directions, l.directions);
        assert_eq!(m.animations.len(), 2);
        assert_eq!(m.animations[0].row_start, 0);
        assert_eq!(m.animations[1].row_start, 8);
    }

    #[test]
    fn ron_roundtrip() {
        let e = entry();
        let l = AtlasLayout::from_entry(&e);
        let m = AtlasManifest::from(&l);
        let s = to_ron(&m).unwrap();
        let back = from_ron(&s).unwrap();
        assert_eq!(m, back);
    }
}
