//! eframe App — all egui panels and tray event handling.

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use eframe::egui::{self, Color32, RichText, ScrollArea, Ui, ViewportCommand};
use tray_icon::{TrayIcon, TrayIconEvent};
use tray_icon::menu::MenuEvent;

use crate::state::WorldSnapshot;

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Overview,
    Factions,
    Ecology,
    Story,
    Query,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Factions => "Factions",
            Tab::Ecology  => "Ecology",
            Tab::Story    => "Story",
            Tab::Query    => "Query",
        }
    }
}

pub struct WorldWatchApp {
    snapshot: Arc<Mutex<WorldSnapshot>>,
    // Keep the tray icon alive for the process lifetime.
    _tray: TrayIcon,
    visible: bool,
    active_tab: Tab,
    // Freeform BRP query.
    query_input: String,
    query_result: String,
    query_tx: mpsc::Sender<String>,
    result_rx: mpsc::Receiver<String>,
    // IDs for tray menu items.
    show_hide_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

impl WorldWatchApp {
    pub fn new(
        snapshot: Arc<Mutex<WorldSnapshot>>,
        tray: TrayIcon,
        query_tx: mpsc::Sender<String>,
        result_rx: mpsc::Receiver<String>,
        show_hide_id: tray_icon::menu::MenuId,
        quit_id: tray_icon::menu::MenuId,
    ) -> Self {
        Self {
            snapshot,
            _tray: tray,
            visible: false,
            active_tab: Tab::Overview,
            query_input: String::new(),
            query_result: String::new(),
            query_tx,
            result_rx,
            show_hide_id,
            quit_id,
        }
    }
}

impl eframe::App for WorldWatchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain any incoming query results.
        if let Ok(result) = self.result_rx.try_recv() {
            self.query_result = result;
        }

        // Poll tray icon and menu events.
        self.poll_tray_events(ctx);

        // Request repaint every 2 s to stay in sync with the polling interval.
        ctx.request_repaint_after(Duration::from_secs(2));

        if !self.visible {
            return;
        }

        let snap = self.snapshot.lock().unwrap().clone();

        egui::TopBottomPanel::top("status_bar").show(ctx, |ui| {
            self.render_status_bar(ui, &snap);
        });

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for tab in [Tab::Overview, Tab::Factions, Tab::Ecology, Tab::Story, Tab::Query] {
                    if ui.selectable_label(self.active_tab == tab, tab.label()).clicked() {
                        self.active_tab = tab;
                    }
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                Tab::Overview => render_overview(ui, &snap),
                Tab::Factions => render_factions(ui, &snap),
                Tab::Ecology  => render_ecology(ui, &snap),
                Tab::Story    => render_story(ui, &snap),
                Tab::Query    => self.render_query(ui),
            }
        });
    }
}

// ── Tray event handling ───────────────────────────────────────────────────────

impl WorldWatchApp {
    fn poll_tray_events(&mut self, ctx: &egui::Context) {
        // Tray icon left/right clicks toggle visibility.
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { .. } = event {
                self.set_visible(!self.visible, ctx);
            }
        }

        // Menu item selections.
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.show_hide_id {
                self.set_visible(!self.visible, ctx);
            } else if event.id == self.quit_id {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    fn set_visible(&mut self, visible: bool, ctx: &egui::Context) {
        self.visible = visible;
        ctx.send_viewport_cmd(ViewportCommand::Visible(visible));
    }
}

// ── Panel renderers ───────────────────────────────────────────────────────────

impl WorldWatchApp {
    fn render_status_bar(&self, ui: &mut Ui, snap: &WorldSnapshot) {
        ui.horizontal(|ui| {
            let (color, label) = if snap.server_online {
                (Color32::from_rgb(80, 200, 80), "ONLINE")
            } else {
                (Color32::from_rgb(220, 60, 60), "OFFLINE")
            };
            ui.colored_label(color, format!("Server: {label}"));
            if snap.server_online {
                ui.separator();
                ui.label(format!("Tick {}", snap.overview.world_tick));
                ui.separator();
                ui.label(format!(
                    "{} entities ({} players / {} NPCs)",
                    snap.overview.total_entities,
                    snap.overview.player_count,
                    snap.overview.npc_count,
                ));
            }
        });
    }

    fn render_query(&mut self, ui: &mut Ui) {
        ui.label("Component type path:");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.query_input);
            if ui.button("Query").clicked() && !self.query_input.is_empty() {
                let _ = self.query_tx.send(self.query_input.clone());
                self.query_result = "(waiting…)".to_owned();
            }
        });
        ui.add_space(4.0);
        ui.separator();
        ScrollArea::vertical().id_salt("query_scroll").show(ui, |ui| {
            ui.monospace(&self.query_result);
        });
    }
}

fn render_overview(ui: &mut Ui, snap: &WorldSnapshot) {
    if !snap.server_online {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("Server offline").color(Color32::from_rgb(220, 60, 60)).size(24.0));
        });
        return;
    }
    let ov = &snap.overview;
    egui::Grid::new("overview_grid")
        .num_columns(2)
        .spacing([40.0, 8.0])
        .show(ui, |ui| {
            ui.label("World tick");    ui.label(ov.world_tick.to_string());    ui.end_row();
            ui.label("Total entities"); ui.label(ov.total_entities.to_string()); ui.end_row();
            ui.label("Players");       ui.label(ov.player_count.to_string());   ui.end_row();
            ui.label("NPCs");          ui.label(ov.npc_count.to_string());       ui.end_row();
        });
}

fn render_factions(ui: &mut Ui, snap: &WorldSnapshot) {
    if snap.factions.is_empty() {
        ui.label("No faction data — server may not have flushed to SQLite yet.");
        return;
    }
    egui::Grid::new("factions_grid")
        .num_columns(6)
        .striped(true)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.strong("Faction");
            ui.strong("Food");
            ui.strong("Gold");
            ui.strong("Military");
            ui.strong("Top Goal");
            ui.end_row();

            for f in &snap.factions {
                ui.label(&f.name);
                ui.label(format!("{:.0}", f.food));
                ui.label(format!("{:.0}", f.gold));
                colored_stat(ui, f.military, 50.0);
                ui.label(&f.top_goal);
                ui.end_row();
            }
        });
}

fn render_ecology(ui: &mut Ui, snap: &WorldSnapshot) {
    if snap.ecology.is_empty() {
        ui.label("No ecology data — server flushes every 30 s.");
        return;
    }
    egui::Grid::new("ecology_grid")
        .num_columns(4)
        .striped(true)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.strong("Region");
            ui.strong("Prey");
            ui.strong("Predator");
            ui.strong("Status");
            ui.end_row();

            for r in &snap.ecology {
                ui.label(&r.region_id);

                let prey_color = if r.prey_collapsed {
                    Color32::from_rgb(220, 60, 60)
                } else {
                    Color32::from_rgb(80, 200, 80)
                };
                ui.colored_label(prey_color, format!("{} ({})", r.prey_count, r.prey_species));

                let pred_color = if r.predator_collapsed {
                    Color32::from_rgb(220, 60, 60)
                } else {
                    Color32::from_rgb(200, 140, 40)
                };
                ui.colored_label(pred_color, format!("{} ({})", r.predator_count, r.predator_species));

                let status = match (r.prey_collapsed, r.predator_collapsed) {
                    (true, true)   => RichText::new("Both collapsed").color(Color32::from_rgb(220, 60, 60)),
                    (true, false)  => RichText::new("Prey collapsed").color(Color32::from_rgb(220, 120, 40)),
                    (false, true)  => RichText::new("Pred. collapsed").color(Color32::from_rgb(220, 120, 40)),
                    (false, false) => RichText::new("Stable").color(Color32::from_rgb(80, 200, 80)),
                };
                ui.label(status);
                ui.end_row();
            }
        });
}

fn render_story(ui: &mut Ui, snap: &WorldSnapshot) {
    if snap.story.is_empty() {
        ui.label("No story events yet — server flushes every 300 ticks (~5 min).");
        return;
    }
    ScrollArea::vertical().id_salt("story_scroll").show(ui, |ui| {
        egui::Grid::new("story_grid")
            .num_columns(3)
            .striped(true)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.strong("Day");
                ui.strong("Event");
                ui.strong("Tags");
                ui.end_row();

                for ev in &snap.story {
                    ui.label(format!("D{} T{}", ev.world_day, ev.tick));
                    ui.label(&ev.kind);
                    ui.label(&ev.lore_tags);
                    ui.end_row();
                }
            });
    });
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Render a stat value colored green/yellow/red based on a `high` threshold.
fn colored_stat(ui: &mut Ui, value: f32, high: f32) {
    let color = if value >= high {
        Color32::from_rgb(80, 200, 80)
    } else if value >= high * 0.4 {
        Color32::from_rgb(220, 180, 40)
    } else {
        Color32::from_rgb(220, 60, 60)
    };
    ui.colored_label(color, format!("{:.0}", value));
}
