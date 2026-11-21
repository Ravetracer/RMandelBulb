// SPDX-License-Identifier: LGPL-2.1-or-later
//
// Ray marching kernel. Mirrors rmb-core/src/render/cpu.rs step by step; keep them in sync.
// The ITERATE marker below is replaced by the generated hybrid iteration (codegen.rs).

alias fl = f32;

struct GLight {
    dir: vec4<f32>,   // w: specular
    color: vec4<f32>,
    opts: vec4<f32>,  // x: shadows, y: softness
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
    color2: vec4<f32>,     // saturation, cut count
    cuts: array<vec4<f32>, 3>,   // axis, position, keep above
    slots: array<vec4<u32>, 8>,  // x: iterations
    lights: array<GLight, 6>,
    comb: vec4<f32>,       // DE combination: mode, max iterations 2, repeat from 2, smoothing
    scales: vec4<f32>,     // DE scale of chain 1, chain 2
    rot4: array<vec4<f32>, 4>, // 4D start rotation (rows)
}

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> prm: array<f32>;
@group(0) @binding(2) var<storage, read> lut: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> accum: array<vec4<f32>>;

struct Orbit {
    de: f32,
    n: u32,
    r2: f32,
    trap: f32,
    escaped: bool,
}

struct Iterated {
    s: vec4<f32>,
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
    orbit: Orbit,
}

fn is_finite(v: f32) -> bool {
    return (bitcast<u32>(v) & 0x7fffffffu) < 0x7f800000u;
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
    loop {
        if (step >= max_steps) {
            break;
        }
        let eps = eps_at(t);
        let ob = orbit(o + d * t, eps * 0.5);
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
            return h;
        }
        last_step = max(ob.de * u.march.y, eps * 0.1);
        t += last_step;
        if (t > max_t) {
            h.steps = step;
            return h;
        }
        step++;
    }
    h.steps = max_steps;
    return h;
}

fn surface_normal(p: vec3<f32>, eps: f32) -> vec3<f32> {
    let h = eps * 0.5;
    let k1 = vec3<f32>(1.0, -1.0, -1.0);
    let k2 = vec3<f32>(-1.0, -1.0, 1.0);
    let k3 = vec3<f32>(-1.0, 1.0, -1.0);
    let k4 = vec3<f32>(1.0, 1.0, 1.0);
    let n = k1 * de_at(p + k1 * h, h) + k2 * de_at(p + k2 * h, h) + k3 * de_at(p + k3 * h, h) + k4 * de_at(p + k4 * h, h);
    let l = length(n);
    if (l > 0.0) {
        return n / l;
    }
    return vec3<f32>(0.0, 0.0, 1.0);
}

fn soft_shadow(p: vec3<f32>, l: vec3<f32>, eps: f32, softness: f32) -> f32 {
    var res = 1.0;
    var t = eps * 4.0;
    let max_t = u.cam_fwd.w;
    for (var i = 0u; i < 160u; i++) {
        let e = eps_at(t) * 0.5 + eps * 0.5;
        let d = de_at(p + l * t, e);
        if (d < e) {
            return 0.0;
        }
        if (softness > 0.0) {
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

fn ambient_occlusion(p: vec3<f32>, n: vec3<f32>, t: f32) -> f32 {
    let radius = u.ao.z * t;
    var mode = u32(u.ao.x);
    if (u.dof.w > 0.5 && mode == 2u) {
        mode = 1u;
    }
    if (mode == 0u) {
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
        return clamp(1.0 - u.ao.y * occ / 0.959, 0.0, 1.0);
    }
    let u1 = rnd();
    let u2 = rnd();
    let d = cosine_dir(n, u1, u2);
    let eps = eps_at(t);
    let h = march_ray(p + n * (eps * 2.0), d, 0.0, radius, 48u);
    if (h.hit) {
        return 1.0 - u.ao.y * clamp(1.0 - h.t / radius, 0.0, 1.0);
    }
    return 1.0;
}

fn background(d: vec3<f32>) -> vec3<f32> {
    if (u.bg_top.w < 0.5) {
        return u.bg_top.xyz;
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
        let n_it = f32(o.n);
        if (o.escaped && o.r2 > 1.0) {
            let lb = log(sqrt(max(u.march.z, 4.0)));
            return n_it - log2(max((0.5 * log(o.r2)) / lb, 1.0e-10));
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

fn gradient(v: f32) -> vec3<f32> {
    var t = v * u.color.y + u.color.z;
    if (u.color.w > 0.5) {
        t = t - floor(t);
    } else {
        t = clamp(t, 0.0, 1.0);
    }
    let x = t * 255.0;
    let i = min(u32(floor(x)), 254u);
    let f = x - f32(i);
    let base = mix(lut[i].xyz, lut[i + 1u].xyz, f);
    return mix(lut[0].xyz, base, u.color2.x);
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
    let h = march_ray(o, d, 0.0, u.cam_fwd.w, u.band.z);
    let glow = u.glow.w * f32(h.steps) / f32(max(u.band.z, 1u));
    var c: vec3<f32>;
    if (!h.hit) {
        let bg = background(d);
        let f = 1.0 - exp(-u.fog.w * u.cam_fwd.w);
        c = mix(bg, u.fog.xyz, f);
    } else {
        let pos = o + d * h.t;
        var n = surface_normal(pos, h.eps);
        if (dot(n, d) > 0.0) {
            n = -n;
        }
        let base = gradient(color_value(h, n, pos));
        let p = pos + n * (h.eps * 1.5);
        let ao = ambient_occlusion(p, n, h.t);
        let hemi = n.z * 0.5 + 0.5;
        var diffuse = mix(u.ground.xyz, u.sky.xyz, hemi) * u.ao.w * ao;
        var spec = vec3<f32>(0.0);
        let shin = u.sky.w;
        for (var i = 0u; i < u.size.w; i++) {
            let li = u.lights[i];
            let ndl = dot(n, li.dir.xyz);
            if (ndl <= 0.0) {
                continue;
            }
            var sh = 1.0;
            if (li.opts.x > 0.5 && u.dof.w < 0.5) {
                sh = soft_shadow(p, li.dir.xyz, h.eps, li.opts.y);
            }
            if (sh <= 0.0) {
                continue;
            }
            let hv = normalize(li.dir.xyz - d);
            let s = pow(max(dot(n, hv), 0.0), shin) * li.dir.w * u.ground.w * (shin + 8.0) / 25.0;
            diffuse += li.color.xyz * ndl * sh;
            spec += li.color.xyz * s * sh;
        }
        let lit = base * diffuse + spec;
        let f = 1.0 - exp(-u.fog.w * h.t);
        c = mix(lit, u.fog.xyz, f);
    }
    c += u.glow.xyz * glow;
    return c;
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
    } else {
        accum[idx] += vec4<f32>(c, 1.0);
    }
}
