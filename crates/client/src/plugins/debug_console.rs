use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use fellytip_shared::{
    components::{Experience, Health, PlayerStandings},
    world::{faction::standing_tier, story::GameEntityId},
};
use crate::{LocalPlayer, PredictedPosition};

type LocalPlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static PredictedPosition,
        &'static Health,
        &'static Experience,
        &'static GameEntityId,
        Option<&'static PlayerStandings>,
    ),
    With<LocalPlayer>,
>;

#[derive(Resource, Default)]
pub struct DebugConsole {
    pub open: bool,
    input: String,
    output: Vec<String>,
    request_focus: bool,
}

pub struct DebugConsolePlugin;

impl Plugin for DebugConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugConsole>()
            .add_systems(Update, toggle_debug_console)
            .add_systems(EguiPrimaryContextPass, draw_debug_console);
    }
}

fn toggle_debug_console(keyboard: Res<ButtonInput<KeyCode>>, mut console: ResMut<DebugConsole>) {
    if keyboard.just_pressed(KeyCode::Backquote) {
        console.open = !console.open;
        if console.open {
            console.input.clear();
            console.request_focus = true;
        }
    }
}

fn draw_debug_console(
    mut ctx: EguiContexts,
    mut console: ResMut<DebugConsole>,
    player_q: LocalPlayerQuery,
) -> Result {
    let egui_ctx = ctx.ctx_mut()?;

    if !console.open {
        return Ok(());
    }

    let input_id = egui::Id::new("debug_console_input");
    let mut pending_cmd: Option<String> = None;

    egui::Window::new("Debug Console")
        .anchor(egui::Align2::CENTER_TOP, [0.0, 20.0])
        .resizable(true)
        .min_width(520.0)
        .show(egui_ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &console.output {
                        ui.monospace(line);
                    }
                });

            ui.separator();

            let response = ui.add(
                egui::TextEdit::singleline(&mut console.input)
                    .id(input_id)
                    .hint_text("help | pos | whoami | teleport <x> <y>")
                    .desired_width(f32::INFINITY),
            );

            if console.request_focus {
                response.request_focus();
                console.request_focus = false;
            }

            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let cmd = console.input.trim().to_string();
                if !cmd.is_empty() {
                    pending_cmd = Some(cmd);
                }
                console.input.clear();
                egui_ctx.memory_mut(|m| m.request_focus(input_id));
            }
        });

    if let Some(cmd) = pending_cmd {
        run_command(&cmd, &mut console, &player_q);
    }

    Ok(())
}

#[cfg(not(target_family = "wasm"))]
fn brp_call(method: &str, params: serde_json::Value) -> String {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });
    match ureq::post("http://localhost:15702/brp")
        .set("Content-Type", "application/json")
        .send_json(&body)
    {
        Ok(resp) => {
            if let Ok(v) = resp.into_json::<serde_json::Value>() {
                if let Some(result) = v.get("result") {
                    return format!("  {result}");
                }
                if let Some(err) = v.get("error") {
                    return format!("  error: {err}");
                }
            }
            "  ok".into()
        }
        Err(e) => format!("  brp error: {e}"),
    }
}

#[cfg(target_family = "wasm")]
fn brp_call(_method: &str, _params: serde_json::Value) -> String {
    "  brp not supported on wasm".into()
}

fn terminal_to_brp(parts: &[&str]) -> Option<(String, serde_json::Value)> {
    match parts {
        ["tp" | "teleport", x, y] => Some((
            "dm/teleport_player".into(),
            serde_json::json!({"x": x.parse::<f32>().ok()?, "y": y.parse::<f32>().ok()?, "z": 0.0}),
        )),
        ["tp" | "teleport", x, y, z] => Some((
            "dm/teleport_player".into(),
            serde_json::json!({"x": x.parse::<f32>().ok()?, "y": y.parse::<f32>().ok()?, "z": z.parse::<f32>().ok()?}),
        )),
        ["time", t] => Some((
            "dm/set_time_of_day".into(),
            serde_json::json!({"time": t.parse::<f32>().ok()?}),
        )),
        ["screenshot"] => Some((
            "dm/take_screenshot".into(),
            serde_json::json!({"path": "/tmp/screenshot.png"}),
        )),
        ["screenshot", path] => Some((
            "dm/take_screenshot".into(),
            serde_json::json!({"path": path}),
        )),
        ["portal"] => Some(("dm/enter_portal".into(), serde_json::json!({}))),
        [method, rest @ ..] if method.starts_with("dm/") => {
            let mut map = serde_json::Map::new();
            for kv in rest.iter() {
                if let Some((k, v)) = kv.split_once('=') {
                    let val = v
                        .parse::<f64>()
                        .map(serde_json::Value::from)
                        .or_else(|_| v.parse::<bool>().map(serde_json::Value::from))
                        .unwrap_or_else(|_| serde_json::Value::String(v.to_string()));
                    map.insert(k.to_string(), val);
                }
            }
            Some((method.to_string(), serde_json::Value::Object(map)))
        }
        _ => None,
    }
}

fn run_command(cmd: &str, console: &mut DebugConsole, player_q: &LocalPlayerQuery) {
    console.output.push(format!("> {cmd}"));
    let parts: Vec<&str> = cmd.split_whitespace().collect();

    // Terminal-only commands
    match parts.as_slice() {
        ["help"] | [] => {
            for line in [
                "  help                              — this list",
                "  pos                               — current coordinates",
                "  whoami                            — name, stats, position",
                "  teleport <x> <y> [z]             — warp to world position (via BRP)",
                "  time <t>                          — set time of day (via BRP)",
                "  screenshot [path]                 — take screenshot (via BRP)",
                "  portal                            — enter portal (via BRP)",
                "  dm/<method> [key=val ...]         — raw BRP pass-through",
            ] {
                console.output.push(line.into());
            }
            return;
        }

        ["clear"] => {
            console.output.clear();
            return;
        }

        ["pos"] | ["position"] => {
            match player_q.single() {
                Ok((pred, ..)) => {
                    console.output.push(format!("  ({:.2}, {:.2}, {:.2})", pred.x, pred.y, pred.z));
                }
                Err(_) => console.output.push("  not connected".into()),
            }
            return;
        }

        ["whoami"] => {
            match player_q.single() {
                Ok((pred, health, exp, geid, standings)) => {
                    let short_id = geid.0.to_string();
                    let short_id = &short_id[..8];
                    console.output.push(format!("  ID:       {short_id}…  (full: {})", geid.0));
                    console.output.push(format!(
                        "  Level:    {}  ({}/{} XP)",
                        exp.level, exp.xp, exp.xp_to_next
                    ));
                    console.output.push(format!("  HP:       {}/{}", health.current, health.max));
                    console.output.push(format!(
                        "  Position: ({:.2}, {:.2}, {:.2})",
                        pred.x, pred.y, pred.z
                    ));
                    if let Some(s) = standings {
                        if !s.standings.is_empty() {
                            let summary: Vec<String> = s
                                .standings
                                .iter()
                                .map(|(name, score)| {
                                    let tier = standing_tier(*score);
                                    format!("{name} {score:+} ({tier:?})")
                                })
                                .collect();
                            console.output.push(format!("  Standing: {}", summary.join(", ")));
                        }
                    }
                }
                Err(_) => console.output.push("  not connected".into()),
            }
            return;
        }

        _ => {}
    }

    // BRP-proxied commands
    if let Some((method, params)) = terminal_to_brp(&parts) {
        let result = brp_call(&method, params);
        console.output.push(result);
    } else {
        console
            .output
            .push(format!("  unknown command '{}' — type help", parts.first().unwrap_or(&"")));
    }
}
