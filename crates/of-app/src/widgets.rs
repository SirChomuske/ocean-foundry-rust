//! Hand-drawn controls that give the app its look: slider, switch,
//! segmented control, filled button, status capsule, progress bar.
//!
//! Each one reports itself to screen readers and can be reached and worked
//! from the keyboard, like the stock controls they replace.

use crate::theme;
use eframe::egui::{self, pos2, vec2, Color32, Rect, Response, Sense, Role, Ui, WidgetInfo};

/// A thin track with a round white knob. Values snap to `decimals` places.
/// Left and right arrows nudge by one step once the slider has been clicked.
pub fn slider(ui: &mut Ui, value: &mut f64, min: f64, max: f64, decimals: u32, accent: Color32, width: f32) -> Response {
    const KNOB: f32 = 8.0;
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click_and_drag());
    let (left, right) = (rect.left() + KNOB + 1.0, rect.right() - KNOB - 1.0);
    let scale = 10f64.powi(decimals as i32);
    let snap = |v: f64| ((v * scale).round() / scale).clamp(min, max);
    let before = *value;

    if response.drag_started() || response.clicked() {
        response.request_focus();
    }
    if let Some(p) = response.interact_pointer_pos() {
        let t = ((p.x - left) / (right - left)).clamp(0.0, 1.0) as f64;
        *value = snap(min + t * (max - min));
    }
    if response.has_focus() {
        ui.memory_mut(|m| m.set_focus_lock_filter(response.id, egui::EventFilter { horizontal_arrows: true, ..Default::default() }));
        let (down, up) = ui.input(|i| (i.num_presses(egui::Key::ArrowLeft), i.num_presses(egui::Key::ArrowRight)));
        if down != up {
            *value = snap(*value + (up as f64 - down as f64) / scale);
        }
    }
    if *value != before {
        response.mark_changed();
    }
    let shown = *value;
    response.widget_info(|| WidgetInfo::slider(true, shown, ""));

    if ui.is_rect_visible(rect) {
        let t = if max > min { ((shown - min) / (max - min)).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let x = egui::lerp(left..=right, t);
        let cy = rect.center().y;
        let lively = ui.ctx().animate_bool_responsive(response.id.with("lively"), response.hovered() || response.dragged());
        let p = ui.painter();
        let track = Rect::from_min_max(pos2(rect.left() + 1.0, cy - 2.0), pos2(rect.right() - 1.0, cy + 2.0));
        p.rect_filled(track, 2.0, theme::track());
        p.rect_filled(Rect::from_min_max(track.min, pos2(x, track.bottom())), 2.0, accent);
        if response.has_focus() {
            p.circle_filled(pos2(x, cy), KNOB + 3.5, accent.gamma_multiply(0.35));
        }
        let r = KNOB + 0.6 * lively;
        p.circle_filled(pos2(x, cy + 1.2), r + 0.8, theme::knob_shadow());
        let knob = if theme::is_light() { Color32::WHITE } else { Color32::from_gray(236 + (19.0 * lively) as u8) };
        p.circle_filled(pos2(x, cy), r, knob);
        if theme::is_light() {
            // On a white card the knob needs an edge as well as a shadow.
            p.circle_stroke(pos2(x, cy), r, egui::Stroke::new(1.0, theme::hairline_strong()));
        }
    }
    response
}

/// An on/off switch that slides.
pub fn toggle(ui: &mut Ui, on: &mut bool, accent: Color32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let state = *on;
    response.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, state, ""));
    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, state);
        let p = ui.painter();
        let radius = rect.height() / 2.0;
        if response.has_focus() {
            p.rect_filled(rect.expand(3.0), radius + 3.0, accent.gamma_multiply(0.35));
        }
        let off = if response.hovered() { theme::control_active() } else { theme::track() };
        p.rect_filled(rect, radius, theme::mix(off, accent, how_on));
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        p.circle_filled(pos2(x, rect.center().y + 1.0), radius - 1.6, theme::knob_shadow());
        p.circle_filled(pos2(x, rect.center().y), radius - 2.0, Color32::WHITE);
    }
    response
}

/// A row of choices in one rounded track; the chosen one is a raised pill
/// that slides to its new place. With an accent colour the pill takes it.
/// `tips` holds what each choice shows when the pointer rests on it, in the
/// order of the choices. Returns true when the choice changed.
pub fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: &str,
    current: &mut T,
    options: &[(T, String)],
    accent: Option<Color32>,
    min_width: f32,
    tips: &[String],
) -> bool {
    const HEIGHT: f32 = 28.0;
    const INSET: f32 = 2.0;
    let font = theme::medium(13.5);
    let galleys: Vec<_> =
        options.iter().map(|(_, text)| ui.painter().layout_no_wrap(text.clone(), font.clone(), Color32::PLACEHOLDER)).collect();
    let widths: Vec<f32> = galleys.iter().map(|g| (g.size().x + 26.0).max(min_width)).collect();
    let total = widths.iter().sum::<f32>() + INSET * 2.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, HEIGHT), Sense::hover());
    let id = ui.make_persistent_id(id);
    ui.painter().rect_filled(rect, 8.0, theme::control());

    let mut changed = false;
    let mut x = rect.left() + INSET;
    let mut cells = Vec::with_capacity(options.len());
    for (i, (value, text)) in options.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(x, rect.top() + INSET), vec2(widths[i], HEIGHT - INSET * 2.0));
        let response = ui.interact(cell, id.with(i), Sense::click());
        if response.clicked() && *current != *value {
            *current = *value;
            changed = true;
        }
        let selected = *current == *value;
        response.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, text));
        cells.push((cell, selected, response.hovered(), response.has_focus()));
        if let Some(tip) = tips.get(i).filter(|t| !t.is_empty()) {
            response.on_hover_text(tip);
        }
        x += widths[i];
    }

    let p = ui.painter();
    if let Some((cell, ..)) = cells.iter().find(|c| c.1) {
        // Animate relative to the track, so moving a window does not make
        // the pill trail behind.
        let offset = ui.ctx().animate_value_with_time(id.with("x"), cell.left() - rect.left(), 0.16);
        let width = ui.ctx().animate_value_with_time(id.with("w"), cell.width(), 0.16);
        let pill = Rect::from_min_size(pos2(rect.left() + offset, cell.top()), vec2(width, cell.height()));
        p.rect_filled(pill.translate(vec2(0.0, 1.0)), 6.0, theme::knob_shadow());
        p.rect_filled(pill, 6.0, accent.unwrap_or(theme::segment()));
    }
    for ((cell, selected, hovered, focused), galley) in cells.into_iter().zip(galleys) {
        if focused {
            p.rect_stroke(cell, 6.0, egui::Stroke::new(1.5, theme::text_faint()), egui::StrokeKind::Inside);
        }
        let colour = if selected {
            // White on the accent colour; the neutral pill takes strong text.
            if accent.is_some() { Color32::WHITE } else { theme::text_strong() }
        } else if hovered {
            theme::text()
        } else {
            theme::text_dim()
        };
        p.galley(cell.center() - galley.size() / 2.0, galley, colour);
    }
    changed
}

/// How wide `segmented` would draw these options.
pub fn segmented_width<T>(ui: &Ui, options: &[(T, String)], min_width: f32) -> f32 {
    let font = theme::medium(13.5);
    options.iter().map(|(_, text)| (ui.painter().layout_no_wrap(text.clone(), font.clone(), Color32::PLACEHOLDER).size().x + 26.0).max(min_width)).sum::<f32>()
        + 4.0
}

/// The same choice as `segmented`, as a drop-down for when there is no room
/// for every option side by side.
pub fn choice_box<T: Copy + PartialEq>(ui: &mut Ui, id: &str, current: &mut T, options: &[(T, String)], tips: &[String]) -> bool {
    let before = *current;
    let index = options.iter().position(|(v, _)| *v == *current).unwrap_or(0);
    let shown = options.get(index).map(|(_, t)| t.clone()).unwrap_or_default();
    let response = egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .icon(chevron)
        .show_ui(ui, |ui| {
            for (i, (value, text)) in options.iter().enumerate() {
                let item = ui.selectable_value(current, *value, text);
                if let Some(tip) = tips.get(i).filter(|t| !t.is_empty()) {
                    item.on_hover_text(tip);
                }
            }
        })
        .response;
    if let Some(tip) = tips.get(index).filter(|t| !t.is_empty()) {
        response.on_hover_text(tip);
    }
    *current != before
}

/// The one filled button on a surface: the action the surface is for.
pub fn primary_button(ui: &mut Ui, text: &str, accent: Color32, enabled: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), theme::medium(14.0), Color32::PLACEHOLDER);
    let size = vec2(galley.size().x + 28.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, text));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let (fill, colour) = if !enabled {
            (theme::control(), theme::text_faint())
        } else if response.is_pointer_button_down_on() {
            (theme::mix(accent, Color32::BLACK, 0.18), Color32::WHITE)
        } else if response.hovered() {
            (theme::mix(accent, Color32::WHITE, 0.12), Color32::WHITE)
        } else {
            (accent, Color32::WHITE)
        };
        if response.has_focus() {
            p.rect_filled(rect.expand(3.0), 10.0, accent.gamma_multiply(0.35));
        }
        p.rect_filled(rect, 7.0, fill);
        p.galley(rect.center() - galley.size() / 2.0, galley, colour);
    }
    response
}

/// A small rounded label with a coloured dot, for a state at a glance.
/// It brightens under the pointer when `sense` makes it clickable.
pub fn capsule(ui: &mut Ui, colour: Color32, text: &str, sense: Sense) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), egui::TextStyle::Small.resolve(ui.style()), Color32::PLACEHOLDER);
    let size = galley.size() + vec2(30.0, 9.0);
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if sense.senses_click() {
        response.widget_info(|| WidgetInfo::labeled(Role::Button, true, text));
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let strength = if sense.senses_click() && response.hovered() { 0.26 } else { 0.15 };
        p.rect_filled(rect, rect.height() / 2.0, colour.gamma_multiply(strength));
        p.circle_filled(pos2(rect.left() + 11.5, rect.center().y), 3.5, colour);
        p.galley(pos2(rect.left() + 21.0, rect.center().y - galley.size().y / 2.0), galley, theme::mix(colour, theme::text_strong(), 0.25));
    }
    response
}

/// How wide `capsule` would draw this text.
pub fn capsule_width(ui: &Ui, text: &str) -> f32 {
    ui.painter().layout_no_wrap(text.to_owned(), egui::TextStyle::Small.resolve(ui.style()), Color32::PLACEHOLDER).size().x + 30.0
}

/// A thin progress bar.
pub fn progress(ui: &mut Ui, fraction: f32, accent: Color32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, theme::track());
    let filled = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()));
    p.rect_filled(filled, 2.0, accent);
}

/// The mark on a pop-up menu button: a pair of small chevrons, one up and
/// one down.
pub fn chevron(ui: &Ui, rect: Rect, visuals: &egui::style::WidgetVisuals, _open: bool) {
    let c = rect.center();
    let stroke = egui::Stroke::new(1.5, visuals.fg_stroke.color.gamma_multiply(0.75));
    for dir in [-1.0, 1.0] {
        let tip = pos2(c.x, c.y + dir * 5.0);
        let base = c.y + dir * 2.0;
        ui.painter().line(vec![pos2(c.x - 3.5, base), tip, pos2(c.x + 3.5, base)], stroke);
    }
}

/// A one-pixel divider across the available width.
pub fn hairline(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::hairline());
}
