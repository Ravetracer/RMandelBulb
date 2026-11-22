//! Writes a deep zoom of a scene: the camera moved along the view ray through the image center
//! toward the surface until it is `zoom` times closer: `deep_scene <in> <out> <zoom> [precision]`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut s = rmb_core::io::load(std::path::Path::new(&a[1])).expect("load");
    let zoom: f64 = a[3].parse().expect("zoom");
    s.march.near = 0.0;
    // Ten times closer per step, measuring the distance again each time (the hit is only
    // accurate to a part of the distance).
    let (t0, _) = rmb_core::render::cpu::pick(&s, 0.0, 0.0, 1024, 1024).expect("no surface at the center");
    let mut left = zoom;
    while left > 1.0 + 1e-9 {
        let f = left.min(10.0);
        let (t, _) = rmb_core::render::cpu::pick(&s, 0.0, 0.0, 1024, 1024).expect("no surface at the center");
        s.camera.position += s.camera.forward * (t - t / f);
        left /= f;
    }
    let (t, _) = rmb_core::render::cpu::pick(&s, 0.0, 0.0, 1024, 1024).expect("no surface");
    let prog = rmb_core::Program::for_scene(&s);
    eprintln!("zoom {:.3e} ratio(180 px) {:.2e}", t0 / t, rmb_core::render::zoom_ratio(&s, &prog, 180));
    let t = t0;
    if let Some(p) = a.get(4) {
        s.march.precision = match p.as_str() {
            "single" => rmb_core::scene::Precision::Single,
            "double" => rmb_core::scene::Precision::Double,
            _ => rmb_core::scene::Precision::Auto,
        };
    }
    std::fs::write(&a[2], s.to_json()).expect("write");
    eprintln!("surface at {t}, camera now {:?}", s.camera.position);
}
