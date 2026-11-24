// SPDX-License-Identifier: LGPL-2.1-or-later
//! Loudness of an animation's sound track for audio-reactive drivers: the sound is decoded once
//! (ffmpeg), split into bass, mids and treble, and reduced to smoothed levels 0..1 at a fixed
//! rate. The levels are stored in the animation, so rendering needs neither the sound file nor
//! ffmpeg (render queue, network clients).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

/// Levels per second.
pub const RATE: f64 = 50.0;
/// Sample rate the sound is decoded at.
const SAMPLE_RATE: u32 = 22050;

/// Smoothed levels 0..1: overall loudness, bass (below ~150 Hz), mids, treble (above ~2.5 kHz).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioLevels {
    /// File the levels belong to (to notice a changed sound).
    pub source: String,
    /// Levels per second.
    pub rate: f64,
    /// `[loudness, bass, mid, treble]` per step, rounded to 1/1000.
    pub levels: Vec<[f32; 4]>,
}

impl AudioLevels {
    /// Levels at `t` seconds of the sound file (between steps linear; outside: 0).
    pub fn at(&self, t: f64) -> [f64; 4] {
        if self.levels.is_empty() || self.rate <= 0.0 || t < 0.0 {
            return [0.0; 4];
        }
        let x = t * self.rate;
        let i = x.floor() as usize;
        if i + 1 >= self.levels.len() {
            return if i < self.levels.len() { self.levels[i].map(|v| v as f64) } else { [0.0; 4] };
        }
        let f = (x - i as f64) as f32;
        let (a, b) = (self.levels[i], self.levels[i + 1]);
        [0, 1, 2, 3].map(|k| (a[k] + (b[k] - a[k]) * f) as f64)
    }

    /// Length of the analysed sound in seconds.
    pub fn duration(&self) -> f64 {
        if self.rate > 0.0 { self.levels.len() as f64 / self.rate } else { 0.0 }
    }
}

/// Decodes `path` with ffmpeg (mono, 22.05 kHz) and analyses it.
pub fn analyse(path: &Path, ffmpeg: &Path) -> Result<AudioLevels, String> {
    let mut child = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-ac", "1", "-ar", &SAMPLE_RATE.to_string(), "-f", "f32le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ffmpeg could not be started: {e}"))?;
    let mut bytes = Vec::new();
    child.stdout.take().ok_or("no output from ffmpeg")?.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() || bytes.is_empty() {
        return Err(format!("{}: {}", path.display(), String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("could not be read")));
    }
    let samples: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    let mut levels = levels_of(&samples, SAMPLE_RATE as f64);
    levels.source = path.to_string_lossy().into_owned();
    Ok(levels)
}

/// The levels of mono samples at `rate` Hz.
pub fn levels_of(samples: &[f32], rate: f64) -> AudioLevels {
    // Band split with one-pole filters: bass below ~150 Hz, treble above ~2.5 kHz, mids between.
    let alpha = |hz: f64| (1.0 - (-std::f64::consts::TAU * hz / rate).exp()) as f32;
    let (a_bass, a_mid) = (alpha(150.0), alpha(2500.0));
    let step = (rate / RATE).round().max(1.0) as usize;
    let mut raw: Vec<[f64; 4]> = Vec::with_capacity(samples.len() / step + 1);
    let (mut lp_bass, mut lp_mid) = (0.0f32, 0.0f32);
    let mut acc = [0.0f64; 4];
    for (n, &s) in samples.iter().enumerate() {
        lp_bass += a_bass * (s - lp_bass);
        lp_mid += a_mid * (s - lp_mid);
        let bands = [s, lp_bass, lp_mid - lp_bass, s - lp_mid];
        for k in 0..4 {
            acc[k] += (bands[k] as f64) * (bands[k] as f64);
        }
        if (n + 1) % step == 0 {
            raw.push(acc.map(|v| (v / step as f64).sqrt()));
            acc = [0.0; 4];
        }
    }
    // Normalize each band by its loud parts (95th percentile), then smooth: fast attack, slower
    // release (like a VU meter), so pulses are visible but do not jitter.
    let mut norm = [1.0f64; 4];
    for (k, nk) in norm.iter_mut().enumerate() {
        let mut v: Vec<f64> = raw.iter().map(|r| r[k]).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p = v.get((v.len() as f64 * 0.95) as usize).copied().unwrap_or(0.0);
        *nk = if p > 1e-9 { p } else { 1.0 };
    }
    let (attack, release) = (1.0 - (-1.0 / (0.02 * RATE)).exp(), 1.0 - (-1.0 / (0.25 * RATE)).exp());
    let mut env = [0.0f64; 4];
    let levels = raw
        .iter()
        .map(|r| {
            for k in 0..4 {
                let x = (r[k] / norm[k]).min(1.0);
                let a = if x > env[k] { attack } else { release };
                env[k] += (x - env[k]) * a;
            }
            env.map(|v| ((v * 1000.0).round() / 1000.0) as f32)
        })
        .collect();
    AudioLevels { source: String::new(), rate: RATE, levels }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bass_beats_show_in_the_bass_level_and_not_in_the_treble() {
        let rate = 22050.0;
        // One second: a 60 Hz beat in the first half, a 5 kHz tone in the second.
        let s: Vec<f32> = (0..22050)
            .map(|n| {
                let t = n as f64 / rate;
                (if t < 0.5 { (std::f64::consts::TAU * 60.0 * t).sin() } else { 0.5 * (std::f64::consts::TAU * 5000.0 * t).sin() }) as f32
            })
            .collect();
        let l = levels_of(&s, rate);
        assert_eq!(l.levels.len(), 50);
        let (early, late) = (l.at(0.4), l.at(0.95));
        assert!(early[1] > 0.5 && early[3] < 0.2, "early {early:?}");
        assert!(late[3] > 0.5 && late[1] < early[1], "late {late:?}");
        assert_eq!(l.at(-1.0), [0.0; 4]);
        assert!((l.duration() - 1.0).abs() < 1e-9);
    }
}
