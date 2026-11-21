// SPDX-License-Identifier: LGPL-2.1-or-later

//! Right panel: camera, ray marching, light, color, effects and scene notes.

use egui::{RichText, Ui};
use rmb_core::scene::{AoMode, BackgroundMode, ColorMode, ColorStop, DiffuseCurve, FogMode, GlowShape, Light, LightKind, MAX_LIGHTS, Scene};
use rmb_core::{V3, v3};

use crate::viewport::{NavMode, Viewport};
use crate::{icons, theme, widgets};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Camera,
    Render,
    Light,
    Color,
    Effects,
    Notes,
}

#[derive(Default)]
pub struct Inspector {
    pub tab: Tab,
    light: usize,
}

impl Inspector {
    pub fn show(&mut self, ui: &mut Ui, scene: &mut Scene, view: &mut Viewport) {
        widgets::segmented(
            ui,
            &mut self.tab,
            &[
                (Tab::Camera, icons::CAMERA, "Camera and image size"),
                (Tab::Render, icons::CUBE, "Ray marching: detail, steps, distance"),
                (Tab::Light, icons::LIGHT, "Lights, ambient light and occlusion"),
                (Tab::Color, icons::PALETTE, "Surface colors"),
                (Tab::Effects, icons::FOG, "Background, fog, glow, depth of field, cutting, image"),
                (Tab::Notes, icons::INFO, "Notes and import messages"),
            ],
        );
        let title = match self.tab {
            Tab::Camera => "Camera",
            Tab::Render => "Ray marching",
            Tab::Light => "Lighting",
            Tab::Color => "Color",
            Tab::Effects => "Effects",
            Tab::Notes => "Notes",
        };
        ui.add_space(6.0);
        ui.label(RichText::new(title).size(theme::fs(16.0)).strong());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
            Tab::Camera => camera(ui, scene, view),
            Tab::Render => render(ui, scene),
            Tab::Light => self.lighting(ui, scene),
            Tab::Color => color(ui, scene),
            Tab::Effects => effects(ui, scene),
            Tab::Notes => notes(ui, scene),
        });
    }

    fn lighting(&mut self, ui: &mut Ui, scene: &mut Scene) {
        let d = rmb_core::scene::Lighting::default();
        let l = &mut scene.lighting;
        widgets::section(ui, "Lights", false);
        ui.horizontal_wrapped(|ui| {
            for i in 0..l.lights.len() {
                let on = l.lights[i].enabled;
                let txt = RichText::new(format!("{}", i + 1)).color(if on { theme::TEXT } else { theme::TEXT_MUTED });
                if ui.add(egui::Button::selectable(self.light == i, txt).min_size(egui::vec2(30.0, 24.0))).on_hover_text(format!("Light {}", i + 1)).clicked() {
                    self.light = i;
                }
            }
            if l.lights.len() < MAX_LIGHTS && widgets::icon_button(ui, icons::PLUS, "Add a light").clicked() {
                l.lights.push(Light { azimuth: 60.0, elevation: 30.0, intensity: 0.6, ..Light::default() });
                self.light = l.lights.len() - 1;
            }
            if !l.lights.is_empty() && widgets::icon_button(ui, icons::TRASH, "Remove the selected light").clicked() {
                l.lights.remove(self.light.min(l.lights.len() - 1));
                self.light = self.light.saturating_sub(1);
            }
        });
        if let Some(li) = l.lights.get_mut(self.light) {
            let dl = Light::default();
            widgets::grid(ui, "light", |ui| {
                widgets::checkbox(ui, "On", "Switch the light on or off", &mut li.enabled);
                widgets::label(ui, "Type", "Directional: parallel light from a direction. Point: light from a position, falls off with the squared distance.");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut li.kind, LightKind::Directional, "Directional").on_hover_text("Parallel light from a direction");
                    ui.selectable_value(&mut li.kind, LightKind::Point, "Point").on_hover_text("Light from a position in the scene");
                });
                ui.end_row();
                if li.kind == LightKind::Point {
                    let speed = (li.position.length() * 0.002).max(1e-9);
                    widgets::drag(ui, "X", "Light position", &mut li.position.x, speed, dl.position.x);
                    widgets::drag(ui, "Y", "Light position", &mut li.position.y, speed, dl.position.y);
                    widgets::drag(ui, "Z", "Light position", &mut li.position.z, speed, dl.position.z);
                } else {
                    widgets::checkbox(ui, "Follows camera", "The direction is relative to the view (light moves with the camera); off: fixed in the world", &mut li.camera_relative);
                    widgets::slider(ui, "Azimuth", "Horizontal direction in degrees (0 = from the camera / +x)", &mut li.azimuth, -180.0..=180.0, dl.azimuth);
                    widgets::slider(ui, "Elevation", "Height above the horizon in degrees", &mut li.elevation, -90.0..=90.0, dl.elevation);
                }
                widgets::color(ui, "Color", "Light color", &mut li.color);
                widgets::slider_log(ui, "Intensity", "Brightness of the light (point lights: at distance 1)", &mut li.intensity, 0.0..=1000.0, dl.intensity);
                widgets::slider(ui, "Specular", "Strength of the highlights of this light", &mut li.specular, 0.0..=2.0, dl.specular);
                widgets::label(ui, "Diffuse", "How the light falls off with the angle to the surface. Wrap curves also light the back sides.");
                egui::ComboBox::from_id_salt("diffuse-curve").selected_text(li.diffuse.label()).show_ui(ui, |ui| {
                    for c in DiffuseCurve::ALL {
                        ui.selectable_value(&mut li.diffuse, c, c.label());
                    }
                });
                ui.end_row();
                widgets::slider(ui, "Occlusion", "How much the ambient occlusion darkens this light (0..1)", &mut li.occlusion, 0.0..=1.0, dl.occlusion);
                widgets::checkbox(ui, "Visible", "The light source itself shows in the image as a glow", &mut li.visible);
                if li.visible {
                    widgets::label(ui, "Glow shape", "Soft: bright core with a soft falloff. Sharp: small spot. Ball: glowing ball that covers what is behind. Halo: transparent halo.");
                    egui::ComboBox::from_id_salt("glow-shape").selected_text(li.glow_shape.label()).show_ui(ui, |ui| {
                        for g in GlowShape::ALL {
                            ui.selectable_value(&mut li.glow_shape, g, g.label());
                        }
                    });
                    ui.end_row();
                    let size_tip = if li.kind == LightKind::Point { "Glow radius in world units" } else { "Glow radius in degrees (shows on the background only)" };
                    widgets::slider_log(ui, "Glow size", size_tip, &mut li.glow_size, 0.0001..=90.0, dl.glow_size);
                    widgets::slider_log(ui, "Glow intensity", "Brightness of the glow (smaller glows are brighter, like in Mandelbulb3D)", &mut li.glow_intensity, 0.0..=100.0, dl.glow_intensity);
                }
                widgets::checkbox(ui, "Shadows", "Cast shadows (costs a shadow ray per sample)", &mut li.shadows);
                if li.shadows {
                    widgets::slider(ui, "Softness", "Width of the shadow edges (0 = hard)", &mut li.softness, 0.0..=0.5, dl.softness);
                }
            });
        }
        if widgets::section(ui, "Ambient", true) {
            l.ambient = d.ambient;
            l.sky_color = d.sky_color;
            l.ground_color = d.ground_color;
            l.shininess = d.shininess;
            l.ambient_camera_relative = d.ambient_camera_relative;
        }
        widgets::grid(ui, "ambient", |ui| {
            widgets::slider(ui, "Amount", "Light from the surroundings", &mut l.ambient, 0.0..=2.0, d.ambient);
            widgets::color(ui, "Sky", "Ambient color from above (+z, or the top of the screen)", &mut l.sky_color);
            widgets::color(ui, "Ground", "Ambient color from below", &mut l.ground_color);
            widgets::checkbox(ui, "Follow camera", "Sky and ground follow the camera's up axis instead of the world z axis (Mandelbulb3D)", &mut l.ambient_camera_relative);
            widgets::slider_log(ui, "Shininess", "Size of the highlights: higher is smaller and sharper", &mut l.shininess, 2.0..=1024.0, d.shininess);
        });
        if widgets::section(ui, "Ambient occlusion", true) {
            l.ao_mode = d.ao_mode;
            l.ao_strength = d.ao_strength;
            l.ao_radius = d.ao_radius;
        }
        widgets::segmented(
            ui,
            &mut l.ao_mode,
            &[
                (AoMode::Off, "Off", "No ambient occlusion"),
                (AoMode::Fast, "Fast", "Five distance samples along the normal: no noise, cheap"),
                (AoMode::Raytraced, "Ray traced", "Random rays into the hemisphere: realistic, refines with the samples"),
            ],
        );
        if l.ao_mode != AoMode::Off {
            widgets::grid(ui, "ao", |ui| {
                widgets::slider(ui, "Strength", "How dark occluded areas get; above 1 the darkening gets steeper (Mandelbulb3D)", &mut l.ao_strength, 0.0..=4.0, d.ao_strength);
                widgets::slider_log(ui, "Radius", "Reach of the occlusion, relative to the view distance", &mut l.ao_radius, 0.002..=2.0, d.ao_radius);
            });
        }
    }
}

fn camera(ui: &mut Ui, scene: &mut Scene, view: &mut Viewport) {
    widgets::section(ui, "Navigation", false);
    widgets::segmented(
        ui,
        &mut view.mode,
        &[
            (NavMode::Orbit, &format!("{}  Orbit", icons::ORBIT), "Drag: orbit around the point under the cursor · right drag: pan · wheel: zoom to the cursor · double-click: look at a point"),
            (NavMode::Fly, &format!("{}  Fly", icons::FLY), "WASD/RF: move, QE: roll, drag: look, wheel: speed. The speed adapts to the distance to the surface."),
        ],
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Reset view").on_hover_text("Camera back to the default position looking at the origin").clicked() {
            scene.camera = Default::default();
        }
        if ui.button("Look at origin").on_hover_text("Turn the camera toward (0, 0, 0)").clicked() {
            let up = scene.camera.up;
            scene.camera.look_at(V3::ZERO, up);
        }
        if ui.button("Level").on_hover_text("Remove the roll: make +z point up on screen").clicked() {
            let target = scene.camera.position + scene.camera.forward;
            scene.camera.look_at(target, v3(0.0, 0.0, 1.0));
        }
    });
    let d = rmb_core::scene::Camera::default();
    let c = &mut scene.camera;
    widgets::section(ui, "Position", false);
    widgets::grid(ui, "cam", |ui| {
        let dist = c.position.length().max(1e-12);
        let speed = (dist * 0.002).max(1e-12);
        widgets::drag(ui, "X", "Camera position", &mut c.position.x, speed, d.position.x);
        widgets::drag(ui, "Y", "Camera position", &mut c.position.y, speed, d.position.y);
        widgets::drag(ui, "Z", "Camera position", &mut c.position.z, speed, d.position.z);
        widgets::slider(ui, "Field of view", "Vertical opening angle in degrees", &mut c.fov, 5.0..=120.0, d.fov);
    });
    let f = c.forward;
    ui.label(RichText::new(format!("Looking along ({:.3}, {:.3}, {:.3})", f.x, f.y, f.z)).size(theme::fs(11.0)).color(theme::TEXT_MUTED));

    widgets::section(ui, "Image size", false);
    ui.label(RichText::new("Size and aspect of saved images; the preview keeps the aspect.").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    widgets::grid(ui, "size", |ui| {
        widgets::label(ui, "Width", "");
        ui.add(egui::DragValue::new(&mut scene.width).range(16..=16384).suffix(" px"));
        ui.end_row();
        widgets::label(ui, "Height", "");
        ui.add(egui::DragValue::new(&mut scene.height).range(16..=16384).suffix(" px"));
        ui.end_row();
    });
    ui.horizontal_wrapped(|ui| {
        for (w, h, t) in [(1280, 720, "720p"), (1920, 1080, "1080p"), (3840, 2160, "4K"), (1080, 1080, "1:1"), (1080, 1350, "4:5"), (1080, 1920, "9:16")] {
            if ui.small_button(t).on_hover_text(format!("{w} × {h}")).clicked() {
                scene.width = w;
                scene.height = h;
            }
        }
    });
}

fn render(ui: &mut Ui, scene: &mut Scene) {
    let d = rmb_core::scene::March::default();
    let m = &mut scene.march;
    if widgets::section(ui, "Ray marching", true) {
        *m = d.clone();
    }
    widgets::grid(ui, "march", |ui| {
        widgets::slider_log(ui, "Detail", "Surface threshold in pixels (Mandelbulb3D's DEstop). Smaller: finer detail, slower.", &mut m.detail, 0.05..=8.0, d.detail);
        widgets::slider(ui, "Step", "Ray step multiplier. Lower it against holes and noise (overstepping).", &mut m.step, 0.05..=1.0, d.step);
        widgets::slider_u32(ui, "Max steps", "Steps per ray before giving up", &mut m.max_steps, 50..=3000, d.max_steps);
        widgets::slider_log(ui, "Max distance", "Rays end after this distance", &mut m.max_distance, 0.01..=1000.0, d.max_distance);
        widgets::slider_u32(ui, "Refinement", "Binary search steps at the surface for an exact hit", &mut m.refine_steps, 0..=12, d.refine_steps);
        widgets::slider_log(ui, "Near", "Rays start at this distance from the camera: hides what is closer (Mandelbulb3D imports set it)", &mut m.near, 0.0..=10.0, d.near);
    });
}

fn color(ui: &mut Ui, scene: &mut Scene) {
    let d = rmb_core::scene::Material::default();
    let m = &mut scene.material;
    widgets::section(ui, "Gradient", false);
    widgets::gradient_strip(ui, &m.gradient, 22.0);
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for name in rmb_core::gradient::preset_names() {
            if ui.small_button(name).clicked()
                && let Some(g) = rmb_core::gradient::preset(name)
            {
                m.gradient = g;
            }
        }
    });
    ui.add_space(4.0);
    let mut remove = None;
    let n = m.gradient.len();
    for (i, s) in m.gradient.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            widgets::color_button(ui, &mut s.color).on_hover_text("Surface color");
            widgets::color_button(ui, &mut s.specular).on_hover_text("Highlight color: multiplies the specular light here");
            ui.add(egui::Slider::new(&mut s.pos, 0.0..=1.0).max_decimals(3)).on_hover_text("Position of this color on the gradient");
            if n > 2 && widgets::icon_button(ui, icons::X, "Remove this color").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        m.gradient.remove(i);
    }
    if ui.button(format!("{}  Add color", icons::PLUS)).clicked() {
        let c = rmb_core::gradient::sample(&m.gradient, 0.5);
        m.gradient.push(ColorStop::new(0.5, c));
    }

    if widgets::section(ui, "Mapping", true) {
        let g = std::mem::take(&mut m.gradient);
        *m = rmb_core::scene::Material { gradient: g, ..d.clone() };
    }
    widgets::grid(ui, "mapping", |ui| {
        widgets::label(ui, "Color by", "Which value of the orbit picks the gradient color");
        egui::ComboBox::from_id_salt("color-mode").selected_text(m.color_mode.label()).show_ui(ui, |ui| {
            for c in ColorMode::ALL {
                ui.selectable_value(&mut m.color_mode, c, c.label());
            }
        });
        ui.end_row();
        widgets::slider_log(ui, "Scale", "How fast the colors change with the value (negative reverses the gradient)", &mut m.color_scale, -10.0..=10.0, d.color_scale);
        widgets::slider(ui, "Offset", "Shifts the gradient", &mut m.color_offset, -5.0..=5.0, d.color_offset);
        widgets::slider(ui, "Detail shift", "Adds this times the log of the surface threshold to iteration values: keeps colors steady while zooming (Mandelbulb3D)", &mut m.detail_shift, -2.0..=2.0, d.detail_shift);
        widgets::checkbox(ui, "Repeat", "Repeat the gradient (else clamp at its ends)", &mut m.repeat);
        widgets::slider(ui, "Color amount", "0: everything in the first gradient color", &mut m.saturation, 0.0..=1.0, d.saturation);
        widgets::slider(ui, "Specular", "Shininess of the material (all lights)", &mut m.specular, 0.0..=3.0, d.specular);
        widgets::slider(ui, "Reflection", "Mirror reflections: this amount times the highlight color of the gradient (0 = off; costs another ray per bounce)", &mut m.reflection, 0.0..=2.0, d.reflection);
        if m.reflection > 0.0 {
            widgets::slider_u32(ui, "Bounces", "Reflections of reflections", &mut m.reflection_bounces, 1..=8, d.reflection_bounces);
        }
    });
}

fn effects(ui: &mut Ui, scene: &mut Scene) {
    let db = rmb_core::scene::Background::default();
    let b = &mut scene.background;
    if widgets::section(ui, "Background", true) {
        *b = db.clone();
    }
    widgets::segmented(
        ui,
        &mut b.mode,
        &[
            (BackgroundMode::Solid, "Solid", "One color"),
            (BackgroundMode::Gradient, "Gradient", "Sky gradient: top, horizon, bottom"),
            (BackgroundMode::Vertical, "Vertical", "Top to bottom over the screen height (Mandelbulb3D)"),
        ],
    );
    widgets::grid(ui, "bg", |ui| {
        widgets::color(ui, if b.mode == BackgroundMode::Solid { "Color" } else { "Top" }, "", &mut b.top);
        if b.mode == BackgroundMode::Gradient {
            widgets::color(ui, "Horizon", "", &mut b.horizon);
        }
        if b.mode != BackgroundMode::Solid {
            widgets::color(ui, "Bottom", "", &mut b.bottom);
        }
        if b.mode == BackgroundMode::Vertical {
            widgets::slider_log(ui, "Curve", "1: even; above 1: more of the top color; below 1: more of the bottom color", &mut b.curve, 0.1..=10.0, db.curve);
        }
    });
    ui.horizontal(|ui| {
        let label = if b.image.is_empty() { "No image".to_string() } else { b.image.clone() };
        if ui.button("Image…").on_hover_text("Panorama image (latitude/longitude) behind the object; the fog keeps the colors above").clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("Image", &["jpg", "jpeg", "png", "bmp"]).pick_file()
        {
            b.image = p.display().to_string();
        }
        if !b.image.is_empty() && widgets::icon_button(ui, icons::X, "Remove the background image").clicked() {
            b.image.clear();
        }
        let found = b.image.is_empty() || rmb_core::maps::get_file(b.image.trim()).is_some();
        ui.label(RichText::new(label).size(theme::fs(11.0)).color(if found { theme::TEXT_MUTED } else { theme::WARN }))
            .on_hover_text(if found { "" } else { "Not found: give the full path or put it in the maps folder (Preferences)" });
    });
    if !b.image.is_empty() {
        widgets::grid(ui, "bg-image", |ui| {
            for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
                widgets::slider(ui, &format!("Rotate {axis}"), "Rotation of the background image in degrees", &mut b.image_rotation[k], -180.0..=360.0, db.image_rotation[k]);
            }
        });
    }

    let df = rmb_core::scene::Fog::default();
    let f = &mut scene.fog;
    if widgets::section(ui, "Fog and glow", true) {
        *f = df.clone();
    }
    widgets::segmented(
        ui,
        &mut f.mode,
        &[
            (FogMode::Exponential, "Exponential", "Fog grows exponentially with the distance"),
            (FogMode::Range, "Range", "Fog grows over the view range, from the near to the max distance (Mandelbulb3D's depth fog)"),
        ],
    );
    widgets::grid(ui, "fog", |ui| {
        match f.mode {
            FogMode::Exponential => {
                widgets::slider_log(ui, "Fog density", "Depth fog per unit of distance (0 = off)", &mut f.density, 0.0..=10.0, df.density);
            }
            FogMode::Range => {
                widgets::slider(ui, "Fog strength", "Fog amount at the far end (0 = off)", &mut f.strength, 0.0..=5.0, df.strength);
                widgets::slider(ui, "Fog start", "Where in the range the fog begins (0..1); before it the image gets brighter", &mut f.start, -1.0..=1.0, df.start);
                widgets::slider_log(ui, "Fog curve", "0: even over the range; higher values put more fog close to the camera", &mut f.curve, 0.0..=100_000.0, df.curve);
                widgets::checkbox(ui, "Far fog", "Square the fog amount: less fog close by (Mandelbulb3D's far fog)", &mut f.squared);
            }
        }
        widgets::checkbox(ui, "Background color", "The fog takes the background color behind each pixel", &mut f.from_background);
        if !f.from_background {
            widgets::color(ui, "Fog color", "", &mut f.color);
        }
        widgets::slider(ui, "Glow", "Light along rays that pass close to the surface (Mandelbulb3D's dynamic fog); negative darkens", &mut f.glow, -4.0..=4.0, df.glow);
        widgets::color(ui, "Glow color", "", &mut f.glow_color);
        widgets::slider_log(ui, "Glow offset", "Ray steps that give no glow yet", &mut f.glow_offset, 0.0..=500.0, df.glow_offset);
        widgets::slider_log(ui, "Second color after", "Ray steps until the glow has the second color (0 = only the first color)", &mut f.glow_mix_steps, 0.0..=2000.0, df.glow_mix_steps);
        widgets::color(ui, "Second glow color", "", &mut f.glow_color2);
        widgets::checkbox(ui, "Cover", "The glow covers the image instead of adding light", &mut f.glow_blend);
        widgets::slider_u32(ui, "Glow iteration", "Only ray steps where the fractal escaped after this many iterations glow (0 = all steps)", &mut f.glow_iteration, 0..=100, df.glow_iteration);
    });

    let dd = rmb_core::scene::Dof::default();
    let dof = &mut scene.dof;
    if widgets::section(ui, "Depth of field", true) {
        *dof = dd.clone();
    }
    widgets::grid(ui, "dof", |ui| {
        widgets::checkbox(ui, "On", "Blur everything outside the focus distance. Double-click into the image to focus.", &mut dof.enabled);
        widgets::slider_log(ui, "Aperture", "Lens size: more blur", &mut dof.aperture, 0.0005..=1.0, dd.aperture);
        widgets::slider_log(ui, "Focus distance", "Distance from the camera that is sharp (double-click into the image to pick)", &mut dof.focus_distance, 0.0001..=100.0, dd.focus_distance);
    });

    if widgets::section(ui, "Cutting planes", true) {
        scene.cut = Default::default();
    }
    widgets::grid(ui, "cut", |ui| {
        for (name, p) in [("X", &mut scene.cut.x), ("Y", &mut scene.cut.y), ("Z", &mut scene.cut.z)] {
            widgets::label(ui, &format!("Cut {name}"), "Removes everything on one side of the plane");
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.enabled, "");
                ui.add_enabled(p.enabled, egui::DragValue::new(&mut p.position).speed(0.005).max_decimals(6));
                ui.add_enabled(p.enabled, egui::Checkbox::new(&mut p.keep_above, "keep +")).on_hover_text("Keep the part above the position (else below)");
            });
            ui.end_row();
        }
    });

    let dp = rmb_core::scene::Post::default();
    let p = &mut scene.post;
    if widgets::section(ui, "Image", true) {
        *p = dp.clone();
    }
    ui.label(RichText::new("Applied to the finished image: changes show at once without re-rendering.").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    widgets::grid(ui, "post", |ui| {
        widgets::slider(ui, "Exposure", "Brightness in stops", &mut p.exposure, -4.0..=4.0, dp.exposure);
        widgets::checkbox(ui, "Filmic", "Filmic tone mapping: soft highlights instead of clipping", &mut p.tone_map);
        widgets::slider(ui, "Gamma", "", &mut p.gamma, 1.0..=3.0, dp.gamma);
        widgets::slider(ui, "Contrast", "", &mut p.contrast, 0.5..=2.0, dp.contrast);
        widgets::slider(ui, "Saturation", "", &mut p.saturation, 0.0..=2.0, dp.saturation);
        widgets::slider(ui, "Vignette", "Darkens the corners", &mut p.vignette, 0.0..=1.0, dp.vignette);
        let note = match rmb_core::render::denoise::available() {
            Ok(p) => format!("Intel Open Image Denoise on saved images (and the \"Denoised\" preview). Library: {}", p.display()),
            Err(e) => format!("Intel Open Image Denoise on saved images. Not available: {e}. Run python3 tools/fetch_oidn.py or set RMB_OIDN_DIR."),
        };
        widgets::checkbox(ui, "Denoise", &note, &mut p.denoise);
        if p.denoise {
            widgets::slider(ui, "Denoise amount", "1: only the denoised image; less mixes the original back in (keeps grain and fine detail)", &mut p.denoise_blend, 0.0..=1.0, dp.denoise_blend);
        }
        widgets::checkbox(
            ui,
            "Display space",
            "Light the colors as shown instead of in linear light, like Mandelbulb3D (stronger colors, harder light). Re-renders. Gamma 2.2 then leaves the image unchanged.",
            &mut p.display_space,
        );
    });
}

fn notes(ui: &mut Ui, scene: &mut Scene) {
    ui.label(RichText::new("Free text saved with the scene; imports list what could not be converted.").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    ui.add(egui::TextEdit::multiline(&mut scene.notes).desired_width(f32::INFINITY).desired_rows(16));
}
