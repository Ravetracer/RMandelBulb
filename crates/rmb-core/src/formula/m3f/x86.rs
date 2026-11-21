// SPDX-License-Identifier: LGPL-2.1-or-later

//! Symbolic translation of Mandelbulb3D formula machine code (32-bit x86, x87 and SSE2) into
//! formula-language statements.
//!
//! Calling convention (Delphi `register`): `eax`, `edx`, `ecx` point to x, y, z, `[ebp+12]` to w
//! and `[ebp+8]` to the iteration record (pointer to its field C1); `[record+48]` is the
//! parameter pointer: options below it, constants from 0 up.
//!
//! The translation keeps the machine state in variables: the x87 stack slots are `f0`…`f7`
//! (by depth), xmm lanes `xNl`/`xNh`, integer registers that hold data `r_eax`…, stack memory
//! `m<offset>`. Pointers and plain integers are tracked statically. Record fields map to the
//! state registers (x, y, z, w or dr, cx…, rout, a0…). Conditional jumps become `if`/`else`
//! regions that end at the branch's immediate post-dominator. The condition of an FPU compare
//! is found by evaluating the flag-handling instructions (fnstsw, sahf, test, and, shr, …) for
//! each possible compare outcome (greater, less, equal, unordered) and seeing in which outcomes
//! the jump is taken. Loops are not supported yet.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

use iced_x86::{Decoder, DecoderOptions, FlowControl, Formatter, Instruction, IntelFormatter, MemorySize, Mnemonic, OpKind, Register};

use super::{M3f, MemSlot, SlotKind};

type Res<T> = Result<T, String>;

/// Compare outcomes, in this order: greater, less, equal, unordered.
const OUTCOMES: usize = 4;

#[derive(Clone, Debug, PartialEq)]
enum Region {
    Rec,
    PVar,
    Stack,
}

/// Integer register / integer memory value.
#[derive(Clone, Debug, PartialEq)]
enum IV {
    Ptr(Region, i64),
    Int(i64),
    /// Data held in a variable (float-valued integer).
    Var(String),
    /// Depends on the last FPU compare: one value per outcome.
    Out([i64; OUTCOMES]),
    Undef,
}

const CF: u32 = 1;
const PF: u32 = 4;
const ZF: u32 = 0x40;
const SF: u32 = 0x80;
const OF: u32 = 0x800;

#[derive(Clone, Debug, PartialEq)]
enum Flags {
    None,
    /// Flags per compare outcome.
    Out([u32; OUTCOMES]),
    /// `cmp a, b` on data.
    Cmp(String, String),
}

#[derive(Clone, Debug, PartialEq)]
struct State {
    regs: [IV; 8],
    depth: usize,
    flags: Flags,
    /// Integer values pushed / stored on the machine stack.
    stack_int: BTreeMap<i64, IV>,
    /// Known constant bit patterns of xmm lanes (masks).
    xmm_known: [[Option<u64>; 2]; 8],
    /// FPU status word compare bits (C0, C2, C3) per compare outcome.
    fpu_status: [i64; OUTCOMES],
}

#[derive(Clone, Debug)]
enum S {
    Line(String),
    If(String, Vec<S>, Vec<S>),
}

/// CFG node: an instruction address in a call context.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Node {
    ctx: Vec<u32>,
    addr: u32,
}

const EXIT: usize = 0;

struct Cfg {
    nodes: Vec<Node>,
    index: HashMap<Node, usize>,
    succ: Vec<Vec<usize>>,
    ipdom: Vec<usize>,
}

pub(crate) struct Tx<'a> {
    m: &'a M3f,
    layout: &'a [MemSlot],
    ins: HashMap<u32, Instruction>,
    cfg: Cfg,
    /// Stack variables used (declared at the top).
    stack_vars: HashSet<String>,
    /// Derived constants (memory slots) used by the code.
    used_slots: BTreeMap<i64, String>,
}

fn fmt_ins(i: &Instruction) -> String {
    let mut f = IntelFormatter::new();
    let mut s = String::new();
    f.format(i, &mut s);
    format!("{:04x}: {s}", i.ip32())
}

fn reg_index(r: Register) -> Option<usize> {
    use Register::*;
    Some(match r {
        EAX | AX | AL | AH => 0,
        ECX | CX | CL | CH => 1,
        EDX | DX | DL | DH => 2,
        EBX | BX | BL | BH => 3,
        ESP | SP => 4,
        EBP | BP => 5,
        ESI | SI => 6,
        EDI | DI => 7,
        _ => return Option::None,
    })
}

const REG_NAMES: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];

fn lit(v: f64) -> String {
    if v.is_nan() {
        return "0.0".into();
    }
    let v = v.clamp(-1.0e300, 1.0e300);
    let s = crate::formula::wgsl::lit(v.abs());
    if v < 0.0 || (v == 0.0 && v.is_sign_negative()) { format!("(-{s})") } else { s }
}

impl<'a> Tx<'a> {
    fn decode_all(code: &[u8]) -> HashMap<u32, Instruction> {
        let mut out = HashMap::new();
        let mut dec = Decoder::with_ip(32, code, 0, DecoderOptions::NONE);
        // Decode linearly and also from every branch target (code may jump into the middle of padding).
        let mut todo = vec![0u32];
        let mut seen = HashSet::new();
        while let Some(a) = todo.pop() {
            if !seen.insert(a) || a as usize >= code.len() {
                continue;
            }
            let _ = dec.set_position(a as usize);
            dec.set_ip(a as u64);
            let mut addr = a;
            while (addr as usize) < code.len() {
                let i = dec.decode();
                if i.is_invalid() {
                    break;
                }
                out.insert(addr, i);
                if matches!(i.flow_control(), FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch | FlowControl::Call) {
                    todo.push(i.near_branch32());
                }
                if matches!(i.flow_control(), FlowControl::Return | FlowControl::UnconditionalBranch) {
                    break;
                }
                addr = i.next_ip32();
                if seen.contains(&addr) {
                    break;
                }
                seen.insert(addr);
            }
        }
        out
    }

    fn build_cfg(ins: &HashMap<u32, Instruction>, code_len: usize) -> Res<Cfg> {
        let mut cfg = Cfg { nodes: vec![Node { ctx: vec![u32::MAX], addr: u32::MAX }], index: HashMap::new(), succ: vec![Vec::new()], ipdom: Vec::new() };
        let start = Node { ctx: Vec::new(), addr: 0 };
        cfg.index.insert(start.clone(), 1);
        cfg.nodes.push(start);
        cfg.succ.push(Vec::new());
        let mut todo = vec![1usize];
        while let Some(n) = todo.pop() {
            let node = cfg.nodes[n].clone();
            let i = ins.get(&node.addr).ok_or_else(|| format!("no instruction at {:04x}", node.addr))?;
            let mut next = Vec::new();
            let target_ok = |t: u32| -> Res<u32> { if (t as usize) < code_len { Ok(t) } else { Err(format!("jump outside the code: {}", fmt_ins(i))) } };
            match i.flow_control() {
                FlowControl::Next => next.push(Some(Node { ctx: node.ctx.clone(), addr: i.next_ip32() })),
                FlowControl::UnconditionalBranch => {
                    if i.op0_kind() != OpKind::NearBranch32 {
                        return Err(format!("indirect jump: {}", fmt_ins(i)));
                    }
                    next.push(Some(Node { ctx: node.ctx.clone(), addr: target_ok(i.near_branch32())? }));
                }
                FlowControl::ConditionalBranch => {
                    next.push(Some(Node { ctx: node.ctx.clone(), addr: target_ok(i.near_branch32())? }));
                    next.push(Some(Node { ctx: node.ctx.clone(), addr: i.next_ip32() }));
                }
                FlowControl::Call => {
                    if i.op0_kind() != OpKind::NearBranch32 {
                        return Err(format!("indirect call (map function?): {}", fmt_ins(i)));
                    }
                    if node.ctx.len() >= 4 {
                        return Err("calls nested too deeply".into());
                    }
                    let mut ctx = node.ctx.clone();
                    ctx.push(i.next_ip32());
                    next.push(Some(Node { ctx, addr: target_ok(i.near_branch32())? }));
                }
                FlowControl::Return => {
                    if let Some((&ret, rest)) = node.ctx.split_last() {
                        next.push(Some(Node { ctx: rest.to_vec(), addr: ret }));
                    } else {
                        next.push(None);
                    }
                }
                _ => return Err(format!("unsupported control flow: {}", fmt_ins(i))),
            }
            for nx in next {
                let id = match nx {
                    None => EXIT,
                    Some(nn) => match cfg.index.get(&nn) {
                        Some(&id) => id,
                        None => {
                            if cfg.nodes.len() > 20_000 {
                                return Err("code too large".into());
                            }
                            let id = cfg.nodes.len();
                            cfg.index.insert(nn.clone(), id);
                            cfg.nodes.push(nn);
                            cfg.succ.push(Vec::new());
                            todo.push(id);
                            id
                        }
                    },
                };
                cfg.succ[n].push(id);
            }
        }
        cfg.ipdom = post_dominators(&cfg.succ)?;
        Ok(cfg)
    }
}

/// Immediate post-dominators (Cooper/Harvey/Kennedy on the reversed graph, root = EXIT).
fn post_dominators(succ: &[Vec<usize>]) -> Res<Vec<usize>> {
    let n = succ.len();
    let mut pred = vec![Vec::new(); n];
    for (a, ss) in succ.iter().enumerate() {
        for &b in ss {
            pred[b].push(a);
        }
    }
    // Reverse post order of the reversed graph from EXIT.
    let mut order = Vec::new();
    let mut seen = vec![false; n];
    let mut stack = vec![(EXIT, 0usize)];
    seen[EXIT] = true;
    while let Some((v, k)) = stack.pop() {
        if k < pred[v].len() {
            stack.push((v, k + 1));
            let w = pred[v][k];
            if !seen[w] {
                seen[w] = true;
                stack.push((w, 0));
            }
        } else {
            order.push(v);
        }
    }
    if seen.iter().skip(1).any(|s| !s) {
        return Err("code with an endless loop".into());
    }
    order.reverse();
    let mut rpo = vec![0usize; n];
    for (i, &v) in order.iter().enumerate() {
        rpo[v] = i;
    }
    let mut idom = vec![usize::MAX; n];
    idom[EXIT] = EXIT;
    let mut changed = true;
    while changed {
        changed = false;
        for &v in order.iter().skip(1) {
            // Predecessors in the reversed graph are the successors.
            let mut new = usize::MAX;
            for &s in &succ[v] {
                if idom[s] == usize::MAX {
                    continue;
                }
                new = if new == usize::MAX {
                    s
                } else {
                    let (mut a, mut b) = (s, new);
                    while a != b {
                        while rpo[a] > rpo[b] {
                            a = idom[a];
                        }
                        while rpo[b] > rpo[a] {
                            b = idom[b];
                        }
                    }
                    a
                };
            }
            if new != usize::MAX && idom[v] != new {
                idom[v] = new;
                changed = true;
            }
        }
    }
    Ok(idom)
}

/// Translates the code of `m` into the `iter` block (and an `init` block for derived values).
pub(crate) fn translate(m: &M3f, layout: &[MemSlot]) -> Res<String> {
    if m.code.is_empty() {
        return Err("no code".into());
    }
    let ins = Tx::decode_all(&m.code);
    let cfg = Tx::build_cfg(&ins, m.code.len())?;
    let mut tx = Tx { m, layout, ins, cfg, stack_vars: HashSet::new(), used_slots: BTreeMap::new() };
    let mut st = State {
        regs: [
            IV::Ptr(Region::Rec, -32),
            IV::Ptr(Region::Rec, -16),
            IV::Ptr(Region::Rec, -24),
            IV::Undef,
            IV::Ptr(Region::Stack, 0),
            IV::Undef,
            IV::Undef,
            IV::Undef,
        ],
        depth: 0,
        flags: Flags::None,
        stack_int: BTreeMap::new(),
        xmm_known: [[None; 2]; 8],
        fpu_status: [0; OUTCOMES],
    };
    // Stack at entry: return address at 0, PIteration3D at +4, w pointer at +8.
    st.stack_int.insert(4, IV::Ptr(Region::Rec, 0));
    st.stack_int.insert(8, IV::Ptr(Region::Rec, -8));
    st.stack_int.insert(0, IV::Int(0x7fff_0000));
    let mut path = Vec::new();
    let body = tx.region(1, EXIT, &mut st, &mut path)?;

    let mut out = String::new();
    if !tx.used_slots.is_empty() {
        out.push_str("    init {\n");
        for (off, name) in &tx.used_slots {
            let slot = layout.iter().find(|s| s.offset == *off).and_then(|s| s.expr.clone()).unwrap_or_else(|| "0.0".into());
            let _ = writeln!(out, "        let {name} = {slot}");
        }
        out.push_str("    }\n");
    }
    out.push_str("    iter {\n");
    let mut decl: Vec<String> = (0..8).map(|k| format!("f{k}")).collect();
    decl.extend(["fc_a", "fc_b", "ft"].iter().map(|s| s.to_string()));
    decl.extend(REG_NAMES.iter().map(|r| format!("r_{r}")));
    for k in 0..8 {
        decl.push(format!("x{k}l"));
        decl.push(format!("x{k}h"));
    }
    let mut sv: Vec<&String> = tx.stack_vars.iter().collect();
    sv.sort();
    decl.extend(sv.into_iter().cloned());
    let text = render(&body, 2);
    for d in decl {
        // Declare only what the body uses.
        if text.contains(&d) {
            let _ = writeln!(out, "        let {d} = 0.0");
        }
    }
    out.push_str(&text);
    out.push_str("    }\n");
    Ok(out)
}

fn render(stmts: &[S], indent: usize) -> String {
    let mut s = String::new();
    let pad = "    ".repeat(indent);
    for st in stmts {
        match st {
            S::Line(l) => {
                let _ = writeln!(s, "{pad}{l}");
            }
            S::If(c, t, e) => {
                let _ = writeln!(s, "{pad}if {c} {{");
                s.push_str(&render(t, indent + 1));
                if !e.is_empty() {
                    let _ = writeln!(s, "{pad}}} else {{");
                    s.push_str(&render(e, indent + 1));
                }
                let _ = writeln!(s, "{pad}}}");
            }
        }
    }
    s
}

/// Value of a memory read.
enum Mem {
    /// A float variable or expression.
    F(String),
    /// Integer value.
    I(IV),
    /// 128 bits for xmm loads: lane expressions and known bit patterns.
    X([String; 2], [Option<u64>; 2]),
}

impl Tx<'_> {
    fn region(&mut self, mut node: usize, stop: usize, st: &mut State, path: &mut Vec<usize>) -> Res<Vec<S>> {
        let mut out = Vec::new();
        let mut steps = 0;
        while node != stop && node != EXIT {
            steps += 1;
            if steps > 50_000 || path.contains(&node) {
                return Err("loops are not supported yet".into());
            }
            let addr = self.cfg.nodes[node].addr;
            let i = *self.ins.get(&addr).ok_or("missing instruction")?;
            let succ = self.cfg.succ[node].clone();
            match i.flow_control() {
                FlowControl::ConditionalBranch => {
                    let taken = self.jcc_outcome(&i, st)?;
                    let join = self.cfg.ipdom[node];
                    match taken {
                        Cond::Always => node = succ[0],
                        Cond::Never => node = succ[1],
                        Cond::Expr(c) => {
                            path.push(node);
                            let mut st_t = st.clone();
                            let mut st_f = st.clone();
                            let mut arm_t = self.region(succ[0], join, &mut st_t, path)?;
                            let mut arm_f = self.region(succ[1], join, &mut st_f, path)?;
                            path.pop();
                            if join == EXIT {
                                // Both arms end in a return; no state after this.
                                out.push(S::If(c, arm_t, arm_f));
                                return Ok(out);
                            }
                            *st = self.merge(&st_t, &st_f, &mut arm_t, &mut arm_f)?;
                            out.push(S::If(c, arm_t, arm_f));
                            node = join;
                        }
                    }
                    continue;
                }
                FlowControl::Call => {
                    let esp = self.esp(st)? - 4;
                    st.stack_int.insert(esp, IV::Int(i.next_ip32() as i64));
                    st.regs[4] = IV::Ptr(Region::Stack, esp);
                }
                FlowControl::Return => {
                    let esp = self.esp(st)?;
                    let extra = if i.op_count() > 0 { i.immediate16() as i64 } else { 0 };
                    st.regs[4] = IV::Ptr(Region::Stack, esp + 4 + extra);
                }
                FlowControl::UnconditionalBranch => {}
                _ => self.exec(&i, st, &mut out).map_err(|e| format!("{e} at {}", fmt_ins(&i)))?,
            }
            node = succ.first().copied().unwrap_or(EXIT);
        }
        Ok(out)
    }

    fn merge(&mut self, a: &State, b: &State, arm_a: &mut Vec<S>, arm_b: &mut Vec<S>) -> Res<State> {
        if a.depth != b.depth {
            return Err(format!("x87 stack depth differs after a branch ({} / {})", a.depth, b.depth));
        }
        let mut m = a.clone();
        #[allow(clippy::needless_range_loop)]
        for r in 0..8 {
            if a.regs[r] == b.regs[r] {
                continue;
            }
            let name = format!("r_{}", REG_NAMES[r]);
            let to_expr = |v: &IV| -> Option<String> {
                match v {
                    IV::Int(k) => Some(lit(*k as f64)),
                    IV::Var(s) => Some(s.clone()),
                    IV::Undef => Some("0.0".into()),
                    _ => None,
                }
            };
            match (to_expr(&a.regs[r]), to_expr(&b.regs[r])) {
                (Some(ea), Some(eb)) => {
                    if ea != name {
                        arm_a.push(S::Line(format!("{name} = {ea}")));
                    }
                    if eb != name {
                        arm_b.push(S::Line(format!("{name} = {eb}")));
                    }
                    m.regs[r] = IV::Var(name);
                }
                _ => {
                    if r == 0 {
                        // eax often holds a status word copy; an unusable value is fine if unused.
                        m.regs[r] = IV::Undef;
                    } else {
                        return Err(format!("register {} differs after a branch", REG_NAMES[r]));
                    }
                }
            }
        }
        let keys: HashSet<i64> = a.stack_int.keys().chain(b.stack_int.keys()).copied().collect();
        for k in keys {
            if a.stack_int.get(&k) != b.stack_int.get(&k) {
                m.stack_int.remove(&k);
            }
        }
        for x in 0..8 {
            for l in 0..2 {
                if a.xmm_known[x][l] != b.xmm_known[x][l] {
                    m.xmm_known[x][l] = None;
                }
            }
        }
        if a.flags != b.flags {
            m.flags = Flags::None;
        }
        Ok(m)
    }

    fn esp(&self, st: &State) -> Res<i64> {
        match &st.regs[4] {
            IV::Ptr(Region::Stack, o) => Ok(*o),
            _ => Err("esp is not a known stack pointer".into()),
        }
    }

    fn st(&self, st: &State, i: usize) -> Res<String> {
        if i >= st.depth {
            return Err(format!("x87 stack underflow (st({i}) with depth {})", st.depth));
        }
        Ok(format!("f{}", st.depth - 1 - i))
    }

    fn push(&self, st: &mut State, val: String, out: &mut Vec<S>) -> Res<()> {
        if st.depth >= 8 {
            return Err("x87 stack overflow".into());
        }
        st.depth += 1;
        out.push(S::Line(format!("f{} = {val}", st.depth - 1)));
        Ok(())
    }

    fn pop(&self, st: &mut State) -> Res<()> {
        if st.depth == 0 {
            return Err("x87 stack underflow".into());
        }
        st.depth -= 1;
        Ok(())
    }

    /// Address of memory operand `ins`: region and byte offset.
    fn address(&self, ins: &Instruction, st: &State) -> Res<(Region, i64)> {
        let disp = ins.memory_displacement32() as i32 as i64;
        let base = ins.memory_base();
        if base == Register::None {
            return Err("absolute memory address".into());
        }
        let bi = reg_index(base).ok_or("unsupported base register")?;
        let (reg, mut off) = match &st.regs[bi] {
            IV::Ptr(r, o) => (r.clone(), *o),
            other => return Err(format!("memory access through {} which is not a pointer ({other:?})", REG_NAMES[bi])),
        };
        if ins.memory_index() != Register::None {
            let ii = reg_index(ins.memory_index()).ok_or("unsupported index register")?;
            match &st.regs[ii] {
                IV::Int(k) => off += k * ins.memory_index_scale() as i64,
                _ => return Err("memory index depends on data (table lookup)".into()),
            }
        }
        off += disp;
        Ok((reg, off))
    }

    fn rec_field(&self, off: i64, write: bool) -> Res<&'static str> {
        let four_d = self.m.is_4d();
        Ok(match off {
            -56 => "cw",
            -48 => "rold",
            -32 => "x",
            -24 => "y",
            -16 => "z",
            -8 => {
                if four_d {
                    "w"
                } else {
                    "dr"
                }
            }
            0 | 8 | 16 if write => return Err("formula writes the start position C".into()),
            0 => "px",
            8 => "py",
            16 => "pz",
            24 => "cx",
            32 => "cy",
            40 => "cz",
            56 => "rout",
            192 => "a6",
            200 => "a0",
            216 => "a2",
            224 => "a3",
            232 => {
                if four_d {
                    "dr"
                } else {
                    "a4"
                }
            }
            240 => "a5",
            248 => "a7",
            320 => "cx",
            328 => "cy",
            336 => "cz",
            344 => "cw",
            _ => return Err(format!("unsupported iteration record field at offset {off}")),
        })
    }

    /// Reads memory of `size` bytes as `kind` ("f" float, "i" integer, "x" 128-bit).
    fn read(&mut self, ins: &Instruction, st: &State, size: MemorySize) -> Res<Mem> {
        let (region, off) = self.address(ins, st)?;
        match region {
            Region::Rec => match (off, size) {
                (48, MemorySize::UInt32 | MemorySize::Int32 | MemorySize::DwordOffset) => Ok(Mem::I(IV::Ptr(Region::PVar, 0))),
                (64, s) if is_int32(s) => Ok(Mem::I(IV::Var("i".into()))),
                (208, s) if is_int32(s) => Ok(Mem::I(IV::Var("a1".into()))),
                (_, MemorySize::Float64 | MemorySize::UInt64 | MemorySize::Int64) => Ok(Mem::F(self.rec_field(off, false)?.into())),
                (_, MemorySize::Packed128_Float64 | MemorySize::UInt128 | MemorySize::Packed128_UInt64 | MemorySize::Packed128_Float32 | MemorySize::Packed128_Int32 | MemorySize::Packed128_UInt32) => {
                    Ok(Mem::X([self.rec_field(off, false)?.into(), self.rec_field(off + 8, false)?.into()], [None, None]))
                }
                _ => Err(format!("unsupported read of record offset {off} ({size:?})")),
            },
            Region::Stack => {
                let name = format!("m{}", -off);
                match size {
                    s if is_int32(s) => Ok(Mem::I(st.stack_int.get(&off).cloned().unwrap_or(IV::Var(self.stack_var(&name))))),
                    MemorySize::Packed128_Float64 | MemorySize::UInt128 | MemorySize::Packed128_UInt64 => {
                        let hi = format!("m{}", -(off + 8));
                        Ok(Mem::X([self.stack_var(&name), self.stack_var(&hi)], [None, None]))
                    }
                    _ => Ok(Mem::F(self.stack_var(&name))),
                }
            }
            Region::PVar => {
                if off < 0 {
                    let slot = self.layout.iter().find(|s| s.offset == off).cloned();
                    let Some(slot) = slot else { return Err(format!("read of parameter memory at {off} matches no option")) };
                    if slot.expr.is_none() {
                        return Err(format!("parameter memory at {off} holds an unsupported value"));
                    }
                    let name = format!("k{}", -off);
                    self.used_slots.insert(off, name.clone());
                    match (slot.kind, size) {
                        (SlotKind::I32, s) if is_int32(s) => Ok(Mem::I(IV::Var(name))),
                        (SlotKind::I32, _) => Err("integer parameter read as float".into()),
                        (SlotKind::F64, MemorySize::Packed128_Float64 | MemorySize::UInt128) => {
                            let next = self.layout.iter().find(|s| s.offset == off + 8 && s.kind == SlotKind::F64).cloned();
                            let Some(n2) = next else { return Err("128-bit parameter read".into()) };
                            let name2 = format!("k{}", -(off + 8));
                            let _ = n2;
                            self.used_slots.insert(off + 8, name2.clone());
                            Ok(Mem::X([name, name2], [None, None]))
                        }
                        (_, s) if is_int32(s) => Err("float parameter read as integer".into()),
                        _ => Ok(Mem::F(name)),
                    }
                } else {
                    let c = &self.m.constants;
                    let get = |o: i64, n: usize| -> Option<Vec<u8>> {
                        let o = usize::try_from(o).ok()?;
                        Some((0..n).map(|k| c.get(o + k).copied().unwrap_or(0)).collect())
                    };
                    let b = get(off, 16).ok_or("negative constant offset")?;
                    let u64at = |k: usize| u64::from_le_bytes(b[k..k + 8].try_into().unwrap_or([0; 8]));
                    match size {
                        MemorySize::Float64 => Ok(Mem::F(lit(f64::from_bits(u64at(0))))),
                        MemorySize::Float32 => Ok(Mem::F(lit(f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64))),
                        MemorySize::Float80 => Err("80-bit constant".into()),
                        s if is_int32(s) => Ok(Mem::I(IV::Int(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64))),
                        MemorySize::Int16 | MemorySize::UInt16 => Ok(Mem::I(IV::Int(i16::from_le_bytes([b[0], b[1]]) as i64))),
                        MemorySize::Int64 | MemorySize::UInt64 => Ok(Mem::F(lit(u64at(0) as i64 as f64))),
                        _ => {
                            let (l, h) = (u64at(0), u64at(8));
                            Ok(Mem::X([lit(f64::from_bits(l)), lit(f64::from_bits(h))], [Some(l), Some(h)]))
                        }
                    }
                }
            }
        }
    }

    fn stack_var(&mut self, name: &str) -> String {
        self.stack_vars.insert(name.to_string());
        name.to_string()
    }

    /// Writes a float (or 128-bit lanes) to memory.
    fn write_f(&mut self, ins: &Instruction, st: &mut State, lanes: &[String], out: &mut Vec<S>) -> Res<()> {
        let (region, off) = self.address(ins, st)?;
        for (k, v) in lanes.iter().enumerate() {
            let o = off + 8 * k as i64;
            match region {
                Region::Rec => {
                    let f = self.rec_field(o, true)?;
                    out.push(S::Line(format!("{f} = {v}")));
                }
                Region::Stack => {
                    let n = self.stack_var(&format!("m{}", -o));
                    st.stack_int.remove(&o);
                    out.push(S::Line(format!("{n} = {v}")));
                }
                Region::PVar => return Err("write to parameter memory".into()),
            }
        }
        Ok(())
    }

    fn write_i(&mut self, ins: &Instruction, st: &mut State, v: IV, out: &mut Vec<S>) -> Res<()> {
        let (region, off) = self.address(ins, st)?;
        match region {
            Region::Stack => {
                st.stack_int.insert(off, v);
                Ok(())
            }
            Region::Rec if off == 208 || off == 64 => {
                if off == 64 {
                    return Err("formula writes the iteration counter".into());
                }
                let e = self.iv_expr(&v)?;
                out.push(S::Line(format!("a1 = {e}")));
                Ok(())
            }
            _ => Err(format!("integer write to {region:?}{off:+}")),
        }
    }

    fn iv_expr(&self, v: &IV) -> Res<String> {
        match v {
            IV::Int(k) => Ok(lit(*k as f64)),
            IV::Var(s) => Ok(s.clone()),
            _ => Err(format!("integer value {v:?} used as data")),
        }
    }

    /// Integer operand value (register, immediate or memory).
    fn ival(&mut self, ins: &Instruction, op: u32, st: &State) -> Res<IV> {
        match ins.op_kind(op) {
            OpKind::Register => {
                let r = ins.op_register(op);
                let idx = reg_index(r).ok_or_else(|| format!("unsupported register {r:?}"))?;
                let v = st.regs[idx].clone();
                Ok(match (r, v) {
                    (Register::AH | Register::BH | Register::CH | Register::DH, IV::Out(o)) => IV::Out(o.map(|x| (x >> 8) & 0xff)),
                    (Register::AL | Register::BL | Register::CL | Register::DL, IV::Out(o)) => IV::Out(o.map(|x| x & 0xff)),
                    (Register::AX | Register::BX | Register::CX | Register::DX, IV::Out(o)) => IV::Out(o.map(|x| x & 0xffff)),
                    (Register::AH | Register::BH | Register::CH | Register::DH, IV::Int(k)) => IV::Int((k >> 8) & 0xff),
                    (Register::AL | Register::BL | Register::CL | Register::DL, IV::Int(k)) => IV::Int(k & 0xff),
                    (_, v) => v,
                })
            }
            OpKind::Immediate8 | OpKind::Immediate8to32 | OpKind::Immediate32 | OpKind::Immediate16 | OpKind::Immediate8to16 => Ok(IV::Int(ins.immediate(op) as i64 as i32 as i64)),
            OpKind::Memory => match self.read(ins, st, ins.memory_size())? {
                Mem::I(v) => Ok(v),
                _ => Err("float memory used as integer".into()),
            },
            k => Err(format!("unsupported operand kind {k:?}")),
        }
    }

    fn set_reg(&self, st: &mut State, r: Register, v: IV) -> Res<()> {
        let idx = reg_index(r).ok_or_else(|| format!("unsupported register {r:?}"))?;
        let full = matches!(r, Register::EAX | Register::ECX | Register::EDX | Register::EBX | Register::ESP | Register::EBP | Register::ESI | Register::EDI);
        if full {
            st.regs[idx] = v;
            return Ok(());
        }
        // Partial register writes: only on per-outcome / constant values.
        let old = st.regs[idx].clone();
        let combine = |o: i64, n: i64| -> i64 {
            match r {
                Register::AH | Register::BH | Register::CH | Register::DH => (o & !0xff00) | ((n & 0xff) << 8),
                Register::AL | Register::BL | Register::CL | Register::DL => (o & !0xff) | (n & 0xff),
                _ => (o & !0xffff) | (n & 0xffff),
            }
        };
        st.regs[idx] = match (old, v) {
            (IV::Out(o), IV::Out(n)) => IV::Out(std::array::from_fn(|k| combine(o[k], n[k]))),
            (IV::Out(o), IV::Int(n)) => IV::Out(o.map(|x| combine(x, n))),
            (IV::Int(o), IV::Out(n)) => IV::Out(n.map(|x| combine(o, x))),
            (IV::Int(o), IV::Int(n)) => IV::Int(combine(o, n)),
            // The other bits held a pointer or data: they are meaningless next to a status
            // word or a byte constant, so they read as 0.
            (_, IV::Out(n)) => IV::Out(n.map(|x| combine(0, x))),
            (_, IV::Int(n)) => IV::Int(combine(0, n)),
            _ => IV::Undef,
        };
        Ok(())
    }

    /// Integer ALU on static / per-outcome values; sets the flags.
    fn alu(&mut self, ins: &Instruction, st: &mut State, out: &mut Vec<S>) -> Res<()> {
        let mn = ins.mnemonic();
        let bits: u32 = match ins.op_kind(0) {
            OpKind::Register => match ins.op_register(0) {
                Register::AL | Register::AH | Register::BL | Register::BH | Register::CL | Register::CH | Register::DL | Register::DH => 8,
                Register::AX | Register::BX | Register::CX | Register::DX | Register::SI | Register::DI | Register::SP | Register::BP => 16,
                _ => 32,
            },
            _ => match ins.memory_size() {
                MemorySize::UInt8 | MemorySize::Int8 => 8,
                MemorySize::UInt16 | MemorySize::Int16 => 16,
                _ => 32,
            },
        };
        let a = self.ival(ins, 0, st)?;
        let b = if matches!(mn, Mnemonic::Inc | Mnemonic::Dec | Mnemonic::Neg | Mnemonic::Not) {
            IV::Int(match mn {
                Mnemonic::Inc | Mnemonic::Dec => 1,
                _ => 0,
            })
        } else if ins.op_count() > 1 {
            self.ival(ins, 1, st)?
        } else {
            IV::Int(1)
        };
        // Pointer arithmetic.
        if let IV::Ptr(r, o) = &a {
            let k = match &b {
                IV::Int(k) => *k,
                _ => return Err("pointer arithmetic with data".into()),
            };
            let nv = match mn {
                Mnemonic::Add => IV::Ptr(r.clone(), o + k),
                Mnemonic::Sub => IV::Ptr(r.clone(), o - k),
                Mnemonic::Inc => IV::Ptr(r.clone(), o + 1),
                Mnemonic::Dec => IV::Ptr(r.clone(), o - 1),
                Mnemonic::Cmp | Mnemonic::Test => return Err("compare on a pointer".into()),
                _ => return Err(format!("{mn:?} on a pointer")),
            };
            if ins.op_kind(0) == OpKind::Register {
                self.set_reg(st, ins.op_register(0), nv)?;
            }
            st.flags = Flags::None;
            return Ok(());
        }
        // Data (variables): only compare/test produce conditions; add/sub/inc/dec compute.
        let is_data = |v: &IV| matches!(v, IV::Var(_));
        if is_data(&a) || is_data(&b) {
            let ea = self.iv_expr(&a)?;
            let eb = self.iv_expr(&b)?;
            match mn {
                Mnemonic::Cmp => st.flags = Flags::Cmp(ea, eb),
                Mnemonic::Test if ea == eb => st.flags = Flags::Cmp(ea, "0.0".into()),
                Mnemonic::Add | Mnemonic::Sub | Mnemonic::Inc | Mnemonic::Dec => {
                    let op = if matches!(mn, Mnemonic::Add | Mnemonic::Inc) { "+" } else { "-" };
                    let res = format!("{ea} {op} {eb}");
                    match ins.op_kind(0) {
                        OpKind::Register => {
                            let idx = reg_index(ins.op_register(0)).ok_or("register")?;
                            let name = format!("r_{}", REG_NAMES[idx]);
                            out.push(S::Line(format!("{name} = {res}")));
                            st.regs[idx] = IV::Var(name.clone());
                            st.flags = Flags::Cmp(name, "0.0".into());
                        }
                        OpKind::Memory => {
                            let (region, off) = self.address(ins, st)?;
                            if region == Region::Rec && off == 208 {
                                out.push(S::Line(format!("a1 = {res}")));
                                st.flags = Flags::Cmp("a1".into(), "0.0".into());
                            } else if region == Region::Stack {
                                let n = self.stack_var(&format!("m{}", -off));
                                out.push(S::Line(format!("{n} = {res}")));
                                st.stack_int.insert(off, IV::Var(n.clone()));
                                st.flags = Flags::Cmp(n, "0.0".into());
                            } else {
                                return Err("integer arithmetic on memory".into());
                            }
                        }
                        _ => return Err("operand".into()),
                    }
                }
                _ => return Err(format!("{mn:?} on data")),
            }
            return Ok(());
        }
        // Static and per-outcome values: compute concretely per outcome.
        let av = match &a {
            IV::Int(k) => [*k; OUTCOMES],
            IV::Out(o) => *o,
            _ => [0; OUTCOMES],
        };
        let bv = match &b {
            IV::Int(k) => [*k; OUTCOMES],
            IV::Out(o) => *o,
            _ => [0; OUTCOMES],
        };
        if matches!(a, IV::Undef) && !matches!(mn, Mnemonic::Xor) {
            return Err(format!("{mn:?} on an unknown value"));
        }
        let mask: i64 = if bits == 32 { 0xffff_ffff } else { (1 << bits) - 1 };
        let sign: i64 = 1 << (bits - 1);
        let mut res = [0i64; OUTCOMES];
        let mut fl = [0u32; OUTCOMES];
        let xor_self = mn == Mnemonic::Xor && ins.op_kind(0) == OpKind::Register && ins.op_kind(1) == OpKind::Register && ins.op_register(0) == ins.op_register(1);
        for k in 0..OUTCOMES {
            let (x, y) = (av[k] & mask, bv[k] & mask);
            let (r, cf, of) = match mn {
                Mnemonic::Add => {
                    let r = x + y;
                    (r, r > mask, false)
                }
                Mnemonic::Sub | Mnemonic::Cmp => (x - y, x < y, ((x ^ y) & (x ^ (x - y)) & sign) != 0),
                Mnemonic::And | Mnemonic::Test => (x & y, false, false),
                Mnemonic::Or => (x | y, false, false),
                Mnemonic::Xor => (if xor_self { 0 } else { x ^ y }, false, false),
                Mnemonic::Shr => {
                    let n = (y & 31) as u32;
                    (if n == 0 { x } else { x >> n }, n > 0 && (x >> (n - 1)) & 1 == 1, false)
                }
                Mnemonic::Shl => {
                    let n = (y & 31) as u32;
                    (x << n, n > 0 && (x << (n - 1)) & sign != 0, false)
                }
                Mnemonic::Sar => {
                    let n = (y & 31) as u32;
                    let sx = if x & sign != 0 { x - (mask + 1) } else { x };
                    (sx >> n, n > 0 && (sx >> (n - 1)) & 1 == 1, false)
                }
                Mnemonic::Inc => (x + 1, false, false),
                Mnemonic::Dec => (x - 1, false, false),
                Mnemonic::Neg => (-x, x != 0, false),
                Mnemonic::Not => (!x, false, false),
                _ => return Err(format!("unsupported integer instruction {mn:?}")),
            };
            let rm = r & mask;
            let mut f = 0;
            if rm == 0 {
                f |= ZF;
            }
            if rm & sign != 0 {
                f |= SF;
            }
            if cf {
                f |= CF;
            }
            if of {
                f |= OF;
            }
            if (rm & 0xff).count_ones().is_multiple_of(2) {
                f |= PF;
            }
            res[k] = rm;
            fl[k] = f;
        }
        if mn != Mnemonic::Not {
            st.flags = Flags::Out(fl);
        }
        if !matches!(mn, Mnemonic::Cmp | Mnemonic::Test) {
            let v = if res.iter().all(|r| *r == res[0]) { IV::Int(res[0]) } else { IV::Out(res) };
            match ins.op_kind(0) {
                OpKind::Register => self.set_reg(st, ins.op_register(0), v)?,
                OpKind::Memory => self.write_i(ins, st, v, out)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn jcc_outcome(&self, ins: &Instruction, st: &State) -> Res<Cond> {
        let mn = ins.mnemonic();
        let test = |f: u32| -> bool {
            let (cf, zf, sf, of, pf) = (f & CF != 0, f & ZF != 0, f & SF != 0, f & OF != 0, f & PF != 0);
            match mn {
                Mnemonic::Jo => of,
                Mnemonic::Jno => !of,
                Mnemonic::Jb => cf,
                Mnemonic::Jae => !cf,
                Mnemonic::Je => zf,
                Mnemonic::Jne => !zf,
                Mnemonic::Jbe => cf || zf,
                Mnemonic::Ja => !cf && !zf,
                Mnemonic::Js => sf,
                Mnemonic::Jns => !sf,
                Mnemonic::Jp => pf,
                Mnemonic::Jnp => !pf,
                Mnemonic::Jl => sf != of,
                Mnemonic::Jge => sf == of,
                Mnemonic::Jle => zf || sf != of,
                Mnemonic::Jg => !zf && sf == of,
                _ => false,
            }
        };
        match &st.flags {
            Flags::Out(f) => {
                let t: Vec<bool> = f.iter().map(|x| test(*x)).collect();
                // Ordered outcomes decide; unordered (NaN) follows "greater" for the condition.
                Ok(match (t[0], t[1], t[2]) {
                    (true, true, true) => Cond::Always,
                    (false, false, false) => Cond::Never,
                    (true, false, false) => Cond::Expr("fc_a > fc_b".into()),
                    (false, true, false) => Cond::Expr("fc_a < fc_b".into()),
                    (false, false, true) => Cond::Expr("fc_a == fc_b".into()),
                    (true, true, false) => Cond::Expr("fc_a != fc_b".into()),
                    (true, false, true) => Cond::Expr("fc_a >= fc_b".into()),
                    (false, true, true) => Cond::Expr("fc_a <= fc_b".into()),
                })
            }
            Flags::Cmp(a, b) => {
                let c = match mn {
                    Mnemonic::Je => "==",
                    Mnemonic::Jne => "!=",
                    Mnemonic::Jb | Mnemonic::Jl | Mnemonic::Js => "<",
                    Mnemonic::Jae | Mnemonic::Jge | Mnemonic::Jns => ">=",
                    Mnemonic::Jbe | Mnemonic::Jle => "<=",
                    Mnemonic::Ja | Mnemonic::Jg => ">",
                    _ => return Err(format!("{mn:?} after an integer compare")),
                };
                Ok(Cond::Expr(format!("{a} {c} {b}")))
            }
            Flags::None => Err(format!("conditional jump without known flags: {}", fmt_ins(ins))),
        }
    }

    /// Sets the flags of an FPU compare `a` vs `b` (also stores the operands for the condition).
    fn fcompare(&self, st: &mut State, a: String, b: String, eflags: bool, out: &mut Vec<S>) {
        out.push(S::Line(format!("fc_a = {a}")));
        out.push(S::Line(format!("fc_b = {b}")));
        if eflags {
            // fcomi: ZF PF CF
            st.flags = Flags::Out([0, CF, ZF, ZF | PF | CF]);
        } else {
            // Status word bits in ax after fnstsw: C0 = 0x100, C2 = 0x400, C3 = 0x4000.
            st.fpu_status = [0, 0x100, 0x4000, 0x4500];
        }
    }

    fn fval(&mut self, ins: &Instruction, op: u32, st: &State) -> Res<String> {
        match ins.op_kind(op) {
            OpKind::Register => {
                let r = ins.op_register(op);
                let i = (r as u32).wrapping_sub(Register::ST0 as u32) as usize;
                if i > 7 {
                    return Err(format!("unexpected register {r:?}"));
                }
                self.st(st, i)
            }
            OpKind::Memory => {
                let size = ins.memory_size();
                match self.read(ins, st, size)? {
                    Mem::F(s) => Ok(s),
                    Mem::I(v) => self.iv_expr(&v),
                    Mem::X(l, _) => Ok(l[0].clone()),
                }
            }
            k => Err(format!("unsupported x87 operand {k:?}")),
        }
    }

    fn st_index(r: Register) -> Option<usize> {
        let i = (r as u32).wrapping_sub(Register::ST0 as u32) as usize;
        (i < 8).then_some(i)
    }

    fn xmm_index(r: Register) -> Option<usize> {
        let i = (r as u32).wrapping_sub(Register::XMM0 as u32) as usize;
        (i < 8).then_some(i)
    }

    /// Lane expressions of an xmm/m128/m64 source operand.
    fn xsrc(&mut self, ins: &Instruction, op: u32, st: &State) -> Res<([String; 2], [Option<u64>; 2])> {
        match ins.op_kind(op) {
            OpKind::Register => {
                let x = Self::xmm_index(ins.op_register(op)).ok_or("expected an xmm register")?;
                Ok(([format!("x{x}l"), format!("x{x}h")], st.xmm_known[x]))
            }
            OpKind::Memory => match self.read(ins, st, ins.memory_size())? {
                Mem::X(l, k) => Ok((l, k)),
                Mem::F(s) => Ok(([s, "0.0".into()], [None, None])),
                Mem::I(_) => Err("integer memory as xmm operand".into()),
            },
            k => Err(format!("unsupported SSE operand {k:?}")),
        }
    }

    fn exec(&mut self, ins: &Instruction, st: &mut State, out: &mut Vec<S>) -> Res<()> {
        use Mnemonic::*;
        let mn = ins.mnemonic();
        match mn {
            Nop | Fnop | Wait | Ffree | Ffreep | Fninit | Fnclex | Fldcw | Fnstcw | Emms | Cld => Ok(()),
            // ---------------------------------------------------------------- integer
            Push => {
                let v = self.ival(ins, 0, st).unwrap_or(IV::Undef);
                let esp = self.esp(st)? - 4;
                st.stack_int.insert(esp, v);
                st.regs[4] = IV::Ptr(Region::Stack, esp);
                Ok(())
            }
            Pop => {
                let esp = self.esp(st)?;
                let v = st.stack_int.get(&esp).cloned().unwrap_or(IV::Undef);
                st.regs[4] = IV::Ptr(Region::Stack, esp + 4);
                match ins.op_kind(0) {
                    OpKind::Register => self.set_reg(st, ins.op_register(0), v),
                    _ => self.write_i(ins, st, v, out),
                }
            }
            Pushad => {
                let esp = self.esp(st)?;
                for (k, r) in [0usize, 1, 2, 3, 4, 5, 6, 7].iter().enumerate() {
                    st.stack_int.insert(esp - 4 * (k as i64 + 1), st.regs[*r].clone());
                }
                st.regs[4] = IV::Ptr(Region::Stack, esp - 32);
                Ok(())
            }
            Popad => {
                let esp = self.esp(st)?;
                for (k, r) in [7usize, 6, 5, 4, 3, 2, 1, 0].iter().enumerate() {
                    if *r != 4 {
                        st.regs[*r] = st.stack_int.get(&(esp + 4 * k as i64)).cloned().unwrap_or(IV::Undef);
                    }
                }
                st.regs[4] = IV::Ptr(Region::Stack, esp + 32);
                Ok(())
            }
            Mov => {
                if ins.op_kind(0) == OpKind::Register {
                    let v = self.ival(ins, 1, st)?;
                    let v = match v {
                        // Copy data into the destination register's own variable.
                        IV::Var(s) if !s.starts_with("r_") || reg_index(ins.op_register(0)).is_some_and(|i| s != format!("r_{}", REG_NAMES[i])) => {
                            let idx = reg_index(ins.op_register(0)).ok_or("register")?;
                            let name = format!("r_{}", REG_NAMES[idx]);
                            out.push(S::Line(format!("{name} = {s}")));
                            IV::Var(name)
                        }
                        v => v,
                    };
                    self.set_reg(st, ins.op_register(0), v)
                } else {
                    let v = self.ival(ins, 1, st)?;
                    self.write_i(ins, st, v, out)
                }
            }
            Movzx | Movsx => {
                let v = self.ival(ins, 1, st)?;
                self.set_reg(st, ins.op_register(0), v)
            }
            Lea => {
                let (r, o) = self.address(ins, st)?;
                self.set_reg(st, ins.op_register(0), IV::Ptr(r, o))
            }
            Xchg => {
                let a = self.ival(ins, 0, st)?;
                let b = self.ival(ins, 1, st)?;
                if ins.op_kind(0) == OpKind::Register && ins.op_kind(1) == OpKind::Register {
                    self.set_reg(st, ins.op_register(0), b)?;
                    self.set_reg(st, ins.op_register(1), a)
                } else {
                    Err("xchg with memory".into())
                }
            }
            Add | Sub | And | Or | Xor | Cmp | Test | Shr | Shl | Sar | Inc | Dec | Neg | Not => self.alu(ins, st, out),
            Sahf => {
                let ax = st.regs[0].clone();
                let IV::Out(o) = ax else { return Err("sahf without a status word in ah".into()) };
                // AH bits 7, 6, 4, 2, 0 -> SF ZF AF PF CF.
                st.flags = Flags::Out(o.map(|v| {
                    let ah = ((v >> 8) & 0xff) as u32;
                    (if ah & 0x80 != 0 { SF } else { 0 }) | (if ah & 0x40 != 0 { ZF } else { 0 }) | (if ah & 4 != 0 { PF } else { 0 }) | (if ah & 1 != 0 { CF } else { 0 })
                }));
                Ok(())
            }
            Fnstsw | Fstsw => {
                // Status word: compare bits plus TOP.
                let top = ((8 - st.depth as i64) & 7) << 11;
                let v = IV::Out(st.fpu_status.map(|c| c | top));
                if ins.op_kind(0) == OpKind::Register {
                    self.set_reg(st, Register::AX, v)
                } else {
                    self.write_i(ins, st, v, out)
                }
            }
            // ---------------------------------------------------------------- x87 loads/stores
            Fld => {
                let v = self.fval(ins, 0, st)?;
                self.push(st, v, out)
            }
            Fild => {
                let v = match self.read(ins, st, ins.memory_size())? {
                    Mem::I(v) => self.iv_expr(&v)?,
                    Mem::F(s) => s,
                    Mem::X(..) => return Err("fild of 128 bits".into()),
                };
                self.push(st, v, out)
            }
            Fld1 => self.push(st, "1.0".into(), out),
            Fldz => self.push(st, "0.0".into(), out),
            Fldpi => self.push(st, lit(std::f64::consts::PI), out),
            Fldl2e => self.push(st, lit(std::f64::consts::LOG2_E), out),
            Fldln2 => self.push(st, lit(std::f64::consts::LN_2), out),
            Fldlg2 => self.push(st, lit(std::f64::consts::LOG10_2), out),
            Fldl2t => self.push(st, lit(std::f64::consts::LOG2_10), out),
            Fst | Fstp => {
                let v = self.st(st, 0)?;
                match ins.op_kind(0) {
                    OpKind::Register => {
                        let i = Self::st_index(ins.op_register(0)).ok_or("register")?;
                        let d = self.st(st, i)?;
                        if d != v {
                            out.push(S::Line(format!("{d} = {v}")));
                        }
                    }
                    _ => {
                        self.write_f(ins, st, &[v], out)?;
                    }
                }
                if mn == Fstp {
                    self.pop(st)?;
                }
                Ok(())
            }
            Fist | Fistp | Fisttp => {
                let v = self.st(st, 0)?;
                let e = if mn == Fisttp { format!("sel({v} < 0.0, -floor(-{v}), floor({v}))") } else { format!("round({v})") };
                let (region, off) = self.address(ins, st)?;
                if region != Region::Stack {
                    return Err("fist to non-stack memory".into());
                }
                let n = self.stack_var(&format!("m{}", -off));
                out.push(S::Line(format!("{n} = {e}")));
                st.stack_int.insert(off, IV::Var(n));
                if mn != Fist {
                    self.pop(st)?;
                }
                Ok(())
            }
            Fxch => {
                // iced lists both operands (st(0), st(i)); use the one that is not st(0).
                let i = (0..ins.op_count()).filter_map(|k| Self::st_index(ins.op_register(k))).find(|&k| k != 0).unwrap_or(1);
                let a = self.st(st, 0)?;
                let b = self.st(st, i)?;
                if a != b {
                    out.push(S::Line(format!("ft = {a}")));
                    out.push(S::Line(format!("{a} = {b}")));
                    out.push(S::Line(format!("{b} = ft")));
                }
                Ok(())
            }
            Fincstp => self.pop(st),
            Fdecstp => {
                if st.depth >= 8 {
                    return Err("x87 stack overflow".into());
                }
                st.depth += 1;
                Ok(())
            }
            // ---------------------------------------------------------------- x87 arithmetic
            Fadd | Faddp | Fsub | Fsubp | Fsubr | Fsubrp | Fmul | Fmulp | Fdiv | Fdivp | Fdivr | Fdivrp | Fiadd | Fisub | Fisubr | Fimul | Fidiv | Fidivr => {
                let (op, rev) = match mn {
                    Fadd | Faddp | Fiadd => ("+", false),
                    Fsub | Fsubp | Fisub => ("-", false),
                    Fsubr | Fsubrp | Fisubr => ("-", true),
                    Fmul | Fmulp | Fimul => ("*", false),
                    Fdiv | Fdivp | Fidiv => ("/", false),
                    _ => ("/", true),
                };
                let pop = matches!(mn, Faddp | Fsubp | Fsubrp | Fmulp | Fdivp | Fdivrp);
                let (dst, src) = if ins.op_count() == 2 {
                    (Self::st_index(ins.op_register(0)).ok_or("dst")?, self.fval(ins, 1, st)?)
                } else if ins.op_count() == 1 && ins.op_kind(0) == OpKind::Memory {
                    (0, self.fval(ins, 0, st)?)
                } else {
                    (1, self.st(st, 0)?)
                };
                let d = self.st(st, dst)?;
                let e = if rev { format!("{src} {op} {d}") } else { format!("{d} {op} {src}") };
                out.push(S::Line(format!("{d} = {e}")));
                if pop {
                    self.pop(st)?;
                }
                Ok(())
            }
            Fchs | Fabs | Fsqrt | Fsin | Fcos | Frndint | F2xm1 => {
                let a = self.st(st, 0)?;
                let e = match mn {
                    Fchs => format!("-{a}"),
                    Fabs => format!("abs({a})"),
                    Fsqrt => format!("sqrt({a})"),
                    Fsin => format!("sin({a})"),
                    Fcos => format!("cos({a})"),
                    Frndint => format!("round({a})"),
                    _ => format!("pow(2.0, {a}) - 1.0"),
                };
                out.push(S::Line(format!("{a} = {e}")));
                Ok(())
            }
            Fsincos => {
                let a = self.st(st, 0)?;
                self.push(st, format!("cos({a})"), out)?;
                out.push(S::Line(format!("{a} = sin({a})")));
                Ok(())
            }
            Fptan => {
                let a = self.st(st, 0)?;
                out.push(S::Line(format!("{a} = tan({a})")));
                self.push(st, "1.0".into(), out)
            }
            Fpatan => {
                let (x, y) = (self.st(st, 0)?, self.st(st, 1)?);
                out.push(S::Line(format!("{y} = atan2({y}, {x})")));
                self.pop(st)
            }
            Fyl2x => {
                let (x, y) = (self.st(st, 0)?, self.st(st, 1)?);
                out.push(S::Line(format!("{y} = {y} * log({x}) * {}", lit(std::f64::consts::LOG2_E))));
                self.pop(st)
            }
            Fyl2xp1 => {
                let (x, y) = (self.st(st, 0)?, self.st(st, 1)?);
                out.push(S::Line(format!("{y} = {y} * log({x} + 1.0) * {}", lit(std::f64::consts::LOG2_E))));
                self.pop(st)
            }
            Fscale => {
                let (a, b) = (self.st(st, 0)?, self.st(st, 1)?);
                out.push(S::Line(format!("{a} = {a} * pow(2.0, sel({b} < 0.0, -floor(-{b}), floor({b})))")));
                Ok(())
            }
            Fprem | Fprem1 => {
                let (a, b) = (self.st(st, 0)?, self.st(st, 1)?);
                let e = if mn == Fprem { format!("{a} % {b}") } else { format!("{a} - {b} * round({a} / {b})") };
                out.push(S::Line(format!("{a} = {e}")));
                // The reduction is complete: C2 = 0.
                st.fpu_status = [0; OUTCOMES];
                Ok(())
            }
            Fxtract => Err("fxtract".into()),
            // ---------------------------------------------------------------- x87 compares
            Fcom | Fcomp | Fcompp | Fucom | Fucomp | Fucompp | Ficom | Ficomp => {
                let a = self.st(st, 0)?;
                let b = if ins.op_count() == 0 {
                    self.st(st, 1)?
                } else if ins.op_count() == 2 {
                    self.fval(ins, 1, st)?
                } else {
                    self.fval(ins, 0, st)?
                };
                self.fcompare(st, a, b, false, out);
                match mn {
                    Fcomp | Fucomp | Ficomp => self.pop(st)?,
                    Fcompp | Fucompp => {
                        self.pop(st)?;
                        self.pop(st)?;
                    }
                    _ => {}
                }
                Ok(())
            }
            Fcomi | Fcomip | Fucomi | Fucomip => {
                let a = self.st(st, 0)?;
                let b = self.fval(ins, 1, st)?;
                self.fcompare(st, a, b, true, out);
                if matches!(mn, Fcomip | Fucomip) {
                    self.pop(st)?;
                }
                Ok(())
            }
            Ftst => {
                let a = self.st(st, 0)?;
                self.fcompare(st, a, "0.0".into(), false, out);
                Ok(())
            }
            Fxam => Err("fxam".into()),
            // ---------------------------------------------------------------- SSE2
            Movsd | Movlpd | Movhpd | Movupd | Movapd | Movaps | Movups | Movq | Movddup | Movlps | Movhps => self.sse_move(ins, st, out),
            Addsd | Subsd | Mulsd | Divsd | Minsd | Maxsd | Sqrtsd | Addpd | Subpd | Mulpd | Divpd | Minpd | Maxpd | Sqrtpd => self.sse_arith(ins, st, out),
            Andpd | Andps | Andnpd | Andnps | Orpd | Orps | Xorpd | Xorps => self.sse_logic(ins, st, out),
            Unpcklpd | Unpckhpd | Shufpd | Haddpd | Pshufd => self.sse_shuffle(ins, st, out),
            Comisd | Ucomisd => {
                let x = Self::xmm_index(ins.op_register(0)).ok_or("xmm")?;
                let (b, _) = self.xsrc(ins, 1, st)?;
                self.fcompare(st, format!("x{x}l"), b[0].clone(), true, out);
                Ok(())
            }
            Cvtsi2sd => {
                let x = Self::xmm_index(ins.op_register(0)).ok_or("xmm")?;
                let v = self.ival(ins, 1, st)?;
                let e = self.iv_expr(&v)?;
                out.push(S::Line(format!("x{x}l = {e}")));
                st.xmm_known[x][0] = None;
                Ok(())
            }
            _ => Err(format!("unsupported instruction {mn:?}")),
        }
    }

    fn sse_move(&mut self, ins: &Instruction, st: &mut State, out: &mut Vec<S>) -> Res<()> {
        use Mnemonic::*;
        let mn = ins.mnemonic();
        let lanes_of = |mn: Mnemonic, ms: MemorySize| -> usize {
            match mn {
                Movsd | Movlpd | Movhpd | Movq | Movlps | Movhps => 1,
                Movddup => 1,
                _ => {
                    if ms == MemorySize::Float64 || ms == MemorySize::UInt64 {
                        1
                    } else {
                        2
                    }
                }
            }
        };
        match (ins.op_kind(0), ins.op_kind(1)) {
            (OpKind::Register, _) => {
                let x = Self::xmm_index(ins.op_register(0)).ok_or("expected xmm destination")?;
                let (src, known) = self.xsrc(ins, 1, st)?;
                let from_mem = ins.op_kind(1) == OpKind::Memory;
                let (l, h) = (format!("x{x}l"), format!("x{x}h"));
                let set = |name: &str, v: &str, out: &mut Vec<S>| {
                    if name != v {
                        out.push(S::Line(format!("{name} = {v}")));
                    }
                };
                match mn {
                    Movhpd | Movhps => {
                        set(&h, &src[0], out);
                        st.xmm_known[x][1] = known[0];
                    }
                    Movlpd | Movlps => {
                        set(&l, &src[0], out);
                        st.xmm_known[x][0] = known[0];
                    }
                    Movddup => {
                        let tmp = src[0].clone();
                        set(&l, &tmp, out);
                        set(&h, &tmp, out);
                        st.xmm_known[x] = [known[0], known[0]];
                    }
                    Movsd | Movq => {
                        set(&l, &src[0], out);
                        st.xmm_known[x][0] = known[0];
                        if from_mem {
                            set(&h, "0.0", out);
                            st.xmm_known[x][1] = Some(0);
                        }
                    }
                    _ => {
                        if lanes_of(mn, ins.memory_size()) == 2 || !from_mem {
                            // Copy through temporaries in case of overlap.
                            if src[0] == h {
                                out.push(S::Line(format!("ft = {}", src[0])));
                                set(&h, &src[1], out);
                                set(&l, "ft", out);
                            } else {
                                set(&l, &src[0], out);
                                set(&h, &src[1], out);
                            }
                            st.xmm_known[x] = known;
                        } else {
                            set(&l, &src[0], out);
                            st.xmm_known[x][0] = known[0];
                        }
                    }
                }
                Ok(())
            }
            (OpKind::Memory, OpKind::Register) => {
                let x = Self::xmm_index(ins.op_register(1)).ok_or("expected xmm source")?;
                let lanes = match mn {
                    Movhpd | Movhps => vec![format!("x{x}h")],
                    Movsd | Movlpd | Movq | Movlps => vec![format!("x{x}l")],
                    _ => vec![format!("x{x}l"), format!("x{x}h")],
                };
                self.write_f(ins, st, &lanes, out)
            }
            _ => Err("unsupported SSE move".into()),
        }
    }

    fn sse_arith(&mut self, ins: &Instruction, st: &mut State, out: &mut Vec<S>) -> Res<()> {
        use Mnemonic::*;
        let mn = ins.mnemonic();
        let x = Self::xmm_index(ins.op_register(0)).ok_or("expected xmm destination")?;
        let (src, _) = self.xsrc(ins, 1, st)?;
        let packed = matches!(mn, Addpd | Subpd | Mulpd | Divpd | Minpd | Maxpd | Sqrtpd);
        let lanes = if packed { 2 } else { 1 };
        // Compute into temporaries first (source and destination may be the same register).
        let mut res = Vec::new();
        #[allow(clippy::needless_range_loop)]
        for k in 0..lanes {
            let d = if k == 0 { format!("x{x}l") } else { format!("x{x}h") };
            let s = &src[k];
            let e = match mn {
                Addsd | Addpd => format!("{d} + {s}"),
                Subsd | Subpd => format!("{d} - {s}"),
                Mulsd | Mulpd => format!("{d} * {s}"),
                Divsd | Divpd => format!("{d} / {s}"),
                Minsd | Minpd => format!("sel({d} < {s}, {d}, {s})"),
                Maxsd | Maxpd => format!("sel({d} > {s}, {d}, {s})"),
                _ => format!("sqrt({s})"),
            };
            res.push((d, e));
        }
        if res.len() == 2 {
            out.push(S::Line(format!("ft = {}", res[1].1)));
            out.push(S::Line(format!("{} = {}", res[0].0, res[0].1)));
            out.push(S::Line(format!("{} = ft", res[1].0)));
        } else {
            out.push(S::Line(format!("{} = {}", res[0].0, res[0].1)));
        }
        for k in 0..lanes {
            st.xmm_known[x][k] = None;
        }
        Ok(())
    }

    fn sse_logic(&mut self, ins: &Instruction, st: &mut State, out: &mut Vec<S>) -> Res<()> {
        use Mnemonic::*;
        let mn = ins.mnemonic();
        let x = Self::xmm_index(ins.op_register(0)).ok_or("expected xmm destination")?;
        let same = ins.op_kind(1) == OpKind::Register && ins.op_register(1) == ins.op_register(0);
        if same && matches!(mn, Xorpd | Xorps | Andnpd | Andnps) {
            out.push(S::Line(format!("x{x}l = 0.0")));
            out.push(S::Line(format!("x{x}h = 0.0")));
            st.xmm_known[x] = [Some(0), Some(0)];
            return Ok(());
        }
        let (_, known) = self.xsrc(ins, 1, st)?;
        const ABS: u64 = 0x7fff_ffff_ffff_ffff;
        const SIGN: u64 = 0x8000_0000_0000_0000;
        for (k, kn) in known.iter().enumerate() {
            let d = if k == 0 { format!("x{x}l") } else { format!("x{x}h") };
            let line = match (mn, *kn) {
                (Andpd | Andps, Some(ABS)) => format!("{d} = abs({d})"),
                (Andpd | Andps, Some(u64::MAX)) => continue,
                (Andpd | Andps, Some(0)) => format!("{d} = 0.0"),
                (Xorpd | Xorps, Some(SIGN)) => format!("{d} = -{d}"),
                (Xorpd | Xorps | Orpd | Orps, Some(0)) => continue,
                (Orpd | Orps, Some(SIGN)) => format!("{d} = -abs({d})"),
                (Andnpd | Andnps, Some(SIGN)) if st.xmm_known[x][k].is_none() => {
                    return Err("andnpd with a variable mask".into());
                }
                _ => return Err(format!("{mn:?} with an unknown mask")),
            };
            out.push(S::Line(line));
            st.xmm_known[x][k] = None;
        }
        Ok(())
    }

    fn sse_shuffle(&mut self, ins: &Instruction, st: &mut State, out: &mut Vec<S>) -> Res<()> {
        use Mnemonic::*;
        let mn = ins.mnemonic();
        let x = Self::xmm_index(ins.op_register(0)).ok_or("expected xmm destination")?;
        let (src, known) = self.xsrc(ins, 1, st)?;
        let (l, h) = (format!("x{x}l"), format!("x{x}h"));
        let (nl, nh, kl, kh) = match mn {
            Unpcklpd => (l.clone(), src[0].clone(), st.xmm_known[x][0], known[0]),
            Unpckhpd => (h.clone(), src[1].clone(), st.xmm_known[x][1], known[1]),
            Haddpd => (format!("{l} + {h}"), format!("{} + {}", src[0], src[1]), None, None),
            Shufpd => {
                let imm = ins.immediate(2);
                let a = if imm & 1 == 0 { l.clone() } else { h.clone() };
                let b = if imm & 2 == 0 { src[0].clone() } else { src[1].clone() };
                (a, b, None, None)
            }
            _ => {
                // pshufd on doubles: dword selectors in pairs.
                let imm = ins.immediate(2);
                let lane = |sel: u64| -> Res<String> {
                    match sel {
                        0b0100 => Ok(src[0].clone()),
                        0b1110 => Ok(src[1].clone()),
                        _ => Err("pshufd that splits doubles".into()),
                    }
                };
                (lane(imm & 0xf)?, lane((imm >> 4) & 0xf)?, None, None)
            }
        };
        out.push(S::Line(format!("ft = {nh}")));
        if nl != l {
            out.push(S::Line(format!("{l} = {nl}")));
        }
        out.push(S::Line(format!("{h} = ft")));
        st.xmm_known[x] = [kl, kh];
        Ok(())
    }
}

enum Cond {
    Always,
    Never,
    Expr(String),
}

fn is_int32(s: MemorySize) -> bool {
    matches!(s, MemorySize::Int32 | MemorySize::UInt32 | MemorySize::DwordOffset)
}
