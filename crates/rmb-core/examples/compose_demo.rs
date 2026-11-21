//! Writes a composed demo scene (.rmb): the bulb on a checkered ground, a second
//! fractal, a carved sphere and a melted ring.
use rmb_core::compose::{BoolOp, ObjectMaterial, Pattern, Primitive, SceneObject, Shape, Transform};
use rmb_core::scene::{ColorStop, DeMode, Fractal, Slot};
use rmb_core::{Scene, v3};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "compose_demo.rmb".into());
    let mut s = Scene::default();
    let two = |a: [f32; 3], b: [f32; 3]| vec![ColorStop::new(0.0, a), ColorStop::new(1.0, b)];
    let mut glow = two([0.05, 0.35, 1.0], [0.9, 0.3, 0.05]);
    glow[0].emission = 1.2;
    let boxf = Fractal { slots: vec![Slot::new("Amazing Box")], max_iterations: 12, bailout: 16.0, de_mode: DeMode::Linear, ..Default::default() };
    s.objects = vec![
        SceneObject { name: "Box fractal".into(), shape: Shape::Fractal(Box::new(boxf)), transform: Transform { position: v3(1.7, 1.2, -0.65), rotation: [0.0, 0.0, 30.0], scale: 0.1 }, material: ObjectMaterial { gradient: glow, color_scale: 0.3, ..Default::default() }, ..Default::default() },
        SceneObject { name: "Ground".into(), shape: Shape::Primitive(Primitive::Plane), transform: Transform { position: v3(0.0, 0.0, -1.1), ..Default::default() }, material: ObjectMaterial { gradient: two([0.85, 0.85, 0.8], [0.12, 0.12, 0.16]), pattern: Pattern::Checker, pattern_size: 0.5, repeat: false, ..Default::default() }, ..Default::default() },
        SceneObject { name: "Carve".into(), op: BoolOp::Subtract, shape: Shape::Primitive(Primitive::Sphere { radius: 0.55 }), transform: Transform { position: v3(0.7, -0.7, 0.5), ..Default::default() }, ..Default::default() },
        SceneObject { name: "Ring".into(), op: BoolOp::SmoothUnion, smooth: 0.15, shape: Shape::Primitive(Primitive::Torus { major: 1.05, minor: 0.06 }), transform: Transform { rotation: [75.0, 0.0, 35.0], ..Default::default() }, material: ObjectMaterial { own: false, ..Default::default() }, ..Default::default() },
    ];
    s.camera.position = v3(3.4, -3.6, 1.3);
    s.camera.look_at(v3(0.3, 0.4, -0.3), v3(0.0, 0.0, 1.0));
    s.march.max_distance = 60.0;
    rmb_core::io::save(&s, std::path::Path::new(&out)).unwrap();
}
