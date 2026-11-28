// SPDX-License-Identifier: LGPL-2.1-or-later

//! Writes the manual's sky examples: the bulb over the sea at sunset, the sky as background
//! and the sun as light 1 (`sky_demo out.rmb`); with `--clouds`, evening cumulus clouds;
//! with `--storm`, a night thunderstorm with a bolt beside the bulb; with `--rain`, a
//! downpour over the sea; with `--snow`, a snowfall seen from above the bulb.
use rmb_core::compose::{ObjectMaterial, Primitive, SceneObject, Shape, Transform};
use rmb_core::scene::{BackgroundMode, ColorStop};
use rmb_core::{Scene, v3};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "sunset-sea.rmb".into());
    let clouds = std::env::args().any(|a| a == "--clouds");
    let storm = std::env::args().any(|a| a == "--storm");
    let rain = std::env::args().any(|a| a == "--rain");
    let snow = std::env::args().any(|a| a == "--snow");
    let mut s = Scene::default();
    s.background.mode = BackgroundMode::Sky;
    s.sky.hour = 19.6;
    if clouds {
        s.sky.hour = 18.6;
        s.sky.clouds.enabled = true;
        s.sky.clouds.apply_preset("Cumulus");
        s.lighting.ambient = 1.0;
    }
    s.fog.density = 0.02;
    s.fog.from_background = true;
    let sea_color = vec![ColorStop::new(0.0, [0.03, 0.08, 0.1]), ColorStop::new(1.0, [0.03, 0.08, 0.1])];
    let mut sea = Primitive::water();
    if let Primitive::Water { spray, .. } = &mut sea {
        *spray = 0.0;
    }
    s.objects = vec![SceneObject { name: "Sea".into(), shape: Shape::Primitive(sea), transform: Transform { position: v3(0.0, 0.0, -1.35), ..Default::default() }, material: ObjectMaterial { gradient: sea_color, ..Default::default() }, ..Default::default() }];
    s.march.max_distance = 200.0;
    // Low over the water, the setting sun beside the bulb.
    let sun = s.sky.sun_dir();
    let a = sun.y.atan2(sun.x) + std::f64::consts::PI + 0.36;
    let back = v3(a.cos(), a.sin(), 0.0);
    s.camera.position = back * 3.6 + v3(0.0, 0.0, -0.6);
    s.camera.look_at(v3(0.0, 0.0, -0.25), v3(0.0, 0.0, 1.0));
    if storm {
        s.sky.hour = 18.6;
        s.sky.clouds.enabled = true;
        s.sky.clouds.apply_preset("Storm");
        s.lighting.ambient = 1.0;
        // Night, a bolt beside the bulb behind it, at the first stroke.
        s.sky.hour = 22.5;
        let f = s.camera.forward;
        s.sky.storm = rmb_core::storm::Storm { enabled: true, mode: rmb_core::storm::StrikeMode::Fixed, direction: f.y.atan2(f.x).to_degrees() + 17.0, distance: 30.0, variant: 3, ground: -1.35, ..Default::default() };
    }
    if rain || snow {
        s.sky.hour = 11.0;
        s.sky.clouds.enabled = true;
        s.sky.clouds.apply_preset("Overcast");
        s.lighting.ambient = 1.0;
        s.sky.weather.apply_preset(if rain { "Downpour" } else { "Snowfall" });
        s.sky.weather.time = 1.3;
        if snow {
            s.sky.weather.snow_cover = 0.9;
        }
        // From a little above, so the tops (snow, wet shine) show.
        s.camera.position = v3(2.4, -2.4, 1.6);
        s.camera.look_at(v3(0.0, 0.0, -0.1), v3(0.0, 0.0, 1.0));
    }
    rmb_core::sky::sync_sun(&mut s);
    rmb_core::io::save(&s, std::path::Path::new(&out)).unwrap();
    println!("{out}");
}
