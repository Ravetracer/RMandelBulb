// SPDX-License-Identifier: LGPL-2.1-or-later
//
// Ray marching kernel. Mirrors rmb-core/src/render/cpu.rs step by step; keep them in sync.
// The ITERATE marker below is replaced by the generated hybrid iteration (codegen.rs).

alias fl = f32;

struct GLight {
    dir: vec4<f32>,   // xyz: direction, or the position of a point light; w: specular
    color: vec4<f32>, // w: kind (0 directional, 1 point, 2 spot, 3 area, 4 line)
    opts: vec4<f32>,  // x: shadows, y: softness, z: diffuse curve, w: occlusion
    glow: vec4<f32>,  // visible light: x: shape + 1 (0 = not visible), y: squared radius, z: additive
    glow_col: vec4<f32>,
    shape0: vec4<f32>, // spot: axis, cos outer; area: normal; line: other end
    shape1: vec4<f32>, // spot: x cos inner; area: half width axis
    shape2: vec4<f32>, // area: half height axis
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
    slots: array<vec4<u32>, 16>, // x: iterations, y: first iteration, z: last, w: flags (1 no constant, 2 no escape test)
    lights: array<GLight, 8>,
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
    emis: vec4<f32>,       // emissive colors: light, reach, glow, glow size (0 when nothing emits)
    emis2: vec4<f32>,      // emissive colors: rays for their light; steam strength (0 when nothing emits), size, grain
    emis3: vec4<f32>,      // steam offset, cover
    path: vec4<f32>,       // path tracing: on, bounces, firefly clamp, sky reach (0 = none)
    ccenter: vec4<f32>,    // position color modes: center
    cdir: vec4<f32>,       // plane color mode: direction (normalized)
    c2opt: vec4<f32>,      // MB3D second color choice: option, multiplier, color on iteration
    // Deep zoom (Program::deep): the reference point as hi + lo (w of hi: 1.0 for dd_one; w of
    // lo: low part of the w slice), low parts of the julia constant and blend weights.
    deep_hi: vec4<f32>,
    deep_lo: vec4<f32>,
    julia_lo: vec4<f32>,
    blend_lo: vec4<f32>,
    // Motion blur: the camera at the end of the exposure (position, w: on; forward, w: tan of
    // half the field of view; right; up).
    mot_pos: vec4<f32>,
    mot_fwd: vec4<f32>,
    mot_right: vec4<f32>,
    mot_up: vec4<f32>,
    fog4: vec4<f32>,       // light scattering: strength (0 = off), forward anisotropy
    gloss: vec4<f32>,      // x: roughness of reflections and refractions (0 = mirror)
    shim: vec4<f32>,       // shimmer amount, size, phase; emission of the color map
    // Placement of the main fractal (MAIN_T): inverse rotation rows over the scale per axis
    // (w of mt0: the smallest scale), position.
    mt0: vec4<f32>,
    mt1: vec4<f32>,
    mt2: vec4<f32>,
    mt_pos: vec4<f32>,
    // Fractal haze (Fog::haze*): strength (0 = off), start share, glow, colors from the
    // gradient; its color, w: end share.
    haze: vec4<f32>,
    haze_col: vec4<f32>,
    haze2: vec4<f32>,      // haze shape (0 distance, 1 iterations), size
    slotw: array<vec4<f32>, 4>, // weight per slot (Mandelbulber per-slot settings)
    cfac: vec4<f32>,       // constant factor xyz, w: minimum iterations
    view: vec4<f32>,       // projection (Projection::index), fisheye half angle, stereo mode, swap eyes
    view2: vec4<f32>,      // half eye distance, convergence distance, anaglyph in color, images per sample
    grid: vec4<f32>,       // mesh export (grid_main): grid origin, step
    grid2: vec4<f32>,      // mesh export: slice index
    tile: vec4<u32>,       // rendered tile of a larger image: left, top, full width, full height
    dof2: vec4<f32>,       // aperture blades (0 = round), their rotation (radians)
}

@group(0) @binding(0) var<uniform> u: U;
// Formula parameters (Program::constants, 4 per entry; from entry 1024 on the low parts for
// the double-float deep zoom). A uniform buffer: every thread reads the same values, which
// the constant cache serves far faster than a storage buffer.
@group(0) @binding(1) var<uniform> prm: array<vec4<f32>, 2048>;
// Surface colors (w: emission) at 0..255, highlight colors (w: transparency) at 256..511, rough diffuse tables at 512..639
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
// Distance of the first hit and whether there is one (EXR depth and alpha; aux w components).
var<private> aux_depth: f32;
var<private> aux_hit: f32;

struct Orbit {
    de: f32,
    n: u32,
    r2: f32,
    r2_old: f32,
    trap: f32,
    // Mandelbulb3D's second color choice (fractal.rs color2_value).
    c2: f32,
    escaped: bool,
    // The object that forms the surface: 0 the main fractal, k the k-th composed object.
    obj: u32,
    // Water: its distance to the rest of the scene (local units).
    shore: f32,
}

// A composed object (rmb-core compose.rs, filled by renderer.rs).
struct Obj {
    inv0: vec4<f32>,  // inverse rotation rows; w: distance scale, operation, smoothing
    inv1: vec4<f32>,
    inv2: vec4<f32>,
    pos: vec4<f32>,   // w: material (0 main, k the k-th object's own)
    prim: vec4<f32>,  // primitive parameters
    prim2: vec4<f32>, // more primitive parameters (water: choppy, octaves, foam, reflection)
    prim3: vec4<f32>, // water: shore waves, spray
    frac: vec4<f32>,  // fractal: w slice, julia, max iterations, repeat from
    julia: vec4<f32>,
    misc: vec4<f32>,  // bailout², formula DE scale, DE scale, kind (0 fractal, 1 primitive, 2 model)
    mat: vec4<f32>,   // material: color mode, scale, offset, repeat
    mat2: vec4<f32>,  // saturation, pattern, pattern size, LUT base
    slots: array<vec4<u32>, 4>,
    size: vec4<f32>,  // scale per own axis
    rep: vec4<f32>,   // repetition: spacing per own axis (0 = none)
    rep_lim: vec4<f32>, // copies on each side (0 = endless)
    comb: vec4<f32>,  // fractal DE combination: mode, max iterations 2, repeat from 2, smoothing
    comb2: vec4<f32>, // DE scale of chain 2, mix power, interpolation weights
}

// A uniform buffer like prm (read on every distance estimate); compose::MAX_OBJECTS entries.
@group(0) @binding(6) var<uniform> objs: array<Obj, 8>;

// Baked 3D models (rmb-core mesh.rs, filled by renderer.rs::volumes_gpu): per object j a header
// of 32 words at 32 j: grid points (3), first word of the distances, grid origin (3 f32), spacing
// (f32), mesh box min (3 f32) and max (3 f32), first word of the triangles (9 f32 each: corner,
// two edges), of the hierarchy nodes (8 words: box min, max, start, count), node count, first
// word of the shading (0: no colors; 7 words per triangle: material, 3 uv pairs), of the vertex
// colors (0: none; 3 RGBA8 words per triangle), of the materials (8 words: linear rgb, texture,
// bump map, bump height, specular map, unused; maps 0xffffffff: none), material count, first
// word of the texture table (4 words: first texel, width, height, flags: wrap u, wrap v, v up),
// texture count, colors on (1/0). Distances are 16-bit, two per word; texels sRGB RGBA8.
@group(0) @binding(7) var<storage, read> vols: array<u32>;

const MODEL_HEADER: u32 = 32u;
// mesh.rs GRID_ERROR, NEAR, SEARCH_RANGE, STACK.
const MODEL_GRID_ERROR: f32 = 1.7320508;
const MODEL_NEAR: f32 = 2.75;
const MODEL_RANGE: f32 = 4.5;
const MODEL_STACK: u32 = 32u;

fn vol_f(i: u32) -> f32 {
    return bitcast<f32>(vols[i]);
}

fn vol_v3(i: u32) -> vec3<f32> {
    return vec3<f32>(vol_f(i), vol_f(i + 1u), vol_f(i + 2u));
}

fn vol_dist(off: u32, i: u32) -> f32 {
    let w = vols[off + i / 2u];
    let v = bitcast<i32>(w << (16u - (i & 1u) * 16u)) >> 16u;
    return f32(v) / 32767.0;
}

fn box_d2(q: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> f32 {
    let d = max(max(lo - q, q - hi), vec3<f32>(0.0));
    return dot(d, d);
}

// Mirrors mesh::Tri::closest: squared distance and the barycentric weights (v, w).
fn tri_closest(p: vec3<f32>, a: vec3<f32>, ab: vec3<f32>, ac: vec3<f32>) -> vec3<f32> {
    let ap = p - a;
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    var vw = vec2<f32>(0.0);
    loop {
        if (d1 <= 0.0 && d2 <= 0.0) {
            break;
        }
        let bp = ap - ab;
        let d3 = dot(ab, bp);
        let d4 = dot(ac, bp);
        if (d3 >= 0.0 && d4 <= d3) {
            vw = vec2<f32>(1.0, 0.0);
            break;
        }
        let vc = d1 * d4 - d3 * d2;
        if (vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0) {
            vw = vec2<f32>(d1 / (d1 - d3), 0.0);
            break;
        }
        let cp = ap - ac;
        let d5 = dot(ab, cp);
        let d6 = dot(ac, cp);
        if (d6 >= 0.0 && d5 <= d6) {
            vw = vec2<f32>(0.0, 1.0);
            break;
        }
        let vb = d5 * d2 - d1 * d6;
        if (vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0) {
            vw = vec2<f32>(0.0, d2 / (d2 - d6));
            break;
        }
        let va = d3 * d6 - d5 * d4;
        if (va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0) {
            let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
            vw = vec2<f32>(1.0 - w, w);
            break;
        }
        let s = va + vb + vc;
        if (s > 0.0) {
            vw = vec2<f32>(vb / s, vc / s);
        }
        break;
    }
    let d = ap - vw.x * ab - vw.y * ac;
    return vec3<f32>(dot(d, d), vw);
}

struct TriHit {
    d2: f32,
    tri: u32,
    v: f32,
    w: f32,
    found: bool,
}

fn node_d2(nodes: u32, i: u32, q: vec3<f32>) -> f32 {
    let b = nodes + i * 8u;
    return box_d2(q, vol_v3(b), vol_v3(b + 3u));
}

// Mirrors mesh::Volume::nearest (header at h).
fn model_nearest(h: u32, q: vec3<f32>, limit: f32) -> TriHit {
    var r: TriHit;
    r.d2 = limit * limit;
    r.found = false;
    let tris = vols[h + 14u];
    let nodes = vols[h + 15u];
    if (vols[h + 16u] == 0u) {
        return r;
    }
    var st_n: array<u32, MODEL_STACK>;
    var st_d: array<f32, MODEL_STACK>;
    st_n[0] = 0u;
    st_d[0] = node_d2(nodes, 0u, q);
    var sp = 1u;
    loop {
        if (sp == 0u) {
            break;
        }
        sp -= 1u;
        let ni = st_n[sp];
        if (st_d[sp] >= r.d2) {
            continue;
        }
        let b = nodes + ni * 8u;
        let start = vols[b + 6u];
        let count = vols[b + 7u];
        if (count > 0u) {
            for (var t = start; t < start + count; t++) {
                let o = tris + t * 9u;
                let c = tri_closest(q, vol_v3(o), vol_v3(o + 3u), vol_v3(o + 6u));
                if (c.x < r.d2) {
                    r.d2 = c.x;
                    r.tri = t;
                    r.v = c.y;
                    r.w = c.z;
                    r.found = true;
                }
            }
            continue;
        }
        let dl = node_d2(nodes, start, q);
        let dr = node_d2(nodes, start + 1u, q);
        var near = start;
        var far = start + 1u;
        var dn = dl;
        var df = dr;
        if (dl > dr) {
            near = start + 1u;
            far = start;
            dn = dr;
            df = dl;
        }
        if (df < r.d2 && sp < MODEL_STACK) {
            st_n[sp] = far;
            st_d[sp] = df;
            sp += 1u;
        }
        if (dn < r.d2 && sp < MODEL_STACK) {
            st_n[sp] = near;
            st_d[sp] = dn;
            sp += 1u;
        }
    }
    return r;
}

// Mirrors mesh::Volume::behind.
fn model_behind(h: u32, q: vec3<f32>, n: TriHit) -> bool {
    let o = vols[h + 14u] + n.tri * 9u;
    let a = vol_v3(o);
    let ab = vol_v3(o + 3u);
    let ac = vol_v3(o + 6u);
    let p = a + n.v * ab + n.w * ac;
    return dot(q - p, cross(ab, ac)) < 0.0;
}

// Mirrors mesh::Volume::distance.
fn model_de(j: u32, q: vec3<f32>) -> f32 {
    let h = j * MODEL_HEADER;
    let res = vec3<u32>(vols[h], vols[h + 1u], vols[h + 2u]);
    if (res.x == 0u) {
        return 1.0e10;
    }
    let mn = vec3<f32>(vol_f(h + 4u), vol_f(h + 5u), vol_f(h + 6u));
    let voxel = vol_f(h + 7u);
    let nf = vec3<f32>(res);
    let hi = mn + (nf - 1.0) * voxel;
    let g3 = clamp((q - mn) / voxel, vec3<f32>(0.0), nf - 1.0);
    let i0 = min(floor(g3), nf - 2.0);
    let f = g3 - i0;
    let i = vec3<u32>(i0);
    let off = vols[h + 3u];
    let nx = res.x;
    let nxy = res.x * res.y;
    let b = i.x + i.y * nx + i.z * nxy;
    let x00 = mix(vol_dist(off, b), vol_dist(off, b + 1u), f.x);
    let x10 = mix(vol_dist(off, b + nx), vol_dist(off, b + nx + 1u), f.x);
    let x01 = mix(vol_dist(off, b + nxy), vol_dist(off, b + nxy + 1u), f.x);
    let x11 = mix(vol_dist(off, b + nxy + nx), vol_dist(off, b + nxy + nx + 1u), f.x);
    let g = mix(mix(x00, x10, f.y), mix(x01, x11, f.y), f.z) * 64.0 * voxel;
    let e = MODEL_GRID_ERROR * voxel;
    let far = select(min(g + e, 0.0), max(g - e, 0.0), g > 0.0);
    if (any(q < mn) || any(q > hi)) {
        let lo = vol_v3(h + 8u);
        let up = vol_v3(h + 11u);
        let away = length(q - clamp(q, mn, hi));
        return max(length(max(max(lo - q, q - up), vec3<f32>(0.0))), far - away);
    }
    if (abs(g) >= MODEL_NEAR * voxel) {
        return far;
    }
    let range = MODEL_RANGE * voxel;
    let n = model_nearest(h, q, range);
    if (!n.found) {
        return select(range, -range, g < -voxel);
    }
    let d = sqrt(n.d2);
    var inside = g < -voxel;
    if (!inside && g < 0.0) {
        inside = model_behind(h, q, n);
    }
    return select(d, -d, inside);
}

fn srgb8_linear(p: u32) -> vec3<f32> {
    return srgb_decode3(vec3<f32>(f32(p & 255u), f32((p >> 8u) & 255u), f32((p >> 16u) & 255u)) / 255.0);
}

fn texel_value(p: u32, raw: bool) -> vec3<f32> {
    let c = vec3<f32>(f32(p & 255u), f32((p >> 8u) & 255u), f32((p >> 16u) & 255u)) / 255.0;
    if (raw) {
        return c;
    }
    return srgb_decode3(c);
}

// Mirrors mesh::Texture::value.
fn model_value(e: u32, u: f32, v: f32) -> f32 {
    let c = model_texture(e, u, v, true);
    return (c.x + c.y + c.z) / 3.0;
}

// Mirrors mesh::wrap_index (mode 0 repeat, 1 clamp, 2 mirror).
fn wrap_index(i: i32, n: i32, mode: u32) -> i32 {
    if (mode == 1u) {
        return clamp(i, 0, n - 1);
    }
    if (mode == 2u) {
        let m = ((i % (2 * n)) + 2 * n) % (2 * n);
        return select(m, 2 * n - 1 - m, m >= n);
    }
    return ((i % n) + n) % n;
}

// Mirrors mesh::reduce_coord.
fn reduce_coord(t: f32, mode: u32) -> f32 {
    if (mode == 1u) {
        return clamp(t, -1.0, 2.0);
    }
    if (mode == 2u) {
        return t - 2.0 * floor(t * 0.5);
    }
    return t - floor(t);
}

// Mirrors mesh::Texture::sample (texture table entry at e; raw: Texture::value's stored values
// without sRGB decoding).
fn model_texture(e: u32, u_: f32, v_: f32, raw: bool) -> vec3<f32> {
    let base = vols[e];
    let w = i32(vols[e + 1u]);
    let h = i32(vols[e + 2u]);
    let flags = vols[e + 3u];
    let wu = flags & 3u;
    let wv = (flags >> 2u) & 3u;
    let v = select(v_, 1.0 - v_, (flags & 16u) != 0u);
    let x = reduce_coord(u_, wu) * f32(w) - 0.5;
    let y = reduce_coord(v, wv) * f32(h) - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let xa = wrap_index(i32(x0), w, wu);
    let xb = wrap_index(i32(x0) + 1, w, wu);
    let ya = wrap_index(i32(y0), h, wv);
    let yb = wrap_index(i32(y0) + 1, h, wv);
    let a = texel_value(vols[base + u32(ya * w + xa)], raw);
    let b = texel_value(vols[base + u32(ya * w + xb)], raw);
    let c = texel_value(vols[base + u32(yb * w + xa)], raw);
    let d = texel_value(vols[base + u32(yb * w + xb)], raw);
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    return top + (bottom - top) * fy;
}

struct ModelSurface {
    base: vec3<f32>,
    spec: f32,
    n: vec3<f32>,
    found: bool,
    // Emitted light (linear RGB; display values after model_surface in display space).
    emis: vec3<f32>,
}

// Mirrors mesh::Volume::normal_map (texture table entry e, model-space normal n).
fn model_normal_map(h: u32, tri: u32, e: u32, strength: f32, u_: f32, v_: f32, n: vec3<f32>) -> vec3<f32> {
    let o = vols[h + 14u] + tri * 9u;
    let ab = vol_v3(o + 3u);
    let ac = vol_v3(o + 6u);
    let s = vols[h + 17u] + tri * 7u;
    let du1 = vol_f(s + 3u) - vol_f(s + 1u);
    let dv1 = vol_f(s + 4u) - vol_f(s + 2u);
    let du2 = vol_f(s + 5u) - vol_f(s + 1u);
    let dv2 = vol_f(s + 6u) - vol_f(s + 2u);
    let det = du1 * dv2 - du2 * dv1;
    if (abs(det) < 1.0e-20) {
        return n;
    }
    let dpdu = (ab * dv2 - ac * dv1) / det;
    let dpdv = (ac * du1 - ab * du2) / det;
    let ou = dpdu - n * dot(dpdu, n);
    let ov = dpdv - n * dot(dpdv, n);
    if (length(ou) <= 1.0e-30 || length(ov) <= 1.0e-30) {
        return n;
    }
    let up = select(-1.0, 1.0, (vols[e + 3u] & 16u) != 0u);
    let c = model_texture(e, u_, v_, true) * 2.0 - 1.0;
    let b = normalize(ou) * (c.x * strength) + normalize(ov) * (c.y * strength * up) + n * max(c.z, 0.0);
    let len = length(b);
    if (len > 0.0) {
        return b / len;
    }
    return n;
}

// Mirrors mesh::Volume::bump (texture table entry e, model-space normal n).
fn model_bump(h: u32, tri: u32, e: u32, height: f32, u_: f32, v_: f32, n: vec3<f32>) -> vec3<f32> {
    let o = vols[h + 14u] + tri * 9u;
    let ab = vol_v3(o + 3u);
    let ac = vol_v3(o + 6u);
    let s = vols[h + 17u] + tri * 7u;
    let du1 = vol_f(s + 3u) - vol_f(s + 1u);
    let dv1 = vol_f(s + 4u) - vol_f(s + 2u);
    let du2 = vol_f(s + 5u) - vol_f(s + 1u);
    let dv2 = vol_f(s + 6u) - vol_f(s + 2u);
    let det = du1 * dv2 - du2 * dv1;
    if (abs(det) < 1.0e-20) {
        return n;
    }
    let dpdu = (ab * dv2 - ac * dv1) / det;
    let dpdv = (ac * du1 - ab * du2) / det;
    let nn = dot(n, cross(dpdu, dpdv));
    if (abs(nn) < 1.0e-20) {
        return n;
    }
    let eu = 1.0 / f32(vols[e + 1u]);
    let ev = 1.0 / f32(vols[e + 2u]);
    let hu = (model_value(e, u_ + eu, v_) - model_value(e, u_ - eu, v_)) / (2.0 * eu) * height;
    let hv = (model_value(e, u_, v_ + ev) - model_value(e, u_, v_ - ev)) / (2.0 * ev) * height;
    let b = n - (hu * cross(dpdv, n) + hv * cross(n, dpdu)) / nn;
    let len = length(b);
    if (len > 0.0) {
        return b / len;
    }
    return n;
}

// Mirrors mesh::Volume::surface (model j, model-space point q and normal n; base: -1 without
// colors). bump: bend the normal.
fn model_surf(j: u32, q: vec3<f32>, n: vec3<f32>, bump: bool) -> ModelSurface {
    var r: ModelSurface;
    r.base = vec3<f32>(-1.0);
    r.spec = 1.0;
    r.n = n;
    r.found = false;
    r.emis = vec3<f32>(0.0);
    let h = j * MODEL_HEADER;
    let sh = vols[h + 17u];
    if (vols[h] == 0u || sh == 0u) {
        return r;
    }
    let near = model_nearest(h, q, MODEL_RANGE * vol_f(h + 7u));
    if (!near.found) {
        return r;
    }
    r.found = true;
    let t = near.tri;
    let u_ = 1.0 - near.v - near.w;
    let s = sh + t * 7u;
    let mat = vols[s];
    let has_mat = mat < vols[h + 20u];
    let m = vols[h + 19u] + mat * 16u;
    let tu = vol_f(s + 1u) * u_ + vol_f(s + 3u) * near.v + vol_f(s + 5u) * near.w;
    let tv = vol_f(s + 2u) * u_ + vol_f(s + 4u) * near.v + vol_f(s + 6u) * near.w;
    let ntex = vols[h + 22u];
    let table = vols[h + 21u];
    if (vols[h + 23u] != 0u) {
        var c = vec3<f32>(1.0);
        var tex = 0xffffffffu;
        if (has_mat) {
            c = vol_v3(m);
            tex = vols[m + 3u];
        }
        let vc = vols[h + 18u];
        if (vc != 0u) {
            let o = vc + t * 3u;
            c *= srgb8_linear(vols[o]) * u_ + srgb8_linear(vols[o + 1u]) * near.v + srgb8_linear(vols[o + 2u]) * near.w;
        }
        if (tex < ntex) {
            c *= model_texture(table + tex * 4u, tu, tv, false);
        }
        r.base = c;
    }
    if (!has_mat) {
        return r;
    }
    let spec = vols[m + 6u];
    if (spec < ntex) {
        r.spec = model_value(table + spec * 4u, tu, tv);
    }
    let bmap = vols[m + 4u];
    if (bump && bmap < ntex) {
        r.n = model_bump(h, t, table + bmap * 4u, vol_f(m + 5u), tu, tv, n);
    }
    let nmap = vols[m + 7u];
    if (bump && nmap < ntex) {
        r.n = model_normal_map(h, t, table + nmap * 4u, vol_f(m + 8u), tu, tv, r.n);
    }
    let em = vol_v3(m + 9u);
    if (any(em > vec3<f32>(0.0))) {
        var et = vec3<f32>(1.0);
        let emap = vols[m + 12u];
        if (emap < ntex) {
            et = model_texture(table + emap * 4u, tu, tv, false);
        }
        r.emis = em * et;
    }
    return r;
}

// Mirrors Shader::model_color.
fn model_color(obj: u32, pos: vec3<f32>, base: vec3<f32>) -> vec3<f32> {
    if (obj == 0u) {
        return base;
    }
    let ob = objs[obj - 1u];
    if (ob.misc.w < 1.5 || u32(ob.mat2.y) != 4u) {
        return base;
    }
    let c = model_surf(obj - 1u, obj_local(obj - 1u, pos), vec3<f32>(0.0), false).base;
    if (c.x < 0.0) {
        return base;
    }
    if (u.bg_rot[1].w > 0.5) {
        return srgb_encode3(c);
    }
    return c;
}

// Mirrors Shader::model_surface: color, specular factor and world normal of a 3D model.
fn model_surface(obj: u32, pos: vec3<f32>, base: vec3<f32>, n: vec3<f32>) -> ModelSurface {
    var r: ModelSurface;
    r.base = base;
    r.spec = 1.0;
    r.n = n;
    r.emis = vec3<f32>(0.0);
    if (obj == 0u) {
        return r;
    }
    let ob = objs[obj - 1u];
    if (ob.misc.w < 1.5 || u32(ob.mat2.y) != 4u) {
        return r;
    }
    // Mirrors compose::normal_to_local / normal_to_world.
    let nl = normalize(vec3<f32>(dot(ob.inv0.xyz, n), dot(ob.inv1.xyz, n), dot(ob.inv2.xyz, n)) * ob.size.xyz);
    let s = model_surf(obj - 1u, obj_local(obj - 1u, pos), nl, true);
    if (!s.found) {
        return r;
    }
    if (s.base.x >= 0.0) {
        r.base = s.base;
        if (u.bg_rot[1].w > 0.5) {
            r.base = srgb_encode3(s.base);
        }
    }
    r.spec = s.spec;
    r.emis = s.emis;
    if (u.bg_rot[1].w > 0.5) {
        r.emis = srgb_encode3(s.emis);
    }
    // The transpose undoes the inverse rotation.
    let m = s.n / ob.size.xyz;
    r.n = normalize(ob.inv0.xyz * m.x + ob.inv1.xyz * m.y + ob.inv2.xyz * m.z);
    return r;
}

// Mirrors ObjectProgram::local (compose::to_local, then compose::repeat_fold).
fn obj_local(j: u32, p: vec3<f32>) -> vec3<f32> {
    let ob = objs[j];
    let d = p - ob.pos.xyz;
    var q = vec3<f32>(dot(ob.inv0.xyz, d), dot(ob.inv1.xyz, d), dot(ob.inv2.xyz, d)) / ob.size.xyz;
    for (var k = 0; k < 3; k++) {
        let c = ob.rep[k];
        if (c > 0.0) {
            var n = floor(q[k] / c + 0.5);
            if (ob.rep_lim[k] > 0.0) {
                n = clamp(n, -ob.rep_lim[k], ob.rep_lim[k]);
            }
            q[k] -= c * n;
        }
    }
    return q;
}

// Mirrors compose::primitive_distance.
fn prim_de(kind: u32, a: vec4<f32>, a2: vec4<f32>, q: vec3<f32>) -> f32 {
    if (kind == 0u) {
        return q.z;
    }
    if (kind == 1u) {
        let r = max(a.w, 0.0);
        let hs = max(a.xyz * 0.5 - vec3<f32>(r), vec3<f32>(0.0));
        let d = abs(q) - hs;
        return length(max(d, vec3<f32>(0.0))) + min(max(d.x, max(d.y, d.z)), 0.0) - r;
    }
    if (kind == 2u) {
        return length(q) - a.x;
    }
    if (kind == 3u) {
        let r = max(a.z, 0.0);
        let dx = length(q.xy) - max(a.x - r, 0.0);
        let dz = abs(q.z) - max(a.y * 0.5 - r, 0.0);
        return min(max(dx, dz), 0.0) + length(max(vec2<f32>(dx, dz), vec2<f32>(0.0))) - r;
    }
    if (kind == 4u) {
        let xy = length(q.xy) - a.x;
        return length(vec2<f32>(xy, q.z)) - a.y;
    }
    if (kind == 6u) {
        let r = max(a.x, 0.0);
        let hh = max(a.y * 0.5 - r, 0.0);
        return length(vec3<f32>(q.xy, q.z - clamp(q.z, -hh, hh))) - r;
    }
    if (kind == 7u) {
        // Capped cone (exact; after Inigo Quilez).
        let r1 = max(a.x, 0.0);
        let r2 = max(a.y, 0.0);
        let h = max(a.z * 0.5, 1.0e-9);
        let qq = vec2<f32>(length(q.xy), q.z);
        let k1 = vec2<f32>(r2, h);
        let k2 = vec2<f32>(r2 - r1, 2.0 * h);
        let ca = vec2<f32>(qq.x - min(qq.x, select(r2, r1, qq.y < 0.0)), abs(qq.y) - h);
        let cb = qq - k1 + k2 * clamp(dot(k1 - qq, k2) / dot(k2, k2), 0.0, 1.0);
        let sg = select(1.0, -1.0, cb.x < 0.0 && ca.y < 0.0);
        return sg * sqrt(min(dot(ca, ca), dot(cb, cb)));
    }
    if (kind == 8u) {
        // Hexagonal prism (exact; after Inigo Quilez).
        let k = vec3<f32>(-0.8660254037844386, 0.5, 0.5773502691896258);
        let r = max(a.x, 0.0);
        var p = abs(q.xy);
        p -= 2.0 * min(dot(k.xy, p), 0.0) * k.xy;
        let ex = p.x - clamp(p.x, -k.z * r, k.z * r);
        let dx = length(vec2<f32>(ex, p.y - r)) * sign(p.y - r);
        let dz = abs(q.z) - a.y * 0.5;
        return min(max(dx, dz), 0.0) + length(max(vec2<f32>(dx, dz), vec2<f32>(0.0)));
    }
    if (kind == 9u) {
        // Equilateral triangle (exact; after Inigo Quilez), extruded along z.
        let k = 1.7320508075688772;
        let r = max(a.x, 0.0) * 0.5;
        var p = vec2<f32>(abs(q.x) - r, q.y + r / k);
        if (p.x + k * p.y > 0.0) {
            p = vec2<f32>(p.x - k * p.y, -k * p.x - p.y) * 0.5;
        }
        p.x -= clamp(p.x, -2.0 * r, 0.0);
        let dx = -length(p) * sign(p.y);
        let dz = abs(q.z) - a.y * 0.5;
        return min(max(dx, dz), 0.0) + length(max(vec2<f32>(dx, dz), vec2<f32>(0.0)));
    }
    return water_distance(a, a2, vec4<f32>(0.0), q, 1.0e30);
}

// Mirrors compose::water_sea01: waves exp(sin - 1) from swells to ripples, dragging the
// point; slow bends and wave groups over large distances (not clamped).
fn water_sea01(a: vec4<f32>, a2: vec4<f32>, x: f32, y: f32) -> f32 {
    let len = max(a.y, 1.0e-6);
    let wind = radians(a.w);
    let drag = max(a2.x, 0.0) * 0.38;
    let g = vec2<f32>(x, y) / (len * 5.0);
    let warp = vec2<f32>(water_noise(g.x, g.y), water_noise(g.x + 31.7, g.y + 17.3)) * 2.0 - 1.0;
    let freq0 = 6.283185307179586 / len;
    var p = (vec2<f32>(x, y) + warp * len * 0.8) * freq0;
    let n = clamp(i32(round(a2.y)), 4, 64);
    let ratio = pow(200.0, 1.0 / f32(max(n - 1, 1)));
    let decay = 1.0 / ratio;
    var f = 0.4;
    var w = 1.0;
    var sum = 0.0;
    var weights = 0.0;
    var squares = 0.0;
    for (var i = 0; i < n; i++) {
        let spread = 0.5 + 1.5 * f32(i) / f32(n);
        let ang = wind + spread * (hash3(i, 0, 11) * 2.0 - 1.0);
        let dir = vec2<f32>(cos(ang), sin(ang));
        let ph = dot(p, dir) * f + a.z * sqrt(f) + hash3(i, 1, 11) * 6.283185307179586;
        let wave = exp(sin(ph) - 1.0);
        let slope = -wave * cos(ph);
        p += dir * slope * w * drag;
        sum += wave * w;
        weights += w;
        squares += w * w;
        w *= decay;
        f *= ratio;
    }
    let z = (sum - 0.4658 * weights) / (0.3025 * sqrt(squares));
    let group = 0.45 + 0.55 * water_noise(x / (len * 7.0) + 5.1, y / (len * 7.0) + 9.3);
    return 0.46 + 0.14 * z * group;
}

// Mirrors compose::water_shore_phase.
fn water_shore_phase(a: vec4<f32>, x: f32, y: f32) -> f32 {
    let len = max(a.y, 1.0e-6);
    let bend = water_noise(x / len * 1.3 + 3.7, y / len * 1.3 + 1.9);
    return a.z * sqrt(1.0 / 0.2) + bend * 6.283185307179586;
}

// Mirrors compose::water_shore: x the addition to the height, y how much the waves break.
fn water_shore(a: vec4<f32>, a3: vec4<f32>, x: f32, y: f32, s_in: f32) -> vec2<f32> {
    let len = max(a.y, 1.0e-6);
    let strength = clamp(a3.x, 0.0, 2.0);
    let range = len * 0.6;
    if (strength <= 0.0 || s_in >= range) {
        return vec2<f32>(0.0);
    }
    let s = max(s_in, 0.0);
    let near = 1.0 - s / range;
    let ph = s * 6.283185307179586 / (len * 0.2) + water_shore_phase(a, x, y);
    let wave = exp((sin(ph) - 1.0) * (1.0 + 2.0 * near));
    let collapse = min(s / (len * 0.15), 1.0);
    let env = near * near * (3.0 - 2.0 * near) * collapse * (2.0 - collapse);
    let height = 0.6 + 0.4 * water_noise(x / len * 1.3 + 8.1, y / len * 1.3 + 2.3);
    return vec2<f32>(strength * 0.22 * env * height * (wave - 0.2), near * near * wave);
}

// Mirrors compose::water_wave01.
fn water_wave01(a: vec4<f32>, a2: vec4<f32>, a3: vec4<f32>, x: f32, y: f32, s: f32) -> f32 {
    return clamp(water_sea01(a, a2, x, y) + water_shore(a, a3, x, y, s).x, 0.0, 1.0);
}

// Mirrors compose::water_height.
fn water_height(a: vec4<f32>, a2: vec4<f32>, a3: vec4<f32>, x: f32, y: f32, s: f32) -> f32 {
    return (water_wave01(a, a2, a3, x, y, s) * 2.0 - 1.0) * a.x;
}

// Mirrors compose::water_distance.
fn water_distance(a: vec4<f32>, a2: vec4<f32>, a3: vec4<f32>, q: vec3<f32>, s: f32) -> f32 {
    let amp = abs(a.x);
    let above = q.z - amp;
    let spray = water_spray(a, a3, q, s);
    if (above > amp) {
        return min(above, spray);
    }
    let len = max(a.y, 1.0e-6);
    let range = len * 0.6;
    let c = max(a2.x, 0.0);
    let steep = amp * 6.283185307179586 / len * (1.6 + c * c * f32(clamp(i32(round(a2.y)), 4, 64)) / 40.0);
    let open = sqrt(1.0 + steep * steep);
    let dz = q.z - water_height(a, a2, a3, q.x, q.y, s);
    var d = dz / open;
    let x = s - range;
    if (a3.x > 0.0 && dz > x * open) {
        let steep_s = open + amp * clamp(a3.x, 0.0, 2.0) / len * 16.0;
        d = dz / steep_s + max(x, 0.0) * (1.0 - open / steep_s);
    }
    if (above > 0.0) {
        d = max(d, above);
    }
    return min(d, spray);
}

// Mirrors compose::water_spray_drift: xy the offset, zw the drift velocity.
fn water_spray_drift(a: vec4<f32>, x: f32, y: f32, drift: f32) -> vec4<f32> {
    let len = max(a.y, 1.0e-6);
    let g = vec2<f32>(x, y) / (len * 1.5);
    let ang = radians(a.w) + (water_noise(g.x + 13.1, g.y + 7.7) * 2.0 - 1.0) * 2.5;
    let far = drift * (0.4 + 0.6 * water_noise(g.x + 2.9, g.y + 21.4));
    let cycle = (water_shore_phase(a, x, y) - 1.5707963267948966) / 6.283185307179586;
    let c = cycle - floor(cycle);
    let t = select((1.0 - c) / 0.35, c / 0.65, c < 0.65);
    let dir = vec2<f32>(cos(ang), sin(ang));
    return vec4<f32>(dir * far * t, dir * far / 0.65);
}

// Mirrors compose::water_spray.
fn water_spray(a: vec4<f32>, a3: vec4<f32>, q: vec3<f32>, s: f32) -> f32 {
    let amp = max(abs(a.x), 1.0e-9);
    let len = max(a.y, 1.0e-6);
    let amount = clamp(a3.y, 0.0, 2.0);
    if (amount <= 0.0 || a3.x <= 0.0) {
        return 1.0e30;
    }
    let size = clamp(a3.z, 0.2, 3.0);
    let vary = clamp(a3.w, 0.0, 1.0);
    let range = len * 0.25;
    let top = amp * 3.0 * amount;
    let cell = amp * 0.6 * max(size, 1.0);
    let r0 = amp * 0.07 * size;
    let drift = top * 0.6;
    let outside = max(s - range - drift, q.z - top - r0);
    if (outside >= cell * 1.6) {
        return outside;
    }
    let w = water_spray_drift(a, q.x, q.y, drift);
    let p = vec3<f32>(q.xy - w.xy, q.z);
    let fc = floor(p.xy / cell);
    let margin = cell * 0.1;
    var d = cell * 1.5 - max(abs(p.x - (fc.x + 0.5) * cell), abs(p.y - (fc.y + 0.5) * cell)) + margin;
    let fall0 = clamp(1.0 - s / range, 0.0, 1.0);
    let fall = fall0 * fall0;
    let lip = 1.0 + 2.0 * r0 / range;
    for (var k = 0; k < 9; k++) {
        let i = i32(fc.x) + k % 3 - 1;
        let j = i32(fc.y) + k / 3 - 1;
        if (hash3(i, j, 31) > 0.55 * min(amount, 1.0)) {
            continue;
        }
        let cx = (f32(i) + 0.5) * cell;
        let cy = (f32(j) + 0.5) * cell;
        let cycle = (water_shore_phase(a, cx, cy) - 1.5707963267948966) / 6.283185307179586;
        let u = (cycle - floor(cycle) - 0.15 * hash3(i, j, 32)) / 0.5;
        if (u < 0.0 || u >= 1.0) {
            continue;
        }
        let reach = cell * 0.5 - r0 - margin;
        let height = top * (0.35 + 0.65 * hash3(i, j, 35));
        let c = vec3<f32>(cx + (hash3(i, j, 33) * 2.0 - 1.0) * reach, cy + (hash3(i, j, 34) * 2.0 - 1.0) * reach, height * 4.0 * u * (1.0 - u));
        let r = r0 * (1.0 - vary * hash3(i, j, 36)) * (1.0 - 0.5 * u) * fall;
        let v = normalize(vec3<f32>(w.zw, height * 4.0 * (1.0 - 2.0 * u) / 0.5));
        let stretch = 1.0 + 1.2 * abs(1.0 - 2.0 * u);
        let e0 = p - c;
        let e = e0 - v * (dot(e0, v) * (1.0 - 1.0 / stretch));
        d = min(d, (length(e) - r) / lip);
    }
    return max(d / (1.0 + drift / len * 6.0), outside);
}

// Mirrors compose::water_fresnel.
fn water_fresnel(cos_i: f32, strength: f32) -> f32 {
    let x = pow(1.0 - clamp(cos_i, 0.0, 1.0), 5.0);
    return min((0.02 + 0.98 * x) * max(strength, 0.0), 1.0);
}

// Mirrors compose::water_noise.
fn water_noise(x: f32, y: f32) -> f32 {
    let i = floor(vec2<f32>(x, y));
    let f = vec2<f32>(x, y) - i;
    let u = f * f * (3.0 - 2.0 * f);
    let ix = i32(i.x);
    let iy = i32(i.y);
    let a = hash3(ix, iy, 7) + (hash3(ix + 1, iy, 7) - hash3(ix, iy, 7)) * u.x;
    let b = hash3(ix, iy + 1, 7) + (hash3(ix + 1, iy + 1, 7) - hash3(ix, iy + 1, 7)) * u.x;
    return a + (b - a) * u.y;
}

// Mirrors compose::water_look: x the height 0..1, y the foam share.
fn water_look(a: vec4<f32>, a2: vec4<f32>, a3: vec4<f32>, q: vec3<f32>, s: f32) -> vec2<f32> {
    let sh = water_shore(a, a3, q.x, q.y, s);
    let h = clamp(water_sea01(a, a2, q.x, q.y) + sh.x, 0.0, 1.0);
    let len = max(a.y, 1.0e-6);
    if (water_spray(a, a3, q, s) < q.z - (h * 2.0 - 1.0) * a.x) {
        return vec2<f32>(1.0, 0.6);
    }
    let crest = clamp((h - 0.62) / 0.16, 0.0, 1.0);
    let crest_foam = crest * crest * max(a2.z, 0.0);
    let strength = clamp(a3.x, 0.0, 1.0);
    var shore_foam = 0.0;
    if (strength > 0.0 && s < len * 0.6) {
        let near = 1.0 - max(s, 0.0) / (len * 0.6);
        let band = len * 0.08 * (0.4 + 1.2 * water_noise(q.x / len * 4.0, q.y / len * 4.0));
        let line = 1.0 - max(s, 0.0) / band;
        shore_foam = strength * clamp(max(max(line, (sh.y - 0.25) / 0.35), near * near * near * 0.7), 0.0, 1.0);
    }
    if (max(crest_foam, shore_foam) <= 0.0) {
        return vec2<f32>(h, 0.0);
    }
    let f = 6.283185307179586 / len;
    let wind = radians(a.w);
    let u = (q.x * cos(wind) + q.y * sin(wind)) * f - a.z * 0.5;
    let v = (q.y * cos(wind) - q.x * sin(wind)) * f;
    let n = 0.6 * water_noise(u * 1.5, v * 4.0) + 0.4 * water_noise(u * 4.0, v * 11.0);
    let broken = clamp((n - 0.3) / 0.35, 0.0, 1.0);
    return vec2<f32>(h, min(max(crest_foam, shore_foam) * broken, 1.0));
}

// Mirrors compose::combine: x the combined distance, y 1 when the object forms the surface.
fn combine_op(op: u32, a: f32, b: f32, k_in: f32) -> vec2<f32> {
    let k = max(k_in, 1.0e-12);
    if (op == 0u) {
        return vec2<f32>(min(a, b), select(0.0, 1.0, b < a));
    }
    if (op == 1u) {
        return vec2<f32>(max(a, -b), select(0.0, 1.0, -b > a));
    }
    if (op == 2u) {
        return vec2<f32>(max(a, b), select(0.0, 1.0, b > a));
    }
    var x = a;
    var y = b;
    var sign = 1.0;
    if (op == 4u) {
        x = -a;
        sign = -1.0;
    } else if (op == 5u) {
        x = -a;
        y = -b;
        sign = -1.0;
    }
    let h = clamp(0.5 + 0.5 * (y - x) / k, 0.0, 1.0);
    let d = y + (x - y) * h - k * h * (1.0 - h);
    return vec2<f32>(sign * d, select(0.0, 1.0, h < 0.5));
}

// Mirrors compose::pattern_value.
fn pattern_value(pattern: u32, size: f32, q: vec3<f32>) -> f32 {
    let s = max(abs(size), 1.0e-9);
    if (pattern == 0u) {
        return 0.0;
    }
    if (pattern == 1u) {
        let c = floor(q.x / s) + floor(q.y / s) + floor(q.z / s + 0.5);
        return c - 2.0 * floor(c / 2.0);
    }
    if (pattern == 2u) {
        return q.z / s;
    }
    if (pattern == 3u) {
        return length(q) / s;
    }
    return 0.0;
}

// How a material picks its colors (mirrors render::cpu::Mat).
struct MatS {
    mode: u32,
    scale: f32,
    offset: f32,
    rep: bool,
    sat: f32,
    pattern: u32,
    psize: f32,
    base: u32,
    // Material index (0 main, k the k-th object's own): glow bands.
    index: u32,
}

fn mat_of(obj: u32) -> MatS {
    var m: MatS;
    if (obj == 0u) {
        m.mode = u32(u.color.x);
        m.scale = u.color.y;
        m.offset = u.color.z;
        m.rep = u.color.w > 0.5;
        m.sat = u.color2.x;
        m.pattern = 0u;
        m.psize = 1.0;
        m.base = 0u;
        m.index = 0u;
        return m;
    }
    let ob = objs[obj - 1u];
    m.mode = u32(ob.mat.x);
    m.scale = ob.mat.y;
    m.offset = ob.mat.z;
    m.rep = ob.mat.w > 0.5;
    m.sat = ob.mat2.x;
    m.pattern = u32(ob.mat2.y);
    m.psize = ob.mat2.z;
    m.base = u32(ob.mat2.w);
    m.index = u32(ob.pos.w);
    return m;
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
    // Mandelbulb3D's second color choice.
    c2: f32,
    // Custom estimates (de custom): `dist` after the last iteration.
    cd: f32,
    // The constant (Julia seed or start point).
    c: vec3<f32>,
    // Deep zoom: the final point and its length in double-float (numeric estimate, mix).
    sd: array<vec2<f32>, 4>,
    rd: vec2<f32>,
}

// Deep zoom mix: the point where the first chain ended, for the second.
var<private> mix_start: array<vec2<f32>, 4>;

// Fixed iteration counts keep a bailout, raised to bailout² · 64 (fractal.rs iterate_from).
var<private> color_bail: bool;

// Mirrors fractal.rs Evaluator::color_orbit (only compiled in with COLOR_ON_IT).
fn color_orbit(p: vec3<f32>, o: Orbit) -> Orbit {
    let n = u32(max(u.c2opt.z, 0.0));
    color_bail = true;
    var q = p;
    if (MAIN_T) {
        q = main_local(p);
    }
    var it = iterate_1(q, n, true, vec4<f32>(0.0), false);
    color_bail = false;
    if (n == 0u) {
        it.s = vec4<f32>(it.c, 0.0);
    }
    let r2 = dot(it.s, it.s);
    var trap = it.trap;
    if (trap >= 3.0e38) {
        trap = r2;
    }
    var r = o;
    r.n = it.n;
    r.r2 = r2;
    r.r2_old = it.rold;
    r.trap = trap;
    r.c2 = color2_value(it.s, it.c, r2, it.rold, trap);
    r.escaped = false;
    return r;
}

// Mirrors fractal.rs color2_value (u.c2opt: option, last length increase multiplier).
fn color2_value(s: vec4<f32>, c: vec3<f32>, r2: f32, rold: f32, trap: f32) -> f32 {
    let opt = u32(u.c2opt.x);
    let pi = 3.14159265358979;
    var v = trap * 4096.0;
    if (opt == 1u) {
        v = log(r2 / (rold + 1.0)) * u.c2opt.y * 512.0;
    } else if (opt == 2u) {
        v = (atan2(s.y - c.y, s.x - c.x) + pi) * 5200.0;
    } else if (opt == 3u) {
        v = (atan2(s.z - c.z, s.x - c.x) + pi) * 5200.0;
    } else if (opt == 4u) {
        v = (atan2(s.z - c.z, s.y - c.y) + pi) * 5200.0;
    } else if (opt == 5u) {
        v = (atan2(s.x, s.y) + pi) * 5215.0;
    }
    if (!is_finite(v)) {
        return 0.0;
    }
    return clamp(v / 32767.0, 0.0, 1.0);
}

struct Hit {
    hit: bool,
    t: f32,
    eps: f32,
    steps: u32,
    glow_steps: u32,
    orbit: Orbit,
    // Glow of emissive colors collected along the ray.
    halo: vec3<f32>,
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

// Mirrors render::srgb_encode.
fn srgb_encode3(c_: vec3<f32>) -> vec3<f32> {
    let c = clamp(c_, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
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
// Brightness of the color map at the last map_color (its emission).
var<private> map_lum: f32;

fn map_color(h: Hit, pos: vec3<f32>, n: vec3<f32>, v: f32, base: vec3<f32>, rough: f32) -> vec3<f32> {
    map_lum = 0.0;
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
                // MB3D's OTrap field holds the second color choice (the orbit trap by default).
                let a = h.orbit.c2 * 32767.0 / 32768.0 - 0.5;
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
    let yc = dot(c, vec3<f32>(0.3, 0.59, 0.11));
    map_lum = yc;
    if (u.cmap2.x > 0.5) {
        let yb = dot(base, vec3<f32>(0.3, 0.59, 0.11));
        return base * (yc / (0.01 + yb));
    }
    return c;
}

// Mirrors render::shimmer: factor of the emitted light at `p` (u.shim: amount, size, phase).
fn shimmer(p: vec3<f32>) -> f32 {
    if (u.shim.x <= 0.0) {
        return 1.0;
    }
    let g = max(u.shim.y, 1.0e-9);
    let step = floor(u.shim.z);
    let f = u.shim.z - step;
    var pat = vec2<f32>(0.0);
    for (var j = 0; j < 2; j++) {
        let k = step + f32(j);
        let q = vec3<f32>(p.x / g + k * 7.31, p.y / g - k * 3.17, p.z / g + k * 5.53);
        pat[j] = (value_noise(q) * 2.0 + value_noise(q * 2.07)) / 3.0;
    }
    let t = f * f * (3.0 - 2.0 * f);
    let n = pat.x + (pat.y - pat.x) * t;
    return max(1.0 + min(u.shim.x, 1.0) * (2.0 * n - 1.0) * 1.5, 0.0);
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
fn combine_de(a: Orbit, b: Orbit, mode: u32, smoothing: f32) -> Orbit {
    let s = max(smoothing, 1.0e-12);
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

// The main fractal's own coordinates (mirrors MainTransform::local).
fn main_local(p: vec3<f32>) -> vec3<f32> {
    let d = p - u.mt_pos.xyz;
    return vec3<f32>(dot(u.mt0.xyz, d), dot(u.mt1.xyz, d), dot(u.mt2.xyz, d));
}

fn orbit(p: vec3<f32>, h: f32) -> Orbit {
    var o: Orbit;
    if (MAIN_T) {
        o = fractal_de(main_local(p), h / u.mt0.w);
        o.de = max(o.de * u.march.w * u.mt0.w, 0.0);
    } else {
        o = fractal_de(p, h);
        o.de = max(o.de * u.march.w, 0.0);
    }
    // Scene composition: the objects in order.
    o = compose_objects(p, h, o);
    var de = o.de;
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
// Low-discrepancy part (Rng::next_f64): per-pixel seed, sample index, draws so far.
var<private> rng_pixel: u32;
var<private> rng_sample: u32;
var<private> rng_n: u32;

fn rnd() -> f32 {
    if (rng_n < 6u) {
        let j = rng_n;
        rng_n += 1u;
        let offset = pcg(rng_pixel ^ ((j / 2u + 1u) * 0x9e3779b9u));
        let step = select(2447445414u, 3242174889u, (j % 2u) == 0u);
        return f32((offset + rng_sample * step) >> 8u) / 16777216.0;
    }
    rng_state = pcg(rng_state);
    return f32(rng_state >> 8u) / 16777216.0;
}

fn eps_at(t: f32) -> f32 {
    return max(u.march.x * u.cam_up.w * t, 1.0e-7);
}

// Mirrors render::hash3.
fn hash3(x: i32, y: i32, z: i32) -> f32 {
    var h = (bitcast<u32>(x) * 0x8da6b343u) ^ (bitcast<u32>(y) * 0xd8163841u) ^ (bitcast<u32>(z) * 0xcb1ab31fu);
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return f32(h >> 8u) / 16777216.0;
}

// Mirrors render::value_noise.
fn value_noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u_ = f * f * (3.0 - 2.0 * f);
    let x = i32(i.x);
    let y = i32(i.y);
    let z = i32(i.z);
    let x00 = mix(hash3(x, y, z), hash3(x + 1, y, z), u_.x);
    let x10 = mix(hash3(x, y + 1, z), hash3(x + 1, y + 1, z), u_.x);
    let x01 = mix(hash3(x, y, z + 1), hash3(x + 1, y, z + 1), u_.x);
    let x11 = mix(hash3(x, y + 1, z + 1), hash3(x + 1, y + 1, z + 1), u_.x);
    return mix(mix(x00, x10, u_.y), mix(x01, x11, u_.y), u_.z);
}

const STEAM_TETRA = array<vec3<f32>, 4>(vec3<f32>(1.0, -1.0, -1.0), vec3<f32>(-1.0, -1.0, 1.0), vec3<f32>(-1.0, 1.0, -1.0), vec3<f32>(1.0, 1.0, 1.0));

// The glow and steam sample of the primary march (Shader::trace, SteamPick): on, seed, total
// weight, picked point, its falloff size, ray distance, falloff toward the surface.
var<private> pick_on: bool;
var<private> pick_seed: u32;
var<private> pick_w: f32;
var<private> pick_pos: vec3<f32>;
var<private> pick_rs: f32;
var<private> pick_t: f32;
var<private> pick_fall: bool;

// The fractal haze of the primary march (SteamPick::haze*): on, light added, share of the light
// behind it that gets through.
var<private> haze_on: bool;
var<private> haze_light: vec3<f32>;
var<private> haze_keep: f32;

// Mirrors Fog::haze_iterations.
fn haze_iterations(ob: Orbit) -> f32 {
    var n = f32(ob.n);
    if (ob.escaped && ob.r2 > 1.0) {
        let lb = log(log(sqrt(max(u.march.z, 4.0))));
        let d = log(0.5 * log(ob.r2));
        var dd = 0.6931472;
        if (ob.r2_old > 1.0) {
            dd = d - log(0.5 * log(ob.r2_old));
        }
        n += clamp((lb - d) / max(dd, 1.0e-6), -1.0, 0.0);
    }
    return n;
}

// Mirrors Fog::haze_density (u.haze2.x: shape, 0 distance; y: size).
fn haze_density(de: f32, n: f32, dist: f32) -> f32 {
    if (u.haze2.x < 0.5) {
        return exp(-max(de, 0.0) / max(u.haze2.y * dist, 1.0e-30));
    }
    let share = n / max(u.frac.z, 1.0);
    if (share > u.haze_col.w) {
        return 0.0;
    }
    let x = clamp((share - u.haze.y) / max(u.haze_col.w - u.haze.y, 1.0e-6), 0.0, 1.0);
    return x * x;
}

// Mirrors SteamPick::offer.
fn pick_offer(step: u32, salt: i32, w: f32, p: vec3<f32>, rs: f32, t: f32, fall: bool) {
    if (w <= 0.0) {
        return;
    }
    pick_w += w;
    if (hash3(i32(step), bitcast<i32>(pick_seed), salt) * pick_w < w) {
        pick_pos = p;
        pick_rs = rs;
        pick_t = t;
        pick_fall = fall;
    }
}

// Mirrors render::steam_density.
fn steam_density(p: vec3<f32>) -> f32 {
    var q = (p + u.emis3.xyz) / max(u.emis2.w, 1.0e-9);
    var n = 0.0;
    var amp = 0.5;
    for (var o = 0; o < 4; o++) {
        n += value_noise(q) * amp;
        q *= 2.03;
        amp *= 0.5;
    }
    n /= 0.9375;
    let lo = 1.0 - clamp(u.emis3.w, 0.0, 1.0) - 0.15;
    let x = clamp((n - lo) / 0.3, 0.0, 1.0);
    return x * x * (3.0 - 2.0 * x);
}

// Mirrors Shader::march_ex (`halo`: collect the glow of emissive colors).
// `primary`: the camera, reflection or bounce ray (refined hit, glow and steam points); else
// an ambient occlusion, emission or scatter ray (mirrors Shader::march_rough).
fn march_ray(o: vec3<f32>, d: vec3<f32>, t0: f32, max_t: f32, max_steps: u32, primary: bool) -> Hit {
    var h: Hit;
    h.hit = false;
    h.halo = vec3<f32>(0.0);
    let halo = primary && pick_on;
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
            if (step > 0u && primary) {
                // Past the refine steps while the point is still where the estimate is zero
                // (mirrors cpu::REFINE_INSIDE_MAX).
                for (var k = 0u; k < u.band.w || (ob_hit.de <= 0.0 && k < 32u); k++) {
                    // Mirrors cpu::REFINE_TOLERANCE.
                    if (dt < eps * (1.0 / 128.0)) {
                        break;
                    }
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
        if (primary && haze_on && ob.obj == 0u) {
            // Mirrors Shader::march_ex: the fractal haze over this step.
            let tm = max(t + 0.5 * last_step, eps);
            let n = haze_iterations(ob);
            let dens = haze_density(ob.de, n, tm);
            if (dens > 1.0e-4) {
                let a = 1.0 - exp(-u.haze.x * dens * last_step / tm);
                var col = u.haze_col.xyz;
                if (u.haze.w > 0.5) {
                    col = gradient(0u, n, t).base;
                }
                haze_light += haze_keep * a * col * u.haze.z;
                haze_keep *= 1.0 - a;
            }
        }
        if (halo) {
            // Mirrors Shader::march_ex: offer the point for the glow and the steam.
            let p = o + d * t;
            let r = max(u.emis.w * t, eps);
            if (u.emis.z > 0.0 && ob.de < 8.0 * r) {
                pick_offer(step, 0x61c0, exp(-ob.de / r) * min(last_step, r) / r * u.emis.z, p, r, t, false);
            }
            let rs = max(u.emis2.z * t, eps);
            if (u.emis2.y > 0.0 && ob.de < 8.0 * rs) {
                pick_offer(step, 0x51ea, exp(-ob.de / rs) * min(last_step, rs) / rs * steam_density(p) * u.emis2.y, p, rs, t, true);
            }
        }
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

// Mirrors Shader::soft_shadow: light reaching `p` from `l` per color channel; glass lets light
// through with GLASS_SHADOW (tinted by its color and the absorption on the way).
fn soft_shadow(p: vec3<f32>, l: vec3<f32>, eps: f32, softness: f32, max_t: f32, dist: f32) -> vec3<f32> {
    var glass = 0.0;
    if (GLASS_SHADOW && u.glass.x > 0.5) {
        glass = u.glow3.w;
    }
    var res = 1.0;
    var tint = vec3<f32>(1.0);
    var crossings = 0u;
    var t = eps * 4.0;
    for (var i = 0u; i < 160u; i++) {
        let q = p + l * t;
        let e = eps_at(t) * 0.5 + eps * 0.5;
        let d = de_at(q, e);
        if (d < e) {
            if (!GLASS_SHADOW || glass <= 0.0 || crossings >= 4u) {
                return vec3<f32>(0.0);
            }
            var hit: Hit;
            hit.hit = true;
            hit.t = t;
            hit.eps = e;
            hit.orbit = orbit(q, e);
            let cols = gradient(hit.orbit.obj, color_value(hit, -l, q), dist + t);
            let through = min(cols.trans * glass, 1.0);
            if (through <= 0.0) {
                return vec3<f32>(0.0);
            }
            // Through the glass to the far side.
            flip = !flip;
            let enter = t;
            t += e * 2.0;
            for (var j = 0u; j < 160u; j++) {
                let e2 = eps_at(t) * 0.5 + eps * 0.5;
                let d2 = de_at(p + l * t, e2);
                if (d2 < e2 || t > max_t) {
                    break;
                }
                t += max(d2 * u.march.y, e2);
            }
            flip = !flip;
            let len = t - enter;
            tint *= through * exp2(len * u.glass.z * log2(max(cols.base, vec3<f32>(1.0e-30))));
            if (max(tint.x, max(tint.y, tint.z)) < 1.0e-3) {
                return vec3<f32>(0.0);
            }
            crossings += 1u;
            t += e * 2.0;
            if (t > max_t) {
                break;
            }
            continue;
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
    return tint * (r * r * (3.0 - 2.0 * r));
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

// Mirrors Shader::ambient_occlusion: x the occlusion factor, yzw the light of emissive colors
// (one hemisphere ray serves both).
fn ambient_occlusion(p: vec3<f32>, n: vec3<f32>, t: f32) -> vec4<f32> {
    let radius = u.ao.z * t;
    var mode = u32(u.ao.x);
    // Fast preview: no traced rays.
    let fast = u.dof.w > 0.5;
    if (fast && mode == 2u) {
        mode = 1u;
    }
    var ao = 1.0;
    if (mode == 1u && !inside_now()) {
        // The fast samples need distances, which inside rendering does not have.
        var occ = 0.0;
        var w = 1.0;
        for (var k = 1u; k <= 5u; k++) {
            let h = radius * f32(k) / 5.0;
            let d = de_at(p + n * h, h * 0.1);
            occ += max(h - d, 0.0) / radius * w;
            w *= 0.6;
        }
        ao = ao_curve(clamp(occ / 0.959, 0.0, 1.0));
    }
    let traced = mode == 2u;
    let light = u.emis.x > 0.0 && !fast;
    var emitted = vec3<f32>(0.0);
    if (traced || light) {
        let u1 = rnd();
        let u2 = rnd();
        let d = cosine_dir(n, u1, u2);
        let eps = eps_at(t);
        var reach = 0.0;
        if (traced) {
            reach = radius;
        }
        if (light) {
            reach = max(reach, u.emis.y * t);
        }
        var steps = 48u;
        if (light) {
            steps = 96u;
        }
        let o = p + n * (eps * 2.0);
        let h = march_ray(o, d, 0.0, reach, steps, false);
        if (h.hit) {
            if (traced && h.t < radius) {
                ao = ao_curve(clamp(1.0 - h.t / radius, 0.0, 1.0));
            }
            if (light) {
                emitted = emission_at(h.orbit, o + d * h.t, h.eps, t + h.t) * u.emis.x;
            }
        }
        // More rays for the emitted light only; the average of all.
        var rays = 1u;
        if (light) {
            rays = clamp(u32(u.emis2.x), 1u, 8u);
        }
        for (var r = 1u; r < rays; r++) {
            let v1 = rnd();
            let v2 = rnd();
            let dr = cosine_dir(n, v1, v2);
            let hr = march_ray(o, dr, 0.0, u.emis.y * t, 96u, false);
            if (hr.hit) {
                emitted += emission_at(hr.orbit, o + dr * hr.t, hr.eps, t + hr.t) * u.emis.x;
            }
        }
        emitted /= f32(rays);
    }
    return vec4<f32>(ao, emitted);
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
    let o = h.orbit;
    let ms = mat_of(o.obj);
    let mode = ms.mode;
    // Objects: primitives by their pattern, fractals with their own bailout.
    var bail2 = u.march.z;
    if (o.obj > 0u) {
        let ob = objs[o.obj - 1u];
        if (ob.misc.w > 0.5) {
            return pattern_value(ms.pattern, ms.psize, obj_local(o.obj - 1u, pos));
        }
        bail2 = ob.misc.x;
    }
    if (mode == 0u) {
        let n_it = f32(o.n) + u.fog3.z * log(h.eps);
        if (o.escaped && o.r2 > 1.0) {
            let lb = log(sqrt(max(bail2, 4.0)));
            return n_it - log2(max((0.5 * log(o.r2)) / lb, 1.0e-10));
        }
        return n_it;
    }
    if (mode == 4u) {
        let n_it = f32(o.n) + u.fog3.z * log(h.eps);
        if (o.escaped && o.r2 > 1.0) {
            let lb = log(log(sqrt(max(bail2, 4.0))));
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
    // Mirrors render::cpu::position_value.
    if (mode == 6u) {
        return length(pos - u.ccenter.xyz);
    }
    if (mode == 5u) {
        return dot(pos - u.ccenter.xyz, u.cdir.xyz);
    }
    if (mode == 7u) {
        return o.c2;
    }
    return n.z * 2.0 + 2.0;
}

struct Colors {
    base: vec3<f32>,
    spec: vec3<f32>,
    trans: f32,
    emis: f32,
}

// Colors of the material of object `obj` (mirrors Shader::gradient).
fn gradient(obj: u32, v: f32, dist: f32) -> Colors {
    let ms = mat_of(obj);
    var t = v * ms.scale + ms.offset + u.color2.z * (dist - u.color2.w);
    if (ms.rep) {
        t = t - floor(t);
    } else {
        t = clamp(t, 0.0, 1.0);
    }
    let x = t * 255.0;
    let i = min(u32(floor(x)), 254u) + ms.base;
    let f = x - f32(i - ms.base);
    let base = mix(lut[i].xyz, lut[i + 1u].xyz, f);
    var c: Colors;
    c.base = mix(lut[ms.base].xyz, base, ms.sat);
    c.spec = mix(lut[i + 256u].xyz, lut[i + 257u].xyz, f);
    c.trans = mix(lut[i + 256u].w, lut[i + 257u].w, f);
    c.emis = mix(lut[i].w, lut[i + 1u].w, f);
    // Glow bands at the exact gradient position (mirrors gradient::apply_bands).
    for (var k = 0u; k < MAX_BANDS; k++) {
        let e = LUT_BANDS + (ms.index * MAX_BANDS + k) * 2u;
        let b1 = lut[e + 1u];
        if (b1.w < 0.5) {
            break;
        }
        let b0 = lut[e];
        var d = abs(t - b0.x);
        if (ms.rep) {
            d = d - floor(d);
            d = min(d, 1.0 - d);
        }
        var w = 0.0;
        if (d < b0.y) {
            if (d <= b0.z) {
                w = 1.0;
            } else {
                let x = (b0.y - d) / (b0.y - b0.z);
                w = x * x * (3.0 - 2.0 * x);
            }
        }
        if (w > 0.0) {
            c.base = c.base + (b1.xyz - c.base) * w;
            c.emis = c.emis + (b0.w - c.emis) * w;
        }
    }
    return c;
}

// Glow bands (renderer.rs bands_gpu): per material MAX_BANDS bands of 2 entries from LUT_BANDS.
const LUT_BANDS: u32 = 5500u;
const MAX_BANDS: u32 = 4u;

// Texture layers (mirrors rmb-core texture.rs). Per material m (0 main, k the k-th object's
// own) 4 layers of 7 entries in `lut` from LUT_LAYERS: [0] mapping (< 0: no more layers),
// space, blend, target; [1] scale, opacity, strength, mask contrast; [2] offset, image (-1:
// solid color); [3] color, mask (-1: none); [4..6] rotation rows, [4].w mask invert, [5].w
// texels per tile, [6].w the image's alpha multiplies the opacity. Targets: 0 color, 1 bump,
// 2 emission, 3 specular, 4 roughness, 5 normal map.
const LUT_LAYERS: u32 = 5248u;

fn layer_base(m: u32, i: u32) -> u32 {
    return LUT_LAYERS + (m * 4u + i) * 7u;
}

// Layer images are stored under the numbers 0xffffffef - index.
fn layer_map(idx: f32) -> i32 {
    if (idx < 0.0) {
        return -1;
    }
    return map_find_u(0xffffffefu - u32(round(idx)));
}

// Mirrors texture::bilinear.
fn map_bilinear(i: i32, pu: f32, pv: f32) -> vec3<f32> {
    return map_bilinear4(i, pu, pv).xyz;
}

// Mirrors texture::bilinear and texture::bilinear_alpha (alpha in w).
fn map_bilinear4(i: i32, pu: f32, pv: f32) -> vec4<f32> {
    let h0 = 1u + 4u * u32(i);
    let w = i32(maps[h0 + 1u]);
    let h = i32(maps[h0 + 2u]);
    let base = maps[h0 + 3u];
    let x = (pu - floor(pu)) * f32(w) - 0.5;
    let y = (pv - floor(pv)) * f32(h) - 0.5;
    let xf = floor(x);
    let yf = floor(y);
    let fx = x - xf;
    let fy = y - yf;
    var c: array<vec4<f32>, 4>;
    for (var k = 0; k < 4; k++) {
        let xx = (((i32(xf) + (k & 1)) % w) + w) % w;
        let yy = (((i32(yf) + (k >> 1u)) % h) + h) % h;
        let p = maps[base + u32(yy * w + xx)];
        c[k] = vec4<f32>(f32((p >> 16u) & 255u), f32((p >> 8u) & 255u), f32(p & 255u), f32(p >> 24u));
    }
    return mix(mix(c[0], c[1], fx), mix(c[2], c[3], fx), fy) / 255.0;
}

fn luminance(c: vec3<f32>) -> f32 {
    return 0.3 * c.x + 0.59 * c.y + 0.11 * c.z;
}

struct UvSets {
    a: vec3<f32>,
    b: vec3<f32>,
    c: vec3<f32>,
}

fn layer_rot(b: u32, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(lut[b + 4u].xyz, v), dot(lut[b + 5u].xyz, v), dot(lut[b + 6u].xyz, v));
}

// Mirrors texture::uv_sets: (u, v, weight) of up to three projections.
fn uv_sets(b: u32, q: vec3<f32>, n: vec3<f32>, value: f32, trap: f32) -> UvSets {
    let mapping = u32(lut[b].x);
    let s = lut[b + 1u].x;
    let o = lut[b + 2u].xyz;
    let d = layer_rot(b, q);
    let a = d * s + o;
    var r: UvSets;
    r.b = vec3<f32>(0.0);
    r.c = vec3<f32>(0.0);
    let tau = 6.283185307179586;
    if (mapping == 0u) {
        let rn = layer_rot(b, n);
        var w = rn * rn * rn * rn;
        let sum = w.x + w.y + w.z;
        if (sum > 1.0e-12) {
            w = w / sum;
        } else {
            w = vec3<f32>(1.0 / 3.0);
        }
        r.a = vec3<f32>(a.y, a.z, w.x);
        r.b = vec3<f32>(a.x, a.z, w.y);
        r.c = vec3<f32>(a.x, a.y, w.z);
    } else if (mapping == 1u) {
        r.a = vec3<f32>(a.x, a.y, 1.0);
    } else if (mapping == 2u) {
        r.a = vec3<f32>(a.x, a.z, 1.0);
    } else if (mapping == 3u) {
        r.a = vec3<f32>(a.y, a.z, 1.0);
    } else if (mapping == 4u) {
        let len = max(length(d), 1.0e-30);
        r.a = vec3<f32>(atan2(d.y, d.x) / tau * s + o.x, acos(clamp(d.z / len, -1.0, 1.0)) / 3.141592653589793 * s + o.y, 1.0);
    } else if (mapping == 5u) {
        r.a = vec3<f32>(atan2(d.y, d.x) / tau * s + o.x, d.z * s + o.y, 1.0);
    } else {
        r.a = vec3<f32>(value * s + o.x, sqrt(max(trap, 0.0)) * 4.0 * s + o.y, 1.0);
    }
    return r;
}

// Loops keep one call site of the lookups (smaller shaders compile faster).
fn sample_sets(i: i32, s: UvSets) -> vec4<f32> {
    var out = vec4<f32>(0.0);
    for (var k = 0u; k < 3u; k++) {
        var t = s.a;
        if (k == 1u) {
            t = s.b;
        } else if (k == 2u) {
            t = s.c;
        }
        if (t.z > 0.0) {
            out += map_bilinear4(i, t.x, t.y) * t.z;
        }
    }
    return out;
}

// Mirrors texture::layer_at: color (display values) and opacity.
fn layer_at(b: u32, s: UvSets) -> vec4<f32> {
    var c = lut[b + 3u].xyz;
    var a = lut[b + 1u].y;
    // Pass 0: the image, pass 1: the mask.
    for (var k = 0u; k < 2u; k++) {
        let img = layer_map(select(lut[b + 2u].w, lut[b + 3u].w, k == 1u));
        if (img < 0) {
            continue;
        }
        let v4 = sample_sets(img, s);
        let v = v4.xyz;
        if (k == 0u) {
            c = v;
            if (lut[b + 6u].w > 0.5) {
                a *= v4.w;
            }
        } else {
            var m = luminance(v);
            if (lut[b + 4u].w > 0.5) {
                m = 1.0 - m;
            }
            a *= clamp((m - 0.5) * lut[b + 1u].w + 0.5, 0.0, 1.0);
        }
    }
    return vec4<f32>(c, a);
}

// Mirrors texture::blend.
fn blend_mode(mode: u32, b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    switch (mode) {
        case 1u: { return b * s; }
        case 2u: { return 1.0 - (1.0 - b) * (1.0 - s); }
        case 3u: { return select(1.0 - 2.0 * (1.0 - b) * (1.0 - s), 2.0 * b * s, b < vec3<f32>(0.5)); }
        case 4u: { return (1.0 - 2.0 * s) * b * b + 2.0 * s * b; }
        case 5u: { return b + s; }
        case 6u: { return max(b - s, vec3<f32>(0.0)); }
        case 7u: { return min(b, s); }
        case 8u: { return max(b, s); }
        case 9u: { return abs(b - s); }
        case 10u: { return max(s + (luminance(b) - luminance(s)), vec3<f32>(0.0)); }
        default: { return s; }
    }
}

// Duff et al. 2017 (mirrors texture::tangents).
fn tangent_a(n: vec3<f32>) -> vec3<f32> {
    let sg = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (sg + n.z);
    return vec3<f32>(1.0 + sg * n.x * n.x * a, sg * n.x * n.y * a, -sg * n.x);
}

fn tangent_b(n: vec3<f32>) -> vec3<f32> {
    let sg = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (sg + n.z);
    return vec3<f32>(n.x * n.y * a, sg + n.y * n.y * a, -n.y);
}

struct Layered {
    base: vec3<f32>,
    emis: f32,
    // Factor of the highlights; the roughness becomes rough * rough_keep + rough_add.
    spec: f32,
    rough_keep: f32,
    rough_add: f32,
}

// Material index and object coordinates of a point on object `obj`, with normal `n`.
struct LayerSpace {
    m: u32,
    q: vec3<f32>,
    nq: vec3<f32>,
}

fn layer_space(obj: u32, pos: vec3<f32>, n: vec3<f32>) -> LayerSpace {
    var r: LayerSpace;
    r.m = 0u;
    r.q = pos;
    r.nq = n;
    if (obj > 0u) {
        let ob = objs[obj - 1u];
        r.m = u32(ob.pos.w);
        r.q = obj_local(obj - 1u, pos);
        r.nq = vec3<f32>(dot(ob.inv0.xyz, n), dot(ob.inv1.xyz, n), dot(ob.inv2.xyz, n));
    }
    return r;
}

// Mirrors Shader::apply_layers (color and emission; the bump is bump_layers): the texture
// layers of the material of object `obj` at `pos` with world normal `n` (zero: unknown).
fn apply_layers(obj: u32, pos: vec3<f32>, n: vec3<f32>, cv: f32, trap: f32, base: vec3<f32>, emis: f32) -> Layered {
    var r: Layered;
    r.base = base;
    r.emis = emis;
    r.spec = 1.0;
    r.rough_keep = 1.0;
    r.rough_add = 0.0;
    let ls = layer_space(obj, pos, n);
    let decode = u.bg_rot[1].w < 0.5;
    for (var i = 0u; i < 4u; i++) {
        let b = layer_base(ls.m, i);
        if (lut[b].x < 0.0) {
            break;
        }
        let tgt = u32(lut[b].w);
        if (tgt == 1u || tgt == 5u) {
            continue;
        }
        let world = lut[b].y > 0.5;
        let l = layer_at(b, uv_sets(b, select(ls.q, pos, world), select(ls.nq, n, world), cv, trap));
        if (l.w <= 0.0) {
            continue;
        }
        if (tgt == 2u) {
            r.emis += lut[b + 1u].z * luminance(l.xyz) * l.w;
            continue;
        }
        if (tgt == 3u) {
            r.spec *= 1.0 + (luminance(l.xyz) - 1.0) * l.w;
            continue;
        }
        if (tgt == 4u) {
            r.rough_keep *= 1.0 - l.w;
            r.rough_add = r.rough_add * (1.0 - l.w) + luminance(l.xyz) * l.w;
            continue;
        }
        var c = l.xyz;
        if (decode && lut[b + 2u].w >= 0.0) {
            c = srgb_decode3(c);
        }
        r.base = mix(r.base, blend_mode(u32(lut[b].z), r.base, c), l.w);
    }
    return r;
}

// Brightness of layer `b` at `q` (bump height).
fn layer_height(b: u32, q: vec3<f32>, n: vec3<f32>, cv: f32, trap: f32) -> f32 {
    return luminance(layer_at(b, uv_sets(b, q, n, cv, trap)).xyz);
}

// Mirrors texture::bump: the world normal `n` bent by the bump layers.
fn bump_layers(obj: u32, pos: vec3<f32>, n: vec3<f32>, cv: f32, trap: f32) -> vec3<f32> {
    let ls = layer_space(obj, pos, n);
    var nn = n;
    var bent = false;
    for (var i = 0u; i < 4u; i++) {
        let b = layer_base(ls.m, i);
        if (lut[b].x < 0.0) {
            break;
        }
        let tgt = u32(lut[b].w);
        if ((tgt != 1u && tgt != 5u) || u32(lut[b].x) == 6u) {
            continue;
        }
        let world = lut[b].y > 0.5;
        let lq = select(ls.q, pos, world);
        let ln = select(ls.nq, n, world);
        if (length(ln) < 0.5) {
            continue;
        }
        let a = layer_at(b, uv_sets(b, lq, ln, cv, trap)).w;
        if (a <= 0.0) {
            continue;
        }
        let e = 1.0 / (max(abs(lut[b + 1u].x), 1.0e-9) * lut[b + 5u].w);
        let t1 = tangent_a(ln);
        let t2 = tangent_b(ln);
        if (tgt == 5u) {
            // Normal map (mirrors texture::bump): per projection the directions in which u and
            // v grow, the image's normal in that frame.
            let img = layer_map(lut[b + 2u].w);
            if (img < 0) {
                continue;
            }
            let s0 = uv_sets(b, lq, ln, cv, trap);
            let s1 = uv_sets(b, lq + t1 * e, ln, cv, trap);
            let s2 = uv_sets(b, lq + t2 * e, ln, cv, trap);
            var acc = vec3<f32>(0.0);
            for (var k = 0u; k < 3u; k++) {
                var t0 = s0.a;
                var ta = s1.a;
                var tb = s2.a;
                if (k == 1u) {
                    t0 = s0.b;
                    ta = s1.b;
                    tb = s2.b;
                } else if (k == 2u) {
                    t0 = s0.c;
                    ta = s1.c;
                    tb = s2.c;
                }
                if (t0.z <= 0.0) {
                    continue;
                }
                let tu = t1 * (ta.x - t0.x) + t2 * (tb.x - t0.x);
                let tv = t1 * (ta.y - t0.y) + t2 * (tb.y - t0.y);
                if (length(tu) < 1.0e-30 || length(tv) < 1.0e-30) {
                    continue;
                }
                let c = map_bilinear(img, t0.x, t0.y) * 2.0 - 1.0;
                acc += normalize((normalize(tu) * c.x - normalize(tv) * c.y) * lut[b + 1u].z + ln * max(c.z, 0.0)) * t0.z;
            }
            if (length(acc) < 1.0e-30) {
                continue;
            }
            var dn = (normalize(acc) - ln) * a;
            if (!world && obj > 0u) {
                let ob = objs[obj - 1u];
                dn = ob.inv0.xyz * dn.x + ob.inv1.xyz * dn.y + ob.inv2.xyz * dn.z;
            }
            nn = nn + dn;
            bent = true;
            continue;
        }
        // The four neighbours in one loop (one call site keeps the shader small).
        var g = vec2<f32>(0.0);
        for (var k = 0u; k < 4u; k++) {
            let dir = select(t1, t2, k >= 2u);
            let sg = select(1.0, -1.0, (k & 1u) == 1u);
            let hv = layer_height(b, lq + dir * (e * sg), ln, cv, trap) * sg * 0.5;
            if (k < 2u) {
                g.x += hv;
            } else {
                g.y += hv;
            }
        }
        var gw = (t1 * g.x + t2 * g.y) * (lut[b + 1u].z * a);
        if (!world && obj > 0u) {
            let ob = objs[obj - 1u];
            gw = ob.inv0.xyz * gw.x + ob.inv1.xyz * gw.y + ob.inv2.xyz * gw.z;
        }
        nn = nn - gw;
        bent = true;
    }
    if (!bent) {
        return n;
    }
    return normalize(nn);
}

// Mirrors Shader::emission_at: emitted radiance for an orbit at `pos`.
fn emission_at(ob: Orbit, pos: vec3<f32>, eps: f32, t: f32) -> vec3<f32> {
    if (u.interior.x > 0.5 && !ob.escaped && ob.obj == 0u) {
        return vec3<f32>(0.0);
    }
    var h: Hit;
    h.orbit = ob;
    h.eps = eps;
    h.t = t;
    let cv = color_value(h, vec3<f32>(0.0), pos);
    var c = gradient(ob.obj, cv, t);
    if (MODELS) {
        c.base = model_color(ob.obj, pos, c.base);
    }
    if (LAYERS) {
        let l = apply_layers(ob.obj, pos, vec3<f32>(0.0), cv, ob.trap, c.base, c.emis);
        c.base = l.base;
        c.emis = l.emis;
    }
    if (c.emis > 0.0) {
        c.emis *= shimmer(pos);
    }
    return c.base * c.emis;
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

struct LightSample {
    dir: vec3<f32>,
    atten: f32,
    dist: f32,
}

// Mirrors FrameLight::sample.
fn light_sample(i: u32, p: vec3<f32>, r: vec2<f32>) -> LightSample {
    let li = u.lights[i];
    var s: LightSample;
    let kind = u32(li.color.w + 0.5);
    if (kind == 0u) {
        s.dir = li.dir.xyz;
        s.atten = 1.0;
        s.dist = u.cam_fwd.w;
        return s;
    }
    var q = li.dir.xyz;
    if (kind == 3u) {
        q = q + li.shape1.xyz * (2.0 * r.x - 1.0) + li.shape2.xyz * (2.0 * r.y - 1.0);
    } else if (kind == 4u) {
        q = q + (li.shape0.xyz - q) * r.x;
    }
    let v = q - p;
    let d2 = max(dot(v, v), 1.0e-30);
    s.dist = sqrt(d2);
    s.dir = v / s.dist;
    s.atten = 1.0 / d2;
    if (kind == 2u) {
        let x = clamp((-dot(s.dir, li.shape0.xyz) - li.shape0.w) / max(li.shape1.x - li.shape0.w, 1.0e-9), 0.0, 1.0);
        s.atten *= x * x * (3.0 - 2.0 * x);
    } else if (kind == 3u) {
        s.atten *= max(-dot(s.dir, li.shape0.xyz), 0.0);
    }
    return s;
}

// Mirrors render::nearest_on_segment.
fn nearest_on_segment(a: vec3<f32>, b: vec3<f32>, o: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    let e = b - a;
    let w = a - o;
    let ee = dot(e, e);
    let ed = dot(e, d);
    let den = ee - ed * ed;
    var s = 0.5;
    if (abs(den) >= 1.0e-12 * max(ee, 1.0e-30)) {
        s = clamp((ed * dot(w, d) - dot(w, e)) / den, 0.0, 1.0);
    }
    return a + e * s;
}

// Mirrors render::light_glow; applies the glow of light `i` to `c`.
fn light_glow(i: u32, o: vec3<f32>, d: vec3<f32>, hit: bool, hit_t: f32, fog_col: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    let li = u.lights[i];
    let r2 = li.glow.y;
    let point = li.color.w > 0.5;
    let kind = u32(li.color.w + 0.5);
    // Area lights: the glowing rectangle (front side) covers what is behind.
    if (kind == 3u) {
        let n = li.shape0.xyz;
        let den = dot(d, n);
        if (den >= 0.0) {
            return c;
        }
        let t = dot(li.dir.xyz - o, n) / den;
        if (t <= 0.0 || (hit && hit_t < t)) {
            return c;
        }
        let q = o + d * t - li.dir.xyz;
        let su = li.shape1.xyz;
        let sv = li.shape2.xyz;
        if (abs(dot(q, su) / max(dot(su, su), 1.0e-30)) > 1.0 || abs(dot(q, sv) / max(dot(sv, sv), 1.0e-30)) > 1.0) {
            return c;
        }
        let fl = max(fog_amount(t), 0.0);
        return fog_col * fl + li.glow_col.xyz * (1.0 - fl);
    }
    var lp = li.dir.xyz;
    if (kind == 4u) {
        lp = nearest_on_segment(li.dir.xyz, li.shape0.xyz, o, d);
    }
    var u_ = 0.0;
    var t_light = u.cam_fwd.w;
    if (point) {
        let v = lp - o;
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
// The fog distance in 0..end where the clamped fog amount reaches `goal` (Fog::distance_for).
fn fog_distance_for(goal: f32, end: f32) -> f32 {
    if (u.fog2.x < 0.5) {
        return clamp(-log(max(1.0 - goal, 1.0e-30)) / max(u.fog.w, 1.0e-30), 0.0, end);
    }
    var lo = 0.0;
    var hi = end;
    for (var i = 0; i < 24; i++) {
        let mid = 0.5 * (lo + hi);
        if (clamp(fog_amount(mid), 0.0, 1.0) < goal) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    return 0.5 * (lo + hi);
}

// How fast the clamped fog amount grows at distance `t` (Fog::density_at).
fn fog_density_at(t: f32) -> f32 {
    if (u.fog2.x < 0.5) {
        return u.fog.w * exp(-u.fog.w * t);
    }
    let h = 1.0e-3 * max(u.cam_fwd.w - u.scales.w, 1.0e-9);
    let lo = max(t - h, 0.0);
    return (clamp(fog_amount(t + h), 0.0, 1.0) - clamp(fog_amount(lo), 0.0, 1.0)) / (t + h - lo);
}

// Henyey-Greenstein phase of the light scattering (Fog::scatter_phase).
fn scatter_phase(cos_t: f32) -> f32 {
    let g = clamp(u.fog4.y, -0.95, 0.95);
    return (1.0 - g * g) / pow(max(1.0 + g * g - 2.0 * g * cos_t, 1.0e-12), 1.5);
}

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
    // The surface's own reflection (water); negative: the material's reflection amount.
    mirror: f32,
}

// Foam on water crests and the sharpness of water highlights (mirror render::cpu).
const WATER_FOAM: vec3<f32> = vec3<f32>(0.85, 0.88, 0.9);
const WATER_SHININESS: f32 = 400.0;

// Mirrors Shader::trace: one ray from `o` along `d` (distances `t0`..`max_t`; fog, glow and
// visible lights measured from `t_base`; `inside`: inside glass; `glass`: light amount that
// scales the diffuse light down by the transparency).
// Mirrors scene::RenderMode::PathTraced (fast navigation previews are shaded classically).
fn path_on() -> bool {
    return u.path.x > 0.5 && u.dof.w < 0.5;
}

// Mirrors Shader::environment: the light of the surroundings for path tracing.
fn environment(d: vec3<f32>) -> vec3<f32> {
    var a = mix(u.ground.xyz, u.sky.xyz, dot(d, u.amb_up.xyz) * 0.5 + 0.5);
    if (u.bg2.z > 0.5 && u.bg_rot[0].w > 0.5) {
        a = background_ambient(d);
    }
    return a * u.ao.w;
}

// Mirrors render::cpu::path_specular_share.
fn path_specular_share(amount: f32, spec: vec3<f32>, trans: f32, glass: f32) -> f32 {
    if (amount <= 0.0) {
        return 0.0;
    }
    let s = amount * (spec.x + spec.y + spec.z) / 3.0;
    var g = 0.0;
    if (glass > 0.0) {
        g = trans * glass;
    }
    return clamp(max(s, g), 0.0, 0.9);
}

fn trace(o: vec3<f32>, d: vec3<f32>, t0: f32, max_t: f32, t_base: f32, sx: f32, sy: f32, inside: bool, glass: f32, env: bool) -> Traced {
    var tr: Traced;
    tr.hit = false;
    tr.mirror = -1.0;
    // Glow and steam (mirrors Shader::trace): pick a point near the glowing surfaces, a random
    // ray from it finds the emitted light of the surface it came from.
    let steam_on = u.emis.z > 0.0 || u.emis2.y > 0.0;
    pick_w = 0.0;
    pick_seed = 0u;
    if (steam_on) {
        pick_seed = u32(rnd() * 16777216.0);
    }
    pick_on = steam_on;
    // Fractal haze along the ray (not inside glass).
    let hz = u.haze.x > 0.0 && !inside;
    haze_on = hz;
    haze_light = vec3<f32>(0.0);
    haze_keep = 1.0;
    var h = march_ray(o, d, t0, max_t, u.band.z, true);
    pick_on = false;
    haze_on = false;
    if (steam_on) {
        let u1 = rnd();
        let u2 = rnd();
        if (pick_w > 0.0) {
            let hh = pick_rs * 0.05;
            var g = vec3<f32>(0.0);
            for (var k = 0u; k < 4u; k++) {
                g += STEAM_TETRA[k] * de_at(pick_pos + STEAM_TETRA[k] * hh, eps_at(pick_t) * 0.5);
            }
            var n = -d;
            if (length(g) > 0.0) {
                n = -normalize(g);
            }
            let dir = cosine_dir(n, u1, u2);
            let sh = march_ray(pick_pos, dir, 0.0, 8.0 * pick_rs, 64u, false);
            if (sh.hit) {
                var f = pick_w;
                if (pick_fall) {
                    f *= exp(-sh.t / pick_rs);
                }
                h.halo += emission_at(sh.orbit, pick_pos + dir * sh.t, sh.eps, pick_t + sh.t) * f;
            }
        }
    }
    let glow = u.glow.w * max(f32(h.glow_steps) - u.glow3.x, 0.0) / f32(max(u.band.z, 1u));
    var glow_mix = 0.0;
    if (u.glow2.w > 0.0) {
        glow_mix = min(f32(h.glow_steps) / u.glow2.w, 1.0);
    }
    let grad = background(d, sy);
    var bg = grad;
    if (env) {
        bg = environment(d);
    } else if (u.bg_rot[0].w > 0.5) {
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
        // Mandelbulb3D's color on iteration: the colors from another point of the orbit.
        if (COLOR_ON_IT && h.orbit.obj == 0u) {
            h.orbit = color_orbit(pos, h.orbit);
        }
        var n = nr.xyz;
        if (dot(n, d) > 0.0) {
            n = -n;
        }
        // Roughness dulls the highlights and softens the diffuse curves.
        var rr = nr.w * u.blend.w;
        let cv = color_value(h, n, pos);
        var cols = gradient(h.orbit.obj, cv, h.t);
        // 3D models: their colors, specular and bump maps, normal maps, emission.
        var n_model = n;
        var model_emis = vec3<f32>(0.0);
        if (MODELS) {
            let ms = model_surface(h.orbit.obj, pos, cols.base, n);
            cols.base = ms.base;
            cols.spec *= ms.spec;
            n_model = ms.n;
            model_emis = ms.emis;
        }
        // Interior colors where the orbit did not escape (mirrors Shader::interior_colors).
        if (u.interior.x > 0.5 && !h.orbit.escaped && h.orbit.obj == 0u) {
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
            cols.emis = 0.0;
        }
        if (LAYERS) {
            let lay = apply_layers(h.orbit.obj, pos, n_model, cv, h.orbit.trap, cols.base, cols.emis);
            cols.base = lay.base;
            cols.emis = lay.emis;
            cols.spec *= lay.spec;
            rr = rr * lay.rough_keep + lay.rough_add;
        }
        let spec_rough = 1.0 + min(2.0 * rr, 1.0) * (4.0 / max(u.sky.w, 1.0) - 1.0);
        var n_bump = n_model;
        if (BUMP) {
            n_bump = bump_layers(h.orbit.obj, pos, n_model, cv, h.orbit.trap);
        }
        if (u.cmap0.x > 0.5 && h.orbit.obj == 0u) {
            cols.base = map_color(h, pos, n, cv, cols.base, rr);
            cols.emis += map_lum * u.shim.w;
        }
        // Shimmer: the emitted light flickers over the surface (mirrors Shader::trace).
        if (cols.emis > 0.0) {
            cols.emis *= shimmer(pos);
        }
        // Water: foam on the crests, and its own (Fresnel) reflection.
        var mirror = -1.0;
        if (WATER && h.orbit.obj > 0u && is_water(h.orbit.obj - 1u)) {
            let j = h.orbit.obj - 1u;
            let look = water_look(objs[j].prim, objs[j].prim2, objs[j].prim3, obj_local(j, pos), h.orbit.shore);
            let lifted = cols.base * (0.55 + 0.9 * look.x);
            cols.base = lifted + (WATER_FOAM - lifted) * look.y;
            mirror = water_fresnel(-dot(d, n), objs[j].prim2.w) * (1.0 - look.y);
        }
        let base = cols.base;
        let p = pos + n * (h.eps * select(1.5, 4.0, inside_now()));
        // Path tracing: the bounces bring the ambient and emitted light.
        var aol = vec4<f32>(1.0, 0.0, 0.0, 0.0);
        if (!path_on()) {
            aol = ambient_occlusion(p, n, h.t);
        }
        let ao = aol.x;
        // Lighting uses the bumped normal.
        n = n_bump;
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
        var ambient = u.ao.w;
        if (path_on()) {
            ambient = 0.0;
        }
        var diffuse = amb * ambient * ao + aol.yzw;
        var spec = vec3<f32>(0.0);
        var shin = u.sky.w;
        // Water glints: a far sharper sun highlight than the scene's shininess.
        if (mirror >= 0.0) {
            shin = max(shin, WATER_SHININESS);
        }
        for (var i = 0u; i < u.size.w; i++) {
            let li = u.lights[i];
            // Direction, falloff and shadow length (mirrors FrameLight::sample); area and line
            // lights at a random point of the light.
            var r = vec2<f32>(0.5);
            if (li.color.w > 2.5) {
                let r1 = rnd();
                let r2 = rnd();
                r = vec2<f32>(r1, r2);
            }
            let ls = light_sample(i, p, r);
            if (ls.atten <= 0.0) {
                continue;
            }
            let ldir = ls.dir;
            let atten = ls.atten;
            let max_l = ls.dist;
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
            var sh = vec3<f32>(1.0);
            if (li.opts.x > 0.5 && u.dof.w < 0.5) {
                sh = soft_shadow(p, ldir, h.eps, li.opts.y, max_l, h.t);
            }
            if (max(sh.x, max(sh.y, sh.z)) <= 0.0) {
                continue;
            }
            let w = atten * (1.0 - li.opts.w * (1.0 - ao));
            let hv = normalize(ldir - d);
            var s = 0.0;
            if (ndl > 0.0) {
                s = pow(max(dot(n, hv), 0.0), shin) * li.dir.w * u.ground.w * (shin + 8.0) / 25.0 * spec_rough;
            }
            diffuse += li.color.xyz * df * w * sh;
            spec += li.color.xyz * s * w * sh;
        }
        // What the reflection takes (water) does not reach the surface color.
        let keep = max(1.0 - cols.trans * glass, 0.0) * (1.0 - max(mirror, 0.0));
        // Emissive colors shine by themselves.
        let lit = base * (diffuse * keep + cols.emis) + spec * cols.spec + model_emis;
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
        tr.mirror = mirror;
    }
    // Glow and steam of the emissive colors the ray passed (strengths included).
    c += h.halo;
    // Light scattered by the fog (mirrors Shader::trace).
    if (!inside && u.fog4.x > 0.0 && u.size.w > 0u) {
        let ut = rnd();
        let ul = rnd();
        var r = vec2<f32>(rnd(), rnd());
        let far = u.cam_fwd.w;
        var end = far;
        if (h.hit) {
            end = h.t - t_base;
        }
        let fa = clamp(fog_amount(0.0), 0.0, 1.0);
        let fb = clamp(fog_amount(end), 0.0, 1.0);
        let i = min(u32(ul * f32(u.size.w)), u.size.w - 1u);
        let li = u.lights[i];
        let kind = u32(li.color.w + 0.5);
        let start = o + d * t_base;
        if (fb - fa > 1.0e-6) {
            var s = 0.0;
            var amount = 0.0;
            if (kind != 0u) {
                // Equiangular sampling around the light.
                var center = li.dir.xyz;
                if (kind == 4u) {
                    center = center + (li.shape0.xyz - center) * 0.5;
                }
                let delta = dot(center - start, d);
                let dist = max(length(center - start - d * delta), 1.0e-6 * end);
                let ta = atan(-delta / dist);
                let tb = atan((end - delta) / dist);
                s = delta + dist * tan(ta + ut * (tb - ta));
                let pdf = dist / (max(tb - ta, 1.0e-12) * (dist * dist + (s - delta) * (s - delta)));
                amount = fog_density_at(s) / pdf;
                s = clamp(s, 0.0, end);
            } else {
                s = fog_distance_for(fa + ut * (fb - fa), end);
                amount = fb - fa;
            }
            let x = start + d * s;
            if (kind < 3u) {
                r = vec2<f32>(0.5);
            }
            let ls = light_sample(i, x, r);
            if (ls.atten > 0.0) {
                var sh = vec3<f32>(1.0);
                if (li.opts.x > 0.5 && u.dof.w < 0.5) {
                    sh = soft_shadow(x, ls.dir, eps_at(t_base + s), li.opts.y, ls.dist, t_base + s);
                }
                let w = ls.atten * scatter_phase(dot(ls.dir, d)) * u.fog4.x * amount * f32(u.size.w);
                c += li.color.xyz * w * sh;
            }
        }
    }
    // The fractal haze in front of everything the ray saw (mirrors Shader::trace).
    if (hz) {
        c = c * haze_keep + haze_light;
        tr.albedo = tr.albedo * haze_keep + (1.0 - haze_keep) * clamp(haze_light / max(1.0 - haze_keep, 1.0e-9), vec3<f32>(0.0), vec3<f32>(1.0));
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

// Mirrors Dof::lens_point: a point on the unit lens (disc, or the polygon of the blades).
fn lens_point(u1: f32, u2: f32) -> vec2<f32> {
    let blades = u.dof2.x;
    if (blades < 3.0) {
        let r = sqrt(u1);
        let a = u2 * 6.283185307179586;
        return vec2<f32>(r * cos(a), r * sin(a));
    }
    let k = min(floor(u1 * blades), blades - 1.0);
    let v = u1 * blades - k;
    let a0 = u.dof2.y + 6.283185307179586 * k / blades;
    let a1 = u.dof2.y + 6.283185307179586 * (k + 1.0) / blades;
    let s = sqrt(v);
    let p = s * (1.0 - u2);
    let q = s * u2;
    return vec2<f32>(p * cos(a0) + q * cos(a1), p * sin(a0) + q * sin(a1));
}

// Mirrors ViewFrame::eye_screen: screen coordinates within one eye's image and the eye
// (-1 left, 1 right, 0 without stereo) for the image position (fx, fy) and image k.
fn eye_screen(fx: f32, fy: f32, k: u32) -> vec3<f32> {
    let mode = u32(u.view.z);
    var ew = f32(u.tile.z);
    var eh = f32(u.tile.w);
    var x = fx;
    var y = fy;
    var eye = 0.0;
    if (mode == 1u) {
        eye = select(1.0, -1.0, k == 0u);
    } else if (mode == 2u) {
        ew = f32(max(u.tile.z / 2u, 1u));
        eye = select(1.0, -1.0, fx < ew);
        x = select(fx - ew, fx, fx < ew);
    } else if (mode == 3u) {
        eh = f32(max(u.tile.w / 2u, 1u));
        eye = select(1.0, -1.0, fy < eh);
        y = select(fy - eh, fy, fy < eh);
    }
    if (u.view.w > 0.5) {
        eye = -eye;
    }
    return vec3<f32>(x / ew * 2.0 - 1.0, 1.0 - y / eh * 2.0, eye);
}

// Mirrors ViewFrame::ray: the projection's direction in camera coordinates (xyz) and the
// eyes' axis (w: 0 outside a fisheye circle, else 1); `side` gets the axis.
var<private> view_side: vec3<f32>;
fn view_dir(sx: f32, sy: f32, th: f32) -> vec4<f32> {
    let proj = u32(u.view.x);
    let aspect = u.cam_right.w;
    view_side = vec3<f32>(1.0, 0.0, 0.0);
    if (proj == 0u) {
        return vec4<f32>(normalize(vec3<f32>(sx * th * aspect, sy * th, 1.0)), 1.0);
    }
    if (proj <= 2u) {
        let x = sx * aspect;
        let y = sy;
        let r = sqrt(x * x + y * y);
        let theta = r * u.view.y;
        if ((proj == 2u && r > 1.0) || theta > 3.141592653589793) {
            return vec4<f32>(0.0, 0.0, 1.0, 0.0);
        }
        var s = u.view.y;
        if (r > 1.0e-12) {
            s = sin(theta) / r;
        }
        return vec4<f32>(x * s, y * s, cos(theta), 1.0);
    }
    var lon = sx * 1.5707963267948966;
    if (proj == 3u) {
        lon = sx * 3.141592653589793;
    }
    let lat = sy * 1.5707963267948966;
    let c80 = 0.17364817766693041;
    let fade = clamp((cos(lat) - c80) / (0.5 - c80), 0.0, 1.0);
    view_side = vec3<f32>(cos(lon), 0.0, -sin(lon)) * fade;
    return vec4<f32>(cos(lat) * sin(lon), sin(lat), cos(lat) * cos(lon), 1.0);
}

// Mirrors Shader::sample_aux: anaglyphs render both eyes per sample and mix them.
fn sample_image(px: u32, py: u32, sample: u32) -> vec3<f32> {
    let n = u32(u.view2.w);
    var cs: array<vec3<f32>, 2>;
    var albs: array<vec3<f32>, 2>;
    var nrm = vec3<f32>(0.0);
    var dep = 0.0;
    var hit = 0.0;
    for (var k = 0u; k < n; k++) {
        cs[k] = sample_pixel(px, py, sample, k);
        albs[k] = aux_albedo;
        nrm += aux_normal;
        dep += aux_depth;
        hit += aux_hit;
    }
    if (n < 2u) {
        return cs[0];
    }
    aux_normal = nrm * 0.5;
    aux_depth = dep * 0.5;
    aux_hit = hit * 0.5;
    aux_albedo = anaglyph(albs[0], albs[1]);
    return anaglyph(cs[0], cs[1]);
}

fn anaglyph(l: vec3<f32>, r: vec3<f32>) -> vec3<f32> {
    var red = l.x;
    if (u.view2.z < 0.5) {
        red = 0.299 * l.x + 0.587 * l.y + 0.114 * l.z;
    }
    return vec3<f32>(red, r.y, r.z);
}

fn sample_pixel(px: u32, py: u32, sample: u32, k: u32) -> vec3<f32> {
    rng_state = pcg(px ^ pcg(py ^ pcg(sample)));
    rng_pixel = pcg(px ^ pcg(py ^ 0x5bd1e995u));
    rng_sample = sample;
    rng_n = 0u;
    var jx = 0.5;
    var jy = 0.5;
    if (sample > 0u) {
        jx = rnd();
        jy = rnd();
    }
    let es = eye_screen(f32(px) + jx, f32(py) + jy, k);
    let sx = es.x;
    let sy = es.y;
    var th = u.cam_pos.w;
    var o = u.cam_pos.xyz;
    var cf = u.cam_fwd.xyz;
    var cr = u.cam_right.xyz;
    var cu = u.cam_up.xyz;
    // Motion blur: a random time within the exposure (Camera::lerp).
    if (u.mot_pos.w > 0.5) {
        let tm = rnd();
        o = mix(o, u.mot_pos.xyz, tm);
        cf = normalize(mix(cf, u.mot_fwd.xyz, tm));
        cr = normalize(mix(cr, u.mot_right.xyz, tm));
        cu = normalize(mix(cu, u.mot_up.xyz, tm));
        th = tan(mix(atan(th), atan(u.mot_fwd.w), tm));
    }
    let vd = view_dir(sx, sy, th);
    if (vd.w < 0.5) {
        aux_albedo = vec3<f32>(0.0);
        aux_normal = vec3<f32>(0.0);
        aux_depth = 0.0;
        aux_hit = 0.0;
        return vec3<f32>(0.0);
    }
    var d = normalize(cr * vd.x + cu * vd.y + cf * vd.z);
    if (es.z != 0.0 && u.view2.x > 0.0) {
        // Both eyes look at the point the center ray meets at the convergence distance.
        var reach = u.view2.y;
        if (u32(u.view.x) == 0u) {
            reach = u.view2.y / vd.z;
        }
        let target_p = o + d * reach;
        o = o + (cr * view_side.x + cu * view_side.y + cf * view_side.z) * (es.z * u.view2.x);
        d = normalize(target_p - o);
    }
    if (u.dof.x > 0.5 && sample > 0u) {
        let plane = u32(u.view.x) == 0u;
        var lr = cr;
        var lu = cu;
        var focus = o + d * u.dof.z;
        if (plane) {
            focus = o + d * (u.dof.z / dot(d, cf));
        } else {
            // Mirrors lens_axes.
            let r0 = cross(d, cu);
            if (length(r0) > 1.0e-6) {
                lr = normalize(r0);
            }
            lu = normalize(cross(lr, d));
        }
        let u1 = rnd();
        let u2 = rnd();
        let lp = lens_point(u1, u2);
        let r = u.dof.y * u.dof.z * 0.1;
        o = o + lr * (r * lp.x) + lu * (r * lp.y);
        d = normalize(focus - o);
    }
    // Without reflections (most scenes) a single trace: compiles much faster.
    if (!REFLECT) {
        let tr = trace(o, d, u.scales.w, u.cam_fwd.w, 0.0, sx, sy, false, 0.0, false);
        aux_albedo = tr.albedo;
        aux_normal = tr.normal;
        aux_depth = select(0.0, tr.t, tr.hit);
        aux_hit = select(0.0, 1.0, tr.hit);
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
    // Mirrors Shader::sample_aux: path tracing continues every path (diffuse or reflection /
    // glass bounces, Russian roulette, firefly clamp).
    let path = path_on();
    var bounces = 0u;
    if (path) {
        bounces = clamp(u32(u.path.y), 1u, 16u);
    } else if (amount > 0.0 || WATER) {
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
        let tr = trace(ro, rd, t0, max_t, t_base, sxr, syr, inside, g, path && b > 0u);
        if (b == 0u) {
            aux_albedo = tr.albedo;
            aux_normal = tr.normal;
            aux_depth = select(0.0, tr.t, tr.hit);
            aux_hit = select(0.0, 1.0, tr.hit);
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
            var add = weight * tr.c;
            let y = dot(add, vec3<f32>(0.2126, 0.7152, 0.0722));
            if (path && u.path.z > 0.0 && y > u.path.z) {
                add *= u.path.z / y;
            }
            c += add;
        }
        if (b == bounces || !tr.hit) {
            break;
        }
        let vis = clamp(1.0 - tr.fog, 0.0, 1.0);
        var bn: Bounce;
        var diffuse = false;
        // Water reflects by its own amount.
        var amt = amount;
        if (tr.mirror >= 0.0) {
            amt = tr.mirror;
        }
        if (path) {
            var ps = path_specular_share(amount, tr.spec, tr.trans, glass);
            if (tr.mirror >= 0.0) {
                ps = tr.mirror;
            }
            var spec_lobe = false;
            if (ps > 0.0) {
                spec_lobe = rnd() < ps;
            }
            if (spec_lobe) {
                bn = bounce(tr, inside);
                weight *= bn.f * amt / ps * vis;
            } else {
                let u1 = rnd();
                let u2 = rnd();
                bn.dir = cosine_dir(tr.n, u1, u2);
                bn.refract = false;
                diffuse = true;
                let keep = max(1.0 - tr.trans * glass, 0.0) * (1.0 - max(tr.mirror, 0.0));
                weight *= tr.base * keep / (1.0 - ps) * vis;
            }
            if (b >= 2u) {
                let q = clamp(max(max(weight.x, weight.y), weight.z), 0.05, 1.0);
                if (rnd() > q) {
                    break;
                }
                weight /= q;
            }
        } else {
            bn = bounce(tr, inside);
            weight *= bn.f * amt * vis;
        }
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
        // Sky reach: diffuse rays beyond it see the open sky (Lighting::path_reach).
        if (diffuse && u.path.w > 0.0) {
            max_t = tr.t + min(u.path.w * tr.t, u.cam_fwd.w);
        }
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
// A random micro facet normal around `n` for the GGX roughness `alpha` (cpu::ggx_normal).
fn ggx_normal(n: vec3<f32>, alpha: f32, u1: f32, u2: f32) -> vec3<f32> {
    var a = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(n.z) < 0.999) {
        a = vec3<f32>(0.0, 0.0, 1.0);
    }
    let t = normalize(cross(a, n));
    let b = cross(n, t);
    let c2 = clamp((1.0 - u1) / (1.0 + (alpha * alpha - 1.0) * u1), 0.0, 1.0);
    let ct = sqrt(c2);
    let st = sqrt(1.0 - c2);
    let phi = u2 * 6.283185307179586;
    return normalize(t * (st * cos(phi)) + b * (st * sin(phi)) + n * ct);
}

fn bounce(s_in: Traced, inside: bool) -> Bounce {
    var r: Bounce;
    var s = s_in;
    // Rough surfaces: a random micro facet (mirrors Shader::bounce).
    if (u.gloss.x > 0.0) {
        let u1 = rnd();
        let u2 = rnd();
        let f = ggx_normal(s.n, u.gloss.x * u.gloss.x, u1, u2);
        if (dot(f, s.dir) < 0.0) {
            s.n = f;
        }
    }
    r.dir = s.dir - s.n * (2.0 * dot(s.dir, s.n));
    if (dot(r.dir, s_in.n) < 0.0) {
        r.dir = r.dir - s_in.n * (2.0 * dot(r.dir, s_in.n));
    }
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
    // 1.0 the compiler cannot see (df64.wgsl).
    dd_one = u.deep_hi.w;
    let px = gid.x;
    let py = gid.y + u.band.x;
    if (px >= u.size.x || gid.y >= u.band.y || py >= u.size.y) {
        return;
    }
    // Pixels of the full image (tiles): same rays and random numbers as without tiles.
    var c = sample_image(px + u.tile.x, py + u.tile.y, u.size.z);
    if (!(is_finite(c.x) && is_finite(c.y) && is_finite(c.z))) {
        c = vec3<f32>(0.0);
    }
    let idx = py * u.size.x + px;
    let dh = aux_depth * aux_hit;
    if (u.size.z == 0u) {
        accum[idx] = vec4<f32>(c, 1.0);
        aux[2u * idx] = vec4<f32>(aux_albedo, dh);
        aux[2u * idx + 1u] = vec4<f32>(aux_normal, aux_hit);
    } else {
        accum[idx] += vec4<f32>(c, 1.0);
        aux[2u * idx] += vec4<f32>(aux_albedo, dh);
        aux[2u * idx + 1u] += vec4<f32>(aux_normal, aux_hit);
    }
}

// Mesh export (rmb-core mesh_export): the distance estimates of one grid slice, packed four per
// accum entry. Size and band rows as for march_main; its own pipeline, compiled only for
// exports.
@compute @workgroup_size(8, 8)
fn grid_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    dd_one = u.deep_hi.w;
    let x = gid.x;
    let y = gid.y + u.band.x;
    if (x >= u.size.x || gid.y >= u.band.y || y >= u.size.y) {
        return;
    }
    let p = u.grid.xyz + vec3<f32>(f32(x), f32(y), u.grid2.x) * u.grid.w;
    let i = y * u.size.x + x;
    accum[i >> 2u][i & 3u] = de_at(p, u.grid.w * 0.5);
}
