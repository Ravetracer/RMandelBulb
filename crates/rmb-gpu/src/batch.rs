// SPDX-License-Identifier: LGPL-2.1-or-later
//! Runs one job of a render queue (`rmb_core::queue`): an image or an animation, on the GPU
//! when a context is given, else on the CPU.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rmb_core::queue::{JobWhat, QueueJob};

use crate::GpuContext;

/// Renders `job` and writes its output; returns a short description of what was written.
/// `progress` gets the finished fraction; setting `cancel` stops after the current pass.
pub fn run_job(job: &QueueJob, ctx: Option<GpuContext>, ffmpeg: &Path, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<String, String> {
    let scene = job.load_scene()?;
    for e in rmb_core::compose::load_models(&scene) {
        eprintln!("3D model not loaded: {e}");
    }
    let (w, h) = job.size(&scene);
    let samples = job.samples.max(1);
    if let Some(dir) = job.output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    match job.what {
        JobWhat::Image { kind, tiles } => {
            let tile = match tiles {
                Some(0) => None,
                Some(t) => Some(t),
                None => rmb_core::io::image::auto_tile(w, h),
            };
            let keep_going = |p: f32, progress: &mut dyn FnMut(f32)| {
                progress(p);
                !cancel.load(Ordering::Relaxed)
            };
            let outcome = match (ctx, tile) {
                (Some(ctx), Some(t)) => {
                    let (img, outcome) = crate::render_tiled_hdr(ctx, &scene, w, h, samples, t, kind == rmb_core::io::image::ImageKind::Exr, |p| keep_going(p, &mut progress))?;
                    rmb_core::io::image::save_render_ex(&job.output, img, &scene, kind, true)?;
                    outcome
                }
                (Some(ctx), None) => {
                    let img = crate::render_blocking_hdr(ctx, &scene, w, h, samples, |p| keep_going(p, &mut progress))?;
                    rmb_core::io::image::save_render(&job.output, img, &scene, kind)?
                }
                (None, _) => {
                    let mut r = rmb_core::render::cpu::CpuRenderer::new(scene.clone(), w, h);
                    for i in 0..samples {
                        if cancel.load(Ordering::Relaxed) {
                            return Err("cancelled".into());
                        }
                        r.render_pass();
                        progress((i + 1) as f32 / samples as f32);
                    }
                    rmb_core::io::image::save_render(&job.output, r.hdr_image(), &scene, kind)?
                }
            };
            let note = if scene.post.denoise { format!(", {}", outcome.describe()) } else { String::new() };
            Ok(format!("{} ({w} × {h}, {samples} samples{note})", job.output.display()))
        }
        JobWhat::Animation { format, from, to } => {
            let mut anim = scene.animation.clone().filter(|a| !a.keyframes.is_empty()).ok_or("the scene has no animation")?;
            anim.ensure_audio_levels(job.scene_file.as_deref().and_then(Path::parent), ffmpeg)?;
            let plan = rmb_core::animation::AnimationPlan::new(&anim)?;
            let last = plan.frame_count().saturating_sub(1);
            let (from, to) = (from.min(last), to.min(last));
            if from > to {
                return Err("the first frame is after the last one".into());
            }
            if format.needs_ffmpeg() && !rmb_core::video::ffmpeg_available(ffmpeg) {
                return Err(format!("{} needs ffmpeg, which was not found", format.label()));
            }
            let audio = anim.audio_track(from, job.scene_file.as_deref().and_then(Path::parent));
            let mut sink = rmb_core::video::VideoSink::new(&job.output, format, w, h, plan.fps(), ffmpeg, audio.as_ref())?;
            let size = (scene.width, scene.height);
            let frame_scene = |t: f64| {
                let mut s = plan.scene_at_time(t);
                (s.width, s.height) = size;
                s
            };
            crate::render_animation(ctx, from..to + 1, w, h, samples, anim.shutter, frame_scene, cancel, &mut progress, format.hdr_frames(), |f, frame| sink.write(f - from, frame))?;
            sink.finish()?;
            Ok(format!("{} ({} frames, {w} × {h}, {samples} samples)", job.output.display(), to + 1 - from))
        }
    }
}
