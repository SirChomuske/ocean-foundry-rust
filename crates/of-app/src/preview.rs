//! The 3D view of the baked build. A background thread draws pictures of
//! it; the viewport shows the latest one and lets it be turned, moved and
//! zoomed. While the view is moving a quick rough picture is drawn, and a
//! clean one follows as soon as it rests.

use crate::files::group;
use crate::theme;
use crate::widgets;
use eframe::egui::{self, Align2, Color32, Rect, RichText, Sense};
use of_core::i18n::I18n;
use of_formats::catalogue;
use of_formats::export::Layer;
use of_ocean::render::{basis, Picture, Scene, View};
use of_ocean::colour::to_oklab;
use of_ocean::{Baked, DRY};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};

/// Which parts of the build the view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layers {
    All,
    NoGlass,
    Foundation,
    /// Only the coloured top blocks.
    Surface,
    /// Only the glass over them.
    Glass,
}

impl Layers {
    pub const ALL: [Layers; 5] = [Layers::All, Layers::NoGlass, Layers::Foundation, Layers::Surface, Layers::Glass];

    pub fn key(self) -> &'static str {
        match self {
            Layers::All => "viewport.layers.all",
            Layers::NoGlass => "viewport.layers.no_glass",
            Layers::Foundation => "viewport.layers.foundation",
            Layers::Surface => "viewport.layers.surface",
            Layers::Glass => "viewport.layers.glass",
        }
    }

    fn layers(self) -> &'static [Layer] {
        match self {
            Layers::All => &Layer::ALL,
            Layers::NoGlass => &[Layer::Terrain, Layer::Foundation, Layer::Surface],
            Layers::Foundation => &[Layer::Terrain, Layer::Foundation],
            Layers::Surface => &[Layer::Surface],
            Layers::Glass => &[Layer::Glass1, Layer::Glass2, Layer::Glass3],
        }
    }
}

/// What the view shows of the build: the build itself, or a map of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Show {
    Build,
    Wanted,
    Blocks,
    Difference,
    Height,
    /// Water depth over the ground.
    Depth,
    /// Where white water was asked for.
    Foam,
    /// Which way the surface faces.
    Normals,
    /// How much sky the surface gives back.
    Reflection,
}

impl Show {
    pub const ALL: [Show; 9] = [
        Show::Build,
        Show::Wanted,
        Show::Blocks,
        Show::Difference,
        Show::Height,
        Show::Depth,
        Show::Foam,
        Show::Normals,
        Show::Reflection,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Show::Build => "viewport.show.build",
            Show::Wanted => "viewport.show.wanted",
            Show::Blocks => "viewport.show.blocks",
            Show::Difference => "viewport.show.difference",
            Show::Height => "viewport.show.height",
            Show::Depth => "viewport.show.depth",
            Show::Foam => "viewport.show.foam",
            Show::Normals => "viewport.show.normals",
            Show::Reflection => "viewport.show.reflection",
        }
    }
}

/// What was asked for in the list of blocks.
pub enum ListAction {
    /// Stop using this block.
    Exclude(String),
    /// Allow every block again.
    Restore,
}

/// The image, or every n-th dot of it each way, so that neither side is
/// longer than the graphics card takes for one texture.
///
/// Why: a texture over the card's limit fails to upload; some cards take
/// no more than 2,048 a side, which a 2,048-column map just reaches (stage 6
/// of the column bake plan).
fn fit_texture(image: egui::ColorImage, most: usize) -> egui::ColorImage {
    let [w, h] = image.size;
    let step = w.max(h).div_ceil(most.max(1));
    if step <= 1 {
        return image;
    }
    let (nw, nh) = (w.div_ceil(step), h.div_ceil(step));
    let dots = (0..nw * nh).map(|i| image.pixels[(i / nw) * step * w + (i % nw) * step]).collect();
    egui::ColorImage::new([nw, nh], dots)
}

/// A map of the build seen from above, one dot per column.
fn map_image(baked: &Baked, show: Show) -> egui::ColorImage {
    let (w, l) = (baked.width.max(1) as usize, baked.length.max(1) as usize);
    let land = Color32::from_rgb(58, 52, 46);
    let wet = baked.water_top.iter().filter(|t| **t != DRY);
    let (low, high) = (wet.clone().min().copied().unwrap_or(0), wet.max().copied().unwrap_or(1));
    let coloured = baked.target.len() == w * l && baked.matched.len() == w * l;
    let mapped = baked.maps.depth.len() == w * l;
    let grey = |t: f32| Color32::from_gray((24.0 + 216.0 * t.clamp(0.0, 1.0)) as u8);
    // The deepest water on the map that has ground under it.
    let deepest = baked.maps.depth.iter().filter(|d| d.is_finite()).fold(1.0f32, |m, d| m.max(*d));
    let dots = (0..w * l)
        .map(|i| {
            if baked.water_top.get(i).is_none_or(|t| *t == DRY) {
                return land;
            }
            let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
            match show {
                Show::Wanted if coloured => rgb(baked.target[i]),
                Show::Blocks if coloured => rgb(baked.matched[i]),
                Show::Difference if coloured => {
                    let (a, b) = (to_oklab(baked.target[i]), to_oklab(baked.matched[i]));
                    let apart = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
                    let t = (apart / 0.1).min(1.0);
                    Color32::from_rgb((22.0 + 233.0 * t) as u8, (26.0 + 190.0 * t * t) as u8, (40.0 * (1.0 - t)) as u8)
                }
                // Shallow is light, deep is dark; with no ground under it, darkest.
                Show::Depth if mapped => {
                    let d = baked.maps.depth[i];
                    if d.is_finite() { grey(1.0 - d / deepest) } else { Color32::from_rgb(14, 18, 30) }
                }
                Show::Foam if mapped => grey(baked.maps.foam[i] as f32 / 255.0),
                Show::Reflection if mapped => grey(baked.maps.sheen[i] as f32 / 255.0),
                // The way each column of water faces, as a normal map would show it.
                Show::Normals => {
                    let h = |x: i64, z: i64| -> f32 {
                        let (x, z) = (x.clamp(0, w as i64 - 1) as usize, z.clamp(0, l as i64 - 1) as usize);
                        let t = baked.water_top[z * w + x];
                        if t == DRY { baked.water_top[i] as f32 } else { t as f32 }
                    };
                    let (x, z) = ((i % w) as i64, (i / w) as i64);
                    let (dx, dz) = ((h(x + 1, z) - h(x - 1, z)) * 0.5, (h(x, z + 1) - h(x, z - 1)) * 0.5);
                    let len = (dx * dx + dz * dz + 1.0).sqrt();
                    let byte = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u8;
                    Color32::from_rgb(byte(-dx / len), byte(dz / len), byte(1.0 / len))
                }
                _ => {
                    let t = (baked.water_top[i] - low) as f32 / (high - low).max(1) as f32;
                    Color32::from_gray((24.0 + 216.0 * t) as u8)
                }
            }
        })
        .collect();
    egui::ColorImage::new([w, l], dots)
}

struct Request {
    generation: u64,
    baked: Arc<Baked>,
    layers: Layers,
    view: View,
}

struct Drawn {
    generation: u64,
    layers: Layers,
    view: View,
    picture: Result<Picture, String>,
}

#[derive(Default)]
struct Slot {
    next: Option<Request>,
    closed: bool,
}

/// The drawing thread. Only the newest request is kept: a view that has
/// already moved on is never drawn.
struct Painter {
    slot: Arc<(Mutex<Slot>, Condvar)>,
    done: Receiver<Drawn>,
}

impl Painter {
    fn new(ctx: &egui::Context) -> Self {
        let slot = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
        let (tx, done) = channel();
        let (theirs, ctx) = (slot.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("preview".into()).spawn(move || {
            // Making a scene takes a moment, so it is kept between pictures.
            let mut scene: Option<(u64, Layers, Result<Scene, String>)> = None;
            loop {
                let request = {
                    let (lock, wake) = &*theirs;
                    let mut slot = lock.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if slot.closed {
                            return;
                        }
                        if let Some(r) = slot.next.take() {
                            break r;
                        }
                        slot = wake.wait(slot).unwrap_or_else(|e| e.into_inner());
                    }
                };
                // Timings go to stderr, which only a test run started from a
                // console or the capped runner ever sees.
                let started = std::time::Instant::now();
                if scene.as_ref().map(|s| (s.0, s.1)) != Some((request.generation, request.layers)) {
                    drop(scene.take());
                    // The scene is read straight from the bake's columns.
                    let made = Scene::new(&request.baked.source(None), request.layers.layers(), catalogue::default());
                    eprintln!("3D view: scene made in {:.2} s", started.elapsed().as_secs_f32());
                    scene = Some((request.generation, request.layers, made));
                }
                let started = std::time::Instant::now();
                let picture = match &scene.as_ref().expect("made above").2 {
                    Ok(s) => Ok(s.draw(&request.view)),
                    Err(e) => Err(e.clone()),
                };
                let v = &request.view;
                eprintln!("3D view: {} x {} picture, {} sample(s), drawn in {:.2} s", v.width, v.height, v.samples, started.elapsed().as_secs_f32());
                let drawn = Drawn { generation: request.generation, layers: request.layers, view: request.view, picture };
                if tx.send(drawn).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        if let Err(e) = spawned {
            log::error!("could not start the drawing thread: {e}");
        }
        Self { slot, done }
    }

    fn ask(&self, request: Request) {
        let (lock, wake) = &*self.slot;
        lock.lock().unwrap_or_else(|e| e.into_inner()).next = Some(request);
        wake.notify_one();
    }

    fn latest(&self) -> Option<Drawn> {
        self.done.try_iter().last()
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        let (lock, wake) = &*self.slot;
        lock.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        wake.notify_one();
    }
}

const START_YAW: f64 = -17.0;
const START_PITCH: f64 = 32.0;
/// Width of the list of blocks, and the narrowest viewport that shows it.
const LIST_WIDTH: f32 = 214.0;
const LIST_FROM: f32 = 560.0;

pub struct Viewer {
    painter: Painter,
    yaw: f64,
    pitch: f64,
    /// Screen points per block; `None` until a build has been framed.
    scale: Option<f64>,
    centre: [f64; 3],
    pub layers: Layers,
    pub show: Show,
    /// The map on screen, and the bake and kind of map it is.
    map: Option<((u64, Show), egui::TextureHandle)>,
    /// Plot size the view was framed for; a different size is framed afresh.
    framed_for: Option<(i32, i32)>,
    texture: Option<egui::TextureHandle>,
    /// What the texture shows.
    shown: Option<(u64, Layers, View)>,
    asked: Option<(u64, Layers, View)>,
    /// Time of the last turn, move or zoom.
    moved_at: f64,
    problem: Option<String>,
    blocks_open: bool,
    /// Cut the build open: everything east of `cut` (a share of its width) is left out.
    pub cutaway: bool,
    pub cut: f64,
}

fn add(a: [f64; 3], b: [f64; 3], k: f64) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + b[i] * k)
}

impl Viewer {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            painter: Painter::new(ctx),
            yaw: START_YAW,
            pitch: START_PITCH,
            scale: None,
            centre: [0.0; 3],
            layers: Layers::All,
            show: Show::Build,
            map: None,
            framed_for: None,
            texture: None,
            shown: None,
            asked: None,
            moved_at: f64::MIN,
            problem: None,
            blocks_open: true,
            cutaway: false,
            cut: 0.5,
        }
    }

    /// Forget the picture, for when there is no longer a build to show.
    pub fn clear(&mut self) {
        self.texture = None;
        self.shown = None;
        self.asked = None;
    }

    /// True once the clean picture of this build is on screen.
    pub fn settled(&self, generation: u64) -> bool {
        match (&self.shown, &self.asked) {
            (Some(shown), Some(asked)) => shown == asked && shown.0 == generation && shown.2.samples > 1,
            _ => self.problem.is_some(),
        }
    }

    /// Turn the whole build to face the viewer and fit it in `rect`.
    fn frame(&mut self, baked: &Baked, rect: Rect) {
        let (w, l) = (baked.width as f64, baked.length as f64);
        let top = (baked.sea_level + baked.stats.range.1 + 4).max(1) as f64;
        self.yaw = START_YAW;
        self.pitch = START_PITCH;
        let yaw = self.yaw.to_radians();
        let (_, right, up) = basis((yaw.sin(), yaw.cos()), self.pitch);
        let across = w * right[0].abs() + l * right[2].abs();
        let tall = w * up[0].abs() + l * up[2].abs() + top * up[1].abs();
        // The list of blocks covers the right edge, so the build is fitted beside it.
        let covered = if rect.width() > LIST_FROM { LIST_WIDTH as f64 + 22.0 } else { 0.0 };
        let fit = ((rect.width() as f64 - covered).max(80.0) / across.max(1.0)).min((rect.height() as f64 - 70.0).max(80.0) / tall.max(1.0));
        let scale = (0.92 * fit).clamp(0.05, 64.0);
        self.scale = Some(scale);
        self.centre = add([w / 2.0, top / 2.0, l / 2.0], right, covered / 2.0 / scale);
        self.framed_for = Some((baked.width, baked.length));
    }

    /// Draw the view of `baked` into `rect` and handle the pointer.
    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, baked: &Arc<Baked>, generation: u64, i18n: &I18n, excluded: &[String]) -> Option<ListAction> {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        if self.framed_for != Some((baked.width, baked.length)) || self.scale.is_none() {
            self.frame(baked, rect);
        }

        if let Some(drawn) = self.painter.latest() {
            match drawn.picture {
                Ok(picture) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([picture.width as usize, picture.height as usize], &picture.rgba);
                    let image = fit_texture(image, ctx.input(|i| i.max_texture_side));
                    match &mut self.texture {
                        Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                        None => self.texture = Some(ctx.load_texture("ocean-view", image, egui::TextureOptions::LINEAR)),
                    }
                    self.shown = Some((drawn.generation, drawn.layers, drawn.view));
                    self.problem = None;
                }
                Err(e) => self.problem = Some(e),
            }
        }

        // ---- Pointer: turn, move, zoom, reset.
        let response = ui.interact(rect, ui.scope_id().with("ocean_view"), Sense::click_and_drag());
        let mut scale = self.scale.unwrap_or(1.0);
        let yaw = self.yaw.to_radians();
        let (_, right, up) = basis((yaw.sin(), yaw.cos()), self.pitch);
        if response.double_clicked() {
            self.frame(baked, rect);
            scale = self.scale.unwrap_or(scale);
            self.moved_at = now;
        } else if response.dragged() {
            let d = response.drag_delta();
            let shift = ui.input(|i| i.modifiers.shift);
            if response.dragged_by(egui::PointerButton::Primary) && !shift {
                self.yaw = (self.yaw - d.x as f64 * 0.4).rem_euclid(360.0);
                self.pitch = (self.pitch + d.y as f64 * 0.3).clamp(8.0, 90.0);
            } else {
                self.centre = add(add(self.centre, right, -(d.x as f64) / scale), up, d.y as f64 / scale);
            }
            if d != egui::Vec2::ZERO {
                self.moved_at = now;
            }
        }
        if response.hovered() {
            let (wheel, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = (wheel as f64 * 0.0022).exp() * pinch as f64;
            if (factor - 1.0).abs() > 1e-4 {
                let grown = (scale * factor).clamp(0.05, 64.0);
                // Keep the block under the pointer where it is.
                let o = response.hover_pos().unwrap_or(rect.center()) - rect.center();
                let k = 1.0 / scale - 1.0 / grown;
                self.centre = add(add(self.centre, right, o.x as f64 * k), up, -(o.y as f64) * k);
                scale = grown;
                self.moved_at = now;
            }
        }
        self.scale = Some(scale);

        // ---- Ask for the picture this view needs.
        let moving = now - self.moved_at < 0.18 || response.dragged();
        let ppp = ctx.pixels_per_point() as f64;
        let sharp = if moving { ppp * 0.5 } else { ppp };
        let (pw, ph) = ((rect.width() as f64 * sharp).round().clamp(16.0, 4096.0), (rect.height() as f64 * sharp).round().clamp(16.0, 4096.0));
        let samples = if moving || pw * ph > 5.0e6 { 1 } else { 2 };
        let cut = self.cutaway.then(|| self.cut * baked.width.max(1) as f64);
        let view = View { yaw: self.yaw, pitch: self.pitch, scale: scale * sharp, centre: self.centre, width: pw as u32, height: ph as u32, samples, cut };
        let wanted = (generation, self.layers, view);
        if self.asked.as_ref() != Some(&wanted) {
            self.painter.ask(Request { generation, baked: baked.clone(), layers: self.layers, view: wanted.2.clone() });
            self.asked = Some(wanted);
        }
        if moving {
            // Come back for the clean picture once the view rests.
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        if let Some(texture) = &self.texture {
            let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().with_clip_rect(rect).image(texture.id(), rect, uv, Color32::WHITE);
        }

        // ---- A map instead of the build.
        if self.show != Show::Build {
            let key = (generation, self.show);
            if self.map.as_ref().map(|m| m.0) != Some(key) {
                let image = fit_texture(map_image(baked, self.show), ctx.input(|i| i.max_texture_side));
                self.map = Some((key, ctx.load_texture("ocean-map", image, egui::TextureOptions::NEAREST)));
            }
            let p = ui.painter().with_clip_rect(rect);
            p.rect_filled(rect, 0.0, theme::bg());
            let covered = if rect.width() > LIST_FROM { LIST_WIDTH + 22.0 } else { 0.0 };
            let room = Rect::from_min_max(rect.min + egui::vec2(16.0, 92.0), rect.max - egui::vec2(covered + 16.0, 62.0));
            let k = (room.width() / baked.width.max(1) as f32).min(room.height() / baked.length.max(1) as f32).max(0.01);
            let place = Rect::from_center_size(room.center(), egui::vec2(baked.width as f32 * k, baked.length as f32 * k));
            if let Some((_, texture)) = &self.map {
                let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                p.image(texture.id(), place, uv, Color32::WHITE);
            }
        } else {
            self.map = None;
        }

        self.overlays(ui, rect, baked, i18n, excluded)
    }

    fn overlays(&mut self, ui: &mut egui::Ui, rect: Rect, baked: &Baked, i18n: &I18n, excluded: &[String]) -> Option<ListAction> {
        let mut action = None;
        // ---- Layer choice, top left.
        // The rows keep clear of the compass at the top right; a row with
        // no room for every option side by side becomes a drop-down.
        let room = (rect.width() - 28.0 - 64.0).max(120.0);
        let corner = Rect::from_min_size(rect.left_top() + egui::vec2(14.0, 12.0), egui::vec2(room, 30.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(corner), |ui| {
            let options: Vec<(Layers, String)> = Layers::ALL.iter().map(|l| (*l, i18n.t(l.key()).to_string())).collect();
            let tips: Vec<String> = Layers::ALL.iter().map(|l| i18n.t(&format!("{}.help", l.key())).to_string()).collect();
            if widgets::segmented_width(ui, &options, 0.0) <= room {
                widgets::segmented(ui, "view_layers", &mut self.layers, &options, None, 0.0, &tips);
            } else {
                widgets::choice_box(ui, "view_layers_box", &mut self.layers, &options, &tips);
            }
        });
        // ---- What to show, under it.
        let corner = Rect::from_min_size(rect.left_top() + egui::vec2(14.0, 48.0), egui::vec2(room, 30.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(corner), |ui| {
            let options: Vec<(Show, String)> = Show::ALL.iter().map(|s| (*s, i18n.t(s.key()).to_string())).collect();
            let tips: Vec<String> = Show::ALL.iter().map(|s| i18n.t(&format!("{}.help", s.key())).to_string()).collect();
            if widgets::segmented_width(ui, &options, 0.0) <= room {
                widgets::segmented(ui, "view_show", &mut self.show, &options, None, 0.0, &tips);
            } else {
                widgets::choice_box(ui, "view_show_box", &mut self.show, &options, &tips);
            }
        });
        // ---- Cut the build open, under that, for the build itself.
        if self.show == Show::Build {
            let corner = Rect::from_min_size(rect.left_top() + egui::vec2(14.0, 84.0), egui::vec2(300.0, 26.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(corner), |ui| {
                ui.horizontal(|ui| {
                    let accent = theme::accent(of_core::params::Mode::Ocean);
                    let tip = i18n.t("viewport.cutaway.help");
                    widgets::toggle(ui, &mut self.cutaway, accent).on_hover_text(tip);
                    ui.label(RichText::new(i18n.t("viewport.cutaway")).color(theme::text_dim())).on_hover_text(tip);
                    if self.cutaway {
                        widgets::slider(ui, &mut self.cut, 0.0, 1.0, 2, accent, 160.0).on_hover_text(tip);
                    }
                });
            });
        }

        // ---- Compass, top right: the needle points north (-Z).
        let yaw = self.yaw.to_radians();
        let (_, right, up) = basis((yaw.sin(), yaw.cos()), self.pitch);
        let north = egui::vec2(-right[2] as f32, up[2] as f32);
        let north = if north.length() > 1e-4 { north.normalized() } else { egui::vec2(0.0, -1.0) };
        let hub = rect.right_top() + egui::vec2(-36.0, 34.0);
        let p = ui.painter().with_clip_rect(rect);
        p.circle(hub, 20.0, theme::elevated().gamma_multiply(0.9), egui::Stroke::new(1.0, theme::hairline_strong()));
        let side = egui::vec2(-north.y, north.x);
        p.add(egui::Shape::convex_polygon(
            vec![hub + north * 13.0, hub + side * 4.0, hub - side * 4.0],
            theme::bad(),
            egui::Stroke::NONE,
        ));
        p.add(egui::Shape::convex_polygon(
            vec![hub - north * 13.0, hub + side * 4.0, hub - side * 4.0],
            theme::text_faint(),
            egui::Stroke::NONE,
        ));
        p.text(hub + north * 30.0, Align2::CENTER_CENTER, i18n.t("viewport.north"), theme::semibold(11.5), theme::text_dim());

        // ---- Blocks used, under the compass.
        let width = LIST_WIDTH;
        let area = Rect::from_min_size(rect.right_top() + egui::vec2(-width - 14.0, 78.0), egui::vec2(width, (rect.height() - 150.0).max(60.0)));
        if rect.width() > LIST_FROM {
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| action = self.block_list(ui, baked, i18n, width, excluded));
        }

        // ---- Facts and hint, bottom left.
        let stats = &baked.stats;
        let facts = i18n.tf(
            "viewport.stats",
            &[
                ("width", &baked.width.to_string()),
                ("length", &baked.length.to_string()),
                ("low", &format!("{:+}", stats.range.0)),
                ("high", &format!("{:+}", stats.range.1)),
                ("kinds", &stats.used.len().to_string()),
                ("blocks", &group(stats.blocks.iter().sum())),
            ],
        );
        let p = ui.painter().with_clip_rect(rect);
        let small = egui::TextStyle::Small.resolve(ui.style());
        let at = rect.left_bottom() + egui::vec2(16.0, -14.0);
        let hint = p.layout_no_wrap(i18n.t("viewport.hint").to_string(), small.clone(), theme::text_faint());
        let facts = p.layout_no_wrap(facts, small, theme::text_dim());
        let size = egui::vec2(facts.size().x.max(hint.size().x), facts.size().y + hint.size().y + 3.0);
        let plate = Rect::from_min_size(at - egui::vec2(0.0, size.y), size).expand2(egui::vec2(9.0, 6.0));
        p.rect_filled(plate, 9.0, theme::bg().gamma_multiply(0.72));
        p.galley(at - egui::vec2(0.0, size.y), facts, theme::text_dim());
        p.galley(at - egui::vec2(0.0, hint.size().y), hint, theme::text_faint());

        if let Some(problem) = &self.problem {
            p.text(
                rect.center_bottom() - egui::vec2(0.0, 70.0),
                Align2::CENTER_CENTER,
                i18n.tf("viewport.failed", &[("error", problem)]),
                egui::TextStyle::Body.resolve(ui.style()),
                theme::bad(),
            );
        }
        action
    }

    fn block_list(&mut self, ui: &mut egui::Ui, baked: &Baked, i18n: &I18n, width: f32, excluded: &[String]) -> Option<ListAction> {
        let mut action = None;
        const SHOWN: usize = 14;
        let catalogue = catalogue::default();
        theme::floating().shadow(egui::Shadow::NONE).fill(theme::elevated().gamma_multiply(0.94)).inner_margin(egui::Margin::symmetric(12, 8)).show(
            ui,
            |ui| {
                ui.set_width(width - 26.0);
                ui.spacing_mut().item_spacing.y = 3.0;
                let title = ui.add(
                    egui::Label::new(RichText::new(i18n.t("viewport.blocks")).font(theme::semibold(12.5)).color(theme::text_dim()))
                        .selectable(false)
                        .sense(Sense::click()),
                );
                if title.on_hover_text(i18n.t("viewport.blocks.help")).clicked() {
                    self.blocks_open = !self.blocks_open;
                }
                if !self.blocks_open {
                    return;
                }
                for (name, count) in baked.stats.used.iter().take(SHOWN) {
                    let row = ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        let (swatch, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
                        let info = catalogue.blocks.get(name);
                        let rgb = info.and_then(|i| i.top).unwrap_or([255, 0, 255]);
                        let alpha = info.and_then(|i| i.alpha).unwrap_or(255);
                        let fill = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                        ui.painter().rect_filled(swatch, 3.0, if alpha < 255 { fill.gamma_multiply(0.62) } else { fill });
                        ui.painter().rect_stroke(swatch, 3.0, egui::Stroke::new(1.0, theme::hairline_strong()), egui::StrokeKind::Inside);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(group(*count)).small().color(theme::text_dim()));
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.add(egui::Label::new(RichText::new(name.trim_start_matches("minecraft:")).small()).truncate());
                            });
                        });
                    });
                    // A click on a block leaves it out of the next bake.
                    if row.response.interact(Sense::click()).on_hover_text(i18n.t("viewport.blocks.exclude")).clicked() {
                        action = Some(ListAction::Exclude(name.clone()));
                    }
                }
                if baked.stats.used.len() > SHOWN {
                    let more = (baked.stats.used.len() - SHOWN).to_string();
                    ui.label(RichText::new(i18n.tf("viewport.blocks.more", &[("count", &more)])).small().color(theme::text_faint()));
                }
                if !excluded.is_empty() {
                    let text = i18n.tf("viewport.blocks.excluded", &[("count", &excluded.len().to_string())]);
                    let restore = ui.add(egui::Label::new(RichText::new(text).small().color(theme::text_dim())).selectable(false).sense(Sense::click()));
                    let names: Vec<&str> = excluded.iter().map(|n| n.trim_start_matches("minecraft:")).collect();
                    if restore.on_hover_text(format!("{}\n{}", i18n.t("viewport.blocks.restore"), names.join(", "))).clicked() {
                        action = Some(ListAction::Restore);
                    }
                }
            },
        );
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_texture_over_the_cards_limit_is_thinned_to_fit() {
        let dots: Vec<Color32> = (0..5000 * 3).map(|i| Color32::from_gray((i % 251) as u8)).collect();
        let image = egui::ColorImage::new([5000, 3], dots.clone());
        let fitted = fit_texture(image.clone(), 2048);
        assert_eq!(fitted.size, [1667, 1]);
        assert_eq!(fitted.pixels[1], dots[3]);
        assert_eq!(fit_texture(image, 8192).size, [5000, 3]);
    }
}
