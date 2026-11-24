// SPDX-License-Identifier: LGPL-2.1-or-later

//! Export window: renders the scene at full size in the background and saves a PNG with the
//! scene in its metadata (open the PNG to get the scene back), or renders the animation into a
//! movie file or numbered PNG images.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use egui::RichText;
use rmb_core::Scene;
use rmb_core::animation::{Animation, AnimationPlan};
use rmb_core::video::{VideoFormat, VideoSink};
use rmb_gpu::GpuContext;

use crate::{theme, widgets};

/// The last finished frame of an animation export (size, RGBA), for the preview.
type Preview = Arc<Mutex<Option<(u32, u32, Vec<u8>)>>>;

struct Job {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<(PathBuf, String), String>>,
    started: Instant,
    preview: Option<Preview>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Image,
    Animation,
}

pub struct Export {
    pub open: bool,
    mode: Mode,
    width: u32,
    height: u32,
    pub samples: u32,
    path: Option<PathBuf>,
    /// Image file kind (PNG 8/16 bit, EXR).
    kind: rmb_core::io::image::ImageKind,
    /// Animation: output file (movies) or folder (images), format and frame range.
    movie_path: Option<PathBuf>,
    format: VideoFormat,
    from: usize,
    to: usize,
    /// Frame count of the last animation seen (rebuilt when it changes).
    frames: Option<(Animation, Result<usize, String>)>,
    /// Folder of the scene file (relative sound paths).
    doc_dir: Option<PathBuf>,
    job: Option<Job>,
    preview_tex: Option<egui::TextureHandle>,
    pub message: Option<String>,
}

impl Export {
    pub fn new(samples: u32) -> Self {
        Export {
            open: false,
            mode: Mode::Image,
            width: 0,
            height: 0,
            samples,
            path: None,
            kind: Default::default(),
            movie_path: None,
            format: VideoFormat::Mp4,
            from: 0,
            to: usize::MAX,
            frames: None,
            doc_dir: None,
            job: None,
            preview_tex: None,
            message: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn open_for(&mut self, scene: &Scene, doc_path: Option<&PathBuf>) {
        self.open = true;
        self.doc_dir = doc_path.and_then(|p| p.parent()).map(Path::to_path_buf);
        self.width = scene.width;
        self.height = scene.height;
        if self.path.is_none() {
            self.path = doc_path.map(|p| p.with_extension(self.kind.extension()));
        }
        if self.movie_path.is_none() {
            self.movie_path = doc_path.map(|p| movie_path(p, self.format));
        }
    }

    /// Opens the window on the animation part.
    pub fn open_animation(&mut self, scene: &Scene, doc_path: Option<&PathBuf>) {
        self.open_for(scene, doc_path);
        self.mode = Mode::Animation;
    }

    /// Frame count of the scene's animation (cached).
    fn frame_count(&mut self, anim: &Animation) -> Result<usize, String> {
        if self.frames.as_ref().is_none_or(|(a, _)| a != anim) {
            self.frames = Some((anim.clone(), AnimationPlan::new(anim).map(|p| p.frame_count())));
        }
        self.frames.as_ref().map(|(_, n)| n.clone()).unwrap_or(Ok(0))
    }

    /// Shows the window; returns a status message when an export finished.
    pub fn show(&mut self, ctx: &egui::Context, scene: &Scene, gpu: Option<GpuContext>, ffmpeg: Option<&Path>) -> Option<String> {
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
        let anim = scene.animation.clone().filter(|a| !a.keyframes.is_empty());
        if anim.is_none() {
            self.mode = Mode::Image;
        }
        let program = rmb_core::video::ffmpeg_program(ffmpeg);
        let mut open = self.open;
        let title = if self.mode == Mode::Image { "Export image" } else { "Export animation" };
        egui::Window::new(title).id(egui::Id::new("export")).open(&mut open).resizable(false).collapsible(false).default_width(380.0).show(ctx, |ui| {
            if anim.is_some() {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(self.job.is_none(), |ui| {
                        ui.selectable_value(&mut self.mode, Mode::Image, "Image").on_hover_text("Render the current view as one PNG");
                        ui.selectable_value(&mut self.mode, Mode::Animation, "Animation").on_hover_text("Render the keyframe animation as a movie or as numbered PNG images");
                    });
                });
                ui.add_space(4.0);
            }
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
            match (self.mode, &anim) {
                (Mode::Animation, Some(a)) => self.animation_settings(ui, a, &program),
                _ => self.image_settings(ui),
            }
            ui.add_space(8.0);
            if let Some(job) = &self.job {
                let p = f32::from_bits(job.progress.load(Ordering::Relaxed));
                ui.add(egui::ProgressBar::new(p).show_percentage().animate(true));
                let el = job.started.elapsed().as_secs_f64();
                let eta = if p > 0.01 { format!(" · about {:.0} s left", el / p as f64 - el) } else { String::new() };
                ui.label(RichText::new(format!("{el:.0} s{eta}")).color(theme::TEXT_MUTED));
                if let Some(pv) = &job.preview {
                    let frame = pv.lock().unwrap_or_else(|e| e.into_inner()).take();
                    if let Some((w, h, rgba)) = frame {
                        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                        match &mut self.preview_tex {
                            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                            None => self.preview_tex = Some(ctx.load_texture("export-preview", img, egui::TextureOptions::LINEAR)),
                        }
                    }
                    if let Some(t) = &self.preview_tex {
                        let size = t.size_vec2();
                        let s = (360.0 / size.x).min(200.0 / size.y);
                        ui.image((t.id(), size * s)).on_hover_text("The last finished frame");
                    }
                }
                if ui.button("Cancel").clicked() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            } else {
                let ready = match self.mode {
                    Mode::Image => self.path.is_some(),
                    Mode::Animation => self.movie_path.is_some() && (!self.format.needs_ffmpeg() || rmb_core::video::ffmpeg_available(&program)),
                };
                let can = ready && gpu.is_some();
                ui.horizontal(|ui| {
                    if ui.add_enabled(can, egui::Button::new(RichText::new(format!("{}  Render and save", crate::icons::EXPORT)).strong()).fill(theme::ACCENT_DIM)).clicked() {
                        match (self.mode, &anim) {
                            (Mode::Animation, Some(a)) => self.start_animation(scene, a, gpu.clone(), program.clone()),
                            _ => self.start(scene, gpu.clone()),
                        }
                    }
                    if gpu.is_none() {
                        ui.label(RichText::new("Needs a GPU; use rmb-cli render --cpu instead").color(theme::WARN));
                    } else if !ready {
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

    fn image_settings(&mut self, ui: &mut egui::Ui) {
        use rmb_core::io::image::ImageKind;
        widgets::grid(ui, "export-kind", |ui| {
            widgets::label(
                ui,
                "Format",
                "PNG 8-bit: the image as shown, with the scene inside (open it to get the scene back). PNG 16-bit: the same with 16 bits per channel, for smooth gradients in further editing. EXR: linear light before the Image settings (denoised as set; no lens flares, bloom or color grading), 32-bit float, with the passes coverage, Z (distance from the camera), albedo and normal for compositing",
            );
            let before = self.kind;
            egui::ComboBox::from_id_salt("export-kind").selected_text(self.kind.label()).show_ui(ui, |ui| {
                for k in ImageKind::ALL {
                    ui.selectable_value(&mut self.kind, k, k.label());
                }
            });
            if self.kind != before
                && let Some(p) = &mut self.path
            {
                *p = p.with_extension(self.kind.extension());
            }
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let label = self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "No file chosen".into());
            let ext = self.kind.extension();
            if ui.button("File…").on_hover_text("Where to save the image").clicked() {
                let filter = if self.kind == ImageKind::Exr { "OpenEXR image" } else { "PNG image" };
                let mut dlg = crate::prefs::dialog(crate::prefs::Folder::Images).add_filter(filter, &[ext]).set_file_name(format!("render.{ext}"));
                if let Some(d) = self.path.as_ref().and_then(|p| p.parent()) {
                    dlg = dlg.set_directory(d);
                }
                if let Some(p) = dlg.save_file() {
                    self.path = Some(p.with_extension(ext));
                }
            }
            ui.label(RichText::new(label).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
        });
    }

    fn animation_settings(&mut self, ui: &mut egui::Ui, anim: &Animation, program: &Path) {
        let count = self.frame_count(anim);
        let has_ffmpeg = rmb_core::video::ffmpeg_available(program);
        widgets::grid(ui, "export-anim", |ui| {
            widgets::label(ui, "Format", "Movie formats are written by ffmpeg (Preferences › Movies); PNG images need nothing extra");
            let before = self.format;
            egui::ComboBox::from_id_salt("movie-format").selected_text(self.format.label()).show_ui(ui, |ui| {
                for f in VideoFormat::ALL {
                    ui.add_enabled_ui(!f.needs_ffmpeg() || has_ffmpeg, |ui| ui.selectable_value(&mut self.format, f, f.label()));
                }
            });
            ui.end_row();
            if self.format != before
                && let Some(p) = &self.movie_path
            {
                self.movie_path = Some(movie_path(p, self.format));
            }
            match &count {
                Ok(n) => {
                    let last = n.saturating_sub(1);
                    self.to = self.to.min(last);
                    self.from = self.from.min(self.to);
                    widgets::label(ui, "Frames", "First and last frame to render (both included)");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.from).range(0..=self.to));
                        ui.label("to");
                        ui.add(egui::DragValue::new(&mut self.to).range(self.from..=last));
                        if ui.small_button("All").on_hover_text("All frames").clicked() {
                            self.from = 0;
                            self.to = last;
                        }
                    });
                    ui.end_row();
                    widgets::label(ui, "Length", "Frames and duration at the animation's frame rate");
                    ui.label(RichText::new(format!("{} frames · {:.1} s at {} fps", self.to + 1 - self.from, (self.to + 1 - self.from) as f64 / anim.fps.max(1.0), anim.fps)).color(theme::TEXT_MUTED));
                    ui.end_row();
                }
                Err(e) => {
                    widgets::label(ui, "Frames", "");
                    ui.label(RichText::new(e).color(theme::WARN));
                    ui.end_row();
                }
            }
        });
        if let Some(a) = anim.audio_track(self.from, self.doc_dir.as_deref()) {
            let name = a.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let (text, color) = if !self.format.has_audio() {
                (format!("Sound {name} left out: {} has no sound", self.format.label()), theme::TEXT_MUTED)
            } else if !a.path.is_file() {
                (format!("Sound file not found: {}", a.path.display()), theme::WARN)
            } else {
                (format!("Sound: {name} from {:.2} s", a.start_s), theme::TEXT_MUTED)
            };
            ui.label(RichText::new(text).size(theme::fs(11.5)).color(color)).on_hover_text("The animation's sound track (Animation panel › Movie)");
        }
        if !has_ffmpeg {
            ui.label(RichText::new("ffmpeg not found: only PNG images (set it in Preferences › Movies)").size(theme::fs(11.5)).color(theme::WARN));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let label = self.movie_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Nothing chosen".into());
            let (text, tip) = if self.format.needs_ffmpeg() { ("File…", "Where to save the movie") } else { ("Folder…", "Folder for the numbered PNG images (frame_00000.png …)") };
            if ui.button(text).on_hover_text(tip).clicked() {
                let mut dlg = crate::prefs::dialog(crate::prefs::Folder::Movies);
                if let Some(d) = self.movie_path.as_ref().and_then(|p| p.parent()) {
                    dlg = dlg.set_directory(d);
                }
                let picked = match self.format.extension() {
                    Some(ext) => dlg.add_filter(self.format.label(), &[ext]).set_file_name(format!("animation.{ext}")).save_file().map(|p| movie_path(&p, self.format)),
                    None => dlg.pick_folder(),
                };
                if let Some(p) = picked {
                    self.movie_path = Some(p);
                }
            }
            ui.label(RichText::new(label).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
        });
    }

    fn start(&mut self, scene: &Scene, gpu: Option<GpuContext>) {
        let (Some(path), Some(ctx)) = (self.path.clone(), gpu) else { return };
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (w, h, n) = (self.width, self.height, self.samples);
        let kind = self.kind;
        let mut sc = scene.clone();
        sc.width = w;
        sc.height = h;
        sc.animation = None;
        let (pr, ca) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let res = rmb_gpu::render_blocking_hdr(ctx, &sc, w, h, n, |p| {
                pr.store(p.to_bits(), Ordering::Relaxed);
                !ca.load(Ordering::Relaxed)
            })
            .and_then(|img| {
                let outcome = rmb_core::io::image::save_render(&path, img, &sc, kind)?;
                let note = if sc.post.denoise { format!(", {}", outcome.describe()) } else { String::new() };
                Ok((path.clone(), note))
            });
            let _ = tx.send(res);
        });
        self.message = None;
        self.job = Some(Job { progress, cancel, rx, started: Instant::now(), preview: None });
    }

    fn start_animation(&mut self, scene: &Scene, anim: &Animation, gpu: Option<GpuContext>, ffmpeg: PathBuf) {
        let (Some(path), Some(ctx)) = (self.movie_path.clone(), gpu) else { return };
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let preview: Preview = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel();
        let (w, h, n, from, to, format) = (self.width, self.height, self.samples, self.from, self.to, self.format);
        let audio = anim.audio_track(from, self.doc_dir.as_deref());
        let anim = anim.clone();
        let size = (scene.width, scene.height);
        let (pr, ca, pv) = (progress.clone(), cancel.clone(), preview.clone());
        std::thread::spawn(move || {
            let run = || -> Result<(PathBuf, String), String> {
                let plan = AnimationPlan::new(&anim)?;
                let mut sink = VideoSink::new(&path, format, w, h, plan.fps(), &ffmpeg, audio.as_ref())?;
                let frame_scene = |t: f64| {
                    let mut s = plan.scene_at_time(t);
                    (s.width, s.height) = size;
                    s
                };
                rmb_gpu::render_animation(Some(ctx), from..to + 1, w, h, n, anim.shutter, frame_scene, &ca, |p| pr.store(p.to_bits(), Ordering::Relaxed), |f, rgba| {
                    sink.write_frame(f - from, &rgba)?;
                    *pv.lock().unwrap_or_else(|e| e.into_inner()) = Some((w, h, rgba));
                    Ok(())
                })?;
                sink.finish()?;
                Ok((path.clone(), format!(", {} frames", to + 1 - from)))
            };
            let _ = tx.send(run());
        });
        self.message = None;
        self.preview_tex = None;
        self.job = Some(Job { progress, cancel, rx, started: Instant::now(), preview: Some(preview) });
    }
}

/// The output path for `format` next to `p` (movies: the format's extension; images: a folder
/// named like the file).
fn movie_path(p: &Path, format: VideoFormat) -> PathBuf {
    match format.extension() {
        Some(ext) => p.with_extension(ext),
        None => p.with_extension(""),
    }
}
