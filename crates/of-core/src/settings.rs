//! Saved settings and automatic saving.

use crate::i18n::Lang;
use crate::params::{Mode, ParamValues};
use crate::paths::write_atomic;
use crate::stage::BakeStamp;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SETTINGS_FILE: &str = "settings.json";
pub const SCHEMA: u32 = 1;

/// Which processor does the heavy calculation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComputeChoice {
    /// Best working GPU, or the CPU if none passes the self-test.
    #[default]
    Auto,
    /// Never use a GPU for calculation.
    Cpu,
    /// A specific adapter, remembered by name and backend.
    Adapter { name: String, backend: String },
}

/// Whether the app shows its light or its dark look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Follow the Windows light or dark setting.
    #[default]
    Auto,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::Auto, Appearance::Light, Appearance::Dark];

    /// Name used in settings, launch options and string keys (`appearance.<key>`).
    pub fn key(self) -> &'static str {
        match self {
            Appearance::Auto => "auto",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema: u32,
    /// Version that last wrote the file, for diagnostics.
    pub app_version: String,
    pub language: Lang,
    pub appearance: Appearance,
    pub mode: Mode,
    pub compute: ComputeChoice,
    pub advanced_open: bool,
    /// Bake again automatically whenever a setting changes.
    pub auto_bake: bool,
    pub values: ParamValues,
    /// One stamp per mode that has been baked.
    pub bakes: Vec<BakeStamp>,
    /// Blocks the coloured surface may not use.
    pub excluded: Vec<String>,
    /// The settings of the last few bakes, newest first.
    pub history: Vec<PastBake>,
}

/// How many past bakes are remembered.
pub const HISTORY: usize = 5;

/// The settings a bake was made with, and when (seconds since 1970).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PastBake {
    pub when: u64,
    pub mode: Mode,
    pub values: ParamValues,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            app_version: crate::VERSION.to_string(),
            language: Lang::En,
            appearance: Appearance::Auto,
            mode: Mode::Ocean,
            compute: ComputeChoice::Auto,
            advanced_open: false,
            auto_bake: true,
            values: ParamValues::default(),
            bakes: Vec::new(),
            excluded: Vec::new(),
            history: Vec::new(),
        }
    }
}

impl Settings {
    /// Remember the settings just baked. Baking the same settings again
    /// only renews their time.
    pub fn remember_bake(&mut self) {
        let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let (mode, values) = (self.mode, self.values.clone());
        self.history.retain(|past| !(past.mode == mode && past.values == values));
        self.history.insert(0, PastBake { when, mode, values });
        self.history.truncate(HISTORY);
    }

    pub fn bake_for(&self, mode: Mode) -> Option<&BakeStamp> {
        self.bakes.iter().find(|b| b.mode == mode)
    }
}

/// What happened when settings were read at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadNote {
    /// First run: no file yet.
    Fresh,
    Loaded,
    /// The file could not be read. It was kept under this name and defaults
    /// were used, so nothing the user had is silently destroyed.
    RecoveredFrom(PathBuf),
    /// Written by a newer version of the app; left untouched on disk and
    /// preserved under this name before this version writes its own.
    NewerSchema(PathBuf),
}

pub fn load(dir: &Path) -> (Settings, LoadNote) {
    let path = dir.join(SETTINGS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), LoadNote::Fresh),
        Err(_) => return (Settings::default(), LoadNote::RecoveredFrom(set_aside(&path, "unreadable"))),
    };
    match serde_json::from_str::<Settings>(&text) {
        Ok(mut s) => {
            if s.schema > SCHEMA {
                return (Settings::default(), LoadNote::NewerSchema(set_aside(&path, "newer")));
            }
            s.schema = SCHEMA;
            s.values.sanitise_all();
            (s, LoadNote::Loaded)
        }
        Err(_) => (Settings::default(), LoadNote::RecoveredFrom(set_aside(&path, "corrupt"))),
    }
}

fn set_aside(path: &Path, why: &str) -> PathBuf {
    let backup = path.with_file_name(format!("settings.{why}.json"));
    let _ = std::fs::copy(path, &backup);
    backup
}

pub fn save(dir: &Path, settings: &Settings) -> std::io::Result<()> {
    let mut s = settings.clone();
    s.schema = SCHEMA;
    s.app_version = crate::VERSION.to_string();
    let text = serde_json::to_string_pretty(&s).map_err(std::io::Error::other)?;
    write_atomic(&dir.join(SETTINGS_FILE), text.as_bytes())
}

/// Saves settings shortly after the last change, so dragging a slider causes
/// one write when the user pauses, not hundreds while it moves.
#[derive(Debug)]
pub struct Autosaver {
    dir: PathBuf,
    delay: Duration,
    dirty_since: Option<Instant>,
    last_error: Option<String>,
}

impl Autosaver {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, delay: Duration::from_millis(600), dirty_since: None, last_error: None }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty_since.is_some()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Call every frame. Returns how long until the next save is due, if one
    /// is pending, so the UI can wake itself up for it.
    pub fn tick(&mut self, settings: &Settings) -> Option<Duration> {
        let since = self.dirty_since?;
        let waited = since.elapsed();
        if waited < self.delay {
            return Some(self.delay - waited);
        }
        self.flush(settings);
        None
    }

    /// Save now if anything is pending. Called on exit.
    pub fn flush(&mut self, settings: &Settings) {
        if self.dirty_since.take().is_some() {
            self.last_error = save(&self.dir, settings).err().map(|e| e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{find, Value};

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("of-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn round_trips_everything() {
        let dir = temp_dir("roundtrip");
        let mut s = Settings::default();
        s.language = Lang::ZhCn;
        s.appearance = Appearance::Light;
        s.mode = *Mode::ALL.last().unwrap();
        s.compute = ComputeChoice::Adapter { name: "Some GPU".into(), backend: "Dx12".into() };
        s.values.set(find("ocean.seed").unwrap(), Value::Int(1234));
        s.values.set(find("ocean.wave_height").unwrap(), Value::Float(0.1 + 0.2));
        s.bakes.push(BakeStamp::capture(&s.values, Mode::Ocean));
        save(&dir, &s).unwrap();
        let (back, note) = load(&dir);
        assert_eq!(note, LoadNote::Loaded);
        assert_eq!(back, s, "floats and all fields must survive exactly");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_run_gives_defaults() {
        let dir = temp_dir("fresh");
        assert_eq!(load(&dir), (Settings::default(), LoadNote::Fresh));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_kept_aside_not_destroyed() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(SETTINGS_FILE), "{ this is not json").unwrap();
        let (s, note) = load(&dir);
        assert_eq!(s, Settings::default());
        match note {
            LoadNote::RecoveredFrom(p) => assert_eq!(std::fs::read_to_string(p).unwrap(), "{ this is not json"),
            other => panic!("unexpected {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_and_out_of_range_entries_are_tolerated() {
        let dir = temp_dir("tolerant");
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"schema":1,"language":"zh-CN","values":{"ocean.width":99999,"gone.param":3},"future_field":true}"#,
        )
        .unwrap();
        let (s, note) = load(&dir);
        assert_eq!(note, LoadNote::Loaded);
        assert_eq!(s.language, Lang::ZhCn);
        assert_eq!(s.values.get(find("ocean.width").unwrap()), Value::Int(crate::params::MAX_EXTENT));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_schema_is_not_overwritten_blindly() {
        let dir = temp_dir("newer");
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"schema":999}"#).unwrap();
        let (_, note) = load(&dir);
        assert!(matches!(note, LoadNote::NewerSchema(p) if p.exists()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autosave_waits_then_writes_once() {
        let dir = temp_dir("autosave");
        let mut auto = Autosaver::new(dir.clone());
        auto.delay = Duration::from_millis(30);
        let s = Settings::default();
        assert_eq!(auto.tick(&s), None, "nothing to save yet");
        auto.mark_dirty();
        assert!(auto.tick(&s).is_some(), "must wait for the pause");
        assert!(!dir.join(SETTINGS_FILE).exists());
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(auto.tick(&s), None);
        assert!(dir.join(SETTINGS_FILE).exists());
        assert!(!auto.is_dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
