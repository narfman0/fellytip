//! Main application state and UI for the Character Studio.

use crate::{
    generator::{FrameRequest, MockGenerator, SpriteGenerator},
    layout::TILE_SIZE,
    openai::OpenAiDalleGenerator,
    postprocess::remove_background,
    stability::StabilityGenerator,
};
use fellytip_shared::bestiary::{load_bestiary, BestiaryEntry, StylePreset};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

// ---------------------------------------------------------------------------
// Sprite generation types (unchanged)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    Mock,
    OpenAI,
    StabilityAi,
}

struct EntityGenState {
    generating: bool,
    gen_results: Vec<Option<egui::TextureHandle>>,
    gen_images: Vec<Option<image::RgbaImage>>,
    selected_variant: Option<usize>,
    gen_receiver: Option<Receiver<(usize, image::RgbaImage)>>,
}

impl EntityGenState {
    fn new() -> Self {
        Self {
            generating: false,
            gen_results: vec![None, None, None, None],
            gen_images: vec![None, None, None, None],
            selected_variant: None,
            gen_receiver: None,
        }
    }
}

// ---------------------------------------------------------------------------
// 3D pipeline types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PipelineStage {
    Draft,
    Approved,
    MeshDone,
    Rigged,
    Animated,
    Merged,
    LodsComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeshBackend {
    Mock,
    Live,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
enum MeshSubStage {
    Idle,
    TextTo3dPreview,
    TextTo3dRefine,
    Rigging,
    Animating { completed: Vec<String>, total: usize },
    Merging,
    GeneratingLods,
}

enum MeshGenEvent {
    Progress(u8, String),
    Done,
    Failed(String),
}

struct MeshGenState {
    sub_stage: MeshSubStage,
    status: String,
    progress: u8,
    receiver: Option<std::sync::mpsc::Receiver<MeshGenEvent>>,
    /// Which animation clips are selected for rig+animate (indexed by CLIP_NAMES).
    selected_clips: Vec<bool>,
}

impl MeshGenState {
    fn new() -> Self {
        Self {
            sub_stage: MeshSubStage::Idle,
            status: String::new(),
            progress: 0,
            receiver: None,
            selected_clips: vec![true; CLIP_NAMES.len()],
        }
    }
}

/// Default animation clip names and their Meshy action IDs.
const CLIP_NAMES: &[&str] = &["idle", "walk", "attack", "death", "run", "behit"];
const CLIP_ACTION_IDS: &[u32] = &[0, 1, 4, 8, 14, 7];

// ---------------------------------------------------------------------------
// StudioApp
// ---------------------------------------------------------------------------

pub struct StudioApp {
    bestiary: Vec<BestiaryEntry>,
    styles: Vec<StylePreset>,
    selected_entity: usize,
    selected_style: usize,
    backend: BackendChoice,
    openai_available: bool,
    stability_available: bool,

    // Per-entity sprite generation state — keyed by bestiary index
    entity_gen: HashMap<usize, EntityGenState>,

    // Per-entity 3D pipeline state
    entity_stage: HashMap<usize, PipelineStage>,
    entity_mesh: HashMap<usize, MeshGenState>,
    meshy_available: bool,
    models_dir: PathBuf,
    mesh_backend: MeshBackend,

    // Approved base.png for the current entity (loaded from disk, persists across navigation)
    approved_texture: Option<egui::TextureHandle>,
    approved_loaded: bool,

    // Post-processing toggles
    remove_bg: bool,

    // Output dir (sprites)
    output_dir: PathBuf,

    // Path to bestiary.toml on disk
    bestiary_path: Option<PathBuf>,

    // Status message for the user
    status: String,

    // Styles editor state
    new_style_name: String,
    new_style_value: String,
}

impl StudioApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let bestiary_path = find_bestiary_path();
        let loaded = bestiary_path
            .as_ref()
            .and_then(|p| load_bestiary(p).ok());
        let (bestiary, styles) = loaded
            .map(|b| (b.entries, b.styles))
            .unwrap_or_default();

        let output_dir = find_assets_dir()
            .map(|d| d.join("sprites"))
            .unwrap_or_else(|| PathBuf::from("assets/sprites"));

        let models_dir = find_assets_dir()
            .map(|d| d.join("models"))
            .unwrap_or_else(|| PathBuf::from("assets/models"));

        let openai_available = std::env::var("SPRITE_GEN_API_KEY").is_ok();
        let stability_available = std::env::var("STABILITY_API_KEY").is_ok();
        let meshy_available = std::env::var("MESHY_API_KEY").is_ok();

        let default_style = bestiary
            .first()
            .and_then(|e| styles.iter().position(|s| s.name == e.ai_style))
            .unwrap_or(0);

        let backend = if stability_available {
            BackendChoice::StabilityAi
        } else {
            BackendChoice::Mock
        };

        let mesh_backend = if meshy_available {
            MeshBackend::Live
        } else {
            MeshBackend::Mock
        };

        let mut app = Self {
            bestiary,
            styles,
            selected_entity: 0,
            selected_style: default_style,
            backend,
            openai_available,
            stability_available,
            entity_gen: HashMap::new(),
            entity_stage: HashMap::new(),
            entity_mesh: HashMap::new(),
            meshy_available,
            models_dir,
            mesh_backend,
            remove_bg: true,
            approved_texture: None,
            approved_loaded: false,
            output_dir,
            bestiary_path,
            status: String::new(),
            new_style_name: String::new(),
            new_style_value: String::new(),
        };
        app.scan_all_stages();
        app
    }

    fn scan_all_stages(&mut self) {
        for (i, entry) in self.bestiary.iter().enumerate() {
            let stage = scan_stage(&self.models_dir, &self.output_dir, &entry.id);
            self.entity_stage.insert(i, stage);
        }
    }

    fn current_entity(&self) -> Option<&BestiaryEntry> {
        self.bestiary.get(self.selected_entity)
    }

    fn selected_style_value(&self) -> &str {
        self.styles
            .get(self.selected_style)
            .map(|s| s.value.as_str())
            .unwrap_or("")
    }

    fn load_approved_base(&mut self, ctx: &egui::Context) {
        self.approved_loaded = true;
        let Some(entity_id) = self.current_entity().map(|e| e.id.clone()) else {
            self.approved_texture = None;
            return;
        };
        let path = self.output_dir.join(entity_id.as_str()).join("base.png");
        self.approved_texture = image::open(&path)
            .ok()
            .map(|img| rgba_to_egui(ctx, &img.to_rgba8(), &format!("approved_{entity_id}")));
    }

    fn make_generator(&self) -> Box<dyn SpriteGenerator + Send + Sync> {
        match self.backend {
            BackendChoice::Mock => Box::new(MockGenerator),
            BackendChoice::OpenAI => OpenAiDalleGenerator::from_env()
                .map(|g| -> Box<dyn SpriteGenerator + Send + Sync> { Box::new(g) })
                .unwrap_or_else(|_| Box::new(MockGenerator)),
            BackendChoice::StabilityAi => StabilityGenerator::from_env()
                .map(|g| -> Box<dyn SpriteGenerator + Send + Sync> { Box::new(g) })
                .unwrap_or_else(|_| Box::new(MockGenerator)),
        }
    }

    // -----------------------------------------------------------------------
    // Poll receivers
    // -----------------------------------------------------------------------

    fn poll_all_gen_receivers(&mut self, ctx: &egui::Context) {
        let entity_indices: Vec<usize> = self.entity_gen.keys().cloned().collect();
        let mut any_update = false;

        for entity_idx in entity_indices {
            let state = self.entity_gen.get_mut(&entity_idx).unwrap();
            if state.gen_receiver.is_none() {
                continue;
            }

            let mut received: Vec<(usize, image::RgbaImage)> = Vec::new();
            let mut disconnected = false;
            if let Some(rx) = &state.gen_receiver {
                loop {
                    match rx.try_recv() {
                        Ok(item) => received.push(item),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
            }

            for (idx, img) in received {
                let label = format!("variant_{}_{}", entity_idx, idx);
                state.gen_results[idx] = Some(rgba_to_egui(ctx, &img, &label));
                state.gen_images[idx] = Some(img);
                any_update = true;
            }

            if disconnected || state.gen_results.iter().all(|r| r.is_some()) {
                state.generating = false;
                state.gen_receiver = None;
            }
        }

        if any_update {
            ctx.request_repaint();
        }
    }

    fn poll_all_mesh_receivers(&mut self, ctx: &egui::Context) {
        let entity_indices: Vec<usize> = self.entity_mesh.keys().cloned().collect();
        let mut any_update = false;

        for entity_idx in entity_indices {
            let state = self.entity_mesh.get_mut(&entity_idx).unwrap();
            if state.receiver.is_none() {
                continue;
            }

            let mut events: Vec<MeshGenEvent> = Vec::new();
            let mut disconnected = false;
            if let Some(rx) = &state.receiver {
                loop {
                    match rx.try_recv() {
                        Ok(ev) => events.push(ev),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
            }

            for ev in events {
                match ev {
                    MeshGenEvent::Progress(pct, msg) => {
                        state.progress = pct;
                        state.status = msg;
                        any_update = true;
                    }
                    MeshGenEvent::Done => {
                        state.progress = 100;
                        state.status = "Done!".into();
                        state.receiver = None;
                        state.sub_stage = MeshSubStage::Idle;
                        // Rescan disk stage
                        if let Some(entry) = self.bestiary.get(entity_idx) {
                            let new_stage =
                                scan_stage(&self.models_dir, &self.output_dir, &entry.id);
                            self.entity_stage.insert(entity_idx, new_stage);
                        }
                        any_update = true;
                    }
                    MeshGenEvent::Failed(msg) => {
                        state.status = format!("Failed: {msg}");
                        state.receiver = None;
                        state.sub_stage = MeshSubStage::Idle;
                        any_update = true;
                    }
                }
            }

            if disconnected {
                let state = self.entity_mesh.get_mut(&entity_idx).unwrap();
                if state.receiver.is_some() {
                    state.receiver = None;
                    state.sub_stage = MeshSubStage::Idle;
                }
                if let Some(entry) = self.bestiary.get(entity_idx) {
                    let new_stage =
                        scan_stage(&self.models_dir, &self.output_dir, &entry.id);
                    self.entity_stage.insert(entity_idx, new_stage);
                }
                any_update = true;
            }
        }

        if any_update {
            ctx.request_repaint();
        }
    }

    // -----------------------------------------------------------------------
    // Sprite generation
    // -----------------------------------------------------------------------

    fn spawn_generate_variants(&mut self) {
        if self.bestiary.is_empty() {
            return;
        }
        let entity_idx = self.selected_entity;
        let entry = self.bestiary[entity_idx].clone();
        let style = self.selected_style_value().to_string();

        let (tx, rx) = mpsc::channel::<(usize, image::RgbaImage)>();
        let remove_bg = self.remove_bg;

        for variant in 0..4usize {
            let tx = tx.clone();
            let entry = entry.clone();
            let style = style.clone();
            let generator = self.make_generator();
            std::thread::spawn(move || {
                let req = FrameRequest {
                    entity_id: entry.id.as_str(),
                    animation: entry
                        .animations
                        .first()
                        .map(|a| a.name.as_str())
                        .unwrap_or("idle"),
                    direction: 0,
                    frame: variant as u32,
                    tile_size: TILE_SIZE,
                    base_prompt: &entry.ai_prompt_base,
                    style: &style,
                };
                if let Ok(mut img) = generator.generate(req) {
                    if remove_bg {
                        remove_background(&mut img, 40);
                    }
                    let _ = tx.send((variant, img));
                }
            });
        }

        let state = self.entity_gen.entry(entity_idx).or_insert_with(EntityGenState::new);
        state.gen_results = vec![None, None, None, None];
        state.gen_images = vec![None, None, None, None];
        state.selected_variant = None;
        state.generating = true;
        state.gen_receiver = Some(rx);
    }

    fn approve_base_png(&mut self, ctx: &egui::Context) {
        let entity_idx = self.selected_entity;
        let Some(variant_idx) = self
            .entity_gen
            .get(&entity_idx)
            .and_then(|s| s.selected_variant)
        else {
            self.status = "No variant selected.".into();
            return;
        };
        let Some(img) = self
            .entity_gen
            .get(&entity_idx)
            .and_then(|s| s.gen_images[variant_idx].clone())
        else {
            self.status = "Variant image not available.".into();
            return;
        };
        let Some(entry) = self.current_entity() else {
            return;
        };
        let entity_id = entry.id.clone();
        let out_dir = self.output_dir.join(entity_id.as_str());

        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            self.status = format!("Failed to create dir: {e}");
            return;
        }

        let path = out_dir.join("base.png");
        match img.save(&path) {
            Ok(_) => {
                self.status = format!("Saved {}", path.display());
                self.approved_loaded = false;
                self.load_approved_base(ctx);
                // Update stage
                let new_stage = scan_stage(&self.models_dir, &self.output_dir, &entity_id);
                self.entity_stage.insert(entity_idx, new_stage);
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn save_bestiary(&mut self) {
        let Some(path) = &self.bestiary_path else {
            self.status = "No bestiary path known; cannot save.".into();
            return;
        };
        let path = path.clone();

        #[derive(serde::Serialize)]
        struct BestiaryFile<'a> {
            styles: &'a Vec<StylePreset>,
            entity: &'a Vec<BestiaryEntry>,
        }

        let file = BestiaryFile {
            styles: &self.styles,
            entity: &self.bestiary,
        };

        match toml::to_string_pretty(&file) {
            Ok(text) => match std::fs::write(&path, text) {
                Ok(_) => self.status = "Saved!".into(),
                Err(e) => self.status = format!("Write error: {e}"),
            },
            Err(e) => self.status = format!("Serialize error: {e}"),
        }
    }

    // -----------------------------------------------------------------------
    // 3D Mesh generation
    // -----------------------------------------------------------------------

    fn spawn_mesh_gen(&mut self, entity_idx: usize) {
        let entry = self.bestiary[entity_idx].clone();
        let models_dir = self.models_dir.clone();
        let is_mock = self.mesh_backend == MeshBackend::Mock;
        let api_key = match self.mesh_backend {
            MeshBackend::Mock => "mock".to_string(),
            MeshBackend::Live => std::env::var("MESHY_API_KEY").unwrap_or_default(),
        };

        let (tx, rx) = std::sync::mpsc::channel::<MeshGenEvent>();

        std::thread::spawn(move || {
            let prompt = if entry.mesh_prompt.is_empty() {
                entry.ai_prompt_base.clone()
            } else {
                entry.mesh_prompt.clone()
            };
            let entity_id = entry.id.as_str();
            let out_dir = models_dir.join(entity_id);
            let _ = std::fs::create_dir_all(&out_dir);
            let out_path = out_dir.join(format!("{entity_id}_mesh.glb"));

            if is_mock {
                let _ = tx.send(MeshGenEvent::Progress(50, "Mock: generating mesh...".into()));
                std::thread::sleep(std::time::Duration::from_millis(500));
                let _ = std::fs::write(&out_path, crate::meshy::mock_glb());
                let _ = tx.send(MeshGenEvent::Done);
                return;
            }

            let client = crate::meshy::MeshyClient::new(api_key);
            let mut state = crate::pipeline_state::PipelineState::load(&models_dir, entity_id);

            // Preview
            let preview_id = if let Some(ref pid) = state.preview_task_id.clone() {
                pid.clone()
            } else {
                match client.submit_preview(&prompt) {
                    Ok(task_id) => {
                        state.preview_task_id = Some(task_id.clone());
                        state.save(&models_dir, entity_id);
                        task_id
                    }
                    Err(e) => {
                        let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                        return;
                    }
                }
            };

            // Poll preview
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                match client.poll("v2/text-to-3d", &preview_id) {
                    Ok(r) if r.status == "SUCCEEDED" => break,
                    Ok(r) if r.status == "FAILED" || r.status == "EXPIRED" => {
                        let _ = tx.send(MeshGenEvent::Failed(format!(
                            "Preview task {} {}",
                            preview_id, r.status
                        )));
                        return;
                    }
                    Ok(r) => {
                        let _ = tx.send(MeshGenEvent::Progress(
                            r.progress / 2,
                            format!("Preview: {}%", r.progress),
                        ));
                    }
                    Err(e) => {
                        let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                        return;
                    }
                }
            }

            // Refine
            let refine_id = if let Some(ref rid) = state.refine_task_id.clone() {
                rid.clone()
            } else {
                match client.submit_refine(&preview_id) {
                    Ok(task_id) => {
                        state.refine_task_id = Some(task_id.clone());
                        state.save(&models_dir, entity_id);
                        task_id
                    }
                    Err(e) => {
                        let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                        return;
                    }
                }
            };

            // Poll refine
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                match client.poll("v2/text-to-3d", &refine_id) {
                    Ok(r) if r.status == "SUCCEEDED" => {
                        // Download the GLB
                        let url = match r.model_url {
                            Some(u) => u,
                            None => {
                                let _ = tx.send(MeshGenEvent::Failed(
                                    "No GLB URL in refine response".into(),
                                ));
                                return;
                            }
                        };
                        match client.download(&url) {
                            Ok(bytes) => {
                                if let Err(e) = std::fs::write(&out_path, &bytes) {
                                    let _ = tx.send(MeshGenEvent::Failed(format!(
                                        "Write mesh failed: {e}"
                                    )));
                                    return;
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                                return;
                            }
                        }
                        break;
                    }
                    Ok(r) if r.status == "FAILED" || r.status == "EXPIRED" => {
                        let _ = tx.send(MeshGenEvent::Failed(format!(
                            "Refine task {} {}",
                            refine_id, r.status
                        )));
                        return;
                    }
                    Ok(r) => {
                        let _ = tx.send(MeshGenEvent::Progress(
                            50 + r.progress / 2,
                            format!("Refine: {}%", r.progress),
                        ));
                    }
                    Err(e) => {
                        let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                        return;
                    }
                }
            }

            let _ = tx.send(MeshGenEvent::Done);
        });

        let state = self.entity_mesh.entry(entity_idx).or_insert_with(MeshGenState::new);
        state.sub_stage = MeshSubStage::TextTo3dPreview;
        state.status = "Starting...".into();
        state.progress = 0;
        state.receiver = Some(rx);
    }

    fn spawn_rig_animate(&mut self, entity_idx: usize) {
        let entry = self.bestiary[entity_idx].clone();
        let models_dir = self.models_dir.clone();
        let is_mock = self.mesh_backend == MeshBackend::Mock;
        let api_key = match self.mesh_backend {
            MeshBackend::Mock => "mock".to_string(),
            MeshBackend::Live => std::env::var("MESHY_API_KEY").unwrap_or_default(),
        };

        // Determine which clips are selected
        let selected_clips: Vec<bool> = self
            .entity_mesh
            .get(&entity_idx)
            .map(|s| s.selected_clips.clone())
            .unwrap_or_else(|| vec![true; CLIP_NAMES.len()]);

        let clip_pairs: Vec<(String, u32)> = CLIP_NAMES
            .iter()
            .zip(CLIP_ACTION_IDS.iter())
            .enumerate()
            .filter_map(|(i, (name, &action_id))| {
                if selected_clips.get(i).copied().unwrap_or(true) {
                    Some((name.to_string(), action_id))
                } else {
                    None
                }
            })
            .collect();

        let (tx, rx) = std::sync::mpsc::channel::<MeshGenEvent>();

        std::thread::spawn(move || {
            let entity_id = entry.id.as_str();
            let out_dir = models_dir.join(entity_id);
            let _ = std::fs::create_dir_all(&out_dir);

            if is_mock {
                let _ = tx.send(MeshGenEvent::Progress(10, "Mock: rigging...".into()));
                std::thread::sleep(std::time::Duration::from_millis(300));
                let rigged_path = out_dir.join(format!("{entity_id}_rigged.glb"));
                let _ = std::fs::write(&rigged_path, crate::meshy::mock_glb());

                for (i, (clip_name, _)) in clip_pairs.iter().enumerate() {
                    let pct = 10 + ((i + 1) * 70 / clip_pairs.len().max(1)) as u8;
                    let _ = tx.send(MeshGenEvent::Progress(
                        pct,
                        format!("Mock: animating {clip_name}..."),
                    ));
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let clip_path = out_dir.join(format!("{clip_name}.glb"));
                    let _ = std::fs::write(&clip_path, crate::meshy::mock_glb());
                }

                // Merge (mock — just copy rigged as merged)
                let _ = tx.send(MeshGenEvent::Progress(90, "Mock: merging...".into()));
                std::thread::sleep(std::time::Duration::from_millis(200));
                let merged_path = out_dir.join(format!("{entity_id}.glb"));
                let _ = std::fs::write(&merged_path, crate::meshy::mock_glb());
                let _ = tx.send(MeshGenEvent::Done);
                return;
            }

            let client = crate::meshy::MeshyClient::new(api_key);
            let mut state = crate::pipeline_state::PipelineState::load(&models_dir, entity_id);

            // Rig
            let rigged_path = out_dir.join(format!("{entity_id}_rigged.glb"));
            let rig_id = if rigged_path.exists() {
                // Already rigged — use existing rig_task_id or skip
                state.rig_task_id.clone().unwrap_or_default()
            } else {
                let refine_id = match &state.refine_task_id {
                    Some(id) => id.clone(),
                    None => {
                        let _ = tx.send(MeshGenEvent::Failed(
                            "No refine task ID — generate mesh first".into(),
                        ));
                        return;
                    }
                };

                let new_rig_id = if let Some(ref rid) = state.rig_task_id.clone() {
                    rid.clone()
                } else {
                    match client.submit_rig(&refine_id) {
                        Ok(id) => {
                            state.rig_task_id = Some(id.clone());
                            state.save(&models_dir, entity_id);
                            id
                        }
                        Err(e) => {
                            let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                            return;
                        }
                    }
                };

                // Poll rig
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    match client.poll("v1/rigging", &new_rig_id) {
                        Ok(r) if r.status == "SUCCEEDED" => {
                            let url = match r.model_url {
                                Some(u) => u,
                                None => {
                                    let _ = tx.send(MeshGenEvent::Failed(
                                        "No GLB URL in rig response".into(),
                                    ));
                                    return;
                                }
                            };
                            match client.download(&url) {
                                Ok(bytes) => {
                                    if let Err(e) = std::fs::write(&rigged_path, &bytes) {
                                        let _ = tx.send(MeshGenEvent::Failed(format!(
                                            "Write rigged failed: {e}"
                                        )));
                                        return;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                                    return;
                                }
                            }
                            break;
                        }
                        Ok(r) if r.status == "FAILED" || r.status == "EXPIRED" => {
                            let _ = tx.send(MeshGenEvent::Failed(format!(
                                "Rig task {} {}",
                                new_rig_id, r.status
                            )));
                            return;
                        }
                        Ok(r) => {
                            let _ = tx.send(MeshGenEvent::Progress(
                                r.progress / 3,
                                format!("Rigging: {}%", r.progress),
                            ));
                        }
                        Err(e) => {
                            let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                            return;
                        }
                    }
                }

                new_rig_id
            };

            // Submit animation jobs
            let total = clip_pairs.len();
            let mut anim_task_ids: Vec<(String, String)> = Vec::new();

            for (clip_name, action_id) in &clip_pairs {
                let task_id = if let Some(existing) =
                    state.animation_task_ids.get(clip_name.as_str()).cloned()
                {
                    existing
                } else {
                    match client.submit_animation(&rig_id, *action_id) {
                        Ok(id) => {
                            state
                                .animation_task_ids
                                .insert(clip_name.clone(), id.clone());
                            state.save(&models_dir, entity_id);
                            id
                        }
                        Err(e) => {
                            let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                            return;
                        }
                    }
                };
                anim_task_ids.push((clip_name.clone(), task_id));
            }

            // Poll each animation
            let mut completed: Vec<String> = Vec::new();
            for (clip_name, task_id) in &anim_task_ids {
                let clip_path = out_dir.join(format!("{clip_name}.glb"));
                if clip_path.exists() {
                    completed.push(clip_name.clone());
                    continue;
                }
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    match client.poll("v1/animations", task_id) {
                        Ok(r) if r.status == "SUCCEEDED" => {
                            let url = match r.model_url {
                                Some(u) => u,
                                None => {
                                    let _ = tx.send(MeshGenEvent::Failed(format!(
                                        "No GLB URL for clip {clip_name}"
                                    )));
                                    return;
                                }
                            };
                            match client.download(&url) {
                                Ok(bytes) => {
                                    if let Err(e) = std::fs::write(&clip_path, &bytes) {
                                        let _ = tx.send(MeshGenEvent::Failed(format!(
                                            "Write clip {clip_name} failed: {e}"
                                        )));
                                        return;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                                    return;
                                }
                            }
                            completed.push(clip_name.clone());
                            let pct =
                                (33 + completed.len() * 50 / total.max(1)).min(83) as u8;
                            let _ = tx.send(MeshGenEvent::Progress(
                                pct,
                                format!("Animated {}/{total}", completed.len()),
                            ));
                            break;
                        }
                        Ok(r) if r.status == "FAILED" || r.status == "EXPIRED" => {
                            let _ = tx.send(MeshGenEvent::Failed(format!(
                                "Anim task {} {}",
                                task_id, r.status
                            )));
                            return;
                        }
                        Ok(r) => {
                            let _ = tx.send(MeshGenEvent::Progress(
                                r.progress / 3,
                                format!("Animating {clip_name}: {}%", r.progress),
                            ));
                        }
                        Err(e) => {
                            let _ = tx.send(MeshGenEvent::Failed(e.to_string()));
                            return;
                        }
                    }
                }
            }

            // Merge with gltf-transform
            let _ = tx.send(MeshGenEvent::Progress(90, "Merging clips...".into()));
            let merged_path = out_dir.join(format!("{entity_id}.glb"));
            let mut cmd = std::process::Command::new("npx");
            cmd.arg("@gltf-transform/cli").arg("merge");
            cmd.arg(&rigged_path);
            for (clip_name, _) in &clip_pairs {
                cmd.arg(out_dir.join(format!("{clip_name}.glb")));
            }
            cmd.arg(&merged_path);
            match cmd.status() {
                Ok(s) if s.success() => {}
                Ok(s) => {
                    let _ = tx.send(MeshGenEvent::Failed(format!("gltf-transform merge exited {s}")));
                    return;
                }
                Err(e) => {
                    let _ = tx.send(MeshGenEvent::Failed(format!("gltf-transform merge error: {e}")));
                    return;
                }
            }

            let _ = tx.send(MeshGenEvent::Done);
        });

        let state = self.entity_mesh.entry(entity_idx).or_insert_with(MeshGenState::new);
        state.sub_stage = MeshSubStage::Rigging;
        state.status = "Starting rig...".into();
        state.progress = 0;
        state.receiver = Some(rx);
    }

    fn spawn_lod_gen(&mut self, entity_idx: usize) {
        let entry = self.bestiary[entity_idx].clone();
        let models_dir = self.models_dir.clone();
        let is_mock = self.mesh_backend == MeshBackend::Mock;

        let (tx, rx) = std::sync::mpsc::channel::<MeshGenEvent>();

        std::thread::spawn(move || {
            let entity_id = entry.id.as_str();
            let out_dir = models_dir.join(entity_id);
            let input = out_dir.join(format!("{entity_id}.glb"));

            if is_mock {
                for (i, (_, suffix)) in [(0.5f32, "lod1"), (0.25, "lod2"), (0.1, "lod3")]
                    .iter()
                    .enumerate()
                {
                    let pct = ((i + 1) * 33).min(99) as u8;
                    let _ = tx.send(MeshGenEvent::Progress(
                        pct,
                        format!("Mock: generating {suffix}..."),
                    ));
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let output = out_dir.join(format!("{entity_id}_{suffix}.glb"));
                    let _ = std::fs::write(&output, crate::meshy::mock_glb());
                }
                let _ = tx.send(MeshGenEvent::Done);
                return;
            }

            for (ratio, suffix) in [(0.5f32, "lod1"), (0.25, "lod2"), (0.1, "lod3")] {
                let output = out_dir.join(format!("{entity_id}_{suffix}.glb"));
                let _ = tx.send(MeshGenEvent::Progress(
                    match suffix {
                        "lod1" => 10,
                        "lod2" => 40,
                        _ => 70,
                    },
                    format!("Generating {suffix}..."),
                ));
                let status = std::process::Command::new("npx")
                    .args([
                        "@gltf-transform/cli",
                        "simplify",
                        "--ratio",
                        &ratio.to_string(),
                        input.to_str().unwrap_or(""),
                        output.to_str().unwrap_or(""),
                    ])
                    .status();
                match status {
                    Ok(s) if s.success() => {}
                    Ok(s) => {
                        let _ = tx.send(MeshGenEvent::Failed(format!(
                            "gltf-transform simplify {suffix} exited {s}"
                        )));
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(MeshGenEvent::Failed(format!(
                            "gltf-transform simplify {suffix} error: {e}"
                        )));
                        return;
                    }
                }
            }
            let _ = tx.send(MeshGenEvent::Done);
        });

        let state = self.entity_mesh.entry(entity_idx).or_insert_with(MeshGenState::new);
        state.sub_stage = MeshSubStage::GeneratingLods;
        state.status = "Starting LOD generation...".into();
        state.progress = 0;
        state.receiver = Some(rx);
    }

    // -----------------------------------------------------------------------
    // UI
    // -----------------------------------------------------------------------

    fn show_entity_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Entities");
        egui::ScrollArea::vertical().id_salt("entity_list").show(ui, |ui| {
            for (i, entry) in self.bestiary.iter().enumerate() {
                let selected = self.selected_entity == i;

                let gen_indicator = match self.entity_gen.get(&i) {
                    Some(s) if s.generating => " ⏳",
                    Some(s) if s.gen_results.iter().any(|r| r.is_some()) => " ✓",
                    _ => "",
                };

                let mesh_badge = match self.entity_mesh.get(&i) {
                    Some(s) if s.receiver.is_some() => " ⏳",
                    _ => match self.entity_stage.get(&i).copied().unwrap_or(PipelineStage::Draft) {
                        PipelineStage::Draft => "",
                        PipelineStage::Approved => " 🖼",
                        PipelineStage::MeshDone => " 🔷",
                        PipelineStage::Rigged => " 🦴",
                        PipelineStage::Animated | PipelineStage::Merged => " 🎬",
                        PipelineStage::LodsComplete => " ✅",
                    },
                };

                let label = if selected {
                    format!("> {}{}{}", entry.display_name, gen_indicator, mesh_badge)
                } else {
                    format!("  {}{}{}", entry.display_name, gen_indicator, mesh_badge)
                };

                if ui.selectable_label(selected, label).clicked() && self.selected_entity != i {
                    self.selected_entity = i;
                    if let Some(e) = self.bestiary.get(i) {
                        if let Some(idx) = self.styles.iter().position(|s| s.name == e.ai_style) {
                            self.selected_style = idx;
                        }
                    }
                    self.approved_texture = None;
                    self.approved_loaded = false;
                    self.status.clear();
                }
            }
        });
    }

    fn show_styles_panel(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Styles", |ui| {
            let mut delete_idx: Option<usize> = None;
            for (i, style) in self.styles.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(style.name.as_str());
                    ui.add(
                        egui::TextEdit::singleline(&mut style.value)
                            .desired_width(ui.available_width() - 60.0),
                    );
                    let btn = egui::Button::new("X").fill(egui::Color32::DARK_RED);
                    if ui.add(btn).clicked() {
                        delete_idx = Some(i);
                    }
                });
            }
            if let Some(idx) = delete_idx {
                self.styles.remove(idx);
                if self.selected_style >= self.styles.len() {
                    self.selected_style = self.styles.len().saturating_sub(1);
                }
            }

            ui.separator();
            ui.label("Add style:");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_style_name)
                        .hint_text("name")
                        .desired_width(80.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_style_value)
                        .hint_text("value")
                        .desired_width(ui.available_width() - 60.0),
                );
                if ui.button("Add").clicked()
                    && !self.new_style_name.is_empty()
                    && !self.new_style_value.is_empty()
                {
                    self.styles.push(StylePreset {
                        name: self.new_style_name.as_str().into(),
                        value: std::mem::take(&mut self.new_style_value),
                    });
                    self.new_style_name.clear();
                }
            });
        });
    }

    fn spawn_preview(&self, entity_id: &str) {
        let _ = std::process::Command::new("cargo")
            .args(["run", "--quiet", "-p", "preview", "--", entity_id])
            .current_dir(find_repo_root())
            .spawn();
    }

    fn show_generation_panel(&mut self, ui: &mut egui::Ui) {
        let entity_idx = self.selected_entity;
        let Some(_entry) = self.bestiary.get(entity_idx) else {
            ui.label("No entity selected.");
            return;
        };

        let display_name = self.bestiary[entity_idx].display_name.clone();
        let entity_id = self.bestiary[entity_idx].id.clone();
        let stage = self.entity_stage.get(&entity_idx).copied().unwrap_or(PipelineStage::Draft);

        ui.horizontal(|ui| {
            ui.heading(format!("Entity: {}", display_name));
            let preview_enabled = stage >= PipelineStage::MeshDone;
            if ui
                .add_enabled(preview_enabled, egui::Button::new("\u{1f50d} Preview"))
                .clicked()
            {
                self.spawn_preview(entity_id.as_str());
            }
        });

        ui.horizontal(|ui| {
            ui.label("Prompt:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.bestiary[entity_idx].ai_prompt_base)
                    .desired_width(ui.available_width()),
            );
            let _ = resp;
        });

        if !self.styles.is_empty() {
            ui.horizontal(|ui| {
                ui.label("Style:");
                let current_name = self
                    .styles
                    .get(self.selected_style)
                    .map(|s| s.name.as_str())
                    .unwrap_or("—");
                egui::ComboBox::from_id_salt("style_select")
                    .selected_text(current_name)
                    .show_ui(ui, |ui| {
                        for (i, preset) in self.styles.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.selected_style,
                                i,
                                preset.name.as_str(),
                            );
                        }
                    });
                if let Some(style_name) =
                    self.styles.get(self.selected_style).map(|s| s.name.clone())
                {
                    self.bestiary[entity_idx].ai_style = style_name;
                }
            });
        }

        ui.horizontal(|ui| {
            ui.label("Backend:");
            egui::ComboBox::from_id_salt("backend_select")
                .selected_text(match self.backend {
                    BackendChoice::Mock => "Mock",
                    BackendChoice::OpenAI => "OpenAI",
                    BackendChoice::StabilityAi => "Stability AI",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.backend, BackendChoice::Mock, "Mock");
                    if self.openai_available {
                        ui.selectable_value(&mut self.backend, BackendChoice::OpenAI, "OpenAI");
                    } else {
                        ui.add_enabled(
                            false,
                            egui::SelectableLabel::new(false, "OpenAI (set SPRITE_GEN_API_KEY)"),
                        );
                    }
                    if self.stability_available {
                        ui.selectable_value(
                            &mut self.backend,
                            BackendChoice::StabilityAi,
                            "Stability AI",
                        );
                    } else {
                        ui.add_enabled(
                            false,
                            egui::SelectableLabel::new(
                                false,
                                "Stability AI (set STABILITY_API_KEY)",
                            ),
                        );
                    }
                });

            let generating = self
                .entity_gen
                .get(&entity_idx)
                .map(|s| s.generating)
                .unwrap_or(false);
            let gen_btn = egui::Button::new("Generate 4 variants");
            if ui.add_enabled(!generating, gen_btn).clicked() {
                self.spawn_generate_variants();
            }
            if generating {
                ui.spinner();
            }
            ui.checkbox(&mut self.remove_bg, "Remove background");
        });

        ui.separator();

        // Approved base.png
        ui.horizontal(|ui| {
            ui.label("Approved base:");
            if let Some(tex) = &self.approved_texture {
                ui.add(egui::Image::new(tex).max_size(egui::vec2(64.0, 64.0)));
            } else {
                ui.weak("(none saved)");
            }
        });

        ui.separator();

        let selected_variant = self
            .entity_gen
            .get(&entity_idx)
            .and_then(|s| s.selected_variant);
        let variant_textures: Vec<Option<egui::TextureHandle>> = (0..4)
            .map(|i| {
                self.entity_gen
                    .get(&entity_idx)
                    .and_then(|s| s.gen_results[i].clone())
            })
            .collect();

        let mut new_selected = selected_variant;

        ui.horizontal(|ui| {
            for (i, tex_slot) in variant_textures.iter().enumerate() {
                let is_selected = selected_variant == Some(i);
                let frame = egui::Frame::default()
                    .stroke(if is_selected {
                        egui::Stroke::new(3.0, egui::Color32::YELLOW)
                    } else {
                        egui::Stroke::new(1.0, egui::Color32::GRAY)
                    })
                    .inner_margin(2.0);

                let clicked = frame
                    .show(ui, |ui| {
                        let label = if is_selected {
                            format!("{} ✓", i + 1)
                        } else {
                            format!("{}", i + 1)
                        };

                        if let Some(tex) = tex_slot {
                            let img = egui::Image::new(tex)
                                .max_size(egui::vec2(80.0, 80.0))
                                .sense(egui::Sense::click());
                            let resp = ui.add(img);
                            ui.label(&label);
                            resp.clicked()
                        } else {
                            let (resp, _painter) = ui.allocate_painter(
                                egui::vec2(80.0, 80.0),
                                egui::Sense::click(),
                            );
                            ui.label(&label);
                            let _ = resp;
                            false
                        }
                    })
                    .inner;

                if clicked {
                    new_selected = Some(i);
                }
            }
        });

        if new_selected != selected_variant {
            self.entity_gen
                .entry(entity_idx)
                .or_insert_with(EntityGenState::new)
                .selected_variant = new_selected;
        }

        ui.separator();

        let approve_enabled = selected_variant.is_some();
        if ui
            .add_enabled(approve_enabled, egui::Button::new("Approve selected → save base.png"))
            .clicked()
        {
            self.approve_base_png(ui.ctx());
        }

        if !self.status.is_empty() {
            ui.label(&self.status);
        }

        ui.separator();

        // Section B: 3D Mesh
        self.show_mesh_panel(ui);

        ui.separator();

        // Section C: Rig & Animate
        self.show_rig_animate_panel(ui);

        ui.separator();

        // Section D: LODs
        self.show_lods_panel(ui);
    }

    fn show_mesh_panel(&mut self, ui: &mut egui::Ui) {
        let entity_idx = self.selected_entity;
        let stage = self.entity_stage.get(&entity_idx).copied().unwrap_or(PipelineStage::Draft);

        ui.collapsing("3D Mesh", |ui| {
            let enabled = stage >= PipelineStage::Approved;
            if !enabled {
                ui.disable();
            }

            // Mesh prompt field
            ui.horizontal(|ui| {
                ui.label("Mesh prompt:");
                if let Some(entry) = self.bestiary.get_mut(entity_idx) {
                    ui.add(
                        egui::TextEdit::singleline(&mut entry.mesh_prompt)
                            .hint_text("(defaults to ai_prompt_base)")
                            .desired_width(ui.available_width()),
                    );
                }
            });

            // Backend selector
            ui.horizontal(|ui| {
                ui.label("Backend:");
                ui.selectable_value(&mut self.mesh_backend, MeshBackend::Mock, "Mock");
                if self.meshy_available {
                    ui.selectable_value(&mut self.mesh_backend, MeshBackend::Live, "Live Meshy");
                } else {
                    ui.add_enabled(
                        false,
                        egui::SelectableLabel::new(false, "Live Meshy (set MESHY_API_KEY)"),
                    );
                }
            });

            let generating = self
                .entity_mesh
                .get(&entity_idx)
                .map(|s| s.receiver.is_some())
                .unwrap_or(false);

            if ui
                .add_enabled(
                    enabled && !generating,
                    egui::Button::new("Generate 3D Mesh"),
                )
                .clicked()
            {
                self.spawn_mesh_gen(entity_idx);
            }

            if let Some(state) = self.entity_mesh.get(&entity_idx) {
                if !state.status.is_empty() {
                    ui.label(&state.status);
                    ui.add(egui::ProgressBar::new(state.progress as f32 / 100.0));
                }
            }

            if stage >= PipelineStage::MeshDone {
                if let Some(entry) = self.bestiary.get(entity_idx) {
                    ui.label(format!(
                        "Mesh: assets/models/{}/{}_mesh.glb",
                        entry.id, entry.id
                    ));
                }
            }
        });
    }

    fn show_rig_animate_panel(&mut self, ui: &mut egui::Ui) {
        let entity_idx = self.selected_entity;
        let stage = self.entity_stage.get(&entity_idx).copied().unwrap_or(PipelineStage::Draft);

        ui.collapsing("Rig & Animate", |ui| {
            let enabled = stage >= PipelineStage::MeshDone;
            if !enabled {
                ui.disable();
            }

            ui.label("Animation clips:");
            // Ensure mesh state exists so we can get selected_clips
            let state = self.entity_mesh.entry(entity_idx).or_insert_with(MeshGenState::new);
            // Make sure selected_clips has the right length
            state.selected_clips.resize(CLIP_NAMES.len(), true);

            // Borrow selected_clips out temporarily to render checkboxes
            for (i, clip_name) in CLIP_NAMES.iter().enumerate() {
                let selected = state.selected_clips.get_mut(i).unwrap();
                ui.checkbox(selected, *clip_name);
            }

            let generating = state.receiver.is_some();
            let status = state.status.clone();
            let progress = state.progress;

            if ui
                .add_enabled(enabled && !generating, egui::Button::new("Rig & Animate"))
                .clicked()
            {
                self.spawn_rig_animate(entity_idx);
            }

            if !status.is_empty() {
                ui.label(&status);
                ui.add(egui::ProgressBar::new(progress as f32 / 100.0));
            }

            if stage >= PipelineStage::Merged {
                if let Some(entry) = self.bestiary.get(entity_idx) {
                    ui.label(format!("Merged: assets/models/{}/{}.glb", entry.id, entry.id));
                }
            }
        });
    }

    fn show_lods_panel(&mut self, ui: &mut egui::Ui) {
        let entity_idx = self.selected_entity;
        let stage = self.entity_stage.get(&entity_idx).copied().unwrap_or(PipelineStage::Draft);

        ui.collapsing("LODs", |ui| {
            let enabled = stage >= PipelineStage::Merged;
            if !enabled {
                ui.disable();
            }

            let generating = self
                .entity_mesh
                .get(&entity_idx)
                .map(|s| s.receiver.is_some())
                .unwrap_or(false);

            if ui
                .add_enabled(enabled && !generating, egui::Button::new("Generate LODs"))
                .clicked()
            {
                self.spawn_lod_gen(entity_idx);
            }

            if let Some(state) = self.entity_mesh.get(&entity_idx) {
                if state.sub_stage == MeshSubStage::GeneratingLods && !state.status.is_empty() {
                    ui.label(&state.status);
                    ui.add(egui::ProgressBar::new(state.progress as f32 / 100.0));
                }
            }

            if stage >= PipelineStage::LodsComplete {
                ui.label("LODs: lod1 / lod2 / lod3 complete");
            }
        });
    }
}

impl eframe::App for StudioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.approved_loaded {
            self.load_approved_base(ctx);
        }
        self.poll_all_gen_receivers(ctx);
        self.poll_all_mesh_receivers(ctx);

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Character Studio");
                ui.separator();
                let save_btn = egui::Button::new("💾 Save to Bestiary")
                    .fill(egui::Color32::from_rgb(0, 100, 0));
                if ui.add(save_btn).clicked() {
                    self.save_bestiary();
                }
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                cols[0].group(|ui| {
                    self.show_entity_list(ui);
                    ui.separator();
                    self.show_styles_panel(ui);
                });

                cols[1].group(|ui| {
                    let height = ui.available_height();
                    egui::ScrollArea::vertical()
                        .id_salt("generation_panel")
                        .max_height(height)
                        .show(ui, |ui| {
                            self.show_generation_panel(ui);
                        });
                });
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rgba_to_egui(ctx: &egui::Context, img: &image::RgbaImage, label: &str) -> egui::TextureHandle {
    let size = [img.width() as usize, img.height() as usize];
    let pixels = img.as_flat_samples();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
    ctx.load_texture(label, color_image, egui::TextureOptions::LINEAR)
}

fn find_bestiary_path() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("assets/bestiary.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn find_assets_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("assets");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn find_repo_root() -> std::path::PathBuf {
    let mut dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = std::fs::read_to_string(&cargo_toml).unwrap_or_default();
            if content.contains("[workspace]") {
                return dir;
            }
        }
        if !dir.pop() {
            break;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn scan_stage(models_dir: &std::path::Path, sprites_dir: &std::path::Path, id: &str) -> PipelineStage {
    let md = models_dir.join(id);
    let clip_names = ["idle", "walk", "attack", "death", "run", "behit"];
    if md.join(format!("{id}_lod1.glb")).exists() {
        return PipelineStage::LodsComplete;
    }
    if md.join(format!("{id}.glb")).exists() {
        return PipelineStage::Merged;
    }
    if clip_names.iter().all(|c| md.join(format!("{c}.glb")).exists()) {
        return PipelineStage::Animated;
    }
    if md.join(format!("{id}_rigged.glb")).exists() {
        return PipelineStage::Rigged;
    }
    if md.join(format!("{id}_mesh.glb")).exists() {
        return PipelineStage::MeshDone;
    }
    if sprites_dir.join(id).join("base.png").exists() {
        return PipelineStage::Approved;
    }
    PipelineStage::Draft
}
