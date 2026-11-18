//! Editor state and project command helpers for `homely-app`.

use crate::i18n::Lang;
use bevy::prelude::*;
use bevy_egui::egui;
use homely_core::command::{CommandHistory, ProjectCommand};
use homely_core::{Level, PointType, Project, Wall, WallAnchorPoint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragHandleKind {
    Anchor,
    HandleIn,
    HandleOut,
}

#[derive(Debug, Clone)]
pub struct DragAnchorState {
    pub wall_id: String,
    pub anchor_index: usize,
    pub kind: DragHandleKind,
    pub original_anchor: WallAnchorPoint,
    /// Other `(wall_id, anchor_index, original_anchor)` sharing the exact same corner position.
    pub linked_anchors: Vec<(String, usize, WallAnchorPoint)>,
}

#[derive(Debug, Clone)]
pub enum ContextMenuKind {
    Point {
        wall_id: String,
        anchor_index: usize,
    },
    Wall {
        wall_id: String,
        wx: f64,
        wy: f64,
    },
    Opening {
        opening_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct ContextMenuState {
    pub screen_pos: egui::Pos2,
    pub kind: ContextMenuKind,
}

/// Central Bevy resource holding the active project, undo/redo history, and 2D/3D editor state.
#[derive(Resource)]
pub struct EditorState {
    pub lang: Lang,
    pub project: Project,
    pub history: CommandHistory,
    pub active_level_id: String,
    pub selected_wall_id: Option<String>,
    pub selected_anchor_index: Option<usize>,
    pub selected_opening_id: Option<String>,
    pub drag_state: Option<DragAnchorState>,
    /// Active start point when chaining new 2-point walls with `Ctrl + Left-click`.
    pub active_draw_from: Option<(f64, f64)>,
    pub context_menu: Option<ContextMenuState>,
    pub wall_thickness: f32,
    pub wall_height: f32,
    pub plan_width: f64,
    pub plan_height: f64,
    pub pan_2d: egui::Vec2,
    pub zoom_2d: f32,
    pub last_canvas_size: egui::Vec2,
    pub dirty_3d: bool,
    pub reset_3d_camera: bool,
    pub show_3d_viewer: bool,
    pub project_path: String,
    pub import_sh3x_path: String,
    pub status: String,
    next_id: u64,
}

impl Default for EditorState {
    fn default() -> Self {
        let mut project = Project {
            id: "proj-default".into(),
            name: "Homely 3D Project".into(),
            version: "0.1.0".into(),
            ..Default::default()
        };
        let mut history = CommandHistory::default();

        let level = Level {
            id: "lvl-0".into(),
            name: "Ground Floor".into(),
            elevation: 0.0,
            height: 2.6,
            floor_slab_thickness: 0.2,
            sort_order: 0,
        };
        let _ = history.execute(&mut project, ProjectCommand::AddLevel(level));

        // Starter L-corner made of two 2-point walls sharing corner (7.0, 2.0)
        let wall1 = Wall {
            id: "wall-1".into(),
            level_id: "lvl-0".into(),
            is_closed_loop: false,
            thickness: 0.24,
            start_height: 2.6,
            end_height: 2.6,
            anchors: vec![corner("a-1", 2.0, 2.0), corner("a-2", 7.0, 2.0)],
        };
        let wall2 = Wall {
            id: "wall-2".into(),
            level_id: "lvl-0".into(),
            is_closed_loop: false,
            thickness: 0.24,
            start_height: 2.6,
            end_height: 2.6,
            anchors: vec![
                corner("a-3", 7.0, 2.0),
                WallAnchorPoint {
                    id: "a-4".into(),
                    pos_x: 7.0,
                    pos_y: 5.5,
                    handle_in_x: Some(8.0),
                    handle_in_y: Some(3.75),
                    handle_out_x: None,
                    handle_out_y: None,
                    point_type: PointType::Smooth,
                },
            ],
        };
        let _ = history.execute(&mut project, ProjectCommand::AddWall(wall1));
        let _ = history.execute(&mut project, ProjectCommand::AddWall(wall2));

        Self {
            lang: Lang::De,
            project,
            history,
            active_level_id: "lvl-0".into(),
            selected_wall_id: Some("wall-1".into()),
            selected_anchor_index: None,
            selected_opening_id: None,
            drag_state: None,
            active_draw_from: None,
            context_menu: None,
            wall_thickness: 0.24,
            wall_height: 2.6,
            plan_width: 20.0,
            plan_height: 15.0,
            pan_2d: egui::Vec2::ZERO,
            zoom_2d: 45.0,
            last_canvas_size: egui::vec2(600.0, 500.0),
            dirty_3d: true,
            reset_3d_camera: false,
            show_3d_viewer: true,
            project_path: "homely_project.h3d".into(),
            import_sh3x_path: "import.sh3x".into(),
            status: "Bereit — Linksklick: Auswählen/Ziehen | Strg+Klick: Wand/Punkt | Rechtsklick: Menü".into(),
            next_id: 100,
        }
    }
}

impl EditorState {
    pub fn reset_new_project(&mut self) {
        let mut project = Project {
            id: "proj-default".into(),
            name: "Homely 3D Project".into(),
            version: "0.1.0".into(),
            ..Default::default()
        };
        let mut history = CommandHistory::default();
        let level = Level {
            id: "lvl-0".into(),
            name: "Ground Floor".into(),
            elevation: 0.0,
            height: 2.6,
            floor_slab_thickness: 0.2,
            sort_order: 0,
        };
        let _ = history.execute(&mut project, ProjectCommand::AddLevel(level));
        self.project = project;
        self.history = history;
        self.active_level_id = "lvl-0".into();
        self.selected_wall_id = None;
        self.selected_anchor_index = None;
        self.selected_opening_id = None;
        self.drag_state = None;
        self.active_draw_from = None;
        self.context_menu = None;
        self.dirty_3d = true;
    }

    pub fn alloc_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{}", self.next_id)
    }

    pub fn execute_cmd(&mut self, cmd: ProjectCommand) {
        if let ProjectCommand::RemoveWall { ref wall_id } = cmd {
            if self.selected_wall_id.as_deref() == Some(wall_id.as_str()) {
                self.selected_wall_id = None;
                self.selected_anchor_index = None;
            }
        }
        if let ProjectCommand::RemoveOpening { ref opening_id } = cmd {
            if self.selected_opening_id.as_deref() == Some(opening_id.as_str()) {
                self.selected_opening_id = None;
            }
        }
        match self.history.execute(&mut self.project, cmd) {
            Ok(()) => {
                self.dirty_3d = true;
            }
            Err(err) => {
                self.status = format!("Error: {err}");
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo(&mut self) {
        self.active_draw_from = None;
        match self.history.undo(&mut self.project) {
            Ok(true) => {
                self.dirty_3d = true;
                self.selected_anchor_index = None;
                self.status = "Undo.".into();
            }
            Ok(false) => {}
            Err(err) => self.status = format!("Undo error: {err}"),
        }
    }

    pub fn redo(&mut self) {
        self.active_draw_from = None;
        match self.history.redo(&mut self.project) {
            Ok(true) => {
                self.dirty_3d = true;
                self.selected_anchor_index = None;
                self.status = "Redo.".into();
            }
            Ok(false) => {}
            Err(err) => self.status = format!("Redo error: {err}"),
        }
    }

    /// Creates a new 2-point wall between `(x0, y0)` and `(x1, y1)`.
    pub fn create_two_point_wall(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) -> String {
        let wall_id = self.alloc_id("wall");
        let a0 = self.alloc_id("anchor");
        let a1 = self.alloc_id("anchor");
        let wall = Wall {
            id: wall_id.clone(),
            level_id: self.active_level_id.clone(),
            is_closed_loop: false,
            thickness: f64::from(self.wall_thickness),
            start_height: f64::from(self.wall_height),
            end_height: f64::from(self.wall_height),
            anchors: vec![corner(&a0, x0, y0), corner(&a1, x1, y1)],
        };
        self.execute_cmd(ProjectCommand::AddWall(wall));
        self.selected_wall_id = Some(wall_id.clone());
        self.selected_anchor_index = Some(1);
        wall_id
    }
}

pub fn corner(id: &str, x: f64, y: f64) -> WallAnchorPoint {
    WallAnchorPoint {
        id: id.into(),
        pos_x: x,
        pos_y: y,
        handle_in_x: None,
        handle_in_y: None,
        handle_out_x: None,
        handle_out_y: None,
        point_type: PointType::Corner,
    }
}
