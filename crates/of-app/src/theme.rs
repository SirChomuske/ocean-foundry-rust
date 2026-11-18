//! Look and feel: fonts, colours and spacing.
//!
//! The look is a quiet "pro app" one, in a dark and a light version: soft
//! raised cards with hairline edges, one accent colour, and weight rather
//! than colour for emphasis. Every colour is read through a function, so
//! the whole app follows the active version.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke, TextStyle};
use of_core::params::Mode;
use of_core::settings::Appearance;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Inter draws Latin text and digits: a typeface made for screens, with an
/// optical-size axis that tightens it at heading sizes.
/// Licence: SIL Open Font License 1.1 (see assets/fonts/Inter-OFL.txt).
static INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-VF.ttf");
/// Noto Sans SC draws Simplified Chinese, in matching weights. Both fonts
/// are embedded so the app needs no installed fonts.
/// Licence: SIL Open Font License 1.1 (see assets/fonts/NOTICE.txt).
static NOTO_SANS_SC: &[u8] = include_bytes!("../../../assets/fonts/NotoSansSC-VF.ttf");

/// True while the light version is showing. Set by [`apply`].
static LIGHT: AtomicBool = AtomicBool::new(false);

pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

struct Palette {
    bg: Color32,
    panel: Color32,
    card: Color32,
    elevated: Color32,
    field: Color32,
    hairline: Color32,
    hairline_strong: Color32,
    control: Color32,
    control_hover: Color32,
    control_active: Color32,
    track: Color32,
    segment: Color32,
    text: Color32,
    text_strong: Color32,
    text_dim: Color32,
    text_faint: Color32,
    good: Color32,
    warn: Color32,
    bad: Color32,
    knob_shadow: Color32,
}

const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// White at low strength: sits correctly on any dark surface.
const fn white(a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(a, a, a, a)
}

/// Black at low strength: sits correctly on any light surface.
const fn black(a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(0, 0, 0, a)
}

const DARK: Palette = Palette {
    bg: rgb(0x0e0e10),
    panel: rgb(0x1b1b1d),
    card: rgb(0x262629),
    elevated: rgb(0x2a2a2d),
    field: rgb(0x151517),
    hairline: white(22),
    hairline_strong: white(34),
    control: white(46),
    control_hover: white(62),
    control_active: white(78),
    track: white(40),
    segment: rgb(0x5c5c61),
    text: rgb(0xeaeaef),
    text_strong: rgb(0xffffff),
    text_dim: rgb(0x98989f),
    text_faint: rgb(0x636368),
    good: rgb(0x30d158),
    warn: rgb(0xffd60a),
    bad: rgb(0xff5a4f),
    knob_shadow: black(70),
};

const LIGHT_PALETTE: Palette = Palette {
    bg: rgb(0xe4e4e9),
    panel: rgb(0xf2f2f6),
    card: rgb(0xffffff),
    elevated: rgb(0xf6f6f9),
    field: rgb(0xffffff),
    hairline: black(20),
    hairline_strong: black(36),
    control: black(34),
    control_hover: black(48),
    control_active: black(62),
    track: black(34),
    segment: rgb(0xffffff),
    text: rgb(0x1d1d1f),
    text_strong: rgb(0x000000),
    text_dim: rgb(0x6e6e73),
    text_faint: rgb(0xa1a1a6),
    good: rgb(0x1f8f3a),
    warn: rgb(0xb26a00),
    bad: rgb(0xd92d20),
    knob_shadow: black(46),
};

/// The fill behind the top bar's buttons: opaque, so the bar reads as
/// controls rather than as letters floating on the window.
pub(crate) fn bar_control() -> Color32 {
    if is_light() {
        rgb(0xd9d9df)
    } else {
        rgb(0x3d3d43)
    }
}

/// The same, under the pointer.
pub(crate) fn bar_control_hover() -> Color32 {
    if is_light() {
        rgb(0xcdcdd5)
    } else {
        rgb(0x4a4a51)
    }
}

/// The same, held down or open.
pub(crate) fn bar_control_active() -> Color32 {
    if is_light() {
        rgb(0xc0c0ca)
    } else {
        rgb(0x56565e)
    }
}

fn palette() -> &'static Palette {
    if is_light() {
        &LIGHT_PALETTE
    } else {
        &DARK
    }
}

macro_rules! colours {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        $($(#[$doc])* pub fn $name() -> Color32 { palette().$name })*
    };
}

colours! {
    /// The canvas behind the preview.
    bg,
    /// Toolbar, side panel and status bar.
    panel,
    /// Floating surfaces: windows, menus and the help card.
    elevated,
    /// One-pixel edges and dividers.
    hairline,
    hairline_strong,
    /// Fill of a resting control. Hovered and pressed states are set in [`apply`].
    control,
    control_active,
    /// The empty part of a slider or progress bar, and a switch that is off.
    track,
    /// The raised part of a neutral segmented control.
    segment,
    text,
    /// Names and whatever the pointer is on.
    text_strong,
    text_dim,
    text_faint,
    good,
    warn,
    bad,
    /// The soft shadow under a knob or raised pill.
    knob_shadow,
}

/// Accent colour follows the mode, so it is always obvious which one is active.
pub fn accent(mode: Mode) -> Color32 {
    match (mode, is_light()) {
        (Mode::Ocean, false) => rgb(0x0a84ff),
        (Mode::Ocean, true) => rgb(0x007aff),
        #[cfg(feature = "dev")]
        (Mode::Lava, false) => rgb(0xff6a1f),
        #[cfg(feature = "dev")]
        (Mode::Lava, true) => rgb(0xf2590d),
    }
}

/// Blend two colours; `t` = 0 gives `a`, 1 gives `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()), f(a.a(), b.a()))
}

/// Slightly heavier text, for control labels that carry a state.
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

/// Headings and names.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// Large titles: semibold, drawn with the font's tighter display shapes.
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}

/// A grouped card: soft fill, hairline edge, generous corners.
pub fn card() -> egui::Frame {
    egui::Frame::new().fill(card_fill()).stroke(Stroke::new(1.0, hairline())).corner_radius(CornerRadius::same(12))
}

fn card_fill() -> Color32 {
    palette().card
}

fn depth(dark: u8, light: u8) -> Color32 {
    Color32::from_black_alpha(if is_light() { light } else { dark })
}

/// A floating surface with a soft, deep shadow.
pub fn floating() -> egui::Frame {
    egui::Frame::new()
        .fill(elevated())
        .stroke(Stroke::new(1.0, hairline_strong()))
        .corner_radius(CornerRadius::same(12))
        .shadow(Shadow { offset: [0, 10], blur: 32, spread: 0, color: depth(130, 44) })
}

pub fn install_fonts(ctx: &egui::Context) {
    use egui::epaint::text::{FontTweak, VariationCoords};
    let mut fonts = FontDefinitions::default();
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    // Each variable font file serves several weights. `opsz` is Inter's
    // optical size: 14 for text, 32 for large titles.
    let faces: [(&str, f32, f32, Option<&str>); 4] =
        [("regular", 400.0, 14.0, None), ("medium", 510.0, 14.0, Some("medium")), ("semibold", 600.0, 14.0, Some("semibold")), ("display", 620.0, 32.0, Some("display"))];
    for (name, weight, optical, family) in faces {
        let inter = format!("inter_{name}");
        let noto = format!("noto_sans_sc_{name}");
        let tweak = FontTweak { coords: VariationCoords::new([(b"wght", weight), (b"opsz", optical)]), ..Default::default() };
        fonts.font_data.insert(inter.clone(), Arc::new(FontData::from_static(INTER).tweak(tweak)));
        // Noto's built-in default weight is Thin, so its weight is always
        // chosen explicitly. Chinese reads lighter than Latin at the same
        // weight, so it is set a touch heavier.
        let tweak = FontTweak { coords: VariationCoords::new([(b"wght", weight + 20.0)]), ..Default::default() };
        fonts.font_data.insert(noto.clone(), Arc::new(FontData::from_static(NOTO_SANS_SC).tweak(tweak)));
        match family {
            Some(family) => {
                // Symbols neither font has fall back to the built-in fonts.
                let mut list = vec![inter, noto];
                list.extend(fallback.iter().cloned());
                fonts.families.insert(FontFamily::Name(family.into()), list);
            }
            None => {
                let list = fonts.families.entry(FontFamily::Proportional).or_default();
                list.insert(0, noto.clone());
                list.insert(0, inter);
                // Keep the monospace design for digits, with Chinese as fallback.
                fonts.families.entry(FontFamily::Monospace).or_default().push(noto);
            }
        }
    }
    ctx.set_fonts(fonts);
}

/// Set the style for this mode and appearance. Returns which version
/// (dark or light) is now showing.
pub fn apply(ctx: &egui::Context, mode: Mode, appearance: Appearance) -> egui::Theme {
    ctx.set_theme(match appearance {
        Appearance::Auto => egui::ThemePreference::System,
        Appearance::Light => egui::ThemePreference::Light,
        Appearance::Dark => egui::ThemePreference::Dark,
    });
    let theme = ctx.theme();
    let light = theme == egui::Theme::Light;
    LIGHT.store(light, Ordering::Relaxed);
    // The window's own title bar follows too.
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match appearance {
        Appearance::Auto => egui::SystemTheme::SystemDefault,
        Appearance::Light => egui::SystemTheme::Light,
        Appearance::Dark => egui::SystemTheme::Dark,
    }));

    let accent = accent(mode);
    let p = palette();
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, semibold(15.5)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 8.0);
        s.button_padding = egui::vec2(12.0, 5.0);
        s.interact_size.y = 28.0;
        s.combo_width = 150.0;
        s.window_margin = egui::Margin::same(18);
        s.menu_margin = egui::Margin::same(6);
        s.icon_width = 18.0;
        s.icon_width_inner = 9.0;
        // Thin scroll bars that float over the content and fade when idle.
        s.scroll = egui::style::ScrollStyle::floating();

        let v = &mut style.visuals;
        v.dark_mode = !light;
        v.override_text_color = Some(p.text);
        v.panel_fill = p.panel;
        v.window_fill = p.elevated;
        v.extreme_bg_color = p.field;
        v.faint_bg_color = p.control;
        v.window_stroke = Stroke::new(1.0, p.hairline_strong);
        v.window_corner_radius = CornerRadius::same(14);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_shadow = Shadow { offset: [0, 18], blur: 48, spread: 0, color: depth(150, 52) };
        v.popup_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: depth(120, 40) };
        v.selection.bg_fill = accent;
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.hyperlink_color = accent;

        let radius = CornerRadius::same(7);
        let w = &mut v.widgets;
        w.noninteractive.corner_radius = radius;
        w.noninteractive.bg_fill = p.elevated;
        w.noninteractive.weak_bg_fill = p.elevated;
        w.noninteractive.bg_stroke = Stroke::new(1.0, p.hairline);
        w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
        // Controls are soft filled shapes with no outline; they deepen
        // under the pointer instead of gaining a border.
        for (state, fill, text) in [
            (&mut w.inactive, p.control, p.text),
            (&mut w.hovered, p.control_hover, p.text_strong),
            (&mut w.active, p.control_active, p.text_strong),
            (&mut w.open, p.control_hover, p.text_strong),
        ] {
            state.corner_radius = radius;
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke = Stroke::new(1.5, text);
            state.expansion = 0.0;
        }
        // Radio buttons are drawn from `bg_fill`; keep them visible at rest.
        w.inactive.bg_fill = p.control_hover;
    });
    theme
}
