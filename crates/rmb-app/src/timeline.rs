// SPDX-License-Identifier: LGPL-2.1-or-later

//! Timeline panel: keyframes, playback, flight recording, timing and camera smoothing of the
//! scene's animation (`Scene::animation`).
//!
//! Editing always happens on the document's scene. Scrubbing and playback show frames in the
//! view without touching the document ("preview"); clicking a keyframe loads it for editing,
//! "Update" stores the current scene back into it.

use std::time::Instant;

use egui::{Color32, RichText, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as ph;
use rmb_core::Scene;
use rmb_core::animation::{Animation, AnimationPlan, CamSample, Easing, Keyframe, Timing, keyframes_from_flight};

use crate::doc::Document;
use crate::viewport::{NavMode, PathOverlay, Recording, Viewport};
use crate::{theme, widgets};

/// What the panel asks the app to do.
pub enum TimelineAction {
    ExportAnimation,
    Message(String),
}

pub struct Timeline {
    pub open: bool,
    frame: usize,
    playing: bool,
    play_pos: f64,
    last_tick: Option<Instant>,
    /// A frame is shown in the view instead of the scene.
    previewing: bool,
    selected: Option<usize>,
    plan: Option<(Animation, Result<AnimationPlan, String>)>,
}

impl Default for Timeline {
    fn default() -> Self {
        Timeline { open: false, frame: 0, playing: false, play_pos: 0.0, last_tick: None, previewing: false, selected: None, plan: None }
    }
}

/// The scene without its animation (what a keyframe stores).
fn snapshot(scene: &Scene) -> Scene {
    Scene { animation: None, ..scene.clone() }
}

impl Timeline {
    /// The cached plan of `anim` (rebuilt when it changes).
    fn plan(&mut self, anim: &Animation) -> Option<&AnimationPlan> {
        if self.plan.as_ref().is_none_or(|(a, _)| a != anim) {
            let p = if anim.keyframes.is_empty() { Err("no keyframes".into()) } else { AnimationPlan::new(anim) };
            self.plan = Some((anim.clone(), p));
        }
        self.plan.as_ref().and_then(|(_, p)| p.as_ref().ok())
    }

    /// Leaves the preview (back to editing the document's scene).
    fn stop_preview(&mut self) {
        self.previewing = false;
        self.playing = false;
    }

    pub fn show(&mut self, ui: &mut Ui, doc: &mut Document, view: &mut Viewport) -> Option<TimelineAction> {
        let mut action = None;
        let anim = doc.scene.animation.clone().unwrap_or_default();
        let fps = anim.fps.clamp(1.0, 240.0);
        let (count, key_frames, path, plan_err) = match self.plan(&anim) {
            Some(p) => (p.frame_count(), p.key_frames(), p.path(), None),
            None => (0, Vec::new(), Vec::new(), self.plan.as_ref().and_then(|(_, p)| p.as_ref().err().cloned())),
        };
        if let Some(s) = self.selected
            && s >= anim.keyframes.len()
        {
            self.selected = None;
        }
        // Playback clock.
        if self.playing && count > 1 {
            let now = Instant::now();
            let dt = self.last_tick.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
            self.last_tick = Some(now);
            self.play_pos = (self.play_pos + dt * fps) % count as f64;
            self.frame = self.play_pos as usize;
            ui.ctx().request_repaint();
        } else {
            self.last_tick = None;
        }
        self.frame = self.frame.min(count.saturating_sub(1));
        let recording = view.recording.is_some();

        // ---- transport row
        ui.horizontal(|ui| {
            ui.add_enabled_ui(count > 1 && !recording, |ui| {
                if widgets::icon_button(ui, ph::SKIP_BACK, "First frame").clicked() {
                    self.frame = 0;
                    self.previewing = true;
                    self.playing = false;
                }
                let (icon, tip) = if self.playing { (ph::PAUSE, "Pause") } else { (ph::PLAY, "Play the animation in the view (fast preview quality)") };
                if widgets::icon_button(ui, icon, tip).clicked() {
                    self.playing = !self.playing;
                    self.play_pos = self.frame as f64;
                    self.previewing = true;
                }
                if widgets::icon_button(ui, ph::SKIP_FORWARD, "Last frame").clicked() {
                    self.frame = count.saturating_sub(1);
                    self.previewing = true;
                    self.playing = false;
                }
            });
            ui.label(RichText::new(format!("Frame {} / {}  ·  {:.2} s / {:.2} s", self.frame, count.saturating_sub(1), self.frame as f64 / fps, count.saturating_sub(1) as f64 / fps)).color(theme::TEXT_MUTED).monospace());
            if self.previewing && ui.button(format!("{}  Edit", ph::PENCIL_SIMPLE)).on_hover_text("Back to the scene (the view shows an animation frame now)").clicked() {
                self.stop_preview();
            }
            ui.separator();
            let rec_text = if recording { RichText::new(format!("{}  Stop recording", ph::STOP)).color(Color32::from_rgb(255, 110, 100)) } else { RichText::new(format!("{}  Record flight", ph::RECORD)) };
            if ui
                .button(rec_text)
                .on_hover_text("Records the camera while you fly (WASD, R/F, Q/E, drag to look). When you stop, keyframes are created along the flight; timing, speed and smoothing are set below")
                .clicked()
            {
                if let Some(rec) = view.recording.take() {
                    action = self.finish_recording(doc, rec.samples);
                } else {
                    self.stop_preview();
                    view.mode = NavMode::Fly;
                    view.recording = Some(Recording { start: Instant::now(), samples: Vec::new() });
                }
            }
            ui.separator();
            ui.add_enabled_ui(count > 0 && !recording, |ui| {
                if ui.button(format!("{}  Export animation…", crate::icons::EXPORT)).on_hover_text("Render the animation into a movie or PNG images").clicked() {
                    action = Some(TimelineAction::ExportAnimation);
                }
            });
            if let Some(e) = &plan_err
                && !anim.keyframes.is_empty()
            {
                ui.label(RichText::new(e).color(theme::WARN));
            }
        });

        // ---- scrubber
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, theme::BG);
        let x_of = |f: usize| rect.left() + 8.0 + (rect.width() - 16.0) * if count > 1 { f as f32 / (count - 1) as f32 } else { 0.0 };
        if count > 1 {
            // Seconds ticks.
            let secs = (count - 1) as f64 / fps;
            let step = [1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| secs / s <= 20.0).unwrap_or(120.0);
            let mut s = 0.0;
            while s <= secs {
                let x = x_of((s * fps).round() as usize);
                painter.line_segment([pos2(x, rect.bottom() - 6.0), pos2(x, rect.bottom())], egui::Stroke::new(1.0, theme::BORDER));
                s += step;
            }
        }
        for (k, f) in key_frames.iter().enumerate() {
            let c = pos2(x_of(*f), rect.center().y);
            let r = if self.selected == Some(k) { 7.0 } else { 5.5 };
            let fill = if self.selected == Some(k) { theme::ACCENT } else { Color32::from_rgb(255, 200, 80) };
            painter.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c + vec2(-r, 0.0)], fill, egui::Stroke::new(1.0, Color32::BLACK)));
        }
        if count > 0 {
            let x = x_of(self.frame);
            painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], egui::Stroke::new(2.0, if self.previewing { Color32::WHITE } else { theme::TEXT_MUTED }));
        }
        if count > 1
            && !recording
            && let Some(p) = resp.interact_pointer_pos()
        {
            let near_key = key_frames.iter().position(|f| (x_of(*f) - p.x).abs() < 7.0);
            if resp.clicked()
                && let Some(k) = near_key
            {
                self.select_key(doc, k, key_frames[k]);
            } else if resp.dragged() || resp.clicked() {
                let t = ((p.x - rect.left() - 8.0) / (rect.width() - 16.0)).clamp(0.0, 1.0);
                self.frame = (t * (count - 1) as f32).round() as usize;
                self.previewing = true;
                self.playing = false;
            }
        }
        resp.on_hover_text("Click or drag to look at a frame; click a diamond to edit that keyframe");

        // ---- settings
        ui.add_space(4.0);
        let mut a = anim.clone();
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    self.keyframe_ui(ui, doc, &mut a, &key_frames);
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(270.0);
                    timing_ui(ui, &mut a);
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(270.0);
                    camera_ui(ui, &mut a);
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    if let Some(m) = self.flight_ui(ui, doc, &mut a) {
                        action = Some(m);
                    }
                });
            });
        });
        if a != anim && doc.scene.animation.as_ref() != Some(&a) {
            doc.scene.animation = if a.keyframes.is_empty() && a.recording.is_empty() && doc.scene.animation.is_none() { None } else { Some(a) };
        }

        // ---- view: preview frame and path overlay
        let frame = self.frame;
        let plan = doc.scene.animation.clone().and_then(|an| self.plan(&an).map(|p| (p.scene_at(frame), p.camera(frame).pos)));
        view.preview = match (&plan, self.previewing && !recording) {
            (Some((scene, _)), true) => Some((scene.clone(), self.playing, format!("Frame {}", self.frame))),
            _ => None,
        };
        if self.previewing && plan.is_none() {
            self.stop_preview();
        }
        view.overlay = (!recording && path.len() > 1).then(|| PathOverlay {
            path,
            keys: doc.scene.animation.as_ref().map(|a| a.keyframes.iter().map(|k| k.scene.camera.position).collect()).unwrap_or_default(),
            selected: self.selected,
            current: plan.map(|p| p.1).filter(|_| self.previewing),
        });
        action
    }

    /// Loads keyframe `k` into the document for editing.
    fn select_key(&mut self, doc: &mut Document, k: usize, frame: usize) {
        let Some(anim) = doc.scene.animation.take() else { return };
        if let Some(key) = anim.keyframes.get(k) {
            doc.scene = Scene { animation: None, ..key.scene.clone() };
        }
        doc.scene.animation = Some(anim);
        self.selected = Some(k);
        self.frame = frame;
        self.stop_preview();
    }

    fn keyframe_ui(&mut self, ui: &mut Ui, doc: &mut Document, a: &mut Animation, key_frames: &[usize]) {
        ui.label(RichText::new(format!("KEYFRAMES ({})", a.keyframes.len())).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
        ui.horizontal_wrapped(|ui| {
            if ui.button(format!("{}  Add", crate::icons::PLUS)).on_hover_text("Adds the current view and settings as a keyframe after the selected one (or at the end)").clicked() {
                let at = self.selected.map(|s| s + 1).unwrap_or(a.keyframes.len());
                let frames = (a.fps * 2.0).round().max(1.0) as u32;
                a.keyframes.insert(at, Keyframe { scene: snapshot(&doc.scene), frames, ..Default::default() });
                self.selected = Some(at);
                self.stop_preview();
            }
            ui.add_enabled_ui(self.selected.is_some(), |ui| {
                if ui.button(format!("{}  Update", ph::ARROWS_CLOCKWISE)).on_hover_text("Stores the current view and settings into the selected keyframe").clicked()
                    && let Some(k) = self.selected.and_then(|s| a.keyframes.get_mut(s))
                {
                    k.scene = snapshot(&doc.scene);
                }
                if widgets::icon_button(ui, ph::CARET_LEFT, "Move the selected keyframe earlier").clicked()
                    && let Some(s) = self.selected.filter(|s| *s > 0)
                {
                    a.keyframes.swap(s, s - 1);
                    self.selected = Some(s - 1);
                }
                if widgets::icon_button(ui, ph::CARET_RIGHT, "Move the selected keyframe later").clicked()
                    && let Some(s) = self.selected.filter(|s| s + 1 < a.keyframes.len())
                {
                    a.keyframes.swap(s, s + 1);
                    self.selected = Some(s + 1);
                }
                if widgets::icon_button(ui, crate::icons::TRASH, "Delete the selected keyframe").clicked()
                    && let Some(s) = self.selected
                {
                    a.keyframes.remove(s);
                    self.selected = None;
                }
            });
        });
        match self.selected.and_then(|s| a.keyframes.get_mut(s).map(|k| (s, k))) {
            Some((s, k)) => {
                widgets::grid(ui, "kf", |ui| {
                    widgets::label(ui, "Selected", "The keyframe being edited (its frame in the animation)");
                    ui.label(RichText::new(format!("#{} · frame {}", s + 1, key_frames.get(s).copied().unwrap_or(0))).color(theme::TEXT_MUTED));
                    ui.end_row();
                    widgets::label(ui, "Frames to next", "Timing \"Per keyframe\": frames from this keyframe to the next");
                    ui.add(egui::DragValue::new(&mut k.frames).range(1..=100000));
                    ui.end_row();
                    widgets::label(ui, "Easing", "Timing \"Per keyframe\": how the frames to the next keyframe are spread");
                    egui::ComboBox::from_id_salt("kf-easing").selected_text(k.easing.label()).show_ui(ui, |ui| {
                        for e in Easing::ALL {
                            ui.selectable_value(&mut k.easing, e, e.label());
                        }
                    });
                    ui.end_row();
                });
            }
            None => {
                ui.label(RichText::new("Click a diamond to edit a keyframe; Add stores the current view.").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            }
        }
    }

    fn flight_ui(&mut self, ui: &mut Ui, doc: &mut Document, a: &mut Animation) -> Option<TimelineAction> {
        ui.label(RichText::new("RECORDED FLIGHT").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
        if a.recording.is_empty() {
            ui.label(RichText::new("Record flight, fly around, stop: keyframes follow the flight.").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            return None;
        }
        let secs = a.recording.last().map(|s| s.t).unwrap_or(0.0) - a.recording[0].t;
        ui.label(RichText::new(format!("{secs:.1} s, {} samples", a.recording.len())).color(theme::TEXT_MUTED));
        let mut msg = None;
        widgets::grid(ui, "flight", |ui| {
            widgets::slider_log(ui, "Key tolerance", "How closely the keyframes follow the flight: smaller gives more keyframes and a more exact path", &mut a.key_tolerance, 0.002..=0.5, 0.05);
        });
        ui.horizontal(|ui| {
            if ui.button(format!("{}  Keyframes again", ph::ARROWS_CLOCKWISE)).on_hover_text("Creates the keyframes from the recorded flight again with this tolerance (replaces the keyframes)").clicked() {
                a.keyframes = keyframes_from_flight(&a.recording, a.key_tolerance, &snapshot(&doc.scene), a.fps);
                self.selected = None;
                msg = Some(TimelineAction::Message(format!("{} keyframes from the flight", a.keyframes.len())));
            }
            if ui.button("Forget").on_hover_text("Removes the recorded flight (the keyframes stay)").clicked() {
                a.recording.clear();
            }
        });
        msg
    }

    /// Turns a recorded flight into keyframes.
    fn finish_recording(&mut self, doc: &mut Document, samples: Vec<CamSample>) -> Option<TimelineAction> {
        // Leave out the time before the first and after the last movement.
        let moved = |a: &CamSample, b: &CamSample| (a.pos - b.pos).length() > 1e-12 || a.rot.angle_to(b.rot) > 1e-9 || a.fov != b.fov;
        let first = samples.iter().position(|s| moved(s, &samples[0])).map(|i| i.saturating_sub(1));
        let last = samples.iter().rposition(|s| moved(s, &samples[samples.len() - 1])).map(|i| (i + 1).min(samples.len() - 1));
        let (Some(first), Some(last)) = (first, last) else {
            return Some(TimelineAction::Message("Nothing recorded: the camera did not move".into()));
        };
        let t0 = samples[first].t;
        let flight: Vec<CamSample> = samples[first..=last].iter().map(|s| CamSample { t: s.t - t0, ..*s }).collect();
        let mut a = doc.scene.animation.clone().unwrap_or_default();
        a.keyframes = keyframes_from_flight(&flight, a.key_tolerance, &snapshot(&doc.scene), a.fps);
        if a.timing == Timing::Keyframes {
            a.timing = Timing::Recorded;
        }
        let msg = format!("Recorded {:.1} s: {} keyframes", flight.last().map(|s| s.t).unwrap_or(0.0), a.keyframes.len());
        a.recording = flight;
        doc.scene.animation = Some(a);
        self.selected = None;
        self.frame = 0;
        self.previewing = true;
        Some(TimelineAction::Message(msg))
    }
}

fn timing_ui(ui: &mut Ui, a: &mut Animation) {
    let d = Animation::default();
    ui.label(RichText::new("TIMING").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    ui.horizontal(|ui| {
        for t in Timing::ALL {
            let tip = match t {
                Timing::Keyframes => "Frames and easing set per keyframe",
                Timing::Recorded => "The pace of the recorded flight, with its speed changes smoothed out",
                Timing::Steady => "One steady pace over the whole length (frames are added or removed between keyframes)",
            };
            ui.selectable_value(&mut a.timing, t, t.label()).on_hover_text(tip);
        }
    });
    widgets::grid(ui, "timing", |ui| {
        widgets::slider(ui, "Frame rate", "Frames per second of the animation", &mut a.fps, 1.0..=120.0, d.fps);
        match a.timing {
            Timing::Keyframes => {}
            Timing::Recorded => {
                widgets::slider(ui, "Smooth speed", "Seconds over which speed changes are smoothed (0: exactly as flown)", &mut a.speed_smoothing_s, 0.0..=10.0, d.speed_smoothing_s);
            }
            Timing::Steady => {
                widgets::slider(ui, "Length", "Length of the animation in seconds", &mut a.length_s, 0.5..=600.0, d.length_s);
                widgets::slider(ui, "Ease in", "Seconds to speed up at the start", &mut a.ease_in_s, 0.0..=10.0, d.ease_in_s);
                widgets::slider(ui, "Ease out", "Seconds to slow down at the end", &mut a.ease_out_s, 0.0..=10.0, d.ease_out_s);
                widgets::checkbox(ui, "Relative to surface", "Speed relative to the distance to the surface (slower close to it, as it feels; keeps zoom flights even); off: even in world units", &mut a.surface_relative);
            }
        }
    });
}

fn camera_ui(ui: &mut Ui, a: &mut Animation) {
    let d = rmb_core::animation::CameraSmoothing::default();
    let c = &mut a.camera;
    ui.horizontal(|ui| {
        ui.label(RichText::new("CAMERAMAN").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
        ui.checkbox(&mut c.on, "").on_hover_text("Smooths the camera like a professional camera operator: no jerks from keys or mouse, smooth turns and climbs");
    });
    ui.add_enabled_ui(c.on, |ui| {
        widgets::grid(ui, "cameraman", |ui| {
            widgets::slider(ui, "Smooth moves", "Seconds over which the position is smoothed (larger: steadier, rounder path)", &mut c.position_s, 0.0..=5.0, d.position_s);
            widgets::slider(ui, "Smooth turns", "Seconds over which turning and tilting are smoothed", &mut c.rotation_s, 0.0..=5.0, d.rotation_s);
            widgets::slider(ui, "Look ahead", "Turns the view toward the direction of travel", &mut c.look_ahead, 0.0..=1.0, d.look_ahead);
            widgets::slider(ui, "Level horizon", "Removes rolling, keeps the horizon level", &mut c.level_horizon, 0.0..=1.0, d.level_horizon);
            widgets::slider(ui, "Bank in turns", "Rolls into turns like a plane or drone", &mut c.bank, 0.0..=1.0, d.bank);
        });
    });
}
