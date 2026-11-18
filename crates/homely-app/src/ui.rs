//! `egui_dock` layout, modifier-driven 2D floorplan editor, context menu, and inspector.

use crate::i18n::{tr, Lang};
use crate::state::{
    corner, ContextMenuKind, ContextMenuState, DragAnchorState, DragHandleKind, EditorState,
};
use bevy::app::AppExit;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use homely_core::command::ProjectCommand;
use homely_core::geometry::{compute_level_wall_offsets, tessellate_wall_centerline};
use homely_core::rooms::{detect_rooms, triangulate_polygon};
use homely_core::sh3x::{load_sh3x, save_sh3x};
use homely_core::{PointType, Wall, WallAnchorPoint};
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
) -> Result {
    let ctx = contexts.ctx_mut()?;
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
        delete_selected_point(&mut editor);
    } else if esc_key {
        editor.active_draw_from = None;
        editor.context_menu = None;
    }

    let mut root_ui = egui::Ui::new(
        ctx.clone(),
        "homely_root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    // Classic flat top menu bar
    egui::Panel::top("menu_bar")
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(32, 35, 42))
                .inner_margin(egui::Margin::symmetric(8, 3)),
        )
        .show(&mut root_ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(s.menu_file, |ui| {
                    if ui.button(s.menu_new).clicked() {
                        editor.reset_new_project();
                        editor.status = s.menu_new.into();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(s.menu_open_h3d).clicked() {
                        let p = editor.project_path.clone();
                        load_project_from(&mut editor, &p);
                        ui.close();
                    }
                    if ui.button(s.menu_save_h3d).clicked() {
                        let p = editor.project_path.clone();
                        save_project_to(&mut editor, &p);
                        ui.close();
                    }
                    if ui.button(s.menu_import_sh3x).clicked() {
                        let p = editor.import_sh3x_path.clone();
                        load_project_from(&mut editor, &p);
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

    // Left Tool Palette (no Undo/Redo buttons; focused on point/wall actions & modifier hints)
    egui::Panel::left("left_tools")
        .exact_size(156.0)
        .show(&mut root_ui, |ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(s.tools).strong().size(13.0));
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
                egui::RichText::new(s.hint_ctrl)
                    .size(11.0)
                    .color(egui::Color32::from_gray(170)),
            );
        });

    egui::Panel::bottom("status_bar").show(&mut root_ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(&editor.status);
        });
    });

    egui::CentralPanel::no_frame().show(&mut root_ui, |ui| {
        let mut viewer = HomelyTabViewer {
            editor: &mut editor,
        };
        DockArea::new(&mut dock.state)
            .show_close_buttons(false)
            .style(Style::from_egui(ui.style().as_ref()))
            .show_inside(ui, &mut viewer);
    });

    draw_context_menu(ctx, &mut editor);

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

    let max_mx = editor.plan_width.floor() as i32;
    let max_my = editor.plan_height.floor() as i32;

    if zoom >= 28.0 {
        let minor_color = egui::Color32::from_rgb(36, 41, 48);
        let max_dx = (editor.plan_width * 10.0).round() as i32;
        for dx in 0..=max_dx {
            if dx % 10 == 0 {
                continue;
            }
            let x = origin.x + (dx as f32) * zoom * 0.1;
            painter.line_segment(
                [egui::pos2(x, plan_rect.top()), egui::pos2(x, plan_rect.bottom())],
                egui::Stroke::new(1.0, minor_color),
            );
        }
        let max_dy = (editor.plan_height * 10.0).round() as i32;
        for dy in 0..=max_dy {
            if dy % 10 == 0 {
                continue;
            }
            let y = origin.y + (dy as f32) * zoom * 0.1;
            painter.line_segment(
                [egui::pos2(plan_rect.left(), y), egui::pos2(plan_rect.right(), y)],
                egui::Stroke::new(1.0, minor_color),
            );
        }
    }

    let grid_color = egui::Color32::from_rgb(50, 56, 66);
    for gx in 0..=max_mx {
        let x = origin.x + (gx as f32) * zoom;
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
        let y = origin.y + (gy as f32) * zoom;
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
        painter.text(
            cp,
            egui::Align2::CENTER_CENTER,
            format!("Room {}\n{:.2} m²", r_idx + 1, room.area_sqm),
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgba_unmultiplied(220, 235, 225, 220),
        );
    }

    // Existing walls (neighbor-aware mitered corners)
    let all_pairs = compute_level_wall_offsets(&level_walls, 0.01);
    for (wall, pairs) in level_walls.iter().zip(all_pairs.iter()) {
        let is_selected = editor.selected_wall_id.as_deref() == Some(&wall.id);
        let fill = if is_selected {
            egui::Color32::from_rgba_unmultiplied(100, 175, 245, 175)
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
    }

    let pointer_pos = response.hover_pos().filter(|p| canvas_rect.contains(*p));
    let ctrl_held = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);

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
    let (primary_pressed, primary_down, secondary_clicked) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.secondary_clicked(),
        )
    });

    // Right-click exits building mode if active, otherwise opens context menu on point or wall
    if response.hovered() && secondary_clicked {
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
            if let Some((wid, _)) = find_wall_at_world(wx, wy, editor) {
                editor.selected_wall_id = Some(wid.clone());
                editor.selected_anchor_index = None;
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
            if ctrl_held {
                handle_ctrl_left_click(editor, pos, world_to_screen, screen_to_world);
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
                if let Some((wid, _)) = find_wall_at_world(wx, wy, editor) {
                    editor.selected_wall_id = Some(wid.clone());
                    editor.selected_anchor_index = None;
                    editor.dirty_3d = true;
                    editor.status = format!("Selected {wid}.");
                }
            }

            editor.drag_state = started_drag;
        }
    }

    // Active drag
    if primary_down {
        if let (Some(drag), Some(pos)) = (editor.drag_state.clone(), pointer_pos) {
            let (wx, wy) = screen_to_world(pos);
            let (sx, sy) = match drag.kind {
                DragHandleKind::Anchor => snap_point_for_drag(wx, wy, editor, &drag),
                _ => (
                    snap_10cm(wx).clamp(0.0, editor.plan_width),
                    snap_10cm(wy).clamp(0.0, editor.plan_height),
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
        let sx = snap_10cm(wx).clamp(0.0, editor.plan_width);
        let sy = snap_10cm(wy).clamp(0.0, editor.plan_height);
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
        level_id: wall.level_id.clone(),
        is_closed_loop: false,
        thickness: wall.thickness,
        start_height: wall.start_height,
        end_height: wall.end_height,
        anchors: vec![a0, corner(&mid_id1, sx, sy)],
    };
    let w2 = Wall {
        id: w2_id,
        level_id: wall.level_id,
        is_closed_loop: false,
        thickness: wall.thickness,
        start_height: wall.start_height,
        end_height: wall.end_height,
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
                                        &[toggle_label, s.ctx_continue_wall, s.ctx_delete_point],
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
                            let w = measure_menu_width(ctx, &[s.ctx_insert_point, s.ctx_delete_wall]);
                            ui.set_width(w);
                            if context_menu_item(ui, s.ctx_insert_point, w) {
                                let sx = snap_10cm(wx).clamp(0.0, editor.plan_width);
                                let sy = snap_10cm(wy).clamp(0.0, editor.plan_height);
                                split_wall_at_point(editor, wall_id, sx, sy);
                                close_menu = true;
                            }
                            if context_menu_item(ui, s.ctx_delete_wall, w) {
                                editor.execute_cmd(ProjectCommand::RemoveWall {
                                    wall_id: wall_id.clone(),
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
            level_id: w1.level_id,
            is_closed_loop: false,
            thickness: w1.thickness,
            start_height: w1.start_height,
            end_height: w1.end_height,
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
    pointer_pos: Option<egui::Pos2>,
    screen_to_world: &impl Fn(egui::Pos2) -> (f64, f64),
) {
    let ruler_bg = egui::Color32::from_rgb(35, 39, 46);
    let ruler_border = egui::Color32::from_rgb(65, 72, 84);
    let text_color = egui::Color32::from_rgb(185, 192, 204);
    let font = egui::FontId::monospace(10.5);

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
        "m",
        font.clone(),
        text_color,
    );

    let step = if zoom < 22.0 { 2 } else { 1 };
    for gx in min_gx..=max_gx {
        if gx % step != 0 {
            continue;
        }
        let x = origin.x + (gx as f32) * zoom;
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
        let y = origin.y + (gy as f32) * zoom;
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
        let sx = snap_10cm(cx);
        let sy = snap_10cm(cy);
        let label = format!("X: {cx:.2} m   Y: {cy:.2} m   (Snap 10cm: {sx:.2}, {sy:.2})");
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
    let desired_size = egui::vec2(140.0, 32.0);
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

fn draw_inspector(ui: &mut egui::Ui, editor: &mut EditorState) {
    let s = tr(editor.lang);
    draw_navigator_overview(ui, editor);
    ui.separator();

    ui.heading(s.plan_size);
    let mut size_changed = false;
    ui.horizontal(|ui| {
        ui.label("W:");
        size_changed |= ui
            .add(
                egui::DragValue::new(&mut editor.plan_width)
                    .speed(0.5)
                    .range(2.0..=200.0)
                    .suffix(" m"),
            )
            .changed();
        ui.label("H:");
        size_changed |= ui
            .add(
                egui::DragValue::new(&mut editor.plan_height)
                    .speed(0.5)
                    .range(2.0..=200.0)
                    .suffix(" m"),
            )
            .changed();
    });
    if size_changed {
        editor.dirty_3d = true;
    }

    ui.separator();
    ui.heading(s.project);
    ui.label(format!("{}: {}", s.name, editor.project.name));
    ui.label(format!("{}: {}", s.walls, editor.project.walls.len()));

    ui.horizontal(|ui| {
        ui.label(format!("{}:", s.file));
        ui.text_edit_singleline(&mut editor.project_path);
    });
    ui.horizontal(|ui| {
        if ui.button(s.save_h3d).clicked() {
            let p = editor.project_path.clone();
            save_project_to(editor, &p);
        }
        if ui.button(s.load_h3d).clicked() {
            let p = editor.project_path.clone();
            load_project_from(editor, &p);
        }
    });
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut editor.import_sh3x_path);
        if ui.button(s.import_sh3x).clicked() {
            let p = editor.import_sh3x_path.clone();
            load_project_from(editor, &p);
        }
    });

    ui.separator();
    ui.heading(s.new_wall_defaults);
    ui.add(egui::Slider::new(&mut editor.wall_thickness, 0.08..=0.60).text(format!("{} (m)", s.thickness)));
    ui.add(egui::Slider::new(&mut editor.wall_height, 1.0..=5.0).text(format!("{} (m)", s.height)));

    ui.separator();
    ui.heading(s.walls);
    let mut remove_id: Option<String> = None;
    for wall in &editor.project.walls {
        let selected = editor.selected_wall_id.as_deref() == Some(&wall.id);
        ui.horizontal(|ui| {
            if ui.selectable_label(selected, &wall.id).clicked() {
                editor.selected_wall_id = Some(wall.id.clone());
                editor.dirty_3d = true;
            }
            if ui.small_button("X").clicked() {
                remove_id = Some(wall.id.clone());
            }
        });
    }
    if let Some(wall_id) = remove_id {
        editor.execute_cmd(ProjectCommand::RemoveWall { wall_id });
    }

    if let Some(sel_id) = editor.selected_wall_id.clone() {
        if let Some(wall) = editor.project.walls.iter().find(|w| w.id == sel_id).cloned() {
            ui.separator();
            ui.heading(format!("Edit {}", wall.id));

            let mut thickness = wall.thickness;
            let mut start_h = wall.start_height;
            let mut end_h = wall.end_height;
            let mut props_changed = false;

            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.thickness));
                props_changed |= ui
                    .add(egui::DragValue::new(&mut thickness).speed(0.01).range(0.05..=1.0).suffix(" m"))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label(format!("{}:", s.height));
                props_changed |= ui
                    .add(egui::DragValue::new(&mut start_h).speed(0.05).range(0.5..=8.0).suffix(" m"))
                    .changed();
                end_h = start_h;
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
                editor.execute_cmd(ProjectCommand::UpdateWallAnchor {
                    wall_id: wall.id,
                    anchor_index: idx,
                    anchor,
                });
            }
        }
    }
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
        snap_10cm(wx).clamp(0.0, editor.plan_width),
        snap_10cm(wy).clamp(0.0, editor.plan_height),
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
        snap_10cm(wx).clamp(0.0, editor.plan_width),
        snap_10cm(wy).clamp(0.0, editor.plan_height),
    )
}

fn snap_10cm(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
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
