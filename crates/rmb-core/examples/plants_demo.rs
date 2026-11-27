//! Writes demo scenes with plants baked onto surfaces (.rmb) into a folder: the bulb overgrown
//! with algae and sea fans (`bulb.rmb`, `bulb_close.rmb`) and a ball with kelp next to it
//! (`ball.rmb`), and parametric L-system trees on a ground plane (`trees.rmb`).
use rmb_core::compose::{Primitive, SceneObject, Shape, Transform};
use rmb_core::plants::PlantSpecies;
use rmb_core::{Scene, v3};

fn species(name: &str, spacing: f64, height: f64, density: f64, upright: f64) -> PlantSpecies {
    let mut s = PlantSpecies::preset(name).expect("preset");
    (s.spacing, s.height, s.density, s.upright) = (spacing, height, density, upright);
    s
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let mut s = Scene::default();
    s.plant_cover.species = vec![species("Algae tuft", 0.035, 0.06, 0.9, 0.3), species("Sea fan", 0.09, 0.12, 0.5, 0.5)];
    s.camera.position = v3(1.9, -2.2, 1.2);
    s.camera.look_at(v3(0.0, 0.0, 0.0), v3(0.0, 0.0, 1.0));
    rmb_core::io::save(&s, &dir.join("bulb.rmb")).unwrap();
    s.camera.position = v3(0.95, -0.95, 0.75);
    s.camera.look_at(v3(0.3, -0.3, 0.3), v3(0.0, 0.0, 1.0));
    rmb_core::io::save(&s, &dir.join("bulb_close.rmb")).unwrap();
    let mut ball = SceneObject { name: "Ball".into(), shape: Shape::Primitive(Primitive::Sphere { radius: 0.6 }), transform: Transform { position: v3(0.0, -2.0, 0.0), ..Default::default() }, ..Default::default() };
    ball.cover.species = vec![species("Kelp", 0.18, 0.5, 0.8, 0.7), species("Staghorn coral", 0.12, 0.12, 0.6, 0.2)];
    s.objects.push(ball);
    s.camera.position = v3(2.6, -4.2, 1.2);
    s.camera.look_at(v3(0.0, -1.4, 0.0), v3(0.0, 0.0, 1.0));
    rmb_core::io::save(&s, &dir.join("ball.rmb")).unwrap();
    // Trees on a ground in a box (the plane is endless).
    let mut t = Scene::default();
    t.fractal.transform = Transform { position: v3(0.0, 0.0, 0.3), scale: 0.6, ..Default::default() };
    let mut ground = SceneObject { name: "Ground".into(), shape: Shape::Primitive(Primitive::Plane), transform: Transform { position: v3(0.0, 0.0, -0.4), ..Default::default() }, ..Default::default() };
    ground.cover.species = vec![species("Monopodial tree", 0.9, 0.9, 0.5, 1.0), species("Ternary tree", 1.1, 0.8, 0.5, 1.0)];
    ground.cover.region = Some([[-4.0, -4.0, -0.1], [4.0, 4.0, 0.1]]);
    t.objects.push(ground);
    t.camera.position = v3(4.2, -4.6, 1.3);
    t.camera.look_at(v3(0.0, 0.0, 0.0), v3(0.0, 0.0, 1.0));
    rmb_core::io::save(&t, &dir.join("trees.rmb")).unwrap();
}
