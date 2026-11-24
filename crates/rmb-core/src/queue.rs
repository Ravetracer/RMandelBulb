// SPDX-License-Identifier: LGPL-2.1-or-later
//! Render queue (batch rendering): images and animations rendered one after the other, for
//! example overnight. The queue is a JSON file; the app keeps one in its settings folder and
//! `rmb-cli batch` runs any queue file (or a list of scene files).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Scene;
use crate::io::image::ImageKind;
use crate::video::VideoFormat;

/// What a job renders.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum JobWhat {
    /// The scene's view as one image; `tiles`: tile size (None: automatic, 0: off).
    Image { kind: ImageKind, tiles: Option<u32> },
    /// Frames `from..=to` of the scene's animation (`to` beyond the end: to the last frame).
    Animation { format: VideoFormat, from: usize, to: usize },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum JobState {
    #[default]
    Waiting,
    Running,
    Done { seconds: f64 },
    Failed { error: String },
    /// Skipped until it is set to waiting again.
    Paused,
}

impl JobState {
    pub fn label(&self) -> String {
        match self {
            JobState::Waiting => "waiting".into(),
            JobState::Running => "rendering".into(),
            JobState::Done { seconds } => format!("done ({})", duration(*seconds)),
            JobState::Failed { error } => format!("failed: {error}"),
            JobState::Paused => "paused".into(),
        }
    }
}

/// Seconds as h:mm:ss or m:ss.
pub fn duration(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueJob {
    pub id: u64,
    pub name: String,
    /// The scene file to render (read when the job starts).
    pub scene_file: Option<PathBuf>,
    /// Or the scene itself as JSON (a snapshot taken when the job was added).
    pub scene: Option<String>,
    pub what: JobWhat,
    /// 0: the scene's size.
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    /// Image file, movie file or folder of PNG frames.
    pub output: PathBuf,
    pub state: JobState,
}

impl Default for QueueJob {
    fn default() -> Self {
        QueueJob { id: 0, name: String::new(), scene_file: None, scene: None, what: JobWhat::Image { kind: ImageKind::Png8, tiles: None }, width: 0, height: 0, samples: 64, output: PathBuf::new(), state: JobState::Waiting }
    }
}

impl QueueJob {
    /// The job's scene (from its snapshot or file).
    pub fn load_scene(&self) -> Result<Scene, String> {
        match (&self.scene, &self.scene_file) {
            (Some(json), _) => Scene::from_json(json),
            (None, Some(f)) => crate::io::load(f),
            _ => Err("the job has no scene".into()),
        }
    }

    /// Image size: the job's, or the scene's (one side given: the other by the aspect).
    pub fn size(&self, scene: &Scene) -> (u32, u32) {
        match (self.width, self.height) {
            (0, 0) => (scene.width, scene.height),
            (w, 0) => (w, ((w as f64) / scene.aspect()).round().max(1.0) as u32),
            (0, h) => (((h as f64) * scene.aspect()).round().max(1.0) as u32, h),
            (w, h) => (w, h),
        }
    }

    /// One line describing the job: "1920 × 1080, 128 samples, MP4, frames 0-240, film.mp4".
    pub fn summary(&self) -> String {
        let size = if self.width == 0 && self.height == 0 { "scene size".to_string() } else { format!("{} × {}", self.width, self.height) };
        let what = match self.what {
            JobWhat::Image { kind, .. } => kind.label().to_string(),
            JobWhat::Animation { format, from, to } => {
                let to = if to == usize::MAX { "end".to_string() } else { to.to_string() };
                format!("{}, frames {from}-{to}", format.label())
            }
        };
        let out = self.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{size}, {} samples, {what}, {out}", self.samples)
    }

    /// A job for a scene file: its animation when it has keyframes, else an image, saved next
    /// to the file (or in `out_dir`).
    pub fn for_file(path: &Path, out_dir: Option<&Path>, samples: u32) -> Result<QueueJob, String> {
        let scene = crate::io::load(path)?;
        let animated = scene.animation.as_ref().is_some_and(|a| !a.keyframes.is_empty());
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "render".into());
        let dir = out_dir.map(Path::to_path_buf).or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
        let (what, output) = if animated {
            (JobWhat::Animation { format: VideoFormat::Mp4, from: 0, to: usize::MAX }, dir.join(format!("{stem}.mp4")))
        } else {
            (JobWhat::Image { kind: ImageKind::Png8, tiles: None }, dir.join(format!("{stem}.png")))
        };
        Ok(QueueJob { id: new_id(), name: stem, scene_file: Some(path.to_path_buf()), what, samples, output, ..Default::default() })
    }
}

/// A new job id (time based, unique within a queue in practice).
pub fn new_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(1);
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(p) => prev = p,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub jobs: Vec<QueueJob>,
}

impl Queue {
    pub fn load(path: &Path) -> Result<Queue, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut q: Queue = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        // A job that was running when the program stopped starts again.
        for j in &mut q.jobs {
            if j.state == JobState::Running {
                j.state = JobState::Waiting;
            }
        }
        Ok(q)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Index of the next waiting job.
    pub fn next_waiting(&self) -> Option<usize> {
        self.jobs.iter().position(|j| j.state == JobState::Waiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_round_trips_and_restarts_running_jobs() {
        let dir = std::env::temp_dir().join(format!("rmb-queue-{}", std::process::id()));
        let path = dir.join("queue.json");
        let mut q = Queue::default();
        let scene = Scene::default();
        q.jobs.push(QueueJob { id: new_id(), name: "a".into(), scene: Some(scene.to_json()), state: JobState::Running, ..Default::default() });
        q.jobs.push(QueueJob { id: new_id(), name: "b".into(), what: JobWhat::Animation { format: VideoFormat::PngSequence, from: 0, to: usize::MAX }, ..Default::default() });
        assert_ne!(q.jobs[0].id, q.jobs[1].id);
        q.save(&path).unwrap();
        let back = Queue::load(&path).unwrap();
        assert_eq!(back.jobs[0].state, JobState::Waiting);
        assert_eq!(back.jobs[1].what, q.jobs[1].what);
        assert_eq!(back.next_waiting(), Some(0));
        assert_eq!(back.jobs[0].size(&scene), (scene.width, scene.height));
        assert!(back.jobs[0].load_scene().is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
