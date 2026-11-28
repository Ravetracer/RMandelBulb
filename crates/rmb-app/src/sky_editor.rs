// SPDX-License-Identifier: LGPL-2.1-or-later

//! Editor of the sky (background mode Sky): presets, the sun's place, the atmosphere, the sun
//! disk, stars and the light the sun gives, with a panorama strip of the horizon.

use egui::{Color32, ColorImage, RichText, TextureHandle, TextureOptions, Ui, vec2};

use rmb_core::sky::{Sky, SkyFrame, SunPlacement};

use crate::{theme, widgets};

const STRIP_W: usize = 192;
const STRIP_H: usize = 40;

pub fn show(ui: &mut Ui, sky: &mut Sky) {
    let d = Sky::default();
    strip(ui, sky);
    ui.horizontal(|ui| {
        widgets::label(ui, "Preset", "Sets the atmosphere (colors, haze, density, ozone, ground, sun color); keeps the sun's place, the stars and the brightness");
        egui::ComboBox::from_id_salt("sky-preset").selected_text("Choose…").show_ui(ui, |ui| {
            for name in Sky::preset_names() {
                if ui.selectable_label(false, name).clicked() {
                    sky.apply_preset(name);
                }
            }
        });
    });
    widgets::segmented(
        ui,
        &mut sky.placement,
        &[
            (SunPlacement::TimeOfDay, SunPlacement::TimeOfDay.label(), "The sun follows the time of day, the latitude and the day of the year (real solar geometry)"),
            (SunPlacement::Manual, SunPlacement::Manual.label(), "Place the sun by its direction and height"),
        ],
    );
    widgets::grid(ui, "sky-sun", |ui| {
        if sky.placement == SunPlacement::TimeOfDay {
            widgets::slider(ui, "Time", "Local solar time in hours (12 = noon, the sun in the south for northern latitudes)", &mut sky.hour, 0.0..=24.0, d.hour);
            widgets::slider(ui, "Latitude", "Degrees north (negative: south). Near the poles the sun stays low or never sets", &mut sky.latitude, -90.0..=90.0, d.latitude);
            widgets::slider(ui, "Day of year", "1 = January 1st; 172 = longest day in the north", &mut sky.day, 1.0..=365.0, d.day);
            widgets::slider(ui, "North", "Direction of north in the world, in degrees from the x axis (like a light's direction)", &mut sky.north, -180.0..=180.0, d.north);
        } else {
            widgets::slider(ui, "Direction", "Direction of the sun in degrees from the x axis", &mut sky.azimuth, -180.0..=180.0, d.azimuth);
            widgets::slider(ui, "Height", "Height of the sun above the horizon in degrees (negative: below it, twilight down to -18°)", &mut sky.elevation, -30.0..=90.0, d.elevation);
        }
        widgets::slider_log(ui, "Time-lapse", "Hours per second in animations (0 = the time stands still)", &mut sky.day_speed, 0.0..=4.0, d.day_speed);
    });
    sub(ui, "Atmosphere");
    widgets::grid(ui, "sky-air", |ui| {
        widgets::color(ui, "Sky color", "Color of the clear sky (Rayleigh scattering: which colors the air scatters most)", &mut sky.rayleigh_color);
        widgets::slider_log(ui, "Scattering", "How much the air scatters: a deeper sky, a whiter horizon and redder sunsets", &mut sky.rayleigh, 0.0..=2.0, d.rayleigh);
        widgets::color(ui, "Haze color", "Color of the haze or dust (Mie scattering)", &mut sky.mie_color);
        widgets::slider_log(ui, "Haze", "Amount of haze or dust: a paler sky and a glow around the sun", &mut sky.mie, 0.0..=3.0, d.mie);
        // One slider for the three channels: moves them together.
        let mut halo = sky.mie_direction.iter().sum::<f64>() / 3.0;
        let before = halo;
        widgets::slider(ui, "Sun glow", "How tightly the haze gathers around the sun (0: evenly, 0.95: a small bright halo). Some presets differ per color (Mars: blue sunsets); this moves all three", &mut halo, 0.0..=0.95, 0.76);
        if halo != before {
            let delta = halo - before;
            sky.mie_direction = sky.mie_direction.map(|g| (g + delta).clamp(-0.95, 0.95));
        }
        widgets::slider_log(ui, "Density", "Thickness of the atmosphere (multiplies scattering and haze)", &mut sky.density, 0.0..=5.0, d.density);
        widgets::slider(ui, "Ozone", "Ozone high above absorbs orange and green: keeps the sky blue at dusk (Earth: 1)", &mut sky.ozone, 0.0..=3.0, d.ozone);
        widgets::color(ui, "Ground", "Color of the ground below the horizon", &mut sky.ground_color);
        widgets::slider_log(ui, "Brightness", "Overall brightness of the sky and the stars", &mut sky.brightness, 0.0..=8.0, d.brightness);
    });
    sub(ui, "Sun and stars");
    widgets::grid(ui, "sky-disk", |ui| {
        widgets::color(ui, "Sun color", "Color of the sunlight before the atmosphere", &mut sky.sun_color);
        widgets::slider_log(ui, "Sun size", "Angular diameter of the sun disk in degrees (ours: 0.53)", &mut sky.sun_size, 0.05..=20.0, d.sun_size);
        widgets::slider_log(ui, "Disk brightness", "Brightness of the sun disk (0 = no disk, only its light)", &mut sky.sun_brightness, 0.0..=10.0, d.sun_brightness);
        widgets::slider(ui, "Stars", "How many stars show where the sky is dark", &mut sky.stars, 0.0..=1.0, d.stars);
        widgets::slider_log(ui, "Star brightness", "Brightness of the stars", &mut sky.star_brightness, 0.0..=20.0, d.star_brightness);
        widgets::checkbox(ui, "Sun lights the scene", "Light 1 follows the sun: direction, color (redder when low) and intensity; the ambient light takes the colors of the sky and the ground. Their settings in the Light tab are then set by the sky", &mut sky.sun_light);
        if sky.sun_light {
            widgets::slider_log(ui, "Sunlight", "Intensity of light 1 relative to the sun's brightness", &mut sky.sun_light_strength, 0.0..=4.0, d.sun_light_strength);
            widgets::slider_log(ui, "Skylight", "Ambient light from the sky relative to its brightness: fills the shadows bluish by day, warm at sunset", &mut sky.sky_light_strength, 0.0..=4.0, d.sky_light_strength);
        }
    });
}

fn sub(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).color(theme::TEXT_MUTED).size(theme::fs(11.0)));
}

/// The horizon all around (left to right: north, east, south, west) from the ground to the
/// zenith, recomputed when the sky changes.
fn strip(ui: &mut Ui, sky: &Sky) {
    let id = egui::Id::new("sky-strip");
    let cached: Option<(Sky, TextureHandle)> = ui.data(|d| d.get_temp(id));
    let tex = match cached {
        Some((s, t)) if s == *sky => t,
        _ => {
            let t = ui.ctx().load_texture("sky-strip", strip_image(sky), TextureOptions::LINEAR);
            ui.data_mut(|d| d.insert_temp(id, (sky.clone(), t.clone())));
            t
        }
    };
    let w = ui.available_width().min(320.0);
    let r = ui.add(egui::Image::new((tex.id(), vec2(w, w * STRIP_H as f32 / STRIP_W as f32 * 1.6))).corner_radius(4.0));
    r.on_hover_text("The sky all around: north, east, south, west, from just below the horizon to the zenith");
}

fn strip_image(sky: &Sky) -> ColorImage {
    let f = SkyFrame::new(&Sky { stars: 0.0, ..sky.clone() });
    let north = sky.north.to_radians();
    let mut px = Vec::with_capacity(STRIP_W * STRIP_H);
    for y in 0..STRIP_H {
        // From the zenith down to 10° below the horizon.
        let el = (90.0 - (y as f64 + 0.5) / STRIP_H as f64 * 100.0).to_radians();
        for x in 0..STRIP_W {
            // Clockwise from north, as on a compass.
            let az = north - (x as f64 + 0.5) / STRIP_W as f64 * std::f64::consts::TAU;
            let d = rmb_core::v3(el.cos() * az.cos(), el.cos() * az.sin(), el.sin());
            let c = rmb_core::sky::radiance(&f, d);
            let enc = |v: f64| (rmb_core::render::srgb_encode((v / (1.0 + v * 0.25)).clamp(0.0, 1.0)) * 255.0).round() as u8;
            px.push(Color32::from_rgb(enc(c[0]), enc(c[1]), enc(c[2])));
        }
    }
    ColorImage::new([STRIP_W, STRIP_H], px)
}
