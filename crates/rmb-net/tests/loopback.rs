// SPDX-License-Identifier: LGPL-2.1-or-later
//! A server and two clients on this machine render an animation and a still image.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

use rmb_core::Scene;
use rmb_net::client::{ClientEvent, ClientOptions, run_client};
use rmb_net::proto::JobKind;
use rmb_net::server::{JobRequest, Output, Status, Target, run_job};

/// Starts a client on a free port (CPU, no announcements) that stops after `jobs` jobs.
fn start_client(dir: &std::path::Path, name: &str) -> Target {
    let (tx, rx) = mpsc::channel();
    let opts = ClientOptions { name: Some(name.into()), port: Some(0), cpu: true, cache: Some(dir.join(name)), silent: true, once: false, ..Default::default() };
    std::thread::spawn(move || {
        let _ = run_client(opts, |e| {
            if let ClientEvent::Started { port, specs, .. } = e {
                let _ = tx.send((port, specs));
            }
        });
    });
    let (port, specs) = rx.recv().unwrap();
    Target { name: name.into(), addr: ([127, 0, 0, 1], port).into(), specs }
}

fn small_scene() -> Scene {
    let mut s = Scene::default();
    s.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    s.post.denoise = false;
    s
}

#[test]
fn clients_render_an_animation_and_a_still() {
    let dir = std::env::temp_dir().join(format!("rmb-net-loop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let clients = vec![start_client(&dir, "a"), start_client(&dir, "b")];

    // Animation: three keyframes, 6 frames into a PNG folder.
    let mut scene = small_scene();
    let mut anim = rmb_core::animation::Animation::default();
    for k in 0..3 {
        let mut s = small_scene();
        s.camera.orbit(rmb_core::math::V3::ZERO, rmb_core::math::v3(0.0, 0.0, 1.0), 0.2 * k as f64, 0.0);
        anim.keyframes.push(rmb_core::animation::Keyframe { scene: s, frames: 3, ..Default::default() });
    }
    scene.animation = Some(anim);
    let plan = rmb_core::animation::AnimationPlan::new(scene.animation.as_ref().unwrap()).unwrap();
    let last = plan.frame_count() - 1;
    let out = dir.join("frames");
    let req = JobRequest { scene: scene.clone(), scene_dir: None, width: 32, height: 20, samples: 2, kind: JobKind::Animation { from: 0, to: last }, output: Output::Animation { path: out.clone(), format: rmb_core::video::VideoFormat::PngSequence, ffmpeg: "ffmpeg".into(), keep_frames: true }, key: None };
    let status = Arc::new(Mutex::new(Status::default()));
    let summary = run_job(req, clients.clone(), status.clone(), Arc::new(AtomicBool::new(false))).unwrap();
    assert!(summary.contains("frames"), "{summary}");
    for i in 0..=last {
        assert!(out.join(format!("frame_{i:05}.png")).is_file(), "frame {i} missing");
    }
    let st = status.lock().unwrap().clone();
    assert_eq!(st.done, last + 1);
    assert_eq!(st.clients.iter().map(|c| c.units_done).sum::<usize>(), last + 1);
    // The clients deleted their copies after the server stored them.
    for c in ["a", "b"] {
        let spool = dir.join(c).join("spool");
        let left = std::fs::read_dir(&spool).map(|d| d.flatten().flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten()).count()).unwrap_or(0);
        assert_eq!(left, 0, "client {c} kept results");
    }

    // Still: samples split over the clients equal one render with all samples.
    let still = small_scene();
    let path = dir.join("still.exr");
    let req = JobRequest { scene: still.clone(), scene_dir: None, width: 32, height: 20, samples: 6, kind: JobKind::Still, output: Output::Still { path: path.clone(), kind: rmb_core::io::image::ImageKind::Exr }, key: None };
    run_job(req, clients, Arc::new(Mutex::new(Status::default())), Arc::new(AtomicBool::new(false))).unwrap();
    let mut local = rmb_core::render::cpu::CpuRenderer::new(still, 32, 20);
    for _ in 0..6 {
        local.render_pass();
    }
    let want = local.hdr_image();
    let exr = exr::prelude::read_all_flat_layers_from_file(&path).unwrap();
    let r: Vec<f32> = exr.layer_data[0].channel_data.list.iter().find(|c| c.name.to_string() == "R").unwrap().sample_data.values_as_f32().collect();
    let worst = r.iter().enumerate().map(|(i, v)| (v - want.color[i * 3]).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "split render differs by {worst}");
    std::fs::remove_dir_all(&dir).ok();
}
