// SPDX-License-Identifier: LGPL-2.1-or-later

//! Mandelbulb3D's JIT formulas: a Pascal subset translated into the formula language.
//!
//! `procedure Name(var x, y, z, w: Double; PIteration3D: TPIteration3D);` with a `var` block and
//! a `begin … end` body of assignments, `if … then … else`, `begin … end` blocks, `for`,
//! `while` and `repeat` loops and calls of the Delphi math functions. Identifiers are
//! case-insensitive; options and named constants are referred to by name;
//! `PIteration3D^.J1` and friends are fields of the iteration record.

use std::collections::HashMap;
use std::fmt::Write;

use super::M3f;

type Res<T> = Result<T, String>;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Id(String),
    Sym(&'static str),
    Eof,
}

const SYMS: [&str; 21] = [":=", "<>", "<=", ">=", "..", "^", ".", ",", ";", ":", "(", ")", "[", "]", "+", "-", "*", "/", "=", "<", ">"];

fn lex(src: &str) -> Res<Vec<Tok>> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '{' {
            while i < b.len() && b[i] != '}' {
                i += 1;
            }
            i += 1;
        } else if c == '(' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == ')') {
                i += 1;
            }
            i += 2;
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c.is_ascii_digit() || (c == '.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()) && !matches!(out.last(), Some(Tok::Id(_)) | Some(Tok::Sym(")")))) {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == '.' && b.get(i + 1) != Some(&'.'))) {
                i += 1;
            }
            if i < b.len() && (b[i] == 'e' || b[i] == 'E') {
                i += 1;
                if i < b.len() && (b[i] == '+' || b[i] == '-') {
                    i += 1;
                }
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let s: String = b[start..i].iter().collect();
            out.push(Tok::Num(s.parse().map_err(|_| format!("bad number {s}"))?));
        } else if c == '$' {
            let start = i + 1;
            i += 1;
            while i < b.len() && b[i].is_ascii_hexdigit() {
                i += 1;
            }
            let s: String = b[start..i].iter().collect();
            out.push(Tok::Num(u64::from_str_radix(&s, 16).map_err(|_| format!("bad hex {s}"))? as f64));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            out.push(Tok::Id(b[start..i].iter().collect::<String>().to_lowercase()));
        } else {
            let rest: String = b[i..(i + 2).min(b.len())].iter().collect();
            let sym = SYMS.iter().find(|s| rest.starts_with(**s)).ok_or_else(|| format!("unexpected character '{c}'"))?;
            out.push(Tok::Sym(sym));
            i += sym.len();
        }
    }
    out.push(Tok::Eof);
    Ok(out)
}

struct Tr<'a> {
    toks: Vec<Tok>,
    pos: usize,
    m: &'a M3f,
    /// Lower-case name -> formula-language expression (options, constants, locals).
    names: HashMap<String, String>,
    locals: Vec<String>,
    tmp: usize,
    out: String,
    indent: usize,
}

impl Tr<'_> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Id(s) if s == kw)
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
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
        if self.eat(sym) { Ok(()) } else { Err(format!("expected '{sym}', found {:?}", self.peek())) }
    }

    fn ident(&mut self) -> Res<String> {
        match self.next() {
            Tok::Id(s) => Ok(s),
            t => Err(format!("expected a name, found {t:?}")),
        }
    }

    fn line(&mut self, s: &str) {
        let _ = writeln!(self.out, "{}{s}", "    ".repeat(self.indent));
    }

    fn temp(&mut self, e: &str) -> String {
        self.tmp += 1;
        let n = format!("t{}", self.tmp);
        self.locals.push(n.clone());
        let l = format!("{n} = {e}");
        self.line(&l);
        n
    }

    fn header(&mut self) -> Res<()> {
        if !self.eat_kw("procedure") {
            return Err("expected 'procedure'".into());
        }
        self.ident()?;
        if self.eat("(") {
            let mut depth = 1;
            while depth > 0 {
                match self.next() {
                    Tok::Sym("(") => depth += 1,
                    Tok::Sym(")") => depth -= 1,
                    Tok::Eof => return Err("unterminated parameter list".into()),
                    _ => {}
                }
            }
        }
        self.eat(";");
        while self.eat_kw("var") || self.eat_kw("const") {
            loop {
                let mut names = vec![self.ident()?];
                while self.eat(",") {
                    names.push(self.ident()?);
                }
                if self.eat("=") {
                    // const name = value;
                    let e = self.expr()?;
                    for n in names {
                        self.names.insert(n, format!("({e})"));
                    }
                } else {
                    self.expect(":")?;
                    self.ident()?;
                    for n in names {
                        let v = format!("v_{n}");
                        self.locals.push(v.clone());
                        self.names.insert(n, v);
                    }
                }
                self.expect(";")?;
                if !matches!(self.peek(), Tok::Id(s) if s != "begin" && s != "var" && s != "const") {
                    break;
                }
            }
        }
        Ok(())
    }

    /// A statement (or nothing before `;` / `end`).
    fn stmt(&mut self) -> Res<()> {
        match self.peek().clone() {
            Tok::Id(k) if k == "begin" => {
                self.pos += 1;
                self.stmts_until("end")?;
                Ok(())
            }
            Tok::Id(k) if k == "if" => {
                self.pos += 1;
                let c = self.cond()?;
                if !self.eat_kw("then") {
                    return Err("expected 'then'".into());
                }
                let l = format!("if {c} {{");
                self.line(&l);
                self.indent += 1;
                self.stmt()?;
                self.indent -= 1;
                if self.eat_kw("else") {
                    self.line("} else {");
                    self.indent += 1;
                    self.stmt()?;
                    self.indent -= 1;
                }
                self.line("}");
                Ok(())
            }
            Tok::Id(k) if k == "while" => {
                self.pos += 1;
                self.line("loop {");
                self.indent += 1;
                let c = self.cond()?;
                let l = format!("if !({c}) {{ break }}");
                self.line(&l);
                if !self.eat_kw("do") {
                    return Err("expected 'do'".into());
                }
                self.stmt()?;
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Tok::Id(k) if k == "repeat" => {
                self.pos += 1;
                self.line("loop {");
                self.indent += 1;
                self.stmts_until("until")?;
                let c = self.cond()?;
                let l = format!("if {c} {{ break }}");
                self.line(&l);
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Tok::Id(k) if k == "for" => {
                self.pos += 1;
                let var = self.ident()?;
                let v = self.lvalue(&var)?;
                self.expect(":=")?;
                let a = self.expr()?;
                let down = if self.eat_kw("to") {
                    false
                } else if self.eat_kw("downto") {
                    true
                } else {
                    return Err("expected 'to'".into());
                };
                let b = self.expr()?;
                let end = self.temp(&b);
                if !self.eat_kw("do") {
                    return Err("expected 'do'".into());
                }
                let l = format!("{v} = {a}");
                self.line(&l);
                self.line("loop {");
                self.indent += 1;
                let l = if down { format!("if {v} < {end} {{ break }}") } else { format!("if {v} > {end} {{ break }}") };
                self.line(&l);
                self.stmt()?;
                let l = format!("{v} = {v} {} 1.0", if down { "-" } else { "+" });
                self.line(&l);
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Tok::Id(k) if k == "end" || k == "else" || k == "until" => Ok(()),
            Tok::Sym(";") => Ok(()),
            Tok::Id(name) => {
                self.pos += 1;
                if name == "sincos" {
                    self.expect("(")?;
                    let a = self.expr()?;
                    let a = self.temp(&a);
                    self.expect(",")?;
                    let s = self.ident()?;
                    let s = self.lvalue(&s)?;
                    self.expect(",")?;
                    let c = self.ident()?;
                    let c = self.lvalue(&c)?;
                    self.expect(")")?;
                    let l = format!("{s} = sin({a})");
                    self.line(&l);
                    let l = format!("{c} = cos({a})");
                    self.line(&l);
                    return Ok(());
                }
                let target = if name == "piteration3d" { self.record_field()? } else { self.lvalue(&name)? };
                self.expect(":=")?;
                let e = self.expr()?;
                let l = format!("{target} = {e}");
                self.line(&l);
                Ok(())
            }
            t => Err(format!("unexpected {t:?}")),
        }
    }

    fn stmts_until(&mut self, kw: &str) -> Res<()> {
        loop {
            if self.eat_kw(kw) {
                return Ok(());
            }
            if *self.peek() == Tok::Eof {
                return Err(format!("missing '{kw}'"));
            }
            self.stmt()?;
            if !self.eat(";") && !self.is_kw(kw) {
                return Err(format!("expected ';' or '{kw}', found {:?}", self.peek()));
            }
        }
    }

    fn lvalue(&self, name: &str) -> Res<String> {
        match name {
            "x" | "y" | "z" => Ok(name.to_string()),
            "w" => Ok(if self.m.is_4d() { "w" } else { "dr" }.into()),
            _ => match self.names.get(name) {
                Some(v) if v.starts_with("v_") || v.starts_with('t') => Ok(v.clone()),
                Some(_) => Err(format!("assignment to the option or constant '{name}'")),
                None => Err(format!("unknown variable '{name}'")),
            },
        }
    }

    /// `PIteration3D^.Field` (after the `PIteration3D` name).
    fn record_field(&mut self) -> Res<String> {
        self.expect("^")?;
        self.expect(".")?;
        let f = self.ident()?;
        let off = match f.as_str() {
            "j4" => -56,
            "rold" => -48,
            "x" => -32,
            "y" => -24,
            "z" => -16,
            "w" => -8,
            "c1" => 0,
            "c2" => 8,
            "c3" => 16,
            "j1" => 24,
            "j2" => 32,
            "j3" => 40,
            "rout" => 56,
            "otrap" => 192,
            "varyscale" => 200,
            "dfree1" => 216,
            "dfree2" => 224,
            "deriv1" => 232,
            "deriv2" => 240,
            "deriv3" => 248,
            _ => return Err(format!("unsupported iteration record field '{f}'")),
        };
        super::record_field(off, self.m.is_4d()).map(str::to_string).ok_or_else(|| format!("record field '{f}'"))
    }

    fn cond(&mut self) -> Res<String> {
        let e = self.expr()?;
        Ok(e)
    }

    /// Relational level (lowest precedence).
    fn expr(&mut self) -> Res<String> {
        let a = self.simple()?;
        let op = match self.peek() {
            Tok::Sym("=") => "==",
            Tok::Sym("<>") => "!=",
            Tok::Sym("<") => "<",
            Tok::Sym("<=") => "<=",
            Tok::Sym(">") => ">",
            Tok::Sym(">=") => ">=",
            _ => return Ok(a),
        };
        self.pos += 1;
        let b = self.simple()?;
        Ok(format!("({a} {op} {b})"))
    }

    fn simple(&mut self) -> Res<String> {
        let mut a = self.term()?;
        loop {
            let op = match self.peek() {
                Tok::Sym("+") => "+",
                Tok::Sym("-") => "-",
                Tok::Id(s) if s == "or" => "||",
                _ => return Ok(a),
            };
            self.pos += 1;
            let b = self.term()?;
            a = format!("({a} {op} {b})");
        }
    }

    fn term(&mut self) -> Res<String> {
        let mut a = self.factor()?;
        loop {
            let op = match self.peek() {
                Tok::Sym("*") => "*",
                Tok::Sym("/") => "/",
                Tok::Id(s) if s == "and" => "&&",
                Tok::Id(s) if s == "div" || s == "mod" => {
                    let s = s.clone();
                    self.pos += 1;
                    let b = self.factor()?;
                    let q = format!("sel(({a}) / ({b}) < 0.0, -floor(-({a}) / ({b})), floor(({a}) / ({b})))");
                    a = if s == "div" { q } else { format!("(({a}) - ({b}) * {q})") };
                    continue;
                }
                _ => return Ok(a),
            };
            self.pos += 1;
            let b = self.factor()?;
            a = format!("({a} {op} {b})");
        }
    }

    fn factor(&mut self) -> Res<String> {
        match self.next() {
            Tok::Num(v) => Ok(crate::formula::wgsl::lit(v)),
            Tok::Sym("(") => {
                let e = self.expr()?;
                self.expect(")")?;
                Ok(format!("({e})"))
            }
            Tok::Sym("-") => Ok(format!("(-{})", self.factor()?)),
            Tok::Sym("+") => self.factor(),
            Tok::Id(s) if s == "not" => Ok(format!("(!{})", self.factor()?)),
            Tok::Id(s) if s == "pi" => Ok("pi".into()),
            Tok::Id(s) if s == "piteration3d" => self.record_field(),
            Tok::Id(s) => {
                if self.eat("(") {
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.expr()?);
                            if self.eat(")") {
                                break;
                            }
                            self.expect(",")?;
                        }
                    }
                    return self.call(&s, args);
                }
                match s.as_str() {
                    "x" | "y" | "z" => Ok(s),
                    "w" => Ok(if self.m.is_4d() { "w" } else { "dr" }.into()),
                    _ => self.names.get(&s).cloned().ok_or_else(|| format!("unknown name '{s}'")),
                }
            }
            t => Err(format!("unexpected {t:?} in an expression")),
        }
    }

    fn call(&mut self, f: &str, args: Vec<String>) -> Res<String> {
        let one = |a: &[String]| -> Res<String> { a.first().cloned().ok_or_else(|| format!("{f} needs an argument")) };
        let two = |a: &[String]| -> Res<(String, String)> {
            if a.len() == 2 { Ok((a[0].clone(), a[1].clone())) } else { Err(format!("{f} needs two arguments")) }
        };
        Ok(match f {
            "abs" | "sqrt" | "sin" | "cos" | "tan" | "exp" | "sqr" | "round" | "sinh" | "cosh" | "tanh" => format!("{f}({})", one(&args)?),
            "ln" => format!("log({})", one(&args)?),
            "arctan" => format!("atan({})", one(&args)?),
            "arcsin" => format!("asin(clamp({}, -1.0, 1.0))", one(&args)?),
            "arccos" => format!("acos(clamp({}, -1.0, 1.0))", one(&args)?),
            "arctan2" => {
                let (y, x) = two(&args)?;
                format!("atan2({y}, {x})")
            }
            "trunc" | "int" => {
                let a = self.temp(&one(&args)?);
                format!("sel({a} < 0.0, -floor(-{a}), floor({a}))")
            }
            "frac" => {
                let a = self.temp(&one(&args)?);
                format!("({a} - sel({a} < 0.0, -floor(-{a}), floor({a})))")
            }
            "floor" | "ceil" => format!("{f}({})", one(&args)?),
            "min" | "max" => {
                let (a, b) = two(&args)?;
                format!("{f}({a}, {b})")
            }
            "sign" => format!("sign({})", one(&args)?),
            "power" | "intpower" => {
                // Delphi's Power keeps the sign of a negative base for integer exponents.
                let (a, b) = two(&args)?;
                let a = self.temp(&a);
                let b = self.temp(&b);
                format!("sel({a} < 0.0, sel(fract({b} * 0.5) == 0.0, 1.0, -1.0) * pow(-{a}, {b}), pow({a}, {b}))")
            }
            _ => return Err(format!("unsupported function '{f}'")),
        })
    }
}

/// The `iter` block of a JIT formula.
pub(crate) fn translate(m: &M3f) -> Res<String> {
    let mut tr = Tr { toks: lex(&m.source)?, pos: 0, m, names: HashMap::new(), locals: Vec::new(), tmp: 0, out: String::new(), indent: 2 };
    for o in &m.options {
        // Options are referred to by their name (one word in JIT formulas).
        let key = o.name.split_whitespace().next().unwrap_or("").to_lowercase();
        tr.names.insert(key, o.ident.clone());
        tr.names.insert(o.name.replace(' ', "_").to_lowercase(), o.ident.clone());
    }
    for (n, v) in &m.named_constants {
        tr.names.insert(n.to_lowercase(), crate::formula::wgsl::lit(*v));
    }
    tr.header()?;
    if !tr.eat_kw("begin") {
        return Err("expected 'begin'".into());
    }
    tr.stmts_until("end")?;
    let mut s = String::from("    iter {\n");
    for l in &tr.locals {
        let _ = writeln!(s, "        let {l} = 0.0");
    }
    s.push_str(&tr.out);
    s.push_str("    }\n");
    Ok(s)
}
