// SPDX-License-Identifier: LGPL-2.1-or-later

//! CPU renderer in f64: the reference for the GPU kernel and the fallback without a GPU.

use rayon::prelude::*;

use super::{Frame, Rng, post_process, shading_scene};
use crate::fractal::{Evaluator, Orbit, Program};
use crate::math::{V3, v3};
use crate::scene::{AoMode, BackgroundMode, ColorMode, Scene};

/// Shading inputs that do not change per pixel.
pub struct Shader<'a> {
    pub scene: &'a Scene,
    pub prog: &'a Program,
    pub frame: Frame,
    pub lut: Vec<[f32; 3]>,
    pub lut_spec: Vec<[f32; 3]>,
    pub bg_image: Option<(std::sync::Arc<crate::maps::Map>, [[f64; 3]; 3])>,
}

pub struct Hit {
    pub t: f64,
    pub pos: V3,
    pub eps: f64,
    pub orbit: Orbit,
    pub steps: u32,
}

impl<'a> Shader<'a> {
    pub fn new(scene: &'a Scene, prog: &'a Program, width: u32, height: u32) -> Self {
        let g = &scene.material.gradient;
        Shader {
            scene,
            prog,
            frame: Frame::new(scene, width, height),
            lut: crate::gradient::lut(g),
            lut_spec: crate::gradient::lut_specular(g),
            bg_image: super::background_image(scene),
        }
    }

    /// Surface threshold at ray distance `t`.
    pub fn eps_at(&self, t: f64) -> f64 {
        (self.scene.march.detail * self.frame.pixel_angle * t).max(1.0e-7)
    }

    /// Marches a ray. Returns the hit (if any), the number of steps taken and the number of
    /// glowing steps (see `Fog::glow_iteration`).
    pub fn march(&self, ev: &mut Evaluator, o: V3, d: V3, t0: f64, max_t: f64, max_steps: u32) -> (Option<Hit>, u32, u32) {
        let m = &self.scene.march;
        let glow_it = self.scene.fog.glow_iteration;
        let mut t = t0;
        let mut last_step = 0.0;
        let mut glow = 0;
        for step in 0..max_steps {
            let p = o + d * t;
            let eps = self.eps_at(t);
            let orbit = ev.orbit(p, eps * 0.5);
            if glow_it == 0 || orbit.iterations == glow_it {
                glow += 1;
            }
            if orbit.de < eps {
                // Binary refinement toward the threshold.
                let (mut t_hit, mut orbit_hit) = (t, orbit);
                let mut dt = last_step * 0.5;
                if step > 0 {
                    for _ in 0..m.refine_steps {
                        let tt = t_hit - dt;
                        let ob = ev.orbit(o + d * tt, self.eps_at(tt) * 0.5);
                        if ob.de < self.eps_at(tt) {
                            t_hit = tt;
                            orbit_hit = ob;
                        }
                        dt *= 0.5;
                    }
                }
                return (Some(Hit { t: t_hit, pos: o + d * t_hit, eps: self.eps_at(t_hit), orbit: orbit_hit, steps: step }), step, glow.min(step));
            }
            // At least a tenth of the threshold: no stalling where the estimate is near zero.
            last_step = (orbit.de * m.step).max(eps * 0.1);
            t += last_step;
            if t > max_t {
                return (None, step, glow.min(step));
            }
        }
        (None, max_steps, glow)
    }

    pub fn normal(&self, ev: &mut Evaluator, p: V3, eps: f64) -> V3 {
        let h = eps * 0.5;
        let k = [v3(1.0, -1.0, -1.0), v3(-1.0, -1.0, 1.0), v3(-1.0, 1.0, -1.0), v3(1.0, 1.0, 1.0)];
        let mut n = V3::ZERO;
        for kk in k {
            n += kk * ev.de(p + kk * h, h);
        }
        n.normalize()
    }

    /// Normal and roughness at a hit at distance `t`. With smoothed normals (Mandelbulb3D's
    /// RMCalculateNormals) the estimate is sampled along the surface: the mean slopes bend the
    /// normal, their spread is the roughness (0..1). Without smoothing the roughness is 0.
    pub fn normal_rough(&self, ev: &mut Evaluator, p: V3, eps: f64, t: f64) -> (V3, f64) {
        let h = eps * 0.5;
        let k = [v3(1.0, -1.0, -1.0), v3(-1.0, -1.0, 1.0), v3(-1.0, 1.0, -1.0), v3(1.0, 1.0, 1.0)];
        let mut g = V3::ZERO;
        for kk in k {
            g += kk * ev.de(p + kk * h, h);
        }
        let s = self.scene.march.smooth_normals.min(8) as i64;
        if s == 0 {
            return (g.normalize(), 0.0);
        }
        let cam = &self.scene.camera;
        // Mandelbulb3D's normal offset (0.15 of the threshold, at most 0.15 pixels) and its
        // normal: the gradient times that offset (the tetrahedron sum is 4 h times the gradient).
        let no = 0.15 * self.scene.march.detail.min(1.0) * self.frame.pixel_angle * t;
        let n0 = g * (no / (4.0 * h));
        let nrm = g.normalize();
        let no2 = no * 2.0;
        let dnn = (ev.de(p, h) + ev.de(p - cam.right * no2, h) + ev.de(p + cam.right * no2, h) + ev.de(p - cam.up * no2, h) + ev.de(p + cam.up * no2, h)) * 0.2;
        let step = no2 * 3.0 / (s as f64 + 0.5);
        let tx = cam.right - nrm * cam.right.dot(nrm);
        let tx = if tx.length() > 1e-9 { tx.normalize() } else { cam.up };
        let ty = nrm.cross(tx);
        let mut slopes = |axis: V3| {
            let (mut sum, mut sq) = (0.0, 0.0);
            for i in -s..=s {
                if i != 0 {
                    let d = (ev.de(p + axis * (i as f64 * step), h) - dnn) / i as f64;
                    sum += d;
                    sq += d * d;
                }
            }
            (sum, sq)
        };
        let ((n1, s1), (n2, s2)) = (slopes(tx), slopes(ty));
        let dm = 2.0 * s as f64;
        let spread = s1 * dm - n1 * n1 + s2 * dm - n2 * n2;
        let dt2 = no2 * 0.5 / (dm * step);
        let rough = ((spread * 7.0 * dt2 * dt2 / (n0.dot(n0) + 1e-40)).max(0.0).sqrt() - 0.05).clamp(0.0, 1.0);
        let n = if s < 8 { n0 + tx * (n1 * dt2) + ty * (n2 * dt2) } else { n0 };
        (n.normalize(), rough)
    }

    fn soft_shadow(&self, ev: &mut Evaluator, p: V3, l: V3, eps: f64, softness: f64, max_t: f64) -> f64 {
        let mut res: f64 = 1.0;
        let mut t = eps * 4.0;
        for _ in 0..160 {
            let q = p + l * t;
            let e = self.eps_at(t) * 0.5 + eps * 0.5;
            let d = ev.de(q, e);
            if d < e {
                return 0.0;
            }
            // Inside rendering: the estimate is a fixed step in the set, not a distance; only hits count.
            if softness > 0.0 && !self.scene.fractal.inside {
                res = res.min(d / (softness * t));
            }
            t += (d * self.scene.march.step).max(e);
            if t > max_t {
                break;
            }
        }
        let r = res.clamp(0.0, 1.0);
        r * r * (3.0 - 2.0 * r)
    }

    fn ambient_occlusion(&self, ev: &mut Evaluator, p: V3, n: V3, t: f64, rng: &mut Rng) -> f64 {
        let l = &self.scene.lighting;
        let radius = l.ao_radius * t;
        match l.ao_mode {
            AoMode::Off => 1.0,
            // The fast samples need distances, which inside rendering does not have.
            AoMode::Fast if self.scene.fractal.inside => 1.0,
            AoMode::Fast => {
                let (mut occ, mut w) = (0.0, 1.0);
                for k in 1..=5 {
                    let h = radius * k as f64 / 5.0;
                    let d = ev.de(p + n * h, h * 0.1);
                    occ += (h - d).max(0.0) / radius * w;
                    w *= 0.6;
                }
                ao_curve((occ / 0.959).clamp(0.0, 1.0), l.ao_strength)
            }
            AoMode::Raytraced => {
                let d = cosine_dir(n, rng.next_f64(), rng.next_f64());
                let eps = self.eps_at(t);
                let (hit, _, _) = self.march(ev, p + n * (eps * 2.0), d, 0.0, radius, 48);
                match hit {
                    Some(h) => ao_curve((1.0 - h.t / radius).clamp(0.0, 1.0), l.ao_strength),
                    None => 1.0,
                }
            }
        }
    }

    /// Background image color in direction `d` (display values in display space, else linear).
    pub fn background_image(&self, d: V3) -> Option<[f64; 3]> {
        let (m, rot) = self.bg_image.as_ref()?;
        let (u, v) = crate::maps::panorama_uv(rot, d.to_array());
        let s = m.sample_panorama(u, v);
        // Map pixels are blue, green, red.
        let rgb = [s[2], s[1], s[0]];
        Some(if self.scene.post.display_space { rgb } else { rgb.map(super::srgb_decode) })
    }

    /// Background color in direction `d`; `sy` is the screen position (-1 bottom .. 1 top).
    pub fn background(&self, d: V3, sy: f64) -> [f64; 3] {
        let b = &self.scene.background;
        let f = |c: [f32; 3]| c.map(|x| x as f64);
        match b.mode {
            BackgroundMode::Solid => f(b.top),
            BackgroundMode::Vertical => {
                let s = ((1.0 - sy) * 0.5).clamp(0.0, 1.0).powf(b.curve.max(0.01));
                let (top, bottom) = (f(b.top), f(b.bottom));
                [0, 1, 2].map(|k| top[k] + (bottom[k] - top[k]) * s)
            }
            BackgroundMode::Gradient => {
                let (from, to, e) = if d.z >= 0.0 { (f(b.horizon), f(b.top), d.z) } else { (f(b.horizon), f(b.bottom), -d.z) };
                let s = e.clamp(0.0, 1.0).powf(0.6);
                [0, 1, 2].map(|k| from[k] + (to[k] - from[k]) * s)
            }
        }
    }

    pub fn color_value(&self, hit: &Hit, n: V3) -> f64 {
        let o = &hit.orbit;
        match self.scene.material.color_mode {
            ColorMode::Iterations => {
                let n_it = o.iterations as f64 + self.scene.material.detail_shift * hit.eps.ln();
                if o.escaped && o.r2 > 1.0 {
                    let lb = self.prog.bailout2.max(4.0).sqrt().ln();
                    n_it - ((0.5 * o.r2.ln()) / lb).max(1e-10).log2()
                } else {
                    n_it
                }
            }
            ColorMode::IterationsRatio => {
                let n_it = o.iterations as f64 + self.scene.material.detail_shift * hit.eps.ln();
                if o.escaped && o.r2 > 1.0 {
                    let lb = self.prog.bailout2.max(4.0).sqrt().ln().ln();
                    let d = (0.5 * o.r2.ln()).ln();
                    let dd = if o.r2_old > 1.0 { d - (0.5 * o.r2_old.ln()).ln() } else { std::f64::consts::LN_2 };
                    n_it + (lb - d) / dd.max(1e-6)
                } else {
                    n_it
                }
            }
            ColorMode::OrbitTrap => o.trap.max(0.0).sqrt() * 4.0,
            ColorMode::Radius => hit.pos.length() * 4.0,
            ColorMode::Normal => n.z * 2.0 + 2.0,
        }
    }

    /// Surface and highlight color for the color value `v`.
    fn gradient(&self, v: f64) -> ([f64; 3], [f64; 3]) {
        let m = &self.scene.material;
        let mut t = v * m.color_scale + m.color_offset;
        t = if m.repeat { t - t.floor() } else { t.clamp(0.0, 1.0) };
        let x = t * (self.lut.len() - 1) as f64;
        let i = (x.floor() as usize).min(self.lut.len() - 2);
        let f = x - i as f64;
        let mix = |l: &[[f32; 3]]| [0, 1, 2].map(|k| l[i][k] as f64 + (l[i + 1][k] as f64 - l[i][k] as f64) * f);
        let base = mix(&self.lut);
        let first = self.lut[0];
        (
            [0, 1, 2].map(|k| first[k] as f64 + (base[k] - first[k] as f64) * m.saturation),
            mix(&self.lut_spec),
        )
    }

    /// Linear radiance of one sample of pixel (`px`, `py`).
    pub fn sample(&self, ev: &mut Evaluator, px: u32, py: u32, sample: u32) -> [f64; 3] {
        self.sample_aux(ev, px, py, sample).color
    }

    /// One sample of pixel (`px`, `py`) with the denoiser's helper values.
    pub fn sample_aux(&self, ev: &mut Evaluator, px: u32, py: u32, sample: u32) -> Sample {
        let sc = self.scene;
        let fr = &self.frame;
        let cam = &sc.camera;
        let mut rng = Rng::new(px, py, sample);
        let (jx, jy) = if sample == 0 { (0.5, 0.5) } else { (rng.next_f64(), rng.next_f64()) };
        let sx = ((px as f64 + jx) / fr.width as f64) * 2.0 - 1.0;
        let sy = 1.0 - ((py as f64 + jy) / fr.height as f64) * 2.0;
        let mut o = cam.position;
        let mut d = cam.ray_dir(sx, sy, fr.aspect);
        if sc.dof.enabled && sample > 0 {
            let focus = o + d * (sc.dof.focus_distance / d.dot(cam.forward));
            let (r, a) = (rng.next_f64().sqrt() * sc.dof.aperture * sc.dof.focus_distance * 0.1, rng.next_f64() * std::f64::consts::TAU);
            o = o + cam.right * (r * a.cos()) + cam.up * (r * a.sin());
            d = (focus - o).normalize();
        }
        let t0 = super::start_distance(sc);
        let first = self.trace(ev, o, d, t0, sc.march.max_distance, 0.0, sy, &mut rng);
        let mut c = first.color;
        // Reflections (Mandelbulb3D's CalcSR): each bounce adds the reflected scene, weighted by
        // the highlight color of the reflecting surface, the reflection amount and how much of
        // the surface the fog leaves visible.
        let m = &sc.material;
        if m.reflection > 0.0 {
            let mut weight = [1.0; 3];
            let mut surf = first.surface;
            for _ in 0..m.reflection_bounces.clamp(1, 8) {
                let Some(s) = surf else { break };
                let vis = (1.0 - s.fog).clamp(0.0, 1.0);
                for (w, sp) in weight.iter_mut().zip(s.spec) {
                    *w *= sp * m.reflection * vis;
                }
                if weight.iter().all(|w| *w < 1e-4) {
                    break;
                }
                let r = s.dir - s.n * (2.0 * s.dir.dot(s.n));
                // Marched as a continuation of the path, so the surface threshold keeps growing
                // with the distance travelled.
                let origin = s.pos - r * s.t;
                let syr = (2.0 * r.dot(cam.up).clamp(-1.0, 1.0).asin() / cam.fov.to_radians()).clamp(-1.0, 1.0);
                let tr = self.trace(ev, origin, r, s.t, s.t + sc.march.max_distance, s.t, syr, &mut rng);
                for k in 0..3 {
                    c[k] += weight[k] * tr.color[k];
                }
                surf = tr.surface;
            }
        }
        Sample { color: c, albedo: first.albedo, normal: first.normal }
    }

    /// Traces one ray from `o` along `d` (from distance `t0` to `max_t`; fog, glow and visible
    /// lights measure from `t_base`): its color and the surface it hit.
    #[allow(clippy::too_many_arguments)]
    fn trace(&self, ev: &mut Evaluator, o: V3, d: V3, t0: f64, max_t: f64, t_base: f64, sy: f64, rng: &mut Rng) -> Traced {
        let sc = self.scene;
        let fr = &self.frame;
        let (hit, _, steps) = self.march(ev, o, d, t0, max_t, sc.march.max_steps);
        let (glow, glow_mix) = sc.fog.glow_at(steps as f64, sc.march.max_steps as f64);
        let grad = self.background(d, sy);
        let bg = self.background_image(d).unwrap_or(grad);
        let fog_col = if sc.fog.from_background { grad } else { sc.fog.color.map(|x| x as f64) };
        let (near, far) = (super::start_distance(sc), sc.march.max_distance);
        let albedo: [f64; 3];
        let mut normal = V3::ZERO;
        let mut surface = None;
        let mut c = match &hit {
            None => {
                let f = sc.fog.amount(far, near, far);
                let c = [0, 1, 2].map(|k| bg[k] + (fog_col[k] - bg[k]) * f.max(0.0));
                albedo = c.map(|v| v.clamp(0.0, 1.0));
                c
            }
            Some(h) => {
                let (n, rough) = self.normal_rough(ev, h.pos, h.eps, h.t);
                let n = if n.dot(d) > 0.0 { -n } else { n };
                // Roughness dulls the highlights toward 1/p (Phong p ~ shininess / 4) and
                // softens the diffuse curves (PaintThread.CalcPixelColor2).
                let rr = rough * sc.lighting.roughness;
                let spec_rough = 1.0 + (2.0 * rr).min(1.0) * (4.0 / sc.lighting.shininess.max(1.0) - 1.0);
                let (base, spec_col) = self.gradient(self.color_value(h, n));
                // Shading point just off the surface; inside rendering puts the surface one
                // threshold into the solid, so it needs a larger offset.
                let p = h.pos + n * (h.eps * if sc.fractal.inside { 4.0 } else { 1.5 });
                let ao = self.ambient_occlusion(ev, p, n, h.t, rng);
                let l = &sc.lighting;
                let hemi = n.dot(fr.ambient_up) * 0.5 + 0.5;
                let mut diffuse = [0, 1, 2].map(|k| (l.ground_color[k] as f64 + (l.sky_color[k] - l.ground_color[k]) as f64 * hemi) * l.ambient * ao);
                let mut spec = [0.0; 3];
                for li in &fr.lights {
                    // Point lights: direction, squared-distance falloff and shadow length.
                    let (ldir, atten, max_t) = match li.point {
                        Some(lp) => {
                            let v = lp - p;
                            let d2 = v.dot(v).max(1e-30);
                            let dist = d2.sqrt();
                            (v * (1.0 / dist), 1.0 / d2, dist)
                        }
                        None => (li.dir, 1.0, sc.march.max_distance),
                    };
                    let ndl = n.dot(ldir);
                    let mut df = li.diffuse.apply(ndl);
                    if rr > 0.0 {
                        df += (super::rough_diffuse(li.diffuse.index() as usize, ndl) - df) * rr;
                    }
                    // Back faces: only wrapping curves reach them, and not through shadows.
                    if df <= 0.0 || (ndl <= 0.0 && li.shadows) {
                        continue;
                    }
                    let sh = if li.shadows { self.soft_shadow(ev, p, ldir, h.eps, li.softness, max_t) } else { 1.0 };
                    if sh <= 0.0 {
                        continue;
                    }
                    let w = sh * atten * (1.0 - li.occlusion * (1.0 - ao));
                    let hv = (ldir - d).normalize();
                    let s = if ndl > 0.0 { n.dot(hv).max(0.0).powf(l.shininess) * li.specular * sc.material.specular * (l.shininess + 8.0) / 25.0 * spec_rough } else { 0.0 };
                    for k in 0..3 {
                        diffuse[k] += li.color[k] * df * w;
                        spec[k] += li.color[k] * s * w;
                    }
                }
                let lit = [0, 1, 2].map(|k| base[k] * diffuse[k] + spec[k] * spec_col[k]);
                let f = sc.fog.amount(h.t - t_base, near, far);
                albedo = [0, 1, 2].map(|k| (base[k] * (1.0 - f) + fog_col[k] * f.max(0.0)).clamp(0.0, 1.0));
                normal = n;
                surface = Some(Surface { pos: p, n, dir: d, t: h.t, spec: spec_col, fog: f });
                [0, 1, 2].map(|k| lit[k] * (1.0 - f) + fog_col[k] * f.max(0.0))
            }
        };
        let g1 = sc.fog.glow_color;
        let g2 = sc.fog.glow_color2;
        let glow_col = [0, 1, 2].map(|k| g1[k] as f64 + (g2[k] as f64 - g1[k] as f64) * glow_mix);
        if sc.fog.glow_blend {
            let g = glow.clamp(0.0, 1.0);
            for (k, ck) in c.iter_mut().enumerate() {
                *ck = *ck * (1.0 - g) + glow_col[k] * g;
            }
        } else {
            for (k, ck) in c.iter_mut().enumerate() {
                *ck += glow_col[k] * glow;
            }
        }
        // Visible light sources (seen from where this ray part starts).
        let hit_t = hit.as_ref().map(|h| h.t - t_base);
        let from = o + d * t_base;
        for li in &fr.lights {
            if let Some(g) = &li.glow
                && let Some((add, keep, fl)) = super::light_glow(li, g, from, d, hit_t, &sc.fog, near, far)
            {
                for k in 0..3 {
                    c[k] = c[k] * keep + fog_col[k] * fl * (1.0 - keep) + add[k];
                }
            }
        }
        Traced { color: c, albedo, normal, surface }
    }
}

/// What [`Shader::trace`] found.
struct Traced {
    color: [f64; 3],
    albedo: [f64; 3],
    normal: V3,
    surface: Option<Surface>,
}

/// A surface hit, as far as a reflection needs it.
#[derive(Clone, Copy)]
struct Surface {
    /// Shading point (just off the surface).
    pos: V3,
    n: V3,
    /// Direction of the ray that hit it.
    dir: V3,
    /// Path length to the hit.
    t: f64,
    /// Highlight color (reflectance).
    spec: [f64; 3],
    /// Fog amount at the hit.
    fog: f64,
}

/// One pixel sample: radiance and the denoiser's helper values (albedo, world normal).
pub struct Sample {
    pub color: [f64; 3],
    pub albedo: [f64; 3],
    pub normal: V3,
}

/// Light left by the occlusion `occ` (0..1) at `strength`: linear up to 1, above 1 it bends
/// toward the square like Mandelbulb3D's ambient shadow amplitude.
pub fn ao_curve(occ: f64, strength: f64) -> f64 {
    if strength <= 1.0 {
        1.0 - strength * occ
    } else {
        let d = 1.0 - occ;
        (d + (strength - 1.0) * (d * d - d)).max(0.0)
    }
}

/// Cosine-weighted direction in the hemisphere around `n`.
pub fn cosine_dir(n: V3, u1: f64, u2: f64) -> V3 {
    let a = if n.z.abs() < 0.999 { v3(0.0, 0.0, 1.0) } else { v3(1.0, 0.0, 0.0) };
    let t = a.cross(n).normalize();
    let b = n.cross(t);
    let r = u1.sqrt();
    let phi = u2 * std::f64::consts::TAU;
    (t * (r * phi.cos()) + b * (r * phi.sin()) + n * (1.0 - u1).max(0.0).sqrt()).normalize()
}

/// Progressive CPU renderer: each `render_pass` adds one sample per pixel.
pub struct CpuRenderer {
    pub scene: Scene,
    prog: Program,
    width: u32,
    height: u32,
    accum: Vec<[f64; 3]>,
    /// Summed albedo and normal per pixel.
    aux: Vec<[f64; 6]>,
    samples: u32,
}

impl CpuRenderer {
    pub fn new(scene: Scene, width: u32, height: u32) -> Self {
        let prog = Program::new(&scene.fractal, &scene.cut);
        let n = (width * height) as usize;
        CpuRenderer { scene, prog, width, height, accum: vec![[0.0; 3]; n], aux: vec![[0.0; 6]; n], samples: 0 }
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn program(&self) -> &Program {
        &self.prog
    }

    pub fn render_pass(&mut self) {
        let sc = shading_scene(&self.scene);
        let shader = Shader::new(&sc, &self.prog, self.width, self.height);
        let (w, sample) = (self.width, self.samples);
        let prog = &self.prog;
        self.accum.par_chunks_mut(w as usize).zip(self.aux.par_chunks_mut(w as usize)).enumerate().for_each_init(
            || prog.evaluator(),
            |ev, (y, (row, aux_row))| {
                for (x, (acc, aux)) in row.iter_mut().zip(aux_row.iter_mut()).enumerate() {
                    let s = shader.sample_aux(ev, x as u32, y as u32, sample);
                    let n = s.normal.to_array();
                    for k in 0..3 {
                        acc[k] += s.color[k];
                        aux[k] += s.albedo[k];
                        aux[k + 3] += n[k];
                    }
                }
            },
        );
        self.samples += 1;
    }

    /// The averaged image with the denoiser's helper images.
    pub fn hdr_image(&self) -> super::denoise::HdrImage {
        let n = self.samples.max(1) as f64;
        let mut img = super::denoise::HdrImage { width: self.width, height: self.height, color: Vec::new(), albedo: Vec::new(), normal: Vec::new() };
        for (a, x) in self.accum.iter().zip(&self.aux) {
            img.color.extend(a.iter().map(|v| (v / n) as f32));
            img.albedo.extend(x[..3].iter().map(|v| (v / n) as f32));
            // Averaged, not renormalized: OIDN takes shorter normals where hits and misses mix.
            img.normal.extend(x[3..].iter().map(|v| (v / n) as f32));
        }
        img
    }

    /// The image as 8-bit RGBA after post processing.
    pub fn rgba8(&self) -> Vec<u8> {
        let n = self.samples.max(1) as f64;
        let (w, h) = (self.width as f64, self.height as f64);
        let mut out = Vec::with_capacity(self.accum.len() * 4);
        for (i, a) in self.accum.iter().enumerate() {
            let (x, y) = ((i as u32 % self.width) as f64, (i as u32 / self.width) as f64);
            let c = post_process(a.map(|v| v / n), &self.scene.post, (x + 0.5) / w, (y + 0.5) / h);
            out.extend_from_slice(&[(c[0] * 255.0 + 0.5) as u8, (c[1] * 255.0 + 0.5) as u8, (c[2] * 255.0 + 0.5) as u8, 255]);
        }
        out
    }
}

/// Distance along the ray through screen point (`sx`, `sy` in -1..1) to the surface, if any
/// (picking, focus distance, navigation speed).
pub fn pick(scene: &Scene, sx: f64, sy: f64, width: u32, height: u32) -> Option<(f64, V3)> {
    let prog = Program::new(&scene.fractal, &scene.cut);
    let shader = Shader::new(scene, &prog, width, height);
    let mut ev = prog.evaluator();
    let d = scene.camera.ray_dir(sx, sy, shader.frame.aspect);
    let (hit, _, _) = shader.march(&mut ev, scene.camera.position, d, super::start_distance(scene), scene.march.max_distance, scene.march.max_steps);
    hit.map(|h| (h.t, h.pos))
}

/// Distance estimate at the camera position (navigation speed).
pub fn distance_at_camera(scene: &Scene) -> f64 {
    let prog = Program::new(&scene.fractal, &scene.cut);
    let mut ev = prog.evaluator();
    ev.de(scene.camera.position, 1e-6)
}
