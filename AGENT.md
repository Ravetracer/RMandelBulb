# AGENT.md – working on RMandelbulb3D

Notes for coding agents (and humans). RMandelbulb3D re-implements Mandelbulb3D (Delphi source in
`../mb3d`) in Rust. `../mandelbulber2-2.34` is a reference only (GPL, no MB3D name mapping).

## Owner and working style

- Keep answers short. Report outcomes honestly.
- **Use half of the CPU cores**: `cargo build -j 8`, `cargo test -j 8 -- --test-threads=8`,
  `RAYON_NUM_THREADS=8`.
- At most two sub-agents at a time, and tell them not to spawn their own.
- After finished work: commit and push to `origin main` (github.com/Ravetracer/RMandelbulb).
  Commit messages, code, comments and docs use plain, neutral English.
- Every control gets a hover text. Actions report what they did, or why they were skipped.

Open work and wished features: `TODO.md`.

## Layout

| Path | Content |
|------|---------|
| `crates/rmb-core` | `compose.rs` (scene composition: objects, primitives, boolean ops, transforms; mirrored in march.wgsl), `mesh.rs` (3D model import and distance grid bake), `texture.rs` (texture layers), `flare.rs` (lens flares: model, presets, screen-space layer added before post processing), `animation.rs` (keyframes, camera path, timing, cameraman smoothing, flight → keyframes), `video.rs` (PNG sequences, movies through ffmpeg), `scene.rs` (model), `formula/` (language `lang.rs`, `cpu.rs` closure compiler, `wgsl.rs` emitter, `lib/*.mbf` built-ins, `m3f/` MB3D machine-code translator, `lib/mb3d.mbf` its generated output), `fractal.rs` (hybrid `Program`, CPU evaluator), `render/` (`cpu.rs` reference renderer, `mod.rs` frame setup and post processing), `io/` (`.rmb` JSON, `m3p.rs` MB3D import, PNG with scene) |
| `crates/rmb-gpu` | `march.wgsl` (kernel, mirrors `render/cpu.rs`), `codegen.rs` (hybrid `iterate` function), `display.wgsl` (post), `renderer.rs` (progressive bands, async shader compile) |
| `crates/rmb-app` | egui app: `app.rs`, `viewport.rs` (preview and navigation), `formulas_panel.rs` (scene list, object editors), `inspector.rs`, `layers.rs` (texture layer editor), `export.rs` (image and animation export), `render_view.rs` (test render window: `render_progressive`, keeps the raw image and develops it again when the post processing changes), `timeline.rs` (animation panel: keyframes, recording, playback), `gizmos.rs` (light gizmos; object gizmos: move, rotate rings, scale/stretch handles), `doc.rs` (undo), `prefs.rs`, `theme.rs` (from RWildFire) |
| `crates/rmb-cli` | Command line |

## Build and test

```sh
cargo build -j 8 -p rmb-app -p rmb-cli
RAYON_NUM_THREADS=8 cargo test -j 8 --workspace -- --test-threads=8
cargo clippy -j 8 --workspace --all-targets
```

- `rmb-gpu/tests/shaders.rs` checks every library formula's WGSL with naga. It also compares
  a GPU render with the CPU one (mean difference must stay below 4 out of 255; it is
  currently 0.02).
- `rmb-core` tests import all `../mb3d/M3Parameter/*.m3p` files.

## Design notes

- **One formula source**: never write a formula twice. Add it to `formula/lib/*.mbf`.
  Parameters that map to MB3D need `alias "Name" (ident, …)` in MB3D option order.
- **CPU and GPU parity**: `render/cpu.rs` and `march.wgsl` are the same algorithm step by step,
  including the random number order. Change both together.
- The shader only changes with the formulas of the slots and the DE variant
  (`Program::signature`). Parameters, iteration counts and all render settings are buffers, so
  changing them never recompiles.
- The renderer splits every pass into row bands, one submission each, with adaptive band
  height (about 12 ms). This avoids driver timeouts and keeps the UI responsive.
- MB3D's spec notes (parameter format, render math, .m3f analysis) came from studying the source.
  MB3D's camera starts rays on a plane; the import converts it to a pinhole camera at the
  point where those rays converge.

## Denoiser

- `render/oidn.rs` (runtime-loaded OIDN, from RWildFire) and `render/denoise.rs` (`HdrImage`:
  averaged color + albedo + normal, `finish` = denoise then `post_process`). Final images (CLI,
  export, "Denoised" preview) go through `HdrImage::finish`; the GPU reads its accumulation and
  the `aux` buffer (binding 4: albedo, normal per pixel) back with `read_hdr`.
- `python3 tools/fetch_oidn.py` fills `oidn/` (gitignored) for development.

## Mandelbulb3D formulas

- `docs/mb3d-notes.md`: parameter format, record layout, calling convention.
- Regenerate the built-in MB3D formulas after translator changes:
  `cargo run --release -p rmb-cli -- m3f ../mb3d/M3Formulas -o crates/rmb-core/src/formula/lib/mb3d.mbf`
  (prints the count and the failure reasons). Then run the GPU tests: every formula must give
  valid WGSL and render.
- `cargo run --release -p m3f-check -- ../mb3d/M3Formulas` runs the original machine code in the
  Unicorn emulator and compares it with every translation (4 iterations, 10 random points).
  All but `_DonutTransform` match (that one drifts by about 1e-5, probably x87 trig precision
  of the emulator). The tool is not a default member: it builds Unicorn (C, needs cmake).
- The translator keeps the machine state in variables (x87 slots `f0`..`f7`, xmm lanes, `r_eax`…)
  and structures branches by post-dominators. FPU compare conditions come from evaluating the
  flag instructions per compare outcome; integer reads of a double's bytes become sign tests
  (`svN`), sign-bit writes become abs/negate/copysign, small-range table reads become `sel`
  chains. dIFS formulas (DEoption 20-22: `esi` = record + 88, `edi` = parameters) translate to
  `de difs`; the hybrid then takes the smallest `rout / a0` (VaryScale) over the iterations.
  Loops become `loop`/`break`/`continue` (natural loops by back edges; registers that change in
  the loop become variables, found by retrying). Value sets of integer indexes turn table
  accesses into `sel` / `if` chains. JIT formulas (`[SOURCE]`, Pascal) go through `pascal.rs`.
  Not supported: map functions, formulas that switch the hybrid's formulas, random numbers.
- Formulas the translator cannot handle are written by hand in `lib/mb3d_builtin.mbf` (also
  MB3D's built-in Bulbox and Aexion C). `m3f-check` falls back to the library formula with the
  .m3f's name, so hand-written versions are checked too; `M3F_VALUES="v1,v2,…"` sets the option
  values (default: the file's defaults). `cargo run -p rmb-core --example m3f_disasm -- f.m3f`
  prints the machine code.
- Map formulas: `call [record+352]` (spherical) / `[record+356]` (direct x/y) become
  `mapsph(n, x, y, z, ch)` / `mapxy(n, u, v, ch)` (`rmb-core/src/maps.rs`; GPU: `maps` buffer,
  binding 5, filled from `Program::map_numbers`). `m3f-check` answers these calls with the same
  sampling (emulator hook), so it checks the translation, not the sampling itself.
- Regenerate from both folders: `rmb-cli m3f ../mb3d/M3Formulas ../mb3d/EM_JIT_M3Formulas -o …`.
- Regenerate without piping the output into `head`: a closed pipe kills `rmb-cli` before it
  writes the file.

## Scene composition

- `Scene::objects` (compose.rs) after the main fractal. `Program::for_scene` builds them;
  object fractals append their constants to the main program's. `Evaluator::orbit` folds the
  objects in order and sets `Orbit::object` (which object forms the surface, for its material).
- GPU: `codegen.rs` emits a chain per fractal object (settings from the `objs` buffer, binding
  6, filled by `renderer.rs::objects_gpu`) and `compose_objects`; object materials are LUT
  blocks of 512 entries from index 1152. Adding objects or changing their formulas / primitive
  type changes the shader signature (recompile); everything else is buffer data.
- `cargo run --release -p rmb-core --example compose_demo -- out.rmb` writes a demo scene.

## Path tracing

- `Lighting::render_mode` (`RenderMode::Classic` default, `PathTraced`). `sample_aux` /
  `sample_pixel` share one bounce loop with one `trace()` call: classic follows reflections /
  glass as before; path tracing picks per surface the reflection/glass lobe (probability
  `path_specular_share`) or a cosine-weighted diffuse bounce, Russian roulette from the third
  bounce, `path_clamp` limits each bounce's light. `trace()` in path mode skips ambient, AO and
  the emission rays; bounce misses see `environment()` (ambient colors × ambient). GPU: uniform
  `path`, `REFLECT` is on for path tracing, fast navigation shades classically (`path_on`).

## Colors, glow bands, bloom

- `gradient.rs`: stops are sampled into 256-entry tables; `GlowBand`s are evaluated exactly at
  the gradient position (`apply_bands`, GPU: `gradient()` in march.wgsl, LUT from `LUT_BANDS`).
  `Material::rotation` and `GlowBand::speed` are integrated over the frames in
  `animation.rs` (`color_phases`), so they only act in animations.
- `render/bloom.rs`: bloom pyramid and streaks (Kawase streak filter) applied in
  `HdrImage::finish*` and `CpuRenderer::rgba8`; the GPU preview runs the same passes
  (`bloom.wgsl`, composite in display.wgsl; streak regions after the pyramid in one buffer).
- Glow and steam (`Material::emission_glow*`, `steam*`): `march_ex` offers points near the
  surface to one `SteamPick` per primary ray (reservoir sampling with `hash3`; glow weight =
  falloff from the surface, steam also × noise density); `trace` casts one ray from the picked
  point toward the falling distance and adds the emission of the surface it hits (so glow and
  steam take the color of the glowing surface, not of the empty space).
- Mandelbulb3D's second color choice (`ColorMode::SecondChoice`, `Material::second_choice`,
  `lli_multiplier`): every chain computes `Orbit::color2` at its end (`fractal::color2_value`,
  GPU `color2_value` at the end of `iterate_K`), the 15-bit value / 32767; the m3p import maps
  TBoptions bit 17 to it with MB3D's color start/end. The orbit mode of color maps reads it
  too (MB3D's OTrap field). Color on iteration (`color_on_iteration`, MB3D `bColorOnIt`):
  `Evaluator::color_orbit` iterates again at the hit with a fixed count and the bailout raised
  to bailout² · 64; GPU behind the `COLOR_ON_IT` switch (`color_orbit`, private `color_bail`).
- Position color modes (`ColorMode::Plane` / `Sphere`, `Material::color_center` /
  `color_direction`, shared by all materials): the gradient runs through world space, so a glow
  band becomes one front of light. GPU: the
  same in `march_ray`/`trace` (private `steam_*` variables). The drift moves `steam_offset` in
  animations (`color_phases`).

## 3D models and texture layers

- `mesh.rs` loads OBJ/STL/PLY/glTF (materials by name, colors, color textures; images that
  are not where the file says are looked up near the model, `nearby_images`) and bakes a signed
  distance grid (exact triangle distances through a BVH; sign by ray parity along three axes,
  majority vote) plus the triangles with a BVH. `Volume::distance` uses the grid away from the
  surface (moved toward zero by `GRID_ERROR` voxels, so it stays a lower bound) and the exact
  nearest-triangle distance within `NEAR` voxels: thin parts and open surfaces become shells and
  stay visible at any distance. Colors come from the nearest triangle (material color × vertex
  color × texture), resolved through a `Look` (cached per volume and settings, `Model::new`):
  the file's materials with the scene's `ModelRef::materials` overrides (color, color texture,
  bump and specular maps; image paths relative to the model). Override images load in the
  background (`texture_file`, bumps `generation()`). `auto_assign` matches images near the
  model to materials by name (kind from words like diffuse/bump/spec); the app runs it once per
  imported model and from the Materials section.
  `mesh::request` bakes in the background (cache keyed by file, modification time and options;
  `generation()` changes when a bake finishes), `compose::load_models` waits for all models of a
  scene (CLI, exports). GPU: `vols` buffer (binding 7, `volumes_gpu`, 32-word header per object),
  read by `model_de` / `model_nearest` / `model_rgb` in march.wgsl exactly like
  `Volume::distance` / `nearest` / `color`. `cargo run --release -p rmb-core --example model_info
  -- model.glb` prints materials and bake statistics.
- `texture.rs` holds the layers (mapping, blend modes, bump, emission). GPU: layer parameters
  in `lut` from `LUT_LAYERS` (4 layers × 7 entries per material), layer images in the `maps`
  buffer under 0xffffffef − index (`renderer.rs::layer_images`).
- Compile-time switches keep shaders small: `LAYERS`, `BUMP` and `MODELS` (codegen.rs, from
  `Program::layers` / `bump` and the model objects) include that code only when the scene uses
  it. Lookups sit in loops with one call site each (the compiler inlines every call; unrolled
  lookups tripled the compile time).

## Lens flares

- `crates/rmb-core/src/flare.rs` draws the flares in screen space on the CPU (one implementation
  for preview and export): `HdrImage::finish_scene` adds them after denoising, the preview gets
  the layer through `GpuRenderer::set_flare` (display.wgsl binding 3, computed on a worker
  thread in `viewport.rs`).
- `cargo run --release -p rmb-core --example flare_presets -- out.png` renders all presets on
  black, to judge changes to the look.

## Testing the app visually

- Xvfb on `:99` (1600×1000), separate `XDG_CONFIG_HOME`; `xdotool` + `import -window root`.
  The NVIDIA GPU works through Xvfb (compute plus egui). Stop with `pkill -x rmb-app`.
