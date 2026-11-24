// SPDX-License-Identifier: LGPL-2.1-or-later

//! Embeds the user manual (docs/manual: chapters `NN-*.md` and `images/*`) into the app: new
//! chapters and images show in the help window without code changes.

use std::fmt::Write;
use std::path::Path;

fn main() {
    // The program icon of the Windows .exe.
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=no program icon: {e}");
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual");
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed={}", root.join("images").display());
    let mut chapters: Vec<_> = std::fs::read_dir(&root)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md") && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(|c: char| c.is_ascii_digit()))).collect())
        .unwrap_or_default();
    chapters.sort();
    let mut images: Vec<_> = std::fs::read_dir(root.join("images")).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect()).unwrap_or_default();
    images.sort();
    let mut out = String::from("/// Chapters of the user manual (file name, Markdown).\npub const CHAPTERS: &[(&str, &str)] = &[\n");
    for p in &chapters {
        println!("cargo:rerun-if-changed={}", p.display());
        let _ = writeln!(out, "    ({:?}, include_str!({:?})),", p.file_name().unwrap().to_string_lossy(), p.canonicalize().unwrap().display().to_string());
    }
    out.push_str("];\n\n/// Images of the user manual (file name, bytes).\npub const IMAGES: &[(&str, &[u8])] = &[\n");
    for p in &images {
        println!("cargo:rerun-if-changed={}", p.display());
        let _ = writeln!(out, "    ({:?}, include_bytes!({:?})),", p.file_name().unwrap().to_string_lossy(), p.canonicalize().unwrap().display().to_string());
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("manual_files.rs");
    std::fs::write(dest, out).unwrap();
}
