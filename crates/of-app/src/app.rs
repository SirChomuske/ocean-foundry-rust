//! The main window.

use crate::files::FilesTool;
use crate::ocean::{self, OceanTool, Status};
use crate::preview::ListAction;
use crate::theme;
use crate::widgets;
use crate::Launch;
use eframe::egui::{self, Align, Align2, Color32, Layout, RichText, Sense};
use of_core::i18n::{I18n, Lang};
use of_core::jobs::{coarse_bytes, coarse_duration, spawn, JobHandle, LastGood, Outcome, Snapshot};
use of_core::params::{self, Kind, Mode, ParamDef, Value, SEED_MAX};
use of_core::paths::DataDir;
use of_core::project::{self, Project};
use of_core::settings::{self, Appearance, Autosaver, ComputeChoice, LoadNote, Settings};
use of_gpu::{BenchResult, CpuReason, Probe, Resolved};
use std::time::Duration;

/// Which tool belongs to a mode: one for each of `Mode::ALL`. Lava is on
/// hold and only the development build has it; the package is the sea alone.
fn slot(mode: Mode) -> usize {
    match mode {
        Mode::Ocean => 0,
        #[cfg(feature = "dev")]
        Mode::Lava => 1,
    }
}

const PANEL_WIDTH: f32 = 400.0;
const LABEL_WIDTH: f32 = 146.0;
const ROW_HEIGHT: f32 = 38.0;
const VALUE_WIDTH: f32 = 58.0;
const HELP_WIDTH: f32 = 300.0;

/// What the pointer is resting on, for the hover-help card.
pub(crate) struct HoverHelp {
    pub title: String,
    pub body: String,
    /// Screen position of the hovered label, so the card lines up with it.
    pub anchor_y: f32,
}

pub struct OceanFoundryApp {
    data: DataDir,
    settings: Settings,
    autosave: Autosaver,
    i18n: I18n,
    launch: Launch,
    /// What the style was last set for; it is set again when any part changes.
    themed_for: Option<(Mode, Appearance, egui::Theme)>,

    probe_job: Option<JobHandle<Probe>>,
    probe: LastGood<Probe>,
    bench_job: Option<JobHandle<BenchResult>>,
    bench: LastGood<BenchResult>,
    /// Message from the most recently cancelled or failed job.
    job_note: Option<String>,

    notices: Vec<String>,
    devices_open: bool,
    files: FilesTool,
    /// One tool for each mode: the sea and lava.
    tools: Vec<OceanTool>,
    hover: Option<HoverHelp>,
    /// Right edge of the control panel, where the help card attaches.
    panel_right: f32,
    frames: u32,
    screenshot_requested: bool,
    /// For automated runs: an export asked for on the command line is
    /// being written.
    exporting: bool,
    /// The bake's status, and the estimate, when the top bar has no room
    /// for them; the status bar shows them instead.
    status_moved: Option<(Color32, String, Option<String>)>,
    /// The name typed for a new profile.
    profile_name: String,
}

/// What was picked in the project menu.
enum ProjectAct {
    Open,
    Save,
    Load(std::path::PathBuf),
    Delete(std::path::PathBuf),
    Keep,
    Restore(usize),
}

fn random_seed() -> i64 {
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded from the operating system on each call.
    let h = std::collections::hash_map::RandomState::new().build_hasher().finish();
    (h % (SEED_MAX as u64 + 1)) as i64
}

impl OceanFoundryApp {
    pub fn new(cc: &eframe::CreationContext<'_>, data: DataDir, launch: Launch) -> Self {
        let (mut settings, note) = settings::load(&data.path);
        if let Some(lang) = launch.lang {
            settings.language = lang;
        }
        if let Some(mode) = launch.mode {
            settings.mode = mode;
        }
        if let Some(appearance) = launch.appearance {
            settings.appearance = appearance;
        }
        let i18n = I18n::new(settings.language);
        let mut notices = Vec::new();
        let file_name = |p: &std::path::Path| p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        match &note {
            LoadNote::RecoveredFrom(p) => notices.push(i18n.tf("notice.settings_recovered", &[("file", &file_name(p))])),
            LoadNote::NewerSchema(p) => notices.push(i18n.tf("notice.settings_newer", &[("file", &file_name(p))])),
            LoadNote::Fresh | LoadNote::Loaded => {}
        }

        theme::install_fonts(&cc.egui_ctx);

        let mut app = Self {
            autosave: Autosaver::new(data.path.clone()),
            tools: Mode::ALL.iter().map(|&m| OceanTool::new(&cc.egui_ctx, &data.path, m)).collect(),
            data,
            settings,
            i18n,
            devices_open: launch.show_devices,
            files: FilesTool::new(),
            launch,
            themed_for: None,
            probe_job: None,
            probe: LastGood::default(),
            bench_job: None,
            bench: LastGood::default(),
            job_note: None,
            notices,
            hover: None,
            panel_right: PANEL_WIDTH,
            frames: 0,
            screenshot_requested: false,
            exporting: false,
            status_moved: None,
            profile_name: String::new(),
        };
        // For automated runs: open a project or profile, go back to a past
        // bake, then apply single settings on top.
        if let Some(path) = app.launch.open_project.clone() {
            app.load_project(&path);
        }
        if let Some(name) = app.launch.load_profile.clone() {
            let path = project::profile_path(&app.data.path, &name);
            if path.is_file() {
                app.load_project(&path);
            } else {
                let text = app.i18n.tf("project.profile.missing", &[("name", &name)]);
                app.notice(text);
            }
        }
        if let Some(i) = app.launch.restore_history {
            app.project_act(ProjectAct::Restore(i));
        }
        for (def, value) in app.launch.set.clone() {
            app.settings.values.set(def, value);
            app.changed();
        }
        if app.launch.restore_blocks {
            app.settings.excluded.clear();
            app.changed();
        }
        for name in app.launch.exclude.clone() {
            if !app.settings.excluded.contains(&name) {
                app.settings.excluded.push(name);
                app.changed();
            }
        }
        for tool in &mut app.tools {
            tool.excluded = app.settings.excluded.clone();
        }
        if app.launch.cpu {
            app.settings.compute = ComputeChoice::Cpu;
        }
        if app.launch.cpu || app.launch.lang.is_some() || app.launch.mode.is_some() || app.launch.appearance.is_some() {
            app.changed();
        }
        if let Some(path) = app.launch.files.clone() {
            app.files.preset(&cc.egui_ctx, &path, app.launch.files_format, app.launch.files_omit_air);
        }
        if let Some(show) = app.launch.show.as_deref().and_then(|name| crate::preview::Show::ALL.into_iter().find(|s| s.key().ends_with(name))) {
            app.tools[slot(app.settings.mode)].set_show(show);
        }
        if app.launch.layers.is_some() || app.launch.cut_open.is_some() {
            app.tools[slot(app.settings.mode)].set_view(app.launch.layers, app.launch.cut_open);
        }
        if let Some(land) = app.launch.land {
            app.tools[slot(app.settings.mode)].set_land(land);
        }
        if let Some(file) = app.launch.build.clone() {
            app.tools[slot(app.settings.mode)].set_build(&file, app.launch.waterline);
        }
        for pond in app.launch.ponds.clone() {
            app.tools[slot(Mode::Ocean)].add_pond(pond);
        }
        for zone in app.launch.zones.clone() {
            app.tools[slot(Mode::Ocean)].add_zone(zone);
        }
        if let Some(file) = app.launch.pattern_image.clone() {
            app.tools[slot(Mode::Ocean)].set_pattern_image(&file);
        }
        if app.launch.bake {
            app.tools[slot(app.settings.mode)].start(&cc.egui_ctx, &app.settings.values, &app.i18n);
        }
        if app.launch.export {
            app.tools[slot(app.settings.mode)].preset_export(app.launch.files_format, app.launch.export_selection, app.launch.files_omit_air);
            if let Some(area) = app.launch.export_area {
                app.tools[slot(app.settings.mode)].preset_area(area);
            }
            app.tools[slot(app.settings.mode)].open_export();
        }
        app.start_probe(&cc.egui_ctx);
        app
    }

    fn start_probe(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.probe_job = Some(spawn("job.selftest", move || ctx.request_repaint(), of_gpu::probe));
    }

    fn start_benchmark(&mut self, ctx: &egui::Context) {
        let Some(probe) = self.probe.get() else { return };
        let gpu = match of_gpu::resolve(&self.settings.compute, probe).device {
            Resolved::Gpu(i) => Some(probe.adapters[i].clone()),
            Resolved::Cpu(_) => None,
        };
        let ctx = ctx.clone();
        self.job_note = None;
        self.bench_job =
            Some(spawn("job.benchmark", move || ctx.request_repaint(), move |job| of_gpu::benchmark(job, gpu.as_ref())));
    }

    fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.i18n.t(key)
    }

    fn changed(&mut self) {
        self.autosave.mark_dirty();
    }

    fn duration_text(&self, d: Duration) -> String {
        let (n, unit, minutes) = coarse_duration(d);
        match minutes {
            Some(m) => format!("{n} {} {m} {}", self.t(unit), self.t("unit.minute")),
            None => format!("{n} {}", self.t(unit)),
        }
    }

    /// Collect finished jobs. A cancelled or failed job leaves the previous
    /// result in place and only leaves a note.
    fn poll_jobs(&mut self) {
        if let Some(outcome) = self.probe_job.as_ref().and_then(JobHandle::poll) {
            self.probe_job = None;
            self.note_outcome(&outcome);
            self.probe.apply(outcome);
        }
        if let Some(outcome) = self.bench_job.as_ref().and_then(JobHandle::poll) {
            self.bench_job = None;
            self.note_outcome(&outcome);
            self.bench.apply(outcome);
        }
        if let Some(error) = self.files.poll() {
            self.job_note = Some(self.i18n.tf("job.failed", &[("error", &error)]));
        }
        match self.tools[slot(self.settings.mode)].poll(&mut self.settings) {
            Some(ocean::Event::Baked) => {
                self.job_note = None;
                self.settings.remember_bake();
                self.changed();
            }
            Some(ocean::Event::Cancelled) => self.job_note = Some(self.t("job.cancelled").to_string()),
            Some(ocean::Event::Failed(e)) => self.job_note = Some(self.tools[slot(self.settings.mode)].failure(&self.i18n, &e)),
            None => {}
        }
    }

    fn bake(&mut self, ctx: &egui::Context) {
        self.job_note = None;
        self.tools[slot(self.settings.mode)].start(ctx, &self.settings.values, &self.i18n);
    }

    /// One line for a past bake: what it was, how large, and how long ago.
    fn past_text(&self, past: &settings::PastBake) -> String {
        let size = |id: &str| past.values.get_id(&format!("{}.{id}", past.mode.key())).and_then(|v| v.as_f64()).unwrap_or(0.0).to_string();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let ago = now.saturating_sub(past.when);
        let age = match ago {
            0..90 => self.t("age.now").to_string(),
            90..5400 => self.i18n.tf("age.minutes", &[("n", &(ago / 60).to_string())]),
            5400..129_600 => self.i18n.tf("age.hours", &[("n", &(ago / 3600).to_string())]),
            _ => self.i18n.tf("age.days", &[("n", &(ago / 86_400).to_string())]),
        };
        let mode = self.t(&format!("mode.{}", past.mode.key())).to_string();
        self.i18n.tf("project.history.item", &[("mode", &mode), ("width", &size("width")), ("length", &size("length")), ("age", &age)])
    }

    /// Everything the current bake is made from, as a project.
    fn current_project(&self) -> Project {
        let ocean = &self.tools[slot(Mode::Ocean)];
        let mut project = Project::new(self.settings.mode, &self.settings.values, &self.settings.excluded, Some(ocean.scene_text().to_string()));
        // The ripples' image travels with the project.
        if !ocean.pattern_image().is_empty() {
            project.pattern_image = project::Carried::read(std::path::Path::new(ocean.pattern_image()));
        }
        project
    }

    fn save_project(&mut self, path: &std::path::Path) {
        if let Err(e) = project::write(path, &self.current_project()) {
            self.notice(self.i18n.tf("project.failed", &[("error", &e)]));
        }
    }

    /// A note at the top of the window that stays until it is dismissed.
    fn notice(&mut self, text: String) {
        if !self.notices.contains(&text) {
            self.notices.push(text);
        }
    }

    fn load_project(&mut self, path: &std::path::Path) {
        let loaded = project::read(path).and_then(|p| {
            if let Some(scene) = &p.scene {
                self.tools[slot(Mode::Ocean)].load_scene(scene)?;
            }
            // An image carried in the project is unpacked when the one it
            // names is not on this computer.
            let ocean = &mut self.tools[slot(Mode::Ocean)];
            if let Some(carried) = &p.pattern_image {
                if !std::path::Path::new(ocean.pattern_image()).is_file() {
                    let placed = carried.place(&self.data.path)?;
                    ocean.set_pattern_image(&placed.to_string_lossy());
                }
            }
            Ok(p)
        });
        match loaded {
            Ok(p) => {
                self.settings.mode = p.mode;
                self.settings.values = p.values;
                self.settings.excluded = p.excluded;
                self.job_note = None;
                self.changed();
            }
            Err(e) => {
                let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notice(self.i18n.tf("project.failed", &[("error", &format!("{file}: {e}"))]));
            }
        }
    }

    fn project_act(&mut self, act: ProjectAct) {
        let dialog = || rfd::FileDialog::new().add_filter(self.t("project.filter"), &[project::EXTENSION]);
        match act {
            ProjectAct::Open => {
                if let Some(path) = dialog().pick_file() {
                    self.load_project(&path);
                }
            }
            ProjectAct::Save => {
                let name = format!("{}.{}", self.settings.mode.key(), project::EXTENSION);
                if let Some(path) = dialog().set_file_name(name).save_file() {
                    self.save_project(&path);
                }
            }
            ProjectAct::Load(path) => self.load_project(&path),
            ProjectAct::Delete(path) => {
                if let Err(e) = std::fs::remove_file(&path) {
                    self.notice(self.i18n.tf("project.failed", &[("error", &e.to_string())]));
                }
            }
            ProjectAct::Keep => {
                let path = project::profile_path(&self.data.path, &self.profile_name);
                self.save_project(&path);
                self.profile_name.clear();
            }
            ProjectAct::Restore(i) => {
                if let Some(past) = self.settings.history.get(i).cloned() {
                    self.settings.mode = past.mode;
                    self.settings.values = past.values;
                    self.changed();
                } else {
                    self.notice(self.i18n.tf("project.history.missing", &[("n", &i.to_string())]));
                }
            }
        }
    }

    fn note_outcome<T>(&mut self, outcome: &Outcome<T>) {
        self.job_note = match outcome {
            Outcome::Done(_) => None,
            Outcome::Cancelled => Some(self.t("job.cancelled").to_string()),
            Outcome::Failed(e) => Some(self.i18n.tf("job.failed", &[("error", e)])),
        };
    }

    fn active_job(&self) -> Option<(&'static str, Snapshot)> {
        if let Some(job) = self.tools[slot(self.settings.mode)].active_job() {
            return Some(job);
        }
        if let Some(job) = self.files.active_job() {
            return Some(job);
        }
        if let Some(j) = &self.bench_job {
            return Some(("job.benchmark", j.snapshot()));
        }
        self.probe_job.as_ref().map(|j| ("job.selftest", j.snapshot()))
    }

    // ---------------------------------------------------------------- top bar

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let mode = self.settings.mode;
        let status = self.tools[slot(self.settings.mode)].status();
        let accent = theme::accent(mode);
        let mut bake = false;
        ui.horizontal_centered(|ui| {
            // The bar's buttons sit on the window frame rather than on a card,
            // so they get a fill of their own: thin tints read as nothing there.
            let fill = theme::bar_control();
            let widgets = &mut ui.visuals_mut().widgets;
            for style in [&mut widgets.inactive, &mut widgets.open, &mut widgets.noninteractive] {
                style.bg_fill = fill;
                style.weak_bg_fill = fill;
            }
            widgets.hovered.bg_fill = theme::bar_control_hover();
            widgets.hovered.weak_bg_fill = theme::bar_control_hover();
            widgets.active.bg_fill = theme::bar_control_active();
            widgets.active.weak_bg_fill = theme::bar_control_active();
            ui.add_space(6.0);
            ui.label(RichText::new(self.t("app.title")).font(theme::semibold(16.0)).color(theme::text_strong()));
            ui.add_space(14.0);

            // The switch between the sea and lava, when both are offered.
            if Mode::ALL.len() > 1 {
                let options: Vec<(Mode, String)> =
                    Mode::ALL.iter().map(|&m| (m, self.t(&format!("mode.{}", m.key())).to_string())).collect();
                let tips: Vec<String> = Mode::ALL.iter().map(|m| self.t(&format!("mode.{}.help", m.key())).to_string()).collect();
                let mut picked = mode;
                if widgets::segmented(ui, "mode", &mut picked, &options, Some(theme::accent(mode)), 84.0, &tips) {
                    self.settings.mode = picked;
                    self.changed();
                }
            }

            // ---- Project files, profiles and the last few bakes.
            ui.add_space(10.0);
            let mut act: Option<ProjectAct> = None;
            let hint = self.t("project.profile.name").to_string();
            let title = self.t("project.menu").to_string();
            let mut typed = std::mem::take(&mut self.profile_name);
            let menu = ui.menu_button(title, |ui| {
                ui.set_min_width(240.0);
                if ui.button(self.t("project.open")).on_hover_text(self.t("project.open.help")).clicked() {
                    act = Some(ProjectAct::Open);
                    ui.close();
                }
                if ui.button(self.t("project.save")).on_hover_text(self.t("project.save.help")).clicked() {
                    act = Some(ProjectAct::Save);
                    ui.close();
                }
                ui.separator();
                ui.label(RichText::new(self.t("project.profiles")).small().color(theme::text_dim())).on_hover_text(self.t("project.profiles.help"));
                for (name, path) in project::profiles(&self.data.path) {
                    ui.horizontal(|ui| {
                        if ui.button(&name).on_hover_text(self.t("project.profile.load")).clicked() {
                            act = Some(ProjectAct::Load(path.clone()));
                            ui.close();
                        }
                        if ui.small_button("\u{00d7}").on_hover_text(self.t("project.profile.delete")).clicked() {
                            act = Some(ProjectAct::Delete(path.clone()));
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut typed).hint_text(hint.as_str()).desired_width(130.0));
                    let named = !project::profile_name(&typed).is_empty();
                    let keep = ui.add_enabled(named, egui::Button::new(self.t("project.profile.save")));
                    if keep.on_hover_text(self.t("project.profile.save.help")).on_disabled_hover_text(self.t("project.profile.save.help")).clicked() {
                        act = Some(ProjectAct::Keep);
                    }
                });
                ui.separator();
                ui.label(RichText::new(self.t("project.history")).small().color(theme::text_dim())).on_hover_text(self.t("project.history.help"));
                if self.settings.history.is_empty() {
                    ui.label(RichText::new(self.t("project.history.none")).small().color(theme::text_faint()));
                }
                for (i, past) in self.settings.history.iter().enumerate() {
                    if ui.button(self.past_text(past)).on_hover_text(self.t("project.history.item.help")).clicked() {
                        act = Some(ProjectAct::Restore(i));
                        ui.close();
                    }
                }
            });
            self.profile_name = typed;
            menu.response.on_hover_text(self.t("project.menu.help"));
            if let Some(act) = act {
                self.project_act(act);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(6.0);
                let mut lang = self.settings.language;
                egui::ComboBox::from_id_salt("language")
                    .selected_text(lang.native_name())
                    .icon(widgets::chevron)
                    .width(104.0)
                    .show_ui(ui, |ui| {
                        for l in Lang::ALL {
                            ui.selectable_value(&mut lang, l, l.native_name());
                        }
                    })
                    .response
                    .on_hover_text(self.t("top.language.help"));
                if lang != self.settings.language {
                    self.settings.language = lang;
                    self.i18n.set_lang(lang);
                    self.changed();
                }
                let mut look = self.settings.appearance;
                egui::ComboBox::from_id_salt("appearance")
                    .selected_text(self.t(&format!("appearance.{}", look.key())).to_string())
                    .icon(widgets::chevron)
                    .width(86.0)
                    .show_ui(ui, |ui| {
                        for a in Appearance::ALL {
                            ui.selectable_value(&mut look, a, self.t(&format!("appearance.{}", a.key())).to_string());
                        }
                    })
                    .response
                    .on_hover_text(self.t("appearance.help"));
                if look != self.settings.appearance {
                    self.settings.appearance = look;
                    self.changed();
                }
                ui.add_space(6.0);

                // Export is only possible while the baked build matches the settings.
                let export_reason = match status {
                    Status::Current => None,
                    Status::Never => Some("export.blocked.never"),
                    Status::Stale(_) | Status::StaleTuning => Some("export.blocked.stale"),
                };
                let export = ui.add_enabled(export_reason.is_none(), egui::Button::new(self.t("action.export")));
                if export.clicked() {
                    self.tools[slot(self.settings.mode)].open_export();
                }
                match export_reason {
                    Some(reason) => export.on_disabled_hover_text(self.t(reason)),
                    None => export.on_hover_text(self.t("action.export.help")),
                };
                let can_bake = !self.tools[slot(self.settings.mode)].baking();
                let button = widgets::primary_button(ui, self.t("action.bake"), accent, can_bake);
                bake = button.clicked();
                button.on_hover_text(self.t("action.bake.help"));
                ui.add_space(4.0);
                let mut live = self.settings.auto_bake;
                let switch = widgets::toggle(ui, &mut live, accent);
                let name = ui.add(egui::Label::new(self.t("action.live")).selectable(false).sense(Sense::click()));
                if name.clicked() {
                    live = !live;
                }
                switch.on_hover_text(self.t("action.live.help"));
                name.on_hover_text(self.t("action.live.help"));
                if live != self.settings.auto_bake {
                    self.settings.auto_bake = live;
                    self.changed();
                }
                ui.add_space(4.0);
                if ui.add(egui::Button::new(self.t("action.files"))).on_hover_text(self.t("action.files.help")).clicked() {
                    self.files.open = !self.files.open;
                }
                ui.add_space(10.0);

                let (colour, text) = match status {
                    Status::Never => (theme::text_dim(), self.t("status.never_baked").to_string()),
                    Status::Current => (theme::good(), self.t("status.current").to_string()),
                    Status::Stale(stage) => {
                        let stage = self.t(&format!("stage.{}", stage.key())).to_string();
                        (theme::warn(), self.i18n.tf("status.stale", &[("stage", &stage)]))
                    }
                    Status::StaleTuning => (theme::warn(), self.t("status.stale_tuning").to_string()),
                };
                // About how big and how long the next bake will be.
                let estimate = match (self.tools[slot(mode)].estimate(&self.settings.values), status == Status::Current) {
                    (Some((blocks, seconds)), false) => {
                        let time = self.duration_text(Duration::from_secs_f64(seconds.clamp(1.0, 1.0e6)));
                        Some(self.i18n.tf("status.estimate", &[("blocks", &crate::files::group(blocks)), ("time", &time)]))
                    }
                    _ => None,
                };
                // In a narrow window the status moves to the status bar, so it
                // never covers the buttons on the left.
                self.status_moved = None;
                if ui.available_width() >= widgets::capsule_width(ui, &text) + 8.0 {
                    widgets::capsule(ui, colour, &text, Sense::hover()).on_hover_text(self.t("status.help"));
                    if let Some(estimate) = estimate {
                        let wide = ui.painter().layout_no_wrap(estimate.clone(), egui::TextStyle::Small.resolve(ui.style()), Color32::PLACEHOLDER).size().x;
                        if ui.available_width() >= wide + 8.0 {
                            ui.label(RichText::new(estimate).small().color(theme::text_dim())).on_hover_text(self.t("status.estimate.help"));
                        }
                    }
                } else {
                    self.status_moved = Some((colour, text, estimate));
                }
            });
        });
        if bake {
            self.bake(ui.ctx());
        }
    }

    // ---------------------------------------------------------- control panel

    fn controls(&mut self, ui: &mut egui::Ui) {
        let mode = self.settings.mode;
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(offset) = self.launch.scroll {
            area = area.vertical_scroll_offset(offset);
        }
        area.show(ui, |ui| {
            ui.add_space(4.0);
            // The land comes first: a build is brought in before anything
            // else is set around it.
            if mode == Mode::Ocean {
                let forced = self.launch.hover.as_deref();
                self.tools[slot(self.settings.mode)].scene_card(ui, &self.settings.values, &self.i18n, theme::accent(mode), &mut self.hover, forced);
            }
            for group in params::groups(mode, false) {
                self.group(ui, mode, group, false);
            }

            let advanced_groups = params::groups(mode, true);
            if !advanced_groups.is_empty() {
                ui.add_space(14.0);
                let header = egui::CollapsingHeader::new(RichText::new(self.t("panel.advanced")).font(theme::semibold(13.5)))
                    .id_salt(("advanced", mode.key()))
                    .open(None)
                    .default_open(self.settings.advanced_open);
                let response = header.show(ui, |ui| {
                    for group in advanced_groups {
                        self.group(ui, mode, group, true);
                    }
                    if mode == Mode::Ocean {
                        let forced = self.launch.hover.as_deref();
                        self.tools[slot(self.settings.mode)].tuning_card(ui, &self.i18n, &mut self.hover, forced);
                    }
                });
                if response.header_response.hovered() || self.launch.hover.as_deref() == Some("panel.advanced") {
                    self.hover = Some(HoverHelp {
                        title: self.t("panel.advanced").to_string(),
                        body: self.t("panel.advanced.help").to_string(),
                        anchor_y: response.header_response.rect.top(),
                    });
                }
                let open = response.fully_open() || response.openness > 0.5;
                if open != self.settings.advanced_open && response.header_response.clicked() {
                    self.settings.advanced_open = open;
                    self.changed();
                }
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(self.t("help.hint")).small().color(theme::text_faint()));
            });
            ui.add_space(14.0);
        });
    }

    /// One titled card of controls, its rows divided by hairlines.
    fn group(&mut self, ui: &mut egui::Ui, mode: Mode, group: &'static str, advanced: bool) {
        // A group none of whose controls can do anything now is left out
        // altogether, not drawn as an empty card under its title.
        let any = params::for_mode(mode)
            .any(|d| d.group == group && d.advanced == advanced && params::is_relevant(d, &self.settings.values));
        if !any {
            return;
        }
        let title = self.t(&format!("group.{group}")).to_string();
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(title).font(theme::semibold(13.0)).color(theme::text_dim()));
        });
        theme::card().inner_margin(egui::Margin::symmetric(14, 2)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut first = true;
            for def in params::for_mode(mode).filter(|d| d.group == group && d.advanced == advanced) {
                if params::is_relevant(def, &self.settings.values) {
                    if !first {
                        widgets::hairline(ui);
                    }
                    first = false;
                    self.row(ui, def);
                }
            }
        });
    }

    /// One control: a name on the left that shows help when hovered, and the
    /// widget on the right. The help card is drawn beside the panel, never
    /// over the widget.
    fn row(&mut self, ui: &mut egui::Ui, def: &'static ParamDef) {
        let label_key = def.label_key();
        let label = self.t(&label_key).to_string();
        let accent = theme::accent(self.settings.mode);
        let size = egui::vec2(ui.available_width(), ROW_HEIGHT);
        ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
            // A switch is narrow, so its name may use the rest of the row.
            let label_width = match def.kind {
                Kind::Bool { .. } => ui.available_width() - 54.0,
                _ => LABEL_WIDTH,
            };
            let (rect, response) = ui.allocate_exact_size(egui::vec2(label_width, ROW_HEIGHT), Sense::hover());
            let forced = self.launch.hover.as_deref() == Some(def.id);
            let hovered = response.hovered() || forced;
            let colour = if hovered { theme::text_strong() } else { theme::text() };
            let galley = ui.painter().layout_no_wrap(label.clone(), egui::TextStyle::Body.resolve(ui.style()), colour);
            let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
            ui.painter().with_clip_rect(rect).galley(pos, galley, colour);
            if hovered {
                self.hover = Some(HoverHelp { title: label.clone(), body: self.t(&def.help_key()).to_string(), anchor_y: rect.top() });
            }

            let current = self.settings.values.get(def);
            let mut new = current.clone();
            // Controls are laid out from the right edge inwards, so every
            // row ends on the same line whatever it holds.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match def.kind {
                Kind::Float { min, max, decimals, .. } => {
                    let mut x = current.as_f64().unwrap_or(min);
                    let before = x;
                    let mut field =
                        egui::DragValue::new(&mut x).range(min..=max).fixed_decimals(decimals as usize).speed((max - min) / 400.0);
                    if ["direction", "rotation", "bearing", "elevation"].iter().any(|end| def.id.ends_with(end)) {
                        field = field.suffix("°");
                    }
                    ui.add_sized([VALUE_WIDTH, 24.0], field);
                    let width = ui.available_width();
                    widgets::slider(ui, &mut x, min, max, decimals as u32, accent, width);
                    if x != before {
                        new = Value::Float(x);
                    }
                }
                Kind::Int { min, max, .. } => {
                    let mut x = current.as_i64().unwrap_or(min);
                    let before = x;
                    let speed = ((max - min) as f64 / 400.0).max(0.2);
                    ui.add_sized([VALUE_WIDTH, 24.0], egui::DragValue::new(&mut x).range(min..=max).speed(speed));
                    let mut f = x as f64;
                    let width = ui.available_width();
                    widgets::slider(ui, &mut f, min as f64, max as f64, 0, accent, width);
                    x = f.round() as i64;
                    if x != before {
                        new = Value::Int(x);
                    }
                }
                Kind::Seed { .. } => {
                    if ui.button(self.t("seed.randomise")).on_hover_text(self.t("seed.randomise.help")).clicked() {
                        new = Value::Int(random_seed());
                    }
                    let mut x = current.as_i64().unwrap_or(0);
                    if ui.add_sized([VALUE_WIDTH + 16.0, 24.0], egui::DragValue::new(&mut x).range(0..=SEED_MAX).speed(1.0)).changed() {
                        new = Value::Int(x);
                    }
                }
                Kind::Choice { options, .. } => {
                    let selected = current.as_choice().unwrap_or("").to_string();
                    let mut picked = selected.clone();
                    egui::ComboBox::from_id_salt(def.id)
                        .selected_text(self.t(&def.option_key(&selected)).to_string())
                        .icon(widgets::chevron)
                        .width(178.0)
                        .show_ui(ui, |ui| {
                            for option in options {
                                ui.selectable_value(&mut picked, option.to_string(), self.t(&def.option_key(option)).to_string());
                            }
                        });
                    if picked != selected {
                        new = Value::Choice(picked);
                    }
                }
                Kind::Bool { .. } => {
                    let mut on = current.as_bool().unwrap_or(false);
                    if widgets::toggle(ui, &mut on, accent).changed() {
                        new = Value::Bool(on);
                    }
                }
                Kind::Colour { default } => {
                    let rgb = params::parse_colour(current.as_choice().unwrap_or(default)).unwrap_or([0; 3]);
                    // What is being typed is kept apart from the value, so a
                    // half-typed colour is not thrown away between frames.
                    let typing = egui::Id::unique(("colour_text", def.id));
                    let mut text = ui.data(|d| d.get_temp::<String>(typing)).unwrap_or_else(|| params::colour_text(rgb));
                    let field = ui
                        .add_sized([92.0, 24.0], egui::TextEdit::singleline(&mut text).char_limit(7))
                        .on_hover_text(self.t("colour.hex.help"));
                    if field.has_focus() {
                        if let Some(typed) = params::parse_colour(&text).filter(|_| field.changed()) {
                            new = Value::Choice(params::colour_text(typed));
                        }
                        ui.data_mut(|d| d.insert_temp(typing, text));
                    } else {
                        ui.data_mut(|d| d.remove::<String>(typing));
                    }
                    let mut picked = rgb;
                    ui.spacing_mut().interact_size = egui::vec2(46.0, 24.0);
                    if ui.color_edit_button_srgb(&mut picked).on_hover_text(self.t("colour.pick.help")).changed() {
                        new = Value::Choice(params::colour_text(picked));
                    }
                }
            });
            if new != current && self.settings.values.set(def, new) {
                self.changed();
            }
        });
    }

    /// The hover-help card: beside the control panel, level with the hovered
    /// name. It takes no pointer input, so it can never block a click.
    fn help_card(&mut self, ctx: &egui::Context) {
        let Some(help) = self.hover.take() else { return };
        let screen = ctx.content_rect();
        let y = help.anchor_y.clamp(screen.top() + 8.0, (screen.bottom() - 150.0).max(screen.top() + 8.0));
        egui::Area::new(egui::Id::unique("hover_help"))
            .order(egui::Order::Tooltip)
            .fixed_pos(egui::pos2(self.panel_right + 10.0, y))
            .interactable(false)
            .show(ctx, |ui| {
                theme::floating().inner_margin(egui::Margin::symmetric(16, 14)).show(ui, |ui| {
                    ui.set_width(HELP_WIDTH);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(RichText::new(help.title).font(theme::semibold(14.0)).color(theme::text_strong()));
                    ui.label(RichText::new(help.body).color(theme::text_dim()));
                });
            });
    }

    // -------------------------------------------------------------- viewport

    fn viewport(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        // A canvas that is near black at the bottom and carries a breath of
        // the mode's colour at the top.
        let top = theme::mix(theme::bg(), theme::accent(self.settings.mode), 0.09);
        let mut mesh = egui::Mesh::default();
        for (corner, colour) in
            [(rect.left_top(), top), (rect.right_top(), top), (rect.left_bottom(), theme::bg()), (rect.right_bottom(), theme::bg())]
        {
            mesh.colored_vertex(corner, colour);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));
        match self.tools[slot(self.settings.mode)].viewport(ui, rect, &self.i18n) {
            Some(ListAction::Exclude(name)) if !self.settings.excluded.contains(&name) => {
                self.settings.excluded.push(name);
                self.changed();
            }
            Some(ListAction::Restore) => {
                self.settings.excluded.clear();
                self.changed();
            }
            _ => {}
        }
        ui.allocate_rect(rect, Sense::hover());
    }

    // ------------------------------------------------------------ status bar

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.add_space(2.0);
            let (colour, text) = self.device_summary();
            if widgets::capsule(ui, colour, &text, Sense::click()).on_hover_text(self.t("compute.help")).clicked() {
                self.devices_open = !self.devices_open;
            }
            if let Some((colour, text, estimate)) = self.status_moved.clone() {
                ui.add_space(6.0);
                widgets::capsule(ui, colour, &text, Sense::hover()).on_hover_text(self.t("status.help"));
                if let Some(estimate) = estimate {
                    ui.label(RichText::new(estimate).small().color(theme::text_dim())).on_hover_text(self.t("status.estimate.help"));
                }
            }

            if let Some((name, snap)) = self.active_job() {
                ui.add_space(6.0);
                ui.label(RichText::new(self.t(name)).small());
                widgets::progress(ui, snap.fraction as f32, theme::accent(self.settings.mode), 160.0);
                let timing = if snap.cancelling {
                    self.t("job.cancelling").to_string()
                } else {
                    match snap.remaining {
                        Some(d) => self.i18n.tf("job.eta", &[("time", &self.duration_text(d))]),
                        None => self.t("job.estimating").to_string(),
                    }
                };
                ui.label(RichText::new(timing).small().color(theme::text_dim()));
                if let Some(bytes) = snap.est_bytes {
                    ui.label(
                        RichText::new(self.i18n.tf("job.memory", &[("size", &coarse_bytes(bytes))])).small().color(theme::text_dim()),
                    );
                }
                let cancel = ui.add_enabled(!snap.cancelling, egui::Button::new(RichText::new(self.t("action.cancel")).small()));
                if cancel.on_hover_text(self.t("action.cancel.help")).clicked() {
                    if self.tools[slot(self.settings.mode)].active_job().is_some() {
                        self.tools[slot(self.settings.mode)].cancel();
                    } else if self.files.busy() {
                        self.files.cancel();
                    } else if let Some(j) = &self.bench_job {
                        j.cancel();
                    } else if let Some(j) = &self.probe_job {
                        j.cancel();
                    }
                }
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            } else if let Some(note) = &self.job_note {
                ui.add_space(6.0);
                ui.label(RichText::new(note).small().color(theme::warn()));
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{} · {}", of_core::VERSION, of_core::MILESTONE)).small().color(theme::text_faint()),
                );
                if let Some(e) = self.autosave.last_error() {
                    ui.label(RichText::new(self.i18n.tf("notice.save_failed", &[("error", e)])).small().color(theme::bad()));
                }
            });
        });
    }

    fn device_summary(&self) -> (Color32, String) {
        let Some(probe) = self.probe.get() else {
            return (theme::text_dim(), self.t("compute.detecting").to_string());
        };
        match of_gpu::resolve(&self.settings.compute, probe).device {
            Resolved::Gpu(i) => (theme::good(), self.i18n.tf("compute.using", &[("name", &probe.adapters[i].name)])),
            Resolved::Cpu(reason) => (
                if reason == CpuReason::Chosen { theme::good() } else { theme::warn() },
                self.i18n.tf("compute.using_cpu", &[("threads", &probe.cpu_threads.to_string())]),
            ),
        }
    }

    fn devices_window(&mut self, ctx: &egui::Context) {
        if !self.devices_open {
            return;
        }
        let mut open = true;
        let mut choice = self.settings.compute.clone();
        let mut run_benchmark = false;
        egui::Window::new(self.t("compute.title").to_string())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::LEFT_BOTTOM, [12.0, -44.0])
            .show(ctx, |ui| {
                ui.set_width(430.0);
                let Some(probe) = self.probe.get() else {
                    ui.label(self.t("compute.detecting"));
                    return;
                };
                let resolution = of_gpu::resolve(&self.settings.compute, probe);
                ui.radio_value(&mut choice, ComputeChoice::Auto, self.t("compute.auto")).on_hover_text(self.t("compute.auto.help"));
                for a in &probe.adapters {
                    if a.kind == of_gpu::DeviceKind::Software {
                        continue;
                    }
                    let this = ComputeChoice::Adapter { name: a.name.clone(), backend: a.backend.clone() };
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(a.usable(), |ui| {
                            ui.radio_value(&mut choice, this, &a.name).on_hover_text(self.t("compute.adapter.help"))
                        });
                        ui.label(RichText::new(format!("{} · {}", self.t(a.kind.key()), a.backend)).small().color(theme::text_dim()));
                        match &a.self_test {
                            of_gpu::SelfTest::Passed => {
                                ui.label(RichText::new(self.t("compute.selftest.pass")).small().color(theme::good()));
                            }
                            of_gpu::SelfTest::Failed(why) => {
                                ui.label(RichText::new(self.t("compute.selftest.fail")).small().color(theme::bad())).on_hover_text(why);
                            }
                        }
                    });
                }
                ui.radio_value(&mut choice, ComputeChoice::Cpu, self.t("compute.cpu")).on_hover_text(self.t("compute.cpu.help"));

                if resolution.requested_missing {
                    ui.label(RichText::new(self.t("compute.missing")).small().color(theme::warn()));
                } else if resolution.device == Resolved::Cpu(CpuReason::NoUsableGpu) {
                    ui.label(RichText::new(self.t("compute.fallback")).small().color(theme::warn()));
                }

                ui.separator();
                let idle = self.bench_job.is_none() && self.probe_job.is_none();
                let measure = ui.add_enabled(idle, egui::Button::new(self.t("compute.benchmark")));
                if measure.on_hover_text(self.t("compute.benchmark.help")).clicked() {
                    run_benchmark = true;
                }
                match self.bench.get() {
                    None => {
                        ui.label(RichText::new(self.t("compute.bench.none")).small().color(theme::text_dim()));
                    }
                    Some(result) => {
                        if let Some((name, rate)) = &result.gpu {
                            ui.label(self.i18n.tf("compute.bench.gpu", &[("name", name), ("rate", &format!("{rate:.0}"))]));
                        }
                        ui.label(self.i18n.tf("compute.bench.cpu", &[("rate", &format!("{:.0}", result.cpu))]));
                        if let Some(e) = &result.gpu_error {
                            ui.label(RichText::new(self.i18n.tf("job.failed", &[("error", e)])).small().color(theme::warn()));
                        }
                    }
                }
            });
        if choice != self.settings.compute {
            self.settings.compute = choice;
            self.changed();
        }
        if run_benchmark {
            self.start_benchmark(ctx);
        }
        if !open {
            self.devices_open = false;
        }
    }

    fn notices_bar(&mut self, ui: &mut egui::Ui) {
        let mut dismiss = None;
        for (i, notice) in self.notices.iter().enumerate() {
            egui::Frame::new().fill(theme::warn().gamma_multiply(0.18)).inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(notice).color(theme::warn()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(self.i18n.t("notice.dismiss")).clicked() {
                            dismiss = Some(i);
                        }
                    });
                });
            });
        }
        if let Some(i) = dismiss {
            self.notices.remove(i);
        }
    }

    /// For automated runs: save a picture of the window, then close.
    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.launch.screenshot.clone() else { return };
        self.frames += 1;
        ctx.request_repaint();
        // Wait for the device check so the status bar shows its final state,
        // but never hang an automated run if it takes too long.
        let idle = self.probe_job.is_none() && !self.files.busy() && self.tools[slot(self.settings.mode)].settled();
        // A time, not a count of frames: frames come fast while a large bake runs.
        let ready = self.frames > 8 && (idle || ctx.input(|i| i.time) > 1800.0);
        if ready && !self.screenshot_requested {
            self.screenshot_requested = true;
            let ctx2 = ctx.clone();
            ctx.request_screenshot(move |image| {
                let result = image::save_buffer(
                    &path,
                    image.as_raw(),
                    image.width() as u32,
                    image.height() as u32,
                    image::ColorType::Rgba8,
                );
                if let Err(e) = result {
                    eprintln!("error: could not save screenshot: {e}");
                }
                ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
            });
        }
    }
}

impl eframe::App for OceanFoundryApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Set again when the mode or appearance changes, or when Windows
        // itself switches between light and dark.
        let want = (self.settings.mode, self.settings.appearance);
        if self.themed_for != Some((want.0, want.1, ctx.theme())) {
            let showing = theme::apply(&ctx, want.0, want.1);
            self.themed_for = Some((want.0, want.1, showing));
            ctx.request_repaint();
        }
        self.poll_jobs();
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::B)) {
            self.bake(&ctx);
        }
        self.tools[slot(self.settings.mode)].tick(&ctx, &self.settings, &self.i18n);
        if self.tools[slot(self.settings.mode)].status() == Status::Current {
            // For automated runs: save as soon as there is a build to save.
            if let Some(path) = self.launch.export_to.take() {
                self.tools[slot(self.settings.mode)].save(&ctx, path);
                self.exporting = true;
            }
            if let Some(path) = self.launch.save_project.take() {
                self.save_project(&path);
            }
            if let Some(name) = self.launch.save_profile.take() {
                let path = project::profile_path(&self.data.path, &name);
                self.save_project(&path);
            }
        }

        // For automated runs: say how the export went, and close when there
        // is no picture to take as well.
        if self.exporting {
            if let Some(report) = self.tools[slot(self.settings.mode)].export_report(&self.i18n) {
                self.exporting = false;
                match report {
                    Ok(text) => println!("{text}"),
                    Err(text) => {
                        eprintln!("error: {text}");
                        crate::SAVE_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                if self.launch.screenshot.is_none() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        egui::Panel::top("top")
            .exact_size(52.0)
            .frame(egui::Frame::new().fill(theme::panel()).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui,|ui| self.top_bar(ui));
        if !self.notices.is_empty() {
            egui::Panel::top("notices").frame(egui::Frame::new()).show(ui,|ui| self.notices_bar(ui));
        }
        egui::Panel::bottom("status")
            .exact_size(34.0)
            .frame(egui::Frame::new().fill(theme::panel()).inner_margin(egui::Margin::symmetric(10, 0)))
            .show(ui,|ui| self.status_bar(ui));
        let panel = egui::Panel::left("controls")
            .exact_size(PANEL_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::panel()).inner_margin(egui::Margin::symmetric(14, 0)))
            .show(ui,|ui| self.controls(ui));
        self.panel_right = panel.response.rect.right();
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::bg())).show(ui,|ui| self.viewport(ui));

        self.devices_window(&ctx);
        self.files.window(&ctx, &self.i18n, theme::accent(self.settings.mode));
        self.tools[slot(self.settings.mode)].export_window(&ctx, &self.i18n, theme::accent(self.settings.mode));
        self.help_card(&ctx);

        if let Some(wait) = self.autosave.tick(&self.settings) {
            ctx.request_repaint_after(wait);
        }
        self.handle_screenshot(&ctx);
    }

    /// Shows through the one-pixel gaps between panels, where the divider
    /// hairlines are drawn.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::panel().to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        self.autosave.flush(&self.settings);
        // Stop background work so the process can end promptly.
        self.files.cancel_all();
        for tool in &self.tools {
            tool.cancel_all();
        }
        if let Some(j) = &self.bench_job {
            j.cancel();
        }
        if let Some(j) = &self.probe_job {
            j.cancel();
        }
        log::info!("settings saved to {}", self.data.path.display());
    }
}
