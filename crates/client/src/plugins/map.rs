//! Mini-map window toggled by M or Tab.
//!
//! Renders a 512×512 downsampled terrain texture from `WorldMap` with optional
//! overlays for settlement locations and faction sphere-of-influence circles.
//! The canvas supports scroll-to-zoom and click-drag-to-pan; it opens centred
//! on the local player.

use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, EguiTextureHandle, EguiUserTextures, egui};
use fellytip_shared::{
    components::PlayerStandings,
    world::{
        civilization::{Settlement, SettlementKind, Settlements},
        faction::{standing_tier, StandingTier},
        map::{TileKind, WorldMap},
    },
};

use crate::{LocalPlayer, PredictedPosition};
use crate::plugins::debug_console::DebugConsole;
use crate::plugins::pause_menu::PauseMenu;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Downsampled terrain texture side length (pixels). 2 tiles per pixel.
const TEX_SIZE: usize = 512;

/// World-space extent: map spans [-512, 512) in both X and Y.
const MAP_W: f32 = 1024.0;
const MAP_H: f32 = 1024.0;

/// Default zoom so the full 1024-unit map fits in the 512-px canvas.
const ZOOM_DEFAULT: f32 = 0.5;
const ZOOM_MIN: f32 = 0.2;
const ZOOM_MAX: f32 = 4.0;

/// Egui canvas inside the map window (pixels).
const CANVAS: f32 = 512.0;

/// Always-visible minimap canvas size (pixels).
const MINI_CANVAS: f32 = 180.0;
/// Minimap zoom: pixels per world unit, giving ±50 world units visible radius.
const MINI_ZOOM: f32 = 1.8;
/// Proximity threshold for "Near: <town>" label (world units).
const NEAR_RADIUS: f32 = 80.0;

// ── Resources ─────────────────────────────────────────────────────────────────

/// Map window state.
#[derive(Resource)]
pub struct MapWindow {
    pub open: bool,
    /// Pixels per world unit.
    pub zoom: f32,
    /// World-space X at the centre of the canvas.
    pub pan_x: f32,
    /// World-space Y at the centre of the canvas.
    pub pan_y: f32,
    pub show_settlements: bool,
    pub show_factions: bool,
}

impl Default for MapWindow {
    fn default() -> Self {
        Self {
            open: false,
            zoom: ZOOM_DEFAULT,
            pan_x: 0.0,
            pan_y: 0.0,
            show_settlements: true,
            show_factions: false,
        }
    }
}

/// One-shot cached terrain texture id.
#[derive(Resource, Default)]
struct TerrainTex(Option<egui::TextureId>);

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapWindow>()
            .init_resource::<TerrainTex>()
            .add_systems(Update, build_terrain_texture)
            .add_systems(EguiPrimaryContextPass, (draw_map, draw_minimap));
    }
}

// ── Terrain texture ───────────────────────────────────────────────────────────

fn tile_color(kind: TileKind) -> [u8; 4] {
    match kind {
        TileKind::Plains                                       => [120, 170,  80, 255],
        TileKind::Grassland                                    => [100, 160,  60, 255],
        TileKind::Forest | TileKind::TemperateForest           => [ 50, 110,  40, 255],
        TileKind::TropicalForest | TileKind::TropicalRainforest => [ 30, 100,  50, 255],
        TileKind::Savanna                                      => [180, 160,  60, 255],
        TileKind::Taiga                                        => [ 60, 120,  80, 255],
        TileKind::Tundra                                       => [160, 150, 120, 255],
        TileKind::Mountain                                     => [150, 140, 130, 255],
        TileKind::Stone                                        => [180, 170, 160, 255],
        TileKind::Desert                                       => [220, 200, 120, 255],
        TileKind::PolarDesert                                  => [200, 200, 190, 255],
        TileKind::Arctic                                       => [220, 235, 250, 255],
        TileKind::Water                                        => [ 60, 120, 200, 255],
        TileKind::River                                        => [ 80, 140, 210, 255],
        TileKind::Void                                         => [  8,   8,  16, 255],
    }
}

/// Generates and registers the 512×512 terrain texture once `WorldMap` exists.
/// Y-flipped so texture V=0 = north (high world-Y), V=1 = south.
fn build_terrain_texture(
    world_map: Option<Res<WorldMap>>,
    mut tex: ResMut<TerrainTex>,
    mut images: ResMut<Assets<Image>>,
    mut user_textures: ResMut<EguiUserTextures>,
) {
    if tex.0.is_some() {
        return;
    }
    let Some(map) = world_map else { return };

    let mut data = vec![0u8; TEX_SIZE * TEX_SIZE * 4];
    for ty in 0..TEX_SIZE {
        for tx in 0..TEX_SIZE {
            let ix = (tx * 2).min(map.width.saturating_sub(1));
            // Flip Y: ty=0 → northernmost tile row.
            let iy = (map.height.saturating_sub(1)).saturating_sub(ty * 2);
            let kind = map
                .column(ix, iy)
                .layers
                .last()
                .map(|l| l.kind)
                .unwrap_or(TileKind::Void);
            let color = tile_color(kind);
            let idx = (ty * TEX_SIZE + tx) * 4;
            data[idx..idx + 4].copy_from_slice(&color);
        }
    }

    let image = Image::new(
        Extent3d {
            width: TEX_SIZE as u32,
            height: TEX_SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    let handle = images.add(image);
    tex.0 = Some(user_textures.add_image(EguiTextureHandle::Strong(handle)));
}

// ── Map window ────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_map(
    mut ctx: EguiContexts,
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    mut map_win: ResMut<MapWindow>,
    tex: Res<TerrainTex>,
    settlements: Option<Res<Settlements>>,
    player_q: Query<(&PredictedPosition, Option<&PlayerStandings>), With<LocalPlayer>>,
    console: Option<Res<DebugConsole>>,
    pause_menu: Option<Res<PauseMenu>>,
) -> Result {
    // Toggle on M or Tab, unless another overlay is open.
    if let Some(ref kb) = keyboard {
        let blocked = console.is_some_and(|c| c.open) || pause_menu.is_some_and(|m| m.open);
        if !blocked && (kb.just_pressed(KeyCode::KeyM) || kb.just_pressed(KeyCode::Tab)) {
            map_win.open = !map_win.open;
            if map_win.open {
                if let Ok((pos, _)) = player_q.single() {
                    map_win.pan_x = pos.x;
                    map_win.pan_y = pos.y;
                    map_win.zoom = ZOOM_DEFAULT;
                }
            }
        }
    }

    if !map_win.open {
        return Ok(());
    }
    let Some(terrain_id) = tex.0 else { return Ok(()) };

    let egui_ctx = ctx.ctx_mut()?;

    egui::Window::new("Map")
        .collapsible(false)
        .resizable(false)
        .default_size(egui::vec2(544.0, 610.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(egui_ctx, |ui| {
            // ── Controls ──────────────────────────────────────────────────────
            ui.horizontal(|ui| {
                if ui.button("✕ Close [M]").clicked() {
                    map_win.open = false;
                }
                ui.separator();
                ui.checkbox(&mut map_win.show_settlements, "Settlements");
                ui.checkbox(&mut map_win.show_factions, "Faction Spheres");
                ui.separator();
                ui.label(format!("Zoom {:.1}×", map_win.zoom * 2.0));
            });

            ui.separator();

            // ── Canvas ────────────────────────────────────────────────────────
            let (resp, painter) = ui.allocate_painter(
                egui::vec2(CANVAS, CANVAS),
                egui::Sense::click_and_drag(),
            );
            let rect = resp.rect;

            // Scroll-wheel zoom.
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let factor = (1.0 + scroll * 0.005).clamp(0.8, 1.25);
                map_win.zoom = (map_win.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
            }

            // Click-drag pan ("grab and pull" semantics).
            if resp.dragged() {
                let d = resp.drag_delta();
                map_win.pan_x -= d.x / map_win.zoom;
                map_win.pan_y += d.y / map_win.zoom; // screen Y+ = world Y-
            }

            // Clamp pan so the view stays within or centred on the map.
            let half_w = CANVAS / (2.0 * map_win.zoom);
            let half_h = CANVAS / (2.0 * map_win.zoom);
            let max_px = (MAP_W / 2.0 - half_w).max(0.0);
            let max_py = (MAP_H / 2.0 - half_h).max(0.0);
            map_win.pan_x = map_win.pan_x.clamp(-max_px, max_px);
            map_win.pan_y = map_win.pan_y.clamp(-max_py, max_py);

            // Snapshot after mutation so inner closures can share immutably.
            let zoom  = map_win.zoom;
            let pan_x = map_win.pan_x;
            let pan_y = map_win.pan_y;

            let world_to_screen = |wx: f32, wy: f32| -> egui::Pos2 {
                egui::pos2(
                    rect.center().x + (wx - pan_x) * zoom,
                    rect.center().y - (wy - pan_y) * zoom,
                )
            };

            // ── Terrain texture ───────────────────────────────────────────────
            // Texture layout (Y-flipped): V=0 = north (high world-Y), V=1 = south.
            // Dark background for any area outside the map bounds.
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(8, 8, 16));

            let nw = world_to_screen(-MAP_W / 2.0,  MAP_H / 2.0);
            let se = world_to_screen( MAP_W / 2.0, -MAP_H / 2.0);
            let map_screen = egui::Rect::from_min_max(nw, se);
            let vis = rect.intersect(map_screen);
            if vis.width() > 0.0 && vis.height() > 0.0 {
                let mw = map_screen.width();
                let mh = map_screen.height();
                let uv = egui::Rect::from_min_max(
                    egui::pos2(
                        (vis.min.x - map_screen.min.x) / mw,
                        (vis.min.y - map_screen.min.y) / mh,
                    ),
                    egui::pos2(
                        (vis.max.x - map_screen.min.x) / mw,
                        (vis.max.y - map_screen.min.y) / mh,
                    ),
                );
                painter.image(terrain_id, vis, uv, egui::Color32::WHITE);
            }

            // ── Faction sphere overlay ────────────────────────────────────────
            if map_win.show_factions {
                if let Some(ref setts) = settlements {
                    let standings = player_q.iter().next().and_then(|(_, s)| s);
                    for s in &setts.0 {
                        let wx = s.x - MAP_W / 2.0;
                        let wy = s.y - MAP_H / 2.0;
                        let sp = world_to_screen(wx, wy);
                        let r = 20.0 * zoom;
                        let color = faction_sphere_color(&s.name, standings);
                        painter.circle_stroke(sp, r, egui::Stroke::new(2.0, color));
                    }
                }
            }

            // ── Settlement overlay ────────────────────────────────────────────
            if map_win.show_settlements {
                if let Some(ref setts) = settlements {
                    for s in &setts.0 {
                        let wx = s.x - MAP_W / 2.0;
                        let wy = s.y - MAP_H / 2.0;
                        let sp = world_to_screen(wx, wy);
                        let (r, fill) = match s.kind {
                            SettlementKind::Capital => (5.0, egui::Color32::from_rgb(255, 220, 60)),
                            SettlementKind::Town    => (3.0, egui::Color32::from_rgb(220, 220, 220)),
                        };
                        painter.circle_filled(sp, r, fill);
                        painter.circle_stroke(
                            sp, r,
                            egui::Stroke::new(1.0, egui::Color32::BLACK),
                        );
                        if matches!(s.kind, SettlementKind::Capital) {
                            painter.text(
                                sp + egui::vec2(7.0, -7.0),
                                egui::Align2::LEFT_BOTTOM,
                                s.name.as_str(),
                                egui::FontId::default(),
                                egui::Color32::from_rgb(255, 220, 60),
                            );
                        }
                    }
                }
            }

            // ── Player marker ─────────────────────────────────────────────────
            if let Ok((pos, _)) = player_q.single() {
                let sp = world_to_screen(pos.x, pos.y);
                painter.circle_filled(sp, 5.0, egui::Color32::from_rgb(255, 60, 60));
                painter.circle_stroke(
                    sp, 5.0,
                    egui::Stroke::new(1.5, egui::Color32::WHITE),
                );
            }

            // ── Legend ────────────────────────────────────────────────────────
            ui.separator();
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(255,  60,  60), "● You");
                ui.colored_label(egui::Color32::from_rgb(255, 220,  60), "● Capital");
                ui.colored_label(egui::Color32::from_rgb(220, 220, 220), "● Town");
                ui.label("  Scroll=zoom  Drag=pan");
            });
        });

    Ok(())
}

// ── Always-visible minimap ────────────────────────────────────────────────────

/// Returns the nearest settlement within `radius` world units of `pos`, or `None`.
fn nearest_within(pos: Vec2, settlements: &Settlements, radius: f32) -> Option<&Settlement> {
    settlements
        .0
        .iter()
        .map(|s| {
            let d = pos.distance(Vec2::new(s.x - MAP_W / 2.0, s.y - MAP_H / 2.0));
            (d, s)
        })
        .filter(|(d, _)| *d < radius)
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, s)| s)
}

/// Always-visible minimap anchored top-right showing terrain around the player,
/// settlement dots, facing direction arrow, coordinates, and nearby town name.
fn draw_minimap(
    mut ctx: EguiContexts,
    tex: Res<TerrainTex>,
    player_q: Query<(&PredictedPosition, &Transform), With<LocalPlayer>>,
    settlements: Option<Res<Settlements>>,
    console: Option<Res<DebugConsole>>,
    pause_menu: Option<Res<PauseMenu>>,
) -> Result {
    // Hide behind other overlays.
    if console.is_some_and(|c| c.open) || pause_menu.is_some_and(|m| m.open) {
        return Ok(());
    }
    let Some(terrain_id) = tex.0 else { return Ok(()) };
    let Ok((pos, transform)) = player_q.single() else { return Ok(()) };

    let px = pos.x;
    let py = pos.y;

    // Visible world radius from canvas centre to edge.
    let vis_radius = (MINI_CANVAS / 2.0) / MINI_ZOOM;

    // UV centre in the Y-flipped terrain texture (V=0 = north = high Y).
    let center_u = (px + MAP_W / 2.0) / MAP_W;
    let center_v = (MAP_H / 2.0 - py) / MAP_H;
    let half_u = vis_radius / MAP_W;
    let half_v = vis_radius / MAP_H;
    let uv = egui::Rect::from_min_max(
        egui::pos2(
            (center_u - half_u).clamp(0.0, 1.0),
            (center_v - half_v).clamp(0.0, 1.0),
        ),
        egui::pos2(
            (center_u + half_u).clamp(0.0, 1.0),
            (center_v + half_v).clamp(0.0, 1.0),
        ),
    );

    egui::Window::new("##minimap")
        .anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0])
        .resizable(false)
        .title_bar(false)
        .show(ctx.ctx_mut()?, |ui| {
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(MINI_CANVAS, MINI_CANVAS), egui::Sense::hover());
            let rect = resp.rect;
            let center = rect.center();

            // Dark fill behind the terrain (visible if clamped UVs cut off).
            painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(8, 8, 16));

            // Terrain image clipped to player vicinity.
            painter.image(terrain_id, rect, uv, egui::Color32::WHITE);

            // Settlement dots within the visible radius.
            if let Some(ref setts) = settlements {
                for s in &setts.0 {
                    let wx = s.x - MAP_W / 2.0;
                    let wy = s.y - MAP_H / 2.0;
                    let dx = wx - px;
                    let dy = wy - py;
                    if dx.abs() > vis_radius * 1.5 || dy.abs() > vis_radius * 1.5 {
                        continue;
                    }
                    let sp = egui::pos2(
                        center.x + dx * MINI_ZOOM,
                        center.y - dy * MINI_ZOOM,
                    );
                    if !rect.contains(sp) {
                        continue;
                    }
                    let (r, fill) = match s.kind {
                        SettlementKind::Capital => (4.0, egui::Color32::from_rgb(255, 220, 60)),
                        SettlementKind::Town    => (3.0, egui::Color32::from_rgb(220, 220, 220)),
                    };
                    painter.circle_filled(sp, r, fill);
                    painter.circle_stroke(sp, r, egui::Stroke::new(1.0, egui::Color32::BLACK));
                }
            }

            // Player dot at canvas centre.
            painter.circle_filled(center, 5.0, egui::Color32::from_rgb(255, 60, 60));
            painter.circle_stroke(center, 5.0, egui::Stroke::new(1.5, egui::Color32::WHITE));

            // Facing direction arrow derived from Transform rotation.
            // -Z is Bevy's default local forward; project onto the world XY plane.
            let fwd = transform.rotation * Vec3::NEG_Z;
            let dir = Vec2::new(fwd.x, fwd.y);
            if dir.length_squared() > 0.01 {
                let dir_norm = dir.normalize();
                let arrow_end = egui::pos2(
                    center.x + dir_norm.x * 12.0,
                    center.y - dir_norm.y * 12.0,
                );
                painter.line_segment(
                    [center, arrow_end],
                    egui::Stroke::new(2.0, egui::Color32::WHITE),
                );
            }

            // Minimap border.
            painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(80, 80, 80)), egui::StrokeKind::Inside);

            // Coordinates and nearby settlement below the canvas.
            ui.label(egui::RichText::new(format!("X {:.0}  Y {:.0}", px, py)).small());
            if let Some(setts) = settlements {
                if let Some(near) = nearest_within(Vec2::new(px, py), &setts, NEAR_RADIUS) {
                    ui.label(
                        egui::RichText::new(format!("Near: {}", near.name))
                            .small()
                            .color(egui::Color32::from_rgb(255, 220, 120)),
                    );
                }
            }
        });

    Ok(())
}

/// Pick a faction sphere colour based on player standing with any faction whose
/// name overlaps the settlement name. Falls back to a neutral blue-grey.
fn faction_sphere_color(
    settlement_name: &str,
    standings: Option<&PlayerStandings>,
) -> egui::Color32 {
    if let Some(s) = standings {
        for (faction, score) in &s.standings {
            if settlement_name.contains(faction.as_str())
                || faction.contains(settlement_name)
            {
                return match standing_tier(*score) {
                    StandingTier::Exalted | StandingTier::Honored =>
                        egui::Color32::from_rgb(100, 220, 100),
                    StandingTier::Friendly | StandingTier::Neutral =>
                        egui::Color32::from_rgb(150, 150, 200),
                    StandingTier::Unfriendly =>
                        egui::Color32::from_rgb(230, 180, 80),
                    StandingTier::Hostile | StandingTier::Hated =>
                        egui::Color32::from_rgb(220, 60, 60),
                };
            }
        }
    }
    egui::Color32::from_rgb(120, 120, 180)
}
