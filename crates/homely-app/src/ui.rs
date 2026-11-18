//! `egui_dock` layout, modifier-driven 2D floorplan editor, context menu, and inspector.

use crate::i18n::{tr, Lang};
use crate::state::{
    corner, BuildingDragState, ContextMenuKind, ContextMenuState, DragAnchorState, DragHandleKind,
    EditorState, ProjectDialogState, ToolMode,
};
use crate::theme::{self, apply_native_theme, dock_style, load_logo_texture};
use bevy::app::AppExit;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use homely_core::command::ProjectCommand;
use homely_core::geometry::{
    compute_level_wall_offsets, project_point_onto_polyline_t, sample_polyline_at_t,
    tessellate_wall_centerline,
};
use homely_core::rooms::{
    collect_connected_building_walls, detect_rooms, point_in_polygon, room_signature,
    triangulate_polygon,
};
use homely_core::sh3x::{load_sh3x, save_sh3x};
use homely_core::{
    BuildingPreset, CatalogCategory, CatalogItem, FloorZone, Opening, OpeningType, PaintStyle,
    PointType, SurfaceFinish, SurfaceTexture, TerrainTexture, UnitSystem, Wall, WallAnchorPoint,
    METERS_PER_FOOT,
};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockTab {
    Floorplan2D,
    Inspector,
    History,
}

#[derive(Resource)]
pub struct DockLayout {
    pub state: DockState<DockTab>,
}

impl Default for DockLayout {
    fn default() -> Self {
        let mut dock = DockState::new(vec![DockTab::Floorplan2D]);
        dock.main_surface_mut()
            .split_right(NodeIndex::root(), 0.70, vec![DockTab::Inspector, DockTab::History]);
        Self { state: dock }
    }
}

struct HomelyTabViewer<'a> {
    editor: &'a mut EditorState,
}

impl TabViewer for HomelyTabViewer<'_> {
    type Tab = DockTab;

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(*tab as u8)
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        let s = tr(self.editor.lang);
        match tab {
            DockTab::Floorplan2D => s.tab_2d.into(),
            DockTab::Inspector => s.tab_inspector.into(),
            DockTab::History => s.tab_history.into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            DockTab::Floorplan2D => draw_2d_floorplan(ui, self.editor),
            DockTab::Inspector => draw_inspector(ui, self.editor),
            DockTab::History => draw_history_panel(ui, self.editor),
        }
    }
}

pub fn draw_dock_ui(
    mut contexts: EguiContexts,
    mut editor: ResMut<EditorState>,
    mut dock: ResMut<DockLayout>,
    mut app_exit: MessageWriter<AppExit>,
    mut theme_applied: Local<bool>,
    mut logo_tex: Local<Option<egui::TextureHandle>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !*theme_applied {
        apply_native_theme(ctx);
        *logo_tex = Some(load_logo_texture(ctx));
        *theme_applied = true;
    }
    let s = tr(editor.lang);

    let wants_kbd = ctx.egui_wants_keyboard_input();
    let (undo_key, redo_key, delete_key, esc_key) = ctx.input(|i| {
        let ctrl = i.modifiers.ctrl || i.modifiers.command;
        let z = i.key_pressed(egui::Key::Z);
        let y = i.key_pressed(egui::Key::Y);
        let del = i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace);
        (
            ctrl && z && !i.modifiers.shift,
            ctrl && (y || (z && i.modifiers.shift)),
            del && !wants_kbd,
            i.key_pressed(egui::Key::Escape),
        )
    });
    if undo_key {
        editor.undo();
    } else if redo_key {
        editor.redo();
    } else if delete_key {
        if let Some(op_id) = editor.selected_opening_id.clone() {
            editor.execute_cmd(ProjectCommand::RemoveOpening { opening_id: op_id });
            editor.status = "Removed opening.".into();
        } else {
            delete_selected_point(&mut editor);
        }
    } else if esc_key {
        editor.active_draw_from = None;
        editor.floor_zone_points.clear();
        editor.context_menu = None;
    }

    let mut root_ui = egui::Ui::new(
        ctx.clone(),
        "homely_root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    // Classic flat top menu bar (app icon lives in the OS window titlebar)
    egui::Panel::top("menu_bar")
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_CANVAS)
                .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
                .inner_margin(egui::Margin::symmetric(10, 4)),
        )
        .show(&mut root_ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(s.menu_file, |ui| {
                    if ui.button(format!("{}...", s.menu_new)).clicked() {
                        editor.project_dialog = Some(ProjectDialogState {
                            is_new: true,
                            name: "Homely 3D Project".into(),
                            path: editor.project_path.clone(),
                            unit_system: editor.unit_system,
                            building_preset: editor.building_preset,
                            plan_width: editor.plan_width,
                            plan_height: editor.plan_height,
                            wall_thickness: editor.wall_thickness,
                            wall_height: editor.wall_height,
                        });
                        ui.close();
                    }
                    if ui.button(s.menu_project_settings).clicked() {
                        editor.project_dialog = Some(ProjectDialogState {
                            is_new: false,
                            name: editor.project.name.clone(),
                            path: editor.project_path.clone(),
                            unit_system: editor.unit_system,
                            building_preset: editor.building_preset,
                            plan_width: editor.plan_width,
                            plan_height: editor.plan_height,
                            wall_thickness: editor.wall_thickness,
                            wall_height: editor.wall_height,
                        });
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(s.menu_open_h3d).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Homely 3D", &["h3d"])
                            .pick_file()
                        {
                            let p = path.to_string_lossy().to_string();
                            editor.project_path = p.clone();
                            load_project_from(&mut editor, &p);
                        }
                        ui.close();
                    }
                    if ui.button(s.menu_save_h3d).clicked() {
                        let p = editor.project_path.clone();
                        save_project_to(&mut editor, &p);
                        ui.close();
                    }
                    if ui.button(s.menu_save_as).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Homely 3D", &["h3d"])
                            .set_file_name(&editor.project_path)
                            .save_file()
                        {
                            let p = path.to_string_lossy().to_string();
                            editor.project_path = p.clone();
                            save_project_to(&mut editor, &p);
                        }
                        ui.close();
                    }
                    if ui.button(s.menu_import_sh3x).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("SH3X Archive", &["sh3x"])
                            .pick_file()
                        {
                            let p = path.to_string_lossy().to_string();
                            editor.import_sh3x_path = p.clone();
                            load_project_from(&mut editor, &p);
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(s.menu_quit).clicked() {
                        app_exit.write(AppExit::Success);
                        ui.close();
                    }
                });

                ui.menu_button(s.menu_edit, |ui| {
                    if ui
                        .add_enabled(editor.can_undo(), egui::Button::new(format!("{}  (Ctrl+Z)", s.menu_undo)))
                        .clicked()
                    {
                        editor.undo();
                        ui.close();
                    }
                    if ui
                        .add_enabled(editor.can_redo(), egui::Button::new(format!("{}  (Ctrl+Y)", s.menu_redo)))
                        .clicked()
                    {
                        editor.redo();
                        ui.close();
                    }
                });

                ui.menu_button(s.menu_view, |ui| {
                    if ui.button(s.menu_reset_2d).clicked() {
                        editor.pan_2d = egui::Vec2::ZERO;
                        editor.zoom_2d = 45.0;
                        ui.close();
                    }
                    if ui.button(s.menu_reset_3d).clicked() {
                        editor.reset_3d_camera = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.checkbox(&mut editor.show_3d_viewer, s.menu_toggle_3d).clicked() {
                        ui.close();
                    }
                });

                ui.menu_button(s.menu_language, |ui| {
                    for lang in [Lang::De, Lang::En] {
                        if ui.selectable_label(editor.lang == lang, lang.label()).clicked() {
                            editor.lang = lang;
                            ui.close();
                        }
                    }
                });
            });
        });

    // Left Tool Palette (slim native rail with vector icons & catalog)
    egui::Panel::left("left_tools")
        .exact_size(172.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
                .inner_margin(egui::Margin::symmetric(8, 8)),
        )
        .show(&mut root_ui, |ui| {
            ui.label(
                egui::RichText::new(s.tools)
                    .strong()
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(4.0);

            if tool_button(
                ui,
                ToolIcon::SelectArrow,
                s.tool_select,
                editor.tool_mode == ToolMode::Select,
                true,
            )
            .clicked()
            {
                editor.tool_mode = ToolMode::Select;
                editor.active_draw_from = None;
            }
            ui.add_space(3.0);

            if tool_button(
                ui,
                ToolIcon::DrawPen,
                s.tool_draw_wall,
                editor.tool_mode == ToolMode::DrawWallPoint,
                true,
            )
            .on_hover_text("Ctrl + Click")
            .clicked()
            {
                editor.tool_mode = ToolMode::DrawWallPoint;
            }
            ui.add_space(3.0);

            if tool_button(
                ui,
                ToolIcon::MarqueeRect,
                s.tool_marquee,
                editor.tool_mode == ToolMode::MarqueeSelect,
                true,
            )
            .on_hover_text("Alt + Drag")
            .clicked()
            {
                editor.tool_mode = ToolMode::MarqueeSelect;
                editor.active_draw_from = None;
            }
            ui.add_space(3.0);

            if tool_button(
                ui,
                ToolIcon::FloorRect,
                s.tool_floor_zone,
                editor.tool_mode == ToolMode::DrawFloorZone,
                true,
            )
            .clicked()
            {
                editor.tool_mode = ToolMode::DrawFloorZone;
                editor.active_draw_from = None;
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            let can_delete_point =
                editor.selected_wall_id.is_some() && editor.selected_anchor_index.is_some();
            if tool_button(ui, ToolIcon::Trash, s.delete_point, false, can_delete_point)
                .on_hover_text("Del / Backspace")
                .clicked()
            {
                delete_selected_point(&mut editor);
            }

            if editor.active_draw_from.is_some() {
                ui.add_space(4.0);
                if tool_button(ui, ToolIcon::Cross, s.stop_drawing, false, true)
                    .on_hover_text("Esc")
                    .clicked()
                {
                    editor.active_draw_from = None;
                }
            }

            ui.add_space(10.0);
            ui.separator();
            ui.label(
                egui::RichText::new(s.catalog_title)
                    .strong()
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
            );
            ui.label(
                egui::RichText::new(s.catalog_hint)
                    .size(10.5)
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(4.0);
            let catalog_items = editor.catalog.clone();
            for item in catalog_items {
                let label = match editor.lang {
                    Lang::De => &item.name_de,
                    Lang::En => &item.name_en,
                };
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(154.0, 28.0),
                    egui::Sense::click_and_drag(),
                );
                if ui.is_rect_visible(rect) {
                    let hovered = resp.hovered();
                    let fill = if hovered {
                        theme::BG_HOVER
                    } else {
                        theme::BG_ELEVATED
                    };
                    let stroke = egui::Stroke::new(
                        1.0,
                        if hovered {
                            theme::ACCENT_BLUE
                        } else {
                            theme::BORDER_SUBTLE
                        },
                    );
                    ui.painter().rect(rect, 5.0, fill, stroke, egui::StrokeKind::Inside);

                    let icon_rect = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + 7.0, rect.center().y - 8.0),
                        egui::vec2(16.0, 16.0),
                    );
                    draw_catalog_icon(ui.painter(), icon_rect, &item);

                    ui.painter().text(
                        egui::pos2(icon_rect.right() + 7.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::proportional(11.5),
                        theme::TEXT_PRIMARY,
                    );
                }
                if resp.drag_started() {
                    editor.dragging_catalog_item = Some(item);
                }
            }

            ui.add_space(10.0);
            ui.separator();
            ui.label(
                egui::RichText::new(s.hint_ctrl)
                    .size(11.0)
                    .color(egui::Color32::from_gray(170)),
            );
        });

    egui::Panel::bottom("status_bar")
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_CANVAS)
                .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
                .inner_margin(egui::Margin::symmetric(10, 3)),
        )
        .show(&mut root_ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&editor.status)
                        .size(11.5)
                        .color(theme::TEXT_MUTED),
                );
            });
        });

    egui::CentralPanel::no_frame().show(&mut root_ui, |ui| {
        let mut viewer = HomelyTabViewer {
            editor: &mut editor,
        };
        DockArea::new(&mut dock.state)
            .show_close_buttons(false)
            .style(dock_style(ui.style().as_ref()))
            .show_inside(ui, &mut viewer);
    });

    draw_context_menu(ctx, &mut editor);
    draw_project_dialog(ctx, &mut editor);

    // Floating ghost badge while dragging a catalog Door/Window onto a wall
    if let Some(ref item) = editor.dragging_catalog_item {
        if let Some(pos) = ctx.pointer_latest_pos() {
            let label = match editor.lang {
                Lang::De => item.name_de.clone(),
                Lang::En => item.name_en.clone(),
            };
            egui::Area::new(egui::Id::new("catalog_drag_ghost"))
                .order(egui::Order::Tooltip)
                .interactable(false)
                .fixed_pos(pos + egui::vec2(14.0, 14.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(label);
                    });
                });
        }
    }

    Ok(())
}

fn draw_2d_floorplan(ui: &mut egui::Ui, editor: &mut EditorState) {
    let full_rect = ui.available_rect_before_wrap();
    const RULER_LEFT: f32 = 34.0;
    const RULER_TOP: f32 = 22.0;

    let canvas_rect = egui::Rect::from_min_max(
        egui::pos2(
            (full_rect.left() + RULER_LEFT).min(full_rect.right()),
            (full_rect.top() + RULER_TOP).min(full_rect.bottom()),
        ),
        full_rect.max,
    );

    let response = ui.allocate_rect(canvas_rect, egui::Sense::click_and_drag());
    editor.last_canvas_size = canvas_rect.size();
    let painter = ui.painter_at(canvas_rect);

    painter.rect_filled(canvas_rect, 0.0, egui::Color32::from_rgb(18, 20, 24));

    if response.dragged_by(egui::PointerButton::Middle) {
        editor.pan_2d += response.drag_delta();
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            editor.zoom_2d = (editor.zoom_2d * (1.0 + scroll * 0.0015)).clamp(8.0, 200.0);
        }
    }

    // Flush top-left origin (0, 0) with zero artificial outer padding
    let origin = canvas_rect.min + editor.pan_2d;
    let zoom = editor.zoom_2d;

    let world_to_screen = |wx: f64, wy: f64| -> egui::Pos2 {
        egui::pos2(origin.x + (wx as f32) * zoom, origin.y + (wy as f32) * zoom)
    };
    let screen_to_world = |pos: egui::Pos2| -> (f64, f64) {
        (
            f64::from((pos.x - origin.x) / zoom),
            f64::from((pos.y - origin.y) / zoom),
        )
    };

    let plan_rect = egui::Rect::from_min_max(
        world_to_screen(0.0, 0.0),
        world_to_screen(editor.plan_width, editor.plan_height),
    );
    painter.rect_filled(plan_rect, 0.0, egui::Color32::from_rgb(28, 32, 38));

    let major_step_m = match editor.unit_system {
        UnitSystem::Metric => 1.0_f64,
        UnitSystem::Imperial => METERS_PER_FOOT,
    };
    let sub_divs: i32 = match editor.unit_system {
        UnitSystem::Metric => 10,   // 10 cm
        UnitSystem::Imperial => 4,  // 3 in (0.25 ft)
    };
    let max_mx = (editor.plan_width / major_step_m).floor() as i32;
    let max_my = (editor.plan_height / major_step_m).floor() as i32;

    if zoom * (major_step_m as f32) >= 28.0 {
        let minor_color = egui::Color32::from_rgb(36, 41, 48);
        let minor_step_m = major_step_m / (sub_divs as f64);
        let max_dx = (editor.plan_width / minor_step_m).round() as i32;
        for dx in 0..=max_dx {
            if dx % sub_divs == 0 {
                continue;
            }
            let x = origin.x + ((dx as f64 * minor_step_m) as f32) * zoom;
            painter.line_segment(
                [egui::pos2(x, plan_rect.top()), egui::pos2(x, plan_rect.bottom())],
                egui::Stroke::new(1.0, minor_color),
            );
        }
        let max_dy = (editor.plan_height / minor_step_m).round() as i32;
        for dy in 0..=max_dy {
            if dy % sub_divs == 0 {
                continue;
            }
            let y = origin.y + ((dy as f64 * minor_step_m) as f32) * zoom;
            painter.line_segment(
                [egui::pos2(plan_rect.left(), y), egui::pos2(plan_rect.right(), y)],
                egui::Stroke::new(1.0, minor_color),
            );
        }
    }

    let grid_color = egui::Color32::from_rgb(50, 56, 66);
    for gx in 0..=max_mx {
        let x = origin.x + ((gx as f64 * major_step_m) as f32) * zoom;
        let color = if gx == 0 {
            egui::Color32::from_rgba_unmultiplied(110, 200, 120, 180)
        } else {
            grid_color
        };
        painter.line_segment(
            [egui::pos2(x, plan_rect.top()), egui::pos2(x, plan_rect.bottom())],
            egui::Stroke::new(if gx == 0 { 1.5 } else { 1.0 }, color),
        );
    }
    for gy in 0..=max_my {
        let y = origin.y + ((gy as f64 * major_step_m) as f32) * zoom;
        let color = if gy == 0 {
            egui::Color32::from_rgba_unmultiplied(220, 100, 100, 180)
        } else {
            grid_color
        };
        painter.line_segment(
            [egui::pos2(plan_rect.left(), y), egui::pos2(plan_rect.right(), y)],
            egui::Stroke::new(if gy == 0 { 1.5 } else { 1.0 }, color),
        );
    }

    // Detected rooms on active level (filled using ear-clipping triangulation + m² label)
    let level_walls: Vec<Wall> = editor
        .project
        .walls
        .iter()
        .filter(|w| w.level_id == editor.active_level_id)
        .cloned()
        .collect();
    let level_height = editor
        .project
        .levels
        .iter()
        .find(|l| l.id == editor.active_level_id)
        .map(|l| l.height)
        .unwrap_or(2.6);

    let rooms = detect_rooms(&level_walls, level_height);
    for (r_idx, room) in rooms.iter().enumerate() {
        let tris = triangulate_polygon(&room.polygon);
        if !tris.is_empty() {
            let mut mesh = egui::Mesh::default();
            let room_color = egui::Color32::from_rgba_unmultiplied(120, 165, 135, 55);
            for p in &room.polygon {
                mesh.colored_vertex(world_to_screen(p.x, p.y), room_color);
            }
            for tri in tris {
                mesh.add_triangle(tri[0] as u32, tri[1] as u32, tri[2] as u32);
            }
            painter.add(egui::Shape::mesh(mesh));
        }
        let cp = world_to_screen(room.centroid.x, room.centroid.y);
        let sig = room_signature(&room.polygon);
        let default_name = match editor.lang {
            Lang::De => format!("Raum {}", r_idx + 1),
            Lang::En => format!("Room {}", r_idx + 1),
        };
        let r_name = editor
            .project
            .room_names
            .get(&sig)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or(default_name);
        let area_disp = editor.unit_system.sqm_to_display_area(room.area_sqm);
        let area_unit = editor.unit_system.area_suffix();
        painter.text(
            cp,
            egui::Align2::CENTER_CENTER,
            format!("{r_name}\n{area_disp:.2}{area_unit}"),
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgba_unmultiplied(220, 235, 225, 220),
        );
    }

    // Existing walls (neighbor-aware mitered corners)
    // Custom polygonal floor zones on active level
    for zone in editor
        .project
        .floor_zones
        .iter()
        .filter(|z| z.level_id == editor.active_level_id && z.points.len() >= 3)
    {
        let poly: Vec<homely_core::geometry::Vec2> = zone
            .points
            .iter()
            .map(|p| homely_core::geometry::Vec2::new(p[0], p[1]))
            .collect();
        let is_z_sel = editor.selected_floor_zone_id.as_deref() == Some(&zone.id);
        let zfill = egui::Color32::from_rgba_unmultiplied(
            zone.color[0],
            zone.color[1],
            zone.color[2],
            if is_z_sel { 130 } else { 85 },
        );
        let tris = triangulate_polygon(&poly);
        if !tris.is_empty() {
            let mut mesh = egui::Mesh::default();
            for p in &poly {
                mesh.colored_vertex(world_to_screen(p.x, p.y), zfill);
            }
            for tri in tris {
                mesh.add_triangle(tri[0] as u32, tri[1] as u32, tri[2] as u32);
            }
            painter.add(egui::Shape::mesh(mesh));
        }
        let screen_pts: Vec<egui::Pos2> =
            poly.iter().map(|p| world_to_screen(p.x, p.y)).collect();
        let zstroke = if is_z_sel {
            egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 200, 255))
        } else {
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(220, 220, 220, 140))
        };
        painter.add(egui::Shape::closed_line(screen_pts.clone(), zstroke));
        if is_z_sel {
            for sp in &screen_pts {
                painter.circle_filled(*sp, 5.0, egui::Color32::from_rgb(100, 200, 255));
            }
        }
        let cx = poly.iter().map(|p| p.x).sum::<f64>() / poly.len() as f64;
        let cy = poly.iter().map(|p| p.y).sum::<f64>() / poly.len() as f64;
        painter.text(
            world_to_screen(cx, cy),
            egui::Align2::CENTER_CENTER,
            &zone.name,
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgba_unmultiplied(240, 244, 250, 220),
        );
    }

    let all_pairs = compute_level_wall_offsets(&level_walls, 0.01);
    for (wall, pairs) in level_walls.iter().zip(all_pairs.iter()) {
        let is_selected = editor.selected_wall_id.as_deref() == Some(&wall.id);
        let is_group_selected = editor.selected_group_wall_ids.contains(&wall.id);
        let fill = if is_selected {
            egui::Color32::from_rgba_unmultiplied(100, 175, 245, 175)
        } else if is_group_selected {
            egui::Color32::from_rgba_unmultiplied(90, 200, 255, 195)
        } else {
            egui::Color32::from_rgba_unmultiplied(210, 208, 200, 175)
        };

        for win in pairs.windows(2) {
            let (l0, r0) = win[0];
            let (l1, r1) = win[1];
            let quad = vec![
                world_to_screen(l0.x, l0.y),
                world_to_screen(l1.x, l1.y),
                world_to_screen(r1.x, r1.y),
                world_to_screen(r0.x, r0.y),
            ];
            painter.add(egui::Shape::convex_polygon(
                quad,
                fill,
                egui::Stroke::new(1.0, egui::Color32::from_gray(235)),
            ));
        }

        for (idx, a) in wall.anchors.iter().enumerate() {
            let ap = world_to_screen(a.pos_x, a.pos_y);
            let is_anchor_selected = is_selected && editor.selected_anchor_index == Some(idx);

            if is_selected {
                if let (Some(ix), Some(iy)) = (a.handle_in_x, a.handle_in_y) {
                    let hp = world_to_screen(ix, iy);
                    painter.line_segment(
                        [ap, hp],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(220, 130, 230)),
                    );
                    painter.circle_filled(hp, 4.5, egui::Color32::from_rgb(220, 130, 230));
                }
                if let (Some(ox), Some(oy)) = (a.handle_out_x, a.handle_out_y) {
                    let hp = world_to_screen(ox, oy);
                    painter.line_segment(
                        [ap, hp],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(220, 130, 230)),
                    );
                    painter.circle_filled(hp, 4.5, egui::Color32::from_rgb(220, 130, 230));
                }
            }

            let color = if is_anchor_selected {
                egui::Color32::from_rgb(255, 140, 50)
            } else if is_selected {
                egui::Color32::from_rgb(80, 210, 255)
            } else {
                egui::Color32::from_rgb(255, 200, 80)
            };
            painter.circle_filled(ap, if is_anchor_selected { 6.5 } else { 5.0 }, color);

            if is_anchor_selected {
                painter.circle_stroke(ap, 9.5, egui::Stroke::new(2.0, egui::Color32::WHITE));
            }
        }

        // Draw doors and windows on this wall
        let line = tessellate_wall_centerline(wall, 0.02);
        for op in editor.project.openings.iter().filter(|o| o.wall_id == wall.id) {
            let Some((mid, dir)) = sample_polyline_at_t(&line, op.segment_t) else {
                continue;
            };
            let is_op_sel = editor.selected_opening_id.as_deref() == Some(&op.id);
            let perp = homely_core::geometry::Vec2::new(-dir.y, dir.x);
            let hw = op.width * 0.5;
            let ht = wall.thickness * 0.5 + 0.02;

            let p_a = homely_core::geometry::Vec2::new(mid.x - dir.x * hw, mid.y - dir.y * hw);
            let p_b = homely_core::geometry::Vec2::new(mid.x + dir.x * hw, mid.y + dir.y * hw);

            // Clear wall interior behind opening
            let gap_quad = vec![
                world_to_screen(p_a.x + perp.x * ht, p_a.y + perp.y * ht),
                world_to_screen(p_b.x + perp.x * ht, p_b.y + perp.y * ht),
                world_to_screen(p_b.x - perp.x * ht, p_b.y - perp.y * ht),
                world_to_screen(p_a.x - perp.x * ht, p_a.y - perp.y * ht),
            ];
            painter.add(egui::Shape::convex_polygon(
                gap_quad,
                egui::Color32::from_rgb(28, 32, 38),
                egui::Stroke::NONE,
            ));

            let accent = if is_op_sel {
                egui::Color32::from_rgb(80, 210, 255)
            } else if op.opening_type == OpeningType::Door {
                egui::Color32::from_rgb(235, 175, 95)
            } else {
                egui::Color32::from_rgb(135, 205, 250)
            };

            match op.opening_type {
                OpeningType::Door => {
                    // Door leaf + quarter-circle swing arc respecting flip_side & flip_hinge
                    let side = if op.swing_flip_side { -1.0 } else { 1.0 };
                    let (hinge_pt, closed_dir) = if op.swing_flip_hinge {
                        (p_b, homely_core::geometry::Vec2::new(-dir.x, -dir.y))
                    } else {
                        (p_a, dir)
                    };
                    let swing_dir = homely_core::geometry::Vec2::new(perp.x * side, perp.y * side);
                    let hinge = world_to_screen(hinge_pt.x, hinge_pt.y);
                    let swing_tip = world_to_screen(
                        hinge_pt.x + swing_dir.x * op.width,
                        hinge_pt.y + swing_dir.y * op.width,
                    );
                    painter.line_segment([hinge, swing_tip], egui::Stroke::new(2.2, accent));

                    let mut arc_pts = Vec::with_capacity(13);
                    for step in 0..=12 {
                        let ang = (step as f64 / 12.0) * std::f64::consts::FRAC_PI_2;
                        let vx = closed_dir.x * ang.cos() + swing_dir.x * ang.sin();
                        let vy = closed_dir.y * ang.cos() + swing_dir.y * ang.sin();
                        arc_pts.push(world_to_screen(
                            hinge_pt.x + vx * op.width,
                            hinge_pt.y + vy * op.width,
                        ));
                    }
                    painter.add(egui::Shape::line(
                        arc_pts,
                        egui::Stroke::new(1.2, accent),
                    ));
                }
                OpeningType::Window | OpeningType::Cutout => {
                    // Double glass line inside wall opening
                    let off = wall.thickness * 0.2;
                    painter.line_segment(
                        [
                            world_to_screen(p_a.x + perp.x * off, p_a.y + perp.y * off),
                            world_to_screen(p_b.x + perp.x * off, p_b.y + perp.y * off),
                        ],
                        egui::Stroke::new(1.8, accent),
                    );
                    painter.line_segment(
                        [
                            world_to_screen(p_a.x - perp.x * off, p_a.y - perp.y * off),
                            world_to_screen(p_b.x - perp.x * off, p_b.y - perp.y * off),
                        ],
                        egui::Stroke::new(1.8, accent),
                    );
                }
            }
        }
    }

    let pointer_pos = response.hover_pos().filter(|p| canvas_rect.contains(*p));
    let ctrl_held = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);

    // Catalog Drag & Drop preview and placement onto walls
    if let Some(drag_item) = editor.dragging_catalog_item.clone() {
        let wall_hit = pointer_pos.and_then(|pos| {
            let (wx, wy) = screen_to_world(pos);
            find_wall_at_world(wx, wy, editor).map(|(wid, _)| (wid, wx, wy))
        });

        if let Some((ref wid, wx, wy)) = wall_hit {
            if let Some(wall) = editor.project.walls.iter().find(|w| w.id == *wid) {
                let line = tessellate_wall_centerline(wall, 0.02);
                let t = project_point_onto_polyline_t(&line, wx, wy);
                if let Some((mid, dir)) = sample_polyline_at_t(&line, t) {
                    let hw = drag_item.default_width * 0.5;
                    let p0 = world_to_screen(mid.x - dir.x * hw, mid.y - dir.y * hw);
                    let p1 = world_to_screen(mid.x + dir.x * hw, mid.y + dir.y * hw);
                    painter.line_segment([p0, p1], egui::Stroke::new(5.0, egui::Color32::LIGHT_GREEN));
                }
            }
        }

        if !ui.input(|i| i.pointer.primary_down()) {
            if let Some((wid, wx, wy)) = wall_hit {
                insert_catalog_opening_at(editor, &wid, wx, wy, &drag_item);
            }
            editor.dragging_catalog_item = None;
        }
    }

    // Live preview when chaining from `active_draw_from`
    if let Some((fx, fy)) = editor.active_draw_from {
        let fp = world_to_screen(fx, fy);
        painter.circle_stroke(fp, 9.0, egui::Stroke::new(2.0, egui::Color32::LIGHT_GREEN));
        if let Some(pos) = pointer_pos {
            let (wx, wy) = screen_to_world(pos);
            let (sx, sy, is_corner_snap) = snap_point_for_draw(wx, wy, editor);
            let sp = world_to_screen(sx, sy);
            painter.line_segment([fp, sp], egui::Stroke::new(2.0, egui::Color32::LIGHT_GREEN));
            let snap_col = if is_corner_snap {
                egui::Color32::from_rgb(255, 210, 60)
            } else {
                egui::Color32::LIGHT_GREEN
            };
            painter.circle_stroke(sp, if is_corner_snap { 10.0 } else { 6.0 }, egui::Stroke::new(2.0, snap_col));
        }
    }

    if let Some((start_w, cur_w)) = editor.selection_box {
        let rect = egui::Rect::from_two_pos(
            world_to_screen(start_w.0, start_w.1),
            world_to_screen(cur_w.0, cur_w.1),
        );
        painter.rect_filled(
            rect,
            2.0,
            egui::Color32::from_rgba_unmultiplied(70, 150, 255, 45),
        );
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(90, 175, 255)),
            egui::StrokeKind::Inside,
        );
    }

    if !editor.floor_zone_points.is_empty() {
        let pts_screen: Vec<egui::Pos2> = editor
            .floor_zone_points
            .iter()
            .map(|&(x, y)| world_to_screen(x, y))
            .collect();
        let line_col = egui::Color32::from_rgb(240, 185, 95);
        for pair in pts_screen.windows(2) {
            painter.line_segment([pair[0], pair[1]], egui::Stroke::new(2.0, line_col));
        }
        for (i, sp) in pts_screen.iter().enumerate() {
            painter.circle_filled(
                *sp,
                if i == 0 { 6.0 } else { 4.5 },
                if i == 0 {
                    egui::Color32::LIGHT_GREEN
                } else {
                    line_col
                },
            );
        }
        if let Some(pos) = pointer_pos {
            let (wx, wy) = screen_to_world(pos);
            let (sx, sy, _) = snap_point_for_draw(wx, wy, editor);
            if let Some(&last) = pts_screen.last() {
                painter.line_segment(
                    [last, world_to_screen(sx, sy)],
                    egui::Stroke::new(1.5, line_col),
                );
            }
        }
    }

    handle_2d_input(
        ui,
        editor,
        &response,
        pointer_pos,
        ctrl_held,
        &world_to_screen,
        &screen_to_world,
    );

    draw_rulers_and_cursor_coords(
        ui,
        full_rect,
        canvas_rect,
        origin,
        zoom,
        0,
        max_mx,
        0,
        max_my,
        editor.plan_width,
        editor.plan_height,
        editor.unit_system,
        pointer_pos,
        &screen_to_world,
    );
}

fn handle_2d_input(
    ui: &egui::Ui,
    editor: &mut EditorState,
    response: &egui::Response,
    pointer_pos: Option<egui::Pos2>,
    ctrl_held: bool,
    world_to_screen: &impl Fn(f64, f64) -> egui::Pos2,
    screen_to_world: &impl Fn(egui::Pos2) -> (f64, f64),
) {
    let (primary_pressed, primary_down, secondary_clicked, alt_held) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.secondary_clicked(),
            i.modifiers.alt,
        )
    });

    // Right-click exits building/floor-zone drawing if active, otherwise opens context menu on point or wall
    if response.hovered() && secondary_clicked {
        if !editor.floor_zone_points.is_empty() {
            if editor.floor_zone_points.len() >= 3 {
                finish_floor_zone_polygon(editor);
            } else {
                editor.floor_zone_points.clear();
            }
            return;
        }
        if editor.active_draw_from.is_some() {
            editor.active_draw_from = None;
            editor.status = "Building finished.".into();
            return;
        }
        if let Some(pos) = pointer_pos {
            if let Some((wid, idx, _)) = find_anchor_at_pos(editor, pos, world_to_screen) {
                editor.selected_wall_id = Some(wid.clone());
                editor.selected_anchor_index = Some(idx);
                editor.context_menu = Some(ContextMenuState {
                    screen_pos: pos,
                    kind: ContextMenuKind::Point {
                        wall_id: wid,
                        anchor_index: idx,
                    },
                });
                return;
            }
            let (wx, wy) = screen_to_world(pos);
            if let Some(op_id) = find_opening_at_world(wx, wy, editor) {
                editor.selected_opening_id = Some(op_id.clone());
                editor.selected_anchor_index = None;
                editor.context_menu = Some(ContextMenuState {
                    screen_pos: pos,
                    kind: ContextMenuKind::Opening { opening_id: op_id },
                });
                return;
            }
            if let Some((wid, _)) = find_wall_at_world(wx, wy, editor) {
                editor.selected_wall_id = Some(wid.clone());
                editor.selected_anchor_index = None;
                editor.selected_opening_id = None;
                editor.context_menu = Some(ContextMenuState {
                    screen_pos: pos,
                    kind: ContextMenuKind::Wall {
                        wall_id: wid,
                        wx,
                        wy,
                    },
                });
                return;
            }
            editor.context_menu = None;
        }
    }

    if response.hovered() && primary_pressed {
        editor.context_menu = None;
        if let Some(pos) = pointer_pos {
            if editor.active_draw_from.is_some() {
                advance_building_mode(editor, pos, screen_to_world);
                return;
            }
            if ctrl_held || editor.tool_mode == ToolMode::DrawWallPoint {
                handle_ctrl_left_click(editor, pos, world_to_screen, screen_to_world);
                return;
            }
            if alt_held || editor.tool_mode == ToolMode::MarqueeSelect {
                let (wx, wy) = screen_to_world(pos);
                editor.selection_box = Some(((wx, wy), (wx, wy)));
                return;
            }
            if editor.tool_mode == ToolMode::DrawFloorZone {
                let (wx, wy) = screen_to_world(pos);
                if editor.floor_zone_points.len() >= 3 {
                    let first = editor.floor_zone_points[0];
                    if world_to_screen(first.0, first.1).distance(pos) <= 14.0 {
                        finish_floor_zone_polygon(editor);
                        return;
                    }
                }
                let (sx, sy, _) = snap_point_for_draw(wx, wy, editor);
                editor.floor_zone_points.push((sx, sy));
                return;
            }

            // Standard mode (no modifier): select/drag handle, anchor, or wall
            let hit_radius = 11.0_f32;
            let mut started_drag: Option<DragAnchorState> = None;

            if let Some(sel_id) = editor.selected_wall_id.as_deref() {
                if let Some(wall) = editor.project.walls.iter().find(|w| w.id == sel_id) {
                    for (idx, a) in wall.anchors.iter().enumerate() {
                        if let (Some(ix), Some(iy)) = (a.handle_in_x, a.handle_in_y) {
                            if world_to_screen(ix, iy).distance(pos) <= hit_radius {
                                editor.selected_anchor_index = Some(idx);
                                started_drag = Some(DragAnchorState {
                                    wall_id: wall.id.clone(),
                                    anchor_index: idx,
                                    kind: DragHandleKind::HandleIn,
                                    original_anchor: a.clone(),
                                    linked_anchors: Vec::new(),
                                });
                                break;
                            }
                        }
                        if let (Some(ox), Some(oy)) = (a.handle_out_x, a.handle_out_y) {
                            if world_to_screen(ox, oy).distance(pos) <= hit_radius {
                                editor.selected_anchor_index = Some(idx);
                                started_drag = Some(DragAnchorState {
                                    wall_id: wall.id.clone(),
                                    anchor_index: idx,
                                    kind: DragHandleKind::HandleOut,
                                    original_anchor: a.clone(),
                                    linked_anchors: Vec::new(),
                                });
                                break;
                            }
                        }
                    }
                }
            }

            if started_drag.is_none() {
                if let Some((wid, idx, orig)) = find_anchor_at_pos(editor, pos, world_to_screen) {
                    let linked = collect_linked_anchors(editor, &wid, idx, orig.pos_x, orig.pos_y);
                    editor.selected_wall_id = Some(wid.clone());
                    editor.selected_anchor_index = Some(idx);
                    editor.dirty_3d = true;
                    editor.status = if linked.is_empty() {
                        format!("Selected {wid} #{idx}.")
                    } else {
                        format!("Shared corner ({} walls).", linked.len() + 1)
                    };
                    started_drag = Some(DragAnchorState {
                        wall_id: wid,
                        anchor_index: idx,
                        kind: DragHandleKind::Anchor,
                        original_anchor: orig,
                        linked_anchors: linked,
                    });
                }
            }

            if started_drag.is_none() {
                let (wx, wy) = screen_to_world(pos);
                if let Some(op_id) = find_opening_at_world(wx, wy, editor) {
                    editor.selected_opening_id = Some(op_id.clone());
                    editor.selected_anchor_index = None;
                    editor.status = format!("Selected opening {op_id}.");
                } else if let Some((wid, _)) = find_wall_at_world(wx, wy, editor) {
                    if editor.selected_group_wall_ids.contains(&wid) {
                        editor.building_drag = Some(BuildingDragState {
                            wall_ids: editor.selected_group_wall_ids.clone(),
                            furniture_ids: editor.selected_group_furniture_ids.clone(),
                            start_world: (wx, wy),
                            applied_dx: 0.0,
                            applied_dy: 0.0,
                            click_to_place: false,
                        });
                        editor.status = format!(
                            "Moving selected group ({} walls)...",
                            editor.selected_group_wall_ids.len()
                        );
                        return;
                    }
                    editor.selected_wall_id = Some(wid.clone());
                    editor.selected_anchor_index = None;
                    editor.selected_opening_id = None;
                    editor.selected_floor_zone_id = None;
                    editor.dirty_3d = true;
                    editor.status = format!("Selected {wid}.");
                } else if let Some((zid, v_idx)) = editor
                    .selected_floor_zone_id
                    .as_ref()
                    .and_then(|zid| {
                        editor
                            .project
                            .floor_zones
                            .iter()
                            .find(|z| z.id == *zid)
                            .and_then(|z| {
                                z.points.iter().enumerate().find_map(|(i, p)| {
                                    (world_to_screen(p[0], p[1]).distance(pos) <= 10.0)
                                        .then(|| (z.id.clone(), i))
                                })
                            })
                    })
                {
                    editor.dragging_floor_zone_vertex = Some((zid, v_idx));
                    return;
                } else if let Some(zid) = editor
                    .project
                    .floor_zones
                    .iter()
                    .rev()
                    .find(|z| {
                        if z.level_id != editor.active_level_id || z.points.len() < 3 {
                            return false;
                        }
                        let poly: Vec<homely_core::geometry::Vec2> = z
                            .points
                            .iter()
                            .map(|p| homely_core::geometry::Vec2::new(p[0], p[1]))
                            .collect();
                        point_in_polygon(homely_core::geometry::Vec2::new(wx, wy), &poly)
                    })
                    .map(|z| z.id.clone())
                {
                    editor.selected_floor_zone_id = Some(zid.clone());
                    editor.selected_wall_id = None;
                    editor.selected_anchor_index = None;
                    editor.selected_opening_id = None;
                    editor.selected_room_sig = None;
                    editor.status = format!("Selected floor zone {zid}.");
                } else {
                    editor.selected_floor_zone_id = None;
                    editor.selected_group_wall_ids.clear();
                    editor.selected_group_furniture_ids.clear();
                }
            }

            editor.drag_state = started_drag;
        }
    }

    // Active floor zone vertex drag
    if let Some((zid, v_idx)) = editor.dragging_floor_zone_vertex.clone() {
        if primary_down {
            if let Some(pos) = pointer_pos {
                let (wx, wy) = screen_to_world(pos);
                let (sx, sy, _) = snap_point_for_draw(wx, wy, editor);
                if let Some(zone) = editor.project.floor_zones.iter_mut().find(|z| z.id == zid) {
                    if let Some(pt) = zone.points.get_mut(v_idx) {
                        *pt = [sx, sy];
                        editor.dirty_3d = true;
                    }
                }
            }
            return;
        } else {
            editor.dragging_floor_zone_vertex = None;
            return;
        }
    }

    // Active Alt+Drag selection marquee box
    if let Some((start_w, mut cur_w)) = editor.selection_box.take() {
        if primary_down {
            if let Some(pos) = pointer_pos {
                cur_w = screen_to_world(pos);
            }
            editor.selection_box = Some((start_w, cur_w));
            return;
        } else {
            let min_x = start_w.0.min(cur_w.0);
            let max_x = start_w.0.max(cur_w.0);
            let min_y = start_w.1.min(cur_w.1);
            let max_y = start_w.1.max(cur_w.1);

            let wall_ids: Vec<String> = editor
                .project
                .walls
                .iter()
                .filter(|w| {
                    w.level_id == editor.active_level_id
                        && w.anchors.iter().any(|a| {
                            a.pos_x >= min_x
                                && a.pos_x <= max_x
                                && a.pos_y >= min_y
                                && a.pos_y <= max_y
                        })
                })
                .map(|w| w.id.clone())
                .collect();

            let furniture_ids: Vec<String> = editor
                .project
                .furniture
                .iter()
                .filter(|f| {
                    f.level_id == editor.active_level_id
                        && ((f.transform.pos_x >= min_x
                            && f.transform.pos_x <= max_x
                            && f.transform.pos_z >= min_y
                            && f.transform.pos_z <= max_y)
                            || f.wall_mount_wall_id
                                .as_ref()
                                .is_some_and(|wid| wall_ids.contains(wid)))
                })
                .map(|f| f.id.clone())
                .collect();

            editor.status = format!(
                "Group selected: {} walls, {} items",
                wall_ids.len(),
                furniture_ids.len()
            );
            editor.selected_group_wall_ids = wall_ids;
            editor.selected_group_furniture_ids = furniture_ids;
            return;
        }
    }

    // Active building / group move (direct drag until release, or click-to-place from context menu)
    if let Some(mut bdrag) = editor.building_drag.take() {
        let keep_moving = if bdrag.click_to_place {
            !primary_pressed
        } else {
            primary_down
        };
        if keep_moving {
            if let Some(pos) = pointer_pos {
                let (wx, wy) = screen_to_world(pos);
                let total_dx = snap_grid(wx - bdrag.start_world.0, editor.unit_system);
                let total_dy = snap_grid(wy - bdrag.start_world.1, editor.unit_system);
                let step_dx = total_dx - bdrag.applied_dx;
                let step_dy = total_dy - bdrag.applied_dy;
                if step_dx.abs() > 1e-9 || step_dy.abs() > 1e-9 {
                    let _ = ProjectCommand::MoveBuilding {
                        wall_ids: bdrag.wall_ids.clone(),
                        furniture_ids: bdrag.furniture_ids.clone(),
                        dx: step_dx,
                        dy: step_dy,
                    }
                    .apply(&mut editor.project);
                    bdrag.applied_dx = total_dx;
                    bdrag.applied_dy = total_dy;
                    editor.dirty_3d = true;
                }
            }
            editor.building_drag = Some(bdrag);
            return;
        } else if bdrag.applied_dx.abs() > 1e-9 || bdrag.applied_dy.abs() > 1e-9 {
            // Undo temporary preview delta and record as a single reversible MoveBuilding command
            let _ = ProjectCommand::MoveBuilding {
                wall_ids: bdrag.wall_ids.clone(),
                furniture_ids: bdrag.furniture_ids.clone(),
                dx: -bdrag.applied_dx,
                dy: -bdrag.applied_dy,
            }
            .apply(&mut editor.project);
            editor.execute_cmd(ProjectCommand::MoveBuilding {
                wall_ids: bdrag.wall_ids,
                furniture_ids: bdrag.furniture_ids,
                dx: bdrag.applied_dx,
                dy: bdrag.applied_dy,
            });
            editor.status = "Moved building.".into();
            return;
        }
    }

    // Active drag
    if primary_down {
        if let (Some(drag), Some(pos)) = (editor.drag_state.clone(), pointer_pos) {
            let (wx, wy) = screen_to_world(pos);
            let (sx, sy) = match drag.kind {
                DragHandleKind::Anchor => snap_point_for_drag(wx, wy, editor, &drag),
                _ => (
                    snap_grid(wx, editor.unit_system).clamp(0.0, editor.plan_width),
                    snap_grid(wy, editor.unit_system).clamp(0.0, editor.plan_height),
                ),
            };

            let mut moved = false;
            if let Some(wall) = editor.project.walls.iter_mut().find(|w| w.id == drag.wall_id) {
                if let Some(a) = wall.anchors.get_mut(drag.anchor_index) {
                    match drag.kind {
                        DragHandleKind::Anchor => {
                            if (sx - a.pos_x).abs() > 1e-9 || (sy - a.pos_y).abs() > 1e-9 {
                                shift_anchor(a, sx, sy);
                                moved = true;
                                editor.dirty_3d = true;
                            }
                        }
                        DragHandleKind::HandleIn => {
                            a.handle_in_x = Some(sx);
                            a.handle_in_y = Some(sy);
                            editor.dirty_3d = true;
                        }
                        DragHandleKind::HandleOut => {
                            a.handle_out_x = Some(sx);
                            a.handle_out_y = Some(sy);
                            editor.dirty_3d = true;
                        }
                    }
                }
            }
            if moved {
                for (lwid, lidx, _) in &drag.linked_anchors {
                    if let Some(wall) = editor.project.walls.iter_mut().find(|w| w.id == *lwid) {
                        if let Some(a) = wall.anchors.get_mut(*lidx) {
                            shift_anchor(a, sx, sy);
                        }
                    }
                }
            }
        }
    } else if let Some(drag) = editor.drag_state.take() {
        let mut cmds = Vec::new();
        if let Some(new_anchor) = editor
            .project
            .walls
            .iter_mut()
            .find(|w| w.id == drag.wall_id)
            .and_then(|w| w.anchors.get_mut(drag.anchor_index))
            .map(|slot| std::mem::replace(slot, drag.original_anchor.clone()))
        {
            if new_anchor != drag.original_anchor {
                cmds.push(ProjectCommand::UpdateWallAnchor {
                    wall_id: drag.wall_id,
                    anchor_index: drag.anchor_index,
                    anchor: new_anchor,
                });
            }
        }
        for (lwid, lidx, orig) in drag.linked_anchors {
            if let Some(new_anchor) = editor
                .project
                .walls
                .iter_mut()
                .find(|w| w.id == lwid)
                .and_then(|w| w.anchors.get_mut(lidx))
                .map(|slot| std::mem::replace(slot, orig.clone()))
            {
                if new_anchor != orig {
                    cmds.push(ProjectCommand::UpdateWallAnchor {
                        wall_id: lwid,
                        anchor_index: lidx,
                        anchor: new_anchor,
                    });
                }
            }
        }
        if cmds.len() == 1 {
            editor.execute_cmd(cmds.remove(0));
        } else if !cmds.is_empty() {
            editor.execute_cmd(ProjectCommand::Batch(cmds));
        }
    }
}

fn start_building_drag(
    editor: &mut EditorState,
    seed_wall_id: &str,
    wx: f64,
    wy: f64,
    click_to_place: bool,
) {
    let level_walls: Vec<Wall> = editor
        .project
        .walls
        .iter()
        .filter(|w| w.level_id == editor.active_level_id)
        .cloned()
        .collect();
    let wall_ids = collect_connected_building_walls(&level_walls, seed_wall_id);
    if wall_ids.is_empty() {
        return;
    }

    let building_walls: Vec<Wall> = level_walls
        .into_iter()
        .filter(|w| wall_ids.contains(&w.id))
        .collect();
    let building_rooms = detect_rooms(&building_walls, 2.6);
    let furniture_ids: Vec<String> = editor
        .project
        .furniture
        .iter()
        .filter(|f| {
            f.level_id == editor.active_level_id
                && (f
                    .wall_mount_wall_id
                    .as_ref()
                    .is_some_and(|wid| wall_ids.contains(wid))
                    || building_rooms.iter().any(|r| {
                        point_in_polygon(
                            homely_core::geometry::Vec2::new(f.transform.pos_x, f.transform.pos_z),
                            &r.polygon,
                        )
                    }))
        })
        .map(|f| f.id.clone())
        .collect();

    editor.status = format!("Moving building ({} walls)...", wall_ids.len());
    editor.building_drag = Some(BuildingDragState {
        wall_ids,
        furniture_ids,
        start_world: (wx, wy),
        applied_dx: 0.0,
        applied_dy: 0.0,
        click_to_place,
    });
}

fn advance_building_mode(
    editor: &mut EditorState,
    pos: egui::Pos2,
    screen_to_world: &impl Fn(egui::Pos2) -> (f64, f64),
) {
    let Some((fx, fy)) = editor.active_draw_from else {
        return;
    };
    let (wx, wy) = screen_to_world(pos);
    let (sx, sy, docked_to_existing) = snap_point_for_draw(wx, wy, editor);

    if (fx - sx).abs() > 1e-4 || (fy - sy).abs() > 1e-4 {
        editor.create_two_point_wall(fx, fy, sx, sy);
        if docked_to_existing {
            editor.active_draw_from = None;
            editor.status = format!("Docked at ({sx:.2}m, {sy:.2}m) — building finished.");
        } else {
            editor.active_draw_from = Some((sx, sy));
            editor.status = format!("Point ({sx:.2}m, {sy:.2}m) — Left-click next point (Right-click/Esc to stop)");
        }
    }
}

fn handle_ctrl_left_click(
    editor: &mut EditorState,
    pos: egui::Pos2,
    world_to_screen: &impl Fn(f64, f64) -> egui::Pos2,
    screen_to_world: &impl Fn(egui::Pos2) -> (f64, f64),
) {
    // 1. Ctrl+Click on an existing anchor point -> enter building mode from that point
    if let Some((wid, idx, orig)) = find_anchor_at_pos(editor, pos, world_to_screen) {
        editor.selected_wall_id = Some(wid);
        editor.selected_anchor_index = Some(idx);
        editor.active_draw_from = Some((orig.pos_x, orig.pos_y));
        editor.status = format!(
            "Building from ({:.2}m, {:.2}m) — Left-click next point (Right-click/Esc to stop)",
            orig.pos_x, orig.pos_y
        );
        return;
    }

    let (wx, wy) = screen_to_world(pos);

    // 2. Ctrl+Click on a wall line -> insert anchor point ONLY (does not enter building mode)
    if let Some((wid, _)) = find_wall_at_world(wx, wy, editor) {
        let sx = snap_grid(wx, editor.unit_system).clamp(0.0, editor.plan_width);
        let sy = snap_grid(wy, editor.unit_system).clamp(0.0, editor.plan_height);
        split_wall_at_point(editor, &wid, sx, sy);
        return;
    }

    // 3. Ctrl+Click on empty space -> enter building mode at this point
    let (sx, sy, _) = snap_point_for_draw(wx, wy, editor);
    editor.active_draw_from = Some((sx, sy));
    editor.status = format!("Building from ({sx:.2}m, {sy:.2}m) — Left-click next point (Right-click/Esc to stop)");
}

fn split_wall_at_point(editor: &mut EditorState, wall_id: &str, sx: f64, sy: f64) {
    let Some(wall) = editor.project.walls.iter().find(|w| w.id == wall_id).cloned() else {
        return;
    };
    if wall.anchors.len() < 2 {
        return;
    }
    let a0 = wall.anchors[0].clone();
    let a1 = wall.anchors[wall.anchors.len() - 1].clone();

    let w1_id = editor.alloc_id("wall");
    let w2_id = editor.alloc_id("wall");
    let mid_id1 = editor.alloc_id("anchor");
    let mid_id2 = editor.alloc_id("anchor");

    let w1 = Wall {
        id: w1_id.clone(),
        name: w1_id.clone(),
        level_id: wall.level_id.clone(),
        is_closed_loop: false,
        thickness: wall.thickness,
        start_height: wall.start_height,
        end_height: wall.end_height,
        side_a_finish: wall.side_a_finish.clone(),
        side_b_finish: wall.side_b_finish.clone(),
        anchors: vec![a0, corner(&mid_id1, sx, sy)],
    };
    let w2 = Wall {
        id: w2_id.clone(),
        name: w2_id,
        level_id: wall.level_id,
        is_closed_loop: false,
        thickness: wall.thickness,
        start_height: wall.start_height,
        end_height: wall.end_height,
        side_a_finish: wall.side_a_finish,
        side_b_finish: wall.side_b_finish,
        anchors: vec![corner(&mid_id2, sx, sy), a1],
    };

    editor.execute_cmd(ProjectCommand::Batch(vec![
        ProjectCommand::RemoveWall {
            wall_id: wall_id.to_string(),
        },
        ProjectCommand::AddWall(w1),
        ProjectCommand::AddWall(w2),
    ]));
    editor.selected_wall_id = Some(w1_id);
    editor.selected_anchor_index = Some(1);
    editor.status = format!("Inserted point at ({sx:.2}m, {sy:.2}m).");
}

fn context_menu_item(ui: &mut egui::Ui, label: &str, width: f32) -> bool {
    let desired_size = egui::vec2(width, 24.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if hovered {
            ui.painter().rect_filled(
                rect,
                3.0,
                egui::Color32::from_rgb(52, 92, 148),
            );
        }
        ui.painter().text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.5),
            if hovered {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_gray(225)
            },
        );
    }
    response.clicked()
}

fn measure_menu_width(ctx: &egui::Context, labels: &[&str]) -> f32 {
    let font = egui::FontId::proportional(12.5);
    let max_text_w = ctx.fonts_mut(|fonts| {
        labels
            .iter()
            .map(|l| {
                fonts
                    .layout_no_wrap((*l).to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
    });
    (max_text_w + 22.0).max(110.0)
}

fn draw_context_menu(ctx: &egui::Context, editor: &mut EditorState) {
    let Some(menu) = editor.context_menu.clone() else {
        return;
    };
    let s = tr(editor.lang);
    let mut close_menu = false;

    let area_resp = egui::Area::new(egui::Id::new("canvas_context_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(menu.screen_pos)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style())
                .inner_margin(egui::Margin::symmetric(4, 4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    match menu.kind {
                        ContextMenuKind::Point {
                            ref wall_id,
                            anchor_index,
                        } => {
                            if let Some(wall) = editor.project.walls.iter().find(|w| w.id == *wall_id).cloned() {
                                if let Some(a) = wall.anchors.get(anchor_index).cloned() {
                                    let is_curve = a.point_type != PointType::Corner;
                                    let toggle_label = if is_curve { s.ctx_to_corner } else { s.ctx_to_curve };
                                    let w = measure_menu_width(
                                        ctx,
                                        &[
                                            toggle_label,
                                            s.ctx_continue_wall,
                                            s.ctx_move_building,
                                            s.ctx_delete_point,
                                        ],
                                    );
                                    ui.set_width(w);
                                    if context_menu_item(ui, toggle_label, w) {
                                        let mut updated = a.clone();
                                        if is_curve {
                                            updated.point_type = PointType::Corner;
                                            updated.handle_in_x = None;
                                            updated.handle_in_y = None;
                                            updated.handle_out_x = None;
                                            updated.handle_out_y = None;
                                        } else {
                                            enable_curve_handles(&wall, anchor_index, &mut updated);
                                        }
                                        editor.execute_cmd(ProjectCommand::UpdateWallAnchor {
                                            wall_id: wall_id.clone(),
                                            anchor_index,
                                            anchor: updated,
                                        });
                                        close_menu = true;
                                    }
                                    if context_menu_item(ui, s.ctx_continue_wall, w) {
                                        editor.active_draw_from = Some((a.pos_x, a.pos_y));
                                        close_menu = true;
                                    }
                                    if context_menu_item(ui, s.ctx_move_building, w) {
                                        start_building_drag(editor, wall_id, a.pos_x, a.pos_y, true);
                                        close_menu = true;
                                    }
                                    ui.separator();
                                    if context_menu_item(ui, s.ctx_delete_point, w) {
                                        editor.selected_wall_id = Some(wall_id.clone());
                                        editor.selected_anchor_index = Some(anchor_index);
                                        delete_selected_point(editor);
                                        close_menu = true;
                                    }
                                }
                            }
                        }
                        ContextMenuKind::Wall {
                            ref wall_id,
                            wx,
                            wy,
                        } => {
                            let w = measure_menu_width(
                                ctx,
                                &[
                                    s.ctx_insert_door,
                                    s.ctx_insert_window,
                                    s.ctx_insert_point,
                                    s.ctx_move_building,
                                    s.ctx_delete_wall,
                                ],
                            );
                            ui.set_width(w);
                            if context_menu_item(ui, s.ctx_insert_door, w) {
                                insert_opening_at(editor, wall_id, wx, wy, OpeningType::Door);
                                close_menu = true;
                            }
                            if context_menu_item(ui, s.ctx_insert_window, w) {
                                insert_opening_at(editor, wall_id, wx, wy, OpeningType::Window);
                                close_menu = true;
                            }
                            ui.separator();
                            if context_menu_item(ui, s.ctx_insert_point, w) {
                                let sx = snap_grid(wx, editor.unit_system).clamp(0.0, editor.plan_width);
                                let sy = snap_grid(wy, editor.unit_system).clamp(0.0, editor.plan_height);
                                split_wall_at_point(editor, wall_id, sx, sy);
                                close_menu = true;
                            }
                            if context_menu_item(ui, s.ctx_move_building, w) {
                                start_building_drag(editor, wall_id, wx, wy, true);
                                close_menu = true;
                            }
                            if context_menu_item(ui, s.ctx_delete_wall, w) {
                                editor.execute_cmd(ProjectCommand::RemoveWall {
                                    wall_id: wall_id.clone(),
                                });
                                close_menu = true;
                            }
                        }
                        ContextMenuKind::Opening { ref opening_id } => {
                            let w = measure_menu_width(ctx, &[s.ctx_delete_opening]);
                            ui.set_width(w);
                            if context_menu_item(ui, s.ctx_delete_opening, w) {
                                editor.execute_cmd(ProjectCommand::RemoveOpening {
                                    opening_id: opening_id.clone(),
                                });
                                close_menu = true;
                            }
                        }
                    }
                });
        });

    // Close context menu if user clicks outside it
    if !close_menu
        && ctx.input(|i| i.pointer.primary_pressed())
        && !area_resp.response.rect.contains(
            ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default(),
        )
    {
        close_menu = true;
    }

    if close_menu {
        editor.context_menu = None;
    }
}

fn delete_selected_point(editor: &mut EditorState) {
    let (Some(wall_id), Some(anchor_idx)) =
        (editor.selected_wall_id.clone(), editor.selected_anchor_index)
    else {
        return;
    };
    let Some(wall) = editor.project.walls.iter().find(|w| w.id == wall_id).cloned() else {
        return;
    };
    let Some(pt) = wall.anchors.get(anchor_idx).cloned() else {
        return;
    };

    // If exactly two 2-point walls meet at this corner, merge them into one wall A -> B.
    let touching: Vec<(String, usize)> = editor
        .project
        .walls
        .iter()
        .filter(|w| w.level_id == editor.active_level_id && w.anchors.len() == 2)
        .filter_map(|w| {
            w.anchors
                .iter()
                .position(|a| (a.pos_x - pt.pos_x).abs() < 1e-4 && (a.pos_y - pt.pos_y).abs() < 1e-4)
                .map(|idx| (w.id.clone(), idx))
        })
        .collect();

    if touching.len() == 2 {
        let (w1_id, w1_idx) = &touching[0];
        let (w2_id, w2_idx) = &touching[1];
        let w1 = editor.project.walls.iter().find(|w| w.id == *w1_id).unwrap().clone();
        let w2 = editor.project.walls.iter().find(|w| w.id == *w2_id).unwrap().clone();
        let other1 = w1.anchors[1 - *w1_idx].clone();
        let other2 = w2.anchors[1 - *w2_idx].clone();
        let merged_id = editor.alloc_id("wall");
        let merged = Wall {
            id: merged_id.clone(),
            name: w1.name,
            level_id: w1.level_id,
            is_closed_loop: false,
            thickness: w1.thickness,
            start_height: w1.start_height,
            end_height: w1.end_height,
            side_a_finish: w1.side_a_finish,
            side_b_finish: w1.side_b_finish,
            anchors: vec![other1, other2],
        };
        editor.execute_cmd(ProjectCommand::Batch(vec![
            ProjectCommand::RemoveWall {
                wall_id: w1_id.clone(),
            },
            ProjectCommand::RemoveWall {
                wall_id: w2_id.clone(),
            },
            ProjectCommand::AddWall(merged),
        ]));
        editor.selected_wall_id = Some(merged_id);
        editor.selected_anchor_index = None;
        editor.status = "Merged walls after deleting point.".into();
    } else if wall.anchors.len() <= 2 {
        editor.execute_cmd(ProjectCommand::RemoveWall { wall_id });
        editor.status = "Removed wall.".into();
    } else {
        editor.execute_cmd(ProjectCommand::RemoveWallAnchor {
            wall_id,
            anchor_index: anchor_idx,
        });
        editor.selected_anchor_index = None;
        editor.status = "Removed point.".into();
    }
}

fn finish_floor_zone_polygon(editor: &mut EditorState) {
    let pts = std::mem::take(&mut editor.floor_zone_points);
    if pts.len() < 3 {
        return;
    }
    let id = editor.alloc_id("fzone");
    let num = editor.project.floor_zones.len() + 1;
    let zone = FloorZone {
        id: id.clone(),
        name: format!("Zone {num}"),
        level_id: editor.active_level_id.clone(),
        points: pts.into_iter().map(|(x, y)| [x, y]).collect(),
        texture: SurfaceTexture::Tile,
        color: [215, 212, 205],
    };
    editor.execute_cmd(ProjectCommand::AddFloorZone(zone));
    editor.selected_floor_zone_id = Some(id);
}

fn find_anchor_at_pos(
    editor: &EditorState,
    pos: egui::Pos2,
    world_to_screen: &impl Fn(f64, f64) -> egui::Pos2,
) -> Option<(String, usize, WallAnchorPoint)> {
    let hit_radius = 11.0_f32;
    for wall in &editor.project.walls {
        if wall.level_id != editor.active_level_id {
            continue;
        }
        for (idx, a) in wall.anchors.iter().enumerate() {
            if world_to_screen(a.pos_x, a.pos_y).distance(pos) <= hit_radius {
                return Some((wall.id.clone(), idx, a.clone()));
            }
        }
    }
    None
}

fn collect_linked_anchors(
    editor: &EditorState,
    exclude_wall: &str,
    exclude_idx: usize,
    px: f64,
    py: f64,
) -> Vec<(String, usize, WallAnchorPoint)> {
    let mut linked = Vec::new();
    for wall in &editor.project.walls {
        if wall.level_id != editor.active_level_id {
            continue;
        }
        for (oi, oa) in wall.anchors.iter().enumerate() {
            if wall.id == exclude_wall && oi == exclude_idx {
                continue;
            }
            if (oa.pos_x - px).abs() < 1e-4 && (oa.pos_y - py).abs() < 1e-4 {
                linked.push((wall.id.clone(), oi, oa.clone()));
            }
        }
    }
    linked
}

fn insert_opening_at(
    editor: &mut EditorState,
    wall_id: &str,
    wx: f64,
    wy: f64,
    kind: OpeningType,
) {
    let Some(wall) = editor.project.walls.iter().find(|w| w.id == wall_id).cloned() else {
        return;
    };
    let line = tessellate_wall_centerline(&wall, 0.02);
    let t = project_point_onto_polyline_t(&line, wx, wy);
    let (width, height, elevation) = match kind {
        OpeningType::Door => (0.90, 2.10, 0.0),
        OpeningType::Window => (1.20, 1.30, 0.90),
        OpeningType::Cutout => (1.00, 2.00, 0.0),
    };
    let op_id = editor.alloc_id("op");
    let default_name = match kind {
        OpeningType::Door => format!("Door {}", op_id),
        OpeningType::Window => format!("Window {}", op_id),
        OpeningType::Cutout => op_id.clone(),
    };
    let opening = Opening {
        id: op_id.clone(),
        name: default_name,
        wall_id: wall_id.to_string(),
        opening_type: kind,
        catalog_item_id: None,
        asset_id: None,
        segment_index: 0,
        segment_t: t,
        elevation,
        width,
        height,
        depth: wall.thickness,
        swing_flip_side: false,
        swing_flip_hinge: false,
        ha_entity_id: None,
    };
    editor.execute_cmd(ProjectCommand::AddOpening(opening));
    editor.selected_opening_id = Some(op_id);
    editor.status = format!("Added {:?} on {wall_id}.", kind);
}

fn insert_catalog_opening_at(
    editor: &mut EditorState,
    wall_id: &str,
    wx: f64,
    wy: f64,
    item: &CatalogItem,
) {
    let Some(wall) = editor.project.walls.iter().find(|w| w.id == wall_id).cloned() else {
        return;
    };
    let line = tessellate_wall_centerline(&wall, 0.02);
    let t = project_point_onto_polyline_t(&line, wx, wy);
    let opening_type = match item.category {
        CatalogCategory::Door => OpeningType::Door,
        CatalogCategory::Window => OpeningType::Window,
        CatalogCategory::Furniture => OpeningType::Cutout,
    };
    let op_id = editor.alloc_id("op");
    let item_name = match editor.lang {
        Lang::De => item.name_de.clone(),
        Lang::En => item.name_en.clone(),
    };
    let opening = Opening {
        id: op_id.clone(),
        name: item_name.clone(),
        wall_id: wall_id.to_string(),
        opening_type,
        catalog_item_id: Some(item.id.clone()),
        asset_id: item.asset_id.clone(),
        segment_index: 0,
        segment_t: t,
        elevation: item.default_elevation,
        width: item.default_width,
        height: item.default_height,
        depth: wall.thickness,
        swing_flip_side: false,
        swing_flip_hinge: false,
        ha_entity_id: None,
    };
    editor.execute_cmd(ProjectCommand::AddOpening(opening));
    editor.selected_opening_id = Some(op_id);
    editor.selected_wall_id = None;
    editor.selected_room_sig = None;
    editor.status = format!("Placed '{item_name}' on {wall_id}.");
}

fn find_opening_at_world(wx: f64, wy: f64, editor: &EditorState) -> Option<String> {
    for op in &editor.project.openings {
        let Some(wall) = editor.project.walls.iter().find(|w| w.id == op.wall_id) else {
            continue;
        };
        if wall.level_id != editor.active_level_id {
            continue;
        }
        let line = tessellate_wall_centerline(wall, 0.02);
        let Some((mid, dir)) = sample_polyline_at_t(&line, op.segment_t) else {
            continue;
        };
        let d_mid = ((wx - mid.x).powi(2) + (wy - mid.y).powi(2)).sqrt();
        if d_mid <= (op.width * 0.5).max(0.45) {
            return Some(op.id.clone());
        }
        // Also check along the door leaf so clicking an open door blade selects it
        if op.opening_type == OpeningType::Door {
            let perp = homely_core::geometry::Vec2::new(-dir.y, dir.x);
            let side = if op.swing_flip_side { -1.0 } else { 1.0 };
            let sign_h = if op.swing_flip_hinge { 1.0 } else { -1.0 };
            let hx = mid.x + dir.x * (op.width * 0.5 * sign_h);
            let hy = mid.y + dir.y * (op.width * 0.5 * sign_h);
            let tx = hx + perp.x * side * op.width;
            let ty = hy + perp.y * side * op.width;
            if point_to_segment_dist(wx, wy, hx, hy, tx, ty) <= 0.30 {
                return Some(op.id.clone());
            }
        }
    }
    None
}

fn find_wall_at_world(wx: f64, wy: f64, editor: &EditorState) -> Option<(String, f64)> {
    let mut best: Option<(String, f64)> = None;
    for wall in &editor.project.walls {
        if wall.level_id != editor.active_level_id {
            continue;
        }
        let line = tessellate_wall_centerline(wall, 0.02);
        let threshold = (wall.thickness * 0.5).max(0.30);
        for win in line.windows(2) {
            let d = point_to_segment_dist(wx, wy, win[0].x, win[0].y, win[1].x, win[1].y);
            if d <= threshold && best.as_ref().is_none_or(|(_, bd)| d < *bd) {
                best = Some((wall.id.clone(), d));
            }
        }
    }
    best
}

#[allow(clippy::too_many_arguments)]
fn draw_rulers_and_cursor_coords(
    ui: &egui::Ui,
    full_rect: egui::Rect,
    canvas_rect: egui::Rect,
    origin: egui::Pos2,
    zoom: f32,
    min_gx: i32,
    max_gx: i32,
    min_gy: i32,
    max_gy: i32,
    plan_width: f64,
    plan_height: f64,
    unit_system: UnitSystem,
    pointer_pos: Option<egui::Pos2>,
    screen_to_world: &impl Fn(egui::Pos2) -> (f64, f64),
) {
    let ruler_bg = egui::Color32::from_rgb(35, 39, 46);
    let ruler_border = egui::Color32::from_rgb(65, 72, 84);
    let text_color = egui::Color32::from_rgb(185, 192, 204);
    let font = egui::FontId::monospace(10.5);

    let major_step_m = match unit_system {
        UnitSystem::Metric => 1.0_f64,
        UnitSystem::Imperial => METERS_PER_FOOT,
    };
    let unit_corner = match unit_system {
        UnitSystem::Metric => "m",
        UnitSystem::Imperial => "ft",
    };

    let top_ruler = egui::Rect::from_min_max(
        egui::pos2(canvas_rect.left(), full_rect.top()),
        egui::pos2(canvas_rect.right(), canvas_rect.top()),
    );
    let left_ruler = egui::Rect::from_min_max(
        egui::pos2(full_rect.left(), canvas_rect.top()),
        egui::pos2(canvas_rect.left(), canvas_rect.bottom()),
    );
    let corner_box = egui::Rect::from_min_max(full_rect.min, canvas_rect.min);

    let p = ui.painter_at(full_rect);
    p.rect_filled(top_ruler, 0.0, ruler_bg);
    p.rect_filled(left_ruler, 0.0, ruler_bg);
    p.rect_filled(corner_box, 0.0, ruler_bg);
    p.text(
        corner_box.center(),
        egui::Align2::CENTER_CENTER,
        unit_corner,
        font.clone(),
        text_color,
    );

    let step = if zoom * (major_step_m as f32) < 22.0 { 2 } else { 1 };
    for gx in min_gx..=max_gx {
        if gx % step != 0 {
            continue;
        }
        let x = origin.x + ((gx as f64 * major_step_m) as f32) * zoom;
        if x < top_ruler.left() || x > top_ruler.right() {
            continue;
        }
        p.line_segment(
            [egui::pos2(x, top_ruler.bottom() - 6.0), egui::pos2(x, top_ruler.bottom())],
            egui::Stroke::new(1.0, ruler_border),
        );
        p.text(
            egui::pos2(x + 2.0, top_ruler.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{gx}"),
            font.clone(),
            text_color,
        );
    }

    for gy in min_gy..=max_gy {
        if gy % step != 0 {
            continue;
        }
        let y = origin.y + ((gy as f64 * major_step_m) as f32) * zoom;
        if y < left_ruler.top() || y > left_ruler.bottom() {
            continue;
        }
        p.line_segment(
            [egui::pos2(left_ruler.right() - 6.0, y), egui::pos2(left_ruler.right(), y)],
            egui::Stroke::new(1.0, ruler_border),
        );
        p.text(
            egui::pos2(left_ruler.left() + 3.0, y + 1.0),
            egui::Align2::LEFT_TOP,
            format!("{gy}"),
            font.clone(),
            text_color,
        );
    }

    p.line_segment(
        [canvas_rect.left_top(), egui::pos2(canvas_rect.right(), canvas_rect.top())],
        egui::Stroke::new(1.0, ruler_border),
    );
    p.line_segment(
        [canvas_rect.left_top(), egui::pos2(canvas_rect.left(), canvas_rect.bottom())],
        egui::Stroke::new(1.0, ruler_border),
    );

    if let Some(pos) = pointer_pos {
        let marker = egui::Color32::from_rgb(100, 195, 255);
        p.line_segment(
            [egui::pos2(pos.x, top_ruler.top()), egui::pos2(pos.x, top_ruler.bottom())],
            egui::Stroke::new(1.5, marker),
        );
        p.line_segment(
            [egui::pos2(left_ruler.left(), pos.y), egui::pos2(left_ruler.right(), pos.y)],
            egui::Stroke::new(1.5, marker),
        );

        let (wx, wy) = screen_to_world(pos);
        let cx = wx.clamp(0.0, plan_width);
        let cy = wy.clamp(0.0, plan_height);
        let sx = snap_grid(cx, unit_system);
        let sy = snap_grid(cy, unit_system);
        let disp_x = unit_system.meters_to_display_len(cx);
        let disp_y = unit_system.meters_to_display_len(cy);
        let disp_sx = unit_system.meters_to_display_len(sx);
        let disp_sy = unit_system.meters_to_display_len(sy);
        let u = unit_system.len_suffix();
        let snap_label = match unit_system {
            UnitSystem::Metric => "Snap 10cm",
            UnitSystem::Imperial => "Snap 3in",
        };
        let label = format!("X: {disp_x:.2}{u}   Y: {disp_y:.2}{u}   ({snap_label}: {disp_sx:.2}, {disp_sy:.2})");
        let badge_pos = egui::pos2(canvas_rect.left() + 8.0, canvas_rect.bottom() - 8.0);
        let galley = p.layout_no_wrap(label, font, egui::Color32::WHITE);
        let badge_rect = egui::Rect::from_min_size(
            egui::pos2(badge_pos.x - 4.0, badge_pos.y - galley.size().y - 6.0),
            galley.size() + egui::vec2(10.0, 6.0),
        );
        p.rect_filled(
            badge_rect,
            4.0,
            egui::Color32::from_rgba_unmultiplied(18, 20, 24, 210),
        );
        p.galley(badge_rect.min + egui::vec2(5.0, 3.0), galley, egui::Color32::WHITE);
    }
}

#[derive(Clone, Copy)]
enum ToolIcon {
    SelectArrow,
    DrawPen,
    MarqueeRect,
    FloorRect,
    Trash,
    Cross,
}

fn tool_button(
    ui: &mut egui::Ui,
    icon: ToolIcon,
    label: &str,
    active: bool,
    enabled: bool,
) -> egui::Response {
    let desired_size = egui::vec2(154.0, 30.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(desired_size, sense);

    if ui.is_rect_visible(rect) {
        let hovered = enabled && response.hovered();
        let fill = if !enabled {
            egui::Color32::from_rgb(32, 35, 40)
        } else if active {
            egui::Color32::from_rgb(42, 108, 185)
        } else if hovered {
            egui::Color32::from_rgb(56, 62, 72)
        } else {
            egui::Color32::from_rgb(40, 44, 52)
        };
        let stroke = if active {
            egui::Stroke::new(1.5, egui::Color32::from_rgb(110, 185, 255))
        } else if hovered {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 110, 128))
        } else {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 66, 76))
        };
        let fg = if enabled {
            egui::Color32::from_gray(240)
        } else {
            egui::Color32::from_gray(115)
        };

        let p = ui.painter();
        p.rect(rect, 6.0, fill, stroke, egui::StrokeKind::Inside);

        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 8.0, rect.center().y - 8.0),
            egui::vec2(16.0, 16.0),
        );
        draw_vector_icon(p, icon_rect, icon, fg);

        p.text(
            egui::pos2(icon_rect.right() + 8.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.5),
            fg,
        );
    }

    response
}

fn draw_vector_icon(p: &egui::Painter, r: egui::Rect, icon: ToolIcon, color: egui::Color32) {
    let s = egui::Stroke::new(1.7, color);
    let pt = |nx: f32, ny: f32| egui::pos2(r.left() + r.width() * nx, r.top() + r.height() * ny);

    match icon {
        ToolIcon::SelectArrow => {
            let pts = vec![
                pt(0.25, 0.15),
                pt(0.25, 0.82),
                pt(0.44, 0.65),
                pt(0.58, 0.88),
                pt(0.68, 0.82),
                pt(0.54, 0.60),
                pt(0.78, 0.58),
            ];
            p.add(egui::Shape::closed_line(pts, s));
        }
        ToolIcon::DrawPen => {
            p.line_segment([pt(0.22, 0.78), pt(0.76, 0.24)], s);
            p.circle_filled(pt(0.20, 0.80), 2.3, color);
            p.circle_filled(pt(0.78, 0.22), 2.3, color);
        }
        ToolIcon::MarqueeRect => {
            p.rect_stroke(
                egui::Rect::from_min_max(pt(0.18, 0.22), pt(0.82, 0.78)),
                1.0,
                s,
                egui::StrokeKind::Inside,
            );
            p.circle_filled(pt(0.18, 0.22), 2.0, color);
            p.circle_filled(pt(0.82, 0.78), 2.0, color);
        }
        ToolIcon::FloorRect => {
            let r_box = egui::Rect::from_min_max(pt(0.18, 0.22), pt(0.82, 0.78));
            p.rect_filled(r_box, 1.0, color.linear_multiply(0.3));
            p.rect_stroke(r_box, 1.0, s, egui::StrokeKind::Inside);
            p.line_segment([pt(0.50, 0.22), pt(0.50, 0.78)], egui::Stroke::new(1.0, color));
            p.line_segment([pt(0.18, 0.50), pt(0.82, 0.50)], egui::Stroke::new(1.0, color));
        }
        ToolIcon::Trash => {
            p.line_segment([pt(0.2, 0.28), pt(0.8, 0.28)], s);
            p.line_segment([pt(0.38, 0.15), pt(0.62, 0.15)], s);
            p.rect_stroke(
                egui::Rect::from_min_max(pt(0.28, 0.28), pt(0.72, 0.88)),
                1.0,
                s,
                egui::StrokeKind::Inside,
            );
            p.line_segment([pt(0.43, 0.42), pt(0.43, 0.76)], egui::Stroke::new(1.2, color));
            p.line_segment([pt(0.57, 0.42), pt(0.57, 0.76)], egui::Stroke::new(1.2, color));
        }
        ToolIcon::Cross => {
            p.line_segment([pt(0.22, 0.22), pt(0.78, 0.78)], s);
            p.line_segment([pt(0.78, 0.22), pt(0.22, 0.78)], s);
        }
    }
}

fn draw_catalog_icon(p: &egui::Painter, r: egui::Rect, item: &CatalogItem) {
    let pt = |nx: f32, ny: f32| egui::pos2(r.left() + r.width() * nx, r.top() + r.height() * ny);
    match item.category {
        CatalogCategory::Door => {
            let s = egui::Stroke::new(1.5, theme::ACCENT_AMBER);
            p.line_segment([pt(0.15, 0.82), pt(0.15, 0.20)], s);
            p.line_segment([pt(0.15, 0.20), pt(0.75, 0.40)], s);
            p.line_segment([pt(0.15, 0.82), pt(0.85, 0.82)], egui::Stroke::new(1.0, theme::TEXT_MUTED));
            if item.id.contains("double") {
                p.line_segment([pt(0.85, 0.82), pt(0.85, 0.28)], s);
            }
        }
        CatalogCategory::Window => {
            let s = egui::Stroke::new(1.5, theme::ACCENT_BLUE);
            p.rect_stroke(
                egui::Rect::from_min_max(pt(0.12, 0.20), pt(0.88, 0.80)),
                1.5,
                s,
                egui::StrokeKind::Inside,
            );
            p.line_segment([pt(0.50, 0.20), pt(0.50, 0.80)], egui::Stroke::new(1.1, theme::ACCENT_BLUE));
            p.line_segment([pt(0.12, 0.50), pt(0.88, 0.50)], egui::Stroke::new(1.1, theme::ACCENT_BLUE));
        }
        CatalogCategory::Furniture => {
            let s = egui::Stroke::new(1.5, egui::Color32::from_rgb(160, 220, 150));
            p.rect_stroke(
                egui::Rect::from_min_max(pt(0.18, 0.25), pt(0.82, 0.78)),
                2.0,
                s,
                egui::StrokeKind::Inside,
            );
        }
    }
}

fn save_project_to(editor: &mut EditorState, path: &str) {
    match save_sh3x(&editor.project, Path::new(path)) {
        Ok(()) => editor.status = format!("Saved {path}"),
        Err(e) => editor.status = format!("Save error: {e}"),
    }
}

fn load_project_from(editor: &mut EditorState, path: &str) {
    match load_sh3x(Path::new(path)) {
        Ok(loaded) => {
            editor.project = loaded;
            editor.history = Default::default();
            editor.selected_wall_id = None;
            editor.selected_anchor_index = None;
            editor.active_draw_from = None;
            editor.dirty_3d = true;
            editor.status = format!("Loaded {path}");
        }
        Err(e) => editor.status = format!("Load error: {e}"),
    }
}

/// Draws the 3D Preview Controls as a right sidebar and bottom-right navigation widget inside the 3D Viewer window.
pub fn draw_preview_3d_overlay(
    ctx: &egui::Context,
    editor: &mut EditorState,
    viewer: &mut homely_viewer3d::Viewer3dState,
) {
    let s = tr(editor.lang);

    let toggle_photo = ctx.input(|i| {
        i.key_pressed(egui::Key::H)
            || (editor.photo_mode_3d && i.key_pressed(egui::Key::Escape))
    });
    if toggle_photo {
        editor.photo_mode_3d = !editor.photo_mode_3d;
    }

    if editor.photo_mode_3d {
        egui::Area::new(egui::Id::new("viewer3d_photo_exit"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(18, 20, 25, 165))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        if ui.button(format!("📷 {}", s.p3d_exit_photo)).clicked() {
                            editor.photo_mode_3d = false;
                        }
                    });
            });
        return;
    }

    let mut root_ui = egui::Ui::new(
        ctx.clone(),
        "viewer3d_root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    let sidebar_resp = egui::Panel::right("viewer3d_sidebar")
        .resizable(true)
        .default_size(265.0)
        .min_size(220.0)
        .max_size(360.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
                .inner_margin(egui::Margin::same(10)),
        )
        .show(&mut root_ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(s.tab_preview_3d);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(format!("📷 {}", s.p3d_photo_mode)).clicked() {
                        editor.photo_mode_3d = true;
                    }
                });
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(egui::RichText::new(s.p3d_camera).strong());
                ui.horizontal_wrapped(|ui| {
                    if ui.button(s.p3d_cam_iso).clicked() {
                        editor.camera_preset_3d = Some(homely_viewer3d::CameraPreset::Iso);
                    }
                    if ui.button(s.p3d_cam_top).clicked() {
                        editor.camera_preset_3d = Some(homely_viewer3d::CameraPreset::TopDown);
                    }
                    if ui.button(s.p3d_cam_front).clicked() {
                        editor.camera_preset_3d = Some(homely_viewer3d::CameraPreset::Front);
                    }
                    if ui.button(s.p3d_cam_side).clicked() {
                        editor.camera_preset_3d = Some(homely_viewer3d::CameraPreset::Side);
                    }
                });
                if ui.button(s.menu_reset_3d).clicked() {
                    editor.reset_3d_camera = true;
                }

                ui.separator();
                ui.label(egui::RichText::new(s.p3d_sky).strong());
                let sky_label = |p: homely_viewer3d::SkyPreset| -> &'static str {
                    match p {
                        homely_viewer3d::SkyPreset::ClearDay => s.sky_clear,
                        homely_viewer3d::SkyPreset::GoldenHour => s.sky_golden,
                        homely_viewer3d::SkyPreset::Overcast => s.sky_overcast,
                        homely_viewer3d::SkyPreset::Night => s.sky_night,
                    }
                };
                let mut env_changed = false;
                egui::ComboBox::from_id_salt("p3d_sky_preset")
                    .selected_text(sky_label(editor.sky_preset))
                    .show_ui(ui, |ui| {
                        for preset in [
                            homely_viewer3d::SkyPreset::ClearDay,
                            homely_viewer3d::SkyPreset::GoldenHour,
                            homely_viewer3d::SkyPreset::Overcast,
                            homely_viewer3d::SkyPreset::Night,
                        ] {
                            if ui
                                .selectable_value(&mut editor.sky_preset, preset, sky_label(preset))
                                .changed()
                            {
                                if preset == homely_viewer3d::SkyPreset::GoldenHour {
                                    editor.sun_elevation_deg = 12.0;
                                } else if preset == homely_viewer3d::SkyPreset::ClearDay
                                    && editor.sun_elevation_deg < 20.0
                                {
                                    editor.sun_elevation_deg = 50.0;
                                }
                                env_changed = true;
                            }
                        }
                    });

                ui.separator();
                ui.label(egui::RichText::new(s.p3d_sun).strong());
                env_changed |= ui
                    .add(
                        egui::Slider::new(&mut editor.sun_azimuth_deg, 0.0..=360.0)
                            .suffix("°")
                            .text(s.p3d_azimuth),
                    )
                    .changed();
                env_changed |= ui
                    .add(
                        egui::Slider::new(&mut editor.sun_elevation_deg, 2.0..=88.0)
                            .suffix("°")
                            .text(s.p3d_elevation),
                    )
                    .changed();
                env_changed |= ui
                    .add(
                        egui::Slider::new(&mut editor.sun_intensity, 1_000.0..=40_000.0)
                            .suffix(" lx")
                            .text(s.p3d_intensity),
                    )
                    .changed();
                env_changed |= ui.checkbox(&mut editor.sun_shadows, s.p3d_shadows).changed();
                if ui.button(s.p3d_reset_lighting).clicked() {
                    editor.sky_preset = homely_viewer3d::SkyPreset::ClearDay;
                    editor.sun_azimuth_deg = 45.0;
                    editor.sun_elevation_deg = 50.0;
                    editor.sun_intensity = 16_000.0;
                    editor.sun_shadows = true;
                    env_changed = true;
                }

                if env_changed {
                    editor.env_dirty_3d = true;
                }

                ui.separator();
                ui.label(egui::RichText::new(s.ter_title).strong());
                let mut ter_changed = false;
                ter_changed |= ui
                    .checkbox(&mut editor.project.terrain.enabled, s.ter_enabled)
                    .changed();

                let ter_tex_label = |t: TerrainTexture| -> &'static str {
                    match t {
                        TerrainTexture::Grass => s.ter_grass,
                        TerrainTexture::Meadow => s.ter_meadow,
                        TerrainTexture::SoilEarth => s.ter_soil,
                        TerrainTexture::Sand => s.ter_sand,
                        TerrainTexture::Gravel => s.ter_gravel,
                        TerrainTexture::StoneRock => s.ter_stone,
                    }
                };
                ui.horizontal(|ui| {
                    ui.label(format!("{}:", s.ter_texture));
                    egui::ComboBox::from_id_salt("p3d_ter_tex")
                        .selected_text(ter_tex_label(editor.project.terrain.texture))
                        .show_ui(ui, |ui| {
                            for tex in [
                                TerrainTexture::Grass,
                                TerrainTexture::Meadow,
                                TerrainTexture::SoilEarth,
                                TerrainTexture::Sand,
                                TerrainTexture::Gravel,
                                TerrainTexture::StoneRock,
                            ] {
                                ter_changed |= ui
                                    .selectable_value(
                                        &mut editor.project.terrain.texture,
                                        tex,
                                        ter_tex_label(tex),
                                    )
                                    .changed();
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label(format!("{}:", s.fin_color_1));
                    ter_changed |= ui
                        .color_edit_button_srgb(&mut editor.project.terrain.tint_color)
                        .changed();
                });

                if editor.project.terrain.enabled {
                    ter_changed |= ui
                        .add(
                            egui::Slider::new(&mut editor.project.terrain.amplitude_m, 0.0..=2.5)
                                .suffix(" m")
                                .text(s.ter_amplitude),
                        )
                        .changed();
                    ter_changed |= ui
                        .add(
                            egui::Slider::new(&mut editor.project.terrain.frequency, 0.05..=0.40)
                                .text(s.ter_frequency),
                        )
                        .changed();
                    ui.horizontal(|ui| {
                        ui.label(format!("{}:", s.ter_seed));
                        ter_changed |= ui
                            .add(egui::DragValue::new(&mut editor.project.terrain.seed))
                            .changed();
                        if ui.button("🎲").clicked() {
                            editor.project.terrain.seed =
                                editor.project.terrain.seed.wrapping_add(17);
                            ter_changed = true;
                        }
                    });
                }

                ter_changed |= ui
                    .checkbox(&mut editor.project.terrain.auto_foundation, s.ter_foundation)
                    .changed();
                if editor.project.terrain.auto_foundation {
                    ter_changed |= ui
                        .add(
                            egui::Slider::new(
                                &mut editor.project.terrain.foundation_depth_m,
                                0.2..=2.0,
                            )
                            .suffix(" m")
                            .text(s.ter_found_depth),
                        )
                        .changed();
                }
                if ter_changed {
                    editor.dirty_3d = true;
                }

                ui.separator();
                ui.small(s.p3d_nav_hint);
            });
        });
    let sidebar_width = sidebar_resp.response.rect.width();

    // Bottom-right free navigation pad (Orbit, Pan, Zoom) positioned inside the 3D viewport left of the sidebar
    let nav_btn = |ui: &mut egui::Ui, label: &str| -> bool {
        let resp = ui.add(egui::Button::new(label).min_size(egui::vec2(26.0, 24.0)));
        resp.clicked() || resp.is_pointer_button_down_on()
    };

    egui::Area::new(egui::Id::new("viewer3d_nav_pad"))
        .anchor(
            egui::Align2::RIGHT_BOTTOM,
            egui::vec2(-(sidebar_width + 12.0), -12.0),
        )
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(24, 27, 33, 215))
                .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Orbit (Rotate scene)
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(s.p3d_nav_orbit).small().strong());
                            ui.horizontal(|ui| {
                                ui.add_space(28.0);
                                if nav_btn(ui, "▲") {
                                    viewer.orbit_delta.y += 0.035;
                                }
                            });
                            ui.horizontal(|ui| {
                                if nav_btn(ui, "◀") {
                                    viewer.orbit_delta.x += 0.04;
                                }
                                if nav_btn(ui, "▼") {
                                    viewer.orbit_delta.y -= 0.035;
                                }
                                if nav_btn(ui, "▶") {
                                    viewer.orbit_delta.x -= 0.04;
                                }
                            });
                        });

                        ui.separator();

                        // Pan (Move view)
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(s.p3d_nav_pan).small().strong());
                            ui.horizontal(|ui| {
                                ui.add_space(28.0);
                                if nav_btn(ui, "⬆") {
                                    viewer.pan_delta.y += 0.18;
                                }
                            });
                            ui.horizontal(|ui| {
                                if nav_btn(ui, "⬅") {
                                    viewer.pan_delta.x -= 0.18;
                                }
                                if nav_btn(ui, "⬇") {
                                    viewer.pan_delta.y -= 0.18;
                                }
                                if nav_btn(ui, "➡") {
                                    viewer.pan_delta.x += 0.18;
                                }
                            });
                        });

                        ui.separator();

                        // Zoom + Photo Mode
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(s.p3d_nav_zoom).small().strong());
                            if ui.add(egui::Button::new("🔍 +").min_size(egui::vec2(48.0, 24.0))).is_pointer_button_down_on() {
                                viewer.zoom_delta -= 0.25;
                            }
                            if ui.add(egui::Button::new("🔍 -").min_size(egui::vec2(48.0, 24.0))).is_pointer_button_down_on() {
                                viewer.zoom_delta += 0.25;
                            }
                        });
                    });
                });
        });
}

fn draw_history_panel(ui: &mut egui::Ui, editor: &mut EditorState) {
    let s = tr(editor.lang);
    ui.heading(s.tab_history);
    ui.separator();

    let undo_labels = editor.history.undo_labels();
    let redo_labels = editor.history.redo_labels();
    let current_len = editor.history.undo_len();
    let mut target_len: Option<usize> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        if ui
            .selectable_label(current_len == 0, format!("0. {}", s.initial_state))
            .clicked()
        {
            target_len = Some(0);
        }

        for (idx, label) in undo_labels.iter().enumerate() {
            let step = idx + 1;
            let is_current = step == current_len;
            if ui
                .selectable_label(is_current, format!("{step}. {label}"))
                .clicked()
            {
                target_len = Some(step);
            }
        }

        for (offset, label) in redo_labels.iter().enumerate() {
            let step = current_len + offset + 1;
            let text = egui::RichText::new(format!("{step}. {label}"))
                .color(egui::Color32::from_gray(120));
            if ui.selectable_label(false, text).clicked() {
                target_len = Some(step);
            }
        }
    });

    if let Some(target) = target_len {
        editor.active_draw_from = None;
        match editor.history.jump_to(&mut editor.project, target) {
            Ok(()) => {
                editor.dirty_3d = true;
                editor.selected_anchor_index = None;
                editor.status = format!("{}: #{target}", s.tab_history);
            }
            Err(err) => {
                editor.status = format!("History error: {err}");
            }
        }
    }
}

fn draw_project_dialog(ctx: &egui::Context, editor: &mut EditorState) {
    let Some(mut dlg) = editor.project_dialog.clone() else {
        return;
    };
    let s = tr(editor.lang);
    let title = if dlg.is_new {
        s.menu_new
    } else {
        s.menu_project_settings
    };

    let mut close_dialog = false;
    let mut commit_dialog = false;

    let preset_label = |p: BuildingPreset| -> &'static str {
        match p {
            BuildingPreset::GermanyDin => s.preset_de,
            BuildingPreset::UsaIrc => s.preset_us,
            BuildingPreset::UkStandard => s.preset_uk,
            BuildingPreset::Custom => s.preset_custom,
        }
    };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.name));
                ui.text_edit_singleline(&mut dlg.name);
            });
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.dlg_location));
                ui.text_edit_singleline(&mut dlg.path);
                if ui.button(s.dlg_browse).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Homely 3D", &["h3d"])
                        .set_file_name(&dlg.path)
                        .save_file()
                    {
                        dlg.path = path.to_string_lossy().to_string();
                    }
                }
            });
            ui.separator();

            // Country building standard preset
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.dlg_preset));
                let mut selected_preset = dlg.building_preset;
                egui::ComboBox::from_id_salt("dlg_building_preset")
                    .selected_text(preset_label(selected_preset))
                    .show_ui(ui, |ui| {
                        for preset in [
                            BuildingPreset::GermanyDin,
                            BuildingPreset::UsaIrc,
                            BuildingPreset::UkStandard,
                            BuildingPreset::Custom,
                        ] {
                            if ui
                                .selectable_value(&mut selected_preset, preset, preset_label(preset))
                                .changed()
                                && selected_preset != BuildingPreset::Custom
                            {
                                let (units, thick, height) = selected_preset.defaults();
                                dlg.unit_system = units;
                                dlg.wall_thickness = thick;
                                dlg.wall_height = height;
                            }
                        }
                    });
                dlg.building_preset = selected_preset;
            });

            // Unit system selector (Metric / Imperial)
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.dlg_units));
                ui.selectable_value(&mut dlg.unit_system, UnitSystem::Metric, s.dlg_unit_metric);
                ui.selectable_value(
                    &mut dlg.unit_system,
                    UnitSystem::Imperial,
                    s.dlg_unit_imperial,
                );
            });

            ui.separator();
            ui.label(s.plan_size);
            let len_suffix = dlg.unit_system.len_suffix();
            let mut disp_w = dlg.unit_system.meters_to_display_len(dlg.plan_width);
            let mut disp_h = dlg.unit_system.meters_to_display_len(dlg.plan_height);
            let (min_plan, max_plan) = match dlg.unit_system {
                UnitSystem::Metric => (2.0, 200.0),
                UnitSystem::Imperial => (6.0, 650.0),
            };
            ui.horizontal(|ui| {
                ui.label("W:");
                if ui
                    .add(
                        egui::DragValue::new(&mut disp_w)
                            .speed(0.5)
                            .range(min_plan..=max_plan)
                            .fixed_decimals(1)
                            .suffix(len_suffix),
                    )
                    .changed()
                {
                    dlg.plan_width = dlg.unit_system.display_len_to_meters(disp_w);
                }
                ui.label("H:");
                if ui
                    .add(
                        egui::DragValue::new(&mut disp_h)
                            .speed(0.5)
                            .range(min_plan..=max_plan)
                            .fixed_decimals(1)
                            .suffix(len_suffix),
                    )
                    .changed()
                {
                    dlg.plan_height = dlg.unit_system.display_len_to_meters(disp_h);
                }
            });

            ui.separator();
            ui.label(s.new_wall_defaults);

            // Quick wall thickness chips for the selected building standard
            let presets_opts = dlg.building_preset.wall_thickness_options();
            if !presets_opts.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for &(chip_label, meters) in presets_opts {
                        let active = (dlg.wall_thickness - meters).abs() < 0.004;
                        if ui.selectable_label(active, chip_label).clicked() {
                            dlg.wall_thickness = meters;
                        }
                    }
                });
            }

            // Wall thickness in cm (Metric) or in (Imperial)
            match dlg.unit_system {
                UnitSystem::Metric => {
                    let mut thick_cm = dlg.wall_thickness * 100.0;
                    if ui
                        .add(
                            egui::Slider::new(&mut thick_cm, 6.0..=60.0)
                                .fixed_decimals(1)
                                .text(format!("{} (cm)", s.thickness)),
                        )
                        .changed()
                    {
                        dlg.wall_thickness = thick_cm / 100.0;
                        dlg.building_preset = BuildingPreset::Custom;
                    }
                }
                UnitSystem::Imperial => {
                    let mut thick_in = UnitSystem::meters_to_inches(dlg.wall_thickness as f64) as f32;
                    if ui
                        .add(
                            egui::Slider::new(&mut thick_in, 2.5..=24.0)
                                .fixed_decimals(1)
                                .text(format!("{} (in)", s.thickness)),
                        )
                        .changed()
                    {
                        dlg.wall_thickness = UnitSystem::inches_to_meters(thick_in as f64) as f32;
                        dlg.building_preset = BuildingPreset::Custom;
                    }
                }
            }

            // Wall height in m (Metric) or ft (Imperial)
            let mut disp_wh = dlg.unit_system.meters_to_display_len(dlg.wall_height as f64) as f32;
            let (min_wh, max_wh) = match dlg.unit_system {
                UnitSystem::Metric => (1.0_f32, 5.0_f32),
                UnitSystem::Imperial => (3.5_f32, 16.5_f32),
            };
            if ui
                .add(
                    egui::Slider::new(&mut disp_wh, min_wh..=max_wh)
                        .fixed_decimals(2)
                        .text(format!("{} ({})", s.height, len_suffix.trim())),
                )
                .changed()
            {
                dlg.wall_height = dlg.unit_system.display_len_to_meters(disp_wh as f64) as f32;
                dlg.building_preset = BuildingPreset::Custom;
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(if dlg.is_new { s.dlg_create } else { s.dlg_apply }).clicked() {
                    commit_dialog = true;
                    close_dialog = true;
                }
                if ui.button(s.dlg_cancel).clicked() {
                    close_dialog = true;
                }
            });
        });

    if commit_dialog {
        if dlg.is_new {
            editor.reset_new_project();
        }
        editor.project.name = dlg.name.clone();
        editor.project_path = dlg.path.clone();
        editor.unit_system = dlg.unit_system;
        editor.building_preset = dlg.building_preset;
        editor.plan_width = dlg.plan_width;
        editor.plan_height = dlg.plan_height;
        editor.wall_thickness = dlg.wall_thickness;
        editor.wall_height = dlg.wall_height;
        editor.dirty_3d = true;
        if dlg.is_new {
            save_project_to(editor, &dlg.path);
        } else {
            editor.status = format!("{}: {}", s.menu_project_settings, editor.project.name);
        }
    }

    if close_dialog {
        editor.project_dialog = None;
    } else {
        editor.project_dialog = Some(dlg);
    }
}

fn draw_surface_finish_editor(
    ui: &mut egui::Ui,
    s: &crate::i18n::Strings,
    id_prefix: &str,
    title: &str,
    finish: &mut SurfaceFinish,
    unit_sys: UnitSystem,
) -> bool {
    let mut changed = false;
    let tex_label = |t: SurfaceTexture| -> &'static str {
        match t {
            SurfaceTexture::Smooth => s.tex_smooth,
            SurfaceTexture::Plaster => s.tex_plaster,
            SurfaceTexture::Brick => s.tex_brick,
            SurfaceTexture::WoodPlanks => s.tex_wood,
            SurfaceTexture::Tile => s.tex_tile,
            SurfaceTexture::Concrete => s.tex_concrete,
        }
    };
    let paint_label = |p: PaintStyle| -> &'static str {
        match p {
            PaintStyle::Solid => s.paint_solid,
            PaintStyle::BaseboardTrim => s.paint_baseboard,
            PaintStyle::CrownTrim => s.paint_crown,
            PaintStyle::OpeningTrim => s.paint_opening,
            PaintStyle::TwoTone => s.paint_twotone,
            PaintStyle::Gradient => s.paint_gradient,
        }
    };

    egui::CollapsingHeader::new(title)
        .id_salt(format!("fin_hdr_{id_prefix}"))
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.fin_texture));
                egui::ComboBox::from_id_salt(format!("fin_tex_{id_prefix}"))
                    .selected_text(tex_label(finish.texture))
                    .show_ui(ui, |ui| {
                        for tex in [
                            SurfaceTexture::Smooth,
                            SurfaceTexture::Plaster,
                            SurfaceTexture::Brick,
                            SurfaceTexture::WoodPlanks,
                            SurfaceTexture::Tile,
                            SurfaceTexture::Concrete,
                        ] {
                            changed |= ui
                                .selectable_value(&mut finish.texture, tex, tex_label(tex))
                                .changed();
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.fin_paint_style));
                egui::ComboBox::from_id_salt(format!("fin_paint_{id_prefix}"))
                    .selected_text(paint_label(finish.paint_style))
                    .show_ui(ui, |ui| {
                        for ps in [
                            PaintStyle::Solid,
                            PaintStyle::BaseboardTrim,
                            PaintStyle::CrownTrim,
                            PaintStyle::OpeningTrim,
                            PaintStyle::TwoTone,
                            PaintStyle::Gradient,
                        ] {
                            changed |= ui
                                .selectable_value(&mut finish.paint_style, ps, paint_label(ps))
                                .changed();
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.fin_color_1));
                changed |= ui.color_edit_button_srgb(&mut finish.primary_color).changed();
            });

            if finish.paint_style != PaintStyle::Solid {
                let mut tex2 = finish.secondary_texture.unwrap_or(finish.texture);
                ui.horizontal(|ui| {
                    ui.label(format!("{}:", s.fin_texture_2));
                    egui::ComboBox::from_id_salt(format!("fin_tex2_{id_prefix}"))
                        .selected_text(tex_label(tex2))
                        .show_ui(ui, |ui| {
                            for tex in [
                                SurfaceTexture::Smooth,
                                SurfaceTexture::Plaster,
                                SurfaceTexture::Brick,
                                SurfaceTexture::WoodPlanks,
                                SurfaceTexture::Tile,
                                SurfaceTexture::Concrete,
                            ] {
                                if ui
                                    .selectable_value(&mut tex2, tex, tex_label(tex))
                                    .changed()
                                {
                                    finish.secondary_texture = Some(tex2);
                                    changed = true;
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label(format!("{}:", s.fin_color_2));
                    changed |= ui
                        .color_edit_button_srgb(&mut finish.secondary_color)
                        .changed();
                });
            }

            if matches!(
                finish.paint_style,
                PaintStyle::BaseboardTrim | PaintStyle::CrownTrim | PaintStyle::OpeningTrim
            ) {
                ui.horizontal(|ui| {
                    ui.label(format!("{}:", s.fin_trim_width));
                    match unit_sys {
                        UnitSystem::Metric => {
                            let mut cm = finish.trim_width_m * 100.0;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut cm)
                                        .speed(0.5)
                                        .range(3.0..=50.0)
                                        .fixed_decimals(1)
                                        .suffix(" cm"),
                                )
                                .changed()
                            {
                                finish.trim_width_m = cm / 100.0;
                                changed = true;
                            }
                        }
                        UnitSystem::Imperial => {
                            let mut inches =
                                UnitSystem::meters_to_inches(finish.trim_width_m as f64) as f32;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut inches)
                                        .speed(0.25)
                                        .range(1.0..=20.0)
                                        .fixed_decimals(1)
                                        .suffix(" in"),
                                )
                                .changed()
                            {
                                finish.trim_width_m =
                                    UnitSystem::inches_to_meters(inches as f64) as f32;
                                changed = true;
                            }
                        }
                    }
                });
            }
        });
    changed
}

fn draw_floor_finish_simple(
    ui: &mut egui::Ui,
    s: &crate::i18n::Strings,
    id_prefix: &str,
    texture: &mut SurfaceTexture,
    color: &mut [u8; 3],
) -> bool {
    let mut changed = false;
    let tex_label = |t: SurfaceTexture| -> &'static str {
        match t {
            SurfaceTexture::Smooth => s.tex_smooth,
            SurfaceTexture::Plaster => s.tex_plaster,
            SurfaceTexture::Brick => s.tex_brick,
            SurfaceTexture::WoodPlanks => s.tex_wood,
            SurfaceTexture::Tile => s.tex_tile,
            SurfaceTexture::Concrete => s.tex_concrete,
        }
    };
    ui.horizontal(|ui| {
        ui.label(format!("{}:", s.fin_texture));
        egui::ComboBox::from_id_salt(format!("floor_tex_{id_prefix}"))
            .selected_text(tex_label(*texture))
            .show_ui(ui, |ui| {
                for tex in [
                    SurfaceTexture::WoodPlanks,
                    SurfaceTexture::Tile,
                    SurfaceTexture::Concrete,
                    SurfaceTexture::Smooth,
                    SurfaceTexture::Plaster,
                    SurfaceTexture::Brick,
                ] {
                    changed |= ui
                        .selectable_value(texture, tex, tex_label(tex))
                        .changed();
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label(format!("{}:", s.fin_color_1));
        changed |= ui.color_edit_button_srgb(color).changed();
    });
    changed
}

fn draw_inspector(ui: &mut egui::Ui, editor: &mut EditorState) {
    let s = tr(editor.lang);
    draw_navigator_overview(ui, editor);
    ui.separator();
    draw_scene_tree(ui, editor);

    if let Some(sig) = editor.selected_room_sig.clone() {
        let mut r_name = editor
            .project
            .room_names
            .get(&sig)
            .cloned()
            .unwrap_or_default();
        ui.separator();
        ui.heading("Room");
        ui.horizontal(|ui| {
            ui.label(format!("{}:", s.object_name));
            if ui.text_edit_singleline(&mut r_name).changed() {
                editor.execute_cmd(ProjectCommand::RenameRoom {
                    signature: sig.clone(),
                    name: r_name,
                });
            }
        });
        let mut floor_finish = editor
            .project
            .room_finishes
            .get(&sig)
            .cloned()
            .unwrap_or_else(SurfaceFinish::default_floor);
        ui.label(egui::RichText::new(s.fin_floor).strong());
        if draw_floor_finish_simple(
            ui,
            s,
            "room_floor",
            &mut floor_finish.texture,
            &mut floor_finish.primary_color,
        ) {
            floor_finish.paint_style = PaintStyle::Solid;
            editor.execute_cmd(ProjectCommand::UpdateRoomFinish {
                signature: sig,
                finish: floor_finish,
            });
        }
    }

    if let Some(zid) = editor.selected_floor_zone_id.clone() {
        if let Some(mut zone) = editor.project.floor_zones.iter().find(|z| z.id == zid).cloned() {
            ui.separator();
            ui.heading(s.floor_zone_title);
            let mut z_changed = false;
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.object_name));
                z_changed |= ui.text_edit_singleline(&mut zone.name).changed();
            });
            z_changed |= draw_floor_finish_simple(
                ui,
                s,
                &format!("fzone_{zid}"),
                &mut zone.texture,
                &mut zone.color,
            );
            let u = editor.unit_system;
            let len_u = u.len_suffix();
            let mut remove_idx: Option<usize> = None;
            let can_remove = zone.points.len() > 3;
            for (idx, pt) in zone.points.iter_mut().enumerate() {
                let mut dx = u.meters_to_display_len(pt[0]);
                let mut dy = u.meters_to_display_len(pt[1]);
                ui.horizontal(|ui| {
                    ui.label(format!("#{idx}"));
                    if ui.add(egui::DragValue::new(&mut dx).speed(0.1).fixed_decimals(2).prefix("X:").suffix(len_u)).changed() {
                        pt[0] = u.display_len_to_meters(dx).clamp(0.0, editor.plan_width);
                        z_changed = true;
                    }
                    if ui.add(egui::DragValue::new(&mut dy).speed(0.1).fixed_decimals(2).prefix("Y:").suffix(len_u)).changed() {
                        pt[1] = u.display_len_to_meters(dy).clamp(0.0, editor.plan_height);
                        z_changed = true;
                    }
                    if can_remove && ui.small_button("×").clicked() {
                        remove_idx = Some(idx);
                    }
                });
            }
            if let Some(idx) = remove_idx {
                zone.points.remove(idx);
                z_changed = true;
            }
            if z_changed {
                editor.execute_cmd(ProjectCommand::UpdateFloorZone(zone));
            }
            if ui.button(s.delete_floor_zone).clicked() {
                editor.execute_cmd(ProjectCommand::RemoveFloorZone { zone_id: zid });
                editor.selected_floor_zone_id = None;
            }
        }
    }

    if let Some(sel_id) = editor.selected_wall_id.clone() {
        if let Some(wall) = editor.project.walls.iter().find(|w| w.id == sel_id).cloned() {
            ui.separator();
            let display = if wall.name.is_empty() { &wall.id } else { &wall.name };
            ui.heading(display);

            let mut w_name = wall.name.clone();
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.object_name));
                if ui.text_edit_singleline(&mut w_name).changed() {
                    editor.execute_cmd(ProjectCommand::RenameWall {
                        wall_id: wall.id.clone(),
                        name: w_name,
                    });
                }
            });

            let mut thickness = wall.thickness;
            let mut start_h = wall.start_height;
            let mut end_h = wall.end_height;
            let mut props_changed = false;
            let unit_sys = editor.unit_system;
            let len_u = unit_sys.len_suffix();

            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.thickness));
                match unit_sys {
                    UnitSystem::Metric => {
                        let mut thick_cm = thickness * 100.0;
                        if ui
                            .add(egui::DragValue::new(&mut thick_cm).speed(0.5).range(5.0..=100.0).fixed_decimals(1).suffix(" cm"))
                            .changed()
                        {
                            thickness = thick_cm / 100.0;
                            props_changed = true;
                        }
                    }
                    UnitSystem::Imperial => {
                        let mut thick_in = UnitSystem::meters_to_inches(thickness);
                        if ui
                            .add(egui::DragValue::new(&mut thick_in).speed(0.25).range(2.0..=40.0).fixed_decimals(1).suffix(" in"))
                            .changed()
                        {
                            thickness = UnitSystem::inches_to_meters(thick_in);
                            props_changed = true;
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.height));
                let mut disp_h = unit_sys.meters_to_display_len(start_h);
                let (min_h, max_h) = match unit_sys {
                    UnitSystem::Metric => (0.5_f64, 8.0_f64),
                    UnitSystem::Imperial => (1.5_f64, 26.0_f64),
                };
                if ui
                    .add(egui::DragValue::new(&mut disp_h).speed(0.1).range(min_h..=max_h).fixed_decimals(2).suffix(len_u))
                    .changed()
                {
                    start_h = unit_sys.display_len_to_meters(disp_h);
                    end_h = start_h;
                    props_changed = true;
                }
            });

            if props_changed {
                editor.execute_cmd(ProjectCommand::UpdateWallProperties {
                    wall_id: wall.id.clone(),
                    thickness,
                    start_height: start_h,
                    end_height: end_h,
                    is_closed_loop: wall.is_closed_loop,
                });
            }

            let mut side_a = wall.side_a_finish.clone();
            let mut side_b = wall.side_b_finish.clone();
            let mut finish_changed = false;
            finish_changed |= draw_surface_finish_editor(
                ui,
                s,
                &format!("wall_{}_a", wall.id),
                s.fin_side_a,
                &mut side_a,
                unit_sys,
            );
            finish_changed |= draw_surface_finish_editor(
                ui,
                s,
                &format!("wall_{}_b", wall.id),
                s.fin_side_b,
                &mut side_b,
                unit_sys,
            );
            if ui.button(s.fin_swap_sides).clicked() {
                std::mem::swap(&mut side_a, &mut side_b);
                finish_changed = true;
            }
            if finish_changed {
                editor.execute_cmd(ProjectCommand::UpdateWallFinish {
                    wall_id: wall.id.clone(),
                    side_a,
                    side_b,
                });
            }

            let mut anchor_update: Option<(usize, WallAnchorPoint)> = None;
            for (idx, a) in wall.anchors.iter().enumerate() {
                let mut edited = a.clone();
                let mut changed = false;
                let is_anchor_selected = editor.selected_anchor_index == Some(idx);

                ui.horizontal(|ui| {
                    if ui.selectable_label(is_anchor_selected, format!("#{idx}")).clicked() {
                        editor.selected_anchor_index = Some(idx);
                    }
                    let old_x = edited.pos_x;
                    let old_y = edited.pos_y;
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut edited.pos_x)
                                .speed(0.01)
                                .range(0.0..=editor.plan_width)
                                .fixed_decimals(2)
                                .prefix("X:"),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut edited.pos_y)
                                .speed(0.01)
                                .range(0.0..=editor.plan_height)
                                .fixed_decimals(2)
                                .prefix("Y:"),
                        )
                        .changed();
                    let dx = edited.pos_x - old_x;
                    let dy = edited.pos_y - old_y;
                    if dx.abs() > 1e-9 || dy.abs() > 1e-9 {
                        if let Some(ref mut ix) = edited.handle_in_x {
                            *ix += dx;
                        }
                        if let Some(ref mut iy) = edited.handle_in_y {
                            *iy += dy;
                        }
                        if let Some(ref mut ox) = edited.handle_out_x {
                            *ox += dx;
                        }
                        if let Some(ref mut oy) = edited.handle_out_y {
                            *oy += dy;
                        }
                    }
                    let is_smooth = edited.point_type != PointType::Corner;
                    if ui.button(if is_smooth { s.curve } else { s.corner }).clicked() {
                        if is_smooth {
                            edited.point_type = PointType::Corner;
                            edited.handle_in_x = None;
                            edited.handle_in_y = None;
                            edited.handle_out_x = None;
                            edited.handle_out_y = None;
                        } else {
                            enable_curve_handles(&wall, idx, &mut edited);
                        }
                        changed = true;
                    }
                });
                if changed {
                    editor.selected_anchor_index = Some(idx);
                    anchor_update = Some((idx, edited));
                }
            }
            if let Some((idx, anchor)) = anchor_update {
                let orig = &wall.anchors[idx];
                let moved = (anchor.pos_x - orig.pos_x).abs() > 1e-9
                    || (anchor.pos_y - orig.pos_y).abs() > 1e-9;
                let linked = if moved {
                    collect_linked_anchors(editor, &wall.id, idx, orig.pos_x, orig.pos_y)
                } else {
                    Vec::new()
                };
                if linked.is_empty() {
                    editor.execute_cmd(ProjectCommand::UpdateWallAnchor {
                        wall_id: wall.id,
                        anchor_index: idx,
                        anchor,
                    });
                } else {
                    let new_x = anchor.pos_x;
                    let new_y = anchor.pos_y;
                    let mut cmds = vec![ProjectCommand::UpdateWallAnchor {
                        wall_id: wall.id,
                        anchor_index: idx,
                        anchor,
                    }];
                    for (lwid, lidx, mut la) in linked {
                        shift_anchor(&mut la, new_x, new_y);
                        cmds.push(ProjectCommand::UpdateWallAnchor {
                            wall_id: lwid,
                            anchor_index: lidx,
                            anchor: la,
                        });
                    }
                    editor.execute_cmd(ProjectCommand::Batch(cmds));
                }
            }
        }
    }

    if let Some(op_id) = editor.selected_opening_id.clone() {
        if let Some(op) = editor.project.openings.iter().find(|o| o.id == op_id).cloned() {
            ui.separator();
            let kind_label = match op.opening_type {
                OpeningType::Door => s.opening_door,
                OpeningType::Window => s.opening_window,
                OpeningType::Cutout => "Cutout",
            };
            let title = if op.name.is_empty() { &op.id } else { &op.name };
            ui.heading(format!("{kind_label}: {title}"));

            let mut edited = op.clone();
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.object_name));
                changed |= ui.text_edit_singleline(&mut edited.name).changed();
            });
            let unit_sys = editor.unit_system;
            let len_u = unit_sys.len_suffix();
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.opening_width));
                let mut disp_w = unit_sys.meters_to_display_len(edited.width);
                let (min_w, max_w) = match unit_sys {
                    UnitSystem::Metric => (0.3_f64, 4.0_f64),
                    UnitSystem::Imperial => (1.0_f64, 13.0_f64),
                };
                if ui
                    .add(egui::DragValue::new(&mut disp_w).speed(0.05).range(min_w..=max_w).fixed_decimals(2).suffix(len_u))
                    .changed()
                {
                    edited.width = unit_sys.display_len_to_meters(disp_w);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.height));
                let mut disp_h = unit_sys.meters_to_display_len(edited.height);
                let (min_h, max_h) = match unit_sys {
                    UnitSystem::Metric => (0.3_f64, 3.5_f64),
                    UnitSystem::Imperial => (1.0_f64, 11.5_f64),
                };
                if ui
                    .add(egui::DragValue::new(&mut disp_h).speed(0.05).range(min_h..=max_h).fixed_decimals(2).suffix(len_u))
                    .changed()
                {
                    edited.height = unit_sys.display_len_to_meters(disp_h);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.opening_elevation));
                let mut disp_e = unit_sys.meters_to_display_len(edited.elevation);
                let max_e = match unit_sys {
                    UnitSystem::Metric => 2.5_f64,
                    UnitSystem::Imperial => 8.5_f64,
                };
                if ui
                    .add(egui::DragValue::new(&mut disp_e).speed(0.05).range(0.0..=max_e).fixed_decimals(2).suffix(len_u))
                    .changed()
                {
                    edited.elevation = unit_sys.display_len_to_meters(disp_e);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.opening_pos));
                changed |= ui
                    .add(egui::DragValue::new(&mut edited.segment_t).speed(0.01).range(0.05..=0.95))
                    .changed();
            });
            if edited.opening_type == OpeningType::Door {
                ui.horizontal(|ui| {
                    if ui.button(s.opening_flip_side).clicked() {
                        edited.swing_flip_side = !edited.swing_flip_side;
                        changed = true;
                    }
                    if ui.button(s.opening_flip_hinge).clicked() {
                        edited.swing_flip_hinge = !edited.swing_flip_hinge;
                        changed = true;
                    }
                });
            }
            if changed {
                editor.execute_cmd(ProjectCommand::UpdateOpening(edited));
            }
            if ui.button(s.ctx_delete_opening).clicked() {
                editor.execute_cmd(ProjectCommand::RemoveOpening { opening_id: op_id });
            }
        }
    }
}

fn draw_scene_tree(ui: &mut egui::Ui, editor: &mut EditorState) {
    let s = tr(editor.lang);
    ui.heading(s.scene_tree);

    let level_walls: Vec<Wall> = editor
        .project
        .walls
        .iter()
        .filter(|w| w.level_id == editor.active_level_id)
        .cloned()
        .collect();
    let level_height = editor
        .project
        .levels
        .iter()
        .find(|l| l.id == editor.active_level_id)
        .map(|l| l.height)
        .unwrap_or(2.6);
    let rooms = detect_rooms(&level_walls, level_height);

    let mut assigned_wall_ids: Vec<String> = Vec::new();
    let mut remove_wall_id: Option<String> = None;

    egui::ScrollArea::vertical()
        .id_salt("scene_tree_scroll")
        .max_height(210.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            for (r_idx, room) in rooms.iter().enumerate() {
                let sig = room_signature(&room.polygon);
                let default_name = match editor.lang {
                    Lang::De => format!("Raum {}", r_idx + 1),
                    Lang::En => format!("Room {}", r_idx + 1),
                };
                let r_name = editor
                    .project
                    .room_names
                    .get(&sig)
                    .filter(|n| !n.is_empty())
                    .cloned()
                    .unwrap_or(default_name);
                let is_room_sel = editor.selected_room_sig.as_deref() == Some(&sig);

                let area_disp = editor.unit_system.sqm_to_display_area(room.area_sqm);
                let area_u = editor.unit_system.area_suffix();
                egui::CollapsingHeader::new(format!("{r_name} ({area_disp:.1}{area_u})"))
                    .id_salt(format!("tree_room_{sig}"))
                    .default_open(true)
                    .show(ui, |ui| {
                        if ui.selectable_label(is_room_sel, format!("★ {r_name}")).clicked() {
                            editor.selected_room_sig = Some(sig.clone());
                            editor.selected_wall_id = None;
                            editor.selected_opening_id = None;
                            editor.selected_anchor_index = None;
                        }

                        let room_walls: Vec<&Wall> = level_walls
                            .iter()
                            .filter(|w| wall_borders_room(w, &room.polygon))
                            .collect();
                        for w in &room_walls {
                            if !assigned_wall_ids.contains(&w.id) {
                                assigned_wall_ids.push(w.id.clone());
                            }
                        }

                        egui::CollapsingHeader::new(format!("{} ({})", s.walls, room_walls.len()))
                            .id_salt(format!("tree_walls_{sig}"))
                            .default_open(true)
                            .show(ui, |ui| {
                                for w in &room_walls {
                                    let sel = editor.selected_wall_id.as_deref() == Some(&w.id);
                                    let label = if w.name.is_empty() { &w.id } else { &w.name };
                                    ui.horizontal(|ui| {
                                        if ui.selectable_label(sel, label).clicked() {
                                            editor.selected_wall_id = Some(w.id.clone());
                                            editor.selected_opening_id = None;
                                            editor.selected_room_sig = None;
                                            editor.dirty_3d = true;
                                        }
                                        if ui.small_button("X").clicked() {
                                            remove_wall_id = Some(w.id.clone());
                                        }
                                    });
                                }
                            });

                        let room_openings: Vec<_> = editor
                            .project
                            .openings
                            .iter()
                            .filter(|o| room_walls.iter().any(|w| w.id == o.wall_id))
                            .cloned()
                            .collect();
                        egui::CollapsingHeader::new(format!("{} ({})", s.openings, room_openings.len()))
                            .id_salt(format!("tree_openings_{sig}"))
                            .default_open(true)
                            .show(ui, |ui| {
                                for op in &room_openings {
                                    let sel = editor.selected_opening_id.as_deref() == Some(&op.id);
                                    let label = if op.name.is_empty() { &op.id } else { &op.name };
                                    if ui.selectable_label(sel, label).clicked() {
                                        editor.selected_opening_id = Some(op.id.clone());
                                        editor.selected_wall_id = None;
                                        editor.selected_room_sig = None;
                                    }
                                }
                            });

                        let room_furn: Vec<_> = editor
                            .project
                            .furniture
                            .iter()
                            .filter(|f| {
                                f.level_id == editor.active_level_id
                                    && point_in_polygon(
                                        homely_core::geometry::Vec2::new(
                                            f.transform.pos_x,
                                            f.transform.pos_z,
                                        ),
                                        &room.polygon,
                                    )
                            })
                            .collect();
                        egui::CollapsingHeader::new(format!("{} ({})", s.furniture, room_furn.len()))
                            .id_salt(format!("tree_furn_{sig}"))
                            .show(ui, |ui| {
                                for f in room_furn {
                                    let label = if f.name.is_empty() { &f.id } else { &f.name };
                                    ui.label(label);
                                }
                            });
                    });
            }

            // Unassigned walls & openings (not bordering any closed room yet)
            let free_walls: Vec<&Wall> = level_walls
                .iter()
                .filter(|w| !assigned_wall_ids.contains(&w.id))
                .collect();
            let free_openings: Vec<_> = editor
                .project
                .openings
                .iter()
                .filter(|o| free_walls.iter().any(|w| w.id == o.wall_id))
                .cloned()
                .collect();

            if !free_walls.is_empty() || !free_openings.is_empty() {
                egui::CollapsingHeader::new(s.unassigned)
                    .id_salt("tree_unassigned")
                    .default_open(true)
                    .show(ui, |ui| {
                        for w in &free_walls {
                            let sel = editor.selected_wall_id.as_deref() == Some(&w.id);
                            let label = if w.name.is_empty() { &w.id } else { &w.name };
                            ui.horizontal(|ui| {
                                if ui.selectable_label(sel, label).clicked() {
                                    editor.selected_wall_id = Some(w.id.clone());
                                    editor.selected_opening_id = None;
                                    editor.selected_room_sig = None;
                                    editor.dirty_3d = true;
                                }
                                if ui.small_button("X").clicked() {
                                    remove_wall_id = Some(w.id.clone());
                                }
                            });
                        }
                        for op in &free_openings {
                            let sel = editor.selected_opening_id.as_deref() == Some(&op.id);
                            let label = if op.name.is_empty() { &op.id } else { &op.name };
                            if ui.selectable_label(sel, label).clicked() {
                                editor.selected_opening_id = Some(op.id.clone());
                                editor.selected_wall_id = None;
                                editor.selected_room_sig = None;
                            }
                        }
                    });
            }
        });

    if let Some(wall_id) = remove_wall_id {
        editor.execute_cmd(ProjectCommand::RemoveWall { wall_id });
    }
}

fn wall_borders_room(wall: &Wall, poly: &[homely_core::geometry::Vec2]) -> bool {
    if wall.anchors.is_empty() || poly.len() < 3 {
        return false;
    }
    let a0 = &wall.anchors[0];
    let al = &wall.anchors[wall.anchors.len() - 1];
    let mid_x = (a0.pos_x + al.pos_x) * 0.5;
    let mid_y = (a0.pos_y + al.pos_y) * 0.5;
    poly.windows(2).chain(std::iter::once(&[poly[poly.len() - 1], poly[0]][..])).any(|e| {
        point_to_segment_dist(mid_x, mid_y, e[0].x, e[0].y, e[1].x, e[1].y) < 0.15
    })
}

fn draw_navigator_overview(ui: &mut egui::Ui, editor: &mut EditorState) {
    let s = tr(editor.lang);
    ui.heading(s.overview);
    let avail_w = ui.available_width().max(120.0);
    let box_h = 135.0;
    let (box_rect, response) =
        ui.allocate_exact_size(egui::vec2(avail_w, box_h), egui::Sense::click_and_drag());
    let painter = ui.painter_at(box_rect);
    painter.rect_filled(box_rect, 2.0, egui::Color32::from_rgb(18, 20, 24));

    let pw = editor.plan_width.max(1.0) as f32;
    let ph = editor.plan_height.max(1.0) as f32;
    let pad = 6.0;
    let scale = ((box_rect.width() - pad * 2.0) / pw)
        .min((box_rect.height() - pad * 2.0) / ph)
        .max(0.1);
    let map_size = egui::vec2(pw * scale, ph * scale);
    let map_min = box_rect.center() - map_size * 0.5;
    let map_rect = egui::Rect::from_min_size(map_min, map_size);

    painter.rect_filled(map_rect, 0.0, egui::Color32::from_rgb(32, 36, 44));
    painter.rect_stroke(
        map_rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(95, 105, 120)),
        egui::StrokeKind::Outside,
    );

    let to_mini = |wx: f64, wy: f64| -> egui::Pos2 {
        egui::pos2(map_min.x + (wx as f32) * scale, map_min.y + (wy as f32) * scale)
    };

    for wall in &editor.project.walls {
        if wall.level_id != editor.active_level_id {
            continue;
        }
        let line = tessellate_wall_centerline(wall, 0.05);
        let color = if editor.selected_wall_id.as_deref() == Some(&wall.id) {
            egui::Color32::from_rgb(100, 185, 255)
        } else {
            egui::Color32::from_rgb(210, 208, 200)
        };
        for win in line.windows(2) {
            painter.line_segment(
                [to_mini(win[0].x, win[0].y), to_mini(win[1].x, win[1].y)],
                egui::Stroke::new(1.5, color),
            );
        }
    }

    let zoom = editor.zoom_2d;
    let view_w = editor.last_canvas_size.x / zoom;
    let view_h = editor.last_canvas_size.y / zoom;
    let view_min_wx = (-editor.pan_2d.x) / zoom;
    let view_min_wy = (-editor.pan_2d.y) / zoom;
    let vp_rect = egui::Rect::from_min_size(
        egui::pos2(
            map_min.x + view_min_wx * scale,
            map_min.y + view_min_wy * scale,
        ),
        egui::vec2(view_w * scale, view_h * scale),
    );
    painter.rect_stroke(
        vp_rect,
        0.0,
        egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 180, 70)),
        egui::StrokeKind::Inside,
    );

    if (response.clicked() || response.dragged()) && response.interact_pointer_pos().is_some() {
        if let Some(pos) = response.interact_pointer_pos() {
            let target_wx = ((pos.x - map_min.x) / scale).clamp(0.0, pw);
            let target_wy = ((pos.y - map_min.y) / scale).clamp(0.0, ph);
            editor.pan_2d = egui::vec2(
                editor.last_canvas_size.x * 0.5 - target_wx * zoom,
                editor.last_canvas_size.y * 0.5 - target_wy * zoom,
            );
        }
    }
}

fn enable_curve_handles(wall: &Wall, anchor_index: usize, a: &mut WallAnchorPoint) {
    a.point_type = PointType::Smooth;
    a.handle_in_x = None;
    a.handle_in_y = None;
    a.handle_out_x = None;
    a.handle_out_y = None;

    let n = wall.anchors.len();
    if anchor_index > 0 || wall.is_closed_loop {
        let prev = &wall.anchors[(anchor_index + n - 1) % n];
        a.handle_in_x = Some((a.pos_x + prev.pos_x) * 0.5);
        a.handle_in_y = Some((a.pos_y + prev.pos_y) * 0.5 - 0.5);
    }
    if anchor_index + 1 < n || wall.is_closed_loop {
        let next = &wall.anchors[(anchor_index + 1) % n];
        a.handle_out_x = Some((a.pos_x + next.pos_x) * 0.5);
        a.handle_out_y = Some((a.pos_y + next.pos_y) * 0.5 + 0.5);
    }
}

fn shift_anchor(a: &mut WallAnchorPoint, sx: f64, sy: f64) {
    let dx = sx - a.pos_x;
    let dy = sy - a.pos_y;
    a.pos_x = sx;
    a.pos_y = sy;
    if let Some(ref mut ix) = a.handle_in_x {
        *ix += dx;
    }
    if let Some(ref mut iy) = a.handle_in_y {
        *iy += dy;
    }
    if let Some(ref mut ox) = a.handle_out_x {
        *ox += dx;
    }
    if let Some(ref mut oy) = a.handle_out_y {
        *oy += dy;
    }
}

fn snap_point_for_draw(wx: f64, wy: f64, editor: &EditorState) -> (f64, f64, bool) {
    let snap_radius = (14.0_f64 / f64::from(editor.zoom_2d)).max(0.25);
    let mut best: Option<(f64, f64, f64)> = None;
    for wall in &editor.project.walls {
        if wall.level_id != editor.active_level_id {
            continue;
        }
        for a in &wall.anchors {
            let d = ((wx - a.pos_x).powi(2) + (wy - a.pos_y).powi(2)).sqrt();
            if d <= snap_radius && best.as_ref().is_none_or(|(_, _, bd)| d < *bd) {
                best = Some((a.pos_x, a.pos_y, d));
            }
        }
    }
    if let Some((ax, ay, _)) = best {
        return (ax, ay, true);
    }
    (
        snap_grid(wx, editor.unit_system).clamp(0.0, editor.plan_width),
        snap_grid(wy, editor.unit_system).clamp(0.0, editor.plan_height),
        false,
    )
}

fn snap_point_for_drag(
    wx: f64,
    wy: f64,
    editor: &EditorState,
    drag: &DragAnchorState,
) -> (f64, f64) {
    let snap_radius = 0.20;
    for wall in &editor.project.walls {
        if wall.level_id != editor.active_level_id {
            continue;
        }
        for (idx, a) in wall.anchors.iter().enumerate() {
            if wall.id == drag.wall_id && idx == drag.anchor_index {
                continue;
            }
            if drag
                .linked_anchors
                .iter()
                .any(|(lw, li, _)| *lw == wall.id && *li == idx)
            {
                continue;
            }
            if ((wx - a.pos_x).powi(2) + (wy - a.pos_y).powi(2)).sqrt() <= snap_radius {
                return (a.pos_x, a.pos_y);
            }
        }
    }
    (
        snap_grid(wx, editor.unit_system).clamp(0.0, editor.plan_width),
        snap_grid(wy, editor.unit_system).clamp(0.0, editor.plan_height),
    )
}

fn snap_grid(v: f64, unit_system: UnitSystem) -> f64 {
    let step = match unit_system {
        UnitSystem::Metric => 0.1,
        UnitSystem::Imperial => METERS_PER_FOOT / 4.0, // 3 inches
    };
    (v / step).round() * step
}

fn point_to_segment_dist(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len_sq).clamp(0.0, 1.0);
    let proj_x = ax + t * dx;
    let proj_y = ay + t * dy;
    ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt()
}
