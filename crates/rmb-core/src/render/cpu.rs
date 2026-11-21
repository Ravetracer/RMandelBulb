// SPDX-License-Identifier: LGPL-2.1-or-later

//! CPU renderer in f64: the reference for the GPU kernel and the fallback without a GPU.

use rayon::prelude::*;

use super::{Frame, Rng, post_process};
use crate::fractal::{Evaluator, Orbit, Program};
use crate::math::{V3, v3};
use crate::scene::{AoMode, BackgroundMode, ColorMode, Scene};

/// Shading inputs that do not change per pixel.
pub struct Shader<'a> {
    pub scene: &'a Scene,
    pub prog: &'a Program,
    pub frame: Frame,
    pub lut: Vec<[f32; 3]>,
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
        Shader { scene, prog, frame: Frame::new(scene, width, height), lut: crate::gradient::lut(&scene.material.gradient) }
    }

    /// Surface threshold at ray distance `t`.
    pub fn eps_at(&self, t: f64) -> f64 {
        (self.scene.march.detail * self.frame.pixel_angle * t).max(1.0e-7)
    }

    /// Marches a ray. Returns the hit (if any) and the number of steps taken.
    pub fn march(&self, ev: &mut Evaluator, o: V3, d: V3, t0: f64, max_t: f64, max_steps: u32) -> (Option<Hit>, u32) {
        let m = &self.scene.march;
        let mut t = t0;
        let mut last_step = 0.0;
        for step in 0..max_steps {
            let p = o + d * t;
            let eps = self.eps_at(t);
            let orbit = ev.orbit(p, eps * 0.5);
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
                return (Some(Hit { t: t_hit, pos: o + d * t_hit, eps: self.eps_at(t_hit), orbit: orbit_hit, steps: step }), step);
            }
            // At least a tenth of the threshold: no stalling where the estimate is near zero.
            last_step = (orbit.de * m.step).max(eps * 0.1);
            t += last_step;
            if t > max_t {
                return (None, step);
            }
        }
        (None, max_steps)
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

    fn soft_shadow(&self, ev: &mut Evaluator, p: V3, l: V3, eps: f64, softness: f64) -> f64 {
        let mut res: f64 = 1.0;
        let mut t = eps * 4.0;
        let max_t = self.scene.march.max_distance;
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
                (1.0 - l.ao_strength * occ / 0.959).clamp(0.0, 1.0)
            }
            AoMode::Raytraced => {
                let d = cosine_dir(n, rng.next_f64(), rng.next_f64());
                let eps = self.eps_at(t);
                let (hit, _) = self.march(ev, p + n * (eps * 2.0), d, 0.0, radius, 48);
                match hit {
                    Some(h) => 1.0 - l.ao_strength * (1.0 - h.t / radius).clamp(0.0, 1.0),
                    None => 1.0,
                }
            }
        }
    }

    pub fn background(&self, d: V3) -> [f64; 3] {
        let b = &self.scene.background;
        let f = |c: [f32; 3]| c.map(|x| x as f64);
        match b.mode {
            BackgroundMode::Solid => f(b.top),
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
                let n_it = o.iterations as f64;
                if o.escaped && o.r2 > 1.0 {
                    let lb = self.prog.bailout2.max(4.0).sqrt().ln();
                    n_it - ((0.5 * o.r2.ln()) / lb).max(1e-10).log2()
                } else {
                    n_it
                }
            }
            ColorMode::OrbitTrap => o.trap.max(0.0).sqrt() * 4.0,
            ColorMode::Radius => hit.pos.length() * 4.0,
            ColorMode::Normal => n.z * 2.0 + 2.0,
        }
    }

    fn gradient(&self, v: f64) -> [f64; 3] {
        let m = &self.scene.material;
        let mut t = v * m.color_scale + m.color_offset;
        t = if m.repeat { t - t.floor() } else { t.clamp(0.0, 1.0) };
        let x = t * (self.lut.len() - 1) as f64;
        let i = (x.floor() as usize).min(self.lut.len() - 2);
        let f = x - i as f64;
        let (a, b) = (self.lut[i], self.lut[i + 1]);
        let base = [0, 1, 2].map(|k| a[k] as f64 + (b[k] as f64 - a[k] as f64) * f);
        let first = self.lut[0];
        [0, 1, 2].map(|k| first[k] as f64 + (base[k] - first[k] as f64) * m.saturation)
    }

    /// Linear radiance of one sample of pixel (`px`, `py`).
    pub fn sample(&self, ev: &mut Evaluator, px: u32, py: u32, sample: u32) -> [f64; 3] {
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
        let (hit, steps) = self.march(ev, o, d, super::start_distance(sc), sc.march.max_distance, sc.march.max_steps);
        let glow = sc.fog.glow * steps as f64 / sc.march.max_steps.max(1) as f64;
        let mut c = match &hit {
            None => {
                let bg = self.background(d);
                let f = 1.0 - (-sc.fog.density * sc.march.max_distance).exp();
                [0, 1, 2].map(|k| bg[k] + (sc.fog.color[k] as f64 - bg[k]) * f)
            }
            Some(h) => {
                let n = self.normal(ev, h.pos, h.eps);
                let n = if n.dot(d) > 0.0 { -n } else { n };
                let base = self.gradient(self.color_value(h, n));
                // Shading point just off the surface; inside rendering puts the surface one
                // threshold into the solid, so it needs a larger offset.
                let p = h.pos + n * (h.eps * if sc.fractal.inside { 4.0 } else { 1.5 });
                let ao = self.ambient_occlusion(ev, p, n, h.t, &mut rng);
                let l = &sc.lighting;
                let hemi = n.z * 0.5 + 0.5;
                let mut diffuse = [0, 1, 2].map(|k| (l.ground_color[k] as f64 + (l.sky_color[k] - l.ground_color[k]) as f64 * hemi) * l.ambient * ao);
                let mut spec = [0.0; 3];
                for li in &fr.lights {
                    let ndl = n.dot(li.dir);
                    if ndl <= 0.0 {
                        continue;
                    }
                    let sh = if li.shadows { self.soft_shadow(ev, p, li.dir, h.eps, li.softness) } else { 1.0 };
                    if sh <= 0.0 {
                        continue;
                    }
                    let hv = (li.dir - d).normalize();
                    let s = n.dot(hv).max(0.0).powf(l.shininess) * li.specular * sc.material.specular * (l.shininess + 8.0) / 25.0;
                    for k in 0..3 {
                        diffuse[k] += li.color[k] * ndl * sh;
                        spec[k] += li.color[k] * s * sh;
                    }
                }
                let lit = [0, 1, 2].map(|k| base[k] * diffuse[k] + spec[k]);
                let f = 1.0 - (-sc.fog.density * h.t).exp();
                [0, 1, 2].map(|k| lit[k] + (sc.fog.color[k] as f64 - lit[k]) * f)
            }
        };
        for (k, ck) in c.iter_mut().enumerate() {
            *ck += sc.fog.glow_color[k] as f64 * glow;
        }
        c
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
    samples: u32,
}

impl CpuRenderer {
    pub fn new(scene: Scene, width: u32, height: u32) -> Self {
        let prog = Program::new(&scene.fractal, &scene.cut);
        CpuRenderer { scene, prog, width, height, accum: vec![[0.0; 3]; (width * height) as usize], samples: 0 }
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn program(&self) -> &Program {
        &self.prog
    }

    pub fn render_pass(&mut self) {
        let shader = Shader::new(&self.scene, &self.prog, self.width, self.height);
        let (w, sample) = (self.width, self.samples);
        let prog = &self.prog;
        self.accum.par_chunks_mut(w as usize).enumerate().for_each_init(
            || prog.evaluator(),
            |ev, (y, row)| {
                for (x, acc) in row.iter_mut().enumerate() {
                    let c = shader.sample(ev, x as u32, y as u32, sample);
                    for k in 0..3 {
                        acc[k] += c[k];
                    }
                }
            },
        );
        self.samples += 1;
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
    let (hit, _) = shader.march(&mut ev, scene.camera.position, d, super::start_distance(scene), scene.march.max_distance, scene.march.max_steps);
    hit.map(|h| (h.t, h.pos))
}

/// Distance estimate at the camera position (navigation speed).
pub fn distance_at_camera(scene: &Scene) -> f64 {
    let prog = Program::new(&scene.fractal, &scene.cut);
    let mut ev = prog.evaluator();
    ev.de(scene.camera.position, 1e-6)
}
