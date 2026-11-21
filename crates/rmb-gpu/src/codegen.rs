// SPDX-License-Identifier: LGPL-2.1-or-later

//! Generates the WGSL of a hybrid formula chain (the fractal part of march.wgsl).
//!
//! The shader depends only on the formulas of the slots and the DE variant of each chain;
//! iteration counts, repeat slots, combine settings and all parameter values come from buffers.
//! A DE combination gets a second `iterate`/`chain_de` pair; `fractal_de` combines them.

use std::fmt::Write;

use rmb_core::formula::DeKind;
use rmb_core::formula::lang::{STATE_AUX, STATE_COUNT, STATE_PX, STATE_ROLD, STATE_ROUT};
use rmb_core::formula::wgsl::{PRELUDE, emit_iter, state_name};
use rmb_core::fractal::Program;

const TEMPLATE: &str = include_str!("march.wgsl");

/// The complete ray marching shader for `prog`.
pub fn march_shader(prog: &Program) -> String {
    let mut s = String::from(PRELUDE);
    let _ = writeln!(s, "const ROTATE_4D: bool = {};", prog.rotation_4d.is_some());
    s.push_str(&chain(prog, 1, 0, "u.frac.z", "u.frac.w"));
    match prog.part2.as_deref() {
        Some(p2) => {
            s.push_str(&chain(p2, 2, prog.slots.len(), "u.comb.y", "u.comb.z"));
            s.push_str(
                "\nfn fractal_de(p: vec3<f32>, h: f32) -> Orbit {\n    let a = chain_de_1(p, h);\n    let b = chain_de_2(p, h);\n    return combine_de(a, b);\n}\n",
            );
        }
        None => s.push_str("\nfn fractal_de(p: vec3<f32>, h: f32) -> Orbit {\n    return chain_de_1(p, h);\n}\n"),
    }
    TEMPLATE.replace("//@ITERATE@", &s)
}

/// `iterate_<k>` and `chain_de_<k>` of one chain whose slots start at `slot_base` in the
/// uniform slot table; `limit` / `repeat` are the uniform expressions of its settings.
fn chain(prog: &Program, k: usize, slot_base: usize, limit: &str, repeat: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "\nconst NSLOTS_{k}: u32 = {}u;", prog.slots.len());
    let _ = writeln!(s, "const NUMERIC_{k}: bool = {};", prog.numeric);
    let _ = writeln!(s, "const DE_LINEAR_{k}: bool = {};", prog.de_kind == DeKind::Linear);
    let _ = writeln!(s, "const DIFS_{k}: bool = {};", prog.difs);
    let _ = writeln!(s, "\nfn iterate_{k}(p: vec3<f32>, fixed_n: u32, use_fixed: bool) -> Iterated {{");
    s.push_str("    var out: Iterated;\n    out.trap = 3.0e38;\n    out.escaped = false;\n");
    let names: Vec<String> = (0..STATE_COUNT).map(state_name).collect();
    let _ = writeln!(
        s,
        "    var v = vec4<fl>(fl(p.x), fl(p.y), fl(p.z), fl(u.frac.x));\n    if (ROTATE_4D) {{\n        let q = vec4<fl>(fl(p.x), fl(p.y), fl(p.z), 0.0);\n        v = vec4<fl>(dot(vec4<fl>(u.rot4[0]), q), dot(vec4<fl>(u.rot4[1]), q), dot(vec4<fl>(u.rot4[2]), q), dot(vec4<fl>(u.rot4[3]), q));\n    }}"
    );
    let _ = writeln!(s, "    var {}: fl = v.x;\n    var {}: fl = v.y;\n    var {}: fl = v.z;\n    var {}: fl = v.w;", names[0], names[1], names[2], names[3]);
    let _ = writeln!(s, "    var {}: fl = v.x;\n    var {}: fl = v.y;\n    var {}: fl = v.z;\n    var {}: fl = v.w;", names[4], names[5], names[6], names[7]);
    let _ = writeln!(s, "    if (u.frac.y > 0.5) {{\n        {} = u.julia.x;\n        {} = u.julia.y;\n        {} = u.julia.z;\n        {} = u.julia.w;\n    }}", names[4], names[5], names[6], names[7]);
    let _ = writeln!(s, "    var {}: fl = 1.0;\n    var {}: fl = 0.0;", names[8], names[9]);
    let _ = writeln!(s, "    var {}: fl = dot(v, v);\n    var {}: fl = 0.0;", names[STATE_ROUT], names[STATE_ROLD]);
    let _ = writeln!(s, "    let {} = fl(p.x);\n    let {} = fl(p.y);\n    let {} = fl(p.z);", names[STATE_PX], names[STATE_PX + 1], names[STATE_PX + 2]);
    for name in &names[STATE_AUX..STATE_COUNT] {
        let _ = writeln!(s, "    var {name}: fl = 0.0;");
    }
    // dIFS: VaryScale starts at 1.
    let _ = writeln!(s, "    if (DIFS_{k}) {{\n        {} = 1.0;\n    }}\n    out.de = 65535.0;", names[STATE_AUX]);
    let _ = writeln!(s, "    if (NSLOTS_{k} == 0u) {{\n        return out;\n    }}");
    let _ = writeln!(s, "    let limit = select(u32({limit}), fixed_n, use_fixed);\n    let repeat_from = u32({repeat});");
    s.push_str("    var slot = 0u;\n    var count = 0u;\n    var n = 0u;\n    let cap = limit * 8u + 64u;\n    var steps = 0u;\n");
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
        "        if (!transform) {{\n            n += 1u;\n        }}\n        count += 1u;\n        if (count >= u.slots[slot + {slot_base}u].x) {{\n            count = 0u;\n            slot += 1u;\n            if (slot >= NSLOTS_{k}) {{\n                slot = repeat_from;\n            }}\n        }}\n\
         \x20       if (DIFS_{k}) {{\n            if (!transform) {{\n                let de_i = f32({rout} / {a0});\n                if (de_i < out.de) {{\n                    out.de = de_i;\n                    out.trap = f32({a2} * {a2});\n                }}\n            }}\n            continue;\n        }}\n\
         \x20       if (!transform) {{\n            let r2 = f32({x} * {x} + {y} * {y} + {z} * {z} + {w} * {w});\n            out.trap = min(out.trap, r2);\n            {rout} = fl(r2);\n\
         \x20           if (!use_fixed && !(r2 <= u.march.z)) {{\n                out.escaped = true;\n                if (!is_finite(r2)) {{\n                    {x} = prev_s.x;\n                    {y} = prev_s.y;\n                    {z} = prev_s.z;\n                    {w} = prev_s.w;\n                    {dr} = prev_dr;\n                }}\n                break;\n            }}\n        }}\n    }}\n",
        x = names[0],
        y = names[1],
        z = names[2],
        w = names[3],
        rout = names[STATE_ROUT],
        a0 = names[STATE_AUX],
        a2 = names[STATE_AUX + 2],
        dr = names[8]
    );
    let _ = writeln!(s, "    out.s = vec4<f32>(f32({}), f32({}), f32({}), f32({}));\n    out.dr = f32({});\n    out.n = n;\n    return out;\n}}", names[0], names[1], names[2], names[3], names[8]);
    s.push_str(&CHAIN_DE.replace("SCALE_K", if k == 1 { "u.scales.x" } else { "u.scales.y" }).replace("_K", &format!("_{k}")));
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
    let it = iterate_K(p, 0u, false);
    let r2 = dot(it.s, it.s);
    let r = sqrt(r2);
    var de = 0.0;
    if (DIFS_K) {
        de = max(it.de, 0.0);
    } else if (it.escaped) {
        if (NUMERIC_K) {
            var g = vec3<f32>(0.0);
            for (var k = 0u; k < 3u; k++) {
                var q = p;
                q[k] += h;
                let o2 = iterate_K(q, it.n, true);
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
    o.trap = it.trap;
    o.escaped = it.escaped || DIFS_K;
    return o;
}
"#;
