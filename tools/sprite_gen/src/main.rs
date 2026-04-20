//! sprite_gen — generate 8-direction sprite atlases from `assets/bestiary.toml`.
//!
//! Usage:
//!   cargo run -p sprite_gen -- [--all | --entity ID] [--output-dir DIR]
//!                              [--bestiary PATH] [--dry-run] [--incremental]
//!                              [--backend mock|openai] [--workers N]
//!                              [--no-quantise]
//!
//! Default backend is the deterministic `MockGenerator`.  `--backend openai`
//! reads `SPRITE_GEN_API_KEY` (and optionally `SPRITE_GEN_ENDPOINT` /
//! `SPRITE_GEN_MODEL`) to hit DALL-E 3.

use anyhow::{anyhow, Context, Result};
use fellytip_shared::bestiary::{load_bestiary, BestiaryEntry};
use sprite_gen::{
    assembler::assemble_atlas,
    generator::{FrameRequest, MockGenerator, SpriteGenerator},
    incremental::can_skip,
    layout::AtlasLayout,
    manifest::{to_ron, AtlasManifest},
    openai::OpenAiDalleGenerator,
    palette::quantise_in_place,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Mock,
    OpenAi,
}

struct Args {
    all: bool,
    entity: Option<String>,
    bestiary: PathBuf,
    output_dir: PathBuf,
    dry_run: bool,
    incremental: bool,
    backend: Backend,
    workers: usize,
    quantise: bool,
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
        std::fs::create_dir_all(&args.output_dir)
            .with_context(|| format!("creating output dir {}", args.output_dir.display()))?;
    }

    // Select a backend.  MockGenerator is always safe; OpenAiDalleGenerator
    // refuses to construct without SPRITE_GEN_API_KEY so a missing secret
    // exits non-zero with a clear message.
    let generator: Box<dyn SpriteGenerator + Sync> = match args.backend {
        Backend::Mock => Box::new(MockGenerator),
        Backend::OpenAi => {
            if args.dry_run {
                // Dry-run doesn't actually call the API, so we don't need a
                // live backend.  The OpenAi prompt shape differs though —
                // use it for prompt formatting if the key happens to exist.
                match OpenAiDalleGenerator::from_env() {
                    Ok(g) => Box::new(g),
                    Err(_) => Box::new(MockGenerator),
                }
            } else {
                Box::new(OpenAiDalleGenerator::from_env()?)
            }
        }
    };

    for entry in selected {
        run_entity(entry, generator.as_ref(), &args)?;
    }
    Ok(())
}

fn run_entity(
    entry: &BestiaryEntry,
    generator: &(dyn SpriteGenerator + Sync),
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

    eprintln!(
        "  {} — generating {}×{} atlas (workers={})",
        entry.id,
        layout.image_width(),
        layout.image_height(),
        args.workers,
    );
    let mut atlas = assemble_atlas(generator, entry, &layout, args.workers)?;
    if args.quantise {
        quantise_in_place(&mut atlas);
    }
    atlas
        .save(&png_path)
        .with_context(|| format!("writing {}", png_path.display()))?;

    let manifest: AtlasManifest = (&layout).into();
    let ron_text = to_ron(&manifest).context("serializing manifest to RON")?;
    std::fs::write(&ron_path, ron_text)
        .with_context(|| format!("writing {}", ron_path.display()))?;
    Ok(())
}

fn print_prompts(entry: &BestiaryEntry, layout: &AtlasLayout, generator: &(dyn SpriteGenerator + Sync)) {
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
    let mut backend = Backend::Mock;
    let mut workers: usize = 1;
    let mut quantise = true;

    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--all"          => all = true,
            "--entity"       => entity = Some(next(&argv, &mut i, "--entity")?),
            "--bestiary"     => bestiary = PathBuf::from(next(&argv, &mut i, "--bestiary")?),
            "--output-dir"   => output_dir = PathBuf::from(next(&argv, &mut i, "--output-dir")?),
            "--dry-run"      => dry_run = true,
            "--incremental"  => incremental = true,
            "--backend"      => backend = parse_backend(&next(&argv, &mut i, "--backend")?)?,
            "--workers"      => workers = next(&argv, &mut i, "--workers")?.parse().context("--workers must be a positive integer")?,
            "--no-quantise"  => quantise = false,
            other => return Err(anyhow!("unknown flag `{other}`")),
        }
        i += 1;
    }

    Ok(Args { all, entity, bestiary, output_dir, dry_run, incremental, backend, workers, quantise })
}

fn parse_backend(s: &str) -> Result<Backend> {
    match s {
        "mock"   => Ok(Backend::Mock),
        "openai" => Ok(Backend::OpenAi),
        other    => Err(anyhow!("unknown backend `{other}` (expected `mock` or `openai`)")),
    }
}

fn next(argv: &[String], i: &mut usize, flag: &str) -> Result<String> {
    *i += 1;
    argv.get(*i)
        .cloned()
        .ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn default_bestiary_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/bestiary.toml")
}
