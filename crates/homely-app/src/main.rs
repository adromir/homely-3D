//! Homely 3D desktop application launcher (2D Planner + decoupled 3D Viewer plugin).

mod i18n;
mod state;
mod theme;
mod ui;

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::window::ExitCondition;
use bevy_egui::{EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext};
use homely_viewer3d::{Viewer3dPlugin, Viewer3dState};
use state::EditorState;
use ui::{draw_dock_ui, DockLayout};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Homely 3D — 2D Planner".into(),
                ..default()
            }),
            exit_condition: ExitCondition::OnPrimaryClosed,
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .add_plugins(Viewer3dPlugin)
        .init_resource::<EditorState>()
        .init_resource::<DockLayout>()
        .add_systems(Startup, setup_2d_ui_camera)
        .add_systems(EguiPrimaryContextPass, draw_dock_ui)
        .add_systems(Update, sync_editor_to_viewer3d)
        .run();
}

fn setup_2d_ui_camera(
    mut commands: Commands,
    mut egui_global_settings: ResMut<EguiGlobalSettings>,
) {
    egui_global_settings.auto_create_primary_context = false;
    commands.spawn((PrimaryEguiContext, Camera2d, RenderLayers::none()));
}

fn sync_editor_to_viewer3d(
    mut editor: ResMut<EditorState>,
    mut viewer: ResMut<Viewer3dState>,
    mut prev_editor_visible: Local<Option<bool>>,
    mut prev_viewer_visible: Local<Option<bool>>,
) {
    if editor.dirty_3d {
        editor.dirty_3d = false;
        viewer.project = editor.project.clone();
        viewer.selected_wall_id = editor.selected_wall_id.clone();
        viewer.plan_width = editor.plan_width;
        viewer.plan_height = editor.plan_height;
        viewer.dirty = true;
    }
    if editor.reset_3d_camera {
        editor.reset_3d_camera = false;
        viewer.reset_camera = true;
    }

    let prev_ed = prev_editor_visible.unwrap_or(true);
    let prev_vw = prev_viewer_visible.unwrap_or(true);
    if editor.show_3d_viewer != prev_ed {
        viewer.visible = editor.show_3d_viewer;
    } else if viewer.visible != prev_vw {
        editor.show_3d_viewer = viewer.visible;
    }
    *prev_editor_visible = Some(editor.show_3d_viewer);
    *prev_viewer_visible = Some(viewer.visible);
}
