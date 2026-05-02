//! Main application state and UI for the Sprite Studio.

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

pub struct StudioApp {
    bestiary: Vec<BestiaryEntry>,
    styles: Vec<StylePreset>,
    selected_entity: usize,
    selected_style: usize,
    backend: BackendChoice,
    openai_available: bool,
    stability_available: bool,

    // Per-entity generation state — keyed by bestiary index
    entity_gen: HashMap<usize, EntityGenState>,

    // Approved base.png for the current entity (loaded from disk, persists across navigation)
    approved_texture: Option<egui::TextureHandle>,
    approved_loaded: bool,

    // Post-processing toggles
    remove_bg: bool,

    // Output dir
    output_dir: PathBuf,

    // Status message for the user
    status: String,
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

        let openai_available = std::env::var("SPRITE_GEN_API_KEY").is_ok();
        let stability_available = std::env::var("STABILITY_API_KEY").is_ok();

        let default_style = bestiary
            .first()
            .and_then(|e| styles.iter().position(|s| s.name == e.ai_style))
            .unwrap_or(0);

        let backend = if stability_available {
            BackendChoice::StabilityAi
        } else {
            BackendChoice::Mock
        };

        Self {
            bestiary,
            styles,
            selected_entity: 0,
            selected_style: default_style,
            backend,
            openai_available,
            stability_available,
            entity_gen: HashMap::new(),
            remove_bg: true,
            approved_texture: None,
            approved_loaded: false,
            output_dir,
            status: String::new(),
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

    // Poll every active per-entity receiver so background generation continues when navigated away.
    fn poll_all_gen_receivers(&mut self, ctx: &egui::Context) {
        let entity_indices: Vec<usize> = self.entity_gen.keys().cloned().collect();
        let mut any_update = false;

        for entity_idx in entity_indices {
            let state = self.entity_gen.get_mut(&entity_idx).unwrap();
            if state.gen_receiver.is_none() {
                continue;
            }

            // Drain the channel into a local vec to avoid holding &state.gen_receiver
            // while mutating other state fields.
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
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn show_entity_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Entities");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, entry) in self.bestiary.iter().enumerate() {
                let selected = self.selected_entity == i;

                let gen_indicator = match self.entity_gen.get(&i) {
                    Some(s) if s.generating => " ⏳",
                    Some(s) if s.gen_results.iter().any(|r| r.is_some()) => " ✓",
                    _ => "",
                };

                let label = if selected {
                    format!("> {}{}", entry.display_name, gen_indicator)
                } else {
                    format!("  {}{}", entry.display_name, gen_indicator)
                };

                if ui.selectable_label(selected, label).clicked() && self.selected_entity != i {
                    self.selected_entity = i;
                    // Sync style selector to the new entity's default
                    if let Some(e) = self.bestiary.get(i) {
                        if let Some(idx) = self.styles.iter().position(|s| s.name == e.ai_style) {
                            self.selected_style = idx;
                        }
                    }
                    // Generation state is kept in entity_gen — background work continues.
                    self.approved_texture = None;
                    self.approved_loaded = false;
                    self.status.clear();
                }
            }
        });
    }

    fn show_generation_panel(&mut self, ui: &mut egui::Ui) {
        let entity_idx = self.selected_entity;
        let Some(entry) = self.bestiary.get(entity_idx).cloned() else {
            ui.label("No entity selected.");
            return;
        };

        ui.heading(format!("Entity: {}", entry.display_name));

        ui.horizontal(|ui| {
            ui.label("Prompt:");
            ui.add(
                egui::TextEdit::singleline(&mut entry.ai_prompt_base.clone())
                    .desired_width(ui.available_width()),
            );
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

        // Approved base.png (persists across navigation)
        ui.horizontal(|ui| {
            ui.label("Approved base:");
            if let Some(tex) = &self.approved_texture {
                ui.add(egui::Image::new(tex).max_size(egui::vec2(64.0, 64.0)));
            } else {
                ui.weak("(none saved)");
            }
        });

        ui.separator();

        // Pre-extract per-slot data to avoid holding an entity_gen borrow inside closures.
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

        // 4 variant thumbnails
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
                            // Empty slot is not clickable
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
    }
}

impl eframe::App for StudioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.approved_loaded {
            self.load_approved_base(ctx);
        }
        self.poll_all_gen_receivers(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                // Left panel — entity list
                cols[0].group(|ui| {
                    self.show_entity_list(ui);
                });

                // Right panel — generation
                cols[1].group(|ui| {
                    self.show_generation_panel(ui);
                });
            });
        });
    }
}

fn rgba_to_egui(ctx: &egui::Context, img: &image::RgbaImage, label: &str) -> egui::TextureHandle {
    let size = [img.width() as usize, img.height() as usize];
    let pixels = img.as_flat_samples();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
    ctx.load_texture(label, color_image, egui::TextureOptions::LINEAR)
}

/// Walk up from cwd looking for `assets/bestiary.toml`.
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

/// Walk up from cwd looking for the `assets/` directory.
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
