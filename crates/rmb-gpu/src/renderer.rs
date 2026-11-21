// SPDX-License-Identifier: LGPL-2.1-or-later

//! Progressive GPU renderer.
//!
//! Every pass adds one sample per pixel to an accumulation buffer. A pass is split into bands
//! of rows (one submission each) so a single dispatch never runs long enough to stall the
//! desktop or trigger a driver timeout; the band height adapts to the measured time. Shaders
//! are compiled on a background thread; the previous image stays visible meanwhile.

use std::borrow::Cow;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use rmb_core::fractal::Program;
use rmb_core::render::Frame;
use rmb_core::scene::{AoMode, BackgroundMode, Scene};

/// A wgpu device + queue pair.
#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
}

impl GpuContext {
    /// Creates a headless context on the best available adapter.
    pub fn new_headless() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rmb-gpu"),
            required_features: wgpu::Features::empty(),
            required_limits: Self::required_limits(&adapter),
            ..Default::default()
        }))
        .map_err(|e| format!("request_device failed: {e}"))?;
        Ok(Self { device, queue, adapter_name: adapter.get_info().name })
    }

    /// Limits we want: the adapter's storage buffer sizes (big images need big buffers).
    pub fn required_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
        let al = adapter.limits();
        wgpu::Limits {
            max_storage_buffer_binding_size: al.max_storage_buffer_binding_size,
            max_buffer_size: al.max_buffer_size,
            max_texture_dimension_2d: al.max_texture_dimension_2d.min(16384),
            ..wgpu::Limits::default()
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GLight {
    dir: [f32; 4],
    color: [f32; 4],
    opts: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    cam_pos: [f32; 4],
    cam_right: [f32; 4],
    cam_up: [f32; 4],
    cam_fwd: [f32; 4],
    size: [u32; 4],
    band: [u32; 4],
    march: [f32; 4],
    frac: [f32; 4],
    julia: [f32; 4],
    ao: [f32; 4],
    sky: [f32; 4],
    ground: [f32; 4],
    bg_top: [f32; 4],
    bg_horizon: [f32; 4],
    bg_bottom: [f32; 4],
    fog: [f32; 4],
    glow: [f32; 4],
    dof: [f32; 4],
    color: [f32; 4],
    color2: [f32; 4],
    cuts: [[f32; 4]; 3],
    slots: [[u32; 4]; 8],
    lights: [GLight; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PostUniforms {
    a: [f32; 4],
    b: [f32; 4],
    size: [u32; 4],
}

fn v4(v: rmb_core::V3, w: f64) -> [f32; 4] {
    [v.x as f32, v.y as f32, v.z as f32, w as f32]
}

fn c4(c: [f32; 3], w: f64) -> [f32; 4] {
    [c[0], c[1], c[2], w as f32]
}

fn uniforms(scene: &Scene, prog: &Program, w: u32, h: u32, fast: bool) -> Uniforms {
    let fr = Frame::new(scene, w, h);
    let cam = &scene.camera;
    let l = &scene.lighting;
    let bg = &scene.background;
    let mut lights = [GLight::zeroed(); 6];
    for (g, fl) in lights.iter_mut().zip(&fr.lights) {
        g.dir = v4(fl.dir, fl.specular);
        g.color = [fl.color[0] as f32, fl.color[1] as f32, fl.color[2] as f32, 0.0];
        g.opts = [if fl.shadows { 1.0 } else { 0.0 }, fl.softness as f32, 0.0, 0.0];
    }
    let mut cuts = [[0.0f32; 4]; 3];
    for (c, (axis, pos, above)) in cuts.iter_mut().zip(&prog.cuts) {
        *c = [*axis as f32, *pos as f32, if *above { 1.0 } else { 0.0 }, 0.0];
    }
    let mut slots = [[0u32; 4]; 8];
    for (s, ps) in slots.iter_mut().zip(&prog.slots) {
        s[0] = ps.iterations.max(1);
    }
    let ao_mode = match l.ao_mode {
        AoMode::Off => 0.0,
        AoMode::Fast => 1.0,
        AoMode::Raytraced => 2.0,
    };
    let m = &scene.material;
    Uniforms {
        cam_pos: v4(cam.position, fr.tan_half),
        cam_right: v4(cam.right, fr.aspect),
        cam_up: v4(cam.up, fr.pixel_angle),
        cam_fwd: v4(cam.forward, scene.march.max_distance),
        size: [w, h, 0, fr.lights.len() as u32],
        band: [0, 0, scene.march.max_steps.max(1), scene.march.refine_steps],
        march: [scene.march.detail as f32, scene.march.step as f32, prog.bailout2.min(3.0e38) as f32, prog.de_scale as f32],
        frac: [prog.w_slice as f32, if prog.julia { 1.0 } else { 0.0 }, prog.max_iterations as f32, prog.repeat_from as f32],
        julia: prog.julia_c.map(|v| v as f32),
        ao: [ao_mode, l.ao_strength as f32, l.ao_radius as f32, l.ambient as f32],
        sky: c4(l.sky_color, l.shininess),
        ground: c4(l.ground_color, m.specular),
        bg_top: c4(bg.top, if bg.mode == BackgroundMode::Gradient { 1.0 } else { 0.0 }),
        bg_horizon: c4(bg.horizon, 0.0),
        bg_bottom: c4(bg.bottom, 0.0),
        fog: c4(scene.fog.color, scene.fog.density),
        glow: c4(scene.fog.glow_color, scene.fog.glow),
        dof: [if scene.dof.enabled { 1.0 } else { 0.0 }, scene.dof.aperture as f32, scene.dof.focus_distance as f32, if fast { 1.0 } else { 0.0 }],
        color: [m.color_mode.index() as f32, m.color_scale as f32, m.color_offset as f32, if m.repeat { 1.0 } else { 0.0 }],
        color2: [m.saturation as f32, prog.cuts.len() as f32, 0.0, 0.0],
        cuts,
        slots,
        lights,
    }
}

fn post_uniforms(scene: &Scene, w: u32, h: u32) -> PostUniforms {
    let p = &scene.post;
    PostUniforms {
        a: [p.exposure as f32, p.gamma as f32, p.contrast as f32, p.saturation as f32],
        b: [if p.tone_map { 1.0 } else { 0.0 }, p.vignette as f32, 0.0, 0.0],
        size: [w, h, 0, 0],
    }
}

fn storage_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16).next_multiple_of(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

struct Target {
    width: u32,
    height: u32,
    accum: wgpu::Buffer,
    output: wgpu::Texture,
    view: wgpu::TextureView,
}

type Compiled = Result<wgpu::ComputePipeline, String>;

/// What the accumulation was rendered with; any change restarts it.
#[derive(Clone, PartialEq)]
struct Key {
    scene: Scene,
    width: u32,
    height: u32,
    fast: bool,
}

pub struct GpuRenderer {
    ctx: GpuContext,
    display_pipeline: wgpu::ComputePipeline,
    /// Explicit layout of the march bindings: a shader that does not read a binding (formulas
    /// without parameters never touch `prm`) must still accept the full bind group.
    march_layout: wgpu::BindGroupLayout,
    march_pipeline_layout: wgpu::PipelineLayout,
    /// Current march pipeline and its signature.
    pipeline: Option<(String, wgpu::ComputePipeline)>,
    /// Recently used pipelines (switching formulas back and forth).
    cache: Vec<(String, wgpu::ComputePipeline)>,
    pending: Option<(String, mpsc::Receiver<Compiled>)>,
    uniform_buf: wgpu::Buffer,
    post_buf: wgpu::Buffer,
    prm: wgpu::Buffer,
    lut: wgpu::Buffer,
    target: Option<Target>,
    key: Option<Key>,
    uniforms: Option<Uniforms>,
    post: Option<PostUniforms>,
    /// Completed passes (samples per pixel).
    samples: u32,
    /// Next row of the running pass.
    row: u32,
    band_rows: u32,
    dirty_display: bool,
    generation: u64,
    last_error: Option<String>,
    /// Validation errors outside error scopes (instead of the default panic).
    device_error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    compiling: bool,
    signature: String,
}

const CACHE_SIZE: usize = 8;
/// Time per band submission we aim for.
const BAND_TARGET: Duration = Duration::from_millis(12);

impl GpuRenderer {
    pub fn new(ctx: GpuContext) -> Self {
        let device = &ctx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("display"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("display.wgsl"))) });
        let display_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("display"),
            layout: None,
            module: &module,
            entry_point: Some("display_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let uniform = |label: &str, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let storage = |read_only: bool| wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None };
        let march_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("march"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(1, storage(true)),
                entry(2, storage(true)),
                entry(3, storage(false)),
            ],
        });
        let device_error = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let sink = device_error.clone();
        device.on_uncaptured_error(std::sync::Arc::new(move |e: wgpu::Error| {
            eprintln!("wgpu error: {e}");
            if let Ok(mut g) = sink.lock() {
                *g = Some(format!("GPU error: {}", e.to_string().lines().next().unwrap_or("")));
            }
        }));
        GpuRenderer {
            uniform_buf: uniform("march-uniforms", std::mem::size_of::<Uniforms>()),
            post_buf: uniform("post-uniforms", std::mem::size_of::<PostUniforms>()),
            prm: storage_buffer(device, "prm", 1024),
            lut: storage_buffer(device, "lut", (rmb_core::gradient::LUT_SIZE * 16) as u64),
            display_pipeline,
            march_pipeline_layout: device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("march"), bind_group_layouts: &[Some(&march_layout)], immediate_size: 0 }),
            march_layout,
            pipeline: None,
            cache: Vec::new(),
            pending: None,
            target: None,
            key: None,
            uniforms: None,
            post: None,
            samples: 0,
            row: 0,
            band_rows: 16,
            dirty_display: false,
            generation: 0,
            last_error: None,
            device_error,
            compiling: false,
            signature: String::new(),
            ctx,
        }
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.clone().or_else(|| self.device_error.lock().ok().and_then(|g| g.clone()))
    }

    /// A shader for the current formulas is being compiled.
    pub fn compiling(&self) -> bool {
        self.compiling
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Progress of the running pass (0..1).
    pub fn pass_progress(&self) -> f32 {
        self.target.as_ref().map(|t| self.row as f32 / t.height.max(1) as f32).unwrap_or(0.0)
    }

    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }

    pub fn output_size(&self) -> Option<(u32, u32)> {
        self.target.as_ref().map(|t| (t.width, t.height))
    }

    /// Changes whenever the output texture was replaced or redrawn.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Sets what to render. Restarts the accumulation when anything but the post processing
    /// changed. `fast` renders a quicker preview (no shadows, fast AO) for navigation.
    pub fn set_scene(&mut self, scene: &Scene, width: u32, height: u32, fast: bool) {
        let (width, height) = (width.max(1), height.max(1));
        let mut render_scene = scene.clone();
        render_scene.post = Default::default();
        render_scene.notes.clear();
        let key = Key { scene: render_scene, width, height, fast };
        let post = post_uniforms(scene, width, height);
        if self.post.is_none_or(|p| bytemuck::bytes_of(&p) != bytemuck::bytes_of(&post)) {
            self.ctx.queue.write_buffer(&self.post_buf, 0, bytemuck::bytes_of(&post));
            self.post = Some(post);
            self.dirty_display = true;
        }
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let prog = Program::new(&scene.fractal, &scene.cut);
        self.ensure_target(width, height);
        let device = &self.ctx.device;
        // Parameters
        let consts: Vec<f32> = prog.constants.iter().map(|v| *v as f32).collect();
        let bytes = (consts.len() * 4).max(16) as u64;
        if self.prm.size() < bytes {
            self.prm = storage_buffer(device, "prm", bytes.next_power_of_two());
        }
        if !consts.is_empty() {
            self.ctx.queue.write_buffer(&self.prm, 0, bytemuck::cast_slice(&consts));
        }
        let lut: Vec<[f32; 4]> = rmb_core::gradient::lut(&scene.material.gradient).iter().map(|c| [c[0], c[1], c[2], 1.0]).collect();
        self.ctx.queue.write_buffer(&self.lut, 0, bytemuck::cast_slice(&lut));
        self.uniforms = Some(uniforms(scene, &prog, width, height, fast));
        self.request_pipeline(&prog);
        self.key = Some(key);
        self.samples = 0;
        self.row = 0;
    }

    fn ensure_target(&mut self, width: u32, height: u32) {
        if self.target.as_ref().is_some_and(|t| t.width == width && t.height == height) {
            return;
        }
        let device = &self.ctx.device;
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let accum = storage_buffer(device, "accum", width as u64 * height as u64 * 16);
        self.target = Some(Target { width, height, accum, output, view });
        self.generation += 1;
        self.samples = 0;
        self.row = 0;
    }

    fn request_pipeline(&mut self, prog: &Program) {
        let sig = prog.signature();
        if self.signature == sig {
            return;
        }
        self.signature = sig.clone();
        if let Some(i) = self.cache.iter().position(|(s, _)| *s == sig) {
            let p = self.cache.remove(i);
            if let Some(old) = self.pipeline.replace(p) {
                self.cache.push(old);
            }
            self.pending = None;
            self.compiling = false;
            self.last_error = None;
            return;
        }
        let src = crate::codegen::march_shader(prog);
        let device = self.ctx.device.clone();
        let layout = self.march_pipeline_layout.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(compile(&device, &layout, &src));
        });
        self.pending = Some((sig, rx));
        self.compiling = true;
    }

    fn poll_pipeline(&mut self) {
        let Some((sig, rx)) = &self.pending else { return };
        match rx.try_recv() {
            Ok(Ok(p)) => {
                let sig = sig.clone();
                if let Some(old) = self.pipeline.replace((sig, p)) {
                    self.cache.push(old);
                    if self.cache.len() > CACHE_SIZE {
                        self.cache.remove(0);
                    }
                }
                self.last_error = None;
                self.pending = None;
                self.compiling = false;
                self.samples = 0;
                self.row = 0;
            }
            Ok(Err(e)) => {
                self.last_error = Some(e);
                self.pending = None;
                self.compiling = false;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.last_error = Some("shader compilation thread failed".into());
                self.pending = None;
                self.compiling = false;
            }
        }
    }

    /// The pipeline for the current scene (None while it compiles or after an error).
    fn current_pipeline(&self) -> Option<&wgpu::ComputePipeline> {
        self.pipeline.as_ref().filter(|(s, _)| *s == self.signature && self.pending.is_none()).map(|(_, p)| p)
    }

    /// Renders bands until `budget` is used up or `max_samples` passes are complete.
    /// Returns true if anything was rendered.
    pub fn work(&mut self, budget: Duration, max_samples: u32) -> bool {
        self.poll_pipeline();
        if self.samples >= max_samples {
            return false;
        }
        let (Some(t), Some(mut un)) = (self.target.as_ref(), self.uniforms) else { return false };
        let (accum, width, height) = (t.accum.clone(), t.width, t.height);
        let Some(pipeline) = self.current_pipeline().cloned() else { return false };
        let device = self.ctx.device.clone();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("march"),
            layout: &self.march_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.prm.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.lut.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: accum.as_entire_binding() },
            ],
        });
        let start = Instant::now();
        let mut worked = false;
        while self.samples < max_samples {
            let rows = self.band_rows.min(height - self.row);
            un.size[2] = self.samples;
            un.band[0] = self.row;
            un.band[1] = rows;
            self.ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&un));
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(width.div_ceil(8), rows.div_ceil(8), 1);
            }
            let t0 = Instant::now();
            self.ctx.queue.submit(Some(enc.finish()));
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            let dt = t0.elapsed();
            worked = true;
            // Adapt the band height toward the target time per submission.
            let ratio = BAND_TARGET.as_secs_f64() / dt.as_secs_f64().max(1e-5);
            let wanted = (rows as f64 * ratio.clamp(0.5, 2.0)).round() as u32;
            self.band_rows = wanted.clamp(8, height.max(8)).next_multiple_of(8);
            self.row += rows;
            if self.row >= height {
                self.row = 0;
                self.samples += 1;
            }
            if start.elapsed() >= budget {
                break;
            }
        }
        if worked {
            self.dirty_display = true;
        }
        worked
    }

    /// Updates the output texture from the accumulation if needed.
    pub fn display(&mut self) {
        if !self.dirty_display {
            return;
        }
        let Some(t) = self.target.as_ref() else { return };
        if self.samples == 0 && self.row == 0 {
            // Nothing rendered for the current scene yet: keep showing the old image.
            return;
        }
        self.dirty_display = false;
        let device = &self.ctx.device;
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("display"),
            layout: &self.display_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.post_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: t.accum.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&t.view) },
            ],
        });
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.display_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(t.width.div_ceil(8), t.height.div_ceil(8), 1);
        }
        self.ctx.queue.submit(Some(enc.finish()));
        self.generation += 1;
    }

    /// Blocks until the current shader is compiled (headless rendering). Returns the error, if any.
    pub fn wait_for_pipeline(&mut self) -> Result<(), String> {
        while self.pending.is_some() {
            std::thread::sleep(Duration::from_millis(5));
            self.poll_pipeline();
        }
        match &self.last_error {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    /// Reads the displayed image back as tightly packed RGBA8 (blocking).
    pub fn read_rgba8(&mut self) -> Option<Vec<u8>> {
        self.dirty_display = true;
        self.display();
        let t = self.target.as_ref()?;
        read_texture_rgba8(&self.ctx, &t.output, t.width, t.height)
    }
}

fn compile(device: &wgpu::Device, layout: &wgpu::PipelineLayout, src: &str) -> Compiled {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("march"), source: wgpu::ShaderSource::Wgsl(Cow::Owned(src.to_string())) });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("march"),
        layout: Some(layout),
        module: &module,
        entry_point: Some("march_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    match pollster::block_on(scope.pop()) {
        Some(err) => Err(format!("shader compilation failed: {err}")),
        None => Ok(pipeline),
    }
}

/// Reads an RGBA8 texture back as tightly packed RGBA8 (blocking).
pub fn read_texture_rgba8(ctx: &GpuContext, texture: &wgpu::Texture, w: u32, h: u32) -> Option<Vec<u8>> {
    let row = (w * 4).next_multiple_of(256);
    let device = &ctx.device;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: row as u64 * h as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &staging, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit(Some(enc.finish()));
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range().ok()?;
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let start = y * row as usize;
        out.extend_from_slice(&data[start..start + (w * 4) as usize]);
    }
    drop(data);
    staging.unmap();
    Some(out)
}

/// Renders `scene` at `width` × `height` with `samples` passes (blocking, headless use).
/// `progress` gets the completed fraction and returns false to cancel.
pub fn render_blocking(ctx: GpuContext, scene: &Scene, width: u32, height: u32, samples: u32, mut progress: impl FnMut(f32) -> bool) -> Result<Vec<u8>, String> {
    let mut r = GpuRenderer::new(ctx);
    r.set_scene(scene, width, height, false);
    r.wait_for_pipeline()?;
    let samples = samples.max(1);
    while r.samples() < samples {
        r.work(Duration::from_millis(100), samples);
        if let Some(e) = r.last_error() {
            return Err(e);
        }
        let p = (r.samples() as f32 + r.pass_progress()) / samples as f32;
        if !progress(p) {
            return Err("cancelled".into());
        }
    }
    r.read_rgba8().ok_or_else(|| "reading the image back failed".to_string())
}
