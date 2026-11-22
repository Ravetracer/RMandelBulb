// SPDX-License-Identifier: LGPL-2.1-or-later

//! Progressive GPU renderer.
//!
//! Every pass adds one sample per pixel to an accumulation buffer. A pass is split into bands
//! of rows (one submission each) so a single dispatch never runs long enough to stall the
//! desktop or trigger a driver timeout; the band height adapts to the measured time. Shaders
//! are compiled on a background thread; the previous image stays visible meanwhile.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use rmb_core::fractal::Program;
use rmb_core::render::Frame;
use rmb_core::render::denoise::HdrImage;
use rmb_core::scene::{AoMode, BackgroundMode, FogMode, Post, Scene};

/// A wgpu device + queue pair.
#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
}

impl GpuContext {
    /// Creates a headless context on the best available adapter.
    pub fn new_headless() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rmb-gpu"),
            required_features: wgpu::Features::empty(),
            required_limits: Self::required_limits(&adapter),
            ..Default::default()
        }))
        .map_err(|e| format!("request_device failed: {e}"))?;
        Ok(Self { device, queue, adapter_name: adapter.get_info().name })
    }

    /// Limits we want: the adapter's storage buffer sizes (big images need big buffers).
    pub fn required_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
        let al = adapter.limits();
        wgpu::Limits {
            max_storage_buffer_binding_size: al.max_storage_buffer_binding_size,
            max_buffer_size: al.max_buffer_size,
            max_texture_dimension_2d: al.max_texture_dimension_2d.min(16384),
            ..wgpu::Limits::default()
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GLight {
    dir: [f32; 4],
    color: [f32; 4],
    opts: [f32; 4],
    glow: [f32; 4],
    glow_col: [f32; 4],
    shape0: [f32; 4],
    shape1: [f32; 4],
    shape2: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    cam_pos: [f32; 4],
    cam_right: [f32; 4],
    cam_up: [f32; 4],
    cam_fwd: [f32; 4],
    size: [u32; 4],
    band: [u32; 4],
    march: [f32; 4],
    frac: [f32; 4],
    julia: [f32; 4],
    ao: [f32; 4],
    sky: [f32; 4],
    ground: [f32; 4],
    bg_top: [f32; 4],
    bg_horizon: [f32; 4],
    bg_bottom: [f32; 4],
    fog: [f32; 4],
    glow: [f32; 4],
    dof: [f32; 4],
    color: [f32; 4],
    color2: [f32; 4],
    cuts: [[f32; 4]; 3],
    slots: [[u32; 4]; 8],
    lights: [GLight; rmb_core::scene::MAX_LIGHTS],
    comb: [f32; 4],
    scales: [f32; 4],
    rot4: [[f32; 4]; 4],
    amb_up: [f32; 4],
    fog2: [f32; 4],
    fog3: [f32; 4],
    glow2: [f32; 4],
    glow3: [f32; 4],
    bg_rot: [[f32; 4]; 3],
    blend: [f32; 4],
    interior: [f32; 4],
    cmap0: [f32; 4],
    cmap1: [f32; 4],
    cmap2: [f32; 4],
    cmap_rot: [[f32; 4]; 3],
    glass: [f32; 4],
    bg2: [f32; 4],
    emis: [f32; 4],
    emis2: [f32; 4],
    emis3: [f32; 4],
    path: [f32; 4],
    ccenter: [f32; 4],
    cdir: [f32; 4],
    c2opt: [f32; 4],
    deep_hi: [f32; 4],
    deep_lo: [f32; 4],
    julia_lo: [f32; 4],
    blend_lo: [f32; 4],
    mot_pos: [f32; 4],
    mot_fwd: [f32; 4],
    mot_right: [f32; 4],
    mot_up: [f32; 4],
    fog4: [f32; 4],
    gloss: [f32; 4],
    shim: [f32; 4],
    mt0: [f32; 4],
    mt1: [f32; 4],
    mt2: [f32; 4],
    mt_pos: [f32; 4],
}

/// Splits an f64 into the high and low f32 of a double-float.
fn split_dd(v: f64) -> (f32, f32) {
    let hi = v as f32;
    (hi, if hi.is_finite() { (v - hi as f64) as f32 } else { 0.0 })
}

/// A composed object (march.wgsl `Obj`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ObjGpu {
    inv: [[f32; 4]; 3],
    pos: [f32; 4],
    prim: [f32; 4],
    frac: [f32; 4],
    julia: [f32; 4],
    misc: [f32; 4],
    mat: [f32; 4],
    mat2: [f32; 4],
    slots: [[u32; 4]; 2],
    size: [f32; 4],
    rep: [f32; 4],
    rep_lim: [f32; 4],
    comb: [f32; 4],
    comb2: [f32; 4],
}

/// Words per model header in the `vols` buffer (march.wgsl `MODEL_HEADER`).
const MODEL_HEADER: usize = 32;

/// The baked models of the objects with their looks (march.wgsl `vols`: a header of
/// `MODEL_HEADER` words per object, then the data).
fn volumes_gpu(prog: &Program) -> Vec<u32> {
    use rmb_core::fractal::ObjectShape;
    let n = prog.objects.len().max(1);
    let mut out = vec![0u32; MODEL_HEADER * n];
    let f = |x: f64| (x as f32).to_bits();
    for (j, o) in prog.objects.iter().enumerate() {
        let ObjectShape::Model(Some(m)) = &o.shape else { continue };
        let (v, look) = (&m.vol, &m.look);
        let h = MODEL_HEADER * j;
        out[h..h + 3].copy_from_slice(&v.res);
        out[h + 3] = out.len() as u32;
        for k in 0..3 {
            out[h + 4 + k] = f(v.min[k]);
            out[h + 8 + k] = f(v.bounds[0][k]);
            out[h + 11 + k] = f(v.bounds[1][k]);
        }
        out[h + 7] = f(v.voxel);
        out.extend(v.dist.chunks(2).map(|c| c[0] as u16 as u32 | (c.get(1).copied().unwrap_or(0) as u16 as u32) << 16));
        out[h + 14] = out.len() as u32;
        out.extend(v.tris.iter().flatten().map(|x| x.to_bits()));
        out[h + 15] = out.len() as u32;
        out[h + 16] = v.nodes.len() as u32;
        for nd in &v.nodes {
            out.extend(nd.lo.iter().chain(&nd.hi).map(|x| x.to_bits()));
            out.extend([nd.start, nd.count]);
        }
        if !look.colored && !look.maps {
            continue;
        }
        out[h + 23] = look.colored as u32;
        out[h + 17] = out.len() as u32;
        for (mat, uv) in v.tri_material.iter().zip(&v.tri_uv) {
            out.push(*mat);
            out.extend(uv.iter().map(|x| x.to_bits()));
        }
        if !v.tri_colors.is_empty() {
            out[h + 18] = out.len() as u32;
            out.extend(v.tri_colors.iter().flatten());
        }
        out[h + 19] = out.len() as u32;
        out[h + 20] = look.materials.len() as u32;
        // 16 words per material: color, texture, bump map, bump height, specular map, normal
        // map, normal strength, emission (3), emission texture, 3 unused.
        for lm in &look.materials {
            out.extend(lm.color.map(|c| c.to_bits()));
            let none = u32::MAX;
            out.extend([lm.texture.unwrap_or(none), lm.bump.unwrap_or(none), lm.bump_height.to_bits(), lm.specular.unwrap_or(none), lm.normal.unwrap_or(none), lm.normal_strength.to_bits()]);
            out.extend(lm.emissive.map(|c| c.to_bits()));
            out.extend([lm.emissive_texture.unwrap_or(none), 0, 0, 0]);
        }
        out[h + 21] = out.len() as u32;
        out[h + 22] = look.textures.len() as u32;
        let table = out.len();
        out.resize(table + 4 * look.textures.len(), 0);
        for (k, t) in look.textures.iter().enumerate() {
            let e = table + 4 * k;
            out[e] = out.len() as u32;
            out[e + 1] = t.width;
            out[e + 2] = t.height;
            out[e + 3] = t.wrap[0] as u32 | (t.wrap[1] as u32) << 2 | (t.v_up as u32) << 4;
            out.extend(t.texels.iter().map(|p| p[0] as u32 | (p[1] as u32) << 8 | (p[2] as u32) << 16));
        }
    }
    out
}

/// LUT entries before the material blocks of the objects.
const LUT_OBJECTS: usize = 1152;
/// LUT entries before the texture layers (march.wgsl `LUT_LAYERS`): after 8 object blocks.
const LUT_LAYERS: usize = LUT_OBJECTS + 512 * 8;

/// Image files of the texture layers of all materials (march.wgsl `layer_map`: stored in the
/// maps buffer under 0xffffffef - index).
fn layer_images(scene: &Scene) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lists = std::iter::once(&scene.material.layers).chain(scene.objects.iter().map(|o| &o.material.layers));
    for l in lists.flatten().filter(|l| l.enabled) {
        for name in [l.image.trim(), l.mask.trim()] {
            if !name.is_empty() && !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The texture layers of the main material and the objects' own ones (appended to `lut` at
/// `LUT_LAYERS`: per material 4 layers of 7 entries, see march.wgsl).
fn layers_gpu(scene: &Scene, lut: &mut Vec<[f32; 4]>) {
    let images = layer_images(scene);
    let index = |m: &Option<std::sync::Arc<rmb_core::maps::Map>>, name: &str| match m {
        Some(_) => images.iter().position(|n| n == name.trim()).map_or(-1.0, |i| i as f32),
        None => -1.0,
    };
    lut.resize(LUT_LAYERS, [0.0; 4]);
    let lists = std::iter::once(&scene.material.layers).chain(scene.objects.iter().map(|o| &o.material.layers));
    for (m, layers) in lists.take(9).enumerate() {
        let prepared = rmb_core::texture::prepare(layers);
        for i in 0..rmb_core::texture::MAX_LAYERS {
            let base = LUT_LAYERS + (m * 4 + i) * 7;
            lut.resize(base + 7, [0.0; 4]);
            let Some(p) = prepared.get(i) else {
                lut[base][0] = -1.0;
                continue;
            };
            let l = &p.layer;
            let r = p.rot;
            let texels = p.image.as_ref().map_or(1.0, |m| m.width.max(m.height) as f32);
            let f = |v: f64| v as f32;
            lut[base] = [l.mapping.index() as f32, if l.space == rmb_core::texture::Space::World { 1.0 } else { 0.0 }, l.blend.index() as f32, l.target.index() as f32];
            lut[base + 1] = [f(l.scale), f(l.opacity), f(l.strength), f(l.mask_contrast)];
            lut[base + 2] = [f(l.offset[0]), f(l.offset[1]), f(l.offset[2]), index(&p.image, &l.image)];
            lut[base + 3] = [l.color[0], l.color[1], l.color[2], index(&p.mask, &l.mask)];
            lut[base + 4] = [f(r[0][0]), f(r[0][1]), f(r[0][2]), if l.mask_invert { 1.0 } else { 0.0 }];
            lut[base + 5] = [f(r[1][0]), f(r[1][1]), f(r[1][2]), texels];
            lut[base + 6] = [f(r[2][0]), f(r[2][1]), f(r[2][2]), if l.use_alpha { 1.0 } else { 0.0 }];
        }
    }
}

/// LUT entries before the glow bands (march.wgsl `LUT_BANDS`): after the texture layers of 9
/// materials.
const LUT_BANDS: usize = LUT_LAYERS + 9 * rmb_core::texture::MAX_LAYERS * 7;

/// The glow bands of the main material and the objects' own ones (appended to `lut` at
/// `LUT_BANDS`: per material `MAX_BANDS` bands of 2 entries: position, half width, core half
/// width, emission; color, 1 = in use).
fn bands_gpu(scene: &Scene, lut: &mut Vec<[f32; 4]>) {
    use rmb_core::gradient::{MAX_BANDS, active_bands};
    lut.resize(LUT_BANDS, [0.0; 4]);
    let lists = std::iter::once(&scene.material.bands).chain(scene.objects.iter().map(|o| &o.material.bands));
    for (m, bands) in lists.take(9).enumerate() {
        let base = LUT_BANDS + m * MAX_BANDS * 2;
        lut.resize(base + MAX_BANDS * 2, [0.0; 4]);
        for (i, b) in active_bands(bands).iter().enumerate() {
            let (half, inner) = b.extent();
            lut[base + 2 * i] = [b.pos as f32, half as f32, inner as f32, b.emission];
            lut[base + 2 * i + 1] = [b.color[0], b.color[1], b.color[2], 1.0];
        }
    }
}

const _: () = assert!(LUT_BANDS == 5500, "march.wgsl LUT_BANDS");

/// The object buffer and the LUT blocks of their materials (appended to `lut`).
fn objects_gpu(scene: &Scene, prog: &Program, lut: &mut Vec<[f32; 4]>) -> Vec<ObjGpu> {
    use rmb_core::fractal::ObjectShape;
    let m = &scene.material;
    let mut out = Vec::new();
    for (j, o) in prog.objects.iter().enumerate() {
        let mut g = ObjGpu::zeroed();
        for r in 0..3 {
            g.inv[r] = [o.inv[r][0] as f32, o.inv[r][1] as f32, o.inv[r][2] as f32, 0.0];
        }
        g.inv[0][3] = o.scale as f32;
        g.size = [o.size.x as f32, o.size.y as f32, o.size.z as f32, 0.0];
        g.rep = [o.repeat[0] as f32, o.repeat[1] as f32, o.repeat[2] as f32, 0.0];
        g.rep_lim = [o.repeat_limit[0] as f32, o.repeat_limit[1] as f32, o.repeat_limit[2] as f32, 0.0];
        g.inv[1][3] = o.op as f32;
        g.inv[2][3] = o.smooth as f32;
        g.pos = v4(o.pos, 0.0);
        g.pos[3] = o.material as f32;
        match &o.shape {
            ObjectShape::Fractal(f) => {
                g.frac = [f.w_slice as f32, if f.julia { 1.0 } else { 0.0 }, f.max_iterations as f32, f.repeat_from as f32];
                g.julia = f.julia_c.map(|v| v as f32);
                g.misc = [f.bailout2 as f32, f.de_scale as f32, f.scene_de_scale as f32, 0.0];
                // Both chains of a DE combination share the eight slot entries.
                let p2 = f.part2.as_deref();
                for (i, s) in f.slots.iter().chain(p2.map(|p| p.slots.iter()).into_iter().flatten()).take(8).enumerate() {
                    g.slots[i / 4][i % 4] = s.iterations.max(1);
                }
                let [w1, w2] = f.blend.unwrap_or([0.0; 2]);
                g.comb = [f.combine_mode.index() as f32, p2.map_or(0.0, |p| p.max_iterations as f32), p2.map_or(0.0, |p| p.repeat_from as f32), f.combine_smooth as f32];
                g.comb2 = [p2.map_or(1.0, |p| p.de_scale as f32), f.mix_pow as f32, w1 as f32, w2 as f32];
            }
            ObjectShape::Primitive(_, a) => {
                g.prim = a.map(|v| v as f32);
                g.misc[3] = 1.0;
            }
            ObjectShape::Model(_) => g.misc[3] = 2.0,
        }
        // The material: the object's own one (its LUT block), or the main one.
        let base = LUT_OBJECTS + 512 * j;
        lut.resize(base, [0.0; 4]);
        match scene.objects.get(o.material.wrapping_sub(1)).filter(|_| o.material > 0) {
            Some(so) => {
                let om = &so.material;
                let gr = &om.gradient;
                lut.extend(rmb_core::gradient::lut(gr).iter().zip(rmb_core::gradient::lut_emission(gr)).map(|(c, e)| [c[0], c[1], c[2], e]));
                lut.extend(rmb_core::gradient::lut_specular(gr).iter().zip(rmb_core::gradient::lut_transparency(gr)).map(|(c, t)| [c[0], c[1], c[2], t]));
                g.mat = [om.color_mode.index() as f32, om.color_scale as f32, om.color_offset as f32, if om.repeat { 1.0 } else { 0.0 }];
                g.mat2 = [1.0, om.pattern.index() as f32, om.pattern_size as f32, base as f32];
            }
            None => {
                lut.resize(base + 512, [0.0; 4]);
                g.mat = [m.color_mode.index() as f32, m.color_scale as f32, m.color_offset as f32, if m.repeat { 1.0 } else { 0.0 }];
                g.mat2 = [m.saturation as f32, 0.0, 1.0, 0.0];
            }
        }
        out.push(g);
    }
    if out.is_empty() {
        // The buffer must not be empty.
        out.push(ObjGpu::zeroed());
    }
    out
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PostUniforms {
    a: [f32; 4],
    b: [f32; 4],
    size: [u32; 4],
    bloom: [f32; 4],
    lv: [[u32; 4]; 6],
    lw: [[f32; 4]; 6],
    streak: [f32; 4],
    slv: [u32; 4],
    /// Streak threshold, length, angle, directions (used by the bloom passes).
    sprm: [f32; 4],
}

/// Streak regions after the bloom pyramid: source, two scratch levels, the sum (first texel,
/// width, height each) and the texels of pyramid and regions.
fn streak_regions(w: u32, h: u32) -> ([[u32; 4]; 4], u32) {
    let (levels, total) = bloom_levels(w, h);
    let (lw, lh) = (levels[0][1], levels[0][2]);
    let n = lw * lh;
    (std::array::from_fn(|k| [total + k as u32 * n, lw, lh, 0]), total + 4 * n)
}

/// The bloom pyramid of a `w` × `h` image: per level its first texel, width and height, and
/// the texels of all levels.
fn bloom_levels(w: u32, h: u32) -> (Vec<[u32; 4]>, u32) {
    let mut off = 0;
    let mut out = Vec::new();
    for (lw, lh) in rmb_core::render::bloom::level_sizes(w, h) {
        out.push([off, lw, lh, 0]);
        off += lw * lh;
    }
    (out, off)
}

/// Uniforms of one bloom pass (bloom.wgsl `BP`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BloomPass {
    dst: [u32; 4],
    src: [u32; 4],
    img: [u32; 4],
    prm: [f32; 4],
    prm2: [f32; 4],
}

fn v4(v: rmb_core::V3, w: f64) -> [f32; 4] {
    [v.x as f32, v.y as f32, v.z as f32, w as f32]
}

fn c4(c: [f32; 3], w: f64) -> [f32; 4] {
    [c[0], c[1], c[2], w as f32]
}

/// Whether a material in use (the main one and the objects' own ones) emits light.
fn emissive_any(scene: &Scene, prog: &Program) -> bool {
    let m = &scene.material;
    let own: Vec<_> = prog.objects.iter().filter_map(|o| scene.objects.get(o.material.wrapping_sub(1)).filter(|_| o.material > 0)).collect();
    let glow_layer = |l: &[rmb_core::texture::TextureLayer]| l.iter().any(|l| l.enabled && l.target == rmb_core::texture::Target::Emission);
    let band = |b: &[rmb_core::gradient::GlowBand]| rmb_core::gradient::active_bands(b).iter().any(|b| b.emission > 0.0);
    m.gradient.iter().chain(own.iter().flat_map(|so| so.material.gradient.iter())).any(|s| s.emission > 0.0)
        || band(&m.bands)
        || own.iter().any(|so| band(&so.material.bands))
        || glow_layer(&m.layers)
        || own.iter().any(|so| glow_layer(&so.material.layers))
}

fn uniforms(scene: &Scene, prog: &Program, w: u32, h: u32, fast: bool) -> Uniforms {
    let fr = Frame::new(scene, w, h);
    let cam = &scene.camera;
    let l = &scene.lighting;
    let bg = &scene.background;
    let mut lights = [GLight::zeroed(); rmb_core::scene::MAX_LIGHTS];
    for (g, fl) in lights.iter_mut().zip(&fr.lights) {
        g.dir = v4(fl.point.unwrap_or(fl.dir), fl.specular);
        use rmb_core::render::LightShape;
        // w: the kind (0 directional, 1 point, 2 spot, 3 area, 4 line; above 0.5 positional).
        let kind = match fl.shape {
            LightShape::Directional => 0.0,
            LightShape::Point => 1.0,
            LightShape::Spot { axis, cos_outer, cos_inner } => {
                g.shape0 = v4(axis, cos_outer);
                g.shape1 = [cos_inner as f32, 0.0, 0.0, 0.0];
                2.0
            }
            LightShape::Area { n, u, v } => {
                g.shape0 = v4(n, 0.0);
                g.shape1 = v4(u, 0.0);
                g.shape2 = v4(v, 0.0);
                3.0
            }
            LightShape::Line { end } => {
                g.shape0 = v4(end, 0.0);
                4.0
            }
        };
        g.color = [fl.color[0] as f32, fl.color[1] as f32, fl.color[2] as f32, if fl.point.is_some() { kind } else { 0.0 }];
        g.opts = [if fl.shadows { 1.0 } else { 0.0 }, fl.softness as f32, fl.diffuse.index() as f32, fl.occlusion as f32];
        if let Some(gl) = &fl.glow {
            g.glow = [gl.shape.index() as f32 + 1.0, gl.r2 as f32, if gl.additive { 1.0 } else { 0.0 }, 0.0];
            g.glow_col = [gl.color[0] as f32, gl.color[1] as f32, gl.color[2] as f32, 0.0];
        }
    }
    let mut cuts = [[0.0f32; 4]; 3];
    for (c, (axis, pos, above)) in cuts.iter_mut().zip(&prog.cuts) {
        *c = [*axis as f32, *pos as f32, if *above { 1.0 } else { 0.0 }, 0.0];
    }
    let mut slots = [[0u32; 4]; 8];
    let all = prog.slots.iter().chain(prog.part2.iter().flat_map(|p| p.slots.iter()));
    for (s, ps) in slots.iter_mut().zip(all) {
        s[0] = ps.iterations.max(1);
    }
    let p2 = prog.part2.as_deref();
    let comb = [prog.combine_mode.index() as f32, p2.map(|p| p.max_iterations).unwrap_or(1) as f32, p2.map(|p| p.repeat_from).unwrap_or(0) as f32, prog.combine_smooth as f32];
    let scales = [
        prog.de_scale as f32,
        p2.map(|p| p.de_scale).unwrap_or(1.0) as f32,
        if prog.inside { 1.0 } else { 0.0 },
        rmb_core::render::start_distance(scene) as f32,
    ];
    let rot = prog.rotation_4d.unwrap_or([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);
    let rot4 = rot.map(|r| r.map(|v| v as f32));
    let ao_mode = match l.ao_mode {
        AoMode::Off => 0.0,
        AoMode::Fast => 1.0,
        AoMode::Raytraced => 2.0,
    };
    let m = &scene.material;
    let fog = &scene.fog;
    Uniforms {
        cam_pos: v4(cam.position, fr.tan_half),
        cam_right: v4(cam.right, fr.aspect),
        cam_up: v4(cam.up, fr.pixel_angle),
        cam_fwd: v4(cam.forward, scene.march.max_distance),
        size: [w, h, 0, fr.lights.len() as u32],
        band: [0, 0, scene.march.max_steps.max(1), scene.march.refine_steps],
        march: [scene.march.detail as f32, scene.march.step as f32, prog.bailout2.min(3.0e38) as f32, prog.scene_de_scale as f32],
        frac: [prog.w_slice as f32, if prog.julia { 1.0 } else { 0.0 }, prog.max_iterations as f32, prog.repeat_from as f32],
        julia: prog.julia_c.map(|v| v as f32),
        ao: [ao_mode, l.ao_strength as f32, l.ao_radius as f32, l.ambient as f32],
        sky: c4(l.sky_color, l.shininess),
        ground: c4(l.ground_color, m.specular),
        bg_top: c4(
            bg.top,
            match bg.mode {
                BackgroundMode::Solid => 0.0,
                BackgroundMode::Gradient => 1.0,
                BackgroundMode::Vertical => 2.0,
            },
        ),
        bg_horizon: c4(bg.horizon, 0.0),
        bg_bottom: c4(bg.bottom, 0.0),
        fog: c4(scene.fog.color, scene.fog.density),
        glow: c4(scene.fog.glow_color, scene.fog.glow),
        dof: [if scene.dof.enabled { 1.0 } else { 0.0 }, scene.dof.aperture as f32, scene.dof.focus_distance as f32, if fast { 1.0 } else { 0.0 }],
        color: [m.color_mode.index() as f32, m.color_scale as f32, m.color_offset as f32, if m.repeat { 1.0 } else { 0.0 }],
        color2: [m.saturation as f32, prog.cuts.len() as f32, m.depth_shift as f32, m.depth_reference as f32],
        cuts,
        slots,
        lights,
        comb,
        scales,
        rot4,
        amb_up: v4(fr.ambient_up, bg.curve),
        fog2: [if fog.mode == FogMode::Range { 1.0 } else { 0.0 }, fog.strength as f32, fog.start as f32, fog.curve as f32],
        fog3: [if fog.squared { 1.0 } else { 0.0 }, if fog.from_background { 1.0 } else { 0.0 }, m.detail_shift as f32, m.reflection_bounces as f32],
        glow2: c4(fog.glow_color2, fog.glow_mix_steps),
        glow3: [fog.glow_offset as f32, if fog.glow_blend { 1.0 } else { 0.0 }, fog.glow_iteration as f32, m.reflection as f32],
        bg_rot: {
            let m = rmb_core::maps::rotation_matrix(bg.image_rotation);
            let has = !bg.image.trim().is_empty() && rmb_core::render::background_image(scene).is_some();
            let flags = [if has { 1.0 } else { 0.0 }, if scene.post.display_space { 1.0 } else { 0.0 }, 0.0];
            [0, 1, 2].map(|i| [m[i][0] as f32, m[i][1] as f32, m[i][2] as f32, flags[i]])
        },
        blend: {
            let [a, b] = prog.blend.unwrap_or([0.0; 2]);
            [a as f32, b as f32, scene.march.smooth_normals.min(8) as f32, l.roughness as f32]
        },
        interior: [if m.interior.is_empty() { 0.0 } else { 1.0 }, m.interior_scale as f32, m.interior_offset as f32, prog.mix_pow as f32],
        cmap0: {
            let on = rmb_core::maps::get_any(&m.color_map.image).is_some();
            let cm = &m.color_map;
            [if on { cm.mode.index() as f32 + 1.0 } else { 0.0 }, cm.scale as f32, cm.offset[0] as f32, cm.offset[1] as f32]
        },
        cmap1: {
            let (s, c) = m.color_map.rotation[2].to_radians().sin_cos();
            [c as f32, s as f32, m.color_map.iter_scale as f32, m.color_map.iter_offset as f32]
        },
        cmap2: {
            let avg = rmb_core::maps::get_any(&m.color_map.image).map(|m| m.average()).unwrap_or([0.0; 3]);
            [if m.color_map.luminance_only { 1.0 } else { 0.0 }, avg[0] as f32, avg[1] as f32, avg[2] as f32]
        },
        cmap_rot: {
            let r = rmb_core::maps::rotation_matrix(m.color_map.rotation);
            [0, 1, 2].map(|i| [r[i][0] as f32, r[i][1] as f32, r[i][2] as f32, 0.0])
        },
        glass: [if m.transmission && m.reflection > 0.0 { 1.0 } else { 0.0 }, m.refraction_index as f32, m.absorption as f32, m.scattering as f32],
        emis: {
            let any = emissive_any(scene, prog);
            let on = |v: f64| if any { v.max(0.0) as f32 } else { 0.0 };
            [on(m.emission_light), m.emission_reach as f32, on(m.emission_glow), m.emission_glow_size as f32]
        },
        emis2: {
            let any = emissive_any(scene, prog);
            [m.emission_rays.clamp(1, 8) as f32, if any { m.steam.max(0.0) as f32 } else { 0.0 }, m.steam_size as f32, m.steam_grain as f32]
        },
        c2opt: [m.second_choice.index() as f32, m.lli_multiplier as f32, m.color_on_iteration as f32, 0.0],
        deep_hi: {
            let r = prog.deep.unwrap_or([0.0; 3]);
            [split_dd(r[0]).0, split_dd(r[1]).0, split_dd(r[2]).0, 1.0]
        },
        deep_lo: {
            let r = prog.deep.unwrap_or([0.0; 3]);
            [split_dd(r[0]).1, split_dd(r[1]).1, split_dd(r[2]).1, split_dd(prog.w_slice).1]
        },
        julia_lo: prog.julia_c.map(|v| split_dd(v).1),
        blend_lo: {
            let [a, b] = prog.blend.unwrap_or([0.0; 2]);
            [split_dd(a).1, split_dd(b).1, 0.0, 0.0]
        },
        ccenter: [m.color_center[0] as f32, m.color_center[1] as f32, m.color_center[2] as f32, 0.0],
        cdir: {
            let d = m.color_direction;
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if len > 0.0 { [(d[0] / len) as f32, (d[1] / len) as f32, (d[2] / len) as f32, 0.0] } else { [0.0, 0.0, 1.0, 0.0] }
        },
        path: {
            let on = l.render_mode == rmb_core::scene::RenderMode::PathTraced;
            [if on { 1.0 } else { 0.0 }, l.path_bounces.clamp(1, 16) as f32, l.path_clamp.max(0.0) as f32, 0.0]
        },
        emis3: [m.steam_offset[0] as f32, m.steam_offset[1] as f32, m.steam_offset[2] as f32, m.steam_cover as f32],
        bg2: [
            bg.image_intensity as f32,
            if bg.image_screen { 1.0 } else { 0.0 },
            if l.ambient_from_background { 1.0 } else { 0.0 },
            if bg.image_fog_add { 1.0 } else { 0.0 },
        ],
        mot_pos: scene.motion_end.as_ref().map_or([0.0; 4], |c| v4(c.position, 1.0)),
        mot_fwd: scene.motion_end.as_ref().map_or([0.0; 4], |c| v4(c.forward, c.tan_half_fov())),
        mot_right: scene.motion_end.as_ref().map_or([0.0; 4], |c| v4(c.right, 0.0)),
        mot_up: scene.motion_end.as_ref().map_or([0.0; 4], |c| v4(c.up, 0.0)),
        fog4: [scene.fog.scatter.max(0.0) as f32, scene.fog.scatter_forward as f32, 0.0, 0.0],
        gloss: [scene.material.gloss.clamp(0.0, 1.0) as f32, 0.0, 0.0, 0.0],
        shim: [m.shimmer.max(0.0) as f32, m.shimmer_size as f32, m.shimmer_phase as f32, m.color_map.emission.max(0.0) as f32],
        // Placement of the main fractal: inverse rotation rows divided by the scale per axis
        // (w of the first: the smallest scale), position (deep zoom: in the reference point).
        mt0: prog.main_transform.map_or([0.0; 4], |t| v4(rmb_core::v3(t.inv[0][0], t.inv[0][1], t.inv[0][2]) * (1.0 / t.size.x), t.scale)),
        mt1: prog.main_transform.map_or([0.0; 4], |t| v4(rmb_core::v3(t.inv[1][0], t.inv[1][1], t.inv[1][2]) * (1.0 / t.size.y), 0.0)),
        mt2: prog.main_transform.map_or([0.0; 4], |t| v4(rmb_core::v3(t.inv[2][0], t.inv[2][1], t.inv[2][2]) * (1.0 / t.size.z), 0.0)),
        mt_pos: prog.main_transform.filter(|_| prog.deep.is_none()).map_or([0.0; 4], |t| v4(t.pos, 0.0)),
    }
}

fn post_uniforms(scene: &Scene, w: u32, h: u32) -> PostUniforms {
    let p = &scene.post;
    PostUniforms {
        a: [p.exposure as f32, p.gamma as f32, p.contrast as f32, p.saturation as f32],
        b: [if p.tone_map { 1.0 } else { 0.0 }, p.vignette as f32, if p.display_space { 1.0 } else { 0.0 }, 0.0],
        size: [w, h, 0, 0],
        bloom: {
            let n = bloom_levels(w, h).0.len();
            [if p.bloom > 0.0 { p.bloom as f32 } else { 0.0 }, n as f32, p.bloom_threshold as f32, 0.0]
        },
        lv: {
            let mut lv = [[0u32; 4]; 6];
            for (d, l) in lv.iter_mut().zip(bloom_levels(w, h).0) {
                *d = l;
            }
            lv
        },
        lw: {
            let (levels, _) = bloom_levels(w, h);
            let mut lw = [[0f32; 4]; 6];
            for (d, wt) in lw.iter_mut().zip(rmb_core::render::bloom::level_weights(p.bloom_size, levels.len())) {
                d[0] = wt;
            }
            lw
        },
        streak: {
            let s = p.streak.max(0.0) as f32;
            [p.streak_tint[0] * s, p.streak_tint[1] * s, p.streak_tint[2] * s, if p.streak > 0.0 { 1.0 } else { 0.0 }]
        },
        slv: streak_regions(w, h).0[3],
        sprm: [p.streak_threshold as f32, p.streak_length as f32, p.streak_angle as f32, p.streak_count.clamp(1, 4) as f32],
    }
}

fn storage_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16).next_multiple_of(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

struct Target {
    width: u32,
    height: u32,
    accum: wgpu::Buffer,
    /// Denoiser helper images (albedo, normal; two vec4 per pixel).
    aux: wgpu::Buffer,
    output: wgpu::Texture,
    view: wgpu::TextureView,
}

type Compiled = Result<wgpu::ComputePipeline, String>;

/// What the accumulation was rendered with; any change restarts it.
#[derive(Clone, PartialEq)]
struct Key {
    scene: Scene,
    width: u32,
    height: u32,
    fast: bool,
}

pub struct GpuRenderer {
    ctx: GpuContext,
    display_pipeline: wgpu::ComputePipeline,
    /// Bloom passes (bloom.wgsl): bright, down, blur_h, blur_v, streak.
    bloom_pipelines: [wgpu::ComputePipeline; 5],
    /// Bloom pyramid, its blur scratch and one uniform buffer per pass.
    bloom_levels: wgpu::Buffer,
    bloom_tmp: wgpu::Buffer,
    bloom_passes: Vec<wgpu::Buffer>,
    /// Explicit layout of the march bindings: a shader that does not read a binding (formulas
    /// without parameters never touch `prm`) must still accept the full bind group.
    march_layout: wgpu::BindGroupLayout,
    march_pipeline_layout: wgpu::PipelineLayout,
    /// Current march pipeline and its signature.
    pipeline: Option<(String, wgpu::ComputePipeline)>,
    /// Recently used pipelines (switching formulas back and forth).
    cache: Vec<(String, wgpu::ComputePipeline)>,
    pending: Option<(String, mpsc::Receiver<Compiled>)>,
    uniform_buf: wgpu::Buffer,
    post_buf: wgpu::Buffer,
    prm: wgpu::Buffer,
    lut: wgpu::Buffer,
    /// Mandelbulb3D maps of the scene (march.wgsl `maps`) and their numbers.
    maps: wgpu::Buffer,
    /// Composed objects (march.wgsl binding 6).
    objs: wgpu::Buffer,
    /// Baked 3D models (binding 7) and which volumes they hold.
    vols: wgpu::Buffer,
    vols_key: Vec<usize>,
    mesh_generation: u64,
    /// Lens flare layer added before the post processing (RGB + pad per pixel) and its size.
    flare: wgpu::Buffer,
    flare_size: (u32, u32),
    map_numbers: Vec<i32>,
    /// Background image file in the maps buffer.
    map_background: String,
    maps_generation: u64,
    target: Option<Target>,
    key: Option<Key>,
    uniforms: Option<Uniforms>,
    post: Option<PostUniforms>,
    /// Completed passes (samples per pixel).
    samples: u32,
    /// Next row of the running pass.
    row: u32,
    band_rows: u32,
    /// Time per submission the band height adapts to.
    band_target: Duration,
    dirty_display: bool,
    generation: u64,
    last_error: Option<String>,
    /// The scene has more formula parameters than the parameter buffer holds.
    prm_error: Option<String>,
    /// Which formula parameters go into the shader as constants (see `set_baking`).
    bake: Option<Vec<bool>>,
    /// The scene renders in double precision (deep zoom).
    deep: bool,
    /// Validation errors outside error scopes (instead of the default panic).
    device_error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    compiling: bool,
    signature: String,
}

const CACHE_SIZE: usize = 8;
/// Time per band submission we aim for.
/// Size of the formula parameter buffer (march.wgsl `prm`): high parts, then the low parts
/// from value `PRM_LO` on.
const PRM_BYTES: usize = 32768;
const PRM_LO: usize = 4096;
const BAND_TARGET: Duration = Duration::from_millis(12);
/// Submission time of renders without a window.
const OFFLINE_BAND_TARGET: Duration = Duration::from_millis(50);

impl GpuRenderer {
    pub fn new(ctx: GpuContext) -> Self {
        let device = &ctx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("display"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("display.wgsl"))) });
        let display_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("display"),
            layout: None,
            module: &module,
            entry_point: Some("display_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bloom_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("bloom"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("bloom.wgsl"))) });
        let bloom_pipelines = ["bloom_bright", "bloom_down", "bloom_blur_h", "bloom_blur_v", "bloom_streak"].map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &bloom_module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let uniform = |label: &str, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let storage = |read_only: bool| wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None };
        let march_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("march"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(1, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(2, storage(true)),
                entry(3, storage(false)),
                entry(4, storage(false)),
                entry(5, storage(true)),
                entry(6, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(7, storage(true)),
            ],
        });
        let device_error = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let sink = device_error.clone();
        device.on_uncaptured_error(std::sync::Arc::new(move |e: wgpu::Error| {
            eprintln!("wgpu error: {e}");
            if let Ok(mut g) = sink.lock() {
                *g = Some(format!("GPU error: {}", e.to_string().lines().next().unwrap_or("")));
            }
        }));
        GpuRenderer {
            uniform_buf: uniform("march-uniforms", std::mem::size_of::<Uniforms>()),
            post_buf: uniform("post-uniforms", std::mem::size_of::<PostUniforms>()),
            prm: uniform("prm", PRM_BYTES),
            lut: storage_buffer(device, "lut", ((rmb_core::gradient::LUT_SIZE * 4 + 128) * 16) as u64),
            maps: storage_buffer(device, "maps", 16),
            objs: uniform("objs", std::mem::size_of::<ObjGpu>() * rmb_core::compose::MAX_OBJECTS),
            vols: storage_buffer(device, "vols", 80),
            vols_key: Vec::new(),
            mesh_generation: rmb_core::mesh::generation(),
            flare: storage_buffer(device, "flare", 16),
            flare_size: (0, 0),
            map_numbers: Vec::new(),
            map_background: String::new(),
            maps_generation: rmb_core::maps::generation(),
            display_pipeline,
            bloom_pipelines,
            bloom_levels: storage_buffer(device, "bloom-levels", 16),
            bloom_tmp: storage_buffer(device, "bloom-tmp", 16),
            bloom_passes: Vec::new(),
            march_pipeline_layout: device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("march"), bind_group_layouts: &[Some(&march_layout)], immediate_size: 0 }),
            march_layout,
            pipeline: None,
            cache: Vec::new(),
            pending: None,
            target: None,
            key: None,
            uniforms: None,
            post: None,
            samples: 0,
            row: 0,
            band_rows: 16,
            band_target: BAND_TARGET,
            dirty_display: false,
            generation: 0,
            last_error: None,
            prm_error: None,
            bake: None,
            deep: false,
            device_error,
            compiling: false,
            signature: String::new(),
            ctx,
        }
    }

    /// Sets the time per submission (default 12 ms, which keeps a window responsive). Renders
    /// without a window use longer submissions: fewer round trips to the GPU.
    pub fn set_band_target(&mut self, target: Duration) {
        self.band_target = target;
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    pub fn last_error(&self) -> Option<String> {
        self.prm_error.clone().or_else(|| self.last_error.clone()).or_else(|| self.device_error.lock().ok().and_then(|g| g.clone()))
    }

    /// A shader for the current formulas is being compiled.
    pub fn compiling(&self) -> bool {
        self.compiling
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Progress of the running pass (0..1).
    pub fn pass_progress(&self) -> f32 {
        self.target.as_ref().map(|t| self.row as f32 / t.height.max(1) as f32).unwrap_or(0.0)
    }

    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }

    pub fn output_size(&self) -> Option<(u32, u32)> {
        self.target.as_ref().map(|t| (t.width, t.height))
    }

    /// Changes whenever the output texture was replaced or redrawn.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Sets what to render. Restarts the accumulation when anything but the post processing
    /// changed. `fast` renders a quicker preview (no shadows, fast AO) for navigation.
    pub fn set_scene(&mut self, scene: &Scene, width: u32, height: u32, fast: bool) {
        let (width, height) = (width.max(1), height.max(1));
        let mut render_scene = scene.clone();
        // Only the color space of the post settings changes the shading.
        render_scene.post = Post { display_space: scene.post.display_space, ..Default::default() };
        render_scene.notes.clear();
        render_scene.animation = None;
        let key = Key { scene: render_scene, width, height, fast };
        // Other map folders: reload the maps and render again.
        if self.maps_generation != rmb_core::maps::generation() {
            self.maps_generation = rmb_core::maps::generation();
            self.map_numbers.clear();
            self.map_background = "\u{0}".into();
            self.key = None;
        }
        let mut post = post_uniforms(scene, width, height);
        post.size[2] = self.flare_size.0;
        post.size[3] = self.flare_size.1;
        if self.post.is_none_or(|p| bytemuck::bytes_of(&p) != bytemuck::bytes_of(&post)) {
            self.ctx.queue.write_buffer(&self.post_buf, 0, bytemuck::bytes_of(&post));
            self.post = Some(post);
            self.dirty_display = true;
        }
        // A model finished baking: build the program again (it picks the model up).
        if self.mesh_generation != rmb_core::mesh::generation() {
            self.mesh_generation = rmb_core::mesh::generation();
            self.key = None;
        }
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let mut prog = Program::for_scene(scene);
        // Deep zoom: everything relative to the camera, the fractal in double-float.
        let shifted;
        let scene = if self.deep_mode(scene, &prog, height) {
            let r = scene.camera.position;
            shifted = rmb_core::render::shifted_scene(scene, r);
            prog = Program::for_scene(&shifted);
            // A placed main fractal iterates from the camera in its own coordinates; the
            // shader then only turns and scales the local position (uniforms: no offset).
            prog.deep = Some(prog.main_transform.map_or(r, |t| t.local(rmb_core::V3::ZERO)).to_array());
            &shifted
        } else {
            scene
        };
        // Path tracing needs the bounce loop too.
        prog.reflections = scene.material.reflection > 0.0 || scene.lighting.render_mode == rmb_core::scene::RenderMode::PathTraced;
        self.ensure_target(width, height);
        // Parameters
        // High parts, then from PRM_LO on the low parts (double-float deep zoom).
        let consts: Vec<f32> = prog.constants.iter().map(|v| split_dd(*v).0).collect();
        let lows: Vec<f32> = prog.constants.iter().map(|v| split_dd(*v).1).collect();
        self.prm_error = (consts.len() > PRM_LO).then(|| format!("too many formula parameters ({} of at most {PRM_LO})", consts.len()));
        if self.prm_error.is_none() && !consts.is_empty() {
            self.ctx.queue.write_buffer(&self.prm, 0, bytemuck::cast_slice(&consts));
            self.ctx.queue.write_buffer(&self.prm, (PRM_LO * 4) as u64, bytemuck::cast_slice(&lows));
        }
        self.upload_maps(&prog, scene);
        self.upload_volumes(&prog);
        let shade = rmb_core::render::shading_scene(scene);
        let g = &shade.material.gradient;
        // Highlight entries carry the transparency in w.
        // Surface entries carry the emission strength in w.
        let mut lut: Vec<[f32; 4]> = rmb_core::gradient::lut(g).iter().zip(rmb_core::gradient::lut_emission(g)).map(|(c, e)| [c[0], c[1], c[2], e]).collect();
        lut.extend(rmb_core::gradient::lut_specular(g).iter().zip(rmb_core::gradient::lut_transparency(g)).map(|(c, t)| [c[0], c[1], c[2], t]));
        let rough = rmb_core::render::rough_tables();
        lut.extend((0..128).map(|i| [0, 1, 2, 3].map(|k| rough[k][i] as f32)));
        // Interior colors (when there are none the shader does not read them).
        let interior = &shade.material.interior;
        let (ib, is) = if interior.is_empty() { (vec![[0.0; 3]; 256], vec![[0.0; 3]; 256]) } else { (rmb_core::gradient::lut(interior), rmb_core::gradient::lut_specular(interior)) };
        lut.extend(ib.iter().chain(&is).map(|c| [c[0], c[1], c[2], 1.0]));
        let objs = objects_gpu(&shade, &prog, &mut lut);
        layers_gpu(&shade, &mut lut);
        bands_gpu(&shade, &mut lut);
        let device = self.ctx.device.clone();
        let device = &device;
        let n = objs.len().min(rmb_core::compose::MAX_OBJECTS);
        self.ctx.queue.write_buffer(&self.objs, 0, bytemuck::cast_slice(&objs[..n]));
        let lut_bytes = (lut.len() * 16) as u64;
        if self.lut.size() < lut_bytes {
            self.lut = storage_buffer(device, "lut", lut_bytes.next_power_of_two());
        }
        self.ctx.queue.write_buffer(&self.lut, 0, bytemuck::cast_slice(&lut));
        self.uniforms = Some(uniforms(&shade, &prog, width, height, fast));
        self.request_pipeline(&prog);
        self.key = Some(key);
        self.samples = 0;
        self.row = 0;
    }

    /// Like [`set_scene`](Self::set_scene) for the next passes, but keeps the samples so far
    /// (motion blur: the scene moves within one frame). The formulas must stay the same.
    pub fn set_scene_keep(&mut self, scene: &Scene, width: u32, height: u32) {
        let (samples, row) = (self.samples, self.row);
        self.set_scene(scene, width, height, false);
        // Same shader (nothing compiling): keep accumulating.
        if self.pending.is_none() {
            self.samples = samples;
            self.row = row;
        }
    }

    /// Uploads the baked models (only when they change).
    fn upload_volumes(&mut self, prog: &Program) {
        use rmb_core::fractal::ObjectShape;
        let key: Vec<usize> = prog
            .objects
            .iter()
            .flat_map(|o| match &o.shape {
                ObjectShape::Model(Some(m)) => [std::sync::Arc::as_ptr(&m.vol) as usize, std::sync::Arc::as_ptr(&m.look) as usize],
                _ => [0, 0],
            })
            .collect();
        if key == self.vols_key && self.vols.size() >= (4 * MODEL_HEADER * prog.objects.len().max(1)) as u64 {
            return;
        }
        let data = volumes_gpu(prog);
        let bytes = (data.len() * 4) as u64;
        if self.vols.size() < bytes {
            self.vols = storage_buffer(&self.ctx.device, "vols", bytes.next_power_of_two());
        }
        self.ctx.queue.write_buffer(&self.vols, 0, bytemuck::cast_slice(&data));
        self.vols_key = key;
    }

    /// Uploads the maps the formulas read (only when the set of map numbers changes).
    fn upload_maps(&mut self, prog: &Program, scene: &Scene) {
        let numbers = prog.map_numbers();
        let b = &scene.background;
        let layers = layer_images(scene);
        let background = format!("{}|{}|{}|{}|{}", b.image.trim(), b.image_spherical, b.image_fit_border, scene.material.color_map.image.trim(), layers.join("|"));
        // New buffers are zeroed: no maps (count 0) until the first upload.
        let none = b.image.trim().is_empty() && scene.material.color_map.image.trim().is_empty() && layers.is_empty();
        if numbers == self.map_numbers && background == self.map_background && !(numbers.is_empty() && none && self.maps.size() > 16) {
            return;
        }
        // The background image goes in under the number 0xffffffff, its ambient copy under
        // 0xfffffffd (march.wgsl).
        let mut maps: Vec<(u32, std::sync::Arc<rmb_core::maps::Map>)> = numbers.iter().filter_map(|n| rmb_core::maps::get(*n).map(|m| (*n as u32, m))).collect();
        if let Some((m, small, _)) = rmb_core::render::background_maps(scene) {
            maps.push((u32::MAX, m));
            maps.push((u32::MAX - 2, small));
        }
        if let Some(m) = rmb_core::maps::get_any(&scene.material.color_map.image) {
            maps.push((u32::MAX - 1, m));
        }
        for (i, name) in layers.iter().enumerate() {
            if let Some(m) = rmb_core::maps::get_any(name) {
                maps.push((u32::MAX - 16 - i as u32, m));
            }
        }
        let mut data: Vec<u32> = vec![maps.len() as u32];
        let mut offset = 1 + 4 * maps.len() as u32;
        for (n, m) in &maps {
            data.extend_from_slice(&[*n, m.width, m.height, offset]);
            offset += m.width * m.height;
        }
        // Blue | green << 8 | red << 16 | alpha << 24 (255 without an alpha channel).
        for (_, m) in &maps {
            match &m.alpha {
                Some(a) => data.extend(m.pixels.iter().zip(a).map(|(p, a)| p[0] as u32 | (p[1] as u32) << 8 | (p[2] as u32) << 16 | (*a as u32) << 24)),
                None => data.extend(m.pixels.iter().map(|p| p[0] as u32 | (p[1] as u32) << 8 | (p[2] as u32) << 16 | 255 << 24)),
            }
        }
        let bytes = (data.len() * 4) as u64;
        if self.maps.size() < bytes {
            self.maps = storage_buffer(&self.ctx.device, "maps", bytes.next_power_of_two());
        }
        self.ctx.queue.write_buffer(&self.maps, 0, bytemuck::cast_slice(&data));
        self.map_numbers = numbers;
        self.map_background = background;
    }

    fn ensure_target(&mut self, width: u32, height: u32) {
        if self.target.as_ref().is_some_and(|t| t.width == width && t.height == height) {
            return;
        }
        let device = &self.ctx.device;
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let accum = storage_buffer(device, "accum", width as u64 * height as u64 * 16);
        let aux = storage_buffer(device, "aux", width as u64 * height as u64 * 32);
        self.target = Some(Target { width, height, accum, aux, output, view });
        self.generation += 1;
        self.samples = 0;
        self.row = 0;
    }

    /// Whether to render `scene` in double precision (`Precision::Auto`: by the zoom, switching
    /// back to single precision only at twice the threshold, so flying around it does not
    /// recompile the shader again and again).
    fn deep_mode(&mut self, scene: &Scene, prog: &Program, height: u32) -> bool {
        use rmb_core::scene::Precision;
        self.deep = match scene.march.precision {
            Precision::Single => false,
            Precision::Double => true,
            Precision::Auto => {
                let limit = rmb_core::render::DEEP_ZOOM_RATIO * if self.deep { 2.0 } else { 1.0 };
                rmb_core::render::zoom_ratio(scene, prog, height) < limit
            }
        };
        self.deep
    }

    /// Whether the current scene renders in double precision (deep zoom).
    pub fn double_precision(&self) -> bool {
        self.deep
    }

    /// Formula parameters to write into the shader as constants (see
    /// [`crate::codegen::march_shader_baked`]): `None` none (the default; changing parameters never
    /// recompiles), else per constant of the program whether to bake it (`true` beyond the
    /// slice). Renders without a window compile their own shader anyway and bake all parameters;
    /// animations bake the ones that stay the same in all frames.
    pub fn set_baking(&mut self, mask: Option<Vec<bool>>) {
        self.bake = mask;
    }

    /// The parameter values to bake for `prog` (see [`GpuRenderer::set_baking`]).
    fn baked(&self, prog: &Program) -> Vec<Option<f64>> {
        let Some(mask) = &self.bake else { return Vec::new() };
        prog.constants.iter().enumerate().map(|(i, v)| mask.get(i).copied().unwrap_or(true).then_some(*v)).collect()
    }

    fn request_pipeline(&mut self, prog: &Program) {
        let baked = self.baked(prog);
        let mut sig = prog.signature();
        if baked.iter().any(Option::is_some) {
            // Baked values are part of the shader.
            sig.push_str("|baked:");
            for v in &baked {
                let _ = write!(sig, "{},", v.map_or(String::new(), |x| format!("{:016x}", x.to_bits())));
            }
        }
        if self.signature == sig {
            return;
        }
        self.signature = sig.clone();
        if let Some(i) = self.cache.iter().position(|(s, _)| *s == sig) {
            let p = self.cache.remove(i);
            if let Some(old) = self.pipeline.replace(p) {
                self.cache.push(old);
            }
            self.pending = None;
            self.compiling = false;
            self.last_error = None;
            return;
        }
        let src = crate::codegen::march_shader_baked(prog, &baked);
        let device = self.ctx.device.clone();
        let layout = self.march_pipeline_layout.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(compile(&device, &layout, &src));
        });
        self.pending = Some((sig, rx));
        self.compiling = true;
    }

    fn poll_pipeline(&mut self) {
        let Some((sig, rx)) = &self.pending else { return };
        match rx.try_recv() {
            Ok(Ok(p)) => {
                let sig = sig.clone();
                if let Some(old) = self.pipeline.replace((sig, p)) {
                    self.cache.push(old);
                    if self.cache.len() > CACHE_SIZE {
                        self.cache.remove(0);
                    }
                }
                self.last_error = None;
                self.pending = None;
                self.compiling = false;
                self.samples = 0;
                self.row = 0;
            }
            Ok(Err(e)) => {
                self.last_error = Some(e);
                self.pending = None;
                self.compiling = false;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.last_error = Some("shader compilation thread failed".into());
                self.pending = None;
                self.compiling = false;
            }
        }
    }

    /// The pipeline for the current scene (None while it compiles or after an error).
    fn current_pipeline(&self) -> Option<&wgpu::ComputePipeline> {
        self.pipeline.as_ref().filter(|(s, _)| *s == self.signature && self.pending.is_none()).map(|(_, p)| p)
    }

    /// Renders bands until `budget` is used up or `max_samples` passes are complete.
    /// Returns true if anything was rendered.
    pub fn work(&mut self, budget: Duration, max_samples: u32) -> bool {
        self.poll_pipeline();
        if self.samples >= max_samples {
            return false;
        }
        let (Some(t), Some(mut un)) = (self.target.as_ref(), self.uniforms) else { return false };
        let (accum, aux, width, height) = (t.accum.clone(), t.aux.clone(), t.width, t.height);
        let Some(pipeline) = self.current_pipeline().cloned() else { return false };
        let device = self.ctx.device.clone();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("march"),
            layout: &self.march_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.prm.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.lut.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: accum.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: aux.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: self.maps.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: self.objs.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: self.vols.as_entire_binding() },
            ],
        });
        let start = Instant::now();
        let mut worked = false;
        while self.samples < max_samples {
            let rows = self.band_rows.min(height - self.row);
            un.size[2] = self.samples;
            un.band[0] = self.row;
            un.band[1] = rows;
            self.ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&un));
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(width.div_ceil(8), rows.div_ceil(8), 1);
            }
            let t0 = Instant::now();
            self.ctx.queue.submit(Some(enc.finish()));
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            let dt = t0.elapsed();
            worked = true;
            // Adapt the band height toward the target time per submission.
            let ratio = self.band_target.as_secs_f64() / dt.as_secs_f64().max(1e-5);
            let wanted = (rows as f64 * ratio.clamp(0.5, 2.0)).round() as u32;
            self.band_rows = wanted.clamp(8, height.max(8)).next_multiple_of(8);
            self.row += rows;
            if self.row >= height {
                self.row = 0;
                self.samples += 1;
            }
            if start.elapsed() >= budget {
                break;
            }
        }
        if worked {
            self.dirty_display = true;
        }
        worked
    }

    /// Sets the lens flare layer added to the image before the post processing (`None`: no
    /// flares); it is stretched over the image if its size differs.
    pub fn set_flare(&mut self, layer: Option<(u32, u32, &[[f32; 3]])>) {
        let size = match layer {
            Some((w, h, px)) if px.len() == (w * h) as usize && w > 0 && h > 0 => {
                let data: Vec<[f32; 4]> = px.iter().map(|c| [c[0], c[1], c[2], 0.0]).collect();
                let bytes = (data.len() * 16) as u64;
                if self.flare.size() < bytes {
                    self.flare = storage_buffer(&self.ctx.device, "flare", bytes.next_power_of_two());
                }
                self.ctx.queue.write_buffer(&self.flare, 0, bytemuck::cast_slice(&data));
                (w, h)
            }
            _ => (0, 0),
        };
        self.flare_size = size;
        if let Some(mut p) = self.post {
            p.size[2] = size.0;
            p.size[3] = size.1;
            self.ctx.queue.write_buffer(&self.post_buf, 0, bytemuck::bytes_of(&p));
            self.post = Some(p);
        }
        self.dirty_display = true;
    }

    /// Updates the output texture from the accumulation if needed.
    pub fn display(&mut self) {
        if !self.dirty_display {
            return;
        }
        let Some(t) = self.target.as_ref() else { return };
        if self.samples == 0 && self.row == 0 {
            // Nothing rendered for the current scene yet: keep showing the old image.
            return;
        }
        self.dirty_display = false;
        let (width, height) = (t.width, t.height);
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        self.bloom_passes(&mut enc, width, height);
        let Some(t) = self.target.as_ref() else { return };
        let device = &self.ctx.device;
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("display"),
            layout: &self.display_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.post_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: t.accum.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&t.view) },
                wgpu::BindGroupEntry { binding: 3, resource: self.flare.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: self.bloom_levels.as_entire_binding() },
            ],
        });
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.display_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(t.width.div_ceil(8), t.height.div_ceil(8), 1);
        }
        self.ctx.queue.submit(Some(enc.finish()));
        self.generation += 1;
    }

    /// Records the bloom pyramid passes (bloom.wgsl) when the post processing has bloom.
    fn bloom_passes(&mut self, enc: &mut wgpu::CommandEncoder, width: u32, height: u32) {
        let Some(post) = self.post else { return };
        let (bloom_on, streak_on) = (post.bloom[0] > 0.0, post.streak[3] > 0.5);
        if !bloom_on && !streak_on {
            return;
        }
        let Some(t) = self.target.as_ref() else { return };
        let device = &self.ctx.device;
        let (levels, _) = bloom_levels(width, height);
        let (regions, texels) = streak_regions(width, height);
        let bytes = texels as u64 * 16;
        if self.bloom_levels.size() < bytes {
            self.bloom_levels = storage_buffer(device, "bloom-levels", bytes.next_power_of_two());
            self.bloom_tmp = storage_buffer(device, "bloom-tmp", bytes.next_power_of_two());
        }
        // Passes: bright into level 0, down into each further level, then blur h + v per level.
        let mut passes: Vec<(usize, BloomPass)> = Vec::new();
        let img = [width, height, post.size[2], post.size[3]];
        let prm = [post.a[0].exp2(), post.bloom[2], 0.0, 0.0];
        let prm2 = [0.0; 4];
        if bloom_on {
            for (k, l) in levels.iter().enumerate() {
                let src = if k == 0 { [0; 4] } else { levels[k - 1] };
                passes.push((if k == 0 { 0 } else { 1 }, BloomPass { dst: *l, src, img, prm, prm2 }));
            }
            for l in &levels {
                passes.push((2, BloomPass { dst: *l, src: *l, img, prm, prm2 }));
                passes.push((3, BloomPass { dst: *l, src: *l, img, prm, prm2 }));
            }
        }
        if streak_on {
            // Bright part into the source region, then per direction three passes through the
            // scratch regions into the sum (the first direction writes, the others add).
            let [src, t1, t2, sum] = regions;
            passes.push((0, BloomPass { dst: src, src: [0; 4], img, prm: [prm[0], post.sprm[0], 0.0, 0.0], prm2 }));
            let (step0, a) = rmb_core::render::bloom::streak_params(post.sprm[1] as f64, src[1]);
            for (i, dir) in rmb_core::render::bloom::streak_dirs(post.sprm[2] as f64, post.sprm[3] as u32).into_iter().enumerate() {
                let chain = [(src, t1), (t1, t2), (t2, sum)];
                for (p, (from, to)) in chain.into_iter().enumerate() {
                    let add = p == 2 && i > 0;
                    let step = step0 * 4f32.powi(p as i32);
                    passes.push((4, BloomPass { dst: to, src: from, img, prm: [prm[0], 0.0, if add { 1.0 } else { 0.0 }, 0.0], prm2: [dir[0], dir[1], step, a] }));
                }
            }
        }
        while self.bloom_passes.len() < passes.len() {
            self.bloom_passes.push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bloom-pass"),
                size: std::mem::size_of::<BloomPass>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        for (i, (kind, bp)) in passes.iter().enumerate() {
            self.ctx.queue.write_buffer(&self.bloom_passes[i], 0, bytemuck::bytes_of(bp));
            let pipe = &self.bloom_pipelines[*kind];
            let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: self.bloom_passes[i].as_entire_binding() }];
            if *kind == 0 {
                entries.push(wgpu::BindGroupEntry { binding: 1, resource: t.accum.as_entire_binding() });
                entries.push(wgpu::BindGroupEntry { binding: 2, resource: self.flare.as_entire_binding() });
            }
            entries.push(wgpu::BindGroupEntry { binding: 3, resource: self.bloom_levels.as_entire_binding() });
            if *kind == 2 || *kind == 3 {
                entries.push(wgpu::BindGroupEntry { binding: 4, resource: self.bloom_tmp.as_entire_binding() });
            }
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("bloom"), layout: &pipe.get_bind_group_layout(0), entries: &entries });
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(bp.dst[1].div_ceil(8), bp.dst[2].div_ceil(8), 1);
        }
    }

    /// Blocks until the current shader is compiled (headless rendering). Returns the error, if any.
    pub fn wait_for_pipeline(&mut self) -> Result<(), String> {
        while self.pending.is_some() {
            std::thread::sleep(Duration::from_millis(5));
            self.poll_pipeline();
        }
        match &self.last_error {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    /// Reads the averaged image and the denoiser's helper images back (blocking).
    pub fn read_hdr(&mut self) -> Option<HdrImage> {
        let t = self.target.as_ref()?;
        let n = (t.width * t.height) as usize;
        let accum = read_buffer_f32(&self.ctx, &t.accum, n * 4)?;
        let aux = read_buffer_f32(&self.ctx, &t.aux, n * 8)?;
        let mut img = HdrImage { width: t.width, height: t.height, color: Vec::with_capacity(n * 3), albedo: Vec::with_capacity(n * 3), normal: Vec::with_capacity(n * 3) };
        for i in 0..n {
            let a = &accum[i * 4..i * 4 + 4];
            let k = 1.0 / a[3].max(1.0);
            img.color.extend(a[..3].iter().map(|v| v * k));
            img.albedo.extend(aux[i * 8..i * 8 + 3].iter().map(|v| v * k));
            img.normal.extend(aux[i * 8 + 4..i * 8 + 7].iter().map(|v| v * k));
        }
        Some(img)
    }

    /// Reads the displayed image back as tightly packed RGBA8 (blocking).
    pub fn read_rgba8(&mut self) -> Option<Vec<u8>> {
        self.dirty_display = true;
        self.display();
        let t = self.target.as_ref()?;
        read_texture_rgba8(&self.ctx, &t.output, t.width, t.height)
    }
}

fn compile(device: &wgpu::Device, layout: &wgpu::PipelineLayout, src: &str) -> Compiled {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("march"), source: wgpu::ShaderSource::Wgsl(Cow::Owned(src.to_string())) });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("march"),
        layout: Some(layout),
        module: &module,
        entry_point: Some("march_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    match pollster::block_on(scope.pop()) {
        Some(err) => Err(format!("shader compilation failed: {err}")),
        None => Ok(pipeline),
    }
}

/// Reads the first `count` floats of a storage buffer (blocking).
fn read_buffer_f32(ctx: &GpuContext, buffer: &wgpu::Buffer, count: usize) -> Option<Vec<f32>> {
    let size = (count * 4) as u64;
    let device = &ctx.device;
    let staging = device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    ctx.queue.submit(Some(enc.finish()));
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let out = bytemuck::cast_slice::<u8, f32>(&slice.get_mapped_range().ok()?).to_vec();
    staging.unmap();
    Some(out)
}

/// Reads an RGBA8 texture back as tightly packed RGBA8 (blocking).
pub fn read_texture_rgba8(ctx: &GpuContext, texture: &wgpu::Texture, w: u32, h: u32) -> Option<Vec<u8>> {
    let row = (w * 4).next_multiple_of(256);
    let device = &ctx.device;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: row as u64 * h as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &staging, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit(Some(enc.finish()));
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range().ok()?;
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let start = y * row as usize;
        out.extend_from_slice(&data[start..start + (w * 4) as usize]);
    }
    drop(data);
    staging.unmap();
    Some(out)
}

/// Renders `scene` at `width` × `height` with `samples` passes (blocking, headless use).
/// `progress` gets the completed fraction and returns false to cancel.
pub fn render_blocking(ctx: GpuContext, scene: &Scene, width: u32, height: u32, samples: u32, progress: impl FnMut(f32) -> bool) -> Result<Vec<u8>, String> {
    let mut r = render_to(ctx, scene, width, height, samples, progress)?;
    r.read_rgba8().ok_or_else(|| "reading the image back failed".to_string())
}

/// [`render_blocking`] that returns the averaged image with the denoiser's helper images
/// (finish it with [`HdrImage::finish`]).
pub fn render_blocking_hdr(ctx: GpuContext, scene: &Scene, width: u32, height: u32, samples: u32, progress: impl FnMut(f32) -> bool) -> Result<HdrImage, String> {
    let mut r = render_to(ctx, scene, width, height, samples, progress)?;
    r.read_hdr().ok_or_else(|| "reading the image back failed".to_string())
}

/// Renders the frames `frames` of an animation at `width` × `height` with `samples` passes each
/// and hands every finished frame (RGBA, post-processed and denoised as its scene says) to
/// `on_frame`. `frame_scene` gives the scene at a time in frames (fractional within a frame's
/// exposure). `shutter` is the exposure as a fraction of the frame time (motion blur; 0: none):
/// each pass then renders a slice of the exposure, and every sample a random time within it.
/// Uses the GPU when `ctx` is given (one renderer for all frames: shaders compile only when the
/// formulas change), else the CPU.
/// `progress` gets the finished fraction; `cancel` stops after the current pass.
#[allow(clippy::too_many_arguments)]
pub fn render_animation(
    ctx: Option<GpuContext>,
    frames: std::ops::Range<usize>,
    width: u32,
    height: u32,
    samples: u32,
    shutter: f64,
    frame_scene: impl Fn(f64) -> Scene,
    cancel: &std::sync::atomic::AtomicBool,
    mut progress: impl FnMut(f32),
    mut on_frame: impl FnMut(usize, Vec<u8>) -> Result<(), String> + Send,
) -> Result<(), String> {
    // `on_frame` (encoding, writing) runs on a second thread while the next frame renders.
    std::thread::scope(|s| {
        let (tx, rx) = mpsc::sync_channel::<(usize, Vec<u8>)>(1);
        let writer = s.spawn(move || -> Result<(), String> {
            for (f, rgba) in rx {
                on_frame(f, rgba)?;
            }
            Ok(())
        });
        let rendered = render_frames(ctx, frames, width, height, samples, shutter, frame_scene, cancel, &mut progress, &tx);
        drop(tx);
        let written = writer.join().unwrap_or_else(|_| Err("writing a frame failed".into()));
        // A writer error ends the rendering too (the channel closes): report it first.
        written.and(rendered)
    })
}

/// Per formula constant whether it is the same at all `times` (all false where the formulas
/// themselves change): those can be baked into one shader for the whole animation.
fn invariant_constants(times: impl Iterator<Item = f64>, frame_scene: &impl Fn(f64) -> Scene) -> Vec<bool> {
    let mut first: Option<(String, Vec<f64>)> = None;
    let mut mask = Vec::new();
    for t in times {
        let p = Program::for_scene(&frame_scene(t));
        match &first {
            None => {
                mask = vec![true; p.constants.len()];
                first = Some((p.signature(), p.constants));
            }
            Some((sig, c)) => {
                if p.signature() != *sig || p.constants.len() != c.len() {
                    return vec![false; c.len().max(p.constants.len())];
                }
                for (m, (a, b)) in mask.iter_mut().zip(c.iter().zip(&p.constants)) {
                    *m &= a == b;
                }
            }
        }
    }
    mask
}

/// The time (in frames) of pass `k` of `samples` of frame `f`: the start of its slice of the
/// exposure, and the slice's length.
fn pass_time(f: usize, k: u32, samples: u32, shutter: f64) -> (f64, f64) {
    let slice = shutter / samples as f64;
    (f as f64 - 0.5 * shutter + k as f64 * slice, slice)
}

/// The scene of pass `k` of frame `f`: settings at the middle of the pass's slice of the
/// exposure, the camera moving over the slice.
fn pass_scene(frame_scene: &impl Fn(f64) -> Scene, f: usize, k: u32, samples: u32, shutter: f64) -> Scene {
    if shutter <= 0.0 {
        return frame_scene(f as f64);
    }
    let (t0, slice) = pass_time(f, k, samples, shutter);
    let mut s = frame_scene(t0 + 0.5 * slice);
    s.camera = frame_scene(t0).camera;
    s.motion_end = Some(frame_scene(t0 + slice).camera);
    s
}

/// The rendering part of [`render_animation`]: hands each frame to `tx`.
#[allow(clippy::too_many_arguments)]
fn render_frames(
    ctx: Option<GpuContext>,
    frames: std::ops::Range<usize>,
    width: u32,
    height: u32,
    samples: u32,
    shutter: f64,
    frame_scene: impl Fn(f64) -> Scene,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut impl FnMut(f32),
    tx: &mpsc::SyncSender<(usize, Vec<u8>)>,
) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let samples = samples.max(1);
    let shutter = shutter.clamp(0.0, 1.0);
    let total = frames.len().max(1) as f32;
    let mut gpu = ctx.map(GpuRenderer::new);
    if let Some(r) = gpu.as_mut() {
        r.set_band_target(OFFLINE_BAND_TARGET);
        // With motion blur the parameters also change within a frame's exposure.
        let times = frames.clone().flat_map(|f| {
            let f = f as f64;
            if shutter > 0.0 { vec![f - 0.5 * shutter, f, f + 0.5 * shutter] } else { vec![f] }
        });
        r.set_baking(Some(invariant_constants(times, &frame_scene)));
    }
    for (done, f) in frames.enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        // The frame's own scene: models, post processing and flares.
        let scene = frame_scene(f as f64);
        rmb_core::compose::load_models(&scene);
        let image = match gpu.as_mut() {
            Some(r) => {
                r.set_scene(&pass_scene(&frame_scene, f, 0, samples, shutter), width, height, false);
                r.wait_for_pipeline()?;
                while r.samples() < samples {
                    if shutter > 0.0 && r.samples() > 0 {
                        r.set_scene_keep(&pass_scene(&frame_scene, f, r.samples(), samples, shutter), width, height);
                        r.wait_for_pipeline()?;
                    }
                    // With motion blur one pass at a time (each pass is a slice of the exposure).
                    let target = if shutter > 0.0 { r.samples() + 1 } else { samples };
                    while r.samples() < target {
                        r.work(Duration::from_millis(100), target);
                        if let Some(e) = r.last_error() {
                            return Err(e);
                        }
                        if cancel.load(Ordering::Relaxed) {
                            return Err("cancelled".into());
                        }
                        progress((done as f32 + (r.samples() as f32 + r.pass_progress()) / samples as f32) / total);
                    }
                }
                r.read_hdr().ok_or("reading the image back failed")?
            }
            None => {
                let mut r = rmb_core::render::cpu::CpuRenderer::new(pass_scene(&frame_scene, f, 0, samples, shutter), width, height);
                for i in 0..samples {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    if shutter > 0.0 && i > 0 {
                        r.set_scene_keep(pass_scene(&frame_scene, f, i, samples, shutter));
                    }
                    r.render_pass();
                    progress((done as f32 + (i + 1) as f32 / samples as f32) / total);
                }
                r.hdr_image()
            }
        };
        let (rgba, _) = image.finish_scene(&scene);
        if tx.send((f, rgba)).is_err() {
            // The writer stopped with an error.
            return Ok(());
        }
        progress((done + 1) as f32 / total);
    }
    Ok(())
}


/// Renders like [`render_blocking_hdr`] and hands intermediate images to `snapshot` (the image
/// and the samples in it) after 1, 2, 4, 8 … samples, at most about once a second after the
/// first. `progress` gets the finished fraction and returns false to cancel.
pub fn render_progressive(ctx: GpuContext, scene: &Scene, width: u32, height: u32, samples: u32, mut progress: impl FnMut(f32) -> bool, mut snapshot: impl FnMut(HdrImage, u32)) -> Result<HdrImage, String> {
    rmb_core::compose::load_models(scene);
    let mut r = GpuRenderer::new(ctx);
    // Its own shader anyway: with the parameters as constants.
    r.set_baking(Some(Vec::new()));
    r.set_scene(scene, width, height, false);
    r.wait_for_pipeline()?;
    let samples = samples.max(1);
    let mut next = 1;
    let mut last = std::time::Instant::now();
    while r.samples() < samples {
        r.work(Duration::from_millis(100), samples);
        if let Some(e) = r.last_error() {
            return Err(e);
        }
        let done = r.samples();
        if done >= next && done < samples && (done == 1 || last.elapsed() >= Duration::from_millis(900)) {
            if let Some(img) = r.read_hdr() {
                snapshot(img, done);
            }
            last = std::time::Instant::now();
            while next <= done {
                next *= 2;
            }
        }
        if !progress((done as f32 + r.pass_progress()) / samples as f32) {
            return Err("cancelled".into());
        }
    }
    r.read_hdr().ok_or_else(|| "reading the image back failed".to_string())
}

fn render_to(ctx: GpuContext, scene: &Scene, width: u32, height: u32, samples: u32, mut progress: impl FnMut(f32) -> bool) -> Result<GpuRenderer, String> {
    // Final renders wait for the 3D models to bake.
    rmb_core::compose::load_models(scene);
    let mut r = GpuRenderer::new(ctx);
    r.set_band_target(OFFLINE_BAND_TARGET);
    r.set_baking(Some(Vec::new()));
    r.set_scene(scene, width, height, false);
    r.wait_for_pipeline()?;
    let samples = samples.max(1);
    while r.samples() < samples {
        r.work(Duration::from_millis(100), samples);
        if let Some(e) = r.last_error() {
            return Err(e);
        }
        let p = (r.samples() as f32 + r.pass_progress()) / samples as f32;
        if !progress(p) {
            return Err("cancelled".into());
        }
    }
    Ok(r)
}
