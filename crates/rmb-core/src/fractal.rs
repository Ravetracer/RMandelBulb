// SPDX-License-Identifier: LGPL-2.1-or-later

//! The hybrid formula chain prepared for rendering, and its CPU evaluation.
//!
//! A `Program` is what both renderers iterate: the active slots with their formulas and
//! constants. The GPU renderer generates its shader from `signature()`-equal programs once and
//! uploads `constants` whenever parameters change. With a DE combination the program has a
//! second part (its own slots, iteration limit and estimate); the two estimates are combined.

use std::sync::Arc;

use crate::formula::lang::{STATE_AUX, STATE_COUNT, STATE_DR, STATE_ITER, STATE_JULIA, STATE_PX, STATE_ROLD, STATE_ROUT, uses_state};
use crate::formula::{DeKind, Formula};
use crate::math::V3;
use crate::scene::{CombineMode, Cutting, DeMode, Fractal, Slot};

pub struct ProgramSlot {
    pub formula: Arc<Formula>,
    pub iterations: u32,
    /// Offset of this slot's constants in `Program::constants`.
    pub offset: usize,
    pub transform: bool,
}

/// Value of a constant expression over a slot's parameters and `init` constants.
fn const_value(e: &crate::formula::lang::Expr, consts: &[f64]) -> Option<f64> {
    use crate::formula::lang::{BinOp, Expr, Func, VarRef};
    Some(match e {
        Expr::Num(v) => *v,
        Expr::Var(VarRef::Param(k)) => *consts.get(*k)?,
        Expr::Neg(a) => -const_value(a, consts)?,
        Expr::Bin(op, a, b) => {
            let (a, b) = (const_value(a, consts)?, const_value(b, consts)?);
            match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => a / b,
                _ => return None,
            }
        }
        Expr::Call(Func::Round, a) => const_value(&a[0], consts)?.round(),
        Expr::Call(Func::Floor, a) => const_value(&a[0], consts)?.floor(),
        _ => return None,
    })
}

pub struct Program {
    pub slots: Vec<ProgramSlot>,
    /// Slot the chain continues with after the last one (index into `slots`).
    pub repeat_from: usize,
    pub max_iterations: u32,
    pub bailout2: f64,
    pub julia: bool,
    pub julia_c: [f64; 4],
    pub w_slice: f64,
    pub de_kind: DeKind,
    pub numeric: bool,
    /// Distance-estimated IFS hybrid (any dIFS formula in it).
    pub difs: bool,
    /// DE multiplier of this part's formulas (weighted by their iteration counts).
    pub de_scale: f64,
    /// DE multiplier of the scene (applied once, after combining).
    pub scene_de_scale: f64,
    /// Parameters and `init` constants of all slots (both parts), concatenated.
    pub constants: Vec<f64>,
    /// Cutting planes: (axis, position, keep_above).
    pub cuts: Vec<(usize, f64, bool)>,
    /// Formulas of the scene that are not in the library.
    pub missing: Vec<String>,
    /// Start point rotation into 4D (rows of a 4x4 matrix), if any formula uses w.
    pub rotation_4d: Option<[[f64; 4]; 4]>,
    /// Second chain of a DE combination.
    pub part2: Option<Box<Program>>,
    pub combine_mode: CombineMode,
    pub combine_smooth: f64,
    /// Inside rendering (the estimate is inverted around the surface threshold).
    pub inside: bool,
}

/// Result of iterating one point.
#[derive(Clone, Copy, Debug, Default)]
pub struct Orbit {
    pub de: f64,
    pub iterations: u32,
    pub r2: f64,
    /// Squared radius before the last iteration.
    pub r2_old: f64,
    pub dr: f64,
    /// Smallest squared radius along the orbit.
    pub trap: f64,
    pub escaped: bool,
}

/// Mandelbulb3D's 4D start rotation: Z(Y(X)) with X in the xw, Y in the yw, Z in the zw plane.
fn rotation_matrix_4d(deg: [f64; 3]) -> [[f64; 4]; 4] {
    let mut m = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    for (axis, a) in deg.iter().enumerate() {
        let (s, c) = a.to_radians().sin_cos();
        let mut r = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        r[axis][axis] = c;
        r[3][3] = c;
        r[axis][3] = -s;
        r[3][axis] = s;
        let mut out = [[0.0; 4]; 4];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..4).map(|k| r[i][k] * m[k][j]).sum();
            }
        }
        m = out;
    }
    m
}

impl Program {
    /// Numbers of the Mandelbulb3D maps the formulas read (both chains). A map number that is
    /// not a constant expression counts every whole parameter value of its slot.
    pub fn map_numbers(&self) -> Vec<i32> {
        let mut out = Vec::new();
        for s in self.slots.iter().chain(self.part2.iter().flat_map(|p| p.slots.iter())) {
            let args = crate::formula::lang::map_number_args(&s.formula.def.iter);
            if args.is_empty() {
                continue;
            }
            let n = s.formula.def.constant_count();
            let consts = &self.constants[s.offset.min(self.constants.len())..(s.offset + n).min(self.constants.len())];
            for a in &args {
                match const_value(a, consts) {
                    Some(v) => out.push(v.round() as i32),
                    None => out.extend(consts.iter().filter(|v| v.fract() == 0.0 && (0.0..100_000.0).contains(*v)).map(|v| *v as i32)),
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn new(f: &Fractal, cut: &Cutting) -> Program {
        let split = f.combine.as_ref().map(|c| c.start.clamp(1, f.slots.len().max(1))).filter(|s| *s < f.slots.len());
        let end1 = split.unwrap_or(f.slots.len());
        let mut p = Self::build(f, &f.slots[..end1], 0, f.repeat_from, f.max_iterations, 0);
        if let (Some(start), Some(c)) = (split, f.combine.as_ref()) {
            let mut p2 = Self::build(f, &f.slots[start..], start, c.repeat_from, c.max_iterations, p.constants.len());
            p.constants.append(&mut p2.constants);
            p.missing.append(&mut p2.missing);
            p.combine_mode = c.mode;
            p.combine_smooth = c.smooth;
            if !p2.slots.is_empty() {
                p.part2 = Some(Box::new(p2));
            }
        }
        p.cuts = cut.planes().iter().enumerate().filter(|(_, pl)| pl.enabled).map(|(a, pl)| (a, pl.position, pl.keep_above)).collect();
        let uses_w = |prog: &Program| prog.slots.iter().any(|s| uses_state(&s.formula.def.iter, 3));
        if f.rotation_4d.iter().any(|a| *a != 0.0) && (uses_w(&p) || p.part2.as_deref().is_some_and(uses_w)) {
            p.rotation_4d = Some(rotation_matrix_4d(f.rotation_4d));
        }
        p
    }

    /// One chain: `slots` start at absolute slot index `first`; `repeat_from` is absolute.
    fn build(f: &Fractal, slots_in: &[Slot], first: usize, repeat_abs: usize, max_iterations: u32, const_base: usize) -> Program {
        let mut slots = Vec::new();
        let mut constants = Vec::new();
        let mut missing = Vec::new();
        let mut repeat_from = None;
        for (i, s) in slots_in.iter().enumerate() {
            if s.iterations == 0 {
                continue;
            }
            let Some(formula) = crate::formula::find(&s.formula) else {
                missing.push(s.formula.clone());
                continue;
            };
            if first + i >= repeat_abs && repeat_from.is_none() {
                repeat_from = Some(slots.len());
            }
            let values: Vec<f64> = formula.def.params.iter().map(|p| s.value(p)).collect();
            let offset = const_base + constants.len();
            constants.extend(formula.cpu.constants(&values));
            slots.push(ProgramSlot { transform: formula.def.transform, formula, iterations: s.iterations, offset });
        }
        let difs = slots.iter().any(|s| s.formula.def.de == DeKind::Difs);
        let kinds: Vec<DeKind> = slots.iter().filter(|s| !s.transform && s.formula.def.de != DeKind::Difs).map(|s| s.formula.def.de).collect();
        let auto_kind = if kinds.iter().all(|k| *k == DeKind::Log) && !kinds.is_empty() { DeKind::Log } else { DeKind::Linear };
        let mixed = kinds.iter().any(|k| *k != kinds[0]) || kinds.contains(&DeKind::Numeric);
        let (de_kind, numeric) = match f.de_mode {
            DeMode::Auto => (if kinds.contains(&DeKind::Numeric) { DeKind::Log } else { auto_kind }, mixed),
            DeMode::Log => (DeKind::Log, false),
            DeMode::Linear => (DeKind::Linear, false),
            DeMode::NumericLog => (DeKind::Log, true),
            DeMode::NumericLinear => (DeKind::Linear, true),
        };
        // Per-formula DE scales, weighted by their iteration counts (as Mandelbulb3D does).
        let (mut wsum, mut wn) = (0.0, 0.0);
        for s in slots.iter().filter(|s| !s.transform) {
            wsum += s.formula.def.de_scale * s.iterations as f64;
            wn += s.iterations as f64;
        }
        Program {
            repeat_from: repeat_from.unwrap_or(0).min(slots.len().saturating_sub(1)),
            slots,
            max_iterations: max_iterations.max(1),
            bailout2: f.bailout * f.bailout,
            julia: f.julia,
            julia_c: f.julia_c,
            w_slice: f.w_slice,
            de_kind,
            numeric: numeric && !difs,
            difs,
            de_scale: if wn > 0.0 { wsum / wn } else { 1.0 },
            scene_de_scale: f.de_scale,
            constants,
            cuts: Vec::new(),
            missing,
            rotation_4d: None,
            part2: None,
            combine_mode: CombineMode::Min,
            combine_smooth: 0.0,
            inside: f.inside,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Identifies the generated shader: formulas per slot and the DE variant of each part, and
    /// whether the 4D rotation is used. Parameter values are not part of it.
    pub fn signature(&self) -> String {
        let part = |p: &Program| -> String {
            let mut s: String = p.slots.iter().map(|sl| format!("{}|{}|", sl.formula.def.name, sl.formula.source)).collect();
            s.push_str(&format!("{:?}{}{}", p.de_kind, p.numeric, p.difs));
            s
        };
        let mut s = part(self);
        if let Some(p2) = &self.part2 {
            s.push_str("||");
            s.push_str(&part(p2));
        }
        s.push_str(if self.rotation_4d.is_some() { "|r4" } else { "" });
        s
    }

    pub fn evaluator(&self) -> Evaluator<'_> {
        let regs = |p: &Program| -> Vec<Vec<f64>> {
            p.slots
                .iter()
                .map(|s| {
                    let mut r = vec![0.0; s.formula.cpu.iter_regs];
                    let n = s.formula.def.constant_count();
                    r[STATE_COUNT..STATE_COUNT + n].copy_from_slice(&self.constants[s.offset..s.offset + n]);
                    r
                })
                .collect()
        };
        Evaluator { prog: self, regs: regs(self), regs2: self.part2.as_deref().map(regs).unwrap_or_default() }
    }
}

/// Per-thread evaluation state (register files of the slots).
pub struct Evaluator<'a> {
    prog: &'a Program,
    regs: Vec<Vec<f64>>,
    regs2: Vec<Vec<f64>>,
}

struct Iterated {
    state: [f64; 4],
    rold: f64,
    dr: f64,
    n: u32,
    trap: f64,
    escaped: bool,
    /// dIFS: smallest distance over the iterations.
    difs_de: f64,
}

/// Iterates `p` through the chain `prog`. `top` holds the shared settings (Julia, 4D rotation).
fn iterate(top: &Program, prog: &Program, regs: &mut [Vec<f64>], p: V3, fixed: Option<u32>) -> Iterated {
    let mut st = [0.0; STATE_COUNT];
    let mut v = [p.x, p.y, p.z, top.w_slice];
    if let Some(m) = &top.rotation_4d {
        let src = [p.x, p.y, p.z, 0.0];
        for (i, row) in m.iter().enumerate() {
            v[i] = (0..4).map(|k| row[k] * src[k]).sum();
        }
    }
    st[..9].copy_from_slice(&[v[0], v[1], v[2], v[3], v[0], v[1], v[2], v[3], 1.0]);
    if top.julia {
        st[4..8].copy_from_slice(&top.julia_c);
    }
    st[STATE_ROUT] = v.iter().map(|x| x * x).sum();
    st[STATE_PX..STATE_PX + 3].copy_from_slice(&[p.x, p.y, p.z]);
    debug_assert_eq!(STATE_AUX + 9, STATE_COUNT);
    st[STATE_JULIA] = if top.julia { 1.0 } else { 0.0 };
    let mut out = Iterated { state: [0.0; 4], rold: 0.0, dr: 1.0, n: 0, trap: f64::MAX, escaped: false, difs_de: 65535.0 };
    if prog.difs {
        // VaryScale starts at 1; the shapes' distances are divided by it.
        st[STATE_AUX] = 1.0;
    }
    if prog.slots.is_empty() {
        return out;
    }
    let limit = fixed.unwrap_or(prog.max_iterations);
    let (mut slot, mut count) = (0usize, 0u32);
    // Transforms do not count as iterations; the step cap ends chains of only transforms.
    let cap = limit.saturating_mul(8).saturating_add(64);
    let mut steps = 0u32;
    while out.n < limit && steps < cap {
        steps += 1;
        let s = &prog.slots[slot];
        let regs = &mut regs[slot];
        let prev = st;
        st[STATE_ROLD] = st[STATE_ROUT];
        st[STATE_ITER] = out.n as f64;
        regs[..STATE_COUNT].copy_from_slice(&st);
        (s.formula.cpu.iter)(regs);
        st.copy_from_slice(&regs[..STATE_COUNT]);
        if !s.transform {
            out.n += 1;
        }
        count += 1;
        if count >= s.iterations {
            count = 0;
            slot += 1;
            if slot >= prog.slots.len() {
                slot = prog.repeat_from;
            }
        }
        if prog.difs {
            if !s.transform {
                let de = st[STATE_ROUT] / st[STATE_AUX];
                if de < out.difs_de {
                    out.difs_de = de;
                    // Coloring: the value the shape left in Dfree1 (a2).
                    out.trap = st[STATE_AUX + 2] * st[STATE_AUX + 2];
                }
            }
            continue;
        }
        if !s.transform {
            let r2 = st[0] * st[0] + st[1] * st[1] + st[2] * st[2] + st[3] * st[3];
            out.trap = out.trap.min(r2);
            st[STATE_ROUT] = r2;
            // NaN counts as escaped (it never comes back): `inside` is false for NaN.
            let inside = r2 <= prog.bailout2;
            if fixed.is_none() && !inside {
                out.escaped = true;
                // Overflow in the last step: the estimate uses the state before it.
                if !r2.is_finite() {
                    st = prev;
                }
                break;
            }
        }
    }
    out.state.copy_from_slice(&st[..4]);
    out.rold = st[STATE_ROLD];
    out.dr = st[STATE_DR];
    out
}

/// Distance estimate of one chain (before the scene's DE scale and the cutting planes).
fn chain_orbit(top: &Program, prog: &Program, regs: &mut [Vec<f64>], p: V3, h: f64) -> Orbit {
    if prog.slots.is_empty() {
        return Orbit { de: 1.0e10, ..Orbit::default() };
    }
    let it = iterate(top, prog, regs, p, None);
    let r2 = it.state.iter().map(|v| v * v).sum::<f64>();
    let r = r2.sqrt();
    let mut de = if prog.difs {
        it.difs_de.max(0.0)
    } else if !it.escaped {
        0.0
    } else if prog.numeric {
        let mut g = [0.0; 3];
        for (k, gk) in g.iter_mut().enumerate() {
            let mut q = p;
            match k {
                0 => q.x += h,
                1 => q.y += h,
                _ => q.z += h,
            }
            let o = iterate(top, prog, regs, q, Some(it.n));
            *gk = (o.state.iter().map(|v| v * v).sum::<f64>().sqrt() - r) / h;
        }
        let gl = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt().max(1e-30);
        match prog.de_kind {
            DeKind::Linear => r / gl,
            _ => 0.5 * r * r.ln() / gl,
        }
    } else {
        match prog.de_kind {
            DeKind::Linear => r / it.dr.abs(),
            _ => 0.5 * r.ln() * r / it.dr,
        }
    };
    if !de.is_finite() {
        de = if it.escaped { 1.0e-3 } else { 0.0 };
    }
    Orbit { de: de * prog.de_scale, iterations: it.n, r2, r2_old: it.rold, dr: it.dr, trap: it.trap, escaped: it.escaped || prog.difs }
}

/// Combines the estimates of the two chains (Mandelbulb3D's DE combination).
pub fn combine(mode: CombineMode, smooth: f64, a: Orbit, b: Orbit) -> Orbit {
    let s = smooth.max(1e-12);
    match mode {
        CombineMode::Min => {
            if b.de < a.de {
                b
            } else {
                a
            }
        }
        CombineMode::Max => {
            if b.de > a.de {
                b
            } else {
                a
            }
        }
        // Inside the second chain counts as outside: its estimate becomes the distance into it.
        CombineMode::Subtract => {
            let inv = if b.de <= 0.0 { s } else { -b.de };
            if inv > a.de { Orbit { de: inv, ..a } } else { a }
        }
        CombineMode::SmoothLinear => {
            let de = (b.de - (s - a.de).max(0.0)).min(a.de - (s - b.de).max(0.0));
            if b.de < a.de { Orbit { de, ..b } } else { Orbit { de, ..a } }
        }
        CombineMode::Smooth => {
            let x = (s - a.de).max(0.0);
            let y = (s - b.de).max(0.0);
            let de = (b.de - x * (s - y) / s).min(a.de - y * (s - x) / s);
            if b.de < a.de { Orbit { de, ..b } } else { Orbit { de, ..a } }
        }
    }
}

impl Evaluator<'_> {
    /// Iterates `p` and estimates its distance to the surface. `h` is the offset of the
    /// numerical gradient (about the surface threshold).
    pub fn orbit(&mut self, p: V3, h: f64) -> Orbit {
        let prog = self.prog;
        let mut o = chain_orbit(prog, prog, &mut self.regs, p, h);
        if let Some(p2) = prog.part2.as_deref() {
            let o2 = chain_orbit(prog, p2, &mut self.regs2, p, h);
            o = combine(prog.combine_mode, prog.combine_smooth, o, o2);
        }
        let mut de = (o.de * prog.scene_de_scale).max(0.0);
        if prog.inside {
            // Mandelbulb3D's inside rendering: points in the set step on with four times the
            // surface threshold (h is half of it); outside, the estimate falls below the
            // threshold one threshold beyond the boundary.
            let s = 2.0 * h;
            de = if prog.difs { 2.0 * s - de } else { 4.0 * s - 3.0 * de }.max(0.0);
        }
        for &(axis, pos, above) in &prog.cuts {
            let v = [p.x, p.y, p.z][axis];
            de = de.max(if above { pos - v } else { v - pos });
        }
        o.de = de;
        o
    }

    pub fn de(&mut self, p: V3, h: f64) -> f64 {
        self.orbit(p, h).de
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::v3;
    use crate::scene::Scene;

    #[test]
    fn mandelbulb_distance_is_plausible() {
        let s = Scene::default();
        let prog = Program::new(&s.fractal, &s.cut);
        assert!(prog.missing.is_empty());
        let mut ev = prog.evaluator();
        // Far away: roughly the distance to the bulb (radius about 1.1).
        let d = ev.de(v3(3.0, 0.0, 0.0), 1e-5);
        assert!(d > 1.0 && d < 2.2, "{d}");
        // The origin is inside.
        assert_eq!(ev.de(V3::ZERO, 1e-5), 0.0);
    }

    #[test]
    fn all_builtin_formulas_evaluate() {
        let lib = crate::formula::library();
        for f in lib.all() {
            let mut s = Scene::default();
            s.fractal.slots = vec![crate::scene::Slot::new(&f.def.name)];
            if f.def.transform {
                s.fractal.slots.push(crate::scene::Slot::new("Mandelbulb"));
            }
            let prog = Program::new(&s.fractal, &s.cut);
            let mut ev = prog.evaluator();
            let d = ev.de(v3(2.5, 0.3, 0.2), 1e-5);
            assert!(d.is_finite() && d >= 0.0, "{}: {d}", f.def.name);
        }
    }
}
