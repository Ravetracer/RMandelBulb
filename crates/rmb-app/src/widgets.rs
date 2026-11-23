// SPDX-License-Identifier: LGPL-2.1-or-later

//! Small reusable widgets. Every value control resets to its default on double-click.

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

/// Section header with an optional reset button. Returns true if reset was clicked.
pub fn section(ui: &mut Ui, text: &str, reset: bool) -> bool {
    let mut clicked = false;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(text.to_uppercase()).size(theme::fs(10.5)).color(theme::TEXT_MUTED).strong());
        if reset {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = ui
                    .add(egui::Button::new(RichText::new(crate::icons::REFRESH).size(theme::fs(11.0))).small().frame(false))
                    .on_hover_text(format!("Reset {} to the defaults", text.to_lowercase()))
                    .clicked();
            });
        }
    });
    ui.add_space(2.0);
    clicked
}

pub fn reset_on_double_click<T: Copy + PartialEq>(mut r: Response, v: &mut T, default: T) -> Response {
    let on_widget = r.ctx.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary) && i.pointer.interact_pos().is_some_and(|p| r.rect.contains(p)));
    if (r.double_clicked() || on_widget) && *v != default {
        *v = default;
        r.mark_changed();
    }
    r
}

/// Labeled slider row (soft range: typed values may leave it).
pub fn slider(ui: &mut Ui, text: &str, tip: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, default: f64) -> Response {
    label(ui, text, tip);
    let r = ui.add(egui::Slider::new(v, range).clamping(egui::SliderClamping::Never).max_decimals(4));
    ui.end_row();
    let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
    reset_on_double_click(r, v, default)
}

pub fn slider_log(ui: &mut Ui, text: &str, tip: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, default: f64) -> Response {
    label(ui, text, tip);
    let r = ui.add(egui::Slider::new(v, range).logarithmic(true).clamping(egui::SliderClamping::Never).max_decimals(5));
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

/// Unbounded number with drag-to-change.
pub fn drag(ui: &mut Ui, text: &str, tip: &str, v: &mut f64, speed: f64, default: f64) -> Response {
    label(ui, text, tip);
    let r = ui.add(egui::DragValue::new(v).speed(speed).max_decimals(10));
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
