//! The ocean in the app: baking it in the background, showing it, and
//! saving it as a build file.
//!
//! The generator's internal numbers are read from `ocean-tuning.json` in the
//! data folder at every bake, so the method can be adjusted without a new
//! build of the program. The land the sea is baked around, and the rivers
//! and waterfalls placed on it, are kept in `ocean-scene.json` beside it.

use crate::app::HoverHelp;
use crate::files::{self, format_label, format_tips, group, note_text};
use crate::preview::{ListAction, Viewer};
use crate::theme;
use crate::widgets;
use eframe::egui::{self, Align2, Color32, Rect, RichText, Sense, Stroke};
use of_core::i18n::I18n;
use of_core::jobs::{spawn, JobCtx, JobError, JobHandle, Outcome, Snapshot};
use of_core::params::{Mode, ParamValues, Stage};
use of_core::settings::Settings;
use of_core::stage::BakeStamp;
use of_formats::catalogue;
use of_formats::export::{self, AirMode, ExportOptions, Selection, COMFORTABLE_REGIONS};
use of_formats::model::{Build, Format, Note};
use of_formats::Progress;
#[cfg(feature = "dev")]
use of_ocean::lava::LavaSettings;
use of_ocean::{Baked, Ground, Land, OceanSettings, Outcome as RiverEnd, Pond, PondEnd, SceneSetup, ShoreKind, Spill, Stream, Survey, Tuning, Zone, ZoneKind, SCENE_FILE, TUNING_BACKUP, TUNING_FILE, TUNING_VERSION};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Explains every number in the tuning file; written beside it.
const TUNING_NOTES: &str = include_str!("../../../assets/TUNING.txt");
pub const TUNING_NOTES_FILE: &str = "TUNING.txt";

/// How the build on screen stands against the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Never,
    Current,
    /// Settings changed since the bake; holds the earliest stage affected.
    Stale(Stage),
    /// The tuning file changed since the bake.
    StaleTuning,
}

/// What a finished bake job means for the rest of the app.
pub enum Event {
    Baked,
    Cancelled,
    Failed(String),
}

struct BakeOut {
    baked: Arc<Baked>,
    tuning: String,
    scene: String,
    excluded: Vec<String>,
    colour_glass: bool,
    placement_y: i32,
}

struct Running {
    handle: JobHandle<BakeOut>,
    stamp: BakeStamp,
    /// The tuning and the scene it was started with.
    recipe: String,
    /// Cancelled because the settings moved on; a new bake follows.
    superseded: bool,
    started: Instant,
}

struct Current {
    baked: Arc<Baked>,
    generation: u64,
    stamp: BakeStamp,
    tuning: String,
    scene: String,
    excluded: Vec<String>,
    colour_glass: bool,
    placement_y: i32,
}

struct Saved {
    path: PathBuf,
    blocks: u64,
    regions: usize,
    bytes: usize,
    notes: Vec<Note>,
}

struct ExportPanel {
    open: bool,
    format: Format,
    selection: Selection,
    omit_air: bool,
    /// Save only a part of the plot: west, north, east, south edges, in
    /// blocks from its north-west corner, all included.
    crop: bool,
    crop_box: [i32; 4],
    /// The block a drag across the area map started at.
    drag_from: Option<(i32, i32)>,
    job: Option<files::Job<Saved>>,
    saved: Option<Saved>,
    problem: Option<Note>,
}

pub struct OceanTool {
    /// Which kind of surface this tool bakes: the sea or lava.
    mode: Mode,
    dir: PathBuf,
    job: Option<Running>,
    current: Option<Current>,
    generation: u64,
    /// Settings and tuning of a bake that was cancelled or failed. It is not
    /// tried again by itself until one of them changes.
    gave_up: Option<(BakeStamp, String)>,
    /// What the next automatic bake is waiting to bake, and since when.
    waiting: Option<((BakeStamp, String), Instant)>,
    hurried: bool,
    tuning_text: String,
    tuning_read: Option<Instant>,
    /// The tuning file was from an earlier method and has been replaced.
    tuning_replaced: bool,
    /// The land, rivers and waterfalls the sea is baked around.
    scene: SceneSetup,
    scene_text: String,
    scene_dirty: bool,
    /// The small map of the land, and what it is a map of.
    map: Option<SceneReading>,
    /// The map of a build, being drawn in the background.
    mapping: Option<(MapKey, JobHandle<Option<LandRead>>)>,
    /// A map that could not be drawn; it is not tried again as it is.
    unmapped: Option<MapKey>,
    /// The build brought in as the land: the file it was read from, the
    /// build, and its size in blocks.
    brought: Option<(String, Arc<Build>, [i32; 3])>,
    /// The build file being read.
    reading: Option<(String, files::Job<(Arc<Build>, [i32; 3])>)>,
    /// A build file that could not be read, and why.
    unread: Option<(String, Note)>,
    /// The ripples' image last checked, when it was changed, and why it
    /// could not be read (`None` when it can).
    pattern_check: Option<(String, Option<std::time::SystemTime>, Option<String>)>,
    /// A bake was asked for while the build file was still being read.
    bake_when_read: bool,
    /// The river whose controls are shown.
    picked: usize,
    /// The pond whose controls are shown.
    pond_picked: usize,
    /// Which zone's settings are shown.
    zone_picked: usize,
    /// Blocks the coloured surface may not use.
    pub excluded: Vec<String>,
    /// The last bake: its columns, its blocks and the seconds it took.
    last: Option<(f64, f64, f64)>,
    /// Why the last bake failed; cleared by the next success.
    pub error: Option<String>,
    viewer: Viewer,
    export: ExportPanel,
    status: Status,
}

/// The text of the tuning file. A missing file (first run, or deleted to
/// get the defaults back) is written out with the defaults. A file written
/// for an earlier method would not be understood, so it is set aside and
/// replaced; the second value says that this happened.
fn read_tuning(dir: &Path) -> (String, bool) {
    let path = dir.join(TUNING_FILE);
    let fresh = || {
        let text = Tuning::default().to_text();
        if let Err(e) = std::fs::write(&path, &text) {
            log::warn!("could not write {}: {e}", path.display());
        }
        text
    };
    match std::fs::read_to_string(&path) {
        Ok(text) if Tuning::version_of(&text) < TUNING_VERSION => {
            if let Err(e) = std::fs::write(dir.join(TUNING_BACKUP), &text) {
                log::warn!("could not keep the earlier tuning file: {e}");
            }
            (fresh(), true)
        }
        Ok(text) => (text, false),
        Err(_) => (fresh(), false),
    }
}

/// The ripple tile in an image file: a normal map, or grey heights.
pub fn read_pattern(file: &str, flip: of_ocean::pattern::FlipY) -> Result<Arc<of_ocean::pattern::NormalMap>, String> {
    let image = image::open(file).map_err(|e| format!("{file}: {e}"))?.to_rgb8();
    let (w, h) = (image.width() as usize, image.height() as usize);
    let (map, _) = of_ocean::pattern::NormalMap::from_rgb(w, h, image.as_raw(), flip)?;
    Ok(Arc::new(map))
}

fn read_scene(dir: &Path) -> SceneSetup {
    match std::fs::read_to_string(dir.join(SCENE_FILE)) {
        Ok(text) => SceneSetup::from_text(&text).unwrap_or_else(|e| {
            log::warn!("{SCENE_FILE} could not be read ({e}); starting from open sea");
            SceneSetup::default()
        }),
        Err(_) => SceneSetup::default(),
    }
}

/// What a map of the land is a map of: the scene, the plot and the wave
/// direction.
type MapKey = (String, i32, i32, i32);

/// The picture of the land on the scene card, and what was read from the
/// land to draw it: the kind of every shore and how each river ends.
struct SceneReading {
    key: MapKey,
    texture: egui::TextureHandle,
    ends: Vec<RiverEnd>,
    /// How each pond came out.
    ponds: Vec<PondEnd>,
    /// Which of the kinds in `SHORE_KEY` the land has.
    seen: [bool; 5],
}

impl SceneReading {
    fn new(ctx: &egui::Context, key: MapKey, read: LandRead) -> Self {
        let image = egui::ColorImage::new(read.size, read.dots);
        Self { key, texture: ctx.load_texture("scene-map", image, egui::TextureOptions::NEAREST), ends: read.ends, ponds: read.ponds, seen: read.seen }
    }
}

/// A map of the land before it is put on screen: its dots row after row,
/// how many there are across and down, how each river ends, and which of
/// the kinds in `SHORE_KEY` the land has.
struct LandRead {
    dots: Vec<Color32>,
    size: [usize; 2],
    ends: Vec<RiverEnd>,
    ponds: Vec<PondEnd>,
    seen: [bool; 5],
}

/// Lay the land with its rivers and read its shore, for the map. `brought`
/// is the build the land is made of, where it is one.
fn read_land(scene: &SceneSetup, brought: Option<&Arc<Build>>, width: i32, length: i32, direction: f64) -> Option<LandRead> {
    let (ground, ends) = scene.laid_with(width, length, direction, brought).ok()??;
    let ponds = ground.ponds().to_vec();
    let survey = Survey::new(&ground);
    // One dot of the picture for every `step` blocks.
    let step = ((width.max(length) as f32 / 400.0).ceil() as i32).max(1);
    let (mw, ml) = ((width / step).max(1), (length / step).max(1));
    let mut seen = [false; 5];
    let mut dots = Vec::with_capacity((mw * ml) as usize);
    for pz in 0..ml {
        for px in 0..mw {
            let (x, z) = (px * step, pz * step);
            let off = survey.off(x, z);
            let on_shore = off > -(0.5 * step as f64 + 0.5) && off <= step as f64 + 0.5;
            let mark = if survey.channel(x, z) {
                Some(4)
            } else if on_shore {
                survey.nearest(x, z).map(|(kind, room)| match kind {
                    ShoreKind::Beach if room >= 0.5 => 1,
                    ShoreKind::Beach => 0,
                    ShoreKind::Rocky => 2,
                    ShoreKind::Cliff => 3,
                })
            } else {
                None
            };
            let h = ground.height(x, z);
            dots.push(if let Some(n) = mark {
                seen[n] = true;
                SHORE_KEY[n].1
            } else if h < -0.5 && ground.roof(x, z).is_none() {
                theme::mix(Color32::from_rgb(92, 164, 216), Color32::from_rgb(40, 96, 164), (-h / 10.0).clamp(0.0, 1.0))
            } else {
                match ground.kind(x, z) {
                    Ground::Sand => Color32::from_rgb(240, 175, 21),
                    Ground::Grass => Color32::from_rgb(94, 169, 24),
                    Ground::Stream => Color32::from_rgb(120, 200, 250),
                    Ground::Fall => Color32::from_rgb(236, 243, 250),
                    Ground::Pond => Color32::from_rgb(60, 118, 214),
                    Ground::Rock => Color32::from_rgb(125, 125, 115),
                }
            });
        }
    }
    Some(LandRead { dots, size: [mw as usize, ml as usize], ends, ponds, seen })
}

/// Help for something in the settings panel, shown on the card beside the
/// panel while the pointer rests on it, so nothing has to be explained in
/// the panel itself. `key` is its name; `key.help` is what is said about it.
fn side_help(response: &egui::Response, i18n: &I18n, key: &str, help: &mut Option<HoverHelp>, forced: Option<&str>) {
    if response.hovered() || forced == Some(key) {
        *help = Some(HoverHelp { title: i18n.t(key).to_string(), body: i18n.t(&format!("{key}.help")).to_string(), anchor_y: response.rect.top() });
    }
}

/// The kinds of shore the map tells apart, with the colour of each.
const SHORE_KEY: [(&str, Color32); 5] = [
    ("scene.shore.beach", Color32::from_rgb(255, 250, 235)),
    ("scene.shore.delta", Color32::from_rgb(196, 92, 232)),
    ("scene.shore.rocky", Color32::from_rgb(214, 112, 40)),
    ("scene.shore.cliff", Color32::from_rgb(222, 48, 56)),
    ("scene.shore.channel", Color32::from_rgb(40, 214, 200)),
];

impl OceanTool {
    pub fn new(ctx: &egui::Context, dir: &Path, mode: Mode) -> Self {
        let notes = dir.join(TUNING_NOTES_FILE);
        let wanted = TUNING_NOTES.replace("\r\n", "\n").replace('\n', "\r\n");
        if std::fs::read_to_string(&notes).ok().as_deref() != Some(wanted.as_str()) {
            if let Err(e) = std::fs::write(&notes, &wanted) {
                log::warn!("could not write {}: {e}", notes.display());
            }
        }
        let (tuning_text, tuning_replaced) = read_tuning(dir);
        // Lava lies on nothing but its own foundation: it has no land.
        let scene = if mode == Mode::Ocean { read_scene(dir) } else { SceneSetup::default() };
        Self {
            mode,
            dir: dir.to_path_buf(),
            job: None,
            current: None,
            generation: 0,
            gave_up: None,
            waiting: None,
            hurried: false,
            tuning_text,
            tuning_read: Some(Instant::now()),
            tuning_replaced,
            scene_text: scene.to_text(),
            scene,
            scene_dirty: false,
            map: None,
            mapping: None,
            unmapped: None,
            brought: None,
            reading: None,
            unread: None,
            pattern_check: None,
            bake_when_read: false,
            picked: 0,
            pond_picked: 0,
            zone_picked: 0,
            excluded: Vec::new(),
            last: None,
            error: None,
            viewer: Viewer::new(ctx),
            export: ExportPanel {
                open: false,
                format: Format::Litematic,
                selection: Selection::Complete,
                omit_air: true,
                crop: false,
                crop_box: [0, 0, 63, 63],
                drag_from: None,
                job: None,
                saved: None,
                problem: None,
            },
            status: Status::Never,
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// Everything outside the settings that a bake depends on.
    fn recipe(&self) -> String {
        format!("{}\u{1}{}\u{1}{}", self.tuning_text, self.scene_text, self.excluded.join(","))
    }

    /// What to say about a failed bake; the build from before it, if there
    /// is one, is still there.
    pub fn failure(&self, i18n: &I18n, error: &str) -> String {
        let key = if self.current.is_some() { "bake.failed.kept" } else { "bake.failed" };
        i18n.tf(key, &[("error", error)])
    }

    pub fn baking(&self) -> bool {
        self.job.is_some()
    }

    pub fn open_export(&mut self) {
        self.export.open = true;
    }

    pub fn active_job(&self) -> Option<(&'static str, Snapshot)> {
        if let Some(j) = &self.export.job {
            return Some(("job.export", j.snapshot()));
        }
        if let (Some(r), None) = (&self.reading, &self.job) {
            return Some(("job.file_open", r.1.snapshot()));
        }
        self.job.as_ref().map(|r| ("job.bake", r.handle.snapshot()))
    }

    /// Cancel whatever is running, at the user's request.
    pub fn cancel(&mut self) {
        if let Some(j) = &self.export.job {
            j.cancel();
        } else if let Some(r) = &mut self.job {
            r.superseded = false;
            r.handle.cancel();
        }
    }

    pub fn cancel_all(&self) {
        if let Some(j) = &self.export.job {
            j.cancel();
        }
        if let Some(r) = &self.job {
            r.handle.cancel();
        }
    }

    /// For automated runs: true once nothing is being worked out and the
    /// finished picture is on screen.
    pub fn settled(&self) -> bool {
        self.job.is_none()
            && self.export.job.is_none()
            && self.reading.is_none()
            && self.mapping.is_none()
            && !self.bake_when_read
            && match &self.current {
                Some(c) => self.viewer.settled(c.generation),
                None => true,
            }
    }

    pub fn start(&mut self, ctx: &egui::Context, values: &ParamValues, i18n: &I18n) {
        if self.job.is_some() {
            return;
        }
        #[cfg(feature = "dev")]
        if self.mode == Mode::Lava {
            self.start_lava(ctx, values, i18n);
            return;
        }
        // Land that is a build needs its file read first.
        self.want_build(ctx);
        let brought = self.build_for_scene();
        if self.scene.land == Land::Build && brought.is_none() {
            if self.reading.is_some() {
                self.bake_when_read = true;
            } else {
                let file = &self.scene.build.file;
                self.error = Some(match self.unread.as_ref().filter(|u| &u.0 == file) {
                    Some((_, note)) => note_text(i18n, note),
                    None => i18n.t("scene.build.none").to_string(),
                });
            }
            return;
        }
        let stamp = BakeStamp::capture(values, self.mode);
        let (dir, values, scene) = (self.dir.clone(), values.clone(), self.scene.clone());
        let excluded = self.excluded.clone();
        let bad_tuning = i18n.tf("bake.tuning_error", &[("file", TUNING_FILE)]);
        let (over, over_estimate) = memory_texts(i18n);
        let ctx = ctx.clone();
        let handle = spawn("job.bake", move || ctx.request_repaint(), move |job: &JobCtx| {
            let (text, _) = read_tuning(&dir);
            let tuning = Tuning::from_text(&text).map_err(|e| JobError::Failed(bad_tuning.replace("{error}", &e)))?;
            let mut settings = OceanSettings::from_params(&values);
            settings.excluded = excluded;
            settings.zones = scene.zones.clone();
            if settings.pattern.source == of_ocean::pattern::Source::Image && !scene.pattern_image.is_empty() {
                // An image that cannot be read leaves the wind's ripples;
                // the scene card says so.
                settings.pattern.image = read_pattern(&scene.pattern_image, settings.pattern.flip)
                    .map_err(|e| log::warn!("ripple image not used: {e}"))
                    .ok();
            }
            let report = |fraction: f32| job.progress(fraction as f64).is_ok();
            let ground = scene
                .laid_with(settings.width, settings.length, settings.waves.direction, brought.as_ref())
                .map_err(JobError::Failed)?
                .map(|laid| laid.0);
            // Refused before it starts when the estimate is over the memory limit.
            let more = of_ocean::estimate::job(&settings, ground.as_ref());
            let whole = of_core::memory::fits(more)
                .map_err(|needs| JobError::Failed(over_estimate.replace("{size}", &of_core::memory::mb(needs))))?;
            job.expect_memory(whole);
            let baked = of_ocean::bake_with("ocean", &settings, ground.as_ref(), catalogue::default(), &tuning, &Progress::new(&report))
                .map_err(|e| bake_failed(e, &over))?;
            Ok(BakeOut {
                baked: Arc::new(baked),
                tuning: text,
                scene: scene.to_text(),
                excluded: settings.excluded,
                colour_glass: settings.colour_glass,
                placement_y: settings.placement_y,
            })
        });
        self.job = Some(Running { handle, stamp, recipe: self.recipe(), superseded: false, started: Instant::now() });
        self.gave_up = None;
        self.waiting = None;
    }

    /// Bake a field of lava from its own settings (development build only).
    #[cfg(feature = "dev")]
    fn start_lava(&mut self, ctx: &egui::Context, values: &ParamValues, i18n: &I18n) {
        let stamp = BakeStamp::capture(values, self.mode);
        let (dir, values, scene) = (self.dir.clone(), values.clone(), self.scene_text.clone());
        let bad_tuning = i18n.tf("bake.tuning_error", &[("file", TUNING_FILE)]);
        let (over, _) = memory_texts(i18n);
        let ctx = ctx.clone();
        let handle = spawn("job.bake", move || ctx.request_repaint(), move |job: &JobCtx| {
            let (text, _) = read_tuning(&dir);
            let tuning = Tuning::from_text(&text).map_err(|e| JobError::Failed(bad_tuning.replace("{error}", &e)))?;
            let settings = LavaSettings::from_params(&values);
            let report = |fraction: f32| job.progress(fraction as f64).is_ok();
            let baked = of_ocean::lava::bake_lava("lava", &settings, catalogue::default(), &tuning, &Progress::new(&report))
                .map_err(|e| bake_failed(e, &over))?;
            Ok(BakeOut { baked: Arc::new(baked), tuning: text, scene, excluded: Vec::new(), colour_glass: true, placement_y: settings.placement_y })
        });
        self.job = Some(Running { handle, stamp, recipe: self.recipe(), superseded: false, started: Instant::now() });
        self.gave_up = None;
        self.waiting = None;
    }

    /// Collect a finished bake. A cancelled or failed bake leaves the
    /// previous build in place.
    pub fn poll(&mut self, settings: &mut Settings) -> Option<Event> {
        if let Some(outcome) = self.reading.as_ref().and_then(|r| r.1.poll()) {
            if let Some((file, _)) = self.reading.take() {
                match outcome {
                    Outcome::Done(Ok((build, size))) => {
                        self.brought = Some((file, build, size));
                        self.unread = None;
                    }
                    Outcome::Done(Err(note)) => self.unread = Some((file, note)),
                    Outcome::Cancelled => self.unread = Some((file, Note::new("err.cancelled"))),
                    Outcome::Failed(e) => self.unread = Some((file, Note::new("err.read").arg("detail", e))),
                }
            }
        }
        if let Some(outcome) = self.export.job.as_ref().and_then(JobHandle::poll) {
            self.export.job = None;
            match outcome {
                Outcome::Done(Ok(saved)) => self.export.saved = Some(saved),
                Outcome::Done(Err(note)) => self.export.problem = Some(note),
                Outcome::Cancelled => self.export.problem = Some(Note::new("err.cancelled")),
                Outcome::Failed(e) => self.export.problem = Some(Note::new("err.write").arg("detail", e)),
            }
        }
        let outcome = self.job.as_ref().and_then(|r| r.handle.poll())?;
        let run = self.job.take()?;
        self.hurried = false;
        match outcome {
            Outcome::Done(out) => {
                self.generation += 1;
                let columns = out.baked.width as f64 * out.baked.length as f64;
                self.last = Some((columns.max(1.0), out.baked.stats.blocks.iter().sum::<u64>() as f64, run.started.elapsed().as_secs_f64()));
                settings.bakes.retain(|b| b.mode != self.mode);
                settings.bakes.push(run.stamp.clone());
                self.current = Some(Current {
                    baked: out.baked,
                    generation: self.generation,
                    stamp: run.stamp,
                    tuning: out.tuning,
                    scene: out.scene,
                    excluded: out.excluded,
                    colour_glass: out.colour_glass,
                    placement_y: out.placement_y,
                });
                self.error = None;
                self.export.saved = None;
                self.export.problem = None;
                Some(Event::Baked)
            }
            Outcome::Cancelled if run.superseded => {
                self.hurried = true;
                None
            }
            Outcome::Cancelled => {
                self.gave_up = Some((run.stamp, run.recipe));
                Some(Event::Cancelled)
            }
            Outcome::Failed(e) => {
                self.gave_up = Some((run.stamp, run.recipe));
                self.error = Some(e.clone());
                Some(Event::Failed(e))
            }
        }
    }

    /// Once a frame: notice a changed tuning file, work out whether the
    /// build is current, and bake again by itself when "Live" is on.
    pub fn tick(&mut self, ctx: &egui::Context, settings: &Settings, i18n: &I18n) {
        if self.tuning_read.is_none_or(|t| t.elapsed() > Duration::from_millis(700)) {
            let (text, replaced) = read_tuning(&self.dir);
            self.tuning_text = text;
            self.tuning_replaced |= replaced;
            self.tuning_read = Some(Instant::now());
        }
        ctx.request_repaint_after(Duration::from_millis(750));

        if settings.mode != self.mode {
            self.status = Status::Never;
            return;
        }
        if self.excluded != settings.excluded {
            self.excluded = settings.excluded.clone();
        }
        if self.mode == Mode::Ocean {
            self.want_build(ctx);
        }
        if self.bake_when_read && self.reading.is_none() {
            self.bake_when_read = false;
            self.start(ctx, &settings.values, i18n);
        }
        let stamp = BakeStamp::capture(&settings.values, self.mode);
        self.status = match &self.current {
            None => Status::Never,
            Some(c) => match c.stamp.stages.iter().zip(&stamp.stages).position(|(a, b)| a != b) {
                Some(i) => Status::Stale(Stage::ALL[i.min(Stage::ALL.len() - 1)]),
                None if c.scene != self.scene_text => Status::Stale(Stage::Scene),
                None if self.mode == Mode::Ocean && c.excluded != self.excluded => Status::Stale(Stage::Blocks),
                None if c.tuning != self.tuning_text => Status::StaleTuning,
                None => Status::Current,
            },
        };

        let recipe = self.recipe();
        if let Some(run) = &mut self.job {
            // A long bake of settings that have since moved on is dropped;
            // a short one is left to finish, so the view keeps up with a drag.
            let moved_on = run.stamp != stamp || run.recipe != recipe;
            if settings.auto_bake && moved_on && !run.superseded && run.started.elapsed() > Duration::from_millis(1200) {
                run.superseded = true;
                run.handle.cancel();
            }
            return;
        }
        if !settings.auto_bake || self.status == Status::Current {
            self.waiting = None;
            return;
        }
        // A build that has not been read cannot be baked around yet.
        if self.scene.land == Land::Build && self.build_for_scene().is_none() {
            self.waiting = None;
            return;
        }
        let wanted = (stamp, recipe);
        if self.gave_up.as_ref() == Some(&wanted) {
            return;
        }
        if self.waiting.as_ref().map(|w| &w.0) != Some(&wanted) {
            self.waiting = Some((wanted, Instant::now()));
        }
        // After a dropped bake, wait for the settings to rest before trying again.
        let rest = Duration::from_millis(if self.hurried { 300 } else { 0 });
        let waited = self.waiting.as_ref().map_or(Duration::ZERO, |w| w.1.elapsed());
        if waited >= rest {
            self.start(ctx, &settings.values, i18n);
        } else {
            ctx.request_repaint_after(rest - waited);
        }
    }

    // -------------------------------------------------------------- viewport

    pub fn viewport(&mut self, ui: &mut egui::Ui, rect: Rect, i18n: &I18n) -> Option<ListAction> {
        let mut action = None;
        match &self.current {
            Some(c) => {
                let (baked, generation) = (c.baked.clone(), c.generation);
                action = self.viewer.show(ui, rect, &baked, generation, i18n, &self.excluded);
            }
            None => {
                self.viewer.clear();
                if self.job.is_some() {
                    message(ui, rect, i18n.t("viewport.baking"), "");
                } else {
                    message(ui, rect, i18n.t("viewport.empty.title"), i18n.t(if self.mode == Mode::Ocean { "viewport.empty.body" } else { "viewport.lava.body" }));
                }
            }
        }
        if let Some(error) = &self.error {
            let text = self.failure(i18n, error);
            let width = (rect.width() - 80.0).clamp(200.0, 640.0);
            let place = Rect::from_center_size(rect.center_top() + egui::vec2(0.0, 76.0), egui::vec2(width, 60.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(place), |ui| {
                ui.vertical_centered(|ui| {
                    egui::Frame::new().fill(theme::bg().gamma_multiply(0.8)).corner_radius(9).inner_margin(egui::Margin::symmetric(12, 7)).show(ui, |ui| {
                        ui.label(RichText::new(text).color(theme::bad()));
                    });
                });
            });
        }
        action
    }

    /// About how many blocks the next bake will make and how many seconds
    /// it will take, going by the last one. Nothing before the first bake.
    pub fn estimate(&self, values: &ParamValues) -> Option<(u64, f64)> {
        let (columns, blocks, seconds) = self.last?;
        let size = |id: &str| values.get_id(&format!("{}.{id}", self.mode.key())).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let share = size("width") * size("length") / columns;
        Some(((blocks * share) as u64, seconds * share))
    }

    /// The land of the sea, as the text of its scene file.
    pub fn scene_text(&self) -> &str {
        &self.scene_text
    }

    /// Take the land from a project file.
    pub fn load_scene(&mut self, text: &str) -> Result<(), String> {
        self.scene = SceneSetup::from_text(text)?;
        self.scene_text = self.scene.to_text();
        self.picked = 0;
        self.pond_picked = 0;
        self.zone_picked = 0;
        self.save_scene();
        Ok(())
    }

    // ---------------------------------------------------------------- export

    pub fn export_window(&mut self, ctx: &egui::Context, i18n: &I18n, accent: Color32) {
        if !self.export.open {
            return;
        }
        let mut open = true;
        let mut pick = false;
        let status = self.status;
        let current = self.current.as_ref();
        let map = self.map.as_ref();
        let panel = &mut self.export;
        egui::Window::new(i18n.t("export.title").to_string())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos([450.0, 70.0])
            .show(ctx, |ui| {
                ui.set_width(500.0);
                let Some(current) = current else {
                    ui.label(RichText::new(i18n.t("export.blocked.never")).color(theme::warn()));
                    return;
                };
                let stats = &current.baked.stats;
                // A part with nothing in it cannot be saved.
                let available = |s: Selection| -> bool {
                    let glass: u64 = stats.blocks[3..].iter().sum();
                    s.layers(current.colour_glass).is_ok()
                        && match s {
                            Selection::Complete | Selection::WaterOnly | Selection::Foundation => true,
                            Selection::Surface => stats.blocks[2] > 0,
                            Selection::AllGlass => glass > 0,
                            Selection::Glass1 => stats.blocks[3] > 0,
                            Selection::Glass2 => stats.blocks[4] > 0,
                            Selection::Glass3 => stats.blocks[5] > 0,
                        }
                };
                if !available(panel.selection) {
                    panel.selection = Selection::Complete;
                }
                let name = |s: Selection| i18n.t(&format!("export.selection.{}", s.key())).to_string();

                ui.add_space(4.0);
                theme::card().inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("export.part")).on_hover_text(i18n.t("export.part.help"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            egui::ComboBox::from_id_salt("export_part")
                                .selected_text(name(panel.selection))
                                .icon(widgets::chevron)
                                .width(190.0)
                                .show_ui(ui, |ui| {
                                    for s in Selection::ALL {
                                        ui.add_enabled_ui(available(s), |ui| ui.selectable_value(&mut panel.selection, s, name(s)));
                                    }
                                })
                                .response
                                .on_hover_text(i18n.t("export.part.help"));
                        });
                    });
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(i18n.t("file.save.heading")).font(theme::semibold(13.0)).color(theme::text_dim()));
                });
                let formats: Vec<(Format, String)> = Format::EXPORTABLE.iter().map(|f| (*f, i18n.t(format_label(*f)).to_string())).collect();
                widgets::segmented(ui, "export_format", &mut panel.format, &formats, None, 0.0, &format_tips(i18n));
                // Save only a part of the plot.
                theme::card().inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let (w, l) = (current.baked.width.max(1), current.baked.length.max(1));
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("export.area")).on_hover_text(i18n.t("export.area.help"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Turned on, it starts as the whole plot.
                            if widgets::toggle(ui, &mut panel.crop, accent).on_hover_text(i18n.t("export.area.help")).changed() && panel.crop {
                                panel.crop_box = [0, 0, w - 1, l - 1];
                            }
                        });
                    });
                    if panel.crop {
                        let b = &mut panel.crop_box;
                        ui.horizontal(|ui| {
                            for (n, key, most) in [(0, "export.area.west", w - 1), (1, "export.area.north", l - 1), (2, "export.area.east", w - 1), (3, "export.area.south", l - 1)] {
                                ui.label(RichText::new(i18n.t(key)).small().color(theme::text_dim())).on_hover_text(i18n.t(&format!("{key}.help")));
                                ui.add(egui::DragValue::new(&mut b[n]).range(0..=most).speed(0.5)).on_hover_text(i18n.t(&format!("{key}.help")));
                            }
                        });
                        (b[0], b[2]) = (b[0].min(b[2]), b[0].max(b[2]));
                        (b[1], b[3]) = (b[1].min(b[3]), b[1].max(b[3]));
                        // The area can also be drawn by dragging across a
                        // small map of the plot.
                        ui.add_space(4.0);
                        let across = ui.available_width();
                        let tall = (across * l as f32 / w as f32).min(180.0);
                        let size = egui::vec2(tall * w as f32 / l as f32, tall);
                        let (space, response) = ui.allocate_exact_size(egui::vec2(across, tall), Sense::drag());
                        let rect = Rect::from_center_size(space.center(), size);
                        match map.filter(|m| (m.key.1, m.key.2) == (w, l)) {
                            Some(m) => egui::Image::new((m.texture.id(), size)).corner_radius(4).paint_at(ui, rect),
                            None => {
                                ui.painter().rect_filled(rect, 4, Color32::from_rgb(38, 92, 140));
                            }
                        }
                        let block = |p: egui::Pos2| -> (i32, i32) {
                            (
                                (((p.x - rect.left()) / rect.width()) * w as f32).floor().clamp(0.0, (w - 1) as f32) as i32,
                                (((p.y - rect.top()) / rect.height()) * l as f32).floor().clamp(0.0, (l - 1) as f32) as i32,
                            )
                        };
                        if response.drag_started() {
                            panel.drag_from = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos()).map(block);
                        }
                        if response.dragged() {
                            if let (Some(from), Some(p)) = (panel.drag_from, response.interact_pointer_pos()) {
                                let to = block(p);
                                *b = [from.0.min(to.0), from.1.min(to.1), from.0.max(to.0), from.1.max(to.1)];
                            }
                        }
                        if response.drag_stopped() {
                            panel.drag_from = None;
                        }
                        // Outside the area is dimmed.
                        let at = |x: i32, z: i32| rect.min + egui::vec2(x as f32 / w as f32 * rect.width(), z as f32 / l as f32 * rect.height());
                        let inside = Rect::from_min_max(at(b[0], b[1]), at(b[2] + 1, b[3] + 1));
                        let painter = ui.painter().with_clip_rect(rect);
                        let dim = Color32::from_black_alpha(120);
                        painter.rect_filled(Rect::from_min_max(rect.min, egui::pos2(rect.right(), inside.top())), 0, dim);
                        painter.rect_filled(Rect::from_min_max(egui::pos2(rect.left(), inside.bottom()), rect.max), 0, dim);
                        painter.rect_filled(Rect::from_min_max(egui::pos2(rect.left(), inside.top()), egui::pos2(inside.left(), inside.bottom())), 0, dim);
                        painter.rect_filled(Rect::from_min_max(egui::pos2(inside.right(), inside.top()), egui::pos2(rect.right(), inside.bottom())), 0, dim);
                        painter.rect_stroke(inside, 0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Inside);
                        response.on_hover_text(i18n.t("export.area.map.help"));
                    }
                });
                if panel.format == Format::Litematic {
                    theme::card().inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(i18n.t("file.omit_air")).on_hover_text(i18n.t("file.omit_air.help"));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                widgets::toggle(ui, &mut panel.omit_air, accent).on_hover_text(i18n.t("file.omit_air.help"));
                            });
                        });
                    });
                }
                let explain = match (panel.format, panel.omit_air) {
                    (Format::Litematic, true) => "file.explain.omit_air",
                    (Format::Litematic, false) => "file.explain.compact",
                    (Format::Sponge, _) => "file.explain.sponge",
                    _ => "file.explain.axiom",
                };
                // One line on how to paste it; the reasons are in its tooltip.
                ui.label(RichText::new(i18n.t(explain)).small().color(theme::text_dim())).on_hover_text(i18n.t(&format!("{explain}.help")));
                ui.add_space(2.0);
                ui.label(i18n.tf(
                    "export.place",
                    &[("y", &current.baked.export_y.to_string()), ("level", &current.placement_y.to_string())],
                ));

                let blocked = match status {
                    Status::Current => None,
                    Status::Never => Some("export.blocked.never"),
                    Status::Stale(_) | Status::StaleTuning => Some("export.blocked.stale"),
                };
                if let Some(reason) = blocked {
                    ui.label(RichText::new(i18n.t(reason)).color(theme::warn()));
                }
                ui.add_space(2.0);
                let save = widgets::primary_button(ui, i18n.t("file.save"), accent, blocked.is_none() && panel.job.is_none());
                if save.on_hover_text(i18n.t("file.save.help")).clicked() {
                    pick = true;
                }
                if let Some(problem) = &panel.problem {
                    ui.label(RichText::new(note_text(i18n, problem)).color(theme::bad()));
                }
                if let Some(saved) = &panel.saved {
                    let file = saved.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ui.label(
                        RichText::new(i18n.tf(
                            "file.saved",
                            &[
                                ("file", &file),
                                ("blocks", &group(saved.blocks)),
                                ("regions", &group(saved.regions as u64)),
                                ("size", &of_core::jobs::coarse_bytes(saved.bytes as u64)),
                            ],
                        ))
                        .color(theme::good()),
                    )
                    .on_hover_text(saved.path.display().to_string());
                    if saved.regions > COMFORTABLE_REGIONS {
                        ui.label(
                            RichText::new(i18n.tf(
                                "file.note.many_regions",
                                &[("count", &group(saved.regions as u64)), ("limit", &group(COMFORTABLE_REGIONS as u64))],
                            ))
                            .small()
                            .color(theme::warn()),
                        );
                    }
                    for note in saved.notes.iter().filter(|n| n.key != "note.many_regions") {
                        ui.label(RichText::new(note_text(i18n, note)).small().color(theme::text_dim()));
                    }
                }
                ui.add_space(2.0);
            });
        if !open {
            self.export.open = false;
        }
        if pick {
            let format = self.export.format;
            let mut name = i18n.t(if self.mode == Mode::Ocean { "export.name" } else { "export.name.lava" }).to_string();
            if self.export.selection != Selection::Complete {
                name.push('-');
                name.push_str(&self.export.selection.key().replace('_', "-"));
            }
            let picked = rfd::FileDialog::new()
                .add_filter(i18n.t(format_label(format)), &[format.extension()])
                .set_file_name(format!("{name}.{}", format.extension()))
                .save_file();
            if let Some(path) = picked {
                self.save(ctx, path);
            }
        }
    }

    /// Write the build on screen to `path` with the options chosen in the export window.
    pub fn save(&mut self, ctx: &egui::Context, path: PathBuf) {
        let Some(current) = &self.current else { return };
        let panel = &mut self.export;
        let baked = current.baked.clone();
        let format = panel.format;
        let options = ExportOptions {
            format,
            selection: panel.selection,
            air: if panel.omit_air && format == Format::Litematic { AirMode::OmitAir } else { AirMode::Compact },
            include_colour_glass: current.colour_glass,
        };
        panel.problem = None;
        panel.saved = None;
        // The part to save, kept to the plot. A part wholly outside it (the
        // plot may have shrunk since it was set) is refused.
        let (w, l) = (baked.width.max(1), baked.length.max(1));
        let crop = match panel.crop.then_some(panel.crop_box) {
            Some(b) if b[0] > w - 1 || b[2] < 0 || b[1] > l - 1 || b[3] < 0 => {
                panel.problem = Some(Note::new("err.area_outside").arg("width", w).arg("length", l));
                return;
            }
            Some(b) => Some([b[0].max(0), b[1].max(0), b[2].min(w - 1), b[3].min(l - 1)]),
            None => None,
        };
        panel.job = Some(files::run("job.export", ctx, move |progress| {
            // The blocks are read from the bake's columns as the file is written.
            let out = export::export(&baked.source(crop), &options, catalogue::default(), progress)?;
            // Written beside the target first, so a failure cannot leave half a file.
            let partial = path.with_extension(format!("{}.part", format.extension()));
            std::fs::write(&partial, &out.bytes)
                .and_then(|_| std::fs::rename(&partial, &path))
                .map_err(|e| Note::new("err.write").arg("detail", e))?;
            Ok(Saved { path, blocks: out.blocks, regions: out.regions, bytes: out.bytes.len(), notes: out.notes })
        }));
    }

    /// For automated runs: how the last export ended, once it has.
    pub fn export_report(&self, i18n: &I18n) -> Option<Result<String, String>> {
        if self.export.job.is_some() {
            return None;
        }
        if let Some(problem) = &self.export.problem {
            return Some(Err(note_text(i18n, problem)));
        }
        let s = self.export.saved.as_ref()?;
        Some(Ok(format!("saved {}: {} blocks, {} bytes", s.path.display(), s.blocks, s.bytes)))
    }

    /// For automated runs: choose the export options.
    pub fn preset_export(&mut self, format: Option<Format>, selection: Option<Selection>, omit_air: bool) {
        self.export.format = format.unwrap_or(self.export.format);
        self.export.selection = selection.unwrap_or(self.export.selection);
        self.export.omit_air = omit_air;
    }

    /// For automated runs: save only this part of the plot.
    pub fn preset_area(&mut self, area: [i32; 4]) {
        self.export.crop = true;
        self.export.crop_box = area;
    }

    // ----------------------------------------------------------------- scene

    /// The image the ripples are taken from, if one is chosen.
    pub fn pattern_image(&self) -> &str {
        &self.scene.pattern_image
    }

    /// For automated runs: take the ripples from this image.
    pub fn set_pattern_image(&mut self, file: &str) {
        self.scene.pattern_image = file.to_string();
        self.scene_text = self.scene.to_text();
        self.save_scene();
    }

    /// For automated runs: show only these layers, and cut the build open
    /// this share of the way across.
    pub fn set_view(&mut self, layers: Option<crate::preview::Layers>, cut: Option<f64>) {
        if let Some(layers) = layers {
            self.viewer.layers = layers;
        }
        if let Some(cut) = cut {
            self.viewer.cutaway = true;
            self.viewer.cut = cut.clamp(0.0, 1.0);
        }
    }

    /// For automated runs: open the viewport on this view.
    pub fn set_show(&mut self, show: crate::preview::Show) {
        self.viewer.show = show;
    }

    /// For automated runs: choose the land, with its own running water.
    pub fn set_land(&mut self, land: Land) {
        let build = std::mem::take(&mut self.scene.build);
        self.scene = SceneSetup::of(land, self.scene.seed);
        self.scene.build = build;
        self.scene_text = self.scene.to_text();
        self.save_scene();
    }

    /// For automated runs: bring in a build file as the land, with the
    /// water resting this share of the way up it.
    pub fn set_build(&mut self, file: &str, waterline: Option<f64>) {
        self.set_land(Land::Build);
        self.scene.build.file = file.to_string();
        if let Some(share) = waterline {
            self.scene.build.waterline = share.clamp(0.0, 1.0);
        }
        self.scene_text = self.scene.to_text();
        self.save_scene();
    }

    /// The build the land is made of, once its file has been read.
    fn build_for_scene(&self) -> Option<Arc<Build>> {
        self.brought.as_ref().filter(|b| b.0 == self.scene.build.file).map(|b| b.1.clone())
    }

    /// Read the build file of the land in the background, unless that has
    /// been done, is under way, or has failed for this file.
    fn want_build(&mut self, ctx: &egui::Context) {
        let file = self.scene.build.file.clone();
        if self.scene.land != Land::Build || file.is_empty() {
            return;
        }
        let known = |name: Option<&String>| name == Some(&file);
        if known(self.brought.as_ref().map(|b| &b.0)) || known(self.reading.as_ref().map(|r| &r.0)) || known(self.unread.as_ref().map(|u| &u.0)) {
            return;
        }
        let path = PathBuf::from(&file);
        let job = files::run("job.file_open", ctx, move |progress| {
            let data = std::fs::read(&path).map_err(|e| Note::new("err.read").arg("detail", e))?;
            let build = of_formats::import(&data, progress)?.build;
            let size = build.bounds().map(|(lo, hi)| [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1]).unwrap_or([0; 3]);
            Ok((Arc::new(build), size))
        });
        self.reading = Some((file, job));
    }

    /// The row of the scene card for the image the ripples are taken from.
    fn pattern_rows(&mut self, ui: &mut egui::Ui, i18n: &I18n, help: &mut Option<HoverHelp>, forced: Option<&str>) {
        ui.horizontal(|ui| {
            let name = ui.label(RichText::new(i18n.t("scene.pattern.file")).color(theme::text_dim()));
            side_help(&name, i18n, "scene.pattern.file", help, forced);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(i18n.t("scene.build.choose")).on_hover_text(i18n.t("scene.pattern.choose.help")).clicked() {
                    let picked = rfd::FileDialog::new()
                        .add_filter(i18n.t("scene.pattern.filter"), &["png", "jpg", "jpeg"])
                        .add_filter(i18n.t("file.filter.any"), &["*"])
                        .pick_file();
                    if let Some(path) = picked {
                        self.scene.pattern_image = path.to_string_lossy().into_owned();
                    }
                }
            });
        });
        let file = self.scene.pattern_image.clone();
        let problem = self.pattern_problem(&file);
        let (shown, colour) = if file.is_empty() {
            (i18n.t("scene.pattern.none").to_string(), theme::warn())
        } else {
            (Path::new(&file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone()), theme::text_dim())
        };
        ui.add(egui::Label::new(RichText::new(shown).small().color(colour)).truncate()).on_hover_text(file.as_str());
        if let Some(why) = problem {
            ui.add(egui::Label::new(RichText::new(i18n.t("scene.pattern.unreadable")).small().color(theme::warn())).truncate())
                .on_hover_text(why);
        }
    }

    /// Why the ripples' image cannot be read, if it cannot. The answer is
    /// kept until the file or its time of change differs.
    fn pattern_problem(&mut self, file: &str) -> Option<String> {
        if file.is_empty() {
            return None;
        }
        let changed = std::fs::metadata(file).and_then(|m| m.modified()).ok();
        match &self.pattern_check {
            Some((f, t, why)) if f == file && *t == changed => why.clone(),
            _ => {
                let why = read_pattern(file, of_ocean::pattern::FlipY::Auto).err();
                self.pattern_check = Some((file.to_string(), changed, why.clone()));
                why
            }
        }
    }

    /// The rows of the scene card for land that is a build: its file, how
    /// far up it the water rests, and where it stands.
    fn build_rows(&mut self, ui: &mut egui::Ui, i18n: &I18n, accent: Color32, help: &mut Option<HoverHelp>, forced: Option<&str>) {
        ui.horizontal(|ui| {
            let name = ui.label(RichText::new(i18n.t("scene.build.file")).color(theme::text_dim()));
            side_help(&name, i18n, "scene.build.file", help, forced);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(i18n.t("scene.build.choose")).on_hover_text(i18n.t("scene.build.choose.help")).clicked() {
                    let picked = rfd::FileDialog::new()
                        .add_filter(i18n.t("file.filter.all"), &["litematic", "schem", "schematic", "bp"])
                        .add_filter(i18n.t("file.filter.any"), &["*"])
                        .pick_file();
                    if let Some(path) = picked {
                        self.scene.build.file = path.to_string_lossy().into_owned();
                        self.unread = None;
                    }
                }
            });
        });
        // The file's name and what is known of it, a line each.
        let file = self.scene.build.file.clone();
        if !file.is_empty() {
            let name = Path::new(&file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
            let (state, colour) = if let Some((_, _, size)) = self.brought.as_ref().filter(|b| b.0 == file) {
                let size = size.map(|n| n.to_string());
                (i18n.tf("scene.build.size", &[("w", &size[0]), ("h", &size[1]), ("l", &size[2])]), theme::text_dim())
            } else if let Some((_, note)) = self.unread.as_ref().filter(|u| u.0 == file) {
                (note_text(i18n, note), theme::warn())
            } else {
                (i18n.t("scene.build.reading").to_string(), theme::text_dim())
            };
            ui.add(egui::Label::new(RichText::new(name).small()).truncate()).on_hover_text(&file);
            ui.add(egui::Label::new(RichText::new(state).small().color(colour)).truncate());
        }
        ui.horizontal(|ui| {
            let name = ui.label(RichText::new(i18n.t("scene.build.waterline")).color(theme::text_dim()));
            side_help(&name, i18n, "scene.build.waterline", help, forced);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let width = ui.available_width().min(190.0);
                widgets::slider(ui, &mut self.scene.build.waterline, 0.0, 1.0, 2, accent, width);
            });
        });
        ui.horizontal(|ui| {
            let name = ui.label(RichText::new(i18n.t("scene.build.offset")).color(theme::text_dim()));
            side_help(&name, i18n, "scene.build.offset", help, forced);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let [east, south] = &mut self.scene.build.offset;
                ui.add(egui::DragValue::new(south).speed(1.0)).on_hover_text(i18n.t("scene.build.south"));
                ui.add(egui::DragValue::new(east).speed(1.0)).on_hover_text(i18n.t("scene.build.east"));
            });
        });
    }

    fn save_scene(&mut self) {
        let path = self.dir.join(SCENE_FILE);
        if let Err(e) = std::fs::write(&path, &self.scene_text) {
            log::warn!("could not write {}: {e}", path.display());
        }
        self.scene_dirty = false;
    }

    /// The card that chooses the land and places rivers and waterfalls.
    /// Nothing is explained on the card itself: every part of it says what
    /// it is for when the pointer rests on it, through `help` for the named
    /// parts (shown beside the panel) and through tooltips for the buttons.
    /// `forced` names a part whose help is shown regardless, for pictures.
    pub fn scene_card(
        &mut self,
        ui: &mut egui::Ui,
        values: &ParamValues,
        i18n: &I18n,
        accent: Color32,
        help: &mut Option<HoverHelp>,
        forced: Option<&str>,
    ) {
        let plot = OceanSettings::from_params(values);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let title = ui.label(RichText::new(i18n.t("scene.title")).font(theme::semibold(13.0)).color(theme::text_dim()));
            side_help(&title, i18n, "scene.title", help, forced);
        });
        let before = self.scene.clone();
        theme::card().inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            let lands: Vec<(Land, String)> = Land::ALL.iter().map(|k| (*k, i18n.t(&format!("scene.land.{}", k.key())).to_string())).collect();
            let tips: Vec<String> = Land::ALL.iter().map(|k| i18n.t(&format!("scene.land.{}.help", k.key())).to_string()).collect();
            let mut land = self.scene.land;
            if widgets::segmented(ui, "scene_land", &mut land, &lands, None, 0.0, &tips) {
                // Each land comes with the running water it is first shown
                // with. A build that was chosen is kept for coming back to.
                let build = std::mem::take(&mut self.scene.build);
                let image = std::mem::take(&mut self.scene.pattern_image);
                let zones = std::mem::take(&mut self.scene.zones);
                self.scene = SceneSetup::of(land, self.scene.seed);
                self.scene.build = build;
                self.scene.pattern_image = image;
                self.scene.zones = zones;
                self.picked = 0;
                (self.map, self.mapping) = (None, None);
            }
            if plot.pattern.source == of_ocean::pattern::Source::Image {
                self.pattern_rows(ui, i18n, help, forced);
            }
            let open = self.scene.land == Land::Open;
            let own = self.scene.land == Land::Build;
            if own {
                self.build_rows(ui, i18n, accent, help, forced);
            }
            if let Some(map) = self.scene_map(ui, plot.width, plot.length, plot.waves.direction, accent) {
                side_help(&map, i18n, if own { "scene.build.map" } else if open { "scene.open.map" } else { "scene.map" }, help, forced);
            }
            // What the colours along the shore stand for.
            if let Some(seen) = self.map.as_ref().map(|m| m.seen) {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(12.0, 4.0);
                    let font = egui::TextStyle::Small.resolve(ui.style());
                    for (n, (key, colour)) in SHORE_KEY.iter().enumerate() {
                        if seen[n] {
                            // A mark and its name are one piece, so a name is
                            // never parted from its mark at the end of a row
                            // and the row never runs past the card.
                            let name = ui.painter().layout_no_wrap(i18n.t(key).to_string(), font.clone(), theme::text_dim());
                            let size = egui::vec2(15.0 + name.size().x, name.size().y.max(10.0));
                            let (place, entry) = ui.allocate_exact_size(size, Sense::hover());
                            let dot = Rect::from_center_size(egui::pos2(place.left() + 5.0, place.center().y), egui::vec2(10.0, 10.0));
                            ui.painter().rect(dot, 3, *colour, Stroke::new(1.0, Color32::from_black_alpha(90)), egui::StrokeKind::Inside);
                            ui.painter().galley(egui::pos2(place.left() + 15.0, place.center().y - name.size().y / 2.0), name, theme::text_dim());
                            entry.on_hover_text(i18n.t(&format!("{key}.help")));
                        }
                    }
                });
            }
            // The open sea has no land for rivers; a build has no shape to choose.
            if open {
                self.zone_rows(ui, i18n, accent, help, forced);
                return;
            }

            let mut remove = None;
            for i in 0..self.scene.streams.len() {
                ui.horizontal(|ui| {
                    let name = i18n.tf("scene.river", &[("n", &(i + 1).to_string())]);
                    if ui.selectable_label(self.picked == i, name).on_hover_text(i18n.t("scene.river.help")).clicked() {
                        self.picked = i;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(i18n.t("scene.remove")).on_hover_text(i18n.t("scene.remove.help")).clicked() {
                            remove = Some(i);
                        }
                    });
                });
                // How it ends, as read from the land it runs over.
                let end = self.map.as_ref().and_then(|m| m.ends.get(i).copied());
                if let Some(end) = end {
                    let key = match end {
                        RiverEnd::Dry => "scene.end.dry",
                        RiverEnd::Inland => "scene.end.inland",
                        RiverEnd::Mouth(ShoreKind::Beach) => "scene.end.beach",
                        RiverEnd::Mouth(ShoreKind::Rocky) => "scene.end.rocky",
                        RiverEnd::Mouth(ShoreKind::Cliff) => "scene.end.cliff",
                        RiverEnd::Delta(_) => "scene.end.delta",
                        RiverEnd::Fall => "scene.end.fall",
                    };
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(RichText::new(i18n.t(key)).small().color(theme::text_dim()));
                    });
                }
                if self.picked == i {
                    let stream = &mut self.scene.streams[i];
                    let wanted = stream.delta > 0.0 && !own;
                    // A river lays no delta over a build.
                    let sliders = [
                        ("scene.width", &mut stream.width, 2.0, 24.0, 0),
                        ("scene.winding", &mut stream.winding, 0.0, 1.0, 2),
                        ("scene.delta", &mut stream.delta, 0.0, 1.0, 2),
                    ];
                    for (key, value, min, max, decimals) in sliders.into_iter().take(if own { 2 } else { 3 }) {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            let name = ui.label(RichText::new(i18n.t(key)).color(theme::text_dim()));
                            side_help(&name, i18n, key, help, forced);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let width = ui.available_width().min(190.0);
                                widgets::slider(ui, value, min, max, decimals, accent, width);
                            });
                        });
                    }
                    // A delta was asked for where the shore has no room for one.
                    if wanted && end.is_some_and(|end| !matches!(end, RiverEnd::Delta(_) | RiverEnd::Dry | RiverEnd::Inland)) {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(RichText::new(i18n.t("scene.delta.none")).small().color(theme::warn()))
                                .on_hover_text(i18n.t("scene.delta.help"));
                        });
                    }
                }
            }
            if let Some(i) = remove {
                self.scene.streams.remove(i);
                self.picked = self.picked.min(self.scene.streams.len().saturating_sub(1));
            }
            ui.horizontal(|ui| {
                let add = ui.add_enabled(self.scene.streams.len() < 12, egui::Button::new(i18n.t("scene.add_river")));
                let add = add.on_hover_text(i18n.t("scene.add_river.help")).on_disabled_hover_text(i18n.t("scene.add_river.full"));
                if add.clicked() {
                    self.scene.streams.push(Stream::default());
                    self.picked = self.scene.streams.len() - 1;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if own {
                        return;
                    }
                    ui.add(egui::DragValue::new(&mut self.scene.seed).speed(0.2));
                    let name = ui.label(RichText::new(i18n.t("scene.seed")).color(theme::text_dim()));
                    side_help(&name, i18n, "scene.seed", help, forced);
                });
            });
            self.pond_rows(ui, i18n, accent, help, forced);
            self.zone_rows(ui, i18n, accent, help, forced);
        });
        if self.scene != before {
            self.scene_text = self.scene.to_text();
            self.scene_dirty = true;
        }
        // Kept once the pointer is let go, not at every step of a drag.
        if self.scene_dirty && !ui.input(|i| i.pointer.any_down()) {
            self.save_scene();
        }
    }

    /// The ponds on the scene card: one row each, with how it came out,
    /// and the depth of the one picked.
    fn pond_rows(&mut self, ui: &mut egui::Ui, i18n: &I18n, accent: Color32, help: &mut Option<HoverHelp>, forced: Option<&str>) {
        let mut remove = None;
        for i in 0..self.scene.ponds.len() {
            ui.horizontal(|ui| {
                let name = i18n.tf("scene.pond", &[("n", &(i + 1).to_string())]);
                if ui.selectable_label(self.pond_picked == i, name).on_hover_text(i18n.t("scene.pond.help")).clicked() {
                    self.pond_picked = i;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(i18n.t("scene.remove")).on_hover_text(i18n.t("scene.remove_pond.help")).clicked() {
                        remove = Some(i);
                    }
                });
            });
            if let Some(end) = self.map.as_ref().and_then(|m| m.ponds.get(i).copied()) {
                let asked = (self.scene.ponds[i].depth.round() as i32).clamp(1, 64);
                let (text, short) = match end {
                    PondEnd::Dry => (i18n.t("scene.pond.dry").to_string(), true),
                    PondEnd::Spills { depth: 0, .. } => (i18n.t("scene.pond.none").to_string(), true),
                    PondEnd::Held { depth, size } => (i18n.tf("scene.pond.held", &[("depth", &depth.to_string()), ("size", &size.to_string())]), false),
                    PondEnd::Spills { depth, size, to } => {
                        let key = match to {
                            Spill::Sea => "scene.pond.spills.sea",
                            Spill::Edge => "scene.pond.spills.edge",
                            Spill::River => "scene.pond.spills.river",
                        };
                        (i18n.tf(key, &[("depth", &depth.to_string()), ("asked", &asked.to_string()), ("size", &size.to_string())]), true)
                    }
                };
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let colour = if short { theme::warn() } else { theme::text_dim() };
                    let line = ui.label(RichText::new(text).small().color(colour));
                    if short {
                        line.on_hover_text(i18n.t("scene.pond.spills.help"));
                    }
                });
            }
            if self.pond_picked == i {
                let pond = &mut self.scene.ponds[i];
                for (key, value, min, max) in [("scene.pond.depth", &mut pond.depth, 1.0, 24.0), ("scene.pond.dig", &mut pond.dig, 0.0, 32.0)] {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let name = ui.label(RichText::new(i18n.t(key)).color(theme::text_dim()));
                        side_help(&name, i18n, key, help, forced);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let width = ui.available_width().min(190.0);
                            widgets::slider(ui, value, min, max, 0, accent, width);
                        });
                    });
                }
            }
        }
        if let Some(i) = remove {
            self.scene.ponds.remove(i);
            self.pond_picked = self.pond_picked.min(self.scene.ponds.len().saturating_sub(1));
        }
        let add = ui.add_enabled(self.scene.ponds.len() < 12, egui::Button::new(i18n.t("scene.add_pond")));
        let add = add.on_hover_text(i18n.t("scene.add_pond.help")).on_disabled_hover_text(i18n.t("scene.add_pond.full"));
        if add.clicked() {
            self.scene.ponds.push(Pond::default());
            self.pond_picked = self.scene.ponds.len() - 1;
        }
    }

    /// The zones on the scene card: one row each, with the size of the
    /// one picked, and buttons to add either kind.
    fn zone_rows(&mut self, ui: &mut egui::Ui, i18n: &I18n, accent: Color32, help: &mut Option<HoverHelp>, forced: Option<&str>) {
        let mut remove = None;
        for i in 0..self.scene.zones.len() {
            let kind = self.scene.zones[i].kind;
            let key = match kind {
                ZoneKind::Calm => "scene.zone.calm",
                ZoneKind::Dry => "scene.zone.dry",
            };
            ui.horizontal(|ui| {
                let name = i18n.tf(key, &[("n", &(i + 1).to_string())]);
                if ui.selectable_label(self.zone_picked == i, name).on_hover_text(i18n.t(&format!("{key}.help"))).clicked() {
                    self.zone_picked = i;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(i18n.t("scene.remove")).on_hover_text(i18n.t("scene.remove_zone.help")).clicked() {
                        remove = Some(i);
                    }
                });
            });
            if self.zone_picked == i {
                let zone = &mut self.scene.zones[i];
                let mut rows = vec![("scene.zone.radius", &mut zone.radius, 2.0, 128.0, 0)];
                if kind == ZoneKind::Calm {
                    rows.push(("scene.zone.strength", &mut zone.strength, 0.0, 1.0, 2));
                }
                for (key, value, min, max, decimals) in rows {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let name = ui.label(RichText::new(i18n.t(key)).color(theme::text_dim()));
                        side_help(&name, i18n, key, help, forced);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let width = ui.available_width().min(190.0);
                            widgets::slider(ui, value, min, max, decimals, accent, width);
                        });
                    });
                }
            }
        }
        if let Some(i) = remove {
            self.scene.zones.remove(i);
            self.zone_picked = self.zone_picked.min(self.scene.zones.len().saturating_sub(1));
        }
        ui.horizontal_wrapped(|ui| {
            let room = self.scene.zones.len() < 12;
            for (kind, key) in [(ZoneKind::Calm, "scene.add_calm"), (ZoneKind::Dry, "scene.add_dry")] {
                let add = ui.add_enabled(room, egui::Button::new(i18n.t(key)));
                let add = add.on_hover_text(i18n.t(&format!("{key}.help"))).on_disabled_hover_text(i18n.t("scene.add_zone.full"));
                if add.clicked() {
                    self.scene.zones.push(Zone { kind, ..Zone::default() });
                    self.zone_picked = self.scene.zones.len() - 1;
                }
            }
        });
    }

    /// For automated runs: add a calm-water or keep-dry area.
    pub fn add_zone(&mut self, zone: Zone) {
        self.scene.zones.push(zone);
        self.scene_text = self.scene.to_text();
        self.save_scene();
    }

    /// For automated runs: add a pond to the land.
    pub fn add_pond(&mut self, pond: Pond) {
        self.scene.ponds.push(pond);
        self.scene_text = self.scene.to_text();
        self.save_scene();
    }

    /// A small map of the land, north up, with the rivers drawn over it.
    /// Each river has a dot at either end that can be dragged. Gives back
    /// the place it was drawn in, for the help shown about it.
    fn scene_map(&mut self, ui: &mut egui::Ui, width: i32, length: i32, direction: f64, accent: Color32) -> Option<egui::Response> {
        // The picture is of the land with its rivers laid, and of how its
        // shore reads. It is worked out again when something has changed
        // and the pointer has been let go, not at every step of a drag.
        // The zones are drawn over the picture, so moving one does not
        // call for a new one.
        let mut bare = self.scene.clone();
        bare.zones.clear();
        let key = (bare.to_text(), width, length, (direction * 10.0).round() as i32);
        let stale = self.map.as_ref().map(|m| &m.key) != Some(&key);
        if stale && (self.map.is_none() || !ui.input(|i| i.pointer.any_down())) {
            if self.scene.land == Land::Build {
                // Reading the ground from a large build takes a moment, so
                // its map is drawn in the background.
                let under_way = self.mapping.as_ref().map(|m| &m.0) == Some(&key) || self.unmapped.as_ref() == Some(&key);
                if let (Some(build), false) = (self.build_for_scene(), under_way) {
                    let (scene, ctx) = (self.scene.clone(), ui.ctx().clone());
                    let work = move |_: &JobCtx| Ok(read_land(&scene, Some(&build), width, length, direction));
                    self.mapping = Some((key, spawn("job.map", move || ctx.request_repaint(), work)));
                }
            } else {
                let read = match read_land(&self.scene, None, width, length, direction) {
                    Some(read) => read,
                    // The open sea is all water.
                    None if self.scene.land == Land::Open => {
                        LandRead { dots: vec![Color32::from_rgb(70, 136, 200)], size: [1, 1], ends: Vec::new(), ponds: Vec::new(), seen: [false; 5] }
                    }
                    None => return None,
                };
                self.map = Some(SceneReading::new(ui.ctx(), key, read));
            }
        }
        if let Some(outcome) = self.mapping.as_ref().and_then(|m| m.1.poll()) {
            let (key, _) = self.mapping.take()?;
            match outcome {
                Outcome::Done(Some(read)) => self.map = Some(SceneReading::new(ui.ctx(), key, read)),
                _ => self.unmapped = Some(key),
            }
        }
        let texture = self.map.as_ref().map(|m| m.texture.clone())?;

        let across = ui.available_width();
        let tall = (across * length as f32 / width as f32).min(280.0);
        let size = egui::vec2(tall * width as f32 / length as f32, tall);
        let (space, shown) = ui.allocate_exact_size(egui::vec2(across, tall), Sense::hover());
        let rect = Rect::from_center_size(space.center(), size);
        egui::Image::new((texture.id(), size)).corner_radius(6).paint_at(ui, rect);

        let painter = ui.painter().with_clip_rect(rect.expand(8.0));
        let place = |x: f64, z: f64| rect.min + egui::vec2((x / width as f64) as f32 * rect.width(), (z / length as f64) as f32 * rect.height());
        // The zones first, so the rivers and ponds are on top of them.
        for i in 0..self.scene.zones.len() {
            let zone = &mut self.scene.zones[i];
            let centre = place(zone.at[0] * width as f64, zone.at[1] * length as f64);
            let reach = (zone.radius as f32 * rect.width() / width as f32).max(3.0);
            let (fill, edge) = match zone.kind {
                ZoneKind::Calm => (Color32::from_rgba_unmultiplied(170, 235, 255, 45), Color32::from_rgb(170, 235, 255)),
                ZoneKind::Dry => (Color32::from_rgba_unmultiplied(250, 196, 80, 70), Color32::from_rgb(250, 196, 80)),
            };
            let picked = self.zone_picked == i;
            painter.circle(centre, reach, fill, Stroke::new(if picked { 2.5 } else { 1.5 }, edge));
            let grip = Rect::from_center_size(centre, egui::vec2(20.0, 20.0));
            let response = ui.interact(grip, ui.make_persistent_id(("zone", i)), Sense::drag());
            if response.dragged() {
                if let Some(p) = response.interact_pointer_pos() {
                    zone.at = [
                        ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64,
                        ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0) as f64,
                    ];
                }
                self.zone_picked = i;
            }
            let radius = if response.hovered() || response.dragged() { 6.0 } else { 4.5 };
            let ring = if picked { Color32::WHITE } else { Color32::from_black_alpha(150) };
            painter.circle(centre, radius + 1.5, ring, Stroke::NONE);
            painter.circle(centre, radius, edge, Stroke::new(1.0, Color32::from_black_alpha(150)));
        }
        for i in 0..self.scene.streams.len() {
            let course = self.scene.course(i, width, length);
            let line: Vec<egui::Pos2> = course.iter().step_by(4).chain(course.last()).map(|p| place(p.0, p.1)).collect();
            let thick = (self.scene.streams[i].width as f32 * rect.width() / width as f32).clamp(1.5, 9.0);
            painter.add(egui::Shape::line(line.clone(), Stroke::new(thick + 2.0, Color32::from_black_alpha(110))));
            painter.add(egui::Shape::line(line, Stroke::new(thick, Color32::from_rgb(140, 214, 255))));
            for end in 0..2 {
                let stream = &mut self.scene.streams[i];
                let at = if end == 0 { &mut stream.from } else { &mut stream.to };
                let centre = place(at[0] * width as f64, at[1] * length as f64);
                let grip = Rect::from_center_size(centre, egui::vec2(20.0, 20.0));
                let response = ui.interact(grip, ui.make_persistent_id(("river", i, end)), Sense::drag());
                if response.dragged() {
                    if let Some(p) = response.interact_pointer_pos() {
                        *at = [
                            ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64,
                            ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0) as f64,
                        ];
                    }
                    self.picked = i;
                }
                let radius = if response.hovered() || response.dragged() { 6.5 } else { 5.0 };
                let ring = if self.picked == i { Color32::WHITE } else { Color32::from_black_alpha(150) };
                painter.circle(centre, radius + 1.5, ring, Stroke::NONE);
                painter.circle(centre, radius, if end == 0 { Color32::from_rgb(245, 247, 250) } else { accent }, Stroke::new(1.0, Color32::from_black_alpha(150)));
            }
        }
        for i in 0..self.scene.ponds.len() {
            let at = &mut self.scene.ponds[i].at;
            let centre = place(at[0] * width as f64, at[1] * length as f64);
            let grip = Rect::from_center_size(centre, egui::vec2(20.0, 20.0));
            let response = ui.interact(grip, ui.make_persistent_id(("pond", i)), Sense::drag());
            if response.dragged() {
                if let Some(p) = response.interact_pointer_pos() {
                    *at = [
                        ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64,
                        ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0) as f64,
                    ];
                }
                self.pond_picked = i;
            }
            let half = if response.hovered() || response.dragged() { 6.5 } else { 5.0 };
            let ring = if self.pond_picked == i { Color32::WHITE } else { Color32::from_black_alpha(150) };
            painter.rect_filled(Rect::from_center_size(centre, egui::vec2(2.0 * half + 3.0, 2.0 * half + 3.0)), 2, ring);
            painter.rect(Rect::from_center_size(centre, egui::vec2(2.0 * half, 2.0 * half)), 2, Color32::from_rgb(245, 247, 250), Stroke::new(2.5, Color32::from_rgb(40, 90, 190)), egui::StrokeKind::Inside);
        }
        Some(shown)
    }

    // ---------------------------------------------------------------- tuning

    /// The card in the advanced panel that leads to the tuning file. What
    /// the file is for is said when the pointer rests on the card's title.
    pub fn tuning_card(&mut self, ui: &mut egui::Ui, i18n: &I18n, help: &mut Option<HoverHelp>, forced: Option<&str>) {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let title = ui.label(RichText::new(i18n.t("tuning.title")).font(theme::semibold(13.0)).color(theme::text_dim()));
            side_help(&title, i18n, "tuning.title", help, forced);
        });
        let file = self.dir.join(TUNING_FILE);
        theme::card().inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            if self.tuning_replaced {
                ui.label(RichText::new(i18n.t("tuning.updated")).small().color(theme::warn()))
                    .on_hover_text(i18n.tf("tuning.updated.help", &[("file", TUNING_BACKUP)]));
            }
            ui.horizontal_wrapped(|ui| {
                let place = file.display().to_string();
                if ui.button(i18n.t("tuning.open")).on_hover_text(i18n.tf("tuning.open.help", &[("file", &place)])).clicked() {
                    // Make sure it exists, then hand it to Notepad.
                    read_tuning(&self.dir);
                    launch("notepad.exe", &[file.as_os_str()]);
                }
                if ui.button(i18n.t("tuning.folder")).on_hover_text(i18n.tf("tuning.folder.help", &[("file", &place)])).clicked() {
                    read_tuning(&self.dir);
                    let mut select = std::ffi::OsString::from("/select,");
                    select.push(file.as_os_str());
                    launch("explorer.exe", &[select.as_os_str()]);
                }
                if ui.button(i18n.t("tuning.reset")).on_hover_text(i18n.tf("tuning.reset.help", &[("file", TUNING_BACKUP)])).clicked() {
                    // The numbers being replaced are kept beside the file.
                    if let Ok(old) = std::fs::read_to_string(&file) {
                        if old != Tuning::default().to_text() {
                            let _ = std::fs::write(self.dir.join(TUNING_BACKUP), old);
                        }
                    }
                    if let Err(e) = std::fs::write(&file, Tuning::default().to_text()) {
                        log::warn!("could not write {}: {e}", file.display());
                    }
                    self.tuning_read = None;
                }
            });
        });
    }
}

fn launch(program: &str, args: &[&std::ffi::OsStr]) {
    if let Err(e) = std::process::Command::new(program).args(args).spawn() {
        log::warn!("could not start {program}: {e}");
    }
}

/// A title and a line or two of text in the middle of the viewport.
fn message(ui: &mut egui::Ui, rect: Rect, title: &str, body: &str) {
    let centre = rect.center();
    ui.painter().text(centre - egui::vec2(0.0, 34.0), Align2::CENTER_CENTER, title, theme::display(26.0), theme::text());
    if body.is_empty() {
        return;
    }
    let width = (rect.width() - 120.0).clamp(200.0, 520.0);
    let place = Rect::from_center_size(centre + egui::vec2(0.0, 30.0), egui::vec2(width, 90.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(place), |ui| {
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(body).color(theme::text_dim()));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use of_core::i18n::Lang;
    use of_core::params::{self, Value};

    fn wait(tool: &mut OceanTool, settings: &mut Settings) -> Event {
        let started = Instant::now();
        loop {
            if let Some(event) = tool.poll(settings) {
                return event;
            }
            assert!(started.elapsed() < Duration::from_secs(120), "the bake did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Re-read the tuning file now, then run one frame of the tool.
    fn frame(tool: &mut OceanTool, ctx: &egui::Context, settings: &Settings, i18n: &I18n) {
        tool.tuning_read = None;
        tool.tick(ctx, settings, i18n);
    }

    #[test]
    fn tuning_file_drives_the_bake_and_a_broken_one_keeps_the_build() {
        let dir = std::env::temp_dir().join(format!("of-ocean-tool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(TUNING_FILE);
        let _ = std::fs::remove_file(&file);

        let ctx = egui::Context::default();
        let i18n = I18n::new(Lang::from_code("en").unwrap());
        let mut settings = Settings::default();
        settings.mode = Mode::Ocean;
        settings.auto_bake = true;
        for id in ["ocean.width", "ocean.length"] {
            settings.values.set(params::find(id).unwrap(), Value::Int(48));
        }

        // First frame: the defaults are written out and a bake starts by itself.
        let mut tool = OceanTool::new(&ctx, &dir, Mode::Ocean);
        assert!(file.exists() && dir.join(TUNING_NOTES_FILE).exists());
        frame(&mut tool, &ctx, &settings, &i18n);
        assert!(tool.baking());
        assert!(matches!(wait(&mut tool, &mut settings), Event::Baked));
        frame(&mut tool, &ctx, &settings, &i18n);
        assert_eq!(tool.status(), Status::Current);
        assert!(!tool.baking(), "a current build must not be baked again");
        let first = tool.current.as_ref().unwrap().baked.clone();

        // A broken file: the bake fails with the place of the mistake, the
        // build stays, and the same broken file is not tried again.
        let good = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, good.replacen("\"waves\": {", "\"waves\": {,", 1)).unwrap();
        frame(&mut tool, &ctx, &settings, &i18n);
        assert_eq!(tool.status(), Status::StaleTuning);
        let Event::Failed(error) = wait(&mut tool, &mut settings) else { panic!("a broken file must fail") };
        assert!(error.contains("line"), "{error}");
        assert!(Arc::ptr_eq(&first, &tool.current.as_ref().unwrap().baked));
        frame(&mut tool, &ctx, &settings, &i18n);
        assert!(!tool.baking());
        assert!(tool.error.is_some());

        // A changed number: bakes again and gives a different sea.
        let mut tuning = Tuning::default();
        tuning.waves.height_in_deviations *= 0.6;
        tuning.waves.peak_enhancement = 1.0;
        std::fs::write(&file, tuning.to_text()).unwrap();
        frame(&mut tool, &ctx, &settings, &i18n);
        assert!(tool.baking());
        assert!(matches!(wait(&mut tool, &mut settings), Event::Baked));
        frame(&mut tool, &ctx, &settings, &i18n);
        assert_eq!(tool.status(), Status::Current);
        assert!(tool.error.is_none());
        let second = tool.current.as_ref().unwrap().baked.clone();
        assert_ne!(first.water_top, second.water_top);

        // Choosing a land makes the build out of date; the next bake has
        // ground in it, and the choice is kept for the next run. (The cliff
        // coast is a development-build land.)
        #[cfg(feature = "dev")]
        {
            tool.set_land(Land::Cliffs);
            frame(&mut tool, &ctx, &settings, &i18n);
            assert!(tool.baking());
            assert!(matches!(wait(&mut tool, &mut settings), Event::Baked));
            frame(&mut tool, &ctx, &settings, &i18n);
            assert_eq!(tool.status(), Status::Current);
            let third = tool.current.as_ref().unwrap().baked.clone();
            assert!(third.stats.blocks[0] > 0, "the land is missing from the build");
            assert_eq!(read_scene(&dir), tool.scene);
            tool.scene.streams.clear();
            tool.scene_text = tool.scene.to_text();
            frame(&mut tool, &ctx, &settings, &i18n);
            assert!(tool.baking(), "removing a river must bake again");
            assert!(matches!(wait(&mut tool, &mut settings), Event::Baked));
        }

        // A tuning file from an earlier method is set aside and replaced.
        std::fs::write(&file, "{ \"waves\": { \"trains\": [] } }").unwrap();
        frame(&mut tool, &ctx, &settings, &i18n);
        assert!(tool.tuning_replaced);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), Tuning::default().to_text());
        assert!(std::fs::read_to_string(dir.join(TUNING_BACKUP)).unwrap().contains("trains"));
        if tool.baking() {
            wait(&mut tool, &mut settings);
        }

        // With "Live" off nothing starts by itself.
        settings.auto_bake = false;
        let mut tuning = Tuning::default();
        tuning.whitecaps.trail_time *= 2.0;
        std::fs::write(&file, tuning.to_text()).unwrap();
        frame(&mut tool, &ctx, &settings, &i18n);
        assert_eq!(tool.status(), Status::StaleTuning);
        assert!(!tool.baking());

        tool.cancel_all();
        drop(tool);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The messages for a bake stopped by the memory guard, and for one refused
/// by its estimate (`{size}` is filled in by the job).
fn memory_texts(i18n: &I18n) -> (String, String) {
    let limit = of_core::memory::mb(of_core::memory::limit());
    (i18n.tf("file.err.memory", &[("limit", &limit)]), i18n.tf("file.err.memory_estimate", &[("limit", &limit), ("size", "{size}")]))
}

/// A bake's error as the job reports it.
fn bake_failed(e: String, over: &str) -> JobError {
    if e == of_formats::CANCELLED {
        JobError::Cancelled
    } else if e == of_formats::MEMORY {
        JobError::Failed(over.to_string())
    } else {
        JobError::Failed(e)
    }
}
