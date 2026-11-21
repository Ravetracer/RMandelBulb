// SPDX-License-Identifier: LGPL-2.1-or-later


//! Flat, dark visual theme. No gradients, few borders, one accent color.

use std::sync::atomic::{AtomicU32, Ordering};

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, Style, TextStyle, Visuals, vec2};

pub const BG: Color32 = Color32::from_rgb(0x15, 0x16, 0x1a);
pub const PANEL: Color32 = Color32::from_rgb(0x1c, 0x1e, 0x23);
pub const SURFACE: Color32 = Color32::from_rgb(0x25, 0x28, 0x2f);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x2e, 0x32, 0x3a);
pub const BORDER: Color32 = Color32::from_rgb(0x30, 0x33, 0x3b);
pub const TEXT: Color32 = Color32::from_rgb(0xe4, 0xe6, 0xeb);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x8e, 0x94, 0xa0);
pub const ACCENT: Color32 = Color32::from_rgb(0x5b, 0xa8, 0xff);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x1f, 0x3a, 0x5c);
pub const WARN: Color32 = Color32::from_rgb(0xf2, 0xb3, 0x4a);
pub const CANVAS: Color32 = Color32::from_rgb(0x0e, 0x0f, 0x12);

/// UI font size in percent of the built-in sizes (Preferences › Font size).
static FONT_PERCENT: AtomicU32 = AtomicU32::new(100);

/// Font size limits of the preference, in percent.
pub const FONT_PERCENT_RANGE: std::ops::RangeInclusive<u32> = 50..=200;

/// A built-in font size scaled by the font size preference.
pub fn fs(px: f32) -> f32 {
    px * FONT_PERCENT.load(Ordering::Relaxed) as f32 / 100.0
}

/// Sets the theme; `font_percent` scales every font of the UI.
pub fn apply(ctx: &egui::Context, font_percent: u32) {
    FONT_PERCENT.store(font_percent.clamp(*FONT_PERCENT_RANGE.start(), *FONT_PERCENT_RANGE.end()), Ordering::Relaxed);
    let mut style = Style::default();
    let mut v = Visuals::dark();
    let r = CornerRadius::same(6);

    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(110) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(100) };
    v.extreme_bg_color = BG;
    v.faint_bg_color = SURFACE;
    v.code_bg_color = SURFACE;
    v.warn_fg_color = WARN;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.slider_trailing_fill = true;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.striped = false;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_MUTED);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = SURFACE;
    w.inactive.weak_bg_fill = SURFACE;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = SURFACE_HOVER;
    w.hovered.weak_bg_fill = SURFACE_HOVER;
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = ACCENT_DIM;
    w.active.weak_bg_fill = ACCENT_DIM;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = SURFACE_HOVER;
    w.open.weak_bg_fill = SURFACE_HOVER;
    w.open.bg_stroke = Stroke::NONE;
    w.open.fg_stroke = Stroke::new(1.0, TEXT);
    w.open.corner_radius = r;

    style.visuals = v;
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(10.0, 5.0);
    style.spacing.interact_size = vec2(36.0, 24.0);
    style.spacing.slider_width = 150.0;
    style.spacing.window_margin = Margin::same(14);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.combo_width = 150.0;
    style.spacing.indent = 14.0;
    // Scrollbars get their own strip instead of floating over the content (they covered
    // controls at the right edge, e.g. the last point of a mixer curve).
    style.spacing.scroll = egui::style::ScrollStyle::solid();

    style.text_styles = [
        (TextStyle::Small, FontId::new(fs(11.0), FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(fs(13.5), FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(fs(13.5), FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(fs(17.0), FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(fs(12.5), FontFamily::Monospace)),
    ]
    .into();

    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}
