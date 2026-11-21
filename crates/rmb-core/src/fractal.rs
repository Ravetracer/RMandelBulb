// SPDX-License-Identifier: LGPL-2.1-or-later

//! The hybrid formula chain prepared for rendering, and its CPU evaluation.
//!
//! A `Program` is what both renderers iterate: the active slots with their formulas and
//! constants. The GPU renderer generates its shader from `signature()`-equal programs once and
//! uploads `constants` whenever parameters change.

use std::sync::Arc;

use crate::formula::lang::{STATE_AUX, STATE_COUNT, STATE_DR, STATE_ITER, STATE_PX, STATE_ROLD, STATE_ROUT};
use crate::formula::{DeKind, Formula};
use crate::math::V3;
use crate::scene::{Cutting, DeMode, Fractal};

pub struct ProgramSlot {
    pub formula: Arc<Formula>,
    pub iterations: u32,
    /// Offset of this slot's constants in `Program::constants`.
    pub offset: usize,
    pub transform: bool,
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
    pub de_scale: f64,
    /// Parameters and `init` constants of all slots, concatenated.
    pub constants: Vec<f64>,
    /// Cutting planes: (axis, position, keep_above).
    pub cuts: Vec<(usize, f64, bool)>,
    /// Formulas of the scene that are not in the library.
    pub missing: Vec<String>,
}

/// Result of iterating one point.
#[derive(Clone, Copy, Debug, Default)]
pub struct Orbit {
    pub de: f64,
    pub iterations: u32,
    pub r2: f64,
    pub dr: f64,
    /// Smallest squared radius along the orbit.
    pub trap: f64,
    pub escaped: bool,
}

impl Program {
    pub fn new(f: &Fractal, cut: &Cutting) -> Program {
        let mut slots = Vec::new();
        let mut constants = Vec::new();
        let mut missing = Vec::new();
        let mut repeat_from = None;
        for (i, s) in f.slots.iter().enumerate() {
            if s.iterations == 0 {
                continue;
            }
            let Some(formula) = crate::formula::find(&s.formula) else {
                missing.push(s.formula.clone());
                continue;
            };
            if i >= f.repeat_from && repeat_from.is_none() {
                repeat_from = Some(slots.len());
            }
            let values: Vec<f64> = formula.def.params.iter().map(|p| s.value(p)).collect();
            let offset = constants.len();
            constants.extend(formula.cpu.constants(&values));
            slots.push(ProgramSlot { transform: formula.def.transform, formula, iterations: s.iterations, offset });
        }
        let kinds: Vec<DeKind> = slots.iter().filter(|s| !s.transform).map(|s| s.formula.def.de).collect();
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
        let formula_scale = if wn > 0.0 { wsum / wn } else { 1.0 };
        let cuts = cut.planes().iter().enumerate().filter(|(_, p)| p.enabled).map(|(a, p)| (a, p.position, p.keep_above)).collect();
        Program {
            repeat_from: repeat_from.unwrap_or(0).min(slots.len().saturating_sub(1)),
            slots,
            max_iterations: f.max_iterations.max(1),
            bailout2: f.bailout * f.bailout,
            julia: f.julia,
            julia_c: f.julia_c,
            w_slice: f.w_slice,
            de_kind,
            numeric,
            de_scale: f.de_scale * formula_scale,
            constants,
            cuts,
            missing,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Identifies the generated shader: formulas per slot and the DE variant. Parameter values
    /// are not part of it.
    pub fn signature(&self) -> String {
        let mut s: String = self.slots.iter().map(|sl| format!("{}|{}|", sl.formula.def.name, sl.formula.source)).collect();
        s.push_str(&format!("{:?}{}", self.de_kind, self.numeric));
        s
    }

    pub fn evaluator(&self) -> Evaluator<'_> {
        let regs = self
            .slots
            .iter()
            .map(|s| {
                let mut r = vec![0.0; s.formula.cpu.iter_regs];
                let n = s.formula.def.constant_count();
                r[STATE_COUNT..STATE_COUNT + n].copy_from_slice(&self.constants[s.offset..s.offset + n]);
                r
            })
            .collect();
        Evaluator { prog: self, regs }
    }
}

/// Per-thread evaluation state (register files of the slots).
pub struct Evaluator<'a> {
    prog: &'a Program,
    regs: Vec<Vec<f64>>,
}

struct Iterated {
    state: [f64; 4],
    dr: f64,
    n: u32,
    trap: f64,
    escaped: bool,
}

impl Evaluator<'_> {
    fn iterate(&mut self, p: V3, fixed: Option<u32>) -> Iterated {
        let prog = self.prog;
        let mut st = [0.0; STATE_COUNT];
        st[..9].copy_from_slice(&[p.x, p.y, p.z, prog.w_slice, p.x, p.y, p.z, prog.w_slice, 1.0]);
        if prog.julia {
            st[4..8].copy_from_slice(&prog.julia_c);
        }
        st[STATE_ROUT] = p.x * p.x + p.y * p.y + p.z * p.z + prog.w_slice * prog.w_slice;
        st[STATE_PX..STATE_PX + 3].copy_from_slice(&[p.x, p.y, p.z]);
        debug_assert_eq!(STATE_AUX + 8, STATE_COUNT);
        let mut out = Iterated { state: [0.0; 4], dr: 1.0, n: 0, trap: f64::MAX, escaped: false };
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
            let regs = &mut self.regs[slot];
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
            if !s.transform {
                let r2 = st[0] * st[0] + st[1] * st[1] + st[2] * st[2] + st[3] * st[3];
                out.trap = out.trap.min(r2);
                st[STATE_ROUT] = r2;
                // NaN counts as escaped (it never comes back): `inside` is false for NaN.
                let inside = r2 <= prog.bailout2;
                if fixed.is_none() && !inside {
                    out.escaped = true;
                    break;
                }
            }
        }
        out.state.copy_from_slice(&st[..4]);
        out.dr = st[STATE_DR];
        out
    }

    /// Iterates `p` and estimates its distance to the surface. `h` is the offset of the
    /// numerical gradient (about the surface threshold).
    pub fn orbit(&mut self, p: V3, h: f64) -> Orbit {
        let prog = self.prog;
        if prog.slots.is_empty() {
            return Orbit { de: 1.0e10, ..Orbit::default() };
        }
        let it = self.iterate(p, None);
        let r2 = it.state.iter().map(|v| v * v).sum::<f64>();
        let r = r2.sqrt();
        let mut de = if !it.escaped {
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
                let o = self.iterate(q, Some(it.n));
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
        de = (de * prog.de_scale).max(0.0);
        for &(axis, pos, above) in &prog.cuts {
            let v = [p.x, p.y, p.z][axis];
            de = de.max(if above { pos - v } else { v - pos });
        }
        Orbit { de, iterations: it.n, r2, dr: it.dr, trap: it.trap, escaped: it.escaped }
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
