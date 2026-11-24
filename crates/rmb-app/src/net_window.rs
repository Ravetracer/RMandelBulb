// SPDX-License-Identifier: LGPL-2.1-or-later

//! Network render window: lists the render clients found on the network (`rmb-cli --network`
//! on other machines), lets the user pick them and renders the scene's animation or a still
//! image on them, with progress per client and for the whole job.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use egui::RichText;
use rmb_core::Scene;
use rmb_core::animation::AnimationPlan;
use rmb_core::io::image::ImageKind;
use rmb_core::video::VideoFormat;
use rmb_net::discovery::Discovery;
use rmb_net::proto::JobKind;
use rmb_net::server::{JobRequest, Output, Status, Target};

use crate::{theme, widgets};

struct Running {
    status: Arc<Mutex<Status>>,
    cancel: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<Result<String, String>>,
}

pub struct NetWindow {
    pub open: bool,
    discovery: Option<Result<Discovery, String>>,
    /// Chosen clients by id (announcement) or by address (added by hand).
    chosen: std::collections::HashSet<u64>,
    /// Clients seen while the window was open: new ones start chosen.
    known: std::collections::HashSet<u64>,
    manual: Vec<String>,
    manual_input: String,
    local: bool,
    key: String,
    animation: bool,
    width: u32,
    height: u32,
    pub samples: u32,
    from: usize,
    to: usize,
    format: VideoFormat,
    movie_path: Option<PathBuf>,
    image_path: Option<PathBuf>,
    image_kind: ImageKind,
    keep_frames: bool,
    doc_dir: Option<PathBuf>,
    running: Option<Running>,
    /// The local client (started once, kept for later jobs).
    local_target: Option<Target>,
    message: Option<String>,
}

impl NetWindow {
    pub fn new(samples: u32) -> Self {
        NetWindow {
            open: false,
            discovery: None,
            chosen: Default::default(),
            known: Default::default(),
            manual: Vec::new(),
            manual_input: String::new(),
            local: false,
            key: String::new(),
            animation: true,
            width: 0,
            height: 0,
            samples,
            from: 0,
            to: usize::MAX,
            format: VideoFormat::Mp4,
            movie_path: None,
            image_path: None,
            image_kind: ImageKind::Png8,
            keep_frames: false,
            doc_dir: None,
            running: None,
            local_target: None,
            message: None,
        }
    }

    /// A job runs on this computer's graphics card too (the preview pauses).
    pub fn local_busy(&self) -> bool {
        self.running.is_some() && self.local
    }

    pub fn open_for(&mut self, scene: &Scene, doc_path: Option<&PathBuf>) {
        self.open = true;
        if self.discovery.is_none() {
            self.discovery = Some(Discovery::start());
        }
        self.doc_dir = doc_path.and_then(|p| p.parent()).map(Path::to_path_buf);
        if self.running.is_none() {
            self.width = scene.width;
            self.height = scene.height;
        }
        if self.movie_path.is_none() {
            self.movie_path = doc_path.map(|p| p.with_extension(self.format.extension().unwrap_or("")));
        }
        if self.image_path.is_none() {
            self.image_path = doc_path.map(|p| p.with_extension(self.image_kind.extension()));
        }
    }

    /// Shows the window; returns a status message when a job ended.
    pub fn show(&mut self, ctx: &egui::Context, scene: &Scene, ffmpeg: Option<&Path>) -> Option<String> {
        let mut finished = None;
        if self.running.as_ref().is_some_and(|r| r.handle.is_finished())
            && let Some(r) = self.running.take()
        {
            let msg = match r.handle.join() {
                Ok(Ok(s)) => format!("Network render saved {s}"),
                Ok(Err(e)) if e == "cancelled" => "Network render cancelled (finished frames stay in the output folder)".to_string(),
                Ok(Err(e)) => format!("Network render failed: {e}"),
                Err(_) => "Network render failed".to_string(),
            };
            self.message = Some(msg.clone());
            finished = Some(msg);
        }
        if self.running.is_some() || self.open {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if !self.open {
            // Stop listening when the window closes (and nothing runs).
            if self.running.is_none() {
                self.discovery = None;
            }
            return finished;
        }
        let anim = scene.animation.clone().filter(|a| !a.keyframes.is_empty());
        if anim.is_none() {
            self.animation = false;
        }
        let program = rmb_core::video::ffmpeg_program(ffmpeg);
        let mut open = self.open;
        egui::Window::new("Network render").id(egui::Id::new("net-render")).open(&mut open).resizable(true).collapsible(false).default_width(520.0).show(ctx, |ui| {
            if let Some(r) = &self.running {
                let s = r.status.lock().map(|s| s.clone()).unwrap_or_default();
                self.progress_ui(ui, &s);
                if ui.button("Cancel").on_hover_text("Stops the job; finished frames stay in the output folder").clicked() {
                    r.cancel.store(true, Ordering::Relaxed);
                }
                return;
            }
            self.clients_ui(ui);
            ui.separator();
            self.job_ui(ui, scene, anim.as_ref(), &program);
            ui.add_space(6.0);
            let targets = self.targets();
            let output_ok = if self.animation { self.movie_path.is_some() } else { self.image_path.is_some() };
            let movie_ok = !self.animation || !self.format.needs_ffmpeg() || rmb_core::video::ffmpeg_available(&program);
            let ready = !targets.is_empty() && output_ok && movie_ok;
            ui.horizontal(|ui| {
                if ui.add_enabled(ready, egui::Button::new(RichText::new(format!("{}  Render on {} client{}", crate::icons::EXPORT, targets.len(), if targets.len() == 1 { "" } else { "s" })).strong()).fill(theme::ACCENT_DIM)).clicked() {
                    self.start(scene, anim.as_ref(), targets.clone(), program.clone());
                }
                let why = if targets.is_empty() {
                    "Choose at least one client"
                } else if !output_ok {
                    "Choose where to save first"
                } else if !movie_ok {
                    "ffmpeg not found (Preferences › Movies)"
                } else {
                    ""
                };
                ui.label(RichText::new(why).color(theme::TEXT_MUTED));
            });
            if let Some(m) = &self.message {
                ui.label(RichText::new(m).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            }
        });
        self.open = open;
        finished
    }

    fn clients_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Start `rmb-cli --network` on every computer that should help; they show up here by themselves.").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
        let seen = match &self.discovery {
            Some(Ok(d)) => d.clients(),
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(theme::WARN));
                Vec::new()
            }
            None => Vec::new(),
        };
        for s in &seen {
            if self.known.insert(s.info.id) && !s.info.busy && s.info.version == rmb_net::version() {
                self.chosen.insert(s.info.id);
            }
        }
        let usable = seen.iter().filter(|s| !s.info.busy && s.info.version == rmb_net::version()).count();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} client{} found, {usable} ready", seen.len(), if seen.len() == 1 { "" } else { "s" })).strong());
            if ui.small_button("All").on_hover_text("Choose all ready clients").clicked() {
                self.chosen.extend(seen.iter().filter(|s| !s.info.busy).map(|s| s.info.id));
            }
            if ui.small_button("None").clicked() {
                self.chosen.clear();
            }
        });
        egui::Grid::new("net-clients").num_columns(3).spacing([10.0, 4.0]).striped(true).show(ui, |ui| {
            for s in &seen {
                let mut on = self.chosen.contains(&s.info.id);
                let wrong = s.info.version != rmb_net::version();
                let ok = !s.info.busy && !wrong;
                if ui.add_enabled(ok, egui::Checkbox::new(&mut on, &s.info.name)).on_hover_text(format!("{} at {}", s.info.name, s.addr)).changed() {
                    if on {
                        self.chosen.insert(s.info.id);
                    } else {
                        self.chosen.remove(&s.info.id);
                    }
                }
                ui.label(RichText::new(s.info.specs.summary()).size(theme::fs(11.5)));
                let state = if wrong {
                    format!("version {}", s.info.version)
                } else if s.info.busy {
                    "busy".into()
                } else if s.info.key {
                    "ready (needs the key)".into()
                } else {
                    "ready".into()
                };
                ui.label(RichText::new(state).size(theme::fs(11.5)).color(if ok { theme::TEXT_MUTED } else { theme::WARN }));
                ui.end_row();
            }
            let mut remove = None;
            for (k, a) in self.manual.iter().enumerate() {
                ui.label(a);
                ui.label(RichText::new("added by address").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
                if ui.small_button("Remove").clicked() {
                    remove = Some(k);
                }
                ui.end_row();
            }
            if let Some(k) = remove {
                self.manual.remove(k);
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.manual_input).hint_text("host or host:port").desired_width(160.0)).on_hover_text("A client the announcements do not reach (another subnet, blocked broadcasts)");
            if ui.button("Add").clicked() && !self.manual_input.trim().is_empty() {
                match rmb_net::discovery::parse_address(self.manual_input.trim()) {
                    Ok(_) => {
                        self.manual.push(self.manual_input.trim().to_string());
                        self.manual_input.clear();
                    }
                    Err(e) => self.message = Some(e),
                }
            }
        });
        widgets::grid(ui, "net-opts", |ui| {
            widgets::checkbox(ui, "This computer too", "Renders on this computer's graphics card as well, like another client", &mut self.local);
            widgets::label(ui, "Key", "Shared key: clients started with --key only work for a server that knows it");
            ui.add(egui::TextEdit::singleline(&mut self.key).password(true).desired_width(160.0));
            ui.end_row();
        });
    }

    fn job_ui(&mut self, ui: &mut egui::Ui, scene: &Scene, anim: Option<&rmb_core::animation::Animation>, program: &Path) {
        if anim.is_some() {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.animation, true, "Animation").on_hover_text("The keyframe animation: every client renders whole frames");
                ui.selectable_value(&mut self.animation, false, "Image").on_hover_text("The current view as one image: every client renders a part of its samples");
            });
        }
        let aspect = scene.aspect();
        widgets::grid(ui, "net-job", |ui| {
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
            widgets::slider_u32(ui, "Samples", "Samples per pixel", &mut self.samples, 1..=4096, 128);
            if self.animation {
                if let Some(Ok(n)) = anim.map(AnimationPlan::new).map(|p| p.map(|p| p.frame_count())) {
                    let last = n.saturating_sub(1);
                    self.to = self.to.min(last);
                    self.from = self.from.min(self.to);
                    widgets::label(ui, "Frames", "First and last frame (both included)");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.from).range(0..=self.to));
                        ui.label("to");
                        ui.add(egui::DragValue::new(&mut self.to).range(self.from..=last));
                        if ui.small_button("All").clicked() {
                            self.from = 0;
                            self.to = last;
                        }
                    });
                    ui.end_row();
                }
                let has_ffmpeg = rmb_core::video::ffmpeg_available(program);
                widgets::label(ui, "Format", "Movies are encoded after the last frame (ffmpeg); PNG images need nothing extra");
                let before = self.format;
                egui::ComboBox::from_id_salt("net-format").selected_text(self.format.label()).show_ui(ui, |ui| {
                    for f in VideoFormat::ALL {
                        ui.add_enabled_ui(!f.needs_ffmpeg() || has_ffmpeg, |ui| ui.selectable_value(&mut self.format, f, f.label()));
                    }
                });
                ui.end_row();
                if self.format != before
                    && let Some(p) = &self.movie_path
                {
                    self.movie_path = Some(p.with_extension(self.format.extension().unwrap_or("")));
                }
                if self.format.needs_ffmpeg() {
                    widgets::checkbox(ui, "Keep frames", "Keeps the PNG frames next to the movie (folder <name>.frames)", &mut self.keep_frames);
                }
            } else {
                widgets::label(ui, "Format", "PNG 8 or 16 bit with the scene inside, or EXR with passes (see Export image)");
                let before = self.image_kind;
                egui::ComboBox::from_id_salt("net-kind").selected_text(self.image_kind.label()).show_ui(ui, |ui| {
                    for k in ImageKind::ALL {
                        ui.selectable_value(&mut self.image_kind, k, k.label());
                    }
                });
                ui.end_row();
                if self.image_kind != before
                    && let Some(p) = &mut self.image_path
                {
                    *p = p.with_extension(self.image_kind.extension());
                }
            }
        });
        ui.horizontal(|ui| {
            let (path, folder) = if self.animation { (&self.movie_path, !self.format.needs_ffmpeg()) } else { (&self.image_path, false) };
            let label = path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Nothing chosen".into());
            if ui.button(if folder { "Folder…" } else { "File…" }).on_hover_text("Where the result goes (on this computer)").clicked() {
                let kind = if self.animation { crate::prefs::Folder::Movies } else { crate::prefs::Folder::Images };
                let mut dlg = crate::prefs::dialog(kind);
                if let Some(d) = path.as_ref().and_then(|p| p.parent()) {
                    dlg = dlg.set_directory(d);
                }
                if self.animation {
                    let picked = match self.format.extension() {
                        Some(ext) => dlg.add_filter(self.format.label(), &[ext]).set_file_name(format!("animation.{ext}")).save_file().map(|p| p.with_extension(ext)),
                        None => dlg.pick_folder(),
                    };
                    if picked.is_some() {
                        self.movie_path = picked;
                    }
                } else {
                    let ext = self.image_kind.extension();
                    if let Some(p) = dlg.add_filter("Image", &[ext]).set_file_name(format!("render.{ext}")).save_file() {
                        self.image_path = Some(p.with_extension(ext));
                    }
                }
            }
            ui.label(RichText::new(label).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
        });
    }

    /// The chosen clients that are still there, the ones added by address and this computer.
    fn targets(&self) -> Vec<Target> {
        let mut t: Vec<Target> = match &self.discovery {
            Some(Ok(d)) => d.clients().into_iter().filter(|s| self.chosen.contains(&s.info.id) && !s.info.busy && s.info.version == rmb_net::version()).map(|s| Target { name: s.info.name, addr: s.addr, specs: s.info.specs }).collect(),
            _ => Vec::new(),
        };
        for a in &self.manual {
            if let Ok(addr) = rmb_net::discovery::parse_address(a) {
                t.push(Target { name: a.clone(), addr, specs: Default::default() });
            }
        }
        if self.local {
            t.push(Target { name: format!("{} (this machine)", rmb_net::host_name()), addr: ([127, 0, 0, 1], 0).into(), specs: Default::default() });
        }
        t
    }

    fn start(&mut self, scene: &Scene, anim: Option<&rmb_core::animation::Animation>, mut targets: Vec<Target>, ffmpeg: PathBuf) {
        // The local client: started once, reused.
        if self.local {
            targets.retain(|t| t.addr.port() != 0);
            if self.local_target.is_none() {
                match rmb_net::client::start_local((!self.key.is_empty()).then(|| self.key.clone())) {
                    Ok(t) => self.local_target = Some(t),
                    Err(e) => {
                        self.message = Some(e);
                        return;
                    }
                }
            }
            targets.extend(self.local_target.clone());
        }
        let (kind, output) = if self.animation && anim.is_some() {
            let Some(path) = self.movie_path.clone() else { return };
            (JobKind::Animation { from: self.from, to: self.to }, Output::Animation { path, format: self.format, ffmpeg, keep_frames: self.keep_frames })
        } else {
            let Some(path) = self.image_path.clone() else { return };
            (JobKind::Still, Output::Still { path, kind: self.image_kind })
        };
        let req = JobRequest { scene: scene.clone(), scene_dir: self.doc_dir.clone(), width: self.width, height: self.height, samples: self.samples, kind, output, key: (!self.key.is_empty()).then(|| self.key.clone()) };
        let status = Arc::new(Mutex::new(Status::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (st, ca) = (status.clone(), cancel.clone());
        let handle = std::thread::spawn(move || rmb_net::server::run_job(req, targets, st, ca));
        self.message = None;
        self.running = Some(Running { status, cancel, handle });
    }

    fn progress_ui(&self, ui: &mut egui::Ui, s: &Status) {
        let f = s.fraction();
        let el = s.started.map(|t: Instant| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        let what = if s.animation { "frames" } else { "pieces" };
        ui.label(RichText::new(format!("{}: {}/{} {what}", s.phase, s.done, s.total)).strong());
        ui.add(egui::ProgressBar::new(f).show_percentage().animate(!s.finished));
        let eta = if f > 0.02 && f < 1.0 { format!(" · about {:.0} s left", el / f as f64 - el) } else { String::new() };
        ui.label(RichText::new(format!("{el:.0} s{eta}")).color(theme::TEXT_MUTED));
        ui.add_space(4.0);
        egui::Grid::new("net-progress").num_columns(4).spacing([10.0, 4.0]).striped(true).show(ui, |ui| {
            for c in &s.clients {
                ui.label(&c.name).on_hover_text(c.specs.summary());
                match c.unit {
                    Some(u) if c.state == "rendering" => {
                        ui.label(u.label());
                        ui.add(egui::ProgressBar::new(c.fraction).desired_width(140.0).show_percentage());
                    }
                    _ => {
                        ui.label(RichText::new(&c.state).color(theme::TEXT_MUTED));
                        ui.label("");
                    }
                }
                ui.label(RichText::new(format!("{} done  {}", c.units_done, c.speed_text(s))).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
                ui.end_row();
            }
        });
    }
}
