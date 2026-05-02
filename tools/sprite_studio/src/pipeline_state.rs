//! Sidecar state for the unified asset pipeline.  Persisted as
//! `assets/models/{id}/pipeline_state.json` so in-flight Meshy task IDs
//! survive studio restarts.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PipelineState {
    pub preview_task_id: Option<String>,
    pub refine_task_id: Option<String>,
    pub rig_task_id: Option<String>,
    /// clip_name -> task_id
    pub animation_task_ids: HashMap<String, String>,
}

impl PipelineState {
    pub fn load(models_dir: &Path, entity_id: &str) -> Self {
        let path = models_dir.join(entity_id).join("pipeline_state.json");
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, models_dir: &Path, entity_id: &str) {
        let dir = models_dir.join(entity_id);
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(dir.join("pipeline_state.json"), s);
        }
    }
}
