// SPDX-License-Identifier: LGPL-2.1-or-later

//! The user manual in the help window: the chapters of docs/manual (embedded by build.rs) drawn
//! from their Markdown, with the images, a chapter list and a search; and the PDF version.

use std::collections::HashMap;

use egui::{Color32, RichText, Ui};

use crate::theme;

include!(concat!(env!("OUT_DIR"), "/manual_files.rs"));

/// A chapter's title (its `# ` line).
fn title(md: &str) -> &str {
    md.lines().find_map(|l| l.strip_prefix("# ")).unwrap_or("Untitled")
}

#[derive(Default)]
pub struct Manual {
    chapter: usize,
    search: String,
    textures: HashMap<String, Option<egui::TextureHandle>>,
    scroll_top: bool,
    message: Option<String>,
}

impl Manual {
    pub fn show(&mut self, ui: &mut Ui) {
        if CHAPTERS.is_empty() {
            ui.label("The manual is not part of this build.");
            return;
        }
        self.chapter = self.chapter.min(CHAPTERS.len() - 1);
        egui::Panel::left("manual-chapters").resizable(true).default_size(210.0).show(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search the manual").desired_width(f32::INFINITY));
            ui.add_space(4.0);
            let q = self.search.trim().to_lowercase();
            egui::ScrollArea::vertical().id_salt("manual-toc").show(ui, |ui| {
                for (i, (_, md)) in CHAPTERS.iter().enumerate() {
                    if !q.is_empty() && !md.to_lowercase().contains(&q) {
                        continue;
                    }
                    let label = format!("{}  {}", i + 1, title(md));
                    if ui.selectable_label(self.chapter == i, label).clicked() {
                        self.chapter = i;
                        self.scroll_top = true;
                    }
                }
            });
            ui.add_space(6.0);
            if ui.button(format!("{}  PDF manual", egui_phosphor::regular::FILE_PDF)).on_hover_text("Open the manual as a PDF document (for reading offline or printing)").clicked() {
                self.message = Some(match open_pdf() {
                    Ok(p) => format!("Opened {}", p.display()),
                    Err(e) => e,
                });
            }
            if let Some(m) = &self.message {
                ui.label(RichText::new(m).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            }
        });
        let md = CHAPTERS[self.chapter].1.replace("{{version}}", env!("CARGO_PKG_VERSION"));
        let mut area = egui::ScrollArea::vertical().id_salt(("manual-page", self.chapter)).auto_shrink([false, false]);
        if std::mem::take(&mut self.scroll_top) {
            area = area.vertical_scroll_offset(0.0);
        }
        area.show(ui, |ui| {
            ui.set_max_width(760.0);
            self.markdown(ui, &md);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if self.chapter > 0 && ui.button(format!("‹  {}", title(CHAPTERS[self.chapter - 1].1))).clicked() {
                    self.chapter -= 1;
                    self.scroll_top = true;
                }
                if self.chapter + 1 < CHAPTERS.len() && ui.button(format!("{}  ›", title(CHAPTERS[self.chapter + 1].1))).clicked() {
                    self.chapter += 1;
                    self.scroll_top = true;
                }
            });
        });
    }

    /// The texture of a manual image (loaded once).
    fn texture(&mut self, ui: &Ui, name: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.textures.get(name) {
            return t.clone();
        }
        let tex = IMAGES.iter().find(|(n, _)| *n == name).and_then(|(_, bytes)| image::load_from_memory(bytes).ok()).map(|img| {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            ui.ctx().load_texture(format!("manual-{name}"), egui::ColorImage::from_rgba_unmultiplied(size, &rgba), egui::TextureOptions::LINEAR)
        });
        self.textures.insert(name.to_string(), tex.clone());
        tex
    }

    /// Draws the Markdown subset the manual uses: `#`-`###` headings, paragraphs with
    /// **bold**, *italic*, `code`, <kbd>keys</kbd> and [links](…); `- ` and `1. ` lists,
    /// pipe tables, fenced code blocks, `!!! tip/note/warning` boxes and images
    /// (`![caption](images/name){: .half}`).
    fn markdown(&mut self, ui: &mut Ui, md: &str) {
        let lines: Vec<&str> = md.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            let t = line.trim();
            if t.is_empty() {
                i += 1;
                continue;
            }
            if t.starts_with("```") {
                let mut code = Vec::new();
                i += 1;
                while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                    code.push(lines[i]);
                    i += 1;
                }
                i += 1;
                egui::Frame::new().fill(theme::SURFACE).corner_radius(4).inner_margin(8).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(code.join("\n")).monospace().color(theme::TEXT)).extend());
                });
                ui.add_space(6.0);
                continue;
            }
            if let Some(h) = t.strip_prefix("# ") {
                ui.label(RichText::new(h).strong().size(theme::fs(22.0)).color(theme::TEXT));
                ui.add_space(6.0);
                i += 1;
                continue;
            }
            if let Some(h) = t.strip_prefix("## ") {
                ui.add_space(10.0);
                ui.label(RichText::new(h).strong().size(theme::fs(17.0)).color(theme::TEXT));
                ui.add_space(4.0);
                i += 1;
                continue;
            }
            if let Some(h) = t.strip_prefix("### ") {
                ui.add_space(6.0);
                ui.label(RichText::new(h).strong().size(theme::fs(14.5)).color(theme::TEXT));
                ui.add_space(2.0);
                i += 1;
                continue;
            }
            if let Some(kind) = t.strip_prefix("!!! ") {
                let mut body = Vec::new();
                i += 1;
                while i < lines.len() && (lines[i].starts_with("    ") || lines[i].trim().is_empty()) {
                    if lines[i].trim().is_empty() && body.last().is_some_and(|l: &String| l.is_empty()) {
                        break;
                    }
                    body.push(lines[i].trim().to_string());
                    i += 1;
                }
                let color = match kind.split_whitespace().next().unwrap_or("note") {
                    "warning" => theme::WARN,
                    "tip" => Color32::from_rgb(110, 200, 140),
                    _ => theme::ACCENT,
                };
                egui::Frame::new().fill(theme::SURFACE).stroke(egui::Stroke::new(1.0, color)).corner_radius(4).inner_margin(8).show(ui, |ui| {
                    ui.label(RichText::new(kind.split_whitespace().next().unwrap_or("note").to_uppercase()).size(theme::fs(10.5)).strong().color(color));
                    inline(ui, &body.join(" "));
                });
                ui.add_space(6.0);
                continue;
            }
            if t.starts_with("![") {
                self.image(ui, t);
                i += 1;
                continue;
            }
            if t.starts_with('|') {
                let mut rows: Vec<Vec<String>> = Vec::new();
                while i < lines.len() && lines[i].trim().starts_with('|') {
                    let cells: Vec<String> = lines[i].trim().trim_matches('|').split('|').map(|c| c.trim().to_string()).collect();
                    if !cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':')) {
                        rows.push(cells);
                    }
                    i += 1;
                }
                table(ui, &rows);
                ui.add_space(6.0);
                continue;
            }
            if let Some(marker) = list_marker(t) {
                // A list: items with their indented continuation lines.
                let mut items: Vec<(String, String)> = Vec::new();
                while i < lines.len() {
                    let l = lines[i];
                    let lt = l.trim();
                    if lt.is_empty() {
                        // A blank line ends the list unless the next line continues it.
                        if lines.get(i + 1).is_some_and(|n| list_marker(n.trim()).is_some() || n.starts_with("   ")) {
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    if let Some(m) = list_marker(lt) {
                        items.push((m.0, m.1.to_string()));
                    } else if l.starts_with(' ') {
                        if let Some(last) = items.last_mut() {
                            last.1.push(' ');
                            last.1.push_str(lt);
                        }
                    } else {
                        break;
                    }
                    i += 1;
                }
                let _ = marker;
                for (bullet, text) in items {
                    ui.horizontal_top(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new(bullet).color(theme::ACCENT));
                        inline(ui, &text);
                    });
                }
                ui.add_space(4.0);
                continue;
            }
            // A paragraph: lines up to a blank line or the start of another block.
            let mut para = Vec::new();
            while i < lines.len() {
                let lt = lines[i].trim();
                if lt.is_empty() || lt.starts_with('#') || lt.starts_with("```") || lt.starts_with("!!!") || lt.starts_with('|') || (lt.starts_with("![") && !para.is_empty()) || list_marker(lt).is_some() {
                    break;
                }
                para.push(lt);
                i += 1;
            }
            if para.is_empty() {
                i += 1;
                continue;
            }
            inline(ui, &para.join(" "));
            ui.add_space(6.0);
        }
    }

    fn image(&mut self, ui: &mut Ui, t: &str) {
        let caption = t.trim_start_matches("![").split("](").next().unwrap_or("");
        let path = t.split("](").nth(1).and_then(|r| r.split(')').next()).unwrap_or("");
        let name = path.rsplit('/').next().unwrap_or(path);
        let share = if t.contains(".half") { 0.5 } else if t.contains(".third") { 0.34 } else { 1.0 };
        match self.texture(ui, name) {
            Some(tex) => {
                let [w, h] = tex.size();
                let max = (ui.available_width() * share).min(w as f32);
                let size = egui::vec2(max, max * h as f32 / w as f32);
                ui.add(egui::Image::new(&tex).fit_to_exact_size(size).corner_radius(4));
            }
            None => {
                ui.label(RichText::new(format!("[image {name}]")).color(theme::TEXT_MUTED));
            }
        }
        if !caption.is_empty() {
            ui.label(RichText::new(strip_inline(caption)).size(theme::fs(11.5)).italics().color(theme::TEXT_MUTED));
        }
        ui.add_space(8.0);
    }
}

/// A pipe table: columns as wide as their longest text allows (shares of the page width),
/// cells wrap inside their column; the first row is the header.
fn table(ui: &mut Ui, rows: &[Vec<String>]) {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    let len = |k: usize| rows.iter().filter_map(|r| r.get(k)).map(|c| strip_inline(c).chars().count().min(60) as f32).fold(4.0, f32::max);
    let weights: Vec<f32> = (0..cols).map(len).collect();
    let total: f32 = weights.iter().sum();
    let gap = 12.0;
    let width = ui.available_width() - gap * (cols as f32 - 1.0);
    for (r, row) in rows.iter().enumerate() {
        let fill = if r == 0 { theme::SURFACE_HOVER } else if r % 2 == 0 { theme::SURFACE } else { Color32::TRANSPARENT };
        egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(4, 2)).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (k, weight) in weights.iter().enumerate() {
                    let w = (width - 8.0) * weight / total;
                    let text = row.get(k).map(String::as_str).unwrap_or("");
                    ui.allocate_ui_with_layout(egui::vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(w);
                        if r == 0 {
                            ui.add(egui::Label::new(RichText::new(strip_inline(text)).strong()).wrap());
                        } else {
                            inline(ui, text);
                        }
                    });
                }
            });
        });
    }
}

/// `- item` or `12. item`: the bullet to show and the item text.
fn list_marker(t: &str) -> Option<(String, &str)> {
    if let Some(rest) = t.strip_prefix("- ") {
        return Some(("•".into(), rest));
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && t[digits..].starts_with(". ") {
        return Some((t[..digits + 1].to_string(), &t[digits + 2..]));
    }
    None
}

/// Text without the inline markup.
fn strip_inline(s: &str) -> String {
    s.replace("**", "").replace('`', "").replace("<kbd>", "").replace("</kbd>", "")
}

/// A paragraph with **bold**, *italic*, `code`, <kbd>keys</kbd> and [links](…).
fn inline(ui: &mut Ui, text: &str) {
    let body = egui::TextStyle::Body.resolve(ui.style());
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let fmt = |font: &egui::FontId, color: Color32, italics: bool| egui::TextFormat { font_id: font.clone(), color, italics, ..Default::default() };
    let mut job = egui::text::LayoutJob::default();
    let (mut bold, mut italic) = (false, false);
    let mut rest = text;
    while !rest.is_empty() {
        let plain = |bold: bool, italic: bool| fmt(&body, if bold { Color32::WHITE } else { theme::TEXT }, italic);
        if let Some(r) = rest.strip_prefix("**") {
            bold = !bold;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("<kbd>") {
            let (k, r2) = r.split_once("</kbd>").unwrap_or((r, ""));
            job.append(&format!(" {k} "), 0.0, egui::TextFormat { background: theme::SURFACE_HOVER, ..fmt(&mono, theme::TEXT, false) });
            rest = r2;
        } else if let Some(r) = rest.strip_prefix('`') {
            let (c, r2) = r.split_once('`').unwrap_or((r, ""));
            job.append(c, 0.0, fmt(&mono, theme::ACCENT, false));
            rest = r2;
        } else if rest.starts_with('*') && rest[1..].starts_with(|c: char| !c.is_whitespace()) {
            italic = !italic;
            rest = &rest[1..];
        } else if rest.starts_with('*') && italic {
            italic = false;
            rest = &rest[1..];
        } else if let Some(r) = rest.strip_prefix('[')
            && let Some((label, after)) = r.split_once("](")
            && let Some((_, r2)) = after.split_once(')')
        {
            job.append(label, 0.0, fmt(&body, theme::ACCENT, italic));
            rest = r2;
        } else {
            let end = rest[1..].find(['*', '`', '<', '[']).map_or(rest.len(), |k| k + 1);
            job.append(&rest[..end], 0.0, plain(bold, italic));
            rest = &rest[end..];
        }
    }
    ui.add(egui::Label::new(job).wrap());
}

/// Opens the PDF manual (`RMandelbulb3D-Manual-*.pdf`) with the system's viewer: next to the
/// program, in `share/doc/rmandelbulb3d`, in the macOS bundle's `Resources` or, for development
/// builds, in docs/manual/build.
pub fn open_pdf() -> Result<std::path::PathBuf, String> {
    use std::path::PathBuf;
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mut dirs: Vec<PathBuf> = exe_dir.iter().cloned().collect();
    if let Some(d) = &exe_dir {
        dirs.extend(d.parent().map(|usr| usr.join("share").join("doc").join("rmandelbulb3d")));
        dirs.extend(d.parent().map(|contents| contents.join("Resources")));
        dirs.extend(d.ancestors().nth(2).map(|root| root.join("docs").join("manual").join("build")));
    }
    let pdf = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())))
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("RMandelbulb3D-Manual") && n.to_string_lossy().ends_with(".pdf")))
        .max()
        .ok_or("The PDF manual (RMandelbulb3D-Manual-….pdf) was not found next to the program; build it with python3 tools/build_manual.py")?;
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(&pdf).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&pdf).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&pdf).spawn();
    result.map(|_| pdf.clone()).map_err(|e| format!("Could not open {}: {e}", pdf.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_is_embedded_and_its_images_exist() {
        assert!(CHAPTERS.len() >= 10, "{} chapters", CHAPTERS.len());
        for (file, md) in CHAPTERS {
            assert!(md.starts_with("# "), "{file} starts with its title");
            for part in md.split("](images/").skip(1) {
                let name = part.split(')').next().unwrap_or("");
                assert!(IMAGES.iter().any(|(n, _)| *n == name), "{file}: image {name} is missing");
            }
        }
        assert_eq!(list_marker("12. Twelve"), Some(("12.".into(), "Twelve")));
        assert_eq!(strip_inline("**a** `b` <kbd>F1</kbd>"), "a b F1");
    }
}
