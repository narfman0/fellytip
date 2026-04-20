//! sprite_gen — AI sprite-sheet generator for Fellytip.
//!
//! Reads `assets/bestiary.toml`, calls an AI image backend, and produces:
//!   crates/client/assets/sprites/{entity_id}/atlas.png
//!   crates/client/assets/sprites/{entity_id}/manifest.json
//!
//! Usage examples:
//!   cargo run -p sprite_gen -- --entity player --output crates/client/assets/sprites/
//!   cargo run -p sprite_gen -- --all --dry-run
//!   cargo run -p sprite_gen -- --all --backend dalle --workers 4
//!   cargo run -p sprite_gen -- --all --incremental --output crates/client/assets/sprites/

mod assembler;
mod generator;
mod manifest;

use anyhow::{Context, Result};
use clap::Parser;
use fellytip_shared::bestiary::{Bestiary, BestiaryEntry};
use generator::{DalleGenerator, MockGenerator, SpriteGenerator};
use manifest::{manifest_from_entry, write_manifest};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "sprite_gen", about = "Generate AI sprite sheets from bestiary.toml")]
struct Cli {
    /// Path to bestiary.toml
    #[arg(long, default_value = "assets/bestiary.toml")]
    bestiary: PathBuf,

    /// Generate only this entity ID (e.g. --entity player)
    #[arg(long)]
    entity: Option<String>,

    /// Generate all entities in the bestiary
    #[arg(long)]
    all: bool,

    /// Output directory (atlas.png and manifest.json go inside {output}/{entity_id}/)
    #[arg(long, default_value = "crates/client/assets/sprites")]
    output: PathBuf,

    /// Maximum concurrent generation tasks
    #[arg(long, default_value = "2")]
    workers: usize,

    /// Print prompts without calling the AI API
    #[arg(long)]
    dry_run: bool,

    /// Skip entities whose atlas.png is newer than bestiary.toml
    #[arg(long)]
    incremental: bool,

    /// AI backend: "mock" (default) or "dalle"
    #[arg(long, default_value = "mock")]
    backend: String,

    /// OpenAI API key (overrides SPRITE_GEN_API_KEY env var)
    #[arg(long)]
    api_key: Option<String>,

    /// Sprite frame size in pixels (square)
    #[arg(long, default_value = "64")]
    frame_size: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    // -- Load bestiary --------------------------------------------------------
    let src = std::fs::read_to_string(&cli.bestiary)
        .with_context(|| format!("Reading {}", cli.bestiary.display()))?;
    let bestiary: Bestiary = toml::from_str(&src).context("Parsing bestiary.toml")?;

    tracing::info!("Loaded {} bestiary entries", bestiary.entity.len());

    // -- Select entries to generate ------------------------------------------
    let entries: Vec<BestiaryEntry> = if cli.all {
        bestiary.entity.clone()
    } else if let Some(ref id) = cli.entity {
        let e = bestiary
            .get(id)
            .with_context(|| format!("Entity '{id}' not found in bestiary"))?
            .clone();
        vec![e]
    } else {
        anyhow::bail!("Specify --entity <id> or --all");
    };

    // -- Incremental: skip up-to-date entries --------------------------------
    let bestiary_mtime = std::fs::metadata(&cli.bestiary)
        .ok()
        .and_then(|m| m.modified().ok());

    let entries: Vec<BestiaryEntry> = entries
        .into_iter()
        .filter(|e| {
            if !cli.incremental {
                return true;
            }
            let atlas = cli.output.join(&e.id).join("atlas.png");
            match (atlas.metadata().and_then(|m| m.modified()).ok(), bestiary_mtime) {
                (Some(atlas_t), Some(bestiary_t)) if atlas_t >= bestiary_t => {
                    tracing::info!("Skipping '{}' (atlas is up-to-date)", e.id);
                    false
                }
                _ => true,
            }
        })
        .collect();

    if entries.is_empty() {
        tracing::info!("All entries are up-to-date. Nothing to do.");
        return Ok(());
    }

    // -- Build generator backend ---------------------------------------------
    let generator: Arc<dyn SpriteGenerator> = match cli.backend.as_str() {
        "mock" => Arc::new(MockGenerator),
        "dalle" => {
            let key = cli
                .api_key
                .clone()
                .or_else(|| std::env::var("SPRITE_GEN_API_KEY").ok())
                .context("No API key: pass --api-key or set SPRITE_GEN_API_KEY")?;
            Arc::new(DalleGenerator::new(key))
        }
        other => anyhow::bail!("Unknown backend '{other}'. Use 'mock' or 'dalle'."),
    };

    // -- Spawn one task per entry, bounded by --workers ----------------------
    let semaphore = Arc::new(tokio::sync::Semaphore::new(cli.workers));

    let mut handles = Vec::new();
    for entry in entries {
        let gen  = Arc::clone(&generator);
        let out  = cli.output.clone();
        let sem  = Arc::clone(&semaphore);
        let size = cli.frame_size;
        let dry  = cli.dry_run;

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            generate_entry(entry, gen, out, size, dry).await
        });
        handles.push(handle);
    }

    let mut had_errors = false;
    for h in handles {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::error!("{e:#}");
                had_errors = true;
            }
            Err(e) => {
                tracing::error!("Task panicked: {e}");
                had_errors = true;
            }
        }
    }

    if had_errors {
        anyhow::bail!("One or more entities failed to generate");
    }
    tracing::info!("Done.");
    Ok(())
}

/// Generate all frames for one bestiary entry and write the atlas + manifest.
async fn generate_entry(
    entry: BestiaryEntry,
    gen: Arc<dyn SpriteGenerator>,
    output_dir: PathBuf,
    frame_size: u32,
    dry_run: bool,
) -> Result<()> {
    tracing::info!(
        "Generating '{}' ({} directions × {} anim frames)",
        entry.id,
        entry.directions,
        entry.total_cols()
    );

    let mut all_frames: Vec<Vec<image::DynamicImage>> = Vec::new();

    for dir in 0..entry.directions {
        let mut dir_frames = Vec::new();
        for anim in &entry.animations {
            for frame_idx in 0..anim.frames {
                // Deterministic seed per (entity_id, direction, anim_name, frame).
                let seed = seahash(entry.id.as_bytes())
                    ^ (dir as u64 * 1_000_000)
                    ^ (seahash(anim.name.as_bytes()) * 1_000)
                    ^ frame_idx as u64;

                let prompt = entry.prompt_for_direction(dir);

                if dry_run {
                    println!(
                        "[dry-run] {}/{} dir={dir} frame={frame_idx}\n  prompt: {prompt}",
                        entry.id, anim.name,
                    );
                    dir_frames.push(placeholder_frame(frame_size));
                    continue;
                }

                tracing::debug!("  dir={dir} {} frame={frame_idx}", anim.name);

                let gen_clone = Arc::clone(&gen);
                let prompt_owned = prompt;
                let img = tokio::task::spawn_blocking(move || {
                    gen_clone.generate_blocking(&prompt_owned, seed, frame_size)
                })
                .await
                .context("Blocking task panicked")??;

                dir_frames.push(img);
            }
        }
        all_frames.push(dir_frames);
    }

    let atlas = assembler::assemble_atlas(&all_frames, frame_size, frame_size)
        .context("Assembling atlas")?;

    let entity_dir = output_dir.join(&entry.id);
    std::fs::create_dir_all(&entity_dir)?;
    let atlas_path = entity_dir.join("atlas.png");
    atlas
        .save(&atlas_path)
        .with_context(|| format!("Saving {}", atlas_path.display()))?;
    tracing::info!("Wrote atlas → {}", atlas_path.display());

    let manifest = manifest_from_entry(&entry, frame_size);
    write_manifest(&manifest, &output_dir)?;

    Ok(())
}

/// Non-cryptographic hash for deterministic seed derivation.
fn seahash(data: &[u8]) -> u64 {
    let mut h: u64 = 0x517c_c1b7_2722_0a95;
    for &b in data {
        h = h.wrapping_mul(0x6c62_272e_07bb_0142).wrapping_add(b as u64);
    }
    h
}

/// Transparent grey placeholder frame used in `--dry-run` mode.
fn placeholder_frame(size: u32) -> image::DynamicImage {
    use image::{Rgba, RgbaImage};
    let mut img = RgbaImage::new(size, size);
    for pixel in img.pixels_mut() {
        *pixel = Rgba([180, 180, 180, 128]);
    }
    image::DynamicImage::ImageRgba8(img)
}
