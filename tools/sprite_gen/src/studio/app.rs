//! Main application state and UI for the Sprite Studio.

use crate::{
    assembler::assemble_atlas,
    generator::{FrameRequest, MockGenerator, SpriteGenerator},
    layout::AtlasLayout,
    manifest::{to_ron, AtlasManifest},
    openai::OpenAiDalleGenerator,
    stability::StabilityGenerator,
};
use fellytip_shared::bestiary::{load_bestiary, BestiaryEntry};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Instant,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    Mock,
    OpenAI,
    StabilityAi,
}

pub struct StudioApp {
    bestiary: Vec<BestiaryEntry>,
    selected_entity: usize,
    backend: BackendChoice,
    openai_available: bool,
    stability_available: bool,

    // Generation state — 4 variant slots
    generating: bool,
    gen_results: Vec<Option<egui::TextureHandle>>,
    gen_images: Vec<Option<image::RgbaImage>>,
    selected_variant: Option<usize>,
    gen_receiver: Option<Receiver<(usize, image::RgbaImage)>>,

    // Animation preview
    selected_anim: usize,
    preview_frame: usize,
    preview_textures: Vec<egui::TextureHandle>,
    anim_generating: bool,
    anim_receiver: Option<Receiver<(usize, image::RgbaImage)>>,
    anim_timer: Instant,

    // Approved base.png for the current entity (loaded from disk, persists across navigation)
    approved_texture: Option<egui::TextureHandle>,
    approved_loaded: bool,

    // Output dir
    output_dir: PathBuf,

    // Status message for the user
    status: String,
}

impl StudioApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let bestiary_path = find_bestiary_path();
        let bestiary = bestiary_path
            .as_ref()
            .and_then(|p| load_bestiary(p).ok())
            .unwrap_or_default();

        let output_dir = find_assets_dir()
            .map(|d| d.join("sprites"))
            .unwrap_or_else(|| PathBuf::from("assets/sprites"));

        let openai_available = std::env::var("SPRITE_GEN_API_KEY").is_ok();
        let stability_available = std::env::var("STABILITY_API_KEY").is_ok();

        Self {
            bestiary,
            selected_entity: 0,
            backend: BackendChoice::Mock,
            openai_available,
            stability_available,
            generating: false,
            gen_results: vec![None, None, None, None],
            gen_images: vec![None, None, None, None],
            selected_variant: None,
            gen_receiver: None,
            selected_anim: 0,
            preview_frame: 0,
            preview_textures: Vec::new(),
            anim_generating: false,
            anim_receiver: None,
            anim_timer: Instant::now(),
            approved_texture: None,
            approved_loaded: false,
            output_dir,
            status: String::new(),
        }
    }

    fn current_entity(&self) -> Option<&BestiaryEntry> {
        self.bestiary.get(self.selected_entity)
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

    fn poll_gen_receiver(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.gen_receiver {
            let mut done_count = 0;
            loop {
                match rx.try_recv() {
                    Ok((idx, img)) => {
                        let label = format!("variant_{}", idx);
                        self.gen_results[idx] = Some(rgba_to_egui(ctx, &img, &label));
                        self.gen_images[idx] = Some(img);
                        done_count += 1;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.generating = false;
                        self.gen_receiver = None;
                        break;
                    }
                }
            }
            // Check if all 4 slots are filled
            if self.gen_results.iter().all(|r| r.is_some()) {
                self.generating = false;
                self.gen_receiver = None;
            }
            if done_count > 0 {
                ctx.request_repaint();
            }
        }
    }

    fn poll_anim_receiver(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.anim_receiver {
            let mut done_count = 0;
            loop {
                match rx.try_recv() {
                    Ok((idx, img)) => {
                        let label = format!("anim_frame_{}", idx);
                        let tex = rgba_to_egui(ctx, &img, &label);
                        if idx < self.preview_textures.len() {
                            self.preview_textures[idx] = tex;
                        } else {
                            // Fill gaps with empty if needed
                            while self.preview_textures.len() < idx {
                                // placeholder — won't be shown until filled
                                self.preview_textures.push(tex.clone());
                            }
                            self.preview_textures.push(tex);
                        }
                        done_count += 1;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.anim_generating = false;
                        self.anim_receiver = None;
                        break;
                    }
                }
            }
            if done_count > 0 {
                ctx.request_repaint();
            }
        }
    }

    fn advance_anim_timer(&mut self, ctx: &egui::Context) {
        if self.preview_textures.is_empty() {
            return;
        }
        let fps = self
            .current_entity()
            .and_then(|e| e.animations.get(self.selected_anim))
            .map(|a| a.fps as f32)
            .unwrap_or(4.0)
            .max(1.0);
        let frame_dur = std::time::Duration::from_secs_f32(1.0 / fps);
        if self.anim_timer.elapsed() >= frame_dur {
            self.preview_frame = (self.preview_frame + 1) % self.preview_textures.len();
            self.anim_timer = Instant::now();
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(frame_dur.saturating_sub(self.anim_timer.elapsed()));
        }
    }

    fn spawn_generate_variants(&mut self) {
        if self.bestiary.is_empty() {
            return;
        }
        let entry = self.bestiary[self.selected_entity].clone();
        self.gen_results = vec![None, None, None, None];
        self.gen_images = vec![None, None, None, None];
        self.selected_variant = None;
        self.generating = true;

        let (tx, rx) = mpsc::channel::<(usize, image::RgbaImage)>();
        self.gen_receiver = Some(rx);

        for variant in 0..4usize {
            let tx = tx.clone();
            let entry = entry.clone();
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
                    tile_size: 64,
                    base_prompt: &entry.ai_prompt_base,
                    style: &entry.ai_style,
                };
                if let Ok(img) = generator.generate(req) {
                    let _ = tx.send((variant, img));
                }
            });
        }
    }

    fn spawn_generate_anim_frames(&mut self) {
        if self.bestiary.is_empty() {
            return;
        }
        let entry = self.bestiary[self.selected_entity].clone();
        let anim = match entry.animations.get(self.selected_anim) {
            Some(a) => a.clone(),
            None => return,
        };

        self.preview_textures.clear();
        self.preview_frame = 0;
        self.anim_generating = true;

        let frame_count = anim.frames as usize;
        let (tx, rx): (Sender<(usize, image::RgbaImage)>, _) = mpsc::channel();
        self.anim_receiver = Some(rx);

        for frame in 0..frame_count {
            let tx = tx.clone();
            let entry = entry.clone();
            let anim_name = anim.name.clone();
            let generator = self.make_generator();
            std::thread::spawn(move || {
                let req = FrameRequest {
                    entity_id: entry.id.as_str(),
                    animation: anim_name.as_str(),
                    direction: 0,
                    frame: frame as u32,
                    tile_size: 64,
                    base_prompt: &entry.ai_prompt_base,
                    style: &entry.ai_style,
                };
                if let Ok(img) = generator.generate(req) {
                    let _ = tx.send((frame, img));
                }
            });
        }
    }

    fn approve_base_png(&mut self, ctx: &egui::Context) {
        let Some(variant_idx) = self.selected_variant else {
            self.status = "No variant selected.".into();
            return;
        };
        let Some(img) = self.gen_images[variant_idx].clone() else {
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

    fn approve_atlas(&mut self) {
        let Some(entry) = self.current_entity() else {
            return;
        };
        let entry = entry.clone();
        let anim = match entry.animations.get(self.selected_anim) {
            Some(a) => a.clone(),
            None => return,
        };
        let out_dir = self.output_dir.join(entry.id.as_str());
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            self.status = format!("Failed to create dir: {e}");
            return;
        }

        let generator = self.make_generator();
        let layout = AtlasLayout::from_entry(&entry);
        match assemble_atlas(generator.as_ref() as &(dyn SpriteGenerator + Sync), &entry, &layout, 4) {
            Ok(atlas) => {
                let png_path = out_dir.join(format!("{}.png", anim.name));
                let ron_path = out_dir.join(format!("{}.ron", anim.name));
                if let Err(e) = atlas.save(&png_path) {
                    self.status = format!("Atlas save failed: {e}");
                    return;
                }
                let manifest = AtlasManifest::from(&layout);
                match to_ron(&manifest) {
                    Ok(ron_text) => {
                        if let Err(e) = std::fs::write(&ron_path, ron_text) {
                            self.status = format!("RON save failed: {e}");
                            return;
                        }
                        self.status = format!(
                            "Atlas saved to {} and {}",
                            png_path.display(),
                            ron_path.display()
                        );
                    }
                    Err(e) => self.status = format!("RON serialization failed: {e}"),
                }
            }
            Err(e) => self.status = format!("Atlas generation failed: {e}"),
        }
    }

    fn show_entity_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Entities");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, entry) in self.bestiary.iter().enumerate() {
                let selected = self.selected_entity == i;
                let label = if selected {
                    format!("> {}", entry.display_name)
                } else {
                    format!("  {}", entry.display_name)
                };
                if ui.selectable_label(selected, label).clicked() && self.selected_entity != i {
                    self.selected_entity = i;
                    self.selected_anim = 0;
                    self.gen_results = vec![None, None, None, None];
                    self.gen_images = vec![None, None, None, None];
                    self.selected_variant = None;
                    self.gen_receiver = None;
                    self.generating = false;
                    self.preview_textures.clear();
                    self.preview_frame = 0;
                    self.anim_receiver = None;
                    self.anim_generating = false;
                    self.approved_texture = None;
                    self.approved_loaded = false;
                    self.status.clear();
                }
            }
        });
    }

    fn show_generation_panel(&mut self, ui: &mut egui::Ui) {
        let Some(entry) = self.bestiary.get(self.selected_entity).cloned() else {
            ui.label("No entity selected.");
            return;
        };

        ui.heading(format!("Entity: {}", entry.display_name));

        ui.horizontal(|ui| {
            ui.label("Prompt:");
            ui.add(
                egui::TextEdit::singleline(
                    &mut entry.ai_prompt_base.clone(),
                )
                .desired_width(ui.available_width()),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Style:");
            ui.add(
                egui::TextEdit::singleline(&mut entry.ai_style.clone())
                    .desired_width(ui.available_width()),
            );
        });

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

            let gen_btn = egui::Button::new("Generate 4 variants");
            if ui.add_enabled(!self.generating, gen_btn).clicked() {
                self.spawn_generate_variants();
            }
            if self.generating {
                ui.spinner();
            }
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

        // 4 variant thumbnails
        ui.horizontal(|ui| {
            for i in 0..4 {
                let is_selected = self.selected_variant == Some(i);
                let frame = egui::Frame::default()
                    .stroke(if is_selected {
                        egui::Stroke::new(3.0, egui::Color32::YELLOW)
                    } else {
                        egui::Stroke::new(1.0, egui::Color32::GRAY)
                    })
                    .inner_margin(2.0);

                frame.show(ui, |ui| {
                    let label = if is_selected {
                        format!("{} ✓", i + 1)
                    } else {
                        format!("{}", i + 1)
                    };

                    if let Some(tex) = &self.gen_results[i] {
                        let img = egui::Image::new(tex)
                            .max_size(egui::vec2(80.0, 80.0))
                            .sense(egui::Sense::click());
                        let resp = ui.add(img);
                        ui.label(&label);
                        if resp.clicked() {
                            self.selected_variant = Some(i);
                        }
                    } else {
                        let (resp, _painter) =
                            ui.allocate_painter(egui::vec2(80.0, 80.0), egui::Sense::click());
                        ui.label(&label);
                        if resp.clicked() && self.gen_results[i].is_some() {
                            self.selected_variant = Some(i);
                        }
                    }
                });
            }
        });

        ui.separator();

        let approve_enabled = self.selected_variant.is_some();
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

    fn show_anim_preview_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Animation preview");

        let Some(entry) = self.bestiary.get(self.selected_entity).cloned() else {
            return;
        };

        if entry.animations.is_empty() {
            ui.label("No animations defined.");
            return;
        }

        // Animation selector
        ui.horizontal(|ui| {
            ui.label("Anim:");
            let current_anim_name = entry
                .animations
                .get(self.selected_anim)
                .map(|a| a.name.as_str())
                .unwrap_or("—");
            egui::ComboBox::from_id_salt("anim_select")
                .selected_text(current_anim_name)
                .show_ui(ui, |ui| {
                    for (i, anim) in entry.animations.iter().enumerate() {
                        if ui
                            .selectable_label(self.selected_anim == i, anim.name.as_str())
                            .clicked()
                            && self.selected_anim != i
                        {
                            self.selected_anim = i;
                            self.preview_textures.clear();
                            self.preview_frame = 0;
                            self.anim_receiver = None;
                            self.anim_generating = false;
                        }
                    }
                });

            // Frame navigation
            let frame_total = self.preview_textures.len().max(1);
            if ui.button("◄").clicked() && self.preview_frame > 0 {
                self.preview_frame -= 1;
                self.anim_timer = Instant::now();
            }
            ui.label(format!("frame {}/{}", self.preview_frame + 1, frame_total));
            if ui.button("►").clicked() && self.preview_frame + 1 < frame_total {
                self.preview_frame += 1;
                self.anim_timer = Instant::now();
            }

            let fps = entry
                .animations
                .get(self.selected_anim)
                .map(|a| a.fps)
                .unwrap_or(4);
            ui.label(format!("FPS: {}", fps));
        });

        // Frame thumbnail
        if let Some(tex) = self.preview_textures.get(self.preview_frame) {
            ui.add(egui::Image::new(tex).max_size(egui::vec2(128.0, 128.0)));
        } else {
            ui.allocate_space(egui::vec2(128.0, 128.0));
        }

        ui.horizontal(|ui| {
            let gen_btn = egui::Button::new("Generate frames");
            if ui.add_enabled(!self.anim_generating, gen_btn).clicked() {
                self.spawn_generate_anim_frames();
            }
            if self.anim_generating {
                ui.spinner();
            }

            let approve_enabled = !self.preview_textures.is_empty();
            if ui
                .add_enabled(approve_enabled, egui::Button::new("Approve → atlas"))
                .clicked()
            {
                self.approve_atlas();
            }
        });
    }
}

impl eframe::App for StudioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.approved_loaded {
            self.load_approved_base(ctx);
        }
        self.poll_gen_receiver(ctx);
        self.poll_anim_receiver(ctx);
        self.advance_anim_timer(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                // Left panel — entity list
                cols[0].group(|ui| {
                    self.show_entity_list(ui);
                });

                // Right panel — generation + animation preview
                cols[1].group(|ui| {
                    // Split right panel vertically
                    let available = ui.available_height();
                    let top_height = available * 0.6;

                    egui::Frame::default().show(ui, |ui| {
                        ui.set_min_height(top_height);
                        self.show_generation_panel(ui);
                    });

                    ui.separator();

                    egui::Frame::default().show(ui, |ui| {
                        self.show_anim_preview_panel(ui);
                    });
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
    // Check argv for --bestiary first
    let args: Vec<String> = std::env::args().collect();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--bestiary" {
            if let Some(path) = it.next() {
                return Some(PathBuf::from(path));
            }
        }
    }

    // Walk upward from cwd
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
