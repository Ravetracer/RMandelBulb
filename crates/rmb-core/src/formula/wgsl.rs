// SPDX-License-Identifier: LGPL-2.1-or-later

//! WGSL code generation for formulas.
//!
//! The generated code uses the scalar type alias `fl` (f32, or f64 on adapters with 64-bit
//! float support), state variables `s_x … s_dr, s_i` and reads the formula constants from
//! `prm[]` at a fixed offset, so parameter changes never need a shader rebuild.

use std::fmt::Write;

use super::lang::{BinOp, Expr, FormulaDef, Func, STATE_NAMES, Stmt, VarRef};

/// Helper functions the generated code may call.
pub const PRELUDE: &str = r#"
fn sqr_(v: fl) -> fl { return v * v; }
"#;

/// WGSL name of a state register.
pub fn state_name(i: usize) -> String {
    format!("s_{}", STATE_NAMES[i])
}

struct Emitter<'a> {
    f: &'a FormulaDef,
    out: String,
    indent: usize,
    /// Loops emitted so far (unique counter names).
    loops: usize,
    /// Double-float code (`dd` values, arithmetic through the df64 functions of rmb-gpu's
    /// df64.wgsl) for deep zooms.
    dd: bool,
    /// `setcount` target: the chain's first slot (0-based slot number) and its slot count;
    /// the counts live in the chain's `cnt` array. None: `setcount` is left out.
    counts: Option<(usize, usize)>,
}

/// Emits the body of one iteration of `f` (a WGSL block, without braces). `offset` is the index
/// of the formula's first constant in `prm`.
pub fn emit_iter(f: &FormulaDef, offset: usize, indent: usize) -> String {
    emit_iter_with(f, offset, indent, false, None)
}

/// [`emit_iter`] (or [`emit_iter_dd`] with `dd`) inside a hybrid chain whose slot iteration
/// counts are the array `cnt`: `setcount` writes `cnt[slot - 1 - first]` if that is one of the
/// chain's `n` slots.
pub fn emit_iter_counts(f: &FormulaDef, offset: usize, indent: usize, dd: bool, first: usize, n: usize) -> String {
    emit_iter_with(f, offset, indent, dd, Some((first, n)))
}

/// [`emit_iter`] in double-float arithmetic: every value is a `dd` (hi + lo of two f32), the
/// constants come from `prm` (high parts, then from entry 1024 on the low parts).
pub fn emit_iter_dd(f: &FormulaDef, offset: usize, indent: usize) -> String {
    emit_iter_with(f, offset, indent, true, None)
}

fn emit_iter_with(f: &FormulaDef, offset: usize, indent: usize, dd: bool, counts: Option<(usize, usize)>) -> String {
    let mut e = Emitter { f, out: String::new(), indent, loops: 0, dd, counts };
    let mut used = vec![false; f.constant_count()];
    mark_params(&f.iter, &mut used);
    for (k, u) in used.iter().enumerate() {
        if *u {
            let pad = "    ".repeat(indent);
            // `prm` is a uniform array of vec4 (uniform reads are much faster than storage).
            let (a, b) = ((offset + k) / 4, (offset + k) % 4);
            if dd {
                // The low parts follow the high ones from entry 1024 (4096 values) on.
                let _ = writeln!(e.out, "{pad}let c{k} = dd(prm[{a}u][{b}u], prm[{}u][{b}u]);", a + 1024);
            } else {
                let _ = writeln!(e.out, "{pad}let c{k} = fl(prm[{a}u][{b}u]);");
            }
        }
    }
    for s in &f.iter {
        e.stmt(s);
    }
    e.out
}

fn mark_params(stmts: &[Stmt], used: &mut [bool]) {
    fn ex(e: &Expr, used: &mut [bool]) {
        match e {
            Expr::Var(VarRef::Param(k)) => used[*k] = true,
            Expr::Neg(a) | Expr::Not(a) => ex(a, used),
            Expr::Bin(_, a, b) => {
                ex(a, used);
                ex(b, used);
            }
            Expr::Call(_, args) => args.iter().for_each(|a| ex(a, used)),
            _ => {}
        }
    }
    for s in stmts {
        match s {
            Stmt::Let(_, e) | Stmt::Assign(_, e) => ex(e, used),
            Stmt::If(c, t, e) => {
                ex(c, used);
                mark_params(t, used);
                mark_params(e, used);
            }
            Stmt::For(_, _, _, b) | Stmt::Loop(b) => mark_params(b, used),
            Stmt::Break | Stmt::Continue => {}
            Stmt::SetCount(a, b) => {
                ex(a, used);
                ex(b, used);
            }
        }
    }
}

impl Emitter<'_> {
    fn var(&self, v: VarRef) -> String {
        match v {
            VarRef::State(i) => state_name(i),
            VarRef::Param(k) => format!("c{k}"),
            VarRef::Local(k) => format!("l{k}_{}", self.f.locals.get(k).map(String::as_str).unwrap_or("v")),
        }
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(v, e) => {
                let l = format!("var {}: {} = {};", self.var(*v), if self.dd { "dd" } else { "fl" }, self.expr(e));
                self.line(&l);
            }
            Stmt::Assign(v, e) => {
                let l = format!("{} = {};", self.var(*v), self.expr(e));
                self.line(&l);
            }
            Stmt::If(c, t, e) => {
                let l = format!("if ({}) {{", self.expr(c));
                self.line(&l);
                self.block(t);
                if !e.is_empty() {
                    self.line("} else {");
                    self.block(e);
                }
                self.line("}");
            }
            Stmt::Loop(body) => {
                self.loops += 1;
                let c = format!("lc{}", self.loops);
                self.line(&format!("var {c}: u32 = 0u;"));
                self.line("loop {");
                self.indent += 1;
                self.line(&format!("{c} += 1u;"));
                self.line(&format!("if ({c} > {}u) {{", super::lang::LOOP_CAP));
                self.line("    break;");
                self.line("}");
                self.indent -= 1;
                self.block(body);
                self.line("}");
            }
            Stmt::Break => self.line("break;"),
            Stmt::Continue => self.line("continue;"),
            Stmt::SetCount(slot, n) => {
                let Some((first, nslots)) = self.counts else { return };
                let (k, v) = if self.dd {
                    (format!("dd_hi({})", self.expr(slot)), format!("dd_hi({})", self.expr(n)))
                } else {
                    (format!("f32({})", self.expr(slot)), format!("f32({})", self.expr(n)))
                };
                self.line("{");
                self.line(&format!("    let ck = i32(round({k})) - {};", first + 1));
                self.line(&format!("    if (ck >= 0 && ck < {nslots}) {{"));
                self.line(&format!("        cnt[ck] = u32(clamp(round({v}), 0.0, 1.0e9));"));
                self.line("    }");
                self.line("}");
            }
            Stmt::For(k, a, b, body) => {
                let n = self.var(VarRef::Local(*k));
                let l = if self.dd {
                    format!("for (var {n}: dd = dd_f({}); dd_lt({n}, dd_f({})); {n} = dd_add({n}, dd_f(1.0))) {{", lit(*a as f64), lit(*b as f64))
                } else {
                    format!("for (var {n}: fl = {}; {n} < {}; {n} += 1.0) {{", lit(*a as f64), lit(*b as f64))
                };
                self.line(&l);
                self.block(body);
                self.line("}");
            }
        }
    }

    fn block(&mut self, b: &[Stmt]) {
        self.indent += 1;
        for s in b {
            self.stmt(s);
        }
        self.indent -= 1;
    }

    fn expr(&self, e: &Expr) -> String {
        if self.dd {
            return self.expr_dd(e);
        }
        match e {
            Expr::Num(v) => gpu_lit(*v),
            Expr::Var(v) => self.var(*v),
            Expr::Neg(a) => format!("(-{})", self.expr(a)),
            Expr::Not(a) => format!("(!{})", self.expr(a)),
            Expr::Bin(op, a, b) => {
                let o = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Rem => "%",
                    BinOp::Lt => "<",
                    BinOp::Le => "<=",
                    BinOp::Gt => ">",
                    BinOp::Ge => ">=",
                    BinOp::Eq => "==",
                    BinOp::Ne => "!=",
                    BinOp::And => "&&",
                    BinOp::Or => "||",
                };
                format!("({} {o} {})", self.expr(a), self.expr(b))
            }
            Expr::Call(f, args) => {
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                use Func::*;
                let name = match f {
                    Abs => "abs",
                    Sqrt => "sqrt",
                    InvSqrt => "inverseSqrt",
                    Sin => "sin",
                    Cos => "cos",
                    Tan => "tan",
                    Asin => "asin",
                    Acos => "acos",
                    Atan => "atan",
                    Atan2 => "atan2",
                    Exp => "exp",
                    Log => "log",
                    Pow => "pow",
                    Floor => "floor",
                    Ceil => "ceil",
                    Fract => "fract",
                    Round => "round",
                    Min => "min",
                    Max => "max",
                    Clamp => "clamp",
                    Sign => "sign",
                    Sinh => "sinh",
                    Cosh => "cosh",
                    Tanh => "tanh",
                    Mix => "mix",
                    Step => "step",
                    Smoothstep => "smoothstep",
                    Sqr => "sqr_",
                    Single => return format!("fl(f32({}))", a[0]),
                    Select => return format!("select({}, {}, {})", a[2], a[1], a[0]),
                    MapXY => return format!("fl(map_xy(f32({}), f32({}), f32({}), f32({})))", a[0], a[1], a[2], a[3]),
                    MapSphere => return format!("fl(map_sph(f32({}), f32({}), f32({}), f32({}), f32({})))", a[0], a[1], a[2], a[3], a[4]),
                };
                // Typed first argument: abstract literals alone would make e.g. `abs(2.0)` abstract.
                format!("{name}(fl({}){})", a[0], a[1..].iter().map(|s| format!(", {s}")).collect::<String>())
            }
        }
    }
}

impl Emitter<'_> {
    /// [`Emitter::expr`] in double-float arithmetic.
    fn expr_dd(&self, e: &Expr) -> String {
        match e {
            Expr::Num(v) => dd_lit(*v),
            Expr::Var(v) => self.var(*v),
            Expr::Neg(a) => format!("(-{})", self.expr_dd(a)),
            Expr::Not(a) => format!("(!{})", self.expr_dd(a)),
            Expr::Bin(op, a, b) => {
                let (x, y) = (self.expr_dd(a), self.expr_dd(b));
                let f = match op {
                    BinOp::Add => "dd_add",
                    BinOp::Sub => "dd_sub",
                    BinOp::Mul => "dd_mul",
                    BinOp::Div => "dd_div",
                    BinOp::Rem => "dd_rem",
                    BinOp::Lt => "dd_lt",
                    BinOp::Le => "dd_le",
                    BinOp::Gt => "dd_gt",
                    BinOp::Ge => "dd_ge",
                    BinOp::Eq => "dd_eq",
                    BinOp::Ne => "dd_ne",
                    BinOp::And => return format!("({x} && {y})"),
                    BinOp::Or => return format!("({x} || {y})"),
                };
                format!("{f}({x}, {y})")
            }
            Expr::Call(f, args) => {
                let a: Vec<String> = args.iter().map(|x| self.expr_dd(x)).collect();
                use Func::*;
                let name = match f {
                    Abs => "dd_abs",
                    Sqrt => "dd_sqrt",
                    InvSqrt => "dd_inverse_sqrt",
                    Sin => "dd_sin",
                    Cos => "dd_cos",
                    Tan => "dd_tan",
                    Asin => "dd_asin",
                    Acos => "dd_acos",
                    Atan => "dd_atan",
                    Atan2 => "dd_atan2",
                    Exp => "dd_exp",
                    Log => "dd_log",
                    Pow => "dd_pow",
                    Floor => "dd_floor",
                    Ceil => "dd_ceil",
                    Fract => "dd_fract",
                    Round => "dd_round",
                    Min => "dd_min",
                    Max => "dd_max",
                    Clamp => "dd_clamp",
                    Sign => "dd_sign",
                    Sinh => "dd_sinh",
                    Cosh => "dd_cosh",
                    Tanh => "dd_tanh",
                    Mix => "dd_mix",
                    Step => "dd_step",
                    Smoothstep => "dd_smoothstep",
                    Sqr => "dd_sqr",
                    Single => return format!("dd_f(dd_hi({}))", a[0]),
                    Select => return format!("select({}, {}, {})", a[2], a[1], a[0]),
                    MapXY => return format!("dd_f(map_xy(dd_hi({}), dd_hi({}), dd_hi({}), dd_hi({})))", a[0], a[1], a[2], a[3]),
                    MapSphere => return format!("dd_f(map_sph(dd_hi({}), dd_hi({}), dd_hi({}), dd_hi({}), dd_hi({})))", a[0], a[1], a[2], a[3], a[4]),
                };
                format!("{name}({})", a.join(", "))
            }
        }
    }
}

/// A double-float literal `dd(hi, lo)`: `v` split into two f32 (within the f32 range).
pub fn dd_lit(v: f64) -> String {
    let v = if v.is_finite() { v.clamp(-3.0e38, 3.0e38) } else { v };
    let hi = v as f32;
    let lo = if hi.is_finite() { (v - hi as f64) as f32 } else { 0.0 };
    let lo = if lo.abs() < 1.0e-37 { 0.0 } else { lo };
    format!("dd({}, {})", gpu_lit(hi as f64), lit(lo as f64))
}

/// A float literal of the formula language / WGSL (abstract, so it adapts to f32 or f64).
pub fn lit(v: f64) -> String {
    if !v.is_finite() {
        return if v.is_nan() { "0.0".into() } else if v > 0.0 { "3.0e38".into() } else { "-3.0e38".into() };
    }
    let s = format!("{v:?}");
    if s.contains('.') || s.contains('e') { s } else { format!("{s}.0") }
}

/// A literal that f32 can represent: clamped to its range; tiny epsilons (used against division
/// by zero) stay tiny but non-zero.
pub fn gpu_lit(v: f64) -> String {
    let v = if v.is_finite() { v.clamp(-3.0e38, 3.0e38) } else { v };
    let v = if v != 0.0 && v.abs() < 1.0e-37 { 1.0e-37f64.copysign(v) } else { v };
    lit(v)
}
