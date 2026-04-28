use anyhow::Result;
use std::path::PathBuf;
use crate::{dalle::DalleClient, meshy::{MeshyClient, MockMeshyClient}};

pub enum Backend {
    Mock,
    Live { dalle: DalleClient, meshy: MeshyClient },
}

pub struct Pipeline {
    pub backend: Backend,
    pub output_dir: PathBuf,
}

impl Pipeline {
    pub async fn run(&self, entity_name: &str, description: &str) -> Result<PathBuf> {
        let out_path = self.output_dir.join(format!("{entity_name}.glb"));

        if out_path.exists() {
            tracing::info!("Skipping {entity_name} — already exists at {}", out_path.display());
            return Ok(out_path);
        }

        let glb_bytes = match &self.backend {
            Backend::Mock => {
                tracing::info!("[mock] Generating mesh for: {entity_name}");
                MockMeshyClient::mock_glb()
            }
            Backend::Live { dalle, meshy } => {
                tracing::info!("Generating DALL-E reference for: {entity_name}");
                let image_data_url = dalle.generate_reference_image(description).await?;
                tracing::info!("Submitting to Meshy...");
                let task_id = meshy.submit_image_to_3d(&image_data_url).await?;
                tracing::info!("Task ID: {task_id} — polling...");
                meshy.wait_and_download(&task_id).await?
            }
        };

        std::fs::create_dir_all(&self.output_dir)?;
        std::fs::write(&out_path, &glb_bytes)?;
        tracing::info!("Saved {}", out_path.display());
        Ok(out_path)
    }
}
