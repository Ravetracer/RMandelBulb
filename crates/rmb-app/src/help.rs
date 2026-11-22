// SPDX-License-Identifier: LGPL-2.1-or-later

//! Help window: getting started, shortcuts, formulas and the formula language, about.

use egui::RichText;

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Start,
    Shortcuts,
    Formulas,
    Language,
    About,
}

#[derive(Default)]
pub struct Help {
    pub open: bool,
    pub tab: Tab,
}

const START: &str = "\
RMandelbulb3D renders 3D fractals by ray marching distance estimates, like Mandelbulb3D.

1. Pick formulas on the left. Up to six formulas form a hybrid: each slot runs its formula the \
given number of times, then the next slot follows.
2. Navigate in the view: drag to orbit around the point under the cursor, wheel to move toward \
the cursor, right drag to pan, double-click to look at a point (and focus there). Switch to Fly \
mode for flights into the fractal (WASD).
3. Adjust light, color and effects on the right. The preview refines over time; while you \
navigate it renders faster at a lower resolution.
4. Export the image (Ctrl+E). The PNG contains the scene: open it to continue editing.

Mandelbulb3D parameters: open .m3p files, or copy the text parameters (Mandelbulb3Dv18{…}) and \
paste them with Ctrl+V. Formulas that are not available yet are listed and skipped.

Fine detail: lower 'Detail' (Ray marching tab). Holes or noise in the surface: lower 'Step' or \
'DE scale'.";

const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+N", "New scene"),
    ("Ctrl+O", "Open (.rmb, .m3p, PNG with scene, MB3D text)"),
    ("Ctrl+S / Ctrl+Shift+S", "Save / Save as"),
    ("Ctrl+E", "Export image"),
    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / Redo"),
    ("Ctrl+C / Ctrl+V", "Copy scene / paste scene or Mandelbulb3D parameters"),
    ("Tab", "Switch Orbit / Fly navigation"),
    ("F1", "Help"),
    ("F12", "Test render window (with denoiser, flares, bloom, streaks)"),
    ("Orbit: left drag", "Orbit around the point under the cursor"),
    ("Orbit: Shift+drag, middle drag", "Look around"),
    ("Right drag", "Pan"),
    ("Wheel", "Orbit: move toward the cursor · Fly: speed"),
    ("Double-click", "Look at the point, set the focus distance"),
    ("Fly: W A S D, R F", "Move forward/left/back/right, up/down (Shift: faster)"),
    ("Fly: Space", "Mouse look: the mouse steers freely (full turns); Space or Esc releases the mouse"),
    ("Q E, arrow keys", "Roll, turn"),
];

const LANGUAGE: &str = r#"Formulas are small text files (*.mbf) in the formula folder (Preferences shows it). They are loaded at start; formulas with the name of a built-in one replace it.

formula "My bulb" {
    category "Bulbs"
    description "Shown as hover text"
    de log                       // log (bulbs), linear (boxes, IFS) or numeric
    alias "MB3D name" (p1, p2)   // import name and option order
    param power "Power" = 8 [2, 16]
    param flip "Flip" = 0 bool    // kinds: int, bool, angle (degrees)
    init {                        // runs once per render (CPU)
        let half = power * 0.5    // top-level lets become constants
    }
    iter {                        // one iteration
        let r = sqrt(x*x + y*y + z*z)
        if r > 1.0 && flip > 0.5 { x = -x } else { y = y * 1.0 }
        for k in 0..3 { z += 0.0 }
        dr = power * pow(r, power - 1.0) * dr + 1.0
        x = ... + cx
    }
}

State: x y z w (point), cx cy cz cw (pixel position or Julia constant), dr (derivative for the distance estimate), i (iteration, read only).
Functions: abs sqrt rsqrt sin cos tan asin acos atan atan2 exp log pow floor ceil fract round min max clamp sign sinh cosh tanh mix step smoothstep sqr sel(cond, a, b). Constants: pi, tau.
Every formula compiles for the GPU (WGSL) and the CPU, so both renderers compute the same."#;

impl Help {
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Help").open(&mut open).default_size([640.0, 560.0]).resizable(true).collapsible(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (t, l) in [(Tab::Start, "Getting started"), (Tab::Shortcuts, "Shortcuts"), (Tab::Formulas, "Formulas"), (Tab::Language, "Formula language"), (Tab::About, "About")] {
                    ui.selectable_value(&mut self.tab, t, l);
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                Tab::Start => {
                    ui.label(START);
                }
                Tab::Shortcuts => {
                    egui::Grid::new("keys").num_columns(2).spacing([16.0, 6.0]).striped(true).show(ui, |ui| {
                        for (k, d) in SHORTCUTS {
                            ui.label(RichText::new(*k).monospace().color(theme::ACCENT));
                            ui.label(*d);
                            ui.end_row();
                        }
                    });
                }
                Tab::Formulas => {
                    let lib = rmb_core::formula::library();
                    for cat in lib.categories() {
                        ui.label(RichText::new(cat.to_uppercase()).size(theme::fs(11.0)).color(theme::TEXT_MUTED).strong());
                        for f in lib.all().iter().filter(|f| f.def.category == cat) {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(&f.def.name).strong());
                                ui.label(RichText::new(&f.def.description).color(theme::TEXT_MUTED));
                                let al: Vec<&str> = f.def.aliases.iter().map(|a| a.name.as_str()).collect();
                                if !al.is_empty() {
                                    ui.label(RichText::new(format!("MB3D: {}", al.join(", "))).size(theme::fs(11.0)).color(theme::ACCENT));
                                }
                            });
                        }
                        ui.add_space(6.0);
                    }
                    if !lib.errors.is_empty() {
                        ui.label(RichText::new("Formula files with errors:").color(theme::WARN));
                        for e in &lib.errors {
                            ui.label(RichText::new(e).monospace().color(theme::WARN));
                        }
                    }
                }
                Tab::Language => {
                    ui.label(RichText::new(LANGUAGE).monospace());
                }
                Tab::About => {
                    ui.label(RichText::new(format!("RMandelbulb3D {}", env!("CARGO_PKG_VERSION"))).size(theme::fs(18.0)).strong());
                    ui.label("A 3D fractal renderer in Rust, after Mandelbulb3D by Jesse (and contributors).");
                    ui.label("GPU rendering with wgpu; user interface with egui.");
                    ui.label("License: GNU LGPL 2.1 or later, like Mandelbulb3D.");
                }
            });
        });
        self.open = open;
    }
}
