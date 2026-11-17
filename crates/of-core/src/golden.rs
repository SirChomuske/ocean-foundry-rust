//! Golden comparisons for tests: a result is compared with a stored, reviewed
//! copy, so an unintended change in output is caught.
//!
//! Set the environment variable `OF_UPDATE_GOLDEN=1` to rewrite the stored
//! copies after an intended change, then review the difference before keeping it.

use std::path::{Path, PathBuf};

/// Folder holding golden files: `tests/golden` at the workspace root.
pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

fn updating() -> bool {
    std::env::var("OF_UPDATE_GOLDEN").map(|v| v == "1").unwrap_or(false)
}

/// Compare text with a golden file. Line endings are ignored.
pub fn check_text(name: &str, actual: &str) -> Result<(), String> {
    let path = golden_dir().join(name);
    let actual = actual.replace("\r\n", "\n");
    if updating() {
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        return std::fs::write(&path, &actual).map_err(|e| e.to_string());
    }
    let expected = std::fs::read_to_string(&path)
        .map_err(|e| format!("golden file {} is missing ({e}); run with OF_UPDATE_GOLDEN=1 to create it", path.display()))?
        .replace("\r\n", "\n");
    if expected == actual {
        return Ok(());
    }
    let line = expected.lines().zip(actual.lines()).position(|(a, b)| a != b).unwrap_or(expected.lines().count().min(actual.lines().count()));
    Err(format!(
        "{name} differs from the golden copy at line {}:\n  expected: {}\n  actual:   {}",
        line + 1,
        expected.lines().nth(line).unwrap_or("<end of file>"),
        actual.lines().nth(line).unwrap_or("<end of file>")
    ))
}

/// How two images of the same size differ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageDiff {
    /// Largest difference in any channel, 0..=255.
    pub max_channel: u8,
    /// Fraction of pixels whose difference exceeds the tolerance.
    pub differing_fraction: f64,
}

/// Compare two RGBA8 images. `tolerance` is the per-channel difference that
/// still counts as equal, to allow for GPU rounding.
pub fn diff_rgba(a: &[u8], b: &[u8], tolerance: u8) -> Result<ImageDiff, String> {
    if a.len() != b.len() || a.len() % 4 != 0 {
        return Err(format!("image sizes differ ({} and {} bytes)", a.len(), b.len()));
    }
    let mut max_channel = 0u8;
    let mut differing = 0usize;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let d = pa.iter().zip(pb).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        max_channel = max_channel.max(d);
        if d > tolerance {
            differing += 1;
        }
    }
    Ok(ImageDiff { max_channel, differing_fraction: differing as f64 / (a.len() / 4).max(1) as f64 })
}

/// A stable text listing of the parameter registry. Guarded by a golden file
/// so that a saved-settings id or a stage assignment is never changed by accident.
pub fn registry_listing() -> String {
    use crate::params::{Kind, REGISTRY};
    let mut out = String::from("# id | stage | advanced | kind\n");
    for p in REGISTRY {
        let kind = match p.kind {
            Kind::Float { min, max, default, decimals } => format!("float {min}..{max} default {default} decimals {decimals}"),
            Kind::Int { min, max, default } => format!("int {min}..{max} default {default}"),
            Kind::Seed { default } => format!("seed default {default}"),
            Kind::Choice { options, default, .. } => format!("choice [{}] default {default}", options.join(",")),
            Kind::Bool { default } => format!("bool default {default}"),
            Kind::Colour { default } => format!("colour default {default}"),
        };
        out.push_str(&format!("{} | {} | {} | {}\n", p.id, p.stage.key(), p.advanced, kind));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "dev")]
    #[test]
    fn registry_matches_the_reviewed_listing() {
        if let Err(e) = check_text("registry.txt", &registry_listing()) {
            panic!("{e}");
        }
    }

    /// The package has the reviewed listing without lava's settings.
    #[cfg(not(feature = "dev"))]
    #[test]
    fn registry_matches_the_reviewed_listing() {
        let path = golden_dir().join("registry.txt");
        let reviewed = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        let expected: String = reviewed.lines().filter(|l| !l.starts_with("lava.")).map(|l| format!("{l}\n")).collect();
        assert!(expected.len() < reviewed.len(), "the reviewed listing has lava's settings");
        assert_eq!(registry_listing(), expected);
    }

    #[test]
    fn image_diff_respects_tolerance() {
        let a = [10u8, 10, 10, 255, 200, 200, 200, 255];
        let b = [11u8, 10, 10, 255, 100, 200, 200, 255];
        let d = diff_rgba(&a, &b, 2).unwrap();
        assert_eq!(d.max_channel, 100);
        assert_eq!(d.differing_fraction, 0.5);
        assert!(diff_rgba(&a, &b[..4], 2).is_err());
    }
}
