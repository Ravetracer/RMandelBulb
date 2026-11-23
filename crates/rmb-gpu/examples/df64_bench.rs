//! Times the double-float functions (df64.wgsl) on the GPU: each thread applies one function
//! `-n` times in a dependent chain. `df64_bench [-n rounds] [-t threads]`.
use std::time::Instant;

use wgpu::util::DeviceExt;

const OPS: [(&str, &str); 14] = [
    ("f32 sin (baseline)", "dd_f(sin(x.x))"),
    ("dd_add", "dd_add(x, b)"),
    ("dd_mul", "dd_mul(x, b)"),
    ("dd_div", "dd_div(b, x)"),
    ("dd_sqrt", "dd_sqrt(dd_abs(x))"),
    ("dd_sincos", "dd_sincos(x)[0]"),
    ("dd_sin + dd_cos", "dd_add(dd_sin(x), dd_cos(x))"),
    ("dd_atan2", "dd_atan2(x, b)"),
    ("dd_acos", "dd_acos(dd_mulf(x, 0.3))"),
    ("dd_pow(x, 8)", "dd_pow(x, dd_f(8.0))"),
    ("dd_pow(x, 7)", "dd_pow(x, dd_f(7.0))"),
    ("dd_pow(x, 7.5)", "dd_pow(dd_abs(x), dd_f(7.5))"),
    ("dd_exp", "dd_exp(x)"),
    ("dd_log", "dd_log(dd_abs(x))"),
];

fn main() {
    let (mut rounds, mut threads) = (256u32, 1u32 << 20);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-n" => rounds = it.next().and_then(|v| v.parse().ok()).expect("rounds"),
            "-t" => threads = it.next().and_then(|v| v.parse().ok()).expect("threads"),
            _ => panic!("unknown argument {a}"),
        }
    }
    let ctx = rmb_gpu::GpuContext::new_headless().expect("gpu");
    let device = &ctx.device;
    let one = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&[1.0f32, 0.0, 0.0, 0.0]), usage: wgpu::BufferUsages::UNIFORM });
    let out = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: threads as u64 * 8, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
    println!("{threads} threads x {rounds} rounds");
    let mut base = 0.0;
    for (label, op) in OPS {
        let src = format!(
            "{}\n@group(0) @binding(0) var<uniform> one: vec4<f32>;\n@group(0) @binding(1) var<storage, read_write> outp: array<vec2<f32>>;\n\
             @compute @workgroup_size(64)\nfn main(@builtin(global_invocation_id) g: vec3<u32>) {{\n    dd_one = one.x;\n    let i = g.x;\n    var x = dd_add(dd_f(0.3 + f32(i % 4096u) * 1.0e-4), dd(0.0, 1.0e-9));\n    let b = dd_add(dd_f(1.1), dd(0.0, 3.0e-9));\n    var acc = dd_f(0.0);\n    for (var r = 0u; r < {rounds}u; r++) {{\n        let y = {op};\n        acc = dd_add(acc, y);\n        x = dd_add(dd_mulf(dd_sub(y, dd_f(floor(y.x))), 0.8), dd_f(0.2 + f32(r % 7u) * 0.01));\n    }}\n    outp[i] = acc;\n}}\n",
            rmb_gpu::codegen::DF64
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(src.into()) });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: one.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: out.as_entire_binding() },
            ],
        });
        let run = || {
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(threads.div_ceil(64), 1, 1);
            }
            ctx.queue.submit(Some(enc.finish()));
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        };
        run();
        let t = Instant::now();
        for _ in 0..3 {
            run();
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / 3.0;
        if base == 0.0 {
            base = ms;
        }
        println!("  {label:<20} {ms:>8.2} ms  {:>6.1}x f32 sin", ms / base);
    }
}
