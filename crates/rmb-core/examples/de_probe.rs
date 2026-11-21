//! Prints the distance estimate along the center ray of a scene: `de_probe <scene> [steps] [from] [to]`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let s = rmb_core::io::load(std::path::Path::new(&a[1])).expect("load");
    let n: usize = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(20);
    let prog = rmb_core::Program::for_scene(&s);
    let mut ev = prog.evaluator();
    let (o, d) = (s.camera.position, s.camera.forward);
    let t0: f64 = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let far: f64 = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(s.march.max_distance.min(8.0));
    for k in 0..=n {
        let t = t0 + (far - t0) * k as f64 / n as f64;
        let ob = ev.orbit(o + d * t, 1e-6);
        println!("t {t:8.4}  de {:10.5}  its {:3}  trap {:.4}", ob.de, ob.iterations, ob.trap);
    }
}
