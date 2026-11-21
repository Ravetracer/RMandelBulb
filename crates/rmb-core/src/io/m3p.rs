// SPDX-License-Identifier: LGPL-2.1-or-later

//! Import of Mandelbulb3D parameters: binary `.m3p` files and the clipboard text format
//! (`Mandelbulb3Dv18{…}`).
//!
//! Supported: header versions (MandId) 20 to 45. Formulas are mapped by name to the library
//! (aliases); unknown formulas stay in the hybrid as missing slots so the user can see them.
//! The MB3D camera (rays starting on a pixel grid plane) becomes a pinhole camera at the
//! point where the MB3D rays converge. Lighting and colors are converted approximately.

use crate::gradient::srgb_to_linear;
use crate::math::{V3, v3};
use crate::scene::{AoMode, BackgroundMode, ColorMode, ColorStop, CombineMode, DeCombine, Light, Scene, Slot};

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
    match hybrid_type {
        1 => warnings.push("interpolation hybrid imported as an alternating hybrid".into()),
        3 => warnings.push("(K/L)IFS hybrid mode is not supported".into()),
        _ => {}
    }
    // Raw slots: (iterations, formula number, name, option values).
    let mut raw: Vec<(i32, i32, String, Vec<f64>)> = (0..n_slots)
        .map(|i| {
            let o = addon + 8 + i * SLOT;
            let fnr = r.i32(o + 4);
            let name = if (0..10).contains(&fnr) { INTERNAL[fnr as usize].to_string() } else { r.cstr(o + 12, 32) };
            let n_opt = (r.i32(o + 8).clamp(0, 16)) as usize;
            (r.i32(o), fnr, name, (0..n_opt).map(|k| r.f64(o + 60 + k * 8)).collect())
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
    if r.u8(addon + 2) & 6 != 0 {
        warnings.push("inside rendering is not supported yet".into());
    }
    let end1 = (hyb1 & 15) as usize;
    let repeat1 = (hyb1 >> 4) as usize;
    let lib = crate::formula::library();
    let mut slots = Vec::new();
    let mut missing = Vec::new();
    for (i, (its, fnr, name, values)) in raw.into_iter().enumerate() {
        if its <= 0 || fnr < 0 {
            continue;
        }
        if hybrid_type == 0 && i > end1 && end1 > 0 {
            continue;
        }
        if hybrid_type == 2 && i > end1 && (i < start2 || i > end2) {
            continue;
        }
        let iterations = if hybrid_type == 1 { 1 } else { its.min(1000) as u32 };
        let mut slot = Slot { formula: name.clone(), iterations, params: Default::default() };
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
                    _ => CombineMode::Min,
                };
                if ftype == 6 {
                    warnings.push("the DE-combination mode 'mix' (dIFS) is shown as a union".into());
                }
                let max2 = max_its2;
                f.combine = Some(DeCombine {
                    mode,
                    start,
                    repeat_from: slots.iter().position(|(i, _)| *i >= repeat2.max(start2)).unwrap_or(start).max(start),
                    max_iterations: if max2 > 0 { max2.min(10_000) as u32 } else { f.max_iterations },
                    smooth: comb_smooth.abs().max(1e-6),
                });
            } else {
                warnings.push("DE combination without a second formula chain imported as an alternating hybrid".into());
            }
        }
        f.slots = slots.into_iter().map(|(_, s)| s).collect();
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

    // ---- lighting (approximate)
    let tb = |k: usize| r.i32(light + 8 + 4 * (k - 3));
    let diffuse_mul = (tb(5) as f64 * 0.02).clamp(0.0, 5.0);
    let spec_mul = ((tb(7) & 0xFFF) as f64 * 0.02).max(0.004);
    let hs_flags = r.u8(133);
    let mut lights = Vec::new();
    let mut shininess = 0.0;
    for i in 0..6 {
        let lo = light + 68 + i * 32;
        let opt = r.u8(lo);
        if opt & 1 != 0 || opt & 2 != 0 {
            continue;
        }
        let func = r.u8(lo + 1);
        let lamp = r.short_float(lo + 2);
        let color = r.rgb(lo + 4);
        let dir_cam = if opt & 4 != 0 {
            // Positional light: shown as a directional light from its position.
            let pos = v3(r.d7(lo + 9), r.d7(lo + 17), r.d7(lo + 25));
            let d = (pos - mid).normalize();
            v3(d.dot(r0), d.dot(r1), d.dot(r2))
        } else {
            let (lx, ly) = (r.d7(lo + 9), r.d7(lo + 17));
            -v3(lx.sin(), ly.sin(), ly.cos() * lx.cos()).normalize()
        };
        let world = opt & 32 != 0 || opt & 4 != 0;
        let mut l = Light {
            color,
            intensity: (lamp * diffuse_mul).clamp(0.0, 10.0),
            specular: (spec_mul * 0.5).min(2.0),
            shadows: opt & 64 == 0 && hs_flags & (4 << i) != 0,
            softness: 0.02,
            camera_relative: !world,
            ..Light::default()
        };
        if world {
            let w = (r0 * dir_cam.x + r1 * dir_cam.y + r2 * dir_cam.z).normalize();
            l.azimuth = w.y.atan2(w.x).to_degrees();
            l.elevation = w.z.clamp(-1.0, 1.0).asin().to_degrees();
        } else {
            // Camera space: x right, y down, z forward.
            l.elevation = (-dir_cam.y).clamp(-1.0, 1.0).asin().to_degrees();
            l.azimuth = dir_cam.x.atan2(-dir_cam.z).to_degrees();
        }
        shininess += (2u32 << (func & 7)) as f64;
        lights.push(l);
    }
    if !lights.is_empty() {
        s.lighting.shininess = (shininess / lights.len() as f64).clamp(2.0, 256.0);
        s.lighting.lights = lights;
    }
    s.lighting.sky_color = r.rgb(light + 52);
    s.lighting.ground_color = r.rgb(light + 56);
    s.lighting.ambient = ((tb(8) & 0xFFF) as f64 / 90.0).clamp(0.0, 3.0);
    let ao_flags = r.u8(149);
    s.lighting.ao_mode = if ao_flags & 1 != 0 { AoMode::Raytraced } else { AoMode::Off };
    s.lighting.ao_strength = ((tb(11) & 0xFF) as f64 / 53.0).clamp(0.0, 1.0);
    s.material.specular = 1.0;

    // ---- surface colors: 10 stops, wrapping from the last to the first
    let mut stops: Vec<ColorStop> = (0..10)
        .map(|k| {
            let o = light + 260 + k * 10;
            ColorStop { pos: r.u16(o) as f64 / 32767.0, color: r.rgb(o + 2) }
        })
        .collect();
    stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    if let Some(first) = stops.first().cloned() {
        stops.push(ColorStop { pos: 1.0, color: first.color });
    }
    s.material.gradient = stops;
    s.material.color_mode = if r.u32(light + 44) & (1 << 17) != 0 { ColorMode::OrbitTrap } else { ColorMode::Iterations };
    s.material.color_scale = 1.0 / s.fractal.max_iterations.max(1) as f64;
    s.material.color_offset = ((tb(9) + 30) as f64 / 90.0).clamp(0.0, 1.0) * 0.25;
    s.material.repeat = true;

    // ---- background and fog: MB3D shows the depth fog colors behind the object
    let depth_col = r.rgb(light + 60);
    let depth_col2 = r.rgb(light + 64);
    s.background.mode = BackgroundMode::Gradient;
    s.background.top = depth_col;
    s.background.bottom = depth_col2;
    s.background.horizon = [0, 1, 2].map(|k| (depth_col[k] + depth_col2[k]) * 0.5);
    s.fog.color = depth_col;
    // MB3D's depth fog grows to 28000 * sDepth at the far end of the ray.
    let far_fog = (tb(4).max(0) as f64 * 0.8e-6 * 28000.0).min(0.98);
    s.fog.density = -(1.0 - far_fog).ln() / s.march.max_distance;
    s.fog.glow = ((tb(6) - 53) as f64 / 60.0).clamp(0.0, 4.0);
    s.fog.glow_color = [srgb_to_linear(r.u8(light + 55)), srgb_to_linear(r.u8(light + 59)), srgb_to_linear(r.u8(light + 63))];
    if r.u8(181) != 0 {
        warnings.push("depth of field settings are not converted; enable DOF in the Effects tab".into());
    }
    if !r.cstr(light + 384, 24).is_empty() {
        warnings.push(format!("background image '{}' is not supported yet", r.cstr(light + 384, 24)));
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
