//! Writes underwater demo scenes (.rmb) into a folder: the bulb half in see-through water over
//! a sandy seabed, seen from above the water (`above.rmb`), from below it (`below.rmb`)
//! and from the waterline (`waterline.rmb`).
use rmb_core::compose::{ObjectMaterial, Pattern, Primitive, SceneObject, Shape, Transform};
use rmb_core::scene::ColorStop;
use rmb_core::{Scene, v3};

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let mut s = Scene::default();
    let mut sea = Primitive::water();
    if let Primitive::Water { spray, .. } = &mut sea {
        *spray = 0.3;
    }
    let two = |a: [f32; 3], b: [f32; 3]| vec![ColorStop::new(0.0, a), ColorStop::new(1.0, b)];
    s.objects = vec![
        SceneObject { name: "Seabed".into(), shape: Shape::Primitive(Primitive::seabed()), transform: Transform { position: v3(0.0, 0.0, -1.3), ..Default::default() }, material: ObjectMaterial { gradient: rmb_core::compose::seabed_gradient(), pattern: Pattern::Seabed, pattern_size: 0.6, color_scale: 1.0, repeat: false, ..Default::default() }, ..Default::default() },
        SceneObject { name: "Sea".into(), shape: Shape::Primitive(sea), transform: Transform { position: v3(0.0, 0.0, 0.2), ..Default::default() }, material: ObjectMaterial { gradient: two([0.05, 0.2, 0.3], [0.05, 0.2, 0.3]), ..Default::default() }, ..Default::default() },
    ];
    s.objects[0].plants = ["Kelp", "Sea fan", "Staghorn coral", "Algae tuft"].iter().filter_map(|n| rmb_core::plants::PlantSpecies::preset(n)).collect();
    s.march.max_distance = 40.0;
    let views = [("above", v3(3.0, -3.2, 1.6), v3(0.0, 0.0, -0.3)), ("below", v3(2.4, -2.6, -0.6), v3(0.0, 0.0, -0.4)), ("waterline", v3(2.6, -2.8, 0.2), v3(0.0, 0.0, 0.0)), ("floor", v3(1.6, -2.2, -1.05), v3(0.6, -0.6, -1.4))];
    for (name, pos, at) in views {
        s.camera.position = pos;
        s.camera.look_at(at, v3(0.0, 0.0, 1.0));
        rmb_core::io::save(&s, &dir.join(format!("{name}.rmb"))).unwrap();
    }
}
