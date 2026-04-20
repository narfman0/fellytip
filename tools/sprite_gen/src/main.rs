//! sprite_gen — generate 8-direction sprite atlases from `assets/bestiary.toml`.
//!
//! Usage:
//!   cargo run -p sprite_gen -- [--all | --entity ID] [--output-dir DIR]
//!                              [--bestiary PATH] [--dry-run] [--incremental]
//!
//! The default backend is the deterministic `MockGenerator`.  The real AI
//! backend is wired up in a follow-up (issue #18).

use anyhow::{anyhow, Context, Result};
use fellytip_shared::bestiary::{load_bestiary, BestiaryEntry};
use sprite_gen::{
    assembler::assemble_atlas,
    generator::{FrameRequest, MockGenerator, SpriteGenerator},
    incremental::can_skip,
    layout::AtlasLayout,
    manifest::{to_ron, AtlasManifest},
};
use std::path::{Path, PathBuf};

struct Args {
    all: bool,
    entity: Option<String>,
    bestiary: PathBuf,
    output_dir: PathBuf,
    dry_run: bool,
    incremental: bool,
}

fn main() -> Result<()> {
    let args = parse_args()?;

    let entries = load_bestiary(&args.bestiary)
        .with_context(|| format!("loading bestiary {}", args.bestiary.display()))?;

    let selected: Vec<&BestiaryEntry> = if let Some(id) = &args.entity {
        let e = entries
            .iter()
            .find(|e| e.id.as_str() == id)
            .ok_or_else(|| anyhow!("no such entity `{id}` in bestiary"))?;
        vec![e]
    } else if args.all {
        entries.iter().collect()
    } else {
        return Err(anyhow!("must pass --all or --entity ID"));
    };

    if !args.dry_run {
        std::fs::create_dir_all(&args.output_dir).with_context(|| {
            format!("creating output dir {}", args.output_dir.display())
        })?;
    }

    let generator = MockGenerator;
    for entry in selected {
        run_entity(entry, &generator, &args)?;
    }
    Ok(())
}

fn run_entity(
    entry: &BestiaryEntry,
    generator: &dyn SpriteGenerator,
    args: &Args,
) -> Result<()> {
    let layout = AtlasLayout::from_entry(entry);
    let png_path = args.output_dir.join(format!("{}.png", entry.id));
    let ron_path = args.output_dir.join(format!("{}.ron", entry.id));

    if args.dry_run {
        print_prompts(entry, &layout, generator);
        return Ok(());
    }

    if args.incremental && can_skip(&args.bestiary, &png_path) {
        eprintln!("  {} — up to date, skipping", entry.id);
        return Ok(());
    }

    eprintln!("  {} — generating {}×{} atlas", entry.id, layout.image_width(), layout.image_height());
    let atlas = assemble_atlas(generator, entry, &layout)?;
    atlas.save(&png_path).with_context(|| format!("writing {}", png_path.display()))?;

    let manifest: AtlasManifest = (&layout).into();
    let ron_text = to_ron(&manifest).context("serializing manifest to RON")?;
    std::fs::write(&ron_path, ron_text)
        .with_context(|| format!("writing {}", ron_path.display()))?;
    Ok(())
}

fn print_prompts(entry: &BestiaryEntry, layout: &AtlasLayout, generator: &dyn SpriteGenerator) {
    println!("{}:", entry.id);
    for slot in &layout.animations {
        for dir in 0..layout.directions {
            for frame in 0..slot.frames {
                let p = generator.prompt_for(
                    FrameRequest {
                        entity_id: entry.id.as_str(),
                        animation: slot.name.as_str(),
                        direction: dir,
                        frame,
                        tile_size: layout.tile_size,
                    },
                    &entry.ai_prompt_base,
                    &entry.ai_style,
                );
                println!("  [{}/dir{}/f{}] {}", slot.name, dir, frame, p);
            }
        }
    }
}

fn parse_args() -> Result<Args> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut all = false;
    let mut entity = None;
    let mut bestiary = default_bestiary_path();
    let mut output_dir = PathBuf::from("crates/client/assets/sprites");
    let mut dry_run = false;
    let mut incremental = false;

    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--all"         => { all = true; }
            "--entity"      => { entity = Some(next(&argv, &mut i, "--entity")?); }
            "--bestiary"    => { bestiary = PathBuf::from(next(&argv, &mut i, "--bestiary")?); }
            "--output-dir"  => { output_dir = PathBuf::from(next(&argv, &mut i, "--output-dir")?); }
            "--dry-run"     => { dry_run = true; }
            "--incremental" => { incremental = true; }
            other => return Err(anyhow!("unknown flag `{other}`")),
        }
        i += 1;
    }

    Ok(Args { all, entity, bestiary, output_dir, dry_run, incremental })
}

fn next(argv: &[String], i: &mut usize, flag: &str) -> Result<String> {
    *i += 1;
    argv.get(*i)
        .cloned()
        .ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn default_bestiary_path() -> PathBuf {
    // Repo root is two levels above this crate.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/bestiary.toml")
}
