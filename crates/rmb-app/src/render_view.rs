// SPDX-License-Identifier: LGPL-2.1-or-later

//! Render window: a test render at the export size (or a share of it) with everything the
//! export applies (denoiser, lens flares, bloom, streaks, post processing). The image refines
//! while it renders; afterwards the post processing settings change it at once (the raw image
//! is kept and developed again in the background). It can be saved as a PNG with the scene.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use egui::RichText;
use rmb_core::Scene;
use rmb_core::render::denoise::HdrImage;
use rmb_core::scene::Post;
use rmb_gpu::GpuContext;

use crate::{icons, theme, widgets};

enum Msg {
    /// An intermediate image and the samples in it.
    Snapshot(HdrImage, u32),
    Done(Result<HdrImage, String>),
}

struct Job {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Msg>,
    started: Instant,
    /// The scene being rendered (at the render size).
    scene: Scene,
}

/// A developed image: RGBA, size and what was used (post settings, raw image generation) plus
/// the denoised image it made (to reuse while the denoise settings stay).
struct Developed {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    post: Post,
    generation: u64,
    denoised: Option<(f64, Arc<HdrImage>)>,
    note: String,
}

pub struct RenderView {
    pub open: bool,
    /// Share of the scene's image size in percent.
    percent: u32,
    samples: u32,
    job: Option<Job>,
    /// The latest raw image, the scene it was rendered with, its samples and a generation that
    /// changes with every new raw image.
    raw: Option<(Arc<HdrImage>, Scene, u32)>,
    generation: u64,
    /// Denoised version of the raw image (blend), when made.
    denoised: Option<(f64, Arc<HdrImage>)>,
    /// Background development of the raw image.
    developing: Option<mpsc::Receiver<Developed>>,
    shown: Option<Developed>,
    tex: Option<egui::TextureHandle>,
    fit: bool,
    status: String,
}

impl RenderView {
    pub fn new(samples: u32) -> Self {
        RenderView { open: false, percent: 50, samples, job: None, raw: None, generation: 0, denoised: None, developing: None, shown: None, tex: None, fit: true, status: String::new() }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Opens the window and starts a render of `scene`.
    pub fn open_and_render(&mut self, scene: &Scene, gpu: Option<GpuContext>) {
        self.open = true;
        self.start(scene, gpu);
    }

    fn size(&self, scene: &Scene) -> (u32, u32) {
        let s = self.percent as f64 / 100.0;
        (((scene.width as f64 * s).round() as u32).max(16), ((scene.height as f64 * s).round() as u32).max(16))
    }

    fn start(&mut self, scene: &Scene, gpu: Option<GpuContext>) {
        if let Some(j) = &self.job {
            j.cancel.store(true, Ordering::Relaxed);
        }
        let Some(ctx) = gpu else {
            self.status = "The render window needs the GPU renderer".into();
            return;
        };
        let (w, h) = self.size(scene);
        let mut sc = scene.clone();
        sc.width = w;
        sc.height = h;
        sc.animation = None;
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (pr, ca, n) = (progress.clone(), cancel.clone(), self.samples.max(1));
        let job_scene = sc.clone();
        std::thread::spawn(move || {
            let snap = tx.clone();
            let res = rmb_gpu::render_progressive(
                ctx,
                &sc,
                w,
                h,
                n,
                |p| {
                    pr.store(p.to_bits(), Ordering::Relaxed);
                    !ca.load(Ordering::Relaxed)
                },
                |img, done| {
                    let _ = snap.send(Msg::Snapshot(img, done));
                },
            );
            let _ = tx.send(Msg::Done(res));
        });
        self.raw = None;
        self.status = format!("Rendering {w} × {h}, {n} samples…");
        self.job = Some(Job { progress, cancel, rx, started: Instant::now(), scene: job_scene });
    }

    /// Takes new raw images from the render job.
    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        let scene = &job.scene.clone();
        let msgs: Vec<Msg> = job.rx.try_iter().collect();
        let secs = job.started.elapsed().as_secs_f64();
        let mut done = None;
        for m in msgs {
            match m {
                Msg::Snapshot(img, n) => self.set_raw(img, scene, n),
                Msg::Done(r) => done = Some(r),
            }
        }
        if let Some(r) = done {
            self.job = None;
            match r {
                Ok(img) => {
                    let n = self.samples.max(1);
                    self.set_raw(img, scene, n);
                    self.status = format!("Done: {n} samples in {secs:.1} s. Post processing changes show here at once.");
                }
                Err(e) => self.status = if e == "cancelled" { "Cancelled".into() } else { format!("Render failed: {e}") },
            }
        }
    }

    fn set_raw(&mut self, img: HdrImage, scene: &Scene, samples: u32) {
        self.generation += 1;
        self.raw = Some((Arc::new(img), scene.clone(), samples));
        self.denoised = None;
    }

    /// Starts developing the raw image with `post` when it or the settings changed.
    fn develop(&mut self, post: &Post) {
        if let Some(rx) = &self.developing {
            match rx.try_recv() {
                Ok(d) => {
                    if d.generation == self.generation {
                        self.denoised = d.denoised.clone().or(self.denoised.take());
                    }
                    self.shown = Some(d);
                    self.tex = None;
                    self.developing = None;
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.developing = None,
            }
        }
        let Some((raw, scene, samples)) = &self.raw else { return };
        if self.shown.as_ref().is_some_and(|d| d.generation == self.generation && d.post == *post) {
            return;
        }
        let (raw, mut scene, samples) = (raw.clone(), scene.clone(), *samples);
        scene.post = post.clone();
        let generation = self.generation;
        // Intermediate images are shown without the denoiser (it would run for every one).
        let final_image = self.job.is_none();
        let cached = self.denoised.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let post = &scene.post;
            let mut note = format!("{samples} samples");
            let mut denoised = None;
            let base: Arc<HdrImage> = if post.denoise && post.denoise_blend > 0.0 && final_image {
                match cached.filter(|(b, _)| *b == post.denoise_blend) {
                    Some((b, d)) => {
                        denoised = Some((b, d.clone()));
                        d
                    }
                    None => {
                        let mut d = (*raw).clone();
                        match d.denoise(post.denoise_blend) {
                            Ok(dev) => {
                                note.push_str(&format!(", denoised on the {}", dev.label()));
                                let d = Arc::new(d);
                                denoised = Some((post.denoise_blend, d.clone()));
                                d
                            }
                            Err(e) => {
                                note.push_str(&format!(", not denoised: {e}"));
                                raw
                            }
                        }
                    }
                }
            } else {
                raw
            };
            let mut img = (*base).clone();
            img.add_flares(&scene);
            rmb_core::render::bloom::apply(&mut img.color, img.width, img.height, post);
            rmb_core::render::chromatic_aberration(&mut img.color, img.width, img.height, post.chromatic);
            let rgba = img.to_rgba8(post);
            let _ = tx.send(Developed { rgba, width: img.width, height: img.height, post: post.clone(), generation, denoised, note });
        });
        self.developing = Some(rx);
    }

    /// Shows the window; returns a status message when an image was saved.
    pub fn show(&mut self, ctx: &egui::Context, scene: &Scene, gpu: Option<GpuContext>) -> Option<String> {
        self.poll_job();
        if !self.open {
            if let Some(j) = &self.job {
                j.cancel.store(true, Ordering::Relaxed);
            }
            return None;
        }
        self.develop(&scene.post);
        // Slow repaints while rendering: every UI frame shares the GPU queue with the render.
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        } else if self.developing.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut saved = None;
        let mut open = self.open;
        egui::Window::new("Render").open(&mut open).default_size([960.0, 640.0]).resizable(true).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let (w, h) = self.size(scene);
                widgets::label(ui, "Size", "Share of the image size set in the Camera tab (Image size)");
                egui::ComboBox::from_id_salt("render-size").selected_text(format!("{} %  ({w} × {h})", self.percent)).show_ui(ui, |ui| {
                    for p in [25, 50, 100] {
                        ui.selectable_value(&mut self.percent, p, format!("{p} %"));
                    }
                });
                widgets::label(ui, "Samples", "Samples per pixel: more give a cleaner image");
                widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut self.samples).range(1..=4096).speed(1.0)).on_hover_text("Samples per pixel: more give a cleaner image. Double click: 64"), &mut self.samples, 64);
                if self.job.is_some() {
                    if ui.button(format!("{}  Cancel", icons::X)).on_hover_text("Stop rendering (keeps the image so far)").clicked()
                        && let Some(j) = &self.job
                    {
                        j.cancel.store(true, Ordering::Relaxed);
                    }
                } else if ui.add(egui::Button::new(RichText::new("Render").strong()).fill(theme::ACCENT_DIM)).on_hover_text("Render the current scene (F12)").clicked() {
                    self.start(scene, gpu.clone());
                }
                let can_save = self.shown.is_some() && self.job.is_none();
                if ui.add_enabled(can_save, egui::Button::new("Save PNG…")).on_hover_text("Save this image as shown, with the scene inside (open the PNG to get the scene back)").clicked()
                    && let Some(d) = &self.shown
                {
                    let dlg = crate::prefs::dialog(crate::prefs::Folder::Images).add_filter("PNG image", &["png"]).set_file_name("render.png");
                    if let Some(p) = dlg.save_file() {
                        let mut sc = self.raw.as_ref().map(|(_, s, _)| s.clone()).unwrap_or_else(|| scene.clone());
                        sc.post = d.post.clone();
                        saved = Some(match rmb_core::io::image::write_png(&p, d.width, d.height, &d.rgba, Some(&sc)) {
                            Ok(()) => format!("Saved {}", p.display()),
                            Err(e) => format!("Saving failed: {e}"),
                        });
                    }
                }
                ui.separator();
                ui.selectable_value(&mut self.fit, true, "Fit").on_hover_text("Scale the image to the window");
                ui.selectable_value(&mut self.fit, false, "100 %").on_hover_text("One image pixel per screen pixel (scroll to move)");
            });
            if let Some(job) = &self.job {
                let p = f32::from_bits(job.progress.load(Ordering::Relaxed));
                ui.add(egui::ProgressBar::new(p).show_percentage().desired_height(10.0));
            }
            let note = self.shown.as_ref().map(|d| format!("  ·  {}", d.note)).unwrap_or_default();
            let developing = if self.developing.is_some() { "  ·  developing…" } else { "" };
            ui.label(RichText::new(format!("{}{note}{developing}", self.status)).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            if self.raw.as_ref().is_some_and(|(_, s, _)| s.post.display_space != scene.post.display_space) {
                ui.label(RichText::new("Display space changed: render again to see it.").size(theme::fs(11.5)).color(theme::WARN));
            }
            ui.separator();
            if let Some(d) = &self.shown {
                let tex = self.tex.get_or_insert_with(|| {
                    let img = egui::ColorImage::from_rgba_unmultiplied([d.width as usize, d.height as usize], &d.rgba);
                    ui.ctx().load_texture("render-view", img, egui::TextureOptions::LINEAR)
                });
                let size = egui::vec2(d.width as f32, d.height as f32) / ui.ctx().pixels_per_point();
                if self.fit {
                    let avail = ui.available_size();
                    let s = (avail.x / size.x).min(avail.y / size.y).max(0.01);
                    ui.centered_and_justified(|ui| ui.image((tex.id(), size * s)));
                } else {
                    egui::ScrollArea::both().show(ui, |ui| ui.image((tex.id(), size)));
                }
            } else {
                ui.centered_and_justified(|ui| ui.label(RichText::new("Rendering…").color(theme::TEXT_MUTED)));
            }
        });
        self.open = open;
        saved
    }
}
