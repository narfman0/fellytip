//! Bestiary types: single source of truth for entity sprite-sheet definitions.
//!
//! These structs mirror `assets/bestiary.toml`.  Pure data — no ECS, no I/O.
//! The `sprite_gen` tool reads this via `toml::from_str::<Bestiary>(&src)`.
//! The client billboard plugin reads the generated manifests instead.

use serde::{Deserialize, Serialize};

/// One animation clip definition (e.g. "walk", 8 frames at 12 fps).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationDef {
    pub name: String,
    pub frames: u32,
    pub fps: u32,
}

/// Full definition of one entity type in the bestiary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BestiaryEntry {
    /// Stable identifier used as the sprite sheet directory name.
    pub id: String,
    pub display_name: String,
    /// Number of facing directions (typically 8).
    pub directions: u8,
    pub animations: Vec<AnimationDef>,
    /// Base descriptive text for the AI prompt (style is appended separately).
    pub ai_prompt_base: String,
    /// Art-style suffix appended to every AI prompt for this entity.
    pub ai_style: String,
    /// Colour palette tag used for post-processing quantisation.
    pub palette_seed: String,
}

impl BestiaryEntry {
    /// Total number of atlas columns (sum of all animation frame counts).
    pub fn total_cols(&self) -> u32 {
        self.animations.iter().map(|a| a.frames).sum()
    }

    /// Starting atlas column for the named animation, or `None` if not found.
    pub fn anim_start_col(&self, name: &str) -> Option<u32> {
        let mut col = 0u32;
        for anim in &self.animations {
            if anim.name == name {
                return Some(col);
            }
            col += anim.frames;
        }
        None
    }

    /// Build the full AI prompt for a given direction index (0 = south, clockwise).
    pub fn prompt_for_direction(&self, dir: u8) -> String {
        const DIR_LABELS: [&str; 8] = [
            "facing south",
            "facing southwest",
            "facing west",
            "facing northwest",
            "facing north",
            "facing northeast",
            "facing east",
            "facing southeast",
        ];
        let dir_label = DIR_LABELS.get(dir as usize).copied().unwrap_or("facing south");
        format!("{}, {}, {}", self.ai_prompt_base, dir_label, self.ai_style)
    }
}

/// The full bestiary deserialized from `assets/bestiary.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Bestiary {
    /// TOML key is `[[entity]]` — a list of entries.
    pub entity: Vec<BestiaryEntry>,
}

impl Bestiary {
    /// Look up an entry by its stable ID.
    pub fn get(&self, id: &str) -> Option<&BestiaryEntry> {
        self.entity.iter().find(|e| e.id == id)
    }
}

// ── Sprite manifest (written by sprite_gen, read by BillboardSpritePlugin) ────

/// Per-animation entry stored in the generated manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestAnim {
    pub name: String,
    /// First atlas column index for this animation.
    pub start_col: u32,
    pub frames: u32,
    pub fps: u32,
}

/// Full sprite-sheet manifest written by `sprite_gen` alongside `atlas.png`.
///
/// One manifest lives at `assets/sprites/{entity_id}/manifest.json`.
/// The client's `BillboardSpritePlugin` reads these at startup to populate
/// `SpriteRegistry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteManifest {
    pub entity_id: String,
    pub frame_width: u32,
    pub frame_height: u32,
    /// Total atlas columns (= sum of all animation frame counts).
    pub atlas_cols: u32,
    /// Total atlas rows (= directions).
    pub atlas_rows: u32,
    pub animations: Vec<ManifestAnim>,
}

impl SpriteManifest {
    /// Find the animation entry by name.
    pub fn animation(&self, name: &str) -> Option<&ManifestAnim> {
        self.animations.iter().find(|a| a.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> BestiaryEntry {
        BestiaryEntry {
            id: "goblin".into(),
            display_name: "Goblin".into(),
            directions: 8,
            animations: vec![
                AnimationDef { name: "idle".into(),   frames: 4, fps: 4  },
                AnimationDef { name: "walk".into(),   frames: 8, fps: 12 },
                AnimationDef { name: "attack".into(), frames: 5, fps: 10 },
            ],
            ai_prompt_base: "goblin warrior".into(),
            ai_style: "pixel art, 64x64".into(),
            palette_seed: "goblin_green".into(),
        }
    }

    #[test]
    fn total_cols_sums_frames() {
        let e = sample_entry();
        assert_eq!(e.total_cols(), 4 + 8 + 5);
    }

    #[test]
    fn anim_start_col_idle() {
        let e = sample_entry();
        assert_eq!(e.anim_start_col("idle"), Some(0));
    }

    #[test]
    fn anim_start_col_walk() {
        let e = sample_entry();
        assert_eq!(e.anim_start_col("walk"), Some(4));
    }

    #[test]
    fn anim_start_col_attack() {
        let e = sample_entry();
        assert_eq!(e.anim_start_col("attack"), Some(12));
    }

    #[test]
    fn anim_start_col_missing() {
        let e = sample_entry();
        assert_eq!(e.anim_start_col("death"), None);
    }

    #[test]
    fn prompt_includes_direction_label() {
        let e = sample_entry();
        let prompt = e.prompt_for_direction(0);
        assert!(prompt.contains("facing south"));
        assert!(prompt.contains("goblin warrior"));
    }

    #[test]
    fn bestiary_get() {
        let b = Bestiary {
            entity: vec![sample_entry()],
        };
        assert!(b.get("goblin").is_some());
        assert!(b.get("dragon").is_none());
    }
}
