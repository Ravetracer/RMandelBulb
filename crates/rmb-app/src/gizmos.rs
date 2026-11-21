// SPDX-License-Identifier: LGPL-2.1-or-later

//! Light gizmos in the view: every light drawn by its type, handles on the selected one.
//!
//! - Point: a sun icon. Spot: icon, cone (to the aim point) and the aim point. Area: the
//!   rectangle with an arrow toward where it shines. Line: the tube between its ends.
//! - Directional lights are dots on a dial in the corner of the view: the dial is the view seen
//!   from the camera, a filled dot shines from the camera's side, a hollow one from behind.
//! - Handles of the selected light: position, aim point (line: other end), the spot's cone rim
//!   (angle), the area's corner (size), the dial dot (direction). Handles move in the plane
//!   facing the camera; with Shift toward or away from the camera.

use egui::{Color32, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use rmb_core::scene::{Camera, LightKind, Scene};
use rmb_core::{V3, v3};

use crate::theme;

/// A draggable part of a light gizmo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Position(usize),
    Target(usize),
    Rim(usize),
    Corner(usize),
    Dial(usize),
    /// Move along world axis 0 (x), 1 (y) or 2 (z).
    Axis(usize, usize),
    /// Center and axis arrows of composed object k.
    ObjCenter(usize),
    ObjAxis(usize, usize),
}

/// World axes and their colors (x red, y green, z blue, as in 3D editors).
pub const AXES: [(V3, Color32, &str); 3] = [
    (V3 { x: 1.0, y: 0.0, z: 0.0 }, Color32::from_rgb(235, 80, 80), "X"),
    (V3 { x: 0.0, y: 1.0, z: 0.0 }, Color32::from_rgb(110, 200, 80), "Y"),
    (V3 { x: 0.0, y: 0.0, z: 1.0 }, Color32::from_rgb(80, 140, 240), "Z"),
];

/// Length of the move arrows in pixels.
const ARROW_PX: f64 = 70.0;

/// Projection of the view (camera and image rectangle).
pub struct Proj {
    cam: Camera,
    t: f64,
    aspect: f64,
    rect: Rect,
}

impl Proj {
    pub fn new(cam: &Camera, aspect: f64, rect: Rect) -> Proj {
        Proj { cam: cam.clone(), t: cam.tan_half_fov(), aspect, rect }
    }

    fn depth(&self, p: V3) -> f64 {
        (p - self.cam.position).dot(self.cam.forward)
    }

    pub fn screen(&self, p: V3) -> Option<Pos2> {
        let d = p - self.cam.position;
        let z = d.dot(self.cam.forward);
        if z <= 1e-9 {
            return None;
        }
        let sx = d.dot(self.cam.right) / (z * self.t * self.aspect);
        let sy = d.dot(self.cam.up) / (z * self.t);
        (sx.abs() < 50.0 && sy.abs() < 50.0).then(|| pos2(self.rect.center().x + sx as f32 * self.rect.width() * 0.5, self.rect.center().y - sy as f32 * self.rect.height() * 0.5))
    }

    /// World length of one pixel at `p`.
    fn world_per_px(&self, p: V3) -> f64 {
        2.0 * self.depth(p).max(1e-9) * self.t / self.rect.height() as f64
    }

    /// A world segment on screen, clipped where it passes behind the camera.
    pub fn segment(&self, a: V3, b: V3) -> Option<(Pos2, Pos2)> {
        let near = 1e-4 * (self.cam.position.length() + 1.0);
        let (za, zb) = (self.depth(a), self.depth(b));
        if za < near && zb < near {
            return None;
        }
        let (a, b) = if za < near {
            (a + (b - a) * ((near - za) / (zb - za)), b)
        } else if zb < near {
            (a, a + (b - a) * ((near - za) / (zb - za)))
        } else {
            (a, b)
        };
        let s = |p: V3| {
            let d = p - self.cam.position;
            let z = d.dot(self.cam.forward).max(near);
            let sx = d.dot(self.cam.right) / (z * self.t * self.aspect);
            let sy = d.dot(self.cam.up) / (z * self.t);
            pos2(self.rect.center().x + sx as f32 * self.rect.width() * 0.5, self.rect.center().y - sy as f32 * self.rect.height() * 0.5)
        };
        Some((s(a), s(b)))
    }

    /// `p` moved by the screen offset `delta` in the plane facing the camera (with `depth`:
    /// toward or away from the camera instead).
    fn drag(&self, p: V3, delta: Vec2, depth: bool) -> V3 {
        let z = self.depth(p).max(1e-9);
        if depth {
            return p - self.cam.forward * (delta.y as f64 * z * 0.005);
        }
        let dx = delta.x as f64 * 2.0 * z * self.t * self.aspect / self.rect.width() as f64;
        let dy = -(delta.y as f64) * 2.0 * z * self.t / self.rect.height() as f64;
        p + self.cam.right * dx + self.cam.up * dy
    }
}

/// The dial of the directional lights: center and radius.
pub fn dial(rect: Rect) -> (Pos2, f32) {
    let r = 34.0;
    (pos2(rect.right() - r - 14.0, rect.top() + r + 14.0), r)
}

/// Direction toward a directional light in view coordinates: right, up, toward the camera.
fn dial_coords(cam: &Camera, d: V3) -> (f64, f64, f64) {
    (d.dot(cam.right), d.dot(cam.up), -d.dot(cam.forward))
}

fn color32(c: [f32; 3], alpha: f32) -> Color32 {
    let g = |v: f32| (rmb_core::render::to_display(v / c.iter().cloned().fold(1.0, f32::max)).clamp(0.0, 1.0) * 255.0) as u8;
    Color32::from_rgba_unmultiplied(g(c[0]), g(c[1]), g(c[2]), (alpha * 255.0) as u8)
}

/// Spot cone rim point used as the angle handle.
fn rim_point(cam: &Camera, pos: V3, target: V3, angle: f64) -> V3 {
    let a = (target - pos).normalize();
    let len = (target - pos).length();
    let mut e = a.cross(cam.forward);
    if e.length() < 1e-6 {
        e = cam.right;
    }
    target + e.normalize() * (len * angle.clamp(0.1, 89.0).to_radians().tan())
}

/// Screen positions of the handles: the selected light's handles and the icons of all lights
/// (for selecting by click).
pub fn handles(scene: &Scene, selected: usize, proj: &Proj, rect: Rect) -> Vec<(Handle, Pos2)> {
    let mut out = Vec::new();
    let (dc, dr) = dial(rect);
    for (i, l) in scene.lighting.lights.iter().enumerate() {
        if l.kind == LightKind::Directional {
            let (x, y, _) = dial_coords(&proj.cam, l.direction(&proj.cam));
            out.push((Handle::Dial(i), dc + vec2(x as f32 * dr, -y as f32 * dr)));
            continue;
        }
        if let Some(s) = proj.screen(l.position) {
            out.push((Handle::Position(i), s));
        }
        if i != selected {
            continue;
        }
        let len = ARROW_PX * proj.world_per_px(l.position);
        for (a, (e, _, _)) in AXES.iter().enumerate() {
            out.extend(proj.screen(l.position + *e * len).map(|s| (Handle::Axis(i, a), s)));
        }
        match l.kind {
            LightKind::Spot => {
                out.extend(proj.screen(l.target).map(|s| (Handle::Target(i), s)));
                out.extend(proj.screen(rim_point(&proj.cam, l.position, l.target, l.cone_angle)).map(|s| (Handle::Rim(i), s)));
            }
            LightKind::Area => {
                let (_, u, v) = rmb_core::render::area_axes(l);
                out.extend(proj.screen(l.target).map(|s| (Handle::Target(i), s)));
                out.extend(proj.screen(l.position + u + v).map(|s| (Handle::Corner(i), s)));
            }
            LightKind::Line => out.extend(proj.screen(l.target).map(|s| (Handle::Target(i), s))),
            _ => {}
        }
    }
    out
}

/// The handle under `p` (the selected object's and light's handles first).
pub fn hit(handles: &[(Handle, Pos2)], p: Pos2, selected: usize) -> Option<Handle> {
    let other = |h: &Handle| object_of(*h).is_none() && light_of(*h) != selected;
    handles
        .iter()
        .filter(|(_, s)| s.distance(p) < 10.0)
        .min_by(|a, b| {
            let ka = (other(&a.0), a.1.distance(p));
            let kb = (other(&b.0), b.1.distance(p));
            ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(h, _)| *h)
}

/// The light a handle belongs to (`usize::MAX` for object handles).
pub fn light_of(h: Handle) -> usize {
    match h {
        Handle::Position(i) | Handle::Target(i) | Handle::Rim(i) | Handle::Corner(i) | Handle::Dial(i) | Handle::Axis(i, _) => i,
        Handle::ObjCenter(_) | Handle::ObjAxis(..) => usize::MAX,
    }
}

/// The composed object a handle belongs to.
pub fn object_of(h: Handle) -> Option<usize> {
    match h {
        Handle::ObjCenter(k) | Handle::ObjAxis(k, _) => Some(k),
        _ => None,
    }
}

/// Handles of composed object `k`: its center and the move arrows.
pub fn object_handles(scene: &Scene, k: usize, proj: &Proj) -> Vec<(Handle, Pos2)> {
    let Some(o) = scene.objects.get(k) else { return Vec::new() };
    let p = o.transform.position;
    let mut out: Vec<(Handle, Pos2)> = proj.screen(p).map(|s| (Handle::ObjCenter(k), s)).into_iter().collect();
    let len = ARROW_PX * proj.world_per_px(p);
    for (a, (e, _, _)) in AXES.iter().enumerate() {
        out.extend(proj.screen(p + *e * len).map(|s| (Handle::ObjAxis(k, a), s)));
    }
    out
}

/// `p` moved along world axis `a` by the part of the mouse move in that axis's screen direction.
fn along_axis(proj: &Proj, p: V3, a: usize, delta: Vec2) -> V3 {
    let e = AXES[a].0;
    let len = ARROW_PX * proj.world_per_px(p);
    match (proj.screen(p), proj.screen(p + e * len)) {
        (Some(p0), Some(p1)) if (p1 - p0).length_sq() > 1.0 => {
            let sd = p1 - p0;
            p + e * (len * (delta.dot(sd) / sd.length_sq()) as f64)
        }
        _ => p,
    }
}

/// Applies a drag of `h` by `delta` (pointer now at `pointer`).
pub fn apply(h: Handle, scene: &mut Scene, proj: &Proj, rect: Rect, pointer: Pos2, delta: Vec2, depth: bool) {
    let cam = proj.cam.clone();
    if let Some(k) = object_of(h) {
        let Some(o) = scene.objects.get_mut(k) else { return };
        let p = o.transform.position;
        o.transform.position = match h {
            Handle::ObjAxis(_, a) => along_axis(proj, p, a, delta),
            _ => proj.drag(p, delta, depth),
        };
        return;
    }
    let Some(l) = scene.lighting.lights.get_mut(light_of(h)) else { return };
    match h {
        Handle::Position(_) => {
            let p = proj.drag(l.position, delta, depth);
            // Spot and area lights keep aiming at the same point; a line moves as a whole.
            if l.kind == LightKind::Line {
                l.target += p - l.position;
            }
            l.position = p;
        }
        Handle::Target(_) => l.target = proj.drag(l.target, delta, depth),
        Handle::Axis(_, a) => {
            let mv = along_axis(proj, l.position, a, delta) - l.position;
            l.position += mv;
            if l.kind == LightKind::Line {
                l.target += mv;
            }
        }
        Handle::ObjCenter(_) | Handle::ObjAxis(..) => {}
        Handle::Rim(_) => {
            let q = proj.drag(rim_point(&cam, l.position, l.target, l.cone_angle), delta, false);
            let a = (l.target - l.position).normalize();
            let along = (q - l.position).dot(a);
            let off = (q - l.position - a * along).length();
            if along > 0.0 {
                l.cone_angle = off.atan2(along).to_degrees().clamp(0.5, 89.0);
            }
        }
        Handle::Corner(_) => {
            let (_, u, v) = rmb_core::render::area_axes(l);
            let q = proj.drag(l.position + u + v, delta, false) - l.position;
            let (uh, vh) = (u.normalize(), v.normalize());
            l.width = (2.0 * q.dot(uh).abs()).max(1e-6);
            l.height = (2.0 * q.dot(vh).abs()).max(1e-6);
        }
        Handle::Dial(_) => {
            let (c, r) = dial(rect);
            let (mut x, mut y) = (((pointer.x - c.x) / r) as f64, (-(pointer.y - c.y) / r) as f64);
            let len = (x * x + y * y).sqrt();
            if len > 1.0 {
                x /= len;
                y /= len;
            }
            let (_, _, z0) = dial_coords(&cam, l.direction(&cam));
            let z = (1.0 - x * x - y * y).max(0.0).sqrt() * if z0 < 0.0 { -1.0 } else { 1.0 };
            if l.camera_relative {
                l.elevation = y.clamp(-1.0, 1.0).asin().to_degrees();
                l.azimuth = x.atan2(z).to_degrees();
            } else {
                let d = cam.right * x + cam.up * y - cam.forward * z;
                l.elevation = d.z.clamp(-1.0, 1.0).asin().to_degrees();
                l.azimuth = d.y.atan2(d.x).to_degrees();
            }
        }
    }
}

/// Draws all light gizmos; `hover` is the handle under the pointer.
pub fn draw(painter: &Painter, scene: &Scene, selected: usize, proj: &Proj, rect: Rect, hover: Option<Handle>) {
    let painter = painter.with_clip_rect(rect);
    let cam = &proj.cam;
    let lights = &scene.lighting.lights;
    // Directional lights on the dial.
    if lights.iter().any(|l| l.kind == LightKind::Directional) {
        let (c, r) = dial(rect);
        painter.circle(c, r, Color32::from_black_alpha(110), Stroke::new(1.0, Color32::from_white_alpha(70)));
        painter.line_segment([c - vec2(r, 0.0), c + vec2(r, 0.0)], Stroke::new(1.0, Color32::from_white_alpha(30)));
        painter.line_segment([c - vec2(0.0, r), c + vec2(0.0, r)], Stroke::new(1.0, Color32::from_white_alpha(30)));
        for (i, l) in lights.iter().enumerate().filter(|(_, l)| l.kind == LightKind::Directional) {
            let (x, y, z) = dial_coords(cam, l.direction(cam));
            let p = c + vec2(x as f32 * r, -y as f32 * r);
            let col = color32(l.color, if l.enabled { 1.0 } else { 0.35 });
            let sel = i == selected;
            let rad = if hover == Some(Handle::Dial(i)) { 7.0 } else if sel { 6.0 } else { 4.5 };
            let ring = Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { theme::ACCENT } else { Color32::BLACK });
            if z >= 0.0 {
                painter.circle(p, rad, col, ring);
            } else {
                painter.circle_stroke(p, rad, Stroke::new(2.0, col));
                painter.circle_stroke(p, rad + 1.5, ring);
            }
            if sel {
                painter.line_segment([c, p], Stroke::new(1.0, col));
            }
        }
    }
    for (i, l) in lights.iter().enumerate() {
        if l.kind == LightKind::Directional {
            continue;
        }
        let sel = i == selected;
        let alpha = if l.enabled { 1.0 } else { 0.35 };
        let col = color32(l.color, alpha);
        let wire = Stroke::new(if sel { 1.6 } else { 1.0 }, color32(l.color, alpha * if sel { 0.95 } else { 0.6 }));
        let seg = |a: V3, b: V3, s: Stroke| {
            if let (Some(x), Some(y)) = (proj.screen(a), proj.screen(b)) {
                painter.line_segment([x, y], s);
            }
        };
        match l.kind {
            LightKind::Spot => {
                let a = (l.target - l.position).normalize();
                let len = (l.target - l.position).length();
                let rad = len * l.cone_angle.clamp(0.1, 89.0).to_radians().tan();
                let e1 = if a.z.abs() < 0.99 { a.cross(v3(0.0, 0.0, 1.0)) } else { a.cross(v3(1.0, 0.0, 0.0)) }.normalize();
                let e2 = a.cross(e1);
                let ring: Vec<V3> = (0..=32).map(|k| {
                    let w = k as f64 / 32.0 * std::f64::consts::TAU;
                    l.target + e1 * (rad * w.cos()) + e2 * (rad * w.sin())
                })
                .collect();
                for w in ring.windows(2) {
                    seg(w[0], w[1], wire);
                }
                for k in [0, 8, 16, 24] {
                    seg(l.position, ring[k], wire);
                }
                seg(l.position, l.target, Stroke::new(1.0, Color32::from_white_alpha(90)));
            }
            LightKind::Area => {
                let (n, u, v) = rmb_core::render::area_axes(l);
                let c = l.position;
                let corners = [c + u + v, c - u + v, c - u - v, c + u - v];
                let pts: Vec<Pos2> = corners.iter().filter_map(|p| proj.screen(*p)).collect();
                if pts.len() == 4 {
                    // Lit side toward the camera: filled; the back side only outlined.
                    let front = n.dot(cam.position - c) > 0.0;
                    let fill = if front { color32(l.color, 0.35 * alpha) } else { Color32::TRANSPARENT };
                    painter.add(egui::Shape::convex_polygon(pts.clone(), fill, wire));
                }
                let reach = u.length().max(v.length()).max(1e-6) * 1.2;
                seg(c, c + n * reach, wire);
                seg(c, l.target, Stroke::new(1.0, Color32::from_white_alpha(60)));
            }
            LightKind::Line => {
                seg(l.position, l.target, Stroke::new(if sel { 4.0 } else { 3.0 }, col));
            }
            _ => {}
        }
        // Icon at the position.
        if let Some(p) = proj.screen(l.position) {
            let r = if hover == Some(Handle::Position(i)) { 8.0 } else { 6.0 };
            for k in 0..8 {
                let w = k as f32 * std::f32::consts::TAU / 8.0;
                let dir = vec2(w.cos(), w.sin());
                painter.line_segment([p + dir * (r + 2.0), p + dir * (r + 6.0)], Stroke::new(1.5, col));
            }
            painter.circle(p, r, col, Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { theme::ACCENT } else { Color32::BLACK }));
        }
    }
    // Selected positional light: a dashed line down to the ground (z = 0) with a mark there,
    // and the move arrows along the world axes.
    if let Some(l) = lights.get(selected).filter(|l| l.kind.positional()) {
        drop_line(&painter, proj, l.position);
        let hot = match hover {
            Some(Handle::Axis(i, a)) if i == selected => Some(a),
            _ => None,
        };
        move_arrows(&painter, proj, l.position, hot);
    }
    // Handles of the selected light.
    for (h, p) in handles(scene, selected, proj, rect) {
        if light_of(h) != selected || matches!(h, Handle::Position(_) | Handle::Dial(_) | Handle::Axis(..)) {
            continue;
        }
        let r = if hover == Some(h) { 7.0 } else { 5.0 };
        match h {
            Handle::Target(_) => {
                let pts = vec![p + vec2(0.0, -r), p + vec2(r, 0.0), p + vec2(0.0, r), p + vec2(-r, 0.0)];
                painter.add(egui::Shape::convex_polygon(pts, Color32::WHITE, Stroke::new(1.0, Color32::BLACK)));
            }
            _ => {
                painter.rect_filled(Rect::from_center_size(p, vec2(r * 1.6, r * 1.6)), 1.0, Color32::WHITE);
                painter.rect_stroke(Rect::from_center_size(p, vec2(r * 1.6, r * 1.6)), 1.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
            }
        }
    }
}

/// A dashed line from `p` down to the ground (z = 0) with a ring there.
fn drop_line(painter: &Painter, proj: &Proj, p: V3) {
    let ground = v3(p.x, p.y, 0.0);
    if let Some((a, b)) = proj.segment(p, ground) {
        let dash = Stroke::new(1.0, Color32::from_white_alpha(140));
        painter.add(egui::Shape::dashed_line(&[a, b], dash, 5.0, 4.0));
        let r = 0.04 * proj.world_per_px(ground) * 150.0;
        let ring: Vec<Pos2> = (0..=24)
            .filter_map(|k| {
                let w = k as f64 / 24.0 * std::f64::consts::TAU;
                proj.screen(ground + v3(r * w.cos(), r * w.sin(), 0.0))
            })
            .collect();
        painter.add(egui::Shape::line(ring, Stroke::new(1.2, Color32::from_white_alpha(160))));
    }
}

/// The x/y/z move arrows at `p` (`hot`: the hovered one).
fn move_arrows(painter: &Painter, proj: &Proj, p: V3, hot: Option<usize>) {
    let len = ARROW_PX * proj.world_per_px(p);
    let Some(p0) = proj.screen(p) else { return };
    for (a, (e, col, name)) in AXES.iter().enumerate() {
        let Some(p1) = proj.screen(p + *e * len) else { continue };
        let hot = hot == Some(a);
        let w = if hot { 3.5 } else { 2.0 };
        painter.line_segment([p0, p1], Stroke::new(w, *col));
        let dir = (p1 - p0).normalized();
        let side = vec2(-dir.y, dir.x);
        let head = if hot { 11.0 } else { 9.0 };
        painter.add(egui::Shape::convex_polygon(vec![p1 + dir * head, p1 + side * head * 0.45, p1 - side * head * 0.45], *col, Stroke::NONE));
        painter.text(p1 + dir * (head + 7.0), egui::Align2::CENTER_CENTER, *name, egui::FontId::proportional(11.0), *col);
    }
}

/// Draws the move gizmo of composed object `k`.
pub fn draw_object(painter: &Painter, scene: &Scene, k: usize, proj: &Proj, rect: Rect, hover: Option<Handle>) {
    let Some(o) = scene.objects.get(k) else { return };
    let painter = painter.with_clip_rect(rect);
    let p = o.transform.position;
    drop_line(&painter, proj, p);
    let hot = match hover {
        Some(Handle::ObjAxis(i, a)) if i == k => Some(a),
        _ => None,
    };
    move_arrows(&painter, proj, p, hot);
    if let Some(s) = proj.screen(p) {
        let r = if hover == Some(Handle::ObjCenter(k)) { 7.0 } else { 5.5 };
        painter.rect_filled(Rect::from_center_size(s, vec2(r * 2.0, r * 2.0)), 2.0, theme::ACCENT);
        painter.rect_stroke(Rect::from_center_size(s, vec2(r * 2.0, r * 2.0)), 2.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
        painter.text(s + vec2(r + 6.0, -r - 4.0), egui::Align2::LEFT_BOTTOM, &o.name, egui::FontId::proportional(11.0), theme::TEXT);
    }
}

/// Hover text for a handle.
pub fn tip(h: Handle, scene: &Scene) -> String {
    if let Some(k) = object_of(h) {
        let name = scene.objects.get(k).map(|o| o.name.as_str()).unwrap_or("");
        return match h {
            Handle::ObjAxis(_, a) => format!("{name}: drag to move along {}", AXES[a].2),
            _ => format!("{name}: drag to move, Shift: toward or away from the camera"),
        };
    }
    let kind = scene.lighting.lights.get(light_of(h)).map(|l| l.kind).unwrap_or_default();
    let n = light_of(h) + 1;
    match h {
        Handle::Position(_) => format!("Light {n} ({}): drag to move, Shift: toward or away from the camera", kind.label()),
        Handle::Target(_) if kind == LightKind::Line => format!("Light {n}: other end of the line"),
        Handle::Target(_) => format!("Light {n}: the point it faces (drag; Shift: depth)"),
        Handle::Rim(_) => format!("Light {n}: drag to widen or narrow the cone"),
        Handle::Corner(_) => format!("Light {n}: drag to resize the rectangle"),
        Handle::Dial(_) => format!("Light {n} (directional): drag to turn it; filled = shines from the camera's side"),
        Handle::Axis(_, a) => format!("Light {n}: drag to move along {}", AXES[a].2),
        Handle::ObjCenter(_) | Handle::ObjAxis(..) => String::new(),
    }
}

/// The light's position as text (shown while dragging).
pub fn position_text(h: Handle, scene: &Scene) -> Option<String> {
    if let Some(k) = object_of(h) {
        let p = scene.objects.get(k)?.transform.position;
        return Some(format!("x {:.3}  y {:.3}  z {:.3}", p.x, p.y, p.z));
    }
    let l = scene.lighting.lights.get(light_of(h))?;
    match h {
        Handle::Position(_) | Handle::Axis(..) if l.kind.positional() => Some(format!("x {:.3}  y {:.3}  z {:.3}", l.position.x, l.position.y, l.position.z)),
        Handle::Target(_) => Some(format!("x {:.3}  y {:.3}  z {:.3}", l.target.x, l.target.y, l.target.z)),
        Handle::Rim(_) => Some(format!("cone {:.1}°", l.cone_angle)),
        Handle::Corner(_) => Some(format!("{:.3} × {:.3}", l.width, l.height)),
        Handle::Dial(_) => Some(format!("azimuth {:.1}°  elevation {:.1}°", l.azimuth, l.elevation)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Orientation guides: ground grid with the axes, and the axis cross in the corner.

/// Draws the ground grid (plane z = 0) around the camera's spot on it, the x and y axes on
/// it and the z axis through the origin.
pub fn draw_grid(painter: &Painter, proj: &Proj, rect: Rect) {
    let painter = painter.with_clip_rect(rect);
    let cam = &proj.cam;
    // Spacing by the camera's height above the plane (or its distance to the origin).
    let h = cam.position.z.abs().max(cam.position.length() * 0.05).max(1e-9);
    let unit = 10f64.powf(h.log10().round());
    let minor = unit / 10.0;
    let (cx, cy) = ((cam.position.x / unit).round() * unit, (cam.position.y / unit).round() * unit);
    let line = |a: V3, b: V3, alpha: u8, w: f32, col: Color32| {
        if let Some((p, q)) = proj.segment(a, b) {
            painter.line_segment([p, q], Stroke::new(w, col.gamma_multiply(alpha as f32 / 255.0)));
        }
    };
    let gray = Color32::from_gray(200);
    // Fine lines near the camera, unit lines farther out.
    for (step, n, alpha) in [(minor, 30i64, 40u8), (unit, 15, 70)] {
        let ext = step * n as f64;
        for k in -n..=n {
            let off = k as f64 * step;
            let x = cx + off;
            let y = cy + off;
            if step == minor && (x / unit - (x / unit).round()).abs() < 1e-6 {
                continue;
            }
            let fade = (1.0 - (k.abs() as f64 / n as f64)).powf(0.7);
            let a = (alpha as f64 * fade) as u8;
            if x.abs() > 1e-9 * unit {
                line(v3(x, cy - ext, 0.0), v3(x, cy + ext, 0.0), a, 1.0, gray);
            }
            if y.abs() > 1e-9 * unit {
                line(v3(cx - ext, y, 0.0), v3(cx + ext, y, 0.0), a, 1.0, gray);
            }
        }
    }
    let far = unit * 15.0;
    line(v3(cx - far, 0.0, 0.0), v3(cx + far, 0.0, 0.0), 200, 1.5, AXES[0].1);
    line(v3(0.0, cy - far, 0.0), v3(0.0, cy + far, 0.0), 200, 1.5, AXES[1].1);
    line(v3(0.0, 0.0, -far * 0.2), v3(0.0, 0.0, far * 0.2), 160, 1.5, AXES[2].1);
}

/// Center and radius of the axis cross (bottom right of the view).
fn triad_area(rect: Rect) -> (Pos2, f32) {
    let r = 30.0;
    (pos2(rect.right() - r - 18.0, rect.bottom() - r - 18.0), r)
}

/// Screen tips of the axis cross: (axis index, sign, position).
fn triad_tips(cam: &Camera, rect: Rect) -> Vec<(usize, f64, Pos2)> {
    let (c, r) = triad_area(rect);
    let mut tips = Vec::new();
    for (a, (e, _, _)) in AXES.iter().enumerate() {
        for sign in [1.0, -1.0] {
            let d = *e * sign;
            tips.push((a, sign, c + vec2(d.dot(cam.right) as f32 * r, -(d.dot(cam.up) as f32) * r)));
        }
    }
    tips
}

/// Draws the axis cross: where the world axes point from this view (the labelled ends are the
/// positive directions; click one to look along it).
pub fn draw_triad(painter: &Painter, cam: &Camera, rect: Rect, hover: Option<(usize, f64)>) {
    let (c, r) = triad_area(rect);
    painter.circle_filled(c, r + 12.0, Color32::from_black_alpha(90));
    // Far ends first, so the near ones are drawn on top.
    let mut tips = triad_tips(cam, rect);
    tips.sort_by(|a, b| {
        let da = (AXES[a.0].0 * a.1).dot(cam.forward);
        let db = (AXES[b.0].0 * b.1).dot(cam.forward);
        db.total_cmp(&da)
    });
    for (a, sign, p) in tips {
        let col = AXES[a].1;
        let hot = hover == Some((a, sign));
        if sign > 0.0 {
            painter.line_segment([c, p], Stroke::new(2.0, col));
            painter.circle(p, if hot { 9.0 } else { 7.5 }, col, Stroke::new(if hot { 2.0 } else { 0.0 }, Color32::WHITE));
            painter.text(p, egui::Align2::CENTER_CENTER, AXES[a].2, egui::FontId::proportional(10.0), Color32::BLACK);
        } else {
            painter.circle(p, if hot { 7.0 } else { 5.0 }, col.gamma_multiply(0.35), Stroke::new(1.0, col));
        }
    }
}

/// The axis end of the cross under `p`: (axis index, sign).
pub fn triad_hit(cam: &Camera, rect: Rect, p: Pos2) -> Option<(usize, f64)> {
    triad_tips(cam, rect).into_iter().filter(|t| t.2.distance(p) < 10.0).min_by(|a, b| a.2.distance(p).total_cmp(&b.2.distance(p))).map(|(a, s, _)| (a, s))
}

/// Points the camera along an axis: from the `sign` side of axis `a` toward `target`, at the
/// same distance.
pub fn look_along(cam: &mut Camera, a: usize, sign: f64, target: V3) {
    let dist = (cam.position - target).length().max(1e-9);
    let e = AXES[a].0 * sign;
    cam.position = target + e * dist;
    // Top and bottom views keep +y up on the screen, the others +z.
    let up = if a == 2 { v3(0.0, 1.0, 0.0) } else { v3(0.0, 0.0, 1.0) };
    cam.look_at(target, up);
}
