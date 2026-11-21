//! Prints key values of an imported scene.
fn main() {
    for a in std::env::args().skip(1) {
        let s = rmb_core::io::load(std::path::Path::new(&a)).expect("load");
        let de = rmb_core::render::cpu::distance_at_camera(&s);
        let hit = rmb_core::render::cpu::pick(&s, 0.0, 0.0, s.width, s.height);
        println!("{a}\n  cam {:?} fwd {:?} fov {} maxd {:.3} detail {} step {}\n  fog {:.4} glow {:.3} de@cam {de:.5} center hit {:?}\n  bailout {} iters {} julia {} {:?}\n  slots {:?}\n  lights {:?}", s.camera.position, s.camera.forward, s.camera.fov, s.march.max_distance, s.march.detail, s.march.step, s.fog.density, s.fog.glow, hit.map(|h| h.0), s.fractal.bailout, s.fractal.max_iterations, s.fractal.julia, s.fractal.julia_c, s.fractal.slots.iter().map(|x| (&x.formula, x.iterations, &x.params)).collect::<Vec<_>>(), s.lighting.lights.iter().map(|l| (l.azimuth as i32, l.elevation as i32, l.intensity)).collect::<Vec<_>>());
    }
}
