//! SQLite persistence for [`Project`] matching the Homely 3D schema.

use crate::{
    Asset, AssetKind, CoreError, FurnitureItem, Level, Opening, OpeningType, PointType, Project,
    Transform3D, Wall, WallAnchorPoint,
};
use rusqlite::{params, Connection};
use std::path::Path;

const SCHEMA_SQL: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS project_metadata (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS assets (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('model', 'texture_albedo', 'texture_normal', 'texture_roughness')),
    sha256 TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    license TEXT,
    source_url TEXT
);

CREATE TABLE IF NOT EXISTS levels (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    elevation REAL NOT NULL DEFAULT 0.0,
    height REAL NOT NULL DEFAULT 2.5,
    floor_slab_thickness REAL NOT NULL DEFAULT 0.2,
    sort_order INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS walls (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    level_id TEXT NOT NULL REFERENCES levels(id) ON DELETE CASCADE,
    is_closed_loop INTEGER NOT NULL DEFAULT 0,
    thickness REAL NOT NULL DEFAULT 0.24,
    start_height REAL NOT NULL DEFAULT 2.5,
    end_height REAL NOT NULL DEFAULT 2.5,
    material_left_json TEXT,
    material_right_json TEXT,
    material_top_json TEXT
);

CREATE TABLE IF NOT EXISTS wall_anchor_points (
    id TEXT PRIMARY KEY,
    wall_id TEXT NOT NULL REFERENCES walls(id) ON DELETE CASCADE,
    point_order INTEGER NOT NULL,
    pos_x REAL NOT NULL,
    pos_y REAL NOT NULL,
    handle_in_x REAL,
    handle_in_y REAL,
    handle_out_x REAL,
    handle_out_y REAL,
    point_type TEXT NOT NULL CHECK(point_type IN ('corner', 'smooth', 'symmetric'))
);

CREATE TABLE IF NOT EXISTS openings (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    wall_id TEXT NOT NULL REFERENCES walls(id) ON DELETE CASCADE,
    opening_type TEXT NOT NULL CHECK(opening_type IN ('door', 'window', 'cutout')),
    catalog_item_id TEXT,
    asset_id TEXT REFERENCES assets(id) ON DELETE SET NULL,
    segment_index INTEGER NOT NULL DEFAULT 0,
    segment_t REAL NOT NULL DEFAULT 0.5,
    elevation REAL NOT NULL DEFAULT 0.0,
    width REAL NOT NULL,
    height REAL NOT NULL,
    depth REAL NOT NULL,
    swing_flip_side INTEGER NOT NULL DEFAULT 0,
    swing_flip_hinge INTEGER NOT NULL DEFAULT 0,
    ha_entity_id TEXT
);

CREATE TABLE IF NOT EXISTS rooms (
    id TEXT PRIMARY KEY,
    level_id TEXT NOT NULL REFERENCES levels(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    boundary_polygon_json TEXT NOT NULL,
    area_sqm REAL NOT NULL,
    volume_cbm REAL NOT NULL,
    floor_material_json TEXT,
    ceiling_material_json TEXT
);

CREATE TABLE IF NOT EXISTS furniture_items (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    level_id TEXT NOT NULL REFERENCES levels(id) ON DELETE CASCADE,
    catalog_item_id TEXT NOT NULL,
    asset_id TEXT REFERENCES assets(id) ON DELETE SET NULL,
    pos_x REAL NOT NULL DEFAULT 0.0,
    pos_y REAL NOT NULL DEFAULT 0.0,
    pos_z REAL NOT NULL DEFAULT 0.0,
    rot_x REAL NOT NULL DEFAULT 0.0,
    rot_y REAL NOT NULL DEFAULT 0.0,
    rot_z REAL NOT NULL DEFAULT 0.0,
    scale_x REAL NOT NULL DEFAULT 1.0,
    scale_y REAL NOT NULL DEFAULT 1.0,
    scale_z REAL NOT NULL DEFAULT 1.0,
    wall_mount_wall_id TEXT REFERENCES walls(id) ON DELETE SET NULL,
    ha_binding_json TEXT
);

CREATE TABLE IF NOT EXISTS stairs (
    id TEXT PRIMARY KEY,
    level_id TEXT NOT NULL REFERENCES levels(id) ON DELETE CASCADE,
    target_level_id TEXT NOT NULL REFERENCES levels(id),
    stair_type TEXT NOT NULL CHECK(stair_type IN ('straight', 'l_quarter', 'u_half', 'spiral')),
    width REAL NOT NULL,
    total_height REAL NOT NULL,
    tread_depth REAL NOT NULL,
    riser_height REAL NOT NULL,
    auto_ceiling_cutout INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS roofs (
    id TEXT PRIMARY KEY,
    base_level_id TEXT NOT NULL REFERENCES levels(id) ON DELETE CASCADE,
    roof_type TEXT NOT NULL CHECK(roof_type IN ('gable', 'hip', 'shed', 'mansard', 'flat')),
    overhang REAL NOT NULL DEFAULT 0.3,
    slope_angle REAL NOT NULL DEFAULT 35.0,
    wall_cut_strategy TEXT NOT NULL DEFAULT 'clip'
);

CREATE TABLE IF NOT EXISTS room_names (
    signature TEXT PRIMARY KEY,
    name TEXT NOT NULL
);
"#;

/// Initializes the Homely 3D SQLite schema on `conn`.
pub fn init_schema(conn: &Connection) -> Result<(), CoreError> {
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(())
}

/// Saves `project` to a SQLite file (using WAL mode and a single transaction).
pub fn save_to_file(project: &Project, path: &Path) -> Result<(), CoreError> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    save_to_conn(project, &mut conn)
}

/// Loads a [`Project`] from a SQLite file.
pub fn load_from_file(path: &Path) -> Result<Project, CoreError> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    load_from_conn(&conn)
}

/// Saves `project` into an open SQLite [`Connection`], replacing previous rows.
pub fn save_to_conn(project: &Project, conn: &mut Connection) -> Result<(), CoreError> {
    init_schema(conn)?;
    let tx = conn.transaction()?;

    tx.execute_batch(
        "DELETE FROM room_names;
         DELETE FROM furniture_items;
         DELETE FROM openings;
         DELETE FROM wall_anchor_points;
         DELETE FROM walls;
         DELETE FROM levels;
         DELETE FROM assets;
         DELETE FROM project_metadata;",
    )?;

    tx.execute(
        "INSERT INTO project_metadata (id, name, version, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            project.id,
            project.name,
            project.version,
            project.created_at,
            project.updated_at
        ],
    )?;

    for asset in &project.assets {
        tx.execute(
            "INSERT INTO assets (id, kind, sha256, relative_path, license, source_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                asset.id,
                asset.kind.as_str(),
                asset.sha256,
                asset.relative_path,
                asset.license,
                asset.source_url
            ],
        )?;
    }

    for level in &project.levels {
        tx.execute(
            "INSERT INTO levels (id, name, elevation, height, floor_slab_thickness, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                level.id,
                level.name,
                level.elevation,
                level.height,
                level.floor_slab_thickness,
                level.sort_order
            ],
        )?;
    }

    for wall in &project.walls {
        tx.execute(
            "INSERT INTO walls (id, name, level_id, is_closed_loop, thickness, start_height, end_height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                wall.id,
                wall.name,
                wall.level_id,
                i32::from(wall.is_closed_loop),
                wall.thickness,
                wall.start_height,
                wall.end_height
            ],
        )?;
        for (order, anchor) in wall.anchors.iter().enumerate() {
            tx.execute(
                "INSERT INTO wall_anchor_points
                 (id, wall_id, point_order, pos_x, pos_y, handle_in_x, handle_in_y, handle_out_x, handle_out_y, point_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    anchor.id,
                    wall.id,
                    order as i64,
                    anchor.pos_x,
                    anchor.pos_y,
                    anchor.handle_in_x,
                    anchor.handle_in_y,
                    anchor.handle_out_x,
                    anchor.handle_out_y,
                    anchor.point_type.as_str()
                ],
            )?;
        }
    }

    for opening in &project.openings {
        tx.execute(
            "INSERT INTO openings
             (id, name, wall_id, opening_type, catalog_item_id, asset_id, segment_index, segment_t, elevation, width, height, depth, swing_flip_side, swing_flip_hinge, ha_entity_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                opening.id,
                opening.name,
                opening.wall_id,
                opening.opening_type.as_str(),
                opening.catalog_item_id,
                opening.asset_id,
                opening.segment_index as i64,
                opening.segment_t,
                opening.elevation,
                opening.width,
                opening.height,
                opening.depth,
                i32::from(opening.swing_flip_side),
                i32::from(opening.swing_flip_hinge),
                opening.ha_entity_id
            ],
        )?;
    }

    for item in &project.furniture {
        tx.execute(
            "INSERT INTO furniture_items
             (id, name, level_id, catalog_item_id, asset_id, pos_x, pos_y, pos_z, rot_x, rot_y, rot_z, scale_x, scale_y, scale_z, wall_mount_wall_id, ha_binding_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                item.id,
                item.name,
                item.level_id,
                item.catalog_item_id,
                item.asset_id,
                item.transform.pos_x,
                item.transform.pos_y,
                item.transform.pos_z,
                item.transform.rot_x,
                item.transform.rot_y,
                item.transform.rot_z,
                item.transform.scale_x,
                item.transform.scale_y,
                item.transform.scale_z,
                item.wall_mount_wall_id,
                item.ha_binding_json
            ],
        )?;
    }

    for (sig, name) in &project.room_names {
        tx.execute(
            "INSERT INTO room_names (signature, name) VALUES (?1, ?2)",
            params![sig, name],
        )?;
    }

    tx.commit()?;
    Ok(())
}

/// Loads a [`Project`] from an open SQLite [`Connection`].
pub fn load_from_conn(conn: &Connection) -> Result<Project, CoreError> {
    let mut project = conn.query_row(
        "SELECT id, name, version, created_at, updated_at FROM project_metadata LIMIT 1",
        [],
        |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                version: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                ..Default::default()
            })
        },
    )?;

    let mut stmt = conn.prepare(
        "SELECT id, kind, sha256, relative_path, license, source_url FROM assets",
    )?;
    for row in stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })? {
        let (id, kind_str, sha256, relative_path, license, source_url) = row?;
        project.assets.push(Asset {
            id,
            kind: AssetKind::parse(&kind_str)?,
            sha256,
            relative_path,
            license,
            source_url,
        });
    }

    let mut stmt = conn.prepare(
        "SELECT id, name, elevation, height, floor_slab_thickness, sort_order
         FROM levels ORDER BY sort_order ASC",
    )?;
    for row in stmt.query_map([], |row| {
        Ok(Level {
            id: row.get(0)?,
            name: row.get(1)?,
            elevation: row.get(2)?,
            height: row.get(3)?,
            floor_slab_thickness: row.get(4)?,
            sort_order: row.get(5)?,
        })
    })? {
        project.levels.push(row?);
    }

    let mut wall_stmt = conn.prepare(
        "SELECT id, name, level_id, is_closed_loop, thickness, start_height, end_height FROM walls",
    )?;
    let mut anchor_stmt = conn.prepare(
        "SELECT id, pos_x, pos_y, handle_in_x, handle_in_y, handle_out_x, handle_out_y, point_type
         FROM wall_anchor_points WHERE wall_id = ?1 ORDER BY point_order ASC",
    )?;

    for wall_row in wall_stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i32>(3)? != 0,
            row.get::<_, f64>(4)?,
            row.get::<_, f64>(5)?,
            row.get::<_, f64>(6)?,
        ))
    })? {
        let (id, name, level_id, is_closed_loop, thickness, start_height, end_height) = wall_row?;
        let mut anchors = Vec::new();
        for a_row in anchor_stmt.query_map([&id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, String>(7)?,
            ))
        })? {
            let (aid, pos_x, pos_y, hi_x, hi_y, ho_x, ho_y, pt_str) = a_row?;
            anchors.push(WallAnchorPoint {
                id: aid,
                pos_x,
                pos_y,
                handle_in_x: hi_x,
                handle_in_y: hi_y,
                handle_out_x: ho_x,
                handle_out_y: ho_y,
                point_type: PointType::parse(&pt_str)?,
            });
        }
        project.walls.push(Wall {
            id,
            name,
            level_id,
            is_closed_loop,
            thickness,
            start_height,
            end_height,
            anchors,
            side_a_finish: crate::SurfaceFinish::default_wall(),
            side_b_finish: crate::SurfaceFinish::default_wall(),
        });
    }

    let mut stmt = conn.prepare(
        "SELECT id, name, wall_id, opening_type, catalog_item_id, asset_id, segment_index, segment_t, elevation, width, height, depth, swing_flip_side, swing_flip_hinge, ha_entity_id
         FROM openings",
    )?;
    for row in stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)? as usize,
            row.get::<_, f64>(7)?,
            row.get::<_, f64>(8)?,
            row.get::<_, f64>(9)?,
            row.get::<_, f64>(10)?,
            row.get::<_, f64>(11)?,
            row.get::<_, i32>(12)? != 0,
            row.get::<_, i32>(13)? != 0,
            row.get::<_, Option<String>>(14)?,
        ))
    })? {
        let (
            id,
            name,
            wall_id,
            type_str,
            catalog_item_id,
            asset_id,
            segment_index,
            segment_t,
            elevation,
            width,
            height,
            depth,
            swing_flip_side,
            swing_flip_hinge,
            ha,
        ) = row?;
        project.openings.push(Opening {
            id,
            name,
            wall_id,
            opening_type: OpeningType::parse(&type_str)?,
            catalog_item_id,
            asset_id,
            segment_index,
            segment_t,
            elevation,
            width,
            height,
            depth,
            swing_flip_side,
            swing_flip_hinge,
            ha_entity_id: ha,
        });
    }

    let mut stmt = conn.prepare(
        "SELECT id, name, level_id, catalog_item_id, asset_id, pos_x, pos_y, pos_z, rot_x, rot_y, rot_z, scale_x, scale_y, scale_z, wall_mount_wall_id, ha_binding_json
         FROM furniture_items",
    )?;
    for row in stmt.query_map([], |row| {
        Ok(FurnitureItem {
            id: row.get(0)?,
            name: row.get(1)?,
            level_id: row.get(2)?,
            catalog_item_id: row.get(3)?,
            asset_id: row.get(4)?,
            transform: Transform3D {
                pos_x: row.get(5)?,
                pos_y: row.get(6)?,
                pos_z: row.get(7)?,
                rot_x: row.get(8)?,
                rot_y: row.get(9)?,
                rot_z: row.get(10)?,
                scale_x: row.get(11)?,
                scale_y: row.get(12)?,
                scale_z: row.get(13)?,
            },
            wall_mount_wall_id: row.get(14)?,
            ha_binding_json: row.get(15)?,
        })
    })? {
        project.furniture.push(row?);
    }

    let mut stmt = conn.prepare("SELECT signature, name FROM room_names")?;
    for row in stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
        let (sig, name) = row?;
        project.room_names.insert(sig, name);
    }

    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_save_and_load_roundtrip() {
        let project = Project {
            id: "proj-1".into(),
            name: "Traumhaus".into(),
            version: "2.0.0".into(),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_100,
            assets: vec![Asset {
                id: "asset-sofa".into(),
                kind: AssetKind::Model,
                sha256: "abc123".into(),
                relative_path: "assets/models/sofa.glb".into(),
                license: Some("CC0".into()),
                source_url: None,
            }],
            levels: vec![Level {
                id: "lvl-0".into(),
                name: "Erdgeschoss".into(),
                elevation: 0.0,
                height: 2.6,
                floor_slab_thickness: 0.2,
                sort_order: 0,
            }],
            walls: vec![Wall {
                id: "wall-1".into(),
                name: "Nordwand".into(),
                level_id: "lvl-0".into(),
                is_closed_loop: false,
                thickness: 0.24,
                start_height: 2.6,
                end_height: 2.6,
                side_a_finish: crate::SurfaceFinish::default_wall(),
                side_b_finish: crate::SurfaceFinish::default_wall(),
                anchors: vec![
                    WallAnchorPoint {
                        id: "a1".into(),
                        pos_x: 0.0,
                        pos_y: 0.0,
                        handle_in_x: None,
                        handle_in_y: None,
                        handle_out_x: Some(1.0),
                        handle_out_y: Some(0.5),
                        point_type: PointType::Smooth,
                    },
                    WallAnchorPoint {
                        id: "a2".into(),
                        pos_x: 5.0,
                        pos_y: 0.0,
                        handle_in_x: None,
                        handle_in_y: None,
                        handle_out_x: None,
                        handle_out_y: None,
                        point_type: PointType::Corner,
                    },
                ],
            }],
            openings: vec![Opening {
                id: "op-1".into(),
                name: "Wohnzimmerfenster".into(),
                wall_id: "wall-1".into(),
                opening_type: OpeningType::Window,
                catalog_item_id: Some("cat-win-standard".into()),
                asset_id: None,
                segment_index: 0,
                segment_t: 0.5,
                elevation: 0.9,
                width: 1.2,
                height: 1.4,
                depth: 0.24,
                swing_flip_side: false,
                swing_flip_hinge: false,
                ha_entity_id: Some("binary_sensor.living_window".into()),
            }],
            furniture: vec![FurnitureItem {
                id: "furn-1".into(),
                name: "Sofa".into(),
                level_id: "lvl-0".into(),
                catalog_item_id: "modern_sofa".into(),
                asset_id: Some("asset-sofa".into()),
                transform: Transform3D {
                    pos_x: 2.5,
                    pos_y: 0.0,
                    pos_z: 1.8,
                    ..Default::default()
                },
                wall_mount_wall_id: None,
                ha_binding_json: None,
            }],
            room_names: Default::default(),
            room_finishes: Default::default(),
        };

        let mut conn = Connection::open_in_memory().unwrap();
        save_to_conn(&project, &mut conn).unwrap();
        let loaded = load_from_conn(&conn).unwrap();
        assert_eq!(project, loaded);
    }
}
