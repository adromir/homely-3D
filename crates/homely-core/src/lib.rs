//! Core data model, command history, and SQLite persistence for Homely 3D.
//!
//! Coordinate conventions:
//! - `1.0` unit = `1.0` meter
//! - 2D floorplan coordinates: `(X, Y)`
//! - 3D coordinates (Bevy / glTF Y-up): `X` width, `Y` height above level floor, `Z` depth
//! - Angles: radians

pub mod command;
pub mod db;
pub mod geometry;
pub mod rooms;
pub mod sh3x;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Errors returned by `homely-core` operations.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("entity not found: {kind} '{id}'")]
    NotFound { kind: &'static str, id: String },

    #[error("duplicate entity: {kind} '{id}'")]
    DuplicateId { kind: &'static str, id: String },

    #[error("invalid index {index} for {kind} '{id}'")]
    InvalidIndex {
        kind: &'static str,
        id: String,
        index: usize,
    },

    #[error("invalid value for {field}: {message}")]
    Validation {
        field: &'static str,
        message: String,
    },

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("archive error: {0}")]
    Archive(#[from] zip::result::ZipError),
}

/// Kind of binary asset stored in a `.sh3x` container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Model,
    TextureAlbedo,
    TextureNormal,
    TextureRoughness,
}

impl AssetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::TextureAlbedo => "texture_albedo",
            Self::TextureNormal => "texture_normal",
            Self::TextureRoughness => "texture_roughness",
        }
    }

    pub fn parse(value: &str) -> Result<Self, CoreError> {
        match value {
            "model" => Ok(Self::Model),
            "texture_albedo" => Ok(Self::TextureAlbedo),
            "texture_normal" => Ok(Self::TextureNormal),
            "texture_roughness" => Ok(Self::TextureRoughness),
            _ => Err(CoreError::Validation {
                field: "assets.kind",
                message: format!("unknown asset kind '{value}'"),
            }),
        }
    }
}

/// Referenced 3D model or PBR texture asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub kind: AssetKind,
    pub sha256: String,
    pub relative_path: String,
    pub license: Option<String>,
    pub source_url: Option<String>,
}

/// Anchor point type for poly-Bezier wall paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointType {
    Corner,
    Smooth,
    Symmetric,
}

impl PointType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Corner => "corner",
            Self::Smooth => "smooth",
            Self::Symmetric => "symmetric",
        }
    }

    pub fn parse(value: &str) -> Result<Self, CoreError> {
        match value {
            "corner" => Ok(Self::Corner),
            "smooth" => Ok(Self::Smooth),
            "symmetric" => Ok(Self::Symmetric),
            _ => Err(CoreError::Validation {
                field: "wall_anchor_points.point_type",
                message: format!("unknown point type '{value}'"),
            }),
        }
    }
}

/// A 2D anchor point on a wall path with optional cubic Bezier tangent handles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WallAnchorPoint {
    pub id: String,
    pub pos_x: f64,
    pub pos_y: f64,
    pub handle_in_x: Option<f64>,
    pub handle_in_y: Option<f64>,
    pub handle_out_x: Option<f64>,
    pub handle_out_y: Option<f64>,
    pub point_type: PointType,
}

/// Type of architectural opening inside a wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpeningType {
    Door,
    Window,
    Cutout,
}

impl OpeningType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Door => "door",
            Self::Window => "window",
            Self::Cutout => "cutout",
        }
    }

    pub fn parse(value: &str) -> Result<Self, CoreError> {
        match value {
            "door" => Ok(Self::Door),
            "window" => Ok(Self::Window),
            "cutout" => Ok(Self::Cutout),
            _ => Err(CoreError::Validation {
                field: "openings.opening_type",
                message: format!("unknown opening type '{value}'"),
            }),
        }
    }
}

/// A parametric opening positioned on a specific wall segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opening {
    pub id: String,
    pub name: String,
    pub wall_id: String,
    pub opening_type: OpeningType,
    pub segment_index: usize,
    pub segment_t: f64,
    pub elevation: f64,
    pub width: f64,
    pub height: f64,
    pub depth: f64,
    pub swing_flip_side: bool,
    pub swing_flip_hinge: bool,
    pub ha_entity_id: Option<String>,
}

/// A poly-Bezier wall belonging to a building level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wall {
    pub id: String,
    pub name: String,
    pub level_id: String,
    pub is_closed_loop: bool,
    pub thickness: f64,
    pub start_height: f64,
    pub end_height: f64,
    pub anchors: Vec<WallAnchorPoint>,
}

/// A building story / level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub id: String,
    pub name: String,
    pub elevation: f64,
    pub height: f64,
    pub floor_slab_thickness: f64,
    pub sort_order: i32,
}

/// Explicit 3D transform (Y-up, meters, radians).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform3D {
    pub pos_x: f64,
    pub pos_y: f64,
    pub pos_z: f64,
    pub rot_x: f64,
    pub rot_y: f64,
    pub rot_z: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub scale_z: f64,
}

impl Default for Transform3D {
    fn default() -> Self {
        Self {
            pos_x: 0.0,
            pos_y: 0.0,
            pos_z: 0.0,
            rot_x: 0.0,
            rot_y: 0.0,
            rot_z: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            scale_z: 1.0,
        }
    }
}

/// A placed furniture or fixture item in a level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FurnitureItem {
    pub id: String,
    pub name: String,
    pub level_id: String,
    pub catalog_item_id: String,
    pub asset_id: Option<String>,
    pub transform: Transform3D,
    pub wall_mount_wall_id: Option<String>,
    pub ha_binding_json: Option<String>,
}

/// Root in-memory project container (runtime Single Source of Truth).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub version: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub assets: Vec<Asset>,
    pub levels: Vec<Level>,
    pub walls: Vec<Wall>,
    pub openings: Vec<Opening>,
    pub furniture: Vec<FurnitureItem>,
    pub room_names: BTreeMap<String, String>,
}
