// SPDX-License-Identifier: LGPL-2.1-or-later

//! Right panel: camera, ray marching, light, color, effects and scene notes.

use egui::{RichText, Ui};
use rmb_core::scene::{AoMode, BackgroundMode, ColorMode, ColorStop, DiffuseCurve, FogMode, GlowShape, HazeShape, Light, LightKind, MAX_LIGHTS, Scene};
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
        // The light picked in the view (selection shared with the view's gizmos).
        if view.light_clicked {
            view.light_clicked = false;
            self.tab = Tab::Light;
        }
        self.light = view.selected_light;
        self.show_tabs(ui, scene, view);
        view.selected_light = self.light;
    }

    fn show_tabs(&mut self, ui: &mut Ui, scene: &mut Scene, view: &mut Viewport) {
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
        // Placing helpers need the whole scene; they run after the controls.
        let mut place: Option<Place> = None;
        let sky_drives = scene.background.mode == BackgroundMode::Sky && scene.sky.sun_light;
        let l = &mut scene.lighting;
        if widgets::section(ui, "Lights", false).open {
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
            if self.light == 0 && sky_drives {
                ui.label(RichText::new("Light 1 follows the sky's sun (Effects › Background); its direction, color and intensity are set there").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            }
            if let Some(li) = l.lights.get_mut(self.light) {
                let dl = Light::default();
                widgets::grid(ui, "light", |ui| {
                    widgets::checkbox(ui, "On", "Switch the light on or off", &mut li.enabled);
                    widgets::label(
                        ui,
                        "Type",
                        "Directional: parallel light from a direction. Point: light from a position, falls off with the squared distance. Spot: a point light shining into a cone. Area: a glowing rectangle (soft shadows). Line: a glowing tube (soft shadows).",
                    );
                    egui::ComboBox::from_id_salt("light-kind").selected_text(li.kind.label()).show_ui(ui, |ui| {
                        for k in LightKind::ALL {
                            ui.selectable_value(&mut li.kind, k, k.label());
                        }
                    });
                    ui.end_row();
                    if li.kind.positional() {
                        let speed = (li.position.length() * 0.002).max(1e-9);
                        let what = if li.kind == LightKind::Line { "Start of the line light" } else { "Light position" };
                        let tip = |axis: &str| format!("{what} along the world {axis} axis (the arrows on the light in the view)");
                        widgets::drag(ui, "X", &tip("x (red)"), &mut li.position.x, speed, dl.position.x);
                        widgets::drag(ui, "Y", &tip("y (green)"), &mut li.position.y, speed, dl.position.y);
                        widgets::drag(ui, "Z", &tip("z (blue, up)"), &mut li.position.z, speed, dl.position.z);
                        widgets::label(ui, "", "");
                        ui.horizontal(|ui| {
                            if ui.small_button("At camera").on_hover_text("Moves the light to the camera (spot and area lights then shine along the view)").clicked() {
                                place = Some(Place::AtCamera);
                            }
                            let (label, tip) = if li.kind == LightKind::Line {
                                ("Center on view", "Moves the line (same length and direction) over the surface point in the middle of the view")
                            } else {
                                ("Aim at view center", "Aims the light at the surface point in the middle of the view")
                            };
                            if li.kind != LightKind::Point && ui.small_button(label).on_hover_text(tip).clicked() {
                                place = Some(Place::AimCenter);
                            }
                        });
                        ui.end_row();
                        if li.kind != LightKind::Point {
                            let (name, what) = if li.kind == LightKind::Line { ("End", "Other end of the line light") } else { ("Aim", "The point the light faces") };
                            let speed = ((li.target - li.position).length().max(li.target.length()) * 0.002).max(1e-6);
                            widgets::drag(ui, &format!("{name} X"), what, &mut li.target.x, speed, dl.target.x);
                            widgets::drag(ui, &format!("{name} Y"), what, &mut li.target.y, speed, dl.target.y);
                            widgets::drag(ui, &format!("{name} Z"), what, &mut li.target.z, speed, dl.target.z);
                        }
                        match li.kind {
                            LightKind::Spot => {
                                widgets::slider(ui, "Cone angle", "Half angle of the light cone in degrees", &mut li.cone_angle, 0.5..=90.0, dl.cone_angle);
                                widgets::slider(ui, "Cone edge", "How soft the edge of the cone is (0 = sharp, 1 = fades from the center)", &mut li.cone_softness, 0.0..=1.0, dl.cone_softness);
                            }
                            LightKind::Area => {
                                widgets::slider_log(ui, "Width", "Width of the glowing rectangle in world units (larger: softer shadows)", &mut li.width, 0.0..=100.0, dl.width);
                                widgets::slider_log(ui, "Height", "Height of the glowing rectangle in world units", &mut li.height, 0.0..=100.0, dl.height);
                            }
                            _ => {}
                        }
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
                    if li.visible && li.kind == LightKind::Area {
                        widgets::slider_log(ui, "Glow intensity", "Brightness of the glowing rectangle", &mut li.glow_intensity, 0.0..=100.0, dl.glow_intensity);
                    } else if li.visible {
                        widgets::label(ui, "Glow shape", "Soft: bright core with a soft falloff. Sharp: small spot. Ball: glowing ball that covers what is behind. Halo: transparent halo.");
                        egui::ComboBox::from_id_salt("glow-shape").selected_text(li.glow_shape.label()).show_ui(ui, |ui| {
                            for g in GlowShape::ALL {
                                ui.selectable_value(&mut li.glow_shape, g, g.label());
                            }
                        });
                        ui.end_row();
                        let size_tip = match li.kind {
                            LightKind::Directional => "Glow radius in degrees (shows on the background only)",
                            LightKind::Area => "Not used: area lights show as their rectangle",
                            LightKind::Line => "Radius of the glowing tube in world units",
                            _ => "Glow radius in world units",
                        };
                        widgets::slider_log(ui, "Glow size", size_tip, &mut li.glow_size, 0.0001..=90.0, dl.glow_size);
                        widgets::slider_log(ui, "Glow intensity", "Brightness of the glow (smaller glows are brighter, like in Mandelbulb3D)", &mut li.glow_intensity, 0.0..=100.0, dl.glow_intensity);
                    }
                    widgets::checkbox(ui, "Shadows", "Cast shadows (costs a shadow ray per sample)", &mut li.shadows);
                    if li.shadows && !matches!(li.kind, LightKind::Area | LightKind::Line) {
                        widgets::slider(ui, "Softness", "Width of the shadow edges (0 = hard; area and line lights get theirs from their size)", &mut li.softness, 0.0..=0.5, dl.softness);
                    }
                });
                lens_flare(ui, &mut li.flare);
            }
        }
        let sec = widgets::section(ui, "Ambient", true);
        if sec.reset {
            l.ambient = d.ambient;
            l.sky_color = d.sky_color;
            l.ground_color = d.ground_color;
            l.shininess = d.shininess;
            l.roughness = d.roughness;
            l.ambient_camera_relative = d.ambient_camera_relative;
            l.ambient_from_background = d.ambient_from_background;
        }
        if sec.open {
            if sky_drives {
                ui.label(RichText::new("Amount, sky and ground colors follow the sky (Effects › Background › Skylight)").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            }
            widgets::grid(ui, "ambient", |ui| {
                widgets::slider(ui, "Amount", "Light from the surroundings", &mut l.ambient, 0.0..=2.0, d.ambient);
                if l.ambient_from_background {
                    widgets::label(ui, "Sky and ground", "Not used: the ambient light comes from the background image (Background › Ambient light)");
                    ui.label(RichText::new("from the background image").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
                    ui.end_row();
                }
                widgets::color(ui, "Sky", "Ambient color from above (+z, or the top of the screen)", &mut l.sky_color);
                widgets::color(ui, "Ground", "Ambient color from below", &mut l.ground_color);
                widgets::checkbox(ui, "Follow camera", "Sky and ground follow the camera's up axis instead of the world z axis (Mandelbulb3D)", &mut l.ambient_camera_relative);
                widgets::slider_log(ui, "Shininess", "Size of the highlights: higher is smaller and sharper", &mut l.shininess, 2.0..=1024.0, d.shininess);
                widgets::slider(ui, "Roughness", "How much rough surface parts lose their highlights and get softer light (needs Smooth normals in the Ray marching tab)", &mut l.roughness, 0.0..=1.0, d.roughness);
            });
        }
        let sec = widgets::section(ui, "Global illumination", true);
        if sec.reset {
            l.render_mode = d.render_mode;
            l.path_bounces = d.path_bounces;
            l.path_clamp = d.path_clamp;
            l.path_reach = d.path_reach;
        }
        if sec.open {
            widgets::segmented(
                ui,
                &mut l.render_mode,
                &[
                    (rmb_core::scene::RenderMode::Classic, "Classic", "Lights with shadows, ambient light with occlusion, emission light rays (Mandelbulb3D style; fast)"),
                    (
                        rmb_core::scene::RenderMode::PathTraced,
                        "Path tracing",
                        "Monte Carlo path tracing: light bounces between the surfaces (diffuse and reflections), glowing colors light everything around them, the ambient colors become the light of the sky. More realistic, needs more samples (use the denoiser).",
                    ),
                ],
            );
            if l.render_mode == rmb_core::scene::RenderMode::PathTraced {
                widgets::grid(ui, "path", |ui| {
                    widgets::slider_u32(ui, "Bounces", "How often light bounces at most: more give brighter, softer indirect light and cost more per sample", &mut l.path_bounces, 1..=16, d.path_bounces);
                    widgets::slider_log(ui, "Firefly clamp", "Limits the light a single bounce adds: removes lone bright pixels at the cost of a little light (0 = off)", &mut l.path_clamp, 0.0..=100.0, d.path_clamp);
                    widgets::slider(ui, "Sky reach", "Diffuse bounces that travel farther than this (relative to the surface's distance from the camera) count as open sky, like Mandelbulb3D's short ambient rays: brighter crevices and halls, less realistic. 0 = no limit (physically correct)", &mut l.path_reach, 0.0..=0.5, d.path_reach);
                });
                ui.label(
                    RichText::new("Ambient occlusion and the emission light rays are not used: the bounces bring that light. While moving the view the preview is shaded classically.")
                        .size(theme::fs(11.0))
                        .color(theme::TEXT_MUTED),
                );
            }
        }
        let sec = widgets::section(ui, "Ambient occlusion", true);
        if sec.reset {
            l.ao_mode = d.ao_mode;
            l.ao_strength = d.ao_strength;
            l.ao_radius = d.ao_radius;
        }
        if sec.open {
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
            if let Some(p) = place {
                let cam = scene.camera.clone();
                // The surface point in the middle of the view (else a point ahead).
                let center = || rmb_core::render::cpu::pick_center(scene).map(|(_, h)| h).unwrap_or(cam.position + cam.forward * rmb_core::render::cpu::distance_at_camera(scene).max(0.1) * 2.0);
                let aim = center();
                if let Some(li) = scene.lighting.lights.get_mut(self.light) {
                    match p {
                        Place::AtCamera => {
                            li.position = cam.position;
                            if li.kind == LightKind::Line {
                                let half = (aim - cam.position).length() * 0.2;
                                li.position = cam.position - cam.right * half;
                                li.target = cam.position + cam.right * half;
                            } else {
                                li.target = aim;
                            }
                        }
                        Place::AimCenter => {
                            if li.kind == LightKind::Line {
                                // Keep the length, center the line over the point.
                                let half = (li.target - li.position) * 0.5;
                                li.position = aim - half;
                                li.target = aim + half;
                            } else {
                                li.target = aim;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Lens flare settings of a light.
fn lens_flare(ui: &mut Ui, f: &mut rmb_core::flare::LensFlare) {
    let d = rmb_core::flare::LensFlare::default();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("LENS FLARE").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
        ui.checkbox(&mut f.enabled, "").on_hover_text("Reflections and diffraction of the camera lens around this light (shown when the light is in or near the view and not hidden)");
        egui::ComboBox::from_id_salt("flare-preset").selected_text("Preset…").show_ui(ui, |ui| {
            for (name, p) in rmb_core::flare::LensFlare::presets() {
                if ui.selectable_label(false, name).on_hover_text("Replaces the flare settings with this look").clicked() {
                    *f = p;
                }
            }
        });
    });
    if !f.enabled {
        return;
    }
    widgets::grid(ui, "flare", |ui| {
        widgets::slider_log(ui, "Intensity", "Brightness of the whole flare", &mut f.intensity, 0.0..=20.0, d.intensity);
        widgets::color(ui, "Tint", "Multiplies the light's color", &mut f.tint);
        widgets::label(ui, "Variation", "Seed of the random parts (spikes, ghosts, sparkles, dirt)");
        ui.horizontal(|ui| {
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut f.seed)), &mut f.seed, d.seed);
            if ui.small_button("Shuffle").on_hover_text("Another random arrangement").clicked() {
                f.seed = rmb_core::math::pcg(f.seed.wrapping_add(1)) % 100000;
            }
        });
        ui.end_row();
    });
    let group = |ui: &mut Ui, title: &str, tip: &str, body: &mut dyn FnMut(&mut Ui)| {
        egui::CollapsingHeader::new(title).id_salt(("flare", title)).show(ui, |ui| body(ui)).header_response.on_hover_text(tip);
    };
    group(ui, "Glow", "The bright core and the wide haze around the light", &mut |ui| {
        widgets::grid(ui, "fl-glow", |ui| {
            widgets::slider(ui, "Core", "Brightness of the core", &mut f.glow, 0.0..=5.0, d.glow);
            widgets::slider_log(ui, "Core size", "Size of the core (share of the image height)", &mut f.glow_size, 0.001..=0.5, d.glow_size);
            widgets::slider(ui, "Haze", "Wide soft glow that lifts the dark parts", &mut f.haze, 0.0..=1.0, d.haze);
            widgets::slider_log(ui, "Haze size", "Reach of the haze (share of the image height)", &mut f.haze_size, 0.02..=3.0, d.haze_size);
        });
    });
    group(ui, "Star and spikes", "Diffraction star of the aperture and fine random spikes", &mut |ui| {
        widgets::grid(ui, "fl-rays", |ui| {
            widgets::slider_u32(ui, "Star rays", "Number of star rays (0 = none; even counts look like real apertures)", &mut f.rays, 0..=32, d.rays);
            widgets::slider(ui, "Ray brightness", "Brightness of the star rays", &mut f.ray_intensity, 0.0..=3.0, d.ray_intensity);
            widgets::slider_log(ui, "Ray length", "Length of the rays (share of the image height)", &mut f.ray_length, 0.01..=3.0, d.ray_length);
            widgets::slider_log(ui, "Ray width", "Thickness of the rays", &mut f.ray_width, 0.1..=10.0, d.ray_width);
            widgets::slider(ui, "Rotation", "Rotation of the star in degrees", &mut f.ray_rotation, -180.0..=180.0, d.ray_rotation);
            widgets::slider(ui, "Uneven", "Varies the length, brightness and angle of the rays", &mut f.ray_random, 0.0..=1.0, d.ray_random);
            widgets::slider_u32(ui, "Spikes", "Number of fine random spikes", &mut f.spikes, 0..=400, d.spikes);
            widgets::slider(ui, "Spike brightness", "Brightness of the fine spikes", &mut f.spike_intensity, 0.0..=2.0, d.spike_intensity);
            widgets::slider_log(ui, "Spike length", "Length of the fine spikes (share of the image height)", &mut f.spike_length, 0.01..=3.0, d.spike_length);
        });
    });
    group(ui, "Anamorphic streak", "The long line of anamorphic lenses, with sparkles along it", &mut |ui| {
        widgets::grid(ui, "fl-streak", |ui| {
            widgets::slider(ui, "Streak", "Brightness of the streak (0 = none)", &mut f.streak, 0.0..=5.0, d.streak);
            widgets::slider_log(ui, "Length", "Length of the streak (share of the image height)", &mut f.streak_length, 0.05..=6.0, d.streak_length);
            widgets::slider_log(ui, "Width", "Thickness of the streak", &mut f.streak_width, 0.1..=10.0, d.streak_width);
            widgets::color(ui, "Color", "Color of the streak (blue is the classic anamorphic look)", &mut f.streak_color);
            widgets::slider(ui, "Angle", "Direction of the streak in degrees (0 = horizontal)", &mut f.streak_angle, -90.0..=90.0, d.streak_angle);
            widgets::slider_u32(ui, "Sparkles", "Small dots along the streak", &mut f.sparkles, 0..=200, d.sparkles);
            widgets::slider(ui, "Sparkle brightness", "Brightness of the sparkles", &mut f.sparkle_intensity, 0.0..=2.0, d.sparkle_intensity);
        });
    });
    group(ui, "Ghosts", "Reflections of the aperture between the lens elements, on the line from the light through the image center", &mut |ui| {
        widgets::grid(ui, "fl-ghosts", |ui| {
            widgets::slider_u32(ui, "Ghosts", "Number of ghosts", &mut f.ghosts, 0..=32, d.ghosts);
            widgets::slider(ui, "Brightness", "Brightness of the ghosts", &mut f.ghost_intensity, 0.0..=2.0, d.ghost_intensity);
            widgets::slider_log(ui, "Size", "Size of the ghosts (share of the image height)", &mut f.ghost_size, 0.005..=0.5, d.ghost_size);
            widgets::slider(ui, "Spread", "How far the ghosts spread along the line", &mut f.ghost_spread, 0.1..=3.0, d.ghost_spread);
            widgets::slider_u32(ui, "Blades", "Aperture blades: the ghost shape (0 = round)", &mut f.ghost_blades, 0..=12, d.ghost_blades);
            widgets::slider(ui, "Softness", "Soft or crisp ghost edges", &mut f.ghost_softness, 0.0..=1.0, d.ghost_softness);
            widgets::slider(ui, "Rings", "Share of hollow ghosts", &mut f.ghost_rings, 0.0..=1.0, d.ghost_rings);
            widgets::slider(ui, "Coating colors", "0: in the light's color; 1: in the varied tints of lens coatings", &mut f.ghost_colors, 0.0..=1.0, d.ghost_colors);
            widgets::slider(ui, "Color fringes", "Chromatic aberration at the edges of ghosts and ring", &mut f.chromatic, 0.0..=3.0, d.chromatic);
        });
        crate::layers::image_row(ui, &mut f.ghost_image, "Bokeh", "An image that shapes every ghost (its brightness; for example a bokeh texture with dust or an odd aperture); none: the aperture blades");
        ghost_list(ui, f);
    });
    group(ui, "Ring", "A halo ring around the light", &mut |ui| {
        widgets::grid(ui, "fl-ring", |ui| {
            widgets::slider(ui, "Ring", "Brightness of the ring (0 = none)", &mut f.ring, 0.0..=3.0, d.ring);
            widgets::slider_log(ui, "Radius", "Radius of the ring (share of the image height)", &mut f.ring_size, 0.01..=2.0, d.ring_size);
            widgets::slider_log(ui, "Width", "Width of the ring (share of the image height)", &mut f.ring_width, 0.001..=0.3, d.ring_width);
            widgets::slider(ui, "Rainbow", "0: in the light's color; 1: rainbow colors", &mut f.rainbow, 0.0..=1.0, d.rainbow);
        });
    });
    group(ui, "Lens dirt", "Dust and smudges on the lens that light up near the light", &mut |ui| {
        widgets::grid(ui, "fl-dirt", |ui| {
            widgets::slider(ui, "Dirt", "Brightness of the dirt (0 = clean lens)", &mut f.dirt, 0.0..=3.0, d.dirt);
            widgets::slider(ui, "Reach", "How far from the light the dirt lights up (share of the image height)", &mut f.dirt_reach, 0.05..=3.0, d.dirt_reach);
        });
    });
}

/// Placing helpers of positional lights.
#[derive(Clone, Copy)]
enum Place {
    AtCamera,
    AimCenter,
}

fn camera(ui: &mut Ui, scene: &mut Scene, view: &mut Viewport) {
    if widgets::section(ui, "Navigation", false).open {
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
    }
    let d = rmb_core::scene::Camera::default();
    let c = &mut scene.camera;
    if widgets::section(ui, "Position", false).open {
        widgets::grid(ui, "cam", |ui| {
            let dist = c.position.length().max(1e-12);
            let speed = (dist * 0.002).max(1e-12);
            widgets::drag(ui, "X", "Camera position along the world x axis (red line of the grid); not left/right of the view", &mut c.position.x, speed, d.position.x);
            widgets::drag(ui, "Y", "Camera position along the world y axis (green line of the grid); not forward/back of the view", &mut c.position.y, speed, d.position.y);
            widgets::drag(ui, "Z", "Camera position along the world z axis (up, blue)", &mut c.position.z, speed, d.position.z);
            widgets::slider(ui, "Field of view", "Vertical opening angle in degrees", &mut c.fov, 5.0..=120.0, d.fov);
        });
        let f = c.forward;
        ui.label(RichText::new(format!("Looking along ({:.3}, {:.3}, {:.3})", f.x, f.y, f.z)).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    }

    projection(ui, scene);

    if widgets::section(ui, "Image size", false).open {
        ui.label(RichText::new("Size and aspect of saved images; the preview keeps the aspect.").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
        widgets::grid(ui, "size", |ui| {
            widgets::label(ui, "Width", "");
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut scene.width).range(16..=16384).suffix(" px")), &mut scene.width, Scene::default().width);
            ui.end_row();
            widgets::label(ui, "Height", "");
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut scene.height).range(16..=16384).suffix(" px")), &mut scene.height, Scene::default().height);
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
}

fn render(ui: &mut Ui, scene: &mut Scene) {
    let d = rmb_core::scene::March::default();
    let m = &mut scene.march;
    let sec = widgets::section(ui, "Ray marching", true);
    if sec.reset {
        *m = d.clone();
    }
    if sec.open {
        widgets::grid(ui, "march", |ui| {
            widgets::slider_log(ui, "Detail", "Surface threshold in pixels (Mandelbulb3D's DEstop). Smaller: finer detail, slower.", &mut m.detail, 0.05..=8.0, d.detail);
            widgets::slider(ui, "Step", "Ray step multiplier. Lower it against holes and noise (overstepping).", &mut m.step, 0.05..=1.0, d.step);
            widgets::slider_log(ui, "Step limit", "Largest ray step, in half widths of the view at that distance (Mandelbulb3D's ray step limiter; 0 = no limit). Helps formulas whose distance estimate is far too large: without it, rays jump past the fractal or deep into it.", &mut m.step_limit, 0.0..=4.0, d.step_limit);
            widgets::checkbox(ui, "Step minus detail", "Every step is the estimate minus most of the surface threshold, with a step factor closer to 1 (Mandelbulb3D's \"step sub DEstop\"): more, shorter steps near the surface, fewer holes.", &mut m.step_sub);
            widgets::slider_u32(ui, "Max steps", "Steps per ray before giving up", &mut m.max_steps, 50..=3000, d.max_steps);
            widgets::slider_log(ui, "Max distance", "Rays end after this distance", &mut m.max_distance, 0.01..=1000.0, d.max_distance);
            widgets::slider_u32(ui, "Refinement", "Binary search steps at the surface for an exact hit", &mut m.refine_steps, 0..=12, d.refine_steps);
            widgets::slider_log(ui, "Near", "Rays start at this distance from the camera: hides what is closer (Mandelbulb3D imports set it)", &mut m.near, 0.0..=10.0, d.near);
            widgets::slider(ui, "Plant detail", "Level of detail of plants: branches and leaves thinner than this many pixels (radius, at their distance) are left out, which speeds up scenes with many plants (0 = all are drawn). About 0.1 gives 1.2x, 0.25 about 1.3x on wide views, but thin twigs then vanish; good for previews, set 0 for final renders. Mesh exports draw every branch.", &mut m.plant_detail, 0.0..=1.0, d.plant_detail);
            widgets::slider_u32(ui, "Smooth normals", "Averages the surface normal along the surface (steps to each side; 0 = off) and measures the roughness for the lighting. Costs more estimates per pixel.", &mut m.smooth_normals, 0..=8, d.smooth_normals);
            widgets::label(
                ui,
                "Precision",
                "Number precision of the GPU renderer. Single: fast, holds up to a zoom of about 10,000 (deeper, the surface gets grainy and blocky). Double: two numbers per value, about 14 digits, for deep zooms to about 1e11; several times slower (formulas with sine and cosine about 25x), and switching compiles the shader again (10-20 s the first time). Auto: double precision when the zoom needs it. The CPU renderer always uses double precision.",
            );
            egui::ComboBox::from_id_salt("precision").selected_text(m.precision.label()).show_ui(ui, |ui| {
                for p in rmb_core::scene::Precision::ALL {
                    ui.selectable_value(&mut m.precision, p, p.label());
                }
            });
            ui.end_row();
        });
    }
}

/// Camera tab: projection (fisheye, panoramas) and stereo rendering.
fn projection(ui: &mut Ui, scene: &mut Scene) {
    use rmb_core::scene::{Projection, StereoMode, View};
    let dv = View::default();
    let sec = widgets::section(ui, "Projection and stereo", true);
    if sec.reset {
        scene.view = dv.clone();
    }
    if !sec.open {
        return;
    }
    // Distance to the surface in the view center (a CPU march; only on demand).
    let probe = scene.clone();
    let center = move || rmb_core::render::cpu::pick_center(&probe).map_or(probe.dof.focus_distance, |(t, _)| t);
    let v = &mut scene.view;
    widgets::grid(ui, "view", |ui| {
        widgets::label(
            ui,
            "Projection",
            "How directions map onto the image. Perspective: a normal camera (field of view above). Fisheye: equidistant, the fisheye field of view spans the image height. Fisheye circle: the same in a circle as high as the image, black outside (planetarium dome masters, square image). Panorama 360°: every direction, equirectangular (VR video, environment maps; 2:1 per eye). Panorama 180°: the front half (VR180; 1:1 per eye).",
        );
        egui::ComboBox::from_id_salt("projection").selected_text(v.projection.label()).show_ui(ui, |ui| {
            for p in Projection::ALL {
                ui.selectable_value(&mut v.projection, p, p.label());
            }
        });
        ui.end_row();
        if matches!(v.projection, Projection::Fisheye | Projection::FisheyeCircle) {
            widgets::slider(ui, "Fisheye angle", "Field of view of the fisheye across the image height, in degrees (180: a half sphere; up to 360)", &mut v.fisheye_fov, 10.0..=360.0, dv.fisheye_fov);
        }
        widgets::label(
            ui,
            "Stereo",
            "Renders an image for each eye. Anaglyph: both in one image for red/cyan glasses (twice the render time). Side by side: left eye in the left half (VR180, cross-eyed or parallel viewing). Over / under: left eye on top (360° VR video). Side by side and over / under split the image size between the eyes.",
        );
        egui::ComboBox::from_id_salt("stereo").selected_text(v.stereo.label()).show_ui(ui, |ui| {
            for m in StereoMode::ALL {
                ui.selectable_value(&mut v.stereo, m, m.label());
            }
        });
        ui.end_row();
        if v.stereo != StereoMode::Off {
            if v.convergence <= 0.0 {
                // Start at the distance to the surface in the view center.
                v.convergence = center();
            }
            let c0 = v.convergence;
            widgets::slider_log(ui, "Convergence", "Distance at which both eyes see the same image point: it appears on the screen plane, nearer things in front of it, farther ones behind (world units; double-click: the surface in the view center)", &mut v.convergence, 1e-6..=100.0, 0.0);
            if v.convergence <= 0.0 {
                v.convergence = if c0 > 0.0 { center() } else { c0 };
            }
            widgets::slider_log(ui, "Eye distance", "Distance between the eyes relative to the convergence distance: more depth. About 1/30 (0.033) is comfortable; strong values strain the eyes", &mut v.eye_ratio, 0.001..=0.3, dv.eye_ratio);
            widgets::checkbox(ui, "Swap eyes", "Right eye first (cross-eyed viewing of side-by-side pairs, or glasses the other way round)", &mut v.swap_eyes);
            if v.stereo == StereoMode::Anaglyph {
                widgets::checkbox(ui, "Color anaglyph", "The red channel shows the left eye's colors. Off: its brightness (less flicker between the eyes on strongly colored surfaces)", &mut v.anaglyph_color);
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        if v.stereo != StereoMode::Off && ui.button("Converge on center").on_hover_text("Set the convergence to the surface in the view center (it then lies on the screen plane)").clicked() {
            v.convergence = center();
        }
        // Image size for the projection's natural aspect (per eye).
        if let Some(a) = v.projection.natural_aspect() {
            let (ex, ey) = match v.stereo {
                StereoMode::SideBySide => (2.0, 1.0),
                StereoMode::OverUnder => (1.0, 2.0),
                _ => (1.0, 1.0),
            };
            let total = a * ex / ey;
            let w = scene.width;
            let h = ((w as f64 / total).round() as u32).max(16);
            if h != scene.height && ui.button("Fit image size").on_hover_text(format!("Set the image to {w} × {h}: {a}:1 per eye, as the projection needs")).clicked() {
                scene.height = h;
            }
        }
    });
    if !scene.view.is_plain() {
        ui.label(RichText::new("Gizmos and guides only show in the perspective projection without stereo; lens flares need the perspective projection (per eye in stereo).").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    }
}

fn color(ui: &mut Ui, scene: &mut Scene) {
    let d = rmb_core::scene::Material::default();
    let m = &mut scene.material;
    if widgets::section(ui, "Gradient", false).open {
        gradient_editor(ui, &mut m.gradient, m.transmission);
        bands_editor(ui, &m.gradient, &mut m.bands, m.repeat, "main");
    }

    let sec = widgets::section(ui, "Mapping", true);
    if sec.reset {
        let g = std::mem::take(&mut m.gradient);
        let bands = std::mem::take(&mut m.bands);
        *m = rmb_core::scene::Material { gradient: g, bands, ..d.clone() };
    }
    if sec.open {
        match mapping(ui, m, &d) {
            Some(PositionRequest::CenterFromView) => {
                // The surface point in the middle of the view (else a point ahead).
                let cam = scene.camera.clone();
                let p = rmb_core::render::cpu::pick_center(scene).map(|(_, h)| h).unwrap_or(cam.position + cam.forward * rmb_core::render::cpu::distance_at_camera(scene).max(0.1) * 2.0);
                scene.material.color_center = [p.x, p.y, p.z];
            }
            Some(PositionRequest::DirectionFromCamera) => {
                let f = scene.camera.forward;
                scene.material.color_direction = [f.x, f.y, f.z];
            }
            None => {}
        }
    }
}

/// Gradient strip, presets, generator, tools and the color stops (see `palette_editor`).
pub fn gradient_editor(ui: &mut Ui, gradient: &mut Vec<ColorStop>, glass: bool) {
    crate::palette_editor::gradient_editor(ui, gradient, glass);
}

/// The glow bands of a gradient: a strip of the gradient with the bands (drag a band's marker
/// to move it) and per band its color, emission, position, width, softness and speed.
pub fn bands_editor(ui: &mut Ui, gradient: &[ColorStop], bands: &mut Vec<rmb_core::gradient::GlowBand>, repeat: bool, id: &str) {
    use rmb_core::gradient::{GlowBand, MAX_BANDS};
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("GLOW BANDS").size(theme::fs(10.5)).color(theme::TEXT_MUTED).strong()).on_hover_text(
            "Stripes over the gradient, independent of its colors: thin ones with emission make single glowing lines through the fractal. Animate them with the gradient's Rotation (Mapping) or each band's own speed.",
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let active = bands.iter().filter(|b| b.enabled).count();
            ui.add_enabled_ui(active < MAX_BANDS, |ui| {
                if widgets::icon_button(ui, icons::PLUS, &format!("Add a glow band (up to {MAX_BANDS} are drawn)")).clicked() {
                    let pos = bands.last().map_or(0.5, |b| (b.pos + 0.25).rem_euclid(1.0));
                    bands.push(GlowBand { pos, ..GlowBand::default() });
                }
            });
        });
    });
    if bands.is_empty() {
        return;
    }
    // The gradient with the bands, and draggable markers.
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::drag());
    let active = rmb_core::gradient::active_bands(bands);
    let mut mesh = egui::Mesh::default();
    let n = 256;
    let w = rect.width() / n as f32;
    for i in 0..n {
        let t = i as f64 / (n - 1) as f64;
        let c = rmb_core::gradient::sample(gradient, t).map(|v| v as f64);
        let (c, e) = rmb_core::gradient::apply_bands(&active, t, repeat, c, 0.0);
        let glow = 1.0 + e.min(4.0) * 0.25;
        let c = c.map(|v| (v * glow) as f32);
        let r = egui::Rect::from_min_max(egui::pos2(rect.left() + i as f32 * w, rect.top()), egui::pos2(rect.left() + (i + 1) as f32 * w + 0.5, rect.bottom()));
        mesh.add_colored_rect(r, widgets::to_color32(c));
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(rect, egui::CornerRadius::same(4), egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
    let x_of = |pos: f64| rect.left() + (pos.rem_euclid(1.0) as f32) * rect.width();
    for b in bands.iter().filter(|b| b.enabled) {
        let x = x_of(b.pos);
        let tri = vec![egui::pos2(x, rect.bottom() - 6.0), egui::pos2(x - 5.0, rect.bottom() + 3.0), egui::pos2(x + 5.0, rect.bottom() + 3.0)];
        ui.painter().add(egui::Shape::convex_polygon(tri, widgets::to_color32(b.color), egui::Stroke::new(1.0, egui::Color32::BLACK)));
    }
    let drag_id = ui.id().with(("band-drag", id));
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let near = bands.iter().enumerate().filter(|(_, b)| b.enabled).map(|(i, b)| (i, (x_of(b.pos) - p.x).abs())).filter(|(_, d)| *d < 10.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
        ui.data_mut(|d| d.insert_temp(drag_id, near));
    }
    let dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id)).flatten();
    if resp.dragged()
        && let (Some(i), Some(p)) = (dragging, resp.interact_pointer_pos())
        && let Some(b) = bands.get_mut(i)
    {
        b.pos = (((p.x - rect.left()) / rect.width()) as f64).clamp(0.0, 1.0);
    }
    resp.on_hover_text("The gradient with its glow bands (brighter where they glow). Drag a band's marker to move it.");
    ui.add_space(6.0);
    let mut remove = None;
    let db = GlowBand::default();
    for (i, b) in bands.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.checkbox(&mut b.enabled, "").on_hover_text("Draw this band");
            widgets::color_button(ui, &mut b.color).on_hover_text("Color of the band");
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut b.emission).range(0.0..=100.0).speed(0.02).max_decimals(2).prefix(format!("{} ", egui_phosphor::regular::SPARKLE)))
                .on_hover_text("Emission: the band shines by itself, lights its surroundings and glows (0 = a plain color stripe; settings under Mapping › Emissive colors)"), &mut b.emission, db.emission);
            ui.spacing_mut().slider_width = (ui.spacing().slider_width - 60.0).max(50.0);
            widgets::reset_on_double_click(widgets::fine_slider(ui, &mut b.pos, 0.0..=1.0, false, "").on_hover_text("Position of the band on the gradient (Shift / Ctrl: finer)"), &mut b.pos, db.pos);
            if widgets::icon_button(ui, icons::X, "Remove this band").clicked() {
                remove = Some(i);
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label("Width").on_hover_text("Width of the band in gradient lengths");
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut b.width).range(0.0005..=1.0).speed(0.0005).max_decimals(4)).on_hover_text("Width of the band in gradient lengths (thin: a single line through the fractal)"), &mut b.width, db.width);
            ui.label("Soft").on_hover_text("How softly the band fades out");
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut b.softness).range(0.0..=1.0).speed(0.01).max_decimals(2)).on_hover_text("Share of the band that fades out (0: hard edges, 1: soft from the center)"), &mut b.softness, db.softness);
            ui.label(egui_phosphor::regular::ARROWS_CLOCKWISE).on_hover_text("Speed");
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut b.speed).speed(0.005).max_decimals(3).suffix("/s"))
                .on_hover_text("Animation: the band wanders through the gradient by this many gradient lengths per second; the gradient's colors stay (negative: the other way)"), &mut b.speed, db.speed);
        });
    }
    if let Some(i) = remove {
        bands.remove(i);
    }
}

/// Buttons of the position color modes that need the scene.
enum PositionRequest {
    CenterFromView,
    DirectionFromCamera,
}

fn mapping(ui: &mut Ui, m: &mut rmb_core::scene::Material, d: &rmb_core::scene::Material) -> Option<PositionRequest> {
    let mut request = None;
    widgets::grid(ui, "mapping", |ui| {
        widgets::label(ui, "Color by", "Which value of the orbit picks the gradient color");
        egui::ComboBox::from_id_salt("color-mode").selected_text(m.color_mode.label()).show_ui(ui, |ui| {
            for c in ColorMode::ALL {
                ui.selectable_value(&mut m.color_mode, c, c.label());
            }
        });
        ui.end_row();
        if m.color_mode == ColorMode::SecondChoice {
            widgets::label(ui, "2nd choice", "Mandelbulb3D's second color choice: a value from the end of the orbit (also used by the orbit mode of color maps)");
            egui::ComboBox::from_id_salt("second-choice").selected_text(m.second_choice.label()).show_ui(ui, |ui| {
                for c in rmb_core::scene::SecondChoice::ALL {
                    ui.selectable_value(&mut m.second_choice, c, c.label());
                }
            });
            ui.end_row();
            if m.second_choice == rmb_core::scene::SecondChoice::LastLengthIncrease {
                widgets::slider_log(ui, "LLI multiplier", "Multiplier of the last length increase: lower on high powers, higher on formulas with low scales (Mandelbulb3D)", &mut m.lli_multiplier, 0.001..=100.0, d.lli_multiplier);
            }
        }
        widgets::label(ui, "Color on iteration", "Mandelbulb3D's 'color on iteration': the colors come from the orbit after this many iterations instead of from where it escaped (0: the start point; -1: off). Main fractal only.");
        let r = ui
            .add(egui::DragValue::new(&mut m.color_on_iteration).range(-1..=255).speed(0.1).custom_formatter(|v, _| if v < 0.0 { "off".to_string() } else { format!("{v:.0}") }))
            .on_hover_text("Mandelbulb3D's 'color on iteration': the colors come from the orbit after this many iterations instead of from where it escaped (0: the start point; -1: off). Main fractal only. Double click: off");
        if r.double_clicked() {
            m.color_on_iteration = -1;
        }
        ui.end_row();
        if matches!(m.color_mode, ColorMode::Plane | ColorMode::Sphere) {
            let what = if m.color_mode == ColorMode::Sphere { "Center of the spheres" } else { "A point on the plane where the gradient starts" };
            for (k, axis) in ["Center X", "Center Y", "Center Z"].iter().enumerate() {
                widgets::drag(ui, axis, &format!("{what} (world units)"), &mut m.color_center[k], 0.01, d.color_center[k]);
            }
            widgets::label(ui, "", "");
            if ui.small_button("Use view center").on_hover_text("Puts the center on the surface point in the middle of the view").clicked() {
                request = Some(PositionRequest::CenterFromView);
            }
            ui.end_row();
            if m.color_mode == ColorMode::Plane {
                for (k, axis) in ["Direction X", "Direction Y", "Direction Z"].iter().enumerate() {
                    widgets::drag(ui, axis, "Direction in which the gradient runs (the planes stand across it)", &mut m.color_direction[k], 0.01, d.color_direction[k]);
                }
                widgets::label(ui, "", "");
                if ui.small_button("From camera").on_hover_text("The gradient runs along the viewing direction: layers move toward or away from the camera").clicked() {
                    request = Some(PositionRequest::DirectionFromCamera);
                }
                ui.end_row();
            }
        }
        widgets::slider_log(
            ui,
            "Scale",
            if matches!(m.color_mode, ColorMode::Plane | ColorMode::Sphere) {
                "Gradient lengths per world unit: small values make wide layers (one glow band then is one front of light through the fractal)"
            } else {
                "How fast the colors change with the value (negative reverses the gradient)"
            },
            &mut m.color_scale,
            -10.0..=10.0,
            d.color_scale,
        );
        widgets::slider(ui, "Offset", "Shifts the gradient", &mut m.color_offset, -5.0..=5.0, d.color_offset);
        widgets::drag(ui, "Rotation", "Animation: the gradient (with its glow bands) turns by this many gradient lengths per second, so colors flow through the fractal (palette rotation; 0 = off)", &mut m.rotation, 0.005, d.rotation);
        if !m.interior.is_empty() {
            widgets::label(ui, "Interior colors", "Colors where rays meet the inside of the fractal (cut planes, camera inside the solid); imported from Mandelbulb3D");
            ui.horizontal(|ui| {
                widgets::gradient_strip_sized(ui, &m.interior, ui.spacing().slider_width, 14.0);
                if widgets::icon_button(ui, icons::X, "Remove the interior colors (the normal gradient is used)").clicked() {
                    m.interior.clear();
                }
            });
            ui.end_row();
        }
        widgets::slider(ui, "Depth shift", "Moves the gradient with the distance from the camera (gradient lengths per world unit beyond the reference)", &mut m.depth_shift, -2.0..=2.0, d.depth_shift);
        if m.depth_shift != 0.0 {
            widgets::slider_log(ui, "Depth reference", "Distance where the depth shift is 0", &mut m.depth_reference, 0.0..=100.0, d.depth_reference);
        }
        widgets::slider(ui, "Detail shift", "Adds this times the log of the surface threshold to iteration values: keeps colors steady while zooming (Mandelbulb3D)", &mut m.detail_shift, -2.0..=2.0, d.detail_shift);
        widgets::checkbox(ui, "Repeat", "Repeat the gradient (else clamp at its ends)", &mut m.repeat);
        widgets::slider(ui, "Color amount", "0: everything in the first gradient color", &mut m.saturation, 0.0..=1.0, d.saturation);
        widgets::slider(ui, "Specular", "Shininess of the material (all lights)", &mut m.specular, 0.0..=3.0, d.specular);
        widgets::slider(ui, "Reflection", "Mirror reflections and glass: the light amount of each reflected or refracted ray; reflections are weighted by the highlight color of the gradient (0 = off; costs another ray per bounce)", &mut m.reflection, 0.0..=2.0, d.reflection);
        if m.reflection > 0.0 {
            widgets::slider_u32(ui, "Bounces", "Reflections of reflections; with glass, how many surfaces a ray passes through or reflects from", &mut m.reflection_bounces, 1..=8, d.reflection_bounces);
            widgets::slider(ui, "Gloss", "Blurs reflections and refractions like brushed metal or frosted glass (0 = mirror sharp); needs more samples to get smooth", &mut m.gloss, 0.0..=1.0, d.gloss);
            widgets::checkbox(ui, "Glass", "Light passes through the surfaces by the transparency of each gradient color (set next to the colors), refracted and absorbed inside", &mut m.transmission);
            if m.transmission {
                widgets::slider(ui, "Refraction index", "How much rays bend entering the glass (1 = not at all, water 1.33, glass 1.5, diamond 2.4)", &mut m.refraction_index, 1.0..=3.0, d.refraction_index);
                widgets::slider_log(ui, "Absorption", "How much light the glass absorbs per world unit; tints with the surface color where the ray entered", &mut m.absorption, 0.0..=1000.0, d.absorption);
                widgets::slider_log(ui, "Scattering", "Light scattered inside the glass, in the surface color", &mut m.scattering, 0.0..=1000.0, d.scattering);
                widgets::checkbox(ui, "Light through glass", "Shadows of glass let light through, tinted by the surface color and the absorption along the way (straight through: no caustics; costs a little more per shadow ray)", &mut m.glass_shadows);
            }
        }
    });
    emissive(ui, m);
    crate::layers::layers_editor(ui, &mut m.layers, "main");
    color_map(ui, m);
    request
}

/// Settings of the emissive gradient colors.
fn emissive(ui: &mut Ui, m: &mut rmb_core::scene::Material) {
    let d = rmb_core::scene::Material::default();
    let sec = widgets::section(ui, "Emissive colors", true);
    if sec.reset {
        m.emission_light = d.emission_light;
        m.emission_reach = d.emission_reach;
        m.emission_glow = d.emission_glow;
        m.emission_glow_size = d.emission_glow_size;
        m.emission_rays = d.emission_rays;
        m.steam = d.steam;
        m.steam_size = d.steam_size;
        m.steam_grain = d.steam_grain;
        m.steam_cover = d.steam_cover;
        m.steam_offset = d.steam_offset;
        m.steam_drift = d.steam_drift;
        m.shimmer = d.shimmer;
        m.shimmer_size = d.shimmer_size;
        m.shimmer_speed = d.shimmer_speed;
        m.shimmer_phase = d.shimmer_phase;
    }
    if sec.open {
        let glow_layer = m.layers.iter().any(|l| l.enabled && l.target == rmb_core::texture::Target::Emission);
        let glow_map = m.color_map.emission > 0.0 && !m.color_map.image.trim().is_empty();
        if !glow_layer && !glow_map && !m.gradient.iter().any(|s| s.emission > 0.0) && !m.bands.iter().any(|b| b.enabled && b.emission > 0.0) {
            ui.label(RichText::new("Give a gradient color or a glow band an emission (the sparkle field next to it), add a Glow texture layer or give the color map an emission to let it shine, light its surroundings and glow.").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
            return;
        }
        widgets::grid(ui, "emissive", |ui| {
            widgets::slider_log(ui, "Light", "How much emissive colors light their surroundings in their color (0 = not; costs a ray per sample, refines with the samples)", &mut m.emission_light, 0.0..=20.0, d.emission_light);
            if m.emission_light > 0.0 {
                widgets::slider_log(ui, "Reach", "How far that light reaches, relative to the distance from the camera", &mut m.emission_reach, 0.01..=10.0, d.emission_reach);
                widgets::slider_u32(ui, "Rays", "Rays per sample that gather this light: more give a cleaner image sooner (each costs about one more ray)", &mut m.emission_rays, 1..=8, d.emission_rays);
            }
            widgets::slider_log(ui, "Glow", "Glow around emissive parts, also in the air in front of them", &mut m.emission_glow, 0.0..=20.0, d.emission_glow);
            if m.emission_glow > 0.0 {
                widgets::slider_log(ui, "Glow size", "Size of the glow, relative to the distance from the camera", &mut m.emission_glow_size, 0.001..=0.5, d.emission_glow_size);
            }
            widgets::slider_log(ui, "Steam", "Glowing haze around emissive parts in their color, broken up by 3D noise like steam or smoke (0 = off)", &mut m.steam, 0.0..=20.0, d.steam);
            if m.steam > 0.0 {
                widgets::slider_log(ui, "Steam size", "How far the haze reaches from the surface, relative to the distance from the camera", &mut m.steam_size, 0.001..=1.0, d.steam_size);
                widgets::slider_log(ui, "Steam grain", "Size of the clouds in world units (small: fine wisps, large: big clouds)", &mut m.steam_grain, 0.0001..=10.0, d.steam_grain);
                widgets::slider(ui, "Steam cover", "How much of the space the clouds fill (0: a few wisps, 1: everywhere)", &mut m.steam_cover, 0.0..=1.0, d.steam_cover);
                for (k, axis) in ["Steam drift X", "Steam drift Y", "Steam drift Z"].iter().enumerate() {
                    widgets::drag(ui, axis, "Animation: the clouds move by this much per second (world units; z is up, so a positive z rises like steam)", &mut m.steam_drift[k], 0.001, d.steam_drift[k]);
                }
                for (k, axis) in ["Steam offset X", "Steam offset Y", "Steam offset Z"].iter().enumerate() {
                    widgets::drag(ui, axis, "Moves the cloud pattern (world units) to pick another look; animations add the drift", &mut m.steam_offset[k], 0.001, d.steam_offset[k]);
                }
            }
            widgets::slider(ui, "Shimmer", "The emitted light flickers in patches like embers or flames (0 = steady); in animations it changes over time", &mut m.shimmer, 0.0..=1.0, d.shimmer);
            if m.shimmer > 0.0 {
                widgets::slider_log(ui, "Shimmer size", "Size of the flickering patches in world units", &mut m.shimmer_size, 0.0001..=10.0, d.shimmer_size);
                widgets::slider_log(ui, "Shimmer speed", "Animation: new flicker patterns per second", &mut m.shimmer_speed, 0.01..=60.0, d.shimmer_speed);
                widgets::slider(ui, "Shimmer pattern", "Picks another flicker pattern (animations move it on by the speed)", &mut m.shimmer_phase, 0.0..=100.0, d.shimmer_phase);
            }
        });
    }
}

fn color_map(ui: &mut Ui, m: &mut rmb_core::scene::Material) {
    let d = rmb_core::scene::ColorMap::default();
    let sec = widgets::section(ui, "Color map", true);
    if sec.reset {
        m.color_map = d.clone();
    }
    if sec.open {
        let cm = &mut m.color_map;
        ui.horizontal(|ui| {
            if ui.button("Image…").on_hover_text("An image that gives the surface color instead of the gradient (or a Mandelbulb3D map number)").clicked()
                && let Some(p) = crate::prefs::dialog(crate::prefs::Folder::Textures).add_filter("Image", &["jpg", "jpeg", "png", "bmp"]).pick_file()
            {
                cm.image = p.display().to_string();
            }
            ui.add(egui::TextEdit::singleline(&mut cm.image).desired_width(140.0).hint_text("off")).on_hover_text("File, name in the map folders, or Mandelbulb3D map number");
            if !cm.image.is_empty() && widgets::icon_button(ui, icons::X, "Remove the color map").clicked() {
                cm.image.clear();
            }
        });
        if cm.image.trim().is_empty() {
            return;
        }
        if rmb_core::maps::get_any(&cm.image).is_none() {
            ui.label(RichText::new("Not found: give the full path or set the maps folder in the preferences").size(theme::fs(11.0)).color(theme::WARN));
        }
        widgets::grid(ui, "cmap", |ui| {
            widgets::label(ui, "Mapping", "Orbit trap × iterations: 2D from the orbit. Surface normal: by direction. 3D waves / 3D projection: by position on the object.");
            egui::ComboBox::from_id_salt("cmap-mode").selected_text(cm.mode.label()).show_ui(ui, |ui| {
                for mm in rmb_core::scene::MapMode::ALL {
                    ui.selectable_value(&mut cm.mode, mm, mm.label());
                }
            });
            ui.end_row();
            widgets::slider_log(ui, "Scale", "Repetitions (orbit mode) or tiles per world unit (3D modes)", &mut cm.scale, 0.01..=100.0, d.scale);
            if cm.mode == rmb_core::scene::MapMode::Normal {
                for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
                    widgets::slider(ui, &format!("Rotate {axis}"), "Rotation of the map in degrees", &mut cm.rotation[k], -180.0..=360.0, 0.0);
                }
            } else {
                widgets::slider(ui, "Offset X", "Shifts the map", &mut cm.offset[0], 0.0..=1.0, 0.0);
                widgets::slider(ui, "Offset Y", "Shifts the map", &mut cm.offset[1], 0.0..=1.0, 0.0);
            }
            if cm.mode == rmb_core::scene::MapMode::Orbit {
                widgets::slider(ui, "Rotate", "Rotation of the map in degrees", &mut cm.rotation[2], -180.0..=180.0, 0.0);
            }
            widgets::checkbox(ui, "Brightness only", "Takes only the brightness of the map; the hue comes from the gradient", &mut cm.luminance_only);
            widgets::slider_log(ui, "Emission", "The bright parts of the map shine by themselves (0 = not). They light only themselves, not their surroundings; Shimmer (Emissive colors) makes them flicker", &mut cm.emission, 0.0..=20.0, 0.0);
        });
    }
}

fn effects(ui: &mut Ui, scene: &mut Scene) {
    let db = rmb_core::scene::Background::default();
    let sec = widgets::section(ui, "Background", true);
    if sec.reset {
        scene.background = db.clone();
        scene.sky = Default::default();
    }
    let b = &mut scene.background;
    if sec.open {
        widgets::segmented(
            ui,
            &mut b.mode,
            &[
                (BackgroundMode::Solid, "Solid", "One color"),
                (BackgroundMode::Gradient, "Gradient", "Sky gradient: top, horizon, bottom"),
                (BackgroundMode::Vertical, "Vertical", "Top to bottom over the screen height (Mandelbulb3D)"),
                (BackgroundMode::Sky, "Sky", "An atmosphere lit by the sun: time of day, sunsets, stars, alien skies; the sun can light the scene"),
            ],
        );
        if b.mode == BackgroundMode::Sky {
            crate::sky_editor::show(ui, &mut scene.sky);
        } else {
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
        }
        let b = &mut scene.background;
        ui.horizontal(|ui| {
            let label = if b.image.is_empty() { "No image".to_string() } else { b.image.clone() };
            if ui.button("Image…").on_hover_text("Panorama image (latitude/longitude) behind the object; the fog keeps the colors above").clicked()
                && let Some(p) = crate::prefs::dialog(crate::prefs::Folder::Textures).add_filter("Image", &["jpg", "jpeg", "png", "bmp"]).pick_file()
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
                widgets::slider(ui, "Brightness", "Brightness of the image, and of the ambient light taken from it", &mut b.image_intensity, 0.0..=4.0, db.image_intensity);
                widgets::checkbox(ui, "Smooth", "Softens the image when it loads, against noise and hard pixel edges (Mandelbulb3D's \"smooth\" option)", &mut b.image_smooth);
                widgets::checkbox(ui, "Fill the screen", "The image fills the screen instead of surrounding the scene (reflections see it by angle)", &mut b.image_screen);
                if !b.image_screen {
                    for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
                        widgets::slider(ui, &format!("Rotate {axis}"), "Rotation of the background image in degrees", &mut b.image_rotation[k], -180.0..=360.0, db.image_rotation[k]);
                    }
                    widgets::checkbox(ui, "Make spherical", "Squeezes a flat photo toward the poles so it wraps around the scene with less distortion (Mandelbulb3D's \"convert to spherical\")", &mut b.image_spherical);
                    widgets::checkbox(ui, "Blend borders", "Blends the left and right edges into each other, so the panorama has no seam", &mut b.image_fit_border);
                }
                widgets::checkbox(ui, "Fog on top", "The fog is added on top of the image instead of covering it", &mut b.image_fog_add);
                widgets::checkbox(ui, "Ambient light", "The image lights the scene: a blurred copy of it replaces the sky and ground colors of the ambient light", &mut scene.lighting.ambient_from_background);
            });
        }
    }

    let df = rmb_core::scene::Fog::default();
    let f = &mut scene.fog;
    let sec = widgets::section(ui, "Fog and glow", true);
    if sec.reset {
        *f = df.clone();
    }
    if sec.open {
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
            widgets::slider_log(
                ui,
                "Light scattering",
                "Light the fog scatters toward the camera: beams through gaps in the fractal and lit haze around lights (0 = off). Needs fog; one more shadow ray per sample, so it takes more samples to get smooth",
                &mut f.scatter,
                0.0..=50.0,
                df.scatter,
            );
            if f.scatter > 0.0 {
                widgets::slider(ui, "Forward scattering", "Above 0 the fog shines most when looking toward a light, like real haze; 0 scatters evenly; below 0 shines back toward the light", &mut f.scatter_forward, -0.9..=0.9, df.scatter_forward);
            }
            widgets::slider(ui, "Glow", "Light along rays that pass close to the surface (Mandelbulb3D's dynamic fog); negative darkens", &mut f.glow, -4.0..=4.0, df.glow);
            widgets::color(ui, "Glow color", "", &mut f.glow_color);
            widgets::slider_log(ui, "Glow offset", "Ray steps that give no glow yet", &mut f.glow_offset, 0.0..=500.0, df.glow_offset);
            widgets::slider(ui, "Offset per distance", "Ray steps added to the glow offset per unit of distance from the camera; negative values give more glow farther away (Mandelbulb3D's fog offset Z)", &mut f.glow_offset_depth, -50.0..=50.0, df.glow_offset_depth);
            widgets::slider_log(ui, "Second color after", "Ray steps until the glow has the second color (0 = only the first color)", &mut f.glow_mix_steps, 0.0..=2000.0, df.glow_mix_steps);
            widgets::color(ui, "Second glow color", "", &mut f.glow_color2);
            widgets::checkbox(ui, "Cover", "The glow covers the image instead of adding light", &mut f.glow_blend);
            widgets::slider_u32(ui, "Glow iteration", "Only ray steps where the fractal escaped after this many iterations glow (0 = all steps)", &mut f.glow_iteration, 0..=100, df.glow_iteration);
            widgets::slider_log(
                ui,
                "Fractal haze",
                "A glowing haze shaped by the main fractal: around its surfaces and in its gaps (Distance) or where its orbits take long to escape (Iterations, like Mandelbulber's iteration fog). It shines by itself and hides what lies behind it (0 = off)",
                &mut f.haze,
                0.0..=100.0,
                df.haze,
            );
            if f.haze > 0.0 {
                widgets::label(ui, "Haze shape", "What shapes the haze");
                egui::ComboBox::from_id_salt("haze-shape").selected_text(f.haze_shape.label()).show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.haze_shape, HazeShape::Distance, "Distance").on_hover_text("Thickest at the surfaces, fading with the distance from them: hugs every structure and fills narrow gaps");
                    ui.selectable_value(&mut f.haze_shape, HazeShape::Iterations, "Iterations").on_hover_text("Where the orbits take long to escape: shells around the fractal (Mandelbulber's iteration fog); depends a lot on the formula");
                });
                ui.end_row();
                match f.haze_shape {
                    HazeShape::Distance => {
                        widgets::slider_log(ui, "Haze reach", "How far the haze reaches from the surfaces, relative to the distance from the camera", &mut f.haze_size, 0.001..=1.0, df.haze_size);
                    }
                    HazeShape::Iterations => {
                        widgets::slider(ui, "Haze from", "Share of the maximum iterations from which on there is haze (higher: closer to the fractal)", &mut f.haze_start, 0.0..=1.0, df.haze_start);
                        widgets::slider(ui, "Haze to", "Share of the maximum iterations above which there is no haze again (1: none cut; lower keeps it out of regions that never escape)", &mut f.haze_end, 0.0..=1.0, df.haze_end);
                    }
                }
                widgets::slider(ui, "Haze glow", "Brightness of the haze's own light (0: a dark smoke that only hides)", &mut f.haze_glow, 0.0..=10.0, df.haze_glow);
                widgets::checkbox(ui, "Haze from gradient", "The haze takes the gradient's colors by the iteration count (like the Iterations color mode) instead of one color", &mut f.haze_gradient);
                if !f.haze_gradient {
                    widgets::color(ui, "Haze color", "", &mut f.haze_color);
                }
            }
        });
    }

    let dd = rmb_core::scene::Dof::default();
    let dof = &mut scene.dof;
    let sec = widgets::section(ui, "Depth of field", true);
    if sec.reset {
        *dof = dd.clone();
    }
    if sec.open {
        widgets::grid(ui, "dof", |ui| {
            widgets::checkbox(ui, "On", "Blur everything outside the focus distance. Double-click into the image to focus.", &mut dof.enabled);
            widgets::slider_log(ui, "Aperture", "Lens size: more blur", &mut dof.aperture, 0.0005..=1.0, dd.aperture);
            widgets::slider_log(ui, "Focus distance", "Distance from the camera that is sharp (double-click into the image to pick)", &mut dof.focus_distance, 0.0001..=100.0, dd.focus_distance);
            widgets::slider_u32(ui, "Blades", "Shape of the lens opening, seen in out-of-focus highlights (bokeh): 0 round, 3 to 9 a polygon with that many corners", &mut dof.blades, 0..=9, dd.blades);
            if dof.blades >= 3 {
                widgets::slider(ui, "Blade rotation", "Turns the polygon of the blades (degrees)", &mut dof.blade_rotation, 0.0..=360.0, dd.blade_rotation);
            }
        });
    }

    let sec = widgets::section(ui, "Cutting planes", true);
    if sec.reset {
        scene.cut = Default::default();
    }
    if sec.open {
        widgets::grid(ui, "cut", |ui| {
            for (name, p) in [("X", &mut scene.cut.x), ("Y", &mut scene.cut.y), ("Z", &mut scene.cut.z)] {
                widgets::label(ui, &format!("Cut {name}"), "Removes everything on one side of the plane");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut p.enabled, "");
                    widgets::reset_on_double_click(ui.add_enabled(p.enabled, egui::DragValue::new(&mut p.position).speed(0.005).max_decimals(6)), &mut p.position, 0.0);
                    ui.add_enabled(p.enabled, egui::Checkbox::new(&mut p.keep_above, "keep +")).on_hover_text("Keep the part above the position (else below)");
                });
                ui.end_row();
            }
        });
    }

    let dp = rmb_core::scene::Post::default();
    let p = &mut scene.post;
    let sec = widgets::section(ui, "Image", true);
    if sec.reset {
        *p = dp.clone();
    }
    if sec.open {
        ui.label(RichText::new("Applied to the finished image: changes show at once without re-rendering.").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
        widgets::grid(ui, "post", |ui| {
            widgets::slider(ui, "Exposure", "Brightness in stops", &mut p.exposure, -4.0..=4.0, dp.exposure);
            widgets::checkbox(ui, "Filmic", "Filmic tone mapping: soft highlights instead of clipping", &mut p.tone_map);
            widgets::slider(ui, "Gamma", "", &mut p.gamma, 1.0..=3.0, dp.gamma);
            widgets::slider(ui, "Contrast", "", &mut p.contrast, 0.5..=2.0, dp.contrast);
            widgets::slider(ui, "Saturation", "", &mut p.saturation, 0.0..=2.0, dp.saturation);
            widgets::slider(ui, "Vignette", "Darkens the corners", &mut p.vignette, 0.0..=1.0, dp.vignette);
            widgets::slider(ui, "Bloom", "Soft glow around bright parts, like light scattering in a lens (0 = off)", &mut p.bloom, 0.0..=3.0, dp.bloom);
            if p.bloom > 0.0 {
                widgets::slider_log(ui, "Bloom threshold", "Brightness above which parts bloom (after exposure; 1 = white). Lower: more of the image glows", &mut p.bloom_threshold, 0.0..=10.0, dp.bloom_threshold);
                widgets::slider(ui, "Bloom size", "0: a tight halo; 1: a wide glow over large parts of the image", &mut p.bloom_size, 0.0..=1.0, dp.bloom_size);
            }
            widgets::slider(ui, "Streaks", "Lines of light through very bright parts, like a camera lens shows them (0 = off)", &mut p.streak, 0.0..=3.0, dp.streak);
            if p.streak > 0.0 {
                widgets::slider_log(ui, "Streak threshold", "Brightness above which parts make streaks (after exposure; 1 = white)", &mut p.streak_threshold, 0.0..=20.0, dp.streak_threshold);
                widgets::slider(ui, "Streak length", "Length of the streaks as a share of the image width", &mut p.streak_length, 0.01..=1.0, dp.streak_length);
                widgets::slider(ui, "Streak angle", "Direction of the streaks in degrees (0: horizontal, like anamorphic lenses)", &mut p.streak_angle, -90.0..=90.0, dp.streak_angle);
                widgets::slider_u32(ui, "Streak directions", "1: a line, 2: a cross, 3 and 4: stars", &mut p.streak_count, 1..=4, dp.streak_count);
                widgets::color(ui, "Streak tint", "Color of the streaks (multiplies the light they spread)", &mut p.streak_tint);
            }
            widgets::slider(ui, "Chromatic aberration", "Colored fringes toward the edges like a cheap lens: red drawn outward, blue inward, by this many percent of the distance from the center (0 = off)", &mut p.chromatic, -3.0..=3.0, dp.chromatic);
            widgets::slider(ui, "Temperature", "White balance: warmer (above 0) or cooler colors", &mut p.temperature, -1.0..=1.0, dp.temperature);
            widgets::slider(ui, "Tint", "White balance: toward magenta (above 0) or green", &mut p.tint, -1.0..=1.0, dp.tint);
            widgets::slider(ui, "Shadow tint", "Split toning: the dark parts take the shadow color by this much (brightness stays; 0 = off)", &mut p.shadow_amount, 0.0..=1.0, dp.shadow_amount);
            if p.shadow_amount > 0.0 {
                widgets::color(ui, "Shadow color", "Color of the dark parts (e.g. teal against orange highlights)", &mut p.shadow_color);
            }
            widgets::slider(ui, "Highlight tint", "Split toning: the bright parts take the highlight color by this much (brightness stays; 0 = off)", &mut p.highlight_amount, 0.0..=1.0, dp.highlight_amount);
            if p.highlight_amount > 0.0 {
                widgets::color(ui, "Highlight color", "Color of the bright parts", &mut p.highlight_color);
            }
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
}

fn notes(ui: &mut Ui, scene: &mut Scene) {
    ui.label(RichText::new("Free text saved with the scene; imports list what could not be converted.").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    ui.add(egui::TextEdit::multiline(&mut scene.notes).desired_width(f32::INFINITY).desired_rows(16));
}

/// The hand-placed ghosts of a lens flare.
fn ghost_list(ui: &mut Ui, f: &mut rmb_core::flare::LensFlare) {
    ui.horizontal(|ui| {
        if f.ghost_list.is_empty() {
            if ui.button("Place ghosts by hand").on_hover_text("Copies the ghosts made from the seed into a list you can edit one by one (position, size, brightness, tint)").clicked() {
                f.ghost_list = f.generated_ghosts();
                if f.ghost_list.is_empty() {
                    f.ghost_list.push(rmb_core::flare::GhostDef::default());
                }
            }
        } else {
            ui.label(RichText::new(format!("{} ghosts placed by hand", f.ghost_list.len())).color(theme::TEXT_MUTED));
            if ui.button(format!("{}  Add", crate::icons::PLUS)).on_hover_text("Adds a ghost").clicked() {
                f.ghost_list.push(rmb_core::flare::GhostDef::default());
            }
            if ui.button("Use generated").on_hover_text("Removes the hand-placed ghosts: the ghosts come from the seed and the sliders above again").clicked() {
                f.ghost_list.clear();
            }
        }
    });
    let mut remove = None;
    let dg = rmb_core::flare::GhostDef::default();
    for (i, g) in f.ghost_list.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let mut c = g.tint;
            widgets::color_button(ui, &mut c).on_hover_text("Lens coating tint (mixed with the light's color by Coating colors)");
            g.tint = c;
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut g.pos).speed(0.01).range(-3.0..=4.0).max_decimals(2).prefix("at ")).on_hover_text("Position on the line from the light (0) through the image center (1) and beyond"), &mut g.pos, dg.pos);
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut g.size).speed(0.002).range(0.001..=1.0).max_decimals(3).prefix("size ")).on_hover_text("Radius (share of the image height)"), &mut g.size, dg.size);
            widgets::reset_on_double_click(ui.add(egui::DragValue::new(&mut g.brightness).speed(0.01).range(0.0..=5.0).max_decimals(2).prefix("× ")).on_hover_text("Brightness (times the ghost brightness above)"), &mut g.brightness, dg.brightness);
            ui.checkbox(&mut g.ring, "").on_hover_text("Hollow ring instead of a disc");
            if widgets::icon_button(ui, crate::icons::TRASH, "Remove this ghost").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        f.ghost_list.remove(i);
    }
}
