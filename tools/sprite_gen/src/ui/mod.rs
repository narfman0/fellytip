//! Sprite Studio egui application.
//!
//! Workflow:
//!   1. Select an entity from the bestiary sidebar.
//!   2. Click "Generate 4 Variants" — shows a 2×2 thumbnail grid as images arrive.
//!   3. Click "Pick" under the preferred variant.
//!   4. All animation frames generate one-by-one; the grid fills in live.
//!   5. Click "↺" on any cell to re-roll just that frame.
//!   6. Click "Save Atlas" to write the PNG + RON sidecar.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use eframe::egui::{self, ColorImage, Context, TextureHandle, Ui};
use image::RgbaImage;
use smol_str::SmolStr;

use fellytip_shared::bestiary::{load_bestiary, BestiaryEntry};
use sprite_gen::{
    generator::{FrameRequest, MockGenerator, SpriteGenerator},
    layout::{AtlasLayout, TILE_SIZE},
    manifest::{to_ron, AtlasManifest},
    openai::OpenAiDalleGenerator,
};

// ── Display constants ─────────────────────────────────────────────────────────

const VARIANT_COUNT: usize = 4;
const THUMB_PX: f32 = 256.0;
const CELL_PX: f32 = 96.0;

// ── Backend ───────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum BackendKind {
    Mock,
    OpenAi,
}

// ── Work channel ──────────────────────────────────────────────────────────────

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
struct CellKey {
    anim: SmolStr,
    dir: u32,
    frame: u32,
}

enum WorkResult {
    Variant { idx: usize, raw: RgbaImage },
    Frame { key: CellKey, raw: RgbaImage },
    Error(String),
}

// ── Phase state machine ───────────────────────────────────────────────────────

struct Cell {
    texture: TextureHandle,
    raw: RgbaImage,
}

enum Phase {
    Idle,
    Variants {
        items: Vec<Option<Cell>>,
        done: bool,
    },
    Animation {
        layout: AtlasLayout,
        cells: HashMap<CellKey, Cell>,
        total: usize,
        done: bool,
    },
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    entries: Vec<BestiaryEntry>,
    selected: Option<usize>,
    phase: Phase,
    backend_kind: BackendKind,
    api_key: String,
    output_dir: String,
    log: Vec<String>,
    result_tx: mpsc::SyncSender<WorkResult>,
    result_rx: mpsc::Receiver<WorkResult>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        tracing::info!("pixels_per_point = {}", cc.egui_ctx.pixels_per_point());

        let (tx, rx) = mpsc::sync_channel(128);
        let bestiary_path = default_bestiary_path();
        let entries = load_bestiary(&bestiary_path)
            .map_err(|e| tracing::warn!("bestiary load failed: {e}"))
            .unwrap_or_default();

        Self {
            entries,
            selected: None,
            phase: Phase::Idle,
            backend_kind: BackendKind::Mock,
            api_key: String::new(),
            output_dir: "crates/client/assets/sprites".to_owned(),
            log: Vec::new(),
            result_tx: tx,
            result_rx: rx,
        }
    }
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain_results(ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(150));

        // egui panel order must be: Top → Bottom → Side → Central.
        egui::TopBottomPanel::top("toolbar").min_height(36.0).show(ctx, |ui| self.render_toolbar(ui));
        egui::TopBottomPanel::bottom("log_panel").max_height(120.0).min_height(24.0).show(ctx, |ui| self.render_log(ui));
        egui::SidePanel::left("sidebar").min_width(190.0).show(ctx, |ui| self.render_sidebar(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.render_center(ui, ctx));
    }
}

// ── Result draining ───────────────────────────────────────────────────────────

impl App {
    fn drain_results(&mut self, ctx: &Context) {
        while let Ok(msg) = self.result_rx.try_recv() {
            match msg {
                WorkResult::Variant { idx, raw } => {
                    if let Phase::Variants { items, .. } = &mut self.phase {
                        if let Some(slot) = items.get_mut(idx) {
                            let tex = img_to_tex(ctx, &format!("var{idx}"), &raw);
                            *slot = Some(Cell { texture: tex, raw });
                        }
                    }
                }
                WorkResult::Frame { key, raw } => {
                    if let Phase::Animation { cells, .. } = &mut self.phase {
                        let name = format!("cell_{}_{}_{}", key.anim, key.dir, key.frame);
                        let tex = img_to_tex(ctx, &name, &raw);
                        cells.insert(key, Cell { texture: tex, raw });
                    }
                }
                WorkResult::Error(msg) => {
                    self.log.push(format!("[error] {msg}"));
                }
            }

            // Auto-flip done flag.
            match &mut self.phase {
                Phase::Variants { items, done } => {
                    if !*done && items.iter().all(|i| i.is_some()) {
                        *done = true;
                        self.log.push("All variants ready — pick one.".to_owned());
                    }
                }
                Phase::Animation { cells, total, done, .. } => {
                    if !*done && cells.len() >= *total {
                        *done = true;
                        self.log.push("Animation complete.".to_owned());
                    }
                }
                _ => {}
            }
        }
    }
}

// ── Panels ────────────────────────────────────────────────────────────────────

impl App {
    fn render_toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Backend:");
            egui::ComboBox::from_id_salt("backend_combo")
                .selected_text(match self.backend_kind {
                    BackendKind::Mock   => "Mock (offline)",
                    BackendKind::OpenAi => "OpenAI DALL-E 3",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.backend_kind, BackendKind::Mock, "Mock (offline)");
                    ui.selectable_value(&mut self.backend_kind, BackendKind::OpenAi, "OpenAI DALL-E 3");
                });

            if self.backend_kind == BackendKind::OpenAi {
                ui.separator();
                ui.label("API Key:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.api_key)
                        .password(true)
                        .desired_width(280.0)
                        .hint_text("sk-…"),
                );
            }

            ui.separator();
            ui.label("Output:");
            ui.add(egui::TextEdit::singleline(&mut self.output_dir).desired_width(300.0));
        });
    }

    fn render_sidebar(&mut self, ui: &mut Ui) {
        ui.heading("Bestiary");
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for i in 0..self.entries.len() {
                let label = format!("{} ({})", self.entries[i].display_name, self.entries[i].id);
                let selected = self.selected == Some(i);
                if ui.selectable_label(selected, &label).clicked() && !selected {
                    self.selected = Some(i);
                    self.phase = Phase::Idle;
                }
            }
        });
    }

    fn render_log(&self, ui: &mut Ui) {
        ui.strong("Log");
        egui::ScrollArea::vertical()
            .id_salt("log_scroll")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &self.log {
                    ui.monospace(line);
                }
            });
    }

    fn render_center(&mut self, ui: &mut Ui, ctx: &Context) {
        let Some(idx) = self.selected else {
            ui.centered_and_justified(|ui| {
                ui.label("← Select an entity from the bestiary to begin.");
            });
            return;
        };

        // Dispatch by phase. Collect deferred actions to avoid borrow conflicts.
        let action = match &self.phase {
            Phase::Idle              => self.ui_idle(ui, idx),
            Phase::Variants { .. }  => self.ui_variants(ui, idx),
            Phase::Animation { .. } => self.ui_animation(ui, idx),
        };

        self.apply_action(action, idx, ctx);
    }
}

// ── Phase UI — returns deferred actions ───────────────────────────────────────

enum Action {
    None,
    StartVariants,
    PickVariant(usize),
    RerollFrame(CellKey),
    SaveAtlas,
    BackToIdle,
}

impl App {
    fn ui_idle(&self, ui: &mut Ui, idx: usize) -> Action {
        let entry = &self.entries[idx];
        let mut action = Action::None;
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.heading(&entry.display_name);
                ui.add_space(8.0);
                ui.label(format!("Prompt: {}", entry.ai_prompt_base));
                ui.label(format!("Style:  {}", entry.ai_style));
                ui.add_space(24.0);
                if ui.button("Generate 4 Variants").clicked() {
                    action = Action::StartVariants;
                }
            });
        });
        action
    }

    fn ui_variants(&self, ui: &mut Ui, _idx: usize) -> Action {
        let Phase::Variants { items, done } = &self.phase else { return Action::None; };

        let ready = items.iter().filter(|i| i.is_some()).count();
        let mut action = Action::None;

        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.heading("Pick a Variant");
                ui.add_space(12.0);
                ui.label(format!("{ready}/{VARIANT_COUNT} ready"));
                if !done {
                    ui.add(egui::Spinner::new());
                }
            });
            ui.add_space(8.0);

            egui::ScrollArea::both().id_salt("variants_scroll").show(ui, |ui| {
                // 2 columns
                ui.horizontal(|ui| {
                    for (i, item) in items.iter().enumerate() {
                        ui.vertical(|ui| {
                            let tex_id = item.as_ref().map(|c| c.texture.id());
                            if let Some(tid) = tex_id {
                                ui.add(egui::Image::new((tid, egui::vec2(THUMB_PX, THUMB_PX))));
                                if ui.button(format!("Pick #{}", i + 1)).clicked() {
                                    action = Action::PickVariant(i);
                                }
                            } else {
                                ui.add_sized(
                                    egui::vec2(THUMB_PX, THUMB_PX),
                                    egui::Label::new("Generating…"),
                                );
                                ui.add_space(24.0); // align with button height
                            }
                        });
                        if i == 1 {
                            ui.add_space(8.0);
                        }
                    }
                });
            });
        });

        action
    }

    fn ui_animation(&self, ui: &mut Ui, _idx: usize) -> Action {
        let Phase::Animation { layout, cells, total, done } = &self.phase else {
            return Action::None;
        };

        let ready = cells.len();
        let mut action = Action::None;

        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.heading("Animation Frames");
                ui.add_space(12.0);
                ui.label(format!("{ready}/{total}"));
                if *done {
                    ui.colored_label(egui::Color32::from_rgb(80, 200, 80), "✓ Done");
                } else {
                    ui.add(egui::Spinner::new());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Save Atlas").clicked() {
                        action = Action::SaveAtlas;
                    }
                    if ui.button("← Back").clicked() {
                        action = Action::BackToIdle;
                    }
                });
            });
            ui.add_space(4.0);

            let layout = layout.clone();
            let cell_keys: Vec<(CellKey, Option<egui::TextureId>)> = layout
                .animations
                .iter()
                .flat_map(|slot| {
                    (0..layout.directions).flat_map(move |dir| {
                        (0..slot.frames).map(move |frame| {
                            let key = CellKey { anim: slot.name.clone(), dir, frame };
                            let tid = cells.get(&key).map(|c| c.texture.id());
                            (key, tid)
                        })
                    })
                })
                .collect();

            egui::ScrollArea::both().id_salt("anim_scroll").show(ui, |ui| {
                for slot in &layout.animations {
                    ui.strong(slot.name.as_str());
                    egui::Grid::new(format!("grid_{}", slot.name))
                        .spacing([4.0, 4.0])
                        .show(ui, |ui| {
                            // Header
                            ui.label("dir↓ / frame→");
                            for f in 0..slot.frames {
                                ui.label(format!("f{f}"));
                            }
                            ui.end_row();

                            for dir in 0..layout.directions {
                                ui.label(format!("d{dir}"));
                                for frame in 0..slot.frames {
                                    let key = CellKey { anim: slot.name.clone(), dir, frame };
                                    let tex_id = cell_keys
                                        .iter()
                                        .find(|(k, _)| *k == key)
                                        .and_then(|(_, t)| *t);

                                    ui.vertical(|ui| {
                                        if let Some(tid) = tex_id {
                                            ui.add(egui::Image::new((
                                                tid,
                                                egui::vec2(CELL_PX, CELL_PX),
                                            )));
                                            if ui.small_button("↺").on_hover_text("Re-roll this frame").clicked() {
                                                action = Action::RerollFrame(key);
                                            }
                                        } else {
                                            ui.add_sized(
                                                egui::vec2(CELL_PX, CELL_PX),
                                                egui::Label::new("…"),
                                            );
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                        });
                    ui.add_space(8.0);
                }
            });
        });

        action
    }

    fn apply_action(&mut self, action: Action, entry_idx: usize, _ctx: &Context) {
        match action {
            Action::None => {}
            Action::StartVariants   => self.start_variants(entry_idx),
            Action::PickVariant(i)  => self.start_animation(entry_idx, i),
            Action::RerollFrame(k)  => self.reroll_frame(entry_idx, k),
            Action::SaveAtlas       => self.save_atlas(entry_idx),
            Action::BackToIdle      => self.phase = Phase::Idle,
        }
    }
}

// ── Worker spawning ───────────────────────────────────────────────────────────

impl App {
    fn make_generator(&mut self) -> Option<Arc<dyn SpriteGenerator + Send + Sync>> {
        match self.backend_kind {
            BackendKind::Mock => Some(Arc::new(MockGenerator)),
            BackendKind::OpenAi => match OpenAiDalleGenerator::from_key(self.api_key.clone()) {
                Ok(g) => Some(Arc::new(g)),
                Err(e) => {
                    self.log.push(format!("[error] {e}"));
                    None
                }
            },
        }
    }

    fn start_variants(&mut self, entry_idx: usize) {
        let Some(gen) = self.make_generator() else { return; };
        let entry = self.entries[entry_idx].clone();
        let tx = self.result_tx.clone();

        self.phase = Phase::Variants { items: (0..VARIANT_COUNT).map(|_| None).collect(), done: false };
        self.log.push(format!("Generating {VARIANT_COUNT} variants for {}…", entry.display_name));

        std::thread::spawn(move || {
            for idx in 0..VARIANT_COUNT {
                // Use direction as variant axis: gives distinct mock colors;
                // for DALL-E each call is independent anyway.
                let req = FrameRequest {
                    entity_id: entry.id.as_str(),
                    animation: "idle",
                    direction: idx as u32,
                    frame: 0,
                    tile_size: TILE_SIZE,
                    base_prompt: entry.ai_prompt_base.as_str(),
                    style: entry.ai_style.as_str(),
                };
                match gen.generate(req) {
                    Ok(raw) => { tx.send(WorkResult::Variant { idx, raw }).ok(); }
                    Err(e)  => { tx.send(WorkResult::Error(format!("variant {idx}: {e}"))).ok(); }
                }
            }
        });
    }

    fn start_animation(&mut self, entry_idx: usize, picked: usize) {
        let Some(gen) = self.make_generator() else { return; };
        let entry = self.entries[entry_idx].clone();
        let layout = AtlasLayout::from_entry(&entry);
        let total = layout.animations.iter()
            .map(|s| layout.directions * s.frames)
            .sum::<u32>() as usize;
        let tx = self.result_tx.clone();

        self.log.push(format!(
            "Picked variant #{} — generating {} frames for {}…",
            picked + 1, total, entry.display_name
        ));
        self.phase = Phase::Animation {
            layout: layout.clone(),
            cells: HashMap::new(),
            total,
            done: false,
        };

        std::thread::spawn(move || {
            for slot in &layout.animations {
                for dir in 0..layout.directions {
                    for frame in 0..slot.frames {
                        let key = CellKey { anim: slot.name.clone(), dir, frame };
                        let req = FrameRequest {
                            entity_id: entry.id.as_str(),
                            animation: slot.name.as_str(),
                            direction: dir,
                            frame,
                            tile_size: TILE_SIZE,
                            base_prompt: entry.ai_prompt_base.as_str(),
                            style: entry.ai_style.as_str(),
                        };
                        match gen.generate(req) {
                            Ok(raw) => { tx.send(WorkResult::Frame { key, raw }).ok(); }
                            Err(e)  => { tx.send(WorkResult::Error(format!("{e}"))).ok(); }
                        }
                    }
                }
            }
        });
    }

    fn reroll_frame(&mut self, entry_idx: usize, key: CellKey) {
        let Some(gen) = self.make_generator() else { return; };
        let entry = self.entries[entry_idx].clone();
        let tx = self.result_tx.clone();
        let key_clone = key.clone();

        self.log.push(format!("Re-rolling {}/{}/{}…", key.anim, key.dir, key.frame));

        std::thread::spawn(move || {
            let req = FrameRequest {
                entity_id: entry.id.as_str(),
                animation: key_clone.anim.as_str(),
                direction: key_clone.dir,
                frame: key_clone.frame,
                tile_size: TILE_SIZE,
                base_prompt: entry.ai_prompt_base.as_str(),
                style: entry.ai_style.as_str(),
            };
            match gen.generate(req) {
                Ok(raw) => { tx.send(WorkResult::Frame { key: key_clone, raw }).ok(); }
                Err(e)  => { tx.send(WorkResult::Error(format!("{e}"))).ok(); }
            }
        });
    }

    fn save_atlas(&mut self, entry_idx: usize) {
        let entry = &self.entries[entry_idx];
        let layout = AtlasLayout::from_entry(entry);

        let Phase::Animation { cells, .. } = &self.phase else { return; };

        let mut atlas = RgbaImage::from_pixel(
            layout.image_width(),
            layout.image_height(),
            image::Rgba([0, 0, 0, 0]),
        );

        for slot in &layout.animations {
            for dir in 0..layout.directions {
                for frame in 0..slot.frames {
                    let key = CellKey { anim: slot.name.clone(), dir, frame };
                    if let Some(cell) = cells.get(&key) {
                        let (ox, oy) = layout.cell_origin(slot.row_start + dir, frame);
                        blit(&mut atlas, &cell.raw, ox, oy);
                    }
                }
            }
        }

        let out = PathBuf::from(&self.output_dir);
        if let Err(e) = std::fs::create_dir_all(&out) {
            self.log.push(format!("[error] mkdir: {e}"));
            return;
        }

        let png = out.join(format!("{}.png", entry.id));
        let ron = out.join(format!("{}.ron", entry.id));

        if let Err(e) = atlas.save(&png) {
            self.log.push(format!("[error] save PNG: {e}"));
            return;
        }

        let manifest = AtlasManifest::from(&layout);
        match to_ron(&manifest) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&ron, text) {
                    self.log.push(format!("[error] save RON: {e}"));
                    return;
                }
            }
            Err(e) => {
                self.log.push(format!("[error] RON serialize: {e}"));
                return;
            }
        }

        self.log.push(format!("Saved → {}", png.display()));
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn img_to_tex(ctx: &Context, name: &str, img: &RgbaImage) -> TextureHandle {
    let size = [img.width() as usize, img.height() as usize];
    let color_img = ColorImage::from_rgba_unmultiplied(size, img.as_flat_samples().as_slice());
    ctx.load_texture(name, color_img, egui::TextureOptions::LINEAR)
}

fn blit(dst: &mut RgbaImage, src: &RgbaImage, ox: u32, oy: u32) {
    for y in 0..src.height() {
        for x in 0..src.width() {
            dst.put_pixel(ox + x, oy + y, *src.get_pixel(x, y));
        }
    }
}

fn default_bestiary_path() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/bestiary.toml")
}
