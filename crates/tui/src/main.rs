//! fellytip-tui — terminal UI client for testing game systems via BRP HTTP.
//!
//! Phase 1: Scaffold + connection
//! Phase 2: Layout + movement
//! Phase 3: Command palette + interactions
//! Phase 4: Polish

mod brp;

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use brp::BrpClient;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use serde_json::Value;
use tokio::sync::mpsc;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EntityInfo {
    pub entity_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub hp: Option<f32>,
    pub hp_max: Option<f32>,
    pub kind: EntityKind,
    pub distance: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntityKind {
    Player,
    Bot,
    Npc,
    Wildlife,
}

impl EntityKind {
    pub fn glyph(&self) -> char {
        match self {
            EntityKind::Player => '@',
            EntityKind::Bot => 'b',
            EntityKind::Npc => 'n',
            EntityKind::Wildlife => 'W',
        }
    }
    pub fn color(&self) -> Color {
        match self {
            EntityKind::Player => Color::Cyan,
            EntityKind::Bot => Color::Blue,
            EntityKind::Npc => Color::Yellow,
            EntityKind::Wildlife => Color::Green,
        }
    }
}

#[derive(Clone, Default)]
pub struct PlayerStats {
    pub hp: f32,
    pub hp_max: f32,
    pub xp: f32,
    pub xp_next: f32,
    pub level: u32,
    pub class: String,
    pub gold: f32,
}

#[derive(Clone)]
pub struct AppState {
    pub server_online: bool,
    pub has_player_position_method: bool,
    pub player_pos: Option<(f32, f32, f32)>,
    pub player_entity: Option<u64>,
    pub player_stats: PlayerStats,
    pub nearby_entities: Vec<EntityInfo>,
    pub event_log: VecDeque<String>,
    pub battle_history_raw: Vec<Value>,
    pub underground_pressure: Option<f32>,
    pub settlements: Vec<Value>,
    pub bots: Vec<Value>,
    pub last_hp: Option<f32>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            server_online: false,
            has_player_position_method: false,
            player_pos: None,
            player_entity: None,
            player_stats: PlayerStats::default(),
            nearby_entities: Vec::new(),
            event_log: VecDeque::with_capacity(200),
            battle_history_raw: Vec::new(),
            underground_pressure: None,
            settlements: Vec::new(),
            bots: Vec::new(),
            last_hp: None,
        }
    }
}

impl AppState {
    pub fn push_log(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if self.event_log.len() >= 200 {
            self.event_log.pop_front();
        }
        self.event_log.push_back(msg);
    }
}

#[derive(PartialEq)]
enum InputMode {
    Normal,
    Command,
}

enum Overlay {
    None,
    Help,
    Inspect(u64, Value),
    RawBrp(String),
}

// ── Commands sent from input handler to poll loop ─────────────────────────────

enum AppCmd {
    Teleport(f32, f32),
    TeleportEntity(u64, f32, f32, f32),
    ChooseClass(String),
    Kill(u64),
    SpawnBot,
    BrpRaw(String),
    Inspect(u64),
    ListSettlements,
    ListBots,
    SpawnNpc,
}

// ── Background poll task ──────────────────────────────────────────────────────

async fn poll_task(
    state: Arc<Mutex<AppState>>,
    mut cmd_rx: mpsc::Receiver<AppCmd>,
    result_tx: mpsc::Sender<String>,
) {
    let brp = BrpClient::new();

    // Capability probe
    let has_player_pos = brp.probe_method("dm/player_position").await;
    {
        let mut s = state.lock().unwrap();
        s.has_player_position_method = has_player_pos;
        s.push_log(if has_player_pos {
            "Server: client mode (dm/player_position available)".to_string()
        } else {
            "Server: standalone mode".to_string()
        });
    }

    let mut t_player_pos = Instant::now();
    let mut t_stats      = Instant::now();
    let mut t_entities   = Instant::now();
    let mut t_battle     = Instant::now();
    let mut t_ping       = Instant::now();

    loop {
        // Handle commands (non-blocking drain)
        while let Ok(cmd) = cmd_rx.try_recv() {
            let msg = handle_cmd(&brp, &state, cmd).await;
            let _ = result_tx.send(msg).await;
        }

        let now = Instant::now();

        // 200ms: player position
        if now.duration_since(t_player_pos) >= Duration::from_millis(200) {
            t_player_pos = now;
            poll_player_pos(&brp, &state).await;
        }

        // 500ms: player stats
        if now.duration_since(t_stats) >= Duration::from_millis(500) {
            t_stats = now;
            poll_player_stats(&brp, &state).await;
        }

        // 1000ms: nearby entities
        if now.duration_since(t_entities) >= Duration::from_millis(1000) {
            t_entities = now;
            poll_entities(&brp, &state).await;
        }

        // 2000ms: battle history
        if now.duration_since(t_battle) >= Duration::from_millis(2000) {
            t_battle = now;
            poll_battle_history(&brp, &state).await;
        }

        // 3000ms: ping
        if now.duration_since(t_ping) >= Duration::from_millis(3000) {
            t_ping = now;
            let online = brp.ping().await;
            let was_online = state.lock().unwrap().server_online;
            if online != was_online {
                let msg = if online { "Server reconnected" } else { "Server offline" }.to_string();
                state.lock().unwrap().push_log(msg);
            }
            state.lock().unwrap().server_online = online;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn poll_player_pos(brp: &BrpClient, state: &Arc<Mutex<AppState>>) {
    let has_method = state.lock().unwrap().has_player_position_method;
    if has_method {
        if let Ok(v) = brp.call("dm/player_position", serde_json::json!({})).await {
            if let (Some(x), Some(y), Some(z)) = (
                v.get("x").and_then(|v| v.as_f64()),
                v.get("y").and_then(|v| v.as_f64()),
                v.get("z").and_then(|v| v.as_f64()),
            ) {
                let mut s = state.lock().unwrap();
                s.server_online = true;
                s.player_pos = Some((x as f32, y as f32, z as f32));
            }
        }
    } else {
        // Standalone: get position from world.query for entity with Experience
        if let Ok(results) = brp.query(&[
            "fellytip_shared::components::WorldPosition",
            "fellytip_shared::components::Experience",
        ]).await {
            if let Some(first) = results.first() {
                let entity_id = first.get("entity").and_then(|e| e.as_u64());
                let pos = first.get("components")
                    .and_then(|c| c.get("fellytip_shared::components::WorldPosition"));
                if let Some(pos) = pos {
                    let x = pos.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let y = pos.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let z = pos.get("z").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let mut s = state.lock().unwrap();
                    s.server_online = true;
                    s.player_pos = Some((x, y, z));
                    if let Some(eid) = entity_id {
                        s.player_entity = Some(eid);
                    }
                }
            }
        }
    }
}

async fn poll_player_stats(brp: &BrpClient, state: &Arc<Mutex<AppState>>) {
    let results = brp.query(&[
        "fellytip_shared::components::WorldPosition",
        "fellytip_shared::components::Experience",
    ]).await.unwrap_or_default();

    if let Some(first) = results.first() {
        let entity_id = first.get("entity").and_then(|e| e.as_u64());
        let comps = first.get("components");

        let xp_comp = comps.and_then(|c| c.get("fellytip_shared::components::Experience"));
        let xp = xp_comp.and_then(|e| e.get("current_xp")).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let xp_next = xp_comp.and_then(|e| e.get("xp_to_next_level")).and_then(|v| v.as_f64()).unwrap_or(100.0) as f32;
        let level = xp_comp.and_then(|e| e.get("level")).and_then(|v| v.as_u64()).unwrap_or(1) as u32;
        let class = xp_comp.and_then(|e| e.get("class")).and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();

        let mut s = state.lock().unwrap();
        s.player_stats.xp = xp;
        s.player_stats.xp_next = if xp_next > 0.0 { xp_next } else { 100.0 };
        s.player_stats.level = level;
        s.player_stats.class = class;
        if let Some(eid) = entity_id {
            s.player_entity = Some(eid);
        }
    }

    // Try to get HP from Health component
    let hp_results = brp.query(&[
        "fellytip_shared::components::Health",
        "fellytip_shared::components::Experience",
    ]).await.unwrap_or_default();

    if let Some(first) = hp_results.first() {
        let comps = first.get("components");
        let hp_comp = comps.and_then(|c| c.get("fellytip_shared::components::Health"));
        if let Some(hp_comp) = hp_comp {
            let hp = hp_comp.get("current").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let hp_max = hp_comp.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0) as f32;

            let mut s = state.lock().unwrap();
            // HP delta tracking
            if let Some(last_hp) = s.last_hp {
                let delta = hp - last_hp;
                if delta.abs() > 0.01 {
                    let msg = if delta < 0.0 {
                        format!("HP -{:.0} ({:.0}/{:.0})", -delta, hp, hp_max)
                    } else {
                        format!("HP +{:.0} ({:.0}/{:.0})", delta, hp, hp_max)
                    };
                    s.push_log(msg);
                }
            }
            s.last_hp = Some(hp);
            s.player_stats.hp = hp;
            s.player_stats.hp_max = if hp_max > 0.0 { hp_max } else { 1.0 };
        }
    }
}

async fn poll_entities(brp: &BrpClient, state: &Arc<Mutex<AppState>>) {
    let results = brp.query(&["fellytip_shared::components::WorldPosition"]).await.unwrap_or_default();
    let exp_results = brp.query(&["fellytip_shared::components::Experience"]).await.unwrap_or_default();
    let bot_results = brp.query(&[
        "fellytip_shared::components::WorldPosition",
        "fellytip_game::plugins::bot::BotController",
    ]).await.unwrap_or_default();

    let player_entity_ids: std::collections::HashSet<u64> = exp_results.iter()
        .filter_map(|v| v.get("entity").and_then(|e| e.as_u64()))
        .collect();
    let bot_entity_ids: std::collections::HashSet<u64> = bot_results.iter()
        .filter_map(|v| v.get("entity").and_then(|e| e.as_u64()))
        .collect();

    let (player_pos, player_entity) = {
        let s = state.lock().unwrap();
        (s.player_pos, s.player_entity)
    };

    let mut entities: Vec<EntityInfo> = results.iter().filter_map(|v| {
        let eid = v.get("entity").and_then(|e| e.as_u64())?;
        let comps = v.get("components")?;
        let pos = comps.get("fellytip_shared::components::WorldPosition")?;
        let x = pos.get("x").and_then(|v| v.as_f64())? as f32;
        let y = pos.get("y").and_then(|v| v.as_f64())? as f32;
        let z = pos.get("z").and_then(|v| v.as_f64())? as f32;

        let kind = if player_entity_ids.contains(&eid) {
            EntityKind::Player
        } else if bot_entity_ids.contains(&eid) {
            EntityKind::Bot
        } else {
            EntityKind::Npc
        };

        let distance = if let Some((px, py, _)) = player_pos {
            ((x - px).powi(2) + (y - py).powi(2)).sqrt()
        } else {
            f32::MAX
        };

        // Skip the player itself from the list
        if Some(eid) == player_entity && kind == EntityKind::Player {
            return None;
        }

        Some(EntityInfo { entity_id: eid, x, y, z, hp: None, hp_max: None, kind, distance })
    }).collect();

    entities.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
    entities.truncate(50);

    let mut s = state.lock().unwrap();
    s.nearby_entities = entities;
}

async fn poll_battle_history(brp: &BrpClient, state: &Arc<Mutex<AppState>>) {
    if let Ok(v) = brp.call("dm/battle_history", serde_json::json!({})).await {
        if let Some(arr) = v.as_array() {
            let new_len = arr.len();
            let old_len = state.lock().unwrap().battle_history_raw.len();
            if new_len > old_len {
                // New entries
                for entry in &arr[old_len..] {
                    let msg = format_battle_entry(entry);
                    state.lock().unwrap().push_log(msg);
                }
            }
            state.lock().unwrap().battle_history_raw = arr.clone();
        }
    }
}

fn format_battle_entry(v: &Value) -> String {
    let tick = v.get("tick").and_then(|t| t.as_u64()).unwrap_or(0);
    let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("?");
    let attacker = v.get("attacker_name").and_then(|n| n.as_str()).unwrap_or("?");
    let target = v.get("target_name").and_then(|n| n.as_str()).unwrap_or("?");
    let dmg = v.get("damage").and_then(|d| d.as_f64()).unwrap_or(0.0);
    format!("[{}] {} {} {} for {:.0}", tick, attacker, kind, target, dmg)
}

async fn handle_cmd(brp: &BrpClient, state: &Arc<Mutex<AppState>>, cmd: AppCmd) -> String {
    match cmd {
        AppCmd::Teleport(x, y) => {
            let has_method = state.lock().unwrap().has_player_position_method;
            if has_method {
                match brp.call("dm/teleport_player", serde_json::json!({ "x": x, "y": y })).await {
                    Ok(_) => {
                        let mut s = state.lock().unwrap();
                        s.push_log(format!("Teleported to ({x:.1}, {y:.1})"));
                        "ok".to_string()
                    }
                    Err(e) => format!("teleport_player error: {e}"),
                }
            } else {
                let entity = state.lock().unwrap().player_entity;
                if let Some(eid) = entity {
                    let pos = state.lock().unwrap().player_pos;
                    let z = pos.map(|(_, _, z)| z).unwrap_or(0.0);
                    match brp.call("dm/teleport", serde_json::json!({ "entity": eid, "x": x, "y": y, "z": z })).await {
                        Ok(_) => {
                            let mut s = state.lock().unwrap();
                            s.push_log(format!("Teleported to ({x:.1}, {y:.1})"));
                            "ok".to_string()
                        }
                        Err(e) => format!("teleport error: {e}"),
                    }
                } else {
                    "no player entity known".to_string()
                }
            }
        }
        AppCmd::TeleportEntity(eid, x, y, z) => {
            match brp.call("dm/teleport", serde_json::json!({ "entity": eid, "x": x, "y": y, "z": z })).await {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("teleport error: {e}"),
            }
        }
        AppCmd::ChooseClass(class) => {
            match brp.call("dm/choose_class", serde_json::json!({ "class": class })).await {
                Ok(_) => {
                    state.lock().unwrap().push_log(format!("Class set to {class}"));
                    "ok".to_string()
                }
                Err(e) => format!("choose_class error: {e}"),
            }
        }
        AppCmd::Kill(eid) => {
            match brp.call("dm/kill", serde_json::json!({ "entity": eid })).await {
                Ok(_) => {
                    state.lock().unwrap().push_log(format!("Killed entity {eid}"));
                    "ok".to_string()
                }
                Err(e) => format!("kill error: {e}"),
            }
        }
        AppCmd::SpawnBot => {
            match brp.call("dm/spawn_bot", serde_json::json!({})).await {
                Ok(_) => {
                    state.lock().unwrap().push_log("Bot spawned".to_string());
                    "ok".to_string()
                }
                Err(e) => format!("spawn_bot error: {e}"),
            }
        }
        AppCmd::BrpRaw(raw) => {
            // Expect: METHOD {json}
            let parts: Vec<&str> = raw.splitn(2, ' ').collect();
            let method = parts[0];
            let params = if parts.len() > 1 {
                serde_json::from_str(parts[1]).unwrap_or(serde_json::json!({}))
            } else {
                serde_json::json!({})
            };
            match brp.call(method, params).await {
                Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|e| e.to_string()),
                Err(e) => format!("error: {e}"),
            }
        }
        AppCmd::Inspect(eid) => {
            let common = &[
                "fellytip_shared::components::WorldPosition",
                "fellytip_shared::components::Experience",
                "fellytip_shared::components::Health",
            ];
            match brp.get_components(eid, common).await {
                Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|e| e.to_string()),
                Err(e) => format!("inspect error: {e}"),
            }
        }
        AppCmd::ListSettlements => {
            match brp.call("dm/list_settlements", serde_json::json!({})).await {
                Ok(v) => {
                    if let Some(arr) = v.as_array() {
                        let mut s = state.lock().unwrap();
                        s.settlements = arr.clone();
                    }
                    "ok".to_string()
                }
                Err(e) => format!("list_settlements error: {e}"),
            }
        }
        AppCmd::ListBots => {
            match brp.call("dm/list_bots", serde_json::json!({})).await {
                Ok(v) => {
                    if let Some(arr) = v.as_array() {
                        let mut s = state.lock().unwrap();
                        s.bots = arr.clone();
                    }
                    "ok".to_string()
                }
                Err(e) => format!("list_bots error: {e}"),
            }
        }
        AppCmd::SpawnNpc => {
            match brp.call("dm/spawn_npc", serde_json::json!({})).await {
                Ok(_) => {
                    state.lock().unwrap().push_log("NPC spawned".to_string());
                    "ok".to_string()
                }
                Err(e) => format!("spawn_npc error: {e}"),
            }
        }
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

fn ui(frame: &mut Frame, state: &AppState, input_mode: &InputMode, input_buf: &str,
      entity_list_state: &mut ListState, overlay: &Overlay, log_scroll: usize) {

    let area = frame.area();

    // Top: map+stats | bottom: log | bottom: input bar
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(8),
            Constraint::Length(3),
        ])
        .split(area);

    // Top row: map (left 60%) | right panel (40%)
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(outer[0]);

    // Right panel: stats (top) | entity list (bottom)
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(top[1]);

    render_map(frame, top[0], state);
    render_player_stats(frame, right[0], state);
    render_entity_list(frame, right[1], state, entity_list_state);
    render_event_log(frame, outer[1], state, log_scroll);
    render_input_bar(frame, outer[2], input_mode, input_buf, state);

    // Overlays
    match overlay {
        Overlay::None => {}
        Overlay::Help => render_help_overlay(frame, area),
        Overlay::Inspect(eid, data) => render_inspect_overlay(frame, area, *eid, data),
        Overlay::RawBrp(raw) => render_raw_overlay(frame, area, raw),
    }
}

fn render_map(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(" MAP ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some((px, py, _)) = state.player_pos else {
        let offline = Paragraph::new(if state.server_online { "Waiting for player..." } else { "Server offline" })
            .alignment(Alignment::Center);
        frame.render_widget(offline, inner);
        return;
    };

    let w = inner.width as i32;
    let h = inner.height as i32;

    // Scale: 1 cell = ~2 world units; map is centered on player
    let scale = 2.0_f32;
    let cx = inner.x as i32 + w / 2;
    let cy = inner.y as i32 + h / 2;

    let mut cells: std::collections::HashMap<(u16, u16), (char, Color)> = std::collections::HashMap::new();

    // Draw nearby entities
    for e in &state.nearby_entities {
        let dx = ((e.x - px) / scale) as i32;
        let dy = ((e.y - py) / scale) as i32;
        let sx = cx + dx;
        let sy = cy - dy; // y increases downward in terminal
        if sx >= inner.x as i32 && sx < (inner.x + inner.width) as i32
            && sy >= inner.y as i32 && sy < (inner.y + inner.height) as i32 {
            cells.insert((sx as u16, sy as u16), (e.kind.glyph(), e.kind.color()));
        }
    }

    // Draw all cells
    for ((col, row), (glyph, color)) in &cells {
        let span = Span::styled(glyph.to_string(), Style::default().fg(*color));
        frame.render_widget(Paragraph::new(span), Rect::new(*col, *row, 1, 1));
    }

    // Draw player at center (always on top)
    let player_span = Span::styled("@", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    frame.render_widget(Paragraph::new(player_span), Rect::new(cx as u16, cy as u16, 1, 1));

    // Legend bottom-left
    let legend = Line::from(vec![
        Span::styled("@", Style::default().fg(Color::Cyan)), Span::raw("=you "),
        Span::styled("n", Style::default().fg(Color::Yellow)), Span::raw("=npc "),
        Span::styled("b", Style::default().fg(Color::Blue)), Span::raw("=bot "),
        Span::styled("W", Style::default().fg(Color::Green)), Span::raw("=wild"),
    ]);
    let legend_y = inner.y + inner.height.saturating_sub(1);
    frame.render_widget(Paragraph::new(legend), Rect::new(inner.x, legend_y, inner.width, 1));
}

fn render_player_stats(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(" PLAYER STATS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !state.server_online {
        frame.render_widget(Paragraph::new("Offline"), inner);
        return;
    }

    let s = &state.player_stats;
    let pos_str = if let Some((x, y, z)) = state.player_pos {
        format!("Pos: ({:.1}, {:.1}, {:.1})", x, y, z)
    } else {
        "Pos: --".to_string()
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(&s.class, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(format!("  Lv.{}", s.level)),
        ]),
        Line::from(pos_str),
    ];

    if inner.height >= 2 {
        frame.render_widget(Paragraph::new(lines), Rect::new(inner.x, inner.y, inner.width, 2.min(inner.height)));
    }

    if inner.height >= 4 {
        // HP bar
        let hp_ratio = if s.hp_max > 0.0 { (s.hp / s.hp_max).clamp(0.0, 1.0) as f64 } else { 0.0 };
        let hp_gauge = Gauge::default()
            .block(Block::default().title("HP"))
            .gauge_style(Style::default().fg(Color::Red))
            .ratio(hp_ratio)
            .label(format!("{:.0}/{:.0}", s.hp, s.hp_max));
        frame.render_widget(hp_gauge, Rect::new(inner.x, inner.y + 2, inner.width, 1));

        // XP bar
        if inner.height >= 5 {
            let xp_ratio = if s.xp_next > 0.0 { (s.xp / s.xp_next).clamp(0.0, 1.0) as f64 } else { 0.0 };
            let xp_gauge = Gauge::default()
                .block(Block::default().title("XP"))
                .gauge_style(Style::default().fg(Color::Magenta))
                .ratio(xp_ratio)
                .label(format!("{:.0}/{:.0}", s.xp, s.xp_next));
            frame.render_widget(xp_gauge, Rect::new(inner.x, inner.y + 3, inner.width, 1));
        }

        // Underground pressure
        if let Some(pressure) = state.underground_pressure {
            if inner.height >= 6 {
                let p_ratio = (pressure / 100.0).clamp(0.0, 1.0) as f64;
                let p_gauge = Gauge::default()
                    .block(Block::default().title("Pressure"))
                    .gauge_style(Style::default().fg(Color::Yellow))
                    .ratio(p_ratio)
                    .label(format!("{:.0}%", pressure));
                frame.render_widget(p_gauge, Rect::new(inner.x, inner.y + 4, inner.width, 1));
            }
        }
    }
}

fn render_entity_list(frame: &mut Frame, area: Rect, state: &AppState, list_state: &mut ListState) {
    let block = Block::default()
        .title(" NEARBY ENTITIES ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let items: Vec<ListItem> = state.nearby_entities.iter().map(|e| {
        let hp_str = if let (Some(hp), Some(hp_max)) = (e.hp, e.hp_max) {
            format!(" HP:{:.0}/{:.0}", hp, hp_max)
        } else {
            String::new()
        };
        let line = Line::from(vec![
            Span::styled(e.kind.glyph().to_string(), Style::default().fg(e.kind.color())),
            Span::raw(format!(" {:.1}m [{:.1},{:.1}]{}", e.distance, e.x, e.y, hp_str)),
        ]);
        ListItem::new(line)
    }).collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, list_state);
}

fn render_event_log(frame: &mut Frame, area: Rect, state: &AppState, scroll: usize) {
    let block = Block::default()
        .title(" EVENT LOG ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner_h = area.height.saturating_sub(2) as usize;
    let log_entries: Vec<&str> = state.event_log.iter().map(|s| s.as_str()).collect();
    let total = log_entries.len();
    let start = if total > inner_h + scroll {
        total - inner_h - scroll
    } else {
        0
    };
    let visible: Vec<Line> = log_entries[start..total.saturating_sub(scroll)]
        .iter()
        .map(|s| Line::from(*s))
        .collect();

    let para = Paragraph::new(visible).block(block).wrap(Wrap { trim: true });
    frame.render_widget(para, area);
}

fn render_input_bar(frame: &mut Frame, area: Rect, input_mode: &InputMode, input_buf: &str, state: &AppState) {
    let prefix = match input_mode {
        InputMode::Normal => "> ",
        InputMode::Command => "CMD> ",
    };
    let status = if state.server_online { "ONLINE" } else { "OFFLINE" };
    let status_color = if state.server_online { Color::Green } else { Color::Red };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(Color::Yellow)),
        Span::raw(input_buf),
        Span::raw("_"),
        Span::raw("  "),
        Span::styled(status, Style::default().fg(status_color)),
        Span::raw("  [Tab=cmd] [?=help] [Ctrl+C=quit]"),
    ]);
    frame.render_widget(Paragraph::new(line), inner);
}

fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let width = 60u16.min(area.width.saturating_sub(4));
    let height = 24u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let text = vec![
        Line::from(Span::styled(" HELP ", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("── Navigation ──────────────────────────"),
        Line::from("  WASD / Arrows  Move player"),
        Line::from("  PgUp/PgDn      Scroll event log"),
        Line::from("  Up/Down        Select entity in list"),
        Line::from("  i              Inspect selected entity"),
        Line::from(""),
        Line::from("── Modes ───────────────────────────────"),
        Line::from("  Tab            Toggle command mode"),
        Line::from("  ?              This help"),
        Line::from("  Ctrl+C         Quit"),
        Line::from(""),
        Line::from("── Commands (Tab mode) ──────────────────"),
        Line::from("  c <class>      Choose class"),
        Line::from("  t x y          Teleport to position"),
        Line::from("  B              Spawn bot"),
        Line::from("  k              Kill selected entity"),
        Line::from("  b              Show battle history"),
        Line::from("  S              List settlements"),
        Line::from("  / METHOD {..}  Raw BRP call"),
        Line::from(""),
        Line::from("  Press any key to close"),
    ];

    let block = Block::default()
        .title(" HELP ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    frame.render_widget(para, popup);
}

fn render_inspect_overlay(frame: &mut Frame, area: Rect, eid: u64, data: &Value) {
    let width = (area.width * 3 / 4).max(40);
    let height = (area.height * 3 / 4).max(10);
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let raw = serde_json::to_string_pretty(data).unwrap_or_default();
    let lines: Vec<Line> = raw.lines().map(|l| Line::from(l.to_string())).collect();

    let block = Block::default()
        .title(format!(" INSPECT Entity {} ", eid))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let para = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(para, popup);
}

fn render_raw_overlay(frame: &mut Frame, area: Rect, raw: &str) {
    let width = (area.width * 3 / 4).max(40);
    let height = (area.height * 3 / 4).max(10);
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let lines: Vec<Line> = raw.lines().map(|l| Line::from(l.to_string())).collect();

    let block = Block::default()
        .title(" BRP RESULT ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let para = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(para, popup);
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("fellytip_tui=debug")
        .with_writer(std::io::stderr)
        .init();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let state = Arc::new(Mutex::new(AppState::default()));
    let (cmd_tx, cmd_rx) = mpsc::channel::<AppCmd>(32);
    let (result_tx, mut result_rx) = mpsc::channel::<String>(32);

    // Start background poll task
    let state_clone = state.clone();
    tokio::spawn(async move {
        poll_task(state_clone, cmd_rx, result_tx).await;
    });

    let mut input_mode = InputMode::Normal;
    let mut input_buf = String::new();
    let mut entity_list_state = ListState::default();
    let mut overlay = Overlay::None;
    let mut log_scroll: usize = 0;

    // Movement step size
    let step = 3.0_f32;

    loop {
        // Collect any results from poll task
        while let Ok(result) = result_rx.try_recv() {
            state.lock().unwrap().push_log(result);
        }

        // Draw
        {
            let s = state.lock().unwrap().clone();
            terminal.draw(|frame| {
                ui(frame, &s, &input_mode, &input_buf, &mut entity_list_state, &overlay, log_scroll);
            })?;
        }

        // Handle input (non-blocking, 50ms timeout)
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Dismiss overlays on any key except help toggle
                match &overlay {
                    Overlay::None => {}
                    Overlay::Help => { overlay = Overlay::None; continue; }
                    Overlay::Inspect(_, _) | Overlay::RawBrp(_) => {
                        overlay = Overlay::None;
                        continue;
                    }
                }

                // Ctrl+C always quits
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    break;
                }

                match input_mode {
                    InputMode::Normal => match key.code {
                        KeyCode::Char('?') => { overlay = Overlay::Help; }
                        KeyCode::Tab => { input_mode = InputMode::Command; input_buf.clear(); }
                        KeyCode::Char('i') => {
                            if let Some(idx) = entity_list_state.selected() {
                                let eid = {
                                    let s = state.lock().unwrap();
                                    s.nearby_entities.get(idx).map(|e| e.entity_id)
                                };
                                if let Some(eid) = eid {
                                    let _ = cmd_tx.send(AppCmd::Inspect(eid)).await;
                                    // Wait briefly for result
                                    tokio::time::sleep(Duration::from_millis(300)).await;
                                    if let Ok(result) = result_rx.try_recv() {
                                        if let Ok(v) = serde_json::from_str(&result) {
                                            overlay = Overlay::Inspect(eid, v);
                                        } else {
                                            overlay = Overlay::RawBrp(result);
                                        }
                                    }
                                }
                            }
                        }
                        // Movement + entity selection (arrows dual-purpose)
                        KeyCode::Char('w') => {
                            move_player(&state, &cmd_tx, 0.0, step).await;
                        }
                        KeyCode::Char('s') => {
                            move_player(&state, &cmd_tx, 0.0, -step).await;
                        }
                        KeyCode::Char('a') => {
                            move_player(&state, &cmd_tx, -step, 0.0).await;
                        }
                        KeyCode::Char('d') => {
                            move_player(&state, &cmd_tx, step, 0.0).await;
                        }
                        KeyCode::Up => {
                            let n = state.lock().unwrap().nearby_entities.len();
                            if n > 0 {
                                let sel = entity_list_state.selected().unwrap_or(0);
                                entity_list_state.select(Some(sel.saturating_sub(1)));
                            } else {
                                move_player(&state, &cmd_tx, 0.0, step).await;
                            }
                        }
                        KeyCode::Down => {
                            let n = state.lock().unwrap().nearby_entities.len();
                            if n > 0 {
                                let sel = entity_list_state.selected().unwrap_or(0);
                                entity_list_state.select(Some((sel + 1).min(n.saturating_sub(1))));
                            } else {
                                move_player(&state, &cmd_tx, 0.0, -step).await;
                            }
                        }
                        KeyCode::Left => {
                            move_player(&state, &cmd_tx, -step, 0.0).await;
                        }
                        KeyCode::Right => {
                            move_player(&state, &cmd_tx, step, 0.0).await;
                        }
                        // Log scroll
                        KeyCode::PageUp => { log_scroll = log_scroll.saturating_add(5); }
                        KeyCode::PageDown => { log_scroll = log_scroll.saturating_sub(5); }
                        _ => {}
                    }
                    InputMode::Command => match key.code {
                        KeyCode::Tab | KeyCode::Esc => {
                            input_mode = InputMode::Normal;
                            input_buf.clear();
                        }
                        KeyCode::Enter => {
                            let cmd_str = input_buf.trim().to_string();
                            input_buf.clear();
                            input_mode = InputMode::Normal;
                            if !cmd_str.is_empty() {
                                handle_command_str(&cmd_str, &state, &cmd_tx).await;
                            }
                        }
                        KeyCode::Backspace => { input_buf.pop(); }
                        KeyCode::Char(c) => { input_buf.push(c); }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}

async fn move_player(state: &Arc<Mutex<AppState>>, cmd_tx: &mpsc::Sender<AppCmd>, dx: f32, dy: f32) {
    let (pos, has_method, player_entity) = {
        let s = state.lock().unwrap();
        (s.player_pos, s.has_player_position_method, s.player_entity)
    };
    if let Some((px, py, pz)) = pos {
        let nx = px + dx;
        let ny = py + dy;
        if has_method {
            let _ = cmd_tx.send(AppCmd::Teleport(nx, ny)).await;
        } else if let Some(eid) = player_entity {
            let _ = cmd_tx.send(AppCmd::TeleportEntity(eid, nx, ny, pz)).await;
        }
    }
}

async fn handle_command_str(cmd_str: &str, state: &Arc<Mutex<AppState>>, cmd_tx: &mpsc::Sender<AppCmd>) {
    let parts: Vec<&str> = cmd_str.splitn(3, ' ').collect();
    match parts[0] {
        "c" => {
            if let Some(class) = parts.get(1) {
                let _ = cmd_tx.send(AppCmd::ChooseClass(class.to_string())).await;
            }
        }
        "t" => {
            if parts.len() >= 3 {
                let x: f32 = parts[1].parse().unwrap_or(0.0);
                let y: f32 = parts[2].parse().unwrap_or(0.0);
                let _ = cmd_tx.send(AppCmd::Teleport(x, y)).await;
            }
        }
        "B" | "b" if parts[0] == "B" => {
            let _ = cmd_tx.send(AppCmd::SpawnBot).await;
        }
        "k" => {
            // Kill selected entity or by ID
            let eid = if let Some(id_str) = parts.get(1) {
                id_str.parse::<u64>().ok()
            } else {
                None
            };
            if let Some(eid) = eid {
                let _ = cmd_tx.send(AppCmd::Kill(eid)).await;
            } else {
                state.lock().unwrap().push_log("k <entity_id>".to_string());
            }
        }
        "b" => {
            // Show battle history in log
            let history = {
                let s = state.lock().unwrap();
                s.battle_history_raw.clone()
            };
            let mut s = state.lock().unwrap();
            s.push_log(format!("--- Battle history ({} entries) ---", history.len()));
            for e in history.iter().rev().take(10) {
                s.push_log(format_battle_entry(e));
            }
        }
        "S" => {
            let _ = cmd_tx.send(AppCmd::ListSettlements).await;
            let _ = cmd_tx.send(AppCmd::ListBots).await;
        }
        "/" => {
            let raw = parts[1..].join(" ");
            let _ = cmd_tx.send(AppCmd::BrpRaw(raw)).await;
        }
        _ => {
            state.lock().unwrap().push_log(format!("Unknown command: {}", cmd_str));
        }
    }
}
