// SPDX-License-Identifier: LGPL-2.1-or-later
//
// Averages the accumulated samples and applies the post processing of
// rmb-core/src/render/mod.rs (`post_process`).

struct P {
    a: vec4<f32>,    // exposure, gamma, contrast, saturation
    b: vec4<f32>,    // tone map, vignette, display space
    size: vec4<u32>, // width, height, lens flare layer width and height (0: none)
    bloom: vec4<f32>, // strength (0: off), pyramid levels, threshold
    lv: array<vec4<u32>, 6>, // bloom levels: first texel, width, height
    lw: array<vec4<f32>, 6>, // bloom level weights
}

@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> accum: array<vec4<f32>>;
@group(0) @binding(2) var out_tex: texture_storage_2d<rgba8unorm, write>;
// Lens flares (rmb-core flare.rs), added before the post processing like HdrImage::add_flares.
@group(0) @binding(3) var<storage, read> flare: array<vec4<f32>>;
// The bloom pyramid (bloom.wgsl).
@group(0) @binding(4) var<storage, read> levels: array<vec4<f32>>;

// Mirrors bloom::sample: bilinear, edges clamped.
fn bloom_sample(k: u32, u: f32, v: f32) -> vec3<f32> {
    let l = p.lv[k];
    let fx = clamp(u * f32(l.y) - 0.5, 0.0, f32(l.y) - 1.0);
    let fy = clamp(v * f32(l.z) - 0.5, 0.0, f32(l.z) - 1.0);
    let x0 = u32(floor(fx));
    let y0 = u32(floor(fy));
    let x1 = min(x0 + 1u, l.y - 1u);
    let y1 = min(y0 + 1u, l.z - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let a = levels[l.x + y0 * l.y + x0].xyz;
    let b = levels[l.x + y0 * l.y + x1].xyz;
    let c = levels[l.x + y1 * l.y + x0].xyz;
    let d = levels[l.x + y1 * l.y + x1].xyz;
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    return top + (bottom - top) * ty;
}

fn aces(x: f32) -> f32 {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

@compute @workgroup_size(8, 8)
fn display_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= p.size.x || gid.y >= p.size.y) {
        return;
    }
    let a = accum[gid.y * p.size.x + gid.x];
    var lin = a.xyz / max(a.w, 1.0);
    if (p.size.z > 0u && p.size.w > 0u) {
        let fx = min(gid.x * p.size.z / p.size.x, p.size.z - 1u);
        let fy = min(gid.y * p.size.w / p.size.y, p.size.w - 1u);
        lin += flare[fy * p.size.z + fx].xyz;
    }
    if (p.bloom.x > 0.0) {
        let u = (f32(gid.x) + 0.5) / f32(p.size.x);
        let v = (f32(gid.y) + 0.5) / f32(p.size.y);
        var s = vec3<f32>(0.0);
        for (var k = 0u; k < u32(p.bloom.y); k++) {
            s += bloom_sample(k, u, v) * p.lw[k].x;
        }
        lin += s * p.bloom.x;
    }
    var c = max(lin * exp2(p.a.x), vec3<f32>(0.0));
    if (p.b.x > 0.5) {
        c = vec3<f32>(aces(c.x), aces(c.y), aces(c.z));
    }
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    c = luma + (c - luma) * p.a.w;
    // Display space: the values are already gamma encoded.
    var g = 1.0 / max(p.a.y, 0.1);
    if (p.b.z > 0.5) {
        g = 2.2 / max(p.a.y, 0.1);
    }
    let du = (f32(gid.x) + 0.5) / f32(p.size.x) - 0.5;
    let dv = (f32(gid.y) + 0.5) / f32(p.size.y) - 0.5;
    let vig = max(1.0 - p.b.y * (du * du + dv * dv) * 2.0, 0.0);
    let d = pow(max(c, vec3<f32>(0.0)), vec3<f32>(g));
    let o = clamp(((d - 0.5) * p.a.z + 0.5) * vig, vec3<f32>(0.0), vec3<f32>(1.0));
    textureStore(out_tex, vec2<i32>(gid.xy), vec4<f32>(o, 1.0));
}
