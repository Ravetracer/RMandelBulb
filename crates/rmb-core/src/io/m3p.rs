// SPDX-License-Identifier: LGPL-2.1-or-later

//! Import of Mandelbulb3D parameters: binary `.m3p` files and the clipboard text format
//! (`Mandelbulb3Dv18{…}`).
//!
//! Supported: header versions (MandId) 20 to 45. Formulas are mapped by name to the library
//! (aliases); unknown formulas stay in the hybrid as missing slots so the user can see them.
//! The MB3D camera (rays starting on a pixel grid plane) becomes a pinhole camera at the
//! point where the MB3D rays converge. Shading follows MB3D (see `docs/mb3d-notes.md`).

use crate::gradient::srgb_to_linear;
use crate::math::{V3, v3};
use crate::formula::lang::DeKind;
use crate::scene::{AoMode, BackgroundMode, ColorMode, ColorStop, CombineMode, DeCombine, DiffuseCurve, FogMode, GlowShape, Light, LightKind, Post, Scene, Slot};

/// Result of an import: the scene and what could not be converted.
pub struct Import {
    pub scene: Scene,
    pub warnings: Vec<String>,
}

const HEADER: usize = 840;
const SLOT: usize = 188;
const ALPHABET: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const INTERNAL: [&str; 10] = ["Integer Power", "Real Power", "Quaternion", "Tricorn", "Amazing Box", "Bulbox", "Folding Int Pow", "test", "testIFS", "Aexion C"];

struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn bytes<const N: usize>(&self, off: usize) -> [u8; N] {
        let mut b = [0u8; N];
        for (i, v) in b.iter_mut().enumerate() {
            *v = self.0.get(off + i).copied().unwrap_or(0);
        }
        b
    }
    fn u8(&self, off: usize) -> u8 {
        self.0.get(off).copied().unwrap_or(0)
    }
    fn u16(&self, off: usize) -> u16 {
        u16::from_le_bytes(self.bytes(off))
    }
    fn i32(&self, off: usize) -> i32 {
        i32::from_le_bytes(self.bytes(off))
    }
    fn u32(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.bytes(off))
    }
    fn f32(&self, off: usize) -> f64 {
        f32::from_le_bytes(self.bytes(off)) as f64
    }
    fn f64(&self, off: usize) -> f64 {
        f64::from_le_bytes(self.bytes(off))
    }
    /// 7 upper bytes of an f64.
    fn d7(&self, off: usize) -> f64 {
        let b: [u8; 7] = self.bytes(off);
        f64::from_le_bytes([0, b[0], b[1], b[2], b[3], b[4], b[5], b[6]])
    }
    /// ShortFloat: mantissa and decimal exponent.
    fn short_float(&self, off: usize) -> f64 {
        let (m, e) = (self.u8(off) as i8, self.u8(off + 1) as i8);
        m as f64 * 10f64.powi(e.clamp(-25, 25) as i32 - 1)
    }
    fn rgb(&self, off: usize) -> [f32; 3] {
        [srgb_to_linear(self.u8(off)), srgb_to_linear(self.u8(off + 1)), srgb_to_linear(self.u8(off + 2))]
    }
    fn cstr(&self, off: usize, len: usize) -> String {
        let b: Vec<u8> = (0..len).map(|i| self.u8(off + i)).take_while(|c| *c != 0).collect();
        String::from_utf8_lossy(&b).trim().to_string()
    }
}

/// Clipboard text starts like `Mandelbulb3Dv18{`.
pub fn looks_like_text(t: &str) -> bool {
    t.trim_start().starts_with("Mandelbulb3D")
}

/// Decodes the radix-64 payload of the text format.
pub fn decode_text(t: &str) -> Result<(Vec<u8>, String), String> {
    let t = t.trim_start();
    let open = t.find('{').ok_or("no '{' in the Mandelbulb3D text")?;
    let version: f64 = t[..open].trim_start_matches("Mandelbulb3D").trim_start_matches(['v', 'V']).trim().parse().unwrap_or(0.0);
    if version < 16.0 {
        return Err(format!("Mandelbulb3D text version {version} is too old (16 or newer needed)"));
    }
    let close = t[open + 1..].find('}').map(|i| open + 1 + i).ok_or("no '}' in the Mandelbulb3D text")?;
    let chars: Vec<u8> = t[open + 1..close].bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(chars.len() / 4 * 3);
    for g in chars.chunks(4) {
        if g.len() < 4 {
            break;
        }
        let mut n: u32 = 0;
        let mut ok = true;
        for (k, c) in g.iter().enumerate() {
            match ALPHABET.iter().position(|a| a == c) {
                Some(v) => n |= (v as u32) << (6 * k),
                None => ok = false,
            }
        }
        if ok {
            out.extend_from_slice(&[(n & 255) as u8, ((n >> 8) & 255) as u8, ((n >> 16) & 255) as u8]);
        }
    }
    let rest = &t[close + 1..];
    let title = rest
        .find("{Titel:")
        .map(|i| rest[i + 7..].split(['}', '\r', '\n']).next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    Ok((out, title))
}

pub fn import_text(t: &str) -> Result<Import, String> {
    let (bytes, title) = decode_text(t)?;
    if bytes.len() < HEADER {
        return Err("the Mandelbulb3D text is too short".into());
    }
    let mut imp = import_binary(&bytes)?;
    if !title.is_empty() {
        imp.scene.notes = format!("{title}\n{}", imp.scene.notes).trim().to_string();
    }
    Ok(imp)
}

pub fn import_binary(b: &[u8]) -> Result<Import, String> {
    let r = Rd(b);
    let mand_id = r.i32(0);
    if b.len() < 100 || !(20..=45).contains(&mand_id) {
        return Err(if (0..20).contains(&mand_id) {
            format!("Mandelbulb3D parameter version {mand_id} is too old (version 20 or newer needed)")
        } else {
            "not a Mandelbulb3D parameter file".into()
        });
    }
    // Version 45 (a test build) has 8 more header bytes and 8 formula slots.
    let (shift, n_slots) = if mand_id == 45 { (8usize, 8usize) } else { (0, 6) };
    let light = 432 + shift;
    let addon = HEADER + shift;
    let mut warnings = Vec::new();
    let mut s = Scene::default();

    let width = r.i32(4);
    let height = r.i32(8);
    if !(1..32768).contains(&width) || !(1..32768).contains(&height) {
        return Err(format!("implausible image size {width} x {height}"));
    }
    (s.width, s.height) = (width as u32, height as u32);

    // ---- camera
    let zoom = r.f64(84).max(1e-300);
    let sw = 2.1345 / (zoom * width as f64);
    let mut rows = [[0.0f64; 3]; 3];
    for (i, row) in rows.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = r.f64(246 + (i * 3 + j) * 8);
        }
    }
    if mand_id > 43 && r.u8(18) & 1 != 0 {
        let q = [rows[0][0], rows[0][1], rows[0][2], rows[1][0]];
        rows = [
            [1.0 - 2.0 * (q[1] * q[1] + q[2] * q[2]), 2.0 * (q[0] * q[1] + q[2] * q[3]), 2.0 * (q[0] * q[2] - q[1] * q[3])],
            [2.0 * (q[0] * q[1] - q[2] * q[3]), 1.0 - 2.0 * (q[0] * q[0] + q[2] * q[2]), 2.0 * (q[2] * q[1] + q[0] * q[3])],
            [2.0 * (q[0] * q[2] + q[1] * q[3]), 2.0 * (q[1] * q[2] - q[0] * q[3]), 1.0 - 2.0 * (q[0] * q[0] + q[1] * q[1])],
        ];
    }
    let r0 = V3::from_array(rows[0]).normalize();
    let r1 = V3::from_array(rows[1]).normalize();
    let r2 = V3::from_array(rows[2]).normalize();
    let mid = v3(r.f64(36), r.f64(44), r.f64(52));
    let (z_start, z_end, z_mid) = (r.f64(20), r.f64(28), r.f64(52));
    let mut fov = r.f64(108);
    if !(1.0..=170.0).contains(&fov) {
        fov = fov.clamp(1.0, 170.0);
    }
    // Optic 1 (planar) is the pinhole camera used here; 0 differs slightly toward the corners.
    if r.u8(148) & 3 == 2 {
        warnings.push("the panorama camera optic is shown as a normal perspective camera".into());
    }
    let half = (fov.to_radians() * 0.5).tan();
    let vp_off = sw * height as f64 * 0.5 / half;
    s.camera.fov = fov;
    s.camera.right = r0;
    s.camera.up = -r1;
    s.camera.forward = r2;
    s.camera.position = mid + r2 * (z_start - z_mid - vp_off);
    s.camera.orthonormalize();
    s.march.max_distance = ((z_end - z_start).abs() + vp_off).max(1e-6);
    // Mandelbulb3D's rays start on a plane in front of the eye.
    s.march.near = vp_off * 0.999;

    // ---- iteration and DE
    let f = &mut s.fractal;
    f.max_iterations = r.i32(12).clamp(1, 10_000) as u32;
    f.bailout = r.f64(92).abs().max(1.0);
    f.julia = r.u8(190) != 0;
    f.julia_c = [r.f64(191), r.f64(199), r.f64(207), r.f64(215)];
    f.rotation_4d = [r.f64(60).to_degrees(), r.f64(68).to_degrees(), r.f64(76).to_degrees()];
    s.march.detail = r.f32(177).max(0.001);
    let step = r.f32(182);
    s.march.step = if step > 1e-4 { step.min(1.0) } else { 0.5 };
    s.march.refine_steps = (r.u8(134) as u32).min(16);
    // MB3D marches without a step limit; small ray steps need many.
    s.march.max_steps = ((1.0 / s.march.step) * 300.0).clamp(400.0, 3000.0) as u32;

    // ---- formulas
    let hybrid_type = r.u8(addon + 1) & 3;
    if r.u8(addon) < 16 {
        warnings.push("old formula section layout; formulas could not be read".into());
    }
    if hybrid_type == 3 {
        warnings.push("(K/L)IFS hybrid mode is not supported".into());
    }
    // Raw slots: (iterations, formula number, name, option values).
    let mut raw: Vec<(i32, i32, String, Vec<f64>)> = (0..n_slots)
        .map(|i| {
            let o = addon + 8 + i * SLOT;
            let fnr = r.i32(o + 4);
            let name = if (0..10).contains(&fnr) { INTERNAL[fnr as usize].to_string() } else { r.cstr(o + 12, 32) };
            let n_opt = (r.i32(o + 8).clamp(0, 16)) as usize;
            let mut values: Vec<f64> = (0..n_opt).map(|k| r.f64(o + 60 + k * 8)).collect();
            // Bulbox got a second threshold later; old files use the first one for both
            // (FileHandling.UpdateFormulaOptionAbove20).
            if fnr == 5 && values.len() == 5 {
                values.push(values[4]);
            }
            (r.i32(o), fnr, name, values)
        })
        .collect();
    let mut hyb1 = r.u8(addon + 5) as u32;
    let mut hyb2 = r.u16(addon + 6) as u32;
    let mut bopt3 = r.u8(addon + 3) as u32;
    let mut max_its2 = r.i32(418 + shift);
    // Upgrades of older versions (FileHandling.pas): the second iteration limit and the
    // smoothing distance of the DE combination did not exist yet.
    if mand_id < 33 {
        max_its2 = f.max_iterations as i32;
    }
    let comb_smooth = if mand_id < 31 { 0.5 } else { r.f32(378) };
    // Before version 42 the hybrid options were stored differently (FileHandling.pas).
    if mand_id < 42 && (0..n_slots).all(|i| i < 6) {
        let vol = r.u8(343).min(5) as u32;
        let j = (0..6).rev().find(|&k| raw[k].0 != 0).unwrap_or(0);
        hyb1 = if hybrid_type != 0 { 0 } else { j as u32 | (vol << 4) };
        hyb2 = 0x151;
        if hybrid_type == 2 {
            hyb2 = 1 | ((j as u32) << 4) | (vol.max(1) << 8);
            raw[0].0 = 1;
            if !matches!(bopt3, 2 | 6) {
                let its1 = f.max_iterations as i32;
                f.max_iterations = max_its2.clamp(1, 10_000) as u32;
                max_its2 = its1;
            }
        } else {
            bopt3 = 0;
        }
        if matches!(bopt3, 2 | 6) {
            let first = raw.remove(0);
            raw.insert(j, first);
            if bopt3 == 6 {
                bopt3 = 5;
            }
            hyb1 = (j as u32).saturating_sub(1) | (vol.saturating_sub(1) << 4);
            hyb2 = j as u32 | ((j as u32) << 4) | ((j as u32) << 8);
        }
    }
    let (start2, end2, repeat2) = ((hyb2 & 15) as usize, ((hyb2 >> 4) & 15) as usize, ((hyb2 >> 8) & 15) as usize);
    // Inside rendering: 1 = inside, 2 = inside and outside (decided below by the camera).
    let inside_mode = (r.u8(addon + 2) >> 1) & 3;
    let end1 = (hyb1 & 15) as usize;
    let repeat1 = (hyb1 >> 4) as usize;
    let lib = crate::formula::library();
    let mut slots = Vec::new();
    let mut missing = Vec::new();
    // For the color index: (iterations, DE kind, first option value, name) of each used slot.
    let mut color_info: Vec<(f64, DeKind, f64, String)> = Vec::new();
    // Interpolation hybrid: the iteration counts of the first two formulas are float weights.
    let weight = |its: i32| f32::from_bits(its as u32) as f64;
    if hybrid_type == 1 {
        let w: Vec<f64> = raw.iter().take(2).map(|r| weight(r.0)).collect();
        if w.len() == 2 && w.iter().all(|v| v.is_finite()) {
            f.blend = Some([w[0], w[1]]);
        }
        if raw.iter().skip(2).any(|r| r.0 != 0 && r.1 >= 0) {
            warnings.push("an interpolation hybrid uses only its first two formulas".into());
        }
    }
    for (i, (its, fnr, name, values)) in raw.into_iter().enumerate() {
        if hybrid_type == 1 && (i > 1 || fnr < 0) {
            continue;
        }
        if hybrid_type != 1 && (its <= 0 || fnr < 0) {
            continue;
        }
        if hybrid_type == 0 && i > end1 && end1 > 0 {
            continue;
        }
        if hybrid_type == 2 && i > end1 && (i < start2 || i > end2) {
            continue;
        }
        let iterations = if hybrid_type == 1 { 1 } else { its.min(1000) as u32 };
        let mut slot = Slot { formula: name.clone(), iterations, ..Slot::default() };
        match lib.find_alias(&name) {
            Some(fm) => {
                let alias = fm.def.aliases.iter().find(|a| a.name.eq_ignore_ascii_case(&name));
                let idents: Vec<String> = match alias {
                    Some(a) if !a.params.is_empty() => a.params.clone(),
                    _ => fm.def.params.iter().map(|p| p.ident.clone()).collect(),
                };
                for (ident, v) in idents.iter().zip(&values) {
                    if ident != "_" && v.is_finite() {
                        slot.params.insert(ident.clone(), *v);
                    }
                }
                slot.formula = fm.def.name.clone();
                if !fm.def.transform {
                    color_info.push((its as f64, fm.def.de, values.first().copied().unwrap_or(0.0), name.to_lowercase()));
                }
            }
            None => missing.push(name),
        }
        slots.push((i, slot));
    }
    drop(lib);
    if !missing.is_empty() {
        warnings.push(format!("formulas not available yet: {}", missing.join(", ")));
    }
    if slots.is_empty() {
        warnings.push("no usable formula; using the default Mandelbulb".into());
    } else {
        f.repeat_from = slots.iter().position(|(i, _)| *i >= repeat1).unwrap_or(0);
        if hybrid_type == 2 {
            // DE combination: two chains, the second from slot start2 to end2.
            let start = slots.iter().position(|(i, _)| *i >= start2.max(end1 + 1));
            if let Some(start) = start.filter(|s| *s > 0) {
                let ftype = (bopt3 as i32 + 1).clamp(1, 6);
                let mode = match ftype {
                    2 => CombineMode::Max,
                    3 => CombineMode::Subtract,
                    4 => CombineMode::SmoothLinear,
                    5 => CombineMode::Smooth,
                    6 => CombineMode::Mix,
                    _ => CombineMode::Min,
                };
                if bopt3 >= 6 {
                    warnings.push("DE-combination mix with the second chain first is shown with the first chain first".into());
                }
                let max2 = max_its2;
                f.combine = Some(DeCombine {
                    mode,
                    start,
                    repeat_from: slots.iter().position(|(i, _)| *i >= repeat2.max(start2)).unwrap_or(start).max(start),
                    max_iterations: if max2 > 0 { max2.min(10_000) as u32 } else { f.max_iterations },
                    smooth: comb_smooth.abs().max(1e-6),
                    // sFmixPow (f32 at 104), 2 before version 39.
                    mix_pow: Some(r.f32(104)).filter(|v| v.is_finite() && *v != 0.0 && mand_id >= 39).unwrap_or(2.0),
                });
            } else {
                warnings.push("DE combination without a second formula chain imported as an alternating hybrid".into());
            }
        }
        f.slots = slots.into_iter().map(|(_, s)| s).collect();
    }

    match inside_mode {
        1 => s.fractal.inside = true,
        2 => {
            // Inside and outside: inside rendering when the camera starts in the set.
            let prog = crate::fractal::Program::for_scene(&s);
            let mut ev = prog.evaluator();
            s.fractal.inside = ev.de(s.camera.position, 1e-9) <= 0.0;
        }
        _ => {}
    }

    // ---- cutting planes: the half-space between camera and plane is removed
    let cut = r.u8(176);
    for (axis, (plane, pos_off)) in [(&mut s.cut.x, 346), (&mut s.cut.y, 354), (&mut s.cut.z, 362)].into_iter().enumerate() {
        if cut & (1 << axis) != 0 {
            plane.enabled = true;
            plane.position = r.f64(pos_off);
            plane.keep_above = r2.to_array()[axis] > 0.0;
        }
    }

    // ---- lighting and colors: Mandelbulb3D shades with the colors as shown (0..255), so the
    // scene does the same (display space) and the factors below follow MakeLightValsFromHeaderLight.
    let tb = |k: usize| r.i32(light + 8 + 4 * (k - 3));
    let tb_options = r.u32(light + 44);
    let add_options = r.u8(light + 7);
    // "Internal gamma of 2": colors are squared, which is close to a linear workflow.
    let sqr_mode = add_options & 1 != 0;
    let light_scale = if sqr_mode { 1.5 } else { 1.0 };
    let soft = |v: f64| if sqr_mode { (v * v + v) * 0.5 } else { v };
    let s_diff = soft(tb(5) as f64 * 0.02);
    let s_spec = soft(((tb(7) & 0xFFF) as f64 * 0.02).max(0.004));
    let hs_flags = r.u8(133);
    // Lights without (calculated) shadows are darkened by this share of the ambient occlusion.
    let diffuse_shadowing = r.u8(light + 68 + 3 * 32 + 16) as f64 / 256.0;
    let mut lights = Vec::new();
    let mut powers = Vec::new();
    for i in 0..6 {
        let lo = light + 68 + i * 32;
        let opt = r.u8(lo);
        if opt & 3 != 0 {
            continue;
        }
        let func = r.u8(lo + 1);
        let lamp = r.short_float(lo + 2);
        if lamp <= 0.0 {
            continue;
        }
        let color = r.rgb(lo + 4);
        let point = opt & 4 != 0;
        let dir_cam = if point {
            // Positional light: the direction is only used for the editor's angles.
            let pos = v3(r.d7(lo + 9), r.d7(lo + 17), r.d7(lo + 25));
            let d = (pos - mid).normalize();
            v3(d.dot(r0), d.dot(r1), d.dot(r2))
        } else {
            let (lx, ly) = (r.d7(lo + 9), r.d7(lo + 17));
            -v3(lx.sin(), ly.sin(), ly.cos() * lx.cos()).normalize()
        };
        let world = opt & 32 != 0 || opt & 4 != 0;
        // Phong exponent of the light; the scene's Blinn highlight needs about four times it.
        let power = (2u32 << (func & 7)) as f64;
        powers.push(power);
        // Shadows switched on but not calculated: the ambient occlusion stands in for them.
        let hs_enabled = opt & 64 == 0;
        let hs_calculated = hs_enabled && hs_flags & (4 << i) != 0;
        let mut l = Light {
            color,
            diffuse: [DiffuseCurve::Lambert, DiffuseCurve::Squared, DiffuseCurve::Wrap, DiffuseCurve::WrapSquared][((func >> 4) & 3) as usize],
            occlusion: if hs_enabled && !hs_calculated { 1.0 } else { diffuse_shadowing },
            intensity: (lamp * s_diff * light_scale).clamp(0.0, 20.0),
            kind: if point { LightKind::Point } else { LightKind::Directional },
            position: v3(r.d7(lo + 9), r.d7(lo + 17), r.d7(lo + 25)),
            shadows: hs_calculated,
            softness: 0.02,
            camera_relative: !world,
            ..Light::default()
        };
        if point {
            // Point light colors are scaled by 1.3 and the light scale once more; the light falls
            // off with the squared distance (no clamp: the falloff needs large values).
            l.intensity = lamp * 1.3 * light_scale * light_scale * s_diff;
        }
        // Visible light source (iLightPos bits 1..3: 2 sharp, 4 halo, 6 ball, others soft).
        let vis = ((opt >> 2) & 6) | ((func & 128) >> 4);
        if vis != 0 {
            l.visible = true;
            l.glow_shape = match vis {
                2 => GlowShape::Sharp,
                4 => GlowShape::Halo,
                6 => GlowShape::Ball,
                _ => GlowShape::Soft,
            };
            // Squared glow radius sLmaxL * 1e-8 (HeaderTrafos), from the color sum in 0..255.
            let bytes = (r.u8(lo + 4) as f64) + (r.u8(lo + 5) as f64) + (r.u8(lo + 6) as f64);
            let (mut r2, glow) = if point {
                let k = lamp * 1.3 * light_scale * light_scale;
                (800.0 * (bytes * k + 128.0 * lamp) * 1e-8, k)
            } else {
                (800.0 * (bytes * light_scale + 128.0) * lamp * 1e-8, lamp * light_scale)
            };
            if vis == 8 {
                r2 *= 5.0;
            }
            l.glow_size = if point { r2.max(0.0).sqrt() } else { (1.0 - r2.clamp(0.0, 2.0)).acos().to_degrees() };
            l.glow_intensity = glow;
        }
        // Specular strength relative to the diffuse one (set after the shininess is known).
        l.specular = s_spec / s_diff.max(1e-3);
        if world {
            let w = (r0 * dir_cam.x + r1 * dir_cam.y + r2 * dir_cam.z).normalize();
            l.azimuth = w.y.atan2(w.x).to_degrees();
            l.elevation = w.z.clamp(-1.0, 1.0).asin().to_degrees();
        } else {
            // Camera space: x right, y down, z forward.
            l.elevation = (-dir_cam.y).clamp(-1.0, 1.0).asin().to_degrees();
            l.azimuth = dir_cam.x.atan2(-dir_cam.z).to_degrees();
        }
        lights.push(l);
    }
    if !lights.is_empty() {
        let shin = (4.0 * powers.iter().sum::<f64>() / powers.len() as f64).clamp(2.0, 1024.0);
        s.lighting.shininess = shin;
        // The scene scales the highlight by (shininess + 8) / 25; Mandelbulb3D does not.
        for l in &mut lights {
            l.specular *= 25.0 / (shin + 8.0);
        }
        s.lighting.lights = lights;
    } else {
        s.lighting.lights.clear();
    }
    s.lighting.sky_color = r.rgb(light + 52);
    s.lighting.ground_color = r.rgb(light + 56);
    s.lighting.ambient = soft((tb(8) & 0xFFF) as f64 / 90.0) * light_scale;
    s.lighting.ambient_camera_relative = tb_options & 0x2000_0000 == 0;
    let ao_flags = r.u8(149);
    s.lighting.ao_mode = if ao_flags & 1 != 0 { AoMode::Raytraced } else { AoMode::Off };
    s.lighting.ao_strength = ((tb(11) & 0xFF) as f64 / 53.0).clamp(0.0, 4.0);
    // MB3D's ambient shadow reaches far: the screen-space kinds search up to about half the image,
    // DEAO rays up to DEAOmaxL (374) times half the image diagonal. As a share of the distance:
    let pa = fov.to_radians() / height as f64;
    let half_diag = 0.5 * ((width * width + height * height) as f64).sqrt();
    s.lighting.ao_radius = if (ao_flags >> 2) & 3 == 3 {
        r.f32(374).clamp(0.0, 100.0) * half_diag * pa
    } else {
        0.5 * width as f64 * pa
    }
    .clamp(0.02, 2.0);

    // ---- surface colors: 10 stops (position 0..32767, diffuse and specular color)
    let cycling = tb_options & 0x4000 != 0;
    let mut stops: Vec<ColorStop> = (0..10)
        .map(|k| {
            let o = light + 260 + k * 10;
            ColorStop { pos: r.u16(o) as f64 / 32767.0, color: r.rgb(o + 2), specular: r.rgb(o + 6), transparency: r.u8(o + 9) as f32 / 255.0, emission: 0.0 }
        })
        .collect();
    stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    // Cycling colors wrap from the last stop to the first, else the last color stays.
    let end = if cycling { stops.first().cloned() } else { stops.last().cloned() };
    if let Some(end) = end {
        stops.push(ColorStop { pos: 1.0, ..end });
    }
    s.material.gradient = stops;
    s.material.repeat = cycling;
    // The stops carry the specular colors.
    s.material.specular = 1.0;
    // Mandelbulb3D's color start and end (HeaderTrafos.CalcSCstartAndSCmul), shared by both
    // color choices: position = (index - start) / (end - start).
    let slider = |v: i32| ((v + 30) as f64 / 90.0).powi(2) * 32767.0 - 10900.0;
    let (mut c_start, mut c_end) = (slider(tb(9)), slider(tb(10)));
    if tb_options & 0x10000 != 0 {
        let span = c_end - c_start;
        let (f1, f2) = (r.u8(light + 48) as f64, r.u8(light + 49) as f64);
        c_end = c_start + span * (f2 - 30.0) / 60.0;
        c_start += span * (f1 - 30.0) / 60.0;
    }
    let mut span = c_end - c_start;
    if span.abs() < 1e-3 {
        span = 1e-3f64.copysign(span);
    }
    // The second color choice (byColor2Option #342, last length increase multiplier #338) and
    // color on iteration (bColorOnIt, byte 19), with the conversions of older files.
    let (mut option, mut color_on_it) = (r.u8(342), r.u8(19));
    let mut lli = r.f32(338);
    if mand_id < 22 {
        option = 0;
        lli = 1.0;
    }
    if mand_id < 43 {
        color_on_it = u8::from(option == 5);
        if option == 6 {
            option = 5;
        }
    }
    s.material.second_choice = crate::scene::SecondChoice::from_mb3d(option);
    s.material.lli_multiplier = if lli.is_finite() { lli } else { 1.0 };
    s.material.color_on_iteration = color_on_it as i32 - 1;
    if tb_options & (1 << 17) != 0 {
        // Colors on the second choice (CalcColors: index = the 15-bit value).
        s.material.color_mode = ColorMode::SecondChoice;
        s.material.color_scale = 32767.0 / span;
        s.material.color_offset = -c_start / span;
    } else {
        // Mandelbulb3D's color index (CalcThread, CalcColors):
        //   index = 32767 - (smooth its + col_plus + var_mul * ln(threshold)) * 32767 / (max its + 1)
        let max_its = s.fractal.combine.as_ref().map_or(s.fractal.max_iterations, |c| c.max_iterations.max(s.fractal.max_iterations));
        let m = 32767.0 / (max_its as f64 + 1.0);
        let (mut y1, mut z1, mut box_sum, mut box_its) = (0.0, 0.0, 0.0, 0.0);
        for (its, de, first, name) in &color_info {
            if *de == DeKind::Numeric {
                y1 += its * 0.6;
                z1 += its;
            } else if first.abs() > 1.0 {
                let mut x = first.abs();
                if x < 1.2 {
                    x = 1.1 + (x - 1.0).powi(2) * 2.5;
                }
                y1 += its * 0.5 / (x.ln() + 0.03);
                z1 += its;
            }
            if *de == DeKind::Linear && name.contains("box") {
                box_sum += first.abs() * its;
                box_its += its;
            }
        }
        let var_mul = if z1 > 1e-3 { y1 / z1 } else { 0.6 };
        let col_plus = if box_its > 0.0 { (box_sum / box_its).powf(0.25) * 40.0 - 49.0 } else { 0.0 } + max_its as f64 * 0.1;
        s.material.color_mode = ColorMode::IterationsRatio;
        s.material.color_scale = -m / span;
        s.material.color_offset = (32767.0 - c_start - m * col_plus) / span;
        s.material.detail_shift = var_mul;
        // The orbit-mode color map reads MB3D's color index as 0..1.
        s.material.color_map.iter_scale = -m / 32767.0;
        s.material.color_map.iter_offset = 1.0 - col_plus * m / 32767.0;
    }

    // ---- color map (diffuse map, LightVals: iColOnOT): number bColorMap | Ex << 8; mode from
    // Lights[1].FreeByte (orbit trap × index, normals, 3D waves, 3D projection).
    let map_nr = r.u8(light + 3) as u32 | (r.u8(light + 68 + 32 + 16) as u32) << 8;
    if map_nr != 0 {
        let cm = &mut s.material.color_map;
        cm.image = map_nr.to_string();
        cm.mode = [crate::scene::MapMode::Orbit, crate::scene::MapMode::Normal, crate::scene::MapMode::Wrap, crate::scene::MapMode::Triplanar][(r.u8(light + 68 + 32 + 24) & 3) as usize];
        let ex2 = r.u8(light + 68 + 64 + 16) as i32 - 30;
        cm.scale = if cm.mode == crate::scene::MapMode::Orbit { 1.05f64.powi(ex2) } else { 1.2f64.powi(ex2) };
        let (a, b, c) = (((tb(7) >> 12) & 0xFF) as f64, ((tb(7) >> 20) & 0xFF) as f64, ((tb(8) >> 20) & 0xFF) as f64);
        cm.offset = [a / 256.0, b / 256.0];
        cm.rotation = if cm.mode == crate::scene::MapMode::Orbit { [0.0, 0.0, (c - 128.0) * 360.0 / 256.0] } else { [180.0 - a * 180.0 / 128.0, b * 180.0 / 128.0, c * 180.0 / 128.0] };
        cm.luminance_only = add_options & 4 != 0;
        if crate::maps::get_any(&cm.image).is_none() {
            warnings.push(format!("color map {map_nr} not found (set the Mandelbulb3D maps folder in the preferences); the gradient is used"));
        }
    }

    // ---- background and depth fog: the background is the vertical gradient of the depth
    // colors, and the fog takes that color.
    s.background.mode = BackgroundMode::Vertical;
    s.background.top = r.rgb(light + 60);
    s.background.bottom = r.rgb(light + 64);
    s.background.horizon = [0, 1, 2].map(|k| (s.background.top[k] + s.background.bottom[k]) * 0.5);
    s.background.curve = match tb_options >> 30 {
        0 => 1.0,
        1 => 2.0,
        _ => 0.5,
    };
    s.fog.color = s.background.top;
    s.fog.from_background = true;
    s.fog.mode = FogMode::Range;
    s.fog.density = 0.0;
    // Fog amount = (28000 - Zpos) * sDepth, Zpos = 32767 * (1 - u), u the depth buffer curve.
    let s_depth = tb(4) as f64 * 0.8e-6;
    s.fog.strength = 32767.0 * s_depth;
    s.fog.start = 4767.0 / 32767.0;
    let z_corr = (fov.to_radians().max(1.0) / height as f64).sin();
    s.fog.curve = ((z_end - z_start) * z_corr / sw).max(0.0);
    s.fog.squared = tb_options & 0x40000 != 0;

    // ---- dynamic fog (glow along the ray steps)
    let step_div = s.march.step;
    let (mut gr, mut off_mul, mut mix_mul, mut base) = (
        (tb(6) - 53) as f64 * step_div * 0.00065,
        2.2 / step_div,
        step_div * 0.015,
        128.0,
    );
    if r.u8(223) > 0 {
        off_mul *= 0.25;
        gr *= 4.0;
        mix_mul *= 4.0;
    } else {
        base = 137.0;
    }
    let off = (base - ((tb(3) & 0xFFFF) as f64).sqrt() * 11.313708) * off_mul * 0.28;
    s.fog.glow = gr * s.march.max_steps as f64;
    s.fog.glow_offset = off.max(0.0);
    s.fog.glow_color = [srgb_to_linear(r.u8(light + 55)), srgb_to_linear(r.u8(light + 59)), srgb_to_linear(r.u8(light + 63))];
    s.fog.glow_color2 = r.rgb(light + 4);
    s.fog.glow_mix_steps = if mix_mul > 0.0 { 1.0 / mix_mul } else { 0.0 };
    // Dynamic fog on iteration N: only ray steps whose estimate stopped after N iterations count.
    s.fog.glow_iteration = r.u8(223) as u32;
    let dfog_opts = r.u8(light + 68 + 24) & 3;
    s.fog.glow_blend = dfog_opts & 1 != 0;

    // ---- output: no tone mapping or vignette; MB3D's gamma slider mixes toward sqrt / square.
    let g = (tb_options >> 23) & 0x3F;
    let gamma = match g.cmp(&32) {
        std::cmp::Ordering::Equal => 2.2,
        std::cmp::Ordering::Greater => 2.2 * (1.0 + (g as f64 - 32.0) / 31.0),
        std::cmp::Ordering::Less => 2.2 / (1.0 + (1.0 - g as f64 / 32.0)),
    };
    s.post = Post {
        exposure: 0.0,
        gamma: if sqr_mode { gamma * 2.0 / 2.2 } else { gamma },
        contrast: 1.0,
        saturation: 1.0,
        tone_map: false,
        vignette: 0.0,
        display_space: !sqr_mode,
        ..Post::default()
    };

    // ---- depth of field (DOF.pas): blur radius in pixels = Ap * (z - z_sharp) / pixel size(z)
    if r.u8(181) & 1 != 0 {
        let pa = fov.to_radians() / height as f64;
        let (z1, z2) = (r.f32(164) * width as f64, r.f32(410 + shift) * width as f64);
        let (lo, hi) = (z1.min(z2), z1.max(z2));
        let w = (1.0 + lo * pa) / (1.0 + hi * pa);
        let sharp = if z2 > 0.0 { (w * hi + lo) / (1.0 + w) } else { z1 };
        // Blur radius factor per pass (1 to 4 passes); the passes add up like independent blurs.
        let ap0 = r.f32(172).clamp(0.0001, 2.0);
        let ap = match (r.u8(181) >> 1) & 3 {
            0 => ap0 * 0.5,
            1 => ap0 * 0.5 * (0.3f64.powi(2) + 0.85f64.powi(2)).sqrt(),
            lp => ap0 / 12.0 * (0..=lp).map(|n| 4f64.powi(n as i32)).sum::<f64>().sqrt(),
        };
        s.dof.enabled = true;
        s.dof.focus_distance = vp_off + sharp.max(0.0) * sw;
        // Lens radius = Ap * focus distance; the scene's lens radius is aperture * focus * 0.1.
        s.dof.aperture = ap * 10.0;
    }
    // ---- interior colors (ICols at light +360: 4 × position u16, RGB, specular byte), used
    // where rays meet the inside (cut planes); index (CalcSCstartAndSCmul, inside):
    // position = 0.5 · (v · 32767 - s⁴k) / ((e⁴ - s⁴)k), v = Rout / RStop, k = 0.000158025.
    {
        let mut istops: Vec<ColorStop> = (0..4)
            .map(|k| {
                let o = light + 360 + k * 6;
                let spec = srgb_to_linear(r.u8(o + 5));
                ColorStop { pos: r.u16(o) as f64 / 32767.0, color: r.rgb(o + 2), specular: [spec; 3], transparency: 0.0, emission: 0.0 }
            })
            .collect();
        istops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        let end = if cycling { istops.first().cloned() } else { istops.last().cloned() };
        if let Some(end) = end {
            istops.push(ColorStop { pos: 1.0, ..end });
        }
        let s4 = ((tb_options & 0x7F) as f64).powi(4);
        let e4 = (((tb_options >> 7) & 0x7F) as f64).powi(4);
        let k = 0.000158025;
        let span = if ((e4 - s4) * k).abs() < 1e-10 { 1e-10 } else { (e4 - s4) * k };
        s.material.interior = istops;
        s.material.interior_scale = 0.5 * 32767.0 / span;
        s.material.interior_offset = -0.5 * s4 * k / span;
    }

    // ---- color variation with depth (VarColZpos, light +0): the gradient position moves by
    // 0.5 · sColZmul per world unit of depth beyond the mid plane, sColZmul = VarColZpos ·
    // -0.005 / (stepWidth · Width).
    let var_col_z = r.u16(light) as i16 as f64;
    if var_col_z != 0.0 {
        s.material.depth_shift = 0.5 * var_col_z * -0.005 / (sw * width as f64);
        s.material.depth_reference = (mid - s.camera.position).length();
    }

    // ---- smoothed normals (iOptions bits 6..9) and the roughness factor (light record +2)
    s.march.smooth_normals = ((r.u16(16) >> 6) & 15).min(8) as u32;
    s.lighting.roughness = r.u8(light + 2) as f64 / 255.0;

    // ---- reflections (CalcSR): SRamount (332) when they are calculated automatically (336
    // bit 0), SRreflectioncount (337) bounces. Transmission (bit 1): refraction index sTRIndex
    // (116), scattering sTRscattering (120), absorption sTransmissionAbsorption (370, per world
    // unit: CalcSR multiplies it with dStepWidth and the path length in steps).
    let sr = r.u8(336);
    if sr & 1 != 0 {
        s.material.reflection = r.f32(332).clamp(0.0, 100.0);
        s.material.reflection_bounces = (r.u8(337) as u32).clamp(1, 8);
        if sr & 2 != 0 {
            s.material.transmission = true;
            // In-and-outside rendering refracts nothing (CalcSRT).
            s.material.refraction_index = if inside_mode == 2 { 1.0 } else { r.f32(116).clamp(1.0, 5.0) };
            s.material.scattering = r.f32(120).clamp(0.0, 10000.0);
            s.material.absorption = r.f32(370).clamp(0.0, 1.0e6);
        }
        if sr & 4 != 0 {
            warnings.push("reflections and transmission only on dIFS formulas are shown on all surfaces".into());
        }
    }

    // ---- background image (spherical; LightVals: BuildRotMatrixS(π - offX·π/128, offY·π/128,
    // offZ·π/128)); the depth fog keeps the vertical gradient.
    let bg_name = r.cstr(light + 384, 24);
    if !bg_name.is_empty() {
        let off = |o: usize| r.u8(light + o) as f64 * 180.0 / 128.0;
        s.background.image = bg_name.clone();
        s.background.image_rotation = [180.0 - off(50), off(51), off(67)];
        // LoadBackgroundPicT and MakeLightValsFromHeaderLight: brightness 1.04^(Lights[4]
        // AdditionalByteEx - 40), screen coordinates (TBoptions bit 15), convert to spherical
        // (AdditionalOptions bit 7), fit border (bit 4), fog added (bit 3), ambient (bit 5).
        s.background.image_intensity = 1.04f64.powi(r.u8(light + 68 + 4 * 32 + 16) as i32 - 40);
        s.background.image_screen = tb_options & 0x8000 != 0;
        s.background.image_spherical = add_options & 0x80 != 0;
        s.background.image_fit_border = add_options & 0x10 != 0;
        s.background.image_fog_add = add_options & 0x08 != 0;
        if add_options & 0x20 != 0 {
            s.lighting.ambient_from_background = true;
            // The image is the ambient light itself; the sky and ground colors are not used.
            s.lighting.ambient = light_scale;
        }
        if tb_options & 0x80000 != 0 {
            warnings.push("the background image is not smoothed on load like Mandelbulb3D does".into());
        }
        if crate::maps::get_file(&bg_name).is_none() {
            warnings.push(format!("background image '{bg_name}' not found (set the Mandelbulb3D maps folder in the preferences)"));
        }
    }

    // ---- notes: authors
    if mand_id >= 41 {
        let authors: Vec<String> = [382, 396].iter().map(|o| decode_author(&r, *o)).filter(|a| !a.is_empty()).collect();
        if !authors.is_empty() {
            s.notes = format!("Authors: {}", authors.join(", "));
        }
    }
    let version = r.f32(424 + shift);
    s.notes = format!("{}\nImported from Mandelbulb3D (parameter version {mand_id}, program {version:.2})", s.notes).trim().to_string();
    // Monte Carlo settings (every file has them; MB3D's defaults: depth 3, diffusity 0, soft
    // shadow radius 1): the bounces for path tracing, the rest as a note when changed.
    // Older parameter versions have other data there.
    let (mc_depth, mc_diff, mc_soft) = (r.u8(186), r.u8(125) as f64 / 100.0, r.short_float(224));
    let mc_valid = mand_id >= 44 && (2..=31).contains(&mc_depth) && mc_diff <= 2.5 && (0.0..=20.0).contains(&mc_soft);
    if mc_valid {
        s.lighting.path_bounces = (mc_depth as u32 - 1).clamp(1, 16);
    }
    if mc_valid && (mc_depth != 3 || mc_diff != 0.0 || (mc_soft - 1.0).abs() > 1e-6) {
        s.notes.push_str(&format!(
            "\nMonte Carlo settings: ambient rays depth {}, reflection diffusity {mc_diff:.2}, soft shadow radius {mc_soft:.2} (path tracing uses the depth as its bounces; Lighting › Global illumination › Sky reach comes close to Mandelbulb3D's short ambient rays)",
            mc_depth.saturating_sub(1)
        ));
    }
    if !warnings.is_empty() {
        s.notes.push_str("\nImport notes:\n- ");
        s.notes.push_str(&warnings.join("\n- "));
    }
    Ok(Import { scene: s, warnings })
}

/// 16 characters of 7 bits, packed LSB first into 14 bytes.
fn decode_author(r: &Rd, off: usize) -> String {
    let mut s = String::new();
    for i in 0..16 {
        let bit = i * 7;
        let v = (r.u32(off + bit / 8) >> (bit % 8)) & 0x7F;
        if v == 0 {
            break;
        }
        s.push(v as u8 as char);
    }
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mb3d/M3Parameter")
    }

    #[test]
    fn imports_sample_parameters() {
        let Ok(rd) = std::fs::read_dir(sample_dir()) else { return };
        let mut ok = 0;
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "m3p") {
                continue;
            }
            let b = std::fs::read(&p).unwrap();
            let imp = import_binary(&b).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let c = &imp.scene.camera;
            assert!((c.forward.length() - 1.0).abs() < 1e-6, "{}", p.display());
            assert!(c.right.dot(c.forward).abs() < 1e-3, "{}", p.display());
            assert!(imp.scene.width > 0 && !imp.scene.fractal.slots.is_empty());
            ok += 1;
        }
        assert!(ok >= 70, "only {ok} files");
    }

    #[test]
    fn text_roundtrip_of_alphabet() {
        // 3 bytes -> 4 chars, first char = lowest 6 bits.
        let (b, _) = decode_text("Mandelbulb3Dv18{\n/...\n}").unwrap();
        assert_eq!(b, vec![1, 0, 0]);
    }
}
