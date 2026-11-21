//! Prints the march shader of a scene: `dump_shader <scene>`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let s = rmb_core::io::load(std::path::Path::new(&a[1])).expect("load");
    let prog = rmb_core::Program::new(&s.fractal, &s.cut);
    println!("{}", rmb_gpu::codegen::march_shader(&prog));
}
