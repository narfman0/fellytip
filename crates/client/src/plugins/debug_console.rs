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
        &'static mut PredictedPosition,
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
            .add_systems(EguiPrimaryContextPass, draw_debug_console);
    }
}

fn draw_debug_console(
    mut ctx: EguiContexts,
    mut console: ResMut<DebugConsole>,
    mut player_q: LocalPlayerQuery,
    keyboard: Res<ButtonInput<KeyCode>>,
) -> Result {
    if keyboard.just_pressed(KeyCode::Backquote) {
        console.open = !console.open;
        if console.open {
            console.input.clear();
            console.request_focus = true;
        }
    }

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
        run_command(&cmd, &mut console, &mut player_q);
    }

    Ok(())
}

fn run_command(cmd: &str, console: &mut DebugConsole, player_q: &mut LocalPlayerQuery) {
    console.output.push(format!("> {cmd}"));
    let parts: Vec<&str> = cmd.split_whitespace().collect();

    match parts.as_slice() {
        ["help"] | [] => {
            for line in [
                "  help                    — this list",
                "  pos                     — current coordinates",
                "  whoami                  — name, stats, position",
                "  teleport <x> <y> [z]   — warp to world position",
            ] {
                console.output.push(line.into());
            }
        }

        ["pos"] | ["position"] => match player_q.single_mut() {
            Ok((pred, ..)) => {
                console.output.push(format!("  ({:.2}, {:.2}, {:.2})", pred.x, pred.y, pred.z));
            }
            Err(_) => console.output.push("  not connected".into()),
        },

        ["whoami"] => match player_q.single_mut() {
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
        },

        ["teleport", xs, ys] | ["tp", xs, ys] => {
            match (xs.parse::<f32>(), ys.parse::<f32>()) {
                (Ok(x), Ok(y)) => match player_q.single_mut() {
                    Ok((mut pred, ..)) => {
                        pred.x = x;
                        pred.y = y;
                        console
                            .output
                            .push(format!("  teleported to ({x:.2}, {y:.2}, {:.2})", pred.z));
                    }
                    Err(_) => console.output.push("  not connected".into()),
                },
                _ => console.output.push("  usage: teleport <x> <y>".into()),
            }
        }

        ["teleport", xs, ys, zs] | ["tp", xs, ys, zs] => {
            match (xs.parse::<f32>(), ys.parse::<f32>(), zs.parse::<f32>()) {
                (Ok(x), Ok(y), Ok(z)) => match player_q.single_mut() {
                    Ok((mut pred, ..)) => {
                        pred.x = x;
                        pred.y = y;
                        pred.z = z;
                        console
                            .output
                            .push(format!("  teleported to ({x:.2}, {y:.2}, {z:.2})"));
                    }
                    Err(_) => console.output.push("  not connected".into()),
                },
                _ => console.output.push("  usage: teleport <x> <y> <z>".into()),
            }
        }

        _ => {
            console
                .output
                .push(format!("  unknown command '{cmd}' — type help"));
        }
    }
}
