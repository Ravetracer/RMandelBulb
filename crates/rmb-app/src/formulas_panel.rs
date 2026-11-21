// SPDX-License-Identifier: LGPL-2.1-or-later

//! Left panel: the hybrid formula chain, the selected formula's parameters and the iteration
//! settings.

use egui::{Color32, RichText, Ui};
use rmb_core::formula::{ParamKind, find, library};
use rmb_core::scene::{DeMode, Fractal, MAX_SLOTS, Scene, Slot};

use crate::{icons, theme, widgets};

#[derive(Default)]
pub struct FormulasPanel {
    pub selected: usize,
    search: String,
}

impl FormulasPanel {
    pub fn show(&mut self, ui: &mut Ui, scene: &mut Scene) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.chain(ui, &mut scene.fractal);
            ui.add_space(4.0);
            ui.separator();
            self.params(ui, &mut scene.fractal);
            ui.separator();
            iteration(ui, &mut scene.fractal);
        });
    }

    fn chain(&mut self, ui: &mut Ui, f: &mut Fractal) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("HYBRID").size(theme::fs(10.5)).color(theme::TEXT_MUTED).strong())
                .on_hover_text("Up to six formulas. Each slot runs its formula the given number of times in a row, then the next slot follows. After the last slot the chain repeats from the 'Repeat from' slot.");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(f.slots.len() < MAX_SLOTS, |ui| {
                    if widgets::icon_button(ui, icons::PLUS, "Add a formula slot").clicked() {
                        f.slots.push(Slot::new("Amazing Box"));
                        self.selected = f.slots.len() - 1;
                    }
                });
            });
        });
        ui.add_space(2.0);
        let mut remove = None;
        let mut swap = None;
        let n = f.slots.len();
        let split = f.combine.as_ref().map(|c| c.start);
        for i in 0..n {
            if split == Some(i) && i > 0 {
                ui.label(RichText::new("CHAIN 2").size(theme::fs(10.0)).color(theme::ACCENT).strong())
                    .on_hover_text("The slots from here form the second chain of the DE combination (see Two chains below)");
            }
            let selected = self.selected == i;
            let slot = &mut f.slots[i];
            let known = find(&slot.formula).is_some();
            let fill = if selected { theme::SURFACE_HOVER } else { theme::SURFACE };
            let frame = egui::Frame::new().fill(fill).corner_radius(8).inner_margin(egui::Margin::symmetric(8, 6));
            let stroke = if selected { egui::Stroke::new(1.0, theme::ACCENT) } else { egui::Stroke::NONE };
            let resp = frame.stroke(stroke).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let num = RichText::new(format!("{}", i + 1)).color(if slot.iterations == 0 { theme::TEXT_MUTED } else { theme::ACCENT }).strong();
                    ui.label(num);
                    let name_color = if !known { theme::WARN } else if slot.iterations == 0 { theme::TEXT_MUTED } else { theme::TEXT };
                    let r = formula_picker(ui, &mut slot.formula, &mut self.search, i, name_color);
                    if r {
                        slot.params.clear();
                        if let Some(fm) = find(&slot.formula) {
                            slot.iterations = slot.iterations.max(1).max(if fm.def.iterations > 1 { fm.def.iterations } else { 1 });
                        }
                        self.selected = i;
                    }
                    if !known {
                        ui.label(RichText::new(icons::WARNING).color(theme::WARN)).on_hover_text("This formula is not available (yet). The slot is skipped when rendering.");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if ui.add(egui::Button::new(RichText::new(icons::X).size(theme::fs(12.0))).frame(false)).on_hover_text("Remove this slot").clicked() {
                            remove = Some(i);
                        }
                        if i + 1 < n && ui.add(egui::Button::new(RichText::new(icons::CARET_DOWN).size(theme::fs(12.0))).frame(false)).on_hover_text("Move down").clicked() {
                            swap = Some((i, i + 1));
                        }
                        if i > 0 && ui.add(egui::Button::new(RichText::new(icons::CARET_UP).size(theme::fs(12.0))).frame(false)).on_hover_text("Move up").clicked() {
                            swap = Some((i - 1, i));
                        }
                        let r = ui.add(egui::DragValue::new(&mut slot.iterations).range(0..=100).prefix("×").speed(0.05));
                        widgets::reset_on_double_click(r, &mut slot.iterations, 1).on_hover_text("Iterations in a row (0 disables the slot)");
                    });
                });
            });
            // Select the slot on any click inside its card (without covering the controls).
            if resp.response.contains_pointer() && ui.input(|inp| inp.pointer.primary_pressed()) {
                self.selected = i;
            }
            ui.add_space(3.0);
        }
        if let Some(i) = remove {
            f.slots.remove(i);
            // Keep the second chain on the same formulas.
            if let Some(c) = f.combine.as_mut() {
                if i < c.start {
                    c.start -= 1;
                    c.repeat_from = c.repeat_from.saturating_sub(1);
                }
                if c.start == 0 || c.start >= f.slots.len() {
                    f.combine = None;
                }
            }
            if f.repeat_from >= f.slots.len() {
                f.repeat_from = f.slots.len().saturating_sub(1);
            }
            self.selected = self.selected.min(f.slots.len().saturating_sub(1));
        }
        if let Some((a, b)) = swap {
            f.slots.swap(a, b);
            if self.selected == a {
                self.selected = b;
            } else if self.selected == b {
                self.selected = a;
            }
        }
        if f.slots.is_empty() {
            ui.label(RichText::new("No formula. Add one with +.").color(theme::TEXT_MUTED));
        }
        if f.slots.len() > 1 {
            ui.horizontal(|ui| {
                widgets::label(ui, "Repeat from", "After the last slot, the chain continues with this slot (MB3D's 'repeat from'). Slots before it run only once at the start.");
                egui::ComboBox::from_id_salt("repeat-from").width(70.0).selected_text(format!("Slot {}", f.repeat_from + 1)).show_ui(ui, |ui| {
                    for i in 0..f.slots.len() {
                        ui.selectable_value(&mut f.repeat_from, i, format!("Slot {}", i + 1));
                    }
                });
            });
        }
    }

    fn params(&mut self, ui: &mut Ui, f: &mut Fractal) {
        let Some(slot) = f.slots.get_mut(self.selected) else { return };
        let Some(fm) = find(&slot.formula) else {
            widgets::section(ui, &slot.formula, false);
            ui.label(RichText::new("This formula is not in the library. Mandelbulb3D formulas are added over time; you can also write your own (Help › Formulas).").color(theme::TEXT_MUTED));
            return;
        };
        let def = &fm.def;
        if widgets::section(ui, &format!("{} · slot {}", def.name, self.selected + 1), true) {
            slot.params.clear();
        }
        if !def.description.is_empty() {
            ui.label(RichText::new(&def.description).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            ui.add_space(4.0);
        }
        if def.params.is_empty() {
            ui.label(RichText::new("No parameters.").color(theme::TEXT_MUTED));
            return;
        }
        widgets::grid(ui, "formula-params", |ui| {
            for p in &def.params {
                let mut v = slot.value(p);
                let before = v;
                let tip = format!("{} (default {})", p.ident, p.default);
                match p.kind {
                    ParamKind::Bool => {
                        let mut b = v > 0.5;
                        widgets::checkbox(ui, &p.label, &tip, &mut b);
                        v = if b { 1.0 } else { 0.0 };
                    }
                    ParamKind::Int => {
                        widgets::label(ui, &p.label, &tip);
                        let r = ui.add(egui::DragValue::new(&mut v).speed(0.05).range(p.min..=p.max).fixed_decimals(0));
                        widgets::reset_on_double_click(r, &mut v, p.default);
                        v = v.round();
                        ui.end_row();
                    }
                    ParamKind::Angle => {
                        widgets::label(ui, &p.label, &tip);
                        let r = ui.add(egui::Slider::new(&mut v, p.min..=p.max).suffix("°").clamping(egui::SliderClamping::Never).max_decimals(3));
                        widgets::reset_on_double_click(r, &mut v, p.default);
                        ui.end_row();
                    }
                    ParamKind::Float => {
                        widgets::slider(ui, &p.label, &tip, &mut v, p.min..=p.max, p.default);
                    }
                }
                if v != before {
                    slot.params.insert(p.ident.clone(), v);
                }
            }
        });
    }
}

/// Button with the formula name that opens a searchable list. Returns true if a formula was chosen.
fn formula_picker(ui: &mut Ui, name: &mut String, search: &mut String, idx: usize, color: Color32) -> bool {
    let mut chosen = false;
    let id = ui.id().with(("formula-picker", idx));
    let btn = ui.add(egui::Button::new(RichText::new(format!("{name}  {}", icons::CARET_DOWN)).color(color)).frame(false)).on_hover_text(find(name).map(|f| f.def.description.clone()).unwrap_or_else(|| "Choose a formula".into()));
    egui::Popup::from_toggle_button_response(&btn).id(id).width(300.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(300.0);
        let r = ui.add(egui::TextEdit::singleline(search).hint_text(format!("{}  Search formulas", icons::MAGNIFYING_GLASS)).desired_width(f32::INFINITY));
        if search.is_empty() {
            r.request_focus();
        }
        ui.add_space(4.0);
        let q = search.to_lowercase();
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            let lib = library();
            for cat in lib.categories() {
                let items: Vec<_> = lib
                    .all()
                    .iter()
                    .filter(|f| f.def.category == cat)
                    .filter(|f| q.is_empty() || f.def.name.to_lowercase().contains(&q) || f.def.aliases.iter().any(|a| a.name.to_lowercase().contains(&q)) || f.def.description.to_lowercase().contains(&q))
                    .collect();
                if items.is_empty() {
                    continue;
                }
                ui.label(RichText::new(cat.to_uppercase()).size(theme::fs(10.0)).color(theme::TEXT_MUTED).strong());
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    for f in items {
                        let sel = f.def.name == *name;
                        let r = ui.selectable_label(sel, &f.def.name).on_hover_text(&f.def.description);
                        if r.clicked() {
                            *name = f.def.name.clone();
                            chosen = true;
                            search.clear();
                            ui.close();
                        }
                    }
                });
                ui.add_space(4.0);
            }
        });
    });
    chosen
}

fn iteration(ui: &mut Ui, f: &mut Fractal) {
    let d = Fractal::default();
    if widgets::section(ui, "Iteration", true) {
        let slots = std::mem::take(&mut f.slots);
        let repeat = f.repeat_from;
        *f = Fractal { slots, repeat_from: repeat, ..d.clone() };
    }
    widgets::grid(ui, "iteration", |ui| {
        widgets::slider_u32(ui, "Max iterations", "Iterations per point, all slots together. More iterations give finer detail (and slower rendering).", &mut f.max_iterations, 1..=100, d.max_iterations);
        widgets::slider_log(ui, "Bailout", "Escape radius: a point escapes when its distance from the origin exceeds this.", &mut f.bailout, 1.0..=1024.0, d.bailout);
        widgets::label(ui, "Distance", "How the distance to the surface is estimated. Auto picks the formula's analytic estimate; numeric is slower but works for every hybrid.");
        egui::ComboBox::from_id_salt("de-mode").selected_text(f.de_mode.label()).show_ui(ui, |ui| {
            for m in DeMode::ALL {
                ui.selectable_value(&mut f.de_mode, m, m.label());
            }
        });
        ui.end_row();
        widgets::slider(ui, "DE scale", "Multiplies the distance estimate. Lower it if the surface shows holes or noise (overstepping).", &mut f.de_scale, 0.05..=2.0, d.de_scale);
        widgets::slider(ui, "W slice", "Start value of the 4th dimension (4D formulas).", &mut f.w_slice, -2.0..=2.0, d.w_slice);
        widgets::checkbox(ui, "Julia", "Use a fixed constant c for every point instead of the point's position.", &mut f.julia);
        if f.julia {
            for (k, n) in ["Julia X", "Julia Y", "Julia Z", "Julia W"].iter().enumerate() {
                widgets::drag(ui, n, "Julia constant", &mut f.julia_c[k], 0.002, 0.0);
            }
        }
        for (k, n) in ["Rotate XW", "Rotate YW", "Rotate ZW"].iter().enumerate() {
            widgets::slider(ui, n, "Rotates the start point into the 4th dimension (formulas that use w)", &mut f.rotation_4d[k], -180.0..=180.0, 0.0);
        }
    });
    combine(ui, f);
}

fn combine(ui: &mut Ui, f: &mut Fractal) {
    widgets::section(ui, "Two chains", false);
    let mut on = f.combine.is_some();
    if ui
        .checkbox(&mut on, "DE combination")
        .on_hover_text("Splits the slots into two chains with their own iteration limit and combines their distance estimates (union, intersection, subtraction or a smooth blend)")
        .changed()
    {
        f.combine = if on {
            let start = (f.slots.len() / 2).max(1);
            Some(rmb_core::scene::DeCombine { start, repeat_from: start, max_iterations: f.max_iterations, ..Default::default() })
        } else {
            None
        };
    }
    let n = f.slots.len();
    let Some(c) = f.combine.as_mut() else { return };
    if n < 2 {
        ui.label(RichText::new("Needs at least two slots.").color(theme::WARN));
        return;
    }
    let d = rmb_core::scene::DeCombine::default();
    widgets::grid(ui, "combine", |ui| {
        widgets::label(ui, "Combine", "How the two estimates are combined");
        egui::ComboBox::from_id_salt("combine-mode").selected_text(c.mode.label()).show_ui(ui, |ui| {
            for m in rmb_core::scene::CombineMode::ALL {
                ui.selectable_value(&mut c.mode, m, m.label());
            }
        });
        ui.end_row();
        widgets::label(ui, "Chain 2 from", "First slot of the second chain");
        egui::ComboBox::from_id_salt("combine-start").width(80.0).selected_text(format!("Slot {}", c.start + 1)).show_ui(ui, |ui| {
            for i in 1..n {
                ui.selectable_value(&mut c.start, i, format!("Slot {}", i + 1));
            }
        });
        ui.end_row();
        c.start = c.start.clamp(1, n - 1);
        widgets::label(ui, "Repeat from", "The second chain continues with this slot after its last one");
        egui::ComboBox::from_id_salt("combine-repeat").width(80.0).selected_text(format!("Slot {}", c.repeat_from.max(c.start) + 1)).show_ui(ui, |ui| {
            for i in c.start..n {
                ui.selectable_value(&mut c.repeat_from, i, format!("Slot {}", i + 1));
            }
        });
        ui.end_row();
        c.repeat_from = c.repeat_from.clamp(c.start, n - 1);
        widgets::slider_u32(ui, "Iterations 2", "Maximum iterations of the second chain", &mut c.max_iterations, 1..=100, d.max_iterations);
        if matches!(c.mode, rmb_core::scene::CombineMode::SmoothLinear | rmb_core::scene::CombineMode::Smooth | rmb_core::scene::CombineMode::Subtract) {
            widgets::slider_log(ui, "Blend", "Distance over which the two shapes blend (world units)", &mut c.smooth, 0.0001..=2.0, d.smooth);
        }
    });
}
