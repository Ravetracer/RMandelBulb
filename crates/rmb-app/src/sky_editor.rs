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
    clouds(ui, &mut sky.clouds);
    storm(ui, &mut sky.storm, sky.clouds.enabled);
    weather(ui, &mut sky.weather);
    sub(ui, "Sun and stars");
    widgets::grid(ui, "sky-disk", |ui| {
        widgets::color(ui, "Sun color", "Color of the sunlight before the atmosphere", &mut sky.sun_color);
        widgets::slider_log(ui, "Sun size", "Angular diameter of the sun disk in degrees (ours: 0.53)", &mut sky.sun_size, 0.05..=20.0, d.sun_size);
        widgets::slider_log(ui, "Disk brightness", "Brightness of the sun disk (0 = no disk, only its light)", &mut sky.sun_brightness, 0.0..=10.0, d.sun_brightness);
        widgets::slider(ui, "Stars", "How many stars show where the sky is dark", &mut sky.stars, 0.0..=1.0, d.stars);
        widgets::slider_log(ui, "Star brightness", "Brightness of the stars", &mut sky.star_brightness, 0.0..=20.0, d.star_brightness);
        widgets::checkbox(ui, "Sun lights the scene", "Light 1 follows the sun: direction, color (redder when low) and intensity; the ambient light takes the colors of the sky and the ground (its Amount in the Light tab scales it; 1 = the sky's brightness). Light 1 and the ambient colors are then set by the sky", &mut sky.sun_light);
        if sky.sun_light {
            widgets::slider_log(ui, "Sunlight", "Intensity of light 1 relative to the sun's brightness", &mut sky.sun_light_strength, 0.0..=4.0, d.sun_light_strength);
            widgets::checkbox(ui, "Environment light", "The sky lights the scene like an HDRI image: sky and clouds from every direction (a sunset glow from its side, the blue zenith from above, a cloud bank where it stands). Off: two colors, sky above and ground below", &mut sky.environment);
            widgets::checkbox(ui, "Only the sky", "Only the sky lights the scene: the sun, the sky's light and lightning. Your other lights are left out of renders (not deleted)", &mut sky.sky_only);
        }
    });
}

/// The clouds' settings (part of the sky editor).
fn clouds(ui: &mut Ui, c: &mut rmb_core::clouds::Clouds) {
    use rmb_core::clouds::Clouds;
    let d = Clouds::default();
    sub(ui, "Clouds");
    widgets::grid(ui, "sky-clouds-on", |ui| {
        widgets::checkbox(ui, "Clouds", "Volumetric clouds in the sky: lit by the sun, casting shadows, reflected by the water, moving with the wind in animations", &mut c.enabled);
    });
    if !c.enabled {
        return;
    }
    ui.horizontal(|ui| {
        widgets::label(ui, "Preset", "Sets the look (coverage, density, type, thickness, darkness, cirrus); keeps the height, size, wind and reach");
        egui::ComboBox::from_id_salt("cloud-preset").selected_text("Choose…").show_ui(ui, |ui| {
            for name in Clouds::preset_names() {
                if ui.selectable_label(false, name).clicked() {
                    c.apply_preset(name);
                }
            }
        });
    });
    widgets::grid(ui, "sky-clouds", |ui| {
        widgets::slider(ui, "Coverage", "Share of the sky the clouds cover (0: clear, 1: overcast)", &mut c.coverage, 0.0..=1.0, d.coverage);
        widgets::slider_log(ui, "Density", "How dense and opaque the clouds are", &mut c.density, 0.05..=5.0, d.density);
        widgets::slider(ui, "Type", "0: stratus (a thin flat layer), 1: cumulus (puffy heaps), 2: cumulonimbus (towering storm clouds with anvil tops)", &mut c.kind, 0.0..=2.0, d.kind);
        widgets::slider_log(ui, "Height", "Height of the cloud base above the world's zero level (world units)", &mut c.base, 0.1..=100.0, d.base);
        widgets::slider_log(ui, "Thickness", "Thickness of the cloud layer (world units); towering types need a thick layer", &mut c.thickness, 0.1..=50.0, d.thickness);
        widgets::slider_log(ui, "Size", "Typical size of a cloud (world units)", &mut c.size, 0.2..=100.0, d.size);
        widgets::slider(ui, "Detail", "How much the edges are torn into wisps and billows", &mut c.detail, 0.0..=1.0, d.detail);
        widgets::slider(ui, "Darkness", "Darker clouds: storm and rain clouds scatter less light", &mut c.darkness, 0.0..=1.0, d.darkness);
        widgets::color(ui, "Tint", "Color of the clouds' own reflection (white for Earth)", &mut c.tint);
        widgets::slider(ui, "Cirrus", "Thin high streaks above the clouds (0: none)", &mut c.cirrus, 0.0..=1.0, d.cirrus);
        widgets::slider(ui, "Wind direction", "Direction the wind blows toward, in degrees from the x axis", &mut c.wind_direction, -180.0..=180.0, d.wind_direction);
        widgets::slider_log(ui, "Wind speed", "How fast the clouds drift in animations (world units per second)", &mut c.wind_speed, 0.0..=10.0, d.wind_speed);
        widgets::slider_log(ui, "Evolution", "How fast the clouds change their shape in animations", &mut c.evolution, 0.0..=4.0, d.evolution);
        widgets::slider_log(ui, "Reach", "How far the cloud layer is drawn in detail (world units); beyond it a distant deck closes the horizon as much as the coverage says", &mut c.reach, 10.0..=5000.0, d.reach);
        widgets::slider_log(ui, "Planet radius", "Radius of the planet in world units: the cloud layer curves down with the ground, so distant clouds sink to the horizon (0 = flat)", &mut c.planet_radius, 0.0..=1_000_000.0, d.planet_radius);
        widgets::checkbox(ui, "Shadows", "The clouds shade the scene from the sun (light 1, when the sun lights the scene)", &mut c.shadows);
    });
}

/// The thunderstorm's settings (part of the sky editor).
fn storm(ui: &mut Ui, s: &mut rmb_core::storm::Storm, clouds: bool) {
    use rmb_core::storm::{Storm, StrikeMode};
    let d = Storm::default();
    sub(ui, "Thunderstorm");
    widgets::grid(ui, "sky-storm-on", |ui| {
        widgets::checkbox(ui, "Lightning", "Lightning bolts and flashes inside the clouds: they light the clouds and the scene (with shadows), and the water reflects them", &mut s.enabled);
    });
    if !s.enabled {
        return;
    }
    widgets::segmented(
        ui,
        &mut s.mode,
        &[
            (StrikeMode::Random, StrikeMode::Random.label(), "Strikes at random times, places and shapes as the animation runs (a still shows one only when its time hits a strike)"),
            (StrikeMode::Fixed, StrikeMode::Fixed.label(), "One bolt where you place it, at the moment of the flash you choose (for stills)"),
        ],
    );
    widgets::grid(ui, "sky-storm", |ui| {
        if s.mode == StrikeMode::Random {
            widgets::slider_log(ui, "Strikes", "Strikes per minute in animations", &mut s.rate, 0.5..=120.0, d.rate);
            widgets::slider_u32(ui, "Sequence", "Number of the random sequence of strikes", &mut s.seed, 0..=999, d.seed);
            widgets::slider_log(ui, "Nearest", "Nearest distance of strikes from the scene's center (world units)", &mut s.distance_min, 0.5..=500.0, d.distance_min);
            widgets::slider_log(ui, "Farthest", "Farthest distance of strikes from the scene's center (world units)", &mut s.distance_max, 1.0..=2000.0, d.distance_max);
            widgets::slider(ui, "Cloud flashes", "Share of strikes that only light the clouds from inside, without a bolt", &mut s.cloud_flashes, 0.0..=1.0, d.cloud_flashes);
        } else {
            widgets::slider(ui, "Direction", "Where the bolt strikes: degrees from the x axis around the scene's center", &mut s.direction, -180.0..=180.0, d.direction);
            widgets::slider_log(ui, "Distance", "How far from the scene's center the bolt strikes (world units)", &mut s.distance, 0.5..=1000.0, d.distance);
            widgets::slider_u32(ui, "Variant", "Shape of the bolt: every number gives another path and branches", &mut s.variant, 0..=999, d.variant);
            widgets::slider(ui, "Moment", "The moment of the flash: 0 the first, brightest stroke; later the bolt flickers with further strokes and fades", &mut s.stage, 0.0..=1.0, d.stage);
        }
        widgets::slider_log(ui, "Brightness", "Brightness of the bolts and flashes", &mut s.brightness, 0.05..=10.0, d.brightness);
        widgets::color(ui, "Color", "Color of the lightning (bluish white on Earth)", &mut s.color);
        widgets::slider(ui, "Branches", "How much the bolts branch", &mut s.branches, 0.0..=1.0, d.branches);
        widgets::slider_log(ui, "Width", "Width of a bolt's channel (world units); far bolts are drawn at least a pixel wide", &mut s.width, 0.001..=2.0, d.width);
        if !clouds {
            widgets::slider_log(ui, "Bolt top", "Where the bolts start without clouds (world height; with clouds they start at the cloud base)", &mut s.top, 0.1..=200.0, d.top);
        }
        widgets::slider(ui, "Strike height", "Where the bolts end: the height of the ground or the sea level (world units)", &mut s.ground, -50.0..=50.0, d.ground);
        widgets::slider_log(ui, "Scene light", "How strongly a strike lights the scene (direct light with shadows; with the sky lighting the scene also a flash of the ambient light)", &mut s.scene_light, 0.0..=10.0, d.scene_light);
    });
}

/// The weather's settings (part of the sky editor).
fn weather(ui: &mut Ui, w: &mut rmb_core::weather::Weather) {
    use rmb_core::weather::{Precipitation, Weather};
    let d = Weather::default();
    sub(ui, "Weather");
    ui.horizontal(|ui| {
        widgets::label(ui, "Preset", "Sets what falls (with its size, speed and color), the amounts, wet surfaces, snow cover, splashes, lens drops and mist");
        egui::ComboBox::from_id_salt("weather-preset").selected_text("Choose…").show_ui(ui, |ui| {
            for name in Weather::preset_names() {
                if ui.selectable_label(false, name).clicked() {
                    w.apply_preset(name);
                }
            }
        });
    });
    let mut kind = w.precipitation;
    widgets::segmented(
        ui,
        &mut kind,
        &[
            (Precipitation::None, Precipitation::None.label(), "Nothing falls"),
            (Precipitation::Rain, Precipitation::Rain.label(), "Thin fast streaks of rain"),
            (Precipitation::Snow, Precipitation::Snow.label(), "Slow drifting snowflakes"),
            (Precipitation::Hail, Precipitation::Hail.label(), "Small fast white hailstones"),
        ],
    );
    if kind != w.precipitation {
        w.set_kind(kind);
    }
    widgets::grid(ui, "sky-weather", |ui| {
        if w.precipitation != Precipitation::None {
            widgets::slider(ui, "Intensity", "How much falls", &mut w.intensity, 0.0..=1.0, d.intensity);
            widgets::slider_log(ui, "Particle size", "Radius of a drop, flake or hailstone (world units)", &mut w.size, 0.0005..=0.5, d.size);
            widgets::slider_log(ui, "Fall speed", "How fast it falls in animations (world units per second); rain streaks grow with it", &mut w.speed, 0.05..=50.0, d.speed);
            widgets::slider(ui, "Wind direction", "Where the wind blows the precipitation, in degrees from the x axis", &mut w.wind_direction, -180.0..=180.0, d.wind_direction);
            widgets::slider(ui, "Wind", "How much the wind slants the fall (0 straight down)", &mut w.wind, 0.0..=2.0, d.wind);
            widgets::color(ui, "Color", "Color of the drops, flakes or hailstones", &mut w.color);
            widgets::slider_log(ui, "Reach", "How far particles are drawn one by one (world units); beyond, the precipitation is a haze", &mut w.reach, 0.5..=100.0, d.reach);
            if w.precipitation != Precipitation::Snow {
                widgets::slider(ui, "Splashes", "Rain rings on the water and short splashes on wet ground", &mut w.splashes, 0.0..=1.0, d.splashes);
                widgets::slider(ui, "Lens drops", "Rain drops on the camera's glass: they run down and leave wet trails", &mut w.lens_drops, 0.0..=1.0, d.lens_drops);
            }
        }
        widgets::slider(ui, "Wetness", "Wet surfaces: darker and glossier where they face the sky", &mut w.wetness, 0.0..=1.0, d.wetness);
        widgets::slider(ui, "Snow cover", "Snow lying on faces that point up, in patches when thin", &mut w.snow_cover, 0.0..=1.0, d.snow_cover);
        if w.snow_cover > 0.0 {
            widgets::color(ui, "Snow color", "Color of the snow cover", &mut w.snow_color);
            widgets::slider(ui, "Snow slope", "How steep the snow still holds: 0 only on flat tops, 1 also on steep sides", &mut w.snow_slope, 0.0..=1.0, d.snow_slope);
        }
        widgets::slider_log(ui, "Mist", "Ground mist: its density at the base (0 = none); it thins out upward", &mut w.mist, 0.0..=5.0, d.mist);
        if w.mist > 0.0 {
            widgets::slider(ui, "Mist base", "Height where the mist is as dense as set (world units; it is denser below)", &mut w.mist_base, -50.0..=50.0, d.mist_base);
            widgets::slider_log(ui, "Mist height", "How quickly the mist thins upward: the height over which it falls to about a third (world units)", &mut w.mist_height, 0.01..=50.0, d.mist_height);
            widgets::color(ui, "Mist color", "Color of the mist (lit by the sky and the sun)", &mut w.mist_color);
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
    // Clouds make the strip costly: it is only redone when no button is held.
    let busy = sky.clouds.enabled && ui.input(|i| i.pointer.any_down());
    let tex = match cached {
        Some((s, t)) if s == *sky || busy => t,
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
    use rayon::prelude::*;
    let f = SkyFrame::new(&Sky { stars: 0.0, ..sky.clone() });
    let cf = sky.clouds.enabled.then(|| rmb_core::clouds::CloudFrame::new(sky, &f));
    let north = sky.north.to_radians();
    let px: Vec<Color32> = (0..STRIP_W * STRIP_H)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % STRIP_W, i / STRIP_W);
            // From the zenith down to 10° below the horizon, clockwise from north (a compass).
            let el = (90.0 - (y as f64 + 0.5) / STRIP_H as f64 * 100.0).to_radians();
            let az = north - (x as f64 + 0.5) / STRIP_W as f64 * std::f64::consts::TAU;
            let d = rmb_core::v3(el.cos() * az.cos(), el.cos() * az.sin(), el.sin());
            let mut c = rmb_core::sky::radiance(&f, d);
            if let Some(cf) = &cf {
                let (keep, add) = rmb_core::clouds::march(cf, rmb_core::V3::ZERO, d, f64::MAX, 0.5, rmb_core::sky::air(&f, d), &[]);
                c = [0, 1, 2].map(|k| c[k] * keep + add[k]);
            }
            let enc = |v: f64| (rmb_core::render::srgb_encode((v / (1.0 + v * 0.25)).clamp(0.0, 1.0)) * 255.0).round() as u8;
            Color32::from_rgb(enc(c[0]), enc(c[1]), enc(c[2]))
        })
        .collect();
    ColorImage::new([STRIP_W, STRIP_H], px)
}
