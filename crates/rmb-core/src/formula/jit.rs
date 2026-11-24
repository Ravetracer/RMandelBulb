// SPDX-License-Identifier: LGPL-2.1-or-later

//! Compiles a formula's `iter` block to native code with Cranelift.
//!
//! The generated function has the signature `extern "C" fn(*mut f64) -> u8` and works on the
//! same register file as the closures of [`super::cpu`] (same layout, same return codes). The
//! registers a formula uses are loaded into SSA variables at entry and the assigned ones are
//! stored back at exit. Arithmetic, comparisons, `sqrt`, `abs`, `floor`, `ceil`, `round`,
//! `min`, `max` and friends are native instructions; transcendental functions, `pow`, `%` and
//! the map lookups call small Rust helpers, so every result equals the closure version.
//!
//! Compiled functions are cached by their AST (and register layout) for the life of the
//! process; the machine code is never freed (the JIT memory provider leaks it on drop).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{AbiParam, Block, InstBuilder, MemFlagsData, SigRef, Signature, Type, Value, types};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Module, default_libcall_names};

use super::cpu::{BREAK, CONTINUE, Layout, NEXT};
use super::lang::{BinOp, Expr, Func, LOOP_CAP, Stmt, VarRef};

/// A compiled `iter` block. The pointer must address at least the formula's `iter_regs`
/// registers.
pub type JitFn = unsafe extern "C" fn(*mut f64) -> u8;

/// `RMB_NO_JIT=1` forces the closure compiler (for comparisons).
pub fn disabled() -> bool {
    static OFF: LazyLock<bool> = LazyLock::new(|| std::env::var("RMB_NO_JIT").is_ok_and(|v| !v.is_empty() && v != "0"));
    *OFF
}

/// The host ISA, or None where Cranelift does not support it.
fn isa() -> Option<OwnedTargetIsa> {
    static ISA: LazyLock<Option<OwnedTargetIsa>> = LazyLock::new(|| {
        let mut flags = settings::builder();
        flags.set("opt_level", "speed").ok()?;
        flags.set("is_pic", "false").ok()?;
        flags.set("use_colocated_libcalls", "false").ok()?;
        cranelift_native::builder().ok()?.finish(settings::Flags::new(flags)).ok()
    });
    ISA.clone()
}

/// Cache of compiled functions by AST and layout (None: compilation failed).
static CACHE: LazyLock<Mutex<HashMap<String, Option<usize>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Compiles `stmts` (an `iter` block) for `regs` registers in layout `l`. None if the host is
/// not supported or compilation fails; the caller then keeps the closures.
pub(super) fn compile(stmts: &[Stmt], l: Layout, regs: usize) -> Option<JitFn> {
    let key = format!("{regs}|{l:?}|{stmts:?}");
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let addr = *cache.entry(key).or_insert_with(|| {
        // Cranelift reports bad input by panicking in places; treat that as a failure.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(stmts, l, regs))).ok().flatten()
    });
    // SAFETY: the address is the entry of a finalized function with the `JitFn` signature;
    // its code is never freed.
    addr.map(|a| unsafe { std::mem::transmute::<usize, JitFn>(a) })
}

fn build(stmts: &[Stmt], l: Layout, regs: usize) -> Option<usize> {
    let mut module = JITModule::new(JITBuilder::with_isa(isa()?, default_libcall_names()));
    let frontend = module.target_config();
    let ptr = frontend.pointer_type();
    let mut ctx = module.make_context();
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr));
    sig.returns.push(AbiParam::new(types::I8));
    ctx.func.signature = sig.clone();
    let id = module.declare_anonymous_function(&sig).ok()?;
    let mut fctx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
        let helper_sig = |n: usize| {
            let mut s = Signature::new(module.isa().default_call_conv());
            s.params.extend((0..n).map(|_| AbiParam::new(types::F64)));
            s.returns.push(AbiParam::new(types::F64));
            s
        };
        let sigs = [helper_sig(1), helper_sig(2), helper_sig(3), helper_sig(4), helper_sig(5)].map(|s| b.import_signature(s));
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let base = b.block_params(entry)[0];
        let exit = b.create_block();
        let flow = b.append_block_param(exit, types::I8);

        // Registers used (read or written) and registers written.
        let mut used = vec![false; regs];
        let mut written = vec![false; regs];
        scan_block(stmts, l, &mut used, &mut written);
        let mut g = Gen { b, ptr, vars: vec![None; regs], loops: Vec::new(), exit, sigs, layout: l, ok: true };
        for k in (0..regs).filter(|k| used[*k]) {
            let v = g.b.declare_var(types::F64);
            let x = g.b.ins().load(types::F64, MemFlagsData::trusted(), base, (k * 8) as i32);
            g.b.def_var(v, x);
            g.vars[k] = Some(v);
        }
        if !g.block(stmts) {
            let next = g.b.ins().iconst(types::I8, NEXT as i64);
            g.b.ins().jump(exit, &[next.into()]);
        }
        g.b.switch_to_block(exit);
        for k in (0..regs).filter(|k| written[*k]) {
            if let Some(v) = g.vars[k] {
                let x = g.b.use_var(v);
                g.b.ins().store(MemFlagsData::trusted(), x, base, (k * 8) as i32);
            }
        }
        g.b.ins().return_(&[flow]);
        let ok = g.ok;
        let mut b = g.b;
        b.seal_all_blocks();
        b.finalize(frontend);
        if !ok {
            return None;
        }
    }
    module.define_function(id, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    Some(module.get_finalized_function(id) as usize)
}

fn mark(l: Layout, v: VarRef, regs: &mut [bool]) {
    if let Some(r) = regs.get_mut(l.index(v)) {
        *r = true;
    }
}

fn scan_block(stmts: &[Stmt], l: Layout, used: &mut [bool], written: &mut [bool]) {
    for s in stmts {
        match s {
            Stmt::Let(v, e) | Stmt::Assign(v, e) => {
                mark(l, *v, used);
                mark(l, *v, written);
                scan_expr(e, l, used);
            }
            Stmt::If(c, t, e) => {
                scan_expr(c, l, used);
                scan_block(t, l, used, written);
                scan_block(e, l, used, written);
            }
            Stmt::For(v, _, _, body) => {
                mark(l, VarRef::Local(*v), used);
                mark(l, VarRef::Local(*v), written);
                scan_block(body, l, used, written);
            }
            Stmt::Loop(body) => scan_block(body, l, used, written),
            Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn scan_expr(e: &Expr, l: Layout, used: &mut [bool]) {
    match e {
        Expr::Num(_) => {}
        Expr::Var(v) => mark(l, *v, used),
        Expr::Neg(a) | Expr::Not(a) => scan_expr(a, l, used),
        Expr::Bin(_, a, b) => {
            scan_expr(a, l, used);
            scan_expr(b, l, used);
        }
        Expr::Call(_, args) => args.iter().for_each(|a| scan_expr(a, l, used)),
    }
}

/// Expressions without helper calls: both sides of `sel`, `&&`, `||` are then evaluated
/// without branches (the language has no side effects, so the result is the same).
fn cheap(e: &Expr) -> bool {
    use Func::*;
    match e {
        Expr::Num(_) | Expr::Var(_) => true,
        Expr::Neg(a) | Expr::Not(a) => cheap(a),
        Expr::Bin(op, a, b) => *op != BinOp::Rem && cheap(a) && cheap(b),
        Expr::Call(f, args) => {
            matches!(f, Abs | Sqrt | InvSqrt | Floor | Ceil | Fract | Round | Min | Max | Clamp | Sign | Sqr | Single | Select | Mix | Step) && args.iter().all(cheap)
        }
    }
}

struct Gen<'a> {
    b: FunctionBuilder<'a>,
    ptr: Type,
    /// SSA variable per register (None: not used).
    vars: Vec<Option<Variable>>,
    /// Innermost loops: (continue target, break target).
    loops: Vec<(Block, Block)>,
    exit: Block,
    /// Helper signatures by argument count - 1.
    sigs: [SigRef; 5],
    layout: Layout,
    /// False when the AST holds something unexpected (wrong argument count).
    ok: bool,
}

impl Gen<'_> {
    fn var(&mut self, v: VarRef) -> Option<Variable> {
        let v = self.vars.get(self.layout.index(v)).copied().flatten();
        if v.is_none() {
            self.ok = false;
        }
        v
    }

    fn read(&mut self, v: VarRef) -> Value {
        match self.var(v) {
            Some(var) => self.b.use_var(var),
            None => self.f64(0.0),
        }
    }

    fn write(&mut self, v: VarRef, x: Value) {
        if let Some(var) = self.var(v) {
            self.b.def_var(var, x);
        }
    }

    fn f64(&mut self, v: f64) -> Value {
        self.b.ins().f64const(v)
    }

    /// Emits a statement list; true if it ends in a jump (the rest would be dead code).
    fn block(&mut self, stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| self.stmt(s))
    }

    fn stmt(&mut self, s: &Stmt) -> bool {
        match s {
            Stmt::Let(v, e) | Stmt::Assign(v, e) => {
                let x = self.num(e);
                self.write(*v, x);
                false
            }
            Stmt::If(c, t, e) => {
                let c = self.cond(c);
                let (tb, merge) = (self.b.create_block(), self.b.create_block());
                let eb = if e.is_empty() { merge } else { self.b.create_block() };
                self.b.ins().brif(c, tb, &[], eb, &[]);
                self.b.switch_to_block(tb);
                let t_end = self.block(t);
                if !t_end {
                    self.b.ins().jump(merge, &[]);
                }
                let mut e_end = false;
                if !e.is_empty() {
                    self.b.switch_to_block(eb);
                    e_end = self.block(e);
                    if !e_end {
                        self.b.ins().jump(merge, &[]);
                    }
                }
                if t_end && e_end {
                    return true;
                }
                self.b.switch_to_block(merge);
                false
            }
            Stmt::For(v, a, b, body) => {
                let reg = VarRef::Local(*v);
                let n = self.b.declare_var(types::I64);
                let start = self.b.ins().iconst(types::I64, *a);
                self.b.def_var(n, start);
                let (head, body_b, latch, done) = (self.b.create_block(), self.b.create_block(), self.b.create_block(), self.b.create_block());
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let k = self.b.use_var(n);
                let more = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, k, *b);
                self.b.ins().brif(more, body_b, &[], done, &[]);
                self.b.switch_to_block(body_b);
                let k = self.b.use_var(n);
                let kf = self.b.ins().fcvt_from_sint(types::F64, k);
                self.write(reg, kf);
                self.loop_body(body, latch, done);
                self.b.switch_to_block(latch);
                let k = self.b.use_var(n);
                let k1 = self.b.ins().iadd_imm_s(k, 1);
                self.b.def_var(n, k1);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(done);
                false
            }
            Stmt::Loop(body) => {
                let n = self.b.declare_var(types::I32);
                let zero = self.b.ins().iconst(types::I32, 0);
                self.b.def_var(n, zero);
                let (head, body_b, latch, done) = (self.b.create_block(), self.b.create_block(), self.b.create_block(), self.b.create_block());
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let k = self.b.use_var(n);
                let more = self.b.ins().icmp_imm_s(IntCC::UnsignedLessThan, k, LOOP_CAP as i64);
                self.b.ins().brif(more, body_b, &[], done, &[]);
                self.b.switch_to_block(body_b);
                self.loop_body(body, latch, done);
                self.b.switch_to_block(latch);
                let k = self.b.use_var(n);
                let k1 = self.b.ins().iadd_imm_s(k, 1);
                self.b.def_var(n, k1);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(done);
                false
            }
            Stmt::Break | Stmt::Continue => {
                let brk = matches!(s, Stmt::Break);
                match self.loops.last() {
                    Some(&(cont, done)) => {
                        self.b.ins().jump(if brk { done } else { cont }, &[]);
                    }
                    None => {
                        let code = self.b.ins().iconst(types::I8, if brk { BREAK } else { CONTINUE } as i64);
                        self.b.ins().jump(self.exit, &[code.into()]);
                    }
                }
                true
            }
        }
    }

    fn loop_body(&mut self, body: &[Stmt], latch: Block, done: Block) {
        self.loops.push((latch, done));
        if !self.block(body) {
            self.b.ins().jump(latch, &[]);
        }
        self.loops.pop();
    }

    /// A condition as an `i8` (0 or 1).
    fn cond(&mut self, e: &Expr) -> Value {
        match e {
            Expr::Not(a) => {
                let a = self.cond(a);
                self.b.ins().icmp_imm_s(IntCC::Equal, a, 0)
            }
            Expr::Bin(op @ (BinOp::And | BinOp::Or), a, rhs) => {
                let a = self.cond(a);
                if cheap(rhs) {
                    let r = self.cond(rhs);
                    return if *op == BinOp::And { self.b.ins().band(a, r) } else { self.b.ins().bor(a, r) };
                }
                // Short circuit: the right side calls helpers.
                let (rb, merge) = (self.b.create_block(), self.b.create_block());
                let res = self.b.append_block_param(merge, types::I8);
                if *op == BinOp::And {
                    self.b.ins().brif(a, rb, &[], merge, &[a.into()]);
                } else {
                    self.b.ins().brif(a, merge, &[a.into()], rb, &[]);
                }
                self.b.switch_to_block(rb);
                let r = self.cond(rhs);
                self.b.ins().jump(merge, &[r.into()]);
                self.b.switch_to_block(merge);
                res
            }
            Expr::Bin(op, a, b) => {
                let (a, b) = (self.num(a), self.num(b));
                let cc = match op {
                    BinOp::Lt => FloatCC::LessThan,
                    BinOp::Le => FloatCC::LessThanOrEqual,
                    BinOp::Gt => FloatCC::GreaterThan,
                    BinOp::Ge => FloatCC::GreaterThanOrEqual,
                    BinOp::Eq => FloatCC::Equal,
                    // `!=` is true for NaN, as in Rust.
                    _ => FloatCC::NotEqual,
                };
                self.b.ins().fcmp(cc, a, b)
            }
            _ => self.b.ins().iconst(types::I8, 0),
        }
    }

    fn num(&mut self, e: &Expr) -> Value {
        match e {
            Expr::Num(v) => self.f64(*v),
            Expr::Var(v) => self.read(*v),
            Expr::Neg(a) => {
                let a = self.num(a);
                self.b.ins().fneg(a)
            }
            Expr::Bin(op, a, b) => {
                let (a, b) = (self.num(a), self.num(b));
                match op {
                    BinOp::Add => self.b.ins().fadd(a, b),
                    BinOp::Sub => self.b.ins().fsub(a, b),
                    BinOp::Mul => self.b.ins().fmul(a, b),
                    BinOp::Div => self.b.ins().fdiv(a, b),
                    _ => self.helper(h_rem as *const () as usize, &[a, b]),
                }
            }
            Expr::Call(f, args) => self.call(*f, args),
            Expr::Not(_) => self.f64(0.0),
        }
    }

    fn helper(&mut self, addr: usize, args: &[Value]) -> Value {
        let sig = self.sigs[args.len() - 1];
        let callee = self.b.ins().iconst(self.ptr, addr as i64);
        let call = self.b.ins().call_indirect(sig, callee, args);
        self.b.inst_results(call)[0]
    }

    fn min(&mut self, a: Value, b: Value) -> Value {
        // f64::min: the other operand when one is NaN.
        let lt = self.b.ins().fcmp(FloatCC::LessThan, a, b);
        let b_nan = self.b.ins().fcmp(FloatCC::Unordered, b, b);
        let c = self.b.ins().bor(lt, b_nan);
        self.b.ins().select(c, a, b)
    }

    fn max(&mut self, a: Value, b: Value) -> Value {
        let gt = self.b.ins().fcmp(FloatCC::GreaterThan, a, b);
        let b_nan = self.b.ins().fcmp(FloatCC::Unordered, b, b);
        let c = self.b.ins().bor(gt, b_nan);
        self.b.ins().select(c, a, b)
    }

    fn call(&mut self, f: Func, args: &[Expr]) -> Value {
        use Func::*;
        let arity = match f {
            MapSphere => 5,
            MapXY => 4,
            Select | Clamp | Mix | Smoothstep => 3,
            Atan2 | Pow | Min | Max | Step => 2,
            _ => 1,
        };
        if args.len() != arity {
            self.ok = false;
            return self.f64(0.0);
        }
        if f == Select {
            let c = self.cond(&args[0]);
            if cheap(&args[1]) && cheap(&args[2]) {
                let (a, b) = (self.num(&args[1]), self.num(&args[2]));
                return self.b.ins().select(c, a, b);
            }
            let (ab, bb, merge) = (self.b.create_block(), self.b.create_block(), self.b.create_block());
            let res = self.b.append_block_param(merge, types::F64);
            self.b.ins().brif(c, ab, &[], bb, &[]);
            self.b.switch_to_block(ab);
            let a = self.num(&args[1]);
            self.b.ins().jump(merge, &[a.into()]);
            self.b.switch_to_block(bb);
            let b = self.num(&args[2]);
            self.b.ins().jump(merge, &[b.into()]);
            self.b.switch_to_block(merge);
            return res;
        }
        let a: Vec<Value> = args.iter().map(|e| self.num(e)).collect();
        let x = a[0];
        let h1 = |g: &mut Self, h: extern "C" fn(f64) -> f64| g.helper(h as *const () as usize, &[x]);
        match f {
            Abs => self.b.ins().fabs(x),
            Sqrt => self.b.ins().sqrt(x),
            InvSqrt => {
                let s = self.b.ins().sqrt(x);
                let one = self.f64(1.0);
                self.b.ins().fdiv(one, s)
            }
            Floor => self.b.ins().floor(x),
            Ceil => self.b.ins().ceil(x),
            Fract => {
                let fl = self.b.ins().floor(x);
                self.b.ins().fsub(x, fl)
            }
            Round => self.b.ins().nearest(x),
            Sqr => self.b.ins().fmul(x, x),
            Single => {
                let s = self.b.ins().fdemote(types::F32, x);
                self.b.ins().fpromote(types::F64, s)
            }
            Sign => {
                let (zero, one, m1) = (self.f64(0.0), self.f64(1.0), self.f64(-1.0));
                let neg = self.b.ins().fcmp(FloatCC::LessThan, x, zero);
                let lo = self.b.ins().select(neg, m1, zero);
                let pos = self.b.ins().fcmp(FloatCC::GreaterThan, x, zero);
                self.b.ins().select(pos, one, lo)
            }
            Sin => h1(self, h_sin),
            Cos => h1(self, h_cos),
            Tan => h1(self, h_tan),
            Asin => h1(self, h_asin),
            Acos => h1(self, h_acos),
            Atan => h1(self, h_atan),
            Exp => h1(self, h_exp),
            Log => h1(self, h_ln),
            Sinh => h1(self, h_sinh),
            Cosh => h1(self, h_cosh),
            Tanh => h1(self, h_tanh),
            Atan2 => self.helper(h_atan2 as *const () as usize, &a),
            Pow => self.helper(h_pow as *const () as usize, &a),
            Min => self.min(a[0], a[1]),
            Max => self.max(a[0], a[1]),
            Step => {
                // step(e, v): 0 below the edge, else 1 (also for NaN).
                let below = self.b.ins().fcmp(FloatCC::LessThan, a[1], a[0]);
                let (zero, one) = (self.f64(0.0), self.f64(1.0));
                self.b.ins().select(below, zero, one)
            }
            Clamp => {
                let m = self.max(a[0], a[1]);
                self.min(m, a[2])
            }
            Mix => {
                let d = self.b.ins().fsub(a[1], a[0]);
                let t = self.b.ins().fmul(d, a[2]);
                self.b.ins().fadd(a[0], t)
            }
            Smoothstep => self.helper(h_smoothstep as *const () as usize, &a),
            MapXY => self.helper(h_map_xy as *const () as usize, &a),
            MapSphere => self.helper(h_map_sphere as *const () as usize, &a),
            Select => unreachable!("handled above"),
        }
    }
}

extern "C" fn h_rem(a: f64, b: f64) -> f64 {
    a % b
}
extern "C" fn h_sin(x: f64) -> f64 {
    x.sin()
}
extern "C" fn h_cos(x: f64) -> f64 {
    x.cos()
}
extern "C" fn h_tan(x: f64) -> f64 {
    x.tan()
}
extern "C" fn h_asin(x: f64) -> f64 {
    x.asin()
}
extern "C" fn h_acos(x: f64) -> f64 {
    x.acos()
}
extern "C" fn h_atan(x: f64) -> f64 {
    x.atan()
}
extern "C" fn h_exp(x: f64) -> f64 {
    x.exp()
}
extern "C" fn h_ln(x: f64) -> f64 {
    x.ln()
}
extern "C" fn h_sinh(x: f64) -> f64 {
    x.sinh()
}
extern "C" fn h_cosh(x: f64) -> f64 {
    x.cosh()
}
extern "C" fn h_tanh(x: f64) -> f64 {
    x.tanh()
}
extern "C" fn h_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}
extern "C" fn h_pow(a: f64, b: f64) -> f64 {
    super::cpu::gpu_pow(a, b)
}
extern "C" fn h_smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    super::cpu::smoothstep(e0, e1, x)
}
extern "C" fn h_map_xy(n: f64, u: f64, v: f64, ch: f64) -> f64 {
    crate::maps::sample_xy(n, u, v, ch)
}
extern "C" fn h_map_sphere(n: f64, x: f64, y: f64, z: f64, ch: f64) -> f64 {
    crate::maps::sample_sphere(n, x, y, z, ch)
}

#[cfg(test)]
mod tests {
    use crate::formula::cpu::CpuFormula;
    use crate::formula::lang::{STATE_AUX, STATE_COUNT, STATE_DR, STATE_ITER, STATE_JULIA, STATE_PX, STATE_ROUT, parse_library};

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.next()
        }
    }

    fn same(a: f64, b: f64) -> bool {
        a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()) || a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
    }

    /// Runs `iters` iterations natively and through the closures from the same registers;
    /// returns the first difference.
    fn compare(cpu: &CpuFormula, consts: &[f64], rng: &mut Rng, iters: usize) -> Option<String> {
        let mut r = vec![0.0; cpu.iter_regs];
        for v in r.iter_mut().take(8) {
            *v = rng.range(-1.5, 1.5);
        }
        r[STATE_DR] = 1.0;
        r[STATE_ROUT] = r[..4].iter().map(|v| v * v).sum();
        r.copy_within(0..3, STATE_PX);
        r[STATE_AUX] = 1.0;
        r[STATE_JULIA] = 0.0;
        r[STATE_COUNT..STATE_COUNT + consts.len()].copy_from_slice(consts);
        let mut c = r.clone();
        for n in 0..iters {
            r[STATE_ITER] = n as f64;
            c[STATE_ITER] = n as f64;
            let (fr, fc) = (cpu.iter(&mut r), cpu.iter_closure(&mut c));
            if fr != fc {
                return Some(format!("iteration {n}: flow {fr} != {fc}"));
            }
            if let Some(k) = (0..r.len()).find(|k| !same(r[*k], c[*k])) {
                return Some(format!("iteration {n}: register {k}: {} != {}", r[k], c[k]));
            }
        }
        None
    }

    #[test]
    fn library_formulas_match_closures() {
        if super::disabled() || super::isa().is_none() {
            return;
        }
        let lib = crate::formula::library();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut failed = Vec::new();
        for f in lib.all() {
            if f.cpu.native().is_none() {
                failed.push(format!("{}: not compiled", f.def.name));
                continue;
            }
            for set in 0..4 {
                // Defaults first, then random values within the parameter ranges.
                let values: Vec<f64> = f.def.params.iter().map(|p| if set == 0 { p.default } else { rng.range(p.min, p.max) }).collect();
                let consts = f.cpu.constants(&values);
                if let Some(e) = (0..6).find_map(|_| compare(&f.cpu, &consts, &mut rng, 5)) {
                    failed.push(format!("{} (set {set}): {e}", f.def.name));
                    break;
                }
            }
        }
        assert!(lib.all().len() > 400);
        assert!(failed.is_empty(), "{} formulas differ:\n{}", failed.len(), failed.join("\n"));
    }

    #[test]
    fn special_values_match_closures() {
        if super::disabled() || super::isa().is_none() {
            return;
        }
        let src = r#"
            formula "Special" {
                iter {
                    let a = x
                    let b = y
                    a0 = min(a, b) + max(a, b) * 3.0
                    a1 = clamp(a, -1.0, b) + sign(a) * 7.0 + step(b, a)
                    a2 = floor(a) + ceil(b) * 2.0 + fract(a) + round(b)
                    a3 = sqrt(a) + rsqrt(b) + abs(a) + sqr(b) + single(a)
                    a4 = sin(a) + cos(b) + tan(a) + asin(a) + acos(b) + atan(a) + atan2(a, b)
                    a5 = exp(a) + log(b) + pow(a, b) + sinh(a) + cosh(b) + tanh(a) + a % b
                    a6 = mix(a, b, 0.25) + smoothstep(a, b, 0.5) + sel(a < b || b != b, a, b)
                    a7 = sel(!(a >= b) && a == a, 1.0, 2.0) + sel(a <= b, 3.0, 4.0) + sel(a > b, sin(a), 6.0)
                    if a > b && sin(a) > 0.0 { cx = 1.0 } else { cx = 2.0 }
                    for k in 0..3 { if a > 0.5 { break } z += k; }
                    loop { w += 1.0; if w > 3.0 { break } if w < 0.0 { continue } }
                }
            }"#;
        let def = &parse_library(src).unwrap()[0];
        let cpu = CpuFormula::compile(def);
        assert!(cpu.native().is_some());
        let vals = [0.0, -0.0, 1.0, -1.0, 0.5, -2.5, 2.5, 3.5, 1e300, -1e300, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 1e-310, 0.7];
        for &a in &vals {
            for &b in &vals {
                let mut r = vec![0.0; cpu.iter_regs];
                r[0] = a;
                r[1] = b;
                r[3] = -0.5;
                let mut c = r.clone();
                assert_eq!(cpu.iter(&mut r), cpu.iter_closure(&mut c));
                for k in 0..r.len() {
                    assert!(same(r[k], c[k]), "x={a} y={b}: register {k}: {} != {}", r[k], c[k]);
                }
            }
        }
    }
}
