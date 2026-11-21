// SPDX-License-Identifier: LGPL-2.1-or-later

//! Generates the WGSL of a hybrid formula chain (the fractal part of march.wgsl).
//!
//! The shader depends only on the formulas of the slots and the DE variant of each chain;
//! iteration counts, repeat slots, combine settings and all parameter values come from buffers.
//! A DE combination gets a second `iterate`/`chain_de` pair; `fractal_de` combines them.

use std::fmt::Write;

use rmb_core::formula::DeKind;
use rmb_core::formula::lang::{STATE_AUX, STATE_COUNT, STATE_JULIA, STATE_PX, STATE_ROLD, STATE_ROUT};
use rmb_core::formula::wgsl::{PRELUDE, emit_iter, state_name};
use rmb_core::fractal::Program;

const TEMPLATE: &str = include_str!("march.wgsl");

/// The complete ray marching shader for `prog`.
pub fn march_shader(prog: &Program) -> String {
    let mut s = String::from(PRELUDE);
    let _ = writeln!(s, "const ROTATE_4D: bool = {};", prog.rotation_4d.is_some());
    let _ = writeln!(s, "const REFLECT: bool = {};", prog.reflections);
    let _ = writeln!(s, "const LAYERS: bool = {};", prog.layers);
    let _ = writeln!(s, "const BUMP: bool = {};", prog.bump);
    let _ = writeln!(s, "const MODELS: bool = {};", prog.objects.iter().any(|o| matches!(o.shape, rmb_core::fractal::ObjectShape::Model(_))));
    s.push_str(&chain(prog, "1", &Uni::main(0, "u.frac.z", "u.frac.w", "u.scales.x")));
    match prog.part2.as_deref() {
        Some(p2) => {
            s.push_str(&chain(p2, "2", &Uni::main(prog.slots.len(), "u.comb.y", "u.comb.z", "u.scales.y")));
            if prog.is_mix() {
                // Mix (mirrors fractal.rs Evaluator::orbit): the dIFS from where chain 1 ends.
                let _ = writeln!(s, "const MIX_LINEAR: bool = {};", prog.de_kind == DeKind::Linear && !prog.difs);
                s.push_str(
                    "\nfn fractal_de(p: vec3<f32>, h: f32) -> Orbit {\n    let a = iterate_1(p, 0u, false, vec4<f32>(0.0), false);\n    let b = iterate_2(p, 0u, false, a.s, true);\n    let d = max(b.de, 0.0) * u.scales.y;\n    var o: Orbit;\n    if (MIX_LINEAR) {\n        o.de = d / max(abs(a.dr), 1.0e-30);\n    } else {\n        o.de = d * pow(max(abs(u.interior.w), 1.0e-30), -1.0 - f32(a.n));\n    }\n    o.n = b.n;\n    o.r2 = dot(a.s, a.s);\n    o.r2_old = a.rold;\n    o.trap = b.trap;\n    o.escaped = true;\n    return o;\n}\n",
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
    TEMPLATE.replace("//@ITERATE@", &s)
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
            "        }}\n        let w1 = fl(u.blend.x);\n        let w2 = fl(u.blend.y);\n        let len_b = sqrt({x} * {x} + {y} * {y} + {z} * {z});\n        let l = length(a_s.xyz) * w1 + len_b * w2;\n        {x} = a_s.x * w1 + {x} * w2;\n        {y} = a_s.y * w1 + {y} * w2;\n        {z} = a_s.z * w1 + {z} * w2;\n        {w} = a_s.w * w1 + {w} * w2;\n        {dr} = abs(a_dr) * w1 + abs({dr}) * w2;\n        let f = l / sqrt({x} * {x} + {y} * {y} + {z} * {z} + 1.0e-40);\n        {x} = {x} * f;\n        {y} = {y} * f;\n        {z} = {z} * f;\n        n += 1u;\n        let r2 = f32(l * l);\n        {rout} = fl(r2);\n        out.trap = min(out.trap, r2);\n        if (!use_fixed && !(r2 <= u.march.z)) {{\n            out.escaped = true;\n            break;\n        }}\n    }}\n    // The normal hybrid loop is not used.\n    if (false) {{"
        );
    }
    s.push_str("    var slot = 0u;\n    var count = 0u;\n    let cap = limit * 8u + 64u;\n    var steps = 0u;\n");
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
         \x20           if (!use_fixed && !(r2 <= {bail2})) {{\n                out.escaped = true;\n                if (!is_finite(r2)) {{\n                    {x} = prev_s.x;\n                    {y} = prev_s.y;\n                    {z} = prev_s.z;\n                    {w} = prev_s.w;\n                    {dr} = prev_dr;\n                }}\n                break;\n            }}\n        }}\n    }}\n",
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
    if blend {
        s.push_str("    }\n");
    }
    let _ = writeln!(s, "    out.s = vec4<f32>(f32({}), f32({}), f32({}), f32({}));\n    out.rold = f32({});\n    out.dr = f32({});\n    out.n = n;\n    return out;\n}}", names[0], names[1], names[2], names[3], names[STATE_ROLD], names[8]);
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
    let it = iterate_K(p, 0u, false, vec4<f32>(0.0), false);
    let r2 = dot(it.s, it.s);
    let r = sqrt(r2);
    var de = 0.0;
    if (DIFS_K) {
        de = max(it.de, 0.0);
    } else if (it.escaped) {
        if (NUMERIC_K) {
            // Gradient step: at least a little relative to the point (mirrors fractal.rs).
            let h = max(h, length(p) * 2e-6);
            var g = vec3<f32>(0.0);
            for (var k = 0u; k < 3u; k++) {
                var q = p;
                q[k] += h;
                let o2 = iterate_K(q, it.n, true, vec4<f32>(0.0), false);
                g[k] = (length(o2.s) - r) / h;
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
    o.escaped = it.escaped || DIFS_K;
    return o;
}
"#;
