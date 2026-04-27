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
        zone::{ZoneMembership, ZoneRegistry, OVERWORLD_ZONE, WORLD_SUNKEN_REALM},
    },
};

use crate::{LocalPlayer, PredictedPosition};
use crate::plugins::camera::OrbitCamera;
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

/// Cached terrain texture ids, one per world.
#[derive(Resource, Default)]
struct TerrainTex {
    /// Surface world (WorldId 0) terrain texture.
    surface: Option<egui::TextureId>,
    /// Sunken Realm world (WorldId 1) terrain texture (cave tile colours).
    sunken_realm: Option<egui::TextureId>,
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapWindow>()
            .init_resource::<TerrainTex>()
            .add_systems(Update, (build_terrain_texture, toggle_map))
            .add_systems(EguiPrimaryContextPass, (draw_map, draw_minimap));
    }
}

// ── Toggle ────────────────────────────────────────────────────────────────────

fn toggle_map(
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    mut map_win: ResMut<MapWindow>,
    player_q: Query<&PredictedPosition, With<LocalPlayer>>,
    console: Option<Res<DebugConsole>>,
    pause_menu: Option<Res<PauseMenu>>,
) {
    let Some(kb) = keyboard else { return };
    let blocked = console.is_some_and(|c| c.open) || pause_menu.is_some_and(|m| m.open);
    if !blocked && (kb.just_pressed(KeyCode::KeyM) || kb.just_pressed(KeyCode::Tab)) {
        map_win.open = !map_win.open;
        if map_win.open {
            if let Ok(pos) = player_q.single() {
                map_win.pan_x = pos.x;
                map_win.pan_y = pos.y;
                map_win.zoom = ZOOM_DEFAULT;
            }
        }
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
        TileKind::CaveFloor                                    => [ 64,  64,  64, 255],
        TileKind::CaveWall                                     => [ 26,  26,  31, 255],
        TileKind::CrystalCave                                  => [ 51, 179, 204, 255],
        TileKind::LavaFloor                                    => [230,  77,  13, 255],
        TileKind::CaveRiver                                    => [ 13,  38, 153, 255],
        TileKind::CavePortal                                   => [204,  26, 230, 255],
        TileKind::Void                                         => [  8,   8,  16, 255],
    }
}

/// Generates and registers the 512×512 terrain textures once `WorldMap` exists.
/// Y-flipped so texture V=0 = north (high world-Y), V=1 = south.
///
/// Produces two textures:
/// - `surface`: samples the topmost layer (surface biomes).
/// - `sunken_realm`: cave-palette placeholder using cave tile colours.
fn build_terrain_texture(
    world_map: Option<Res<WorldMap>>,
    mut tex: ResMut<TerrainTex>,
    mut images: ResMut<Assets<Image>>,
    mut user_textures: ResMut<EguiUserTextures>,
) {
    if tex.surface.is_some() && tex.sunken_realm.is_some() {
        return;
    }
    let Some(map) = world_map else { return };

    // ── Surface texture ───────────────────────────────────────────────────────
    if tex.surface.is_none() {
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
        tex.surface = Some(user_textures.add_image(EguiTextureHandle::Strong(handle)));
    }

    // ── Sunken Realm texture ──────────────────────────────────────────────────
    // Generate a cave-palette texture by sampling only cave tile kinds from the
    // surface WorldMap. Tiles whose topmost layer is a surface biome are rendered
    // as CaveFloor (grey), while cave-specific tiles keep their colours.
    if tex.sunken_realm.is_none() {
        let cave_kinds = [
            TileKind::CaveFloor,
            TileKind::CaveWall,
            TileKind::CrystalCave,
            TileKind::LavaFloor,
            TileKind::CaveRiver,
            TileKind::CavePortal,
            TileKind::Void,
        ];
        let mut data = vec![0u8; TEX_SIZE * TEX_SIZE * 4];
        for ty in 0..TEX_SIZE {
            for tx in 0..TEX_SIZE {
                let ix = (tx * 2).min(map.width.saturating_sub(1));
                let iy = (map.height.saturating_sub(1)).saturating_sub(ty * 2);
                let kind = map
                    .column(ix, iy)
                    .layers
                    .last()
                    .map(|l| l.kind)
                    .unwrap_or(TileKind::Void);
                // If the tile is already a cave kind, use its colour directly;
                // otherwise render as generic cave floor.
                let cave_kind = if cave_kinds.contains(&kind) {
                    kind
                } else {
                    TileKind::CaveFloor
                };
                let color = tile_color(cave_kind);
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
        tex.sunken_realm = Some(user_textures.add_image(EguiTextureHandle::Strong(handle)));
    }
}

// ── Map window ────────────────────────────────────────────────────────────────

fn draw_map(
    mut ctx: EguiContexts,
    mut map_win: ResMut<MapWindow>,
    tex: Res<TerrainTex>,
    settlements: Option<Res<Settlements>>,
    player_q: Query<(&PredictedPosition, Option<&PlayerStandings>), With<LocalPlayer>>,
) -> Result {
    if !map_win.open {
        return Ok(());
    }
    let Some(terrain_id) = tex.surface else { return Ok(()) };

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
                            SettlementKind::PeacefulSanctuary => (3.0, egui::Color32::from_rgb(180, 230, 200)),
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
/// The map rotates so the player's forward direction is always at the top.
/// Selects the surface or Sunken Realm texture based on the player's world_id.
#[allow(clippy::too_many_arguments)]
fn draw_minimap(
    mut ctx: EguiContexts,
    tex: Res<TerrainTex>,
    player_q: Query<(&PredictedPosition, Option<&ZoneMembership>), With<LocalPlayer>>,
    camera_q: Query<&OrbitCamera>,
    settlements: Option<Res<Settlements>>,
    console: Option<Res<DebugConsole>>,
    pause_menu: Option<Res<PauseMenu>>,
    zone_registry: Option<Res<ZoneRegistry>>,
) -> Result {
    // Hide behind other overlays.
    if console.is_some_and(|c| c.open) || pause_menu.is_some_and(|m| m.open) {
        return Ok(());
    }
    let Ok((pos, zone_membership)) = player_q.single() else { return Ok(()) };

    // Select terrain texture based on which world the player is in.
    // Uses world_id from ZoneMembership → ZoneRegistry; falls back to z-check.
    let is_underground = if let (Some(registry), Some(membership)) = (&zone_registry, zone_membership) {
        registry.get(membership.0)
            .map(|z| z.world_id == WORLD_SUNKEN_REALM)
            .unwrap_or(false)
    } else {
        // Fallback: check for non-overworld zone membership or z-coordinate
        zone_membership
            .map(|z| z.0 != OVERWORLD_ZONE)
            .unwrap_or(false)
            || pos.z < -1.0
    };

    let terrain_id = if is_underground {
        tex.sunken_realm.or(tex.surface)
    } else {
        tex.surface
    };
    let Some(terrain_id) = terrain_id else { return Ok(()) };

    let px = pos.x;
    let py = pos.y;

    // Camera yaw: 0 = looking from +Z, increases CCW. Forward in world = (-sin_yaw, -cos_yaw).
    let yaw = camera_q.iter().next().map(|c| c.yaw).unwrap_or(0.0);
    let (sin_yaw, cos_yaw) = yaw.sin_cos();

    // Converts a world-space offset (dx, dy) from the player to canvas-space delta,
    // rotating so the player's forward direction points to canvas "up".
    let world_to_canvas = |dx: f32, dy: f32| -> egui::Vec2 {
        let cx = (-dx * cos_yaw + dy * sin_yaw) * MINI_ZOOM;
        let cy = (dx * sin_yaw + dy * cos_yaw) * MINI_ZOOM;
        egui::vec2(cx, cy)
    };

    // Visible world radius from canvas centre to edge.
    let vis_radius = (MINI_CANVAS / 2.0) / MINI_ZOOM;

    egui::Window::new("##minimap")
        .anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0])
        .resizable(false)
        .title_bar(false)
        .show(ctx.ctx_mut()?, |ui| {
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(MINI_CANVAS, MINI_CANVAS), egui::Sense::hover());
            let rect = resp.rect;
            let center = rect.center();

            // Dark fill behind terrain.
            painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(8, 8, 16));

            // Terrain rendered as a rotated quad mesh so it follows the camera yaw.
            // For each canvas corner, compute the world position and then its UV.
            {
                let half = MINI_CANVAS / 2.0;
                let corners: [(f32, f32); 4] = [
                    (-half, -half), // TL
                    ( half, -half), // TR
                    ( half,  half), // BR
                    (-half,  half), // BL
                ];
                let mut mesh = egui::epaint::Mesh::with_texture(terrain_id);
                for (cpx, cpy) in corners {
                    // Canvas delta → world delta (inverse rotation by yaw+π).
                    let cdx = cpx / MINI_ZOOM;
                    let cdy = -cpy / MINI_ZOOM;
                    let world_dx = -cdx * cos_yaw - cdy * sin_yaw;
                    let world_dy = cdx * sin_yaw - cdy * cos_yaw;
                    let wx = px + world_dx;
                    let wy = py + world_dy;
                    let u = ((wx + MAP_W / 2.0) / MAP_W).clamp(0.0, 1.0);
                    let v = ((MAP_H / 2.0 - wy) / MAP_H).clamp(0.0, 1.0);
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: center + egui::vec2(cpx, cpy),
                        uv: egui::pos2(u, v),
                        color: egui::Color32::WHITE,
                    });
                }
                mesh.indices = vec![0, 1, 2, 0, 2, 3];
                painter.add(egui::Shape::Mesh(mesh.into()));
            }

            // Settlement dots rotated to match minimap orientation.
            if let Some(ref setts) = settlements {
                for s in &setts.0 {
                    let wx = s.x - MAP_W / 2.0;
                    let wy = s.y - MAP_H / 2.0;
                    let dx = wx - px;
                    let dy = wy - py;
                    if dx.abs() > vis_radius * 1.5 || dy.abs() > vis_radius * 1.5 {
                        continue;
                    }
                    let cdelta = world_to_canvas(dx, dy);
                    let sp = center + cdelta;
                    if !rect.contains(sp) {
                        continue;
                    }
                    let (r, fill) = match s.kind {
                        SettlementKind::Capital => (4.0, egui::Color32::from_rgb(255, 220, 60)),
                        SettlementKind::Town    => (3.0, egui::Color32::from_rgb(220, 220, 220)),
                        SettlementKind::PeacefulSanctuary => (3.0, egui::Color32::from_rgb(180, 230, 200)),
                    };
                    painter.circle_filled(sp, r, fill);
                    painter.circle_stroke(sp, r, egui::Stroke::new(1.0, egui::Color32::BLACK));
                }
            }

            // Player dot at canvas centre.
            painter.circle_filled(center, 5.0, egui::Color32::from_rgb(255, 60, 60));
            painter.circle_stroke(center, 5.0, egui::Stroke::new(1.5, egui::Color32::WHITE));

            // Forward arrow always points up since the map rotates with the player.
            let arrow_end = center - egui::vec2(0.0, 12.0);
            painter.line_segment(
                [center, arrow_end],
                egui::Stroke::new(2.0, egui::Color32::WHITE),
            );

            // North indicator: small "N" label at the top of the rotated map.
            // Rotated north direction on canvas.
            let north_canvas = world_to_canvas(0.0, 1.0).normalized() * (MINI_CANVAS / 2.0 - 8.0);
            painter.text(
                center + north_canvas,
                egui::Align2::CENTER_CENTER,
                "N",
                egui::FontId::proportional(10.0),
                egui::Color32::from_rgb(200, 200, 255),
            );

            // Minimap border.
            painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(80, 80, 80)), egui::StrokeKind::Inside);

            // Coordinates, world label, and nearby settlement below the canvas.
            ui.label(egui::RichText::new(format!("X {:.0}  Y {:.0}", px, py)).small());
            {
                let world_label = if is_underground { "Sunken Realm" } else { "Surface" };
                let world_color = if is_underground {
                    egui::Color32::from_rgb(130, 100, 200)
                } else {
                    egui::Color32::from_rgb(120, 200, 120)
                };
                ui.label(egui::RichText::new(world_label).small().color(world_color));
            }
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
