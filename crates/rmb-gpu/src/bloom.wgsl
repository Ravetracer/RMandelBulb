// SPDX-License-Identifier: LGPL-2.1-or-later
//
// Bloom pyramid of rmb-core/src/render/bloom.rs: the light above the threshold averaged into
// levels (4 × 4 blocks, then 2 × 2), each blurred with the 5-tap binomial filter. display.wgsl
// adds the levels to the image.

struct BP {
    dst: vec4<u32>, // first texel, width, height of the level written
    src: vec4<u32>, // first texel, width, height of the level read
    img: vec4<u32>, // image width, height, lens flare layer width, height (0: none)
    prm: vec4<f32>, // exposure factor, threshold
}

@group(0) @binding(0) var<uniform> bp: BP;
@group(0) @binding(1) var<storage, read> accum: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> flare: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> levels: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> tmp: array<vec4<f32>>;

// The radiance display.wgsl shows (with the lens flares), negative and invalid values as 0.
fn radiance(x: u32, y: u32) -> vec3<f32> {
    let a = accum[y * bp.img.x + x];
    var lin = a.xyz / max(a.w, 1.0);
    if (bp.img.z > 0u && bp.img.w > 0u) {
        let fx = min(x * bp.img.z / bp.img.x, bp.img.z - 1u);
        let fy = min(y * bp.img.w / bp.img.y, bp.img.w - 1u);
        lin += flare[fy * bp.img.z + fx].xyz;
    }
    // NaN fails every comparison.
    let ok = (lin == lin) & (abs(lin) < vec3<f32>(3.0e38));
    return select(vec3<f32>(0.0), max(lin, vec3<f32>(0.0)), ok);
}

// Mirrors bloom::bright_factor.
fn bright(c: vec3<f32>) -> vec3<f32> {
    let y = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)) * bp.prm.x;
    return c * (max(y - bp.prm.y, 0.0) / max(y, 1.0e-6));
}

@compute @workgroup_size(8, 8)
fn bloom_bright(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= bp.dst.y || gid.y >= bp.dst.z) {
        return;
    }
    var s = vec3<f32>(0.0);
    var n = 0.0;
    for (var y = gid.y * 4u; y < min(gid.y * 4u + 4u, bp.img.y); y++) {
        for (var x = gid.x * 4u; x < min(gid.x * 4u + 4u, bp.img.x); x++) {
            s += bright(radiance(x, y));
            n += 1.0;
        }
    }
    levels[bp.dst.x + gid.y * bp.dst.y + gid.x] = vec4<f32>(s / max(n, 1.0), 0.0);
}

@compute @workgroup_size(8, 8)
fn bloom_down(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= bp.dst.y || gid.y >= bp.dst.z) {
        return;
    }
    var s = vec3<f32>(0.0);
    var n = 0.0;
    for (var y = gid.y * 2u; y < min(gid.y * 2u + 2u, bp.src.z); y++) {
        for (var x = gid.x * 2u; x < min(gid.x * 2u + 2u, bp.src.y); x++) {
            s += levels[bp.src.x + y * bp.src.y + x].xyz;
            n += 1.0;
        }
    }
    levels[bp.dst.x + gid.y * bp.dst.y + gid.x] = vec4<f32>(s / max(n, 1.0), 0.0);
}

const TAPS = array<f32, 5>(0.0625, 0.25, 0.375, 0.25, 0.0625);

@compute @workgroup_size(8, 8)
fn bloom_blur_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= bp.dst.y || gid.y >= bp.dst.z) {
        return;
    }
    var s = vec3<f32>(0.0);
    for (var t = 0; t < 5; t++) {
        let x = u32(clamp(i32(gid.x) + t - 2, 0, i32(bp.dst.y) - 1));
        s += levels[bp.dst.x + gid.y * bp.dst.y + x].xyz * TAPS[t];
    }
    tmp[bp.dst.x + gid.y * bp.dst.y + gid.x] = vec4<f32>(s, 0.0);
}

@compute @workgroup_size(8, 8)
fn bloom_blur_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= bp.dst.y || gid.y >= bp.dst.z) {
        return;
    }
    var s = vec3<f32>(0.0);
    for (var t = 0; t < 5; t++) {
        let y = u32(clamp(i32(gid.y) + t - 2, 0, i32(bp.dst.z) - 1));
        s += tmp[bp.dst.x + y * bp.dst.y + gid.x].xyz * TAPS[t];
    }
    levels[bp.dst.x + gid.y * bp.dst.y + gid.x] = vec4<f32>(s, 0.0);
}
