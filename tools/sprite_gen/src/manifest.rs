//! Writes JSON manifests consumed by the client's `BillboardSpritePlugin`.
//!
//! Uses `SpriteManifest` from `fellytip_shared::bestiary` — that type is the
//! contract shared between the generator (writer) and the renderer (reader).

use anyhow::Result;
use fellytip_shared::bestiary::{BestiaryEntry, ManifestAnim, SpriteManifest};
use std::path::Path;

/// Build a `SpriteManifest` from a bestiary entry and the chosen frame size.
pub fn manifest_from_entry(entry: &BestiaryEntry, frame_size: u32) -> SpriteManifest {
    let mut animations = Vec::new();
    let mut col = 0u32;
    for anim in &entry.animations {
        animations.push(ManifestAnim {
            name:      anim.name.clone(),
            start_col: col,
            frames:    anim.frames,
            fps:       anim.fps,
        });
        col += anim.frames;
    }
    SpriteManifest {
        entity_id:    entry.id.clone(),
        frame_width:  frame_size,
        frame_height: frame_size,
        atlas_cols:   col,
        atlas_rows:   entry.directions as u32,
        animations,
    }
}

/// Write `manifest` as `{output_dir}/{entity_id}/manifest.json`.
pub fn write_manifest(manifest: &SpriteManifest, output_dir: &Path) -> Result<()> {
    let dir = output_dir.join(&manifest.entity_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("manifest.json");
    let json = serde_json::to_string_pretty(manifest)?;
    std::fs::write(&path, json)?;
    tracing::info!("Wrote manifest → {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fellytip_shared::bestiary::{AnimationDef, BestiaryEntry};

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
            ai_prompt_base: "goblin".into(),
            ai_style: "pixel art".into(),
            palette_seed: "green".into(),
        }
    }

    #[test]
    fn manifest_cols_sum() {
        let m = manifest_from_entry(&sample_entry(), 64);
        assert_eq!(m.atlas_cols, 4 + 8 + 5);
    }

    #[test]
    fn manifest_rows_equals_directions() {
        let m = manifest_from_entry(&sample_entry(), 64);
        assert_eq!(m.atlas_rows, 8);
    }

    #[test]
    fn manifest_anim_start_cols() {
        let m = manifest_from_entry(&sample_entry(), 64);
        assert_eq!(m.animations[0].start_col, 0);
        assert_eq!(m.animations[1].start_col, 4);
        assert_eq!(m.animations[2].start_col, 12);
    }

    #[test]
    fn manifest_roundtrip_json() {
        let m = manifest_from_entry(&sample_entry(), 64);
        let json = serde_json::to_string(&m).unwrap();
        let decoded: SpriteManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.entity_id, "goblin");
        assert_eq!(decoded.animations.len(), 3);
    }
}
