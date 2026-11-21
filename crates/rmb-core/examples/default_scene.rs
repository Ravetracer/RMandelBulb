//! Prints the default scene as JSON.
fn main() {
    println!("{}", rmb_core::Scene::default().to_json());
}
