// SPDX-License-Identifier: LGPL-2.1-or-later

//! Compiles formula ASTs into closures over a flat `f64` register file.
//!
//! Register layout of `iter`: `[state (STATE_COUNT)] [params + init constants] [locals]`, then
//! for formulas with `setcount` the iteration counts of the hybrid's slots (`COUNT_SLOTS`,
//! by slot index; the hybrid loop fills them before and reads them after the call).
//! Register layout of `init`: `[params + init constants] [init locals]`.
//!
//! `iter` also compiles to native code (`super::jit`, Cranelift) on first use; the closures
//! stay as the fallback when the host is not supported, compilation fails or `RMB_NO_JIT=1`.

use std::sync::OnceLock;

use super::lang::{BinOp, COUNT_SLOTS, Expr, FormulaDef, Func, STATE_COUNT, Stmt, VarRef};

pub type NumFn = Box<dyn Fn(&[f64]) -> f64 + Send + Sync>;
pub type BoolFn = Box<dyn Fn(&[f64]) -> bool + Send + Sync>;
/// A compiled statement; returns the control flow: `NEXT`, `BREAK` or `CONTINUE`.
pub type StmtFn = Box<dyn Fn(&mut [f64]) -> u8 + Send + Sync>;

pub const NEXT: u8 = 0;
pub const BREAK: u8 = 1;
pub const CONTINUE: u8 = 2;

/// Register offsets of the three variable classes.
#[derive(Clone, Copy, Debug)]
pub(super) struct Layout {
    state: usize,
    param: usize,
    local: usize,
    /// First of the `COUNT_SLOTS` slot iteration counts (`setcount`).
    pub(super) count: usize,
}

impl Layout {
    pub(super) fn index(&self, v: VarRef) -> usize {
        match v {
            VarRef::State(i) => self.state + i,
            VarRef::Param(i) => self.param + i,
            VarRef::Local(i) => self.local + i,
        }
    }
}

/// A formula compiled for the CPU.
pub struct CpuFormula {
    /// `iter` as closures (the reference, and the fallback of the native code).
    closure: StmtFn,
    /// `iter` as native code, compiled on first use (None: not available).
    jit: OnceLock<Option<super::jit::JitFn>>,
    /// The `iter` block and its layout, kept for the native compilation.
    iter_ast: Vec<Stmt>,
    iter_layout: Layout,
    init: StmtFn,
    /// Registers needed by `iter`.
    pub iter_regs: usize,
    /// First register of the slot iteration counts (`setcount`; None: the formula sets none).
    pub count_regs: Option<usize>,
    n_params: usize,
    n_const: usize,
    init_regs: usize,
}

impl CpuFormula {
    pub fn compile(f: &FormulaDef) -> Self {
        let n_const = f.constant_count();
        let count = STATE_COUNT + n_const + f.locals.len();
        let iter_layout = Layout { state: 0, param: STATE_COUNT, local: STATE_COUNT + n_const, count };
        let init_layout = Layout { state: usize::MAX / 4, param: 0, local: n_const, count: usize::MAX / 4 };
        CpuFormula {
            closure: block(&f.iter, iter_layout),
            jit: OnceLock::new(),
            iter_ast: f.iter.clone(),
            iter_layout,
            init: block(&f.init, init_layout),
            iter_regs: count + if f.sets_counts { COUNT_SLOTS } else { 0 },
            count_regs: f.sets_counts.then_some(count),
            n_params: f.params.len(),
            n_const,
            init_regs: n_const + f.init_locals,
        }
    }

    /// Runs one iteration on the register file `r` (at least `iter_regs` long; the state,
    /// then the constants, then the locals). Returns `NEXT`, `BREAK` or `CONTINUE`.
    #[inline]
    pub fn iter(&self, r: &mut [f64]) -> u8 {
        match self.native() {
            // SAFETY: the code reads and writes registers below `iter_regs` only.
            Some(f) if r.len() >= self.iter_regs => unsafe { f(r.as_mut_ptr()) },
            _ => (self.closure)(r),
        }
    }

    /// [`Self::iter`] through the closures, never the native code.
    pub fn iter_closure(&self, r: &mut [f64]) -> u8 {
        (self.closure)(r)
    }

    /// The native code of `iter`, compiled on the first call (None when disabled by
    /// `RMB_NO_JIT`, not supported on this host or failed).
    pub fn native(&self) -> Option<super::jit::JitFn> {
        *self.jit.get_or_init(|| if super::jit::disabled() { None } else { super::jit::compile(&self.iter_ast, self.iter_layout, self.iter_regs) })
    }

    /// Parameters followed by the `init` constants computed from them.
    pub fn constants(&self, params: &[f64]) -> Vec<f64> {
        let mut regs = vec![0.0; self.init_regs];
        for (r, p) in regs.iter_mut().zip(params.iter().take(self.n_params)) {
            *r = *p;
        }
        let _ = (self.init)(&mut regs);
        regs.truncate(self.n_const);
        regs
    }
}

fn block(stmts: &[Stmt], l: Layout) -> StmtFn {
    let fns: Vec<StmtFn> = stmts.iter().map(|s| stmt(s, l)).collect();
    match fns.len() {
        0 => Box::new(|_| NEXT),
        1 => fns.into_iter().next().unwrap_or_else(|| Box::new(|_| NEXT)),
        _ => Box::new(move |r| {
            for f in &fns {
                let flow = f(r);
                if flow != NEXT {
                    return flow;
                }
            }
            NEXT
        }),
    }
}

fn stmt(s: &Stmt, l: Layout) -> StmtFn {
    match s {
        Stmt::Let(v, e) | Stmt::Assign(v, e) => {
            let k = l.index(*v);
            let e = num(e, l);
            Box::new(move |r| {
                let v = e(r);
                r[k] = v;
                NEXT
            })
        }
        Stmt::If(c, t, e) => {
            let c = cond(c, l);
            let t = block(t, l);
            if e.is_empty() {
                Box::new(move |r| if c(r) { t(r) } else { NEXT })
            } else {
                let e = block(e, l);
                Box::new(move |r| if c(r) { t(r) } else { e(r) })
            }
        }
        Stmt::For(v, a, b, body) => {
            let k = l.index(VarRef::Local(*v));
            let (a, b) = (*a, *b);
            let body = block(body, l);
            Box::new(move |r| {
                for n in a..b {
                    r[k] = n as f64;
                    if body(r) == BREAK {
                        break;
                    }
                }
                NEXT
            })
        }
        Stmt::Loop(body) => {
            let body = block(body, l);
            Box::new(move |r| {
                for _ in 0..super::lang::LOOP_CAP {
                    if body(r) == BREAK {
                        break;
                    }
                }
                NEXT
            })
        }
        Stmt::Break => Box::new(|_| BREAK),
        Stmt::Continue => Box::new(|_| CONTINUE),
        Stmt::SetCount(slot, n) => {
            let (slot, n, base) = (num(slot, l), num(n, l), l.count);
            Box::new(move |r| {
                let k = slot(r).round_ties_even();
                if (1.0..=COUNT_SLOTS as f64).contains(&k) {
                    r[base + k as usize - 1] = count_value(n(r));
                }
                NEXT
            })
        }
    }
}

/// The iteration count `setcount` stores: `clamp(round(n), 0, 1e9)` (NaN: 0).
pub fn count_value(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else { n.round_ties_even().clamp(0.0, 1.0e9) }
}

fn cond(e: &Expr, l: Layout) -> BoolFn {
    match e {
        Expr::Not(a) => {
            let a = cond(a, l);
            Box::new(move |r| !a(r))
        }
        Expr::Bin(op @ (BinOp::And | BinOp::Or), a, b) => {
            let (a, b) = (cond(a, l), cond(b, l));
            if *op == BinOp::And { Box::new(move |r| a(r) && b(r)) } else { Box::new(move |r| a(r) || b(r)) }
        }
        Expr::Bin(op, a, b) => {
            let (a, b) = (num(a, l), num(b, l));
            match op {
                BinOp::Lt => Box::new(move |r| a(r) < b(r)),
                BinOp::Le => Box::new(move |r| a(r) <= b(r)),
                BinOp::Gt => Box::new(move |r| a(r) > b(r)),
                BinOp::Ge => Box::new(move |r| a(r) >= b(r)),
                BinOp::Eq => Box::new(move |r| a(r) == b(r)),
                _ => Box::new(move |r| a(r) != b(r)),
            }
        }
        _ => Box::new(|_| false),
    }
}

fn num(e: &Expr, l: Layout) -> NumFn {
    match e {
        Expr::Num(v) => {
            let v = *v;
            Box::new(move |_| v)
        }
        Expr::Var(v) => {
            let k = l.index(*v);
            Box::new(move |r| r[k])
        }
        Expr::Neg(a) => {
            let a = num(a, l);
            Box::new(move |r| -a(r))
        }
        Expr::Bin(op, a, b) => {
            // Common shapes without the extra closure call.
            if let (Expr::Var(va), Expr::Var(vb)) = (a.as_ref(), b.as_ref()) {
                let (ka, kb) = (l.index(*va), l.index(*vb));
                match op {
                    BinOp::Add => return Box::new(move |r| r[ka] + r[kb]),
                    BinOp::Sub => return Box::new(move |r| r[ka] - r[kb]),
                    BinOp::Mul => return Box::new(move |r| r[ka] * r[kb]),
                    _ => {}
                }
            }
            let (a, b) = (num(a, l), num(b, l));
            match op {
                BinOp::Add => Box::new(move |r| a(r) + b(r)),
                BinOp::Sub => Box::new(move |r| a(r) - b(r)),
                BinOp::Mul => Box::new(move |r| a(r) * b(r)),
                BinOp::Div => Box::new(move |r| a(r) / b(r)),
                _ => Box::new(move |r| a(r) % b(r)),
            }
        }
        Expr::Call(f, args) => call(*f, args, l),
        Expr::Not(_) => Box::new(|_| 0.0),
    }
}

/// `pow` as the GPU computes it: undefined (NaN) for negative bases.
pub fn gpu_pow(a: f64, b: f64) -> f64 {
    if a < 0.0 { f64::NAN } else { a.powf(b) }
}

pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn call(f: Func, args: &[Expr], l: Layout) -> NumFn {
    use Func::*;
    if matches!(f, MapXY | MapSphere) {
        let a: Vec<NumFn> = args.iter().map(|e| num(e, l)).collect();
        return if f == MapXY {
            Box::new(move |r| crate::maps::sample_xy(a[0](r), a[1](r), a[2](r), a[3](r)))
        } else {
            Box::new(move |r| crate::maps::sample_sphere(a[0](r), a[1](r), a[2](r), a[3](r), a[4](r)))
        };
    }
    if f == Select {
        let c = cond(&args[0], l);
        let (a, b) = (num(&args[1], l), num(&args[2], l));
        return Box::new(move |r| if c(r) { a(r) } else { b(r) });
    }
    let mut a: Vec<NumFn> = args.iter().map(|e| num(e, l)).collect();
    if a.len() == 1 {
        let x = a.pop().unwrap_or_else(|| Box::new(|_| 0.0));
        let g: fn(f64) -> f64 = match f {
            Abs => f64::abs,
            Sqrt => f64::sqrt,
            InvSqrt => |v| 1.0 / v.sqrt(),
            Sin => f64::sin,
            Cos => f64::cos,
            Tan => f64::tan,
            Asin => f64::asin,
            Acos => f64::acos,
            Atan => f64::atan,
            Exp => f64::exp,
            Log => f64::ln,
            Floor => f64::floor,
            Ceil => f64::ceil,
            Fract => |v| v - v.floor(),
            Round => f64::round_ties_even,
            Sign => |v| if v > 0.0 { 1.0 } else if v < 0.0 { -1.0 } else { 0.0 },
            Sinh => f64::sinh,
            Cosh => f64::cosh,
            Tanh => f64::tanh,
            Sqr => |v| v * v,
            Single => |v| v as f32 as f64,
            _ => |v| v,
        };
        return Box::new(move |r| g(x(r)));
    }
    if a.len() == 2 {
        let y = a.pop().unwrap_or_else(|| Box::new(|_| 0.0));
        let x = a.pop().unwrap_or_else(|| Box::new(|_| 0.0));
        let g: fn(f64, f64) -> f64 = match f {
            Atan2 => f64::atan2,
            Pow => gpu_pow,
            Min => f64::min,
            Max => f64::max,
            Step => |e, v| if v < e { 0.0 } else { 1.0 },
            _ => |v, _| v,
        };
        return Box::new(move |r| g(x(r), y(r)));
    }
    let z = a.pop().unwrap_or_else(|| Box::new(|_| 0.0));
    let y = a.pop().unwrap_or_else(|| Box::new(|_| 0.0));
    let x = a.pop().unwrap_or_else(|| Box::new(|_| 0.0));
    let g: fn(f64, f64, f64) -> f64 = match f {
        Clamp => |v, lo, hi| v.max(lo).min(hi),
        Mix => |a, b, t| a + (b - a) * t,
        _ => smoothstep,
    };
    Box::new(move |r| g(x(r), y(r), z(r)))
}
