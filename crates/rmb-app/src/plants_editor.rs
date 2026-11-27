// SPDX-License-Identifier: LGPL-2.1-or-later

//! Editor of plants: on a seabed, or baked onto any other surface (main fractal, objects).
//! Species from presets, their placement and colors, and the L-system that grows them (start
//! string, rules, turtle settings) with a preview.

use egui::{Color32, RichText, Ui, vec2};
use rmb_core::plant_cover::{PlantCover, Status};
use rmb_core::plants::{MAX_SEGMENTS, MAX_SPECIES, PlantRule, PlantSpecies};

use crate::{icons, theme, widgets};

const SYMBOLS: &str = "F draws a branch segment, f moves without drawing, + and - turn left and right, & and ^ pitch down and up, \\ and / roll, | turns around, [ starts a side branch (shorter and thinner by the level factors), ] ends it, ! thins the branch, L adds a leaf. Other letters (A, B, …) draw nothing: use them as growing tips in the rules.";

/// The plants section of a seabed object (`surface` false) or of a cover (`surface` true:
/// growth direction and steepest slope too).
pub fn plants_editor(ui: &mut Ui, plants: &mut Vec<PlantSpecies>, surface: bool) {
    let intro = if surface {
        "Plants grown by L-systems: a start string is rewritten by rules a number of times, then a turtle draws the result. Up to four kinds grow on this surface; each takes its colors from its own settings. Their places are baked once (again when the surface or the placement changes)."
    } else {
        "Plants grown by L-systems: a start string is rewritten by rules a number of times, then a turtle draws the result. Up to four kinds grow on the ground; each takes its colors from its own settings."
    };
    ui.add(egui::Label::new(RichText::new(intro).color(theme::TEXT_MUTED)).wrap());
    ui.horizontal(|ui| {
        ui.add_enabled_ui(plants.len() < 8, |ui| {
            ui.menu_button(format!("{}  Add plant", icons::PLUS), |ui| {
                for name in PlantSpecies::preset_names() {
                    if ui.button(name).on_hover_text(format!("Add the preset '{name}'")).clicked() {
                        if let Some(mut p) = PlantSpecies::preset(name) {
                            p.seed = plants.len() as u32 + 1;
                            plants.push(p);
                        }
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Add a kind of plant from a preset; edit its L-system below");
        });
        let enabled = plants.iter().filter(|p| p.enabled).count();
        if enabled > MAX_SPECIES {
            ui.label(RichText::new(format!("{}  only the first {MAX_SPECIES} visible kinds grow", icons::WARNING)).color(theme::WARN));
        }
    });
    let mut remove = None;
    let mut swap = None;
    let n = plants.len();
    for (i, p) in plants.iter_mut().enumerate() {
        let id = ui.id().with(("plant", i));
        egui::CollapsingHeader::new(RichText::new(if p.enabled { p.name.clone() } else { format!("{} (hidden)", p.name) }).strong()).id_salt(id).default_open(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                if widgets::icon_button(ui, icons::CARET_UP, "Move up").clicked() && i > 0 {
                    swap = Some((i, i - 1));
                }
                if widgets::icon_button(ui, icons::CARET_DOWN, "Move down").clicked() && i + 1 < n {
                    swap = Some((i, i + 1));
                }
                if widgets::icon_button(ui, icons::TRASH, "Remove this kind of plant").clicked() {
                    remove = Some(i);
                }
            });
            species_editor(ui, p, i, surface);
        });
    }
    if let Some((a, b)) = swap {
        plants.swap(a, b);
    }
    if let Some(i) = remove {
        plants.remove(i);
    }
}

fn species_editor(ui: &mut Ui, p: &mut PlantSpecies, i: usize, surface: bool) {
    let d = PlantSpecies::preset(&p.name).unwrap_or_default();
    let (place, units) = if surface { ("surface", "the object's own units") } else { ("ground", "seabed units") };
    widgets::grid(ui, &format!("plant-{i}"), |ui| {
        widgets::label(ui, "Name", "Name of this kind of plant");
        ui.text_edit_singleline(&mut p.name);
        ui.end_row();
        widgets::checkbox(ui, "Visible", "Grow this kind of plant", &mut p.enabled);
        widgets::slider(ui, "Density", &format!("Share of the places on the {place} with a plant (0 = none, 1 = everywhere)"), &mut p.density, 0.0..=1.0, d.density);
        widgets::slider(ui, "Clumping", "0 = spread evenly; 1 = only in patches", &mut p.clumping, 0.0..=1.0, d.clumping);
        let spacing_tip = if surface {
            "Distance between neighbouring plants (the object's own units). Small spacings on large surfaces make many plants: the bake raises it when there would be too many".to_string()
        } else {
            "Distance between neighbouring plants (seabed units). Raised automatically so wide plants do not reach past their neighbours".to_string()
        };
        widgets::slider_log(ui, "Spacing", &spacing_tip, &mut p.spacing, 0.0005..=100.0, d.spacing);
        widgets::slider_log(ui, "Height", &format!("Height of the tallest plants ({units})"), &mut p.height, 0.0005..=100.0, d.height);
        widgets::slider(ui, "Size variation", "How much smaller some plants are (0 = all alike)", &mut p.size_variation, 0.0..=0.95, d.size_variation);
        let sway_tip = if surface { "Swaying, timed by the section's sway speed (0 = still)" } else { "Swaying in the current, timed by the seabed's sway speed and time (0 = still)" };
        widgets::slider(ui, "Sway", sway_tip, &mut p.sway, 0.0..=1.0, d.sway);
        if surface {
            widgets::slider(ui, "Upright", "Direction of growth: 0 = straight out of the surface (along its normal), 1 = straight up (the object's own up axis). Plants never grow into the surface", &mut p.upright, 0.0..=1.0, d.upright);
            widgets::slider(ui, "Steepest slope", "Plants grow only where the surface is at most this steep (degrees from facing up; 90 = up to vertical walls, 180 = everywhere, also under overhangs)", &mut p.max_slope, 0.0..=180.0, d.max_slope);
        }
        widgets::label(ui, "Branch colors", "Colors of the stems and branches at the root and at the top");
        ui.horizontal(|ui| {
            widgets::color_button(ui, &mut p.branch_colors[0]).on_hover_text("At the root");
            widgets::color_button(ui, &mut p.branch_colors[1]).on_hover_text("At the top");
        });
        ui.end_row();
        widgets::label(ui, "Leaf colors", "Colors of the leaves near the root and at the top");
        ui.horizontal(|ui| {
            widgets::color_button(ui, &mut p.leaf_colors[0]).on_hover_text("Near the root");
            widgets::color_button(ui, &mut p.leaf_colors[1]).on_hover_text("At the top");
        });
        ui.end_row();
    });
    ui.add_space(6.0);
    ui.label(RichText::new("L-SYSTEM").size(theme::fs(10.5)).color(theme::TEXT_MUTED).strong()).on_hover_text(SYMBOLS);
    widgets::grid(ui, &format!("plant-ls-{i}"), |ui| {
        widgets::label(ui, "Start", &format!("The string the growth starts from. {SYMBOLS}"));
        ui.add(egui::TextEdit::singleline(&mut p.axiom).font(egui::TextStyle::Monospace)).on_hover_text(SYMBOLS);
        ui.end_row();
        widgets::slider_u32(ui, "Generations", "How often the rules rewrite the string (more = bigger, finer plants; the drawing stops at 4000 segments)", &mut p.iterations, 0..=10, d.iterations);
    });
    ui.label(RichText::new("Rules").color(theme::TEXT_MUTED)).on_hover_text("Each rule replaces a symbol by a string in every generation. Several rules for one symbol are picked at random by their weights");
    let mut remove = None;
    for (k, r) in p.rules.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let mut sym = r.symbol.to_string();
            let resp = ui.add(egui::TextEdit::singleline(&mut sym).font(egui::TextStyle::Monospace).desired_width(18.0)).on_hover_text("The symbol this rule replaces");
            if resp.changed()
                && let Some(c) = sym.chars().last()
            {
                r.symbol = c;
            }
            ui.label("->");
            ui.add(egui::TextEdit::singleline(&mut r.replacement).font(egui::TextStyle::Monospace).desired_width(ui.available_width() - 90.0)).on_hover_text(SYMBOLS);
            ui.add(egui::DragValue::new(&mut r.weight).speed(0.01).range(0.0..=100.0).max_decimals(2)).on_hover_text("Weight: how often this rule is picked among the rules of its symbol");
            if widgets::icon_button(ui, icons::X, "Remove this rule").clicked() {
                remove = Some(k);
            }
        });
    }
    if let Some(k) = remove {
        p.rules.remove(k);
    }
    if ui.button(format!("{}  Add rule", icons::PLUS)).on_hover_text("Add a rule (symbol → replacement)").clicked() {
        p.rules.push(PlantRule { symbol: 'A', replacement: "F[+A][-A]".into(), weight: 1.0 });
    }
    widgets::grid(ui, &format!("plant-turtle-{i}"), |ui| {
        widgets::slider(ui, "Angle", "Turning angle of + - & ^ \\ / (degrees)", &mut p.angle, 0.0..=180.0, d.angle);
        widgets::slider(ui, "Angle variation", "Random change of every turn (degrees): makes the plants less regular", &mut p.angle_variation, 0.0..=90.0, d.angle_variation);
        widgets::slider(ui, "Branch length", "Length of a side branch relative to its parent (per [ )", &mut p.length_scale, 0.1..=1.5, d.length_scale);
        widgets::slider_log(ui, "Thickness", "Radius of the stem relative to a segment's length", &mut p.thickness, 0.001..=1.0, d.thickness);
        widgets::slider(ui, "Thinning", "Radius of a side branch relative to its parent (per [ and per !)", &mut p.thinning, 0.1..=1.0, d.thinning);
        widgets::slider_log(ui, "Leaf length", "Length of a leaf (L) relative to a segment's length", &mut p.leaf_length, 0.0..=10.0, d.leaf_length);
        widgets::slider(ui, "Leaf width", "Width of a leaf relative to its length", &mut p.leaf_width, 0.0..=1.0, d.leaf_width);
        widgets::slider(ui, "Tropism", "Bending of every segment: toward up (positive, like kelp held up by floats) or down (negative, drooping)", &mut p.tropism, -0.5..=0.5, d.tropism);
        widgets::label(ui, "Variant", "Random variant of the growth: picks among the rules and varies the angles");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut p.seed).speed(0.2)).on_hover_text("Number of the variant");
            if widgets::icon_button(ui, icons::REFRESH, "Next variant").clicked() {
                p.seed = p.seed.wrapping_add(1);
            }
        });
        ui.end_row();
    });
    preview(ui, p);
}

/// Side and top view of the grown plant.
fn preview(ui: &mut Ui, p: &PlantSpecies) {
    let t = rmb_core::plants::template(p);
    let size = vec2(ui.available_width().min(260.0), 150.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, theme::BG);
    let half = rect.width() * 0.5;
    let side = egui::Rect::from_min_size(rect.min, vec2(half, rect.height())).shrink(8.0);
    let top = egui::Rect::from_min_size(rect.min + vec2(half, 0.0), vec2(half, rect.height())).shrink(8.0);
    let reach = t.radius.max(0.05) as f32;
    let scale_side = (side.height() / t.top as f32).min(side.width() / (2.0 * reach));
    let scale_top = top.width().min(top.height()) / (2.0 * reach);
    let to_side = |a: [f32; 3]| egui::pos2(side.center().x + a[0] * scale_side, side.bottom() - a[2] * scale_side);
    let to_top = |a: [f32; 3]| egui::pos2(top.center().x + a[0] * scale_top, top.center().y - a[1] * scale_top);
    let col = |c: [f32; 3]| widgets::to_color32(c);
    for g in &t.segs {
        let h = ((g.a[2] + g.b[2]) * 0.5 / t.top as f32).clamp(0.0, 1.0);
        let pair = if g.leaf { p.leaf_colors } else { p.branch_colors };
        let c = col([0, 1, 2].map(|k| pair[0][k] + (pair[1][k] - pair[0][k]) * h));
        if g.leaf {
            // A leaf: the diamond through its tips and its widest points (in the middle).
            let mid = [0, 1, 2].map(|k| (g.a[k] + g.b[k]) * 0.5);
            let (l, r) = ([0, 1, 2].map(|k| mid[k] + g.side[k] * g.ra), [0, 1, 2].map(|k| mid[k] - g.side[k] * g.ra));
            for to in [&to_side as &dyn Fn([f32; 3]) -> egui::Pos2, &to_top] {
                painter.add(egui::Shape::convex_polygon(vec![to(g.a), to(l), to(g.b), to(r)], c, egui::Stroke::new(0.5, c)));
            }
        } else {
            painter.line_segment([to_side(g.a), to_side(g.b)], egui::Stroke::new((g.ra * 2.0 * scale_side).max(1.0), c));
            painter.line_segment([to_top(g.a), to_top(g.b)], egui::Stroke::new((g.ra * 2.0 * scale_top).max(1.0), c));
        }
    }
    painter.line_segment([egui::pos2(side.left(), side.bottom()), egui::pos2(side.right(), side.bottom())], egui::Stroke::new(1.0, Color32::from_gray(90)));
    let full = t.segs.len() >= MAX_SEGMENTS;
    let text = format!("{} segments{}", t.segs.len(), if full { " (limit reached: fewer generations)" } else { "" });
    ui.label(RichText::new(text).size(theme::fs(10.5)).color(if full { theme::WARN } else { theme::TEXT_MUTED })).on_hover_text("Side view (left) and from above (right). More segments make renders near the plants slower");
}

/// The plants of a surface (`PlantCover`): bake state, region, sway, and the species.
pub fn cover_editor(ui: &mut Ui, cover: &mut PlantCover, status: Option<&Status>, id: &str) {
    match status {
        None => {
            ui.label(RichText::new("No plants yet: add one below").color(theme::TEXT_MUTED));
        }
        Some(Status::Baking(f)) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Placing the plants… {:.0} %", f * 100.0)).on_hover_text("The roots are searched on the surface in the background; the plants appear when that is done");
            });
        }
        Some(Status::Ready(set)) => {
            ui.label(RichText::new(format!("{} plants", set.roots.len())).color(theme::TEXT_MUTED)).on_hover_text("Plants placed on this surface (baked: placed again when the surface or the placement settings change)");
            for n in &set.notes {
                ui.add(egui::Label::new(RichText::new(format!("{}  {n}", icons::WARNING)).color(theme::WARN)).wrap());
            }
        }
        Some(Status::Failed(e)) => {
            ui.add(egui::Label::new(RichText::new(format!("{}  Placing the plants failed: {e}", icons::WARNING)).color(theme::WARN)).wrap());
        }
    }
    widgets::grid(ui, &format!("{id}-cover"), |ui| {
        let mut fit = cover.region.is_none();
        if widgets::checkbox(ui, "Fit to the surface", "Plants grow everywhere on the surface (within ±64 units). Off: only inside a box of the object's own coordinates", &mut fit).changed() {
            cover.region = if fit {
                None
            } else {
                Some(match status {
                    Some(Status::Ready(set)) => set.region,
                    _ => [[-1.0; 3], [1.0; 3]],
                })
            };
        }
        if let Some(r) = &mut cover.region {
            for (k, (name, tip)) in [("From", "Lower corner of the box where plants grow (x, y, z in the object's own units)"), ("To", "Upper corner of the box where plants grow (x, y, z in the object's own units)")].iter().enumerate() {
                widgets::label(ui, name, tip);
                ui.horizontal(|ui| {
                    for v in r[k].iter_mut() {
                        ui.add(egui::DragValue::new(v).speed(0.01).max_decimals(4)).on_hover_text(*tip);
                    }
                });
                ui.end_row();
            }
        }
        widgets::slider(ui, "Sway speed", "How fast the plants sway in animations (phase per second)", &mut cover.sway_speed, 0.0..=10.0, 1.0);
    });
    plants_editor(ui, &mut cover.species, true);
}
