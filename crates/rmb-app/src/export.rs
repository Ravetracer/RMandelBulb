// SPDX-License-Identifier: LGPL-2.1-or-later

//! Export window: renders the scene at full size in the background and saves a PNG with the
//! scene in its metadata (open the PNG to get the scene back).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use egui::RichText;
use rmb_core::Scene;
use rmb_gpu::GpuContext;

use crate::{theme, widgets};

struct Job {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<(PathBuf, String), String>>,
    started: Instant,
}

pub struct Export {
    pub open: bool,
    width: u32,
    height: u32,
    pub samples: u32,
    path: Option<PathBuf>,
    job: Option<Job>,
    pub message: Option<String>,
}

impl Export {
    pub fn new(samples: u32) -> Self {
        Export { open: false, width: 0, height: 0, samples, path: None, job: None, message: None }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn open_for(&mut self, scene: &Scene, doc_path: Option<&PathBuf>) {
        self.open = true;
        self.width = scene.width;
        self.height = scene.height;
        if self.path.is_none() {
            self.path = doc_path.map(|p| p.with_extension("png"));
        }
    }

    /// Shows the window; returns a status message when an export finished.
    pub fn show(&mut self, ctx: &egui::Context, scene: &Scene, gpu: Option<GpuContext>, last_dir: Option<&PathBuf>) -> Option<String> {
        let mut finished = None;
        if let Some(job) = &self.job {
            match job.rx.try_recv() {
                Ok(Ok((p, note))) => {
                    let msg = format!("Saved {} ({:.1} s{note})", p.display(), job.started.elapsed().as_secs_f64());
                    self.message = Some(msg.clone());
                    finished = Some(msg);
                    self.job = None;
                }
                Ok(Err(e)) => {
                    let msg = format!("Export failed: {e}");
                    self.message = Some(msg.clone());
                    finished = Some(msg);
                    self.job = None;
                }
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        if !self.open {
            return finished;
        }
        let mut open = self.open;
        egui::Window::new("Export image").open(&mut open).resizable(false).collapsible(false).default_width(360.0).show(ctx, |ui| {
            let aspect = scene.aspect();
            widgets::grid(ui, "export", |ui| {
                widgets::label(ui, "Width", "Image width in pixels");
                if ui.add(egui::DragValue::new(&mut self.width).range(16..=16384).suffix(" px")).changed() {
                    self.height = ((self.width as f64) / aspect).round().max(16.0) as u32;
                }
                ui.end_row();
                widgets::label(ui, "Height", "Image height in pixels (follows the scene's aspect)");
                if ui.add(egui::DragValue::new(&mut self.height).range(16..=16384).suffix(" px")).changed() {
                    self.width = ((self.height as f64) * aspect).round().max(16.0) as u32;
                }
                ui.end_row();
                widgets::label(ui, "Scale", "");
                ui.horizontal(|ui| {
                    for (f, t) in [(0.5, "½×"), (1.0, "1×"), (2.0, "2×"), (4.0, "4×")] {
                        if ui.small_button(t).on_hover_text("Relative to the scene's image size").clicked() {
                            self.width = ((scene.width as f64) * f).round() as u32;
                            self.height = ((scene.height as f64) * f).round() as u32;
                        }
                    }
                });
                ui.end_row();
                widgets::slider_u32(ui, "Samples", "Samples per pixel: anti-aliasing, soft shadows, ray traced occlusion and depth of field get smoother with more", &mut self.samples, 1..=1024, 128);
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "No file chosen".into());
                if ui.button("File…").clicked() {
                    let mut dlg = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("render.png");
                    if let Some(d) = self.path.as_ref().and_then(|p| p.parent()).or(last_dir.map(|p| p.as_path())) {
                        dlg = dlg.set_directory(d);
                    }
                    if let Some(p) = dlg.save_file() {
                        self.path = Some(if p.extension().is_none() { p.with_extension("png") } else { p });
                    }
                }
                ui.label(RichText::new(label).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            });
            ui.add_space(8.0);
            if let Some(job) = &self.job {
                let p = f32::from_bits(job.progress.load(Ordering::Relaxed));
                ui.add(egui::ProgressBar::new(p).show_percentage().animate(true));
                let el = job.started.elapsed().as_secs_f64();
                let eta = if p > 0.01 { format!(" · about {:.0} s left", el / p as f64 - el) } else { String::new() };
                ui.label(RichText::new(format!("{el:.0} s{eta}")).color(theme::TEXT_MUTED));
                if ui.button("Cancel").clicked() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            } else {
                let can = self.path.is_some() && gpu.is_some();
                ui.horizontal(|ui| {
                    if ui.add_enabled(can, egui::Button::new(RichText::new(format!("{}  Render and save", crate::icons::EXPORT)).strong()).fill(theme::ACCENT_DIM)).clicked() {
                        self.start(scene, gpu.clone());
                    }
                    if gpu.is_none() {
                        ui.label(RichText::new("Needs a GPU; use rmb-cli render --cpu instead").color(theme::WARN));
                    } else if self.path.is_none() {
                        ui.label(RichText::new("Choose a file first").color(theme::TEXT_MUTED));
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
        let (Some(path), Some(ctx)) = (self.path.clone(), gpu) else { return };
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (w, h, n) = (self.width, self.height, self.samples);
        let mut sc = scene.clone();
        sc.width = w;
        sc.height = h;
        let (pr, ca) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let res = rmb_gpu::render_blocking_hdr(ctx, &sc, w, h, n, |p| {
                pr.store(p.to_bits(), Ordering::Relaxed);
                !ca.load(Ordering::Relaxed)
            })
            .and_then(|img| {
                let (rgba, outcome) = img.finish(&sc.post);
                let note = if sc.post.denoise { format!(", {}", outcome.describe()) } else { String::new() };
                rmb_core::io::image::write_png(&path, w, h, &rgba, Some(&sc)).map(|_| (path.clone(), note))
            });
            let _ = tx.send(res);
        });
        self.message = None;
        self.job = Some(Job { progress, cancel, rx, started: Instant::now() });
    }
}
