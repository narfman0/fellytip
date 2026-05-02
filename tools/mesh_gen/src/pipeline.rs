use anyhow::Result;
use std::path::PathBuf;

use crate::meshy::{MeshyClient, MockMeshyClient};

pub enum Backend {
    Mock,
    Live { meshy: MeshyClient },
}

pub struct Pipeline {
    pub backend: Backend,
    pub model_dir: PathBuf,
}

impl Pipeline {
    pub async fn run(&self, entity_name: &str, description: &str) -> Result<PathBuf> {
        let anim_path = self.model_dir.join(format!("{entity_name}_animated.glb"));

        if anim_path.exists() {
            tracing::info!("Animated mesh exists, skipping: {}", anim_path.display());
            return Ok(anim_path);
        }

        tracing::info!("Generating rigged+animated mesh for: {entity_name}");
        let glb_bytes = match &self.backend {
            Backend::Mock => {
                tracing::info!("[mock] animated mesh for {entity_name}");
                MockMeshyClient::mock_glb()
            }
            Backend::Live { meshy } => {
                let task_id = meshy.submit_text_to_animated_3d(description).await?;
                tracing::info!("Meshy text-to-3d task: {task_id}");
                meshy.wait_and_download_text_3d(&task_id).await?
            }
        };
        std::fs::create_dir_all(&self.model_dir)?;
        std::fs::write(&anim_path, &glb_bytes)?;
        tracing::info!("Saved animated mesh: {}", anim_path.display());
        Ok(anim_path)
    }
}
