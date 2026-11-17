//! Project files and profiles.
//!
//! A project file holds everything a bake is made from: the mode, every
//! setting, the blocks left out, and the land the sea is baked around. A
//! profile is the same thing kept in the data folder under a name, so a
//! look can be called back with one click.

use crate::params::{Mode, ParamValues};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The ending of a project file.
pub const EXTENSION: &str = "ofproject";
/// The folder inside the data folder that profiles are kept in.
pub const PROFILES: &str = "profiles";
/// The folder inside the data folder that files carried in projects are
/// unpacked into.
pub const MAPS: &str = "maps";
/// Files larger than this are not carried inside a project.
const CARRY_LIMIT: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// The program that wrote the file, and its version.
    pub app: String,
    pub version: String,
    pub mode: Mode,
    pub values: ParamValues,
    /// Blocks the coloured surface may not use.
    #[serde(default)]
    pub excluded: Vec<String>,
    /// The land of the sea, as the text of its scene file.
    #[serde(default)]
    pub scene: Option<String>,
    /// The ripples' image, carried inside the project so it still opens
    /// on another computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern_image: Option<Carried>,
}

/// A file carried inside a project: its name and its bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Carried {
    pub name: String,
    /// The bytes, in base 64.
    pub data: String,
}

impl Carried {
    /// Take a copy of a file to carry, unless it is missing or too large.
    pub fn read(path: &Path) -> Option<Self> {
        if std::fs::metadata(path).ok()?.len() > CARRY_LIMIT {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        Some(Self { name, data: encode(&bytes) })
    }

    /// Put the file in the `maps` folder of `data` and return where it is.
    /// A different file already there under the same name is kept, and
    /// this one gets a numbered name beside it.
    pub fn place(&self, data: &Path) -> Result<PathBuf, String> {
        let bytes = decode(&self.data).ok_or("the image carried in the project is damaged")?;
        let folder = data.join(MAPS);
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let plain: String = self.name.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
        let plain = if plain.trim().is_empty() { "pattern.png".to_string() } else { plain };
        let (stem, ending) = match plain.rsplit_once('.') {
            Some((s, e)) => (s.to_string(), format!(".{e}")),
            None => (plain.clone(), String::new()),
        };
        for n in 0..1000 {
            let path = folder.join(if n == 0 { plain.clone() } else { format!("{stem} ({n}){ending}") });
            match std::fs::read(&path) {
                Ok(there) if there == bytes => return Ok(path),
                Ok(_) => continue,
                Err(_) => {
                    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
                    return Ok(path);
                }
            }
        }
        Err("too many images of the same name".into())
    }
}

const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(DIGITS[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        acc = acc << 6 | DIGITS.iter().position(|&d| d == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

impl Project {
    pub fn new(mode: Mode, values: &ParamValues, excluded: &[String], scene: Option<String>) -> Self {
        Self {
            app: "Ocean Foundry".into(),
            version: crate::VERSION.into(),
            mode,
            values: values.clone(),
            excluded: excluded.to_vec(),
            scene,
            pattern_image: None,
        }
    }
}

pub fn write(path: &Path, project: &Project) -> Result<(), String> {
    let text = serde_json::to_string_pretty(project).map_err(|e| e.to_string())?;
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, text).map_err(|e| e.to_string())?;
    std::fs::rename(&part, path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        e.to_string()
    })
}

/// Read a project. Settings it does not have keep their standard values,
/// and values out of range are brought back inside it.
pub fn read(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut project: Project = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    project.values.sanitise_all();
    Ok(project)
}

/// The name a profile is filed under: letters, digits, spaces, `-` and `_`.
pub fn profile_name(wanted: &str) -> String {
    let name: String = wanted.trim().chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).take(40).collect();
    name.trim().to_string()
}

pub fn profile_path(data: &Path, name: &str) -> PathBuf {
    data.join(PROFILES).join(format!("{}.{EXTENSION}", profile_name(name)))
}

/// The profiles in the data folder, by name.
pub fn profiles(data: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(data.join(PROFILES))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == EXTENSION))
        .filter_map(|path| Some((path.file_stem()?.to_str()?.to_string(), path)))
        .collect();
    found.sort_by_key(|(name, _)| name.to_lowercase());
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{find, Value};

    #[test]
    fn a_project_reads_back_as_written_and_profiles_are_listed_by_name() {
        let dir = std::env::temp_dir().join(format!("of-project-test-{}", std::process::id()));
        let mut values = ParamValues::default();
        values.set(find("ocean.wave_height").unwrap(), Value::Float(12.5));
        let project = Project::new(Mode::Ocean, &values, &["minecraft:blue_wool".to_string()], Some("{}".into()));
        for name in ["Storm", "calm sea"] {
            write(&profile_path(&dir, name), &project).unwrap();
        }
        let listed = profiles(&dir);
        assert_eq!(listed.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["calm sea", "Storm"]);
        assert_eq!(read(&listed[0].1).unwrap(), project);
        assert_eq!(profile_name("  a/b\\c: d  "), "abc d");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn carried_files_come_back_byte_for_byte() {
        for bytes in [&b""[..], b"a", b"ab", b"abc", b"abcd", &(0..=255u8).collect::<Vec<_>>()] {
            assert_eq!(decode(&encode(bytes)).unwrap(), bytes);
        }
        assert_eq!(encode(b"Man"), "TWFu");
        assert_eq!(encode(b"Ma"), "TWE=");
        let dir = std::env::temp_dir().join(format!("of-carry-test-{}", std::process::id()));
        let source = dir.join("ripples.png");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&source, b"first").unwrap();
        let carried = Carried::read(&source).unwrap();
        let data = dir.join("data");
        let placed = carried.place(&data).unwrap();
        assert_eq!(std::fs::read(&placed).unwrap(), b"first");
        assert_eq!(carried.place(&data).unwrap(), placed, "the same file is not copied twice");
        std::fs::write(&source, b"second").unwrap();
        let other = Carried::read(&source).unwrap().place(&data).unwrap();
        assert_ne!(other, placed, "a different file of the same name is kept apart");
        assert_eq!(std::fs::read(&placed).unwrap(), b"first");
        let mut project = Project::new(Mode::Ocean, &ParamValues::default(), &[], None);
        project.pattern_image = Some(carried);
        write(&dir.join("p.ofproject"), &project).unwrap();
        assert_eq!(read(&dir.join("p.ofproject")).unwrap(), project);
        assert!(!dir.join("p.ofproject.part").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
