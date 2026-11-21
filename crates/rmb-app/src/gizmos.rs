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
}

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

/// The handle under `p` (selected light's handles first).
pub fn hit(handles: &[(Handle, Pos2)], p: Pos2, selected: usize) -> Option<Handle> {
    let light = |h: &Handle| match *h {
        Handle::Position(i) | Handle::Target(i) | Handle::Rim(i) | Handle::Corner(i) | Handle::Dial(i) => i,
    };
    handles
        .iter()
        .filter(|(_, s)| s.distance(p) < 10.0)
        .min_by(|a, b| {
            let ka = (light(&a.0) != selected, a.1.distance(p));
            let kb = (light(&b.0) != selected, b.1.distance(p));
            ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(h, _)| *h)
}

/// The light a handle belongs to.
pub fn light_of(h: Handle) -> usize {
    match h {
        Handle::Position(i) | Handle::Target(i) | Handle::Rim(i) | Handle::Corner(i) | Handle::Dial(i) => i,
    }
}

/// Applies a drag of `h` by `delta` (pointer now at `pointer`).
pub fn apply(h: Handle, scene: &mut Scene, proj: &Proj, rect: Rect, pointer: Pos2, delta: Vec2, depth: bool) {
    let cam = proj.cam.clone();
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
    // Handles of the selected light.
    for (h, p) in handles(scene, selected, proj, rect) {
        if light_of(h) != selected || matches!(h, Handle::Position(_) | Handle::Dial(_)) {
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

/// Hover text for a handle.
pub fn tip(h: Handle, scene: &Scene) -> String {
    let kind = scene.lighting.lights.get(light_of(h)).map(|l| l.kind).unwrap_or_default();
    let n = light_of(h) + 1;
    match h {
        Handle::Position(_) => format!("Light {n} ({}): drag to move, Shift: toward or away from the camera", kind.label()),
        Handle::Target(_) if kind == LightKind::Line => format!("Light {n}: other end of the line"),
        Handle::Target(_) => format!("Light {n}: the point it faces (drag; Shift: depth)"),
        Handle::Rim(_) => format!("Light {n}: drag to widen or narrow the cone"),
        Handle::Corner(_) => format!("Light {n}: drag to resize the rectangle"),
        Handle::Dial(_) => format!("Light {n} (directional): drag to turn it; filled = shines from the camera's side"),
    }
}
