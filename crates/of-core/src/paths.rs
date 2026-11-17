//! Where the app keeps its own files.
//!
//! Portable first: a data folder beside the executable, so the extracted ZIP
//! is self-contained and can be moved or deleted as one unit. If that folder
//! cannot be written (for example the ZIP was extracted somewhere read-only),
//! the per-user application data folder is used instead.

use std::path::{Path, PathBuf};

pub const DATA_DIR_NAME: &str = "OceanFoundry-data";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    pub path: PathBuf,
    /// True when the folder sits beside the executable.
    pub portable: bool,
}

fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Resolve the data folder, creating it if needed. `override_dir` lets tests
/// and the command line point somewhere else.
pub fn data_dir(override_dir: Option<&Path>) -> std::io::Result<DataDir> {
    if let Some(dir) = override_dir {
        return if writable(dir) {
            Ok(DataDir { path: dir.to_path_buf(), portable: false })
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("cannot write to {}", dir.display())))
        };
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let dir = exe_dir.join(DATA_DIR_NAME);
        if writable(&dir) {
            return Ok(DataDir { path: dir, portable: true });
        }
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("OceanFoundry");
    if writable(&dir) {
        Ok(DataDir { path: dir, portable: false })
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("cannot write to {}", dir.display())))
    }
}

/// Write a file so that a crash or power loss mid-write can never leave a
/// half-written file in place of a good one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(match path.extension() {
        Some(e) => format!("{}.tmp", e.to_string_lossy()),
        None => "tmp".to_string(),
    });
    std::fs::write(&tmp, bytes)?;
    // On Windows, rename replaces an existing destination file.
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
