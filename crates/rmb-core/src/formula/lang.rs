// SPDX-License-Identifier: LGPL-2.1-or-later

//! The formula language: a small scalar language for fractal iterations.
//!
//! One formula source compiles to WGSL (GPU renderer) and to closures over `f64` registers
//! (CPU renderer, picking, tests), so both renderers always iterate the same math.
//!
//! ```text
//! formula "Amazing Box" {
//!     category "Boxes"
//!     de linear
//!     alias "Amazing Box"
//!     param scale "Scale" = 2.0 [-4, 4]
//!     param fold "Fold" = 1.0 [0, 3]
//!     init { let fold2 = fold * 2.0; }
//!     iter {
//!         x = abs(x + fold) - abs(x - fold) - x;
//!         if r2 < 1.0 { ... } else { ... }
//!         dr = dr * abs(scale) + 1.0;
//!     }
//! }
//! ```
//!
//! Values are scalars; comparisons and `&&`, `||`, `!` give booleans for `if` and `sel`.
//! State variables: `x y z w` (the point), `cx cy cz cw` (the constant: pixel position or the
//! Julia seed), `dr` (running derivative for the distance estimate), `i` (iteration index,
//! read only), `rout` (squared length after the last counted iteration, kept by the hybrid
//! loop), `rold` (`rout` before this iteration), `px py pz` (the point's original position,
//! read only) and `a0` … `a7` (scratch values kept over the iterations of one point, start at
//! 0; Mandelbulb3D formulas keep e.g. a varying scale there). `let` declares a mutable local. `init` runs once per render on the CPU: its
//! top-level `let`s become read-only constants of `iter` (e.g. a rotation matrix).

use std::fmt;

/// Names of the state registers, in register order.
pub const STATE_NAMES: [&str; 23] = [
    "x", "y", "z", "w", "cx", "cy", "cz", "cw", "dr", "i", "rout", "rold", "px", "py", "pz", "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7",
];
pub const STATE_COUNT: usize = STATE_NAMES.len();
/// Register index of the derivative.
pub const STATE_DR: usize = 8;
/// Register index of the iteration counter (read only).
pub const STATE_ITER: usize = 9;
pub const STATE_ROUT: usize = 10;
pub const STATE_ROLD: usize = 11;
/// First of the three original position registers (read only).
pub const STATE_PX: usize = 12;
/// First of the eight scratch registers.
pub const STATE_AUX: usize = 15;

/// State registers formulas may not assign.
pub fn is_read_only(i: usize) -> bool {
    i == STATE_ITER || (STATE_PX..STATE_PX + 3).contains(&i)
}

#[derive(Debug, Clone)]
pub struct LangError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for LangError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for LangError {}

type Res<T> = Result<T, LangError>;

// ---------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Str(String),
    Sym(&'static str),
    Eof,
}

const SYMBOLS: [&str; 27] = [
    "+=", "-=", "*=", "/=", "==", "!=", "<=", ">=", "&&", "||", "..", "+", "-", "*", "/", "%", "<", ">", "=", "!", "(", ")", "{", "}", "[",
    "]", ",",
];

fn lex(src: &str) -> Res<Vec<(Tok, usize)>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b';' {
            // Statement separators are optional; `;` is accepted and ignored.
            i += 1;
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && b.get(i + 1) != Some(&b'.'))) {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                i += 1;
                if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
                    i += 1;
                }
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let s = &src[start..i];
            let v = s.parse::<f64>().map_err(|_| LangError { line, msg: format!("bad number '{s}'") })?;
            out.push((Tok::Num(v), line));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push((Tok::Ident(src[start..i].to_string()), line));
        } else if c == b'"' {
            let start = i + 1;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            if i >= b.len() {
                return Err(LangError { line, msg: "unterminated string".into() });
            }
            out.push((Tok::Str(src[start..i].to_string()), line));
            i += 1;
        } else {
            let sym = SYMBOLS.iter().find(|s| src[i..].starts_with(**s)).ok_or_else(|| LangError { line, msg: format!("unexpected character '{}'", c as char) })?;
            out.push((Tok::Sym(sym), line));
            i += sym.len();
        }
    }
    out.push((Tok::Eof, line));
    Ok(out)
}

// ---------------------------------------------------------------- AST

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarRef {
    /// State register (`STATE_NAMES`).
    State(usize),
    /// Parameter or `init` constant of the formula (parameters first).
    Param(usize),
    /// Local variable.
    Local(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    Abs,
    Sqrt,
    InvSqrt,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Exp,
    Log,
    Pow,
    Floor,
    Ceil,
    Fract,
    Round,
    Min,
    Max,
    Clamp,
    Sign,
    Sinh,
    Cosh,
    Tanh,
    Mix,
    Step,
    Smoothstep,
    Sqr,
    /// Rounds to 32-bit float precision (Mandelbulb3D's Single options).
    Single,
    Select,
}

impl Func {
    fn from_name(s: &str) -> Option<(Func, usize)> {
        use Func::*;
        Some(match s {
            "abs" => (Abs, 1),
            "sqrt" => (Sqrt, 1),
            "rsqrt" => (InvSqrt, 1),
            "sin" => (Sin, 1),
            "cos" => (Cos, 1),
            "tan" => (Tan, 1),
            "asin" => (Asin, 1),
            "acos" => (Acos, 1),
            "atan" => (Atan, 1),
            "atan2" => (Atan2, 2),
            "exp" => (Exp, 1),
            "log" => (Log, 1),
            "pow" => (Pow, 2),
            "floor" => (Floor, 1),
            "ceil" => (Ceil, 1),
            "fract" => (Fract, 1),
            "round" => (Round, 1),
            "min" => (Min, 2),
            "max" => (Max, 2),
            "clamp" => (Clamp, 3),
            "sign" => (Sign, 1),
            "sinh" => (Sinh, 1),
            "cosh" => (Cosh, 1),
            "tanh" => (Tanh, 1),
            "mix" => (Mix, 3),
            "step" => (Step, 2),
            "smoothstep" => (Smoothstep, 3),
            "sqr" => (Sqr, 1),
            "single" => (Single, 1),
            "sel" => (Select, 3),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64),
    Var(VarRef),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// Declares (and initializes) a local; `Param` targets in `init` define constants.
    Let(VarRef, Expr),
    Assign(VarRef, Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    /// `for v in a..b`: counter local, start, end (exclusive), body.
    For(usize, i64, i64, Vec<Stmt>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum DeKind {
    /// `0.5 * r * ln(r) / dr`: power formulas (bulbs, quaternions).
    #[default]
    Log,
    /// `r / |dr|`: folding / IFS formulas (boxes, sponges).
    Linear,
    /// Numerical gradient of the escape potential (no derivative needed).
    Numeric,
    /// Distance-estimated IFS (Mandelbulb3D's dIFS): shapes write their distance into `rout`,
    /// transforms scale `a0`; the estimate is the smallest `rout / a0` over the iterations.
    Difs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ParamKind {
    #[default]
    Float,
    Int,
    Bool,
    /// Degrees in the UI and in the value.
    Angle,
}

/// Another name of a formula. `params` maps the other program's option order to parameter
/// identifiers (`_` skips an option); empty means "same order".
#[derive(Clone, Debug)]
pub struct Alias {
    pub name: String,
    pub params: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ParamDef {
    pub ident: String,
    pub label: String,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub kind: ParamKind,
}

/// A parsed and resolved formula.
#[derive(Clone, Debug)]
pub struct FormulaDef {
    pub name: String,
    pub category: String,
    pub description: String,
    pub de: DeKind,
    /// Other names (e.g. MB3D formula names) that map to this formula on import.
    pub aliases: Vec<Alias>,
    /// Default iteration count of a hybrid slot with this formula.
    pub iterations: u32,
    /// The formula only transforms the point (no `+ c`): skipped for the escape test and not
    /// counted as an iteration (as in Mandelbulb3D).
    pub transform: bool,
    /// Multiplier of the distance estimate when this formula is in the hybrid.
    pub de_scale: f64,
    pub params: Vec<ParamDef>,
    /// Names of the `init` constants (indices after the parameters).
    pub derived: Vec<String>,
    pub init: Vec<Stmt>,
    pub init_locals: usize,
    pub iter: Vec<Stmt>,
    /// Local names of `iter`, by index (for readable WGSL).
    pub locals: Vec<String>,
}

impl FormulaDef {
    pub fn param_count(&self) -> usize {
        self.params.len()
    }

    /// Parameters plus `init` constants.
    pub fn constant_count(&self) -> usize {
        self.params.len() + self.derived.len()
    }

    pub fn default_values(&self) -> Vec<f64> {
        self.params.iter().map(|p| p.default).collect()
    }
}

// ---------------------------------------------------------------- parser

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

/// Name resolution scope while parsing a block.
struct Scope<'a> {
    params: &'a [ParamDef],
    derived: Vec<String>,
    /// Stack of block scopes: (name, local index).
    frames: Vec<Vec<(String, usize)>>,
    local_names: Vec<String>,
    /// In `init`: top-level `let`s define constants.
    init: bool,
}

impl Scope<'_> {
    fn lookup(&self, name: &str) -> Option<VarRef> {
        for f in self.frames.iter().rev() {
            if let Some((_, i)) = f.iter().rev().find(|(n, _)| n == name) {
                return Some(VarRef::Local(*i));
            }
        }
        if let Some(i) = self.derived.iter().position(|n| n == name) {
            return Some(VarRef::Param(self.params.len() + i));
        }
        if let Some(i) = self.params.iter().position(|p| p.ident == name) {
            return Some(VarRef::Param(i));
        }
        if !self.init
            && let Some(i) = STATE_NAMES.iter().position(|n| *n == name)
        {
            return Some(VarRef::State(i));
        }
        None
    }

    fn declare(&mut self, name: &str) -> VarRef {
        if self.init && self.frames.len() == 1 {
            self.derived.push(name.to_string());
            return VarRef::Param(self.params.len() + self.derived.len() - 1);
        }
        let i = self.local_names.len();
        self.local_names.push(name.to_string());
        self.frames.last_mut().expect("scope frame").push((name.to_string(), i));
        VarRef::Local(i)
    }
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }

    fn line(&self) -> usize {
        self.toks[self.pos].1
    }

    fn err<T>(&self, msg: impl Into<String>) -> Res<T> {
        Err(LangError { line: self.line(), msg: msg.into() })
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, sym: &str) -> bool {
        if matches!(self.peek(), Tok::Sym(s) if *s == sym) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, sym: &str) -> Res<()> {
        if self.eat(sym) { Ok(()) } else { self.err(format!("expected '{sym}', found {:?}", self.peek())) }
    }

    fn ident(&mut self) -> Res<String> {
        match self.next() {
            Tok::Ident(s) => Ok(s),
            t => self.err(format!("expected a name, found {t:?}")),
        }
    }

    fn string(&mut self) -> Res<String> {
        match self.next() {
            Tok::Str(s) => Ok(s),
            t => self.err(format!("expected a string, found {t:?}")),
        }
    }

    fn number(&mut self) -> Res<f64> {
        let neg = self.eat("-");
        match self.next() {
            Tok::Num(v) => Ok(if neg { -v } else { v }),
            Tok::Ident(s) if s == "pi" => Ok(if neg { -std::f64::consts::PI } else { std::f64::consts::PI }),
            t => self.err(format!("expected a number, found {t:?}")),
        }
    }

    fn formula(&mut self) -> Res<FormulaDef> {
        let kw = self.ident()?;
        if kw != "formula" {
            return self.err(format!("expected 'formula', found '{kw}'"));
        }
        let name = self.string()?;
        self.expect("{")?;
        let mut f = FormulaDef {
            name,
            category: "Other".into(),
            description: String::new(),
            de: DeKind::Log,
            aliases: Vec::new(),
            iterations: 1,
            transform: false,
            de_scale: 1.0,
            params: Vec::new(),
            derived: Vec::new(),
            init: Vec::new(),
            init_locals: 0,
            iter: Vec::new(),
            locals: Vec::new(),
        };
        let mut has_iter = false;
        while !self.eat("}") {
            let key = self.ident()?;
            match key.as_str() {
                "category" => f.category = self.string()?,
                "description" => f.description = self.string()?,
                "alias" => {
                    let name = self.string()?;
                    let mut params = Vec::new();
                    if self.eat("(") {
                        loop {
                            params.push(self.ident()?);
                            if self.eat(")") {
                                break;
                            }
                            self.expect(",")?;
                        }
                    }
                    f.aliases.push(Alias { name, params });
                }
                "transform" => f.transform = true,
                "iterations" => f.iterations = self.number()? as u32,
                "descale" => f.de_scale = self.number()?,
                "de" => {
                    f.de = match self.ident()?.as_str() {
                        "log" => DeKind::Log,
                        "linear" => DeKind::Linear,
                        "numeric" => DeKind::Numeric,
                        "difs" => DeKind::Difs,
                        o => return self.err(format!("unknown DE kind '{o}'")),
                    }
                }
                "param" => {
                    let ident = self.ident()?;
                    let label = self.string()?;
                    self.expect("=")?;
                    let default = self.number()?;
                    let (mut min, mut max) = (default.min(0.0), default.max(1.0) * 2.0);
                    if self.eat("[") {
                        min = self.number()?;
                        self.expect(",")?;
                        max = self.number()?;
                        self.expect("]")?;
                    }
                    let kind = match self.peek() {
                        Tok::Ident(k) if k == "int" => ParamKind::Int,
                        Tok::Ident(k) if k == "bool" => ParamKind::Bool,
                        Tok::Ident(k) if k == "angle" => ParamKind::Angle,
                        _ => ParamKind::Float,
                    };
                    if kind != ParamKind::Float {
                        self.pos += 1;
                    }
                    if kind == ParamKind::Bool {
                        (min, max) = (0.0, 1.0);
                    }
                    if kind == ParamKind::Angle && min == default.min(0.0) && max == default.max(1.0) * 2.0 {
                        (min, max) = (-180.0, 180.0);
                    }
                    if f.params.iter().any(|p| p.ident == ident) || STATE_NAMES.contains(&ident.as_str()) {
                        return self.err(format!("parameter '{ident}' is defined twice or shadows a state variable"));
                    }
                    f.params.push(ParamDef { ident, label, default, min, max, kind });
                }
                "init" => {
                    let mut sc = Scope { params: &f.params, derived: Vec::new(), frames: vec![Vec::new()], local_names: Vec::new(), init: true };
                    self.expect("{")?;
                    f.init = self.block_body(&mut sc)?;
                    f.derived = sc.derived;
                    f.init_locals = sc.local_names.len();
                }
                "iter" => {
                    let mut sc = Scope { params: &f.params, derived: f.derived.clone(), frames: vec![Vec::new()], local_names: Vec::new(), init: false };
                    self.expect("{")?;
                    f.iter = self.block_body(&mut sc)?;
                    f.locals = sc.local_names;
                    has_iter = true;
                }
                o => return self.err(format!("unknown formula item '{o}'")),
            }
        }
        if !has_iter {
            return self.err(format!("formula '{}' has no iter block", f.name));
        }
        for a in &f.aliases {
            if let Some(bad) = a.params.iter().find(|p| *p != "_" && !f.params.iter().any(|q| q.ident == **p)) {
                return self.err(format!("alias '{}' maps to unknown parameter '{bad}'", a.name));
            }
        }
        Ok(f)
    }

    /// Statements up to the closing `}` (already past the opening one).
    fn block_body(&mut self, sc: &mut Scope) -> Res<Vec<Stmt>> {
        let mut out = Vec::new();
        while !self.eat("}") {
            if *self.peek() == Tok::Eof {
                return self.err("missing '}'");
            }
            out.push(self.stmt(sc)?);
        }
        Ok(out)
    }

    fn nested_block(&mut self, sc: &mut Scope) -> Res<Vec<Stmt>> {
        self.expect("{")?;
        sc.frames.push(Vec::new());
        let b = self.block_body(sc);
        sc.frames.pop();
        b
    }

    fn stmt(&mut self, sc: &mut Scope) -> Res<Stmt> {
        let name = self.ident()?;
        match name.as_str() {
            "let" => {
                let n = self.ident()?;
                self.expect("=")?;
                let e = self.expr(sc)?;
                self.check_float(&e)?;
                let v = sc.declare(&n);
                Ok(Stmt::Let(v, e))
            }
            "if" => self.if_stmt(sc),
            "for" => {
                let n = self.ident()?;
                if self.ident()? != "in" {
                    return self.err("expected 'in'");
                }
                let a = self.number()? as i64;
                self.expect("..")?;
                let b = self.number()? as i64;
                self.expect("{")?;
                sc.frames.push(Vec::new());
                let VarRef::Local(idx) = sc.declare(&n) else { return self.err("loop counter must be local") };
                let body = self.block_body(sc);
                sc.frames.pop();
                Ok(Stmt::For(idx, a, b, body?))
            }
            _ => {
                let target = sc.lookup(&name).ok_or_else(|| LangError { line: self.line(), msg: format!("unknown variable '{name}'") })?;
                match target {
                    VarRef::Param(_) => return self.err(format!("'{name}' is a parameter and read only")),
                    VarRef::State(s) if is_read_only(s) => return self.err(format!("'{name}' is read only")),
                    _ => {}
                }
                let op = match self.next() {
                    Tok::Sym("=") => None,
                    Tok::Sym("+=") => Some(BinOp::Add),
                    Tok::Sym("-=") => Some(BinOp::Sub),
                    Tok::Sym("*=") => Some(BinOp::Mul),
                    Tok::Sym("/=") => Some(BinOp::Div),
                    t => return self.err(format!("expected an assignment after '{name}', found {t:?}")),
                };
                let e = self.expr(sc)?;
                self.check_float(&e)?;
                let e = match op {
                    None => e,
                    Some(op) => Expr::Bin(op, Box::new(Expr::Var(target)), Box::new(e)),
                };
                Ok(Stmt::Assign(target, e))
            }
        }
    }

    fn if_stmt(&mut self, sc: &mut Scope) -> Res<Stmt> {
        let c = self.expr(sc)?;
        if !is_bool(&c) {
            return self.err("'if' needs a condition (comparison)");
        }
        let then = self.nested_block(sc)?;
        let mut els = Vec::new();
        if matches!(self.peek(), Tok::Ident(s) if s == "else") {
            self.pos += 1;
            if matches!(self.peek(), Tok::Ident(s) if s == "if") {
                self.pos += 1;
                els.push(self.if_stmt(sc)?);
            } else {
                els = self.nested_block(sc)?;
            }
        }
        Ok(Stmt::If(c, then, els))
    }

    fn check_float(&self, e: &Expr) -> Res<()> {
        if is_bool(e) { self.err("expected a number, found a condition") } else { Ok(()) }
    }

    fn expr(&mut self, sc: &mut Scope) -> Res<Expr> {
        self.binary(sc, 0)
    }

    fn binary(&mut self, sc: &mut Scope, min_prec: u8) -> Res<Expr> {
        let mut lhs = self.unary(sc)?;
        loop {
            let (op, prec) = match self.peek() {
                Tok::Sym("||") => (BinOp::Or, 1),
                Tok::Sym("&&") => (BinOp::And, 2),
                Tok::Sym("<") => (BinOp::Lt, 3),
                Tok::Sym("<=") => (BinOp::Le, 3),
                Tok::Sym(">") => (BinOp::Gt, 3),
                Tok::Sym(">=") => (BinOp::Ge, 3),
                Tok::Sym("==") => (BinOp::Eq, 3),
                Tok::Sym("!=") => (BinOp::Ne, 3),
                Tok::Sym("+") => (BinOp::Add, 4),
                Tok::Sym("-") => (BinOp::Sub, 4),
                Tok::Sym("*") => (BinOp::Mul, 5),
                Tok::Sym("/") => (BinOp::Div, 5),
                Tok::Sym("%") => (BinOp::Rem, 5),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            self.pos += 1;
            let rhs = self.binary(sc, prec + 1)?;
            let logic = matches!(op, BinOp::And | BinOp::Or);
            if logic != is_bool(&lhs) || logic != is_bool(&rhs) {
                return self.err(if logic { "'&&' and '||' need conditions on both sides" } else { "arithmetic or comparison on a condition" });
            }
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self, sc: &mut Scope) -> Res<Expr> {
        if self.eat("-") {
            let e = self.unary(sc)?;
            self.check_float(&e)?;
            return Ok(match e {
                Expr::Num(v) => Expr::Num(-v),
                e => Expr::Neg(Box::new(e)),
            });
        }
        if self.eat("!") {
            let e = self.unary(sc)?;
            if !is_bool(&e) {
                return self.err("'!' needs a condition");
            }
            return Ok(Expr::Not(Box::new(e)));
        }
        if self.eat("+") {
            return self.unary(sc);
        }
        self.primary(sc)
    }

    fn primary(&mut self, sc: &mut Scope) -> Res<Expr> {
        match self.next() {
            Tok::Num(v) => Ok(Expr::Num(v)),
            Tok::Sym("(") => {
                let e = self.expr(sc)?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Ident(n) => {
                if self.eat("(") {
                    let (func, arity) = Func::from_name(&n).ok_or_else(|| LangError { line: self.line(), msg: format!("unknown function '{n}'") })?;
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.expr(sc)?);
                            if self.eat(")") {
                                break;
                            }
                            self.expect(",")?;
                        }
                    }
                    if args.len() != arity {
                        return self.err(format!("'{n}' takes {arity} arguments, got {}", args.len()));
                    }
                    for (k, a) in args.iter().enumerate() {
                        let want_bool = func == Func::Select && k == 0;
                        if is_bool(a) != want_bool {
                            return self.err(format!("argument {} of '{n}' has the wrong type", k + 1));
                        }
                    }
                    return Ok(Expr::Call(func, args));
                }
                match n.as_str() {
                    "pi" => Ok(Expr::Num(std::f64::consts::PI)),
                    "tau" => Ok(Expr::Num(std::f64::consts::TAU)),
                    _ => sc.lookup(&n).map(Expr::Var).ok_or_else(|| LangError { line: self.line(), msg: format!("unknown variable '{n}'") }),
                }
            }
            t => self.err(format!("unexpected {t:?}")),
        }
    }
}

/// True if the statements read or write state register `idx`.
pub fn uses_state(stmts: &[Stmt], idx: usize) -> bool {
    fn ex(e: &Expr, idx: usize) -> bool {
        match e {
            Expr::Var(VarRef::State(i)) => *i == idx,
            Expr::Neg(a) | Expr::Not(a) => ex(a, idx),
            Expr::Bin(_, a, b) => ex(a, idx) || ex(b, idx),
            Expr::Call(_, args) => args.iter().any(|a| ex(a, idx)),
            _ => false,
        }
    }
    stmts.iter().any(|s| match s {
        Stmt::Let(v, e) | Stmt::Assign(v, e) => *v == VarRef::State(idx) || ex(e, idx),
        Stmt::If(c, t, e) => ex(c, idx) || uses_state(t, idx) || uses_state(e, idx),
        Stmt::For(_, _, _, b) => uses_state(b, idx),
    })
}

pub fn is_bool(e: &Expr) -> bool {
    match e {
        Expr::Not(_) => true,
        Expr::Bin(op, ..) => matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or),
        _ => false,
    }
}

/// Parses a formula library source (any number of `formula` blocks).
pub fn parse_library(src: &str) -> Result<Vec<FormulaDef>, LangError> {
    let mut p = Parser { toks: lex(src)?, pos: 0 };
    let mut out = Vec::new();
    while *p.peek() != Tok::Eof {
        out.push(p.formula()?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_formula() {
        let src = r#"
            formula "T" {
                de linear
                param s "Scale" = 2 [-3, 3]
                init { let s2 = s * s; }
                iter {
                    let r2 = x*x + y*y + z*z;
                    if r2 < 1.0 && s2 > 0.0 { x *= s; } else if r2 > 2.0 { y = -y; } else { z = sel(x > 0.0, 1.0, 2.0); }
                    for k in 0..3 { x += k; }
                    dr = dr * abs(s) + 1.0;
                }
            }"#;
        let lib = parse_library(src).unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].derived, vec!["s2".to_string()]);
        assert_eq!(lib[0].locals, vec!["r2".to_string(), "k".to_string()]);
        assert_eq!(lib[0].de, DeKind::Linear);
    }

    #[test]
    fn rejects_errors() {
        for src in [
            r#"formula "A" { iter { q = 1.0; } }"#,
            r#"formula "A" { param s "S" = 1 iter { s = 1.0; } }"#,
            r#"formula "A" { iter { if x { } } }"#,
            r#"formula "A" { iter { x = sin(1.0, 2.0); } }"#,
            r#"formula "A" { iter { x = (x < 1.0) + 1.0; } }"#,
        ] {
            assert!(parse_library(src).is_err(), "{src}");
        }
    }
}
