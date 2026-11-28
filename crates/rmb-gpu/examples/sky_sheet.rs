// SPDX-License-Identifier: LGPL-2.1-or-later

//! Renders every sky preset at several times of day as one sheet (rows: presets, columns:
//! hours), to judge changes to the sky model: `sky_sheet out.png [--panorama]
//! [--clouds <preset>] [--rows clouds]` (`--rows clouds`: rows are the cloud presets over
//! Earth clear instead of the sky presets).

use rmb_core::Scene;
use rmb_core::scene::{BackgroundMode, Projection};
use rmb_core::sky::Sky;
use rmb_core::v3;

const HOURS: [f64; 6] = [7.0, 10.0, 13.0, 18.5, 19.8, 21.5];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "sky_sheet.png".into());
    let panorama = args.iter().any(|a| a == "--panorama");
    let clouds = args.iter().position(|a| a == "--clouds").and_then(|i| args.get(i + 1)).cloned();
    let cloud_rows = args.windows(2).any(|w| w[0] == "--rows" && w[1] == "clouds");
    let weather_rows = args.windows(2).any(|w| w[0] == "--rows" && w[1] == "weather");
    let (w, h) = if panorama { (320u32, 160u32) } else { (256, 144) };
    let names: Vec<&str> = if weather_rows {
        rmb_core::weather::Weather::preset_names()
    } else if cloud_rows {
        rmb_core::clouds::Clouds::preset_names()
    } else {
        Sky::preset_names()
    };
    let mut sheet = image::RgbaImage::new(w * HOURS.len() as u32, h * names.len() as u32);
    for (row, name) in names.iter().enumerate() {
        for (col, hour) in HOURS.iter().enumerate() {
            let mut s = Scene::default();
            s.background.mode = BackgroundMode::Sky;
            if weather_rows {
                s.sky.weather.apply_preset(name);
                s.sky.weather.time = 1.3;
                // Grey skies for the wet and white presets.
                if s.sky.weather.precipitation != rmb_core::weather::Precipitation::None {
                    s.sky.clouds.enabled = true;
                    s.sky.clouds.apply_preset("Overcast");
                }
            } else if cloud_rows {
                s.sky.clouds.enabled = true;
                s.sky.clouds.apply_preset(name);
            } else {
                s.sky.apply_preset(name);
                if let Some(c) = &clouds {
                    s.sky.clouds.enabled = true;
                    s.sky.clouds.apply_preset(c);
                }
            }
            s.lighting.ambient = 1.0;
            s.sky.hour = *hour;
            s.camera.position = v3(3.2, -3.2, 0.3);
            s.camera.look_at(v3(0.0, 0.0, 0.6), v3(0.0, 0.0, 1.0));
            if panorama {
                s.view.projection = Projection::Panorama;
            }
            let ctx = rmb_gpu::GpuContext::new_headless().expect("GPU");
            let img = rmb_gpu::render_blocking(ctx, &s, w, h, 4, |_| true).expect("render");
            for (i, px) in img.chunks(4).enumerate() {
                let (x, y) = (i as u32 % w, i as u32 / w);
                sheet.put_pixel(col as u32 * w + x, row as u32 * h + y, image::Rgba([px[0], px[1], px[2], 255]));
            }
        }
        eprintln!("{name}");
    }
    sheet.save(&out).expect("save");
    println!("{out}: rows {names:?}, hours {HOURS:?}");
}
