//! Checks the double-float library (df64.wgsl) on the GPU against f64 on the CPU: the driver
//! must keep the error-free transformations intact.

use wgpu::util::DeviceExt;

const FUNCS: [&str; 20] = [
    "dd_divf(a, 3.5)",
    "dd_sqrt(dd_sub(dd_f(1.0), dd_sqr(dd_divf(a, 3.5))))",
    "dd_add(a, b)",
    "dd_sub(a, b)",
    "dd_mul(a, b)",
    "dd_div(a, b)",
    "dd_sqrt(dd_abs(a))",
    "dd_exp(a)",
    "dd_log(dd_abs(a))",
    "dd_sin(a)",
    "dd_cos(a)",
    "dd_atan2(a, b)",
    "dd_pow(dd_abs(a), b)",
    "dd_pow(a, dd_f(8.0))",
    "dd_asin(dd_divf(a, 3.5))",
    "dd_acos(dd_divf(a, 3.5))",
    "dd_tanh(a)",
    "dd_floor(dd_mulf(a, 1000.0))",
    "dd_rem(a, b)",
    "dd_sinh(a)",
];

fn reference(k: usize, a: f64, b: f64) -> f64 {
    let k = match k {
        0 => return a / 3.5,
        1 => return (1.0 - (a / 3.5).powi(2)).sqrt(),
        k => k - 2,
    };
    match k {
        0 => a + b,
        1 => a - b,
        2 => a * b,
        3 => a / b,
        4 => a.abs().sqrt(),
        5 => a.exp(),
        6 => a.abs().ln(),
        7 => a.sin(),
        8 => a.cos(),
        9 => a.atan2(b),
        10 => a.abs().powf(b),
        11 => a.powi(8),
        12 => (a / 3.5).asin(),
        13 => (a / 3.5).acos(),
        14 => a.tanh(),
        15 => (a * 1000.0).floor(),
        16 => a % b,
        _ => a.sinh(),
    }
}

/// Splits an f64 into hi + lo f32.
fn split(v: f64) -> (f32, f32) {
    let hi = v as f32;
    (hi, (v - hi as f64) as f32)
}

#[test]
fn df64_matches_f64() {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    // Values with a full low part (not representable in f32).
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut rnd = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let n = 512;
    let mut vals = Vec::new();
    let mut input: Vec<f32> = Vec::new();
    for _ in 0..n {
        let a = (rnd() - 0.5) * 6.0 + rnd() * 1e-9;
        let b = 0.2 + rnd() * 2.8 + rnd() * 1e-9;
        let (ah, al) = split(a);
        let (bh, bl) = split(b);
        vals.push((ah as f64 + al as f64, bh as f64 + bl as f64));
        input.extend([ah, al, bh, bl]);
    }
    let body: String = FUNCS.iter().enumerate().map(|(k, f)| format!("    outp[i * {}u + {k}u] = {f};\n", FUNCS.len())).collect();
    let src = format!(
        "{}\n@group(0) @binding(0) var<storage, read> inp: array<vec4<f32>>;\n@group(0) @binding(1) var<storage, read_write> outp: array<vec2<f32>>;\n@group(0) @binding(2) var<storage, read> one: array<f32>;\n\
         @compute @workgroup_size(64)\nfn main(@builtin(global_invocation_id) g: vec3<u32>) {{\n    dd_one = one[0];\n    let i = g.x;\n    if (i >= arrayLength(&inp)) {{\n        return;\n    }}\n    let a = dd(inp[i].x, inp[i].y);\n    let b = dd(inp[i].z, inp[i].w);\n{body}}}\n",
        rmb_gpu::codegen::DF64
    );
    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("df64 test"), source: wgpu::ShaderSource::Wgsl(src.into()) });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
    let inp = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&input), usage: wgpu::BufferUsages::STORAGE });
    let out_size = (n * FUNCS.len() * 8) as u64;
    let outp = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: out_size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let one = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&[1.0f32]), usage: wgpu::BufferUsages::STORAGE });
    let staging = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: out_size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: inp.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: outp.as_entire_binding() }, wgpu::BindGroupEntry { binding: 2, resource: one.as_entire_binding() }],
    });
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups((n as u32).div_ceil(64), 1, 1);
    }
    enc.copy_buffer_to_buffer(&outp, 0, &staging, 0, out_size);
    ctx.queue.submit(Some(enc.finish()));
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let out: Vec<f32> = bytemuck::cast_slice(&slice.get_mapped_range().unwrap()).to_vec();
    let mut worst = vec![0.0f64; FUNCS.len()];
    for (i, (a, b)) in vals.iter().enumerate() {
        for (k, w) in worst.iter_mut().enumerate() {
            let j = (i * FUNCS.len() + k) * 2;
            let got = out[j] as f64 + out[j + 1] as f64;
            let want = reference(k, *a, *b);
            *w = w.max((got - want).abs() / want.abs().max(1.0));
        }
    }
    let mut bad = Vec::new();
    for (k, f) in FUNCS.iter().enumerate() {
        eprintln!("{f:32} worst relative error {:.2e}", worst[k]);
        // Arithmetic (the first seven) to a few units of 2^-48, the rest to 1e-13.
        let limit = if k < 7 { 2e-14 } else { 1e-13 };
        // NaN counts as a failure.
        if worst[k].is_nan() || worst[k] > limit {
            bad.push(*f);
        }
    }
    assert!(bad.is_empty(), "too imprecise: {bad:?}");
}
