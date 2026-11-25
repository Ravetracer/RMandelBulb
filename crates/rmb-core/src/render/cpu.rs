// SPDX-License-Identifier: LGPL-2.1-or-later

//! CPU renderer in f64: the reference for the GPU kernel and the fallback without a GPU.

use rayon::prelude::*;

use super::{Frame, Rng, post_process, shading_scene};
use crate::fractal::{Evaluator, Orbit, Program};
use crate::math::{V3, v3};
use crate::scene::{AoMode, BackgroundMode, ColorMode, MapMode, Scene};

/// Shading inputs that do not change per pixel.
pub struct Shader<'a> {
    pub scene: &'a Scene,
    pub prog: &'a Program,
    pub frame: Frame,
    /// Materials: 0 the main one, k the one of `Scene::objects[k - 1]`.
    pub materials: Vec<Mat>,
    pub bg_image: Option<(std::sync::Arc<crate::maps::Map>, [[f64; 3]; 3])>,
    /// The 16 × 8 copy of the background image for ambient light.
    pub bg_ambient: Option<std::sync::Arc<crate::maps::Map>>,
    /// Color map with its rotation matrix and average color (display values).
    pub color_map: Option<ColorMapData>,
    /// Interior colors (empty: none).
    pub lut_int: Vec<[f32; 3]>,
    pub lut_int_spec: Vec<[f32; 3]>,
    /// Whether any color of any material emits.
    pub emissive: bool,
    /// The scene has water (it reflects even without the material's reflection).
    pub water: bool,
}

/// Color of the foam on water crests (march.wgsl `WATER_FOAM`).
const WATER_FOAM: [f64; 3] = [0.85, 0.88, 0.9];
/// Highlight sharpness of water (march.wgsl `WATER_SHININESS`).
const WATER_SHININESS: f64 = 400.0;

/// The binary refinement of a hit stops at steps below this part of the threshold.
pub const REFINE_TOLERANCE: f64 = 1.0 / 128.0;

/// Upper limit of the refinement steps while the hit point is still where the estimate is zero
/// (march.wgsl `REFINE_INSIDE_MAX`): a point that did not escape (too few iterations for the
/// zoom) has no gradient, so its normal, shadow and occlusion would come out black.
pub const REFINE_INSIDE_MAX: u32 = 32;

/// Tetrahedron directions of the distance gradient at a steam point (march.wgsl `STEAM_TETRA`).
const STEAM_TETRA: [[f64; 3]; 4] = [[1.0, -1.0, -1.0], [-1.0, -1.0, 1.0], [-1.0, 1.0, -1.0], [1.0, 1.0, 1.0]];

/// Color value of the position modes at world point `p` (march.wgsl `position_value`).
pub fn position_value(m: &crate::scene::Material, mode: ColorMode, p: V3) -> f64 {
    let c = v3(m.color_center[0], m.color_center[1], m.color_center[2]);
    let d = p - c;
    if mode == ColorMode::Sphere {
        return d.length();
    }
    let dir = v3(m.color_direction[0], m.color_direction[1], m.color_direction[2]);
    let len = dir.length();
    if len > 0.0 { d.dot(dir * (1.0 / len)) } else { d.z }
}

/// Whether the scene is path traced.
fn sc_path(sc: &Scene) -> bool {
    sc.lighting.render_mode == crate::scene::RenderMode::PathTraced
}

/// Path tracing: probability of following the reflection / glass at a surface (else a diffuse
/// bounce): the reflection amount times the highlight color's average, at least the glass
/// transparency share, at most 0.9 (march.wgsl `path_specular_share`).
pub fn path_specular_share(amount: f64, spec: [f64; 3], trans: f64, glass: f64) -> f64 {
    if amount <= 0.0 {
        return 0.0;
    }
    let s = amount * (spec[0] + spec[1] + spec[2]) / 3.0;
    let g = if glass > 0.0 { trans * glass } else { 0.0 };
    s.max(g).clamp(0.0, 0.9)
}

/// What the primary march of a ray collects besides the hit (march.wgsl `pick_*`, `haze_*`):
/// the glow and steam sample (`steam` on, random seed, total weight, the picked point, its
/// falloff size, ray distance and whether the light falls off along the way to the surface),
/// and the fractal haze (`haze` on, the light it added and the share of light behind it that
/// gets through).
#[derive(Clone, Copy, Default)]
pub struct SteamPick {
    pub steam: bool,
    pub seed: u32,
    pub w: f64,
    pub pos: V3,
    pub rs: f64,
    pub t: f64,
    pub fall: bool,
    pub haze: bool,
    pub haze_light: [f64; 3],
    pub haze_keep: f64,
}

impl SteamPick {
    /// Offers the point `p` (ray distance `t`, falloff size `rs`) with weight `w`: reservoir
    /// sampling with the hashed random number of `step` and `salt`.
    #[allow(clippy::too_many_arguments)]
    fn offer(&mut self, step: u32, salt: i32, w: f64, p: V3, rs: f64, t: f64, fall: bool) {
        if w <= 0.0 {
            return;
        }
        self.w += w;
        if (super::hash3(step as i32, self.seed as i32, salt) as f64) * self.w < w {
            self.pos = p;
            self.rs = rs;
            self.t = t;
            self.fall = fall;
        }
    }
}

/// A material prepared for shading: its gradient tables and how a color value picks a color.
pub struct Mat {
    pub lut: Vec<[f32; 3]>,
    pub lut_spec: Vec<[f32; 3]>,
    pub lut_trans: Vec<f32>,
    pub lut_emis: Vec<f32>,
    /// Glow bands over the gradient.
    pub bands: Vec<crate::gradient::GlowBand>,
    pub mode: ColorMode,
    pub scale: f64,
    pub offset: f64,
    pub repeat: bool,
    pub saturation: f64,
    /// Primitives: `Pattern::index` and size.
    pub pattern: u32,
    pub pattern_size: f64,
    /// Texture layers that can be drawn.
    pub layers: Vec<crate::texture::Prepared>,
}

impl Mat {
    #[allow(clippy::too_many_arguments)]
    fn new(g: &[crate::scene::ColorStop], bands: &[crate::gradient::GlowBand], mode: ColorMode, scale: f64, offset: f64, repeat: bool, saturation: f64, pattern: u32, pattern_size: f64, layers: &[crate::texture::TextureLayer]) -> Mat {
        Mat {
            lut: crate::gradient::lut(g),
            lut_spec: crate::gradient::lut_specular(g),
            lut_trans: crate::gradient::lut_transparency(g),
            lut_emis: crate::gradient::lut_emission(g),
            bands: crate::gradient::active_bands(bands),
            mode,
            scale,
            offset,
            repeat,
            saturation,
            pattern,
            pattern_size,
            layers: crate::texture::prepare(layers),
        }
    }

    /// The materials of a scene: the main one, then one per object.
    pub fn of_scene(scene: &Scene) -> Vec<Mat> {
        let m = &scene.material;
        let mut out = vec![Mat::new(&m.gradient, &m.bands, m.color_mode, m.color_scale, m.color_offset, m.repeat, m.saturation, 0, 1.0, &m.layers)];
        for o in &scene.objects {
            let om = &o.material;
            out.push(Mat::new(&om.gradient, &om.bands, om.color_mode, om.color_scale, om.color_offset, om.repeat, 1.0, om.pattern.index(), om.pattern_size, &om.layers));
        }
        out
    }
}

#[derive(Clone, Copy)]
pub struct Hit {
    pub t: f64,
    pub pos: V3,
    pub eps: f64,
    pub orbit: Orbit,
    pub steps: u32,
}

impl<'a> Shader<'a> {
    pub fn new(scene: &'a Scene, prog: &'a Program, width: u32, height: u32) -> Self {
        let materials = Mat::of_scene(scene);
        // Only materials that are used count for emission.
        let used = |k: usize| k == 0 || prog.objects.iter().any(|o| o.material == k);
        let glows = |m: &Mat| m.lut_emis.iter().any(|e| *e > 0.0) || m.bands.iter().any(|b| b.emission > 0.0) || m.layers.iter().any(|l| l.layer.target == crate::texture::Target::Emission);
        let emissive = materials.iter().enumerate().any(|(k, m)| used(k) && glows(m));
        Shader {
            scene,
            prog,
            frame: Frame::new(scene, width, height),
            materials,
            bg_image: super::background_image(scene),
            bg_ambient: super::background_maps(scene).map(|(_, s, _)| s),
            color_map: crate::maps::get_any(&scene.material.color_map.image).map(|m| {
                let avg = m.average();
                (m, crate::maps::rotation_matrix(scene.material.color_map.rotation), avg)
            }),
            lut_int: if scene.material.interior.is_empty() { Vec::new() } else { crate::gradient::lut(&scene.material.interior) },
            lut_int_spec: if scene.material.interior.is_empty() { Vec::new() } else { crate::gradient::lut_specular(&scene.material.interior) },
            emissive,
            water: prog.has_water(),
        }
    }

    /// Water at the hit `orbit` / `pos`: its color from `base` (brighter on the crests where
    /// light shines through, foam on the highest) and its Fresnel reflection for a ray
    /// arriving at the cosine `cos_i` (None: not water).
    fn water_at(&self, orbit: &Orbit, pos: V3, cos_i: f64, base: [f64; 3]) -> Option<([f64; 3], f64)> {
        let ob = self.prog.objects.get((orbit.object as usize).checked_sub(1)?)?;
        let crate::fractal::ObjectShape::Primitive(crate::fractal::WATER_KIND, a) = &ob.shape else { return None };
        let (h, foam) = crate::compose::water_look(a, ob.local(pos), orbit.shore);
        let lift = 0.55 + 0.9 * h;
        let color = [0, 1, 2].map(|k| base[k] * lift + (WATER_FOAM[k] - base[k] * lift) * foam);
        Some((color, crate::compose::water_fresnel(cos_i, a[7]) * (1.0 - foam)))
    }

    /// Surface threshold at ray distance `t`.
    pub fn eps_at(&self, t: f64) -> f64 {
        (self.scene.march.detail * self.frame.pixel_angle * t).max(1.0e-7)
    }

    /// Marches a ray. Returns the hit (if any), the number of steps taken and the number of
    /// glowing steps (see `Fog::glow_iteration`).
    pub fn march(&self, ev: &mut Evaluator, o: V3, d: V3, t0: f64, max_t: f64, max_steps: u32) -> (Option<Hit>, u32, f64) {
        self.march_ex(ev, o, d, t0, max_t, max_steps, true, None)
    }

    /// [`Shader::march`] without the binary refinement of the hit: for the ambient occlusion,
    /// emission and scatter rays, which need neither the exact distance nor the exact point.
    pub fn march_rough(&self, ev: &mut Evaluator, o: V3, d: V3, t0: f64, max_t: f64, max_steps: u32) -> Option<Hit> {
        self.march_ex(ev, o, d, t0, max_t, max_steps, false, None).0
    }

    /// [`Shader::march`] that with `pick` also offers the points near emissive parts for the
    /// glow and the steam (see [`SteamPick`]).
    #[allow(clippy::too_many_arguments)]
    fn march_ex(&self, ev: &mut Evaluator, o: V3, d: V3, t0: f64, max_t: f64, max_steps: u32, refine: bool, mut pick: Option<&mut SteamPick>) -> (Option<Hit>, u32, f64) {
        let m = &self.scene.march;
        let sm = &self.scene.material;
        let glow_it = self.scene.fog.glow_iteration;
        let mut t = t0;
        let mut last_step = 0.0;
        // Glowing steps (steps cut by the limiter count by the part they cover, like MB3D).
        let mut glow = 0.0;
        // Ray step limiter (Mandelbulb3D's RayMarch): the estimate before the last step and
        // the step factor.
        let cap = self.frame.step_cap;
        let mut last_de = 0.0;
        for step in 0..max_steps {
            let p = o + d * t;
            let eps = self.eps_at(t);
            let orbit = ev.orbit(p, eps * 0.5);
            let glowing = glow_it == 0 || orbit.iterations == glow_it;
            if orbit.de < eps {
                // Binary refinement toward the threshold, until the step is a small part of the
                // threshold (the evaluations at the surface are the most expensive ones).
                let (mut t_hit, mut orbit_hit) = (t, orbit);
                let mut dt = last_step * 0.5;
                if step > 0 && refine {
                    let mut k = 0;
                    while k < m.refine_steps || (orbit_hit.de <= 0.0 && k < REFINE_INSIDE_MAX) {
                        k += 1;
                        if dt < eps * REFINE_TOLERANCE {
                            break;
                        }
                        let tt = t_hit - dt;
                        let ob = ev.orbit(o + d * tt, self.eps_at(tt) * 0.5);
                        if ob.de < self.eps_at(tt) {
                            t_hit = tt;
                            orbit_hit = ob;
                        }
                        dt *= 0.5;
                    }
                }
                return (Some(Hit { t: t_hit, pos: o + d * t_hit, eps: self.eps_at(t_hit), orbit: orbit_hit, steps: step }), step, glow);
            }
            // At least a tenth of the threshold: no stalling where the estimate is near zero.
            if cap > 0.0 {
                // The estimate grows by at most the last step; where it falls faster than the
                // ray advances, the next step shrinks (to half at most); no step beyond the cap.
                let de = if step > 0 { orbit.de.min(last_de + last_step) } else { orbit.de };
                let shrink = if step > 0 && last_de > de + 1e-30 { (last_step / (last_de - de)).clamp(0.5, 1.0) } else { 1.0 };
                last_de = de;
                let want = de * m.step * shrink;
                if glowing {
                    glow += if want > cap * t { cap * t / want } else { 1.0 };
                }
                last_step = want.min(cap * t).max(eps * 0.1);
            } else {
                if glowing {
                    glow += 1.0;
                }
                last_step = (orbit.de * m.step).max(eps * 0.1);
            }
            if let Some(pk) = pick.as_deref_mut().filter(|pk| pk.haze) {
                // Fractal haze over this step: thickness relative to the distance of the step's middle
                // (the same at every zoom), its own light dimmed by the haze in front of it.
                let f = &self.scene.fog;
                let tm = (t + 0.5 * last_step).max(eps);
                let n = crate::scene::Fog::haze_iterations(orbit.iterations, orbit.r2, orbit.r2_old, orbit.escaped, self.prog.bailout2);
                let dens = if orbit.object == 0 { f.haze_density(orbit.de, n, self.prog.max_iterations as f64, tm) } else { 0.0 };
                if dens > 1e-4 {
                    let a = 1.0 - (-f.haze * dens * last_step / tm).exp();
                    // Colors from the main gradient by the smooth iteration count.
                    let col = if f.haze_gradient { self.gradient(0, n, t).0 } else { f.haze_color.map(|v| v as f64) };
                    for (l, c) in pk.haze_light.iter_mut().zip(col) {
                        *l += pk.haze_keep * a * c * f.haze_glow;
                    }
                    pk.haze_keep *= 1.0 - a;
                }
            }
            if let Some(pk) = pick.as_deref_mut().filter(|pk| pk.steam) {
                // Glow and steam: offer the point in proportion to its weight (glow: falloff from
                // the surface; steam: also the noise density).
                let r = (sm.emission_glow_size * t).max(eps);
                if sm.emission_glow > 0.0 && orbit.de < 8.0 * r {
                    pk.offer(step, 0x61c0, (-orbit.de / r).exp() * last_step.min(r) / r * sm.emission_glow, p, r, t, false);
                }
                let rs = (sm.steam_size * t).max(eps);
                if sm.steam > 0.0 && orbit.de < 8.0 * rs {
                    let dens = super::steam_density(p, sm.steam_grain, sm.steam_offset, sm.steam_cover);
                    pk.offer(step, 0x51ea, (-orbit.de / rs).exp() * last_step.min(rs) / rs * dens * sm.steam, p, rs, t, true);
                }
            }
            t += last_step;
            if t > max_t {
                return (None, step, glow.min(step as f64));
            }
        }
        (None, max_steps, glow)
    }

    /// Emitted radiance (color × emission strength of the gradient) for an orbit at `p`.
    fn emission_at(&self, orbit: &Orbit, p: V3, eps: f64, t: f64) -> [f64; 3] {
        if orbit.object == 0 && !self.lut_int.is_empty() && !orbit.escaped {
            return [0.0; 3];
        }
        let h = Hit { t, pos: p, eps, orbit: *orbit, steps: 0 };
        let mat = self.material_of(orbit);
        let cv = self.color_value(&h, V3::ZERO);
        let (base, _, _, e) = self.gradient(mat, cv, t);
        let base = self.model_color(&h, mat, base);
        let (l, _) = self.apply_layers(mat, &h, V3::ZERO, cv, base, e);
        let e = if l.emis > 0.0 { l.emis * super::shimmer(&self.scene.material, p) } else { l.emis };
        l.base.map(|c| c * e)
    }

    /// The color of a 3D model at a hit on it (material pattern "Model colors"; else `base`).
    fn model_color(&self, h: &Hit, mat: usize, base: [f64; 3]) -> [f64; 3] {
        if mat == 0 || self.materials[mat].pattern != crate::compose::Pattern::Model.index() {
            return base;
        }
        let Some(ob) = (h.orbit.object > 0).then(|| self.prog.objects.get(h.orbit.object as usize - 1)).flatten() else { return base };
        let crate::fractal::ObjectShape::Model(Some(m)) = &ob.shape else { return base };
        let q = ob.local(h.pos);
        match m.vol.color(q.to_array(), &m.look) {
            Some(c) if self.scene.post.display_space => c.map(super::srgb_encode),
            Some(c) => c,
            None => base,
        }
    }

    /// Color, specular factor and normal of a 3D model at hit `h` with normal `n` (material
    /// pattern "Model colors": the model's materials with their bump and specular maps; else
    /// `base`, 1 and `n`).
    /// The model's own emission comes last (added to the shaded color).
    fn model_surface(&self, h: &Hit, mat: usize, base: [f64; 3], n: V3) -> ([f64; 3], f64, V3, [f64; 3]) {
        let plain = (base, 1.0, n, [0.0; 3]);
        if mat == 0 || self.materials[mat].pattern != crate::compose::Pattern::Model.index() {
            return plain;
        }
        let Some(ob) = (h.orbit.object > 0).then(|| self.prog.objects.get(h.orbit.object as usize - 1)).flatten() else { return plain };
        let crate::fractal::ObjectShape::Model(Some(m)) = &ob.shape else { return plain };
        let q = ob.local(h.pos);
        let nl = crate::compose::normal_to_local(&ob.inv, ob.size, n);
        let Some(s) = m.vol.surface(q.to_array(), &m.look, Some(nl.to_array())) else { return plain };
        let display = self.scene.post.display_space;
        let color = match s.color {
            Some(c) if display => c.map(super::srgb_encode),
            Some(c) => c,
            None => base,
        };
        let b = s.normal;
        let emission = if display { s.emission.map(super::srgb_encode) } else { s.emission };
        (color, s.specular, crate::compose::normal_to_world(&ob.inv, ob.size, v3(b[0], b[1], b[2])), emission)
    }

    /// Where the texture layers of a hit are evaluated (`n`: world normal, zero if unknown).
    fn layer_coord(&self, h: &Hit, n: V3, cv: f64) -> crate::texture::Coord {
        let mut c = crate::texture::Coord { obj: h.pos, world: h.pos, n_obj: n, n_world: n, value: cv, trap: h.orbit.trap };
        if let Some(ob) = (h.orbit.object > 0).then(|| self.prog.objects.get(h.orbit.object as usize - 1)).flatten() {
            c.obj = ob.local(h.pos);
            c.n_obj = crate::compose::rotate(&ob.inv, n);
        }
        c
    }

    /// Applies the texture layers of material `mat`: surface color, emission and the bumped
    /// normal (unchanged when `n` is zero).
    #[allow(clippy::too_many_arguments)]
    fn apply_layers(&self, mat: usize, h: &Hit, n: V3, cv: f64, base: [f64; 3], emis: f64) -> (crate::texture::Applied, V3) {
        let layers = &self.materials[mat].layers;
        if layers.is_empty() {
            return (crate::texture::Applied { base, emis, spec: 1.0, rough_keep: 1.0, rough_add: 0.0 }, n);
        }
        let c = self.layer_coord(h, n, cv);
        let applied = crate::texture::apply(layers, &c, !self.scene.post.display_space, base, emis);
        if n == V3::ZERO || !layers.iter().any(|l| l.layer.target.bends()) {
            return (applied, n);
        }
        let inv = (h.orbit.object > 0).then(|| self.prog.objects.get(h.orbit.object as usize - 1)).flatten().map(|ob| ob.inv);
        let to_world = |v: V3| match &inv {
            // The transpose undoes the inverse rotation.
            Some(m) => v3(m[0][0] * v.x + m[1][0] * v.y + m[2][0] * v.z, m[0][1] * v.x + m[1][1] * v.y + m[2][1] * v.z, m[0][2] * v.x + m[1][2] * v.y + m[2][2] * v.z),
            None => v,
        };
        (applied, crate::texture::bump(layers, &c, &to_world))
    }

    /// Surface color at the surface point `p` with normal `n` without lighting (mesh export
    /// vertex colors): gradient, interior colors, 3D model colors, texture layers and color
    /// maps, as the shading's base color.
    pub fn surface_base(&self, ev: &mut Evaluator, p: V3, n: V3) -> [f64; 3] {
        let sc = self.scene;
        let t = (p - sc.camera.position).length();
        let eps = (sc.march.detail * self.frame.pixel_angle * t).max(1e-12);
        let orbit = ev.orbit(p, eps * 0.5);
        let h0 = Hit { t, pos: p, eps, orbit, steps: 0 };
        let recolored = ev.color_orbit(p, &h0.orbit).map(|orbit| Hit { orbit, ..h0 });
        let h = recolored.as_ref().unwrap_or(&h0);
        let cv = self.color_value(h, n);
        let main = h.orbit.object == 0;
        let mat = self.material_of(&h.orbit);
        let base = match self.interior_colors(h, n, t).filter(|_| main) {
            Some((b, _)) => b,
            None => {
                let (b, ..) = self.gradient(mat, cv, t);
                self.model_surface(h, mat, b, n).0
            }
        };
        let (lay, _) = self.apply_layers(mat, h, n, cv, base, 0.0);
        if main { self.map_color(h, n, cv, lay.base, 0.0).0 } else { lay.base }
    }

    /// Material index of the surface an orbit belongs to.
    pub fn material_of(&self, orbit: &Orbit) -> usize {
        match orbit.object {
            0 => 0,
            k => self.prog.objects.get(k as usize - 1).map_or(0, |o| o.material),
        }
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

    /// Light reaching `p` from direction `l` (0 = in shadow, per color channel). With
    /// `Material::glass_shadows`, glass surfaces let light through: by their transparency and the
    /// glass amount, absorbed by their color along the straight path inside (at most four
    /// crossings). `dist`: distance of `p` from the camera (for the gradient's depth shift).
    #[allow(clippy::too_many_arguments)]
    fn soft_shadow(&self, ev: &mut Evaluator, p: V3, l: V3, eps: f64, softness: f64, max_t: f64, dist: f64) -> [f64; 3] {
        let m = &self.scene.material;
        let glass = if m.glass_shadows && m.transmission { m.reflection } else { 0.0 };
        let mut res: f64 = 1.0;
        let mut tint = [1.0f64; 3];
        let mut crossings = 0;
        let mut t = eps * 4.0;
        for _ in 0..160 {
            let q = p + l * t;
            let e = self.eps_at(t) * 0.5 + eps * 0.5;
            let d = ev.de(q, e);
            if d < e {
                if glass <= 0.0 || crossings >= 4 {
                    return [0.0; 3];
                }
                let orbit = ev.orbit(q, e);
                let hit = Hit { t, pos: q, eps: e, orbit, steps: 0 };
                let (base, _, trans, _) = self.gradient(self.material_of(&hit.orbit), self.color_value(&hit, -l), dist + t);
                let pass = (trans * glass).min(1.0);
                if pass <= 0.0 {
                    return [0.0; 3];
                }
                // Through the glass to the far side.
                ev.flip = !ev.flip;
                let enter = t;
                t += e * 2.0;
                for _ in 0..160 {
                    let e = self.eps_at(t) * 0.5 + eps * 0.5;
                    let d = ev.de(p + l * t, e);
                    if d < e || t > max_t {
                        break;
                    }
                    t += (d * self.scene.march.step).max(e);
                }
                ev.flip = !ev.flip;
                let len = t - enter;
                for k in 0..3 {
                    tint[k] *= pass * (len * m.absorption * base[k].max(1e-30).log2()).exp2();
                }
                if tint[0].max(tint[1]).max(tint[2]) < 1e-3 {
                    return [0.0; 3];
                }
                crossings += 1;
                t += e * 2.0;
                if t > max_t {
                    break;
                }
                continue;
            }
            // Inside rendering: the estimate is a fixed step in the set, not a distance; only hits count.
            if softness > 0.0 && !ev.inside() {
                res = res.min(d / (softness * t));
            }
            t += (d * self.scene.march.step).max(e);
            if t > max_t {
                break;
            }
        }
        let r = res.clamp(0.0, 1.0);
        let r = r * r * (3.0 - 2.0 * r);
        tint.map(|v| v * r)
    }

    /// Ambient occlusion and the light of emissive colors at `p`. Both use one random ray into
    /// the hemisphere (ray traced occlusion counts hits within the occlusion radius, the
    /// emission of whatever the ray hits within the emission reach adds to the light).
    fn ambient_occlusion(&self, ev: &mut Evaluator, p: V3, n: V3, t: f64, rng: &mut Rng) -> (f64, [f64; 3]) {
        let l = &self.scene.lighting;
        let m = &self.scene.material;
        let radius = l.ao_radius * t;
        let light = self.emissive && m.emission_light > 0.0;
        let traced = l.ao_mode == AoMode::Raytraced;
        let mut ao = if traced { 1.0 } else { self.ao_fast(ev, p, n, t) };
        let mut emitted = [0.0; 3];
        if traced || light {
            let d = cosine_dir(n, rng.next_f64(), rng.next_f64());
            let eps = self.eps_at(t);
            let reach = if traced { radius } else { 0.0 }.max(if light { m.emission_reach * t } else { 0.0 });
            let o = p + n * (eps * 2.0);
            if let Some(h) = self.march_rough(ev, o, d, 0.0, reach, if light { 96 } else { 48 }) {
                if traced && h.t < radius {
                    ao = ao_curve((1.0 - h.t / radius).clamp(0.0, 1.0), l.ao_strength);
                }
                if light {
                    let e = self.emission_at(&h.orbit, h.pos, h.eps, t + h.t);
                    emitted = e.map(|v| v * m.emission_light);
                }
            }
            // More rays for the emitted light only; the average of all.
            let rays = if light { m.emission_rays.clamp(1, 8) } else { 1 };
            for _ in 1..rays {
                let d = cosine_dir(n, rng.next_f64(), rng.next_f64());
                if let Some(h) = self.march_rough(ev, o, d, 0.0, m.emission_reach * t, 96) {
                    let e = self.emission_at(&h.orbit, h.pos, h.eps, t + h.t);
                    for k in 0..3 {
                        emitted[k] += e[k] * m.emission_light;
                    }
                }
            }
            emitted = emitted.map(|v| v / rays as f64);
        }
        (ao, emitted)
    }

    /// The fast (distance sampled) ambient occlusion.
    fn ao_fast(&self, ev: &mut Evaluator, p: V3, n: V3, t: f64) -> f64 {
        let l = &self.scene.lighting;
        let radius = l.ao_radius * t;
        match l.ao_mode {
            AoMode::Fast if ev.inside() => 1.0,
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
            _ => 1.0,
        }
    }

    /// Background image color in direction `d` at screen position `sx`, `sy` (-1..1; images in
    /// screen coordinates) in display values in display space, else linear.
    pub fn background_image(&self, d: V3, sx: f64, sy: f64) -> Option<[f64; 3]> {
        let (m, rot) = self.bg_image.as_ref()?;
        let s = if self.scene.background.image_screen {
            m.sample_clamped((sx + 1.0) * 0.5, (1.0 - sy) * 0.5)
        } else {
            let (u, v) = crate::maps::panorama_uv(rot, d.to_array());
            m.sample_panorama(u, v)
        };
        Some(self.image_rgb(s))
    }

    /// Ambient light from the background image around the normal `n`.
    /// Light of the surroundings in direction `d` for path tracing: the ambient sky and ground
    /// colors (or the background image's ambient copy) times the ambient strength.
    fn environment(&self, d: V3) -> [f64; 3] {
        let l = &self.scene.lighting;
        let a = match self.background_ambient(d).filter(|_| l.ambient_from_background) {
            Some(a) => a,
            None => {
                let hemi = d.dot(self.frame.ambient_up) * 0.5 + 0.5;
                [0, 1, 2].map(|k| l.ground_color[k] as f64 + (l.sky_color[k] - l.ground_color[k]) as f64 * hemi)
            }
        };
        a.map(|v| v * l.ambient)
    }

    pub fn background_ambient(&self, n: V3) -> Option<[f64; 3]> {
        let (m, (_, rot)) = (self.bg_ambient.as_ref()?, self.bg_image.as_ref()?);
        let (u, v) = crate::maps::panorama_uv(rot, n.to_array());
        Some(self.image_rgb(m.sample_panorama(u, v)))
    }

    /// A background image sample (blue, green, red) as a color with the image brightness.
    fn image_rgb(&self, s: [f64; 3]) -> [f64; 3] {
        let rgb = [s[2], s[1], s[0]];
        let k = self.scene.background.image_intensity;
        if self.scene.post.display_space { rgb.map(|v| v * k) } else { rgb.map(|v| super::srgb_decode(v) * k) }
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
        let mat = &self.materials[self.material_of(o)];
        // Objects: primitives by their pattern, fractals with their own bailout.
        let mut bailout2 = self.prog.bailout2;
        if let Some(ob) = (o.object > 0).then(|| self.prog.objects.get(o.object as usize - 1)).flatten() {
            match &ob.shape {
                crate::fractal::ObjectShape::Primitive(..) | crate::fractal::ObjectShape::Model(_) => {
                    let q = ob.local(hit.pos);
                    return crate::compose::pattern_value(mat.pattern, mat.pattern_size, q);
                }
                crate::fractal::ObjectShape::Fractal(f) => bailout2 = f.bailout2,
            }
        }
        match mat.mode {
            ColorMode::Iterations => {
                let n_it = o.iterations as f64 + self.scene.material.detail_shift * hit.eps.ln();
                if o.escaped && o.r2 > 1.0 {
                    let lb = bailout2.max(4.0).sqrt().ln();
                    n_it - ((0.5 * o.r2.ln()) / lb).max(1e-10).log2()
                } else {
                    n_it
                }
            }
            ColorMode::IterationsRatio => {
                let n_it = o.iterations as f64 + self.scene.material.detail_shift * hit.eps.ln();
                if o.escaped && o.r2 > 1.0 {
                    let lb = bailout2.max(4.0).sqrt().ln().ln();
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
            ColorMode::Plane | ColorMode::Sphere => position_value(&self.scene.material, mat.mode, hit.pos),
            ColorMode::SecondChoice => o.color2,
        }
    }

    /// Surface and highlight color for the color value `v`.
    /// Surface color from the color map (Mandelbulb3D's diffuse maps, PaintThread): `base` is
    /// the gradient color, `v` the color value, `rough` the roughness (blends toward the map's
    /// average color in the 3D modes).
    /// Also returns the brightness of the map there (its emission).
    fn map_color(&self, h: &Hit, n: V3, v: f64, base: [f64; 3], rough: f64) -> ([f64; 3], f64) {
        let Some((m, rot, avg)) = &self.color_map else { return (base, 0.0) };
        let cm = &self.scene.material.color_map;
        let frac = |x: f64| x - x.floor();
        let mut c = match cm.mode {
            MapMode::Orbit => {
                // Orbit trap (MB3D: 15-bit trap · 4096) and iteration value, both around 0.
                // MB3D's OTrap field holds the second color choice (the orbit trap by default).
                let a = h.orbit.color2 * 32767.0 / 32768.0 - 0.5;
                let b = (v * cm.iter_scale + cm.iter_offset).clamp(0.0, 1.0) - 0.5;
                let (s, co) = cm.rotation[2].to_radians().sin_cos();
                m.rgb_tiled(frac((co * a + s * b + cm.offset[0]) * cm.scale), frac((co * b - s * a + cm.offset[1]) * cm.scale))
            }
            MapMode::Normal => {
                let (u, w) = crate::maps::panorama_uv(rot, n.to_array());
                let s = m.sample_panorama(u, w);
                [s[2], s[1], s[0]]
            }
            MapMode::Wrap | MapMode::Triplanar => {
                let p = h.pos * cm.scale;
                let an = v3(n.x.abs(), n.y.abs(), n.z.abs());
                let wsum = (an.x + an.y + an.z).max(1e-30);
                let wv = [an.x / wsum, an.y / wsum, an.z / wsum];
                let (ox, oy) = (cm.offset[0], cm.offset[1]);
                if cm.mode == MapMode::Wrap {
                    let (sx, sy, sz) = (p.x.sin(), p.y.sin(), p.z.sin());
                    m.rgb_tiled(sy * wv[0] + sx * (wv[1] + wv[2]) + ox, sz * (wv[0] + wv[1]) + sy * wv[2] + oy)
                } else {
                    let a = m.rgb_tiled(p.y + ox, p.z + oy);
                    let b = m.rgb_tiled(p.x + ox, p.z + oy);
                    let c = m.rgb_tiled(p.x + ox, p.y + oy);
                    [0, 1, 2].map(|k| a[k] * wv[0] + b[k] * wv[1] + c[k] * wv[2])
                }
            }
        };
        if cm.mode != MapMode::Orbit && rough > 0.0 {
            c = [0, 1, 2].map(|k| c[k] + (avg[k] - c[k]) * rough);
        }
        if !self.scene.post.display_space {
            c = c.map(super::srgb_decode);
        }
        let y = |v: &[f64; 3]| 0.3 * v[0] + 0.59 * v[1] + 0.11 * v[2];
        let lum = y(&c);
        if cm.luminance_only {
            let f = lum / (0.01 + y(&base));
            return (base.map(|b| b * f), lum);
        }
        (c, lum)
    }

    /// Interior colors for a hit whose orbit did not escape (None: not interior or no interior
    /// colors): indexed by the final radius relative to the bailout (inside rendering: the
    /// iteration value), like Mandelbulb3D's CalcColorsInside.
    fn interior_colors(&self, hit: &Hit, n: V3, dist: f64) -> Option<([f64; 3], [f64; 3])> {
        let m = &self.scene.material;
        if self.lut_int.is_empty() || hit.orbit.escaped {
            return None;
        }
        let v = if self.scene.fractal.inside {
            self.color_value(hit, n) / (self.scene.fractal.max_iterations as f64 + 1.0)
        } else {
            (hit.orbit.r2 / self.prog.bailout2.max(1e-30)).clamp(0.0, 1.0)
        };
        let mut t = v * m.interior_scale + m.interior_offset + m.depth_shift * (dist - m.depth_reference);
        t = if m.repeat { t - t.floor() } else { t.clamp(0.0, 1.0) };
        let x = t * (self.lut_int.len() - 1) as f64;
        let i = (x.floor() as usize).min(self.lut_int.len() - 2);
        let f = x - i as f64;
        let mix = |l: &[[f32; 3]]| [0, 1, 2].map(|k| l[i][k] as f64 + (l[i + 1][k] as f64 - l[i][k] as f64) * f);
        Some((mix(&self.lut_int), mix(&self.lut_int_spec)))
    }

    /// Surface color, highlight color and transparency for the color value `v` of a hit at
    /// distance `dist`.
    fn gradient(&self, mat: usize, v: f64, dist: f64) -> ([f64; 3], [f64; 3], f64, f64) {
        let sm = &self.scene.material;
        let m = &self.materials[mat];
        let mut t = v * m.scale + m.offset + sm.depth_shift * (dist - sm.depth_reference);
        t = if m.repeat { t - t.floor() } else { t.clamp(0.0, 1.0) };
        let x = t * (m.lut.len() - 1) as f64;
        let i = (x.floor() as usize).min(m.lut.len() - 2);
        let f = x - i as f64;
        let mix = |l: &[[f32; 3]]| [0, 1, 2].map(|k| l[i][k] as f64 + (l[i + 1][k] as f64 - l[i][k] as f64) * f);
        let base = mix(&m.lut);
        let first = m.lut[0];
        let base = [0, 1, 2].map(|k| first[k] as f64 + (base[k] - first[k] as f64) * m.saturation);
        let emis = m.lut_emis[i] as f64 + (m.lut_emis[i + 1] as f64 - m.lut_emis[i] as f64) * f;
        // Glow bands at the exact gradient position (not through the table).
        let (base, emis) = crate::gradient::apply_bands(&m.bands, t, m.repeat, base, emis);
        (base, mix(&m.lut_spec), m.lut_trans[i] as f64 + (m.lut_trans[i + 1] as f64 - m.lut_trans[i] as f64) * f, emis)
    }

    /// Linear radiance of one sample of pixel (`px`, `py`).
    pub fn sample(&self, ev: &mut Evaluator, px: u32, py: u32, sample: u32) -> [f64; 3] {
        self.sample_aux(ev, px, py, sample).color
    }

    /// One sample of pixel (`px`, `py`) with the denoiser's helper values. Anaglyphs render
    /// both eyes and mix them.
    pub fn sample_aux(&self, ev: &mut Evaluator, px: u32, py: u32, sample: u32) -> Sample {
        let v = &self.frame.view;
        if v.eye_count() == 1 {
            return self.sample_eye(ev, px, py, sample, 0);
        }
        let l = self.sample_eye(ev, px, py, sample, 0);
        let r = self.sample_eye(ev, px, py, sample, 1);
        Sample { color: v.anaglyph(l.color, r.color), albedo: v.anaglyph(l.albedo, r.albedo), normal: (l.normal + r.normal) * 0.5, depth: (l.depth + r.depth) * 0.5, hit: (l.hit + r.hit) * 0.5 }
    }

    /// One sample of pixel (`px`, `py`) for image `k` (the eye of an anaglyph).
    fn sample_eye(&self, ev: &mut Evaluator, px: u32, py: u32, sample: u32, k: u32) -> Sample {
        let sc = self.scene;
        let fr = &self.frame;
        let mut rng = Rng::new(px, py, sample);
        let (jx, jy) = if sample == 0 { (0.5, 0.5) } else { (rng.next_f64(), rng.next_f64()) };
        // Motion blur: a random time within the exposure.
        let moved;
        let cam = match &sc.motion_end {
            Some(end) => {
                moved = sc.camera.lerp(end, rng.next_f64());
                &moved
            }
            None => &sc.camera,
        };
        let (sx, sy, eye) = fr.view.eye_screen(px as f64 + jx, py as f64 + jy, fr.width, fr.height, k);
        let Some((mut o, mut d)) = fr.view.ray(cam, sx, sy, fr.aspect, eye) else {
            return Sample { color: [0.0; 3], albedo: [0.0; 3], normal: V3::ZERO, depth: 0.0, hit: 0.0 };
        };
        if sc.dof.enabled && sample > 0 {
            // Perspective: focus on a plane; other projections: on a sphere, with the lens
            // across the ray.
            let plane = fr.view.projection == crate::scene::Projection::Perspective;
            let focus = o + d * if plane { sc.dof.focus_distance / d.dot(cam.forward) } else { sc.dof.focus_distance };
            let (lr, lu) = if plane { (cam.right, cam.up) } else { lens_axes(d, cam) };
            let (u1, u2) = (rng.next_f64(), rng.next_f64());
            let (lx, ly) = sc.dof.lens_point(u1, u2);
            let r = sc.dof.aperture * sc.dof.focus_distance * 0.1;
            o = o + lr * (r * lx) + lu * (r * ly);
            d = (focus - o).normalize();
        }
        let t0 = super::start_distance(sc);
        let m = &sc.material;
        let amount = m.reflection;
        let bounces = m.reflection_bounces.clamp(1, 8);
        // Glass scales the diffuse light down by the transparency on every surface a path
        // continues from (Mandelbulb3D's bScaleAmbDiffDown).
        let glass = if m.transmission && amount > 0.0 { amount } else { 0.0 };
        let first = self.trace(ev, Seg { o, d, t0, max_t: sc.march.max_distance, t_base: 0.0, sx, sy, inside: false, glass, env: false }, &mut rng);
        let mut c = first.color;
        // Reflections and transmission (Mandelbulb3D's CalcSR): each part of the path adds the
        // scene it sees, weighted by the reflectance or transmittance of the surfaces before it,
        // the light amount and how much of those surfaces the fog leaves visible. Where Mandelbulb3D
        // follows both the reflected and the refracted ray, one of them is picked by its weight.
        // Path tracing continues every path: at each surface either the reflection / glass
        // (probability `ps`) or a diffuse bounce (cosine weighted around the normal, weighted by
        // the surface color), Russian roulette from the third bounce, the light of each bounce
        // limited by the firefly clamp.
        let path = sc_path(sc);
        let max_b = if path { sc.lighting.path_bounces.clamp(1, 16) } else if amount > 0.0 || self.water { bounces } else { 0 };
        if max_b > 0 {
            let mut weight = [1.0; 3];
            let mut surf = first.surface;
            let mut inside = false;
            for b in 0..max_b {
                let Some(s) = surf else { break };
                let vis = (1.0 - s.fog).clamp(0.0, 1.0);
                // Water reflects by its own amount.
                let amount = s.mirror.unwrap_or(amount);
                let (dir, refract, diffuse) = if path {
                    let ps = match s.mirror {
                        Some(m) => m,
                        None => path_specular_share(amount, s.spec, s.trans, glass),
                    };
                    if ps > 0.0 && rng.next_f64() < ps {
                        let (dir, f, refract) = self.bounce(&s, inside, &mut rng);
                        for (w, fk) in weight.iter_mut().zip(f) {
                            *w *= fk * amount / ps * vis;
                        }
                        (dir, refract, false)
                    } else {
                        let (u1, u2) = (rng.next_f64(), rng.next_f64());
                        let keep = (1.0 - s.trans * glass).max(0.0) * (1.0 - s.mirror.unwrap_or(0.0));
                        for (w, bk) in weight.iter_mut().zip(s.base) {
                            *w *= bk * keep / (1.0 - ps) * vis;
                        }
                        (cosine_dir(s.n, u1, u2), false, true)
                    }
                } else {
                    let (dir, f, refract) = self.bounce(&s, inside, &mut rng);
                    for (w, fk) in weight.iter_mut().zip(f) {
                        *w *= fk * amount * vis;
                    }
                    (dir, refract, false)
                };
                if path && b >= 2 {
                    let q = weight[0].max(weight[1]).max(weight[2]).clamp(0.05, 1.0);
                    if rng.next_f64() > q {
                        break;
                    }
                    weight = weight.map(|w| w / q);
                }
                if weight.iter().all(|w| *w < 1e-4) {
                    break;
                }
                // Refracted rays start on the far side of the surface, reflected ones at the
                // shading point. Both are marched as a continuation of the path, so the surface
                // threshold keeps growing with the distance travelled.
                let start = if refract { s.hit - s.n * s.eps } else { s.pos };
                if refract {
                    inside = !inside;
                    ev.flip = inside;
                }
                let c0 = &sc.camera;
                let syr = (2.0 * dir.dot(c0.up).clamp(-1.0, 1.0).asin() / c0.fov.to_radians()).clamp(-1.0, 1.0);
                let sxr = (dir.dot(c0.right).atan2(dir.dot(c0.forward)) / (fr.tan_half * fr.aspect).atan()).clamp(-1.0, 1.0);
                // Sky reach: diffuse rays beyond it see the open sky (path_reach).
                let reach = if diffuse && sc.lighting.path_reach > 0.0 { (sc.lighting.path_reach * s.t).min(sc.march.max_distance) } else { sc.march.max_distance };
                let seg = Seg { o: start - dir * s.t, d: dir, t0: s.t, max_t: s.t + reach, t_base: s.t, sx: sxr, sy: syr, inside, glass: if b + 1 < max_b { glass } else { 0.0 }, env: path };
                let tr = self.trace(ev, seg, &mut rng);
                if inside {
                    // Absorbed by the color where the ray entered; light scattered on the way.
                    let len = tr.surface.map_or(sc.march.max_distance, |e| e.t - s.t);
                    let ab = s.base.map(|v| (len * m.absorption * v.max(1e-30).log2()).exp2());
                    let sc_amt = (1.0 - (0.3 * ab[0] + 0.59 * ab[1] + 0.11 * ab[2])) * m.scattering / 330.0;
                    if sc_amt > 0.0 {
                        let end = tr.surface.map_or(start + dir * len, |e| e.hit);
                        let light = self.light_sum(start, end);
                        for k in 0..3 {
                            c[k] += light[k] * s.base[k] * weight[k] * (ab[k] + 1.0) * 0.5 * sc_amt;
                        }
                    }
                    for (w, a) in weight.iter_mut().zip(ab) {
                        *w *= a;
                    }
                }
                let mut add: [f64; 3] = std::array::from_fn(|k| weight[k] * tr.color[k]);
                let limit = sc.lighting.path_clamp;
                let y = 0.2126 * add[0] + 0.7152 * add[1] + 0.0722 * add[2];
                if path && limit > 0.0 && y > limit {
                    add = add.map(|v| v * limit / y);
                }
                for k in 0..3 {
                    c[k] += add[k];
                }
                surf = tr.surface;
            }
            ev.flip = false;
        }
        let (depth, hit) = first.surface.map_or((0.0, 0.0), |s| (s.t, 1.0));
        Sample { color: c, albedo: first.albedo, normal: first.normal, depth, hit }
    }

    /// Direction of the path after surface `s` (`inside`: the ray travels inside glass), the
    /// factor of the path weight and whether the ray was refracted. Mirrors CalcSR.CalcRay:
    /// Fresnel reflectance and total internal reflection.
    fn bounce(&self, s: &Surface, inside: bool, rng: &mut Rng) -> (V3, [f64; 3], bool) {
        let m = &self.scene.material;
        // Rough surfaces: reflect and refract on a random micro facet (GGX); facets facing away
        // from the ray fall back to the surface normal.
        let gloss = m.gloss.clamp(0.0, 1.0);
        let n = if gloss > 0.0 {
            let (u1, u2) = (rng.next_f64(), rng.next_f64());
            let f = ggx_normal(s.n, gloss * gloss, u1, u2);
            if f.dot(s.dir) < 0.0 { f } else { s.n }
        } else {
            s.n
        };
        let mut reflect = s.dir - n * (2.0 * s.dir.dot(n));
        // Below the surface: mirrored back above it.
        if reflect.dot(s.n) < 0.0 {
            reflect = reflect - s.n * (2.0 * reflect.dot(s.n));
        }
        let s = &Surface { n, ..*s };
        let a = s.trans;
        if !m.transmission || a <= 0.0 {
            return (reflect, s.spec, false);
        }
        let eta = if inside { m.refraction_index } else { 1.0 / m.refraction_index };
        let ci = -s.dir.dot(s.n);
        let k = 1.0 - eta * eta * (1.0 - ci * ci);
        let smax = s.spec[0].max(s.spec[1]).max(s.spec[2]);
        if k <= 0.0 {
            // Total internal reflection.
            let g = 1.0 - a + a * a / (smax + 0.01);
            return (reflect, s.spec.map(|v| v * g), false);
        }
        let ct = k.sqrt();
        let t = (s.dir * eta + s.n * (eta * ci - ct)).normalize();
        let ci = ci.abs();
        let sq = |x: f64| x * x;
        let fr = if ct > 1e-16 { (sq((ci - eta * ct) / (ci + eta * ct)) + sq((ct - eta * ci) / (ct + eta * ci))) * 0.5 } else { ci };
        let wt = ((1.0 - fr * smax) * a).max(0.0);
        let wr = s.spec.map(|v| v * (fr + (1.0 - fr) * (1.0 - a)));
        let lr = (wr[0] + wr[1] + wr[2]) / 3.0;
        if wt + lr <= 0.0 {
            return (reflect, [0.0; 3], false);
        }
        let p = wt / (wt + lr);
        if rng.next_f64() < p { (t, [wt / p; 3], true) } else { (reflect, wr.map(|v| v / (1.0 - p)), false) }
    }

    /// Light reaching the segment `a`..`b` (Mandelbulb3D's AddLight): point lights by their
    /// squared distance to it.
    fn light_sum(&self, a: V3, b: V3) -> [f64; 3] {
        let mut sum = [0.0; 3];
        let ab = b - a;
        let l2 = ab.dot(ab).max(1e-30);
        for li in &self.frame.lights {
            let w = match li.point {
                Some(lp) => {
                    let s = ((lp - a).dot(ab) / l2).clamp(0.0, 1.0);
                    let v = lp - (a + ab * s);
                    1.0 / v.dot(v).max(1e-12)
                }
                None => 1.0,
            };
            for (s, c) in sum.iter_mut().zip(li.color) {
                *s += c * w;
            }
        }
        sum
    }

    /// Traces one part of a path: its color and the surface it hit.
    fn trace(&self, ev: &mut Evaluator, seg: Seg, rng: &mut Rng) -> Traced {
        let Seg { o, d, t0, max_t, t_base, sx, sy, inside, glass, env } = seg;
        let path = sc_path(self.scene);
        let sc = self.scene;
        let fr = &self.frame;
        // Glow and steam: the light of emissive parts scattered near their surfaces along the
        // ray. One point is picked in proportion to the weight (falloff from the surface per
        // length, for the steam times its noise density); a random ray from it toward the
        // nearest surface (cosine weighted around the falling distance) finds the emitted light
        // of that surface, so glow and steam take the color of the glowing parts themselves.
        let steam_on = self.emissive && (sc.material.steam > 0.0 || sc.material.emission_glow > 0.0);
        // Fractal haze along the ray (not inside glass).
        let haze_on = sc.fog.haze > 0.0 && !inside;
        let mut pick = SteamPick { steam: steam_on, seed: if steam_on { (rng.next_f64() * 16_777_216.0) as u32 } else { 0 }, haze: haze_on, haze_keep: 1.0, ..SteamPick::default() };
        let (hit, _, steps) = self.march_ex(ev, o, d, t0, max_t, sc.march.max_steps, true, (steam_on || haze_on).then_some(&mut pick));
        let mut halo = [0.0; 3];
        if steam_on {
            let (u1, u2) = (rng.next_f64(), rng.next_f64());
            if pick.w > 0.0 {
                // Toward the surface: around the falling distance (tetrahedron differences).
                let hh = pick.rs * 0.05;
                let mut g = V3::ZERO;
                for k in STEAM_TETRA {
                    let kv = v3(k[0], k[1], k[2]);
                    g += kv * ev.orbit(pick.pos + kv * hh, self.eps_at(pick.t) * 0.5).de;
                }
                let n = if g.length() > 0.0 { -g.normalize() } else { -d };
                let dir = cosine_dir(n, u1, u2);
                if let Some(h) = self.march_rough(ev, pick.pos, dir, 0.0, 8.0 * pick.rs, 64) {
                    let e = self.emission_at(&h.orbit, h.pos, h.eps, pick.t + h.t);
                    let f = pick.w * if pick.fall { (-h.t / pick.rs).exp() } else { 1.0 };
                    for k in 0..3 {
                        halo[k] += e[k] * f;
                    }
                }
            }
        }
        let (glow, glow_mix) = sc.fog.glow_at(steps, sc.march.max_steps as f64);
        let grad = self.background(d, sy);
        let bg = if env { self.environment(d) } else { self.background_image(d, sx, sy).unwrap_or(grad) };
        let fog_col = if sc.fog.from_background { grad } else { sc.fog.color.map(|x| x as f64) };
        let (near, far) = (super::start_distance(sc), sc.march.max_distance);
        let albedo: [f64; 3];
        let mut normal = V3::ZERO;
        let mut surface = None;
        let mut c = match &hit {
            None => {
                // Inside glass there is no fog (CalcOpenAir).
                let f = if inside { 0.0 } else { sc.fog.amount(far, near, far) };
                // The fog can lie on top of a background image instead of covering it.
                let cover = if sc.background.image_fog_add && self.bg_image.is_some() { 0.0 } else { 1.0 };
                let c = [0, 1, 2].map(|k| bg[k] * (1.0 - f.max(0.0) * cover) + fog_col[k] * f.max(0.0));
                albedo = c.map(|v| v.clamp(0.0, 1.0));
                c
            }
            Some(h) => {
                let (n, rough) = self.normal_rough(ev, h.pos, h.eps, h.t);
                // Mandelbulb3D's color on iteration: the colors from another point of the orbit.
                let recolored = ev.color_orbit(h.pos, &h.orbit).map(|orbit| Hit { orbit, ..*h });
                let h = recolored.as_ref().unwrap_or(h);
                let n = if n.dot(d) > 0.0 { -n } else { n };
                // Roughness dulls the highlights toward 1/p (Phong p ~ shininess / 4) and
                // softens the diffuse curves (PaintThread.CalcPixelColor2).
                let rr = rough * sc.lighting.roughness;
                let cv = self.color_value(h, n);
                let main = h.orbit.object == 0;
                let mat = self.material_of(&h.orbit);
                // 3D models: their colors, specular and bump maps.
                let (base, spec_col, trans, emis, n_model, model_emis) = match self.interior_colors(h, n, h.t).filter(|_| main) {
                    Some((b, s)) => (b, s, 0.0, 0.0, n, [0.0; 3]),
                    None => {
                        let (b, s, tr, e) = self.gradient(mat, cv, h.t);
                        let (b, sf, nm, me) = self.model_surface(h, mat, b, n);
                        (b, s.map(|c| c * sf), tr, e, nm, me)
                    }
                };
                let (lay, n_bump) = self.apply_layers(mat, h, n_model, cv, base, emis);
                let (base, emis, spec_col) = (lay.base, lay.emis, spec_col.map(|c| c * lay.spec));
                // Roughness layers.
                let rr = rr * lay.rough_keep + lay.rough_add;
                let spec_rough = 1.0 + (2.0 * rr).min(1.0) * (4.0 / sc.lighting.shininess.max(1.0) - 1.0);
                // Color maps belong to the main fractal; their bright parts can shine.
                let (base, emis) = if main {
                    let (b, lum) = self.map_color(h, n, cv, base, rr);
                    (b, emis + lum * sc.material.color_map.emission)
                } else {
                    (base, emis)
                };
                // Shimmer: the emitted light flickers over the surface (and in time).
                let emis = if emis > 0.0 { emis * super::shimmer(&sc.material, h.pos) } else { emis };
                // Water: foam on the crests, and its own (Fresnel) reflection.
                let (base, mirror) = match self.water.then(|| self.water_at(&h.orbit, h.pos, -d.dot(n), base)).flatten() {
                    Some((color, refl)) => (color, Some(refl)),
                    None => (base, None),
                };
                // Water glints: a far sharper sun highlight than the scene's shininess.
                let shininess = if mirror.is_some() { sc.lighting.shininess.max(WATER_SHININESS) } else { sc.lighting.shininess };
                // Shading point just off the surface; inside rendering puts the surface one
                // threshold into the solid, so it needs a larger offset.
                let p = h.pos + n * (h.eps * if ev.inside() { 4.0 } else { 1.5 });
                // Path tracing: the bounces bring the ambient and emitted light.
                let (ao, emitted) = if path { (1.0, [0.0; 3]) } else { self.ambient_occlusion(ev, p, n, h.t, rng) };
                // Lighting uses the bumped normal.
                let n = n_bump;
                let l = &sc.lighting;
                let hemi = n.dot(fr.ambient_up) * 0.5 + 0.5;
                let amb = match self.background_ambient(n).filter(|_| l.ambient_from_background) {
                    // The image light blends into the fog color by the fog at infinity
                    // (PaintThread: bUseSmallBGpicForAmb).
                    Some(a) => {
                        let fi = sc.fog.amount(far, near, far).clamp(0.0, 1.0);
                        let keep = if sc.background.image_fog_add { 1.0 } else { 1.0 - fi };
                        [0, 1, 2].map(|k| a[k] * keep + fog_col[k] * fi)
                    }
                    None => [0, 1, 2].map(|k| l.ground_color[k] as f64 + (l.sky_color[k] - l.ground_color[k]) as f64 * hemi),
                };
                let ambient = if path { 0.0 } else { l.ambient };
                let mut diffuse = [0, 1, 2].map(|k| amb[k] * ambient * ao + emitted[k]);
                let mut spec = [0.0; 3];
                for li in &fr.lights {
                    // Direction, falloff and shadow length; area and line lights at a random
                    // point of the light.
                    let (r1, r2) = if li.sampled() { (rng.next_f64(), rng.next_f64()) } else { (0.5, 0.5) };
                    let (ldir, atten, max_t) = li.sample(p, r1, r2, sc.march.max_distance);
                    if atten <= 0.0 {
                        continue;
                    }
                    let ndl = n.dot(ldir);
                    let mut df = li.diffuse.apply(ndl);
                    if rr > 0.0 {
                        df += (super::rough_diffuse(li.diffuse.index() as usize, ndl) - df) * rr;
                    }
                    // Back faces: only wrapping curves reach them, and not through shadows.
                    if df <= 0.0 || (ndl <= 0.0 && li.shadows) {
                        continue;
                    }
                    let sh = if li.shadows { self.soft_shadow(ev, p, ldir, h.eps, li.softness, max_t, h.t) } else { [1.0; 3] };
                    if sh[0].max(sh[1]).max(sh[2]) <= 0.0 {
                        continue;
                    }
                    let w = atten * (1.0 - li.occlusion * (1.0 - ao));
                    let hv = (ldir - d).normalize();
                    let s = if ndl > 0.0 { n.dot(hv).max(0.0).powf(shininess) * li.specular * sc.material.specular * (shininess + 8.0) / 25.0 * spec_rough } else { 0.0 };
                    for k in 0..3 {
                        diffuse[k] += li.color[k] * df * w * sh[k];
                        spec[k] += li.color[k] * s * w * sh[k];
                    }
                }
                // What the reflection takes (water) does not reach the surface color.
                let keep = (1.0 - trans * glass).max(0.0) * (1.0 - mirror.unwrap_or(0.0));
                // Emissive colors shine by themselves.
                let lit = [0, 1, 2].map(|k| base[k] * (diffuse[k] * keep + emis) + spec[k] * spec_col[k] + model_emis[k]);
                let f = if inside { 0.0 } else { sc.fog.amount(h.t - t_base, near, far) };
                albedo = [0, 1, 2].map(|k| (base[k] * (1.0 - f) + fog_col[k] * f.max(0.0)).clamp(0.0, 1.0));
                normal = n;
                surface = Some(Surface { pos: p, hit: h.pos, eps: h.eps, n, dir: d, t: h.t, spec: spec_col, base, trans, fog: f, mirror });
                [0, 1, 2].map(|k| lit[k] * (1.0 - f) + fog_col[k] * f.max(0.0))
            }
        };
        // Glow and steam of the emissive colors the ray passed (strengths included).
        for k in 0..3 {
            c[k] += halo[k];
        }
        // Light scattered by the fog: one random light, one point along the ray (positional
        // lights: equiangular sampling around the light, which keeps the haze near it smooth;
        // directional lights: in proportion to the fog added there), lit through a shadow ray.
        if !inside && sc.fog.scatter > 0.0 && !fr.lights.is_empty() {
            let (ut, ul, r1, r2) = (rng.next_f64(), rng.next_f64(), rng.next_f64(), rng.next_f64());
            let end = hit.as_ref().map_or(far, |h| h.t - t_base);
            let a = sc.fog.amount(0.0, near, far).clamp(0.0, 1.0);
            let b = sc.fog.amount(end, near, far).clamp(0.0, 1.0);
            let count = fr.lights.len();
            let li = &fr.lights[((ul * count as f64) as usize).min(count - 1)];
            let start = o + d * t_base;
            let sample = match li.point {
                _ if b - a <= 1e-6 => None,
                Some(p0) => {
                    let center = match li.shape {
                        super::LightShape::Line { end } => p0 + (end - p0) * 0.5,
                        _ => p0,
                    };
                    let delta = (center - start).dot(d);
                    let dist = (center - start - d * delta).length().max(1e-6 * end);
                    let (ta, tb) = ((-delta / dist).atan(), ((end - delta) / dist).atan());
                    let s = delta + dist * (ta + ut * (tb - ta)).tan();
                    let pdf = dist / ((tb - ta).max(1e-12) * (dist * dist + (s - delta) * (s - delta)));
                    Some((s.clamp(0.0, end), sc.fog.density_at(s, near, far) / pdf))
                }
                None => Some((sc.fog.distance_for(a + ut * (b - a), near, far, end), b - a)),
            };
            if let Some((s, amount)) = sample {
                let x = start + d * s;
                let (r1, r2) = if li.sampled() { (r1, r2) } else { (0.5, 0.5) };
                let (ldir, atten, max_l) = li.sample(x, r1, r2, sc.march.max_distance);
                if atten > 0.0 {
                    let sh = if li.shadows { self.soft_shadow(ev, x, ldir, self.eps_at(t_base + s), li.softness, max_l, t_base + s) } else { [1.0; 3] };
                    let w = atten * sc.fog.scatter_phase(ldir.dot(d)) * sc.fog.scatter * amount * count as f64;
                    for k in 0..3 {
                        c[k] += li.color[k] * w * sh[k];
                    }
                }
            }
        }
        // The fractal haze in front of everything the ray saw.
        let mut albedo = albedo;
        if haze_on {
            let keep = pick.haze_keep;
            for k in 0..3 {
                c[k] = c[k] * keep + pick.haze_light[k];
                albedo[k] = albedo[k] * keep + (1.0 - keep) * (pick.haze_light[k] / (1.0 - keep).max(1e-9)).clamp(0.0, 1.0);
            }
        }
        if inside {
            // Inside glass: no glow and no visible lights.
            return Traced { color: c, albedo, normal, surface };
        }
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

/// A loaded color map: image, rotation matrix (normal mode), average color.
pub type ColorMapData = (std::sync::Arc<crate::maps::Map>, [[f64; 3]; 3], [f64; 3]);

/// What [`Shader::trace`] found.
struct Traced {
    color: [f64; 3],
    albedo: [f64; 3],
    normal: V3,
    surface: Option<Surface>,
}

/// One part of a light path: from `o` along `d`, distances `t0`..`max_t`; fog, glow and
/// visible lights measure from `t_base`; `sx`, `sy` are the screen position of the direction
/// for the background. `inside`: the ray travels inside glass. `glass`: the light amount that scales the
/// diffuse light down by the transparency (0: none).
#[derive(Clone, Copy)]
struct Seg {
    o: V3,
    d: V3,
    t0: f64,
    max_t: f64,
    t_base: f64,
    sx: f64,
    sy: f64,
    inside: bool,
    glass: f64,
    /// Path tracing bounce: a miss sees the environment light instead of the background.
    env: bool,
}

/// A surface hit, as far as a reflection or refraction needs it.
#[derive(Clone, Copy)]
struct Surface {
    /// Shading point (just off the surface, on the side of the ray).
    pos: V3,
    /// Hit point and its surface threshold.
    hit: V3,
    eps: f64,
    /// Normal, facing the ray.
    n: V3,
    /// Direction of the ray that hit it.
    dir: V3,
    /// Path length to the hit.
    t: f64,
    /// Highlight color (reflectance).
    spec: [f64; 3],
    /// Surface color (absorption inside glass) and transparency.
    base: [f64; 3],
    trans: f64,
    /// Fog amount at the hit.
    fog: f64,
    /// The surface's own reflection (water), instead of the material's reflection amount.
    mirror: Option<f64>,
}

/// One pixel sample: radiance and the denoiser's helper values (albedo, world normal).
pub struct Sample {
    pub color: [f64; 3],
    pub albedo: [f64; 3],
    pub normal: V3,
    /// Distance of the first hit from the camera (0 for misses) and whether there is one (0..1;
    /// anaglyphs average both eyes).
    pub depth: f64,
    pub hit: f64,
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

/// A random micro facet normal around `n` for the GGX roughness `alpha` (same frame as
/// [`cosine_dir`]).
pub fn ggx_normal(n: V3, alpha: f64, u1: f64, u2: f64) -> V3 {
    let a = if n.z.abs() < 0.999 { v3(0.0, 0.0, 1.0) } else { v3(1.0, 0.0, 0.0) };
    let t = a.cross(n).normalize();
    let b = n.cross(t);
    let c2 = ((1.0 - u1) / (1.0 + (alpha * alpha - 1.0) * u1)).clamp(0.0, 1.0);
    let (ct, st) = (c2.sqrt(), (1.0 - c2).sqrt());
    let phi = u2 * std::f64::consts::TAU;
    (t * (st * phi.cos()) + b * (st * phi.sin()) + n * ct).normalize()
}

/// Progressive CPU renderer: each `render_pass` adds one sample per pixel.
pub struct CpuRenderer {
    pub scene: Scene,
    prog: Program,
    width: u32,
    height: u32,
    accum: Vec<[f64; 3]>,
    /// Summed albedo and normal per pixel.
    aux: Vec<[f64; 8]>,
    samples: u32,
    /// Index of the first sample in the sums (network renders split the samples).
    first: u32,
    /// Renders the region at (left, top) of a full image (full width, full height): the same
    /// rays and random numbers as that image's pixels (tiled renders).
    tile: Option<[u32; 4]>,
}

impl CpuRenderer {
    pub fn new(scene: Scene, width: u32, height: u32) -> Self {
        let scene = crate::animation::still_scene(&scene).unwrap_or(scene);
        let prog = Program::for_scene(&scene);
        let n = (width * height) as usize;
        CpuRenderer { scene, prog, width, height, accum: vec![[0.0; 3]; n], aux: vec![[0.0; 8]; n], samples: 0, first: 0, tile: None }
    }

    /// Starts again at sample `first` with empty sums: the next passes are samples `first`,
    /// `first + 1` … of a whole render (same random numbers), averaged by themselves.
    pub fn start_at_sample(&mut self, first: u32) {
        self.accum.iter_mut().for_each(|a| *a = [0.0; 3]);
        self.aux.iter_mut().for_each(|a| *a = [0.0; 8]);
        self.samples = first;
        self.first = first;
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Renders this renderer's size as the region at `[left, top]` of an image of
    /// `[full width, full height]` (None: the whole image). Starts again.
    pub fn set_tile(&mut self, tile: Option<[u32; 4]>) {
        self.tile = tile;
        self.start_at_sample(0);
    }

    /// Renders the next passes with `scene` but keeps the samples so far (motion blur: the scene
    /// moves within one frame; the size stays).
    pub fn set_scene_keep(&mut self, scene: Scene) {
        let scene = crate::animation::still_scene(&scene).unwrap_or(scene);
        self.prog = Program::for_scene(&scene);
        self.scene = scene;
    }

    pub fn program(&self) -> &Program {
        &self.prog
    }

    pub fn render_pass(&mut self) {
        let sc = shading_scene(&self.scene);
        let [ox, oy, fw, fh] = self.tile.unwrap_or([0, 0, self.width, self.height]);
        let shader = Shader::new(&sc, &self.prog, fw, fh);
        let (w, sample) = (self.width, self.samples);
        let prog = &self.prog;
        self.accum.par_chunks_mut(w as usize).zip(self.aux.par_chunks_mut(w as usize)).enumerate().for_each_init(
            || prog.evaluator(),
            |ev, (y, (row, aux_row))| {
                for (x, (acc, aux)) in row.iter_mut().zip(aux_row.iter_mut()).enumerate() {
                    let s = shader.sample_aux(ev, x as u32 + ox, y as u32 + oy, sample);
                    let n = s.normal.to_array();
                    for k in 0..3 {
                        acc[k] += s.color[k];
                        aux[k] += s.albedo[k];
                        aux[k + 3] += n[k];
                    }
                    {
                        aux[6] += s.depth * s.hit;
                        aux[7] += s.hit;
                    }
                }
            },
        );
        self.samples += 1;
    }

    /// The averaged image with the denoiser's helper images.
    pub fn hdr_image(&self) -> super::denoise::HdrImage {
        let n = (self.samples - self.first).max(1) as f64;
        let mut img = super::denoise::HdrImage { width: self.width, height: self.height, ..Default::default() };
        for (a, x) in self.accum.iter().zip(&self.aux) {
            img.color.extend(a.iter().map(|v| (v / n) as f32));
            img.albedo.extend(x[..3].iter().map(|v| (v / n) as f32));
            // Averaged, not renormalized: OIDN takes shorter normals where hits and misses mix.
            img.normal.extend(x[3..6].iter().map(|v| (v / n) as f32));
            img.alpha.push((x[7] / n) as f32);
            img.depth.push(if x[7] > 0.0 { (x[6] / x[7]) as f32 } else { f32::INFINITY });
        }
        img
    }

    /// The image as 8-bit RGBA after post processing.
    pub fn rgba8(&self) -> Vec<u8> {
        let n = (self.samples - self.first).max(1) as f64;
        let (w, h) = (self.width as f64, self.height as f64);
        // Bloom works on the whole image (like the GPU preview, without lens flares here).
        let mut lin: Vec<f32> = self.accum.iter().flat_map(|a| a.map(|v| (v / n) as f32)).collect();
        super::bloom::apply(&mut lin, self.width, self.height, &self.scene.post);
        super::chromatic_aberration(&mut lin, self.width, self.height, self.scene.post.chromatic);
        let mut out = Vec::with_capacity(self.accum.len() * 4);
        for (i, a) in lin.as_chunks::<3>().0.iter().enumerate() {
            let (x, y) = ((i as u32 % self.width) as f64, (i as u32 / self.width) as f64);
            let c = post_process(a.map(|v| v as f64), &self.scene.post, (x + 0.5) / w, (y + 0.5) / h);
            out.extend_from_slice(&[(c[0] * 255.0 + 0.5) as u8, (c[1] * 255.0 + 0.5) as u8, (c[2] * 255.0 + 0.5) as u8, 255]);
        }
        out
    }
}

/// Two unit axes across the direction `d` (the lens of depth of field for projections other
/// than the perspective).
pub fn lens_axes(d: V3, cam: &crate::scene::Camera) -> (V3, V3) {
    let r = d.cross(cam.up);
    let r = if r.length() > 1e-6 { r.normalize() } else { cam.right };
    (r, r.cross(d).normalize())
}

/// Screen point (`sx`, `sy` in -1..1 over the whole image) to the camera ray with the scene's
/// projection and stereo layout.
fn screen_ray(scene: &Scene, sx: f64, sy: f64, width: u32, height: u32) -> Option<(V3, V3)> {
    let fx = (sx + 1.0) * 0.5 * width as f64;
    let fy = (1.0 - sy) * 0.5 * height as f64;
    super::view_ray(scene, fx, fy, width, height)
}

/// Distance along the ray through screen point (`sx`, `sy` in -1..1) to the surface, if any
/// (picking, focus distance, navigation speed).
pub fn pick(scene: &Scene, sx: f64, sy: f64, width: u32, height: u32) -> Option<(f64, V3)> {
    let prog = Program::for_scene(scene);
    let shader = Shader::new(scene, &prog, width, height);
    let mut ev = prog.evaluator();
    let (o, d) = screen_ray(scene, sx, sy, width, height)?;
    let (hit, _, _) = shader.march(&mut ev, o, d, super::start_distance(scene), scene.march.max_distance, scene.march.max_steps);
    hit.map(|h| ((h.pos - scene.camera.position).length(), h.pos))
}

/// Share (0..1) of a small disc around a light that the camera sees past the fractal: rays to
/// the light's position (`directional`: toward the direction `p`) and to points around it
/// within `radius` (in units of the view height).
pub fn light_visibility(scene: &Scene, p: V3, directional: bool, radius: f64) -> f64 {
    let prog = Program::for_scene(scene);
    let shader = Shader::new(scene, &prog, 64, 64);
    let mut ev = prog.evaluator();
    let cam = &scene.camera;
    let o = cam.position;
    let (base, dist) = if directional { (p.normalize(), scene.march.max_distance) } else { ((p - o).normalize(), (p - o).length()) };
    let spread = radius * 2.0 * cam.tan_half_fov();
    let (right, up) = {
        let r = base.cross(cam.up);
        let r = if r.length() > 1e-9 { r.normalize() } else { cam.right };
        (r, r.cross(base).normalize())
    };
    let mut seen = 0;
    const N: usize = 13;
    for k in 0..N {
        let (ox, oy) = if k == 0 {
            (0.0, 0.0)
        } else {
            let ring = if k <= 6 { 0.5 } else { 1.0 };
            let a = k as f64 * std::f64::consts::TAU / 6.0 + if k > 6 { 0.5 } else { 0.0 };
            (ring * a.cos() * spread, ring * a.sin() * spread)
        };
        let d = (base + right * ox + up * oy).normalize();
        let (hit, _, _) = shader.march(&mut ev, o, d, super::start_distance(scene), dist, scene.march.max_steps);
        if hit.is_none_or(|h| h.t >= dist * 0.999) {
            seen += 1;
        }
    }
    seen as f64 / N as f64
}

/// Distance along the camera's forward axis to the surface, if any (the view center of every
/// projection and stereo layout: pivots, focus distance).
pub fn pick_center(scene: &Scene) -> Option<(f64, V3)> {
    let prog = Program::for_scene(scene);
    let shader = Shader::new(scene, &prog, 256, 256);
    let mut ev = prog.evaluator();
    let cam = &scene.camera;
    let (hit, _, _) = shader.march(&mut ev, cam.position, cam.forward, super::start_distance(scene), scene.march.max_distance, scene.march.max_steps);
    hit.map(|h| (h.t, h.pos))
}

/// The object at screen position (`sx`, `sy`): 0 the main fractal, k the k-th composed object;
/// None where the ray hits nothing.
pub fn pick_object(scene: &Scene, sx: f64, sy: f64, width: u32, height: u32) -> Option<u32> {
    let prog = Program::for_scene(scene);
    let shader = Shader::new(scene, &prog, width, height);
    let mut ev = prog.evaluator();
    let (o, d) = screen_ray(scene, sx, sy, width, height)?;
    let (hit, _, _) = shader.march(&mut ev, o, d, super::start_distance(scene), scene.march.max_distance, scene.march.max_steps);
    hit.map(|h| h.orbit.object)
}

/// Distance estimate at the camera position (navigation speed).
pub fn distance_at_camera(scene: &Scene) -> f64 {
    let prog = Program::for_scene(scene);
    let mut ev = prog.evaluator();
    ev.de(scene.camera.position, 1e-6)
}
