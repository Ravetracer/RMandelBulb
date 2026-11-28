// SPDX-License-Identifier: LGPL-2.1-or-later

//! The gradient (palette) editor: an interactive strip with draggable color stops, presets and
//! saved palettes, a random palette generator, whole-palette adjustments and tools, and the
//! list of stops.

use std::sync::{Mutex, OnceLock};

use egui::{RichText, Ui};
use egui_phosphor::regular as ph;
use rmb_core::palette::{self, Adjust, Harmony, Order, PaletteOptions};
use rmb_core::scene::ColorStop;
use serde::{Deserialize, Serialize};

use crate::{icons, theme, widgets};

/// A palette the user saved (`palettes.json` in the config folder).
#[derive(Clone, Serialize, Deserialize)]
struct SavedPalette {
    name: String,
    stops: Vec<ColorStop>,
}

fn saved_path() -> std::path::PathBuf {
    crate::prefs::config_dir().join("palettes.json")
}

/// The saved palettes, loaded on first use.
fn saved() -> &'static Mutex<Vec<SavedPalette>> {
    static SAVED: OnceLock<Mutex<Vec<SavedPalette>>> = OnceLock::new();
    SAVED.get_or_init(|| Mutex::new(std::fs::read_to_string(saved_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()))
}

/// Writes the saved palettes; returns the error text on failure.
fn store(list: &[SavedPalette]) -> Result<(), String> {
    let path = saved_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// Editor state kept between frames (per editor).
#[derive(Clone)]
struct State {
    selected: Option<usize>,
    generator: PaletteOptions,
    /// The adjustment sliders work on `base` (the gradient when they were first touched) and
    /// wrote `out`; any other change of the gradient starts a new base.
    adjust: Adjust,
    hardness: f64,
    base: Vec<ColorStop>,
    out: Vec<ColorStop>,
    save_name: String,
    message: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            selected: None,
            generator: PaletteOptions::default(),
            adjust: Adjust::default(),
            hardness: 0.0,
            base: Vec::new(),
            out: Vec::new(),
            save_name: String::new(),
            message: None,
        }
    }
}

/// Gradient strip, presets and tools, generator, adjustments and the color stops (surface,
/// highlight, transparency with `glass`, emission, position).
pub fn gradient_editor(ui: &mut Ui, gradient: &mut Vec<ColorStop>, glass: bool) {
    let id = ui.id().with("palette-editor");
    let mut st: State = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if st.selected.is_some_and(|i| i >= gradient.len()) {
        st.selected = None;
    }
    strip(ui, gradient, &mut st);
    ui.add_space(4.0);
    toolbar(ui, gradient, &mut st);
    if let Some(m) = &st.message {
        ui.label(RichText::new(m).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    }
    generator(ui, gradient, &mut st);
    adjustments(ui, gradient, &mut st);
    ui.add_space(4.0);
    stops(ui, gradient, &mut st, glass);
    ui.data_mut(|d| d.insert_temp(id, st));
}

/// The gradient with a marker per stop: drag a marker to move the stop, click the strip to add
/// a stop there, right-click (or double-click) a marker to remove it. Clicking a marker selects
/// its stop in the list.
fn strip(ui: &mut Ui, gradient: &mut Vec<ColorStop>, st: &mut State) {
    let height = 24.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height + 12.0), egui::Sense::click_and_drag());
    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), height));
    let lut = rmb_core::gradient::lut(gradient);
    let mut mesh = egui::Mesh::default();
    let w = bar.width() / lut.len() as f32;
    for (i, c) in lut.iter().enumerate() {
        let r = egui::Rect::from_min_max(egui::pos2(bar.left() + i as f32 * w, bar.top()), egui::pos2(bar.left() + (i + 1) as f32 * w + 0.5, bar.bottom()));
        mesh.add_colored_rect(r, widgets::to_color32(*c));
    }
    let painter = ui.painter();
    painter.add(egui::Shape::mesh(mesh));
    // Emissive parts: a thin glow line above the strip.
    for (i, e) in rmb_core::gradient::lut_emission(gradient).iter().enumerate() {
        if *e > 0.0 {
            let x = bar.left() + i as f32 * w;
            let a = (e.min(4.0) / 4.0 * 255.0) as u8;
            painter.line_segment([egui::pos2(x, bar.top() + 1.0), egui::pos2(x + w, bar.top() + 1.0)], egui::Stroke::new(2.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, a)));
        }
    }
    painter.rect_stroke(bar, egui::CornerRadius::same(4), egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
    let x_of = |pos: f64| bar.left() + pos.clamp(0.0, 1.0) as f32 * bar.width();
    let pos_of = |x: f32| (((x - bar.left()) / bar.width()) as f64).clamp(0.0, 1.0);
    for (i, s) in gradient.iter().enumerate() {
        let x = x_of(s.pos);
        let sel = st.selected == Some(i);
        let top = bar.bottom() - 4.0;
        let tri = vec![egui::pos2(x, top), egui::pos2(x - 6.0, rect.bottom()), egui::pos2(x + 6.0, rect.bottom())];
        let stroke = if sel { egui::Stroke::new(2.0, theme::TEXT) } else { egui::Stroke::new(1.0, egui::Color32::BLACK) };
        painter.add(egui::Shape::convex_polygon(tri, widgets::to_color32(s.color), stroke));
    }
    let near = |gradient: &[ColorStop], p: egui::Pos2| -> Option<usize> {
        gradient.iter().enumerate().map(|(i, s)| (i, (x_of(s.pos) - p.x).abs())).filter(|(_, d)| *d < 7.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i).filter(|_| p.y > bar.bottom() - 6.0)
    };
    let drag_id = ui.id().with("stop-drag");
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let hit = near(gradient, p);
        if hit.is_some() {
            st.selected = hit;
        }
        ui.data_mut(|d| d.insert_temp(drag_id, hit));
    }
    let dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id)).flatten();
    if resp.dragged()
        && let (Some(i), Some(p)) = (dragging, resp.interact_pointer_pos())
        && let Some(s) = gradient.get_mut(i)
    {
        s.pos = pos_of(p.x);
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.insert_temp(drag_id, None::<usize>));
    }
    let remove = |gradient: &mut Vec<ColorStop>, st: &mut State, i: usize| {
        if gradient.len() > 2 {
            gradient.remove(i);
            st.selected = None;
        }
    };
    if (resp.secondary_clicked() || resp.double_clicked())
        && let Some(i) = resp.interact_pointer_pos().and_then(|p| near(gradient, p))
    {
        remove(gradient, st, i);
    } else if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match near(gradient, p) {
            Some(i) => st.selected = Some(i),
            None => {
                // A new stop with the color the gradient has there.
                let t = pos_of(p.x);
                let mut s = ColorStop::new(t, rmb_core::gradient::sample(gradient, t));
                s.specular = rmb_core::gradient::sample_specular(gradient, t);
                s.emission = rmb_core::gradient::sample_emission(gradient, t);
                s.transparency = rmb_core::gradient::sample_transparency(gradient, t);
                gradient.push(s);
                st.selected = Some(gradient.len() - 1);
            }
        }
    }
    resp.on_hover_text("Drag a marker to move its color. Click the strip to add a color there, right-click or double-click a marker to remove it. A white line marks emissive parts.");
}

/// Presets and saved palettes, random palette and the palette tools.
fn toolbar(ui: &mut Ui, gradient: &mut Vec<ColorStop>, st: &mut State) {
    ui.horizontal_wrapped(|ui| {
        ui.menu_button(format!("{}  Presets", ph::PALETTE), |ui| {
            for name in rmb_core::gradient::preset_names() {
                if ui.button(name).on_hover_text("Replace the colors with this gradient").clicked()
                    && let Some(g) = rmb_core::gradient::preset(name)
                {
                    *gradient = g;
                    st.selected = None;
                    ui.close();
                }
            }
            let mut list = saved().lock().map(|l| l.clone()).unwrap_or_default();
            if !list.is_empty() {
                ui.separator();
                ui.label(RichText::new("SAVED").size(theme::fs(10.0)).color(theme::TEXT_MUTED));
            }
            let mut delete = None;
            for (i, p) in list.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.button(&p.name).on_hover_text("Replace the colors with this saved palette").clicked() {
                        *gradient = p.stops.clone();
                        st.selected = None;
                        ui.close();
                    }
                    if widgets::icon_button(ui, ph::TRASH, "Delete this saved palette").clicked() {
                        delete = Some(i);
                    }
                });
            }
            if let Some(i) = delete {
                let name = list.remove(i).name;
                st.message = Some(match store(&list) {
                    Ok(()) => format!("Deleted the palette \"{name}\"."),
                    Err(e) => format!("Could not save the palettes: {e}"),
                });
                if let Ok(mut l) = saved().lock() {
                    *l = list;
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut st.save_name).hint_text("Name").desired_width(120.0)).on_hover_text("Name of the palette to save");
                let name = st.save_name.trim().to_string();
                if ui.add_enabled(!name.is_empty(), egui::Button::new(format!("{}  Save", ph::FLOPPY_DISK))).on_hover_text("Save the current colors as a palette (a saved palette with the same name is replaced)").clicked() {
                    let mut list = saved().lock().map(|l| l.clone()).unwrap_or_default();
                    list.retain(|p| p.name != name);
                    list.push(SavedPalette { name: name.clone(), stops: gradient.clone() });
                    st.message = Some(match store(&list) {
                        Ok(()) => format!("Saved the palette \"{name}\"."),
                        Err(e) => format!("Could not save the palette: {e}"),
                    });
                    if let Ok(mut l) = saved().lock() {
                        *l = list;
                    }
                    st.save_name.clear();
                    ui.close();
                }
            });
        })
        .response
        .on_hover_text("Built-in gradients and your saved palettes; save the current colors");
        if ui.button(format!("{}  Random", ph::DICE_FIVE)).on_hover_text("A new random palette with the generator's settings (Generator below)").clicked() {
            st.generator.seed = st.generator.seed.wrapping_add(1).max(1).wrapping_mul(6_364_136_223_846_793_005).rotate_left(17) | 1;
            *gradient = palette::generate(&st.generator);
            st.selected = None;
        }
        ui.menu_button(format!("{}  Tools", ph::WRENCH), |ui| {
            let mut apply = |ui: &mut Ui, text: &str, tip: &str, f: &dyn Fn(&[ColorStop]) -> Vec<ColorStop>| {
                if ui.button(text).on_hover_text(tip).clicked() {
                    *gradient = f(gradient);
                    st.selected = None;
                }
            };
            apply(ui, "Reverse", "The gradient runs the other way", &palette::reverse);
            apply(ui, "Shift by a quarter", "Moves all colors a quarter along the gradient (wrapping around)", &|g| palette::rotate(g, 0.25));
            apply(ui, "Mirror", "Squeezes the gradient into the first half and mirrors it into the second: repeats without a seam", &palette::mirror);
            apply(ui, "Repeat twice", "The gradient twice over the same length", &|g| palette::repeat(g, 2));
            apply(ui, "Space evenly", "Spreads the colors evenly in their order", &|g| {
                let mut s = g.to_vec();
                s.sort_by(|a, b| a.pos.total_cmp(&b.pos));
                palette::space_evenly(&mut s);
                s
            });
            apply(ui, "Sort by lightness", "Orders the colors from dark to light and spaces them evenly", &palette::sort_by_lightness);
            apply(ui, "Invert lightness", "Dark colors become light and light ones dark (hue and saturation stay)", &palette::invert);
            apply(ui, "Hard steps", "Each color becomes a solid band (no blends)", &|g| palette::set_hardness(g, 1.0));
            apply(ui, "Smooth blends", "One stop per color, blending smoothly, spaced evenly", &|g| palette::set_hardness(g, 0.0));
        })
        .response
        .on_hover_text("Change the whole palette at once");
    });
}

/// The random palette generator: changing a setting makes the palette again with the same
/// random choices; Random (or New) draws new ones.
fn generator(ui: &mut Ui, gradient: &mut Vec<ColorStop>, st: &mut State) {
    egui::CollapsingHeader::new(RichText::new(format!("{}  Generator", ph::MAGIC_WAND)).size(theme::fs(12.0))).id_salt("palette-generator").show(ui, |ui| {
        let before = st.generator.clone();
        let d = PaletteOptions::default();
        let g = &mut st.generator;
        widgets::grid(ui, "palette-generator-grid", |ui| {
            widgets::label(ui, "Harmony", "How the hues of the colors relate");
            egui::ComboBox::from_id_salt("palette-harmony").height(400.0).selected_text(g.harmony.label()).show_ui(ui, |ui| {
                for h in Harmony::ALL {
                    ui.selectable_value(&mut g.harmony, h, h.label()).on_hover_text(harmony_tip(h));
                }
            });
            ui.end_row();
            let mut n = g.colors as u32;
            widgets::slider_u32(ui, "Colors", "Number of colors in the palette", &mut n, 2..=16, d.colors as u32);
            g.colors = n.clamp(2, 16) as usize;
            widgets::checkbox(ui, "Random hue", "Pick the base hue at random (else the hue below)", &mut g.random_hue);
            if !g.random_hue {
                widgets::slider(ui, "Hue", "Base hue in degrees (0 red, 120 green, 240 blue)", &mut g.hue, 0.0..=360.0, d.hue);
            }
            widgets::slider(ui, "Hue jitter", "Random deviation of every hue from the harmony, in degrees", &mut g.hue_jitter, 0.0..=90.0, d.hue_jitter);
            if g.harmony != Harmony::DarkNeon {
                widgets::slider(ui, "Saturation min", "Least saturation of the colors (0 gray, 1 the most saturated)", &mut g.saturation[0], 0.0..=1.0, d.saturation[0]);
                widgets::slider(ui, "Saturation max", "Most saturation of the colors", &mut g.saturation[1], 0.0..=1.0, d.saturation[1]);
                widgets::slider(ui, "Lightness min", "Darkest lightness of the colors (0 black, 1 white; perceptual)", &mut g.lightness[0], 0.0..=1.0, d.lightness[0]);
                widgets::slider(ui, "Lightness max", "Brightest lightness of the colors", &mut g.lightness[1], 0.0..=1.0, d.lightness[1]);
            }
            widgets::label(ui, "Order", "Order of the colors along the gradient");
            egui::ComboBox::from_id_salt("palette-order").selected_text(g.order.label()).show_ui(ui, |ui| {
                for o in Order::ALL {
                    ui.selectable_value(&mut g.order, o, o.label());
                }
            });
            ui.end_row();
            widgets::slider(ui, "Hardness", "0: smooth blends between the colors, 1: solid bands with hard edges", &mut g.hardness, 0.0..=1.0, d.hardness);
            widgets::checkbox(ui, "Seamless", "The gradient ends with its first color, so repeated gradients have no seam", &mut g.cyclic);
            widgets::slider(ui, "Emissive share", "Share of the colors (the brightest first) that glow by themselves", &mut g.emissive_share, 0.0..=1.0, d.emissive_share);
            if g.emissive_share > 0.0 {
                widgets::slider(ui, "Emission", "Emission strength of the glowing colors (settings under Mapping › Emissive colors)", &mut g.emission, 0.0..=20.0, d.emission);
            }
        });
        ui.horizontal(|ui| {
            if ui.button(format!("{}  New", ph::DICE_FIVE)).on_hover_text("A new random palette with these settings").clicked() {
                st.generator.seed = st.generator.seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407) | 1;
                *gradient = palette::generate(&st.generator);
                st.selected = None;
            }
            if ui.button("Defaults").on_hover_text("Reset the generator settings").clicked() {
                st.generator = PaletteOptions { seed: st.generator.seed, ..PaletteOptions::default() };
            }
        });
        if st.generator != before {
            *gradient = palette::generate(&st.generator);
            st.selected = None;
        }
    });
}

fn harmony_tip(h: Harmony) -> &'static str {
    match h {
        Harmony::Random => "Any hues",
        Harmony::Analogous => "Neighboring hues: calm, natural palettes",
        Harmony::Complementary => "The base hue and its opposite: strong contrast",
        Harmony::SplitComplementary => "The base hue and the two neighbors of its opposite",
        Harmony::Triadic => "Three hues a third of the circle apart: colorful but balanced",
        Harmony::Tetradic => "Four hues a quarter of the circle apart",
        Harmony::Monochrome => "One hue in different lightness and saturation",
        Harmony::DarkNeon => "Mostly very dark, dull colors with a few bright, saturated ones: dark structures with neon lines (raise the emissive share to make them glow)",
    }
}

/// Hue, saturation, lightness, contrast and hardness of the whole palette.
fn adjustments(ui: &mut Ui, gradient: &mut Vec<ColorStop>, st: &mut State) {
    egui::CollapsingHeader::new(RichText::new(format!("{}  Adjust", ph::SLIDERS_HORIZONTAL)).size(theme::fs(12.0))).id_salt("palette-adjust").show(ui, |ui| {
        // Edited elsewhere since the last adjustment: start from the gradient as it is now.
        if *gradient != st.out {
            st.base = gradient.clone();
            st.adjust = Adjust::default();
            st.hardness = -1.0;
        }
        let before = (st.adjust, st.hardness);
        let a = &mut st.adjust;
        widgets::grid(ui, "palette-adjust-grid", |ui| {
            widgets::slider(ui, "Hue", "Turns all hues by this many degrees", &mut a.hue, -180.0..=180.0, 0.0);
            widgets::slider(ui, "Saturation", "Less (-1: gray) or more saturated colors", &mut a.saturation, -1.0..=1.0, 0.0);
            widgets::slider(ui, "Lightness", "Darker or lighter colors (perceptual lightness)", &mut a.lightness, -1.0..=1.0, 0.0);
            widgets::slider(ui, "Contrast", "Less or more difference between dark and light colors", &mut a.contrast, -1.0..=1.0, 0.0);
            let mut h = st.hardness.max(0.0);
            let r = widgets::slider(ui, "Hardness", "0: one stop per color with smooth blends, 1: solid bands with hard edges (spaces the colors evenly)", &mut h, 0.0..=1.0, 0.0);
            if r.changed() {
                st.hardness = h;
            }
        });
        if (st.adjust, st.hardness) != before {
            let mut g = palette::adjust(&st.base, st.adjust);
            if st.hardness >= 0.0 {
                g = palette::set_hardness(&g, st.hardness);
            }
            *gradient = g;
            st.out = gradient.clone();
            st.selected = None;
        }
        if ui.small_button("Apply and reset").on_hover_text("Keep the adjusted colors and put the sliders back to zero").clicked() {
            st.out.clear();
        }
    });
}

/// The color stops, sorted by position; the selected one is highlighted.
fn stops(ui: &mut Ui, gradient: &mut Vec<ColorStop>, st: &mut State, glass: bool) {
    let mut remove = None;
    let n = gradient.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| gradient[*a].pos.total_cmp(&gradient[*b].pos));
    for i in order {
        let selected = st.selected == Some(i);
        let frame = egui::Frame::new().corner_radius(4).inner_margin(egui::Margin::symmetric(2, 1)).fill(if selected { theme::SURFACE_HOVER } else { egui::Color32::TRANSPARENT });
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let s = &mut gradient[i];
                let r = widgets::color_button(ui, &mut s.color).on_hover_text("Surface color");
                if r.clicked() {
                    st.selected = Some(i);
                }
                widgets::color_button(ui, &mut s.specular).on_hover_text("Highlight color: multiplies the specular light here");
                if glass {
                    widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut s.transparency).range(0.0..=1.0).speed(0.01).max_decimals(2)).on_hover_text("Transparency of the glass at this color (0 = opaque, 1 = clear)"), &mut s.transparency, 0.0);
                    ui.spacing_mut().slider_width = (ui.spacing().slider_width - 50.0).max(60.0);
                }
                let e = ui
                    .add(egui::DragValue::new(&mut s.emission).range(0.0..=100.0).speed(0.02).max_decimals(2).prefix(format!("{} ", ph::SPARKLE)))
                    .on_hover_text("Emission: this color shines by itself, lights its surroundings and glows (0 = off; settings under Mapping › Emissive colors)");
                let e = widgets::reset_on_double_click(e, &mut s.emission, 0.0);
                if s.emission > 0.0 {
                    e.highlight();
                }
                ui.spacing_mut().slider_width = (ui.spacing().slider_width - 55.0).max(50.0);
                if widgets::fine_slider(ui, &mut s.pos, 0.0..=1.0, false, "").on_hover_text("Position of this color on the gradient (Shift / Ctrl: finer)").changed() {
                    st.selected = Some(i);
                }
                if n > 2 && widgets::icon_button(ui, icons::X, "Remove this color").clicked() {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        gradient.remove(i);
        st.selected = None;
    }
    if ui.button(format!("{}  Add color", icons::PLUS)).on_hover_text("Add a color in the middle of the gradient (or click the strip where you want it)").clicked() {
        let c = rmb_core::gradient::sample(gradient, 0.5);
        gradient.push(ColorStop::new(0.5, c));
        st.selected = Some(gradient.len() - 1);
    }
}
