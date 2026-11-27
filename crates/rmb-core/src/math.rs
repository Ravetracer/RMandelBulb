// SPDX-License-Identifier: LGPL-2.1-or-later

//! Minimal 3D vector math in f64.

use std::ops::{Add, AddAssign, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

pub const fn v3(x: f64, y: f64, z: f64) -> V3 {
    V3 { x, y, z }
}

impl V3 {
    pub const ZERO: V3 = v3(0.0, 0.0, 0.0);

    pub fn dot(self, o: V3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: V3) -> V3 {
        v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn normalize(self) -> V3 {
        let l = self.length();
        if l > 0.0 { self * (1.0 / l) } else { v3(0.0, 0.0, 1.0) }
    }

    pub fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    pub fn from_array(a: [f64; 3]) -> V3 {
        v3(a[0], a[1], a[2])
    }

    pub fn to_f32(self) -> [f32; 3] {
        [self.x as f32, self.y as f32, self.z as f32]
    }

    pub fn mul_elem(self, o: V3) -> V3 {
        v3(self.x * o.x, self.y * o.y, self.z * o.z)
    }

    pub fn max_elem(self) -> f64 {
        self.x.max(self.y).max(self.z)
    }

    /// Rotates `self` about the unit axis `k` by `angle` radians (Rodrigues).
    pub fn rotate(self, k: V3, angle: f64) -> V3 {
        let (s, c) = angle.sin_cos();
        self * c + k.cross(self) * s + k * (k.dot(self) * (1.0 - c))
    }
}

impl From<[f64; 3]> for V3 {
    fn from(a: [f64; 3]) -> V3 {
        v3(a[0], a[1], a[2])
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl AddAssign for V3 {
    fn add_assign(&mut self, o: V3) {
        *self = *self + o;
    }
}

impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f64> for V3 {
    type Output = V3;
    fn mul(self, s: f64) -> V3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        v3(-self.x, -self.y, -self.z)
    }
}

/// Integer hash (PCG) shared with the GPU kernels: same jitter on CPU and GPU.
pub fn pcg(v: u32) -> u32 {
    let state = v.wrapping_mul(747796405).wrapping_add(2891336453);
    let word = ((state >> ((state >> 28).wrapping_add(4))) ^ state).wrapping_mul(277803737);
    (word >> 22) ^ word
}

/// Uniform float in [0, 1) from a hash.
pub fn unit(h: u32) -> f64 {
    (h >> 8) as f64 / 16_777_216.0
}

/// Unit quaternion (rotation) in f64.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Quat {
    pub w: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Default for Quat {
    fn default() -> Self {
        Quat::IDENTITY
    }
}

impl Quat {
    pub const IDENTITY: Quat = Quat { w: 1.0, x: 0.0, y: 0.0, z: 0.0 };

    /// Rotation that takes the x, y, z axes to the orthonormal right-handed basis `a`, `b`, `c`.
    pub fn from_basis(a: V3, b: V3, c: V3) -> Quat {
        let (m00, m11, m22) = (a.x, b.y, c.z);
        let tr = m00 + m11 + m22;
        let q = if tr > 0.0 {
            let s = (tr + 1.0).sqrt() * 2.0;
            Quat { w: 0.25 * s, x: (b.z - c.y) / s, y: (c.x - a.z) / s, z: (a.y - b.x) / s }
        } else if m00 > m11 && m00 > m22 {
            let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
            Quat { w: (b.z - c.y) / s, x: 0.25 * s, y: (b.x + a.y) / s, z: (c.x + a.z) / s }
        } else if m11 > m22 {
            let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
            Quat { w: (c.x - a.z) / s, x: (b.x + a.y) / s, y: 0.25 * s, z: (c.y + b.z) / s }
        } else {
            let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
            Quat { w: (a.y - b.x) / s, x: (c.x + a.z) / s, y: (c.y + b.z) / s, z: 0.25 * s }
        };
        q.normalize()
    }

    /// The images of the x, y, z axes.
    pub fn basis(self) -> (V3, V3, V3) {
        (self.rotate(v3(1.0, 0.0, 0.0)), self.rotate(v3(0.0, 1.0, 0.0)), self.rotate(v3(0.0, 0.0, 1.0)))
    }

    /// Rotation by `angle` radians about the unit axis `k`.
    pub fn from_axis_angle(k: V3, angle: f64) -> Quat {
        let (s, c) = (angle * 0.5).sin_cos();
        Quat { w: c, x: k.x * s, y: k.y * s, z: k.z * s }
    }

    pub fn dot(self, o: Quat) -> f64 {
        self.w * o.w + self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn negated(self) -> Quat {
        Quat { w: -self.w, x: -self.x, y: -self.y, z: -self.z }
    }

    pub fn conj(self) -> Quat {
        Quat { w: self.w, x: -self.x, y: -self.y, z: -self.z }
    }

    pub fn scale(self, s: f64) -> Quat {
        Quat { w: self.w * s, x: self.x * s, y: self.y * s, z: self.z * s }
    }

    pub fn plus(self, o: Quat) -> Quat {
        Quat { w: self.w + o.w, x: self.x + o.x, y: self.y + o.y, z: self.z + o.z }
    }

    pub fn normalize(self) -> Quat {
        let l = self.dot(self).sqrt();
        if l > 0.0 { self.scale(1.0 / l) } else { Quat::IDENTITY }
    }

    /// Hamilton product (`self` after `o`).
    pub fn times(self, o: Quat) -> Quat {
        Quat {
            w: self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
            x: self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            y: self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            z: self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
        }
    }

    pub fn rotate(self, v: V3) -> V3 {
        let u = v3(self.x, self.y, self.z);
        let t = u.cross(v) * 2.0;
        v + t * self.w + u.cross(t)
    }

    /// Logarithm of a unit quaternion (the rotation vector halved).
    pub fn log(self) -> V3 {
        let u = v3(self.x, self.y, self.z);
        let s = u.length();
        if s < 1e-12 {
            return u;
        }
        u * (s.atan2(self.w) / s)
    }

    pub fn exp(v: V3) -> Quat {
        let a = v.length();
        if a < 1e-12 {
            return Quat { w: 1.0, x: v.x, y: v.y, z: v.z }.normalize();
        }
        let s = a.sin() / a;
        Quat { w: a.cos(), x: v.x * s, y: v.y * s, z: v.z * s }
    }

    /// Angle between two orientations in radians.
    pub fn angle_to(self, o: Quat) -> f64 {
        2.0 * self.dot(o).abs().clamp(0.0, 1.0).acos()
    }

    /// Spherical interpolation along the shorter way.
    pub fn slerp(self, o: Quat, t: f64) -> Quat {
        let o = if self.dot(o) < 0.0 { o.negated() } else { o };
        self.times(Quat::exp(self.conj().times(o).log() * t)).normalize()
    }
}
