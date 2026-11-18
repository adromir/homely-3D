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

pub const METERS_PER_FOOT: f64 = 0.3048;
pub const METERS_PER_INCH: f64 = 0.0254;
pub const SQFT_PER_SQM: f64 = 10.763_910_416_7;

/// Display and input measurement system (internal storage is always meters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitSystem {
    #[default]
    Metric,
    Imperial,
}

impl UnitSystem {
    pub fn meters_to_display_len(self, m: f64) -> f64 {
        match self {
            Self::Metric => m,
            Self::Imperial => m / METERS_PER_FOOT,
        }
    }

    pub fn display_len_to_meters(self, val: f64) -> f64 {
        match self {
            Self::Metric => val,
            Self::Imperial => val * METERS_PER_FOOT,
        }
    }

    pub fn meters_to_inches(m: f64) -> f64 {
        m / METERS_PER_INCH
    }

    pub fn inches_to_meters(inches: f64) -> f64 {
        inches * METERS_PER_INCH
    }

    pub fn sqm_to_display_area(self, sqm: f64) -> f64 {
        match self {
            Self::Metric => sqm,
            Self::Imperial => sqm * SQFT_PER_SQM,
        }
    }

    pub fn area_suffix(self) -> &'static str {
        match self {
            Self::Metric => "m²",
            Self::Imperial => "sq ft",
        }
    }

    pub fn len_suffix(self) -> &'static str {
        match self {
            Self::Metric => "m",
            Self::Imperial => "ft",
        }
    }
}

/// Regional building standard presets providing typical wall thicknesses and ceiling heights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildingPreset {
    #[default]
    GermanyDin,
    UsaIrc,
    UkStandard,
    Custom,
}

impl BuildingPreset {
    /// Returns `(unit_system, default_wall_thickness_m, default_wall_height_m)`.
    pub fn defaults(self) -> (UnitSystem, f32, f32) {
        match self {
            // DIN: 24 cm exterior/masonry wall, 2.50 m ceiling height
            Self::GermanyDin => (UnitSystem::Metric, 0.24, 2.50),
            // US IRC: 4.5 in (2x4 + drywall) interior/standard wall, 8 ft (2.4384 m) ceiling
            Self::UsaIrc => (UnitSystem::Imperial, 0.1143, 2.4384),
            // UK: 30 cm cavity wall, 2.40 m ceiling height
            Self::UkStandard => (UnitSystem::Metric, 0.30, 2.40),
            Self::Custom => (UnitSystem::Metric, 0.24, 2.60),
        }
    }

    /// Common wall thicknesses `(label, meters)` for quick selection.
    pub fn wall_thickness_options(self) -> &'static [(&'static str, f32)] {
        match self {
            Self::GermanyDin => &[
                ("11.5 cm (Innen)", 0.115),
                ("17.5 cm (Tragend)", 0.175),
                ("24.0 cm (Außen)", 0.24),
                ("36.5 cm (Dämmziegel)", 0.365),
            ],
            Self::UsaIrc => &[
                ("4.5 in (2x4)", 0.1143),
                ("6.5 in (2x6)", 0.1651),
                ("8.0 in (CMU)", 0.2032),
            ],
            Self::UkStandard => &[
                ("10.0 cm (Partition)", 0.10),
                ("21.5 cm (Solid)", 0.215),
                ("30.0 cm (Cavity)", 0.30),
            ],
            Self::Custom => &[],
        }
    }
}

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

/// Category of a placeable catalog object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogCategory {
    Door,
    Window,
    Furniture,
}

/// Entry in the object catalog (doors, windows, furniture) with optional 3D asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogItem {
    pub id: String,
    pub name_de: String,
    pub name_en: String,
    pub category: CatalogCategory,
    pub default_width: f64,
    pub default_height: f64,
    pub default_depth: f64,
    pub default_elevation: f64,
    pub asset_id: Option<String>,
}

/// Built-in starter catalog for doors and windows.
pub fn builtin_catalog() -> Vec<CatalogItem> {
    vec![
        CatalogItem {
            id: "cat-door-single".into(),
            name_de: "Innentür (0.90m)".into(),
            name_en: "Interior Door (0.90m)".into(),
            category: CatalogCategory::Door,
            default_width: 0.90,
            default_height: 2.10,
            default_depth: 0.24,
            default_elevation: 0.0,
            asset_id: None,
        },
        CatalogItem {
            id: "cat-door-double".into(),
            name_de: "Doppeltür (1.60m)".into(),
            name_en: "Double Door (1.60m)".into(),
            category: CatalogCategory::Door,
            default_width: 1.60,
            default_height: 2.10,
            default_depth: 0.24,
            default_elevation: 0.0,
            asset_id: None,
        },
        CatalogItem {
            id: "cat-win-standard".into(),
            name_de: "Fenster (1.20x1.30m)".into(),
            name_en: "Window (1.20x1.30m)".into(),
            category: CatalogCategory::Window,
            default_width: 1.20,
            default_height: 1.30,
            default_depth: 0.24,
            default_elevation: 0.90,
            asset_id: None,
        },
        CatalogItem {
            id: "cat-win-tall".into(),
            name_de: "Bodentiefes Fenster (1.40x2.10m)".into(),
            name_en: "Tall Window (1.40x2.10m)".into(),
            category: CatalogCategory::Window,
            default_width: 1.40,
            default_height: 2.10,
            default_depth: 0.24,
            default_elevation: 0.0,
            asset_id: None,
        },
    ]
}

/// A parametric opening positioned on a specific wall segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opening {
    pub id: String,
    pub name: String,
    pub wall_id: String,
    pub opening_type: OpeningType,
    pub catalog_item_id: Option<String>,
    pub asset_id: Option<String>,
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

/// Procedural surface texture pattern for walls and floors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceTexture {
    #[default]
    Smooth,
    Plaster,
    Brick,
    WoodPlanks,
    Tile,
    Concrete,
}

/// Painter / trim style applied to a wall or floor surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaintStyle {
    #[default]
    Solid,
    BaseboardTrim,
    CrownTrim,
    OpeningTrim,
    TwoTone,
    Gradient,
}

/// Tintable surface texture and paint finish for one wall face or room floor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceFinish {
    pub texture: SurfaceTexture,
    #[serde(default)]
    pub secondary_texture: Option<SurfaceTexture>,
    pub paint_style: PaintStyle,
    pub primary_color: [u8; 3],
    pub secondary_color: [u8; 3],
    pub trim_width_m: f32,
}

impl SurfaceFinish {
    pub fn default_wall() -> Self {
        Self {
            texture: SurfaceTexture::Plaster,
            secondary_texture: None,
            paint_style: PaintStyle::Solid,
            primary_color: [222, 219, 212],
            secondary_color: [248, 248, 246],
            trim_width_m: 0.12,
        }
    }

    pub fn default_floor() -> Self {
        Self {
            texture: SurfaceTexture::WoodPlanks,
            secondary_texture: None,
            paint_style: PaintStyle::Solid,
            primary_color: [162, 128, 92],
            secondary_color: [120, 90, 60],
            trim_width_m: 0.12,
        }
    }
}

/// Procedural surface texture pattern for 3D terrain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerrainTexture {
    #[default]
    Grass,
    Meadow,
    SoilEarth,
    Sand,
    Gravel,
    StoneRock,
}

/// Configurable terrain generator and automatic foundation settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainSettings {
    pub enabled: bool,
    pub seed: u32,
    pub amplitude_m: f32,
    pub frequency: f32,
    pub auto_foundation: bool,
    pub foundation_depth_m: f32,
    pub texture: TerrainTexture,
    pub tint_color: [u8; 3],
}

impl Default for TerrainSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            seed: 42,
            amplitude_m: 0.45,
            frequency: 0.14,
            auto_foundation: true,
            foundation_depth_m: 0.60,
            texture: TerrainTexture::Grass,
            tint_color: [98, 145, 78],
        }
    }
}

/// Deterministic multi-octave terrain height in meters at world `(x, z)`.
pub fn sample_terrain_height(settings: &TerrainSettings, x: f64, z: f64) -> f64 {
    if !settings.enabled || settings.amplitude_m <= 1e-4 {
        return 0.0;
    }
    let f = f64::from(settings.frequency.max(0.01));
    let s = f64::from(settings.seed % 10_000) * 1.37;
    let n1 = ((x * f + s).sin() * (z * f * 0.9 - s * 0.7).cos())
        + 0.5 * ((x * f * 2.1 - s * 0.4).cos() + (z * f * 1.9 + s * 0.8).sin());
    let n2 = 0.25 * ((x * f * 4.3 + z * f * 3.7 + s).sin());
    (n1 * 0.45 + n2) * f64::from(settings.amplitude_m)
}

/// A poly-Bezier polygonal custom floor finish zone (supports straight and curved edges).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorZone {
    pub id: String,
    pub name: String,
    pub level_id: String,
    pub anchors: Vec<WallAnchorPoint>,
    pub texture: SurfaceTexture,
    pub color: [u8; 3],
}

impl Default for SurfaceFinish {
    fn default() -> Self {
        Self::default_wall()
    }
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
    #[serde(default = "SurfaceFinish::default_wall")]
    pub side_a_finish: SurfaceFinish,
    #[serde(default = "SurfaceFinish::default_wall")]
    pub side_b_finish: SurfaceFinish,
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
    #[serde(default)]
    pub room_finishes: BTreeMap<String, SurfaceFinish>,
    #[serde(default)]
    pub floor_zones: Vec<FloorZone>,
    #[serde(default)]
    pub terrain: TerrainSettings,
}
