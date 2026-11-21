// SPDX-License-Identifier: LGPL-2.1-or-later

//! Top level: layout, menus, actions, shortcuts, files.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use rmb_core::Scene;

use crate::doc::Document;
use crate::export::Export;
use crate::formulas_panel::FormulasPanel;
use crate::help::Help;
use crate::inspector::Inspector;
use crate::prefs::Prefs;
use crate::viewport::{NavMode, Viewport};
use crate::{icons, theme, widgets};

#[derive(Clone, Debug, PartialEq)]
enum Action {
    New,
    Open,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    Export,
    Undo,
    Redo,
    Copy,
    Paste,
    Prefs,
    Help,
    Quit,
}

pub struct App {
    doc: Document,
    view: Viewport,
    formulas: FormulasPanel,
    inspector: Inspector,
    export: Export,
    help: Help,
    prefs: Prefs,
    prefs_open: bool,
    status: Option<(String, Instant, bool)>,
    confirm_close: bool,
    allow_close: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let prefs = Prefs::load();
        crate::icons::install(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, prefs.font_percent);
        rmb_core::formula::load_user_formulas(&crate::prefs::user_formula_dir());
        if let Some(d) = &prefs.formula_dir {
            rmb_core::formula::load_user_formulas(d);
        }
        let mut app = App {
            doc: Document::new(Scene::default()),
            view: Viewport::new(cc.wgpu_render_state.as_ref()),
            formulas: FormulasPanel::default(),
            inspector: Inspector::default(),
            export: Export::new(prefs.export_samples),
            help: Help::default(),
            prefs,
            prefs_open: false,
            status: None,
            confirm_close: false,
            allow_close: false,
        };
        if let Some(p) = initial {
            app.open_path(&p);
        }
        let errors = rmb_core::formula::library().errors.clone();
        if !errors.is_empty() {
            app.notify_err(format!("Some formula files have errors (Help › Formulas): {}", errors[0]));
        }
        app
    }

    fn notify(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now(), false));
    }

    fn notify_err(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now(), true));
    }

    fn save_prefs(&mut self) {
        if let Err(e) = self.prefs.save() {
            self.notify_err(format!("Preferences not saved: {e}"));
        }
    }

    fn open_path(&mut self, p: &Path) {
        match rmb_core::io::load(p) {
            Ok(scene) => {
                let warn = scene.notes.contains("Import notes:");
                let native = p.extension().is_some_and(|e| e == rmb_core::io::EXTENSION);
                self.doc.load(scene, if native { Some(p.to_path_buf()) } else { None });
                self.formulas.selected = 0;
                self.view.reset_navigation();
                self.prefs.last_dir = p.parent().map(Path::to_path_buf);
                self.prefs.add_recent(p.to_path_buf());
                self.save_prefs();
                if warn {
                    self.inspector.tab = crate::inspector::Tab::Notes;
                    self.notify_err(format!("Opened {} with import notes (see the Notes tab)", p.display()));
                } else {
                    self.notify(format!("Opened {}", p.display()));
                }
            }
            Err(e) => self.notify_err(format!("Could not open {}: {e}", p.display())),
        }
    }

    fn save_to(&mut self, p: PathBuf) {
        let p = if p.extension().is_none() { p.with_extension(rmb_core::io::EXTENSION) } else { p };
        match rmb_core::io::save(&self.doc.scene, &p) {
            Ok(()) => {
                self.doc.mark_saved(p.clone());
                self.prefs.last_dir = p.parent().map(Path::to_path_buf);
                self.prefs.add_recent(p.clone());
                self.save_prefs();
                self.notify(format!("Saved {}", p.display()));
            }
            Err(e) => self.notify_err(format!("Save failed: {e}")),
        }
    }

    fn dialog(&self) -> rfd::FileDialog {
        let mut d = rfd::FileDialog::new();
        if let Some(dir) = &self.prefs.last_dir {
            d = d.set_directory(dir);
        }
        d
    }

    fn run(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::New => {
                self.doc.load(Scene::default(), None);
                self.view.reset_navigation();
                self.notify("New scene");
            }
            Action::Open => {
                if let Some(p) = self
                    .dialog()
                    .add_filter("Scenes", &["rmb", "m3p", "png", "txt"])
                    .add_filter("RMandelbulb3D scene", &["rmb"])
                    .add_filter("Mandelbulb3D parameters", &["m3p", "txt"])
                    .add_filter("PNG with scene", &["png"])
                    .pick_file()
                {
                    self.open_path(&p);
                }
            }
            Action::OpenPath(p) => self.open_path(&p),
            Action::Save => match self.doc.path.clone() {
                Some(p) => self.save_to(p),
                None => self.run(Action::SaveAs, ctx),
            },
            Action::SaveAs => {
                if let Some(p) = self.dialog().add_filter("RMandelbulb3D scene", &["rmb"]).set_file_name("scene.rmb").save_file() {
                    self.save_to(p);
                }
            }
            Action::Export => self.export.open_for(&self.doc.scene, self.doc.path.as_ref()),
            Action::Undo => self.doc.undo(),
            Action::Redo => self.doc.redo(),
            Action::Copy => {
                ctx.copy_text(self.doc.scene.to_json());
                self.notify("Scene copied to the clipboard");
            }
            Action::Paste => {
                // The text arrives as a paste event (see `ui`); trigger it via the viewport command.
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            Action::Prefs => self.prefs_open = true,
            Action::Help => self.help.open = true,
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn paste_text(&mut self, text: &str) {
        match rmb_core::io::parse_text(text) {
            Ok(scene) => {
                let warn = scene.notes.contains("Import notes:");
                self.doc.replace(scene);
                self.formulas.selected = 0;
                self.view.reset_navigation();
                if warn {
                    self.inspector.tab = crate::inspector::Tab::Notes;
                    self.notify_err("Pasted parameters with import notes (see the Notes tab)");
                } else {
                    self.notify("Scene pasted");
                }
            }
            Err(e) => self.notify_err(format!("Paste: {e}")),
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let c = Modifiers::COMMAND;
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        ctx.input_mut(|i| {
            if i.consume_shortcut(&KeyboardShortcut::new(cs, Key::Z)) || i.consume_shortcut(&KeyboardShortcut::new(c, Key::Y)) {
                return Some(Action::Redo);
            }
            if i.consume_shortcut(&KeyboardShortcut::new(c, Key::Z)) {
                return Some(Action::Undo);
            }
            if i.consume_shortcut(&KeyboardShortcut::new(cs, Key::S)) {
                return Some(Action::SaveAs);
            }
            for (k, a) in [(Key::S, Action::Save), (Key::O, Action::Open), (Key::N, Action::New), (Key::E, Action::Export), (Key::Q, Action::Quit)] {
                if i.consume_shortcut(&KeyboardShortcut::new(c, k)) {
                    return Some(a);
                }
            }
            if i.consume_key(Modifiers::NONE, Key::F1) {
                return Some(Action::Help);
            }
            None
        })
    }

    fn top_bar(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(icons::ATOM).color(theme::ACCENT).size(theme::fs(19.0)));
            ui.label(RichText::new("RMandelbulb3D").strong().size(theme::fs(15.0)));
            ui.add_space(10.0);
            ui.menu_button("File", |ui| {
                ui.set_min_width(220.0);
                let item = |ui: &mut Ui, t: &str, k: &str| ui.add(egui::Button::new(t).shortcut_text(k));
                if item(ui, "New", "Ctrl+N").clicked() {
                    action = Some(Action::New);
                }
                if item(ui, "Open…", "Ctrl+O").on_hover_text("RMandelbulb3D scenes, Mandelbulb3D .m3p files and text parameters, PNGs rendered by RMandelbulb3D").clicked() {
                    action = Some(Action::Open);
                }
                ui.menu_button("Open recent", |ui| {
                    if self.prefs.recent.is_empty() {
                        ui.label(RichText::new("Nothing yet").color(theme::TEXT_MUTED));
                    }
                    for p in self.prefs.recent.clone() {
                        if ui.button(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).on_hover_text(p.display().to_string()).clicked() {
                            action = Some(Action::OpenPath(p));
                        }
                    }
                });
                ui.separator();
                if item(ui, "Save", "Ctrl+S").clicked() {
                    action = Some(Action::Save);
                }
                if item(ui, "Save as…", "Ctrl+Shift+S").clicked() {
                    action = Some(Action::SaveAs);
                }
                ui.separator();
                if item(ui, "Export image…", "Ctrl+E").clicked() {
                    action = Some(Action::Export);
                }
                ui.separator();
                if item(ui, "Quit", "Ctrl+Q").clicked() {
                    action = Some(Action::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                ui.set_min_width(240.0);
                if ui.add_enabled(self.doc.can_undo(), egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() {
                    action = Some(Action::Undo);
                }
                if ui.add_enabled(self.doc.can_redo(), egui::Button::new("Redo").shortcut_text("Ctrl+Shift+Z")).clicked() {
                    action = Some(Action::Redo);
                }
                ui.separator();
                if ui.add(egui::Button::new("Copy scene").shortcut_text("Ctrl+C")).on_hover_text("The scene as text, to paste into a message or another instance").clicked() {
                    action = Some(Action::Copy);
                }
                if ui.add(egui::Button::new("Paste scene").shortcut_text("Ctrl+V")).on_hover_text("An RMandelbulb3D scene or Mandelbulb3D text parameters (Mandelbulb3Dv18{…})").clicked() {
                    action = Some(Action::Paste);
                }
                ui.separator();
                if ui.button("Preferences…").clicked() {
                    action = Some(Action::Prefs);
                }
            });
            ui.menu_button("Help", |ui| {
                ui.set_min_width(200.0);
                if ui.add(egui::Button::new("Help").shortcut_text("F1")).clicked() {
                    action = Some(Action::Help);
                }
                if ui.button("Formula reference").clicked() {
                    self.help.open = true;
                    self.help.tab = crate::help::Tab::Formulas;
                }
                if ui.button("About").clicked() {
                    self.help.open = true;
                    self.help.tab = crate::help::Tab::About;
                }
            });
            ui.add_space(6.0);
            ui.add_enabled_ui(self.doc.can_undo(), |ui| {
                if widgets::icon_button(ui, icons::UNDO, "Undo (Ctrl+Z)").clicked() {
                    action = Some(Action::Undo);
                }
            });
            ui.add_enabled_ui(self.doc.can_redo(), |ui| {
                if widgets::icon_button(ui, icons::REDO, "Redo (Ctrl+Shift+Z)").clicked() {
                    action = Some(Action::Redo);
                }
            });
            ui.separator();
            let mut m = self.view.mode;
            if ui.selectable_label(m == NavMode::Orbit, format!("{}  Orbit", icons::ORBIT)).on_hover_text("Orbit around the point under the cursor (Tab switches)").clicked() {
                m = NavMode::Orbit;
            }
            if ui.selectable_label(m == NavMode::Fly, format!("{}  Fly", icons::FLY)).on_hover_text("Fly with WASD, look with the mouse (Tab switches)").clicked() {
                m = NavMode::Fly;
            }
            self.view.mode = m;

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if self.export.busy() { format!("{}  Exporting…", icons::EXPORT) } else { format!("{}  Export", icons::EXPORT) };
                if ui.add(egui::Button::new(RichText::new(label).strong()).fill(theme::ACCENT_DIM)).on_hover_text("Render and save an image (Ctrl+E)").clicked() {
                    action = Some(Action::Export);
                }
                ui.add_space(8.0);
                ui.label(RichText::new(self.doc.title()).color(theme::TEXT_MUTED));
            });
        });
        action
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            let expired = self.status.as_ref().is_some_and(|(_, t, err)| t.elapsed() > Duration::from_secs(if *err { 15 } else { 6 }));
            if expired {
                self.status = None;
            }
            if let Some((m, _, err)) = &self.status {
                let c = if *err { theme::WARN } else { theme::TEXT };
                ui.label(RichText::new(m).color(c).size(theme::fs(12.0)));
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let v = &self.view;
                let small = |t: String| RichText::new(t).size(theme::fs(11.5)).color(theme::TEXT_MUTED);
                ui.label(small(v.adapter_name()));
                ui.separator();
                ui.label(small(format!("{} × {}", v.render_size.0, v.render_size.1)));
                ui.separator();
                let conv = v.converged_after.map(|d| format!(" · done in {:.1} s", d.as_secs_f64())).unwrap_or_default();
                ui.label(small(format!("{} spp{conv}", v.samples()))).on_hover_text("Samples per pixel of the preview");
                if let Some((sx, sy)) = v.hover {
                    ui.separator();
                    ui.label(small(format!("{sx:+.3}, {sy:+.3}")));
                }
            });
        });
    }

    fn prefs_window(&mut self, ctx: &egui::Context) {
        if !self.prefs_open {
            return;
        }
        let mut open = true;
        let before = self.prefs.clone();
        egui::Window::new("Preferences").open(&mut open).resizable(false).collapsible(false).default_width(380.0).show(ctx, |ui| {
            let d = Prefs::default();
            let p = &mut self.prefs;
            widgets::section(ui, "Preview", false);
            widgets::grid(ui, "prefs-preview", |ui| {
                widgets::slider_u32(ui, "Resolution", "Preview resolution in percent of the view size. Lower is faster.", &mut p.preview_percent, 25..=100, d.preview_percent);
                widgets::slider_u32(ui, "While moving", "Resolution while navigating, in percent of the preview resolution", &mut p.navigate_percent, 10..=100, d.navigate_percent);
                widgets::slider_u32(ui, "Samples", "The preview stops refining after this many samples per pixel", &mut p.preview_samples, 1..=4096, d.preview_samples);
                widgets::checkbox(ui, "Image aspect", "Show the preview in the aspect ratio of the image size (else fill the view)", &mut p.keep_aspect);
            });
            widgets::section(ui, "Navigation", false);
            widgets::grid(ui, "prefs-nav", |ui| {
                widgets::slider(ui, "Mouse speed", "", &mut p.mouse_speed, 0.1..=4.0, d.mouse_speed);
                widgets::checkbox(ui, "Invert Y", "Invert the vertical mouse direction", &mut p.invert_y);
            });
            widgets::section(ui, "Interface", false);
            widgets::grid(ui, "prefs-ui", |ui| {
                widgets::slider_u32(ui, "Font size", "In percent", &mut p.font_percent, 50..=200, d.font_percent);
                widgets::slider_u32(ui, "Export samples", "Default samples per pixel of exported images", &mut p.export_samples, 1..=1024, d.export_samples);
            });
            widgets::section(ui, "Formulas", false);
            let dir = crate::prefs::user_formula_dir();
            ui.label(RichText::new(format!("Your formulas (*.mbf) are loaded from {} at start.", dir.display())).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            ui.horizontal(|ui| {
                if ui.button("Open folder").on_hover_text("Creates the folder if needed").clicked() {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = open_folder(&dir);
                }
                if ui.button("Reload formulas").clicked() {
                    rmb_core::formula::load_user_formulas(&dir);
                    if let Some(d) = &p.formula_dir {
                        rmb_core::formula::load_user_formulas(d);
                    }
                }
            });
        });
        if self.prefs != before {
            if self.prefs.font_percent != before.font_percent {
                theme::apply(ctx, self.prefs.font_percent);
            }
            self.export.samples = self.prefs.export_samples;
            self.save_prefs();
        }
        self.prefs_open = open;
    }

    fn close_dialog(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.doc.modified() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if !self.confirm_close {
            return;
        }
        egui::Modal::new(egui::Id::new("confirm-close")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(RichText::new("Unsaved changes").strong().size(theme::fs(15.0)));
            ui.label("The scene has changes that are not saved.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.confirm_close = false;
                    self.run(Action::Save, ctx);
                    if !self.doc.modified() {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                if ui.button("Discard").clicked() {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_close = false;
                }
            });
        });
    }
}

fn open_folder(p: &Path) -> std::io::Result<std::process::Child> {
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let cmd = "xdg-open";
    std::process::Command::new(cmd).arg(p).spawn()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.close_dialog(&ctx);

        // Files dropped onto the window
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(p) = dropped.first() {
            self.open_path(&p.clone());
        }
        // Clipboard paste (Ctrl+V) and copy (Ctrl+C) while no text field has the focus.
        let typing = ctx.egui_wants_keyboard_input();
        let (pasted, copied) = ctx.input(|i| {
            let mut p = None;
            let mut c = false;
            for e in &i.events {
                match e {
                    egui::Event::Paste(t) => p = Some(t.clone()),
                    egui::Event::Copy => c = true,
                    _ => {}
                }
            }
            (p, c)
        });
        if !typing {
            if let Some(t) = pasted {
                self.paste_text(&t);
            }
            if copied {
                self.run(Action::Copy, &ctx);
            }
        }
        if let Some(a) = self.shortcuts(&ctx) {
            self.run(a, &ctx);
        }

        egui::Panel::top("topbar").exact_size(46.0).frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 6))).show(ui, |ui| {
            if let Some(a) = self.top_bar(ui) {
                self.run(a, &ctx);
            }
        });
        egui::Panel::bottom("status").exact_size(26.0).frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(12, 3))).show(ui, |ui| {
            self.status_bar(ui);
        });
        egui::Panel::left("formulas").resizable(true).default_size(320.0).size_range(260.0..=480.0).frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 8))).show(ui, |ui| {
            self.formulas.show(ui, &mut self.doc.scene);
        });
        egui::Panel::right("inspector").resizable(true).default_size(340.0).size_range(300.0..=520.0).frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 10))).show(ui, |ui| {
            self.inspector.show(ui, &mut self.doc.scene, &mut self.view);
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CANVAS)).show(ui, |ui| {
            let paused = self.export.busy();
            self.view.paused = paused;
            self.view.show(ui, frame, &mut self.doc, &self.prefs);
        });

        self.help.show(&ctx);
        self.prefs_window(&ctx);
        if let Some(msg) = self.export.show(&ctx, &self.doc.scene, self.view.gpu_context(), self.prefs.last_dir.as_ref()) {
            if msg.starts_with("Export failed") {
                self.notify_err(msg);
            } else {
                self.notify(msg);
            }
        }

        // One undo step per finished edit: not while a button is held or the view moves.
        let busy = ctx.input(|i| i.pointer.any_down()) || self.view.navigating();
        if !busy {
            self.doc.commit();
        }
        let title = format!("{} — RMandelbulb3D", self.doc.title());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}
