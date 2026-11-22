//! Prints the march shader of a scene: `dump_shader <scene> [deep]`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let s = rmb_core::io::load(std::path::Path::new(&a[1])).expect("load");
    let mut prog = rmb_core::Program::for_scene(&s);
    if a.get(2).is_some_and(|v| v == "deep") {
        prog.deep = Some(s.camera.position.to_array());
    }
    println!("{}", rmb_gpu::codegen::march_shader(&prog));
}
