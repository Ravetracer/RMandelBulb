// SPDX-License-Identifier: LGPL-2.1-or-later

//! Mandelbulb3D formula files (`*.m3f`) translated into the formula language.
//!
//! An `.m3f` file holds 32-bit x86 machine code (x87, some SSE2) of one iteration, the
//! formula's options and constants. `translate` reads the file, rebuilds the parameter memory
//! the code expects (Mandelbulb3D's `FillCustomVBufWithVars`) and turns the machine code into
//! formula-language source by symbolic execution (`x86.rs`). The result compiles for the GPU
//! and the CPU like any other formula.

mod pascal;
mod x86;

use std::fmt::Write;

/// One (expanded) option: what the user edits.
#[derive(Clone, Debug)]
pub struct Opt {
    /// Mandelbulb3D option type (0 = Double … 22 = DReci2).
    pub kind: u8,
    pub name: String,
    pub default: f64,
    /// Formula-language identifier.
    pub ident: String,
}

/// A parsed `.m3f` file.
#[derive(Clone, Debug, Default)]
pub struct M3f {
    pub name: String,
    pub options: Vec<Opt>,
    pub de_option: i32,
    pub de_scale: f64,
    pub ade_scale: f64,
    pub version: i32,
    /// Constants area (8 bytes per constant, from offset 0 of the parameter pointer).
    pub constants: Vec<u8>,
    pub code: Vec<u8>,
    pub description: String,
    /// The formula is Pascal source for Mandelbulb3D's JIT compiler.
    pub has_source: bool,
    /// The Pascal source (`[SOURCE]` section).
    pub source: String,
    /// Named constants (`.Double name = value` in `[CONSTANTS]`).
    pub named_constants: Vec<(String, f64)>,
}

const TYPE_NAMES: [&str; 23] = [
    "DOUBLE",
    "SINGLE",
    "INTEGER",
    "DOUBLEANGLE",
    "SINGLEANGLE",
    "3DOUBLEANGLES",
    "3SINGLEANGLES",
    "BOXSCALE",
    "FOLDING",
    "DSQUARE",
    "NOVARIABLE",
    "FOLDING16",
    "6SINGLEANGLES",
    "DRECIPRO",
    "2DOUBLES",
    "DSQRRECI",
    "2SINGLES",
    "4SINGLES",
    "3SCALESANGLES",
    "SCALESROT",
    "2INTEGER",
    "SRECI2",
    "DRECI2",
];

fn parse_num(s: &str) -> f64 {
    s.trim().replace(',', ".").parse::<f64>().unwrap_or(0.0)
}

fn ident_for(i: usize, name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_');
    if s.is_empty() { format!("o{i}") } else { format!("o{i}_{s}") }
}

impl M3f {
    pub fn parse(name: &str, text: &str) -> Result<M3f, String> {
        let mut m = M3f { name: name.to_string(), de_scale: 1.0, ade_scale: 1.0, ..Default::default() };
        let mut section = String::new();
        let mut code_hex = String::new();
        let mut desc = Vec::new();
        let mut after_end = false;
        for raw in text.lines() {
            let line = raw.trim();
            if after_end {
                desc.push(raw.trim_end().to_string());
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line.to_uppercase();
                if section == "[END]" {
                    after_end = true;
                }
                continue;
            }
            if line.is_empty() {
                if section == "[SOURCE]" {
                    m.source.push('\n');
                }
                continue;
            }
            match section.as_str() {
                "[OPTIONS]" => m.option_line(line),
                "[CONSTANTS]" => m.constant_line(line),
                "[CODE]" => code_hex.extend(line.chars().filter(|c| c.is_ascii_hexdigit())),
                "[SOURCE]" => {
                    m.has_source = true;
                    m.source.push_str(raw);
                    m.source.push('\n');
                }
                _ => {}
            }
        }
        if code_hex.len() >= 2 {
            let n = code_hex.len() / 2 * 2;
            m.code = (0..n).step_by(2).filter_map(|i| u8::from_str_radix(&code_hex[i..i + 2], 16).ok()).collect();
        }
        let d = desc.join("\n");
        m.description = d.trim().trim_start_matches("Description:").trim().to_string();
        // A [SOURCE] section ends at the next section header; drop the header lines kept.
        m.source = m.source.lines().filter(|l| !l.trim().starts_with('[')).collect::<Vec<_>>().join("\n");
        if m.code.is_empty() && !m.has_source {
            return Err("no [CODE] section".into());
        }
        Ok(m)
    }

    fn option_line(&mut self, line: &str) {
        let upper = line.to_uppercase();
        let last = line.rsplit(|c: char| c.is_whitespace() || c == '=').find(|s| !s.is_empty()).unwrap_or("");
        let key = upper.trim_start_matches('.');
        if key.starts_with("DEOPTION") {
            self.de_option = parse_num(last) as i32;
            return;
        }
        if key.starts_with("DESCALE") {
            self.de_scale = parse_num(last);
            return;
        }
        if key.starts_with("ADESCALE") {
            self.ade_scale = parse_num(last);
            return;
        }
        if key.starts_with("VERSION") {
            self.version = parse_num(last) as i32;
            return;
        }
        if ["SSE", "SIPOW", "RSTOP", "DIFS"].iter().any(|k| key.starts_with(k)) {
            return;
        }
        let first = key.split_whitespace().next().unwrap_or("");
        let Some(kind) = TYPE_NAMES.iter().position(|t| *t == first) else { return };
        if self.options.len() >= 16 {
            return;
        }
        let kind = kind as u8;
        let (lhs, rhs) = line.split_once('=').unwrap_or((line, "0"));
        let label = lhs.split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
        let default = parse_num(rhs.split_whitespace().last().unwrap_or("0"));
        let mut push = |kind: u8, name: String, default: f64| {
            let i = self.options.len();
            if i < 16 {
                self.options.push(Opt { kind, ident: ident_for(i, &name), name, default });
            }
        };
        match kind {
            5 | 6 => {
                for ax in ["X", "Y", "Z"] {
                    push(kind, format!("{label} {ax}"), default);
                }
            }
            12 => {
                for pl in ["YZ", "XZ", "XY", "XW", "YW", "ZW"] {
                    push(kind, format!("{label} {pl}"), default);
                }
            }
            18 => {
                push(kind, label.clone(), default);
                for c in ["A", "B", "C"] {
                    push(6, format!("Rotation {c}"), 0.0);
                }
            }
            19 => {
                let scale = parse_num(rhs.split_whitespace().next().unwrap_or("1"));
                push(kind, format!("Scale {label}"), scale);
                push(6, format!("Rotation {label}"), default);
            }
            _ => push(kind, label, default),
        }
    }

    fn constant_line(&mut self, line: &str) {
        let upper = line.trim_start_matches('.').to_uppercase();
        let last = line.rsplit(|c: char| c.is_whitespace() || c == '=').find(|s| !s.is_empty()).unwrap_or("0");
        if line.starts_with('.')
            && let Some((lhs, _)) = line.split_once('=')
            && let Some(name) = lhs.split_whitespace().nth(1)
        {
            let v = if upper.starts_with("INT") { parse_int(last) as f64 } else { parse_num(last) };
            self.named_constants.push((name.to_string(), v));
        }
        let mut slot = [0u8; 8];
        if upper.starts_with("DOUBLE") {
            slot = parse_num(last).to_le_bytes();
        } else if upper.starts_with("INT64") {
            slot = parse_int(last).to_le_bytes();
        } else if upper.starts_with("INT") {
            slot[..4].copy_from_slice(&(parse_int(last) as i32).to_le_bytes());
        } else if upper.starts_with("SINGLE") {
            slot[..4].copy_from_slice(&(parse_num(last) as f32).to_le_bytes());
        } else {
            return;
        }
        self.constants.extend_from_slice(&slot);
    }

    /// The formula is a 4D formula (w is the fourth coordinate, the derivative lives elsewhere).
    pub fn is_4d(&self) -> bool {
        matches!(self.de_option, 4..=6)
    }
}

fn parse_int(s: &str) -> i64 {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('$') {
        u64::from_str_radix(h, 16).map(|v| v as i64).unwrap_or(0)
    } else {
        s.parse::<i64>().unwrap_or(0)
    }
}

/// The state register a field of Mandelbulb3D's iteration record maps to (offsets from the
/// record pointer, which points at the field C1).
pub fn record_field(off: i64, four_d: bool) -> Option<&'static str> {
    Some(match off {
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
        _ => return None,
    })
}

/// The parameter memory below the parameter pointer for option `values`: (offset, kind,
/// value) of every slot that holds a number (tests run the original code with it).
pub fn parameter_memory(m: &M3f, values: &[f64]) -> Result<Vec<(i64, SlotKind, f64)>, String> {
    let layout = memory_layout(&m.options);
    let mut src = String::from("formula \"eval\" {\n");
    for o in &m.options {
        let _ = writeln!(src, "    param {} \"x\" = 0", o.ident);
    }
    src.push_str("    init {\n");
    let used: Vec<&MemSlot> = layout.iter().filter(|s| s.expr.is_some()).collect();
    let mut seen = std::collections::HashSet::new();
    for s in &used {
        for l in &s.prelude {
            if seen.insert(l.clone()) {
                let _ = writeln!(src, "        {l}");
            }
        }
    }
    for (k, s) in used.iter().enumerate() {
        let _ = writeln!(src, "        let s{k} = {}", s.expr.as_deref().unwrap_or("0.0"));
    }
    src.push_str("    }\n    iter { }\n}\n");
    let defs = crate::formula::lang::parse_library(&src).map_err(|e| e.to_string())?;
    let def = defs.first().ok_or("no formula")?;
    let cpu = crate::formula::cpu::CpuFormula::compile(def);
    let consts = cpu.constants(values);
    let n = m.options.len();
    let pos = |k: usize| def.derived.iter().position(|d| *d == format!("s{k}")).map(|p| n + p);
    Ok(used.iter().enumerate().map(|(k, s)| (s.offset, s.kind, pos(k).and_then(|p| consts.get(p)).copied().unwrap_or(0.0))).collect())
}

/// A value the formula code finds in its parameter memory below the parameter pointer.
#[derive(Clone, Debug)]
pub(crate) struct MemSlot {
    /// Byte offset from the parameter pointer (negative).
    pub offset: i64,
    pub kind: SlotKind,
    /// Formula-language expression of the value (in terms of the parameters), or None if the
    /// slot holds something that cannot be expressed (a function pointer).
    pub expr: Option<String>,
    /// `init` statements the expression needs (e.g. the steps of a 4D rotation matrix).
    pub prelude: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    F64,
    F32,
    I32,
}

/// Rebuilds Mandelbulb3D's `FillCustomVBufWithVars`: where each option value (or the values
/// derived from it) lies below the parameter pointer.
pub(crate) fn memory_layout(opts: &[Opt]) -> Vec<MemSlot> {
    let mut slots = vec![MemSlot { offset: -8, kind: SlotKind::F64, expr: Some("0.5".into()), prelude: Vec::new() }];
    let mut p: i64 = -8;
    let mut preludes: Vec<(i64, Vec<String>)> = Vec::new();
    let id = |i: usize| opts.get(i).map(|o| o.ident.clone()).unwrap_or_else(|| "0.0".into());
    let mut push = |p: &mut i64, size: u8, kind: SlotKind, expr: Option<String>| {
        *p -= size as i64;
        slots.push(MemSlot { offset: *p, kind, expr, prelude: Vec::new() });
    };
    let rot = |a: String, b: String, c: String| -> [String; 9] {
        let (sx, cx) = (format!("sin({a} * pi / 180.0)"), format!("cos({a} * pi / 180.0)"));
        let (sy, cy) = (format!("sin({b} * pi / 180.0)"), format!("cos({b} * pi / 180.0)"));
        let (sz, cz) = (format!("sin({c} * pi / 180.0)"), format!("cos({c} * pi / 180.0)"));
        [
            format!("{cy} * {cz}"),
            format!("-{cy} * {sz}"),
            sy.clone(),
            format!("{sx} * {sy} * {cz} + {cx} * {sz}"),
            format!("{cx} * {cz} - {sx} * {sy} * {sz}"),
            format!("-{sx} * {cy}"),
            format!("{sx} * {sz} - {cx} * {sy} * {cz}"),
            format!("{cx} * {sy} * {sz} + {sx} * {cz}"),
            format!("{cx} * {cy}"),
        ]
    };
    let mut i = 0;
    while i < opts.len().min(16) {
        let v = id(i);
        use SlotKind::*;
        match opts[i].kind {
            0 => push(&mut p, 8, F64, Some(v)),
            1 => push(&mut p, 4, F32, Some(v)),
            2 => push(&mut p, 4, I32, Some(format!("round({v})"))),
            3 => {
                push(&mut p, 8, F64, Some(format!("sin({v} * pi / 180.0)")));
                push(&mut p, 8, F64, Some(format!("cos({v} * pi / 180.0)")));
            }
            4 => {
                push(&mut p, 4, F32, Some(format!("sin({v} * pi / 180.0)")));
                push(&mut p, 4, F32, Some(format!("cos({v} * pi / 180.0)")));
            }
            5 | 6 => {
                let m = rot(v, id(i + 1), id(i + 2));
                let (size, kind) = if opts[i].kind == 5 { (8, F64) } else { (4, F32) };
                for e in m {
                    push(&mut p, size, kind, Some(e));
                }
                i += 2;
            }
            7 => {
                let prev = if i > 0 { id(i - 1) } else { "0.0".into() };
                push(&mut p, 8, F64, Some(format!("{prev} / sqr(max(1.0e-40, {v}))")));
                push(&mut p, 8, F64, Some(format!("sqr(max(1.0e-40, {v}))")));
            }
            8 => {
                push(&mut p, 8, F64, Some(v.clone()));
                push(&mut p, 8, F64, Some(format!("2.0 * {v}")));
                push(&mut p, 8, F64, Some(format!("-{v}")));
                push(&mut p, 8, F64, Some(format!("-2.0 * {v}")));
                push(&mut p, 4, I32, None);
            }
            9 => push(&mut p, 8, F64, Some(format!("sqr({v})"))),
            10 => {}
            11 => {
                push(&mut p, 8, F64, Some(v.clone()));
                push(&mut p, 8, F64, Some(v.clone()));
                push(&mut p, 8, F64, Some(format!("-{v}")));
                push(&mut p, 8, F64, Some(format!("-{v}")));
            }
            12 => {
                let (prelude, m) = rot4d(i, &(0..6).map(|k| id(i + k)).collect::<Vec<_>>());
                for e in m {
                    push(&mut p, 4, F32, Some(e));
                    preludes.push((p, prelude.clone()));
                }
                i += 5;
            }
            13 => push(&mut p, 8, F64, Some(format!("1.0 / sel(abs({v}) < 1.0e-40, sel({v} < 0.0, -1.0e-40, 1.0e-40), {v})"))),
            14 => {
                push(&mut p, 8, F64, Some(v.clone()));
                push(&mut p, 8, F64, Some(v));
            }
            15 => push(&mut p, 8, F64, Some(format!("1.0 / max(1.0e-40, sqr({v}))"))),
            16 | 17 => {
                for _ in 0..(if opts[i].kind == 17 { 4 } else { 2 }) {
                    push(&mut p, 4, F32, Some(v.clone()));
                }
            }
            18 => {
                let m = rot(id(i + 1), id(i + 2), id(i + 3));
                for e in m {
                    push(&mut p, 4, F32, Some(format!("({e}) * {v}")));
                }
                i += 3;
            }
            19 => {
                let r = id(i + 1);
                push(&mut p, 4, F32, Some(format!("sin({r} * pi / 180.0) * {v}")));
                push(&mut p, 4, F32, Some(format!("cos({r} * pi / 180.0) * {v}")));
                i += 1;
            }
            20 => {
                push(&mut p, 4, I32, Some(format!("round({v})")));
                push(&mut p, 4, I32, Some(format!("round({v})")));
            }
            21 => {
                push(&mut p, 4, F32, Some(v.clone()));
                push(&mut p, 4, F32, Some(format!("1.0 / max(1.0e-30, {v})")));
            }
            22 => {
                push(&mut p, 8, F64, Some(v.clone()));
                push(&mut p, 8, F64, Some(format!("1.0 / max(1.0e-40, {v})")));
            }
            _ => {}
        }
        // Mandelbulb3D stops filling past 256 bytes.
        if -p > 256 {
            break;
        }
        i += 1;
    }
    for (off, pre) in preludes {
        if let Some(s) = slots.iter_mut().find(|s| s.offset == off) {
            s.prelude = pre;
        }
    }
    slots
}

/// Mandelbulb3D's `BuildRotMatrix4d`: six plane rotations (YZ, XZ, XY, XW, YW, ZW), each
/// multiplied from the left. Returns the `init` statements and the 16 entries (row major).
fn rot4d(opt: usize, angles: &[String]) -> (Vec<String>, Vec<String>) {
    const I1: [usize; 6] = [1, 0, 0, 0, 1, 2];
    const I2: [usize; 6] = [2, 2, 1, 3, 3, 3];
    let mut cur: Vec<Vec<String>> = (0..4).map(|r| (0..4).map(|c| if r == c { "1.0".to_string() } else { "0.0".to_string() }).collect()).collect();
    let mut lines = Vec::new();
    for (st, a) in angles.iter().enumerate() {
        let s = format!("sin({a} * pi / 180.0)");
        let c = format!("cos({a} * pi / 180.0)");
        let mut rot: Vec<Vec<String>> = (0..4).map(|r| (0..4).map(|k| if r == k { "1.0".to_string() } else { "0.0".to_string() }).collect()).collect();
        rot[I1[st]][I1[st]] = c.clone();
        rot[I2[st]][I2[st]] = c;
        rot[I1[st]][I2[st]] = format!("-{s}");
        rot[I2[st]][I1[st]] = s;
        let mut next = cur.clone();
        for r in 0..4 {
            for col in 0..4 {
                let terms: Vec<String> = (0..4)
                    .filter(|&k| rot[r][k] != "0.0" && cur[k][col] != "0.0")
                    .map(|k| if rot[r][k] == "1.0" { cur[k][col].clone() } else { format!("({}) * {}", rot[r][k], cur[k][col]) })
                    .collect();
                let name = format!("q{opt}_{st}_{r}{col}");
                lines.push(format!("let {name} = {}", if terms.is_empty() { "0.0".to_string() } else { terms.join(" + ") }));
                next[r][col] = name;
            }
        }
        cur = next;
    }
    (lines, cur.into_iter().flatten().collect())
}

/// Result of a translation.
pub struct Translated {
    /// Formula-language source of the formula.
    pub source: String,
}

/// Translates a parsed `.m3f` into formula-language source.
pub fn translate(m: &M3f) -> Result<Translated, String> {

    if m.de_option > 22 {
        return Err(format!("unknown DE option {}", m.de_option));
    }
    let body = if m.has_source {
        pascal::translate(m)?
    } else {
        let layout = memory_layout(&m.options);
        x86::translate(m, &layout)?
    };
    let mut s = String::new();
    let category = if m.de_option >= 20 {
        "MB3D dIFS"
    } else if m.de_option < 0 {
        "MB3D transforms"
    } else if m.is_4d() {
        "MB3D 4D"
    } else {
        "MB3D 3D"
    };
    let de = match m.de_option {
        2 | 5 | 6 | 11 => "linear",
        20..=22 => "difs",
        _ => "numeric",
    };
    let first_line = m.description.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").replace('"', "'");
    let _ = writeln!(s, "formula \"{}\" {{", m.name);
    let _ = writeln!(s, "    category \"{category}\"");
    let _ = writeln!(s, "    description \"Mandelbulb3D formula {}. {}\"", m.name, first_line.chars().take(200).collect::<String>());
    let _ = writeln!(s, "    de {de}");
    if m.de_option < 0 || matches!(m.de_option, 21 | 22) {
        s.push_str("    transform\n");
    }
    let scale = if de == "linear" { m.ade_scale } else { m.de_scale };
    if scale.is_finite() && scale > 0.0 && scale != 1.0 {
        let _ = writeln!(s, "    descale {}", crate::formula::wgsl::lit(scale));
    }
    let _ = writeln!(s, "    alias \"{}\"", m.name);
    for o in &m.options {
        let d = o.default;
        let span = d.abs().max(1.0) * 2.0;
        let (kind, lo, hi) = match o.kind {
            2 | 20 => ("int", d - span, d + span),
            3..=6 | 12 => ("angle", -180.0, 180.0),
            10 => ("int", 0.0, 16.0),
            _ => ("", d - span, d + span),
        };
        let label = o.name.replace('"', "'");
        let _ = writeln!(s, "    param {} \"{}\" = {} [{}, {}] {kind}", o.ident, label, num(d), num(lo), num(hi));
    }
    s.push_str(&body);
    s.push_str("}\n");
    Ok(Translated { source: s })
}

fn num(v: f64) -> String {
    crate::formula::wgsl::lit(if v.is_finite() { v } else { 0.0 })
}

/// Reads and translates a file; the formula name is the file stem.
pub fn translate_file(path: &std::path::Path) -> Result<Translated, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text: String = bytes.iter().map(|&b| b as char).collect();
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    translate(&M3f::parse(&name, &text)?)
}
