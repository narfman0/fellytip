//! Pure-data bestiary schema — the single source of truth for AI sprite
//! generation and billboard rendering.
//!
//! This module is deliberately ECS-free; Bevy never sees these types.  The
//! `sprite_studio` tool loads `assets/bestiary.toml` to drive image generation,
//! and the client billboard renderer keys atlas lookups on the same ids.

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::collections::HashSet;
use std::path::Path;
use thiserror::Error;

fn default_directions() -> u8 { 8 }

/// A named art-style preset.  Defined once in `[[styles]]` at the top of
/// `bestiary.toml` and referenced by name in each entity's `ai_style` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StylePreset {
    pub name: SmolStr,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BestiaryEntry {
    pub id: SmolStr,
    pub display_name: String,
    #[serde(default = "default_directions")]
    pub directions: u8,
    pub ai_prompt_base: String,
    /// Name of a `StylePreset` defined in `[[styles]]`, or a literal style
    /// string if no matching preset exists.
    pub ai_style: SmolStr,
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

/// Parsed and validated bestiary: style presets + entity definitions.
#[derive(Debug, Clone)]
pub struct Bestiary {
    pub styles: Vec<StylePreset>,
    pub entries: Vec<BestiaryEntry>,
}

impl Bestiary {
    /// Resolve a style key to its value string.  Falls back to the key itself
    /// so literal style strings in tests / old files still work.
    pub fn resolve_style<'a>(&'a self, key: &'a str) -> &'a str {
        self.styles
            .iter()
            .find(|s| s.name.as_str() == key)
            .map(|s| s.value.as_str())
            .unwrap_or(key)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct BestiaryFile {
    #[serde(rename = "styles", default)]
    styles: Vec<StylePreset>,
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

    #[error("entity `{entity}` animation `{animation}` has zero frames")]
    ZeroFrames { entity: SmolStr, animation: SmolStr },
}

/// Load and validate `bestiary.toml` from disk.
pub fn load_bestiary(path: &Path) -> Result<Bestiary, BestiaryError> {
    let text = std::fs::read_to_string(path).map_err(|e| BestiaryError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    parse_bestiary(&text)
}

/// Parse and validate bestiary text.  Split from `load_bestiary` so tests
/// can exercise it without touching the filesystem.
pub fn parse_bestiary(text: &str) -> Result<Bestiary, BestiaryError> {
    let file: BestiaryFile = toml::from_str(text)?;
    validate(&file.entries)?;
    Ok(Bestiary { styles: file.styles, entries: file.entries })
}

/// Bestiary ids that every in-game entity kind needs an entry for.  This
/// list is the drift guard: add a new `EntityKind` / `WildlifeKind` variant
/// or faction without updating `assets/bestiary.toml` and the
/// `bestiary_covers_all_entity_kinds` test fails.
///
/// Resolution rules (consumed by the billboard renderer):
///
/// - No `EntityKind` on the replicated entity → `"hero"` (local player).
/// - `EntityKind::FactionNpc` + `FactionBadge::faction_id` → `"{faction_id}_npc"`.
/// - `EntityKind::Wildlife` + `WildlifeKind::{variant}` → lowercase variant name.
/// - `EntityKind::Settlement` → no billboard; rendered by the PBR pipeline.
pub const REQUIRED_BESTIARY_IDS: &[&str] = &[
    "hero",
    "ash_covenant_npc",
    "deep_tide_npc",
    "iron_wolves_npc",
    "merchant_guild_npc",
    "bison",
    "dog",
    "horse",
];

fn validate(entries: &[BestiaryEntry]) -> Result<(), BestiaryError> {
    let mut seen: HashSet<&SmolStr> = HashSet::new();
    for e in entries {
        if !seen.insert(&e.id) {
            return Err(BestiaryError::DuplicateId(e.id.clone()));
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
ai_prompt_base = "goblin scout"
ai_style = "pixel art"
palette_seed = "forest_green"
"#;

    #[test]
    fn parses_minimal_bestiary() {
        let b = parse_bestiary(MINIMAL).expect("should parse");
        assert_eq!(b.entries.len(), 1);
        let e = &b.entries[0];
        assert_eq!(e.id.as_str(), "goblin_scout");
        assert_eq!(e.display_name, "Goblin Scout");
        assert!(e.animations.is_empty());
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
            styles: vec![],
            entries: vec![original.clone()],
        };
        let s = toml::to_string(&wrapped).unwrap();
        let parsed = parse_bestiary(&s).unwrap();
        assert_eq!(parsed.entries, vec![original]);
    }

    #[test]
    fn rejects_malformed_toml() {
        let err = parse_bestiary("not [[ valid = ").unwrap_err();
        assert!(matches!(err, BestiaryError::Parse(_)));
    }

    #[test]
    fn styles_resolve_by_name() {
        let src = r#"
[[styles]]
name  = "tiny"
value = "pixel art, 32x32"

[[entity]]
id             = "goblin_scout"
display_name   = "Goblin Scout"
ai_prompt_base = "goblin scout"
ai_style       = "tiny"
palette_seed   = "forest_green"
"#;
        let b = parse_bestiary(src).unwrap();
        assert_eq!(b.resolve_style("tiny"), "pixel art, 32x32");
        assert_eq!(b.entries[0].ai_style.as_str(), "tiny");
    }

    #[test]
    fn rejects_duplicate_ids() {
        let src = format!("{MINIMAL}\n{MINIMAL}");
        let err = parse_bestiary(&src).unwrap_err();
        assert!(matches!(err, BestiaryError::DuplicateId(ref id) if id.as_str() == "goblin_scout"));
    }

    #[test]
    fn rejects_zero_frame_animation() {
        let src = r#"
[[entity]]
id = "zero"
display_name = "Zero"
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
        let b = load_bestiary(&path).expect("repo bestiary.toml must parse");
        assert!(!b.entries.is_empty(), "bestiary must declare at least one entity");
    }

    /// Drift guard: every id the client renderer expects must be present in
    /// the checked-in bestiary.  Adding a new `EntityKind` / `WildlifeKind`
    /// variant (or a new faction) without updating `assets/bestiary.toml`
    /// fails here.
    #[test]
    fn bestiary_covers_all_entity_kinds() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/bestiary.toml");
        let b = load_bestiary(&path).expect("repo bestiary.toml must parse");
        let ids: std::collections::HashSet<&str> =
            b.entries.iter().map(|e| e.id.as_str()).collect();
        for required in REQUIRED_BESTIARY_IDS {
            assert!(
                ids.contains(required),
                "bestiary.toml is missing required entity id `{required}` — \
                 add a `[[entity]]` block or remove the variant from \
                 REQUIRED_BESTIARY_IDS"
            );
        }
    }
}
