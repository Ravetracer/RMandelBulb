// SPDX-License-Identifier: LGPL-2.1-or-later

//! Keyframe animation: keyframes are whole scenes; every number blends between them.
//!
//! The camera follows its own path: a centripetal Catmull-Rom spline through the keyframe
//! positions (no overshoot, no loops) and a quaternion spline (squad) for the orientation.
//! Frames are spread over that path by one of three timings: frame counts per keyframe, the
//! times of a recorded flight (with a smoothed speed profile), or a steady pace measured as
//! perceived speed (movement relative to the distance to the surface, plus turning).
//! An optional "cameraman" pass then low-passes position and orientation per frame (zero
//! phase, ends held), can turn the view toward the direction of travel, level the horizon and
//! bank into turns.
//!
//! Everything else in the scene blends with a monotone cubic (no overshoot) through the
//! keyframe values: the scenes are blended as JSON, numbers interpolated (integers rounded,
//! some magnitudes on a log scale), everything else held from the keyframe before.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::math::{Quat, V3};
use crate::scene::Scene;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animation {
    pub fps: f64,
    pub keyframes: Vec<Keyframe>,
    pub timing: Timing,
    /// Steady timing: length of the animation in seconds.
    pub length_s: f64,
    /// Steady timing: seconds to speed up at the start and to slow down at the end.
    pub ease_in_s: f64,
    pub ease_out_s: f64,
    /// Recorded timing: the speed is smoothed over this many seconds (0: as flown).
    pub speed_smoothing_s: f64,
    /// Steady timing: measure speed relative to the distance to the surface (slower close to
    /// it, as it feels), else in world units.
    pub surface_relative: bool,
    pub camera: CameraSmoothing,
    /// The raw recorded flight (kept to create the keyframes again).
    pub recording: Vec<CamSample>,
    /// Allowed deviation from the flight when keyframes are created from it.
    pub key_tolerance: f64,
    /// Sound file put under the movie (relative paths are relative to the scene file).
    pub audio: Option<std::path::PathBuf>,
    /// Position in the sound file (seconds) where frame 0 starts; negative values start the
    /// sound later.
    pub audio_offset_s: f64,
    /// Motion blur: how long each frame is exposed, as a fraction of the frame time (0: off,
    /// 0.5: a film camera's 180° shutter).
    pub shutter: f64,
    /// Scene values computed by expressions in every frame (after the keyframes).
    pub drivers: Vec<Driver>,
    /// The drivers also act on still images and the view outside the animation preview: at
    /// frame 0, with `value` = the scene's own value (`still_scene`).
    pub still_drivers: bool,
    /// Levels of the sound track for audio-reactive drivers (`audio::analyse`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_levels: Option<crate::audio::AudioLevels>,
}

/// A scene value driven by an expression of time (and other values) in an animation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Driver {
    pub enabled: bool,
    /// The value: a path into the scene ("camera.fov", "fractal.slots.0.params.power").
    pub path: String,
    /// The expression (see `expr`): variables `t` (seconds), `frame`, `frames`, `u` (0..1 over
    /// the animation), `key` (keyframe position), `fps`, `value` (the keyframed value).
    pub expression: String,
}

impl Default for Driver {
    fn default() -> Self {
        Driver { enabled: true, path: String::new(), expression: "value".into() }
    }
}

impl Animation {
    /// Whether a driver reads the sound levels (`audio`, `bass`, `mid`, `treble`).
    pub fn drivers_use_audio(&self) -> bool {
        self.drivers.iter().any(|d| d.enabled && ["audio", "bass", "mid", "treble"].iter().any(|v| uses_name(&d.expression, v)))
    }

    /// Analyses the sound track for the drivers when they need it and the stored levels are
    /// missing or belong to another file (`base`: folder of the scene file). Returns whether it
    /// analysed.
    pub fn ensure_audio_levels(&mut self, base: Option<&std::path::Path>, ffmpeg: &std::path::Path) -> Result<bool, String> {
        if !self.drivers_use_audio() {
            return Ok(false);
        }
        let track = self.audio_track(0, base).ok_or("the drivers use the sound, but the animation has no sound file")?;
        let source = track.path.to_string_lossy().into_owned();
        if self.audio_levels.as_ref().is_some_and(|l| l.source == source) {
            return Ok(false);
        }
        self.audio_levels = Some(crate::audio::analyse(&track.path, ffmpeg)?);
        Ok(true)
    }
}

/// Whether `name` occurs in `expr` as a whole word.
fn uses_name(expr: &str, name: &str) -> bool {
    let b = expr.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'@' || c == b'.';
    expr.match_indices(name).any(|(i, _)| (i == 0 || !word(b[i - 1])) && b.get(i + name.len()).is_none_or(|c| !word(*c)))
}

/// Variables of a driver expression at one moment of the animation.
#[derive(Clone, Copy, Debug)]
pub struct DriverTime {
    /// Time in frames (fractional between frames).
    pub frame: f64,
    pub frames: usize,
    pub fps: f64,
    /// Keyframe position (0 at the first keyframe, 1 at the second …).
    pub key: f64,
    /// Sound levels at this moment: loudness, bass, mid, treble (0 without a sound).
    pub audio: [f64; 4],
}

/// A value at a path of a scene's JSON (`a.b.0.c`), or None.
pub fn value_at(v: &Value, path: &str) -> Option<f64> {
    let mut cur = v;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    match cur {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Sets the number at a path (objects get missing keys; booleans take values above 0.5 as
/// true). False when the path does not exist.
fn set_value_at(v: &mut Value, path: &str, x: f64) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = v;
    for (k, part) in parts.iter().enumerate() {
        let last = k + 1 == parts.len();
        cur = match cur {
            Value::Object(m) => {
                if last && !m.contains_key(*part) {
                    m.insert((*part).to_string(), Value::Null);
                }
                match m.get_mut(*part) {
                    Some(c) => c,
                    None => return false,
                }
            }
            Value::Array(a) => match part.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                Some(c) => c,
                None => return false,
            },
            _ => return false,
        };
    }
    *cur = match cur {
        Value::Bool(_) => Value::Bool(x > 0.5),
        // Integers (iteration counts) stay integers.
        Value::Number(n) if n.is_u64() || n.is_i64() => serde_json::json!(x.round() as i64),
        _ => serde_json::json!(x),
    };
    true
}

/// The default of a formula parameter that the scene does not list (it is at its default):
/// `fractal.slots.N.params.IDENT` and the same below `objects.K.shape.Fractal`.
fn param_default(v: &Value, path: &str) -> Option<f64> {
    let (slot_path, ident) = path.rsplit_once(".params.")?;
    let formula = match slot_path.rsplit_once('.') {
        Some(_) => {
            let mut cur = v;
            for part in slot_path.split('.') {
                cur = match cur {
                    Value::Object(m) => m.get(part)?,
                    Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
                    _ => return None,
                };
            }
            cur.get("formula")?.as_str()?.to_string()
        }
        None => return None,
    };
    let f = crate::formula::find(&formula)?;
    f.def.params.iter().find(|p| p.ident == ident).map(|p| p.default)
}

/// A driver with its parsed expression.
#[derive(Clone, Debug)]
pub struct CompiledDriver {
    pub path: String,
    pub expr: crate::expr::Expr,
}

/// Parses the enabled drivers; the errors name the driver (`path: message`).
pub fn compile_drivers(drivers: &[Driver]) -> (Vec<CompiledDriver>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for d in drivers.iter().filter(|d| d.enabled && !d.path.trim().is_empty()) {
        match crate::expr::Expr::parse(&d.expression) {
            Ok(expr) => out.push(CompiledDriver { path: d.path.trim().to_string(), expr }),
            Err(e) => errors.push(format!("{}: {e}", d.path)),
        }
    }
    (out, errors)
}

/// A still image's scene with its drivers applied (`Animation::still_drivers`): at frame 0 of
/// one frame, `value` = the scene's own value, the sound levels at its start. The result has
/// `still_drivers` off, so it is never driven twice. Also the sky's sun written into light 1
/// (`sky::sync_sun`) when that changes it. `None` when nothing applies.
pub fn still_scene(scene: &Scene) -> Option<Scene> {
    let Some(a) = scene.animation.as_ref().filter(|a| a.still_drivers && a.drivers.iter().any(|d| d.enabled)) else {
        if scene.background.mode != crate::scene::BackgroundMode::Sky || !scene.sky.sun_light {
            return None;
        }
        let mut s = scene.clone();
        return crate::sky::sync_sun(&mut s).then_some(s);
    };
    let drivers = compile_drivers(&a.drivers).0;
    let audio = a.audio_levels.as_ref().map_or([0.0; 4], |l| l.at(a.audio_offset_s.max(0.0)));
    let mut s = scene.clone();
    if let Some(an) = s.animation.as_mut() {
        an.still_drivers = false;
    }
    apply_drivers(&mut s, &drivers, DriverTime { frame: 0.0, frames: 1, fps: a.fps, key: 0.0, audio });
    crate::sky::sync_sun(&mut s);
    Some(s)
}

/// Applies drivers to a scene at time `tm`; returns the errors (`path: message`). Values a
/// driver cannot set (missing path, not finite) stay as they are.
pub fn apply_drivers(scene: &mut Scene, drivers: &[CompiledDriver], tm: DriverTime) -> Vec<String> {
    if drivers.is_empty() {
        return Vec::new();
    }
    let Ok(mut v) = serde_json::to_value(&*scene) else { return vec!["the scene could not be read".into()] };
    let mut errors = Vec::new();
    let fps = tm.fps.max(1e-9);
    for d in drivers {
        let current = value_at(&v, &d.path).or_else(|| param_default(&v, &d.path));
        let vars = |name: &str| match name {
            "t" => Some(tm.frame / fps),
            "frame" => Some(tm.frame),
            "frames" => Some(tm.frames as f64),
            "u" => Some(if tm.frames > 1 { tm.frame / (tm.frames - 1) as f64 } else { 0.0 }),
            "key" => Some(tm.key),
            "fps" => Some(fps),
            "audio" => Some(tm.audio[0]),
            "bass" => Some(tm.audio[1]),
            "mid" => Some(tm.audio[2]),
            "treble" => Some(tm.audio[3]),
            "value" => current,
            _ => None,
        };
        let paths = |p: &str| value_at(&v, p).or_else(|| param_default(&v, p));
        match d.expr.eval(&vars, &paths) {
            Ok(x) if x.is_finite() => {
                if !set_value_at(&mut v, &d.path, x) {
                    errors.push(format!("{}: no such value in the scene", d.path));
                }
            }
            Ok(_) => errors.push(format!("{}: the result is not a number", d.path)),
            Err(e) => errors.push(format!("{}: {e}", d.path)),
        }
    }
    match serde_json::from_value::<Scene>(v) {
        Ok(mut s) => {
            // Not part of the JSON.
            s.motion_end = scene.motion_end.take();
            *scene = s;
        }
        Err(e) => errors.push(format!("a driven value does not fit: {e}")),
    }
    errors
}

/// Numeric values of a scene a driver can drive (paths), with their current values; formula
/// parameters at their defaults included.
pub fn drivable_values(scene: &Scene) -> Vec<(String, f64)> {
    let mut s = scene.clone();
    s.animation = None;
    let Ok(v) = serde_json::to_value(&s) else { return Vec::new() };
    let mut out = Vec::new();
    fn walk(v: &Value, path: &str, out: &mut Vec<(String, f64)>) {
        match v {
            Value::Number(n) => out.push((path.to_string(), n.as_f64().unwrap_or(0.0))),
            Value::Bool(b) => out.push((path.to_string(), if *b { 1.0 } else { 0.0 })),
            Value::Object(m) => {
                for (k, c) in m {
                    if path.is_empty() && matches!(k.as_str(), "version" | "formulas" | "notes") {
                        continue;
                    }
                    walk(c, &if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, out);
                }
            }
            Value::Array(a) => {
                for (i, c) in a.iter().enumerate() {
                    walk(c, &format!("{path}.{i}"), out);
                }
            }
            _ => {}
        }
    }
    walk(&v, "", &mut out);
    // Formula parameters at their defaults (not in the JSON).
    for (i, slot) in scene.fractal.slots.iter().enumerate() {
        if let Some(f) = crate::formula::find(&slot.formula) {
            for p in &f.def.params {
                let path = format!("fractal.slots.{i}.params.{}", p.ident);
                if !out.iter().any(|(q, _)| *q == path) {
                    out.push((path, slot.params.get(&p.ident).copied().unwrap_or(p.default)));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            fps: 30.0,
            keyframes: Vec::new(),
            timing: Timing::Keyframes,
            length_s: 10.0,
            ease_in_s: 1.0,
            ease_out_s: 1.0,
            speed_smoothing_s: 1.0,
            surface_relative: true,
            camera: CameraSmoothing::default(),
            recording: Vec::new(),
            key_tolerance: 0.05,
            audio: None,
            audio_offset_s: 0.0,
            shutter: 0.0,
            drivers: Vec::new(),
            still_drivers: false,
            audio_levels: None,
        }
    }
}

impl Animation {
    /// The sound track for a movie that starts at frame `from` (`base`: folder of the scene
    /// file, for relative paths).
    pub fn audio_track(&self, from: usize, base: Option<&std::path::Path>) -> Option<crate::video::AudioTrack> {
        let p = self.audio.as_ref().filter(|p| !p.as_os_str().is_empty())?;
        let path = match base {
            Some(b) if p.is_relative() => b.join(p),
            _ => p.clone(),
        };
        Some(crate::video::AudioTrack { path, start_s: self.audio_offset_s + from as f64 / self.fps.max(1e-9) })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keyframe {
    pub scene: Scene,
    /// Keyframe timing: frames from this keyframe to the next.
    pub frames: u32,
    /// Keyframe timing: how the frames to the next keyframe are spread.
    pub easing: Easing,
    /// Time in the recorded flight (seconds from its start).
    pub time_s: Option<f64>,
}

impl Default for Keyframe {
    fn default() -> Self {
        Keyframe { scene: Scene::default(), frames: 60, easing: Easing::Linear, time_s: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Easing {
    /// Even steps (the path still bends smoothly through the keyframe).
    #[default]
    Linear,
    /// Starts and stops slowly.
    Smooth,
    EaseIn,
    EaseOut,
}

impl Easing {
    pub const ALL: [Easing; 4] = [Easing::Linear, Easing::Smooth, Easing::EaseIn, Easing::EaseOut];

    pub fn label(self) -> &'static str {
        match self {
            Easing::Linear => "Even",
            Easing::Smooth => "Slow in and out",
            Easing::EaseIn => "Slow start",
            Easing::EaseOut => "Slow end",
        }
    }

    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::Smooth => t * t * (3.0 - 2.0 * t),
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Timing {
    /// Frame counts and easing per keyframe.
    #[default]
    Keyframes,
    /// The times of the recorded flight, with a smoothed speed.
    Recorded,
    /// One steady pace over the whole length.
    Steady,
}

impl Timing {
    pub const ALL: [Timing; 3] = [Timing::Keyframes, Timing::Recorded, Timing::Steady];

    pub fn label(self) -> &'static str {
        match self {
            Timing::Keyframes => "Per keyframe",
            Timing::Recorded => "As flown",
            Timing::Steady => "Steady",
        }
    }
}

/// The "cameraman": smoothing of the camera per frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSmoothing {
    pub on: bool,
    /// Low-pass window of the position (seconds).
    pub position_s: f64,
    /// Low-pass window of the orientation (seconds).
    pub rotation_s: f64,
    /// 0..1: turns the view toward the direction of travel.
    pub look_ahead: f64,
    /// 0..1: removes the roll (keeps the horizon level).
    pub level_horizon: f64,
    /// 0..1: rolls into turns like a plane or drone.
    pub bank: f64,
}

impl Default for CameraSmoothing {
    fn default() -> Self {
        CameraSmoothing { on: false, position_s: 0.5, rotation_s: 0.8, look_ahead: 0.0, level_horizon: 0.0, bank: 0.0 }
    }
}

/// One sample of a recorded flight.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CamSample {
    pub t: f64,
    pub pos: V3,
    pub rot: Quat,
    pub fov: f64,
}

impl CamSample {
    pub fn of(scene: &Scene, t: f64) -> CamSample {
        CamSample { t, pos: scene.camera.position, rot: scene.camera.orientation(), fov: scene.camera.fov }
    }
}

/// Camera of one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameCam {
    /// Position in keyframe space (keyframe index plus fraction).
    pub u: f64,
    pub pos: V3,
    pub rot: Quat,
    pub fov: f64,
}

// ---------------------------------------------------------------------------------------------
// Splines

/// Centripetal Catmull-Rom between `p1` and `p2` (`p0`, `p3` the neighbours) at `t` in 0..1.
fn catmull_rom(p0: V3, p1: V3, p2: V3, p3: V3, t: f64) -> V3 {
    let k = |a: V3, b: V3| (b - a).length().sqrt().max(1e-12);
    let (t0, t1) = (0.0, k(p0, p1));
    let t2 = t1 + k(p1, p2);
    let t3 = t2 + k(p2, p3);
    let s = t1 + (t2 - t1) * t;
    let lerp = |a: V3, b: V3, ta: f64, tb: f64| a * ((tb - s) / (tb - ta)) + b * ((s - ta) / (tb - ta));
    let a1 = lerp(p0, p1, t0, t1);
    let a2 = lerp(p1, p2, t1, t2);
    let a3 = lerp(p2, p3, t2, t3);
    let b1 = lerp(a1, a2, t0, t2);
    let b2 = lerp(a2, a3, t1, t3);
    lerp(b1, b2, t1, t2)
}

/// Monotone cubic Hermite through `v1`..`v2` (neighbours `v0`, `v3`): no overshoot.
fn monotone(v0: f64, v1: f64, v2: f64, v3: f64, t: f64) -> f64 {
    let d1 = v1 - v0;
    let d = v2 - v1;
    let d3 = v3 - v2;
    let tangent = |a: f64, b: f64| if a * b <= 0.0 { 0.0 } else { 2.0 * a * b / (a + b) };
    let (m1, m2) = (tangent(d1, d), tangent(d, d3));
    let (t2, t3) = (t * t, t * t * t);
    v1 * (2.0 * t3 - 3.0 * t2 + 1.0) + m1 * (t3 - 2.0 * t2 + t) + v2 * (-2.0 * t3 + 3.0 * t2) + m2 * (t3 - t2)
}

/// Squad interpolation of orientations `q[i]` → `q[i+1]` (signs already made consistent).
fn squad(q: &[Quat], i: usize, t: f64) -> Quat {
    let n = q.len();
    let at = |k: isize| q[k.clamp(0, n as isize - 1) as usize];
    let inner = |k: isize| {
        let qk = at(k);
        let a = qk.conj().times(at(k + 1)).log();
        let b = qk.conj().times(at(k - 1)).log();
        qk.times(Quat::exp((a + b) * -0.25))
    };
    let (q1, q2) = (at(i as isize), at(i as isize + 1));
    let (s1, s2) = (inner(i as isize), inner(i as isize + 1));
    q1.slerp(q2, t).slerp(s1.slerp(s2, t), 2.0 * t * (1.0 - t))
}

/// Quaternions with neighbours on the same side (no flips).
fn consistent(mut q: Vec<Quat>) -> Vec<Quat> {
    for i in 1..q.len() {
        if q[i].dot(q[i - 1]) < 0.0 {
            q[i] = q[i].negated();
        }
    }
    q
}

/// A camera path through keys (positions, orientations, field of view).
struct CamSpline {
    pos: Vec<V3>,
    rot: Vec<Quat>,
    fov: Vec<f64>,
}

impl CamSpline {
    fn new(pos: Vec<V3>, rot: Vec<Quat>, fov: Vec<f64>) -> CamSpline {
        CamSpline { pos, rot: consistent(rot), fov }
    }

    fn keys(&self) -> usize {
        self.pos.len()
    }

    /// Camera at `u` (key index plus fraction).
    fn at(&self, u: f64) -> FrameCam {
        let n = self.keys();
        if n == 1 {
            return FrameCam { u: 0.0, pos: self.pos[0], rot: self.rot[0], fov: self.fov[0] };
        }
        let u = u.clamp(0.0, (n - 1) as f64);
        let i = (u.floor() as usize).min(n - 2);
        let t = u - i as f64;
        let p = |k: isize| self.pos[k.clamp(0, n as isize - 1) as usize];
        let ii = i as isize;
        // Extrapolated end points that continue the curvature (a straight continuation with
        // only two keys), so the path does not start or end with a straight stub or a hook.
        let ext = |a: V3, b: V3, c: Option<V3>| match c {
            Some(c) => a * 3.0 - b * 3.0 + c,
            None => a * 2.0 - b,
        };
        let p0 = if i == 0 { ext(p(0), p(1), (n > 2).then(|| p(2))) } else { p(ii - 1) };
        let p3 = if i + 2 >= n { ext(p(ii + 1), p(ii), (n > 2).then(|| p(ii - 1))) } else { p(ii + 2) };
        let f = |k: isize| self.fov[k.clamp(0, n as isize - 1) as usize];
        FrameCam { u, pos: catmull_rom(p0, p(ii), p(ii + 1), p3, t), rot: squad(&self.rot, i, t), fov: monotone(f(ii - 1), f(ii), f(ii + 1), f(ii + 2), t) }
    }
}

// ---------------------------------------------------------------------------------------------
// Blending the other parameters

/// Scene fields interpolated on a log scale (they change by orders of magnitude).
const LOG_FIELDS: [&str; 5] = ["march.max_distance", "march.near", "lighting.ao_radius", "dof.focus_distance", "fractal.bailout"];

fn same_shape(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => true,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len(),
        (Value::Object(_), Value::Object(_)) => true,
        _ => false,
    }
}

/// Blends `v1` → `v2` at `t` (neighbours `v0`, `v3`); `path` is the dotted field name.
fn blend(v0: &Value, v1: &Value, v2: &Value, v3: &Value, t: f64, path: &str) -> Value {
    if !same_shape(v1, v2) {
        return v1.clone();
    }
    let v0 = if same_shape(v0, v1) { v0 } else { v1 };
    let v3 = if same_shape(v3, v2) { v3 } else { v2 };
    match (v1, v2) {
        (Value::Number(a), Value::Number(b)) => {
            let g = |v: &Value| v.as_f64().unwrap_or(0.0);
            let (x0, x1, x2, x3) = (g(v0), a.as_f64().unwrap_or(0.0), b.as_f64().unwrap_or(0.0), g(v3));
            let log = LOG_FIELDS.contains(&path) && [x0, x1, x2, x3].iter().all(|x| *x > 0.0);
            let r = if log { monotone(x0.ln(), x1.ln(), x2.ln(), x3.ln(), t).exp() } else { monotone(x0, x1, x2, x3, t) };
            if a.is_i64() || a.is_u64() {
                let r = r.round();
                if a.is_u64() && b.is_u64() { Value::from(r.max(0.0) as u64) } else { Value::from(r as i64) }
            } else {
                serde_json::Number::from_f64(r).map(Value::Number).unwrap_or_else(|| v1.clone())
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            let el = |v: &Value, k: usize| v.as_array().and_then(|x| x.get(k)).cloned().unwrap_or(Value::Null);
            Value::Array((0..a.len()).map(|k| blend(&el(v0, k), &a[k], &b[k], &el(v3, k), t, path)).collect())
        }
        (Value::Object(a), Value::Object(b)) => {
            let mut out = a.clone();
            for (key, va) in a {
                if key == "camera" && path.is_empty() || key == "animation" {
                    continue;
                }
                let Some(vb) = b.get(key) else { continue };
                let p = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                let n0 = v0.get(key).unwrap_or(va);
                let n3 = v3.get(key).unwrap_or(vb);
                out.insert(key.clone(), blend(n0, va, vb, n3, t, &p));
            }
            Value::Object(out)
        }
        _ => v1.clone(),
    }
}

// ---------------------------------------------------------------------------------------------
// Distance to the surface (for perceived speed and keyframe errors)

struct Surface {
    progs: Vec<crate::Program>,
}

impl Surface {
    fn new(scenes: &[&Scene]) -> Surface {
        Surface { progs: scenes.iter().map(|s| crate::Program::for_scene(s)).collect() }
    }

    /// Distance from `p` to the fractal of key `k` (at least a tiny positive value).
    fn distance(&self, k: usize, p: V3) -> f64 {
        let Some(prog) = self.progs.get(k.min(self.progs.len().saturating_sub(1))) else { return 1.0 };
        if prog.inside {
            return 1.0;
        }
        let mut ev = prog.evaluator();
        let h = 1e-7 * (1.0 + p.length());
        let d = ev.de(p, h);
        if d.is_finite() { d.max(1e-9) } else { 1.0 }
    }
}

// ---------------------------------------------------------------------------------------------
// Smoothing helpers

/// Zero-phase Gaussian low-pass of `v` (σ in samples); the ends are held by point reflection.
fn gauss_smooth<T: Copy>(v: &[T], sigma: f64, zero: T, add: impl Fn(T, T, f64) -> T, reflect: impl Fn(T, T) -> T) -> Vec<T> {
    let n = v.len();
    if n < 3 || sigma < 0.3 {
        return v.to_vec();
    }
    let r = (sigma * 3.0).ceil() as isize;
    let w: Vec<f64> = (-r..=r).map(|k| (-(k * k) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    let get = |i: isize| -> T {
        if i < 0 {
            let k = (-i).min(n as isize - 1) as usize;
            reflect(v[0], v[k])
        } else if i >= n as isize {
            let k = (2 * (n as isize - 1) - i).max(0) as usize;
            reflect(v[n - 1], v[k])
        } else {
            v[i as usize]
        }
    };
    (0..n as isize)
        .map(|i| {
            let (mut acc, mut ws) = (zero, 0.0);
            for (j, wk) in w.iter().enumerate() {
                acc = add(acc, get(i + j as isize - r), *wk);
                ws += wk;
            }
            add(zero, acc, 1.0 / ws)
        })
        .collect()
}

fn smooth_positions(p: &[V3], sigma: f64) -> Vec<V3> {
    gauss_smooth(p, sigma, V3::ZERO, |a, b, w| a + b * w, |c, x| c * 2.0 - x)
}

/// Orientations smoothed in the tangent space of their neighbourhood (signs consistent).
fn smooth_rotations(q: &[Quat], sigma: f64) -> Vec<Quat> {
    let q = consistent(q.to_vec());
    let z = Quat { w: 0.0, x: 0.0, y: 0.0, z: 0.0 };
    let reflect = |c: Quat, x: Quat| {
        let x = if c.dot(x) < 0.0 { x.negated() } else { x };
        c.times(Quat::exp(-c.conj().times(x).log()))
    };
    let s = gauss_smooth(&q, sigma, z, |a, b, w| a.plus(b.scale(w)), reflect);
    s.into_iter().map(|x| x.normalize()).collect()
}

// ---------------------------------------------------------------------------------------------
// The plan

/// An animation prepared for rendering: the camera of every frame and the keyframe scenes.
pub struct AnimationPlan {
    values: Vec<Value>,
    keys: Vec<Scene>,
    frames: Vec<FrameCam>,
    fps: f64,
    /// Per frame: how far each palette and glow band has turned (see `color_speeds`).
    phases: Vec<BTreeMap<String, f64>>,
    drivers: Vec<CompiledDriver>,
    /// Sound levels and the sound's position at frame 0 (seconds).
    audio: Option<(crate::audio::AudioLevels, f64)>,
}

/// The palette rotations, glow band speeds, texture drifts, the steam drift and the shimmer
/// speed of a scene by name ("m": the main material, "m.b2": its third band, "m.l1.0": the x
/// drift of its second layer, "o1", "o1.b0": the first object's, "steam0..2", "shimmer").
fn color_speeds(s: &Scene) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    let mut add = |name: String, rotation: f64, bands: &[crate::gradient::GlowBand], layers: &[crate::texture::TextureLayer]| {
        out.insert(name.clone(), rotation);
        for (i, b) in bands.iter().enumerate() {
            out.insert(format!("{name}.b{i}"), b.speed);
        }
        // Texture layers scroll by their drift.
        for (i, l) in layers.iter().enumerate() {
            for (k, d) in l.drift.iter().enumerate() {
                out.insert(format!("{name}.l{i}.{k}"), *d);
            }
        }
    };
    add("m".into(), s.material.rotation, &s.material.bands, &s.material.layers);
    for (k, o) in s.objects.iter().enumerate() {
        add(format!("o{k}"), o.material.rotation, &o.material.bands, &o.material.layers);
    }
    for (k, v) in s.material.steam_drift.iter().enumerate() {
        out.insert(format!("steam{k}"), *v);
    }
    out.insert("shimmer".into(), s.material.shimmer_speed);
    // Water waves move by their speed.
    for (k, o) in s.objects.iter().enumerate() {
        if let crate::compose::Shape::Primitive(crate::compose::Primitive::Water { speed, .. }) = o.shape {
            out.insert(format!("o{k}.water"), speed);
            // The medium's clock (bubbles, particles) runs in seconds.
            out.insert(format!("o{k}.medium"), 1.0);
        }
        // Seaweed sways.
        if let crate::compose::Shape::Primitive(crate::compose::Primitive::Seabed { sway_speed, .. }) = o.shape {
            out.insert(format!("o{k}.seabed"), sway_speed);
        }
        if o.cover.active() {
            out.insert(format!("o{k}.plants"), o.cover.sway_speed);
        }
    }
    // The sky's clock runs by its day speed (hours per second).
    if s.sky.day_speed != 0.0 {
        out.insert("sky.time".into(), s.sky.day_speed);
    }
    // Plants on the main fractal sway.
    if s.plant_cover.active() {
        out.insert("plants".into(), s.plant_cover.sway_speed);
    }
    out
}

/// The phases of every frame: the speeds (linear between the keyframes, by the frames'
/// keyframe positions `us`) summed over the frames before.
fn color_phases(keys: &[Scene], us: &[f64], fps: f64) -> Vec<BTreeMap<String, f64>> {
    let speeds: Vec<BTreeMap<String, f64>> = keys.iter().map(color_speeds).collect();
    let n = keys.len();
    let mut acc: BTreeMap<String, f64> = BTreeMap::new();
    let mut out = Vec::with_capacity(us.len());
    for &u in us {
        out.push(acc.clone());
        let i = (u.floor().max(0.0) as usize).min(n.saturating_sub(2));
        let t = (u - i as f64).clamp(0.0, 1.0);
        for (name, &a) in &speeds[i] {
            let b = speeds.get(i + 1).and_then(|s| s.get(name)).copied().unwrap_or(a);
            *acc.entry(name.clone()).or_insert(0.0) += (a + (b - a) * t) / fps;
        }
    }
    out
}

/// Turns the palettes and moves the glow bands of `s` by `phase`.
fn apply_phases(s: &mut Scene, phase: &BTreeMap<String, f64>) {
    let turn = |name: String, offset: &mut f64, bands: &mut [crate::gradient::GlowBand], layers: &mut [crate::texture::TextureLayer]| {
        *offset += phase.get(&name).copied().unwrap_or(0.0);
        for (i, b) in bands.iter_mut().enumerate() {
            b.pos += phase.get(&format!("{name}.b{i}")).copied().unwrap_or(0.0);
        }
        for (i, l) in layers.iter_mut().enumerate() {
            for (k, o) in l.offset.iter_mut().enumerate() {
                *o += phase.get(&format!("{name}.l{i}.{k}")).copied().unwrap_or(0.0);
            }
        }
    };
    let m = &mut s.material;
    turn("m".into(), &mut m.color_offset, &mut m.bands, &mut m.layers);
    // The steam pattern moves with its drift.
    for (k, o) in s.material.steam_offset.iter_mut().enumerate() {
        *o -= phase.get(&format!("steam{k}")).copied().unwrap_or(0.0);
    }
    s.material.shimmer_phase += phase.get("shimmer").copied().unwrap_or(0.0);
    for (k, o) in s.objects.iter_mut().enumerate() {
        let m = &mut o.material;
        turn(format!("o{k}"), &mut m.color_offset, &mut m.bands, &mut m.layers);
        if let crate::compose::Shape::Primitive(crate::compose::Primitive::Water { phase: p, medium, .. }) = &mut o.shape {
            *p += phase.get(&format!("o{k}.water")).copied().unwrap_or(0.0);
            medium.time += phase.get(&format!("o{k}.medium")).copied().unwrap_or(0.0);
        }
        if let crate::compose::Shape::Primitive(crate::compose::Primitive::Seabed { phase: p, .. }) = &mut o.shape {
            *p += phase.get(&format!("o{k}.seabed")).copied().unwrap_or(0.0);
        }
        o.cover.phase += phase.get(&format!("o{k}.plants")).copied().unwrap_or(0.0);
    }
    s.plant_cover.phase += phase.get("plants").copied().unwrap_or(0.0);
    s.sky.hour = (s.sky.hour + phase.get("sky.time").copied().unwrap_or(0.0)).rem_euclid(24.0);
}

impl AnimationPlan {
    pub fn new(anim: &Animation) -> Result<AnimationPlan, String> {
        if anim.keyframes.is_empty() {
            return Err("the animation has no keyframes".into());
        }
        let keys: Vec<Scene> = anim.keyframes.iter().map(|k| Scene { animation: None, ..k.scene.clone() }).collect();
        let values = keys.iter().map(|s| serde_json::to_value(s).map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?;
        let spline = CamSpline::new(
            keys.iter().map(|s| s.camera.position).collect(),
            keys.iter().map(|s| s.camera.orientation()).collect(),
            keys.iter().map(|s| s.camera.fov).collect(),
        );
        let fps = anim.fps.clamp(1.0, 240.0);
        let us = frame_params(anim, &spline, &keys, fps);
        let mut frames: Vec<FrameCam> = us.iter().map(|u| spline.at(*u)).collect();
        if anim.camera.on {
            cameraman(&mut frames, &anim.camera, fps, &keys);
        }
        let phases = color_phases(&keys, &us, fps);
        Ok(AnimationPlan { values, keys, frames, fps, phases, drivers: compile_drivers(&anim.drivers).0, audio: anim.audio_levels.clone().map(|l| (l, anim.audio_offset_s)) })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    pub fn camera(&self, frame: usize) -> FrameCam {
        self.frames[frame.min(self.frames.len() - 1)]
    }

    /// Positions of all frames (to draw the path).
    pub fn path(&self) -> Vec<V3> {
        self.frames.iter().map(|f| f.pos).collect()
    }

    /// The frame nearest to each keyframe.
    pub fn key_frames(&self) -> Vec<usize> {
        (0..self.keys.len())
            .map(|k| {
                self.frames
                    .iter()
                    .enumerate()
                    .min_by(|a, b| (a.1.u - k as f64).abs().total_cmp(&(b.1.u - k as f64).abs()))
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            })
            .collect()
    }

    /// The scene of frame `frame`.
    pub fn scene_at(&self, frame: usize) -> Scene {
        let f = frame.min(self.frames.len() - 1);
        self.scene_of(self.frames[f], self.phases.get(f), f as f64)
    }

    /// The scene at time `t` in frames, between whole frames (motion blur): camera and
    /// settings follow the path between the neighbouring frames, continued straight beyond the
    /// first and the last frame.
    pub fn scene_at_time(&self, t: f64) -> Scene {
        let n = self.frames.len();
        if n < 2 || (t.fract() == 0.0 && t >= 0.0) {
            return self.scene_at(t.max(0.0) as usize);
        }
        let f = (t.floor().max(0.0) as usize).min(n - 2);
        let a = t - f as f64;
        let (c0, c1) = (self.frames[f], self.frames[f + 1]);
        let fc = FrameCam { u: c0.u + (c1.u - c0.u) * a, pos: c0.pos + (c1.pos - c0.pos) * a, rot: c0.rot.slerp(c1.rot, a), fov: c0.fov + (c1.fov - c0.fov) * a };
        let phase = match (self.phases.get(f), self.phases.get(f + 1)) {
            (Some(p0), Some(p1)) => Some(p0.iter().map(|(k, v0)| (k.clone(), v0 + (p1.get(k).copied().unwrap_or(*v0) - v0) * a)).collect()),
            (p0, _) => p0.cloned(),
        };
        self.scene_of(fc, phase.as_ref(), t)
    }

    /// The scene for a camera on the path and the color phases at that time.
    fn scene_of(&self, fc: FrameCam, phase: Option<&BTreeMap<String, f64>>, t: f64) -> Scene {
        let n = self.keys.len();
        let mut scene = if n == 1 {
            self.keys[0].clone()
        } else {
            let i = (fc.u.floor().max(0.0) as usize).min(n - 2);
            let t = (fc.u - i as f64).clamp(0.0, 1.0);
            if t <= 1e-12 {
                self.keys[i].clone()
            } else if t >= 1.0 - 1e-12 {
                self.keys[i + 1].clone()
            } else {
                let v = |k: isize| &self.values[k.clamp(0, n as isize - 1) as usize];
                let ii = i as isize;
                let b = blend(v(ii - 1), v(ii), v(ii + 1), v(ii + 2), t, "");
                serde_json::from_value(b).unwrap_or_else(|_| self.keys[i].clone())
            }
        };
        scene.camera.position = fc.pos;
        scene.camera.set_orientation(fc.rot);
        scene.camera.fov = fc.fov;
        scene.animation = None;
        if let Some(p) = phase {
            apply_phases(&mut scene, p);
        }
        let audio = self.audio.as_ref().map_or([0.0; 4], |(l, off)| l.at(off + t / self.fps.max(1e-9)));
        apply_drivers(&mut scene, &self.drivers, DriverTime { frame: t, frames: self.frames.len(), fps: self.fps, key: fc.u, audio });
        self.wet_lens(&mut scene, t);
        // The sun's light follows the animated time of day.
        crate::sky::sync_sun(&mut scene);
        scene
    }

    /// Lens drops after the camera left the water (or met spray) on the path before frame
    /// time `t`: raises the medium's `lens_wet` (see [`crate::render::cpu::lens_wetness`]).
    fn wet_lens(&self, scene: &mut Scene, t: f64) {
        let fps = self.fps.max(1e-9);
        let frames = &self.frames;
        let cam_at = |s: f64| {
            let f = (s * fps).clamp(0.0, (frames.len() - 1) as f64);
            let i = (f.floor() as usize).min(frames.len().saturating_sub(2));
            match frames.get(i + 1) {
                Some(b) => frames[i].pos + (b.pos - frames[i].pos) * (f - i as f64),
                None => frames[i].pos,
            }
        };
        let wet = crate::render::cpu::lens_wetness(scene, t / fps, &cam_at);
        if wet <= 0.0 {
            return;
        }
        for o in &mut scene.objects {
            if let crate::compose::Shape::Primitive(crate::compose::Primitive::Water { medium, .. }) = &mut o.shape
                && medium.see_through
            {
                medium.lens_wet = medium.lens_wet.max(wet);
                break;
            }
        }
    }
}

/// Keyframe-space position of every frame for the animation's timing.
fn frame_params(anim: &Animation, spline: &CamSpline, keys: &[Scene], fps: f64) -> Vec<f64> {
    let n = spline.keys();
    if n == 1 {
        let frames = match anim.timing {
            Timing::Steady => (anim.length_s * fps).round().max(1.0) as usize,
            _ => anim.keyframes[0].frames.max(1) as usize,
        };
        return vec![0.0; frames];
    }
    match anim.timing {
        Timing::Keyframes => {
            let mut us = Vec::new();
            for (i, k) in anim.keyframes.iter().take(n - 1).enumerate() {
                let f = k.frames.max(1);
                for j in 0..f {
                    us.push(i as f64 + k.easing.apply(j as f64 / f as f64));
                }
            }
            us.push((n - 1) as f64);
            us
        }
        Timing::Steady => {
            let (u, s) = perceived_length(anim, spline, keys);
            let total = *s.last().unwrap_or(&0.0);
            let frames = (anim.length_s * fps).round().max(2.0) as usize;
            let len = (frames - 1) as f64 / fps;
            let prof = ease_profile(frames, len, anim.ease_in_s, anim.ease_out_s, fps);
            prof.iter().map(|p| invert(&u, &s, p * total)).collect()
        }
        Timing::Recorded => {
            // Key times: recorded, else from the frame counts.
            let mut times = Vec::with_capacity(n);
            let mut acc = 0.0;
            for (i, k) in anim.keyframes.iter().enumerate() {
                let t = k.time_s.unwrap_or(acc);
                times.push(if i > 0 { t.max(times[i - 1] + 1e-3) } else { t });
                acc = times[i] + k.frames.max(1) as f64 / fps;
            }
            let t0 = times[0];
            let duration = times[n - 1] - t0;
            let (u, s) = perceived_length(anim, spline, keys);
            let s_key: Vec<f64> = (0..n).map(|k| s[u.iter().position(|x| *x >= k as f64 - 1e-9).unwrap_or(u.len() - 1)]).collect();
            let frames = (duration * fps).round().max(1.0) as usize + 1;
            // Path position per frame as flown (even within a segment), then a smoothed speed.
            let flown: Vec<f64> = (0..frames)
                .map(|f| {
                    let t = t0 + f as f64 / fps;
                    let k = times.iter().rposition(|x| *x <= t).unwrap_or(0).min(n - 2);
                    let a = ((t - times[k]) / (times[k + 1] - times[k])).clamp(0.0, 1.0);
                    s_key[k] + (s_key[k + 1] - s_key[k]) * a
                })
                .collect();
            let mut vel: Vec<f64> = flown.windows(2).map(|w| w[1] - w[0]).collect();
            let sigma = anim.speed_smoothing_s * fps;
            if sigma > 0.3 && !vel.is_empty() {
                // Speeds are smoothed with even reflection (the speed itself is held at the ends).
                vel = gauss_smooth(&vel, sigma, 0.0, |a, b, w| a + b * w, |_, x| x);
            }
            let total: f64 = vel.iter().sum();
            let target = *s.last().unwrap_or(&0.0);
            let k = if total > 0.0 { target / total } else { 0.0 };
            let mut pos = 0.0;
            let mut us = vec![0.0];
            for v in vel {
                pos += v * k;
                us.push(invert(&u, &s, pos));
            }
            us
        }
    }
}

/// Dense samples of the path: keyframe-space positions and their cumulative perceived length.
fn perceived_length(anim: &Animation, spline: &CamSpline, keys: &[Scene]) -> (Vec<f64>, Vec<f64>) {
    const PER_KEY: usize = 96;
    let n = spline.keys();
    let count = (n - 1) * PER_KEY + 1;
    let us: Vec<f64> = (0..count).map(|i| i as f64 / PER_KEY as f64).collect();
    let cams: Vec<FrameCam> = us.iter().map(|u| spline.at(*u)).collect();
    let surf = if anim.surface_relative { Some(Surface::new(&keys.iter().collect::<Vec<_>>())) } else { None };
    let world: f64 = cams.windows(2).map(|w| (w[1].pos - w[0].pos).length()).sum::<f64>().max(1e-12);
    let dist: Vec<f64> = match &surf {
        Some(s) => smooth_scalar(&cams.iter().map(|c| s.distance(c.u.floor() as usize, c.pos)).collect::<Vec<_>>(), 3.0),
        None => vec![world; cams.len()],
    };
    let mut s = vec![0.0];
    for i in 1..cams.len() {
        let d = 0.5 * (dist[i] + dist[i - 1]);
        let mv = (cams[i].pos - cams[i - 1].pos).length() / d;
        let turn = cams[i].rot.angle_to(cams[i - 1].rot);
        s.push(s[i - 1] + (mv * mv + turn * turn).sqrt() + 1e-9);
    }
    (us, s)
}

fn smooth_scalar(v: &[f64], sigma: f64) -> Vec<f64> {
    gauss_smooth(v, sigma, 0.0, |a, b, w| a + b * w, |_, x| x)
}

/// Keyframe-space position where the cumulative length `s` reaches `target`.
fn invert(u: &[f64], s: &[f64], target: f64) -> f64 {
    let i = s.partition_point(|x| *x < target);
    if i == 0 {
        return u[0];
    }
    if i >= s.len() {
        return *u.last().unwrap_or(&0.0);
    }
    let a = (target - s[i - 1]) / (s[i] - s[i - 1]).max(1e-300);
    u[i - 1] + (u[i] - u[i - 1]) * a
}

/// Fraction of the way (0..1) per frame for a speed that ramps up over `ein` seconds and down
/// over `eout` seconds (smooth ramps), constant in between.
fn ease_profile(frames: usize, len: f64, ein: f64, eout: f64, fps: f64) -> Vec<f64> {
    let total = len.max(1e-9);
    let (ein, eout) = (ein.clamp(0.0, total), eout.clamp(0.0, total));
    let (ein, eout) = if ein + eout > total { (ein * total / (ein + eout), eout * total / (ein + eout)) } else { (ein, eout) };
    let speed = |t: f64| {
        let up = if ein > 0.0 && t < ein { let x = t / ein; x * x * (3.0 - 2.0 * x) } else { 1.0 };
        let down = if eout > 0.0 && t > total - eout { let x = (total - t) / eout; x * x * (3.0 - 2.0 * x) } else { 1.0 };
        up.min(down)
    };
    // Integrate the speed finely, then normalize.
    let steps = (frames - 1).max(1) * 16;
    let dt = total / steps as f64;
    let mut cum = vec![0.0];
    for i in 0..steps {
        let t = (i as f64 + 0.5) * dt;
        cum.push(cum[i] + speed(t) * dt);
    }
    let all = cum[steps].max(1e-300);
    (0..frames).map(|f| cum[((f as f64 / fps / dt).round() as usize).min(steps)] / all).collect()
}

/// The cameraman pass on the frame cameras.
fn cameraman(frames: &mut [FrameCam], c: &CameraSmoothing, fps: f64, keys: &[Scene]) {
    let n = frames.len();
    if n < 3 {
        return;
    }
    let pos = smooth_positions(&frames.iter().map(|f| f.pos).collect::<Vec<_>>(), c.position_s * fps);
    let rot = smooth_rotations(&frames.iter().map(|f| f.rot).collect::<Vec<_>>(), c.rotation_s * fps);
    // Reference "up" for the horizon: the mean up axis of the keyframes.
    let mut world_up = V3::ZERO;
    for k in keys {
        world_up += k.camera.up;
    }
    let world_up = world_up.normalize();
    // Velocity (smoothed positions), for looking ahead and banking.
    let vel: Vec<V3> = (0..n).map(|i| pos[(i + 1).min(n - 1)] - pos[i.saturating_sub(1)]).collect();
    let mut out: Vec<Quat> = Vec::with_capacity(n);
    for i in 0..n {
        let mut q = rot[i];
        let (r, f, u) = q.basis();
        if c.look_ahead > 0.0 && vel[i].length() > 1e-12 {
            let fa = vel[i].normalize();
            let ra = fa.cross(u);
            if ra.length() > 1e-6 {
                let ra = ra.normalize();
                q = q.slerp(Quat::from_basis(ra, fa, ra.cross(fa).normalize()), c.look_ahead.clamp(0.0, 1.0));
            }
        }
        if c.level_horizon > 0.0 {
            let (_, f, _) = q.basis();
            let rl = f.cross(world_up);
            if rl.length() > 1e-6 {
                let rl = rl.normalize();
                q = q.slerp(Quat::from_basis(rl, f, rl.cross(f).normalize()), c.level_horizon.clamp(0.0, 1.0));
            }
        }
        let _ = (r, f);
        out.push(q);
    }
    if c.bank > 0.0 {
        // Yaw rate about the world up axis (radians per second), smoothed.
        let heading = |q: Quat| {
            let (_, f, _) = q.basis();
            let h = f - world_up * f.dot(world_up);
            h.normalize()
        };
        let mut yaw: Vec<f64> = (0..n)
            .map(|i| {
                let (a, b) = (heading(out[i.saturating_sub(1)]), heading(out[(i + 1).min(n - 1)]));
                let s = world_up.dot(a.cross(b));
                let steps = ((i + 1).min(n - 1) - i.saturating_sub(1)).max(1) as f64;
                s.atan2(a.dot(b)) * fps / steps
            })
            .collect();
        yaw = smooth_scalar(&yaw, 0.25 * fps);
        for i in 0..n {
            // Turning left (positive yaw) rolls left; about 20° at 1 rad/s with full banking.
            let roll = (-yaw[i] * 0.35 * c.bank).clamp(-0.7, 0.7);
            let (_, f, _) = out[i].basis();
            out[i] = Quat::from_axis_angle(f, roll).times(out[i]).normalize();
        }
    }
    for (i, fr) in frames.iter_mut().enumerate() {
        fr.pos = pos[i];
        fr.rot = out[i];
    }
}

// ---------------------------------------------------------------------------------------------
// Flight recording → keyframes

/// Creates keyframes from a recorded flight: as few as needed so that the camera path through
/// them stays within `tolerance` of the flight (position error relative to the distance to
/// the surface, plus orientation error in radians). Keyframe scenes are `base` with the
/// recorded camera.
pub fn keyframes_from_flight(samples: &[CamSample], tolerance: f64, base: &Scene, fps: f64) -> Vec<Keyframe> {
    if samples.is_empty() {
        return Vec::new();
    }
    let t0 = samples[0].t;
    let surf = Surface::new(&[base]);
    let dist: Vec<f64> = samples.iter().map(|s| surf.distance(0, s.pos)).collect();
    let rot = consistent(samples.iter().map(|s| s.rot).collect());
    let mut keys = vec![0usize];
    if samples.len() > 1 {
        keys.push(samples.len() - 1);
    }
    let tol = tolerance.max(1e-6);
    loop {
        let spline = CamSpline::new(keys.iter().map(|&k| samples[k].pos).collect(), keys.iter().map(|&k| rot[k]).collect(), keys.iter().map(|&k| samples[k].fov).collect());
        // Geometric error: distance to the nearest point of the segment's path and the
        // orientation there (the timing is the job of the animation's timing modes).
        const STEPS: usize = 48;
        let mut worst = (0.0, 0usize);
        for seg in 0..keys.len().saturating_sub(1) {
            let (a, b) = (keys[seg], keys[seg + 1]);
            let path: Vec<FrameCam> = (0..=STEPS).map(|j| spline.at(seg as f64 + j as f64 / STEPS as f64)).collect();
            for (i, s) in samples.iter().enumerate().take(b).skip(a + 1) {
                let (j, _) = path.iter().enumerate().map(|(j, c)| (j, (c.pos - s.pos).length())).min_by(|x, y| x.1.total_cmp(&y.1)).unwrap_or((0, 0.0));
                // Refine on the two neighbouring pieces of the polyline.
                let mut best = (path[j].pos, path[j].rot, (path[j].pos - s.pos).length());
                for k in [j.saturating_sub(1), j] {
                    if k + 1 > STEPS {
                        continue;
                    }
                    let (p, q) = (path[k].pos, path[k + 1].pos);
                    let d = q - p;
                    let f = ((s.pos - p).dot(d) / d.dot(d).max(1e-300)).clamp(0.0, 1.0);
                    let c = p + d * f;
                    let e = (c - s.pos).length();
                    if e < best.2 {
                        best = (c, path[k].rot.slerp(path[k + 1].rot, f), e);
                    }
                }
                let e = best.2 / dist[i] + best.1.angle_to(rot[i]);
                if e > worst.0 {
                    worst = (e, i);
                }
            }
        }
        if worst.0 <= tol || keys.len() >= 500 {
            break;
        }
        let pos = keys.partition_point(|&k| k < worst.1);
        keys.insert(pos, worst.1);
    }
    keys.iter()
        .enumerate()
        .map(|(j, &k)| {
            let s = &samples[k];
            let mut scene = Scene { animation: None, ..base.clone() };
            scene.camera.position = s.pos;
            scene.camera.set_orientation(s.rot);
            scene.camera.fov = s.fov;
            let next = keys.get(j + 1).map(|&n| samples[n].t).unwrap_or(s.t);
            Keyframe { scene, frames: ((next - s.t) * fps).round().max(1.0) as u32, easing: Easing::Linear, time_s: Some(s.t - t0) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_drivers_follow_the_sound_levels() {
        let mut anim = Animation::default();
        for _ in 0..2 {
            anim.keyframes.push(Keyframe { scene: Scene::default(), frames: 30, ..Default::default() });
        }
        // Bass on during the first half second only (30 fps: frames 0-14).
        let levels: Vec<[f32; 4]> = (0..100).map(|i| if i < 25 { [1.0, 1.0, 0.0, 0.0] } else { [0.0; 4] }).collect();
        anim.audio_levels = Some(crate::audio::AudioLevels { source: "x".into(), rate: crate::audio::RATE, levels });
        anim.drivers = vec![Driver { path: "fractal.slots.0.params.power".into(), expression: "value + 2 * bass".into(), ..Default::default() }];
        assert!(anim.drivers_use_audio());
        let plan = AnimationPlan::new(&anim).unwrap();
        let p = |f: usize| plan.scene_at(f).fractal.slots[0].params["power"];
        assert!((p(5) - 10.0).abs() < 1e-6 && (p(20) - 8.0).abs() < 1e-6, "{} {}", p(5), p(20));
        // The sound offset shifts it: frame 0 now is 1 s into the sound (silent).
        anim.audio_offset_s = 1.0;
        let plan = AnimationPlan::new(&anim).unwrap();
        assert!((plan.scene_at(5).fractal.slots[0].params["power"] - 8.0).abs() < 1e-6);
        assert!(!uses_name("treble_x + abass", "bass") && uses_name("1 + bass*2", "bass"));
    }

    #[test]
    fn drivers_change_values_per_frame() {
        let mut anim = Animation::default();
        for _ in 0..2 {
            anim.keyframes.push(Keyframe { scene: Scene::default(), frames: 10, ..Default::default() });
        }
        anim.drivers = vec![
            Driver { path: "camera.fov".into(), expression: "value + frame".into(), ..Default::default() },
            // A formula parameter at its default (not in the scene's JSON).
            Driver { path: "fractal.slots.0.params.power".into(), expression: "value + 0.5 * u".into(), ..Default::default() },
            // Reads another driven value; integers stay integers.
            Driver { path: "fractal.max_iterations".into(), expression: "@camera.fov / 4".into(), ..Default::default() },
            Driver { path: "camera.fov".into(), expression: "1 +".into(), enabled: false },
        ];
        let plan = AnimationPlan::new(&anim).unwrap();
        let (a, b) = (plan.scene_at(0), plan.scene_at(4));
        let fov0 = Scene::default().camera.fov;
        assert!((a.camera.fov - fov0).abs() < 1e-9 && (b.camera.fov - fov0 - 4.0).abs() < 1e-9, "{} {}", a.camera.fov, b.camera.fov);
        let last = plan.frame_count() - 1;
        let power = plan.scene_at(last).fractal.slots[0].params["power"];
        assert!((power - 8.5).abs() < 1e-9, "{power}");
        assert_eq!(b.fractal.max_iterations, ((fov0 + 4.0) / 4.0).round() as u32);
        let (_, errors) = compile_drivers(&[Driver { path: "x".into(), expression: "1 +".into(), ..Default::default() }]);
        assert_eq!(errors.len(), 1);
        let mut s = Scene::default();
        let e = apply_drivers(&mut s, &compile_drivers(&[Driver { path: "no.such".into(), expression: "1".into(), ..Default::default() }]).0, DriverTime { frame: 0.0, frames: 1, fps: 30.0, key: 0.0, audio: [0.0; 4] });
        assert!(e[0].contains("no such value"), "{e:?}");
        assert!(drivable_values(&Scene::default()).iter().any(|(p, _)| p == "fractal.slots.0.params.power"));
    }

    #[test]
    fn water_waves_move_by_their_speed() {
        use crate::compose::{Primitive, SceneObject, Shape};
        let mut s = Scene::default();
        s.objects.push(SceneObject { shape: Shape::Primitive(Primitive::water()), ..Default::default() });
        let Shape::Primitive(Primitive::Water { speed, .. }) = s.objects[0].shape else { panic!() };
        let anim = Animation { fps: 25.0, keyframes: vec![Keyframe { scene: s.clone(), frames: 50, ..Default::default() }, Keyframe { scene: s, frames: 50, ..Default::default() }], ..Default::default() };
        let plan = AnimationPlan::new(&anim).expect("plan");
        let phase = |f: usize| match plan.scene_at(f).objects[0].shape {
            Shape::Primitive(Primitive::Water { phase, .. }) => phase,
            _ => f64::NAN,
        };
        assert!((phase(25) - phase(0) - speed).abs() < 1e-9, "{} {}", phase(0), phase(25));
        // The medium's clock (bubbles, particles) runs in seconds.
        let time = |f: usize| match plan.scene_at(f).objects[0].shape {
            Shape::Primitive(Primitive::Water { medium, .. }) => medium.time,
            _ => f64::NAN,
        };
        assert!((time(50) - time(0) - 2.0).abs() < 1e-9, "{} {}", time(0), time(50));
    }

    #[test]
    fn still_drivers_apply_once_and_only_when_switched_on() {
        let mut s = Scene::default();
        let fov0 = s.camera.fov;
        let mut a = Animation { drivers: vec![Driver { path: "camera.fov".into(), expression: "value + 5".into(), ..Default::default() }], ..Default::default() };
        s.animation = Some(a.clone());
        assert!(still_scene(&s).is_none(), "off by default");
        a.still_drivers = true;
        s.animation = Some(a);
        let d = still_scene(&s).expect("driven");
        assert!((d.camera.fov - fov0 - 5.0).abs() < 1e-9);
        assert!(still_scene(&d).is_none(), "driven twice");
    }
    use crate::math::v3;

    fn key(pos: V3, target: V3) -> Keyframe {
        let mut s = Scene::default();
        s.camera.position = pos;
        s.camera.look_at(target, v3(0.0, 0.0, 1.0));
        Keyframe { scene: s, frames: 30, ..Default::default() }
    }

    fn orbit_keys(n: usize) -> Vec<Keyframe> {
        (0..n)
            .map(|i| {
                let a = i as f64 * 0.7;
                key(v3(3.0 * a.cos(), 3.0 * a.sin(), 0.5 + 0.2 * i as f64), V3::ZERO)
            })
            .collect()
    }

    #[test]
    fn quat_round_trips_camera_basis() {
        let s = key(v3(1.0, -2.0, 0.7), v3(0.2, 0.1, -0.3)).scene;
        let q = s.camera.orientation();
        let mut c = s.camera.clone();
        c.set_orientation(q);
        for (a, b) in [(c.right, s.camera.right), (c.up, s.camera.up), (c.forward, s.camera.forward)] {
            assert!((a - b).length() < 1e-9);
        }
    }

    #[test]
    fn palettes_turn_and_bands_wander() {
        let mut keys = orbit_keys(2);
        for k in &mut keys {
            k.scene.material.rotation = 0.5;
            k.scene.material.bands = vec![crate::gradient::GlowBand { pos: 0.2, speed: -0.25, ..Default::default() }];
        }
        // The second key turns twice as fast: the speed blends linearly.
        keys[1].scene.material.rotation = 1.0;
        let anim = Animation { keyframes: keys, ..Default::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        let fps = plan.fps();
        let last = plan.frame_count() - 1;
        let s0 = plan.scene_at(0);
        assert_eq!(s0.material.color_offset, 0.0);
        assert_eq!(s0.material.bands[0].pos, 0.2);
        let s = plan.scene_at(last);
        let secs = last as f64 / fps;
        // Average speed 0.75 turns per second over the whole time.
        assert!((s.material.color_offset - 0.75 * secs).abs() < 0.02, "{}", s.material.color_offset);
        assert!((s.material.bands[0].pos - (0.2 - 0.25 * secs)).abs() < 1e-9);
    }

    #[test]
    fn texture_layers_drift_and_sub_frame_times_blend() {
        let mut keys = orbit_keys(2);
        for k in &mut keys {
            k.scene.material.layers = vec![crate::texture::TextureLayer { offset: [0.5, 0.0, 0.0], drift: [0.0, 2.0, 0.0], ..Default::default() }];
        }
        let anim = Animation { keyframes: keys, ..Default::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        let fps = plan.fps();
        let last = plan.frame_count() - 1;
        let s = plan.scene_at(last);
        let secs = last as f64 / fps;
        assert_eq!(s.material.layers[0].offset[0], 0.5);
        assert!((s.material.layers[0].offset[1] - 2.0 * secs).abs() < 1e-9);
        // Between two frames: halfway in the drift and on the camera path.
        let (a, b, m) = (plan.scene_at(3), plan.scene_at(4), plan.scene_at_time(3.5));
        assert!((m.material.layers[0].offset[1] - 0.5 * (a.material.layers[0].offset[1] + b.material.layers[0].offset[1])).abs() < 1e-9);
        let mid = (a.camera.position + b.camera.position) * 0.5;
        assert!((m.camera.position - mid).length() < 1e-9);
        assert_eq!(plan.scene_at_time(3.0), a);
    }

    #[test]
    fn shimmer_moves_on_in_time() {
        let mut keys = orbit_keys(2);
        for k in &mut keys {
            k.scene.material.shimmer = 0.5;
            k.scene.material.shimmer_speed = 4.0;
        }
        let anim = Animation { keyframes: keys, ..Default::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        let last = plan.frame_count() - 1;
        let secs = last as f64 / plan.fps();
        assert_eq!(plan.scene_at(0).material.shimmer_phase, 0.0);
        assert!((plan.scene_at(last).material.shimmer_phase - 4.0 * secs).abs() < 1e-9);
        // The factor changes smoothly with the phase and stays around 1.
        let mut m = plan.scene_at(0).material;
        let p = crate::v3(0.3, 0.1, 0.2);
        let a = crate::render::shimmer(&m, p);
        m.shimmer_phase = 0.01;
        let b = crate::render::shimmer(&m, p);
        assert!((a - b).abs() < 0.05 && a > 0.0 && a < 2.0);
    }

    #[test]
    fn path_passes_through_keyframes() {
        let anim = Animation { keyframes: orbit_keys(4), ..Default::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        assert_eq!(plan.frame_count(), 3 * 30 + 1);
        for (k, f) in plan.key_frames().into_iter().enumerate() {
            let c = plan.camera(f);
            assert!((c.pos - anim.keyframes[k].scene.camera.position).length() < 1e-9, "key {k}");
            assert!(c.rot.angle_to(anim.keyframes[k].scene.camera.orientation()) < 1e-6);
        }
    }

    #[test]
    fn orientation_has_no_flips() {
        let anim = Animation { keyframes: orbit_keys(6), ..Default::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        let max_step = (1..plan.frame_count()).map(|i| plan.camera(i).rot.angle_to(plan.camera(i - 1).rot)).fold(0.0, f64::max);
        assert!(max_step < 0.1, "{max_step}");
    }

    #[test]
    fn steady_timing_keeps_the_pace() {
        // Keyframes at very uneven spacing: the frames must still move evenly.
        let mut keys = vec![key(v3(3.0, 0.0, 0.0), V3::ZERO), key(v3(3.0, 0.3, 0.0), V3::ZERO), key(v3(3.0, 4.0, 0.0), V3::ZERO), key(v3(3.0, 4.2, 0.0), V3::ZERO)];
        for k in &mut keys {
            k.scene.camera.forward = v3(-1.0, 0.0, 0.0);
            k.scene.camera.right = v3(0.0, 1.0, 0.0);
            k.scene.camera.up = v3(0.0, 0.0, 1.0);
        }
        let anim = Animation { keyframes: keys, timing: Timing::Steady, length_s: 4.0, ease_in_s: 0.0, ease_out_s: 0.0, surface_relative: false, ..Default::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        let steps: Vec<f64> = (1..plan.frame_count()).map(|i| (plan.camera(i).pos - plan.camera(i - 1).pos).length()).collect();
        let mean = steps.iter().sum::<f64>() / steps.len() as f64;
        let spread = steps.iter().map(|s| (s - mean).abs()).fold(0.0, f64::max) / mean;
        assert!(spread < 0.05, "spread {spread}");
    }

    /// A jittery flight: a smooth curve plus keyboard-like steps and hand shake.
    fn jittery_flight() -> Vec<CamSample> {
        let base = Scene::default();
        (0..600)
            .map(|i| {
                let t = i as f64 / 60.0;
                let jitter = if (i / 7) % 2 == 0 { 0.02 } else { -0.02 };
                let mut c = base.camera.clone();
                c.position = v3(3.0 * (0.3 * t).cos(), 3.0 * (0.3 * t).sin(), 1.0 + jitter);
                c.look_at(v3(jitter * 3.0, 0.0, 0.0), v3(0.0, 0.0, 1.0));
                CamSample { t, pos: c.position, rot: c.orientation(), fov: c.fov }
            })
            .collect()
    }

    fn jerk(pos: &[V3]) -> f64 {
        pos.windows(4).map(|w| (w[3] - w[2] * 3.0 + w[1] * 3.0 - w[0]).length()).sum()
    }

    #[test]
    fn cameraman_removes_jitter_and_keeps_the_ends() {
        let flight = jittery_flight();
        let keys: Vec<Keyframe> = flight
            .iter()
            .step_by(5)
            .map(|s| {
                let mut sc = Scene::default();
                sc.camera.position = s.pos;
                sc.camera.set_orientation(s.rot);
                Keyframe { scene: sc, frames: 5, time_s: Some(s.t), ..Default::default() }
            })
            .collect();
        let mut anim = Animation { keyframes: keys, timing: Timing::Recorded, speed_smoothing_s: 0.0, fps: 60.0, ..Default::default() };
        let raw = AnimationPlan::new(&anim).unwrap().path();
        anim.camera.on = true;
        let plan = AnimationPlan::new(&anim).unwrap();
        let smooth = plan.path();
        assert_eq!(raw.len(), smooth.len());
        assert!(jerk(&smooth) < jerk(&raw) * 0.1, "{} vs {}", jerk(&smooth), jerk(&raw));
        assert!((smooth[0] - raw[0]).length() < 1e-9 && (smooth[smooth.len() - 1] - raw[raw.len() - 1]).length() < 1e-9);
    }

    #[test]
    fn recorded_speed_is_smoothed() {
        // Keys at even times but uneven distances: as flown the speed jumps per segment; the
        // smoothed speed must change gradually.
        let mut keys = Vec::new();
        let xs = [0.0, 0.2, 2.0, 2.2, 4.0];
        for (i, x) in xs.iter().enumerate() {
            let mut k = key(v3(*x, -5.0, 0.0), v3(*x, 0.0, 0.0));
            k.time_s = Some(i as f64);
            keys.push(k);
        }
        let anim = Animation { keyframes: keys, timing: Timing::Recorded, speed_smoothing_s: 1.5, surface_relative: false, ..Default::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        let v: Vec<f64> = (1..plan.frame_count()).map(|i| (plan.camera(i).pos - plan.camera(i - 1).pos).length()).collect();
        let max_change = v.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f64::max);
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        assert!(max_change < mean * 0.1, "speed jumps by {max_change} (mean {mean})");
    }

    #[test]
    fn flight_becomes_few_keyframes_within_tolerance() {
        let flight: Vec<CamSample> = (0..300)
            .map(|i| {
                let t = i as f64 / 60.0;
                let mut c = Scene::default().camera;
                c.position = v3(3.0 * (0.4 * t).cos(), 3.0 * (0.4 * t).sin(), 1.0);
                c.look_at(V3::ZERO, v3(0.0, 0.0, 1.0));
                CamSample { t, pos: c.position, rot: c.orientation(), fov: c.fov }
            })
            .collect();
        let keys = keyframes_from_flight(&flight, 0.01, &Scene::default(), 30.0);
        assert!(keys.len() >= 2 && keys.len() < 20, "{} keys", keys.len());
        assert_eq!(keys[0].time_s, Some(0.0));
        let anim = Animation { keyframes: keys, timing: Timing::Recorded, speed_smoothing_s: 0.0, fps: 60.0, ..Default::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        assert_eq!(plan.frame_count(), 300);
        // Every recorded position lies close to the path.
        let surf = Surface::new(&[&Scene::default()]);
        let path = plan.path();
        for s in &flight {
            let d = path.iter().map(|p| (*p - s.pos).length()).fold(f64::MAX, f64::min);
            assert!(d / surf.distance(0, s.pos) < 0.02, "{d}");
        }
    }

    #[test]
    fn other_parameters_blend() {
        let mut a = key(v3(3.0, 0.0, 0.0), V3::ZERO);
        let mut b = key(v3(0.0, 3.0, 0.0), V3::ZERO);
        a.scene.lighting.ambient = 0.2;
        b.scene.lighting.ambient = 0.6;
        a.scene.fractal.max_iterations = 10;
        b.scene.fractal.max_iterations = 20;
        a.scene.fractal.inside = false;
        b.scene.fractal.inside = true;
        a.frames = 10;
        let anim = Animation { keyframes: vec![a, b], ..Default::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        let mid = plan.scene_at(5);
        assert!((mid.lighting.ambient - 0.4).abs() < 1e-9);
        assert_eq!(mid.fractal.max_iterations, 15);
        assert!(!mid.fractal.inside);
        assert!(plan.scene_at(10).fractal.inside);
    }
}
