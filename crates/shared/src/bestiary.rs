//! Pure-data bestiary schema — the single source of truth for AI sprite
//! generation and billboard rendering.
//!
//! This module is deliberately ECS-free; Bevy never sees these types.  The
//! `sprite_gen` tool loads `assets/bestiary.toml` to drive image generation,
//! and the client billboard renderer keys atlas lookups on the same ids.

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::collections::HashSet;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BestiaryEntry {
    pub id: SmolStr,
    pub display_name: String,
    pub directions: u8,
    pub ai_prompt_base: String,
    pub ai_style: String,
    pub palette_seed: String,
    #[serde(rename = "animation", default)]
    pub animations: Vec<AnimationDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationDef {
    pub name: SmolStr,
    pub frames: u16,
    pub fps: u8,
}

#[derive(Debug, Serialize, Deserialize)]
struct BestiaryFile {
    #[serde(rename = "entity", default)]
    entries: Vec<BestiaryEntry>,
}

#[derive(Debug, Error)]
pub enum BestiaryError {
    #[error("reading {path}: {source}")]
    Io { path: String, source: std::io::Error },

    #[error("parsing bestiary TOML: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("duplicate entity id `{0}`")]
    DuplicateId(SmolStr),

    #[error("entity `{0}` must declare at least one animation")]
    NoAnimations(SmolStr),

    #[error("entity `{id}` has invalid directions value {value} (expected 4 or 8)")]
    BadDirections { id: SmolStr, value: u8 },

    #[error("entity `{entity}` animation `{animation}` has zero frames")]
    ZeroFrames { entity: SmolStr, animation: SmolStr },
}

/// Load and validate `bestiary.toml` from disk.
pub fn load_bestiary(path: &Path) -> Result<Vec<BestiaryEntry>, BestiaryError> {
    let text = std::fs::read_to_string(path).map_err(|e| BestiaryError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    parse_bestiary(&text)
}

/// Parse and validate bestiary text.  Split from `load_bestiary` so tests
/// can exercise it without touching the filesystem.
pub fn parse_bestiary(text: &str) -> Result<Vec<BestiaryEntry>, BestiaryError> {
    let file: BestiaryFile = toml::from_str(text)?;
    validate(&file.entries)?;
    Ok(file.entries)
}

fn validate(entries: &[BestiaryEntry]) -> Result<(), BestiaryError> {
    let mut seen: HashSet<&SmolStr> = HashSet::new();
    for e in entries {
        if !seen.insert(&e.id) {
            return Err(BestiaryError::DuplicateId(e.id.clone()));
        }
        if e.directions != 4 && e.directions != 8 {
            return Err(BestiaryError::BadDirections {
                id: e.id.clone(),
                value: e.directions,
            });
        }
        if e.animations.is_empty() {
            return Err(BestiaryError::NoAnimations(e.id.clone()));
        }
        for a in &e.animations {
            if a.frames == 0 {
                return Err(BestiaryError::ZeroFrames {
                    entity: e.id.clone(),
                    animation: a.name.clone(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[[entity]]
id = "goblin_scout"
display_name = "Goblin Scout"
directions = 8
ai_prompt_base = "goblin scout"
ai_style = "pixel art"
palette_seed = "forest_green"

[[entity.animation]]
name = "idle"
frames = 4
fps = 4
"#;

    #[test]
    fn parses_minimal_bestiary() {
        let out = parse_bestiary(MINIMAL).expect("should parse");
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.id.as_str(), "goblin_scout");
        assert_eq!(e.display_name, "Goblin Scout");
        assert_eq!(e.directions, 8);
        assert_eq!(e.animations.len(), 1);
        assert_eq!(e.animations[0].name.as_str(), "idle");
        assert_eq!(e.animations[0].frames, 4);
    }

    #[test]
    fn roundtrip_serialize_deserialize() {
        let original = BestiaryEntry {
            id: "orc_grunt".into(),
            display_name: "Orc Grunt".into(),
            directions: 8,
            ai_prompt_base: "orc with axe".into(),
            ai_style: "pixel art".into(),
            palette_seed: "mud".into(),
            animations: vec![AnimationDef {
                name: "walk".into(),
                frames: 6,
                fps: 10,
            }],
        };
        let wrapped = BestiaryFile {
            entries: vec![original.clone()],
        };
        let s = toml::to_string(&wrapped).unwrap();
        let parsed = parse_bestiary(&s).unwrap();
        assert_eq!(parsed, vec![original]);
    }

    #[test]
    fn rejects_malformed_toml() {
        let err = parse_bestiary("not [[ valid = ").unwrap_err();
        assert!(matches!(err, BestiaryError::Parse(_)));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let src = format!("{MINIMAL}\n{MINIMAL}");
        let err = parse_bestiary(&src).unwrap_err();
        assert!(matches!(err, BestiaryError::DuplicateId(ref id) if id.as_str() == "goblin_scout"));
    }

    #[test]
    fn rejects_missing_animations() {
        let src = r#"
[[entity]]
id = "empty"
display_name = "Empty"
directions = 8
ai_prompt_base = "x"
ai_style = "y"
palette_seed = "z"
"#;
        let err = parse_bestiary(src).unwrap_err();
        assert!(matches!(err, BestiaryError::NoAnimations(ref id) if id.as_str() == "empty"));
    }

    #[test]
    fn rejects_bad_directions() {
        let src = r#"
[[entity]]
id = "weird"
display_name = "Weird"
directions = 6
ai_prompt_base = "x"
ai_style = "y"
palette_seed = "z"

[[entity.animation]]
name = "idle"
frames = 1
fps = 1
"#;
        let err = parse_bestiary(src).unwrap_err();
        assert!(matches!(err, BestiaryError::BadDirections { value: 6, .. }));
    }

    #[test]
    fn rejects_zero_frame_animation() {
        let src = r#"
[[entity]]
id = "zero"
display_name = "Zero"
directions = 8
ai_prompt_base = "x"
ai_style = "y"
palette_seed = "z"

[[entity.animation]]
name = "idle"
frames = 0
fps = 1
"#;
        let err = parse_bestiary(src).unwrap_err();
        assert!(matches!(err, BestiaryError::ZeroFrames { .. }));
    }

    #[test]
    fn repo_bestiary_toml_is_valid() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/bestiary.toml");
        let entries = load_bestiary(&path).expect("repo bestiary.toml must parse");
        assert!(!entries.is_empty(), "bestiary must declare at least one entity");
    }
}
