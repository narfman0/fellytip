use anyhow::Result;
use base64::Engine as _;
use std::path::PathBuf;

use crate::{
    dalle::DalleClient,
    meshy::{MeshyClient, MockMeshyClient},
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// DALL-E 3 → PNG billboard sprite
    Sprite = 0,
    /// Sprite PNG → Meshy image-to-3d → static textured GLB
    Mesh = 1,
    /// Entity description → Meshy text-to-3d (rigged + animated) → animated GLB
    Animated = 2,
}

pub enum Backend {
    Mock,
    Live { dalle: DalleClient, meshy: MeshyClient },
}

pub struct Pipeline {
    pub backend: Backend,
    pub sprite_dir: PathBuf,
    pub model_dir: PathBuf,
}

pub struct PipelineOutput {
    pub sprite: Option<PathBuf>,
    pub mesh: Option<PathBuf>,
    pub animated: Option<PathBuf>,
}

impl Pipeline {
    pub async fn run(
        &self,
        entity_name: &str,
        description: &str,
        up_to: Stage,
    ) -> Result<PipelineOutput> {
        let mut out = PipelineOutput { sprite: None, mesh: None, animated: None };

        // --- Stage 1: sprite billboard ---
        let sprite_path = self.sprite_dir.join(format!("{entity_name}.png"));
        let sprite_data_url: Option<String>;

        if up_to >= Stage::Sprite {
            if sprite_path.exists() {
                tracing::info!("Sprite exists, reusing: {}", sprite_path.display());
                let bytes = std::fs::read(&sprite_path)?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                sprite_data_url = Some(format!("data:image/png;base64,{b64}"));
            } else {
                tracing::info!("[stage 1] Generating billboard sprite for: {entity_name}");
                let png_bytes = match &self.backend {
                    Backend::Mock => {
                        tracing::info!("[mock] sprite for {entity_name}");
                        MockMeshyClient::mock_png()
                    }
                    Backend::Live { dalle, .. } => {
                        dalle.generate_billboard_sprite(description).await?
                    }
                };
                std::fs::create_dir_all(&self.sprite_dir)?;
                std::fs::write(&sprite_path, &png_bytes)?;
                tracing::info!("Saved sprite: {}", sprite_path.display());
                let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
                sprite_data_url = Some(format!("data:image/png;base64,{b64}"));
            }
            out.sprite = Some(sprite_path.clone());
        } else {
            sprite_data_url = None;
        }

        // --- Stage 2: static textured mesh ---
        let mesh_path = self.model_dir.join(format!("{entity_name}.glb"));

        if up_to >= Stage::Mesh {
            if mesh_path.exists() {
                tracing::info!("Static mesh exists, skipping: {}", mesh_path.display());
            } else {
                tracing::info!("[stage 2] Generating static mesh for: {entity_name}");
                let glb_bytes = match &self.backend {
                    Backend::Mock => {
                        tracing::info!("[mock] static mesh for {entity_name}");
                        MockMeshyClient::mock_glb()
                    }
                    Backend::Live { meshy, .. } => {
                        let data_url = sprite_data_url
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("sprite data URL missing for mesh stage"))?;
                        let task_id = meshy.submit_image_to_3d(data_url).await?;
                        tracing::info!("Meshy image-to-3d task: {task_id}");
                        meshy.wait_and_download_image_3d(&task_id).await?
                    }
                };
                std::fs::create_dir_all(&self.model_dir)?;
                std::fs::write(&mesh_path, &glb_bytes)?;
                tracing::info!("Saved static mesh: {}", mesh_path.display());
            }
            out.mesh = Some(mesh_path);
        }

        // --- Stage 3: rigged + animated mesh ---
        let anim_path = self.model_dir.join(format!("{entity_name}_animated.glb"));

        if up_to >= Stage::Animated {
            if anim_path.exists() {
                tracing::info!("Animated mesh exists, skipping: {}", anim_path.display());
            } else {
                tracing::info!("[stage 3] Generating rigged+animated mesh for: {entity_name}");
                let glb_bytes = match &self.backend {
                    Backend::Mock => {
                        tracing::info!("[mock] animated mesh for {entity_name}");
                        MockMeshyClient::mock_glb()
                    }
                    Backend::Live { meshy, .. } => {
                        let task_id = meshy.submit_text_to_animated_3d(description).await?;
                        tracing::info!("Meshy text-to-3d task: {task_id}");
                        meshy.wait_and_download_text_3d(&task_id).await?
                    }
                };
                std::fs::write(&anim_path, &glb_bytes)?;
                tracing::info!("Saved animated mesh: {}", anim_path.display());
            }
            out.animated = Some(anim_path);
        }

        Ok(out)
    }
}
