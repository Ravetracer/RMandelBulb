// SPDX-License-Identifier: LGPL-2.1-or-later

//! Fractal formulas: the formula language, its compilers and the built-in library.

pub mod cpu;
pub mod lang;
pub mod m3f;
pub mod wgsl;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

pub use lang::{Alias, DeKind, FormulaDef, LangError, ParamDef, ParamKind};

/// Built-in formula sources (file name, source).
const BUILTIN: &[(&str, &str)] = &[
    ("bulbs.mbf", include_str!("lib/bulbs.mbf")),
    ("boxes.mbf", include_str!("lib/boxes.mbf")),
    ("ifs.mbf", include_str!("lib/ifs.mbf")),
    ("hypercomplex.mbf", include_str!("lib/hypercomplex.mbf")),
    ("transforms.mbf", include_str!("lib/transforms.mbf")),
    // Mandelbulb3D formulas translated from their machine code (`rmb-cli m3f`).
    ("mb3d.mbf", include_str!("lib/mb3d.mbf")),
];

/// A library formula: definition plus its CPU compilation.
pub struct Formula {
    pub def: FormulaDef,
    pub cpu: cpu::CpuFormula,
    /// Where it came from (built-in file name or user file path).
    pub source: String,
}

/// All known formulas, by name.
pub struct Library {
    formulas: Vec<Arc<Formula>>,
    by_name: HashMap<String, usize>,
    /// Lower-case aliases (MB3D names) to formula index.
    by_alias: HashMap<String, usize>,
    /// Problems while loading user formulas.
    pub errors: Vec<String>,
}

impl Library {
    fn builtin() -> Self {
        let mut lib = Library { formulas: Vec::new(), by_name: HashMap::new(), by_alias: HashMap::new(), errors: Vec::new() };
        for (file, src) in BUILTIN {
            match lang::parse_library(src) {
                Ok(defs) => defs.into_iter().for_each(|d| lib.add(d, file)),
                // Built-in sources are covered by tests; report instead of panicking.
                Err(e) => lib.errors.push(format!("{file}: {e}")),
            }
        }
        lib
    }

    fn add(&mut self, def: FormulaDef, source: &str) {
        let cpu = cpu::CpuFormula::compile(&def);
        let name = def.name.clone();
        let aliases: Vec<String> = def.aliases.iter().map(|a| &a.name).chain(std::iter::once(&def.name)).map(|a| a.to_lowercase()).collect();
        let f = Arc::new(Formula { def, cpu, source: source.to_string() });
        let idx = match self.by_name.get(&name) {
            Some(i) => {
                self.formulas[*i] = f;
                *i
            }
            None => {
                self.formulas.push(f);
                self.formulas.len() - 1
            }
        };
        for a in aliases {
            self.by_alias.insert(a, idx);
        }
        self.by_name.insert(name, idx);
    }

    /// Loads user formula files (`*.mbf`) from a folder; they may replace built-ins.
    pub fn load_dir(&mut self, dir: &std::path::Path) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "mbf")).collect();
        paths.sort();
        for p in paths {
            match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| lang::parse_library(&s).map_err(|e| e.to_string())) {
                Ok(defs) => defs.into_iter().for_each(|d| self.add(d, &p.display().to_string())),
                Err(e) => self.errors.push(format!("{}: {e}", p.display())),
            }
        }
    }

    /// Translates Mandelbulb3D formula files (`*.m3f`) of a folder and adds those that
    /// translate. Returns (translated, failed).
    pub fn load_m3f_dir(&mut self, dir: &std::path::Path) -> (usize, usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return (0, 0) };
        let mut paths: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("m3f"))).collect();
        paths.sort();
        let (mut ok, mut failed) = (0, 0);
        for p in paths {
            match m3f::translate_file(&p).and_then(|t| lang::parse_library(&t.source).map_err(|e| e.to_string())) {
                Ok(defs) => {
                    ok += 1;
                    defs.into_iter().for_each(|d| self.add(d, &p.display().to_string()));
                }
                Err(_) => failed += 1,
            }
        }
        (ok, failed)
    }

    pub fn get(&self, name: &str) -> Option<Arc<Formula>> {
        self.by_name.get(name).map(|i| self.formulas[*i].clone())
    }

    /// Looks up by name or alias, ignoring case.
    pub fn find(&self, name: &str) -> Option<Arc<Formula>> {
        self.get(name).or_else(|| self.by_alias.get(&name.trim().to_lowercase()).map(|i| self.formulas[*i].clone()))
    }

    /// Looks up by alias first (importers: MB3D names), then by name.
    pub fn find_alias(&self, name: &str) -> Option<Arc<Formula>> {
        self.by_alias.get(&name.trim().to_lowercase()).map(|i| self.formulas[*i].clone()).or_else(|| self.get(name))
    }

    pub fn all(&self) -> &[Arc<Formula>] {
        &self.formulas
    }

    /// Categories in first-seen order.
    pub fn categories(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in &self.formulas {
            if !out.contains(&f.def.category) {
                out.push(f.def.category.clone());
            }
        }
        out
    }
}

static LIBRARY: LazyLock<RwLock<Library>> = LazyLock::new(|| RwLock::new(Library::builtin()));

/// Read access to the global formula library.
pub fn library() -> std::sync::RwLockReadGuard<'static, Library> {
    LIBRARY.read().unwrap_or_else(|e| e.into_inner())
}

/// Adds the user formulas of `dir` to the global library.
pub fn load_user_formulas(dir: &std::path::Path) {
    LIBRARY.write().unwrap_or_else(|e| e.into_inner()).load_dir(dir);
}

/// Adds the translatable Mandelbulb3D formulas (`*.m3f`) of `dir`; returns (translated, failed).
pub fn load_m3f_formulas(dir: &std::path::Path) -> (usize, usize) {
    LIBRARY.write().unwrap_or_else(|e| e.into_inner()).load_m3f_dir(dir)
}

/// Looks a formula up in the global library (name or alias).
pub fn find(name: &str) -> Option<Arc<Formula>> {
    library().find(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_library_parses() {
        let lib = library();
        assert!(lib.errors.is_empty(), "{:?}", lib.errors);
        assert!(lib.all().len() >= 20, "only {} formulas", lib.all().len());
        assert!(lib.find("integer power").is_some());
    }
}
