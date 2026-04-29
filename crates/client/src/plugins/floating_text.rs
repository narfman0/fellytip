//! Floating combat text overlay (damage numbers, "Miss!", critical hits).
//!
//! Listens to `ClientDamageMsg` and renders transient labels above the impact
//! point using egui Areas projected from world-space to screen-space.
//!
//! Visual scheme:
//!   - Regular hit  : white number, 14 px, floats up, fades out over 1.2 s
//!   - Miss         : grey "Miss!", 13 px, fades out over 1.0 s
//!   - Critical hit : gold number + "!", 20 px, fades out over 1.5 s

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use fellytip_shared::protocol::ClientDamageMsg;
use super::camera::OrbitCamera;

#[derive(Clone)]
struct FloatEntry {
    /// Impact position in Bevy world space (x, y_up, z).
    world_pos: Vec3,
    text: String,
    color: egui::Color32,
    font_size: f32,
    age: f32,
    max_age: f32,
}

#[derive(Resource, Default)]
pub struct FloatingTextQueue(Vec<FloatEntry>);

pub struct FloatingTextPlugin;

impl Plugin for FloatingTextPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FloatingTextQueue>()
            .add_systems(Update, enqueue_damage_texts)
            .add_systems(EguiPrimaryContextPass, draw_floating_texts);
    }
}

fn enqueue_damage_texts(
    mut messages: MessageReader<ClientDamageMsg>,
    mut queue: ResMut<FloatingTextQueue>,
) {
    for msg in messages.read() {
        // msg.x/y/z are already in Bevy space (emitted as pos.x, pos.z, pos.y).
        // Offset upward by 1.8 units so text appears above the entity's head.
        let world_pos = Vec3::new(msg.x, msg.y + 1.8, msg.z);

        let entry = if msg.is_miss {
            FloatEntry {
                world_pos,
                text: "Miss!".to_string(),
                color: egui::Color32::from_rgb(180, 180, 180),
                font_size: 13.0,
                age: 0.0,
                max_age: 1.0,
            }
        } else if msg.is_critical {
            FloatEntry {
                world_pos,
                text: format!("{}!", msg.damage),
                color: egui::Color32::from_rgb(255, 200, 0),
                font_size: 20.0,
                age: 0.0,
                max_age: 1.5,
            }
        } else {
            FloatEntry {
                world_pos,
                text: msg.damage.to_string(),
                color: egui::Color32::WHITE,
                font_size: 14.0,
                age: 0.0,
                max_age: 1.2,
            }
        };
        queue.0.push(entry);
    }
}

fn draw_floating_texts(
    mut ctx: EguiContexts,
    mut queue: ResMut<FloatingTextQueue>,
    camera_q: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    time: Res<Time>,
) -> Result {
    let egui_ctx = ctx.ctx_mut()?;
    let Ok((camera, camera_transform)) = camera_q.single() else {
        return Ok(());
    };
    let dt = time.delta_secs();

    let mut i = 0;
    while i < queue.0.len() {
        queue.0[i].age += dt;
        if queue.0[i].age >= queue.0[i].max_age {
            queue.0.swap_remove(i);
            continue;
        }

        let entry = &queue.0[i];
        let t = entry.age / entry.max_age;
        // Float upward 40 logical pixels over the full lifetime.
        let float_px = t * 40.0;
        let alpha = ((1.0 - t) * 255.0) as u8;

        if let Ok(screen_pos) = camera.world_to_viewport(camera_transform, entry.world_pos) {
            let color = egui::Color32::from_rgba_unmultiplied(
                entry.color.r(),
                entry.color.g(),
                entry.color.b(),
                alpha,
            );
            let text = entry.text.clone();
            let font_size = entry.font_size;
            egui::Area::new(egui::Id::new(("float_text", i, entry.age.to_bits())))
                .fixed_pos(egui::pos2(screen_pos.x - 20.0, screen_pos.y - float_px))
                .interactable(false)
                .order(egui::Order::Tooltip)
                .show(egui_ctx, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .color(color)
                            .size(font_size)
                            .strong(),
                    );
                });
        }

        i += 1;
    }

    Ok(())
}
