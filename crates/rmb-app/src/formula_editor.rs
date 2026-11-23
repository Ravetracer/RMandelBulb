// SPDX-License-Identifier: LGPL-2.1-or-later

//! The formula editor: writes and changes formulas in the formula language (`*.mbf`) inside
//! the app, like Mandelbulber's custom formula editor or Mandelbulb3D's JIT formula editor.
//! Applied formulas join the library at once (the shader recompiles), the edited slot switches
//! to them, and scenes that use them carry their source along.

use egui::{Color32, RichText, Ui};

use crate::{icons, theme};

/// A new formula to start from.
const TEMPLATE: &str = r#"// A new formula: one iteration of z -> f(z) + c, plus the derivative dr for the
// distance estimate. Help > Language lists the statements and functions.
formula "My formula" {
    category "My formulas"
    description "What it does (shown as hover text)"
    de log
    param power "Power" = 8 [2, 16]
    iter {
        let r = max(sqrt(x * x + y * y + z * z), 1.0e-20)
        let theta = acos(clamp(z / r, -1.0, 1.0)) * power
        let phi = atan2(y, x) * power
        dr = power * pow(r, power - 1.0) * dr + 1.0
        let zr = pow(r, power)
        x = zr * sin(theta) * cos(phi) + cx
        y = zr * sin(theta) * sin(phi) + cy
        z = zr * cos(theta) + cz
    }
}
"#;

const KEYWORDS: &[&str] = &[
    "formula", "category", "description", "de", "alias", "param", "transform", "init", "iter", "let", "if", "else", "for", "in", "loop", "break", "continue",
    "log", "linear", "numeric", "difs", "bool", "int", "angle",
];

#[derive(Default)]
pub struct FormulaEditor {
    pub open: bool,
    text: String,
    /// The formula the text came from (None: new).
    from: Option<String>,
    status: Option<(String, bool)>,
    /// Template picker search text.
    search: String,
}

/// What the editor asks of the formula slot it was opened for.
pub enum EditorAction {
    /// Use this formula in the slot.
    Use(String),
}

impl FormulaEditor {
    /// Opens the editor on the source of formula `name`. Formulas that come with the program
    /// are copied under a new name, so the original stays available.
    pub fn open_formula(&mut self, name: &str) {
        let Some(f) = rmb_core::formula::find(name) else { return self.open_new() };
        self.text = if f.builtin { rename_first(&f.text, &format!("{} custom", f.def.name)) } else { f.text.clone() };
        self.from = Some(f.def.name.clone());
        self.status = Some((
            if f.builtin { format!("A copy of the built-in \"{}\". Apply to use it in the slot.", f.def.name) } else { format!("\"{}\" from {}", f.def.name, f.source) },
            false,
        ));
        self.open = true;
    }

    pub fn open_new(&mut self) {
        self.text = TEMPLATE.to_string();
        self.from = None;
        self.status = None;
        self.open = true;
    }

    /// Draws the window; returns what the edited slot should do.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<EditorAction> {
        if !self.open {
            return None;
        }
        let mut action = None;
        let mut open = self.open;
        egui::Window::new(format!("{}  Formula editor", icons::ATOM)).open(&mut open).default_size([640.0, 560.0]).resizable(true).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let apply = ui.button(format!("{}  Apply", icons::REFRESH)).on_hover_text("Compile the formula, add it to the library and use it in the selected slot (Ctrl+Enter)");
                let keys = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
                if apply.clicked() || keys {
                    action = self.apply();
                }
                if ui.button(format!("{}  Save as file", egui_phosphor::regular::FLOPPY_DISK)).on_hover_text("Save the formula to your formula folder, so it loads at every start").clicked() {
                    self.save();
                }
                if ui.button(format!("{}  New", icons::PLUS)).on_hover_text("Start from an empty formula").clicked() {
                    self.open_new();
                }
                ui.menu_button(format!("{}  Copy from…", icons::MAGNIFYING_GLASS), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search formulas").desired_width(220.0));
                    let q = self.search.to_lowercase();
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        let names: Vec<String> = rmb_core::formula::library().all().iter().map(|f| f.def.name.clone()).filter(|n| q.is_empty() || n.to_lowercase().contains(&q)).take(200).collect();
                        for n in names {
                            if ui.button(&n).on_hover_text("Start from a copy of this formula").clicked() {
                                self.open_formula(&n);
                                ui.close();
                            }
                        }
                    });
                })
                .response
                .on_hover_text("Start from a copy of any formula of the library (built-in, Mandelbulb3D or yours)");
            });
            if let Some((msg, err)) = &self.status {
                ui.label(RichText::new(msg).size(theme::fs(11.5)).color(if *err { theme::WARN } else { theme::TEXT_MUTED }));
            }
            ui.add_space(4.0);
            code_area(ui, &mut self.text, self.status.as_ref().and_then(|(m, e)| (*e).then(|| error_line(m)).flatten()));
        });
        self.open = open;
        action
    }

    /// Compiles the text into the library.
    fn apply(&mut self) -> Option<EditorAction> {
        match rmb_core::formula::add_source(&self.text, "editor") {
            Ok(names) if names.is_empty() => {
                self.status = Some(("No formula in the text (it needs a formula \"Name\" { … } block).".into(), true));
                None
            }
            Ok(names) => {
                self.status = Some((format!("Applied: {}. Scenes that use it carry its code along; Save as file keeps it for every start.", names.join(", ")), false));
                self.from = names.first().cloned();
                names.into_iter().next().map(EditorAction::Use)
            }
            Err(e) => {
                self.status = Some((format!("Error in {e}"), true));
                None
            }
        }
    }

    /// Writes the text to the user formula folder (named after its first formula).
    fn save(&mut self) {
        let names = match rmb_core::formula::lang::parse_library(&self.text) {
            Ok(defs) if !defs.is_empty() => defs.into_iter().map(|d| d.name).collect::<Vec<_>>(),
            Ok(_) => {
                self.status = Some(("Nothing saved: no formula in the text.".into(), true));
                return;
            }
            Err(e) => {
                self.status = Some((format!("Nothing saved, error in {e}"), true));
                return;
            }
        };
        let dir = crate::prefs::user_formula_dir();
        let file: String = names[0].chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let path = dir.join(format!("{file}.mbf"));
        let result = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &self.text));
        self.status = Some(match result {
            Ok(()) => {
                let _ = rmb_core::formula::add_source(&self.text, &path.display().to_string());
                (format!("Saved to {}", path.display()), false)
            }
            Err(e) => (format!("Could not save {}: {e}", path.display()), true),
        });
    }
}

/// The line number in an error message ("line 12: …").
fn error_line(msg: &str) -> Option<usize> {
    let rest = &msg[msg.find("line ")? + 5..];
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// `text` with the name of its first formula replaced.
fn rename_first(text: &str, name: &str) -> String {
    let Some(i) = text.find("formula \"") else { return text.to_string() };
    let start = i + "formula \"".len();
    let Some(len) = text[start..].find('"') else { return text.to_string() };
    format!("{}{}{}", &text[..start], name, &text[start + len..])
}

/// The code with line numbers and syntax colors; `error` marks a line.
fn code_area(ui: &mut Ui, text: &mut String, error: Option<usize>) {
    let font = egui::FontId::monospace(theme::fs(12.5));
    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            let lines = text.lines().count().max(1) + usize::from(text.ends_with('\n'));
            let mut gutter = egui::text::LayoutJob::default();
            for n in 1..=lines {
                let color = if Some(n) == error { theme::WARN } else { theme::TEXT_MUTED.gamma_multiply(0.6) };
                gutter.append(&format!("{n:>4}\n"), 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
            }
            ui.label(gutter);
            let f = font.clone();
            // No wrapping: the line numbers stay with their lines (the area scrolls sideways).
            let mut layouter = move |ui: &Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                let mut job = highlight(buf.as_str(), &f);
                job.wrap.max_width = f32::INFINITY;
                ui.fonts_mut(|fo| fo.layout_job(job))
            };
            ui.add(egui::TextEdit::multiline(text).code_editor().desired_width(f32::INFINITY).desired_rows(28).lock_focus(true).layouter(&mut layouter))
                .on_hover_text("The formula code. Ctrl+Enter applies it.");
        });
    });
}

/// Syntax colors: keywords, numbers, strings, comments.
fn highlight(src: &str, font: &egui::FontId) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let fmt = |color: Color32| egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
    let (kw, num, string, comment, plain) = (theme::ACCENT, Color32::from_rgb(214, 164, 92), Color32::from_rgb(140, 196, 120), theme::TEXT_MUTED.gamma_multiply(0.75), theme::TEXT);
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        let start = i;
        if src[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            job.append(&src[start..i], 0.0, fmt(comment));
        } else if c == '"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            i = (i + 1).min(b.len());
            job.append(&src[start..i], 0.0, fmt(string));
        } else if c.is_ascii_digit() || (c == '.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'.' || ((b[i] == b'-' || b[i] == b'+') && matches!(b[i - 1], b'e' | b'E'))) {
                i += 1;
            }
            job.append(&src[start..i], 0.0, fmt(num));
        } else if c.is_alphabetic() || c == '_' {
            while i < b.len() && ((b[i] as char).is_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            job.append(word, 0.0, fmt(if KEYWORDS.contains(&word) { kw } else { plain }));
        } else {
            i += c.len_utf8();
            while !src.is_char_boundary(i) {
                i += 1;
            }
            job.append(&src[start..i], 0.0, fmt(plain));
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_error_lines_and_renames() {
        assert_eq!(error_line("Error in line 12: unknown variable 'q'"), Some(12));
        assert_eq!(error_line("no line here"), None);
        assert_eq!(rename_first("// c\nformula \"Mandelbulb\" {\n}", "Mandelbulb custom"), "// c\nformula \"Mandelbulb custom\" {\n}");
        // The template compiles.
        assert_eq!(rmb_core::formula::lang::parse_library(TEMPLATE).map(|d| d.len()).ok(), Some(1));
    }
}
