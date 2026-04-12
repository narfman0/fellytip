//! DM (Dungeon Master) tab — live world manipulation via custom BRP methods.
//!
//! Provides a form-based UI for spawning entities, adjusting faction and
//! ecology state, triggering war parties, killing/teleporting entities, and
//! a command-line input for power users.

use std::collections::VecDeque;
use std::sync::mpsc;

use eframe::egui::{self, Color32, RichText, ScrollArea, TextEdit, Ui};
use serde_json::{Value, json};

// ── Sub-tab ───────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Default)]
pub enum DmSubTab {
    #[default]
    SpawnNpc,
    Faction,
    Ecology,
    Entity,
    WarParty,
}

impl DmSubTab {
    fn label(self) -> &'static str {
        match self {
            DmSubTab::SpawnNpc  => "Spawn NPC",
            DmSubTab::Faction   => "Faction",
            DmSubTab::Ecology   => "Ecology",
            DmSubTab::Entity    => "Entity",
            DmSubTab::WarParty  => "War Party",
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct DmTab {
    pub active_sub: DmSubTab,

    // Spawn NPC
    pub spawn_faction: String,
    pub spawn_x: String,
    pub spawn_y: String,
    pub spawn_z: String,
    pub spawn_level: String,

    // Faction
    pub faction_id: String,
    pub faction_food: String,
    pub faction_gold: String,
    pub faction_military: String,

    // Ecology
    pub eco_region: String,
    pub eco_prey: String,
    pub eco_predator: String,

    // Entity
    pub target_entity: String,
    pub teleport_x: String,
    pub teleport_y: String,
    pub teleport_z: String,

    // War Party
    pub war_attacker: String,
    pub war_target: String,

    // Command line
    pub cmdline: String,

    // Result log (newest first, capped at 50 entries)
    pub results: VecDeque<String>,

    pub dm_tx: mpsc::Sender<(String, Value)>,
}

impl DmTab {
    pub fn new(dm_tx: mpsc::Sender<(String, Value)>) -> Self {
        Self {
            active_sub: DmSubTab::default(),
            spawn_faction: "iron_wolves".to_owned(),
            spawn_x: "0".to_owned(),
            spawn_y: "0".to_owned(),
            spawn_z: "0".to_owned(),
            spawn_level: "1".to_owned(),
            faction_id: "iron_wolves".to_owned(),
            faction_food: "".to_owned(),
            faction_gold: "".to_owned(),
            faction_military: "".to_owned(),
            eco_region: "macro_0_0".to_owned(),
            eco_prey: "".to_owned(),
            eco_predator: "".to_owned(),
            target_entity: "".to_owned(),
            teleport_x: "0".to_owned(),
            teleport_y: "0".to_owned(),
            teleport_z: "0".to_owned(),
            war_attacker: "iron_wolves".to_owned(),
            war_target: "ash_covenant".to_owned(),
            cmdline: String::new(),
            results: VecDeque::new(),
            dm_tx,
        }
    }

    /// Push a result string (newest first).
    fn push_result(&mut self, msg: String) {
        self.results.push_front(msg);
        self.results.truncate(50);
    }

    /// Send a DM command and record the pending status.
    fn send(&mut self, method: &str, params: Value) {
        self.push_result(format!("→ {method} {params}"));
        let _ = self.dm_tx.send((method.to_owned(), params));
    }

    /// Drain and display any incoming results via `dm_result_rx`.
    pub fn drain_results(&mut self, dm_result_rx: &mpsc::Receiver<String>) {
        while let Ok(r) = dm_result_rx.try_recv() {
            self.push_result(r);
        }
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

pub fn render_dm(tab: &mut DmTab, ui: &mut Ui) {
    // Sub-tab bar
    ui.horizontal(|ui| {
        for sub in [
            DmSubTab::SpawnNpc,
            DmSubTab::Faction,
            DmSubTab::Ecology,
            DmSubTab::Entity,
            DmSubTab::WarParty,
        ] {
            if ui.selectable_label(tab.active_sub == sub, sub.label()).clicked() {
                tab.active_sub = sub;
            }
        }
    });
    ui.separator();

    // Main area: left = form, right = result log.
    egui::SidePanel::right("dm_results")
        .resizable(true)
        .default_width(320.0)
        .show_inside(ui, |ui| {
            ui.strong("Result log");
            ui.separator();
            ScrollArea::vertical().id_salt("dm_results_scroll").show(ui, |ui| {
                for entry in &tab.results {
                    let color = if entry.starts_with('✓') {
                        Color32::from_rgb(80, 200, 80)
                    } else if entry.starts_with('✗') {
                        Color32::from_rgb(220, 60, 60)
                    } else {
                        Color32::GRAY
                    };
                    ui.colored_label(color, entry.as_str());
                }
            });
        });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        match tab.active_sub {
            DmSubTab::SpawnNpc => render_spawn_npc(tab, ui),
            DmSubTab::Faction  => render_faction(tab, ui),
            DmSubTab::Ecology  => render_ecology(tab, ui),
            DmSubTab::Entity   => render_entity(tab, ui),
            DmSubTab::WarParty => render_war_party(tab, ui),
        }

        ui.separator();
        render_cmdline(tab, ui);
    });
}

// ── Sub-panel renderers ───────────────────────────────────────────────────────

fn render_spawn_npc(tab: &mut DmTab, ui: &mut Ui) {
    ui.strong("Spawn NPC");
    ui.add_space(4.0);
    egui::Grid::new("dm_spawn_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Faction");
            ui.add(TextEdit::singleline(&mut tab.spawn_faction).hint_text("iron_wolves"));
            ui.end_row();
            ui.label("X");
            ui.add(TextEdit::singleline(&mut tab.spawn_x).hint_text("0"));
            ui.end_row();
            ui.label("Y");
            ui.add(TextEdit::singleline(&mut tab.spawn_y).hint_text("0"));
            ui.end_row();
            ui.label("Z");
            ui.add(TextEdit::singleline(&mut tab.spawn_z).hint_text("0"));
            ui.end_row();
            ui.label("Level");
            ui.add(TextEdit::singleline(&mut tab.spawn_level).hint_text("1"));
            ui.end_row();
        });
    ui.add_space(8.0);
    if ui.button("Spawn NPC").clicked() {
        if let (Ok(x), Ok(y), Ok(z)) = (
            tab.spawn_x.parse::<f32>(),
            tab.spawn_y.parse::<f32>(),
            tab.spawn_z.parse::<f32>(),
        ) {
            let level = tab.spawn_level.parse::<u32>().unwrap_or(1);
            tab.send("dm/spawn_npc", json!({
                "faction": tab.spawn_faction,
                "x": x, "y": y, "z": z,
                "level": level,
            }));
        } else {
            tab.push_result("✗ invalid x/y/z — must be numbers".to_owned());
        }
    }
}

fn render_faction(tab: &mut DmTab, ui: &mut Ui) {
    ui.strong("Set Faction Resources");
    ui.label(RichText::new("Leave a field blank to leave it unchanged.").color(Color32::GRAY).small());
    ui.add_space(4.0);
    egui::Grid::new("dm_faction_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Faction ID");
            ui.add(TextEdit::singleline(&mut tab.faction_id).hint_text("iron_wolves"));
            ui.end_row();
            ui.label("Food");
            ui.add(TextEdit::singleline(&mut tab.faction_food).hint_text("leave blank to skip"));
            ui.end_row();
            ui.label("Gold");
            ui.add(TextEdit::singleline(&mut tab.faction_gold).hint_text("leave blank to skip"));
            ui.end_row();
            ui.label("Military");
            ui.add(TextEdit::singleline(&mut tab.faction_military).hint_text("leave blank to skip"));
            ui.end_row();
        });
    ui.add_space(8.0);
    if ui.button("Apply").clicked() {
        let mut params = json!({ "faction_id": tab.faction_id });
        if let Ok(v) = tab.faction_food.parse::<f32>() {
            params["food"] = json!(v);
        }
        if let Ok(v) = tab.faction_gold.parse::<f32>() {
            params["gold"] = json!(v);
        }
        if let Ok(v) = tab.faction_military.parse::<f32>() {
            params["military"] = json!(v);
        }
        tab.send("dm/set_faction", params);
    }
}

fn render_ecology(tab: &mut DmTab, ui: &mut Ui) {
    ui.strong("Set Ecology Population");
    ui.label(
        RichText::new("Region IDs: macro_0_0 … macro_3_3 (4×4 grid). Leave blank to skip.")
            .color(Color32::GRAY)
            .small(),
    );
    ui.add_space(4.0);
    egui::Grid::new("dm_eco_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Region");
            ui.add(TextEdit::singleline(&mut tab.eco_region).hint_text("macro_0_0"));
            ui.end_row();
            ui.label("Prey count");
            ui.add(TextEdit::singleline(&mut tab.eco_prey).hint_text("leave blank to skip"));
            ui.end_row();
            ui.label("Predator count");
            ui.add(TextEdit::singleline(&mut tab.eco_predator).hint_text("leave blank to skip"));
            ui.end_row();
        });
    ui.add_space(8.0);
    if ui.button("Apply").clicked() {
        let mut params = json!({ "region": tab.eco_region });
        if let Ok(v) = tab.eco_prey.parse::<f64>() {
            params["prey"] = json!(v);
        }
        if let Ok(v) = tab.eco_predator.parse::<f64>() {
            params["predator"] = json!(v);
        }
        tab.send("dm/set_ecology", params);
    }
}

fn render_entity(tab: &mut DmTab, ui: &mut Ui) {
    ui.strong("Entity Control");
    ui.label(
        RichText::new("Entity ID is the `entity` field from the Query tab.")
            .color(Color32::GRAY)
            .small(),
    );
    ui.add_space(4.0);
    egui::Grid::new("dm_entity_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Entity ID");
            ui.add(TextEdit::singleline(&mut tab.target_entity).hint_text("12345678"));
            ui.end_row();
        });

    ui.add_space(4.0);
    if ui.button("Kill entity").clicked() {
        if let Ok(id) = tab.target_entity.parse::<u64>() {
            tab.send("dm/kill", json!({ "entity": id }));
        } else {
            tab.push_result("✗ invalid entity ID — must be a u64".to_owned());
        }
    }

    ui.add_space(8.0);
    ui.separator();
    ui.label("Teleport to:");
    egui::Grid::new("dm_teleport_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("X");
            ui.add(TextEdit::singleline(&mut tab.teleport_x).hint_text("0"));
            ui.end_row();
            ui.label("Y");
            ui.add(TextEdit::singleline(&mut tab.teleport_y).hint_text("0"));
            ui.end_row();
            ui.label("Z");
            ui.add(TextEdit::singleline(&mut tab.teleport_z).hint_text("0"));
            ui.end_row();
        });
    ui.add_space(4.0);
    if ui.button("Teleport entity").clicked() {
        if let (Ok(id), Ok(x), Ok(y), Ok(z)) = (
            tab.target_entity.parse::<u64>(),
            tab.teleport_x.parse::<f32>(),
            tab.teleport_y.parse::<f32>(),
            tab.teleport_z.parse::<f32>(),
        ) {
            tab.send("dm/teleport", json!({ "entity": id, "x": x, "y": y, "z": z }));
        } else {
            tab.push_result("✗ invalid entity ID or x/y/z".to_owned());
        }
    }
}

fn render_war_party(tab: &mut DmTab, ui: &mut Ui) {
    ui.strong("Trigger War Party");
    ui.label(
        RichText::new("Tags up to 10 adult NPCs of the attacker faction as a war party.")
            .color(Color32::GRAY)
            .small(),
    );
    ui.add_space(4.0);
    egui::Grid::new("dm_war_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Attacker faction");
            ui.add(TextEdit::singleline(&mut tab.war_attacker).hint_text("iron_wolves"));
            ui.end_row();
            ui.label("Target faction");
            ui.add(TextEdit::singleline(&mut tab.war_target).hint_text("ash_covenant"));
            ui.end_row();
        });
    ui.add_space(8.0);
    if ui.button("Trigger War Party").clicked() {
        tab.send("dm/trigger_war_party", json!({
            "attacker_faction": tab.war_attacker,
            "target_faction":   tab.war_target,
        }));
    }
}

// ── Command line ──────────────────────────────────────────────────────────────

fn render_cmdline(tab: &mut DmTab, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(">");
        let response = ui.add(
            TextEdit::singleline(&mut tab.cmdline)
                .hint_text("/spawn npc iron_wolves 0 0 0  |  /kill <id>  |  /teleport <id> x y z  |  /faction <id> food=N  |  /war <atk> <tgt>  |  /ecology <region> prey=N pred=N")
                .desired_width(f32::INFINITY),
        );
        let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if submitted || ui.button("Run").clicked() {
            let cmd = std::mem::take(&mut tab.cmdline);
            if let Some((method, params)) = parse_cmdline(&cmd) {
                tab.send(&method, params);
            } else {
                tab.push_result(format!("✗ unrecognised command: {cmd}"));
            }
        }
    });
}

/// Parse a slash-command string into a `(method, params)` pair.
///
/// Supported syntax:
/// - `/spawn npc <faction> <x> <y> <z> [level=N]`
/// - `/kill <entity_id>`
/// - `/teleport <entity_id> <x> <y> <z>`
/// - `/faction <id> [food=N] [gold=N] [military=N]`
/// - `/war <attacker> <target>`
/// - `/ecology <region> [prey=N] [pred=N]`
pub fn parse_cmdline(input: &str) -> Option<(String, Value)> {
    let input = input.trim();
    if !input.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = input[1..].split_whitespace().collect();
    match parts.as_slice() {
        // /spawn npc <faction> <x> <y> <z> [level=N]
        ["spawn", "npc", faction, x, y, z, rest @ ..] => {
            let x: f32 = x.parse().ok()?;
            let y: f32 = y.parse().ok()?;
            let z: f32 = z.parse().ok()?;
            let level = rest.iter()
                .find_map(|s| s.strip_prefix("level=")?.parse::<u32>().ok())
                .unwrap_or(1);
            Some(("dm/spawn_npc".to_owned(), json!({
                "faction": faction, "x": x, "y": y, "z": z, "level": level
            })))
        }
        // /kill <entity>
        ["kill", id] => {
            let id: u64 = id.parse().ok()?;
            Some(("dm/kill".to_owned(), json!({ "entity": id })))
        }
        // /teleport <entity> <x> <y> <z>
        ["teleport", id, x, y, z] => {
            let id: u64 = id.parse().ok()?;
            let x: f32 = x.parse().ok()?;
            let y: f32 = y.parse().ok()?;
            let z: f32 = z.parse().ok()?;
            Some(("dm/teleport".to_owned(), json!({ "entity": id, "x": x, "y": y, "z": z })))
        }
        // /faction <id> [food=N] [gold=N] [military=N]
        ["faction", faction_id, kv @ ..] => {
            let mut params = json!({ "faction_id": faction_id });
            for item in kv {
                if let Some(v) = item.strip_prefix("food=").and_then(|s| s.parse::<f32>().ok()) {
                    params["food"] = json!(v);
                } else if let Some(v) = item.strip_prefix("gold=").and_then(|s| s.parse::<f32>().ok()) {
                    params["gold"] = json!(v);
                } else if let Some(v) = item.strip_prefix("military=").and_then(|s| s.parse::<f32>().ok()) {
                    params["military"] = json!(v);
                }
            }
            Some(("dm/set_faction".to_owned(), params))
        }
        // /war <attacker> <target>
        ["war", attacker, target] => {
            Some(("dm/trigger_war_party".to_owned(), json!({
                "attacker_faction": attacker,
                "target_faction":   target,
            })))
        }
        // /ecology <region> [prey=N] [pred=N]
        ["ecology", region, kv @ ..] => {
            let mut params = json!({ "region": region });
            for item in kv {
                if let Some(v) = item.strip_prefix("prey=").and_then(|s| s.parse::<f64>().ok()) {
                    params["prey"] = json!(v);
                } else if let Some(v) = item.strip_prefix("pred=").and_then(|s| s.parse::<f64>().ok()) {
                    params["predator"] = json!(v);
                }
            }
            Some(("dm/set_ecology".to_owned(), params))
        }
        _ => None,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_spawn_npc() {
        let (m, p) = parse_cmdline("/spawn npc iron_wolves 10 20 5").unwrap();
        assert_eq!(m, "dm/spawn_npc");
        assert_eq!(p["faction"], "iron_wolves");
        assert_eq!(p["x"].as_f64().unwrap(), 10.0);
        assert_eq!(p["level"], 1);
    }

    #[test]
    fn parse_spawn_npc_with_level() {
        let (m, p) = parse_cmdline("/spawn npc ash_covenant 0 0 0 level=3").unwrap();
        assert_eq!(m, "dm/spawn_npc");
        assert_eq!(p["level"], 3);
    }

    #[test]
    fn parse_kill() {
        let (m, p) = parse_cmdline("/kill 99999").unwrap();
        assert_eq!(m, "dm/kill");
        assert_eq!(p["entity"], 99999u64);
    }

    #[test]
    fn parse_teleport() {
        let (m, p) = parse_cmdline("/teleport 42 1.5 2.5 3.5").unwrap();
        assert_eq!(m, "dm/teleport");
        assert_eq!(p["entity"], 42u64);
        assert!((p["x"].as_f64().unwrap() - 1.5).abs() < 0.01);
    }

    #[test]
    fn parse_faction() {
        let (m, p) = parse_cmdline("/faction iron_wolves food=80 military=30").unwrap();
        assert_eq!(m, "dm/set_faction");
        assert_eq!(p["faction_id"], "iron_wolves");
        assert_eq!(p["food"].as_f64().unwrap() as i32, 80);
        assert_eq!(p["military"].as_f64().unwrap() as i32, 30);
        assert!(p["gold"].is_null());
    }

    #[test]
    fn parse_war() {
        let (m, p) = parse_cmdline("/war iron_wolves ash_covenant").unwrap();
        assert_eq!(m, "dm/trigger_war_party");
        assert_eq!(p["attacker_faction"], "iron_wolves");
        assert_eq!(p["target_faction"],   "ash_covenant");
    }

    #[test]
    fn parse_ecology() {
        let (m, p) = parse_cmdline("/ecology macro_0_0 prey=200 pred=40").unwrap();
        assert_eq!(m, "dm/set_ecology");
        assert_eq!(p["region"], "macro_0_0");
        assert_eq!(p["prey"].as_f64().unwrap() as i32, 200);
        assert_eq!(p["predator"].as_f64().unwrap() as i32, 40);
    }

    #[test]
    fn parse_unrecognised() {
        assert!(parse_cmdline("/bogus whatever").is_none());
        assert!(parse_cmdline("no slash").is_none());
    }
}
