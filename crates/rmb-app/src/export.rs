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

/// Tiles when the image is large (`io::image::auto_tile`).
const TILES_AUTO: u32 = u32::MAX;

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
    /// Tile size: `TILES_AUTO`, 0 (off) or pixels per side.
    tiles: u32,
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
    /// A job for the render queue ("Add to queue"), taken by the app.
    pub queued: Option<rmb_core::queue::QueueJob>,
    /// Name of the scene file (job names).
    doc_name: String,
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
            tiles: TILES_AUTO,
            movie_path: None,
            format: VideoFormat::Mp4,
            from: 0,
            to: usize::MAX,
            frames: None,
            doc_dir: None,
            job: None,
            queued: None,
            doc_name: String::new(),
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
        self.doc_name = doc_path.and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Scene".into());
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
                if ui.add(egui::DragValue::new(&mut self.width).range(16..=32768).suffix(" px")).changed() {
                    self.height = ((self.width as f64) / aspect).round().max(16.0) as u32;
                }
                ui.end_row();
                widgets::label(ui, "Height", "Image height in pixels (follows the scene's aspect)");
                if ui.add(egui::DragValue::new(&mut self.height).range(16..=32768).suffix(" px")).changed() {
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
                    if ui.add_enabled(ready, egui::Button::new("Add to queue")).on_hover_text("Adds a job with these settings and a copy of the scene as it is now to the render queue (File › Render queue)").clicked() {
                        self.queued = Some(self.queue_job(scene, anim.is_some()));
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
            widgets::label(
                ui,
                "Tiles",
                "Renders large images in parts on the GPU and puts them together (sizes beyond the graphics memory, prints). Every tile renders the same pixels as the whole image would; the denoiser works per tile with an overlap. Auto: tiles of 2048 pixels above about 16 megapixels",
            );
            let name = |t: u32| match t {
                TILES_AUTO => "Auto".to_string(),
                0 => "Off".to_string(),
                t => format!("{t} px"),
            };
            egui::ComboBox::from_id_salt("export-tiles").selected_text(name(self.tiles)).show_ui(ui, |ui| {
                for t in [TILES_AUTO, 0, 1024, 2048, 4096] {
                    ui.selectable_value(&mut self.tiles, t, name(t));
                }
            });
            ui.end_row();
        });
        let (w, h) = (self.width as u64, self.height as u64);
        let tile = self.tile_size();
        let tiles = tile.map(|t| format!("{} × {} tiles, ", w.div_ceil(t as u64), h.div_ceil(t as u64))).unwrap_or_default();
        // Main memory: the image (12 bytes per pixel; EXR passes 32 more) and the output.
        let per_px = 12 + if self.kind == ImageKind::Exr { 32 } else { 8 };
        let bytes = (w * h * per_px) as f64;
        let mem = if bytes >= 1e9 { format!("{:.1} GB", bytes / 1e9) } else { format!("{:.0} MB", (bytes / 1e6).max(1.0)) };
        ui.label(RichText::new(format!("{tiles}about {mem} of memory")).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
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

    /// A render queue job with the current settings and a copy of `scene`.
    fn queue_job(&self, scene: &Scene, has_anim: bool) -> rmb_core::queue::QueueJob {
        use rmb_core::queue::{JobWhat, QueueJob};
        let mut sc = scene.clone();
        if self.mode == Mode::Image {
            sc.animation = None;
        }
        let (what, output, kind) = if self.mode == Mode::Animation && has_anim {
            (JobWhat::Animation { format: self.format, from: self.from, to: self.to }, self.movie_path.clone().unwrap_or_default(), "animation")
        } else {
            let tiles = match self.tiles {
                TILES_AUTO => None,
                t => Some(t),
            };
            (JobWhat::Image { kind: self.kind, tiles }, self.path.clone().unwrap_or_default(), "image")
        };
        QueueJob {
            id: rmb_core::queue::new_id(),
            name: format!("{} ({kind})", self.doc_name),
            scene_file: self.doc_dir.as_ref().map(|d| d.join(format!("{}.rmb", self.doc_name))),
            scene: Some(sc.to_json()),
            what,
            width: self.width,
            height: self.height,
            samples: self.samples,
            output,
            ..Default::default()
        }
    }

    /// The tile size of the current settings (None: the whole image at once).
    fn tile_size(&self) -> Option<u32> {
        match self.tiles {
            TILES_AUTO => rmb_core::io::image::auto_tile(self.width, self.height),
            0 => None,
            t => Some(t),
        }
    }

    fn start(&mut self, scene: &Scene, gpu: Option<GpuContext>) {
        let (Some(path), Some(ctx)) = (self.path.clone(), gpu) else { return };
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (w, h, n) = (self.width, self.height, self.samples);
        let kind = self.kind;
        let tile = self.tile_size();
        let mut sc = scene.clone();
        sc.width = w;
        sc.height = h;
        sc.animation = None;
        let (pr, ca) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let report = |p: f32| {
                pr.store(p.to_bits(), Ordering::Relaxed);
                !ca.load(Ordering::Relaxed)
            };
            let res = match tile {
                // Tiles are denoised one by one while rendering.
                Some(t) => rmb_gpu::render_tiled_hdr(ctx, &sc, w, h, n, t, kind == rmb_core::io::image::ImageKind::Exr, report).and_then(|(img, outcome)| rmb_core::io::image::save_render_ex(&path, img, &sc, kind, true).map(|_| outcome)),
                None => rmb_gpu::render_blocking_hdr(ctx, &sc, w, h, n, report).and_then(|img| rmb_core::io::image::save_render(&path, img, &sc, kind)),
            }
            .map(|outcome| {
                let note = if sc.post.denoise { format!(", {}", outcome.describe()) } else { String::new() };
                (path.clone(), note)
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
        let doc_dir = self.doc_dir.clone();
        let size = (scene.width, scene.height);
        let (pr, ca, pv) = (progress.clone(), cancel.clone(), preview.clone());
        std::thread::spawn(move || {
            let run = || -> Result<(PathBuf, String), String> {
                let mut anim = anim;
                anim.ensure_audio_levels(doc_dir.as_deref(), &ffmpeg)?;
                let plan = AnimationPlan::new(&anim)?;
                let mut sink = VideoSink::new(&path, format, w, h, plan.fps(), &ffmpeg, audio.as_ref())?;
                let frame_scene = |t: f64| {
                    let mut s = plan.scene_at_time(t);
                    (s.width, s.height) = size;
                    s
                };
                rmb_gpu::render_animation(Some(ctx), from..to + 1, w, h, n, anim.shutter, frame_scene, &ca, |p| pr.store(p.to_bits(), Ordering::Relaxed), format.hdr_frames(), |f, frame| {
                    // The preview shows 8-bit frames (HDR frames are written as they are).
                    if let rmb_core::video::Frame::Rgba(rgba) = &frame {
                        *pv.lock().unwrap_or_else(|e| e.into_inner()) = Some((w, h, rgba.clone()));
                    }
                    sink.write(f - from, frame)
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
