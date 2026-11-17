//! The "Build files" window: open any supported build file, see what is in
//! it, and save it in another format.

use crate::theme;
use crate::widgets;
use eframe::egui::{self, Color32, RichText};
use of_core::i18n::I18n;
use of_core::jobs::{spawn, JobCtx, JobError, JobHandle, Outcome, Snapshot};
use of_formats::catalogue;
use of_formats::export::{self, COMFORTABLE_REGIONS};
use of_formats::model::{Build, Format, Note};
use of_formats::Progress;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Loaded {
    path: PathBuf,
    build: Arc<Build>,
    notes: Vec<Note>,
    size: [i32; 3],
    blocks: u64,
    air: u64,
    kinds: Vec<(String, u64)>,
    kind_count: usize,
    unknown: Vec<String>,
}

/// The loaded build rearranged as air-free boxes, ready to save.
struct Boxed {
    build: Arc<Build>,
    notes: Vec<Note>,
}

struct Saved {
    path: PathBuf,
    blocks: u64,
    regions: usize,
    bytes: usize,
    notes: Vec<Note>,
}

pub(crate) type Job<T> = JobHandle<Result<T, Note>>;

pub struct FilesTool {
    pub open: bool,
    loaded: Option<Loaded>,
    boxed: Option<Boxed>,
    saved: Option<Saved>,
    /// Height the save options took on the last frame.
    below_height: f32,
    /// Why the last open or save did not happen. The previous file stays loaded.
    problem: Option<Note>,
    import_job: Option<Job<Loaded>>,
    box_job: Option<Job<Boxed>>,
    save_job: Option<Job<Saved>>,
    format: Format,
    omit_air: bool,
}

/// Run a file operation as a job. Cancelling surfaces as a cancelled job;
/// any other refusal is returned as a value so it can be shown translated.
pub(crate) fn run<T: Send + 'static>(
    name: &str,
    ctx: &egui::Context,
    work: impl FnOnce(&Progress) -> Result<T, Note> + Send + 'static,
) -> Job<T> {
    let ctx = ctx.clone();
    spawn(name, move || ctx.request_repaint(), move |job: &JobCtx| {
        let report = |fraction: f32| job.progress(fraction as f64).is_ok();
        match work(&Progress::new(&report)) {
            Err(p) if p.key == of_formats::CANCELLED => Err(JobError::Cancelled),
            other => Ok(other),
        }
    })
}

fn load(path: &Path, progress: &Progress) -> Result<Loaded, Note> {
    let data = std::fs::read(path).map_err(|e| Note::new("err.read").arg("detail", e))?;
    let imported = of_formats::import(&data, progress)?;
    let build = imported.build;
    let (blocks, air) = build.counts();
    let histogram = build.histogram();
    let size = build.bounds().map(|(lo, hi)| [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1]).unwrap_or([0; 3]);
    Ok(Loaded {
        path: path.to_path_buf(),
        notes: imported.notes,
        size,
        blocks,
        air,
        kind_count: histogram.len(),
        kinds: histogram.iter().take(8).map(|(s, n)| (s.to_string(), *n)).collect(),
        unknown: catalogue::default().unknown_in(&build),
        build: Arc::new(build),
    })
}

pub(crate) fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn note_text(i18n: &I18n, note: &Note) -> String {
    let args: Vec<(&str, &str)> = note.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    i18n.tf(&format!("file.{}", note.key), &args)
}

pub(crate) fn format_label(format: Format) -> &'static str {
    match format {
        Format::Litematic => "file.format.litematic",
        Format::Sponge => "file.format.sponge",
        Format::Legacy => "file.format.legacy",
        Format::Axiom => "file.format.axiom",
    }
}

/// What each format that can be written shows when the pointer rests on it.
pub(crate) fn format_tips(i18n: &I18n) -> Vec<String> {
    Format::EXPORTABLE.iter().map(|f| i18n.t(&format!("{}.help", format_label(*f))).to_string()).collect()
}

impl FilesTool {
    pub fn new() -> Self {
        Self {
            open: false,
            below_height: 215.0,
            loaded: None,
            boxed: None,
            saved: None,
            problem: None,
            import_job: None,
            box_job: None,
            save_job: None,
            format: Format::Litematic,
            omit_air: false,
        }
    }

    /// For automated runs: open the window on a file with options chosen.
    pub fn preset(&mut self, ctx: &egui::Context, path: &Path, format: Option<Format>, omit_air: bool) {
        self.open = true;
        self.format = format.unwrap_or(self.format);
        self.omit_air = omit_air;
        self.start_import(ctx, path.to_path_buf());
    }

    pub fn busy(&self) -> bool {
        self.import_job.is_some() || self.box_job.is_some() || self.save_job.is_some()
    }

    pub fn active_job(&self) -> Option<(&'static str, Snapshot)> {
        if let Some(j) = &self.save_job {
            return Some(("job.file_save", j.snapshot()));
        }
        if let Some(j) = &self.import_job {
            return Some(("job.file_open", j.snapshot()));
        }
        self.box_job.as_ref().map(|j| ("job.file_boxes", j.snapshot()))
    }

    pub fn cancel(&self) {
        if let Some(j) = &self.save_job {
            j.cancel();
        } else if let Some(j) = &self.import_job {
            j.cancel();
        } else if let Some(j) = &self.box_job {
            j.cancel();
        }
    }

    pub fn cancel_all(&self) {
        [self.save_job.as_ref().map(|j| j.cancel()), self.import_job.as_ref().map(|j| j.cancel()), self.box_job.as_ref().map(|j| j.cancel())];
    }

    fn start_import(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.problem = None;
        self.import_job = Some(run("job.file_open", ctx, move |progress| load(&path, progress)));
    }

    fn start_boxing(&mut self, ctx: &egui::Context) {
        let Some(loaded) = &self.loaded else { return };
        let build = loaded.build.clone();
        self.box_job = Some(run("job.file_boxes", ctx, move |progress| {
            let (build, notes) = export::omit_air(&build, progress)?;
            Ok(Boxed { build: Arc::new(build), notes })
        }));
    }

    fn start_save(&mut self, ctx: &egui::Context, path: PathBuf) {
        let Some(loaded) = &self.loaded else { return };
        let format = self.format;
        let (build, mut notes) = match (&self.boxed, self.omit_air && format == Format::Litematic) {
            (Some(boxed), true) => (boxed.build.clone(), boxed.notes.clone()),
            _ => (loaded.build.clone(), Vec::new()),
        };
        self.problem = None;
        self.save_job = Some(run("job.file_save", ctx, move |progress| {
            let out = export::convert(&build, format, catalogue::default(), progress)?;
            // Written beside the target first, so a failure cannot leave half a file.
            let partial = path.with_extension(format!("{}.part", format.extension()));
            std::fs::write(&partial, &out.bytes)
                .and_then(|_| std::fs::rename(&partial, &path))
                .map_err(|e| Note::new("err.write").arg("detail", e))?;
            notes.extend(out.notes);
            Ok(Saved { path, blocks: out.blocks, regions: out.regions, bytes: out.bytes.len(), notes })
        }));
    }

    /// Collect finished jobs. A cancelled or refused job changes nothing
    /// that was already loaded.
    pub fn poll(&mut self) -> Option<String> {
        let mut failure = None;
        fn take<T>(job: &mut Option<Job<T>>, problem: &mut Option<Note>, failure: &mut Option<String>) -> Option<T> {
            let outcome = job.as_ref().and_then(JobHandle::poll)?;
            *job = None;
            match outcome {
                Outcome::Done(Ok(value)) => return Some(value),
                Outcome::Done(Err(note)) => *problem = Some(note),
                Outcome::Cancelled => *problem = Some(Note::new("err.cancelled")),
                Outcome::Failed(e) => *failure = Some(e),
            }
            None
        }
        if let Some(loaded) = take(&mut self.import_job, &mut self.problem, &mut failure) {
            self.loaded = Some(loaded);
            self.boxed = None;
            self.saved = None;
            if let Some(j) = self.box_job.take() {
                j.cancel();
            }
        }
        if let Some(boxed) = take(&mut self.box_job, &mut self.problem, &mut failure) {
            self.boxed = Some(boxed);
        }
        if let Some(saved) = take(&mut self.save_job, &mut self.problem, &mut failure) {
            self.saved = Some(saved);
        }
        failure
    }

    pub fn window(&mut self, ctx: &egui::Context, i18n: &I18n, accent: Color32) {
        if !self.open {
            return;
        }
        // Boxes are worked out as soon as they are asked for, so the region
        // count is on screen before anything is saved.
        if self.omit_air && self.format == Format::Litematic && self.loaded.is_some() && self.boxed.is_none() && self.box_job.is_none() && self.import_job.is_none() {
            let refused = self.problem.as_ref().is_some_and(|p| p.key == "err.empty" || p.key == "err.cancelled");
            if !refused {
                self.start_boxing(ctx);
            }
        }
        let mut open = true;
        let mut pick_open = false;
        let mut pick_save = false;
        // The details get whatever height is left once the save options
        // below them (measured on the previous frame) are allowed for.
        let floor = ctx.content_rect().bottom() - 34.0 - 4.0;
        let below = self.below_height;
        let mut measured = below;
        egui::Window::new(i18n.t("file.title").to_string())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos([430.0, 60.0])
            // Tall enough not to limit the details, short enough that the
            // window still starts below the toolbar.
            .default_height((floor - 60.0 - 100.0).max(300.0))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.horizontal(|ui| {
                    let button = ui.add_enabled(!self.busy(), egui::Button::new(i18n.t("file.open")));
                    if button.on_hover_text(i18n.t("file.open.help")).clicked() {
                        pick_open = true;
                    }
                    if let Some(loaded) = &self.loaded {
                        let name = loaded.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        ui.label(RichText::new(name).font(theme::semibold(14.0)).color(theme::text_strong()))
                            .on_hover_text(loaded.path.display().to_string());
                    }
                });
                if let Some(problem) = &self.problem {
                    ui.label(RichText::new(note_text(i18n, problem)).color(theme::bad()));
                    if self.loaded.is_some() && problem.key != "err.cancelled" {
                        ui.label(RichText::new(i18n.t("file.kept")).small().color(theme::text_dim()));
                    }
                }
                // Lifted out while it is drawn, so the save options can
                // change the tool's own settings alongside it.
                let Some(loaded) = self.loaded.take() else { return };
                let has_air = loaded.air > 0;
                // On a small screen the details scroll, so the save options
                // are never pushed off the bottom.
                // A faint handle stays in view, so it is clear when there is more.
                ui.spacing_mut().scroll.dormant_handle_opacity = 0.4;
                let room = (floor - ui.cursor().top() - below - 24.0).max(120.0);
                egui::ScrollArea::vertical().max_height(room).show(ui, |ui| {
                    ui.add_space(2.0);
                    self.summary(ui, i18n, &loaded);
                    ui.add_space(2.0);
                });
                ui.add_space(2.0);
                let top = ui.cursor().top();
                self.save_options(ui, i18n, has_air, accent, &mut pick_save);
                measured = ui.cursor().top() - top;
                self.loaded = Some(loaded);
            });
        if measured != self.below_height {
            self.below_height = measured;
            ctx.request_repaint();
        }
        if !open {
            self.open = false;
        }
        if pick_open {
            let picked = rfd::FileDialog::new()
                .add_filter(i18n.t("file.filter.all"), &["litematic", "schem", "schematic", "bp"])
                .add_filter(i18n.t("file.filter.any"), &["*"])
                .pick_file();
            if let Some(path) = picked {
                self.start_import(ctx, path);
            }
        }
        if pick_save {
            let stem = self.loaded.as_ref().and_then(|l| l.path.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let picked = rfd::FileDialog::new()
                .add_filter(i18n.t(format_label(self.format)), &[self.format.extension()])
                .set_file_name(format!("{stem}.{}", self.format.extension()))
                .save_file();
            if let Some(path) = picked {
                self.start_save(ctx, path);
            }
        }
    }

    fn summary(&self, ui: &mut egui::Ui, i18n: &I18n, loaded: &Loaded) {
        let b = &loaded.build;
        let row = |ui: &mut egui::Ui, label: &str, value: String| {
            ui.label(RichText::new(i18n.t(label)).color(theme::text_dim()));
            ui.label(value);
            ui.end_row();
        };
        theme::card().inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("file_summary").num_columns(2).spacing([20.0, 5.0]).min_row_height(16.0).show(ui, |ui| {
                let mut kind = b.source.map(|f| i18n.t(format_label(f)).to_string()).unwrap_or_default();
                if let Some(v) = b.source_version {
                    kind.push_str(&format!(" · {}", i18n.tf("file.revision", &[("n", &v.to_string())])));
                }
                row(ui, "file.kind", kind);
                row(
                    ui,
                    "file.game",
                    match b.data_version {
                        Some(v) => i18n.tf("file.data_version", &[("n", &v.to_string())]),
                        None => i18n.t("file.data_version.none").to_string(),
                    },
                );
                row(ui, "file.size", format!("{} × {} × {}", loaded.size[0], loaded.size[1], loaded.size[2]));
                row(ui, "file.regions", group(b.regions.len() as u64));
                row(ui, "file.blocks", i18n.tf("file.blocks.value", &[("blocks", &group(loaded.blocks)), ("air", &group(loaded.air))]));
                row(
                    ui,
                    "file.entities",
                    i18n.tf(
                        "file.entities.value",
                        &[("block", &group(b.block_entity_count() as u64)), ("other", &group(b.entity_count() as u64))],
                    ),
                );
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(i18n.tf("file.kinds", &[("n", &group(loaded.kind_count as u64))])).small().color(theme::text_dim()));
        });
        theme::card().inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().interact_size.y = 16.0;
            ui.spacing_mut().item_spacing.y = 2.0;
            // Two columns, read down the left one first.
            let half = loaded.kinds.len().div_ceil(2);
            ui.columns(2, |columns| {
                for (i, (state, n)) in loaded.kinds.iter().enumerate() {
                    columns[i / half.max(1)].horizontal(|ui| {
                        ui.add_sized([58.0, 16.0], egui::Label::new(RichText::new(group(*n)).small().color(theme::text_dim())).halign(egui::Align::RIGHT));
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new(state.trim_start_matches("minecraft:")).small()).truncate());
                    });
                }
            });
        });
        if !loaded.unknown.is_empty() {
            let more = if loaded.unknown.len() > 6 { ", …" } else { "" };
            let list = loaded.unknown.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
            let version = &catalogue::default().game_version;
            ui.label(
                RichText::new(i18n.tf(
                    "file.unknown",
                    &[("count", &loaded.unknown.len().to_string()), ("list", &format!("{list}{more}")), ("version", version)],
                ))
                .small()
                .color(theme::warn()),
            );
        }
        for note in &loaded.notes {
            ui.label(RichText::new(note_text(i18n, note)).small().color(theme::warn()));
        }
    }

    fn save_options(&mut self, ui: &mut egui::Ui, i18n: &I18n, has_air: bool, accent: Color32, pick_save: &mut bool) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(i18n.t("file.save.heading")).font(theme::semibold(13.0)).color(theme::text_dim()));
        });
        let formats: Vec<(Format, String)> = Format::EXPORTABLE.iter().map(|f| (*f, i18n.t(format_label(*f)).to_string())).collect();
        widgets::segmented(ui, "file_format", &mut self.format, &formats, None, 0.0, &format_tips(i18n));
        let boxing = self.format == Format::Litematic && self.omit_air;
        if self.format == Format::Litematic {
            theme::card().inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(i18n.t("file.omit_air")).on_hover_text(i18n.t("file.omit_air.help"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::toggle(ui, &mut self.omit_air, accent).on_hover_text(i18n.t("file.omit_air.help"));
                    });
                });
            });
        }
        let explain = match (self.format, self.omit_air) {
            (Format::Litematic, true) => "file.explain.omit_air",
            (Format::Litematic, false) if has_air => "file.explain.compact",
            (Format::Litematic, false) => "file.explain.no_air",
            (Format::Sponge, _) => "file.explain.sponge",
            _ => "file.explain.axiom",
        };
        ui.label(RichText::new(i18n.t(explain)).small().color(theme::text_dim())).on_hover_text(i18n.t(&format!("{explain}.help")));

        let mut ready = !self.busy();
        if boxing {
            match &self.boxed {
                Some(boxed) => {
                    let count = boxed.build.regions.len();
                    ui.label(i18n.tf("file.region_count", &[("count", &group(count as u64))]));
                    if count > COMFORTABLE_REGIONS {
                        ui.label(
                            RichText::new(i18n.tf(
                                "file.note.many_regions",
                                &[("count", &group(count as u64)), ("limit", &group(COMFORTABLE_REGIONS as u64))],
                            ))
                            .small()
                            .color(theme::warn()),
                        );
                    }
                }
                None => {
                    ready = false;
                    if self.box_job.is_some() {
                        ui.label(RichText::new(i18n.t("file.counting")).color(theme::text_dim()));
                    }
                }
            }
        }
        ui.add_space(2.0);
        if widgets::primary_button(ui, i18n.t("file.save"), accent, ready).on_hover_text(i18n.t("file.save.help")).clicked() {
            *pick_save = true;
        }
        if let Some(saved) = &self.saved {
            let name = saved.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(
                RichText::new(i18n.tf(
                    "file.saved",
                    &[
                        ("file", &name),
                        ("blocks", &group(saved.blocks)),
                        ("regions", &group(saved.regions as u64)),
                        ("size", &of_core::jobs::coarse_bytes(saved.bytes as u64)),
                    ],
                ))
                .color(theme::good()),
            )
            .on_hover_text(saved.path.display().to_string());
            for note in saved.notes.iter().filter(|n| n.key != "note.many_regions") {
                ui.label(RichText::new(note_text(i18n, note)).small().color(theme::text_dim()));
            }
        }
        ui.add_space(2.0);
    }
}
