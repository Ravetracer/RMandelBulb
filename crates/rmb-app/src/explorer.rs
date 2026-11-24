// SPDX-License-Identifier: LGPL-2.1-or-later

//! The explore lane at the bottom: presets to start from, random scenes and mutations of the
//! current scene, as thumbnails rendered one after another on a background thread (GPU, or
//! CPU without one). A click on a thumbnail takes its scene (one undo step).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, CornerRadius, RichText, Sense, Stroke, vec2};
use rmb_core::Scene;
use rmb_core::explore::{Mutation, RandomKind};
use rmb_gpu::{GpuContext, GpuRenderer};

use crate::{icons, theme};

const THUMB_W: u32 = 192;
const THUMB_H: u32 = 108;
const THUMB_SAMPLES: u32 = 6;
/// Most thumbnails a strip keeps.
const MAX_THUMBS: usize = 96;
/// Random scenes and mutations noisier than this (`explore::thumb_quality`) are made again
/// with another seed (dust and grain; measured on random batches).
const MAX_NOISE: f64 = 0.22;
/// Tries per thumbnail with the filter on.
const FILTER_TRIES: u64 = 4;

/// The presets: (name, scene).
const PRESETS: &[(&str, &str)] = &[
    ("Mandelbulb", include_str!("../presets/mandelbulb.rmb")),
    ("Amazing Box", include_str!("../presets/amazing-box.rmb")),
    ("Menger sponge", include_str!("../presets/menger.rmb")),
    ("Kaleidoscopic IFS", include_str!("../presets/kifs.rmb")),
    ("Quaternion Julia", include_str!("../presets/quaternion-julia.rmb")),
    ("Neon box city", include_str!("../presets/neon-box-city.rmb")),
    ("Glass bulb", include_str!("../presets/glass-bulb.rmb")),
    ("Hybrid", include_str!("../presets/hybrid.rmb")),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Presets,
    Random,
    Mutations,
    Snapshots,
}

impl Mode {
    fn index(self) -> usize {
        match self {
            Mode::Presets => 0,
            Mode::Random => 1,
            Mode::Mutations => 2,
            Mode::Snapshots => 3,
        }
    }
}

/// What a thumbnail job makes.
#[derive(Clone)]
enum Source {
    Scene(Arc<Scene>),
    Random(RandomKind, u64),
    Mutation(Arc<Scene>, Mutation, f64, u64),
}

struct Job {
    key: u64,
    /// Remakes noisy random scenes and mutations (`MAX_NOISE`).
    filter: bool,
    /// The strip's batch number when the job was made (stale jobs are skipped).
    batch: u64,
    strip: usize,
    source: Source,
}

struct Thumb {
    key: u64,
    name: String,
    done: Option<(Scene, egui::TextureHandle)>,
}

pub struct Explorer {
    pub open: bool,
    mode: Mode,
    kind: RandomKind,
    pub batch_size: usize,
    /// Quality filter: noisy random scenes and mutations are made again.
    pub filter: bool,
    mutation: Mutation,
    strength: f64,
    strips: [Vec<Thumb>; 4],
    batches: Arc<[AtomicU64; 4]>,
    next_key: u64,
    seed: u64,
    jobs: Option<Sender<Job>>,
    results: Option<Receiver<(u64, Scene, Vec<u8>)>>,
    /// Height of the header row(s) in the last frame (it wraps in narrow windows).
    header_h: f32,
}

impl Default for Explorer {
    fn default() -> Self {
        Explorer {
            open: false,
            mode: Mode::Presets,
            kind: RandomKind::Any,
            batch_size: 12,
            filter: true,
            mutation: Mutation::All,
            strength: 0.5,
            strips: Default::default(),
            batches: Arc::new(Default::default()),
            next_key: 1,
            seed: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64),
            jobs: None,
            results: None,
            header_h: 24.0,
        }
    }
}

/// The scene of a thumbnail job (random scenes are framed on the way); `attempt` > 0 varies
/// the seed (quality filter).
fn scene_of(source: &Source, attempt: u64) -> Scene {
    let vary = |seed: u64| seed ^ attempt.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    match source {
        Source::Scene(s) => (**s).clone(),
        Source::Random(kind, seed) => rmb_core::explore::random_framed(*kind, vary(*seed), 6),
        Source::Mutation(base, what, strength, seed) => rmb_core::explore::mutate(base, *what, *strength, vary(*seed)),
    }
}

/// Renders a thumbnail (RGBA8) and its noise (`explore::thumb_quality`; 0 on the CPU).
fn render(r: &mut Option<GpuRenderer>, scene: &Scene) -> (Vec<u8>, f64) {
    let mut s = scene.clone();
    // Thumbnails: no denoiser, no flares. The surface threshold is in pixels: scaled to the
    // scene's image height, the rays stop where they do in the full image (the iteration
    // count there picks the colors, which would shift at the thumbnail's coarser threshold).
    s.march.detail *= THUMB_H as f64 / s.height.max(1) as f64;
    s.post.denoise = false;
    s.lighting.lights.iter_mut().for_each(|l| l.flare.enabled = false);
    rmb_core::compose::load_models(&s);
    if let Some(r) = r.as_mut() {
        r.set_scene(&s, THUMB_W, THUMB_H, false);
        if r.wait_for_pipeline().is_ok() {
            // Read back at half the samples too: the change shows dust and grain.
            let mut half = None;
            for target in [THUMB_SAMPLES / 2, THUMB_SAMPLES] {
                while r.samples() < target {
                    if !r.work(std::time::Duration::from_millis(200), target) && r.last_error().is_some() {
                        break;
                    }
                }
                if target < THUMB_SAMPLES {
                    half = r.read_rgba8();
                }
            }
            if let Some(img) = r.read_rgba8() {
                let noise = half.map_or(0.0, |h| rmb_core::explore::thumb_quality(&h, &img).1);
                return (img, noise);
            }
        }
    }
    let mut cpu = rmb_core::render::cpu::CpuRenderer::new(s, THUMB_W, THUMB_H);
    cpu.render_pass();
    (cpu.rgba8(), 0.0)
}

impl Explorer {
    /// Starts the background worker (once).
    fn worker(&mut self, ctx: &egui::Context, gpu: Option<GpuContext>) -> Sender<Job> {
        if let Some(tx) = &self.jobs {
            return tx.clone();
        }
        let (tx, rx) = channel::<Job>();
        let (rtx, rrx) = channel();
        let batches = self.batches.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            // One renderer for all thumbnails: formulas seen before reuse their shader.
            let mut r = gpu.map(GpuRenderer::new);
            while let Ok(job) = rx.recv() {
                if batches[job.strip].load(Ordering::Relaxed) != job.batch {
                    continue;
                }
                // The quality filter remakes noisy random scenes and mutations; the least noisy
                // try stays.
                let tries = if job.filter && !matches!(job.source, Source::Scene(_)) { FILTER_TRIES } else { 1 };
                let mut best: Option<(Scene, Vec<u8>, f64)> = None;
                for attempt in 0..tries {
                    if batches[job.strip].load(Ordering::Relaxed) != job.batch {
                        break;
                    }
                    let scene = scene_of(&job.source, attempt);
                    let (img, noise) = render(&mut r, &scene);
                    if best.as_ref().is_none_or(|b| noise < b.2) {
                        best = Some((scene, img, noise));
                    }
                    if noise <= MAX_NOISE {
                        break;
                    }
                }
                let Some((scene, img, _)) = best else { continue };
                if rtx.send((job.key, scene, img)).is_err() {
                    return;
                }
                repaint.request_repaint();
            }
        });
        self.jobs = Some(tx.clone());
        self.results = Some(rrx);
        tx
    }

    /// Queues thumbnails for `strip` (replacing it).
    fn start(&mut self, ctx: &egui::Context, gpu: Option<GpuContext>, strip: usize, items: Vec<(String, Source)>) {
        let tx = self.worker(ctx, gpu);
        let batch = self.batches[strip].fetch_add(1, Ordering::Relaxed) + 1;
        let mut list = Vec::new();
        for (name, source) in items.into_iter().take(MAX_THUMBS) {
            let key = self.next_key;
            self.next_key += 1;
            list.push(Thumb { key, name, done: None });
            let _ = tx.send(Job { key, filter: self.filter, batch, strip, source });
        }
        self.strips[strip] = list;
    }

    /// Adds the current scene to the snapshots (rendered like the other thumbnails).
    fn snapshot(&mut self, ctx: &egui::Context, gpu: Option<GpuContext>, current: &Scene) {
        let tx = self.worker(ctx, gpu);
        let strip = Mode::Snapshots.index();
        let batch = self.batches[strip].load(Ordering::Relaxed);
        let key = self.next_key;
        self.next_key += 1;
        let n = self.strips[strip].len() + 1;
        self.strips[strip].push(Thumb { key, name: format!("Snapshot {n}"), done: None });
        let _ = tx.send(Job { key, filter: false, batch, strip, source: Source::Scene(Arc::new(current.clone())) });
    }

    fn next_seed(&mut self) -> u64 {
        self.seed = rmb_core::math::pcg(self.seed as u32) as u64 ^ self.seed.rotate_left(21).wrapping_add(0x9e37_79b9);
        self.seed
    }

    fn fill(&mut self, ctx: &egui::Context, gpu: Option<GpuContext>, current: &Scene) {
        let items: Vec<(String, Source)> = match self.mode {
            Mode::Presets => PRESETS.iter().filter_map(|(name, json)| Scene::from_json(json).ok().map(|s| (name.to_string(), Source::Scene(Arc::new(s))))).collect(),
            Mode::Random => (0..self.batch_size).map(|_| ("Random".to_string(), Source::Random(self.kind, self.next_seed()))).collect(),
            Mode::Mutations => {
                let base = Arc::new(current.clone());
                (0..self.batch_size).map(|_| ("Mutation".to_string(), Source::Mutation(base.clone(), self.mutation, self.strength, self.next_seed()))).collect()
            }
            Mode::Snapshots => return,
        };
        self.start(ctx, gpu, self.mode.index(), items);
    }

    fn busy(&self, strip: usize) -> bool {
        self.strips[strip].iter().any(|t| t.done.is_none())
    }

    fn cancel(&mut self, strip: usize) {
        self.batches[strip].fetch_add(1, Ordering::Relaxed);
        self.strips[strip].retain(|t| t.done.is_some());
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.results else { return };
        while let Ok((key, scene, img)) = rx.try_recv() {
            let Some(t) = self.strips.iter_mut().flatten().find(|t| t.key == key) else { continue };
            let color = egui::ColorImage::from_rgba_unmultiplied([THUMB_W as usize, THUMB_H as usize], &img);
            let tex = ctx.load_texture(format!("explore-{key}"), color, egui::TextureOptions::LINEAR);
            t.done = Some((scene, tex));
        }
    }

    /// Height the lane needs (thumbnails, scroll bar, margins and the header as last laid out).
    pub fn lane_height(&self) -> f32 {
        self.header_h.max(20.0) + THUMB_H as f32 * 0.85 + 34.0
    }

    /// Shows the lane; returns a scene the user picked.
    pub fn show(&mut self, ui: &mut egui::Ui, current: &Scene, gpu: Option<GpuContext>) -> Option<Scene> {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        let strip = self.mode.index();
        if self.mode == Mode::Presets && self.strips[0].is_empty() {
            self.fill(&ctx, gpu.clone(), current);
        }
        let mut picked = None;
        let header = ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("EXPLORE").size(theme::fs(10.5)).color(theme::TEXT_MUTED).strong());
            ui.add_space(8.0);
            ui.selectable_value(&mut self.mode, Mode::Presets, "Presets").on_hover_text("Scenes to start from");
            ui.selectable_value(&mut self.mode, Mode::Random, "Random").on_hover_text("Batches of random fractals with random colors");
            ui.selectable_value(&mut self.mode, Mode::Mutations, "Mutations").on_hover_text("Variants of the current scene");
            let n = self.strips[Mode::Snapshots.index()].len();
            let label = if n > 0 { format!("Snapshots ({n})") } else { "Snapshots".to_string() };
            ui.selectable_value(&mut self.mode, Mode::Snapshots, label).on_hover_text("Scenes you kept with Snapshot (for this session): go back to one by clicking it");
            if ui.button(icons::CAMERA).on_hover_text("Snapshot: keeps the current scene in the Snapshots strip, to come back to it after trying mutations or edits (for this session)").clicked() {
                self.snapshot(&ctx, gpu.clone(), current);
            }
            ui.separator();
            let busy = self.busy(self.mode.index());
            let start = match self.mode {
                Mode::Presets => false,
                Mode::Snapshots => {
                    if n > 0 && ui.button(format!("{}  Clear", icons::TRASH)).on_hover_text("Removes all snapshots").clicked() {
                        self.cancel(Mode::Snapshots.index());
                        self.strips[Mode::Snapshots.index()].clear();
                    }
                    false
                }
                Mode::Random => {
                    egui::ComboBox::from_id_salt("explore-kind").selected_text(self.kind.label()).show_ui(ui, |ui| {
                        for k in RandomKind::ALL {
                            ui.selectable_value(&mut self.kind, k, k.label());
                        }
                    });
                    ui.checkbox(&mut self.filter, "Filter").on_hover_text("Makes dusty, grainy or washed-out scenes again with another seed (up to four tries each; slower batches)");
                    ui.label(RichText::new("Count").color(theme::TEXT_MUTED));
                    ui.add(egui::DragValue::new(&mut self.batch_size).range(1..=48)).on_hover_text("Random scenes per batch");
                    batch_button(ui, busy, &format!("{}  New batch", icons::REFRESH), "Random fractals of the chosen kind, framed by the camera; click one to take it")
                }
                Mode::Mutations => {
                    egui::ComboBox::from_id_salt("explore-mutation").selected_text(self.mutation.label()).show_ui(ui, |ui| {
                        for m in Mutation::ALL {
                            ui.selectable_value(&mut self.mutation, m, m.label());
                        }
                    });
                    ui.label(RichText::new("Count").color(theme::TEXT_MUTED));
                    ui.add(egui::DragValue::new(&mut self.batch_size).range(1..=48)).on_hover_text("Variants per click");
                    ui.label(RichText::new("Strength").color(theme::TEXT_MUTED));
                    ui.add(egui::DragValue::new(&mut self.strength).speed(0.01).range(0.0..=1.0)).on_hover_text("How far the variants go from the current scene");
                    ui.checkbox(&mut self.filter, "Filter").on_hover_text("Makes dusty, grainy or washed-out variants again with another seed (up to four tries each; slower batches)");
                    batch_button(ui, busy, &format!("{}  Mutate", icons::REFRESH), "Variants of the current scene; click one to take it")
                }
            };
            if start && busy {
                self.cancel(self.mode.index());
            } else if start {
                self.fill(&ctx, gpu.clone(), current);
            }
            if self.busy(self.mode.index()) {
                ui.spinner();
            }
        });
        self.header_h = header.response.rect.height();
        ui.add_space(4.0);
        let size = vec2(THUMB_W as f32, THUMB_H as f32) * 0.85;
        let list = &self.strips[strip];
        if list.is_empty() {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), size.y), Sense::hover());
            let text = match self.mode {
                Mode::Presets => "Loading presets…",
                Mode::Random => "Empty: start a new batch of random fractals.",
                Mode::Mutations => "Mutate makes variants of the current scene.",
                Mode::Snapshots => "Snapshot keeps the current scene here.",
            };
            ui.painter().text(rect.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(theme::fs(13.0)), theme::TEXT_MUTED);
            return None;
        }
        ui.style_mut().always_scroll_the_only_direction = true;
        egui::ScrollArea::horizontal().id_salt(("explore-strip", strip)).show(ui, |ui| {
            ui.horizontal(|ui| {
                for t in list {
                    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                    let p = ui.painter();
                    p.rect_filled(rect, CornerRadius::same(6), theme::BG);
                    match &t.done {
                        Some((_, tex)) => {
                            p.image(tex.id(), rect.shrink(1.0), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        None => {
                            p.text(rect.center(), egui::Align2::CENTER_CENTER, "…", egui::FontId::proportional(theme::fs(18.0)), theme::TEXT_MUTED);
                        }
                    }
                    if matches!(self.mode, Mode::Presets | Mode::Snapshots) {
                        let bar = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 18.0), rect.max);
                        p.rect_filled(bar, CornerRadius::same(4), Color32::from_black_alpha(150));
                        p.text(bar.left_center() + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, &t.name, egui::FontId::proportional(theme::fs(11.0)), theme::TEXT);
                    }
                    let stroke = if resp.hovered() { Stroke::new(2.0, theme::ACCENT) } else { Stroke::new(1.0, theme::BORDER) };
                    p.rect_stroke(rect, CornerRadius::same(6), stroke, egui::StrokeKind::Inside);
                    if let Some((scene, _)) = &t.done
                        && resp.on_hover_text(format!("{}\n{}\nClick to take this scene", t.name, formula_list(scene))).clicked()
                    {
                        picked = Some(scene.clone());
                    }
                }
            });
        });
        if self.busy(strip) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        picked
    }
}

/// The formulas of a scene, for the hover text.
fn formula_list(scene: &Scene) -> String {
    scene.fractal.slots.iter().map(|s| s.formula.as_str()).collect::<Vec<_>>().join(" + ")
}

/// The button that starts a batch; "Cancel" while the strip's batch runs. True when clicked.
fn batch_button(ui: &mut egui::Ui, busy: bool, label: &str, tip: &str) -> bool {
    if busy {
        ui.button(format!("{}  Cancel", icons::X)).on_hover_text("Stops making thumbnails; finished ones stay").clicked()
    } else {
        ui.button(label).on_hover_text(tip).clicked()
    }
}
