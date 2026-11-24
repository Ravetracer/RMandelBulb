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
    ("kleinian.mbf", include_str!("lib/kleinian.mbf")),
    ("ifs.mbf", include_str!("lib/ifs.mbf")),
    ("hypercomplex.mbf", include_str!("lib/hypercomplex.mbf")),
    ("transforms.mbf", include_str!("lib/transforms.mbf")),
    // Mandelbulb3D formulas translated from their machine code (`rmb-cli m3f`).
    ("mb3d_builtin.mbf", include_str!("lib/mb3d_builtin.mbf")),
    ("mb3d.mbf", include_str!("lib/mb3d.mbf")),
];

/// A library formula: definition plus its CPU compilation.
pub struct Formula {
    pub def: FormulaDef,
    pub cpu: cpu::CpuFormula,
    /// Where it came from (built-in file name, user file path, "scene" or "editor").
    pub source: String,
    /// Its source text (the `formula` block).
    pub text: String,
    /// One of the formulas that come with the program (unchanged).
    pub builtin: bool,
}

impl Formula {
    /// Hash of the source text: identifies the generated code (a changed formula of the same
    /// name compiles a new shader).
    pub fn text_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.text.hash(&mut h);
        h.finish()
    }
}

/// All known formulas, by name.
#[derive(Clone)]
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
            match lang::parse_library_texts(src) {
                Ok(defs) => defs.into_iter().for_each(|(d, t)| lib.add(d, file, t, true)),
                // Built-in sources are covered by tests; report instead of panicking.
                Err(e) => lib.errors.push(format!("{file}: {e}")),
            }
        }
        lib
    }

    fn add(&mut self, def: FormulaDef, source: &str, text: String, builtin: bool) {
        let cpu = cpu::CpuFormula::compile(&def);
        let name = def.name.clone();
        let aliases: Vec<String> = def.aliases.iter().map(|a| &a.name).chain(std::iter::once(&def.name)).map(|a| a.to_lowercase()).collect();
        let f = Arc::new(Formula { def, cpu, source: source.to_string(), text, builtin });
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
            match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| lang::parse_library_texts(&s).map_err(|e| e.to_string())) {
                Ok(defs) => defs.into_iter().for_each(|(d, t)| self.add(d, &p.display().to_string(), t, false)),
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
            match m3f::translate_file(&p).and_then(|t| lang::parse_library_texts(&t.source).map_err(|e| e.to_string())) {
                Ok(defs) => {
                    ok += 1;
                    defs.into_iter().for_each(|(d, t)| self.add(d, &p.display().to_string(), t, false));
                }
                Err(_) => failed += 1,
            }
        }
        (ok, failed)
    }

    /// Adds (or replaces) the formulas of a source text; returns their names.
    pub fn add_source(&mut self, src: &str, origin: &str) -> Result<Vec<String>, LangError> {
        let defs = lang::parse_library_texts(src)?;
        let names = defs.iter().map(|(d, _)| d.name.clone()).collect();
        defs.into_iter().for_each(|(d, t)| self.add(d, origin, t, false));
        Ok(names)
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

/// The global library as a snapshot: readers take the current one (no lock is held while they
/// use it, so nested lookups never wait for a writer); writers build a changed copy and swap
/// it in, one at a time (`WRITER`).
static LIBRARY: LazyLock<RwLock<Arc<Library>>> = LazyLock::new(|| RwLock::new(Arc::new(Library::builtin())));
static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The current formula library.
pub fn library() -> Arc<Library> {
    LIBRARY.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Changes the global library: `f` works on a copy that then replaces it.
fn change<T>(f: impl FnOnce(&mut Library) -> T) -> T {
    let _w = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let mut lib = (*library()).clone();
    let out = f(&mut lib);
    *LIBRARY.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(lib);
    out
}

/// Adds the user formulas of `dir` to the global library.
pub fn load_user_formulas(dir: &std::path::Path) {
    change(|lib| lib.load_dir(dir));
}

/// Adds the translatable Mandelbulb3D formulas (`*.m3f`) of `dir`; returns (translated, failed).
pub fn load_m3f_formulas(dir: &std::path::Path) -> (usize, usize) {
    change(|lib| lib.load_m3f_dir(dir))
}

/// Looks a formula up in the global library (name or alias).
pub fn find(name: &str) -> Option<Arc<Formula>> {
    library().find(name)
}

/// Adds (or replaces) the formulas of a source text in the global library (formula editor,
/// formulas stored in scenes); returns their names.
pub fn add_source(src: &str, origin: &str) -> Result<Vec<String>, LangError> {
    change(|lib| lib.add_source(src, origin))
}

/// The source texts of the formulas `names` uses that do not come with the program (user
/// files, edited or imported formulas), by formula name: a scene carries them along.
pub fn user_sources<'a>(names: impl Iterator<Item = &'a str>) -> std::collections::BTreeMap<String, String> {
    let lib = library();
    names.filter_map(|n| lib.find(n)).filter(|f| !f.builtin).map(|f| (f.def.name.clone(), f.text.clone())).collect()
}

/// Registers formula sources a scene carried along, unless the library has the same text
/// already. Problems go to the library's error list.
pub fn register_sources(sources: &std::collections::BTreeMap<String, String>) {
    let current = library();
    if sources.iter().all(|(name, text)| current.get(name).is_some_and(|f| f.text == *text)) {
        return;
    }
    change(|lib| register_into(lib, sources));
}

fn register_into(lib: &mut Library, sources: &std::collections::BTreeMap<String, String>) {
    for (name, text) in sources {
        if lib.get(name).is_some_and(|f| f.text == *text) {
            continue;
        }
        if let Err(e) = lib.add_source(text, "scene") {
            lib.errors.push(format!("formula \"{name}\" from a scene: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edited_formulas_travel_with_scenes_and_change_the_shader() {
        let src = |k: f64| format!("formula \"Test Embedded\" {{\n    param s \"Scale\" = 2\n    iter {{\n        x = x * s + cx * {k}\n        y = y * s + cy\n        z = z * s + cz\n        dr = dr * s\n    }}\n}}\n");
        add_source(&src(1.0), "editor").expect("parses");
        let mut scene = crate::Scene::default();
        scene.fractal.slots = vec![crate::scene::Slot::new("Test Embedded"), crate::scene::Slot::new("Mandelbulb")];
        let json = scene.to_json();
        let back: crate::Scene = serde_json::from_str(&json).expect("json");
        assert_eq!(back.formulas.len(), 1, "only the non-built-in formula");
        assert!(back.formulas["Test Embedded"].contains("cx * 1"));
        let sig1 = crate::Program::for_scene(&scene).signature();
        // A scene with a changed version registers it; the shader signature follows the text.
        let changed = json.replace("cx * 1", "cx * 3");
        let loaded = crate::Scene::from_json(&changed).expect("load");
        assert!(loaded.formulas.is_empty(), "registered and emptied");
        assert!(find("Test Embedded").is_some_and(|f| f.text.contains("cx * 3")));
        assert_ne!(sig1, crate::Program::for_scene(&scene).signature());
    }

    #[test]
    fn builtin_library_parses() {
        let lib = library();
        assert!(lib.errors.is_empty(), "{:?}", lib.errors);
        assert!(lib.all().len() >= 20, "only {} formulas", lib.all().len());
        assert!(lib.find("integer power").is_some());
        // Every built-in formula knows its own text, which parses back to the same formula.
        for f in lib.all() {
            let back = lang::parse_library(&f.text).unwrap_or_else(|e| panic!("{}: {e}", f.def.name));
            assert_eq!(back.len(), 1, "{}", f.def.name);
            assert_eq!(back[0].name, f.def.name);
        }
    }
}
