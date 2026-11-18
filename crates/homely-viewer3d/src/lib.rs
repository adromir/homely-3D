//! Dedicated 3D Bevy viewer plugin for Homely 3D (decoupled from 2D UI / egui).

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::window::{PresentMode, WindowCloseRequested, WindowRef, WindowResolution};
use homely_core::geometry::{compute_level_wall_offsets, Vec2 as CoreVec2};
use homely_core::rooms::{detect_rooms, triangulate_polygon};
use homely_core::Project;

/// Bevy plugin that spawns the `Homely 3D — 3D Viewer` window and keeps 3D geometry in sync.
pub struct Viewer3dPlugin;

impl Plugin for Viewer3dPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Viewer3dState>()
            .add_systems(Startup, setup_3d_viewer)
            .add_systems(Update, (rebuild_3d_scene, update_3d_camera_and_visibility));
    }
}

/// Synchronized state consumed by [`Viewer3dPlugin`].
#[derive(Resource)]
pub struct Viewer3dState {
    pub project: Project,
    pub selected_wall_id: Option<String>,
    pub plan_width: f64,
    pub plan_height: f64,
    pub dirty: bool,
    pub reset_camera: bool,
    pub visible: bool,
}

impl Default for Viewer3dState {
    fn default() -> Self {
        Self {
            project: Project::default(),
            selected_wall_id: None,
            plan_width: 20.0,
            plan_height: 15.0,
            dirty: true,
            reset_camera: false,
            visible: true,
        }
    }
}

/// Marker component for the dedicated 3D Viewer OS window.
#[derive(Component)]
pub struct Viewer3dWindow;

/// Marker component for dynamically generated 3D meshes (walls & room floors).
#[derive(Component)]
pub struct SceneMesh3d;

/// Orbit camera controller state.
#[derive(Component)]
pub struct OrbitCamera {
    pub focus: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl OrbitCamera {
    pub fn transform(&self) -> Transform {
        let rot = Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0);
        let pos = self.focus + rot * Vec3::new(0.0, 0.0, self.distance);
        Transform::from_translation(pos).looking_at(self.focus, Vec3::Y)
    }
}

fn setup_3d_viewer(mut commands: Commands) {
    let viewer_window = commands
        .spawn((
            Viewer3dWindow,
            Window {
                title: "Homely 3D — 3D Viewer".into(),
                resolution: WindowResolution::new(960, 720),
                present_mode: PresentMode::AutoVsync,
                ..default()
            },
        ))
        .id();

    let orbit = OrbitCamera {
        focus: Vec3::new(5.0, 1.0, 4.0),
        distance: 12.0,
        yaw: 0.6,
        pitch: -0.55,
    };
    let transform = orbit.transform();
    commands.spawn((
        Camera3d::default(),
        RenderTarget::Window(WindowRef::Entity(viewer_window)),
        transform,
        orbit,
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(12.0, 16.0, 12.0).looking_at(Vec3::new(5.0, 0.0, 4.0), Vec3::Y),
    ));
}

fn rebuild_3d_scene(
    mut commands: Commands,
    mut state: ResMut<Viewer3dState>,
    existing: Query<Entity, With<SceneMesh3d>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !state.dirty {
        return;
    }
    state.dirty = false;

    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let pw = state.plan_width.max(1.0) as f32;
    let ph = state.plan_height.max(1.0) as f32;
    commands.spawn((
        SceneMesh3d,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(pw, ph))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.24, 0.27),
            perceptual_roughness: 0.9,
            ..default()
        })),
        Transform::from_xyz(pw * 0.5, 0.0, ph * 0.5),
    ));

    let wall_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.86, 0.85, 0.82),
        perceptual_roughness: 0.8,
        ..default()
    });
    let selected_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.72, 0.95),
        perceptual_roughness: 0.6,
        ..default()
    });
    let floor_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.47, 0.38),
        perceptual_roughness: 0.75,
        ..default()
    });

    for level in &state.project.levels {
        let level_walls: Vec<_> = state
            .project
            .walls
            .iter()
            .filter(|w| w.level_id == level.id)
            .cloned()
            .collect();
        let level_elev = level.elevation as f32;

        for room in detect_rooms(&level_walls, level.height) {
            let tris = triangulate_polygon(&room.polygon);
            if tris.is_empty() {
                continue;
            }
            let positions: Vec<[f32; 3]> = room
                .polygon
                .iter()
                .map(|p| [p.x as f32, level_elev + 0.01, p.y as f32])
                .collect();
            let mut indices = Vec::with_capacity(tris.len() * 3);
            for tri in tris {
                indices.extend_from_slice(&[tri[0] as u32, tri[2] as u32, tri[1] as u32]);
            }
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_indices(Indices::U32(indices));
            mesh.duplicate_vertices();
            mesh.compute_flat_normals();

            commands.spawn((
                SceneMesh3d,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(floor_mat.clone()),
                Transform::IDENTITY,
            ));
        }

        let all_pairs = compute_level_wall_offsets(&level_walls, 0.02);
        for (wall, pairs) in level_walls.iter().zip(all_pairs.iter()) {
            if pairs.len() < 2 {
                continue;
            }
            let is_selected = state.selected_wall_id.as_deref() == Some(&wall.id);
            let mat = if is_selected {
                selected_mat.clone()
            } else {
                wall_mat.clone()
            };

            let y_bot = level_elev;
            let y_top = level_elev + ((wall.start_height + wall.end_height) * 0.5) as f32;
            if let Some(mesh) = build_mitered_wall_mesh(pairs, y_bot, y_top, wall.is_closed_loop) {
                commands.spawn((
                    SceneMesh3d,
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(mat),
                    Transform::IDENTITY,
                ));
            }
        }
    }
}

fn update_3d_camera_and_visibility(
    mut commands: Commands,
    mut state: ResMut<Viewer3dState>,
    mut close_events: MessageReader<WindowCloseRequested>,
    mut viewer_window: Query<(Entity, &mut Window), With<Viewer3dWindow>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut mouse_wheel: MessageReader<MouseWheel>,
    mut camera_query: Query<(&mut Transform, &mut OrbitCamera, &mut RenderTarget)>,
) {
    let existing_win = viewer_window.single_mut().ok();
    if let Some((win_entity, _)) = existing_win.as_ref() {
        for ev in close_events.read() {
            if ev.window == *win_entity {
                state.visible = false;
            }
        }
    } else {
        close_events.clear();
    }

    let Ok((mut transform, mut orbit, mut render_target)) = camera_query.single_mut() else {
        return;
    };

    if state.visible {
        if let Some((_, mut win)) = existing_win {
            if !win.visible {
                win.visible = true;
            }
        } else {
            // Re-spawn the 3D Viewer window if the user previously closed it with X
            let new_win = commands
                .spawn((
                    Viewer3dWindow,
                    Window {
                        title: "Homely 3D — 3D Viewer".into(),
                        resolution: WindowResolution::new(960, 720),
                        present_mode: PresentMode::AutoVsync,
                        ..default()
                    },
                ))
                .id();
            *render_target = RenderTarget::Window(WindowRef::Entity(new_win));
        }
    } else if let Some((_, mut win)) = existing_win {
        if win.visible {
            win.visible = false;
        }
    }

    if state.reset_camera {
        state.reset_camera = false;
        orbit.focus = Vec3::new((state.plan_width * 0.25) as f32, 1.0, (state.plan_height * 0.25) as f32);
        orbit.distance = 12.0;
        orbit.yaw = 0.6;
        orbit.pitch = -0.55;
    }

    let cursor_in_3d = viewer_window
        .single()
        .is_ok_and(|(_, w)| w.visible && w.cursor_position().is_some());

    if cursor_in_3d {
        if mouse_buttons.pressed(MouseButton::Right) {
            for ev in mouse_motion.read() {
                orbit.yaw -= ev.delta.x * 0.006;
                orbit.pitch = (orbit.pitch - ev.delta.y * 0.006).clamp(-1.45, -0.05);
            }
        } else {
            mouse_motion.clear();
        }

        for ev in mouse_wheel.read() {
            orbit.distance = (orbit.distance - ev.y * 0.8).clamp(2.0, 60.0);
        }
    } else {
        mouse_motion.clear();
        mouse_wheel.clear();
    }

    *transform = orbit.transform();
}

fn build_mitered_wall_mesh(
    pairs: &[(CoreVec2, CoreVec2)],
    y_bot: f32,
    y_top: f32,
    is_closed: bool,
) -> Option<Mesh> {
    if pairs.len() < 2 {
        return None;
    }

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let push_quad = |positions: &mut Vec<[f32; 3]>,
                     indices: &mut Vec<u32>,
                     a: [f32; 3],
                     b: [f32; 3],
                     c: [f32; 3],
                     d: [f32; 3]| {
        let base = positions.len() as u32;
        positions.extend_from_slice(&[a, b, c, d]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };

    for win in pairs.windows(2) {
        let (l0, r0) = win[0];
        let (l1, r1) = win[1];

        let bl0 = [l0.x as f32, y_bot, l0.y as f32];
        let br0 = [r0.x as f32, y_bot, r0.y as f32];
        let bl1 = [l1.x as f32, y_bot, l1.y as f32];
        let br1 = [r1.x as f32, y_bot, r1.y as f32];

        let tl0 = [l0.x as f32, y_top, l0.y as f32];
        let tr0 = [r0.x as f32, y_top, r0.y as f32];
        let tl1 = [l1.x as f32, y_top, l1.y as f32];
        let tr1 = [r1.x as f32, y_top, r1.y as f32];

        push_quad(&mut positions, &mut indices, tl0, tl1, tr1, tr0);
        push_quad(&mut positions, &mut indices, bl0, bl1, tl1, tl0);
        push_quad(&mut positions, &mut indices, br1, br0, tr0, tr1);
        push_quad(&mut positions, &mut indices, br0, br1, bl1, bl0);
    }

    let joined_ends = pairs.len() >= 3 && {
        let (l0, r0) = pairs[0];
        let (ln, rn) = pairs[pairs.len() - 1];
        ((l0.x - ln.x).powi(2) + (l0.y - ln.y).powi(2)) < 1e-8
            && ((r0.x - rn.x).powi(2) + (r0.y - rn.y).powi(2)) < 1e-8
    };

    if !is_closed && !joined_ends {
        let (l0, r0) = pairs[0];
        let (ln, rn) = pairs[pairs.len() - 1];
        push_quad(
            &mut positions,
            &mut indices,
            [r0.x as f32, y_bot, r0.y as f32],
            [l0.x as f32, y_bot, l0.y as f32],
            [l0.x as f32, y_top, l0.y as f32],
            [r0.x as f32, y_top, r0.y as f32],
        );
        push_quad(
            &mut positions,
            &mut indices,
            [ln.x as f32, y_bot, ln.y as f32],
            [rn.x as f32, y_bot, rn.y as f32],
            [rn.x as f32, y_top, rn.y as f32],
            [ln.x as f32, y_top, ln.y as f32],
        );
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(Indices::U32(indices));
    mesh.duplicate_vertices();
    mesh.compute_flat_normals();
    Some(mesh)
}
