// SPDX-License-Identifier: LGPL-2.1-or-later

//! Generates the WGSL of a hybrid formula chain (the `iterate` function of march.wgsl).
//!
//! The shader depends only on the formulas of the slots and the DE variant; iteration counts,
//! the repeat slot and all parameter values come from buffers.

use std::fmt::Write;

use rmb_core::formula::DeKind;
use rmb_core::formula::lang::{STATE_AUX, STATE_COUNT, STATE_PX, STATE_ROLD, STATE_ROUT};
use rmb_core::formula::wgsl::{PRELUDE, emit_iter, state_name};
use rmb_core::fractal::Program;

const TEMPLATE: &str = include_str!("march.wgsl");

/// The complete ray marching shader for `prog`.
pub fn march_shader(prog: &Program) -> String {
    TEMPLATE.replace("//@ITERATE@", &iterate_fn(prog))
}

fn iterate_fn(prog: &Program) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "const NSLOTS: u32 = {}u;", prog.slots.len());
    let _ = writeln!(s, "const NUMERIC: bool = {};", prog.numeric);
    let _ = writeln!(s, "const DE_LINEAR: bool = {};", prog.de_kind == DeKind::Linear);
    s.push_str(PRELUDE);
    s.push_str(
        "\nfn iterate(p: vec3<f32>, fixed_n: u32, use_fixed: bool) -> Iterated {\n\
         \x20   var out: Iterated;\n\
         \x20   out.trap = 3.0e38;\n\
         \x20   out.escaped = false;\n",
    );
    let names: Vec<String> = (0..STATE_COUNT).map(state_name).collect();
    let _ = writeln!(s, "    var {}: fl = p.x;\n    var {}: fl = p.y;\n    var {}: fl = p.z;\n    var {}: fl = u.frac.x;", names[0], names[1], names[2], names[3]);
    let _ = writeln!(s, "    var {}: fl = p.x;\n    var {}: fl = p.y;\n    var {}: fl = p.z;\n    var {}: fl = u.frac.x;", names[4], names[5], names[6], names[7]);
    let _ = writeln!(s, "    if (u.frac.y > 0.5) {{\n        {} = u.julia.x;\n        {} = u.julia.y;\n        {} = u.julia.z;\n        {} = u.julia.w;\n    }}", names[4], names[5], names[6], names[7]);
    let _ = writeln!(s, "    var {}: fl = 1.0;\n    var {}: fl = 0.0;", names[8], names[9]);
    let _ = writeln!(s, "    var {}: fl = dot(vec4<fl>(p, u.frac.x), vec4<fl>(p, u.frac.x));\n    var {}: fl = 0.0;", names[STATE_ROUT], names[STATE_ROLD]);
    let _ = writeln!(s, "    let {} = fl(p.x);\n    let {} = fl(p.y);\n    let {} = fl(p.z);", names[STATE_PX], names[STATE_PX + 1], names[STATE_PX + 2]);
    for name in &names[STATE_AUX..STATE_COUNT] {
        let _ = writeln!(s, "    var {name}: fl = 0.0;");
    }
    s.push_str(
        "    if (NSLOTS == 0u) {\n        return out;\n    }\n\
         \x20   let limit = select(u32(u.frac.z), fixed_n, use_fixed);\n\
         \x20   let repeat_from = u32(u.frac.w);\n\
         \x20   var slot = 0u;\n    var count = 0u;\n    var n = 0u;\n\
         \x20   let cap = limit * 8u + 64u;\n    var steps = 0u;\n\
         \x20   loop {\n        if (n >= limit || steps >= cap) {\n            break;\n        }\n        steps += 1u;\n",
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
        "        if (!transform) {{\n            n += 1u;\n        }}\n        count += 1u;\n        if (count >= u.slots[slot].x) {{\n            count = 0u;\n            slot += 1u;\n            if (slot >= NSLOTS) {{\n                slot = repeat_from;\n            }}\n        }}\n\
         \x20       if (!transform) {{\n            let r2 = f32({x} * {x} + {y} * {y} + {z} * {z} + {w} * {w});\n            out.trap = min(out.trap, r2);\n            {rout} = fl(r2);\n\
         \x20           if (!use_fixed && !(r2 <= u.march.z)) {{\n                out.escaped = true;\n                break;\n            }}\n        }}\n    }}\n",
        x = names[0],
        y = names[1],
        z = names[2],
        w = names[3],
        rout = names[STATE_ROUT]
    );
    let _ = writeln!(s, "    out.s = vec4<f32>(f32({}), f32({}), f32({}), f32({}));\n    out.dr = f32({});\n    out.n = n;\n    return out;\n}}", names[0], names[1], names[2], names[3], names[8]);
    s
}
