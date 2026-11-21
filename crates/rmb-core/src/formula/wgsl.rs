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
}

/// Emits the body of one iteration of `f` (a WGSL block, without braces). `offset` is the index
/// of the formula's first constant in `prm`.
pub fn emit_iter(f: &FormulaDef, offset: usize, indent: usize) -> String {
    let mut e = Emitter { f, out: String::new(), indent };
    let mut used = vec![false; f.constant_count()];
    mark_params(&f.iter, &mut used);
    for (k, u) in used.iter().enumerate() {
        if *u {
            let pad = "    ".repeat(indent);
            let _ = writeln!(e.out, "{pad}let c{k} = fl(prm[{}u]);", offset + k);
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
            Stmt::For(_, _, _, b) => mark_params(b, used),
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
                let l = format!("var {}: fl = {};", self.var(*v), self.expr(e));
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
            Stmt::For(k, a, b, body) => {
                let n = self.var(VarRef::Local(*k));
                let l = format!("for (var {n}: fl = {}; {n} < {}; {n} += 1.0) {{", lit(*a as f64), lit(*b as f64));
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
                };
                // Typed first argument: abstract literals alone would make e.g. `abs(2.0)` abstract.
                format!("{name}(fl({}){})", a[0], a[1..].iter().map(|s| format!(", {s}")).collect::<String>())
            }
        }
    }
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
