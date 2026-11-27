// SPDX-License-Identifier: LGPL-2.1-or-later

//! Generates the WGSL of a hybrid formula chain (the fractal part of march.wgsl).
//!
//! The shader depends only on the formulas of the slots and the DE variant of each chain;
//! iteration counts, repeat slots, combine settings and all parameter values come from buffers.
//! A DE combination gets a second `iterate`/`chain_de` pair; `fractal_de` combines them.

use std::fmt::Write;

use rmb_core::formula::DeKind;
use rmb_core::formula::lang::{STATE_AUX, STATE_COUNT, STATE_DIST, STATE_JULIA, STATE_PX, STATE_ROLD, STATE_ROUT};
use rmb_core::formula::wgsl::{PRELUDE, emit_iter, emit_iter_counts, emit_iter_dd, state_name};
use rmb_core::fractal::Program;

const TEMPLATE: &str = include_str!("march.wgsl");

/// The double-float library of the deep zoom shader (`dd` = hi + lo of two f32).
pub const DF64: &str = include_str!("df64.wgsl");

/// The complete ray marching shader for `prog`.
pub fn march_shader(prog: &Program) -> String {
    march_shader_baked(prog, &[])
}

/// [`march_shader`] with formula parameters as constants: `baked[i]` is the value of
/// `prog.constants[i]` to write into the shader instead of reading it from `prm` (`None` or
/// beyond the slice: read it). Constants let the compiler fold the formula code: final renders
/// run 5-35 % faster, for a compile per render.
pub fn march_shader_baked(prog: &Program, baked: &[Option<f64>]) -> String {
    let mut s = String::from(PRELUDE);
    s.push_str(DF64);
    let _ = writeln!(s, "const ROTATE_4D: bool = {};", prog.rotation_4d.is_some());
    let _ = writeln!(s, "const REFLECT: bool = {};", prog.reflections);
    let _ = writeln!(s, "const MAIN_T: bool = {};", prog.main_transform.is_some());
    let _ = writeln!(s, "const LAYERS: bool = {};", prog.layers);
    let _ = writeln!(s, "const BUMP: bool = {};", prog.bump);
    let _ = writeln!(s, "const GLASS_SHADOW: bool = {};", prog.glass_shadows);
    // Water surfaces reflect by themselves (Shader::water_at).
    let water: Vec<String> = prog.objects.iter().enumerate().filter(|(_, o)| matches!(o.shape, rmb_core::fractal::ObjectShape::Primitive(rmb_core::fractal::WATER_KIND, _))).map(|(j, _)| format!("j == {j}u")).collect();
    let _ = writeln!(s, "const WATER: bool = {};", !water.is_empty());
    let _ = writeln!(s, "fn is_water(j: u32) -> bool {{\n    return {};\n}}", if water.is_empty() { "false".to_string() } else { water.join(" || ") });
    // The water medium (Program::medium): see-through water the camera and rays can enter.
    let _ = writeln!(s, "const MEDIUM: bool = {};\nconst MEDIUM_OBJ: u32 = {}u;", prog.medium.is_some(), prog.medium.map_or(0, |(j, _)| j));
    let _ = writeln!(s, "const COLOR_ON_IT: bool = {};", prog.color_on_it >= 0 && prog.part2.is_none() && !prog.slots.is_empty());
    let _ = writeln!(s, "const MODELS: bool = {};", prog.objects.iter().any(|o| matches!(o.shape, rmb_core::fractal::ObjectShape::Model(_))));
    let _ = writeln!(s, "const PLANTS: bool = {};", prog.objects.iter().any(|o| o.plants.is_some()));
    let _ = writeln!(s, "const COVER: bool = {};", !prog.covers.is_empty());
    // Deep zoom: the main fractal in double-float arithmetic (composed objects stay f32, they
    // are evaluated in local coordinates).
    let main_chain = if prog.deep.is_some() { chain_dd } else { chain };
    let extras = prog.slot_extras();
    s.push_str(&main_chain(prog, "1", &Uni { extras, ..Uni::main(0, "u.frac.z", "u.frac.w", "u.scales.x") }));
    match prog.part2.as_deref() {
        Some(p2) => {
            s.push_str(&main_chain(p2, "2", &Uni { extras, ..Uni::main(prog.slots.len(), "u.comb.y", "u.comb.z", "u.scales.y") }));
            if prog.is_mix() {
                // Mix (mirrors fractal.rs Evaluator::orbit): the dIFS from where chain 1 ends.
                let _ = writeln!(s, "const MIX_LINEAR: bool = {};", prog.de_kind == DeKind::Linear && !prog.difs);
                s.push_str(
                    "\nfn fractal_de(p: vec3<f32>, h: f32) -> Orbit {\n    dd_target = h;\n    let a = iterate_1(p, 0u, false, vec4<f32>(0.0), false);\n    mix_start = a.sd;\n    let b = iterate_2(p, 0u, false, a.s, true);\n    let d = max(b.de, 0.0) * u.scales.y;\n    var o: Orbit;\n    if (MIX_LINEAR) {\n        o.de = d / max(abs(a.dr), 1.0e-30);\n    } else {\n        o.de = d * pow(max(abs(u.interior.w), 1.0e-30), -1.0 - f32(a.n));\n    }\n    o.n = b.n;\n    o.r2 = dot(a.s, a.s);\n    o.r2_old = a.rold;\n    o.trap = b.trap;\n    o.c2 = b.c2;\n    o.escaped = true;\n    return o;\n}\n",
                );
            } else {
                s.push_str(
                    "\nfn fractal_de(p: vec3<f32>, h: f32) -> Orbit {\n    let a = chain_de_1(p, h);\n    let b = chain_de_2(p, h);\n    return combine_de(a, b, u32(u.comb.x), u.comb.w);\n}\n",
                );
            }
        }
        None => s.push_str("\nfn fractal_de(p: vec3<f32>, h: f32) -> Orbit {\n    return chain_de_1(p, h);\n}\n"),
    }
    s.push_str(&objects(prog));
    if baked.iter().any(Option::is_some) {
        s = bake_constants(&s, baked);
    }
    TEMPLATE.replace("//@ITERATE@", &s)
}

/// Index of the first low part in `prm` (renderer.rs `PRM_LO`).
const PRM_LO: usize = 4096;

/// Replaces the parameter reads `prm[a][b]` (index 4 a + b; from `PRM_LO` on the low parts of
/// the double-float values) of `src` by the values in `baked`.
fn bake_constants(src: &str, baked: &[Option<f64>]) -> String {
    let part = |k: usize| -> Option<f32> {
        let v = if k < PRM_LO { baked.get(k).copied().flatten()? } else { baked.get(k - PRM_LO).copied().flatten()? };
        let hi = v as f32;
        let x = if k < PRM_LO { hi } else { (v - hi as f64) as f32 };
        x.is_finite().then_some(x)
    };
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find("prm[") {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let read = || -> Option<(usize, usize)> {
            let (a, r) = rest[4..].split_once("u][")?;
            let (b, _) = r.split_once("u]")?;
            let len = 4 + a.len() + 3 + b.len() + 2;
            Some((a.parse::<usize>().ok()? * 4 + b.parse::<usize>().ok()?, len))
        };
        match read() {
            Some((k, len)) if part(k).is_some() => {
                let _ = write!(out, "({:?})", part(k).unwrap_or_default());
                rest = &rest[len..];
            }
            _ => {
                out.push_str("prm[");
                rest = &rest[4..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A field of march.wgsl's uniform struct `U`: offset in 4-byte words, u32 (else f32), array
/// length (`None`: a single `vec4`). Arrays of structs (the lights) are left out.
struct UField {
    name: String,
    word: usize,
    uint: bool,
    array: Option<usize>,
}

/// The layout of `struct U` in march.wgsl (every member is a `vec4` or an array of 16-byte
/// aligned items, so the offsets are the plain sums of the sizes).
fn uniform_layout() -> (Vec<UField>, usize) {
    let body = TEMPLATE.split_once("struct U {").and_then(|(_, r)| r.split_once("\n}")).map(|(b, _)| b).unwrap_or("");
    let light_words = TEMPLATE.split_once("struct GLight {").and_then(|(_, r)| r.split_once("\n}")).map_or(0, |(b, _)| b.matches("vec4<").count() * 4);
    let mut fields = Vec::new();
    let mut word = 0;
    for line in body.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        let Some((name, ty)) = line.split_once(':') else { continue };
        let ty = ty.trim().trim_end_matches(',');
        let (item, count) = match ty.strip_prefix("array<") {
            Some(a) => {
                let (item, n) = a.trim_end_matches('>').rsplit_once(',').unwrap_or((a, "1"));
                (item.trim(), n.trim().parse::<usize>().ok())
            }
            None => (ty, None),
        };
        let words = if item == "GLight" { light_words } else { 4 };
        if item != "GLight" {
            fields.push(UField { name: name.trim().to_string(), word, uint: item.contains("u32"), array: count });
        }
        word += words * count.unwrap_or(1);
    }
    (fields, word)
}

/// Words of `U` that change with every band or pass and are never baked: the sample index and
/// the band rows; also `deep_hi.w`, the `dd_one` that must stay a run-time value (df64.wgsl).
pub fn uniform_words_never_baked() -> Vec<usize> {
    let (fields, _) = uniform_layout();
    let at = |n: &str| fields.iter().find(|f| f.name == n).map_or(usize::MAX, |f| f.word);
    // Tile offsets change between the tiles of one render, which share the shader.
    vec![at("size") + 2, at("band"), at("band") + 1, at("deep_hi") + 3, at("tile"), at("tile") + 1]
}

/// Replaces the reads of uniform fields with constant indexes (`u.fog.x`, `u.glow2.xyz`,
/// `u.bg_rot[1].w`, `u.julia`) by the values in `words` (the bytes of `U` as 4-byte words) where
/// `bake[word]` is true (beyond the slice: true) for all words read. Feature switches, counts
/// and settings then become constants the compiler folds; reads with run-time indexes stay.
pub fn bake_uniforms(src: &str, words: &[u32], bake: &[bool]) -> String {
    let (fields, total) = uniform_layout();
    let never = uniform_words_never_baked();
    let ok = |w: usize| w < total && w < words.len() && !never.contains(&w) && bake.get(w).copied().unwrap_or(true);
    let lit = |w: usize, uint: bool| -> Option<String> {
        if uint {
            return Some(format!("{}u", words[w]));
        }
        let x = f32::from_bits(words[w]);
        x.is_finite().then(|| format!("{x:?}"))
    };
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let bytes = src.as_bytes();
    while i < src.len() {
        let start_ok = src[i..].starts_with("u.") && (i == 0 || !(ident(bytes[i - 1] as char) || bytes[i - 1] == b'.'));
        if start_ok && let Some((text, len)) = bake_read(&src[i..], &fields, &ok, &lit, ident) {
            out.push_str(&text);
            i += len;
            continue;
        }
        let ch = src[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// One uniform read at the start of `s` (`u.` …): the literal and the length it replaces.
fn bake_read(s: &str, fields: &[UField], ok: &impl Fn(usize) -> bool, lit: &impl Fn(usize, bool) -> Option<String>, ident: impl Fn(char) -> bool) -> Option<(String, usize)> {
    let rest = &s[2..];
    let name_len = rest.find(|c: char| !ident(c)).unwrap_or(rest.len());
    let f = fields.iter().find(|f| f.name == rest[..name_len])?;
    let mut len = 2 + name_len;
    let mut base = f.word;
    if let Some(n) = f.array {
        // Only constant indexes: `[2]` or `[2u]`.
        let r = s[len..].strip_prefix('[')?;
        let (idx, _) = r.split_once(']')?;
        let k: usize = idx.trim_end_matches('u').parse().ok()?;
        if k >= n {
            return None;
        }
        base += 4 * k;
        len += idx.len() + 2;
    }
    // Swizzle (none: the whole vec4).
    let mut comps: Vec<usize> = vec![0, 1, 2, 3];
    if let Some(r) = s[len..].strip_prefix('.') {
        let n = r.find(|c: char| !ident(c)).unwrap_or(r.len());
        let sw = &r[..n];
        if sw.is_empty() || sw.len() > 4 || !sw.chars().all(|c| "xyzw".contains(c)) {
            return None;
        }
        comps = sw.chars().map(|c| "xyzw".find(c).unwrap_or(0)).collect();
        len += 1 + n;
    }
    // The next character must not continue an index or a member access.
    if s[len..].starts_with('[') {
        return None;
    }
    let vals: Option<Vec<String>> = comps.iter().map(|&c| ok(base + c).then(|| lit(base + c, f.uint)).flatten()).collect();
    let vals = vals?;
    let text = if vals.len() == 1 {
        format!("({})", vals[0])
    } else {
        format!("vec{}<{}>({})", vals.len(), if f.uint { "u32" } else { "f32" }, vals.join(", "))
    };
    Some((text, len))
}

/// Mandelbulber per-slot settings in a hybrid loop (mirrors fractal.rs iterate_from): skips
/// slots outside their iteration range (all of them in a row ends the orbit) and reads the
/// slot's flags and weight. Empty without extras.
fn extras_skip(cfg: &Uni, k: &str) -> String {
    if !cfg.extras {
        return String::new();
    }
    let b = cfg.slot_base;
    format!(
        "        let slot_ix = slot + {b}u;\n        if (n < u.slots[slot_ix].y || n > u.slots[slot_ix].z) {{\n            count = 0u;\n            slot += 1u;\n            if (slot >= NSLOTS_{k}) {{\n                slot = repeat_from;\n            }}\n            skipped += 1u;\n            if (skipped > NSLOTS_{k}) {{\n                break;\n            }}\n            continue;\n        }}\n        skipped = 0u;\n        let slot_flags = u.slots[slot_ix].w;\n        let slot_wt = u.slotw[slot_ix / 4u][slot_ix % 4u];\n"
    )
}

/// Before the formula: no constant for slots with flag 1 (`zero`: the zero value).
fn extras_pre(cfg: &Uni, n: &[String], zero: &str) -> String {
    if !cfg.extras {
        return String::new();
    }
    let (c0, c1, c2, c3) = (&n[4], &n[5], &n[6], &n[7]);
    format!("        let keep_c0 = {c0};\n        let keep_c1 = {c1};\n        let keep_c2 = {c2};\n        let keep_c3 = {c3};\n        if ((slot_flags & 1u) != 0u) {{\n            {c0} = {zero};\n            {c1} = {zero};\n            {c2} = {zero};\n            {c3} = {zero};\n        }}\n")
}

/// After the formula: the constant back, the weight blend with the point before it (`prev`:
/// the previous point and derivative per component; `blend`: a, b, w -> a + (b - a) w).
fn extras_post(cfg: &Uni, n: &[String], prev: [&str; 5], blend: impl Fn(&str, &str) -> String) -> String {
    if !cfg.extras {
        return String::new();
    }
    let (c0, c1, c2, c3) = (&n[4], &n[5], &n[6], &n[7]);
    let mut s = format!("        if ((slot_flags & 1u) != 0u) {{\n            {c0} = keep_c0;\n            {c1} = keep_c1;\n            {c2} = keep_c2;\n            {c3} = keep_c3;\n        }}\n        if (slot_wt != 1.0) {{\n");
    for (name, p) in [&n[0], &n[1], &n[2], &n[3], &n[8]].into_iter().zip(prev) {
        let _ = writeln!(s, "            {name} = {};", blend(p, name));
    }
    s.push_str("        }\n");
    s
}

/// The extra escape condition: the slot tests (no flag 2) and the minimum iterations passed.
fn extras_bail(cfg: &Uni) -> &'static str {
    if cfg.extras { " && (slot_flags & 2u) == 0u && n >= u32(u.cfac.w)" } else { "" }
}

/// `chain` / `chain_dd` start: the constant factor (main fractal with extras).
fn extras_init(cfg: &Uni, n: &[String], mul: impl Fn(&str, &str) -> String) -> String {
    if !cfg.extras {
        return String::new();
    }
    let mut s = String::new();
    for (c, f) in [(&n[4], "u.cfac.x"), (&n[5], "u.cfac.y"), (&n[6], "u.cfac.z")] {
        let _ = writeln!(s, "    {c} = {};", mul(c, f));
    }
    s.push_str("    var skipped = 0u;\n");
    s
}

/// The composed objects (mirrors fractal.rs Evaluator::orbit): a chain per fractal object and
/// `compose_objects`, which folds them into the estimate in order.
fn objects(prog: &Program) -> String {
    use rmb_core::fractal::ObjectShape;
    let mut s = String::new();
    for (j, o) in prog.objects.iter().enumerate() {
        if let ObjectShape::Fractal(f) = &o.shape {
            s.push_str(&chain(f, &format!("o{j}"), &Uni::object(j)));
            // The DE combination of the object (mirrors fractal.rs fractal_orbit).
            match f.part2.as_deref() {
                Some(p2) => {
                    s.push_str(&chain(p2, &format!("o{j}b"), &Uni::object2(j, f.slots.len())));
                    if f.is_mix() {
                        let linear = f.de_kind == DeKind::Linear && !f.difs;
                        let _ = writeln!(
                            s,
                            "\nfn fractal_de_o{j}(p: vec3<f32>, h: f32) -> Orbit {{\n    let ob = objs[{j}];\n    let a = iterate_o{j}(p, 0u, false, vec4<f32>(0.0), false);\n    let b = iterate_o{j}b(p, 0u, false, a.s, true);\n    let d = max(b.de, 0.0) * ob.comb2.x;\n    var o: Orbit;\n    if ({linear}) {{\n        o.de = d / max(abs(a.dr), 1.0e-30);\n    }} else {{\n        o.de = d * pow(max(abs(ob.comb2.y), 1.0e-30), -1.0 - f32(a.n));\n    }}\n    o.n = b.n;\n    o.r2 = dot(a.s, a.s);\n    o.r2_old = a.rold;\n    o.trap = b.trap;\n    o.c2 = b.c2;\n    o.escaped = true;\n    return o;\n}}"
                        );
                    } else {
                        let _ = writeln!(s, "\nfn fractal_de_o{j}(p: vec3<f32>, h: f32) -> Orbit {{\n    return combine_de(chain_de_o{j}(p, h), chain_de_o{j}b(p, h), u32(objs[{j}].comb.x), objs[{j}].comb.w);\n}}");
                    }
                }
                None => {
                    let _ = writeln!(s, "\nfn fractal_de_o{j}(p: vec3<f32>, h: f32) -> Orbit {{\n    return chain_de_o{j}(p, h);\n}}");
                }
            }
        }
    }
    s.push_str("\nfn compose_objects(p: vec3<f32>, h: f32, o_in: Orbit) -> Orbit {\n    var o = o_in;\n");
    // Water last: it sees the distance to the rest of the scene (mirrors Evaluator::orbit).
    let is_water = |o: &rmb_core::fractal::ObjectProgram| matches!(o.shape, ObjectShape::Primitive(rmb_core::fractal::WATER_KIND, _));
    let order: Vec<usize> = (0..prog.objects.len()).filter(|&j| !is_water(&prog.objects[j])).chain((0..prog.objects.len()).filter(|&j| is_water(&prog.objects[j]))).collect();
    let mut dry = false;
    // Baked plants after all objects but the water (one call site).
    let cover = "    if (COVER) {\n        o = cover_union(p, o);\n    }\n";
    for j in order {
        let o = &prog.objects[j];
        if is_water(o) && !dry {
            s.push_str(cover);
            s.push_str("    let dry = o.de;\n");
            dry = true;
        }
        let _ = writeln!(s, "    {{\n        let ob = objs[{j}];\n        let q = obj_local({j}u, p);\n        let sc = ob.inv0.w;\n        let op = u32(ob.inv1.w);\n        var b: Orbit;");
        match &o.shape {
            ObjectShape::Primitive(rmb_core::fractal::WATER_KIND, _) if prog.medium.is_some_and(|(m, _)| m == j) => {
                // The medium (mirrors Evaluator::orbit): seen from inside the air above is the
                // solid; shadow and occlusion rays pass through it.
                let _ = writeln!(
                    s,
                    "        if (water_mode == 2u) {{\n            b.de = 3.0e38;\n        }} else {{\n            b.shore = dry / sc;\n            if (water_mode == 1u) {{\n                b.de = -water_body_distance(ob.prim, ob.prim2, ob.prim3, q, b.shore) * sc;\n            }} else {{\n                b.de = water_distance(ob.prim, ob.prim2, ob.prim3, q, b.shore) * sc;\n            }}\n        }}\n        b.escaped = true;"
                );
            }
            ObjectShape::Primitive(rmb_core::fractal::WATER_KIND, _) => {
                let _ = writeln!(s, "        b.shore = dry / sc;\n        b.de = water_distance(ob.prim, ob.prim2, ob.prim3, q, b.shore) * sc;\n        b.escaped = true;");
            }
            ObjectShape::Fractal(_) => {
                let _ = writeln!(
                    s,
                    "        b = fractal_de_o{j}(q, h / sc);\n        b.de = max(b.de * ob.misc.z, 0.0) * sc;\n        // A fractal has no distance inside; cutting with it needs one.\n        if (b.de <= 0.0 && (op == 1u || op == 4u)) {{\n            b.de = -2.0 * h;\n        }}"
                );
            }
            ObjectShape::Primitive(kind, _) if o.plants.is_some() => {
                let _ = writeln!(s, "        b.de = prim_de({kind}u, ob.prim, ob.prim2, ob.prim3, q);\n        b.de = plant_de({j}u, ob.prim, ob.prim2, q, ob.prim3.w, b.de, plant_lod(p, sc)).x * sc;\n        b.escaped = true;");
            }
            ObjectShape::Primitive(kind, _) => {
                let _ = writeln!(s, "        b.de = prim_de({kind}u, ob.prim, ob.prim2, ob.prim3, q) * sc;\n        b.escaped = true;");
            }
            ObjectShape::Model(_) => {
                let _ = writeln!(s, "        b.de = model_de({j}u, q) * sc;\n        b.escaped = true;");
            }
        }
        let _ = writeln!(s, "        let c = combine_op(op, o.de, b.de, ob.inv2.w);\n        if (c.y > 0.5) {{\n            o = b;\n            o.obj = {}u;\n        }}\n        o.de = c.x;\n    }}", j + 1);
    }
    if !dry {
        s.push_str(cover);
    }
    s.push_str("    return o;\n}\n");
    s
}

/// Where a chain reads its settings: WGSL expressions (uniforms for the main fractal, the
/// object buffer for composed objects). `slot_iter` is the iteration count of the slot whose
/// index replaces `SLOT` (see [`Uni::slot_iter`]).
struct Uni {
    wslice: String,
    julia_on: String,
    julia: String,
    limit: String,
    repeat: String,
    bail2: String,
    slot_iter: String,
    scale: String,
    rot4: &'static str,
    /// Interpolation hybrid weights (first chain only).
    blend: Option<(String, String)>,
    /// Mandelbulber per-slot settings in the shader (main fractal: `u.slots`, `u.slotw`,
    /// `u.cfac`); the chain's first slot in those tables.
    extras: bool,
    slot_base: usize,
}

impl Uni {
    /// A chain of the main fractal whose slots start at `slot_base` in the uniform slot table.
    fn main(slot_base: usize, limit: &str, repeat: &str, scale: &str) -> Uni {
        Uni {
            wslice: "u.frac.x".into(),
            julia_on: "u.frac.y > 0.5".into(),
            julia: "u.julia".into(),
            limit: limit.into(),
            repeat: repeat.into(),
            bail2: "u.march.z".into(),
            slot_iter: format!("u.slots[SLOT + {slot_base}u].x"),
            scale: scale.into(),
            rot4: "ROTATE_4D",
            blend: (slot_base == 0).then(|| ("u.blend.x".into(), "u.blend.y".into())),
            extras: false,
            slot_base,
        }
    }

    /// The fractal of composed object `j` (settings in `objs[j]`).
    fn object(j: usize) -> Uni {
        Uni {
            wslice: format!("objs[{j}].frac.x"),
            julia_on: format!("objs[{j}].frac.y > 0.5"),
            julia: format!("objs[{j}].julia"),
            limit: format!("objs[{j}].frac.z"),
            repeat: format!("objs[{j}].frac.w"),
            bail2: format!("objs[{j}].misc.x"),
            slot_iter: format!("objs[{j}].slots[SLOT / 4u][SLOT % 4u]"),
            scale: format!("objs[{j}].misc.y"),
            rot4: "false",
            blend: Some((format!("objs[{j}].comb2.z"), format!("objs[{j}].comb2.w"))),
            extras: false,
            slot_base: 0,
        }
    }

    /// The second chain of object `j`'s DE combination (its slots after the first chain's
    /// `slot_base`).
    fn object2(j: usize, slot_base: usize) -> Uni {
        Uni {
            limit: format!("objs[{j}].comb.y"),
            repeat: format!("objs[{j}].comb.z"),
            slot_iter: format!("objs[{j}].slots[(SLOT + {slot_base}u) / 4u][(SLOT + {slot_base}u) % 4u]"),
            scale: format!("objs[{j}].comb2.x"),
            blend: None,
            ..Uni::object(j)
        }
    }

    /// The iteration count of slot `slot` (a WGSL u32 expression).
    fn slot_iter(&self, slot: &str) -> String {
        self.slot_iter.replace("SLOT", slot)
    }
}

/// Formulas that set slot iteration counts (`setcount`): the chain keeps the counts of its
/// slots in `cnt` (from the slot table at the start), `run` is the current slot's run length
/// (fixed when the slot is entered). Empty for other chains.
fn counts_init(prog: &Program, cfg: &Uni) -> String {
    if !prog.sets_counts() {
        return String::new();
    }
    let mut s = format!("    var cnt: array<u32, {}>;\n", prog.slots.len());
    for i in 0..prog.slots.len() {
        let _ = writeln!(s, "    cnt[{i}] = u32({});", cfg.slot_iter(&format!("{i}u")));
    }
    s.push_str("    var run = 0u;\n    var zero_skipped = 0u;\n");
    s
}

/// In the hybrid loop of a chain with `counts_init`: takes the run length when a slot is
/// entered and skips slots whose count is 0 (all of them in a row ends the orbit; mirrors
/// fractal.rs iterate_from).
fn counts_skip(prog: &Program, k: &str) -> String {
    if !prog.sets_counts() {
        return String::new();
    }
    format!(
        "        if (count == 0u) {{\n            run = cnt[slot];\n        }}\n        if (run == 0u) {{\n            slot += 1u;\n            if (slot >= NSLOTS_{k}) {{\n                slot = repeat_from;\n            }}\n            zero_skipped += 1u;\n            if (zero_skipped > NSLOTS_{k}) {{\n                break;\n            }}\n            continue;\n        }}\n        zero_skipped = 0u;\n"
    )
}

/// The run length the hybrid loop compares the slot's count with.
fn run_length(prog: &Program, cfg: &Uni) -> String {
    if prog.sets_counts() { "run".into() } else { cfg.slot_iter("slot") }
}

/// One slot's formula in a hybrid loop (`dd`: double-float).
fn slot_iter_code(prog: &Program, i: usize, dd: bool) -> String {
    let sl = &prog.slots[i];
    if prog.sets_counts() {
        emit_iter_counts(&sl.formula.def, sl.offset, 4, dd, prog.first_slot, prog.slots.len())
    } else if dd {
        emit_iter_dd(&sl.formula.def, sl.offset, 4)
    } else {
        emit_iter(&sl.formula.def, sl.offset, 4)
    }
}

/// `iterate_<k>` and `chain_de_<k>` of one chain.
fn chain(prog: &Program, k: &str, cfg: &Uni) -> String {
    let (limit, repeat) = (cfg.limit.as_str(), cfg.repeat.as_str());
    let mut s = String::new();
    let _ = writeln!(s, "\nconst NSLOTS_{k}: u32 = {}u;", prog.slots.len());
    let _ = writeln!(s, "const NUMERIC_{k}: bool = {};", prog.numeric);
    let _ = writeln!(s, "const DE_LINEAR_{k}: bool = {};", prog.de_kind == DeKind::Linear);
    let _ = writeln!(s, "const DE_MAXAXIS_{k}: bool = {};", prog.de_kind == DeKind::MaxAxis);
    let _ = writeln!(s, "const DE_CUSTOM_{k}: bool = {};", prog.de_kind == DeKind::Custom);
    let _ = writeln!(s, "const DIFS_{k}: bool = {};", prog.difs);
    let _ = writeln!(s, "const DEEP_{k}: bool = false;");
    let _ = writeln!(s, "\nfn iterate_{k}(p: vec3<f32>, fixed_n: u32, use_fixed: bool, start: vec4<f32>, use_start: bool) -> Iterated {{");
    s.push_str("    var out: Iterated;\n    out.trap = 3.0e38;\n    out.escaped = false;\n");
    let names: Vec<String> = (0..STATE_COUNT).map(state_name).collect();
    let _ = writeln!(
        s,
        "    var v = vec4<fl>(fl(p.x), fl(p.y), fl(p.z), fl({}));\n    if ({}) {{\n        let q = vec4<fl>(fl(p.x), fl(p.y), fl(p.z), 0.0);\n        v = vec4<fl>(dot(vec4<fl>(u.rot4[0]), q), dot(vec4<fl>(u.rot4[1]), q), dot(vec4<fl>(u.rot4[2]), q), dot(vec4<fl>(u.rot4[3]), q));\n    }}",
        cfg.wslice, cfg.rot4
    );
    let _ = writeln!(s, "    var {}: fl = v.x;\n    var {}: fl = v.y;\n    var {}: fl = v.z;\n    var {}: fl = v.w;", names[0], names[1], names[2], names[3]);
    let _ = writeln!(s, "    var {}: fl = v.x;\n    var {}: fl = v.y;\n    var {}: fl = v.z;\n    var {}: fl = v.w;", names[4], names[5], names[6], names[7]);
    let (jo, j) = (&cfg.julia_on, &cfg.julia);
    let _ = writeln!(s, "    if ({jo}) {{\n        {} = {j}.x;\n        {} = {j}.y;\n        {} = {j}.z;\n        {} = {j}.w;\n    }}", names[4], names[5], names[6], names[7]);
    s.push_str(&extras_init(cfg, &names, |c, f| format!("{c} * fl({f})")));
    let _ = writeln!(s, "    var {}: fl = 1.0;\n    var {}: fl = 0.0;", names[8], names[9]);
    let _ = writeln!(s, "    var {}: fl = dot(v, v);\n    var {}: fl = 0.0;", names[STATE_ROUT], names[STATE_ROLD]);
    // A mix continues from the first chain's point; the constant stays the original one.
    let _ = writeln!(
        s,
        "    if (use_start) {{\n        {} = fl(start.x);\n        {} = fl(start.y);\n        {} = fl(start.z);\n        {} = fl(start.w);\n        {} = fl(dot(start, start));\n    }}",
        names[0], names[1], names[2], names[3], names[STATE_ROUT]
    );
    let _ = writeln!(s, "    let {} = fl(p.x);\n    let {} = fl(p.y);\n    let {} = fl(p.z);", names[STATE_PX], names[STATE_PX + 1], names[STATE_PX + 2]);
    for name in &names[STATE_AUX..STATE_JULIA] {
        let _ = writeln!(s, "    var {name}: fl = 0.0;");
    }
    let _ = writeln!(s, "    let {}: fl = select(0.0, 1.0, {jo});", names[STATE_JULIA]);
    // dIFS: VaryScale starts at 1.
    let _ = writeln!(s, "    if (DIFS_{k}) {{\n        {} = 1.0;\n    }}\n    out.de = 65535.0;", names[STATE_AUX]);
    let _ = writeln!(s, "    if (NSLOTS_{k} == 0u) {{\n        return out;\n    }}");
    let _ = writeln!(s, "    let limit = select(u32({limit}), fixed_n, use_fixed);\n    let repeat_from = u32({repeat});");
    s.push_str("    var n = 0u;\n");
    // Interpolation hybrid (mirrors fractal.rs): both formulas from the same point, blended.
    let blend = cfg.blend.as_ref().filter(|_| prog.blend.is_some() && prog.slots.len() >= 2);
    if let Some((bw1, bw2)) = blend {
        let (x, y, z, w, dr, rout, rold, it) = (&names[0], &names[1], &names[2], &names[3], &names[8], &names[STATE_ROUT], &names[STATE_ROLD], &names[9]);
        let bail2 = &cfg.bail2;
        let _ = writeln!(s, "    loop {{\n        if (n >= limit) {{\n            break;\n        }}\n        {rold} = {rout};\n        {it} = fl(n);");
        let _ = writeln!(s, "        let in_s = vec4<fl>({x}, {y}, {z}, {w});\n        let in_dr = {dr};\n        {{ // {}", prog.slots[0].formula.def.name);
        s.push_str(&emit_iter(&prog.slots[0].formula.def, prog.slots[0].offset, 3));
        let _ = writeln!(s, "        }}\n        let a_s = vec4<fl>({x}, {y}, {z}, {w});\n        let a_dr = {dr};\n        {x} = in_s.x;\n        {y} = in_s.y;\n        {z} = in_s.z;\n        {w} = in_s.w;\n        {dr} = in_dr;\n        {{ // {}", prog.slots[1].formula.def.name);
        s.push_str(&emit_iter(&prog.slots[1].formula.def, prog.slots[1].offset, 3));
        let _ = writeln!(
            s,
            "        }}\n        let w1 = fl({bw1});\n        let w2 = fl({bw2});\n        let len_b = sqrt({x} * {x} + {y} * {y} + {z} * {z});\n        let l = length(a_s.xyz) * w1 + len_b * w2;\n        {x} = a_s.x * w1 + {x} * w2;\n        {y} = a_s.y * w1 + {y} * w2;\n        {z} = a_s.z * w1 + {z} * w2;\n        {w} = a_s.w * w1 + {w} * w2;\n        {dr} = abs(a_dr) * w1 + abs({dr}) * w2;\n        let f = l / sqrt({x} * {x} + {y} * {y} + {z} * {z} + 1.0e-40);\n        {x} = {x} * f;\n        {y} = {y} * f;\n        {z} = {z} * f;\n        n += 1u;\n        let r2 = f32(l * l);\n        {rout} = fl(r2);\n        out.trap = min(out.trap, r2);\n        if ((!use_fixed || color_bail) && !(r2 <= select({bail2}, {bail2} * min({bail2}, 5.0e36 / max({bail2}, 1.0)) * 64.0, color_bail))) {{\n            out.escaped = true;\n            break;\n        }}\n    }}\n    // The normal hybrid loop is not used.\n    if (false) {{"
        );
    }
    s.push_str("    var slot = 0u;\n    var count = 0u;\n    let cap = limit * 8u + 64u;\n    var steps = 0u;\n");
    s.push_str(&counts_init(prog, cfg));
    s.push_str(&f32_loop(prog, k, cfg, &names));
    if blend.is_some() {
        s.push_str("    }\n");
    }
    let _ = writeln!(s, "    out.s = vec4<f32>(f32({}), f32({}), f32({}), f32({}));\n    out.rold = f32({});\n    out.dr = f32({});\n    out.n = n;\n    out.cd = f32({});", names[0], names[1], names[2], names[3], names[STATE_ROLD], names[8], names[STATE_DIST]);
    // Mandelbulb3D's second color choice (mirrors fractal.rs chain_orbit).
    let _ = writeln!(s, "    out.c = vec3<f32>(f32({}), f32({}), f32({}));\n    out.c2 = color2_value(out.s, out.c, dot(out.s, out.s), out.rold, out.trap);\n    return out;\n}}", names[4], names[5], names[6]);
    s.push_str(&CHAIN_DE.replace("SCALE_K", &cfg.scale).replace("_K", &format!("_{k}")));
    s
}

/// The hybrid loop of a single precision chain (the variables `n`, `slot`, `count`, `cap`,
/// `steps`, `limit`, `repeat_from` and the registers `names` exist).
fn f32_loop(prog: &Program, k: &str, cfg: &Uni, names: &[String]) -> String {
    let mut s = String::new();
    s.push_str("    loop {\n        if (n >= limit || steps >= cap) {\n            break;\n        }\n        steps += 1u;\n");
    s.push_str(&extras_skip(cfg, k));
    s.push_str(&counts_skip(prog, k));
    let _ = writeln!(
        s,
        "        let prev_s = vec4<fl>({}, {}, {}, {});\n        let prev_dr = {};",
        names[0], names[1], names[2], names[3], names[8]
    );
    s.push_str(&extras_pre(cfg, names, "0.0"));
    let _ = writeln!(s, "        {} = fl(n);\n        {} = {};\n        var transform = false;\n        switch slot {{", names[9], names[STATE_ROLD], names[STATE_ROUT]);
    for (i, sl) in prog.slots.iter().enumerate() {
        let _ = writeln!(s, "            case {i}u: {{ // {}", sl.formula.def.name);
        if sl.transform {
            s.push_str("                transform = true;\n");
        }
        s.push_str(&slot_iter_code(prog, i, false));
        s.push_str("            }\n");
    }
    s.push_str("            default: {}\n        }\n");
    s.push_str(&extras_post(cfg, names, ["prev_s.x", "prev_s.y", "prev_s.z", "prev_s.w", "prev_dr"], |a, b| format!("{a} + ({b} - {a}) * fl(slot_wt)")));
    let _ = write!(
        s,
        "        if (!transform) {{\n            n += 1u;\n        }}\n        count += 1u;\n        if (count >= {slot_iter}) {{\n            count = 0u;\n            slot += 1u;\n            if (slot >= NSLOTS_{k}) {{\n                slot = repeat_from;\n            }}\n        }}\n\
         \x20       if (DIFS_{k}) {{\n            if (!transform) {{\n                let de_i = f32({rout} / {a0});\n                if (de_i < out.de) {{\n                    out.de = de_i;\n                    out.trap = f32({a2} * {a2});\n                }}\n            }}\n            continue;\n        }}\n\
         \x20       if (!transform) {{\n            let r2 = f32({x} * {x} + {y} * {y} + {z} * {z} + {w} * {w});\n            out.trap = min(out.trap, r2);\n            {rout} = fl(r2);\n\
         \x20           if ((!use_fixed || color_bail) && !(r2 <= select({bail2}, {bail2} * min({bail2}, 5.0e36 / max({bail2}, 1.0)) * 64.0, color_bail)){extra_bail}) {{\n                out.escaped = true;\n                if (!is_finite(r2)) {{\n                    {x} = prev_s.x;\n                    {y} = prev_s.y;\n                    {z} = prev_s.z;\n                    {w} = prev_s.w;\n                    {dr} = prev_dr;\n                }}\n                break;\n            }}\n        }}\n    }}\n",
        x = names[0],
        y = names[1],
        z = names[2],
        w = names[3],
        rout = names[STATE_ROUT],
        a0 = names[STATE_AUX],
        a2 = names[STATE_AUX + 2],
        dr = names[8],
        slot_iter = run_length(prog, cfg),
        bail2 = cfg.bail2,
        extra_bail = extras_bail(cfg),
    );
    s
}

/// [`chain`] in double-float arithmetic for deep zooms (main fractal only): the iteration starts
/// at the reference point `u.deep_hi/lo` plus the local position `p`, every register is a `dd`.
/// The outputs are f32 like [`chain`]'s, plus the dd state for the numeric estimate and the mix.
fn chain_dd(prog: &Program, k: &str, cfg: &Uni) -> String {
    let (limit, repeat) = (cfg.limit.as_str(), cfg.repeat.as_str());
    let mut s = String::new();
    let _ = writeln!(s, "\nconst NSLOTS_{k}: u32 = {}u;", prog.slots.len());
    let _ = writeln!(s, "const NUMERIC_{k}: bool = {};", prog.numeric);
    let _ = writeln!(s, "const DE_LINEAR_{k}: bool = {};", prog.de_kind == DeKind::Linear);
    let _ = writeln!(s, "const DE_MAXAXIS_{k}: bool = {};", prog.de_kind == DeKind::MaxAxis);
    let _ = writeln!(s, "const DE_CUSTOM_{k}: bool = {};", prog.de_kind == DeKind::Custom);
    let _ = writeln!(s, "const DIFS_{k}: bool = {};", prog.difs);
    let _ = writeln!(s, "const DEEP_{k}: bool = true;");
    // Mixed precision: once the derivative is large, f32 rounding moves the point by far less
    // than the surface threshold, so the iteration continues in f32 (not for the numeric
    // estimate, whose differences need double-float, nor for interpolation hybrids).
    let blend_dd = k == "1" && prog.blend.is_some() && prog.slots.len() >= 2;
    let _ = writeln!(s, "const DD_SWITCH_{k}: bool = {};", !prog.numeric && !blend_dd);
    let _ = writeln!(s, "\nfn iterate_{k}(p: vec3<f32>, fixed_n: u32, use_fixed: bool, start: vec4<f32>, use_start: bool) -> Iterated {{");
    s.push_str("    var out: Iterated;\n    out.trap = 3.0e38;\n    out.escaped = false;\n");
    let n: Vec<String> = (0..STATE_COUNT).map(state_name).collect();
    let (x, y, z, w) = (&n[0], &n[1], &n[2], &n[3]);
    let (dr, it, rout, rold) = (&n[8], &n[9], &n[STATE_ROUT], &n[STATE_ROLD]);
    let dist = &n[STATE_DIST];
    // World position = reference point + local position.
    s.push_str("    let wx = dd_add(dd(u.deep_hi.x, u.deep_lo.x), dd_f(p.x));\n    let wy = dd_add(dd(u.deep_hi.y, u.deep_lo.y), dd_f(p.y));\n    let wz = dd_add(dd(u.deep_hi.z, u.deep_lo.z), dd_f(p.z));\n");
    let _ = writeln!(s, "    var v0 = wx;\n    var v1 = wy;\n    var v2 = wz;\n    var v3 = dd({}, u.deep_lo.w);", cfg.wslice);
    let _ = writeln!(
        s,
        "    if ({}) {{\n        let q = array<dd, 3>(wx, wy, wz);\n        var r = array<dd, 4>(dd_f(0.0), dd_f(0.0), dd_f(0.0), dd_f(0.0));\n        for (var i = 0u; i < 4u; i++) {{\n            for (var j = 0u; j < 3u; j++) {{\n                r[i] = dd_add(r[i], dd_mulf(q[j], u.rot4[i][j]));\n            }}\n        }}\n        v0 = r[0];\n        v1 = r[1];\n        v2 = r[2];\n        v3 = r[3];\n    }}",
        cfg.rot4
    );
    let _ = writeln!(s, "    var {x}: dd = v0;\n    var {y}: dd = v1;\n    var {z}: dd = v2;\n    var {w}: dd = v3;");
    let _ = writeln!(s, "    var {}: dd = v0;\n    var {}: dd = v1;\n    var {}: dd = v2;\n    var {}: dd = v3;", n[4], n[5], n[6], n[7]);
    let (jo, j) = (&cfg.julia_on, &cfg.julia);
    let _ = writeln!(
        s,
        "    if ({jo}) {{\n        {} = dd({j}.x, u.julia_lo.x);\n        {} = dd({j}.y, u.julia_lo.y);\n        {} = dd({j}.z, u.julia_lo.z);\n        {} = dd({j}.w, u.julia_lo.w);\n    }}",
        n[4], n[5], n[6], n[7]
    );
    s.push_str(&extras_init(cfg, &n, |c, f| format!("dd_mulf({c}, {f})")));
    let _ = writeln!(s, "    var {dr}: dd = dd_f(1.0);\n    var {it}: dd = dd_f(0.0);");
    let _ = writeln!(s, "    var {rout}: dd = dd_add(dd_add(dd_sqr(v0), dd_sqr(v1)), dd_add(dd_sqr(v2), dd_sqr(v3)));\n    var {rold}: dd = dd_f(0.0);");
    // A mix continues from the first chain's point (passed in `mix_start`).
    let _ = writeln!(
        s,
        "    if (use_start) {{\n        {x} = mix_start[0];\n        {y} = mix_start[1];\n        {z} = mix_start[2];\n        {w} = mix_start[3];\n        {rout} = dd_add(dd_add(dd_sqr({x}), dd_sqr({y})), dd_add(dd_sqr({z}), dd_sqr({w})));\n    }}"
    );
    let _ = writeln!(s, "    let {} = wx;\n    let {} = wy;\n    let {} = wz;", n[STATE_PX], n[STATE_PX + 1], n[STATE_PX + 2]);
    for name in &n[STATE_AUX..STATE_JULIA] {
        let _ = writeln!(s, "    var {name}: dd = dd_f(0.0);");
    }
    let _ = writeln!(s, "    let {}: dd = select(dd_f(0.0), dd_f(1.0), {jo});", n[STATE_JULIA]);
    let _ = writeln!(s, "    if (DIFS_{k}) {{\n        {} = dd_f(1.0);\n    }}\n    out.de = 65535.0;", n[STATE_AUX]);
    let _ = writeln!(s, "    if (NSLOTS_{k} == 0u) {{\n        return out;\n    }}");
    let _ = writeln!(s, "    let limit = select(u32({limit}), fixed_n, use_fixed);\n    let repeat_from = u32({repeat});");
    s.push_str("    var n = 0u;\n");
    // bailout² · 64 kept below the f32 maximum (a baked constant that overflows is an error).
    let bail_plain = format!("(!use_fixed || color_bail) && !(r2 <= select({b}, {b} * min({b}, 5.0e36 / max({b}, 1.0)) * 64.0, color_bail))", b = cfg.bail2);
    // The interpolation hybrid ignores the per-slot settings (like fractal.rs).
    let bail = format!("{bail_plain}{}", extras_bail(cfg));
    let blend = k == "1" && prog.blend.is_some() && prog.slots.len() >= 2;
    // Declared before the interpolation loop: the f32 continuation below reads them.
    s.push_str("    var slot = 0u;\n    var count = 0u;\n    let cap = limit * 8u + 64u;\n    var steps = 0u;\n");
    s.push_str("    var switched = false;\n");
    s.push_str(&counts_init(prog, cfg));
    if blend {
        let _ = writeln!(s, "    loop {{\n        if (n >= limit) {{\n            break;\n        }}\n        {rold} = {rout};\n        {it} = dd_f(f32(n));");
        let _ = writeln!(s, "        let in_x = {x};\n        let in_y = {y};\n        let in_z = {z};\n        let in_w = {w};\n        let in_dr = {dr};\n        {{ // {}", prog.slots[0].formula.def.name);
        s.push_str(&emit_iter_dd(&prog.slots[0].formula.def, prog.slots[0].offset, 3));
        let _ = writeln!(
            s,
            "        }}\n        let a_x = {x};\n        let a_y = {y};\n        let a_z = {z};\n        let a_w = {w};\n        let a_dr = {dr};\n        {x} = in_x;\n        {y} = in_y;\n        {z} = in_z;\n        {w} = in_w;\n        {dr} = in_dr;\n        {{ // {}",
            prog.slots[1].formula.def.name
        );
        s.push_str(&emit_iter_dd(&prog.slots[1].formula.def, prog.slots[1].offset, 3));
        let _ = writeln!(
            s,
            "        }}\n        let w1 = dd(u.blend.x, u.blend_lo.x);\n        let w2 = dd(u.blend.y, u.blend_lo.y);\n        let len_a = dd_sqrt(dd_add(dd_add(dd_sqr(a_x), dd_sqr(a_y)), dd_sqr(a_z)));\n        let len_b = dd_sqrt(dd_add(dd_add(dd_sqr({x}), dd_sqr({y})), dd_sqr({z})));\n        let l = dd_add(dd_mul(len_a, w1), dd_mul(len_b, w2));\n        {x} = dd_add(dd_mul(a_x, w1), dd_mul({x}, w2));\n        {y} = dd_add(dd_mul(a_y, w1), dd_mul({y}, w2));\n        {z} = dd_add(dd_mul(a_z, w1), dd_mul({z}, w2));\n        {w} = dd_add(dd_mul(a_w, w1), dd_mul({w}, w2));\n        {dr} = dd_add(dd_mul(dd_abs(a_dr), w1), dd_mul(dd_abs({dr}), w2));\n        let f = dd_div(l, dd_sqrt(dd_add(dd_add(dd_add(dd_sqr({x}), dd_sqr({y})), dd_sqr({z})), dd_f(1.0e-40))));\n        {x} = dd_mul({x}, f);\n        {y} = dd_mul({y}, f);\n        {z} = dd_mul({z}, f);\n        n += 1u;\n        {rout} = dd_sqr(l);\n        let r2 = dd_hi({rout});\n        out.trap = min(out.trap, r2);\n        if ({bail_plain}) {{\n            out.escaped = true;\n            break;\n        }}\n    }}\n    // The normal hybrid loop is not used.\n    if (false) {{"
        );
    }
    s.push_str("    loop {\n        if (n >= limit || steps >= cap) {\n            break;\n        }\n        steps += 1u;\n");
    s.push_str(&extras_skip(cfg, k));
    s.push_str(&counts_skip(prog, k));
    let _ = writeln!(s, "        let prev_x = {x};\n        let prev_y = {y};\n        let prev_z = {z};\n        let prev_w = {w};\n        let prev_dr = {dr};");
    s.push_str(&extras_pre(cfg, &n, "dd_f(0.0)"));
    let _ = writeln!(s, "        {it} = dd_f(f32(n));\n        {rold} = {rout};\n        var transform = false;\n        switch slot {{");
    for (i, sl) in prog.slots.iter().enumerate() {
        let _ = writeln!(s, "            case {i}u: {{ // {}", sl.formula.def.name);
        if sl.transform {
            s.push_str("                transform = true;\n");
        }
        s.push_str(&slot_iter_code(prog, i, true));
        s.push_str("            }\n");
    }
    s.push_str("            default: {}\n        }\n");
    s.push_str(&extras_post(cfg, &n, ["prev_x", "prev_y", "prev_z", "prev_w", "prev_dr"], |a, b| format!("dd_add({a}, dd_mulf(dd_sub({b}, {a}), slot_wt))")));
    let _ = write!(
        s,
        "        if (!transform) {{\n            n += 1u;\n        }}\n        count += 1u;\n        if (count >= {slot_iter}) {{\n            count = 0u;\n            slot += 1u;\n            if (slot >= NSLOTS_{k}) {{\n                slot = repeat_from;\n            }}\n        }}\n\
         \x20       if (DIFS_{k}) {{\n            if (!transform) {{\n                let de_i = dd_hi(dd_div({rout}, {a0}));\n                if (de_i < out.de) {{\n                    out.de = de_i;\n                    out.trap = dd_hi(dd_sqr({a2}));\n                }}\n                if ({switch}abs(dd_hi({a0})) * dd_target > 1.0e-5 * {zlen}) {{\n                    switched = true;\n                    break;\n                }}\n            }}\n            continue;\n        }}\n\
         \x20       if (!transform) {{\n            {rout} = dd_add(dd_add(dd_sqr({x}), dd_sqr({y})), dd_add(dd_sqr({z}), dd_sqr({w})));\n            let r2 = dd_hi({rout});\n            out.trap = min(out.trap, r2);\n\
         \x20           if ({bail}) {{\n                out.escaped = true;\n                if (!is_finite(r2)) {{\n                    {x} = prev_x;\n                    {y} = prev_y;\n                    {z} = prev_z;\n                    {w} = prev_w;\n                    {dr} = prev_dr;\n                }}\n                break;\n            }}\n            if ({switch}abs(dd_hi({dr})) * dd_target > 1.0e-5 * {zlen}) {{\n                switched = true;\n                break;\n            }}\n        }}\n    }}\n",
        a0 = n[STATE_AUX],
        a2 = n[STATE_AUX + 2],
        slot_iter = run_length(prog, cfg),
        switch = format!("DD_SWITCH_{k} && dd_target > 0.0 && "),
        zlen = format!("length(vec4<f32>(dd_hi({x}), dd_hi({y}), dd_hi({z}), dd_hi({w})))"),
    );
    if blend {
        s.push_str("    }\n");
    }
    // The rest of the iterations in f32: the registers again as f32 (shadowing the dd ones).
    s.push_str("    if (switched) {\n");
    let lets = [STATE_PX, STATE_PX + 1, STATE_PX + 2, STATE_JULIA];
    for (i, name) in n.iter().enumerate() {
        let _ = writeln!(s, "        let t{i} = dd_hi({name});");
    }
    s.push_str("        {\n");
    for (i, name) in n.iter().enumerate() {
        let kw = if lets.contains(&i) { "let" } else { "var" };
        let _ = writeln!(s, "            {kw} {name}: fl = t{i};");
    }
    s.push_str(&f32_loop(prog, k, cfg, &n));
    let _ = writeln!(
        s,
        "            out.s = vec4<f32>(f32({x}), f32({y}), f32({z}), f32({w}));\n            out.rold = f32({rold});\n            out.dr = f32({dr});\n            out.n = n;\n            out.cd = f32({dist});\n            out.sd = array<vec2<f32>, 4>(dd_f(out.s.x), dd_f(out.s.y), dd_f(out.s.z), dd_f(out.s.w));\n            out.rd = dd_f(length(out.s));\n            out.c = vec3<f32>(f32({}), f32({}), f32({}));\n            out.c2 = color2_value(out.s, out.c, dot(out.s, out.s), out.rold, out.trap);\n            return out;\n        }}\n    }}",
        n[4], n[5], n[6]
    );
    let _ = writeln!(s, "    out.s = vec4<f32>(dd_hi({x}), dd_hi({y}), dd_hi({z}), dd_hi({w}));\n    out.rold = dd_hi({rold});\n    out.dr = dd_hi({dr});\n    out.n = n;\n    out.cd = dd_hi({dist});");
    let _ = writeln!(s, "    out.sd = array<vec2<f32>, 4>({x}, {y}, {z}, {w});\n    out.rd = dd_sqrt(dd_add(dd_add(dd_sqr({x}), dd_sqr({y})), dd_add(dd_sqr({z}), dd_sqr({w}))));");
    let _ = writeln!(s, "    out.c = vec3<f32>(dd_hi({}), dd_hi({}), dd_hi({}));\n    out.c2 = color2_value(out.s, out.c, dot(out.s, out.s), out.rold, out.trap);\n    return out;\n}}", n[4], n[5], n[6]);
    s.push_str(&CHAIN_DE.replace("SCALE_K", &cfg.scale).replace("_K", &format!("_{k}")));
    s
}

/// Distance estimate of one chain (`_K` is replaced by the chain number).
const CHAIN_DE: &str = r#"
fn chain_de_K(p: vec3<f32>, h: f32) -> Orbit {
    var o: Orbit;
    if (NSLOTS_K == 0u) {
        o.de = 1.0e10;
        return o;
    }
    // Deep zoom: the precision the point needs (see DD_SWITCH).
    dd_target = h;
    let it = iterate_K(p, 0u, false, vec4<f32>(0.0), false);
    let r2 = dot(it.s, it.s);
    let r = sqrt(r2);
    var de = 0.0;
    if (DIFS_K) {
        de = max(it.de, 0.0);
    } else if (DE_CUSTOM_K) {
        // The formula's own estimate, escaped or not.
        de = max(it.cd, 0.0);
    } else if (it.escaped) {
        if (NUMERIC_K) {
            // Gradient step: at least a little relative to the point (mirrors fractal.rs; f32
            // cancels below 2e-6, double-float differences go to NUMERIC_STEP_F64).
            var hs = max(h, length(p) * 2e-6);
            if (DEEP_K) {
                hs = max(h, length(u.deep_hi.xyz) * 1e-13);
            }
            var g = vec3<f32>(0.0);
            for (var k = 0u; k < 3u; k++) {
                var q = p;
                q[k] += hs;
                let o2 = iterate_K(q, it.n, true, vec4<f32>(0.0), false);
                if (DEEP_K) {
                    g[k] = dd_hi(dd_sub(o2.rd, it.rd)) / hs;
                } else {
                    g[k] = (length(o2.s) - r) / hs;
                }
            }
            let gl = max(length(g), 1.0e-30);
            if (DE_LINEAR_K) {
                de = r / gl;
            } else {
                de = 0.5 * r * log(r) / gl;
            }
        } else {
            if (DE_MAXAXIS_K) {
                de = max(abs(it.s.x), max(abs(it.s.y), abs(it.s.z))) / abs(it.dr);
            } else if (DE_LINEAR_K) {
                de = r / abs(it.dr);
            } else {
                de = 0.5 * log(r) * r / it.dr;
            }
        }
    }
    if (!is_finite(de)) {
        de = select(0.0, 1.0e-3, it.escaped);
    }
    o.de = de * SCALE_K;
    o.n = it.n;
    o.r2 = r2;
    o.r2_old = it.rold;
    o.trap = it.trap;
    o.c2 = it.c2;
    o.escaped = it.escaped || DIFS_K;
    return o;
}
"#;

#[cfg(test)]
mod tests {
    use super::{bake_constants, bake_uniforms, uniform_layout};

    #[test]
    fn uniform_layout_matches_the_struct() {
        let (fields, words) = uniform_layout();
        assert_eq!(words * 4, crate::renderer::uniforms_size());
        assert_eq!(fields[0].name, "cam_pos");
        assert!(fields.iter().any(|f| f.name == "slots" && f.uint && f.array == Some(16)));
    }

    #[test]
    fn bakes_uniform_reads_with_constant_indexes() {
        let (fields, words) = uniform_layout();
        let mut w = vec![0u32; words];
        let at = |n: &str| fields.iter().find(|f| f.name == n).unwrap().word;
        w[at("fog") + 3] = 0.5f32.to_bits();
        w[at("glow2")] = 1.0f32.to_bits();
        w[at("glow2") + 1] = 2.0f32.to_bits();
        w[at("glow2") + 2] = 3.0f32.to_bits();
        w[at("bg_rot") + 4 + 3] = 7.0f32.to_bits();
        w[at("band") + 2] = 400;
        w[at("band")] = 9;
        let mut bake = vec![true; words];
        bake[at("fog") + 2] = false;
        let src = "a = u.fog.w; b = u.glow2.xyz; c = u.bg_rot[1].w; d = u.band.z; e = u.band.x; f = u.lights[i].dir; g = mu.fog.w; h = u.fog.xyz; k = u.slots[slot + 2u].x;";
        assert_eq!(
            bake_uniforms(src, &w, &bake),
            "a = (0.5); b = vec3<f32>(1.0, 2.0, 3.0); c = (7.0); d = (400u); e = u.band.x; f = u.lights[i].dir; g = mu.fog.w; h = u.fog.xyz; k = u.slots[slot + 2u].x;"
        );
    }

    #[test]
    fn bakes_only_the_given_finite_parameters() {
        let src = "a = prm[0u][1u]; b = prm[1u][0u]; c = prm[2u][3u]; d = prm[0u][2u]; e = prm[1024u][1u];";
        let tenth = 0.1f64;
        let lo = (tenth - tenth as f32 as f64) as f32;
        let baked = [None, Some(tenth), Some(f64::INFINITY), None, Some(-0.25)];
        assert_eq!(bake_constants(src, &baked), format!("a = (0.1); b = (-0.25); c = prm[2u][3u]; d = prm[0u][2u]; e = ({lo:?});"));
    }
}
