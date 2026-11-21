//! Renders every lens flare preset on black into one PNG (to judge the looks).
use rmb_core::flare::LensFlare;
use rmb_core::scene::{Light, LightKind, Scene};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "flares.png".into());
    let (w, h) = (480u32, 270u32);
    let presets = LensFlare::presets();
    let cols = 4;
    let rows = presets.len().div_ceil(cols);
    let mut sheet = vec![0u8; (w as usize * cols) * (h as usize * rows) * 4];
    for (i, (_, f)) in presets.iter().enumerate() {
        let mut s = Scene::default();
        s.fractal.slots.clear();
        let cam = s.camera.clone();
        let p = cam.position + cam.forward * 2.0 - cam.right * 0.45 + cam.up * 0.25;
        s.lighting.lights = vec![Light { kind: LightKind::Point, position: p, color: [1.0, 0.95, 0.85], flare: f.clone(), ..Default::default() }];
        let img = rmb_core::flare::layer(&s, w, h).unwrap_or_default();
        let (ox, oy) = ((i % cols) * w as usize, (i / cols) * h as usize);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let c = img.get(y * w as usize + x).copied().unwrap_or([0.0; 3]);
                let o = ((oy + y) * w as usize * cols + ox + x) * 4;
                for k in 0..3 {
                    let v = 1.0 - (-c[k] as f64 * 1.5).exp();
                    sheet[o + k] = (v.powf(1.0 / 2.2) * 255.0) as u8;
                }
                sheet[o + 3] = 255;
            }
        }
    }
    rmb_core::io::image::write_png(std::path::Path::new(&out), w * cols as u32, h * rows as u32, &sheet, None).unwrap();
}
