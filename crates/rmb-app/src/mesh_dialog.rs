// SPDX-License-Identifier: LGPL-2.1-or-later

//! Mesh and voxel export window: samples the scene's distance estimate in a box (GPU when
//! available) and saves a closed triangle mesh (PLY, OBJ, STL) or voxels (PNG slices,
//! MagicaVoxel) in the background.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use egui::RichText;
use rmb_core::Scene;
use rmb_core::math::V3;
use rmb_core::mesh_export::{Grid, MeshFormat, MeshSettings};
use rmb_core::voxel_export::{VoxelFormat, VoxelSettings};
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
    /// Voxel mode, its settings and output (folder for PNG slices, file for MagicaVoxel).
    voxels: bool,
    vox: VoxelSettings,
    vox_path: Option<PathBuf>,
    job: Option<Job>,
    /// Box fitting running in the background.
    fitting: Option<mpsc::Receiver<Option<(V3, V3)>>>,
    /// The shapes the box was fitted to (fitted again when they change).
    fitted: Option<String>,
    message: Option<String>,
}

impl MeshDialog {
    pub fn new() -> Self {
        MeshDialog { open: false, settings: MeshSettings::default(), path: None, voxels: false, vox: VoxelSettings::default(), vox_path: None, job: None, fitting: None, fitted: None, message: None }
    }

    /// The box to show in the view while the window is open.
    pub fn shown_box(&self) -> Option<(V3, V3)> {
        self.open.then_some((self.settings.min, self.settings.max))
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Opens the window for a mesh (`voxels` false) or voxel export.
    pub fn open_for(&mut self, scene: &Scene, doc_path: Option<&PathBuf>, voxels: bool) {
        self.open = true;
        self.voxels = voxels;
        if self.path.is_none() {
            self.path = doc_path.map(|p| p.with_extension(self.settings.format.extension()));
        }
        if self.vox_path.is_none() {
            self.vox_path = doc_path.map(|p| self.vox_target(p));
        }
        if self.fitted.as_deref() != Some(shapes(scene).as_str()) {
            self.fit(scene);
        }
    }

    /// The voxel output next to `p`: a folder for PNG slices, a .vox file.
    fn vox_target(&self, p: &std::path::Path) -> PathBuf {
        match self.vox.format {
            VoxelFormat::PngStack => p.with_extension(""),
            VoxelFormat::MagicaVoxel => p.with_extension("vox"),
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
        let title = if self.voxels { "Export voxels" } else { "Export mesh" };
        egui::Window::new(title).id(egui::Id::new("mesh-export")).open(&mut open).resizable(false).collapsible(false).default_width(400.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(idle, |ui| {
                    ui.selectable_value(&mut self.voxels, false, "Mesh").on_hover_text("A closed triangle mesh for 3D printing and 3D programs");
                    ui.selectable_value(&mut self.voxels, true, "Voxels").on_hover_text("Voxels: PNG images per layer, or a MagicaVoxel model");
                });
            });
            let text = if self.voxels {
                "Saves the grid as voxels: one PNG image per z layer (bottom first), or a MagicaVoxel model."
            } else {
                "Saves the surface as a closed triangle mesh for 3D printing and 3D programs."
            };
            ui.label(RichText::new(text).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
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
                    if self.voxels {
                        let v = &mut self.vox;
                        widgets::checkbox(ui, "Colors", "The scene's colors (gradient, color maps, texture layers, model colors; no lighting) on the voxels next to empty space; the hidden inside gets each layer's mean color. Off: white on black", &mut v.colors);
                        widgets::checkbox(ui, "Hollow", "Only the voxels next to empty space (the visible shell): far fewer voxels, faster in MagicaVoxel", &mut v.hollow);
                        widgets::label(ui, "Format", "PNG slices: a folder with one image per z layer, bottom first, seen from above (3D printing, image tools, volume programs). MagicaVoxel: one .vox model, at most 256 voxels per side, colors in a palette of 255");
                        let before = v.format;
                        egui::ComboBox::from_id_salt("vox-format").selected_text(v.format.label()).show_ui(ui, |ui| {
                            for f in VoxelFormat::ALL {
                                ui.selectable_value(&mut v.format, f, f.label());
                            }
                        });
                        ui.end_row();
                        if v.format != before
                            && let Some(p) = self.vox_path.clone()
                        {
                            self.vox_path = Some(self.vox_target(&p));
                        }
                        return;
                    }
                    let s = &mut self.settings;
                    widgets::slider(ui, "Remove pieces under", "Removes loose pieces with fewer triangles than this share of the mesh, in percent (the dust around fractal surfaces, which 3D printers cannot print; 0 keeps everything)", &mut s.min_piece, 0.0..=5.0, d.min_piece);
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
                    let shown = if self.voxels { &self.vox_path } else { &self.path };
                    let label = shown.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "No file chosen".into());
                    if self.voxels {
                        let folder = self.vox.format == VoxelFormat::PngStack;
                        if ui.button(if folder { "Folder…" } else { "File…" }).on_hover_text(if folder { "The folder for the PNG images (created if needed)" } else { "Where to save the .vox file" }).clicked() {
                            let mut dlg = crate::prefs::dialog(crate::prefs::Folder::Models);
                            if let Some(dir) = self.vox_path.as_ref().and_then(|p| p.parent()) {
                                dlg = dlg.set_directory(dir);
                            }
                            let picked = if folder { dlg.pick_folder() } else { dlg.add_filter("MagicaVoxel", &["vox"]).set_file_name("fractal.vox").save_file() };
                            if let Some(p) = picked {
                                self.vox_path = Some(if folder { p } else { p.with_extension("vox") });
                            }
                        }
                    } else if ui.button("File…").on_hover_text("Where to save the mesh").clicked() {
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
            let used = if self.voxels { rmb_core::voxel_export::limited(&self.settings, &self.vox) } else { self.settings.clone() };
            let g = Grid::new(&used);
            let samples = g.samples() as f64;
            let device = if gpu.is_some() { "GPU" } else { "CPU" };
            let what = if self.voxels { "voxels" } else { "samples" };
            ui.label(RichText::new(format!("Grid {} × {} × {} ({:.1} million {what}, on the {device})", g.n[0], g.n[1], g.n[2], samples / 1e6)).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            if used.resolution < self.settings.resolution {
                ui.label(RichText::new("MagicaVoxel models hold at most 256 voxels per side: the resolution is limited.").size(theme::fs(11.5)).color(theme::WARN));
            }
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
                let has_target = if self.voxels { self.vox_path.is_some() } else { self.path.is_some() };
                let ready = has_target && self.fitting.is_none();
                let label = if self.voxels { "Export voxels" } else { "Export mesh" };
                ui.horizontal(|ui| {
                    if ui.add_enabled(ready, egui::Button::new(RichText::new(format!("{}  {label}", crate::icons::EXPORT)).strong()).fill(theme::ACCENT_DIM)).clicked() {
                        self.start(scene, gpu.clone());
                    }
                    if !has_target {
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
        let voxels = self.voxels;
        let Some(path) = (if voxels { self.vox_path.clone() } else { self.path.clone() }) else { return };
        let vox = self.vox.clone();
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
                let used = if voxels { rmb_core::voxel_export::limited(&settings, &vox) } else { settings.clone() };
                let mut gpu_field = match gpu {
                    Some(ctx) => Some(rmb_gpu::GridField::new(ctx, &sc, Grid::new(&used))?),
                    None => None,
                };
                let field = gpu_field.as_mut().map(|f| f as &mut dyn rmb_core::mesh_export::Field);
                let mut report = |p: f32| {
                    pr.store(p.to_bits(), Ordering::Relaxed);
                    !ca.load(Ordering::Relaxed)
                };
                if voxels {
                    let (n, files) = rmb_core::voxel_export::export(&sc, &used, &vox, field, &path, &mut report)?;
                    return Ok(format!("Saved {} ({n} voxels, {} file{})", path.display(), files.len(), if files.len() == 1 { "" } else { "s" }));
                }
                let (v, t, pieces) = rmb_core::mesh_export::export(&sc, &used, field, &path, &mut report)?;
                let removed = if pieces > 0 { format!(", {pieces} loose pieces removed") } else { String::new() };
                Ok(format!("Saved {} ({v} vertices, {t} triangles{removed})", path.display()))
            })();
            let _ = tx.send(res);
        });
        self.message = None;
        self.job = Some(Job { progress, cancel, rx, started: Instant::now() });
    }
}
