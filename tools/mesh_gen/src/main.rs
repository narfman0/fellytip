use anyhow::Result;
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

mod meshy;
mod pipeline;

use fellytip_shared::bestiary::load_bestiary;
use pipeline::{Backend, Pipeline};

#[derive(Parser)]
#[command(
    name = "mesh_gen",
    about = "3D asset pipeline: text description → Meshy rigged+animated GLB"
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

    /// Output directory for GLB files
    #[arg(long, default_value = "assets/models")]
    output: PathBuf,
}

#[derive(ValueEnum, Clone)]
enum BackendArg {
    Mock,
    Live,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mesh_gen=info".parse()?))
        .init();

    let cli = Cli::parse();

    let backend = match cli.backend {
        BackendArg::Mock => Backend::Mock,
        BackendArg::Live => {
            let meshy_key = std::env::var("MESHY_API_KEY")
                .expect("MESHY_API_KEY must be set for live backend");
            Backend::Live {
                meshy: meshy::MeshyClient::new(meshy_key),
            }
        }
    };

    let pipeline = Pipeline {
        backend,
        model_dir: cli.output,
    };

    if cli.all {
        let bestiary = load_bestiary_from_workspace()?;
        for entry in &bestiary {
            let desc = format!("{} — {}", entry.display_name, entry.ai_prompt_base);
            let slug = entry.id.to_lowercase().replace(' ', "_");
            pipeline.run(&slug, &desc).await?;
        }
    } else if let (Some(text), Some(name)) = (cli.text, cli.name) {
        pipeline.run(&name, &text).await?;
    } else if let Some(entity_id) = cli.entity {
        let bestiary = load_bestiary_from_workspace()?;
        let entry = bestiary
            .iter()
            .find(|e| e.id.as_str().eq_ignore_ascii_case(&entity_id))
            .ok_or_else(|| anyhow::anyhow!("Entity '{entity_id}' not found in bestiary"))?;
        let desc = format!("{} — {}", entry.display_name, entry.ai_prompt_base);
        let slug = entry.id.to_lowercase().replace(' ', "_");
        pipeline.run(&slug, &desc).await?;
    } else {
        eprintln!("Specify --entity ID, --all, or --text DESC --name NAME");
        eprintln!("  --backend live  (requires MESHY_API_KEY)");
        std::process::exit(1);
    }

    Ok(())
}

fn load_bestiary_from_workspace() -> Result<Vec<fellytip_shared::bestiary::BestiaryEntry>> {
    let mut dir = std::env::current_dir()?;
    loop {
        let p = dir.join("assets/bestiary.toml");
        if p.exists() {
            return Ok(load_bestiary(&p)?.entries);
        }
        if !dir.pop() {
            anyhow::bail!("Could not find assets/bestiary.toml — run from workspace root");
        }
    }
}
