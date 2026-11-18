//! `.sh3x` ZIP container persistence (`manifest.json` + `project.db`).

use crate::{db, CoreError, Project};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Metadata stored in `manifest.json` inside a `.sh3x` archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sh3xManifest {
    pub schema_version: u32,
    pub project_id: String,
    pub name: String,
    pub version: String,
    pub level_count: usize,
    pub wall_count: usize,
}

impl Sh3xManifest {
    pub fn from_project(project: &Project) -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            project_id: project.id.clone(),
            name: project.name.clone(),
            version: project.version.clone(),
            level_count: project.levels.len(),
            wall_count: project.walls.len(),
        }
    }
}

/// Saves `project` into a `.sh3x` ZIP archive at `path`.
pub fn save_sh3x(project: &Project, path: &Path) -> Result<(), CoreError> {
    let temp_db = temp_db_path("save");
    let db_result = (|| -> Result<Vec<u8>, CoreError> {
        db::save_to_file(project, &temp_db)?;
        Ok(std::fs::read(&temp_db)?)
    })();
    let _ = std::fs::remove_file(&temp_db);
    let _ = std::fs::remove_file(wal_sidecar(&temp_db, "-wal"));
    let _ = std::fs::remove_file(wal_sidecar(&temp_db, "-shm"));
    let db_bytes = db_result?;

    let file = File::create(path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let manifest = Sh3xManifest::from_project(project);
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    zip.start_file("manifest.json", options)?;
    zip.write_all(&manifest_bytes)?;

    zip.start_file("project.db", options)?;
    zip.write_all(&db_bytes)?;

    zip.finish()?;
    Ok(())
}

/// Loads a [`Project`] from a `.sh3x` ZIP archive at `path`.
pub fn load_sh3x(path: &Path) -> Result<Project, CoreError> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;

    let mut db_bytes = Vec::new();
    {
        let mut entry = archive.by_name("project.db")?;
        entry.read_to_end(&mut db_bytes)?;
    }

    let temp_db = temp_db_path("load");
    let result = (|| -> Result<Project, CoreError> {
        std::fs::write(&temp_db, &db_bytes)?;
        db::load_from_file(&temp_db)
    })();
    let _ = std::fs::remove_file(&temp_db);
    result
}

fn temp_db_path(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "homely3d_{tag}_{}_{nanos}.db",
        std::process::id()
    ))
}

fn wal_sidecar(db_path: &Path, suffix: &str) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Level;

    #[test]
    fn sh3x_save_and_load_roundtrip() {
        let project = Project {
            id: "p-sh3x".into(),
            name: "Archive House".into(),
            version: "0.1.0".into(),
            levels: vec![Level {
                id: "l-1".into(),
                name: "EG".into(),
                elevation: 0.0,
                height: 2.5,
                floor_slab_thickness: 0.2,
                sort_order: 0,
            }],
            ..Default::default()
        };

        let archive_path = std::env::temp_dir().join(format!(
            "homely3d_test_{}.sh3x",
            std::process::id()
        ));
        save_sh3x(&project, &archive_path).unwrap();
        let loaded = load_sh3x(&archive_path).unwrap();
        let _ = std::fs::remove_file(&archive_path);

        assert_eq!(project, loaded);
    }
}
