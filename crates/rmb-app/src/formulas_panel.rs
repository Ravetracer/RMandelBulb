// SPDX-License-Identifier: LGPL-2.1-or-later

//! Left panel: the scene's objects (main fractal and composed objects), and for the selected
//! one its hybrid formula chain, parameters and iteration settings, or its primitive, placement,
//! operation and material.

use egui::{Color32, RichText, Ui};
use rmb_core::compose::{BoolOp, MAX_OBJECTS, ModelRef, ObjectMaterial, Pattern, Primitive, SceneObject, Shape, Transform};
use rmb_core::formula::{ParamKind, find, library};
use rmb_core::mesh::MaterialOverride;
use rmb_core::scene::{ColorMode, DeMode, Fractal, MAX_SLOTS, Scene, Slot};

use crate::{icons, theme, widgets};

#[derive(Default)]
pub struct FormulasPanel {
    pub selected: usize,
    search: String,
    editor: crate::formula_editor::FormulaEditor,
}

impl FormulasPanel {
    /// `selected`: the object being edited (None: the main fractal), shared with the view.
    pub fn show(&mut self, ui: &mut Ui, scene: &mut Scene, selected: &mut Option<usize>) {
        if selected.is_some_and(|k| k >= scene.objects.len()) {
            *selected = None;
        }
        // Models that bake in the background show up when they are done.
        let baking = scene.objects.iter().any(|o| matches!(&o.shape, Shape::Model(m) if matches!(rmb_core::mesh::request(&m.path, &m.options), rmb_core::mesh::Status::Baking(_))));
        if baking {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
        }
        auto_assign_textures(scene);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let before = *selected;
            self.objects(ui, scene, selected);
            if *selected != before {
                self.selected = 0;
            }
            ui.separator();
            match *selected {
                None => {
                    self.chain(ui, &mut scene.fractal);
                    ui.add_space(4.0);
                    ui.separator();
                    self.params(ui, &mut scene.fractal);
                    if let Some(slot) = scene.fractal.slots.get_mut(self.selected) {
                        slot_settings(ui, slot, self.selected);
                    }
                    ui.separator();
                    iteration(ui, &mut scene.fractal, true);
                    let cam = scene.camera.clone();
                    let t = &mut scene.fractal.transform;
                    let sec = widgets::section(ui, "Placement", !t.is_identity());
                    if sec.reset {
                        *t = Transform::default();
                    }
                    if sec.open {
                        placement(ui, t, &cam, "main-place", false);
                    }
                }
                Some(k) => {
                    let glass = scene.material.transmission;
                    self.object(ui, &mut scene.objects[k], &scene.camera, glass);
                }
            }
        });
        // The formula editor works on the selected slot of the fractal being edited.
        if let Some(crate::formula_editor::EditorAction::Use(name)) = self.editor.show(ui.ctx()) {
            let fractal = match *selected {
                None => Some(&mut scene.fractal),
                Some(k) => match &mut scene.objects[k].shape {
                    Shape::Fractal(f) => Some(f.as_mut()),
                    _ => None,
                },
            };
            if let Some(slot) = fractal.and_then(|f| f.slots.get_mut(self.selected)) {
                slot.formula = name;
            }
        }
    }

    /// The list of the scene's objects with the buttons to add, order and remove them.
    fn objects(&mut self, ui: &mut Ui, scene: &mut Scene, selected: &mut Option<usize>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("SCENE").size(theme::fs(10.5)).color(theme::TEXT_MUTED).strong()).on_hover_text(
                "The main fractal and the objects combined with it, in order: each object is added to, cut away from or intersected with everything above it (Mandelbulber's boolean mode)",
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(scene.objects.len() < MAX_OBJECTS, |ui| {
                    ui.menu_button(icons::PLUS, |ui| {
                        let cam = &scene.camera;
                        // New objects appear in front of the camera.
                        let ahead = cam.position + cam.forward * rmb_core::render::cpu::distance_at_camera(scene).clamp(0.2, 10.0).max(1.0);
                        let mut add = None;
                        if ui.button("Fractal").on_hover_text("Another fractal with its own formulas, placement and colors").clicked() {
                            let f = Fractal { slots: vec![Slot::new("Amazing Box")], max_iterations: 12, bailout: 16.0, ..Default::default() };
                            add = Some(SceneObject { name: "Fractal".into(), shape: Shape::Fractal(Box::new(f)), transform: Transform { position: ahead, scale: 0.15, ..Default::default() }, ..Default::default() });
                        }
                        ui.separator();
                        for p in Primitive::defaults() {
                            let tip = match p {
                                Primitive::Plane => "A ground: everything below it is solid",
                                Primitive::Water { .. } => "An ocean surface: choppy waves with foam, reflecting the sky and the scene; the waves move in animations (see-through water: give its colors transparency and turn on Glass in the Color tab)",
                                _ => "A basic shape",
                            };
                            if ui.button(p.name()).on_hover_text(tip).clicked() {
                                add = Some(primitive_object(p, ahead));
                            }
                        }
                        ui.separator();
                        if ui.button("3D model…").on_hover_text("A 3D model file (OBJ, STL, PLY, glTF/GLB, FBX; from Blender: File › Export › glTF 2.0 or Wavefront OBJ)").clicked()
                            && let Some(p) = crate::prefs::dialog(crate::prefs::Folder::Models).add_filter("3D model", &MODEL_FILES).pick_file()
                        {
                            add = Some(model_object(&p, ahead));
                        }
                        if let Some(o) = add {
                            scene.objects.push(o);
                            *selected = Some(scene.objects.len() - 1);
                            ui.close();
                        }
                    })
                    .response
                    .on_hover_text("Add an object (fractal, basic shape or 3D model)");
                });
            });
        });
        let mut action: Option<(usize, i32)> = None;
        let row = |ui: &mut Ui, sel: bool, icon: &str, name: &str, badge: &str, tip: &str| -> bool {
            let text = format!("{icon}  {name}");
            let r = ui.add(egui::Button::selectable(sel, RichText::new(text)).min_size(egui::vec2(ui.available_width() - 70.0, 24.0)));
            if !badge.is_empty() {
                ui.label(RichText::new(badge).color(theme::TEXT_MUTED).monospace());
            }
            r.on_hover_text(tip).clicked()
        };
        ui.horizontal(|ui| {
            if row(ui, selected.is_none(), icons::ATOM, "Main fractal", "", "The main fractal (formulas, colors and all settings of the inspector)") {
                *selected = None;
            }
        });
        for (k, o) in scene.objects.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let icon = match o.shape {
                    Shape::Fractal(_) => icons::ATOM,
                    Shape::Primitive(_) => icons::CUBE,
                    Shape::Model(_) => egui_phosphor::regular::SHAPES,
                };
                let badge = match o.op {
                    BoolOp::Union => "+",
                    BoolOp::Subtract => "−",
                    BoolOp::Intersect => "∩",
                    BoolOp::SmoothUnion => "~+",
                    BoolOp::SmoothSubtract => "~−",
                    BoolOp::SmoothIntersect => "~∩",
                };
                let label = format!("{icon}  {}", o.name);
                let name = if o.enabled { RichText::new(label) } else { RichText::new(label).color(theme::TEXT_MUTED) };
                let r = ui.add(egui::Button::selectable(*selected == Some(k), name).min_size(egui::vec2(ui.available_width() - 70.0, 24.0)));
                if r.on_hover_text(format!("{} ({})", o.name, o.op.label())).clicked() {
                    *selected = Some(k);
                }
                ui.label(RichText::new(badge).color(theme::TEXT_MUTED).monospace()).on_hover_text(o.op.label());
                let eye = if o.enabled { egui_phosphor::regular::EYE } else { egui_phosphor::regular::EYE_SLASH };
                if widgets::icon_button(ui, eye, "Show or hide this object").clicked() {
                    o.enabled = !o.enabled;
                }
            });
        }
        if let Some(k) = *selected {
            ui.horizontal(|ui| {
                if widgets::icon_button(ui, icons::CARET_UP, "Move up (the order decides what is cut from what)").clicked() && k > 0 {
                    action = Some((k, -1));
                }
                if widgets::icon_button(ui, icons::CARET_DOWN, "Move down").clicked() && k + 1 < scene.objects.len() {
                    action = Some((k, 1));
                }
                if scene.objects.len() < MAX_OBJECTS && widgets::icon_button(ui, egui_phosphor::regular::COPY, "Duplicate this object").clicked() {
                    action = Some((k, 2));
                }
                if widgets::icon_button(ui, icons::TRASH, "Remove this object").clicked() {
                    action = Some((k, 3));
                }
            });
        }
        match action {
            Some((k, -1)) => {
                scene.objects.swap(k, k - 1);
                *selected = Some(k - 1);
            }
            Some((k, 1)) => {
                scene.objects.swap(k, k + 1);
                *selected = Some(k + 1);
            }
            Some((k, 2)) => {
                let mut c = scene.objects[k].clone();
                c.name = format!("{} copy", c.name);
                scene.objects.insert(k + 1, c);
                *selected = Some(k + 1);
            }
            Some((k, 3)) => {
                scene.objects.remove(k);
                *selected = None;
            }
            _ => {}
        }
    }

    /// Editor of a composed object.
    fn object(&mut self, ui: &mut Ui, o: &mut SceneObject, cam: &rmb_core::scene::Camera, glass: bool) {
        widgets::grid(ui, "object", |ui| {
            widgets::label(ui, "Name", "Name of the object");
            ui.text_edit_singleline(&mut o.name);
            ui.end_row();
            widgets::checkbox(ui, "Visible", "Include this object in the scene", &mut o.enabled);
            widgets::label(ui, "Operation", "How the object is combined with everything above it in the list");
            egui::ComboBox::from_id_salt("obj-op").selected_text(o.op.label()).show_ui(ui, |ui| {
                for op in BoolOp::ALL {
                    ui.selectable_value(&mut o.op, op, op.label());
                }
            });
            ui.end_row();
            if o.op.smooth() {
                widgets::slider_log(ui, "Blend", "Distance over which the object melts into the others (world units)", &mut o.smooth, 0.0001..=2.0, 0.1);
            }
        });
        if widgets::section(ui, "Placement", false).open {
            placement(ui, &mut o.transform, cam, "obj-place", true);
        }
        if widgets::section(ui, "Repetition", false).open {
            widgets::grid(ui, "obj-repeat", |ui| {
                for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
                    let tip = format!(
                        "Copies of the object along its own {axis} axis, this far apart (in its own units, before the scale; 0: no copies). Keep it larger than the object, or the copies cut into each other. Double click: 0"
                    );
                    widgets::label(ui, &format!("Spacing {axis}"), &tip);
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::DragValue::new(&mut o.repeat[k]).speed(0.01).range(0.0..=1.0e6).max_decimals(4)).on_hover_text(&tip);
                        if r.double_clicked() {
                            o.repeat[k] = 0.0;
                        }
                        if o.repeat[k] > 0.0 {
                            let tip = "Copies on each side of the original (0: endless). Double click: 0";
                            let r = ui.add(egui::DragValue::new(&mut o.repeat_limit[k]).speed(0.1).range(0..=10000).prefix("× ")).on_hover_text(tip);
                            if r.double_clicked() {
                                o.repeat_limit[k] = 0;
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        }
        match &mut o.shape {
            Shape::Primitive(p) => {
                if widgets::section(ui, p.name(), false).open {
                    primitive_params(ui, p);
                }
            }
            Shape::Fractal(f) => {
                ui.separator();
                self.chain(ui, f);
                ui.add_space(4.0);
                ui.separator();
                self.params(ui, f);
                ui.separator();
                iteration(ui, f, false);
            }
            Shape::Model(m) => model_editor(ui, m),
        }
        let patterns: &[Pattern] = match o.shape {
            Shape::Fractal(_) => &[],
            Shape::Primitive(_) => &Pattern::ALL,
            Shape::Model(_) => &Pattern::FOR_MODELS,
        };
        object_material(ui, &mut o.material, patterns, glass);
    }

    fn chain(&mut self, ui: &mut Ui, f: &mut Fractal) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("HYBRID").size(theme::fs(10.5)).color(theme::TEXT_MUTED).strong())
                .on_hover_text("Up to nine formulas. Each slot runs its formula the given number of times in a row, then the next slot follows. After the last slot the chain repeats from the 'Repeat from' slot.");
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
            if widgets::section(ui, &slot.formula, false).open {
                ui.label(RichText::new("This formula is not in the library. Mandelbulb3D formulas are added over time; you can also write your own (Help › Formulas).").color(theme::TEXT_MUTED));
            }
            return;
        };
        let def = &fm.def;
        let sec = widgets::section(ui, &format!("{} · slot {}", def.name, self.selected + 1), true);
        if sec.reset {
            slot.params.clear();
        }
        if sec.open {
            if !def.description.is_empty() {
                ui.label(RichText::new(&def.description).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                if ui.small_button(format!("{}  Edit code", icons::ATOM)).on_hover_text("Open this formula in the formula editor (built-in formulas open as a copy)").clicked() {
                    self.editor.open_formula(&def.name);
                }
                if ui.small_button(format!("{}  New formula", icons::PLUS)).on_hover_text("Write a new formula in the formula editor; Apply puts it into this slot").clicked() {
                    self.editor.open_new();
                }
                if !fm.builtin {
                    ui.label(RichText::new(format!("from {}", fm.source)).size(theme::fs(10.5)).color(theme::TEXT_MUTED));
                }
            });
            ui.add_space(2.0);
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
}

/// Mandelbulber's per-slot settings of the main fractal: iteration range, weight, adding the
/// constant, escape test.
fn slot_settings(ui: &mut Ui, slot: &mut Slot, index: usize) {
    let sec = widgets::section(ui, &format!("Slot {} settings", index + 1), slot.has_extras());
    if sec.reset {
        let (formula, iterations, params) = (slot.formula.clone(), slot.iterations, slot.params.clone());
        *slot = Slot { formula, iterations, params, ..Slot::default() };
    }
    if !sec.open {
        return;
    }
    widgets::grid(ui, "slot-settings", |ui| {
        widgets::slider_u32(ui, "From iteration", "The slot runs only from this iteration on (0: from the start; transforms do not count). Before it the chain skips the slot (Mandelbulber's start iteration)", &mut slot.start, 0..=100, 0);
        widgets::label(ui, "To iteration", "The slot runs only up to this iteration (inclusive); after it the chain skips the slot (Mandelbulber's stop iteration). Off: to the end");
        ui.horizontal(|ui| {
            let mut limited = slot.stop.is_some();
            if ui.checkbox(&mut limited, "").on_hover_text("Limit the slot to a last iteration").changed() {
                slot.stop = limited.then_some(slot.start.max(5));
            }
            if let Some(stop) = &mut slot.stop {
                widgets::reset_on_double_click(ui.add(egui::DragValue::new(stop).range(0..=1000).speed(0.1)).on_hover_text("Last iteration of the slot. Double click: 5"), stop, 5);
            }
        });
        ui.end_row();
        widgets::slider(ui, "Weight", "Share of the formula's result (Mandelbulber's formula weight): below 1 the point blends with where it was before the formula, for soft hybrids", &mut slot.weight, 0.0..=1.0, 1.0);
        widgets::checkbox(ui, "Add C", "The formula adds the constant (the point's position, or the Julia constant). Off: it runs as if the constant were zero (Mandelbulber's \"don't add c\")", &mut slot.add_c);
        widgets::checkbox(ui, "Escape test", "Test for escape after this slot. Off: points may grow past the bailout here and come back in later slots (Mandelbulber's \"check for bailout\")", &mut slot.check_bailout);
    });
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

/// Position, rotation, scale and stretch of an object (`gizmos`: it has handles in the view)
/// or the main fractal.
fn placement(ui: &mut Ui, t: &mut Transform, cam: &rmb_core::scene::Camera, id: &str, gizmos: bool) {
    let hint = |text: &str, handles: &str| if gizmos { format!("{text}; or {handles}") } else { text.to_string() };
    widgets::grid(ui, id, |ui| {
        let speed = (t.position.length() * 0.002).max(1e-4);
        widgets::drag(ui, "X", &hint("Position along the world x axis (red)", "drag the arrows in the view"), &mut t.position.x, speed, 0.0);
        widgets::drag(ui, "Y", "Position along the world y axis (green)", &mut t.position.y, speed, 0.0);
        widgets::drag(ui, "Z", "Position along the world z axis (blue, up)", &mut t.position.z, speed, 0.0);
        widgets::label(ui, "", "");
        if ui.small_button("In front of the camera").on_hover_text("Moves it to the middle of the view").clicked() {
            t.position = cam.position + cam.forward * 2.0;
        }
        ui.end_row();
        for (k, axis) in ["Turn X", "Turn Y", "Turn Z"].iter().enumerate() {
            let tip = hint("Rotation in degrees (about x, then y, then z)", "drag the rings in the view (Rotate in the toolbar)") + ". Double click: 0";
            widgets::label(ui, axis, &tip);
            let r = ui.add(egui::DragValue::new(&mut t.rotation[k]).speed(0.5).max_decimals(2).suffix("°")).on_hover_text(&tip);
            if r.double_clicked() {
                t.rotation[k] = 0.0;
            }
            ui.end_row();
        }
        widgets::slider_log(ui, "Scale", &hint("Size as a whole", "drag its center in the view (Scale in the toolbar)"), &mut t.scale, 0.0001..=100.0, 1.0);
        for (k, axis) in ["Stretch X", "Stretch Y", "Stretch Z"].iter().enumerate() {
            let tip = hint("Extra size along its own axis (1: none)", "drag the axis handles in the view (Scale in the toolbar)") + ". Double click: 1";
            widgets::label(ui, axis, &tip);
            let r = ui.add(egui::DragValue::new(&mut t.stretch[k]).speed(0.005).max_decimals(3)).on_hover_text(&tip);
            if r.double_clicked() {
                t.stretch[k] = 1.0;
            }
            ui.end_row();
            t.stretch[k] = t.stretch[k].clamp(0.001, 1000.0);
        }
    });
}

/// A new primitive object at `at` (planes and water at the ground below it).
fn primitive_object(p: Primitive, at: rmb_core::V3) -> SceneObject {
    let mut material = ObjectMaterial { color_scale: 1.0, repeat: false, ..Default::default() };
    let mut pos = at;
    match p {
        Primitive::Plane => {
            pos = rmb_core::v3(0.0, 0.0, -1.2);
            material.pattern = Pattern::Checker;
            material.pattern_size = 0.5;
            material.gradient = vec![rmb_core::scene::ColorStop::new(0.0, [0.8, 0.8, 0.78]), rmb_core::scene::ColorStop::new(1.0, [0.12, 0.12, 0.15])];
        }
        Primitive::Water { .. } => {
            pos = rmb_core::v3(0.0, 0.0, -1.0);
            material.gradient = vec![rmb_core::scene::ColorStop::new(0.0, [0.0, 0.04, 0.09]), rmb_core::scene::ColorStop::new(1.0, [0.02, 0.1, 0.16])];
        }
        _ => {}
    }
    SceneObject { name: p.name().into(), shape: Shape::Primitive(p), transform: Transform { position: pos, ..Default::default() }, material, ..Default::default() }
}

/// The size parameters of a primitive.
fn primitive_params(ui: &mut Ui, p: &mut Primitive) {
    if matches!(p, Primitive::Plane) {
        ui.add(egui::Label::new(RichText::new("The ground: everything below it is solid. Turn it for walls.").color(theme::TEXT_MUTED)).wrap());
        return;
    }
    widgets::grid(ui, "prim", |ui| match p {
        Primitive::Plane => {}
        Primitive::Box { size, rounding } => {
            for (k, n) in ["Width", "Depth", "Height"].iter().enumerate() {
                widgets::slider_log(ui, n, "Size of the box (before the object's scale)", &mut size[k], 0.001..=100.0, 1.0);
            }
            widgets::slider_log(ui, "Rounding", "Radius of the rounded edges", rounding, 0.0..=10.0, 0.0);
        }
        Primitive::Sphere { radius } => {
            widgets::slider_log(ui, "Radius", "Radius of the sphere", radius, 0.001..=100.0, 0.5);
        }
        Primitive::Cylinder { radius, height, rounding } => {
            widgets::slider_log(ui, "Radius", "Radius of the cylinder", radius, 0.001..=100.0, 0.3);
            widgets::slider_log(ui, "Height", "Height of the cylinder (along its z axis)", height, 0.001..=100.0, 1.0);
            widgets::slider_log(ui, "Rounding", "Radius of the rounded edges", rounding, 0.0..=10.0, 0.0);
        }
        Primitive::Torus { major, minor } => {
            widgets::slider_log(ui, "Ring radius", "Radius of the ring", major, 0.001..=100.0, 0.6);
            widgets::slider_log(ui, "Thickness", "Radius of the tube", minor, 0.001..=100.0, 0.15);
        }
        Primitive::Capsule { radius, height } => {
            widgets::slider_log(ui, "Radius", "Radius of the capsule", radius, 0.001..=100.0, 0.25);
            widgets::slider_log(ui, "Height", "Length from end to end, the round ends included (along its z axis)", height, 0.001..=100.0, 1.0);
        }
        Primitive::Cone { bottom, top, height } => {
            widgets::slider_log(ui, "Bottom radius", "Radius at the bottom", bottom, 0.0..=100.0, 0.4);
            widgets::slider_log(ui, "Top radius", "Radius at the top (0: pointed)", top, 0.0..=100.0, 0.0);
            widgets::slider_log(ui, "Height", "Height of the cone (along its z axis)", height, 0.001..=100.0, 1.0);
        }
        Primitive::HexPrism { radius, height } => {
            widgets::slider_log(ui, "Radius", "Distance from the axis to the middle of a side", radius, 0.001..=100.0, 0.4);
            widgets::slider_log(ui, "Height", "Height of the prism (along its z axis)", height, 0.001..=100.0, 1.0);
        }
        Primitive::TriPrism { side, height } => {
            widgets::slider_log(ui, "Side", "Side length of the triangle", side, 0.001..=100.0, 0.8);
            widgets::slider_log(ui, "Height", "Height of the prism (along its z axis)", height, 0.001..=100.0, 1.0);
        }
        Primitive::Water { amplitude, wavelength, phase, angle, choppy, detail, foam, reflect, speed } => {
            widgets::slider_log(ui, "Wave height", "Height of the largest waves", amplitude, 0.0..=10.0, 0.08);
            widgets::slider_log(ui, "Wave length", "Length of the largest waves (across the wind they are a bit shorter)", wavelength, 0.001..=100.0, 1.0);
            widgets::slider(ui, "Choppiness", "Shape of the crests: 0 = round swell, 1 = like the sea, 2 = very choppy", choppy, 0.0..=2.0, 1.0);
            widgets::slider(ui, "Detail", "Number of waves, from long swells to small ripples: more give a richer sea (and cost speed)", detail, 4.0..=64.0, 32.0);
            widgets::slider(ui, "Direction", "Wind direction in degrees: the waves are longer along it", angle, -180.0..=180.0, 0.0);
            widgets::slider(ui, "Foam", "White, broken foam on the highest crests, streaked along the wind", foam, 0.0..=1.0, 0.3);
            widgets::slider(ui, "Reflection", "How strongly the water reflects the sky and the scene (1 = like water: weak looking straight down, a mirror at grazing angles). Works without the material's Reflection", reflect, 0.0..=2.0, 1.0);
            widgets::slider(ui, "Speed", "How fast the waves move in animations (per second; 0 = still); long waves run faster than ripples, like on the sea", speed, 0.0..=5.0, 1.0);
            widgets::slider(ui, "Time", "The moment of the waves: change it to pick another wave pattern in a still image (animations add Speed per second)", phase, -100.0..=100.0, 0.0);
        }
    });
}

/// The material of an object.
/// The material of an object. `patterns`: how primitives and models pick their colors (empty:
/// a fractal, colored by its orbit).
fn object_material(ui: &mut Ui, m: &mut ObjectMaterial, patterns: &[Pattern], glass: bool) {
    let primitive = !patterns.is_empty();
    if widgets::section(ui, "Material", false).open {
        ui.checkbox(&mut m.own, "Own colors").on_hover_text("Off: the object uses the main fractal's colors (Color tab)");
        if !m.own {
            return;
        }
        crate::inspector::gradient_editor(ui, &mut m.gradient, glass);
        crate::inspector::bands_editor(ui, &m.gradient, &mut m.bands, m.repeat, "object");
        widgets::grid(ui, "obj-mat", |ui| {
            if primitive {
                widgets::label(ui, "Pattern", "How the shape picks its colors from the gradient");
                egui::ComboBox::from_id_salt("obj-pattern").selected_text(m.pattern.label()).show_ui(ui, |ui| {
                    for &p in patterns {
                        ui.selectable_value(&mut m.pattern, p, p.label());
                    }
                });
                ui.end_row();
                if !matches!(m.pattern, Pattern::Solid | Pattern::Model) {
                    widgets::slider_log(ui, "Pattern size", "Size of the checker cells, or the height / distance per gradient length", &mut m.pattern_size, 0.001..=100.0, 1.0);
                }
            } else {
                widgets::label(ui, "Color by", "Which value of the orbit picks the gradient color (the position modes and MB3D's 2nd choice use the settings of the main material, Color tab)");
                egui::ComboBox::from_id_salt("obj-color-mode").selected_text(m.color_mode.label()).show_ui(ui, |ui| {
                    for c in ColorMode::ALL {
                        ui.selectable_value(&mut m.color_mode, c, c.label());
                    }
                });
                ui.end_row();
            }
            widgets::slider_log(ui, "Scale", "How fast the colors change with the value", &mut m.color_scale, -10.0..=10.0, if primitive { 1.0 } else { 0.15 });
            widgets::slider(ui, "Offset", "Shifts the gradient (with a solid pattern: picks the color)", &mut m.color_offset, -5.0..=5.0, 0.0);
            widgets::drag(ui, "Rotation", "Animation: the gradient (with its glow bands) turns by this many gradient lengths per second (palette rotation; 0 = off)", &mut m.rotation, 0.005, 0.0);
            widgets::checkbox(ui, "Repeat", "Repeat the gradient (else clamp at its ends)", &mut m.repeat);
        });
        crate::layers::layers_editor(ui, &mut m.layers, "object");
    }
}

/// File types of 3D models.
const MODEL_FILES: [&str; 6] = ["obj", "stl", "ply", "gltf", "glb", "fbx"];

/// A new object for the model file `p`, placed at `pos`.
fn model_object(p: &std::path::Path, pos: rmb_core::V3) -> SceneObject {
    let name = p.file_stem().map_or_else(|| "Model".to_string(), |s| s.to_string_lossy().into_owned());
    SceneObject {
        name,
        shape: Shape::Model(ModelRef { path: p.display().to_string(), ..Default::default() }),
        transform: Transform { position: pos, scale: 0.5, ..Default::default() },
        material: ObjectMaterial { pattern: Pattern::Model, ..Default::default() },
        ..Default::default()
    }
}

/// File and bake settings of a 3D model, with the bake state.
fn model_editor(ui: &mut Ui, m: &mut ModelRef) {
    use rmb_core::mesh::{Status, UpAxis};
    if widgets::section(ui, "3D model", false).open {
        ui.horizontal(|ui| {
            if ui.button("Change…").on_hover_text("Pick another model file").clicked()
                && let Some(p) = crate::prefs::dialog(crate::prefs::Folder::Models).add_filter("3D model", &MODEL_FILES).pick_file()
            {
                m.path = p.display().to_string();
                m.materials.clear();
                m.auto_assigned = false;
            }
            let name = std::path::Path::new(&m.path).file_name().map_or_else(|| m.path.clone(), |f| f.to_string_lossy().into_owned());
            ui.label(name).on_hover_text(m.path.clone());
        });
        match rmb_core::mesh::request(&m.path, &m.options) {
            Status::Ready(v) => {
                ui.label(RichText::new(format!("{} triangles, {} materials, {} × {} × {} grid", v.tris.len(), v.materials.len(), v.res[0], v.res[1], v.res[2])).size(theme::fs(11.0)).color(theme::TEXT_MUTED))
                    .on_hover_text("The grid gives the distance away from the model; near its surface the triangles themselves are used, so detail stays sharp at any distance");
            }
            Status::Baking(p) => {
                ui.add(egui::ProgressBar::new(p).show_percentage().text("Baking…")).on_hover_text("Turning the model into a distance grid (it appears when done)");
            }
            Status::Failed(e) => {
                ui.label(RichText::new(e).size(theme::fs(11.0)).color(theme::WARN));
            }
        }
        let o = &mut m.options;
        widgets::grid(ui, "model", |ui| {
            widgets::label(ui, "Resolution", "Grid points along the longest side of the distance grid used away from the surface. Higher makes rendering faster near complex models but bakes longer and needs more memory (256: about 32 MB)");
            egui::ComboBox::from_id_salt("model-res").selected_text(o.resolution.to_string()).show_ui(ui, |ui| {
                for r in [64, 96, 128, 192, 256] {
                    ui.selectable_value(&mut o.resolution, r, r.to_string());
                }
            });
            ui.end_row();
            widgets::label(ui, "Up axis", "Which axis of the file points up. Auto: Y for glTF and OBJ (Blender's export default), Z for STL and PLY");
            egui::ComboBox::from_id_salt("model-up").selected_text(format!("{:?}", o.up_axis)).show_ui(ui, |ui| {
                for a in [UpAxis::Auto, UpAxis::Y, UpAxis::Z] {
                    ui.selectable_value(&mut o.up_axis, a, format!("{a:?}"));
                }
            });
            ui.end_row();
            widgets::checkbox(ui, "Keep file units", "Off: the model is centered and sized to 2 units (then scaled by the placement). On: the file's own coordinates", &mut o.keep_units);
        });
        if o.resolution > 192 {
            ui.label(RichText::new("High resolutions take a few seconds to bake.").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
        }
        if let Status::Ready(v) = rmb_core::mesh::request(&m.path, &m.options) {
            materials_editor(ui, m, &v.materials);
        }
    }
}

/// Assigns the images near each newly baked model to its materials by name (once per model).
fn auto_assign_textures(scene: &mut Scene) {
    for o in &mut scene.objects {
        let Shape::Model(m) = &mut o.shape else { continue };
        if m.auto_assigned {
            continue;
        }
        if let rmb_core::mesh::Status::Ready(v) = rmb_core::mesh::request(&m.path, &m.options) {
            let images = rmb_core::mesh::nearby_images(std::path::Path::new(&m.path));
            m.materials = rmb_core::mesh::auto_assign(&m.path, &v.materials, &images);
            m.auto_assigned = true;
        }
    }
}

/// The override of material `name` (created if missing).
fn material_override<'a>(list: &'a mut Vec<MaterialOverride>, name: &str) -> &'a mut MaterialOverride {
    let i = match list.iter().position(|o| o.name == name) {
        Some(i) => i,
        None => {
            list.push(MaterialOverride { name: name.to_string(), ..Default::default() });
            list.len() - 1
        }
    };
    &mut list[i]
}

/// The file name of an image path for display.
fn short_name(p: &str) -> String {
    std::path::Path::new(p).file_name().map_or_else(|| p.to_string(), |f| f.to_string_lossy().into_owned())
}

/// Picks an image for a material: from the file, none, an image near the model or any file.
/// `value`: None = the file's own, Some("") = none, else the image.
fn image_combo(ui: &mut Ui, id: &str, model: &str, file: &str, images: &[std::path::PathBuf], value: &mut Option<String>, tip: &str) {
    let text = match value.as_deref() {
        None if file.is_empty() => "None".to_string(),
        None => format!("{} (file)", short_name(file)),
        Some("") => "None".to_string(),
        Some(p) => short_name(p),
    };
    egui::ComboBox::from_id_salt(id)
        .width(150.0)
        .selected_text(text)
        .show_ui(ui, |ui| {
            if !file.is_empty() {
                ui.selectable_value(value, None, format!("{} (from the model file)", short_name(file)));
            }
            ui.selectable_value(value, Some(String::new()), "None");
            ui.separator();
            for p in images {
                let name = rmb_core::mesh::image_name(model, p);
                let selected = value.as_deref() == Some(name.as_str());
                if ui.selectable_label(selected, &name).clicked() {
                    *value = Some(name);
                }
            }
            if ui.button("Browse…").on_hover_text("Pick any image file").clicked()
                && let Some(p) = crate::prefs::dialog(crate::prefs::Folder::Textures).add_filter("Image", &rmb_core::mesh::IMAGE_EXTENSIONS).pick_file()
            {
                *value = Some(rmb_core::mesh::image_name(model, &p));
            }
        })
        .response
        .on_hover_text(tip);
}

/// Colors and textures of the model's materials (changes are saved with the scene).
fn materials_editor(ui: &mut Ui, m: &mut ModelRef, mats: &[rmb_core::mesh::MeshMaterial]) {
    if mats.is_empty() {
        return;
    }
    if widgets::section(ui, "Materials", false).open {
        let id = ui.id().with(("model-materials", &m.path));
        let mut filter: String = ui.data_mut(|d| d.get_temp(id.with("filter")).unwrap_or_default());
        let mut bulk: [f32; 3] = ui.data_mut(|d| d.get_temp(id.with("bulk")).unwrap_or([0.05, 0.12, 0.45]));
        let images: std::sync::Arc<Vec<std::path::PathBuf>> = ui.data_mut(|d| d.get_temp(id.with("images"))).unwrap_or_else(|| {
            let list = std::sync::Arc::new(rmb_core::mesh::nearby_images(std::path::Path::new(&m.path)));
            ui.data_mut(|d| d.insert_temp(id.with("images"), list.clone()));
            list
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Filter").desired_width(110.0)).on_hover_text("Show only the materials whose name contains this text");
            if ui.button("Auto-assign").on_hover_text("Assign the images near the model file to the materials by name (replaces the image choices, keeps the colors)").clicked() {
                let auto = rmb_core::mesh::auto_assign(&m.path, mats, &images);
                for o in &mut m.materials {
                    o.texture = None;
                    o.bump.clear();
                    o.specular.clear();
                    o.normal.clear();
                }
                for a in auto {
                    let o = material_override(&mut m.materials, &a.name);
                    o.texture = a.texture;
                    o.bump = a.bump;
                    o.specular = a.specular;
                    o.normal = a.normal;
                }
            }
            if ui.button("Rescan").on_hover_text(format!("Look for images near the model again ({} found)", images.len())).clicked() {
                ui.data_mut(|d| d.remove::<std::sync::Arc<Vec<std::path::PathBuf>>>(id.with("images")));
            }
            if ui.button("Reset").on_hover_text("Use the materials exactly as the model file has them").clicked() {
                m.materials.clear();
            }
        });
        let needle = filter.to_lowercase();
        let shown: Vec<&rmb_core::mesh::MeshMaterial> = mats.iter().filter(|mm| needle.is_empty() || mm.name.to_lowercase().contains(&needle)).collect();
        ui.horizontal(|ui| {
            widgets::color_button(ui, &mut bulk).on_hover_text("Color for 'Color all shown'");
            if ui.button(format!("Color all shown ({})", shown.len())).on_hover_text("Gives every material in the list below this color (use the filter to pick them)").clicked() {
                for mm in &shown {
                    material_override(&mut m.materials, &mm.name).color = Some(bulk);
                }
            }
        });
        let mut open: Option<String> = ui.data_mut(|d| d.get_temp(id.with("open")).unwrap_or_default());
        egui::ScrollArea::vertical().id_salt(id.with("scroll")).min_scrolled_height(320.0).max_height(360.0).show(ui, |ui| {
            for mm in &shown {
                let o = material_override(&mut m.materials, &mm.name);
                let expanded = open.as_deref() == Some(mm.name.as_str());
                ui.horizontal(|ui| {
                    let mut c = o.color.unwrap_or(mm.color);
                    let r = widgets::color_button(ui, &mut c).on_hover_text("Base color (the texture multiplies it). Right click: the model file's color");
                    if c != o.color.unwrap_or(mm.color) {
                        o.color = Some(c);
                    }
                    if r.secondary_clicked() {
                        o.color = None;
                    }
                    let maps = !o.bump.is_empty() || !o.specular.is_empty() || !o.normal.is_empty() || mm.normal.is_some() || o.emission.is_some() || mm.emissive.iter().any(|c| *c > 0.0);
                    let changed = o.color.is_some() || o.texture.is_some() || maps;
                    let arrow = if expanded { egui_phosphor::regular::CARET_DOWN } else { egui_phosphor::regular::CARET_RIGHT };
                    let text = format!("{arrow} {}{}", mm.name, if maps { format!(" {}", egui_phosphor::regular::STACK) } else { String::new() });
                    let label = if changed { RichText::new(text).strong() } else { RichText::new(text) };
                    let size = egui::vec2(150.0, ui.spacing().interact_size.y);
                    let resp = ui
                        .allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.set_min_size(size);
                            ui.add(egui::Button::new(label).frame(false).truncate())
                        })
                        .inner
                        .on_hover_text(format!("{}: click for bump, specular and normal maps and emission (bold: changed here; layers icon: has maps or emission)", mm.name));
                    if resp.clicked() {
                        open = if expanded { None } else { Some(mm.name.clone()) };
                    }
                    image_combo(ui, &format!("mat-tex-{}", mm.name), &m.path, &mm.texture_file, &images, &mut o.texture, "Color texture (multiplies the base color)");
                });
                if expanded {
                    ui.indent(id.with(("maps", &mm.name)), |ui| {
                        widgets::grid(ui, &format!("mat-maps-{}", mm.name), |ui| {
                            widgets::label(ui, "Bump", "Height map: bright is high. Bends the surface normal for fine relief without changing the shape");
                            let mut v = Some(o.bump.clone());
                            image_combo(ui, &format!("mat-bump-{}", mm.name), &m.path, "", &images, &mut v, "Bump (height) map");
                            o.bump = v.unwrap_or_default();
                            ui.end_row();
                            if !o.bump.is_empty() {
                                widgets::slider_log(ui, "Strength", "Height of the bumps (1: white is 0.3 % of the model's size above black)", &mut o.bump_strength, 0.01..=20.0, 1.0);
                            }
                            widgets::label(ui, "Specular", "Specular map: bright parts shine, dark parts stay matte");
                            let mut v = Some(o.specular.clone());
                            image_combo(ui, &format!("mat-spec-{}", mm.name), &m.path, "", &images, &mut v, "Specular map");
                            o.specular = v.unwrap_or_default();
                            ui.end_row();
                            widgets::label(ui, "Normal map", "Tangent-space normal map (bluish image: red right, green up): bends the surface normal for fine relief");
                            let mut v = Some(o.normal.clone());
                            let from_file = if mm.normal.is_some() { "(from the model file)" } else { "" };
                            image_combo(ui, &format!("mat-normal-{}", mm.name), &m.path, from_file, &images, &mut v, "Normal map");
                            o.normal = v.unwrap_or_default();
                            ui.end_row();
                            if !o.normal.is_empty() || mm.normal.is_some() {
                                widgets::slider_log(ui, "Normal strength", "How far the normal map bends the surface (1: as drawn)", &mut o.normal_strength, 0.01..=10.0, 1.0);
                            }
                            let file_emission = mm.emissive;
                            let emits = |e: &[f32; 3]| e.iter().any(|c| *c > 0.0);
                            let mut on = o.emission.map_or(emits(&file_emission), |e| emits(&e));
                            let was = on;
                            widgets::checkbox(ui, "Emission", "The material shines by itself in this color (glTF emissive; brighter than white glows with bloom)", &mut on);
                            if on {
                                let e = o.emission.filter(emits).unwrap_or(if emits(&file_emission) { file_emission } else { [1.0; 3] });
                                // Color (brightest channel 1) and strength.
                                let mut k = e.iter().copied().fold(0.0f32, f32::max).max(1e-6);
                                let mut col = e.map(|c| c / k);
                                widgets::color(ui, "Emission color", "Color of the emitted light", &mut col);
                                let mut kk = k as f64;
                                widgets::slider_log(ui, "Emission strength", "Brightness of the emitted light", &mut kk, 0.01..=50.0, 1.0);
                                k = kk as f32;
                                let e2 = col.map(|c| c * k);
                                if !was || e2 != e {
                                    o.emission = Some(e2);
                                }
                            } else if was {
                                // Off: none, also where the file has emission.
                                o.emission = emits(&file_emission).then_some([0.0; 3]);
                            }
                        });
                    });
                }
            }
        });
        m.materials.retain(|o| !o.is_empty());
        ui.data_mut(|d| {
            d.insert_temp(id.with("filter"), filter);
            d.insert_temp(id.with("bulk"), bulk);
            d.insert_temp(id.with("open"), open);
        });
    }
}

fn iteration(ui: &mut Ui, f: &mut Fractal, main: bool) {
    let d = Fractal::default();
    let sec = widgets::section(ui, "Iteration", true);
    if sec.reset {
        let slots = std::mem::take(&mut f.slots);
        let (repeat, transform) = (f.repeat_from, f.transform.clone());
        *f = Fractal { slots, repeat_from: repeat, transform, ..d.clone() };
    }
    if sec.open {
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
            if main {
                widgets::checkbox(
                    ui,
                    "Inside",
                    "Render the inside of the set: escaping points are solid, the set is empty space. For views from within the fractal (slower: rays step in small steps).",
                    &mut f.inside,
                );
            }
            if f.julia {
                for (k, n) in ["Julia X", "Julia Y", "Julia Z", "Julia W"].iter().enumerate() {
                    widgets::drag(ui, n, "Julia constant", &mut f.julia_c[k], 0.002, 0.0);
                }
            }
            if main {
                for (k, n) in ["Rotate XW", "Rotate YW", "Rotate ZW"].iter().enumerate() {
                    widgets::slider(ui, n, "Rotates the start point into the 4th dimension (formulas that use w)", &mut f.rotation_4d[k], -180.0..=180.0, 0.0);
                }
                widgets::slider_u32(
                    ui,
                    "Min iterations",
                    "No point escapes before this many iterations (Mandelbulber's and Mandelbulb3D's minimum iterations): fills thin or empty parts of hybrids whose first formulas throw points out",
                    &mut f.min_iterations,
                    0..=50,
                    d.min_iterations,
                );
                for (k, n) in ["C factor X", "C factor Y", "C factor Z"].iter().enumerate() {
                    widgets::slider(ui, n, "Multiplies the constant (the point's position, or the Julia constant) per axis before the formulas add it (Mandelbulber's constant factor)", &mut f.c_factor[k], -2.0..=2.0, 1.0);
                }
            }
        });
        combine(ui, f);
    }
}

fn combine(ui: &mut Ui, f: &mut Fractal) {
    if widgets::section(ui, "Two chains", false).open {
        let mut blend = f.blend.is_some();
        if ui
            .checkbox(&mut blend, "Interpolate slots 1 and 2")
            .on_hover_text("Every iteration runs the first two formulas on the same point and blends their results by these weights (Mandelbulb3D's interpolation hybrid); the other slots are not used")
            .changed()
        {
            f.blend = blend.then_some([0.5, 0.5]);
            if blend {
                f.combine = None;
            }
        }
        if let Some(w) = f.blend.as_mut() {
            widgets::grid(ui, "blend", |ui| {
                widgets::slider(ui, "Weight 1", "Share of slot 1", &mut w[0], -1.0..=2.0, 0.5);
                widgets::slider(ui, "Weight 2", "Share of slot 2", &mut w[1], -1.0..=2.0, 0.5);
            });
            if f.slots.len() < 2 {
                ui.label(RichText::new("Needs at least two slots.").color(theme::WARN));
            }
            return;
        }
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
}
