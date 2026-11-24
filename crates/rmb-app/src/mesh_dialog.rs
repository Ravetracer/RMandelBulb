// SPDX-License-Identifier: LGPL-2.1-or-later

//! Mesh export window: samples the scene's distance estimate in a box (GPU when available) and
//! saves a closed triangle mesh (PLY, OBJ, STL) in the background.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use egui::RichText;
use rmb_core::Scene;
use rmb_core::math::V3;
use rmb_core::mesh_export::{Grid, MeshFormat, MeshSettings};
use rmb_gpu::GpuContext;

use crate::{theme, widgets};

struct Job {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<String, String>>,
    started: Instant,
}

/// The parts of a scene that decide its surfaces.
fn shapes(scene: &Scene) -> String {
    serde_json::to_string(&(&scene.fractal, &scene.objects, &scene.cut)).unwrap_or_default()
}

pub struct MeshDialog {
    pub open: bool,
    settings: MeshSettings,
    path: Option<PathBuf>,
    job: Option<Job>,
    /// Box fitting running in the background.
    fitting: Option<mpsc::Receiver<Option<(V3, V3)>>>,
    /// The shapes the box was fitted to (fitted again when they change).
    fitted: Option<String>,
    message: Option<String>,
}

impl MeshDialog {
    pub fn new() -> Self {
        MeshDialog { open: false, settings: MeshSettings::default(), path: None, job: None, fitting: None, fitted: None, message: None }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn open_for(&mut self, scene: &Scene, doc_path: Option<&PathBuf>) {
        self.open = true;
        if self.path.is_none() {
            self.path = doc_path.map(|p| p.with_extension(self.settings.format.extension()));
        }
        if self.fitted.as_deref() != Some(shapes(scene).as_str()) {
            self.fit(scene);
        }
    }

    /// Fits the box to the scene's surfaces (background thread).
    fn fit(&mut self, scene: &Scene) {
        let (tx, rx) = mpsc::channel();
        let sc = scene.clone();
        std::thread::spawn(move || {
            let _ = tx.send(rmb_core::mesh_export::fit_bounds(&sc));
        });
        self.fitting = Some(rx);
        self.fitted = Some(shapes(scene));
    }

    /// Shows the window; returns a status message when an export finished.
    pub fn show(&mut self, ctx: &egui::Context, scene: &Scene, gpu: Option<GpuContext>) -> Option<String> {
        let mut finished = None;
        if let Some(job) = &self.job {
            match job.rx.try_recv() {
                Ok(r) => {
                    let msg = match r {
                        Ok(m) => format!("{m} ({:.1} s)", job.started.elapsed().as_secs_f64()),
                        Err(e) => format!("Mesh export failed: {e}"),
                    };
                    self.message = Some(msg.clone());
                    finished = Some(msg);
                    self.job = None;
                }
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        if let Some(rx) = &self.fitting {
            match rx.try_recv() {
                Ok(Some((lo, hi))) => {
                    (self.settings.min, self.settings.max) = (lo, hi);
                    self.fitting = None;
                }
                Ok(None) => {
                    self.message = Some("No surface found within ±64: set the box by hand".into());
                    self.fitting = None;
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(mpsc::TryRecvError::Disconnected) => self.fitting = None,
            }
        }
        if !self.open {
            return finished;
        }
        let mut open = self.open;
        let idle = self.job.is_none();
        egui::Window::new("Export mesh").id(egui::Id::new("mesh-export")).open(&mut open).resizable(false).collapsible(false).default_width(400.0).show(ctx, |ui| {
            ui.label(RichText::new("Saves the surface as a closed triangle mesh for 3D printing and 3D programs.").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            ui.add_space(4.0);
            let d = MeshSettings::default();
            ui.add_enabled_ui(idle, |ui| {
                widgets::grid(ui, "mesh", |ui| {
                    let s = &mut self.settings;
                    for (name, lo, hi) in [("X", &mut s.min.x, &mut s.max.x), ("Y", &mut s.min.y, &mut s.max.y), ("Z", &mut s.min.z, &mut s.max.z)] {
                        widgets::label(ui, &format!("Box {name}"), "The sampled box: from, to (world units). Where the surface reaches the box, the mesh is closed by a flat cap");
                        ui.horizontal(|ui| {
                            let speed = ((*hi - *lo).abs() * 0.002).max(1e-9);
                            ui.add(egui::DragValue::new(lo).speed(speed).max_decimals(6));
                            ui.add(egui::DragValue::new(hi).speed(speed).max_decimals(6));
                        });
                        ui.end_row();
                    }
                    widgets::slider_u32(ui, "Resolution", "Grid cells along the longest side of the box: finer detail, more triangles (they grow with the square), longer export", &mut s.resolution, 32..=2048, d.resolution);
                    widgets::slider(ui, "Thickness", "How far outside the fractal the surface lies, in grid cells: 0.5 keeps the shape; more thickens thin parts so they survive 3D printing", &mut s.thickness, 0.0..=4.0, d.thickness);
                    widgets::checkbox(ui, "Refine", "Moves every vertex onto the surface after the grid placed it: sharper edges, a little slower", &mut s.refine);
                    widgets::checkbox(ui, "Vertex colors", "The scene's colors at every vertex (PLY and OBJ; STL has none)", &mut s.colors);
                    widgets::checkbox(ui, "Y up", "Write y as the up axis (many 3D programs; RMandelbulb3D uses z up)", &mut s.y_up);
                    widgets::label(ui, "Format", "PLY: binary, small, with colors (Blender, MeshLab). OBJ: text, colors as an extension many programs read. STL: binary, for slicers of 3D printers, no colors");
                    let before = s.format;
                    egui::ComboBox::from_id_salt("mesh-format").selected_text(s.format.label()).show_ui(ui, |ui| {
                        for f in MeshFormat::ALL {
                            ui.selectable_value(&mut s.format, f, f.label());
                        }
                    });
                    if s.format != before
                        && let Some(p) = &mut self.path
                    {
                        *p = p.with_extension(s.format.extension());
                    }
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    let fitting = self.fitting.is_some();
                    if ui.add_enabled(!fitting, egui::Button::new(if fitting { "Fitting…" } else { "Fit box to surface" })).on_hover_text("Finds the box around the scene's surfaces (a coarse search)").clicked() {
                        self.fit(scene);
                    }
                    let label = self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "No file chosen".into());
                    if ui.button("File…").on_hover_text("Where to save the mesh").clicked() {
                        let f = self.settings.format;
                        let mut dlg = crate::prefs::dialog(crate::prefs::Folder::Models).add_filter(f.label(), &[f.extension()]).set_file_name(format!("fractal.{}", f.extension()));
                        if let Some(dir) = self.path.as_ref().and_then(|p| p.parent()) {
                            dlg = dlg.set_directory(dir);
                        }
                        if let Some(p) = dlg.save_file() {
                            self.path = Some(match MeshFormat::of_path(&p) {
                                Some(f) => {
                                    self.settings.format = f;
                                    p
                                }
                                None => p.with_extension(f.extension()),
                            });
                        }
                    }
                    ui.label(RichText::new(label).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
                });
            });
            let g = Grid::new(&self.settings);
            let samples = g.samples() as f64;
            let device = if gpu.is_some() { "GPU" } else { "CPU" };
            ui.label(RichText::new(format!("Grid {} × {} × {} ({:.1} million samples, on the {device})", g.n[0], g.n[1], g.n[2], samples / 1e6)).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            ui.add_space(6.0);
            if let Some(job) = &self.job {
                let p = f32::from_bits(job.progress.load(Ordering::Relaxed));
                ui.add(egui::ProgressBar::new(p).show_percentage().animate(true));
                let el = job.started.elapsed().as_secs_f64();
                let eta = if p > 0.02 { format!(" · about {:.0} s left", el / p as f64 - el) } else { String::new() };
                ui.label(RichText::new(format!("{el:.0} s{eta}")).color(theme::TEXT_MUTED));
                if ui.button("Cancel").clicked() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            } else {
                let ready = self.path.is_some() && self.fitting.is_none();
                ui.horizontal(|ui| {
                    if ui.add_enabled(ready, egui::Button::new(RichText::new(format!("{}  Export mesh", crate::icons::EXPORT)).strong()).fill(theme::ACCENT_DIM)).clicked() {
                        self.start(scene, gpu.clone());
                    }
                    if self.path.is_none() {
                        ui.label(RichText::new("Choose where to save first").color(theme::TEXT_MUTED));
                    }
                });
                if let Some(m) = &self.message {
                    ui.label(RichText::new(m).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
                }
            }
        });
        self.open = open;
        finished
    }

    fn start(&mut self, scene: &Scene, gpu: Option<GpuContext>) {
        let Some(path) = self.path.clone() else { return };
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (sc, settings) = (scene.clone(), self.settings.clone());
        let (pr, ca) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let res = (|| {
                for e in rmb_core::compose::load_models(&sc) {
                    eprintln!("3D model not loaded: {e}");
                }
                // The GPU computes the distances when there is one; the CPU otherwise.
                let mut gpu_field = match gpu {
                    Some(ctx) => Some(rmb_gpu::GridField::new(ctx, &sc, Grid::new(&settings))?),
                    None => None,
                };
                let field = gpu_field.as_mut().map(|f| f as &mut dyn rmb_core::mesh_export::Field);
                let (v, t) = rmb_core::mesh_export::export(&sc, &settings, field, &path, &mut |p| {
                    pr.store(p.to_bits(), Ordering::Relaxed);
                    !ca.load(Ordering::Relaxed)
                })?;
                Ok(format!("Saved {} ({v} vertices, {t} triangles)", path.display()))
            })();
            let _ = tx.send(res);
        });
        self.message = None;
        self.job = Some(Job { progress, cancel, rx, started: Instant::now() });
    }
}
