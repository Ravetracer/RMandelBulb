// SPDX-License-Identifier: LGPL-2.1-or-later

//! The render view: progressive preview, mouse and keyboard navigation, picking.
//!
//! Orbit mode: left drag orbits around the surface point under the cursor, right drag pans,
//! middle drag looks around, the wheel moves toward the point under the cursor.
//! Fly mode: WASD / R F move, Q E roll, left drag looks, the wheel changes the speed; the speed
//! follows the distance to the surface, so flying stays controllable at any zoom.
//! While navigating the preview renders at a lower resolution without shadows.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::{Color32, Key, PointerButton, Rect, Sense, Ui, pos2, vec2};
use rmb_core::render::cpu::{CpuRenderer, distance_at_camera, pick};
use rmb_core::{Scene, V3};
use rmb_gpu::{GpuContext, GpuRenderer};

use crate::doc::Document;
use crate::prefs::Prefs;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavMode {
    Orbit,
    Fly,
}

/// How long after the last navigation input the preview stays in the fast mode.
const SETTLE: Duration = Duration::from_millis(250);

struct CpuJob {
    scene: Scene,
    size: (u32, u32),
    rx: mpsc::Receiver<(Vec<u8>, u32)>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// What a denoised preview shows: scene, size and sample count.
type DenoiseKey = (Scene, (u32, u32), u32);

/// A denoised preview being computed.
struct DenoiseJob {
    key: DenoiseKey,
    rx: mpsc::Receiver<(Vec<u8>, rmb_core::render::denoise::Outcome)>,
}

pub struct Viewport {
    gpu: Option<GpuRenderer>,
    /// Show the preview through the AI denoiser.
    pub show_denoised: bool,
    denoise_job: Option<DenoiseJob>,
    /// Last denoised preview and what it shows.
    denoised: Option<(egui::TextureHandle, DenoiseKey)>,
    /// What the denoiser did last (for the status line).
    pub denoise_status: Option<String>,
    pub gpu_error: Option<String>,
    texture: Option<(egui::TextureId, u64)>,
    cpu_texture: Option<egui::TextureHandle>,
    cpu_job: Option<CpuJob>,
    cpu_samples: u32,
    pub mode: NavMode,
    last_nav: Option<Instant>,
    pivot: Option<V3>,
    /// Last positive distance estimate at the camera (navigation speed inside the set).
    nav_scale: f64,
    /// Fly speed multiplier (wheel in fly mode).
    pub fly_speed: f64,
    /// Rendered size of the last frame.
    pub render_size: (u32, u32),
    pass_start: Instant,
    /// Scene and size the timing refers to.
    timed: Option<(Scene, (u32, u32))>,
    pub converged_after: Option<Duration>,
    /// Cursor position over the image in screen coordinates (-1..1), for the status line.
    pub hover: Option<(f64, f64)>,
    /// Do not render (an export uses the GPU).
    pub paused: bool,
    /// An animation frame shown instead of the scene (navigation is off), and whether it is
    /// rendered fast (playback).
    pub preview: Option<(Scene, bool, String)>,
    /// Camera path drawn over the image.
    pub overlay: Option<PathOverlay>,
    /// Flight being recorded.
    pub recording: Option<Recording>,
}

/// The animation's camera path, drawn over the view.
pub struct PathOverlay {
    pub path: Vec<V3>,
    pub keys: Vec<V3>,
    pub selected: Option<usize>,
    /// Position of the frame shown in the timeline.
    pub current: Option<V3>,
}

/// A flight being recorded: camera samples with their time.
pub struct Recording {
    pub start: Instant,
    pub samples: Vec<rmb_core::animation::CamSample>,
}

impl Viewport {
    pub fn new(render_state: Option<&egui_wgpu::RenderState>) -> Self {
        let (gpu, gpu_error) = match render_state {
            Some(rs) => {
                let ctx = GpuContext { device: rs.device.clone(), queue: rs.queue.clone(), adapter_name: rs.adapter.get_info().name };
                (Some(GpuRenderer::new(ctx)), None)
            }
            None => (None, Some("no GPU (wgpu) available: rendering on the CPU".to_string())),
        };
        Viewport {
            gpu,
            show_denoised: false,
            denoise_job: None,
            denoised: None,
            denoise_status: None,
            gpu_error,
            texture: None,
            cpu_texture: None,
            cpu_job: None,
            cpu_samples: 0,
            mode: NavMode::Orbit,
            last_nav: None,
            pivot: None,
            nav_scale: 0.1,
            fly_speed: 1.0,
            render_size: (0, 0),
            pass_start: Instant::now(),
            timed: None,
            converged_after: None,
            hover: None,
            paused: false,
            preview: None,
            overlay: None,
            recording: None,
        }
    }

    /// Forgets navigation state that belongs to the previous scene (orbit pivot, speed).
    pub fn reset_navigation(&mut self) {
        self.pivot = None;
        self.nav_scale = 0.1;
    }

    pub fn gpu_context(&self) -> Option<GpuContext> {
        self.gpu.as_ref().map(|g| g.context().clone())
    }

    pub fn adapter_name(&self) -> String {
        self.gpu.as_ref().map(|g| g.context().adapter_name.clone()).unwrap_or_else(|| "CPU".into())
    }

    pub fn samples(&self) -> u32 {
        self.gpu.as_ref().map(|g| g.samples()).unwrap_or(self.cpu_samples)
    }

    pub fn error(&self) -> Option<String> {
        self.gpu.as_ref().and_then(|g| g.last_error())
    }

    pub fn compiling(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| g.compiling())
    }

    pub fn navigating(&self) -> bool {
        self.last_nav.is_some_and(|t| t.elapsed() < SETTLE)
    }

    fn nav(&mut self) {
        self.last_nav = Some(Instant::now());
    }

    /// Image rectangle inside the canvas.
    fn image_rect(canvas: Rect, scene: &Scene, prefs: &Prefs) -> Rect {
        if !prefs.keep_aspect {
            return canvas;
        }
        let a = scene.aspect() as f32;
        let (cw, ch) = (canvas.width(), canvas.height());
        let (w, h) = if cw / ch > a { (ch * a, ch) } else { (cw, cw / a) };
        Rect::from_center_size(canvas.center(), vec2(w, h))
    }

    pub fn show(&mut self, ui: &mut Ui, frame: &eframe::Frame, doc: &mut Document, prefs: &Prefs) {
        let canvas = ui.available_rect_before_wrap();
        ui.painter().rect_filled(canvas, 0.0, theme::CANVAS);
        let rect = Self::image_rect(canvas.shrink(if prefs.keep_aspect { 12.0 } else { 0.0 }), &doc.scene, prefs);
        let resp = ui.interact(rect, ui.id().with("viewport"), Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let full = ((rect.width() * ppp * prefs.preview_percent as f32 / 100.0).round().max(16.0) as u32, (rect.height() * ppp * prefs.preview_percent as f32 / 100.0).round().max(16.0) as u32);

        let previewing = self.preview.is_some();
        if previewing {
            self.hover = resp.hover_pos().map(|p| Self::screen(rect, p));
        } else {
            self.navigate(ui, &resp, rect, doc, prefs);
        }
        if let Some(rec) = self.recording.as_mut() {
            rec.samples.push(rmb_core::animation::CamSample::of(&doc.scene, rec.start.elapsed().as_secs_f64()));
            ui.ctx().request_repaint();
        }
        let fast = self.navigating() || self.preview.as_ref().is_some_and(|p| p.1);
        let size = if fast {
            let s = prefs.navigate_percent.clamp(10, 100) as f32 / 100.0;
            (((full.0 as f32) * s).round().max(16.0) as u32, ((full.1 as f32) * s).round().max(16.0) as u32)
        } else {
            full
        };
        self.render_size = size;
        let preview = self.preview.take();
        let shown = preview.as_ref().map(|p| &p.0).unwrap_or(&doc.scene);
        let mut texture = if self.gpu.is_some() { self.update_gpu(frame, shown, size, fast, prefs) } else { self.update_cpu(ui.ctx(), shown, size) };
        if self.show_denoised
            && !fast
            && let Some(t) = self.update_denoised(ui.ctx(), shown, size, prefs)
        {
            texture = Some(t);
        }
        let camera = shown.camera.clone();
        let aspect = shown.aspect();
        self.preview = preview;
        if let Some(tex) = texture {
            ui.painter().image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        if prefs.keep_aspect {
            ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
        }
        self.draw_overlay(ui, rect, &camera, aspect);
        self.hud(ui, rect, doc);
        if !self.paused && (self.navigating() || self.compiling() || self.samples() < prefs.preview_samples) {
            ui.ctx().request_repaint();
        }
    }

    fn update_gpu(&mut self, frame: &eframe::Frame, scene: &Scene, size: (u32, u32), fast: bool, prefs: &Prefs) -> Option<egui::TextureId> {
        let gpu = self.gpu.as_mut()?;
        if self.timed.as_ref().is_none_or(|(s, z)| s != scene || *z != size) {
            self.timed = Some((scene.clone(), size));
            self.pass_start = Instant::now();
            self.converged_after = None;
        }
        gpu.set_scene(scene, size.0, size.1, fast);
        let max = if self.paused { 0 } else if fast { 1 } else { prefs.preview_samples.max(1) };
        let budget = if fast { Duration::from_millis(40) } else { Duration::from_millis(28) };
        gpu.work(budget, max);
        if gpu.samples() >= max && self.converged_after.is_none() && !fast {
            self.converged_after = Some(self.pass_start.elapsed());
        }
        gpu.display();
        let (view, generation) = (gpu.output_view()?.clone(), gpu.generation());
        let rs = frame.wgpu_render_state()?;
        let mut renderer = rs.renderer.write();
        match self.texture {
            Some((_, g)) if g == generation => {}
            Some((id, _)) => {
                renderer.update_egui_texture_from_wgpu_texture(&rs.device, &view, wgpu::FilterMode::Linear, id);
                self.texture = Some((id, generation));
            }
            None => {
                let id = renderer.register_native_texture(&rs.device, &view, wgpu::FilterMode::Linear);
                self.texture = Some((id, generation));
            }
        }
        self.texture.map(|t| t.0)
    }

    /// Denoised preview (GPU only): denoises at 1, 4, 16, 64, … samples and when the preview is
    /// done, on a worker thread. Returns the texture while it matches the current scene.
    fn update_denoised(&mut self, ctx: &egui::Context, scene: &Scene, size: (u32, u32), prefs: &Prefs) -> Option<egui::TextureId> {
        let gpu = self.gpu.as_mut()?;
        if let Some(job) = &self.denoise_job {
            match job.rx.try_recv() {
                Ok((rgba, outcome)) => {
                    let (w, h) = (job.key.1.0 as usize, job.key.1.1 as usize);
                    let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                    let key = job.key.clone();
                    match &mut self.denoised {
                        Some((t, k)) => {
                            t.set(ci, egui::TextureOptions::LINEAR);
                            *k = key;
                        }
                        None => self.denoised = Some((ctx.load_texture("denoised-preview", ci, egui::TextureOptions::LINEAR), key)),
                    }
                    self.denoise_status = Some(outcome.describe());
                    self.denoise_job = None;
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(50)),
                Err(mpsc::TryRecvError::Disconnected) => self.denoise_job = None,
            }
        }
        let n = gpu.samples();
        // The last checkpoint reached: a power of 4, or all samples of the preview.
        let checkpoint = if n >= prefs.preview_samples.max(1) { n } else if n == 0 { 0 } else { 1 << (2 * (n.ilog2() / 2)) };
        let have = self.denoised.as_ref().filter(|(_, (s, z, _))| s == scene && *z == size).map_or(0, |(_, (_, _, k))| *k);
        if checkpoint > 0
            && have < checkpoint
            && self.denoise_job.is_none()
            && !self.paused
            && gpu.output_size() == Some(size)
            && let Some(img) = gpu.read_hdr()
        {
            let (tx, rx) = mpsc::channel();
            let mut post = scene.post.clone();
            post.denoise = true;
            if post.denoise_blend <= 0.0 {
                post.denoise_blend = 1.0;
            }
            let c = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(img.finish(&post));
                c.request_repaint();
            });
            self.denoise_job = Some(DenoiseJob { key: (scene.clone(), size, n), rx });
        }
        self.denoised.as_ref().filter(|(_, (s, z, _))| s == scene && *z == size).map(|(t, _)| t.id())
    }

    /// CPU fallback: a worker thread renders passes and sends images.
    fn update_cpu(&mut self, ctx: &egui::Context, scene: &Scene, size: (u32, u32)) -> Option<egui::TextureId> {
        // The CPU is slow: render at most 480 pixels wide.
        let s = (480.0 / size.0 as f32).min(1.0);
        let size = (((size.0 as f32) * s).max(16.0) as u32, ((size.1 as f32) * s).max(16.0) as u32);
        let restart = self.cpu_job.as_ref().is_none_or(|j| j.scene != *scene || j.size != size);
        if restart {
            if let Some(j) = self.cpu_job.take() {
                j.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let (tx, rx) = mpsc::channel();
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (sc, st, c) = (scene.clone(), stop.clone(), ctx.clone());
            std::thread::spawn(move || {
                let mut r = CpuRenderer::new(sc, size.0, size.1);
                while !st.load(std::sync::atomic::Ordering::Relaxed) && r.samples() < 64 {
                    r.render_pass();
                    if tx.send((r.rgba8(), r.samples())).is_err() {
                        break;
                    }
                    c.request_repaint();
                }
            });
            self.cpu_job = Some(CpuJob { scene: scene.clone(), size, rx, stop });
            self.cpu_samples = 0;
        }
        if let Some(j) = &self.cpu_job {
            while let Ok((img, n)) = j.rx.try_recv() {
                let ci = egui::ColorImage::from_rgba_unmultiplied([size.0 as usize, size.1 as usize], &img);
                match &mut self.cpu_texture {
                    Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                    None => self.cpu_texture = Some(ctx.load_texture("cpu-preview", ci, egui::TextureOptions::LINEAR)),
                }
                self.cpu_samples = n;
            }
        }
        self.cpu_texture.as_ref().map(|t| t.id())
    }

    fn screen(rect: Rect, p: egui::Pos2) -> (f64, f64) {
        (((p.x - rect.left()) / rect.width() * 2.0 - 1.0) as f64, (1.0 - (p.y - rect.top()) / rect.height() * 2.0) as f64)
    }

    /// Surface point under `p`. Hits right at the camera (camera inside the set) do not count.
    fn pick_at(rect: Rect, p: egui::Pos2, scene: &Scene) -> Option<(f64, V3)> {
        let (sx, sy) = Self::screen(rect, p);
        pick(scene, sx, sy, rect.width().max(1.0) as u32, rect.height().max(1.0) as u32).filter(|(t, _)| *t > 1e-12)
    }

    /// Distance scale for navigation speeds: the distance estimate at the camera. Inside the
    /// set the estimate is 0; then the last positive value is used, so the camera can move out.
    fn nav_distance(&mut self, scene: &Scene) -> f64 {
        // Inside rendering: the estimate at the camera is a step size, not a distance; use the
        // distance to the surface ahead.
        let d = if scene.fractal.inside { pick(scene, 0.0, 0.0, 256, 256).map(|(t, _)| t * 0.5).unwrap_or(0.0) } else { distance_at_camera(scene) };
        if d.is_finite() && d > 1e-14 {
            self.nav_scale = d;
            d
        } else {
            self.nav_scale.max(1e-14)
        }
    }

    fn navigate(&mut self, ui: &Ui, resp: &egui::Response, rect: Rect, doc: &mut Document, prefs: &Prefs) {
        let before = doc.scene.camera.clone();
        self.navigate_inner(ui, resp, rect, doc, prefs);
        // A degenerate step (e.g. a zero-length direction) must never leave the camera unusable.
        let c = &doc.scene.camera;
        let finite = |v: V3| v.x.is_finite() && v.y.is_finite() && v.z.is_finite();
        if !(finite(c.position) && finite(c.forward) && finite(c.up) && finite(c.right)) {
            doc.scene.camera = before;
        }
    }

    fn navigate_inner(&mut self, ui: &Ui, resp: &egui::Response, rect: Rect, doc: &mut Document, prefs: &Prefs) {
        let scene = &mut doc.scene;
        let speed = prefs.mouse_speed * 0.005;
        let inv = if prefs.invert_y { -1.0 } else { 1.0 };
        self.hover = resp.hover_pos().map(|p| Self::screen(rect, p));

        // Double-click: look at the point and focus there.
        if resp.double_clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some((t, hit)) = Self::pick_at(rect, p, scene)
        {
            let up = scene.camera.up;
            scene.camera.look_at(hit, up);
            scene.dof.focus_distance = t;
            self.pivot = Some(hit);
            self.nav();
            return;
        }

        let press = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started_by(PointerButton::Primary) && self.mode == NavMode::Orbit {
            let picked = press.and_then(|p| Self::pick_at(rect, p, scene)).map(|(_, h)| h);
            self.pivot = match picked.or(self.pivot) {
                Some(p) => Some(p),
                None => {
                    let d = self.nav_distance(scene);
                    Some(scene.camera.position + scene.camera.forward * (d * 2.0))
                }
            };
        }
        let delta = resp.drag_delta();
        if delta != egui::Vec2::ZERO {
            let (dx, dy) = (delta.x as f64, delta.y as f64 * inv);
            let shift = ui.input(|i| i.modifiers.shift);
            if resp.dragged_by(PointerButton::Primary) && self.mode == NavMode::Orbit && !shift {
                if let Some(pv) = self.pivot {
                    let up = scene.camera.up;
                    scene.camera.orbit(pv, up, dx * speed, -dy * speed);
                }
            } else if resp.dragged_by(PointerButton::Primary) || resp.dragged_by(PointerButton::Middle) {
                // Look around (rotate in place); with Shift in orbit mode as well.
                scene.camera.rotate_local(dx * speed * 0.5, -dy * speed * 0.5, 0.0);
            } else if resp.dragged_by(PointerButton::Secondary) {
                let dist = self.pivot.map(|p| (p - scene.camera.position).length()).unwrap_or_else(|| self.nav_distance(scene) * 2.0);
                let per_px = 2.0 * scene.camera.tan_half_fov() * dist / rect.height() as f64;
                let c = &mut scene.camera;
                c.position = c.position - c.right * (dx * per_px) + c.up * (dy * per_px * inv);
                if let Some(p) = self.pivot.as_mut() {
                    *p = *p - c.right * (dx * per_px) + c.up * (dy * per_px * inv);
                }
            }
            self.nav();
        }

        // Wheel
        let scroll = if resp.hovered() { ui.input(|i| i.smooth_scroll_delta.y) as f64 } else { 0.0 };
        if scroll != 0.0 {
            match self.mode {
                NavMode::Orbit => {
                    let f = (scroll / 50.0).clamp(-1.0, 1.0) * 0.15;
                    let target = resp.hover_pos().and_then(|p| Self::pick_at(rect, p, scene));
                    match target {
                        Some((t, hit)) => {
                            let dir = (hit - scene.camera.position).normalize();
                            scene.camera.position += dir * (t * f);
                        }
                        None => {
                            let d = self.nav_distance(scene);
                            scene.camera.position += scene.camera.forward * (d * f);
                        }
                    }
                }
                NavMode::Fly => {
                    self.fly_speed = (self.fly_speed * (1.0 + scroll / 400.0)).clamp(0.01, 100.0);
                }
            }
            self.nav();
        }

        // Keyboard focus: the view takes it when clicked, or when hovered while nothing else
        // has it; Tab and the arrows then stay with the view instead of moving the focus.
        if resp.hovered() && (ui.input(|i| i.pointer.any_pressed()) || ui.memory(|m| m.focused().is_none())) {
            resp.request_focus();
        }
        if resp.has_focus() {
            ui.memory_mut(|m| m.set_focus_lock_filter(resp.id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false }));
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab)) {
                self.mode = if self.mode == NavMode::Orbit { NavMode::Fly } else { NavMode::Orbit };
            }
        }
        if resp.has_focus() {
            let dt = ui.input(|i| i.stable_dt).min(0.1) as f64;
            let (fwd, side, upd, roll, yaw, pitch) = ui.input(|i| {
                let k = |a: Key, b: Key| (i.key_down(a) as i32 - i.key_down(b) as i32) as f64;
                (k(Key::W, Key::S), k(Key::D, Key::A), k(Key::R, Key::F), k(Key::E, Key::Q), k(Key::ArrowRight, Key::ArrowLeft), k(Key::ArrowUp, Key::ArrowDown))
            });
            let fast = ui.input(|i| i.modifiers.shift);
            if fwd != 0.0 || side != 0.0 || upd != 0.0 {
                let d = self.nav_distance(scene);
                let v = d * 1.5 * self.fly_speed * if fast { 4.0 } else { 1.0 } * dt;
                let c = &mut scene.camera;
                c.position = c.position + c.forward * (fwd * v) + c.right * (side * v) + c.up * (upd * v);
                self.nav();
            }
            if roll != 0.0 || yaw != 0.0 || pitch != 0.0 {
                scene.camera.rotate_local(yaw * dt * 0.8, pitch * dt * 0.8, roll * dt * 1.2);
                self.nav();
            }
        }
    }

    /// Draws the animation's camera path, its keyframes and the current frame.
    fn draw_overlay(&self, ui: &Ui, rect: Rect, cam: &rmb_core::scene::Camera, aspect: f64) {
        let Some(o) = &self.overlay else { return };
        let t = cam.tan_half_fov();
        let project = |p: V3| -> Option<egui::Pos2> {
            let d = p - cam.position;
            let z = d.dot(cam.forward);
            if z <= 1e-9 {
                return None;
            }
            let sx = d.dot(cam.right) / (z * t * aspect);
            let sy = d.dot(cam.up) / (z * t);
            (sx.abs() < 4.0 && sy.abs() < 4.0).then(|| pos2(rect.center().x + sx as f32 * rect.width() * 0.5, rect.center().y - sy as f32 * rect.height() * 0.5))
        };
        let painter = ui.painter().with_clip_rect(rect);
        let line = egui::Stroke::new(2.0, Color32::from_rgba_unmultiplied(255, 200, 80, 200));
        let mut run: Vec<egui::Pos2> = Vec::new();
        for p in &o.path {
            match project(*p) {
                Some(s) => run.push(s),
                None => {
                    if run.len() > 1 {
                        painter.add(egui::Shape::line(std::mem::take(&mut run), line));
                    }
                    run.clear();
                }
            }
        }
        if run.len() > 1 {
            painter.add(egui::Shape::line(run, line));
        }
        for (i, k) in o.keys.iter().enumerate() {
            if let Some(s) = project(*k) {
                let sel = o.selected == Some(i);
                let r = if sel { 6.0 } else { 4.5 };
                let pts = vec![s + vec2(0.0, -r), s + vec2(r, 0.0), s + vec2(0.0, r), s + vec2(-r, 0.0)];
                let fill = if sel { theme::ACCENT } else { Color32::from_rgb(255, 200, 80) };
                painter.add(egui::Shape::convex_polygon(pts, fill, egui::Stroke::new(1.0, Color32::BLACK)));
            }
        }
        if let Some(s) = o.current.and_then(project) {
            painter.circle(s, 5.0, Color32::WHITE, egui::Stroke::new(1.5, Color32::BLACK));
        }
    }

    fn hud(&self, ui: &Ui, rect: Rect, doc: &Document) {
        let p = ui.painter();
        let mut y = rect.top() + 10.0;
        let font = egui::FontId::proportional(theme::fs(12.0));
        let pill = |text: String, color: Color32, y: f32| {
            let galley = ui.painter().layout_no_wrap(text, font.clone(), color);
            let r = Rect::from_min_size(pos2(rect.left() + 10.0, y), galley.size() + vec2(16.0, 8.0));
            p.rect_filled(r, 6.0, Color32::from_black_alpha(150));
            p.galley(r.min + vec2(8.0, 4.0), galley, color);
            r.height() + 6.0
        };
        if let Some(rec) = &self.recording {
            y += pill(format!("●  Recording flight  {:.1} s  ·  stop in the timeline", rec.start.elapsed().as_secs_f64()), Color32::from_rgb(255, 90, 80), y);
        }
        if let Some((_, _, label)) = &self.preview {
            y += pill(format!("{}  {label}  ·  navigation is off; press Edit in the timeline or click a keyframe", crate::icons::CAMERA), theme::ACCENT, y);
        }
        if self.compiling() {
            y += pill(format!("{}  Compiling shader…", crate::icons::GEAR), theme::TEXT, y);
        }
        if let Some(e) = self.error() {
            y += pill(format!("{}  {}", crate::icons::WARNING, e.lines().next().unwrap_or("")), theme::WARN, y);
        }
        let prog_missing: Vec<&str> = doc.scene.fractal.slots.iter().filter(|s| s.iterations > 0 && rmb_core::formula::find(&s.formula).is_none()).map(|s| s.formula.as_str()).collect();
        if !prog_missing.is_empty() {
            y += pill(format!("{}  Formulas not available: {}", crate::icons::WARNING, prog_missing.join(", ")), theme::WARN, y);
        }
        if !doc.scene.fractal.inside && distance_at_camera(&doc.scene) <= 0.0 && !doc.scene.fractal.slots.is_empty() {
            y += pill(
                format!("{}  The camera is inside the fractal: move out (fly backward, wheel), use Camera › Reset view, or turn on Inside rendering", crate::icons::INFO),
                theme::WARN,
                y,
            );
        }
        if let Some(e) = &self.gpu_error {
            pill(format!("{}  {e}", crate::icons::INFO), theme::TEXT_MUTED, y);
        }
        if self.mode == NavMode::Fly {
            let t = format!("{}  Fly  ·  WASD move · R/F up/down · Q/E roll · drag to look · wheel: speed ×{:.2}", crate::icons::FLY, self.fly_speed);
            let galley = ui.painter().layout_no_wrap(t, font.clone(), theme::TEXT);
            let r = Rect::from_min_size(pos2(rect.left() + 10.0, rect.bottom() - galley.size().y - 18.0), galley.size() + vec2(16.0, 8.0));
            p.rect_filled(r, 6.0, Color32::from_black_alpha(150));
            p.galley(r.min + vec2(8.0, 4.0), galley, theme::TEXT);
        }
    }
}
