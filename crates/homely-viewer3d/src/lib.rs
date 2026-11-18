//! Dedicated 3D Bevy viewer plugin for Homely 3D (decoupled from 2D UI / egui).

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{PresentMode, WindowCloseRequested, WindowRef, WindowResolution};
use homely_core::geometry::{
    compute_level_wall_offsets, polyline_length, sample_polyline_at_t, slice_offset_pairs,
    tessellate_wall_centerline, Vec2 as CoreVec2,
};
use homely_core::rooms::{detect_rooms, point_in_polygon, room_signature, triangulate_polygon};
use homely_core::{
    sample_terrain_height, Opening, OpeningType, PaintStyle, Project, SurfaceFinish,
    SurfaceTexture, TerrainSettings, TerrainTexture,
};

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
    mut images: ResMut<Assets<Image>>,
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

    // Collect ground-floor room polygons so terrain stays cleanly recessed beneath interior slabs
    let ground_rooms: Vec<Vec<CoreVec2>> = state
        .project
        .levels
        .iter()
        .filter(|l| l.elevation.abs() < 0.05)
        .flat_map(|l| {
            let lw: Vec<_> = state
                .project
                .walls
                .iter()
                .filter(|w| w.level_id == l.id)
                .cloned()
                .collect();
            detect_rooms(&lw, l.height)
                .into_iter()
                .map(|r| r.polygon)
        })
        .collect();

    let terrain_mesh = build_terrain_mesh(pw, ph, &state.project.terrain, &ground_rooms);
    let terrain_mat = create_terrain_material(&state.project.terrain, &mut images, &mut materials);
    commands.spawn((
        SceneMesh3d,
        Mesh3d(meshes.add(terrain_mesh)),
        MeshMaterial3d(terrain_mat),
        Transform::IDENTITY,
    ));

    let foundation_mat = materials.add(StandardMaterial {
        base_color: Color::srgb_u8(116, 120, 126),
        perceptual_roughness: 0.92,
        ..default()
    });

    let selected_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.72, 0.95),
        perceptual_roughness: 0.6,
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
            let sig = room_signature(&room.polygon);
            let floor_finish = state
                .project
                .room_finishes
                .get(&sig)
                .cloned()
                .unwrap_or_else(SurfaceFinish::default_floor);
            let floor_mat = create_finish_material(&floor_finish, 2.5, true, &mut images, &mut materials);

            let positions: Vec<[f32; 3]> = room
                .polygon
                .iter()
                .map(|p| [p.x as f32, level_elev + 0.01, p.y as f32])
                .collect();
            let uvs: Vec<[f32; 2]> = room
                .polygon
                .iter()
                .map(|p| [(p.x as f32) * 0.5, (p.y as f32) * 0.5])
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
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_indices(Indices::U32(indices));
            mesh.duplicate_vertices();
            mesh.compute_flat_normals();

            commands.spawn((
                SceneMesh3d,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(floor_mat),
                Transform::IDENTITY,
            ));

            // Automatic room foundation slab beneath ground-level rooms
            if state.project.terrain.auto_foundation && level_elev.abs() < 0.05 {
                let mut min_h = 0.0_f32;
                for p in &room.polygon {
                    let h = sample_terrain_height(&state.project.terrain, p.x, p.y) as f32;
                    min_h = min_h.min(h);
                }
                let found_bot =
                    (min_h - state.project.terrain.foundation_depth_m).min(-0.20);
                if let Some(fmesh) = build_room_foundation_mesh(&room.polygon, found_bot, level_elev) {
                    commands.spawn((
                        SceneMesh3d,
                        Mesh3d(meshes.add(fmesh)),
                        MeshMaterial3d(foundation_mat.clone()),
                        Transform::IDENTITY,
                    ));
                }
            }
        }

        // Custom floor zones (e.g., tile kitchen island, parquet rug) rendered slightly above base room floor
        for zone in state.project.floor_zones.iter().filter(|z| z.level_id == level.id) {
            let zw = (zone.max_x - zone.min_x).abs().max(0.1) as f32;
            let zd = (zone.max_y - zone.min_y).abs().max(0.1) as f32;
            let cx = ((zone.min_x + zone.max_x) * 0.5) as f32;
            let cz = ((zone.min_y + zone.max_y) * 0.5) as f32;
            let zone_finish = SurfaceFinish {
                texture: zone.texture,
                secondary_texture: None,
                paint_style: PaintStyle::Solid,
                primary_color: zone.color,
                secondary_color: zone.color,
                trim_width_m: 0.12,
            };
            let zone_mat = create_finish_material(&zone_finish, 2.5, true, &mut images, &mut materials);
            let mut zmesh = Mesh::from(Plane3d::default().mesh().size(zw, zd));
            if let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) =
                zmesh.attribute_mut(Mesh::ATTRIBUTE_UV_0)
            {
                for uv in uvs.iter_mut() {
                    uv[0] *= zw * 0.5;
                    uv[1] *= zd * 0.5;
                }
            }
            commands.spawn((
                SceneMesh3d,
                Mesh3d(meshes.add(zmesh)),
                MeshMaterial3d(zone_mat),
                Transform::from_xyz(cx, level_elev + 0.015, cz),
            ));
        }

        let all_pairs = compute_level_wall_offsets(&level_walls, 0.02);
        for (wall, pairs) in level_walls.iter().zip(all_pairs.iter()) {
            if pairs.len() < 2 {
                continue;
            }
            let is_selected = state.selected_wall_id.as_deref() == Some(&wall.id);
            let y_bot = level_elev;
            let wall_h = ((wall.start_height + wall.end_height) * 0.5).max(0.2) as f32;
            let y_top = level_elev + wall_h;

            let (mat_a, mat_b, mat_cap) = if is_selected {
                (selected_mat.clone(), selected_mat.clone(), selected_mat.clone())
            } else {
                let ma = create_finish_material(&wall.side_a_finish, wall_h, false, &mut images, &mut materials);
                let mb = create_finish_material(&wall.side_b_finish, wall_h, false, &mut images, &mut materials);
                let mc = materials.add(StandardMaterial {
                    base_color: Color::srgb_u8(
                        wall.side_a_finish.primary_color[0],
                        wall.side_a_finish.primary_color[1],
                        wall.side_a_finish.primary_color[2],
                    ),
                    perceptual_roughness: 0.85,
                    ..default()
                });
                (ma, mb, mc)
            };

            let spawn_wall_meshes = |commands: &mut Commands,
                                     meshes: &mut Assets<Mesh>,
                                     sub_pairs: &[(CoreVec2, CoreVec2)],
                                     yb: f32,
                                     yt: f32,
                                     closed: bool| {
                let (m_a, m_b, m_cap) = build_mitered_wall_meshes(sub_pairs, yb, yt, y_bot, y_top, closed);
                if let Some(m) = m_a {
                    commands.spawn((SceneMesh3d, Mesh3d(meshes.add(m)), MeshMaterial3d(mat_a.clone()), Transform::IDENTITY));
                }
                if let Some(m) = m_b {
                    commands.spawn((SceneMesh3d, Mesh3d(meshes.add(m)), MeshMaterial3d(mat_b.clone()), Transform::IDENTITY));
                }
                if let Some(m) = m_cap {
                    commands.spawn((SceneMesh3d, Mesh3d(meshes.add(m)), MeshMaterial3d(mat_cap.clone()), Transform::IDENTITY));
                }
            };

            let centerline = tessellate_wall_centerline(wall, 0.02);
            let total_len = polyline_length(&centerline);

            // Automatic wall foundation footing under ground-level walls
            if state.project.terrain.auto_foundation && level_elev.abs() < 0.05 {
                let mut min_h = 0.0_f32;
                for p in &centerline {
                    let h = sample_terrain_height(&state.project.terrain, p.x, p.y) as f32;
                    min_h = min_h.min(h);
                }
                let found_bot =
                    (min_h - state.project.terrain.foundation_depth_m).min(-0.20);
                let (fa, fb, fcap) =
                    build_mitered_wall_meshes(pairs, found_bot, level_elev, found_bot, level_elev, wall.is_closed_loop);
                for m_opt in [fa, fb, fcap] {
                    if let Some(m) = m_opt {
                        commands.spawn((
                            SceneMesh3d,
                            Mesh3d(meshes.add(m)),
                            MeshMaterial3d(foundation_mat.clone()),
                            Transform::IDENTITY,
                        ));
                    }
                }
            }

            let mut wall_openings: Vec<&Opening> = state
                .project
                .openings
                .iter()
                .filter(|o| o.wall_id == wall.id)
                .collect();

            if wall_openings.is_empty() || total_len < 1e-4 {
                spawn_wall_meshes(&mut commands, &mut meshes, pairs, y_bot, y_top, wall.is_closed_loop);
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
                    spawn_wall_meshes(&mut commands, &mut meshes, &sub_pairs, y_bot, y_top, false);
                }

                // Sill below opening + Lintel above opening
                if t1 - t0 > 1e-4 {
                    let op_pairs = slice_offset_pairs(&centerline, pairs, t0, t1);
                    let op_bot = (y_bot + op.elevation as f32).clamp(y_bot, y_top);
                    let op_top = (op_bot + op.height as f32).clamp(op_bot, y_top);

                    if op_bot - y_bot > 0.01 {
                        spawn_wall_meshes(&mut commands, &mut meshes, &op_pairs, y_bot, op_bot, false);
                    }
                    if y_top - op_top > 0.01 {
                        spawn_wall_meshes(&mut commands, &mut meshes, &op_pairs, op_top, y_top, false);
                    }

                    // Door leaf / Window glass pane + optional OpeningTrim casing
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

                        // OpeningTrim casing around doors/windows
                        if wall.side_a_finish.paint_style == PaintStyle::OpeningTrim
                            || wall.side_b_finish.paint_style == PaintStyle::OpeningTrim
                        {
                            let trim_col = if wall.side_a_finish.paint_style == PaintStyle::OpeningTrim {
                                wall.side_a_finish.secondary_color
                            } else {
                                wall.side_b_finish.secondary_color
                            };
                            let trim_mat = materials.add(StandardMaterial {
                                base_color: Color::srgb_u8(trim_col[0], trim_col[1], trim_col[2]),
                                perceptual_roughness: 0.55,
                                ..default()
                            });
                            let tw = wall.side_a_finish.trim_width_m.clamp(0.04, 0.25);
                            let casing_d = (wall.thickness as f32) + 0.03;
                            let rot = Quat::from_rotation_y(angle);
                            let right_dir = Vec3::new(dir.x as f32, 0.0, dir.y as f32).normalize_or_zero();
                            let cx = mid.x as f32;
                            let cz = mid.y as f32;

                            // Top lintel casing
                            commands.spawn((
                                SceneMesh3d,
                                Mesh3d(meshes.add(Cuboid::new(op_w + tw * 2.0, tw, casing_d))),
                                MeshMaterial3d(trim_mat.clone()),
                                Transform::from_xyz(cx, op_top + tw * 0.5, cz).with_rotation(rot),
                            ));
                            // Left & right jamb casings
                            for sign in [-1.0_f32, 1.0_f32] {
                                let off = right_dir * (sign * (op_w * 0.5 + tw * 0.5));
                                commands.spawn((
                                    SceneMesh3d,
                                    Mesh3d(meshes.add(Cuboid::new(tw, op_h, casing_d))),
                                    MeshMaterial3d(trim_mat.clone()),
                                    Transform::from_xyz(cx + off.x, op_bot + op_h * 0.5, cz + off.z)
                                        .with_rotation(rot),
                                ));
                            }
                        }
                    }
                }

                cursor_t = t1;
            }

            // Solid wall after last opening
            if 1.0 - cursor_t > 1e-4 {
                let tail_pairs = slice_offset_pairs(&centerline, pairs, cursor_t, 1.0);
                spawn_wall_meshes(&mut commands, &mut meshes, &tail_pairs, y_bot, y_top, false);
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

fn texture_factor(tex: SurfaceTexture, px: u32, py: u32, noise: f32) -> f32 {
    match tex {
        SurfaceTexture::Smooth => 1.0 + noise * 0.03,
        SurfaceTexture::Plaster => 1.0 + noise * 0.14,
        SurfaceTexture::Brick => {
            let row = (py / 16) % 2;
            let bx = (px + row * 16) % 32;
            let by = py % 16;
            if bx < 2 || by < 2 {
                0.72 + noise * 0.05
            } else {
                0.96 + noise * 0.08
            }
        }
        SurfaceTexture::WoodPlanks => {
            let plank = py / 16;
            let seam = py % 16 == 0 || ((px + plank * 37) % 64 == 0);
            let grain = ((px as f32 * 0.12 + plank as f32 * 1.7).sin()) * 0.04;
            if seam {
                0.74
            } else {
                0.98 + grain + noise * 0.05
            }
        }
        SurfaceTexture::Tile => {
            if px % 32 < 2 || py % 32 < 2 {
                0.76
            } else {
                1.0 + noise * 0.03
            }
        }
        SurfaceTexture::Concrete => {
            let panel_seam = px == 0 || py % 64 == 0;
            if panel_seam {
                0.82
            } else {
                0.96 + noise * 0.11
            }
        }
    }
}

fn create_finish_material(
    finish: &SurfaceFinish,
    wall_height: f32,
    is_floor: bool,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    let size = 128u32;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let c1 = [
        finish.primary_color[0] as f32,
        finish.primary_color[1] as f32,
        finish.primary_color[2] as f32,
    ];
    let c2 = [
        finish.secondary_color[0] as f32,
        finish.secondary_color[1] as f32,
        finish.secondary_color[2] as f32,
    ];
    let trim_frac = (finish.trim_width_m / wall_height.max(0.5)).clamp(0.02, 0.45);
    let tex2 = finish.secondary_texture.unwrap_or(finish.texture);

    for py in 0..size {
        // v = 0 at top of image (y_top on walls), 1 at bottom of image (y_bot on walls)
        let v_top_down = (py as f32 + 0.5) / size as f32;
        let h_frac = 1.0 - v_top_down; // 0 = floor/bottom, 1 = ceiling/top

        for px in 0..size {
            let u = (px as f32 + 0.5) / size as f32;

            // Deterministic pseudo-noise for organic surface textures
            let hash = ((px.wrapping_mul(374761393) ^ py.wrapping_mul(668265263))
                .wrapping_mul(1274126177))
                >> 24;
            let noise = (hash as f32) / 255.0 - 0.5;

            let tf1 = texture_factor(finish.texture, px, py, noise);
            let tf2 = texture_factor(tex2, px, py, noise);

            let (base_rgb, tex_factor) = if is_floor {
                match finish.paint_style {
                    PaintStyle::Solid | PaintStyle::OpeningTrim => (c1, tf1),
                    PaintStyle::BaseboardTrim | PaintStyle::CrownTrim => {
                        if u < trim_frac * 0.5
                            || u > 1.0 - trim_frac * 0.5
                            || v_top_down < trim_frac * 0.5
                            || v_top_down > 1.0 - trim_frac * 0.5
                        {
                            (c2, tf2)
                        } else {
                            (c1, tf1)
                        }
                    }
                    PaintStyle::TwoTone => {
                        if ((px / 32) + (py / 32)) % 2 == 0 {
                            (c1, tf1)
                        } else {
                            (c2, tf2)
                        }
                    }
                    PaintStyle::Gradient => {
                        let t = (u + v_top_down) * 0.5;
                        (
                            [
                                c1[0] * (1.0 - t) + c2[0] * t,
                                c1[1] * (1.0 - t) + c2[1] * t,
                                c1[2] * (1.0 - t) + c2[2] * t,
                            ],
                            tf1 * (1.0 - t) + tf2 * t,
                        )
                    }
                }
            } else {
                match finish.paint_style {
                    PaintStyle::Solid | PaintStyle::OpeningTrim => (c1, tf1),
                    PaintStyle::BaseboardTrim => {
                        if h_frac <= trim_frac {
                            (c2, tf2)
                        } else if (h_frac - trim_frac).abs() < 0.012 {
                            ([c2[0] * 0.85, c2[1] * 0.85, c2[2] * 0.85], tf2)
                        } else {
                            (c1, tf1)
                        }
                    }
                    PaintStyle::CrownTrim => {
                        if h_frac >= 1.0 - trim_frac {
                            (c2, tf2)
                        } else if (h_frac - (1.0 - trim_frac)).abs() < 0.012 {
                            ([c2[0] * 0.85, c2[1] * 0.85, c2[2] * 0.85], tf2)
                        } else {
                            (c1, tf1)
                        }
                    }
                    PaintStyle::TwoTone => {
                        if h_frac < 0.45 {
                            (c2, tf2)
                        } else if (h_frac - 0.45).abs() < 0.015 {
                            ([c2[0] * 0.82, c2[1] * 0.82, c2[2] * 0.82], tf2)
                        } else {
                            (c1, tf1)
                        }
                    }
                    PaintStyle::Gradient => (
                        [
                            c2[0] * (1.0 - h_frac) + c1[0] * h_frac,
                            c2[1] * (1.0 - h_frac) + c1[1] * h_frac,
                            c2[2] * (1.0 - h_frac) + c1[2] * h_frac,
                        ],
                        tf2 * (1.0 - h_frac) + tf1 * h_frac,
                    ),
                }
            };

            data.push((base_rgb[0] * tex_factor).clamp(0.0, 255.0) as u8);
            data.push((base_rgb[1] * tex_factor).clamp(0.0, 255.0) as u8);
            data.push((base_rgb[2] * tex_factor).clamp(0.0, 255.0) as u8);
            data.push(255);
        }
    }

    let mut img = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: if is_floor {
            ImageAddressMode::Repeat
        } else {
            ImageAddressMode::ClampToEdge
        },
        ..default()
    });
    let tex = images.add(img);
    let roughness = match finish.texture {
        SurfaceTexture::Tile => 0.35,
        SurfaceTexture::Smooth => 0.65,
        SurfaceTexture::WoodPlanks => 0.60,
        SurfaceTexture::Plaster | SurfaceTexture::Concrete => 0.85,
        SurfaceTexture::Brick => 0.90,
    };
    materials.add(StandardMaterial {
        base_color_texture: Some(tex),
        perceptual_roughness: roughness,
        ..default()
    })
}

fn build_mitered_wall_meshes(
    pairs: &[(CoreVec2, CoreVec2)],
    y_bot: f32,
    y_top: f32,
    wall_y_bot: f32,
    wall_y_top: f32,
    is_closed: bool,
) -> (Option<Mesh>, Option<Mesh>, Option<Mesh>) {
    if pairs.len() < 2 {
        return (None, None, None);
    }

    let wall_h = (wall_y_top - wall_y_bot).max(0.01);
    let v_bot = 1.0 - ((y_bot - wall_y_bot) / wall_h).clamp(0.0, 1.0);
    let v_top = 1.0 - ((y_top - wall_y_bot) / wall_h).clamp(0.0, 1.0);

    let mut pos_a: Vec<[f32; 3]> = Vec::new();
    let mut uv_a: Vec<[f32; 2]> = Vec::new();
    let mut idx_a: Vec<u32> = Vec::new();

    let mut pos_b: Vec<[f32; 3]> = Vec::new();
    let mut uv_b: Vec<[f32; 2]> = Vec::new();
    let mut idx_b: Vec<u32> = Vec::new();

    let mut pos_cap: Vec<[f32; 3]> = Vec::new();
    let mut uv_cap: Vec<[f32; 2]> = Vec::new();
    let mut idx_cap: Vec<u32> = Vec::new();

    let push_quad = |positions: &mut Vec<[f32; 3]>,
                     uvs: &mut Vec<[f32; 2]>,
                     indices: &mut Vec<u32>,
                     a: [f32; 3],
                     b: [f32; 3],
                     c: [f32; 3],
                     d: [f32; 3],
                     ua: [f32; 2],
                     ub: [f32; 2],
                     uc: [f32; 2],
                     ud: [f32; 2]| {
        let base = positions.len() as u32;
        positions.extend_from_slice(&[a, b, c, d]);
        uvs.extend_from_slice(&[ua, ub, uc, ud]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };

    let mut u_acc = 0.0_f32;
    for win in pairs.windows(2) {
        let (l0, r0) = win[0];
        let (l1, r1) = win[1];
        let seg_len = (((l1.x - l0.x).powi(2) + (l1.y - l0.y).powi(2)).sqrt() as f32) * 0.5;
        let u0 = u_acc;
        let u1 = u_acc + seg_len;
        u_acc = u1;

        let bl0 = [l0.x as f32, y_bot, l0.y as f32];
        let br0 = [r0.x as f32, y_bot, r0.y as f32];
        let bl1 = [l1.x as f32, y_bot, l1.y as f32];
        let br1 = [r1.x as f32, y_bot, r1.y as f32];

        let tl0 = [l0.x as f32, y_top, l0.y as f32];
        let tr0 = [r0.x as f32, y_top, r0.y as f32];
        let tl1 = [l1.x as f32, y_top, l1.y as f32];
        let tr1 = [r1.x as f32, y_top, r1.y as f32];

        // Side A (left face)
        push_quad(
            &mut pos_a, &mut uv_a, &mut idx_a,
            bl0, bl1, tl1, tl0,
            [u0, v_bot], [u1, v_bot], [u1, v_top], [u0, v_top],
        );
        // Side B (right face)
        push_quad(
            &mut pos_b, &mut uv_b, &mut idx_b,
            br1, br0, tr0, tr1,
            [u1, v_bot], [u0, v_bot], [u0, v_top], [u1, v_top],
        );
        // Top & bottom caps
        push_quad(
            &mut pos_cap, &mut uv_cap, &mut idx_cap,
            tl0, tl1, tr1, tr0,
            [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0],
        );
        push_quad(
            &mut pos_cap, &mut uv_cap, &mut idx_cap,
            br0, br1, bl1, bl0,
            [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0],
        );
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
            &mut pos_cap, &mut uv_cap, &mut idx_cap,
            [r0.x as f32, y_bot, r0.y as f32],
            [l0.x as f32, y_bot, l0.y as f32],
            [l0.x as f32, y_top, l0.y as f32],
            [r0.x as f32, y_top, r0.y as f32],
            [0.0, v_bot], [1.0, v_bot], [1.0, v_top], [0.0, v_top],
        );
        push_quad(
            &mut pos_cap, &mut uv_cap, &mut idx_cap,
            [ln.x as f32, y_bot, ln.y as f32],
            [rn.x as f32, y_bot, rn.y as f32],
            [rn.x as f32, y_top, rn.y as f32],
            [ln.x as f32, y_top, ln.y as f32],
            [0.0, v_bot], [1.0, v_bot], [1.0, v_top], [0.0, v_top],
        );
    }

    let finalize = |positions: Vec<[f32; 3]>, uvs: Vec<[f32; 2]>, indices: Vec<u32>| -> Option<Mesh> {
        if indices.is_empty() {
            return None;
        }
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(Indices::U32(indices));
        mesh.duplicate_vertices();
        mesh.compute_flat_normals();
        Some(mesh)
    };

    (
        finalize(pos_a, uv_a, idx_a),
        finalize(pos_b, uv_b, idx_b),
        finalize(pos_cap, uv_cap, idx_cap),
    )
}

fn build_terrain_mesh(
    pw: f32,
    ph: f32,
    settings: &TerrainSettings,
    ground_rooms: &[Vec<CoreVec2>],
) -> Mesh {
    let cols = ((pw * 2.0).round() as usize).clamp(12, 80);
    let rows = ((ph * 2.0).round() as usize).clamp(12, 80);

    let mut positions = Vec::with_capacity((cols + 1) * (rows + 1));
    let mut uvs = Vec::with_capacity((cols + 1) * (rows + 1));
    let mut indices = Vec::with_capacity(cols * rows * 6);

    for r in 0..=rows {
        let vz = r as f32 / rows as f32;
        let z = vz * ph;
        for c in 0..=cols {
            let ux = c as f32 / cols as f32;
            let x = ux * pw;
            let mut h = sample_terrain_height(settings, f64::from(x), f64::from(z)) as f32;
            if settings.enabled {
                let p = CoreVec2::new(f64::from(x), f64::from(z));
                if ground_rooms.iter().any(|poly| point_in_polygon(p, poly)) {
                    h = h.min(-0.04);
                }
            }
            positions.push([x, h, z]);
            uvs.push([x * 0.4, z * 0.4]);
        }
    }

    let stride = cols + 1;
    for r in 0..rows {
        for c in 0..cols {
            let i0 = (r * stride + c) as u32;
            let i1 = i0 + 1;
            let i2 = ((r + 1) * stride + c) as u32;
            let i3 = i2 + 1;
            indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh.compute_normals();
    mesh
}

fn create_terrain_material(
    settings: &TerrainSettings,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    let size = 128u32;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let tint = [
        settings.tint_color[0] as f32,
        settings.tint_color[1] as f32,
        settings.tint_color[2] as f32,
    ];

    for py in 0..size {
        for px in 0..size {
            let h1 = ((px.wrapping_mul(374761393) ^ py.wrapping_mul(668265263))
                .wrapping_mul(1274126177))
                >> 24;
            let n1 = (h1 as f32) / 255.0 - 0.5;
            let h2 = (((px / 4).wrapping_mul(2246822519) ^ (py / 4).wrapping_mul(3266489917))
                .wrapping_mul(668265263))
                >> 24;
            let n2 = (h2 as f32) / 255.0 - 0.5;

            let (r, g, b) = match settings.texture {
                TerrainTexture::Grass => {
                    let blade = ((px as f32 * 0.7 + py as f32 * 0.3).sin()) * 0.06;
                    let f = 1.0 + n1 * 0.22 + n2 * 0.14 + blade;
                    (tint[0] * f, tint[1] * f, tint[2] * f)
                }
                TerrainTexture::Meadow => {
                    let flower = if h1 > 246 { 1.35 } else { 1.0 };
                    let f = (1.0 + n1 * 0.20 + n2 * 0.16) * flower;
                    (
                        (tint[0] * f + if h1 > 246 { 65.0 } else { 0.0 }),
                        (tint[1] * f + if h1 > 246 { 55.0 } else { 0.0 }),
                        tint[2] * f,
                    )
                }
                TerrainTexture::SoilEarth => {
                    let f = 0.85 + n1 * 0.25 + n2 * 0.18;
                    (124.0 * f, 90.0 * f, 60.0 * f)
                }
                TerrainTexture::Sand => {
                    let ripple = ((py as f32 * 0.25 + (px as f32 * 0.08).sin() * 2.0).sin()) * 0.05;
                    let f = 0.96 + n1 * 0.10 + ripple;
                    (216.0 * f, 194.0 * f, 146.0 * f)
                }
                TerrainTexture::Gravel => {
                    let pebble = if h1 > 210 { 1.18 } else if h1 < 45 { 0.78 } else { 0.96 };
                    let f = pebble + n2 * 0.10;
                    (145.0 * f, 144.0 * f, 140.0 * f)
                }
                TerrainTexture::StoneRock => {
                    let crack = if ((px + py * 2) % 37) < 2 { 0.72 } else { 1.0 };
                    let f = crack * (0.94 + n1 * 0.16 + n2 * 0.18);
                    (122.0 * f, 126.0 * f, 130.0 * f)
                }
            };

            data.push(r.clamp(0.0, 255.0) as u8);
            data.push(g.clamp(0.0, 255.0) as u8);
            data.push(b.clamp(0.0, 255.0) as u8);
            data.push(255);
        }
    }

    let mut img = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..default()
    });
    materials.add(StandardMaterial {
        base_color_texture: Some(images.add(img)),
        perceptual_roughness: 0.94,
        ..default()
    })
}

fn build_room_foundation_mesh(poly: &[CoreVec2], y_bot: f32, y_top: f32) -> Option<Mesh> {
    if poly.len() < 3 || y_top - y_bot <= 0.01 {
        return None;
    }
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let base = positions.len() as u32;
        positions.extend_from_slice(&[
            [a.x as f32, y_bot, a.y as f32],
            [b.x as f32, y_bot, b.y as f32],
            [b.x as f32, y_top, b.y as f32],
            [a.x as f32, y_top, a.y as f32],
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
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
