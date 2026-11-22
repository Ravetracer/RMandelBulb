// SPDX-License-Identifier: LGPL-2.1-or-later

//! Editor of a material's texture layers (rmb-core texture.rs).

use std::collections::HashMap;

use egui::{RichText, Ui};
use rmb_core::texture::{BlendMode, MAX_LAYERS, Mapping, Space, Target, TextureLayer};

use crate::{icons, theme, widgets};

const IMAGES: [&str; 5] = ["jpg", "jpeg", "png", "bmp", "pgm"];

/// Small previews of the layer images, by file name.
#[derive(Clone, Default)]
struct Thumbs(HashMap<String, Option<egui::TextureHandle>>);

/// A preview of image `name` (None if it does not load).
fn thumbnail(ui: &Ui, name: &str) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("layer-thumbs");
    if let Some(t) = ui.ctx().data(|d| d.get_temp::<Thumbs>(id)).and_then(|c| c.0.get(name).cloned()) {
        return t;
    }
    let tex = rmb_core::maps::get_any(name).map(|m| {
        let step = (m.width.max(m.height) / 48).max(1);
        let (w, h) = ((m.width / step).max(1), (m.height / step).max(1));
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let p = m.pixels[((y * step) * m.width + x * step) as usize];
                rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        ui.ctx().load_texture(format!("layer-thumb-{name}"), img, egui::TextureOptions::LINEAR)
    });
    ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Thumbs>(id).0.insert(name.to_string(), tex.clone()));
    tex
}

/// Image file row: pick button, name field, preview, remove button. `what` names it in the
/// hover texts.
pub fn image_row(ui: &mut Ui, name: &mut String, what: &str, tip: &str) {
    ui.horizontal(|ui| {
        if ui.button(format!("{}…", what)).on_hover_text(tip).clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("Image", &IMAGES).pick_file()
        {
            *name = p.display().to_string();
        }
        ui.add(egui::TextEdit::singleline(name).desired_width(110.0).hint_text("none")).on_hover_text("File, name in the map folders, or Mandelbulb3D map number");
        if !name.trim().is_empty() {
            match thumbnail(ui, name.trim()) {
                Some(t) => {
                    ui.add(egui::Image::new(&t).fit_to_exact_size(egui::vec2(22.0, 22.0))).on_hover_text(name.trim().to_string());
                }
                None => {
                    ui.label(RichText::new(icons::WARNING).color(theme::WARN)).on_hover_text("Not found: give the full path or set the maps folder in the preferences");
                }
            }
            if widgets::icon_button(ui, icons::X, &format!("Remove the {}", what.to_lowercase())).clicked() {
                name.clear();
            }
        }
    });
}

fn title(l: &TextureLayer) -> String {
    let file = |s: &str| std::path::Path::new(s.trim()).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let what = if !l.image.trim().is_empty() {
        file(&l.image)
    } else if !l.mask.trim().is_empty() {
        format!("color through {}", file(&l.mask))
    } else {
        "solid color".into()
    };
    match l.target {
        Target::Color => format!("{what} · {}", l.blend.label()),
        t => format!("{what} · {}", t.label()),
    }
}

/// The layer list of a material. `id` keeps the widgets of different materials apart.
pub fn layers_editor(ui: &mut Ui, layers: &mut Vec<TextureLayer>, id: &str) {
    widgets::section(ui, "Texture layers", false);
    if layers.is_empty() {
        ui.label(RichText::new("Images (or a color through a mask) on top of the gradient colors, as bumps, or as glow.").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
    }
    let n = layers.len();
    let mut action: Option<(usize, i32)> = None;
    // Listed top layer first, like in image editors.
    for i in (0..n).rev() {
        let l = &mut layers[i];
        ui.horizontal(|ui| {
            let eye = if l.enabled { egui_phosphor::regular::EYE } else { egui_phosphor::regular::EYE_SLASH };
            if widgets::icon_button(ui, eye, if l.enabled { "Hide this layer" } else { "Show this layer" }).clicked() {
                l.enabled = !l.enabled;
            }
            let text = RichText::new(title(l)).color(if l.enabled { theme::TEXT } else { theme::TEXT_MUTED });
            egui::CollapsingHeader::new(text).id_salt((id, "layer", i)).default_open(i + 1 == n).show(ui, |ui| layer_editor(ui, l, &format!("{id}-{i}")));
        });
        ui.horizontal(|ui| {
            ui.add_space(30.0);
            if i + 1 < n && widgets::icon_button(ui, egui_phosphor::regular::ARROW_UP, "Move this layer up (over the next one)").clicked() {
                action = Some((i, 1));
            }
            if i > 0 && widgets::icon_button(ui, egui_phosphor::regular::ARROW_DOWN, "Move this layer down").clicked() {
                action = Some((i, -1));
            }
            if widgets::icon_button(ui, egui_phosphor::regular::COPY, "Duplicate this layer").clicked() {
                action = Some((i, 2));
            }
            if widgets::icon_button(ui, icons::TRASH, "Delete this layer").clicked() {
                action = Some((i, 0));
            }
        });
    }
    match action {
        Some((i, 1)) => layers.swap(i, i + 1),
        Some((i, -1)) => layers.swap(i, i - 1),
        Some((i, 2)) if n < MAX_LAYERS => {
            let c = layers[i].clone();
            layers.insert(i + 1, c);
        }
        Some((i, 0)) => {
            layers.remove(i);
        }
        _ => {}
    }
    ui.add_enabled_ui(n < MAX_LAYERS, |ui| {
        if ui.button(format!("{}  Add layer", icons::PLUS)).on_hover_text(format!("Add a texture layer on top (up to {MAX_LAYERS} per material)")).clicked() {
            layers.push(TextureLayer::default());
        }
    });
}

fn layer_editor(ui: &mut Ui, l: &mut TextureLayer, id: &str) {
    let d = TextureLayer::default();
    let before = l.target;
    widgets::segmented(
        ui,
        &mut l.target,
        &[
            (Target::Color, "Color", "Paints the surface color"),
            (Target::Bump, "Bump", "The brightness of the image is a height: bends the surface for the lighting"),
            (Target::Normal, "Normal", "A normal map (tangent space: red right, green up, blue out, like the normal maps of 3D models) bends the surface for the lighting"),
            (Target::Emission, "Glow", "Bright parts shine by themselves (adds to the gradient's emission; settings under Emissive colors)"),
            (Target::Specular, "Shine", "The brightness scales the highlights: white keeps them, black removes them"),
            (Target::Roughness, "Rough", "The brightness is the roughness: bright parts get dull highlights and softer light"),
        ],
    );
    // Normal maps are drawn at strength 1, bumps need more.
    if l.target != before {
        if l.target == Target::Normal {
            l.strength = 1.0;
        } else if before == Target::Normal {
            l.strength = d.strength;
        }
    }
    image_row(ui, &mut l.image, "Image", "The image of this layer (empty: a solid color)");
    if l.image.trim().is_empty() && l.target == Target::Color {
        ui.horizontal(|ui| {
            widgets::color_button(ui, &mut l.color).on_hover_text("The solid color of this layer");
            ui.label(RichText::new("Solid color").color(theme::TEXT_MUTED));
        });
    }
    image_row(ui, &mut l.mask, "Mask", "An image whose brightness is the opacity of this layer (white: shown, black: hidden)");
    widgets::grid(ui, &format!("layer-{id}"), |ui| {
        if !l.image.trim().is_empty() {
            widgets::checkbox(ui, "Image alpha", "Transparent parts of the image (PNG alpha channel) let the layers below show through", &mut l.use_alpha);
        }
        if !l.mask.trim().is_empty() {
            widgets::checkbox(ui, "Invert mask", "Black shows the layer, white hides it", &mut l.mask_invert);
            widgets::slider_log(ui, "Mask contrast", "Sharpens (above 1) or softens (below 1) the mask edges", &mut l.mask_contrast, 0.1..=20.0, d.mask_contrast);
        }
        widgets::label(ui, "Mapping", "How the image is put on the surface. Triplanar: three projections blended by the surface direction, no seams (best for fractals). Orbit: by the fractal's color value and orbit trap.");
        egui::ComboBox::from_id_salt(format!("layer-map-{id}")).selected_text(l.mapping.label()).show_ui(ui, |ui| {
            for m in Mapping::ALL {
                ui.selectable_value(&mut l.mapping, m, m.label());
            }
        });
        ui.end_row();
        if l.mapping != Mapping::Orbit {
            widgets::label(ui, "Space", "Object: the texture moves and turns with the object. World: the object moves through a fixed texture.");
            egui::ComboBox::from_id_salt(format!("layer-space-{id}")).selected_text(if l.space == Space::Object { "Object" } else { "World" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut l.space, Space::Object, "Object");
                ui.selectable_value(&mut l.space, Space::World, "World");
            });
            ui.end_row();
        }
        let tip = match l.mapping {
            Mapping::Spherical | Mapping::Cylindrical => "Repetitions around the center",
            Mapping::Orbit => "Repetitions over the color value and orbit trap",
            _ => "Image tiles per unit (larger: smaller texture)",
        };
        widgets::slider_log(ui, "Scale", tip, &mut l.scale, 0.01..=100.0, d.scale);
        for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
            widgets::slider(ui, &format!("Offset {axis}"), "Shifts the texture (in tiles)", &mut l.offset[k], -1.0..=1.0, 0.0);
        }
        for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
            widgets::slider(ui, &format!("Drift {axis}"), "Animations: the texture moves by this many tiles per second (flowing lava, moving clouds)", &mut l.drift[k], -5.0..=5.0, 0.0);
        }
        if l.mapping != Mapping::Orbit {
            for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
                widgets::slider(ui, &format!("Rotate {axis}"), "Turns the projection (degrees)", &mut l.rotation[k], -180.0..=180.0, 0.0);
            }
        }
        if l.target == Target::Color {
            widgets::label(ui, "Blend", "How the layer combines with the colors below it");
            egui::ComboBox::from_id_salt(format!("layer-blend-{id}")).selected_text(l.blend.label()).show_ui(ui, |ui| {
                for b in BlendMode::ALL {
                    ui.selectable_value(&mut l.blend, b, b.label());
                }
            });
            ui.end_row();
        }
        widgets::slider(ui, "Opacity", "How much of the layer shows (also scales bumps and glow)", &mut l.opacity, 0.0..=1.0, d.opacity);
        match l.target {
            Target::Bump => {
                widgets::slider_log(ui, "Strength", "Height of the relief (per brightness step of one texel)", &mut l.strength, 0.01..=100.0, d.strength);
            }
            Target::Emission => {
                widgets::slider_log(ui, "Strength", "Brightness of the glow where the image is white", &mut l.strength, 0.01..=100.0, d.strength);
            }
            Target::Normal => {
                widgets::slider_log(ui, "Strength", "How far the normals bend the surface (1: as drawn)", &mut l.strength, 0.01..=10.0, 1.0);
            }
            Target::Color | Target::Specular | Target::Roughness => {}
        }
    });
    if l.target.bends() && l.mapping == Mapping::Orbit {
        ui.label(RichText::new("Bumps and normal maps need a projection (not Orbit).").size(theme::fs(11.0)).color(theme::WARN));
    }
    if l.target == Target::Normal && l.image.trim().is_empty() {
        ui.label(RichText::new("A normal map needs an image.").size(theme::fs(11.0)).color(theme::WARN));
    }
}
