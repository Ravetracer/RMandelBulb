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
- **Keep the user manual current**: every new feature or changed control updates
  `docs/manual` in the same change (the chapter that covers it; new screenshots where the UI
  changed visibly; a how-to in `17-how-tos.md` where a feature needs several steps). See
  "User manual" below.

Open work and wished features: `TODO.md`.

## Layout

| Path | Content |
|------|---------|
| `crates/rmb-core` | `compose.rs` (scene composition: objects, primitives, boolean ops, transforms; mirrored in march.wgsl), `mesh.rs` (3D model import and distance grid bake), `texture.rs` (texture layers), `palette.rs` (palette generator and edits), `flare.rs` (lens flares: model, presets, screen-space layer added before post processing), `animation.rs` (keyframes, camera path, timing, cameraman smoothing, flight → keyframes), `video.rs` (PNG sequences, movies through ffmpeg), `scene.rs` (model), `formula/` (language `lang.rs`, `cpu.rs` closure compiler, `wgsl.rs` emitter, `lib/*.mbf` built-ins, `m3f/` MB3D machine-code translator, `lib/mb3d.mbf` its generated output), `fractal.rs` (hybrid `Program`, CPU evaluator), `render/` (`cpu.rs` reference renderer, `mod.rs` frame setup and post processing), `io/` (`.rmb` JSON, `m3p.rs` MB3D import, PNG with scene) |
| `crates/rmb-gpu` | `march.wgsl` (kernel, mirrors `render/cpu.rs`), `codegen.rs` (hybrid `iterate` function), `display.wgsl` (post), `renderer.rs` (progressive bands, async shader compile) |
| `crates/rmb-app` | egui app: `app.rs`, `viewport.rs` (preview and navigation), `formulas_panel.rs` (scene list, object editors), `inspector.rs`, `layers.rs` (texture layer editor), `palette_editor.rs` (gradient editor: stops on the strip, generator, tools, saved palettes in `palettes.json`), `export.rs` (image and animation export), `render_view.rs` (test render window: `render_progressive`, keeps the raw image and develops it again when the post processing changes), `timeline.rs` (animation panel: keyframes, recording, playback), `gizmos.rs` (light gizmos; object gizmos: move, rotate rings, scale/stretch handles), `doc.rs` (undo), `prefs.rs`, `theme.rs` (from RWildFire) |
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
- Performance: `cargo run -j 8 --release -p rmb-gpu --example bench -- <scene…> -W 640 -H 360`
  times GPU passes with features switched off one at a time (AO, shadows, lights, refine,
  normal smoothing). Build the CLI separately (`-p rmb-cli` without `--example`) before
  comparing images: `--example` builds only the example. A CLI built elsewhere (e.g. a
  worktree for a baseline) finds the denoiser only with `RMB_OIDN_DIR=<repo>/oidn`; check its
  "denoised on …" line before comparing timings.

## Design notes

- **One formula source**: never write a formula twice. Add it to `formula/lib/*.mbf`.
  Parameters that map to MB3D need `alias "Name" (ident, …)` in MB3D option order.
- **CPU and GPU parity**: `render/cpu.rs` and `march.wgsl` are the same algorithm step by step,
  including the random number order. Change both together.
- The shader only changes with the formulas of the slots and the DE variant
  (`Program::signature`). Parameters, iteration counts and all render settings are buffers, so
  changing them never recompiles (renders without a window bake the parameters, see below).
- **Cost model**: points far from the fractal bail out after a few iterations, points at the
  surface run the full iteration count. The pass time is mostly the evaluations at the
  surface: the hit refinement, the normal, the first steps of AO and shadow rays. The binary
  refinement stops at steps below threshold/128 (`REFINE_TOLERANCE`; `refine_steps` is the
  maximum). AO, emission and scatter rays skip the refinement (`march_rough`).
- Renders without a window bake the formula parameters and the uniform settings into the
  shader as constants (`GpuRenderer::set_baking`, `set_uniform_baking`,
  `codegen::march_shader_baked`, `codegen::bake_uniforms`: reads like `u.fog.x` with constant
  indexes; the sample index, band rows and `deep_hi.w` (`dd_one`) stay run-time values). The
  preview keeps reading them from the buffers, so editing never recompiles; once the view rests
  it compiles a fully baked shader in the background and continues with it
  (`set_rest_baking`).
- Data read on every distance estimate (formula parameters `prm`, composed objects `objs`)
  lives in uniform buffers: all threads read the same values, which the constant cache serves
  far faster than a storage buffer (1.4-1.8x on whole passes). Large or per-pixel data (LUT,
  maps, model volumes, accumulation) stays in storage buffers.
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
- Formula library (`formula/mod.rs`): a copy-on-write snapshot (`library()` returns an `Arc`;
  writers go through `change`), so nested lookups never wait for a writer. Every formula keeps
  its source text (`Formula::text`, from `lang::parse_library_texts`); `Program::signature`
  hashes it, so an edited formula of the same name compiles a new shader. `add_source` adds
  formulas at run time (the app's formula editor, `rmb-app/src/formula_editor.rs`).
  `Scene::to_json` embeds the texts of the non-built-in formulas a scene uses
  (`Scene::formulas`), `Scene::from_json` registers them and empties the field.
- Mandelbulber per-slot settings (`Slot::start/stop/weight/add_c/check_bailout`,
  `Fractal::min_iterations`, `c_factor`): `fractal.rs` `iterate_from`; on the GPU only when
  `Program::slot_extras()` (part of the signature), emitted by `codegen.rs` `extras_*` into the
  f32 and df64 loops (`u.slots` y/z/w, `u.slotw`, `u.cfac`). Object fractals drop them
  (`Program::clear_extras`). Up to 9 slots (`MAX_SLOTS`; uniform tables hold 16).
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
- Repetition: `ObjectProgram::local` (= `compose::to_local` + `repeat_fold`) gives every
  local object point, on the GPU `obj_local` (fields `rep`, `rep_lim`); distances, materials
  and layers all go through it.
- Object fractals are built with `Program::new_at` (DE combination, interpolation; no 4D
  rotation, no cuts) and evaluated by `fractal_orbit` like the main fractal. GPU: per object
  `fractal_de_o{j}` with a second chain `o{j}b` (`Uni::object2`; `comb`, `comb2` in `Obj`; both
  chains share the object's eight slot entries).
- The main fractal's placement (`Fractal::transform`, `Program::main_transform`) applies in
  `Evaluator::orbit` / `color_orbit` and the GPU `orbit` / `color_orbit` (`MAIN_T`, uniforms
  `mt0..2`, `mt_pos`); objects and cut planes stay in world coordinates. Deep zoom: the
  reference point is the camera in the fractal's coordinates, `mt_pos` is zero.
- `cargo run --release -p rmb-core --example compose_demo -- out.rmb` writes a demo scene.

## Deep zoom (double precision)

- `march.precision` (Auto / Single / Double). `GpuRenderer::deep_mode` decides (Auto:
  `render::zoom_ratio` below `DEEP_ZOOM_RATIO`, back to single only at twice the threshold).
- Deep mode renders `render::shifted_scene(scene, camera)`: all world positions minus the camera
  position, so marching, normals, AO and shadows are exact in f32 near the camera.
  `Program::deep` holds the reference point; only the main fractal's iteration adds it back, in
  double-float (`codegen::chain_dd`, `df64.wgsl`, `formula::wgsl::emit_iter_dd`). Composed
  objects stay f32 (local coordinates).
- df64 needs `dd_one` (1.0 read from the uniforms): without it the NVIDIA compiler simplifies
  `(a + b) - a` to `b` and every result drops to f32 precision. `tests/df64.rs` checks every
  function on the GPU against f64.
- The iteration switches to f32 once `|dr| * h > 1e-5 |z|` (f32 rounding then moves the point
  by a few percent of the surface threshold), not for numeric estimates (their differences need
  df64) or interpolation hybrids. Parameter low parts live in `prm` from entry 1024 on.
- `examples/deep_scene.rs` writes a zoomed scene (ten times closer per step: a hit is only
  accurate to its threshold); `examples/dump_shader.rs <scene> deep` prints the deep shader.

## Motion blur

- `Animation::shutter`; `render_frames` (renderer.rs) renders pass k of a frame with
  `pass_scene`: the settings at the middle of the pass's slice of the exposure
  (`AnimationPlan::scene_at_time`, fractional frames), `Scene::motion_end` = the camera at the
  slice's end. Every sample draws one extra random number for its time (`Camera::lerp`, same in
  `sample_pixel`) only when `motion_end` is set. `GpuRenderer::set_scene_keep` /
  `CpuRenderer::set_scene_keep` change the scene between passes without restarting.

## Glossy reflections

- `Material::gloss`: `bounce()` (cpu.rs, march.wgsl) draws two random numbers for a GGX micro
  facet (`ggx_normal`) only when gloss > 0, reflects and refracts on it; reflections below
  the surface are mirrored back above it. The weight stays the reflectance.

## Light scattering in fog

- `Fog::scatter` / `scatter_forward`: at the end of `trace` (not inside glass) four random
  numbers (distance, light, area/line point), one light, one point along the segment:
  equiangular around positional lights (`Fog::density_at` / pdf), else by the fog amount
  (`Fog::distance_for`), a `soft_shadow` toward the light, Henyey-Greenstein phase
  (`Fog::scatter_phase`). GPU in `trace` with `fog_density_at`, `fog_distance_for`,
  `scatter_phase` (uniform `fog4`).

## Fractal haze

- `Fog::haze*` (`HazeShape::Distance` / `Iterations`): accumulated in `march_ex` (CPU, through
  `SteamPick::haze*`) and `march_ray` (GPU, private `haze_*`) on the primary march of every
  `trace` segment (not inside glass). Optical depth per step is density × step / distance of
  the step's middle (scale-free: the same at every zoom); the haze's light is added and the
  rest dimmed at the end of `trace`. `Fog::haze_iterations` is the smooth iteration count from
  the growth of the last radius (any power); it also picks the gradient color.

## Path tracing

- `Lighting::render_mode` (`RenderMode::Classic` default, `PathTraced`). `sample_aux` /
  `sample_pixel` share one bounce loop with one `trace()` call: classic follows reflections /
  glass as before; path tracing picks per surface the reflection/glass lobe (probability
  `path_specular_share`) or a cosine-weighted diffuse bounce, Russian roulette from the third
  bounce, `path_clamp` limits each bounce's light. `trace()` in path mode skips ambient, AO and
  the emission rays; bounce misses see `environment()` (ambient colors × ambient). GPU: uniform
  `path`, `REFLECT` is on for path tracing, fast navigation shades classically (`path_on`).

## Colors, glow bands, bloom

- `palette.rs`: random palettes and whole-palette edits in OKLCh (perceptual; out-of-gamut
  colors lose chroma). `gradient.rs`: stops are sampled into 256-entry tables; `GlowBand`s are
  evaluated exactly at the gradient position (`apply_bands`, GPU: `gradient()` in march.wgsl, LUT from `LUT_BANDS`).
  `Material::rotation` and `GlowBand::speed` are integrated over the frames in
  `animation.rs` (`color_phases`), so they only act in animations.
- Post effects: `render::post_process` (white balance before the tone mapping, split toning
  after the saturation) and `render::chromatic_aberration` (on the linear image after bloom);
  display.wgsl mirrors both (`lin_at`, `lin_bilinear`, `split_tone`).
- `render/bloom.rs`: bloom pyramid and streaks (Kawase streak filter) applied in
  `HdrImage::finish*` and `CpuRenderer::rgba8`; the GPU preview runs the same passes
  (`bloom.wgsl`, composite in display.wgsl; streak regions after the pyramid in one buffer).
- Glow and steam (`Material::emission_glow*`, `steam*`): `march_ex` offers points near the
  surface to one `SteamPick` per primary ray (reservoir sampling with `hash3`; glow weight =
  falloff from the surface, steam also × noise density); `trace` casts one ray from the picked
  point toward the falling distance and adds the emission of the surface it hits (so glow and
  steam take the color of the glowing surface, not of the empty space).
- Shimmer (`Material::shimmer*`, `render::shimmer`, GPU `shimmer`, uniform `shim`): the
  emission factor from value noise at the world position, cross-faded between the patterns
  of `floor(phase)` and the next; `shimmer_phase` advances by `shimmer_speed` in animations
  (phase "shimmer"). Applied in `trace` and `emission_at`. Color maps can emit
  (`ColorMap::emission` × the map's brightness, `map_color` returns it; GPU `map_lum`), only
  in `trace` (they do not light their surroundings).
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

- `mesh.rs` loads OBJ/STL/PLY/glTF/FBX (FBX through the `ufbx` crate, converted to Y-up;
  materials by name, colors, color textures, glTF/FBX normal maps and emission, OBJ `Ke`; images that
  are not where the file says are looked up near the model, `nearby_images`) and bakes a signed
  distance grid (exact triangle distances through a BVH; sign by ray parity along three axes,
  majority vote) plus the triangles with a BVH. `Volume::distance` uses the grid away from the
  surface (moved toward zero by `GRID_ERROR` voxels, so it stays a lower bound) and the exact
  nearest-triangle distance within `NEAR` voxels: thin parts and open surfaces become shells and
  stay visible at any distance. Colors come from the nearest triangle (material color × vertex
  color × texture), resolved through a `Look` (cached per volume and settings, `Model::new`):
  the file's materials with the scene's `ModelRef::materials` overrides (color, color texture,
  bump, specular and normal maps, emission; image paths relative to the model). Normal maps
  (`Volume::normal_map`) use the triangle's texture derivatives; green points up the image
  (`-dpdv` unless the texture counts v from the bottom). Emission is added to the shaded color
  (`model_emis` in `trace`). Override images load in the
  background (`texture_file`, bumps `generation()`). `auto_assign` matches images near the
  model to materials by name (kind from words like diffuse/bump/spec); the app runs it once per
  imported model and from the Materials section.
  `mesh::request` bakes in the background (cache keyed by file, modification time and options;
  `generation()` changes when a bake finishes); the distance grid is also cached on disk
  (`bake_cached`, `grid_cache_dir`: one `.grid` file per file/time/options, checked against the
  grid size and a triangle checksum, oldest files removed above 2 GB), `compose::load_models` waits for all models of a
  scene (CLI, exports). GPU: `vols` buffer (binding 7, `volumes_gpu`, 32-word header per object, 16 words per look material),
  read by `model_de` / `model_nearest` / `model_rgb` in march.wgsl exactly like
  `Volume::distance` / `nearest` / `color`. `cargo run --release -p rmb-core --example model_info
  -- model.glb` prints materials and bake statistics.
- `texture.rs` holds the layers (mapping, blend modes; targets color, bump, normal map,
  emission, specular, roughness). `texture::apply` returns `Applied` (color, emission, highlight
  factor, roughness as keep/add), `texture::bump` bends the normal for bump and normal map
  layers (normal maps: tangent frame from one-texel differences of the layer's u/v, per
  triplanar projection). `Map::alpha` keeps an image's alpha channel; `TextureLayer::drift`
  scrolls the offset in animations (phase `m.l{i}.{k}`). GPU: layer parameters in `lut` from
  `LUT_LAYERS` (4 layers × 7 entries per material, [6].w: use alpha), layer images in the
  `maps` buffer under 0xffffffef − index (`renderer.rs::layer_images`); texels are packed with
  alpha in the top byte (255 without).
- Compile-time switches keep shaders small: `LAYERS`, `BUMP` and `MODELS` (codegen.rs, from
  `Program::layers` / `bump` and the model objects) include that code only when the scene uses
  it. Lookups sit in loops with one call site each (the compiler inlines every call; unrolled
  lookups tripled the compile time).

## Lens flares

- `crates/rmb-core/src/flare.rs` draws the flares in screen space on the CPU (one implementation
  for preview and export): `HdrImage::finish_scene` adds them after denoising, the preview gets
  the layer through `GpuRenderer::set_flare` (display.wgsl binding 3, computed on a worker
  thread in `viewport.rs`).
- Ghosts come from `LensFlare::generated_ghosts` (seeded) unless `ghost_list` holds a
  hand-made layout (`GhostDef`); `ghost_image` (bokeh) replaces the aperture shape by an
  image's brightness.
- `cargo run --release -p rmb-core --example flare_presets -- out.png` renders all presets on
  black, to judge changes to the look (the sheet's display curve lifts dark values strongly).

## Explore lane

- `rmb-core/src/explore.rs`: random scenes (`random_framed`: formulas with good parameter
  ranges, palette from `palette.rs`, camera framed with `frame`/`coverage` via CPU picks) and
  mutations (`mutate`). `rmb-app/src/explorer.rs`: the bottom lane (Presets, Random,
  Mutations); thumbnails render one after another on a worker thread with one unbaked
  `GpuRenderer` (shaders reused), with `march.detail` scaled to the scene's image height so
  the colors match the full image. Presets are `crates/rmb-app/presets/*.rmb`, written by
  `cargo run --release -p rmb-core --example make_presets -- crates/rmb-app/presets`.

## User manual

- `docs/manual/NN-*.md` (one chapter per file, sorted by name; `# ` chapter, `##`/`###`
  sections; `![Caption](images/x.png)` alone in a paragraph is a figure, `{: .half}` makes it
  half wide; `!!! tip` / `!!! note` / `!!! warning` boxes; `{{version}}`). Conventions as in
  the RWildFire manual.
- The app embeds every chapter and image (`rmb-app/build.rs` → `manual.rs`, Help › Manual);
  its renderer covers the subset above plus pipe tables and fenced code. A test checks that
  every referenced image exists.
- PDF: `python3 tools/build_manual.py` (Python `markdown`, Chrome, pdftotext) →
  `docs/manual/build/RMandelbulb3D-Manual-<version>.pdf` (gitignored); Help › Manual › PDF
  manual opens it.
- Screenshots: Xvfb (see below), the right panel's left edge moves with its content (find it
  per shot); keep private paths out of them. Rendered pictures: JPEG, 960 × 540 or smaller.
- How-to example scenes and their pictures: `python3 tools/make_manual_examples.py`
  (`docs/manual/examples/*.rmb`, `images/howto-*.jpg`); presets: `make_presets` example.

## Testing the app visually

- Xvfb on `:99` (1600×1000), separate `XDG_CONFIG_HOME`; `xdotool` + `import -window root`.
  The NVIDIA GPU works through Xvfb (compute plus egui). Stop with `pkill -x rmb-app`.
