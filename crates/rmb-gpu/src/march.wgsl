// SPDX-License-Identifier: LGPL-2.1-or-later
//
// Ray marching kernel. Mirrors rmb-core/src/render/cpu.rs step by step; keep them in sync.
// The ITERATE marker below is replaced by the generated hybrid iteration (codegen.rs).

alias fl = f32;

struct GLight {
    dir: vec4<f32>,   // xyz: direction, or the position of a point light; w: specular
    color: vec4<f32>, // w: point light
    opts: vec4<f32>,  // x: shadows, y: softness, z: diffuse curve, w: occlusion
    glow: vec4<f32>,  // visible light: x: shape + 1 (0 = not visible), y: squared radius, z: additive
    glow_col: vec4<f32>,
}

struct U {
    cam_pos: vec4<f32>,    // w: tan(fov / 2)
    cam_right: vec4<f32>,  // w: aspect
    cam_up: vec4<f32>,     // w: pixel angle
    cam_fwd: vec4<f32>,    // w: max distance
    size: vec4<u32>,       // width, height, sample, light count
    band: vec4<u32>,       // first row, rows, max steps, refine steps
    march: vec4<f32>,      // detail, step, bailout^2, DE scale
    frac: vec4<f32>,       // w slice, julia, max iterations, repeat from
    julia: vec4<f32>,
    ao: vec4<f32>,         // mode, strength, radius, ambient
    sky: vec4<f32>,        // w: shininess
    ground: vec4<f32>,     // w: material specular
    bg_top: vec4<f32>,     // w: background mode
    bg_horizon: vec4<f32>,
    bg_bottom: vec4<f32>,
    fog: vec4<f32>,        // w: density
    glow: vec4<f32>,       // w: amount
    dof: vec4<f32>,        // enabled, aperture, focus distance, fast preview
    color: vec4<f32>,      // mode, scale, offset, repeat
    color2: vec4<f32>,     // saturation, cut count, depth shift, depth reference
    cuts: array<vec4<f32>, 3>,   // axis, position, keep above
    slots: array<vec4<u32>, 8>,  // x: iterations
    lights: array<GLight, 6>,
    comb: vec4<f32>,       // DE combination: mode, max iterations 2, repeat from 2, smoothing
    scales: vec4<f32>,     // DE scale of chain 1, chain 2, inside rendering, ray start distance
    rot4: array<vec4<f32>, 4>, // 4D start rotation (rows)
    amb_up: vec4<f32>,     // xyz: ambient sky axis, w: vertical background curve
    fog2: vec4<f32>,       // fog mode, strength, start, curve
    fog3: vec4<f32>,       // squared, from background, color detail shift, reflection bounces
    glow2: vec4<f32>,      // second glow color, w: steps to the second color
    glow3: vec4<f32>,      // glow offset (steps), blend, glow iteration, reflection amount
    bg_rot: array<vec4<f32>, 3>, // background image rotation rows; [0].w: has image, [1].w: display space
    blend: vec4<f32>,      // interpolation hybrid weights of slots 1 and 2, smooth normals, roughness
    interior: vec4<f32>,   // interior colors: on, scale, offset; w: mix power of the DE combination
    cmap0: vec4<f32>,      // color map: mode + 1 (0 = off), scale, offset x, offset y
    cmap1: vec4<f32>,      // cos, sin of the 2D rotation, iteration scale, iteration offset
    cmap2: vec4<f32>,      // luminance only, average color
    cmap_rot: array<vec4<f32>, 3>, // rotation of the normal mode (rows)
    glass: vec4<f32>,      // transmission on, refraction index, absorption, scattering
    bg2: vec4<f32>,        // background image brightness, screen coordinates, ambient from it, fog added
}

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> prm: array<f32>;
// Surface colors at 0..255, highlight colors (w: transparency) at 256..511, rough diffuse tables at 512..639
// (one table per component), interior colors at 640..895 and their highlights at 896..1151.
@group(0) @binding(2) var<storage, read> lut: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> accum: array<vec4<f32>>;
// Denoiser helper images per pixel: [2 i] albedo, [2 i + 1] world normal (summed like accum).
@group(0) @binding(4) var<storage, read_write> aux: array<vec4<f32>>;

// Mandelbulb3D maps (rmb-core maps.rs): [0] count; per map i at 1 + 4 i: number, width,
// height, offset of its pixels (u32: blue | green << 8 | red << 16, top row first).
@group(0) @binding(5) var<storage, read> maps: array<u32>;

var<private> aux_albedo: vec3<f32>;
// Swaps inside and outside rendering (rays travelling inside glass; Evaluator::flip).
var<private> flip: bool;

fn inside_now() -> bool {
    return (u.scales.z > 0.5) != flip;
}
var<private> aux_normal: vec3<f32>;

struct Orbit {
    de: f32,
    n: u32,
    r2: f32,
    r2_old: f32,
    trap: f32,
    escaped: bool,
}

struct Iterated {
    s: vec4<f32>,
    rold: f32,
    dr: f32,
    n: u32,
    trap: f32,
    escaped: bool,
    // dIFS: smallest distance over the iterations.
    de: f32,
}

struct Hit {
    hit: bool,
    t: f32,
    eps: f32,
    steps: u32,
    glow_steps: u32,
    orbit: Orbit,
}

fn is_finite(v: f32) -> bool {
    return (bitcast<u32>(v) & 0x7fffffffu) < 0x7f800000u;
}

// Map lookups (mirror maps.rs): uniform cubic B-spline over the pixels with wrap-around.
fn map_find(n: f32) -> i32 {
    return map_find_u(u32(max(round(n), 0.0)));
}

// The background image is stored under the number 0xffffffff.
fn map_find_u(k: u32) -> i32 {
    let count = maps[0];
    for (var i = 0u; i < count; i++) {
        if (maps[1u + 4u * i] == k) {
            return i32(i);
        }
    }
    return -1;
}

fn spline_weights(t: f32) -> vec4<f32> {
    let w3 = t * t * t / 6.0;
    let w0 = 1.0 / 6.0 + 0.5 * t * (t - 1.0) - w3;
    let w2 = t + w0 - 2.0 * w3;
    return vec4<f32>(w0, 1.0 - w0 - w2 - w3, w2, w3);
}

fn map_sample(i: i32, x: f32, y: f32, ch: f32) -> f32 {
    let h0 = 1u + 4u * u32(i);
    let w = i32(maps[h0 + 1u]);
    let h = i32(maps[h0 + 2u]);
    let base = maps[h0 + 3u];
    let xf = floor(x);
    let yf = floor(y);
    let wx = spline_weights(x - xf);
    let wy = spline_weights(y - yf);
    let shift = u32(clamp(round(ch), 0.0, 2.0)) * 8u;
    var sum = 0.0;
    for (var j = 0; j < 4; j++) {
        let yy = (((i32(yf) - 1 + j) % h) + h) % h;
        var row = 0.0;
        for (var k = 0; k < 4; k++) {
            let xx = (((i32(xf) - 1 + k) % w) + w) % w;
            let p = maps[base + u32(yy * w + xx)];
            row += f32((p >> shift) & 255u) * wx[k];
        }
        sum += row * wy[j];
    }
    return sum / 255.0;
}

// RGB of map `i` at u, v (0..1): wrapped horizontally, clamped vertically (maps.rs
// sample_panorama; pixels are blue | green << 8 | red << 16).
fn map_panorama(i: i32, pu: f32, pv: f32) -> vec3<f32> {
    return map_clamped(i, pu, pv, true);
}

// Mirrors Map::sample_panorama (`wrap`) and Map::sample_clamped.
fn map_clamped(i: i32, pu: f32, pv: f32, wrap: bool) -> vec3<f32> {
    let h0 = 1u + 4u * u32(i);
    let w = i32(maps[h0 + 1u]);
    let h = i32(maps[h0 + 2u]);
    let base = maps[h0 + 3u];
    let x = clamp(pu, 0.0, 1.0) * f32(w);
    let y = clamp(pv, 0.0, 1.0) * f32(h);
    let xf = min(floor(x), f32(w - 1));
    let yf = min(floor(y), f32(h - 1));
    let wx = spline_weights(x - xf);
    let wy = spline_weights(y - yf);
    var sum = vec3<f32>(0.0);
    for (var j = 0; j < 4; j++) {
        let yy = clamp(i32(yf) - 1 + j, 0, h - 1);
        for (var k = 0; k < 4; k++) {
            var xx = clamp(i32(xf) - 1 + k, 0, w - 1);
            if (wrap) {
                xx = (((i32(xf) - 1 + k) % w) + w) % w;
            }
            let p = maps[base + u32(yy * w + xx)];
            sum += vec3<f32>(f32((p >> 16u) & 255u), f32((p >> 8u) & 255u), f32(p & 255u)) * (wx[k] * wy[j]);
        }
    }
    return sum / 255.0;
}

// RGB of map `i` at u, v, tiled in both directions (maps.rs rgb_tiled).
fn map_tiled(i: i32, pu: f32, pv: f32) -> vec3<f32> {
    let h0 = 1u + 4u * u32(i);
    let w = i32(maps[h0 + 1u]);
    let h = i32(maps[h0 + 2u]);
    let base = maps[h0 + 3u];
    let x = (pu - floor(pu)) * f32(w);
    let y = (pv - floor(pv)) * f32(h);
    let xf = floor(x);
    let yf = floor(y);
    let wx = spline_weights(x - xf);
    let wy = spline_weights(y - yf);
    var sum = vec3<f32>(0.0);
    for (var j = 0; j < 4; j++) {
        let yy = (((i32(yf) - 1 + j) % h) + h) % h;
        for (var k = 0; k < 4; k++) {
            let xx = (((i32(xf) - 1 + k) % w) + w) % w;
            let p = maps[base + u32(yy * w + xx)];
            sum += vec3<f32>(f32((p >> 16u) & 255u), f32((p >> 8u) & 255u), f32(p & 255u)) * (wx[k] * wy[j]);
        }
    }
    return sum / 255.0;
}

fn srgb_decode3(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

// Mirrors Shader::background_image (maps.rs: panorama_uv, sample_panorama).
fn background_image(d: vec3<f32>, sx: f32, sy: f32) -> vec3<f32> {
    let i = map_find_u(0xffffffffu);
    if (i < 0) {
        return vec3<f32>(0.0);
    }
    if (u.bg2.y > 0.5) {
        return image_rgb(map_clamped(i, (sx + 1.0) * 0.5, (1.0 - sy) * 0.5, false));
    }
    let v = panorama_dir(d);
    return image_rgb(map_panorama(i, atan2(v.x, v.z) * (-0.5 / 3.141592653589793) + 0.5, 0.5 - asin(clamp(v.y, -1.0, 1.0)) / 3.141592653589793));
}

// Mirrors Shader::background_ambient (the ambient copy is stored under 0xfffffffd).
fn background_ambient(n: vec3<f32>) -> vec3<f32> {
    let i = map_find_u(0xfffffffdu);
    if (i < 0) {
        return vec3<f32>(0.0);
    }
    let v = panorama_dir(n);
    return image_rgb(map_panorama(i, atan2(v.x, v.z) * (-0.5 / 3.141592653589793) + 0.5, 0.5 - asin(clamp(v.y, -1.0, 1.0)) / 3.141592653589793));
}

fn image_rgb(c: vec3<f32>) -> vec3<f32> {
    var rgb = c;
    if (u.bg_rot[1].w < 0.5) {
        rgb = srgb_decode3(rgb);
    }
    return rgb * u.bg2.x;
}

// Direction in the image's frame (mirrors maps::panorama_uv).
fn panorama_dir(d: vec3<f32>) -> vec3<f32> {
    let l = max(length(d), 1.0e-30);
    return (u.bg_rot[0].xyz * d.x + u.bg_rot[1].xyz * d.y + u.bg_rot[2].xyz * d.z) / l;
}

// Mirrors Shader::map_color (the color map is stored under the number 0xfffffffe).
fn map_color(h: Hit, pos: vec3<f32>, n: vec3<f32>, v: f32, base: vec3<f32>, rough: f32) -> vec3<f32> {
    let i = map_find_u(0xfffffffeu);
    if (i < 0) {
        return base;
    }
    let mode = u32(u.cmap0.x) - 1u;
    let scale = u.cmap0.y;
    let ox = u.cmap0.z;
    let oy = u.cmap0.w;
    var c = vec3<f32>(0.0);
    if (mode == 1u) {
        let r = u.cmap_rot[0].xyz * n.x + u.cmap_rot[1].xyz * n.y + u.cmap_rot[2].xyz * n.z;
        let l = max(length(r), 1.0e-30);
        let rv = r / l;
        c = map_panorama(i, atan2(rv.x, rv.z) * (-0.5 / 3.141592653589793) + 0.5, 0.5 - asin(clamp(rv.y, -1.0, 1.0)) / 3.141592653589793);
    } else {
        // Up to three tiled lookups (3D projection), from one call site: compile time.
        let p = pos * scale;
        let an = abs(n);
        let wv = an / max(an.x + an.y + an.z, 1.0e-30);
        var count = 1;
        if (mode == 3u) {
            count = 3;
        }
        for (var k = 0; k < count; k++) {
            var pu = 0.0;
            var pv = 0.0;
            var wk = 1.0;
            if (mode == 0u) {
                let a = clamp(h.orbit.trap * 4096.0, 0.0, 32767.0) / 32768.0 - 0.5;
                let b = clamp(v * u.cmap1.z + u.cmap1.w, 0.0, 1.0) - 0.5;
                let qu = (u.cmap1.x * a + u.cmap1.y * b + ox) * scale;
                let qv = (u.cmap1.x * b - u.cmap1.y * a + oy) * scale;
                pu = qu - floor(qu);
                pv = qv - floor(qv);
            } else if (mode == 2u) {
                let sx = sin(p.x);
                let sy = sin(p.y);
                let sz = sin(p.z);
                pu = sy * wv.x + sx * (wv.y + wv.z) + ox;
                pv = sz * (wv.x + wv.y) + sy * wv.z + oy;
            } else if (k == 0) {
                pu = p.y + ox;
                pv = p.z + oy;
                wk = wv.x;
            } else if (k == 1) {
                pu = p.x + ox;
                pv = p.z + oy;
                wk = wv.y;
            } else {
                pu = p.x + ox;
                pv = p.y + oy;
                wk = wv.z;
            }
            c += map_tiled(i, pu, pv) * wk;
        }
    }
    if (mode != 0u && rough > 0.0) {
        c = mix(c, u.cmap2.yzw, rough);
    }
    if (u.bg_rot[1].w < 0.5) {
        c = srgb_decode3(c);
    }
    if (u.cmap2.x > 0.5) {
        let yc = dot(c, vec3<f32>(0.3, 0.59, 0.11));
        let yb = dot(base, vec3<f32>(0.3, 0.59, 0.11));
        return base * (yc / (0.01 + yb));
    }
    return c;
}

fn map_xy(n: f32, u_: f32, v_: f32, ch: f32) -> f32 {
    let i = map_find(n);
    if (i < 0) {
        return 0.0;
    }
    let h0 = 1u + 4u * u32(i);
    return map_sample(i, (u_ - floor(u_)) * f32(maps[h0 + 1u]), (v_ - floor(v_)) * f32(maps[h0 + 2u]), ch);
}

fn map_sph(n: f32, x: f32, y: f32, z: f32, ch: f32) -> f32 {
    let i = map_find(n);
    if (i < 0) {
        return 0.0;
    }
    let h0 = 1u + 4u * u32(i);
    let l = sqrt(x * x + y * y + z * z);
    var d = vec3<f32>(0.0);
    if (l > 0.0) {
        d = vec3<f32>(x, y, z) / l;
    }
    let u_ = atan2(d.x, d.y) * (-0.5 / 3.141592653589793) + 0.5;
    let v_ = 0.5 - asin(clamp(d.z, -1.0, 1.0)) / 3.141592653589793;
    let mw = f32(maps[h0 + 1u]);
    let mh = f32(maps[h0 + 2u]);
    return map_sample(i, clamp(u_ * mw, 0.0, mw), clamp(v_ * mh, 0.0, mh), ch);
}

//@ITERATE@

// DE combination of two chains (mirrors fractal::combine).
fn combine_de(a: Orbit, b: Orbit) -> Orbit {
    let mode = u32(u.comb.x);
    let s = max(u.comb.w, 1.0e-12);
    var o = a;
    if (b.de < a.de) {
        o = b;
    }
    if (mode == 0u) {
        return o;
    }
    if (mode == 1u) {
        if (b.de > a.de) {
            return b;
        }
        return a;
    }
    if (mode == 2u) {
        var inv = -b.de;
        if (b.de <= 0.0) {
            inv = s;
        }
        var r = a;
        if (inv > a.de) {
            r.de = inv;
        }
        return r;
    }
    if (mode == 3u) {
        o.de = min(b.de - max(s - a.de, 0.0), a.de - max(s - b.de, 0.0));
        return o;
    }
    let x = max(s - a.de, 0.0);
    let y = max(s - b.de, 0.0);
    o.de = min(b.de - x * (s - y) / s, a.de - y * (s - x) / s);
    return o;
}

fn orbit(p: vec3<f32>, h: f32) -> Orbit {
    var o = fractal_de(p, h);
    var de = max(o.de * u.march.w, 0.0);
    if (inside_now()) {
        // Inside rendering (mirrors fractal.rs).
        let s = 2.0 * h;
        if (DIFS_1) {
            de = max(2.0 * s - de, 0.0);
        } else {
            de = max(4.0 * s - 3.0 * de, 0.0);
        }
    }
    let ncuts = u32(u.color2.y);
    for (var c = 0u; c < ncuts; c++) {
        let cp = u.cuts[c];
        let v = p[u32(cp.x)];
        de = max(de, select(v - cp.y, cp.y - v, cp.z > 0.5));
    }
    o.de = de;
    return o;
}

fn de_at(p: vec3<f32>, h: f32) -> f32 {
    return orbit(p, h).de;
}

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

var<private> rng_state: u32;

fn rnd() -> f32 {
    rng_state = pcg(rng_state);
    return f32(rng_state >> 8u) / 16777216.0;
}

fn eps_at(t: f32) -> f32 {
    return max(u.march.x * u.cam_up.w * t, 1.0e-7);
}

fn march_ray(o: vec3<f32>, d: vec3<f32>, t0: f32, max_t: f32, max_steps: u32) -> Hit {
    var h: Hit;
    h.hit = false;
    var t = t0;
    var last_step = 0.0;
    var step = 0u;
    var glow = 0u;
    let glow_it = u32(u.glow3.z);
    loop {
        if (step >= max_steps) {
            break;
        }
        let eps = eps_at(t);
        let ob = orbit(o + d * t, eps * 0.5);
        if (glow_it == 0u || ob.n == glow_it) {
            glow++;
        }
        if (ob.de < eps) {
            var t_hit = t;
            var ob_hit = ob;
            var dt = last_step * 0.5;
            if (step > 0u) {
                for (var k = 0u; k < u.band.w; k++) {
                    let tt = t_hit - dt;
                    let o2 = orbit(o + d * tt, eps_at(tt) * 0.5);
                    if (o2.de < eps_at(tt)) {
                        t_hit = tt;
                        ob_hit = o2;
                    }
                    dt *= 0.5;
                }
            }
            h.hit = true;
            h.t = t_hit;
            h.eps = eps_at(t_hit);
            h.orbit = ob_hit;
            h.steps = step;
            h.glow_steps = min(glow, step);
            return h;
        }
        last_step = max(ob.de * u.march.y, eps * 0.1);
        t += last_step;
        if (t > max_t) {
            h.steps = step;
            h.glow_steps = min(glow, step);
            return h;
        }
        step++;
    }
    h.steps = max_steps;
    h.glow_steps = glow;
    return h;
}

// Mirrors Shader::normal_rough: xyz normal, w roughness. All estimates come from one call
// site in one loop (each call site inlines the whole fractal: compile time).
fn surface_normal_rough(p: vec3<f32>, eps: f32, t: f32) -> vec4<f32> {
    let h = eps * 0.5;
    let s = i32(min(u.blend.z, 8.0));
    let right = u.cam_right.xyz;
    let up = u.cam_up.xyz;
    let no = 0.15 * min(u.march.x, 1.0) * u.cam_up.w * t;
    let no2 = no * 2.0;
    let step = no2 * 3.0 / (f32(max(s, 0)) + 0.5);
    // Samples: 4 tetrahedron points, then (with smoothing) 5 for the center value and 2 s along
    // each of the two tangents.
    var count = 4;
    if (s > 0) {
        count = 9 + 4 * s;
    }
    var g = vec3<f32>(0.0);
    var nrm = vec3<f32>(0.0, 0.0, 1.0);
    var tx = right;
    var ty = up;
    var dnn = 0.0;
    var n1 = 0.0;
    var s1 = 0.0;
    var n2 = 0.0;
    var s2 = 0.0;
    for (var j = 0; j < count; j++) {
        var q = p;
        var k = vec3<f32>(0.0);
        var i = 0;
        if (j < 4) {
            k = vec3<f32>(select(-1.0, 1.0, j == 0 || j == 3), select(-1.0, 1.0, j >= 2), select(-1.0, 1.0, j == 1 || j == 3));
            q = p + k * h;
        } else if (j < 9) {
            var offs = array<vec3<f32>, 5>(vec3<f32>(0.0), -right, right, -up, up);
            q = p + offs[j - 4] * no2;
        } else {
            let m = (j - 9) % (2 * s);
            i = select(m - s, m - s + 1, m >= s);
            q = p + select(ty, tx, j - 9 < 2 * s) * (f32(i) * step);
        }
        let v = de_at(q, h);
        if (j < 4) {
            g += k * v;
            if (j == 3) {
                if (length(g) > 0.0) {
                    nrm = g / length(g);
                }
                tx = right - nrm * dot(right, nrm);
                if (length(tx) > 1.0e-9) {
                    tx = normalize(tx);
                } else {
                    tx = up;
                }
                ty = cross(nrm, tx);
            }
        } else if (j < 9) {
            dnn += v * 0.2;
        } else {
            let a = (v - dnn) / f32(i);
            if (j - 9 < 2 * s) {
                n1 += a;
                s1 += a * a;
            } else {
                n2 += a;
                s2 += a * a;
            }
        }
    }
    if (s <= 0) {
        return vec4<f32>(nrm, 0.0);
    }
    let n0 = g * (no / (4.0 * h));
    let dm = 2.0 * f32(s);
    let spread = s1 * dm - n1 * n1 + s2 * dm - n2 * n2;
    let dt2 = no2 * 0.5 / (dm * step);
    let rough = clamp(sqrt(max(spread * 7.0 * dt2 * dt2 / (dot(n0, n0) + 1.0e-30), 0.0)) - 0.05, 0.0, 1.0);
    var n = n0;
    if (s < 8) {
        n = n0 + tx * (n1 * dt2) + ty * (n2 * dt2);
    }
    if (length(n) > 0.0) {
        n = n / length(n);
    } else {
        n = nrm;
    }
    return vec4<f32>(n, rough);
}

// Mirrors render::rough_diffuse.
fn rough_diffuse(k: u32, ndl: f32) -> f32 {
    let t = 62.0 - 60.0 * ndl;
    var ip = i32(trunc(t)) - 1;
    var f = t - trunc(t);
    if (ip < 0) {
        ip = 0;
        f = 0.0;
    } else if (ip > 124) {
        ip = 124;
        f = 1.0;
    }
    let w = spline_weights(f);
    var r = 0.0;
    for (var i = 0; i < 4; i++) {
        r += lut[512u + u32(ip + i)][min(k, 3u)] * w[i];
    }
    return r;
}

fn soft_shadow(p: vec3<f32>, l: vec3<f32>, eps: f32, softness: f32, max_t: f32) -> f32 {
    var res = 1.0;
    var t = eps * 4.0;
    for (var i = 0u; i < 160u; i++) {
        let e = eps_at(t) * 0.5 + eps * 0.5;
        let d = de_at(p + l * t, e);
        if (d < e) {
            return 0.0;
        }
        // Inside rendering: the estimate is a fixed step in the set, not a distance; only hits count.
        if (softness > 0.0 && !inside_now()) {
            res = min(res, d / (softness * t));
        }
        t += max(d * u.march.y, e);
        if (t > max_t) {
            break;
        }
    }
    let r = clamp(res, 0.0, 1.0);
    return r * r * (3.0 - 2.0 * r);
}

fn cosine_dir(n: vec3<f32>, u1: f32, u2: f32) -> vec3<f32> {
    var a = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(n.z) < 0.999) {
        a = vec3<f32>(0.0, 0.0, 1.0);
    }
    let t = normalize(cross(a, n));
    let b = cross(n, t);
    let r = sqrt(u1);
    let phi = u2 * 6.283185307179586;
    return normalize(t * (r * cos(phi)) + b * (r * sin(phi)) + n * sqrt(max(1.0 - u1, 0.0)));
}

// Mirrors cpu::ao_curve.
fn ao_curve(occ: f32) -> f32 {
    let s = u.ao.y;
    if (s <= 1.0) {
        return 1.0 - s * occ;
    }
    let d = 1.0 - occ;
    return max(d + (s - 1.0) * (d * d - d), 0.0);
}

fn ambient_occlusion(p: vec3<f32>, n: vec3<f32>, t: f32) -> f32 {
    let radius = u.ao.z * t;
    var mode = u32(u.ao.x);
    if (u.dof.w > 0.5 && mode == 2u) {
        mode = 1u;
    }
    if (mode == 0u) {
        return 1.0;
    }
    if (mode == 1u && inside_now()) {
        // The fast samples need distances, which inside rendering does not have.
        return 1.0;
    }
    if (mode == 1u) {
        var occ = 0.0;
        var w = 1.0;
        for (var k = 1u; k <= 5u; k++) {
            let h = radius * f32(k) / 5.0;
            let d = de_at(p + n * h, h * 0.1);
            occ += max(h - d, 0.0) / radius * w;
            w *= 0.6;
        }
        return ao_curve(clamp(occ / 0.959, 0.0, 1.0));
    }
    let u1 = rnd();
    let u2 = rnd();
    let d = cosine_dir(n, u1, u2);
    let eps = eps_at(t);
    let h = march_ray(p + n * (eps * 2.0), d, 0.0, radius, 48u);
    if (h.hit) {
        return ao_curve(clamp(1.0 - h.t / radius, 0.0, 1.0));
    }
    return 1.0;
}

fn background(d: vec3<f32>, sy: f32) -> vec3<f32> {
    if (u.bg_top.w < 0.5) {
        return u.bg_top.xyz;
    }
    if (u.bg_top.w > 1.5) {
        let s = pow(clamp((1.0 - sy) * 0.5, 0.0, 1.0), max(u.amb_up.w, 0.01));
        return mix(u.bg_top.xyz, u.bg_bottom.xyz, s);
    }
    var to = u.bg_top.xyz;
    var e = d.z;
    if (d.z < 0.0) {
        to = u.bg_bottom.xyz;
        e = -d.z;
    }
    let s = pow(clamp(e, 0.0, 1.0), 0.6);
    return mix(u.bg_horizon.xyz, to, s);
}

fn color_value(h: Hit, n: vec3<f32>, pos: vec3<f32>) -> f32 {
    let mode = u32(u.color.x);
    let o = h.orbit;
    if (mode == 0u) {
        let n_it = f32(o.n) + u.fog3.z * log(h.eps);
        if (o.escaped && o.r2 > 1.0) {
            let lb = log(sqrt(max(u.march.z, 4.0)));
            return n_it - log2(max((0.5 * log(o.r2)) / lb, 1.0e-10));
        }
        return n_it;
    }
    if (mode == 4u) {
        let n_it = f32(o.n) + u.fog3.z * log(h.eps);
        if (o.escaped && o.r2 > 1.0) {
            let lb = log(log(sqrt(max(u.march.z, 4.0))));
            let d = log(0.5 * log(o.r2));
            var dd = 0.6931471805599453;
            if (o.r2_old > 1.0) {
                dd = d - log(0.5 * log(o.r2_old));
            }
            return n_it + (lb - d) / max(dd, 1.0e-6);
        }
        return n_it;
    }
    if (mode == 1u) {
        return sqrt(max(o.trap, 0.0)) * 4.0;
    }
    if (mode == 2u) {
        return length(pos) * 4.0;
    }
    return n.z * 2.0 + 2.0;
}

struct Colors {
    base: vec3<f32>,
    spec: vec3<f32>,
    trans: f32,
}

fn gradient(v: f32, dist: f32) -> Colors {
    var t = v * u.color.y + u.color.z + u.color2.z * (dist - u.color2.w);
    if (u.color.w > 0.5) {
        t = t - floor(t);
    } else {
        t = clamp(t, 0.0, 1.0);
    }
    let x = t * 255.0;
    let i = min(u32(floor(x)), 254u);
    let f = x - f32(i);
    let base = mix(lut[i].xyz, lut[i + 1u].xyz, f);
    var c: Colors;
    c.base = mix(lut[0].xyz, base, u.color2.x);
    c.spec = mix(lut[i + 256u].xyz, lut[i + 257u].xyz, f);
    c.trans = mix(lut[i + 256u].w, lut[i + 257u].w, f);
    return c;
}

// Mirrors scene::GlowShape::shape: (share of the pixel that stays, light added).
fn glow_shape(shape: u32, u_: f32, r2_: f32, point: bool) -> vec2<f32> {
    let r2 = max(r2_, 1.0e-30);
    if (shape == 0u) {
        let q = (u_ + 0.0004) * 25.0;
        return vec2<f32>(1.0, (1.0 - u_) * 0.1 / (q * q) / r2);
    }
    if (shape == 1u) {
        return vec2<f32>(1.0, 0.05 * pow(max(1.0 - sqrt(u_), 0.0), 50.0) / r2 * select(1.0, 1.5, point));
    }
    if (shape == 2u) {
        let f = u_ * 1.05;
        if (f < 1.0) {
            let t = select(0.7, max(f - 0.95, 0.0) * 10.0, point);
            return vec2<f32>(t, 4e-3 * sqrt(1.05 - f * f) / r2 * select(0.7, 1.0, point));
        }
        if (point) {
            return vec2<f32>((f - 0.95) * 10.0, 17.8e-3 * (1.05 - f) / r2);
        }
        return vec2<f32>((f - 1.0) * 6.0 + 0.7, 12.5e-3 * (1.05 - f) / r2);
    }
    let u2 = u_ * u_;
    return vec2<f32>(u2 * u2, 7e-3 * (1.0 - u_) / r2);
}

// Mirrors render::light_glow; applies the glow of light `i` to `c`.
fn light_glow(i: u32, o: vec3<f32>, d: vec3<f32>, hit: bool, hit_t: f32, fog_col: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    let li = u.lights[i];
    let r2 = li.glow.y;
    let point = li.color.w > 0.5;
    var u_ = 0.0;
    var t_light = u.cam_fwd.w;
    if (point) {
        let v = li.dir.xyz - o;
        let along = dot(v, d);
        if (along < 0.0) {
            return c;
        }
        let w = v - d * along;
        let d2 = dot(w, w);
        if (d2 >= r2) {
            return c;
        }
        if (hit && hit_t < along - sqrt(r2 - d2)) {
            return c;
        }
        u_ = d2 / r2;
        t_light = along;
    } else {
        if (hit) {
            return c;
        }
        let a = max(1.0 - dot(li.dir.xyz, d), 0.0);
        if (a >= r2) {
            return c;
        }
        u_ = a / r2;
    }
    let s = glow_shape(u32(li.glow.x) - 1u, u_, r2, point);
    let fl = fog_amount(t_light);
    let flux = s.y * (1.0 - fl * 0.9);
    var keep = 1.0;
    if (li.glow.z < 0.5) {
        keep = clamp(s.x, 0.0, 1.0);
    }
    return c * keep + fog_col * max(fl, 0.0) * (1.0 - keep) + li.glow_col.xyz * flux;
}

// Mirrors scene::Fog::amount.
fn fog_amount(t: f32) -> f32 {
    if (u.fog2.x < 0.5) {
        return 1.0 - exp(-u.fog.w * t);
    }
    if (u.fog2.y == 0.0) {
        return 0.0;
    }
    let near = u.scales.w;
    let x = clamp((t - near) / max(u.cam_fwd.w - near, 1.0e-30), 0.0, 1.0);
    let c = max(u.fog2.w, 0.0);
    var v = x;
    if (c >= 1.0e-6) {
        v = (sqrt(1.0 + c * x) - 1.0) / (sqrt(1.0 + c) - 1.0);
    }
    var f = min(u.fog2.y * (v - u.fog2.z), 1.0);
    if (u.fog3.x > 0.5 && f > 0.0) {
        f = f * f;
    }
    return f;
}

// Mirrors cpu.rs Traced / Surface.
struct Traced {
    c: vec3<f32>,
    albedo: vec3<f32>,
    normal: vec3<f32>,
    hit: bool,
    pos: vec3<f32>,
    hit_pos: vec3<f32>,
    eps: f32,
    n: vec3<f32>,
    dir: vec3<f32>,
    t: f32,
    spec: vec3<f32>,
    base: vec3<f32>,
    trans: f32,
    fog: f32,
}

// Mirrors Shader::trace: one ray from `o` along `d` (distances `t0`..`max_t`; fog, glow and
// visible lights measured from `t_base`; `inside`: inside glass; `glass`: light amount that
// scales the diffuse light down by the transparency).
fn trace(o: vec3<f32>, d: vec3<f32>, t0: f32, max_t: f32, t_base: f32, sx: f32, sy: f32, inside: bool, glass: f32) -> Traced {
    var tr: Traced;
    tr.hit = false;
    let h = march_ray(o, d, t0, max_t, u.band.z);
    let glow = u.glow.w * max(f32(h.glow_steps) - u.glow3.x, 0.0) / f32(max(u.band.z, 1u));
    var glow_mix = 0.0;
    if (u.glow2.w > 0.0) {
        glow_mix = min(f32(h.glow_steps) / u.glow2.w, 1.0);
    }
    let grad = background(d, sy);
    var bg = grad;
    if (u.bg_rot[0].w > 0.5) {
        bg = background_image(d, sx, sy);
    }
    var fog_col = u.fog.xyz;
    if (u.fog3.y > 0.5) {
        fog_col = grad;
    }
    var c: vec3<f32>;
    tr.normal = vec3<f32>(0.0);
    if (!h.hit) {
        var f = 0.0;
        if (!inside) {
            f = fog_amount(u.cam_fwd.w);
        }
        var cover = 1.0;
        if (u.bg2.w > 0.5 && u.bg_rot[0].w > 0.5) {
            cover = 0.0;
        }
        c = bg * (1.0 - max(f, 0.0) * cover) + fog_col * max(f, 0.0);
        tr.albedo = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    } else {
        let pos = o + d * h.t;
        let nr = surface_normal_rough(pos, h.eps, h.t);
        var n = nr.xyz;
        if (dot(n, d) > 0.0) {
            n = -n;
        }
        // Roughness dulls the highlights and softens the diffuse curves.
        let rr = nr.w * u.blend.w;
        let spec_rough = 1.0 + min(2.0 * rr, 1.0) * (4.0 / max(u.sky.w, 1.0) - 1.0);
        let cv = color_value(h, n, pos);
        var cols = gradient(cv, h.t);
        // Interior colors where the orbit did not escape (mirrors Shader::interior_colors).
        if (u.interior.x > 0.5 && !h.orbit.escaped) {
            var v = clamp(h.orbit.r2 / max(u.march.z, 1.0e-30), 0.0, 1.0);
            if (u.scales.z > 0.5) {
                v = color_value(h, n, pos) / (u.frac.z + 1.0);
            }
            var t = v * u.interior.y + u.interior.z + u.color2.z * (h.t - u.color2.w);
            if (u.color.w > 0.5) {
                t = t - floor(t);
            } else {
                t = clamp(t, 0.0, 1.0);
            }
            let x = t * 255.0;
            let i = min(u32(floor(x)), 254u);
            let f = x - f32(i);
            cols.base = mix(lut[640u + i].xyz, lut[641u + i].xyz, f);
            cols.spec = mix(lut[896u + i].xyz, lut[897u + i].xyz, f);
            cols.trans = 0.0;
        }
        if (u.cmap0.x > 0.5) {
            cols.base = map_color(h, pos, n, cv, cols.base, rr);
        }
        let base = cols.base;
        let p = pos + n * (h.eps * select(1.5, 4.0, inside_now()));
        let ao = ambient_occlusion(p, n, h.t);
        let hemi = dot(n, u.amb_up.xyz) * 0.5 + 0.5;
        var amb = mix(u.ground.xyz, u.sky.xyz, hemi);
        if (u.bg2.z > 0.5 && u.bg_rot[0].w > 0.5) {
            let fi = clamp(fog_amount(u.cam_fwd.w), 0.0, 1.0);
            var keep = 1.0 - fi;
            if (u.bg2.w > 0.5) {
                keep = 1.0;
            }
            amb = background_ambient(n) * keep + fog_col * fi;
        }
        var diffuse = amb * u.ao.w * ao;
        var spec = vec3<f32>(0.0);
        let shin = u.sky.w;
        for (var i = 0u; i < u.size.w; i++) {
            let li = u.lights[i];
            // Point lights: direction, squared-distance falloff and shadow length.
            var ldir = li.dir.xyz;
            var atten = 1.0;
            var max_l = u.cam_fwd.w;
            if (li.color.w > 0.5) {
                let v = li.dir.xyz - p;
                let d2 = max(dot(v, v), 1.0e-30);
                let dist = sqrt(d2);
                ldir = v / dist;
                atten = 1.0 / d2;
                max_l = dist;
            }
            let ndl = dot(n, ldir);
            let curve = u32(li.opts.z);
            var df = max(ndl, 0.0);
            if (curve == 1u) {
                df = df * df;
            } else if (curve == 2u) {
                df = ndl * 0.5 + 0.5;
            } else if (curve == 3u) {
                df = (ndl * 0.5 + 0.5) * (ndl * 0.5 + 0.5);
            }
            if (rr > 0.0) {
                df += (rough_diffuse(curve, ndl) - df) * rr;
            }
            // Back faces: only wrapping curves reach them, and not through shadows.
            if (df <= 0.0 || (ndl <= 0.0 && li.opts.x > 0.5)) {
                continue;
            }
            var sh = 1.0;
            if (li.opts.x > 0.5 && u.dof.w < 0.5) {
                sh = soft_shadow(p, ldir, h.eps, li.opts.y, max_l);
            }
            if (sh <= 0.0) {
                continue;
            }
            let w = sh * atten * (1.0 - li.opts.w * (1.0 - ao));
            let hv = normalize(ldir - d);
            var s = 0.0;
            if (ndl > 0.0) {
                s = pow(max(dot(n, hv), 0.0), shin) * li.dir.w * u.ground.w * (shin + 8.0) / 25.0 * spec_rough;
            }
            diffuse += li.color.xyz * df * w;
            spec += li.color.xyz * s * w;
        }
        let keep = max(1.0 - cols.trans * glass, 0.0);
        let lit = base * diffuse * keep + spec * cols.spec;
        var f = 0.0;
        if (!inside) {
            f = fog_amount(h.t - t_base);
        }
        c = lit * (1.0 - f) + fog_col * max(f, 0.0);
        tr.albedo = clamp(base * (1.0 - f) + fog_col * max(f, 0.0), vec3<f32>(0.0), vec3<f32>(1.0));
        tr.normal = n;
        tr.hit = true;
        tr.pos = p;
        tr.hit_pos = pos;
        tr.eps = h.eps;
        tr.base = base;
        tr.trans = cols.trans;
        tr.n = n;
        tr.dir = d;
        tr.t = h.t;
        tr.spec = cols.spec;
        tr.fog = f;
    }
    if (inside) {
        // Inside glass: no glow and no visible lights.
        tr.c = c;
        return tr;
    }
    let glow_col = mix(u.glow.xyz, u.glow2.xyz, glow_mix);
    if (u.glow3.y > 0.5) {
        c = mix(c, glow_col, clamp(glow, 0.0, 1.0));
    } else {
        c += glow_col * glow;
    }
    // Visible light sources (seen from where this ray part starts).
    let ray_from = o + d * t_base;
    for (var i = 0u; i < u.size.w; i++) {
        if (u.lights[i].glow.x > 0.5) {
            c = light_glow(i, ray_from, d, h.hit, h.t - t_base, fog_col, c);
        }
    }
    tr.c = c;
    return tr;
}

fn sample_pixel(px: u32, py: u32, sample: u32) -> vec3<f32> {
    rng_state = pcg(px ^ pcg(py ^ pcg(sample)));
    var jx = 0.5;
    var jy = 0.5;
    if (sample > 0u) {
        jx = rnd();
        jy = rnd();
    }
    let sx = ((f32(px) + jx) / f32(u.size.x)) * 2.0 - 1.0;
    let sy = 1.0 - ((f32(py) + jy) / f32(u.size.y)) * 2.0;
    let th = u.cam_pos.w;
    var o = u.cam_pos.xyz;
    var d = normalize(u.cam_fwd.xyz + u.cam_right.xyz * (sx * th * u.cam_right.w) + u.cam_up.xyz * (sy * th));
    if (u.dof.x > 0.5 && sample > 0u) {
        let focus = o + d * (u.dof.z / dot(d, u.cam_fwd.xyz));
        let r = sqrt(rnd()) * u.dof.y * u.dof.z * 0.1;
        let a = rnd() * 6.283185307179586;
        o = o + u.cam_right.xyz * (r * cos(a)) + u.cam_up.xyz * (r * sin(a));
        d = normalize(focus - o);
    }
    // Without reflections (most scenes) a single trace: compiles much faster.
    if (!REFLECT) {
        let tr = trace(o, d, u.scales.w, u.cam_fwd.w, 0.0, sx, sy, false, 0.0);
        aux_albedo = tr.albedo;
        aux_normal = tr.normal;
        return tr.c;
    }
    // Primary ray, reflections and transmission (mirrors Shader::sample_aux) in one loop with
    // a single trace call.
    var c = vec3<f32>(0.0);
    var weight = vec3<f32>(1.0);
    var ro = o;
    var rd = d;
    var t0 = u.scales.w;
    var max_t = u.cam_fwd.w;
    var t_base = 0.0;
    var sxr = sx;
    var syr = sy;
    let amount = u.glow3.w;
    var bounces = 0u;
    if (amount > 0.0) {
        bounces = clamp(u32(u.fog3.w), 1u, 8u);
    }
    var glass = 0.0;
    if (u.glass.x > 0.5 && amount > 0.0) {
        glass = amount;
    }
    let fov = 2.0 * atan(u.cam_pos.w);
    var inside = false;
    var prev: Traced;
    for (var b = 0u; b <= bounces; b++) {
        var g = glass;
        if (b == bounces) {
            g = 0.0;
        }
        let tr = trace(ro, rd, t0, max_t, t_base, sxr, syr, inside, g);
        if (b == 0u) {
            aux_albedo = tr.albedo;
            aux_normal = tr.normal;
            c = tr.c;
        } else {
            if (inside) {
                // Absorbed by the color where the ray entered; light scattered on the way.
                var len = u.cam_fwd.w;
                var end = ro + rd * (t0 + len);
                if (tr.hit) {
                    len = tr.t - prev.t;
                    end = tr.hit_pos;
                }
                let ab = exp2(len * u.glass.z * log2(max(prev.base, vec3<f32>(1.0e-30))));
                let sc_amt = (1.0 - dot(ab, vec3<f32>(0.3, 0.59, 0.11))) * u.glass.w / 330.0;
                if (sc_amt > 0.0) {
                    c += light_sum(ro + rd * t0, end) * prev.base * weight * (ab + 1.0) * 0.5 * sc_amt;
                }
                weight *= ab;
            }
            c += weight * tr.c;
        }
        if (b == bounces || !tr.hit) {
            break;
        }
        let bn = bounce(tr, inside);
        weight *= bn.f * amount * clamp(1.0 - tr.fog, 0.0, 1.0);
        if (all(weight < vec3<f32>(1.0e-4))) {
            break;
        }
        var start = tr.pos;
        if (bn.refract) {
            start = tr.hit_pos - tr.n * tr.eps;
            inside = !inside;
            flip = inside;
        }
        prev = tr;
        rd = bn.dir;
        ro = start - rd * tr.t;
        t0 = tr.t;
        max_t = tr.t + u.cam_fwd.w;
        t_base = tr.t;
        syr = clamp(2.0 * asin(clamp(dot(rd, u.cam_up.xyz), -1.0, 1.0)) / fov, -1.0, 1.0);
        sxr = clamp(atan2(dot(rd, u.cam_right.xyz), dot(rd, u.cam_fwd.xyz)) / atan(u.cam_pos.w * u.cam_right.w), -1.0, 1.0);
    }
    flip = false;
    return c;
}

struct Bounce {
    dir: vec3<f32>,
    f: vec3<f32>,
    refract: bool,
}

// Mirrors Shader::bounce: direction after surface `s`, path weight factor, refracted or not.
fn bounce(s: Traced, inside: bool) -> Bounce {
    var r: Bounce;
    r.dir = s.dir - s.n * (2.0 * dot(s.dir, s.n));
    r.f = s.spec;
    r.refract = false;
    let a = s.trans;
    if (u.glass.x < 0.5 || a <= 0.0) {
        return r;
    }
    var eta = 1.0 / u.glass.y;
    if (inside) {
        eta = u.glass.y;
    }
    var ci = -dot(s.dir, s.n);
    let k = 1.0 - eta * eta * (1.0 - ci * ci);
    let smax = max(max(s.spec.x, s.spec.y), s.spec.z);
    if (k <= 0.0) {
        // Total internal reflection.
        r.f = s.spec * (1.0 - a + a * a / (smax + 0.01));
        return r;
    }
    let ct = sqrt(k);
    let t = normalize(s.dir * eta + s.n * (eta * ci - ct));
    ci = abs(ci);
    var fr = ci;
    if (ct > 1.0e-16) {
        let rs = (ci - eta * ct) / (ci + eta * ct);
        let rp = (ct - eta * ci) / (ct + eta * ci);
        fr = (rs * rs + rp * rp) * 0.5;
    }
    let wt = max((1.0 - fr * smax) * a, 0.0);
    let wr = s.spec * (fr + (1.0 - fr) * (1.0 - a));
    let lr = (wr.x + wr.y + wr.z) / 3.0;
    if (wt + lr <= 0.0) {
        r.f = vec3<f32>(0.0);
        return r;
    }
    let p = wt / (wt + lr);
    if (rnd() < p) {
        r.dir = t;
        r.f = vec3<f32>(wt / p);
        r.refract = true;
    } else {
        r.f = wr / (1.0 - p);
    }
    return r;
}

// Mirrors Shader::light_sum.
fn light_sum(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    let ab = b - a;
    let l2 = max(dot(ab, ab), 1.0e-30);
    for (var i = 0u; i < u.size.w; i++) {
        let li = u.lights[i];
        var w = 1.0;
        if (li.color.w > 0.5) {
            let s = clamp(dot(li.dir.xyz - a, ab) / l2, 0.0, 1.0);
            let v = li.dir.xyz - (a + ab * s);
            w = 1.0 / max(dot(v, v), 1.0e-12);
        }
        sum += li.color.xyz * w;
    }
    return sum;
}

@compute @workgroup_size(8, 8)
fn march_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let px = gid.x;
    let py = gid.y + u.band.x;
    if (px >= u.size.x || gid.y >= u.band.y || py >= u.size.y) {
        return;
    }
    var c = sample_pixel(px, py, u.size.z);
    if (!(is_finite(c.x) && is_finite(c.y) && is_finite(c.z))) {
        c = vec3<f32>(0.0);
    }
    let idx = py * u.size.x + px;
    if (u.size.z == 0u) {
        accum[idx] = vec4<f32>(c, 1.0);
        aux[2u * idx] = vec4<f32>(aux_albedo, 0.0);
        aux[2u * idx + 1u] = vec4<f32>(aux_normal, 0.0);
    } else {
        accum[idx] += vec4<f32>(c, 1.0);
        aux[2u * idx] += vec4<f32>(aux_albedo, 0.0);
        aux[2u * idx + 1u] += vec4<f32>(aux_normal, 0.0);
    }
}
