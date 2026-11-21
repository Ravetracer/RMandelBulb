// SPDX-License-Identifier: LGPL-2.1-or-later

//! Output of animation frames: numbered PNG files, or a movie file written by an installed
//! ffmpeg (frames are piped to it as raw RGBA).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VideoFormat {
    /// Numbered PNG files in a folder (no ffmpeg needed).
    #[default]
    PngSequence,
    /// H.264 in MP4: plays everywhere.
    Mp4,
    /// H.265 in MP4: smaller files.
    Hevc,
    /// ProRes 422 HQ in MOV: for editing.
    ProRes,
    /// VP9 in WebM: for the web.
    WebM,
    /// Animated GIF (256 colors).
    Gif,
}

impl VideoFormat {
    pub const ALL: [VideoFormat; 6] = [VideoFormat::Mp4, VideoFormat::Hevc, VideoFormat::ProRes, VideoFormat::WebM, VideoFormat::Gif, VideoFormat::PngSequence];

    pub fn needs_ffmpeg(self) -> bool {
        self != VideoFormat::PngSequence
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoFormat::PngSequence => "PNG images",
            VideoFormat::Mp4 => "MP4 (H.264)",
            VideoFormat::Hevc => "MP4 (H.265)",
            VideoFormat::ProRes => "MOV (ProRes)",
            VideoFormat::WebM => "WebM (VP9)",
            VideoFormat::Gif => "GIF",
        }
    }

    /// File extension (none: a folder of images).
    pub fn extension(self) -> Option<&'static str> {
        match self {
            VideoFormat::PngSequence => None,
            VideoFormat::Mp4 | VideoFormat::Hevc => Some("mp4"),
            VideoFormat::ProRes => Some("mov"),
            VideoFormat::WebM => Some("webm"),
            VideoFormat::Gif => Some("gif"),
        }
    }

    /// The format a path's extension asks for (MP4 means H.264).
    pub fn from_path(p: &Path) -> VideoFormat {
        match p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
            Some("mp4" | "m4v") => VideoFormat::Mp4,
            Some("mov") => VideoFormat::ProRes,
            Some("webm") => VideoFormat::WebM,
            Some("gif") => VideoFormat::Gif,
            _ => VideoFormat::PngSequence,
        }
    }

    fn codec_args(self) -> &'static [&'static str] {
        match self {
            VideoFormat::Mp4 => &["-c:v", "libx264", "-preset", "slow", "-crf", "16", "-pix_fmt", "yuv420p", "-movflags", "+faststart"],
            VideoFormat::Hevc => &["-c:v", "libx265", "-preset", "slow", "-crf", "18", "-pix_fmt", "yuv420p", "-tag:v", "hvc1"],
            VideoFormat::ProRes => &["-c:v", "prores_ks", "-profile:v", "3", "-pix_fmt", "yuv422p10le"],
            VideoFormat::WebM => &["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "20", "-pix_fmt", "yuv420p"],
            VideoFormat::Gif | VideoFormat::PngSequence => &[],
        }
    }
}

/// The ffmpeg program: `configured` if given, else `ffmpeg` from the search path.
pub fn ffmpeg_program(configured: Option<&Path>) -> PathBuf {
    configured.filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

/// Whether the ffmpeg program runs (checked once per program).
pub fn ffmpeg_available(program: &Path) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CHECKED: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let checked = CHECKED.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(ok) = checked.lock().unwrap_or_else(|e| e.into_inner()).get(program) {
        return *ok;
    }
    let ok = Command::new(program).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    checked.lock().unwrap_or_else(|e| e.into_inner()).insert(program.to_path_buf(), ok);
    ok
}

/// Where the frames go.
pub enum VideoSink {
    Png { dir: PathBuf, w: u32, h: u32 },
    Ffmpeg { child: Child, stdin: Option<ChildStdin>, frame_bytes: usize },
}

impl VideoSink {
    /// Opens the output: a folder for PNG images, else a movie file written by `ffmpeg`.
    pub fn new(path: &Path, format: VideoFormat, w: u32, h: u32, fps: f64, ffmpeg: &Path) -> Result<VideoSink, String> {
        if format == VideoFormat::PngSequence {
            std::fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
            return Ok(VideoSink::Png { dir: path.to_path_buf(), w, h });
        }
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r", &format!("{fps}"), "-i", "-"]);
        if format == VideoFormat::Gif {
            // One palette for the whole animation, with light dithering.
            cmd.args(["-vf", "split[a][b];[a]palettegen=stats_mode=full[p];[b][p]paletteuse=dither=sierra2_4a", "-loop", "0"]);
        } else {
            // The 4:2:0 codecs need even sizes.
            cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"]);
            cmd.args(format.codec_args());
        }
        cmd.arg(path);
        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("could not start ffmpeg ({}): {e}", ffmpeg.display()))?;
        let stdin = child.stdin.take();
        Ok(VideoSink::Ffmpeg { child, stdin, frame_bytes: (w * h * 4) as usize })
    }

    /// Adds frame `index` (RGBA, 8 bits per channel).
    pub fn write_frame(&mut self, index: usize, rgba: &[u8]) -> Result<(), String> {
        match self {
            VideoSink::Png { dir, w, h } => crate::io::image::write_png(&dir.join(format!("frame_{index:05}.png")), *w, *h, rgba, None),
            VideoSink::Ffmpeg { stdin, frame_bytes, .. } => {
                if rgba.len() != *frame_bytes {
                    return Err("frame size does not match the movie".into());
                }
                stdin.as_mut().ok_or("ffmpeg has closed")?.write_all(rgba).map_err(|e| format!("ffmpeg stopped taking frames: {e}"))
            }
        }
    }

    /// Closes the output; for movies waits for ffmpeg and reports its errors.
    pub fn finish(self) -> Result<(), String> {
        match self {
            VideoSink::Png { .. } => Ok(()),
            VideoSink::Ffmpeg { child, stdin, .. } => {
                drop(stdin);
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                if out.status.success() { Ok(()) } else { Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr).trim())) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_sequence_writes_numbered_files() {
        let dir = std::env::temp_dir().join(format!("rmb-seq-{}", std::process::id()));
        let mut sink = VideoSink::new(&dir, VideoFormat::PngSequence, 2, 2, 25.0, Path::new("ffmpeg")).unwrap();
        for i in 0..3 {
            sink.write_frame(i, &[128u8; 16]).unwrap();
        }
        sink.finish().unwrap();
        assert!(dir.join("frame_00002.png").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn movie_through_ffmpeg() {
        let ffmpeg = ffmpeg_program(None);
        if !ffmpeg_available(&ffmpeg) {
            eprintln!("no ffmpeg, skipped");
            return;
        }
        for format in [VideoFormat::Mp4, VideoFormat::Gif] {
            let path = std::env::temp_dir().join(format!("rmb-movie-{}.{}", std::process::id(), format.extension().unwrap()));
            // Odd size: padded for H.264.
            let (w, h) = (33u32, 17u32);
            let mut sink = VideoSink::new(&path, format, w, h, 24.0, &ffmpeg).unwrap();
            for i in 0..5 {
                let rgba: Vec<u8> = (0..w * h).flat_map(|p| [(p * 7 + i * 40) as u8, 90, 200, 255]).collect();
                sink.write_frame(i as usize, &rgba).unwrap();
            }
            sink.finish().unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() > 100, "{format:?}");
            std::fs::remove_file(&path).unwrap();
        }
    }
}
