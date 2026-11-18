//! Dedicated 3D Bevy viewer plugin for Homely 3D (decoupled from 2D UI / egui).

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{PresentMode, WindowCloseRequested, WindowRef, WindowResolution};
use homely_core::geometry::{
    compute_level_wall_offsets, polyline_length, sample_polyline_at_t, slice_offset_pairs,
    tessellate_wall_centerline, Vec2 as CoreVec2,
};
use homely_core::rooms::{detect_rooms, triangulate_polygon};
use homely_core::{Opening, OpeningType, Project};

/// Bevy plugin that spawns the `Homely 3D — 3D Viewer` window and keeps 3D geometry in sync.
pub struct Viewer3dPlugin;

impl Plugin for Viewer3dPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Viewer3dState>()
            .add_systems(Startup, setup_3d_viewer)
            .add_systems(
                Update,
                (
                    rebuild_3d_scene,
                    update_3d_environment,
                    update_3d_camera_and_visibility,
                ),
            );
    }
}

/// Skybox / atmosphere preset for the 3D viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkyPreset {
    #[default]
    ClearDay,
    GoldenHour,
    Overcast,
    Night,
}

/// One-click camera angle presets for the 3D viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraPreset {
    Iso,
    TopDown,
    Front,
    Side,
}

/// Synchronized state consumed by [`Viewer3dPlugin`].
#[derive(Resource)]
pub struct Viewer3dState {
    pub project: Project,
    pub selected_wall_id: Option<String>,
    pub plan_width: f64,
    pub plan_height: f64,
    pub dirty: bool,
    pub env_dirty: bool,
    pub reset_camera: bool,
    pub camera_preset: Option<CameraPreset>,
    pub orbit_delta: Vec2,
    pub pan_delta: Vec2,
    pub zoom_delta: f32,
    pub visible: bool,
    pub ui_wants_pointer: bool,
    pub sun_azimuth_deg: f32,
    pub sun_elevation_deg: f32,
    pub sun_intensity: f32,
    pub sun_shadows: bool,
    pub sky_preset: SkyPreset,
}

impl Default for Viewer3dState {
    fn default() -> Self {
        Self {
            project: Project::default(),
            selected_wall_id: None,
            plan_width: 20.0,
            plan_height: 15.0,
            dirty: true,
            env_dirty: true,
            reset_camera: false,
            camera_preset: None,
            orbit_delta: Vec2::ZERO,
            pan_delta: Vec2::ZERO,
            zoom_delta: 0.0,
            visible: true,
            ui_wants_pointer: false,
            sun_azimuth_deg: 45.0,
            sun_elevation_deg: 50.0,
            sun_intensity: 16_000.0,
            sun_shadows: true,
            sky_preset: SkyPreset::ClearDay,
        }
    }
}

/// Marker component for the dedicated 3D Viewer OS window.
#[derive(Component)]
pub struct Viewer3dWindow;

/// Marker component for the 3D Viewer camera.
#[derive(Component)]
pub struct Viewer3dCamera;

/// Marker component for dynamically generated 3D meshes (walls & room floors).
#[derive(Component)]
pub struct SceneMesh3d;

/// Marker component for the configurable sun directional light.
#[derive(Component)]
pub struct SunLight;

/// Marker component for the procedural textured sky dome.
#[derive(Component)]
pub struct SkyDome;

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

fn setup_3d_viewer(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    state: Res<Viewer3dState>,
) {
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
        Viewer3dCamera,
        Camera3d::default(),
        RenderTarget::Window(WindowRef::Entity(viewer_window)),
        transform,
        orbit,
    ));

    let sun_dir = sun_direction(state.sun_azimuth_deg, state.sun_elevation_deg);
    let target = Vec3::new((state.plan_width * 0.5) as f32, 0.0, (state.plan_height * 0.5) as f32);
    commands.spawn((
        SunLight,
        DirectionalLight {
            illuminance: state.sun_intensity,
            shadow_maps_enabled: state.sun_shadows,
            color: sun_color(state.sky_preset, state.sun_elevation_deg),
            ..default()
        },
        Transform::from_translation(target + sun_dir * 30.0).looking_at(target, Vec3::Y),
    ));

    let sky_img = generate_sky_texture(state.sky_preset, state.sun_azimuth_deg, state.sun_elevation_deg);
    let sky_tex = images.add(sky_img);
    let sky_mat = materials.add(StandardMaterial {
        base_color_texture: Some(sky_tex),
        unlit: true,
        cull_mode: None,
        ..default()
    });
    commands.spawn((
        SkyDome,
        Mesh3d(meshes.add(Sphere::new(180.0).mesh().uv(48, 24))),
        MeshMaterial3d(sky_mat),
        Transform::from_translation(target),
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
    let door_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.52, 0.34, 0.20),
        perceptual_roughness: 0.65,
        ..default()
    });
    let glass_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.55, 0.80, 0.95, 0.42),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.15,
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
            let centerline = tessellate_wall_centerline(wall, 0.02);
            let total_len = polyline_length(&centerline);

            let mut wall_openings: Vec<&Opening> = state
                .project
                .openings
                .iter()
                .filter(|o| o.wall_id == wall.id)
                .collect();

            if wall_openings.is_empty() || total_len < 1e-4 {
                if let Some(mesh) = build_mitered_wall_mesh(pairs, y_bot, y_top, wall.is_closed_loop) {
                    commands.spawn((
                        SceneMesh3d,
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(mat),
                        Transform::IDENTITY,
                    ));
                }
                continue;
            }

            // Compute normalized [t0, t1] spans for each opening and sort left-to-right
            wall_openings.sort_by(|a, b| a.segment_t.total_cmp(&b.segment_t));
            let mut cursor_t = 0.0_f64;

            for op in wall_openings {
                let half_t = (op.width * 0.5 / total_len).clamp(0.01, 0.49);
                let t0 = (op.segment_t - half_t).clamp(cursor_t, 1.0);
                let t1 = (op.segment_t + half_t).clamp(t0, 1.0);

                // Solid wall before opening
                if t0 - cursor_t > 1e-4 {
                    let sub_pairs = slice_offset_pairs(&centerline, pairs, cursor_t, t0);
                    if let Some(mesh) = build_mitered_wall_mesh(&sub_pairs, y_bot, y_top, false) {
                        commands.spawn((
                            SceneMesh3d,
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(mat.clone()),
                            Transform::IDENTITY,
                        ));
                    }
                }

                // Sill below opening + Lintel above opening
                if t1 - t0 > 1e-4 {
                    let op_pairs = slice_offset_pairs(&centerline, pairs, t0, t1);
                    let op_bot = (y_bot + op.elevation as f32).clamp(y_bot, y_top);
                    let op_top = (op_bot + op.height as f32).clamp(op_bot, y_top);

                    if op_bot - y_bot > 0.01 {
                        if let Some(mesh) = build_mitered_wall_mesh(&op_pairs, y_bot, op_bot, false) {
                            commands.spawn((
                                SceneMesh3d,
                                Mesh3d(meshes.add(mesh)),
                                MeshMaterial3d(mat.clone()),
                                Transform::IDENTITY,
                            ));
                        }
                    }
                    if y_top - op_top > 0.01 {
                        if let Some(mesh) = build_mitered_wall_mesh(&op_pairs, op_top, y_top, false) {
                            commands.spawn((
                                SceneMesh3d,
                                Mesh3d(meshes.add(mesh)),
                                MeshMaterial3d(mat.clone()),
                                Transform::IDENTITY,
                            ));
                        }
                    }

                    // Door leaf or Window glass pane inside opening
                    if let Some((mid, dir)) = sample_polyline_at_t(&centerline, (t0 + t1) * 0.5) {
                        let op_h = (op_top - op_bot).max(0.05);
                        let op_w = ((t1 - t0) * total_len) as f32;
                        let angle = -(dir.y as f32).atan2(dir.x as f32);
                        let (pane_mat, pane_thick) = match op.opening_type {
                            OpeningType::Door => (door_mat.clone(), 0.05_f32),
                            OpeningType::Window => (glass_mat.clone(), 0.03_f32),
                            OpeningType::Cutout => (glass_mat.clone(), 0.0_f32),
                        };
                        if pane_thick > 0.0 {
                            commands.spawn((
                                SceneMesh3d,
                                Mesh3d(meshes.add(Cuboid::new(op_w, op_h, pane_thick))),
                                MeshMaterial3d(pane_mat),
                                Transform::from_xyz(mid.x as f32, op_bot + op_h * 0.5, mid.y as f32)
                                    .with_rotation(Quat::from_rotation_y(angle)),
                            ));
                        }
                    }
                }

                cursor_t = t1;
            }

            // Solid wall after last opening
            if 1.0 - cursor_t > 1e-4 {
                let tail_pairs = slice_offset_pairs(&centerline, pairs, cursor_t, 1.0);
                if let Some(mesh) = build_mitered_wall_mesh(&tail_pairs, y_bot, y_top, false) {
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
}

fn sun_direction(azimuth_deg: f32, elevation_deg: f32) -> Vec3 {
    let az = azimuth_deg.to_radians();
    let el = elevation_deg.clamp(2.0, 89.0).to_radians();
    Vec3::new(el.cos() * az.cos(), el.sin(), el.cos() * az.sin()).normalize()
}

fn sun_color(preset: SkyPreset, elevation_deg: f32) -> Color {
    match preset {
        SkyPreset::ClearDay => {
            if elevation_deg < 20.0 {
                Color::srgb(1.0, 0.86, 0.70)
            } else {
                Color::srgb(1.0, 0.98, 0.94)
            }
        }
        SkyPreset::GoldenHour => Color::srgb(1.0, 0.72, 0.45),
        SkyPreset::Overcast => Color::srgb(0.86, 0.90, 0.95),
        SkyPreset::Night => Color::srgb(0.45, 0.58, 0.85),
    }
}

fn sky_palette(preset: SkyPreset) -> ([f32; 3], [f32; 3], [f32; 3], f32) {
    // (zenith, horizon, ground, ambient_brightness)
    match preset {
        SkyPreset::ClearDay => ([0.18, 0.46, 0.86], [0.68, 0.84, 0.96], [0.22, 0.25, 0.28], 380.0),
        SkyPreset::GoldenHour => ([0.20, 0.30, 0.56], [0.95, 0.62, 0.38], [0.24, 0.19, 0.18], 260.0),
        SkyPreset::Overcast => ([0.48, 0.53, 0.59], [0.74, 0.77, 0.81], [0.25, 0.26, 0.28], 420.0),
        SkyPreset::Night => ([0.03, 0.05, 0.12], [0.09, 0.13, 0.24], [0.04, 0.05, 0.08], 90.0),
    }
}

fn generate_sky_texture(preset: SkyPreset, azimuth_deg: f32, elevation_deg: f32) -> Image {
    let width = 256u32;
    let height = 128u32;
    let (zenith, horizon, ground, _) = sky_palette(preset);
    let sun_dir = sun_direction(azimuth_deg, elevation_deg);
    let sun_tint: [f32; 3] = match preset {
        SkyPreset::ClearDay => [1.0, 0.96, 0.86],
        SkyPreset::GoldenHour => [1.0, 0.72, 0.38],
        SkyPreset::Overcast => [0.88, 0.90, 0.92],
        SkyPreset::Night => [0.65, 0.76, 0.98],
    };

    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        // v = 0 at top (zenith), 1 at bottom (nadir)
        let v = (y as f32 + 0.5) / height as f32;
        let theta = v * std::f32::consts::PI; // 0 = +Y zenith, PI = -Y nadir
        let dir_y = theta.cos();
        let sin_theta = theta.sin();

        let base = if dir_y >= 0.0 {
            let t = (1.0 - dir_y).powf(2.2);
            [
                zenith[0] * (1.0 - t) + horizon[0] * t,
                zenith[1] * (1.0 - t) + horizon[1] * t,
                zenith[2] * (1.0 - t) + horizon[2] * t,
            ]
        } else {
            let t = (-dir_y).min(1.0).powf(0.6);
            [
                horizon[0] * (1.0 - t) + ground[0] * t,
                horizon[1] * (1.0 - t) + ground[1] * t,
                horizon[2] * (1.0 - t) + ground[2] * t,
            ]
        };

        for x in 0..width {
            let u = (x as f32 + 0.5) / width as f32;
            let phi = u * std::f32::consts::TAU;
            let dir = Vec3::new(sin_theta * phi.cos(), dir_y, sin_theta * phi.sin());
            let cos_sun = dir.dot(sun_dir).max(0.0);

            let glow = match preset {
                SkyPreset::Overcast => cos_sun.powf(12.0) * 0.14,
                SkyPreset::Night => cos_sun.powf(220.0) * 0.65 + cos_sun.powf(18.0) * 0.08,
                _ => cos_sun.powf(260.0) * 0.95 + cos_sun.powf(14.0) * 0.25,
            };

            let r = (base[0] + sun_tint[0] * glow).clamp(0.0, 1.0);
            let g = (base[1] + sun_tint[1] * glow).clamp(0.0, 1.0);
            let b = (base[2] + sun_tint[2] * glow).clamp(0.0, 1.0);
            data.push((r * 255.0) as u8);
            data.push((g * 255.0) as u8);
            data.push((b * 255.0) as u8);
            data.push(255);
        }
    }

    Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn update_3d_environment(
    mut state: ResMut<Viewer3dState>,
    mut sun_q: Query<(&mut DirectionalLight, &mut Transform), With<SunLight>>,
    sky_q: Query<&MeshMaterial3d<StandardMaterial>, With<SkyDome>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut clear_color: ResMut<ClearColor>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    if !state.env_dirty {
        return;
    }
    state.env_dirty = false;

    let target = Vec3::new(
        (state.plan_width * 0.5) as f32,
        0.0,
        (state.plan_height * 0.5) as f32,
    );
    let sun_dir = sun_direction(state.sun_azimuth_deg, state.sun_elevation_deg);
    let s_color = sun_color(state.sky_preset, state.sun_elevation_deg);
    let (_, horizon, _, amb_brightness) = sky_palette(state.sky_preset);

    clear_color.0 = Color::srgb(horizon[0], horizon[1], horizon[2]);
    ambient.color = s_color;
    ambient.brightness = amb_brightness;

    if let Ok((mut light, mut transform)) = sun_q.single_mut() {
        light.illuminance = if state.sky_preset == SkyPreset::Night {
            (state.sun_intensity * 0.12).max(200.0)
        } else if state.sky_preset == SkyPreset::Overcast {
            state.sun_intensity * 0.55
        } else {
            state.sun_intensity
        };
        light.color = s_color;
        light.shadow_maps_enabled = state.sun_shadows;
        *transform = Transform::from_translation(target + sun_dir * 35.0).looking_at(target, Vec3::Y);
    }

    let sky_img = generate_sky_texture(state.sky_preset, state.sun_azimuth_deg, state.sun_elevation_deg);
    let sky_tex = images.add(sky_img);
    for mat_handle in &sky_q {
        if let Some(mut mat) = materials.get_mut(&mat_handle.0) {
            mat.base_color_texture = Some(sky_tex.clone());
        }
    }
}

fn update_3d_camera_and_visibility(
    mut commands: Commands,
    mut state: ResMut<Viewer3dState>,
    mut close_events: MessageReader<WindowCloseRequested>,
    mut viewer_window: Query<(Entity, &mut Window), With<Viewer3dWindow>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
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

    let center = Vec3::new(
        (state.plan_width * 0.25) as f32,
        1.0,
        (state.plan_height * 0.25) as f32,
    );
    if state.reset_camera {
        state.reset_camera = false;
        orbit.focus = center;
        orbit.distance = 12.0;
        orbit.yaw = 0.6;
        orbit.pitch = -0.55;
    }
    if let Some(preset) = state.camera_preset.take() {
        orbit.focus = center;
        match preset {
            CameraPreset::Iso => {
                orbit.distance = 14.0;
                orbit.yaw = 0.65;
                orbit.pitch = -0.55;
            }
            CameraPreset::TopDown => {
                orbit.distance = 18.0;
                orbit.yaw = 0.0;
                orbit.pitch = -1.48;
            }
            CameraPreset::Front => {
                orbit.distance = 14.0;
                orbit.yaw = 0.0;
                orbit.pitch = -0.22;
            }
            CameraPreset::Side => {
                orbit.distance = 14.0;
                orbit.yaw = std::f32::consts::FRAC_PI_2;
                orbit.pitch = -0.22;
            }
        }
    }

    // Apply UI button navigation deltas (orbit / pan / zoom)
    if state.orbit_delta != Vec2::ZERO {
        orbit.yaw += state.orbit_delta.x;
        orbit.pitch = (orbit.pitch + state.orbit_delta.y).clamp(-1.48, 0.20);
        state.orbit_delta = Vec2::ZERO;
    }
    if state.pan_delta != Vec2::ZERO {
        let right = Vec3::new(orbit.yaw.cos(), 0.0, -orbit.yaw.sin());
        let forward = Vec3::new(-orbit.yaw.sin(), 0.0, -orbit.yaw.cos());
        orbit.focus += right * state.pan_delta.x + forward * state.pan_delta.y;
        state.pan_delta = Vec2::ZERO;
    }
    if state.zoom_delta.abs() > 1e-6 {
        orbit.distance = (orbit.distance + state.zoom_delta).clamp(1.5, 80.0);
        state.zoom_delta = 0.0;
    }

    let cursor_in_3d = viewer_window
        .single()
        .is_ok_and(|(_, w)| w.visible && w.cursor_position().is_some());

    if cursor_in_3d && !state.ui_wants_pointer {
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let panning = mouse_buttons.pressed(MouseButton::Middle)
            || (shift && mouse_buttons.pressed(MouseButton::Right));
        let orbiting = !shift && mouse_buttons.pressed(MouseButton::Right);

        if panning {
            let right = Vec3::new(orbit.yaw.cos(), 0.0, -orbit.yaw.sin());
            let forward = Vec3::new(-orbit.yaw.sin(), 0.0, -orbit.yaw.cos());
            let pan_scale = orbit.distance * 0.0018;
            for ev in mouse_motion.read() {
                orbit.focus -= right * (ev.delta.x * pan_scale);
                orbit.focus += forward * (ev.delta.y * pan_scale);
            }
        } else if orbiting {
            for ev in mouse_motion.read() {
                orbit.yaw -= ev.delta.x * 0.006;
                orbit.pitch = (orbit.pitch - ev.delta.y * 0.006).clamp(-1.48, 0.20);
            }
        } else {
            mouse_motion.clear();
        }

        // Keyboard WASD / Arrow panning while cursor is inside 3D window
        let dt = time.delta_secs();
        let speed = (orbit.distance * 0.85).max(3.0) * dt;
        let right = Vec3::new(orbit.yaw.cos(), 0.0, -orbit.yaw.sin());
        let forward = Vec3::new(-orbit.yaw.sin(), 0.0, -orbit.yaw.cos());
        if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
            orbit.focus += forward * speed;
        }
        if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
            orbit.focus -= forward * speed;
        }
        if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
            orbit.focus += right * speed;
        }
        if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
            orbit.focus -= right * speed;
        }

        for ev in mouse_wheel.read() {
            orbit.distance = (orbit.distance - ev.y * 0.9).clamp(1.5, 80.0);
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
