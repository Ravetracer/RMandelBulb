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
    slots: array<vec4<u32>, 8>,  // x: iterations
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
}

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> prm: array<f32>;
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

struct Orbit {
    de: f32,
    n: u32,
    r2: f32,
    r2_old: f32,
    trap: f32,
    escaped: bool,
    // The object that forms the surface: 0 the main fractal, k the k-th composed object.
    obj: u32,
}

// A composed object (rmb-core compose.rs, filled by renderer.rs).
struct Obj {
    inv0: vec4<f32>,  // inverse rotation rows; w: distance scale, operation, smoothing
    inv1: vec4<f32>,
    inv2: vec4<f32>,
    pos: vec4<f32>,   // w: material (0 main, k the k-th object's own)
    prim: vec4<f32>,  // primitive parameters
    frac: vec4<f32>,  // fractal: w slice, julia, max iterations, repeat from
    julia: vec4<f32>,
    misc: vec4<f32>,  // bailout², formula DE scale, DE scale, kind (0 fractal, 1 primitive, 2 model)
    mat: vec4<f32>,   // material: color mode, scale, offset, repeat
    mat2: vec4<f32>,  // saturation, pattern, pattern size, LUT base
    slots: array<vec4<u32>, 2>,
    size: vec4<f32>,  // scale per own axis
}

@group(0) @binding(6) var<storage, read> objs: array<Obj>;

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
    let m = vols[h + 19u] + mat * 8u;
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
    // The transpose undoes the inverse rotation.
    let m = s.n / ob.size.xyz;
    r.n = normalize(ob.inv0.xyz * m.x + ob.inv1.xyz * m.y + ob.inv2.xyz * m.z);
    return r;
}

// Mirrors compose::to_local.
fn obj_local(j: u32, p: vec3<f32>) -> vec3<f32> {
    let ob = objs[j];
    let d = p - ob.pos.xyz;
    return vec3<f32>(dot(ob.inv0.xyz, d), dot(ob.inv1.xyz, d), dot(ob.inv2.xyz, d)) / ob.size.xyz;
}

// Mirrors compose::primitive_distance.
fn prim_de(kind: u32, a: vec4<f32>, q: vec3<f32>) -> f32 {
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
    // Water: three octaves of waves at turning directions.
    let len = max(a.y, 1.0e-6);
    let angle = radians(a.w);
    var hgt = 0.0;
    var slope = 0.0;
    for (var i = 0; i < 3; i++) {
        let f = exp2(f32(i));
        let dir = angle + f32(i) * 1.1;
        let k = 6.283185307179586 * f / len;
        let amp = a.x / f;
        hgt += amp * sin(k * (q.x * cos(dir) + q.y * sin(dir)) + a.z * (1.0 + 0.3 * f32(i)));
        slope += amp * k;
    }
    return (q.z - hgt) / sqrt(1.0 + slope * slope);
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
    o.de = max(o.de * u.march.w, 0.0);
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

fn rnd() -> f32 {
    rng_state = pcg(rng_state);
    return f32(rng_state >> 8u) / 16777216.0;
}

fn eps_at(t: f32) -> f32 {
    return max(u.march.x * u.cam_up.w * t, 1.0e-7);
}

// Mirrors Shader::march_ex (`halo`: collect the glow of emissive colors).
fn march_ray(o: vec3<f32>, d: vec3<f32>, t0: f32, max_t: f32, max_steps: u32, halo_on: bool) -> Hit {
    var h: Hit;
    h.hit = false;
    h.halo = vec3<f32>(0.0);
    let halo = halo_on && u.emis.z > 0.0;
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
        if (halo) {
            let r = max(u.emis.w * t, eps);
            if (ob.de < 8.0 * r) {
                let e = emission_at(ob, o + d * t, eps, t);
                h.halo += e * (exp(-ob.de / r) * min(last_step, r) / r);
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
        let h = march_ray(p + n * (eps * 2.0), d, 0.0, reach, steps, false);
        if (h.hit) {
            if (traced && h.t < radius) {
                ao = ao_curve(clamp(1.0 - h.t / radius, 0.0, 1.0));
            }
            if (light) {
                emitted = emission_at(h.orbit, p + n * (eps * 2.0) + d * h.t, h.eps, t + h.t) * u.emis.x;
            }
        }
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
    return c;
}

// Texture layers (mirrors rmb-core texture.rs). Per material m (0 main, k the k-th object's
// own) 4 layers of 7 entries in `lut` from LUT_LAYERS: [0] mapping (< 0: no more layers),
// space, blend, target; [1] scale, opacity, strength, mask contrast; [2] offset, image (-1:
// solid color); [3] color, mask (-1: none); [4..6] rotation rows, [4].w mask invert, [5].w
// texels per tile.
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
    var c: array<vec3<f32>, 4>;
    for (var k = 0; k < 4; k++) {
        let xx = (((i32(xf) + (k & 1)) % w) + w) % w;
        let yy = (((i32(yf) + (k >> 1u)) % h) + h) % h;
        let p = maps[base + u32(yy * w + xx)];
        c[k] = vec3<f32>(f32((p >> 16u) & 255u), f32((p >> 8u) & 255u), f32(p & 255u));
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
fn sample_sets(i: i32, s: UvSets) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    for (var k = 0u; k < 3u; k++) {
        var t = s.a;
        if (k == 1u) {
            t = s.b;
        } else if (k == 2u) {
            t = s.c;
        }
        if (t.z > 0.0) {
            out += map_bilinear(i, t.x, t.y) * t.z;
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
        let v = sample_sets(img, s);
        if (k == 0u) {
            c = v;
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
    let ls = layer_space(obj, pos, n);
    let decode = u.bg_rot[1].w < 0.5;
    for (var i = 0u; i < 4u; i++) {
        let b = layer_base(ls.m, i);
        if (lut[b].x < 0.0) {
            break;
        }
        let tgt = u32(lut[b].w);
        if (tgt == 1u) {
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
        if (u32(lut[b].w) != 1u || u32(lut[b].x) == 6u) {
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
    let h = march_ray(o, d, t0, max_t, u.band.z, true);
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
        var cols = gradient(h.orbit.obj, cv, h.t);
        // 3D models: their colors, specular and bump maps.
        var n_model = n;
        if (MODELS) {
            let ms = model_surface(h.orbit.obj, pos, cols.base, n);
            cols.base = ms.base;
            cols.spec *= ms.spec;
            n_model = ms.n;
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
        }
        var n_bump = n_model;
        if (BUMP) {
            n_bump = bump_layers(h.orbit.obj, pos, n_model, cv, h.orbit.trap);
        }
        if (u.cmap0.x > 0.5 && h.orbit.obj == 0u) {
            cols.base = map_color(h, pos, n, cv, cols.base, rr);
        }
        let base = cols.base;
        let p = pos + n * (h.eps * select(1.5, 4.0, inside_now()));
        let aol = ambient_occlusion(p, n, h.t);
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
        var diffuse = amb * u.ao.w * ao + aol.yzw;
        var spec = vec3<f32>(0.0);
        let shin = u.sky.w;
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
        // Emissive colors shine by themselves.
        let lit = base * (diffuse * keep + cols.emis) + spec * cols.spec;
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
    // Glow of the emissive colors the ray passed.
    c += h.halo * u.emis.z;
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
