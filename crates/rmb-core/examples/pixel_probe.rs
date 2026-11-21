//! Prints how one pixel is shaded: `pixel_probe <scene> <width> <height> <px> <py>`.
use rmb_core::render::cpu::Shader;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let s = rmb_core::io::load(std::path::Path::new(&a[1])).expect("load");
    let (w, h, px, py): (u32, u32, u32, u32) = (a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap(), a[5].parse().unwrap());
    let prog = rmb_core::Program::new(&s.fractal, &s.cut);
    let sh = Shader::new(&s, &prog, w, h);
    let mut ev = prog.evaluator();
    let sx = ((px as f64 + 0.5) / w as f64) * 2.0 - 1.0;
    let sy = 1.0 - ((py as f64 + 0.5) / h as f64) * 2.0;
    let d = s.camera.ray_dir(sx, sy, sh.frame.aspect);
    let t0 = rmb_core::render::start_distance(&s);
    let (hit, steps, glow) = sh.march(&mut ev, s.camera.position, d, t0, s.march.max_distance, s.march.max_steps);
    println!("t0 {t0:.4} steps {steps} glowing steps {glow}");
    if let Some(hh) = hit {
        let n = sh.normal(&mut ev, hh.pos, hh.eps);
        println!("hit t {:.5} eps {:.2e} de {:.3e} its {} escaped {} normal {:?} n.d {:.3}", hh.t, hh.eps, hh.orbit.de, hh.orbit.iterations, hh.orbit.escaped, n, n.dot(d));
        println!("color {:?}", sh.sample(&mut ev, px, py, 0));
    } else {
        println!("miss");
    }
}
