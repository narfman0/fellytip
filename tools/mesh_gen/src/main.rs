use anyhow::Result;
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

mod dalle;
mod meshy;
mod pipeline;

use fellytip_shared::bestiary::load_bestiary;
use pipeline::{Backend, Pipeline, Stage};

#[derive(Parser)]
#[command(
    name = "mesh_gen",
    about = "3D asset pipeline: DALL-E sprite → Meshy static mesh → Meshy rigged+animated mesh"
)]
struct Cli {
    /// Generate assets for a specific bestiary entity by id
    #[arg(long)]
    entity: Option<String>,

    /// Generate from a free-form text description (requires --name)
    #[arg(long)]
    text: Option<String>,

    /// Name to use for the output file when using --text
    #[arg(long)]
    name: Option<String>,

    /// Generate assets for all bestiary entities
    #[arg(long)]
    all: bool,

    /// Backend to use
    #[arg(long, default_value = "mock")]
    backend: BackendArg,

    /// How far to run the pipeline
    #[arg(long, default_value = "animated")]
    stage: StageArg,

    /// Output directory for GLB files
    #[arg(long, default_value = "assets/models")]
    output: PathBuf,

    /// Output directory for sprite PNG files
    #[arg(long, default_value = "assets/sprites")]
    sprite_output: PathBuf,
}

#[derive(ValueEnum, Clone)]
enum BackendArg {
    Mock,
    Live,
}

#[derive(ValueEnum, Clone)]
enum StageArg {
    /// Only generate the billboard sprite PNG
    Sprite,
    /// Sprite + static textured GLB
    Mesh,
    /// Full pipeline: sprite + static mesh + rigged+animated GLB
    Animated,
}

impl From<StageArg> for Stage {
    fn from(s: StageArg) -> Self {
        match s {
            StageArg::Sprite => Stage::Sprite,
            StageArg::Mesh => Stage::Mesh,
            StageArg::Animated => Stage::Animated,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mesh_gen=info".parse()?))
        .init();

    let cli = Cli::parse();
    let stage = Stage::from(cli.stage);

    let backend = match cli.backend {
        BackendArg::Mock => Backend::Mock,
        BackendArg::Live => {
            let api_key = std::env::var("SPRITE_GEN_API_KEY")
                .expect("SPRITE_GEN_API_KEY must be set for live backend");
            let meshy_key = std::env::var("MESHY_API_KEY")
                .expect("MESHY_API_KEY must be set for live backend");
            Backend::Live {
                dalle: dalle::DalleClient::new(api_key),
                meshy: meshy::MeshyClient::new(meshy_key),
            }
        }
    };

    let pipeline = Pipeline {
        backend,
        sprite_dir: cli.sprite_output,
        model_dir: cli.output,
    };

    if cli.all {
        let bestiary = load_bestiary_from_workspace()?;
        for entry in &bestiary {
            let desc = format!("{} — {}", entry.display_name, entry.ai_prompt_base);
            let slug = entry.id.to_lowercase().replace(' ', "_");
            pipeline.run(&slug, &desc, stage).await?;
        }
    } else if let (Some(text), Some(name)) = (cli.text, cli.name) {
        pipeline.run(&name, &text, stage).await?;
    } else if let Some(entity_id) = cli.entity {
        let bestiary = load_bestiary_from_workspace()?;
        let entry = bestiary.iter()
            .find(|e| e.id.as_str().eq_ignore_ascii_case(&entity_id))
            .ok_or_else(|| anyhow::anyhow!("Entity '{entity_id}' not found in bestiary"))?;
        let desc = format!("{} — {}", entry.display_name, entry.ai_prompt_base);
        let slug = entry.id.to_lowercase().replace(' ', "_");
        pipeline.run(&slug, &desc, stage).await?;
    } else {
        eprintln!("Specify --entity ID, --all, or --text DESC --name NAME");
        eprintln!("  --stage sprite|mesh|animated  (default: animated = full pipeline)");
        eprintln!("  --backend live                (requires SPRITE_GEN_API_KEY + MESHY_API_KEY)");
        std::process::exit(1);
    }

    Ok(())
}

fn load_bestiary_from_workspace() -> Result<Vec<fellytip_shared::bestiary::BestiaryEntry>> {
    let mut dir = std::env::current_dir()?;
    loop {
        let p = dir.join("assets/bestiary.toml");
        if p.exists() {
            return Ok(load_bestiary(&p)?);
        }
        if !dir.pop() {
            anyhow::bail!("Could not find assets/bestiary.toml — run from workspace root");
        }
    }
}
