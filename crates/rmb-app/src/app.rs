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
    QuickSave,
    Open,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    Export,
    ExportAnimation,
    ExportMesh,
    ExportVoxels,
    NetRender,
    Render,
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
    mesh: crate::mesh_dialog::MeshDialog,
    net: crate::net_window::NetWindow,
    render: crate::render_view::RenderView,
    timeline: crate::timeline::Timeline,
    explorer: crate::explorer::Explorer,
    /// Top bar: width of the left part with labels, and of the right part (compact mode).
    topbar_full: f32,
    topbar_right: f32,
    /// UI scale to apply once the slider is let go.
    pending_scale: Option<u32>,
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
        // CPU work leaves the chosen number of cores free (the pool is built once, at start).
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        let _ = rayon::ThreadPoolBuilder::new().num_threads(cores.saturating_sub(prefs.free_cores).max(1)).build_global();
        crate::widgets::set_closed_sections(&prefs.closed_sections);
        crate::prefs::set_dialog_dirs(&prefs);
        crate::icons::install(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, prefs.font_percent);
        cc.egui_ctx.set_zoom_factor(prefs.ui_scale_percent as f32 / 100.0);
        rmb_core::formula::load_user_formulas(&crate::prefs::user_formula_dir());
        if let Some(d) = &prefs.formula_dir {
            rmb_core::formula::load_user_formulas(d);
        }
        rmb_core::maps::set_dirs(prefs.maps_dir.iter().cloned().collect());
        let mut app = App {
            doc: Document::new(Scene::default()),
            view: Viewport::new(cc.wgpu_render_state.as_ref()),
            formulas: FormulasPanel::default(),
            explorer: {
                let mut e = crate::explorer::Explorer::default();
                e.batch_size = prefs.explore_batch;
                e
            },
            topbar_full: 0.0,
            topbar_right: 0.0,
            pending_scale: None,
            inspector: Inspector::default(),
            export: Export::new(prefs.export_samples),
            mesh: crate::mesh_dialog::MeshDialog::new(),
            net: crate::net_window::NetWindow::new(prefs.export_samples),
            render: crate::render_view::RenderView::new(prefs.export_samples),
            timeline: crate::timeline::Timeline::default(),
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
        crate::prefs::set_dialog_dirs(&self.prefs);
        if let Err(e) = self.prefs.save() {
            self.notify_err(format!("Preferences not saved: {e}"));
        }
    }

    /// A scene that comes into the editor: gets the denoiser when the preferences ask for it.
    fn incoming(&self, mut scene: Scene) -> Scene {
        if self.prefs.always_denoise && !scene.post.denoise {
            scene.post.denoise = true;
            scene.post.denoise_blend = self.prefs.denoise_blend;
        }
        scene
    }

    fn open_path(&mut self, p: &Path) {
        match rmb_core::io::load(p).map(|s| self.incoming(s)) {
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
        crate::prefs::dialog(crate::prefs::Folder::Scenes)
    }

    fn run(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::New => {
                let scene = Scene { width: self.prefs.new_width.max(16), height: self.prefs.new_height.max(16), ..Scene::default() };
                let scene = self.incoming(scene);
                self.doc.load(scene, None);
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
            Action::QuickSave => self.quick_save(),
            Action::Export => self.export.open_for(&self.doc.scene, self.doc.path.as_ref()),
            Action::Render => self.render.open_and_render(&self.doc.scene, self.view.gpu_context()),
            Action::ExportAnimation => self.export.open_animation(&self.doc.scene, self.doc.path.as_ref()),
            Action::ExportMesh => self.mesh.open_for(&self.doc.scene, self.doc.path.as_ref(), false),
            Action::ExportVoxels => self.mesh.open_for(&self.doc.scene, self.doc.path.as_ref(), true),
            Action::NetRender => self.net.open_for(&self.doc.scene, self.doc.path.as_ref()),
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
        match rmb_core::io::parse_text(text).map(|s| self.incoming(s)) {
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

    /// Saves the scene without a dialog: `qsave_<date>_<time>.rmb` in the quick save folder
    /// (else the scenes folder, else the config folder). The document keeps its own file.
    fn quick_save(&mut self) {
        let dir = self.prefs.quick_save_dir.clone().or_else(|| self.prefs.folders.scenes.clone()).unwrap_or_else(|| crate::prefs::config_dir().join("quicksave"));
        let name = format!("qsave_{}.rmb", timestamp());
        let path = dir.join(name);
        let result = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| rmb_core::io::save(&self.doc.scene, &path));
        match result {
            Ok(()) => self.notify(format!("Quick saved {}", path.display())),
            Err(e) => self.notify_err(format!("Quick save failed: {e}")),
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let c = Modifiers::COMMAND;
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        ctx.input_mut(|i| {
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, Key::S)) {
                return Some(Action::QuickSave);
            }
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
            if i.consume_key(Modifiers::NONE, Key::F12) {
                return Some(Action::Render);
            }
            None
        })
    }

    fn top_bar(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        // Compact: icons only when the labels did not fit (measured in the last full frame).
        let total = ui.available_width();
        let compact = self.topbar_full > 0.0 && self.topbar_full + self.topbar_right + 12.0 > total;
        let lbl = |icon: &str, text: &str| if compact { icon.to_string() } else { format!("{icon}  {text}") };
        let tip = |t: &str, label: &str| if compact { format!("{label}: {t}") } else { t.to_string() };
        let start_x = ui.cursor().min.x;
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
                if item(ui, "Quick save", "Ctrl+Alt+S").on_hover_text("Saves the scene without asking, with the date and time as its name (folder in the Preferences)").clicked() {
                    action = Some(Action::QuickSave);
                }
                ui.separator();
                if item(ui, "Export image…", "Ctrl+E").clicked() {
                    action = Some(Action::Export);
                }
                let has_anim = self.doc.scene.animation.as_ref().is_some_and(|a| !a.keyframes.is_empty());
                if ui.add_enabled(has_anim, egui::Button::new("Export animation…")).on_disabled_hover_text("Add keyframes in the timeline first").clicked() {
                    action = Some(Action::ExportAnimation);
                    ui.close();
                }
                if ui.button("Export mesh…").on_hover_text("Save the surface as a 3D mesh (PLY, OBJ, STL) for 3D printing and 3D programs").clicked() {
                    action = Some(Action::ExportMesh);
                    ui.close();
                }
                if ui.button("Export voxels…").on_hover_text("Save the scene as voxels: PNG images per layer, or a MagicaVoxel model").clicked() {
                    action = Some(Action::ExportVoxels);
                    ui.close();
                }
                ui.separator();
                if ui.button("Network render…").on_hover_text("Render the animation or an image on other computers of the network (rmb-cli --network)").clicked() {
                    action = Some(Action::NetRender);
                    ui.close();
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
            if ui.selectable_label(m == NavMode::Orbit, lbl(icons::ORBIT, "Orbit")).on_hover_text(tip("Orbit around the point under the cursor (Tab switches)", "Orbit")).clicked() {
                m = NavMode::Orbit;
            }
            if ui.selectable_label(m == NavMode::Fly, lbl(icons::FLY, "Fly")).on_hover_text(tip("Fly with WASD, look with the mouse (Tab switches)", "Fly")).clicked() {
                m = NavMode::Fly;
            }
            self.view.mode = m;
            ui.separator();
            let dn_tip = match rmb_core::render::denoise::available() {
                Ok(_) => "Show the preview through the AI denoiser (Intel Open Image Denoise). Updates at 1, 4, 16, 64, … samples. Saving denoises only with Effects › Image › Denoise.".to_string(),
                Err(e) => format!("The denoiser library is not available: {e}. Run python3 tools/fetch_oidn.py or set RMB_OIDN_DIR."),
            };
            let enabled = self.view.gpu_context().is_some();
            if ui.add_enabled(enabled, egui::Button::selectable(self.view.show_denoised, lbl(egui_phosphor::regular::SPARKLE, "Denoised"))).on_hover_text(tip(&dn_tip, "Denoised")).on_disabled_hover_text("Needs the GPU renderer").clicked() {
                self.view.show_denoised = !self.view.show_denoised;
            }
            ui.separator();
            if ui
                .selectable_label(self.view.show_guides, lbl(egui_phosphor::regular::GRID_FOUR, "Grid"))
                .on_hover_text(tip("Ground grid (z = 0) with the x (red), y (green) and z (blue) axes, and the axis cross in the corner (click an axis to look along it)", "Grid"))
                .clicked()
            {
                self.view.show_guides = !self.view.show_guides;
            }
            if ui
                .selectable_label(self.view.show_lights, lbl(icons::LIGHT, "Lights"))
                .on_hover_text(tip("Show the lights in the view with handles to move and aim them (click one to edit it)", "Lights"))
                .clicked()
            {
                self.view.show_lights = !self.view.show_lights;
            }
            ui.separator();
            for mode in crate::gizmos::GizmoMode::ALL {
                if ui
                    .selectable_label(self.view.gizmo_mode == mode, lbl(mode.icon(), mode.label()))
                    .on_hover_text(tip(&format!("{} (select an object in the scene list or Ctrl+click it in the view)", mode.tip()), mode.label()))
                    .clicked()
                {
                    self.view.gizmo_mode = mode;
                }
            }
            ui.separator();
            if ui
                .selectable_label(self.timeline.open, lbl(egui_phosphor::regular::FILM_STRIP, "Animation"))
                .on_hover_text(tip("Timeline: keyframes, flight recording, playback and movie export", "Animation"))
                .clicked()
            {
                self.timeline.open = !self.timeline.open;
            }
            if ui
                .selectable_label(self.explorer.open, lbl(egui_phosphor::regular::COMPASS, "Explore"))
                .on_hover_text(tip("Explore lane: presets to start from, random fractals and variants of the current scene", "Explore"))
                .clicked()
            {
                self.explorer.open = !self.explorer.open;
            }

            if !compact {
                self.topbar_full = ui.cursor().min.x - start_x;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let right_start = ui.cursor().max.x;
                let label = if self.export.busy() { format!("{}  Exporting…", icons::EXPORT) } else { format!("{}  Export", icons::EXPORT) };
                if ui.add(egui::Button::new(RichText::new(label).strong()).fill(theme::ACCENT_DIM)).on_hover_text("Render and save an image (Ctrl+E)").clicked() {
                    action = Some(Action::Export);
                }
                let label = if self.render.busy() { format!("{}  Rendering…", egui_phosphor::regular::IMAGE) } else { format!("{}  Render", egui_phosphor::regular::IMAGE) };
                if ui
                    .button(label)
                    .on_hover_text("Test render in a window with everything the export applies: denoiser, lens flares, bloom, streaks (F12). Post processing changes show there at once.")
                    .clicked()
                {
                    action = Some(Action::Render);
                }
                self.topbar_right = right_start - ui.cursor().max.x;
                // The document name only where it fits.
                let title = RichText::new(self.doc.title()).color(theme::TEXT_MUTED);
                let width = ui.painter().layout_no_wrap(self.doc.title(), egui::FontId::proportional(theme::fs(14.0)), theme::TEXT_MUTED).size().x;
                if ui.available_width() > width + 24.0 {
                    ui.add_space(8.0);
                    ui.label(title);
                }
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
                if v.show_denoised
                    && let Some(d) = &v.denoise_status
                {
                    ui.separator();
                    ui.label(small(d.clone())).on_hover_text("What the denoiser did for the preview");
                }
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
        let mut m3f_result = None;
        egui::Window::new("Preferences").open(&mut open).resizable(false).collapsible(false).default_width(400.0).show(ctx, |ui| {
            let d = Prefs::default();
            let p = &mut self.prefs;
            let h = ctx.content_rect().height() - 140.0;
            egui::ScrollArea::vertical().max_height(h).min_scrolled_height(h.min(700.0)).show(ui, |ui| {
            if widgets::section(ui, "Appearance", false).open {
                widgets::grid(ui, "prefs-look", |ui| {
                    widgets::slider_u32(
                        ui,
                        "UI scale",
                        "Size of the whole interface in percent: texts, controls and spacing together. Lower it on small or high-resolution screens where everything looks too big",
                        &mut p.ui_scale_percent,
                        50..=200,
                        d.ui_scale_percent,
                    );
                    widgets::slider_u32(ui, "Font size", "Size of the texts only, in percent of the built-in size (UI scale changes everything together)", &mut p.font_percent, 50..=200, d.font_percent);
                });
                if p.font_percent != 100 {
                    ui.label(RichText::new("Font sizes other than 100 % can make texts overlap or not fit their controls; UI scale keeps the proportions.").size(theme::fs(11.0)).color(theme::WARN));
                }
            }
            if widgets::section(ui, "Preview", false).open {
                widgets::grid(ui, "prefs-preview", |ui| {
                    widgets::slider_u32(ui, "Resolution", "Preview resolution in percent of the view size. Lower is faster.", &mut p.preview_percent, 25..=100, d.preview_percent);
                    widgets::slider_u32(ui, "While moving", "Resolution while navigating, in percent of the preview resolution", &mut p.navigate_percent, 10..=100, d.navigate_percent);
                    widgets::slider_u32(ui, "Samples", "The preview stops refining after this many samples per pixel", &mut p.preview_samples, 1..=4096, d.preview_samples);
                    widgets::slider(ui, "Time limit", "The preview stops refining after this many seconds (0: no limit; the sample limit above still applies)", &mut p.preview_seconds, 0.0..=600.0, d.preview_seconds);
                    widgets::checkbox(ui, "Image aspect", "Show the preview in the aspect ratio of the image size (else fill the view)", &mut p.keep_aspect);
                    widgets::checkbox(
                        ui,
                        "Optimize at rest",
                        "When the view and the settings rest for a moment, compile a shader with all settings as constants in the background and continue with it: the same image, often 5-30 % faster passes. Costs a compile (about a second) after each change",
                        &mut p.optimize_resting,
                    );
                });
            }
            if widgets::section(ui, "Navigation", false).open {
                widgets::grid(ui, "prefs-nav", |ui| {
                    widgets::slider(ui, "Mouse speed", "How fast dragging turns the view and the camera", &mut p.mouse_speed, 0.1..=4.0, d.mouse_speed);
                    widgets::checkbox(ui, "Invert Y", "Invert the vertical mouse direction", &mut p.invert_y);
                });
            }
            if widgets::section(ui, "Rendering", false).open {
                widgets::grid(ui, "prefs-render", |ui| {
                    widgets::slider_u32(ui, "Export samples", "Default samples per pixel of exported images and the render window", &mut p.export_samples, 1..=1024, d.export_samples);
                    widgets::checkbox(
                        ui,
                        "Always denoise",
                        "Scenes that come in without the denoiser (new, opened, pasted, from the explore lane) get it, so saved images are denoised. Scenes that already use it keep their setting",
                        &mut p.always_denoise,
                    );
                    if p.always_denoise {
                        widgets::slider(ui, "Denoise amount", "1: only the denoised image; less mixes the original back in", &mut p.denoise_blend, 0.0..=1.0, d.denoise_blend);
                    }
                    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
                    let mut free = p.free_cores as u32;
                    widgets::slider_u32(
                        ui,
                        "Free CPU cores",
                        &format!("CPU cores the CPU work (CPU renderer, 3D model baking, framing random scenes) leaves free for other programs ({} of {cores} used). Takes effect at the next start", cores.saturating_sub(p.free_cores).max(1)),
                        &mut free,
                        0..=(cores as u32 - 1),
                        d.free_cores as u32,
                    );
                    p.free_cores = free as usize;
                });
            }
            if widgets::section(ui, "New scenes", false).open {
                widgets::grid(ui, "prefs-new", |ui| {
                    let mut w = p.new_width;
                    let mut hh = p.new_height;
                    widgets::slider_u32(ui, "Width", "Image width of new scenes (File › New), in pixels", &mut w, 16..=7680, d.new_width);
                    widgets::slider_u32(ui, "Height", "Image height of new scenes (File › New), in pixels", &mut hh, 16..=4320, d.new_height);
                    p.new_width = w;
                    p.new_height = hh;
                });
            }
            if widgets::section(ui, "Folders", false).open {
                ui.label(RichText::new("Where the file dialogs start for each kind of file (else in the folder used last).").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
                widgets::grid(ui, "prefs-folders", |ui| {
                    for kind in crate::prefs::Folder::ALL {
                        folder_row(ui, kind.label(), "Start folder of the file dialogs for this kind of file", p.folders.get_mut(kind));
                    }
                    folder_row(ui, "Quick save", "Where File › Quick save (Ctrl+Alt+S) puts scenes (else the Scenes folder, else the config folder)", &mut p.quick_save_dir);
                });
            }
            if widgets::section(ui, "Explore", false).open {
                widgets::grid(ui, "prefs-explore", |ui| {
                    let mut n = p.explore_batch as u32;
                    widgets::slider_u32(ui, "Batch size", "Scenes per new batch of random fractals or mutations in the explore lane", &mut n, 1..=48, d.explore_batch as u32);
                    p.explore_batch = n as usize;
                });
            }
            if widgets::section(ui, "Formulas", false).open {
                let dir = crate::prefs::user_formula_dir();
                ui.label(RichText::new(format!("Your formulas (*.mbf) are loaded from {} at start.", dir.display())).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
                ui.horizontal(|ui| {
                    if ui.button("Open folder").on_hover_text("Creates the folder if needed").clicked() {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = open_folder(&dir);
                    }
                    if ui.button("Reload formulas").on_hover_text("Load the formula files again (after editing them outside the app)").clicked() {
                        rmb_core::formula::load_user_formulas(&dir);
                        if let Some(d) = &p.formula_dir {
                            rmb_core::formula::load_user_formulas(d);
                        }
                    }
                    if ui.button("Import .m3f…").on_hover_text("Translate the Mandelbulb3D formula files (*.m3f, also JIT formulas) of a folder and add those that translate to the library for this session; save scenes to keep them").clicked()
                        && let Some(folder) = rfd::FileDialog::new().set_title("Folder with Mandelbulb3D formulas (*.m3f)").pick_folder()
                    {
                        let (ok, failed) = rmb_core::formula::load_m3f_formulas(&folder);
                        m3f_result = Some(format!("Mandelbulb3D formulas from {}: {ok} added, {failed} could not be translated", folder.display()));
                    }
                });
            }
            if widgets::section(ui, "Maps", false).open {
                let maps_label = match &p.maps_dir {
                    Some(d) => format!("Mandelbulb3D maps (height maps and other images formulas read) from {}", d.display()),
                    None => "Mandelbulb3D maps: set the M3Maps folder of Mandelbulb3D for formulas that read maps (also RMB_MAPS_DIR or a maps folder next to the program).".to_string(),
                };
                ui.label(RichText::new(maps_label).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
                ui.horizontal(|ui| {
                    if ui.button("Choose folder…").on_hover_text("Mandelbulb3D's M3Maps folder (files named by their number, e.g. 10.jpg)").clicked()
                        && let Some(d) = rfd::FileDialog::new().pick_folder()
                    {
                        p.maps_dir = Some(d);
                        rmb_core::maps::set_dirs(p.maps_dir.iter().cloned().collect());
                    }
                    if p.maps_dir.is_some() && ui.button("Clear").on_hover_text("Use only the default folders").clicked() {
                        p.maps_dir = None;
                        rmb_core::maps::set_dirs(Vec::new());
                    }
                });
            }
            if widgets::section(ui, "Movies", false).open {
                let program = rmb_core::video::ffmpeg_program(p.ffmpeg.as_deref());
                let found = rmb_core::video::ffmpeg_available(&program);
                let ff_label = match (&p.ffmpeg, found) {
                    (Some(f), true) => format!("ffmpeg: {}", f.display()),
                    (Some(f), false) => format!("ffmpeg not working: {}", f.display()),
                    (None, true) => "ffmpeg found on the search path: animations can be saved as movies.".to_string(),
                    (None, false) => "ffmpeg not found: animations are saved as PNG images. Install ffmpeg or choose the program.".to_string(),
                };
                ui.label(RichText::new(ff_label).size(theme::fs(11.5)).color(if found { theme::TEXT_MUTED } else { theme::WARN }));
                ui.horizontal(|ui| {
                    if ui.button("Choose ffmpeg…").on_hover_text("The ffmpeg program that writes movie files (MP4, MOV, WebM, GIF)").clicked()
                        && let Some(f) = rfd::FileDialog::new().pick_file()
                    {
                        p.ffmpeg = Some(f);
                    }
                    if p.ffmpeg.is_some() && ui.button("Clear").on_hover_text("Use ffmpeg from the search path").clicked() {
                        p.ffmpeg = None;
                    }
                });
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Changes are saved right away.").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            });
        });
        if let Some(msg) = m3f_result {
            self.notify(msg);
        }
        if self.prefs != before {
            if self.prefs.font_percent != before.font_percent {
                theme::apply(ctx, self.prefs.font_percent);
            }
            // The scale applies when the slider is let go (the window would jump under the mouse).
            if self.prefs.ui_scale_percent != before.ui_scale_percent {
                self.pending_scale = Some(self.prefs.ui_scale_percent);
            }
            self.explorer.batch_size = self.prefs.explore_batch;
            self.export.samples = self.prefs.export_samples;
            self.save_prefs();
        }
        if let Some(scale) = self.pending_scale
            && !ctx.input(|i| i.pointer.any_down())
        {
            ctx.set_zoom_factor(scale as f32 / 100.0);
            self.pending_scale = None;
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
        // Sections opened or closed since the last frame: keep them in the preferences.
        let closed = crate::widgets::closed_sections();
        if closed != self.prefs.closed_sections {
            self.prefs.closed_sections = closed;
            self.save_prefs();
        }

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
            self.formulas.show(ui, &mut self.doc.scene, &mut self.view.selected_object);
        });
        egui::Panel::right("inspector").resizable(true).default_size(340.0).size_range(300.0..=520.0).frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 10))).show(ui, |ui| {
            self.inspector.show(ui, &mut self.doc.scene, &mut self.view);
        });
        if self.explorer.open {
            let mut picked = None;
            let gpu = self.view.gpu_context();
            egui::Panel::bottom("explore").resizable(false).exact_size(150.0).frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 8))).show(ui, |ui| {
                picked = self.explorer.show(ui, &self.doc.scene, gpu);
            });
            if let Some(scene) = picked {
                let scene = self.incoming(scene);
                self.doc.replace(scene);
                self.formulas.selected = 0;
                self.notify("Took the scene from the explore lane (Ctrl+Z goes back)");
            }
        }
        if self.timeline.open {
            let mut act = None;
            egui::Panel::bottom("timeline").resizable(true).default_size(230.0).size_range(150.0..=420.0).frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 8))).show(ui, |ui| {
                act = self.timeline.show(ui, &mut self.doc, &mut self.view);
            });
            match act {
                Some(crate::timeline::TimelineAction::ExportAnimation) => self.export.open_animation(&self.doc.scene, self.doc.path.as_ref()),
                Some(crate::timeline::TimelineAction::Message(m)) => self.notify(m),
                None => {}
            }
        } else {
            self.view.preview = None;
            self.view.overlay = None;
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CANVAS)).show(ui, |ui| {
            let paused = self.export.busy() || self.render.busy() || self.mesh.busy() || self.net.local_busy();
            self.view.paused = paused;
            self.view.show(ui, frame, &mut self.doc, &self.prefs);
        });

        self.help.show(&ctx);
        self.prefs_window(&ctx);
        if let Some(msg) = self.render.show(&ctx, &self.doc.scene, self.view.gpu_context()) {
            if msg.starts_with("Saving failed") {
                self.notify_err(msg);
            } else {
                self.notify(msg);
            }
        }
        if let Some(msg) = self.net.show(&ctx, &self.doc.scene, self.prefs.ffmpeg.as_deref()) {
            if msg.contains("failed") {
                self.notify_err(msg);
            } else {
                self.notify(msg);
            }
        }
        if let Some(msg) = self.mesh.show(&ctx, &self.doc.scene, self.view.gpu_context()) {
            if msg.starts_with("Mesh export failed") {
                self.notify_err(msg);
            } else {
                self.notify(msg);
            }
        }
        if let Some(msg) = self.export.show(&ctx, &self.doc.scene, self.view.gpu_context(), self.prefs.ffmpeg.as_deref()) {
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

/// A preferences row: a folder with buttons to choose and clear it.
fn folder_row(ui: &mut Ui, label: &str, tip: &str, dir: &mut Option<PathBuf>) {
    widgets::label(ui, label, tip);
    ui.horizontal(|ui| {
        let shown = dir.as_ref().map_or("—".to_string(), |d| d.display().to_string());
        if ui.button("Choose…").on_hover_text(tip).clicked()
            && let Some(d) = rfd::FileDialog::new().pick_folder()
        {
            *dir = Some(d);
        }
        if dir.is_some() && ui.small_button(icons::X).on_hover_text("Clear: back to the folder used last").clicked() {
            *dir = None;
        }
        ui.label(RichText::new(shown).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    });
    ui.end_row();
}

/// Date and time (UTC) as `YYYYMMDD_HHMMSS`.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}_{:02}{:02}{:02}", rest / 3600, rest % 3600 / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamps_have_date_and_time() {
        let t = super::timestamp();
        assert_eq!(t.len(), 15, "{t}");
        assert!(t.starts_with("20"), "{t}");
        assert_eq!(&t[8..9], "_");
    }
}
