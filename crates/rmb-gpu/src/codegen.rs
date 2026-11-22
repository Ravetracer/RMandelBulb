// SPDX-License-Identifier: LGPL-2.1-or-later

//! Generates the WGSL of a hybrid formula chain (the fractal part of march.wgsl).
//!
//! The shader depends only on the formulas of the slots and the DE variant of each chain;
//! iteration counts, repeat slots, combine settings and all parameter values come from buffers.
//! A DE combination gets a second `iterate`/`chain_de` pair; `fractal_de` combines them.

use std::fmt::Write;

use rmb_core::formula::DeKind;
use rmb_core::formula::lang::{STATE_AUX, STATE_COUNT, STATE_JULIA, STATE_PX, STATE_ROLD, STATE_ROUT};
use rmb_core::formula::wgsl::{PRELUDE, emit_iter, emit_iter_dd, state_name};
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
    let _ = writeln!(s, "const LAYERS: bool = {};", prog.layers);
    let _ = writeln!(s, "const BUMP: bool = {};", prog.bump);
    let _ = writeln!(s, "const COLOR_ON_IT: bool = {};", prog.color_on_it >= 0 && prog.part2.is_none() && !prog.slots.is_empty());
    let _ = writeln!(s, "const MODELS: bool = {};", prog.objects.iter().any(|o| matches!(o.shape, rmb_core::fractal::ObjectShape::Model(_))));
    // Deep zoom: the main fractal in double-float arithmetic (composed objects stay f32, they
    // are evaluated in local coordinates).
    let main_chain = if prog.deep.is_some() { chain_dd } else { chain };
    s.push_str(&main_chain(prog, "1", &Uni::main(0, "u.frac.z", "u.frac.w", "u.scales.x")));
    match prog.part2.as_deref() {
        Some(p2) => {
            s.push_str(&main_chain(p2, "2", &Uni::main(prog.slots.len(), "u.comb.y", "u.comb.z", "u.scales.y")));
            if prog.is_mix() {
                // Mix (mirrors fractal.rs Evaluator::orbit): the dIFS from where chain 1 ends.
                let _ = writeln!(s, "const MIX_LINEAR: bool = {};", prog.de_kind == DeKind::Linear && !prog.difs);
                s.push_str(
                    "\nfn fractal_de(p: vec3<f32>, h: f32) -> Orbit {\n    dd_target = h;\n    let a = iterate_1(p, 0u, false, vec4<f32>(0.0), false);\n    mix_start = a.sd;\n    let b = iterate_2(p, 0u, false, a.s, true);\n    let d = max(b.de, 0.0) * u.scales.y;\n    var o: Orbit;\n    if (MIX_LINEAR) {\n        o.de = d / max(abs(a.dr), 1.0e-30);\n    } else {\n        o.de = d * pow(max(abs(u.interior.w), 1.0e-30), -1.0 - f32(a.n));\n    }\n    o.n = b.n;\n    o.r2 = dot(a.s, a.s);\n    o.r2_old = a.rold;\n    o.trap = b.trap;\n    o.c2 = b.c2;\n    o.escaped = true;\n    return o;\n}\n",
                );
            } else {
                s.push_str(
                    "\nfn fractal_de(p: vec3<f32>, h: f32) -> Orbit {\n    let a = chain_de_1(p, h);\n    let b = chain_de_2(p, h);\n    return combine_de(a, b);\n}\n",
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

/// The composed objects (mirrors fractal.rs Evaluator::orbit): a chain per fractal object and
/// `compose_objects`, which folds them into the estimate in order.
fn objects(prog: &Program) -> String {
    use rmb_core::fractal::ObjectShape;
    let mut s = String::new();
    for (j, o) in prog.objects.iter().enumerate() {
        if let ObjectShape::Fractal(f) = &o.shape {
            s.push_str(&chain(f, &format!("o{j}"), &Uni::object(j)));
        }
    }
    s.push_str("\nfn compose_objects(p: vec3<f32>, h: f32, o_in: Orbit) -> Orbit {\n    var o = o_in;\n");
    for (j, o) in prog.objects.iter().enumerate() {
        let _ = writeln!(s, "    {{\n        let ob = objs[{j}];\n        let q = obj_local({j}u, p);\n        let sc = ob.inv0.w;\n        let op = u32(ob.inv1.w);\n        var b: Orbit;");
        match &o.shape {
            ObjectShape::Fractal(_) => {
                let _ = writeln!(
                    s,
                    "        b = chain_de_o{j}(q, h / sc);\n        b.de = max(b.de * ob.misc.z, 0.0) * sc;\n        // A fractal has no distance inside; cutting with it needs one.\n        if (b.de <= 0.0 && (op == 1u || op == 4u)) {{\n            b.de = -2.0 * h;\n        }}"
                );
            }
            ObjectShape::Primitive(kind, _) => {
                let _ = writeln!(s, "        b.de = prim_de({kind}u, ob.prim, q) * sc;\n        b.escaped = true;");
            }
            ObjectShape::Model(_) => {
                let _ = writeln!(s, "        b.de = model_de({j}u, q) * sc;\n        b.escaped = true;");
            }
        }
        let _ = writeln!(s, "        let c = combine_op(op, o.de, b.de, ob.inv2.w);\n        if (c.y > 0.5) {{\n            o = b;\n            o.obj = {}u;\n        }}\n        o.de = c.x;\n    }}", j + 1);
    }
    s.push_str("    return o;\n}\n");
    s
}

/// Where a chain reads its settings: WGSL expressions (uniforms for the main fractal, the
/// object buffer for composed objects). `slot_iter` is the iteration count of slot `slot`.
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
            slot_iter: format!("u.slots[slot + {slot_base}u].x"),
            scale: scale.into(),
            rot4: "ROTATE_4D",
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
            slot_iter: format!("objs[{j}].slots[slot / 4u][slot % 4u]"),
            scale: format!("objs[{j}].misc.y"),
            rot4: "false",
        }
    }
}

/// `iterate_<k>` and `chain_de_<k>` of one chain.
fn chain(prog: &Program, k: &str, cfg: &Uni) -> String {
    let (limit, repeat) = (cfg.limit.as_str(), cfg.repeat.as_str());
    let mut s = String::new();
    let _ = writeln!(s, "\nconst NSLOTS_{k}: u32 = {}u;", prog.slots.len());
    let _ = writeln!(s, "const NUMERIC_{k}: bool = {};", prog.numeric);
    let _ = writeln!(s, "const DE_LINEAR_{k}: bool = {};", prog.de_kind == DeKind::Linear);
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
    let blend = k == "1" && prog.blend.is_some() && prog.slots.len() >= 2;
    if blend {
        let (x, y, z, w, dr, rout, rold, it) = (&names[0], &names[1], &names[2], &names[3], &names[8], &names[STATE_ROUT], &names[STATE_ROLD], &names[9]);
        let _ = writeln!(s, "    loop {{\n        if (n >= limit) {{\n            break;\n        }}\n        {rold} = {rout};\n        {it} = fl(n);");
        let _ = writeln!(s, "        let in_s = vec4<fl>({x}, {y}, {z}, {w});\n        let in_dr = {dr};\n        {{ // {}", prog.slots[0].formula.def.name);
        s.push_str(&emit_iter(&prog.slots[0].formula.def, prog.slots[0].offset, 3));
        let _ = writeln!(s, "        }}\n        let a_s = vec4<fl>({x}, {y}, {z}, {w});\n        let a_dr = {dr};\n        {x} = in_s.x;\n        {y} = in_s.y;\n        {z} = in_s.z;\n        {w} = in_s.w;\n        {dr} = in_dr;\n        {{ // {}", prog.slots[1].formula.def.name);
        s.push_str(&emit_iter(&prog.slots[1].formula.def, prog.slots[1].offset, 3));
        let _ = writeln!(
            s,
            "        }}\n        let w1 = fl(u.blend.x);\n        let w2 = fl(u.blend.y);\n        let len_b = sqrt({x} * {x} + {y} * {y} + {z} * {z});\n        let l = length(a_s.xyz) * w1 + len_b * w2;\n        {x} = a_s.x * w1 + {x} * w2;\n        {y} = a_s.y * w1 + {y} * w2;\n        {z} = a_s.z * w1 + {z} * w2;\n        {w} = a_s.w * w1 + {w} * w2;\n        {dr} = abs(a_dr) * w1 + abs({dr}) * w2;\n        let f = l / sqrt({x} * {x} + {y} * {y} + {z} * {z} + 1.0e-40);\n        {x} = {x} * f;\n        {y} = {y} * f;\n        {z} = {z} * f;\n        n += 1u;\n        let r2 = f32(l * l);\n        {rout} = fl(r2);\n        out.trap = min(out.trap, r2);\n        if ((!use_fixed || color_bail) && !(r2 <= select(u.march.z, u.march.z * u.march.z * 64.0, color_bail))) {{\n            out.escaped = true;\n            break;\n        }}\n    }}\n    // The normal hybrid loop is not used.\n    if (false) {{"
        );
    }
    s.push_str("    var slot = 0u;\n    var count = 0u;\n    let cap = limit * 8u + 64u;\n    var steps = 0u;\n");
    s.push_str(&f32_loop(prog, k, cfg, &names));
    if blend {
        s.push_str("    }\n");
    }
    let _ = writeln!(s, "    out.s = vec4<f32>(f32({}), f32({}), f32({}), f32({}));\n    out.rold = f32({});\n    out.dr = f32({});\n    out.n = n;", names[0], names[1], names[2], names[3], names[STATE_ROLD], names[8]);
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
    let _ = writeln!(
        s,
        "        let prev_s = vec4<fl>({}, {}, {}, {});\n        let prev_dr = {};",
        names[0], names[1], names[2], names[3], names[8]
    );
    let _ = writeln!(s, "        {} = fl(n);\n        {} = {};\n        var transform = false;\n        switch slot {{", names[9], names[STATE_ROLD], names[STATE_ROUT]);
    for (i, sl) in prog.slots.iter().enumerate() {
        let _ = writeln!(s, "            case {i}u: {{ // {}", sl.formula.def.name);
        if sl.transform {
            s.push_str("                transform = true;\n");
        }
        s.push_str(&emit_iter(&sl.formula.def, sl.offset, 4));
        s.push_str("            }\n");
    }
    s.push_str("            default: {}\n        }\n");
    let _ = write!(
        s,
        "        if (!transform) {{\n            n += 1u;\n        }}\n        count += 1u;\n        if (count >= {slot_iter}) {{\n            count = 0u;\n            slot += 1u;\n            if (slot >= NSLOTS_{k}) {{\n                slot = repeat_from;\n            }}\n        }}\n\
         \x20       if (DIFS_{k}) {{\n            if (!transform) {{\n                let de_i = f32({rout} / {a0});\n                if (de_i < out.de) {{\n                    out.de = de_i;\n                    out.trap = f32({a2} * {a2});\n                }}\n            }}\n            continue;\n        }}\n\
         \x20       if (!transform) {{\n            let r2 = f32({x} * {x} + {y} * {y} + {z} * {z} + {w} * {w});\n            out.trap = min(out.trap, r2);\n            {rout} = fl(r2);\n\
         \x20           if ((!use_fixed || color_bail) && !(r2 <= select({bail2}, {bail2} * {bail2} * 64.0, color_bail))) {{\n                out.escaped = true;\n                if (!is_finite(r2)) {{\n                    {x} = prev_s.x;\n                    {y} = prev_s.y;\n                    {z} = prev_s.z;\n                    {w} = prev_s.w;\n                    {dr} = prev_dr;\n                }}\n                break;\n            }}\n        }}\n    }}\n",
        x = names[0],
        y = names[1],
        z = names[2],
        w = names[3],
        rout = names[STATE_ROUT],
        a0 = names[STATE_AUX],
        a2 = names[STATE_AUX + 2],
        dr = names[8],
        slot_iter = cfg.slot_iter,
        bail2 = cfg.bail2,
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
    let bail = format!("(!use_fixed || color_bail) && !(r2 <= select({b}, {b} * {b} * 64.0, color_bail))", b = cfg.bail2);
    let blend = k == "1" && prog.blend.is_some() && prog.slots.len() >= 2;
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
            "        }}\n        let w1 = dd(u.blend.x, u.blend_lo.x);\n        let w2 = dd(u.blend.y, u.blend_lo.y);\n        let len_a = dd_sqrt(dd_add(dd_add(dd_sqr(a_x), dd_sqr(a_y)), dd_sqr(a_z)));\n        let len_b = dd_sqrt(dd_add(dd_add(dd_sqr({x}), dd_sqr({y})), dd_sqr({z})));\n        let l = dd_add(dd_mul(len_a, w1), dd_mul(len_b, w2));\n        {x} = dd_add(dd_mul(a_x, w1), dd_mul({x}, w2));\n        {y} = dd_add(dd_mul(a_y, w1), dd_mul({y}, w2));\n        {z} = dd_add(dd_mul(a_z, w1), dd_mul({z}, w2));\n        {w} = dd_add(dd_mul(a_w, w1), dd_mul({w}, w2));\n        {dr} = dd_add(dd_mul(dd_abs(a_dr), w1), dd_mul(dd_abs({dr}), w2));\n        let f = dd_div(l, dd_sqrt(dd_add(dd_add(dd_add(dd_sqr({x}), dd_sqr({y})), dd_sqr({z})), dd_f(1.0e-40))));\n        {x} = dd_mul({x}, f);\n        {y} = dd_mul({y}, f);\n        {z} = dd_mul({z}, f);\n        n += 1u;\n        {rout} = dd_sqr(l);\n        let r2 = dd_hi({rout});\n        out.trap = min(out.trap, r2);\n        if ({bail}) {{\n            out.escaped = true;\n            break;\n        }}\n    }}\n    // The normal hybrid loop is not used.\n    if (false) {{"
        );
    }
    s.push_str("    var slot = 0u;\n    var count = 0u;\n    let cap = limit * 8u + 64u;\n    var steps = 0u;\n");
    s.push_str("    var switched = false;\n");
    s.push_str("    loop {\n        if (n >= limit || steps >= cap) {\n            break;\n        }\n        steps += 1u;\n");
    let _ = writeln!(s, "        let prev_x = {x};\n        let prev_y = {y};\n        let prev_z = {z};\n        let prev_w = {w};\n        let prev_dr = {dr};");
    let _ = writeln!(s, "        {it} = dd_f(f32(n));\n        {rold} = {rout};\n        var transform = false;\n        switch slot {{");
    for (i, sl) in prog.slots.iter().enumerate() {
        let _ = writeln!(s, "            case {i}u: {{ // {}", sl.formula.def.name);
        if sl.transform {
            s.push_str("                transform = true;\n");
        }
        s.push_str(&emit_iter_dd(&sl.formula.def, sl.offset, 4));
        s.push_str("            }\n");
    }
    s.push_str("            default: {}\n        }\n");
    let _ = write!(
        s,
        "        if (!transform) {{\n            n += 1u;\n        }}\n        count += 1u;\n        if (count >= {slot_iter}) {{\n            count = 0u;\n            slot += 1u;\n            if (slot >= NSLOTS_{k}) {{\n                slot = repeat_from;\n            }}\n        }}\n\
         \x20       if (DIFS_{k}) {{\n            if (!transform) {{\n                let de_i = dd_hi(dd_div({rout}, {a0}));\n                if (de_i < out.de) {{\n                    out.de = de_i;\n                    out.trap = dd_hi(dd_sqr({a2}));\n                }}\n                if ({switch}abs(dd_hi({a0})) * dd_target > 1.0e-5 * {zlen}) {{\n                    switched = true;\n                    break;\n                }}\n            }}\n            continue;\n        }}\n\
         \x20       if (!transform) {{\n            {rout} = dd_add(dd_add(dd_sqr({x}), dd_sqr({y})), dd_add(dd_sqr({z}), dd_sqr({w})));\n            let r2 = dd_hi({rout});\n            out.trap = min(out.trap, r2);\n\
         \x20           if ({bail}) {{\n                out.escaped = true;\n                if (!is_finite(r2)) {{\n                    {x} = prev_x;\n                    {y} = prev_y;\n                    {z} = prev_z;\n                    {w} = prev_w;\n                    {dr} = prev_dr;\n                }}\n                break;\n            }}\n            if ({switch}abs(dd_hi({dr})) * dd_target > 1.0e-5 * {zlen}) {{\n                switched = true;\n                break;\n            }}\n        }}\n    }}\n",
        a0 = n[STATE_AUX],
        a2 = n[STATE_AUX + 2],
        slot_iter = cfg.slot_iter,
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
        "            out.s = vec4<f32>(f32({x}), f32({y}), f32({z}), f32({w}));\n            out.rold = f32({rold});\n            out.dr = f32({dr});\n            out.n = n;\n            out.sd = array<vec2<f32>, 4>(dd_f(out.s.x), dd_f(out.s.y), dd_f(out.s.z), dd_f(out.s.w));\n            out.rd = dd_f(length(out.s));\n            out.c = vec3<f32>(f32({}), f32({}), f32({}));\n            out.c2 = color2_value(out.s, out.c, dot(out.s, out.s), out.rold, out.trap);\n            return out;\n        }}\n    }}",
        n[4], n[5], n[6]
    );
    let _ = writeln!(s, "    out.s = vec4<f32>(dd_hi({x}), dd_hi({y}), dd_hi({z}), dd_hi({w}));\n    out.rold = dd_hi({rold});\n    out.dr = dd_hi({dr});\n    out.n = n;");
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
            if (DE_LINEAR_K) {
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
    use super::bake_constants;

    #[test]
    fn bakes_only_the_given_finite_parameters() {
        let src = "a = prm[0u][1u]; b = prm[1u][0u]; c = prm[2u][3u]; d = prm[0u][2u]; e = prm[1024u][1u];";
        let tenth = 0.1f64;
        let lo = (tenth - tenth as f32 as f64) as f32;
        let baked = [None, Some(tenth), Some(f64::INFINITY), None, Some(-0.25)];
        assert_eq!(bake_constants(src, &baked), format!("a = (0.1); b = (-0.25); c = prm[2u][3u]; d = prm[0u][2u]; e = ({lo:?});"));
    }
}
