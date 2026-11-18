//! Homely 3D desktop application launcher (2D Planner + decoupled 3D Viewer plugin).

mod i18n;
mod state;
mod theme;
mod ui;

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::window::{ExitCondition, WindowCreated};
use bevy::winit::WINIT_WINDOWS;
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
        .add_systems(Update, (sync_editor_to_viewer3d, apply_window_icons))
        .run();
}

fn apply_window_icons(mut created: MessageReader<WindowCreated>) {
    let events: Vec<Entity> = created.read().map(|e| e.window).collect();
    if events.is_empty() {
        return;
    }
    let (rgba, w, h) = theme::load_logo_rgba();
    let Ok(icon) = winit::window::Icon::from_rgba(rgba, w, h) else {
        return;
    };
    WINIT_WINDOWS.with_borrow(|winit_windows| {
        for entity in events {
            if let Some(win) = winit_windows.get_window(entity) {
                win.set_window_icon(Some(icon.clone()));
            }
        }
    });
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
    if editor.env_dirty_3d {
        editor.env_dirty_3d = false;
        viewer.sun_azimuth_deg = editor.sun_azimuth_deg;
        viewer.sun_elevation_deg = editor.sun_elevation_deg;
        viewer.sun_intensity = editor.sun_intensity;
        viewer.sun_shadows = editor.sun_shadows;
        viewer.sky_preset = editor.sky_preset;
        viewer.env_dirty = true;
    }
    if editor.reset_3d_camera {
        editor.reset_3d_camera = false;
        viewer.reset_camera = true;
    }
    if let Some(preset) = editor.camera_preset_3d.take() {
        viewer.camera_preset = Some(preset);
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
