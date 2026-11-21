// SPDX-License-Identifier: LGPL-2.1-or-later
//
// Averages the accumulated samples and applies the post processing of
// rmb-core/src/render/mod.rs (`post_process`).

struct P {
    a: vec4<f32>,    // exposure, gamma, contrast, saturation
    b: vec4<f32>,    // tone map, vignette
    size: vec4<u32>, // width, height
}

@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var<storage, read> accum: array<vec4<f32>>;
@group(0) @binding(2) var out_tex: texture_storage_2d<rgba8unorm, write>;

fn aces(x: f32) -> f32 {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

@compute @workgroup_size(8, 8)
fn display_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= p.size.x || gid.y >= p.size.y) {
        return;
    }
    let a = accum[gid.y * p.size.x + gid.x];
    var c = max(a.xyz / max(a.w, 1.0) * exp2(p.a.x), vec3<f32>(0.0));
    if (p.b.x > 0.5) {
        c = vec3<f32>(aces(c.x), aces(c.y), aces(c.z));
    }
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    c = luma + (c - luma) * p.a.w;
    let g = 1.0 / max(p.a.y, 0.1);
    let du = (f32(gid.x) + 0.5) / f32(p.size.x) - 0.5;
    let dv = (f32(gid.y) + 0.5) / f32(p.size.y) - 0.5;
    let vig = max(1.0 - p.b.y * (du * du + dv * dv) * 2.0, 0.0);
    let d = pow(max(c, vec3<f32>(0.0)), vec3<f32>(g));
    let o = clamp(((d - 0.5) * p.a.z + 0.5) * vig, vec3<f32>(0.0), vec3<f32>(1.0));
    textureStore(out_tex, vec2<i32>(gid.xy), vec4<f32>(o, 1.0));
}
