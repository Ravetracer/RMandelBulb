// SPDX-License-Identifier: LGPL-2.1-or-later

//! Render queue window: images and animations rendered one after the other (batch rendering,
//! for example overnight). Jobs come from the export window ("Add to queue", a snapshot of the
//! scene) or from scene files; the queue is kept in the settings folder.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use egui::RichText;
use rmb_core::queue::{JobState, Queue, QueueJob};
use rmb_gpu::GpuContext;

use crate::theme;

struct Running {
    id: u64,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<String, String>>,
    started: Instant,
}

pub struct QueueWindow {
    pub open: bool,
    queue: Queue,
    /// Working through the waiting jobs.
    active: bool,
    running: Option<Running>,
    message: Option<String>,
}

fn queue_file() -> PathBuf {
    crate::prefs::config_dir().join("queue.json")
}

impl QueueWindow {
    pub fn new() -> Self {
        let queue = Queue::load(&queue_file()).unwrap_or_default();
        QueueWindow { open: false, queue, active: false, running: None, message: None }
    }

    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    pub fn waiting(&self) -> usize {
        self.queue.jobs.iter().filter(|j| j.state == JobState::Waiting).count()
    }

    fn save(&mut self) {
        if let Err(e) = self.queue.save(&queue_file()) {
            self.message = Some(format!("Queue not saved: {e}"));
        }
    }

    /// Adds a job (from the export window) and opens the window.
    pub fn add(&mut self, job: QueueJob) {
        self.message = Some(format!("Added {}", job.name));
        self.queue.jobs.push(job);
        self.save();
        self.open = true;
    }

    /// Updates the running job, starts the next one; returns a message when a job ended.
    pub fn poll(&mut self, gpu: Option<GpuContext>, ffmpeg: Option<&std::path::Path>) -> Option<String> {
        let mut msg = None;
        if let Some(r) = &self.running
            && let Ok(res) = r.rx.try_recv()
        {
            let (id, secs) = (r.id, r.started.elapsed().as_secs_f64());
            let r = self.running.take();
            let cancelled = r.is_some_and(|r| r.cancel.load(Ordering::Relaxed));
            if let Some(j) = self.queue.jobs.iter_mut().find(|j| j.id == id) {
                j.state = match &res {
                    Ok(_) => JobState::Done { seconds: secs },
                    Err(_) if cancelled => JobState::Paused,
                    Err(e) => JobState::Failed { error: e.clone() },
                };
                msg = Some(match &res {
                    Ok(s) => format!("Queue: {} saved {s}", j.name),
                    Err(_) if cancelled => format!("Queue: {} stopped", j.name),
                    Err(e) => format!("Queue: {} failed: {e}", j.name),
                });
            }
            self.save();
        }
        if self.active && self.running.is_none() {
            match self.queue.next_waiting() {
                Some(i) => self.start(i, gpu, ffmpeg),
                None => {
                    self.active = false;
                    if msg.is_some() {
                        msg = Some(format!("{} · queue finished", msg.unwrap_or_default()));
                    }
                }
            }
        }
        msg
    }

    fn start(&mut self, i: usize, gpu: Option<GpuContext>, ffmpeg: Option<&std::path::Path>) {
        let job = self.queue.jobs[i].clone();
        self.queue.jobs[i].state = JobState::Running;
        self.save();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let ffmpeg = rmb_core::video::ffmpeg_program(ffmpeg);
        let (pr, ca) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let r = rmb_gpu::batch::run_job(&job, gpu, &ffmpeg, &ca, |p| pr.store(p.to_bits(), Ordering::Relaxed));
            let _ = tx.send(r);
        });
        self.running = Some(Running { id: self.queue.jobs[i].id, progress, cancel, rx, started: Instant::now() });
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if !self.open {
            return;
        }
        let mut open = self.open;
        let mut changed = false;
        egui::Window::new("Render queue").id(egui::Id::new("render-queue")).open(&mut open).resizable(true).collapsible(false).default_width(560.0).show(ctx, |ui| {
            ui.label(RichText::new("Renders the waiting jobs one after the other. Add jobs with \"Add to queue\" in the export window (a copy of the scene as it is then) or as scene files.").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            ui.horizontal(|ui| {
                let waiting = self.waiting();
                if self.active {
                    if ui.button("Stop").on_hover_text("Stops the current job (it is paused) and the queue").clicked() {
                        self.active = false;
                        if let Some(r) = &self.running {
                            r.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                } else if ui.add_enabled(waiting > 0, egui::Button::new(RichText::new(format!("{}  Start ({waiting} waiting)", crate::icons::EXPORT)).strong()).fill(theme::ACCENT_DIM)).on_hover_text("Renders the waiting jobs from the top; the preview pauses meanwhile").clicked() {
                    self.active = true;
                }
                if ui.button("Add scene files…").on_hover_text("One job per file: its animation when it has keyframes, else an image, saved next to the file (128 samples)").clicked()
                    && let Some(files) = crate::prefs::dialog(crate::prefs::Folder::Scenes).add_filter("Scenes", &["rmb", "m3p", "png", "txt"]).pick_files()
                {
                    for f in files {
                        match QueueJob::for_file(&f, None, 128) {
                            Ok(j) => self.queue.jobs.push(j),
                            Err(e) => self.message = Some(e),
                        }
                    }
                    changed = true;
                }
                if ui.button("Remove finished").on_hover_text("Removes the jobs that are done").clicked() {
                    self.queue.jobs.retain(|j| !matches!(j.state, JobState::Done { .. }));
                    changed = true;
                }
            });
            if let Some(m) = &self.message {
                ui.label(RichText::new(m).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            }
            ui.separator();
            if self.queue.jobs.is_empty() {
                ui.label(RichText::new("The queue is empty.").color(theme::TEXT_MUTED));
            }
            let running_id = self.running.as_ref().map(|r| r.id);
            let mut action: Option<(usize, &str)> = None;
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                egui::Grid::new("queue-jobs").num_columns(3).spacing([10.0, 6.0]).striped(true).show(ui, |ui| {
                    let n = self.queue.jobs.len();
                    for (i, j) in self.queue.jobs.iter().enumerate() {
                        ui.vertical(|ui| {
                            ui.set_min_width(300.0);
                            ui.label(RichText::new(&j.name).strong());
                            ui.label(RichText::new(j.summary()).size(theme::fs(11.0)).color(theme::TEXT_MUTED)).on_hover_text(j.output.display().to_string());
                        });
                        if Some(j.id) == running_id
                            && let Some(r) = &self.running
                        {
                            let p = f32::from_bits(r.progress.load(Ordering::Relaxed));
                            let el = r.started.elapsed().as_secs_f64();
                            let eta = if p > 0.02 { format!(", about {} left", rmb_core::queue::duration(el / p as f64 - el)) } else { String::new() };
                            ui.vertical(|ui| {
                                ui.add(egui::ProgressBar::new(p).desired_width(160.0).show_percentage());
                                ui.label(RichText::new(format!("{}{eta}", rmb_core::queue::duration(el))).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
                            });
                        } else {
                            let color = if matches!(j.state, JobState::Failed { .. }) { theme::WARN } else { theme::TEXT_MUTED };
                            ui.label(RichText::new(j.state.label()).size(theme::fs(11.5)).color(color));
                        }
                        ui.horizontal(|ui| {
                            let idle = Some(j.id) != running_id;
                            if ui.add_enabled(idle && i > 0, egui::Button::new(crate::icons::CARET_UP).small()).on_hover_text("Earlier").clicked() {
                                action = Some((i, "up"));
                            }
                            if ui.add_enabled(idle && i + 1 < n, egui::Button::new(crate::icons::CARET_DOWN).small()).on_hover_text("Later").clicked() {
                                action = Some((i, "down"));
                            }
                            let (label, tip) = if j.state == JobState::Waiting { ("Pause", "Skip this job until it is set to waiting again") } else { ("Again", "Render this job again (waiting)") };
                            if ui.add_enabled(idle, egui::Button::new(label).small()).on_hover_text(tip).clicked() {
                                action = Some((i, "toggle"));
                            }
                            if ui.add_enabled(idle, egui::Button::new(crate::icons::TRASH).small()).on_hover_text("Remove the job").clicked() {
                                action = Some((i, "remove"));
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            if let Some((i, a)) = action {
                match a {
                    "up" => self.queue.jobs.swap(i, i - 1),
                    "down" => self.queue.jobs.swap(i, i + 1),
                    "toggle" => {
                        let j = &mut self.queue.jobs[i];
                        j.state = if j.state == JobState::Waiting { JobState::Paused } else { JobState::Waiting };
                    }
                    _ => {
                        self.queue.jobs.remove(i);
                    }
                }
                changed = true;
            }
        });
        if changed {
            self.save();
        }
        self.open = open;
    }
}
