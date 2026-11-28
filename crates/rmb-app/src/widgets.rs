// SPDX-License-Identifier: LGPL-2.1-or-later

//! Small reusable widgets. Every value control resets to its default on double-click; while
//! dragging, Shift makes it 10 times and Ctrl 100 times finer (`fine_drag`).

use egui::{Color32, Response, RichText, Ui, vec2};

use crate::theme;

/// Two-column grid of label + control rows.
pub fn grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([10.0, 6.0]).min_col_width(96.0).show(ui, add);
}

pub fn label(ui: &mut Ui, text: &str, tip: &str) {
    let r = ui.label(RichText::new(text).color(theme::TEXT_MUTED));
    if !tip.is_empty() {
        r.on_hover_text(tip);
    }
}

/// A section header: open or closed, and whether its reset button was clicked.
pub struct Section {
    pub open: bool,
    pub reset: bool,
}

/// Keys of the closed sections (kept in the preferences, see [`set_closed_sections`]).
static CLOSED: std::sync::Mutex<std::collections::BTreeSet<String>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Sets the closed sections (from the preferences, at start).
pub fn set_closed_sections(keys: &[String]) {
    if let Ok(mut c) = CLOSED.lock() {
        *c = keys.iter().cloned().collect();
    }
}

/// The closed sections (to keep in the preferences).
pub fn closed_sections() -> Vec<String> {
    CLOSED.lock().map(|c| c.iter().cloned().collect()).unwrap_or_default()
}

/// Section header, a click opens or closes the section, with an optional reset button. The
/// caller draws the content only when `open`.
pub fn section(ui: &mut Ui, text: &str, reset: bool) -> Section {
    let key = format!("{:016x}", ui.id().with(("section", text)).value());
    let mut open = CLOSED.lock().map(|c| !c.contains(&key)).unwrap_or(true);
    let mut clicked = false;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let arrow = if open { crate::icons::CARET_DOWN } else { crate::icons::CARET_RIGHT };
        let header = RichText::new(format!("{arrow}  {}", text.to_uppercase())).size(theme::fs(10.5)).color(theme::TEXT_MUTED).strong();
        let r = ui.add(egui::Label::new(header).sense(egui::Sense::click())).on_hover_text(if open { "Click to close this section" } else { "Click to open this section" });
        if r.clicked() {
            open = !open;
            if let Ok(mut c) = CLOSED.lock() {
                if open {
                    c.remove(&key);
                } else {
                    c.insert(key.clone());
                }
            }
        }
        if reset && open {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = ui
                    .add(egui::Button::new(RichText::new(crate::icons::REFRESH).size(theme::fs(11.0))).small().frame(false))
                    .on_hover_text(format!("Reset {} to the defaults", text.to_lowercase()))
                    .clicked();
            });
        }
    });
    ui.add_space(2.0);
    Section { open, reset: clicked }
}

pub fn reset_on_double_click<T: Copy + PartialEq>(mut r: Response, v: &mut T, default: T) -> Response {
    let on_widget = r.ctx.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary) && i.pointer.interact_pos().is_some_and(|p| r.rect.contains(p)));
    if (r.double_clicked() || on_widget) && *v != default {
        *v = default;
        r.mark_changed();
    }
    r
}

/// How much finer a drag moves values: Shift 10 times, Ctrl 100 times (`None`: normal).
pub fn fine_factor(ui: &Ui) -> Option<f64> {
    ui.input(|i| {
        if i.modifiers.command {
            Some(0.01)
        } else if i.modifiers.shift {
            Some(0.1)
        } else {
            None
        }
    })
}

/// A value as text: up to four decimals from 1 on, four significant digits below, without
/// trailing zeros (at least one decimal).
pub fn fmt_value(v: f64) -> String {
    let a = v.abs();
    let dec = if a >= 1.0 || a == 0.0 { 4 } else { (3 - a.log10().floor() as i32).clamp(4, 9) as usize };
    let mut s = format!("{v:.dec$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.push('0');
        }
    }
    s
}

/// Slider position (0..1) of `v` in `lo..=hi`: linear, or logarithmic (equal ratios; a range
/// from 0 starts with a short linear part so 0 stays reachable).
fn slider_t(v: f64, lo: f64, hi: f64, log: bool) -> f64 {
    let t = if !log || hi <= 0.0 || lo < 0.0 {
        (v - lo) / (hi - lo)
    } else if lo > 0.0 {
        (v.max(lo * 1e-9) / lo).ln() / (hi / lo).ln()
    } else {
        let e = hi * 1e-3;
        (1.0 + v.max(0.0) / e).ln() / (1.0 + hi / e).ln()
    };
    if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 }
}

/// The value at slider position `t` (inverse of [`slider_t`]).
fn slider_value(t: f64, lo: f64, hi: f64, log: bool) -> f64 {
    if !log || hi <= 0.0 || lo < 0.0 {
        lo + (hi - lo) * t
    } else if lo > 0.0 {
        lo * (hi / lo).powf(t)
    } else {
        let e = hi * 1e-3;
        e * ((1.0 + hi / e).powf(t) - 1.0)
    }
}

/// State of a slider drag: relative (fine) once Shift or Ctrl was held during it.
#[derive(Clone, Copy)]
struct SliderDrag {
    relative: bool,
}

/// A slider that draws like egui's, with fine adjustment: a plain drag puts the value under
/// the pointer (aiming at round values); with Shift (10 times) or Ctrl (100 times finer) held
/// the drag moves the value relative to where it is, and stays relative until the button is
/// released, so letting go of the key never makes the value jump. The knob and the number
/// always show the value. Values may lie outside the range (typed); the knob then rests at
/// the end.
pub fn fine_slider(ui: &mut Ui, v: &mut f64, range: std::ops::RangeInclusive<f64>, log: bool, suffix: &str) -> Response {
    let (lo, hi) = (*range.start(), *range.end());
    ui.horizontal(|ui| {
        let size = vec2(ui.spacing().slider_width, ui.spacing().interact_size.y);
        let (rect, mut r) = ui.allocate_exact_size(size, egui::Sense::drag());
        let radius = rect.height() / 2.5;
        let x_range = rect.x_range().shrink(radius);
        let fine = fine_factor(ui);
        let id = r.id;
        if r.is_pointer_button_down_on() || r.dragged() {
            let mut st: SliderDrag = ui.data(|d| d.get_temp(id)).unwrap_or(SliderDrag { relative: false });
            if r.drag_started() || r.is_pointer_button_down_on() && !r.dragged() {
                // A new press: relative from the start when a modifier is held.
                st.relative = fine.is_some();
            }
            st.relative |= fine.is_some();
            let new = if st.relative {
                let dx = r.drag_delta().x as f64;
                let k = fine.unwrap_or(1.0);
                let t = slider_t(*v, lo, hi, log) + dx / x_range.span().max(1.0) as f64 * k;
                // Outside the range (typed values), move linearly from where the value is.
                if (lo..=hi).contains(v) { slider_value(t.clamp(0.0, 1.0), lo, hi, log) } else { *v + (hi - lo) * dx / x_range.span().max(1.0) as f64 * k }
            } else if let Some(p) = r.interact_pointer_pos() {
                let at = |x: f32| slider_value(egui::emath::remap_clamp(x, x_range, 0.0..=1.0) as f64, lo, hi, log);
                let aim = ui.input(|i| i.aim_radius());
                egui::emath::smart_aim::best_in_range_f64(at(p.x - aim), at(p.x + aim))
            } else {
                *v
            };
            ui.data_mut(|d| d.insert_temp(id, st));
            if new != *v && new.is_finite() {
                *v = new;
                r.mark_changed();
            }
        }
        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact(&r);
            let wv = &ui.visuals().widgets;
            let rail_r = (ui.spacing().slider_rail_height / 2.0).max(0.0);
            let rail = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.center().y - rail_r), egui::pos2(rect.right(), rect.center().y + rail_r));
            let corner = wv.inactive.corner_radius;
            ui.painter().rect_filled(rail, corner, wv.inactive.bg_fill);
            let cx = egui::emath::lerp(x_range, slider_t(*v, lo, hi, log) as f32);
            if ui.visuals().slider_trailing_fill {
                let mut fill = rail;
                fill.max.x = cx + corner.nw as f32;
                ui.painter().rect_filled(fill, corner, ui.visuals().selection.bg_fill);
            }
            let center = egui::pos2(cx, rail.center().y);
            match ui.style().visuals.handle_shape {
                egui::style::HandleShape::Circle => {
                    ui.painter().add(egui::epaint::CircleShape { center, radius: radius + visuals.expansion, fill: visuals.bg_fill, stroke: visuals.fg_stroke });
                }
                egui::style::HandleShape::Rect { aspect_ratio } => {
                    let h = vec2(radius * aspect_ratio, radius) + vec2(visuals.expansion, visuals.expansion);
                    ui.painter().rect(egui::Rect::from_center_size(center, 2.0 * h), visuals.corner_radius, visuals.bg_fill, visuals.fg_stroke, egui::StrokeKind::Inside);
                }
            }
        }
        // The number: dragging it moves by the slider's scale per pixel (linear ranges) or by
        // a share of the value (logarithmic).
        let speed = if log { (v.abs() * 0.01).max((hi - lo).abs() * 1e-6) } else { (hi - lo).abs() / x_range.span().max(1.0) as f64 };
        let b = fine_drag_value_ex(ui, v, speed, suffix);
        if b.changed() {
            r.mark_changed();
        }
        r | b
    })
    .inner
}

/// Labeled slider row (soft range: typed values may leave it).
pub fn slider(ui: &mut Ui, text: &str, tip: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, default: f64) -> Response {
    label(ui, text, tip);
    let r = fine_slider(ui, v, range, false, "");
    ui.end_row();
    let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
    reset_on_double_click(r, v, default)
}

pub fn slider_log(ui: &mut Ui, text: &str, tip: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, default: f64) -> Response {
    label(ui, text, tip);
    let r = fine_slider(ui, v, range, true, "");
    ui.end_row();
    let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
    reset_on_double_click(r, v, default)
}

pub fn slider_u32(ui: &mut Ui, text: &str, tip: &str, v: &mut u32, range: std::ops::RangeInclusive<u32>, default: u32) -> Response {
    label(ui, text, tip);
    let r = ui.add(egui::Slider::new(v, range).clamping(egui::SliderClamping::Never));
    ui.end_row();
    let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
    reset_on_double_click(r, v, default)
}

/// A drag number with Ctrl for 100 times finer steps (egui's own Shift gives 10 times).
pub fn fine_drag_value(ui: &mut Ui, v: &mut f64, speed: f64) -> Response {
    fine_drag_value_ex(ui, v, speed, "")
}

fn fine_drag_value_ex(ui: &mut Ui, v: &mut f64, speed: f64, suffix: &str) -> Response {
    // The speed itself is scaled, so the number shows the value while dragging.
    let speed = if ui.input(|i| i.modifiers.command) { speed * 0.01 } else { speed };
    ui.add(egui::DragValue::new(v).speed(speed).max_decimals(10).suffix(suffix).custom_formatter(|x, _| fmt_value(x)))
}

/// Unbounded number with drag-to-change.
pub fn drag(ui: &mut Ui, text: &str, tip: &str, v: &mut f64, speed: f64, default: f64) -> Response {
    label(ui, text, tip);
    let r = fine_drag_value(ui, v, speed);
    ui.end_row();
    let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
    reset_on_double_click(r, v, default)
}

pub fn checkbox(ui: &mut Ui, text: &str, tip: &str, v: &mut bool) -> Response {
    label(ui, text, tip);
    let r = ui.checkbox(v, "");
    ui.end_row();
    if tip.is_empty() { r } else { r.on_hover_text(tip) }
}

/// Linear RGB color swatch with picker.
pub fn color(ui: &mut Ui, text: &str, tip: &str, c: &mut [f32; 3]) -> Response {
    label(ui, text, tip);
    let r = color_button(ui, c);
    ui.end_row();
    if tip.is_empty() { r } else { r.on_hover_text(tip) }
}

pub fn color_button(ui: &mut Ui, c: &mut [f32; 3]) -> Response {
    let mut rgb = *c;
    let r = egui::widgets::color_picker::color_edit_button_rgb(ui, &mut rgb);
    // The picker converts through HSV every frame, which changes the last bits: only real
    // edits count (else showing a color would mark the scene as modified).
    if (0..3).any(|k| (rgb[k] - c[k]).abs() > 1e-5) {
        *c = rgb;
    }
    r
}

pub fn to_color32(c: [f32; 3]) -> Color32 {
    let s = |v: f32| rmb_core::gradient::linear_to_srgb(v);
    Color32::from_rgb(s(c[0]), s(c[1]), s(c[2]))
}

/// Pill-shaped segmented selector.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new().fill(theme::BG).corner_radius(8).inner_margin(3).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let w = (ui.available_width() - 2.0 * (options.len() as f32 - 1.0)) / options.len() as f32;
            for (opt, text, tip) in options {
                let selected = *value == *opt;
                let fill = if selected { theme::SURFACE_HOVER } else { Color32::TRANSPARENT };
                let txt = RichText::new(*text).color(if selected { theme::TEXT } else { theme::TEXT_MUTED });
                let mut r = ui.add_sized([w, 26.0], egui::Button::new(txt).fill(fill).corner_radius(6));
                if !tip.is_empty() {
                    r = r.on_hover_text(*tip);
                }
                if r.clicked() && !selected {
                    *value = *opt;
                    changed = true;
                }
            }
        });
    });
    changed
}

pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> Response {
    ui.add(egui::Button::new(RichText::new(icon).size(theme::fs(15.0))).frame(false).min_size(vec2(26.0, 26.0))).on_hover_text(tip)
}

/// A gradient strip of a fixed width (in grids, where the available width is unbounded).
pub fn gradient_strip_sized(ui: &mut Ui, stops: &[rmb_core::scene::ColorStop], width: f32, height: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), egui::Sense::click());
    let lut = rmb_core::gradient::lut(stops);
    let mut mesh = egui::Mesh::default();
    let n = lut.len();
    let w = rect.width() / n as f32;
    for (i, c) in lut.iter().enumerate() {
        let r = egui::Rect::from_min_max(egui::pos2(rect.left() + i as f32 * w, rect.top()), egui::pos2(rect.left() + (i + 1) as f32 * w + 0.5, rect.bottom()));
        mesh.add_colored_rect(r, to_color32(*c));
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(rect, egui::CornerRadius::same(4), egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
    resp
}

#[cfg(test)]
mod tests {
    use super::fmt_value;

    /// Drags the slider (0..24, starting at 10) by `dx` pixels from its knob with `mods` held
    /// and returns the value.
    fn drag_slider(dx: f32, mods: egui::Modifiers) -> f64 {
        drag_slider_ex(dx, mods, None)
    }

    /// As [`drag_slider`]; `release` lets go of the modifiers after that many of the ten steps.
    fn drag_slider_ex(dx: f32, mods: egui::Modifiers, release: Option<usize>) -> f64 {
        let ctx = egui::Context::default();
        let mut v = 10.0;
        let mut knob = egui::Pos2::ZERO;
        let mods_now = std::cell::Cell::new(mods);
        let frame = |events: Vec<egui::Event>, v: &mut f64, knob: &mut egui::Pos2| {
            let mut events = events;
            events.insert(0, egui::Event::ModifiersChanged(mods_now.get()));
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 200.0))), events, ..Default::default() };
            ctx.run_ui(raw, |ui| {
                let r = super::fine_slider(ui, v, 0.0..=24.0, false, "");
                // The knob of a value 10 on the rail (left part of the response).
                let (w, h) = (ui.spacing().slider_width, ui.spacing().interact_size.y);
                let radius = h / 2.5;
                *knob = egui::pos2(r.rect.left() + radius + (w - 2.0 * radius) * 10.0 / 24.0, r.rect.center().y);
            })
            // Nothing paints in the test; dropping the output unapplied is intended.
            .drop_without_applying_deltas();
        };
        frame(vec![], &mut v, &mut knob);
        let p = knob;
        let press = |down: bool, at: egui::Pos2| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: down, modifiers: mods };
        frame(vec![egui::Event::PointerMoved(p), press(true, p)], &mut v, &mut knob);
        let mut at = p;
        for step in 0..10 {
            if release == Some(step) {
                mods_now.set(egui::Modifiers::NONE);
            }
            at.x += dx / 10.0;
            frame(vec![egui::Event::PointerMoved(at)], &mut v, &mut knob);
        }
        frame(vec![press(false, at)], &mut v, &mut knob);
        v
    }

    #[test]
    fn shift_and_ctrl_drag_finer() {
        let plain = drag_slider(50.0, egui::Modifiers::NONE);
        let shift = drag_slider(50.0, egui::Modifiers::SHIFT);
        let ctrl = drag_slider(50.0, egui::Modifiers::COMMAND);
        let (a, b, c) = (plain - 10.0, shift - 10.0, ctrl - 10.0);
        assert!(a > 1.0, "plain {plain}");
        assert!((b / a - 0.1).abs() < 0.03, "shift {shift} plain {plain}");
        assert!((c / a - 0.01).abs() < 0.003, "ctrl {ctrl} plain {plain}");
        // Letting go of Shift halfway: the rest moves at the normal rate from the fine value,
        // never back to the pointer's place.
        let half = drag_slider_ex(50.0, egui::Modifiers::SHIFT, Some(5)) - 10.0;
        let expect = 0.5 * b + 0.5 * a;
        assert!((half - expect).abs() < 0.1 * a, "released halfway {half}, expected about {expect}");
    }

    #[test]
    fn values_show_up_to_four_decimals() {
        assert_eq!(fmt_value(8.0), "8.0");
        assert_eq!(fmt_value(0.4), "0.4");
        assert_eq!(fmt_value(19.62549), "19.6255");
        assert_eq!(fmt_value(-123.5), "-123.5");
        assert_eq!(fmt_value(0.025), "0.025");
        assert_eq!(fmt_value(0.000123456), "0.0001235");
        assert_eq!(fmt_value(0.0), "0.0");
    }
}
