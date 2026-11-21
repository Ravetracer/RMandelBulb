// SPDX-License-Identifier: LGPL-2.1-or-later

//! Runs the original machine code of Mandelbulb3D formulas (`*.m3f`) in the Unicorn x86
//! emulator and compares the results with the translated formulas (CPU closures), over a few
//! iterations from random start points.
//!
//! `cargo run --release -p m3f-check -- <folder or .m3f> [-v]`

use std::path::{Path, PathBuf};

use rmb_core::formula::cpu::CpuFormula;
use rmb_core::formula::lang::{STATE_COUNT, STATE_NAMES};
use rmb_core::formula::m3f::{M3f, SlotKind, parameter_memory, record_field, translate};
use unicorn_engine::unicorn_const::{Arch, Mode, Prot as Permission};
use unicorn_engine::{RegisterX86, Unicorn};

const CODE: u64 = 0x0001_0000;
const MAGIC: u64 = 0x0008_0000;
const DATA: u64 = 0x0010_0000;
const REC: u64 = DATA + 0x1100;
const PVAR: u64 = DATA + 0x8000;
const STACK_TOP: u64 = DATA + 0xF_0000;

/// Record offsets compared after each iteration (doubles), and the integer field bFirstIt.
const COMPARED: [i64; 16] = [-56, -32, -24, -16, -8, 24, 32, 40, 56, 192, 200, 216, 224, 232, 240, 248];

fn state_index(name: &str) -> usize {
    STATE_NAMES.iter().position(|n| *n == name).unwrap_or(0)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Equal within rounding, relative to `scale` (the size of the whole state: cancellation in
/// big values leaves small absolute noise in the others). Values beyond 1e100 have escaped;
/// any such values count as equal.
fn close(a: f64, b: f64, scale: f64) -> bool {
    let big = |v: f64| !v.is_finite() || v.abs() > 1e100;
    if big(a) || big(b) || scale > 1e100 {
        return big(a) == big(b) || scale > 1e100;
    }
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(scale).max(1.0)
}

enum Outcome {
    Ok,
    Untranslated,
    Pascal,
    Mismatch(String),
    Emulator(String),
}

fn check(path: &Path, verbose: bool) -> Outcome {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return Outcome::Emulator(e.to_string()),
    };
    let text: String = bytes.iter().map(|&b| b as char).collect();
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let m = match M3f::parse(&name, &text) {
        Ok(m) => m,
        Err(_) => return Outcome::Untranslated,
    };
    if m.has_source {
        // Pascal JIT formula: no machine code to compare with.
        return Outcome::Pascal;
    }
    let src = match translate(&m) {
        Ok(t) => t.source,
        Err(_) => return Outcome::Untranslated,
    };
    let defs = match rmb_core::formula::lang::parse_library(&src) {
        Ok(d) => d,
        Err(_) => return Outcome::Untranslated,
    };
    let def = &defs[0];
    let cpu = CpuFormula::compile(def);
    let values: Vec<f64> = m.options.iter().map(|o| o.default).collect();
    let consts = cpu.constants(&values);
    let mem = match parameter_memory(&m, &values) {
        Ok(v) => v,
        Err(e) => return Outcome::Emulator(e),
    };
    let four_d = m.is_4d();
    let difs = m.de_option >= 20;
    let counted = m.de_option >= 0 && !difs;

    let mut emu = match Unicorn::new(Arch::X86, Mode::MODE_32) {
        Ok(e) => e,
        Err(e) => return Outcome::Emulator(format!("{e:?}")),
    };
    let code_size = (m.code.len() as u64 + 0xfff) & !0xfff;
    let setup = (|| -> Result<(), unicorn_engine::unicorn_const::uc_error> {
        emu.mem_map(CODE, code_size.max(0x1000), Permission::ALL)?;
        emu.mem_map(MAGIC, 0x1000, Permission::ALL)?;
        emu.mem_map(DATA, 0x10_0000, Permission::READ | Permission::WRITE)?;
        emu.mem_write(CODE, &m.code)?;
        // Constants from PVar upward, the 0.5 and the options below it.
        emu.mem_write(PVAR, &m.constants)?;
        for (off, kind, v) in &mem {
            let a = (PVAR as i64 + off) as u64;
            match kind {
                SlotKind::F64 => emu.mem_write(a, &v.to_le_bytes())?,
                SlotKind::F32 => emu.mem_write(a, &(*v as f32).to_le_bytes())?,
                SlotKind::I32 => emu.mem_write(a, &(*v as i32).to_le_bytes())?,
            }
        }
        // SSE on: CR4.OSFXSR | OSXMMEXCPT, CR0.MP, no EM.
        let cr0 = emu.reg_read(RegisterX86::CR0)?;
        emu.reg_write(RegisterX86::CR0, (cr0 & !4) | 2)?;
        let cr4 = emu.reg_read(RegisterX86::CR4)?;
        emu.reg_write(RegisterX86::CR4, cr4 | 0x600)?;
        Ok(())
    })();
    if let Err(e) = setup {
        return Outcome::Emulator(format!("setup: {e:?}"));
    }

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ name.len() as u64);
    for point in 0..10 {
        let p = [rng.next() * 3.2 - 1.6, rng.next() * 3.2 - 1.6, rng.next() * 3.2 - 1.6];
        // Translated state.
        let mut regs = vec![0.0; cpu.iter_regs];
        regs[STATE_COUNT..STATE_COUNT + consts.len()].copy_from_slice(&consts);
        let set = |regs: &mut Vec<f64>, name: &str, v: f64| regs[state_index(name)] = v;
        for (n, v) in [("x", p[0]), ("y", p[1]), ("z", p[2]), ("w", 0.0), ("cx", p[0]), ("cy", p[1]), ("cz", p[2]), ("cw", 0.0), ("dr", 1.0)] {
            set(&mut regs, n, v);
        }
        for (k, n) in ["px", "py", "pz"].iter().enumerate() {
            set(&mut regs, n, p[k]);
        }
        let r0 = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
        set(&mut regs, "rout", r0);
        // Emulator record.
        let mut rec = vec![0u8; 0x400];
        let put = |rec: &mut Vec<u8>, off: i64, v: f64| {
            let o = (off + 0x100) as usize;
            rec[o..o + 8].copy_from_slice(&v.to_le_bytes());
        };
        let put32 = |rec: &mut Vec<u8>, off: i64, b: [u8; 4]| {
            let o = (off + 0x100) as usize;
            rec[o..o + 4].copy_from_slice(&b);
        };
        for (off, v) in [(-32, p[0]), (-24, p[1]), (-16, p[2]), (-8, if four_d { 0.0 } else { 1.0 }), (0, p[0]), (8, p[1]), (16, p[2]), (24, p[0]), (32, p[1]), (40, p[2]), (56, r0)] {
            put(&mut rec, off, v);
        }
        if four_d {
            put(&mut rec, 232, 1.0);
        }
        if difs {
            // VaryScale starts at 1 in the dIFS loop.
            put(&mut rec, 200, 1.0);
            set(&mut regs, "a0", 1.0);
        }
        put32(&mut rec, 48, (PVAR as u32).to_le_bytes());
        put32(&mut rec, 68, 60i32.to_le_bytes());
        put32(&mut rec, 72, (256.0f32).to_le_bytes());
        for it in 0..4i32 {
            // Hybrid loop bookkeeping before the call.
            let rout = regs[state_index("rout")];
            set(&mut regs, "rold", rout);
            set(&mut regs, "i", it as f64);
            put(&mut rec, -48, rout);
            put32(&mut rec, 64, it.to_le_bytes());
            if emu.mem_write(REC - 0x100, &rec).is_err() {
                return Outcome::Emulator("record write".into());
            }
            let esp = STACK_TOP - 0x100;
            let mut stack = Vec::new();
            stack.extend_from_slice(&(MAGIC as u32).to_le_bytes());
            if !difs {
                stack.extend_from_slice(&(REC as u32).to_le_bytes());
                stack.extend_from_slice(&((REC - 8) as u32).to_le_bytes());
            }
            let regs_ok = (|| -> Result<(), unicorn_engine::unicorn_const::uc_error> {
                emu.mem_write(esp, &stack)?;
                emu.reg_write(RegisterX86::ESP, esp)?;
                emu.reg_write(RegisterX86::EBP, 0)?;
                emu.reg_write(RegisterX86::EAX, REC - 32)?;
                emu.reg_write(RegisterX86::EDX, REC - 24)?;
                emu.reg_write(RegisterX86::ECX, REC - 16)?;
                emu.reg_write(RegisterX86::EBX, 0)?;
                emu.reg_write(RegisterX86::ESI, if difs { REC + 88 } else { 0 })?;
                emu.reg_write(RegisterX86::EDI, if difs { PVAR } else { 0 })?;
                emu.reg_write(RegisterX86::FPCW, 0x1332)?;
                emu.emu_start(CODE, MAGIC, 2_000_000, 1_000_000)?;
                Ok(())
            })();
            if let Err(e) = regs_ok {
                return Outcome::Emulator(format!("run: {e:?} (point {point}, iteration {it})"));
            }
            let Ok(out) = emu.mem_read_as_vec(REC - 0x100, 0x400) else { return Outcome::Emulator("record read".into()) };
            rec = out;
            // Translated iteration.
            (cpu.iter)(&mut regs);
            if std::env::var_os("M3F_DEBUG").is_some() {
                let f = |off: i64| f64::from_le_bytes(rec[(off + 0x100) as usize..(off + 0x108) as usize].try_into().unwrap_or([0; 8]));
                eprintln!("p{point} it{it}: emu x {:.6e} y {:.6e} z {:.6e} w {:.6e} J4 {:.6e} | ours x {:.6e} y {:.6e} z {:.6e} w {:.6e} cw {:.6e}", f(-32), f(-24), f(-16), f(-8), f(-56), regs[state_index("x")], regs[state_index("y")], regs[state_index("z")], regs[state_index("w")], regs[state_index("cw")]);
            }
            let fv = |off: i64| f64::from_le_bytes(rec[(off + 0x100) as usize..(off + 0x108) as usize].try_into().unwrap_or([0; 8]));
            let scale = [-32, -24, -16, -8].iter().map(|o| fv(*o).abs()).fold(0.0, f64::max);
            for off in COMPARED {
                let Some(field) = record_field(off, four_d) else { continue };
                // Ju1..Ju4 share registers with J; compare only the J fields.
                let o = (off + 0x100) as usize;
                let emu_v = f64::from_le_bytes(rec[o..o + 8].try_into().unwrap_or([0; 8]));
                let our = regs[state_index(field)];
                if !close(emu_v, our, scale) {
                    if verbose {
                        eprintln!("{src}");
                    }
                    return Outcome::Mismatch(format!("{field} (record {off:+}) after iteration {it} at point {point}: original {emu_v:.12e}, translated {our:.12e}"));
                }
            }
            let o = 208 + 0x100;
            let emu_fi = i32::from_le_bytes(rec[o..o + 4].try_into().unwrap_or([0; 4])) as f64;
            if emu_fi != regs[state_index("a1")] {
                return Outcome::Mismatch(format!("bFirstIt after iteration {it}: original {emu_fi}, translated {}", regs[state_index("a1")]));
            }
            // Loop bookkeeping after the call (counted formulas update Rout).
            if counted {
                let (x, y, z) = (regs[state_index("x")], regs[state_index("y")], regs[state_index("z")]);
                let w = if four_d { regs[state_index("w")] } else { 0.0 };
                let r = x * x + y * y + z * z + w * w;
                set(&mut regs, "rout", r);
                put(&mut rec, 56, r);
            }
            if !(regs[state_index("x")].is_finite()) {
                break;
            }
        }
    }
    Outcome::Ok
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "-v");
    let Some(input) = args.iter().find(|a| !a.starts_with('-')).map(PathBuf::from) else {
        eprintln!("usage: m3f-check <folder or .m3f> [-v]");
        std::process::exit(2);
    };
    let mut files: Vec<PathBuf> = if input.is_dir() {
        std::fs::read_dir(&input).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("m3f"))).collect()).unwrap_or_default()
    } else {
        vec![input]
    };
    files.sort();
    let (mut ok, mut skip, mut bad, mut err, mut pascal) = (0, 0, 0, 0, 0);
    for f in &files {
        let name = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        match check(f, verbose && files.len() == 1) {
            Outcome::Ok => ok += 1,
            Outcome::Untranslated => skip += 1,
            Outcome::Pascal => pascal += 1,
            Outcome::Mismatch(m) => {
                bad += 1;
                println!("MISMATCH {name}: {m}");
            }
            Outcome::Emulator(e) => {
                err += 1;
                println!("EMULATOR {name}: {e}");
            }
        }
    }
    println!("\n{ok} match, {bad} differ, {err} emulator problems, {skip} not translated, {pascal} Pascal (not checked) (of {})", files.len());
}
