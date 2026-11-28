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

## Versions and releases

- One version for all crates: `[workspace.package] version` in Cargo.toml, `MAJOR.FEATURE.FIX`
  (major: incompatible files / network protocol; feature: new features; fix: bug fixes and
  small improvements). Note user-visible changes under `## [Unreleased]` in CHANGELOG.md as
  part of each change.
- `tools/release.py fix|feature|major [--push]` bumps, moves the changelog entries, commits
  and tags `vX.Y.Z`; the tag runs `.github/workflows/release.yml` (packages via
  `tools/package.py` + `packaging/`, release notes from the changelog section). CI
  (`ci.yml`) runs clippy and the tests without GPU on every push.
- Linux builds run on Ubuntu 22.04 (glibc 2.35); binaries built on this machine need glibc
  2.39. AppImage locally: build it in a throwaway `ubuntu:22.04` Docker container (download
  and run appimagetool only inside it).
- Platform folders: `rmb_core::paths` (settings, caches). OIDN is looked up next to the
  program, in `lib/`, `../lib` (AppImage) and `../Frameworks` (macOS bundle).

## Layout

| Path | Content |
|------|---------|
| `crates/rmb-core` | `mesh_export.rs` (mesh export: grid, surface nets, PLY/OBJ/STL), `compose.rs` (scene composition: objects, primitives, boolean ops, transforms; mirrored in march.wgsl), `mesh.rs` (3D model import and distance grid bake), `texture.rs` (texture layers), `palette.rs` (palette generator and edits), `flare.rs` (lens flares: model, presets, screen-space layer added before post processing), `animation.rs` (keyframes, camera path, timing, cameraman smoothing, flight → keyframes), `video.rs` (PNG sequences, movies through ffmpeg), `scene.rs` (model), `formula/` (language `lang.rs`, `cpu.rs` closure compiler, `jit.rs` Cranelift JIT of it, `wgsl.rs` emitter, `lib/*.mbf` built-ins, `m3f/` MB3D machine-code translator, `lib/mb3d.mbf` its generated output), `fractal.rs` (hybrid `Program`, CPU evaluator), `render/` (`cpu.rs` reference renderer, `mod.rs` frame setup and post processing), `io/` (`.rmb` JSON, `m3p.rs` MB3D import, PNG with scene) |
| `crates/rmb-gpu` | `march.wgsl` (kernel, mirrors `render/cpu.rs`), `codegen.rs` (hybrid `iterate` function), `display.wgsl` (post), `renderer.rs` (progressive bands, async shader compile) |
| `crates/rmb-app` | egui app: `app.rs`, `viewport.rs` (preview and navigation), `formulas_panel.rs` (scene list, object editors), `inspector.rs`, `layers.rs` (texture layer editor), `palette_editor.rs` (gradient editor: stops on the strip, generator, tools, saved palettes in `palettes.json`), `export.rs` (image and animation export), `render_view.rs` (test render window: `render_progressive`, keeps the raw image and develops it again when the post processing changes), `timeline.rs` (animation panel: keyframes, recording, playback), `gizmos.rs` (light gizmos; object gizmos: move, rotate rings, scale/stretch handles), `doc.rs` (undo), `prefs.rs`, `theme.rs` (from RWildFire) |
| `crates/rmb-cli` | Command line (`net.rs`: network render commands) |
| `crates/rmb-net` | Network rendering: `proto.rs` (messages, specs, hashes), `discovery.rs` (UDP announcements), `assets.rs` (files of a job, client cache), `client.rs`, `server.rs`; test `tests/loopback.rs` |

## Build and test

```sh
cargo build -j 8 -p rmb-app -p rmb-cli
RAYON_NUM_THREADS=8 cargo test -j 8 --workspace -- --test-threads=8
cargo clippy -j 8 --workspace --all-targets
```

- `rmb-gpu/tests/shaders.rs` checks every library formula's WGSL with naga and renders a
  sample on the GPU (`formula_sample_renders_on_the_gpu`: one formula per kind plus every
  25th; each shader is a full driver compile, seconds each after a `march.wgsl` change empties
  the driver's disk cache). The whole library on the GPU: `cargo test --release -p rmb-gpu
  --test shaders -- --ignored` (after regenerating or translating formulas). It also compares
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
  including the random number order. Change both together. The first `LD_DRAWS` (6) random
  numbers of a sample come from the R2 sequence over the sample index with a per-pixel offset
  (low discrepancy; u32 fixed point, identical on both); later draws are PCG.
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

## CPU JIT

- `formula/jit.rs` compiles a formula's `iter` block with Cranelift into
  `extern "C" fn(*mut f64) -> u8` over the same register file as the closures of
  `formula/cpu.rs`. `CpuFormula::iter` compiles on first use (`OnceLock`) and falls back to the
  closures when the host is not supported or compilation fails; `RMB_NO_JIT=1` forces the
  closures (`iter_closure` always uses them). Compiled code is cached per AST + layout for the
  process (never freed); all ~480 library formulas compile in well under a second together.
- Registers live in SSA variables (loaded at entry, written ones stored at exit; faster than
  load/store per access). Arithmetic, comparisons, sqrt/abs/floor/ceil/round, min/max (with
  `f64::min`/`max` NaN rules), `single`, `sel` are native; transcendentals, `pow` (`gpu_pow`),
  `%` (Rust remainder), smoothstep and map lookups call Rust helpers, so results are
  bit-identical. `jit::tests` compares every library formula with the closures on random
  points. A change to the language (`lang.rs` functions or statements) needs both compilers.

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
  (prints the count and the failure reasons). Then run the GPU tests including the ignored
  `every_formula_renders_on_the_gpu` (`-- --ignored`): every formula must give valid WGSL
  and render.
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
  Not supported: map functions, random numbers; formulas that switch the hybrid's formulas
  (`_Cond*`) are hand-written (below).
- Formulas that set slot iteration counts (MB3D's `_Cond*`, hand-written in
  `lib/mb3d_builtin.mbf`): statement `setcount(slot, count)` (`Stmt::SetCount`, slot numbers
  1-based in `Fractal::slots`; `FormulaDef::sets_counts`). CPU: the counts live in
  `COUNT_SLOTS` registers after the locals (`CpuFormula::count_regs`, filled and read back by
  `iterate_from`; JIT: branch-free selects). The hybrid loop keeps a count per slot for each
  point (reset to the scene's counts at the start of every orbit; MB3D keeps them from pixel to
  pixel), fixes a slot's run length when it is entered and skips slots with count 0.
  `Program::build` keeps zero-count slots in chains with such a formula (`Program::first_slot`
  maps slot numbers). GPU: only for those chains `cnt`/`run` in `codegen.rs` (`counts_init`,
  `counts_skip`, `emit_iter_counts`); other shaders are unchanged. The m3p import keeps the
  zero-count slots and maps the formula numbers (params `slot*`) to scene slots.
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
- DE kinds `maxaxis` and `custom` (`DeKind`): custom formulas write `dist` (alias of `a7`,
  `lang::STATE_DIST`); `Iterated::custom` / WGSL `Iterated.cd` carry its final value, the
  chain uses it whether escaped or not, and `Program::new` sets the bailout to 1e18 (no escape;
  finite, as the baked shader squares it). Kleinian formulas in `lib/kleinian.mbf`.
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

## Projections and stereo

- `Scene::view` (`View`: `Projection`, `StereoMode`). `render::ViewFrame` (in `Frame::view`)
  does the work for CPU and GPU: `eye_screen` maps an image position to the eye's
  sub-image (side by side / over-under split the image; `Frame::aspect` is one eye's aspect),
  `ray` gives origin and direction (perspective, equidistant fisheye, equirectangular; eyes
  offset along the camera's right or, in panoramas, the ray's horizontal right, both aimed at
  the point at the convergence distance). Anaglyphs render both eyes per sample
  (`Shader::sample_aux` → `sample_eye`; GPU `sample_image` loops over `sample_pixel` with one
  call site) and mix them (`ViewFrame::anaglyph`). GPU: uniforms `view`, `view2`,
  `eye_screen`, `view_dir` in march.wgsl. Pixel angle per projection in `Frame::new`.
- Screen-space overlays (gizmos, guides, path overlay) assume the plain perspective; they are
  skipped unless `View::is_plain()`. Lens flares (`flare::layer`) work in perspective stereo:
  one `mono_layer` per eye (camera moved by half the eye distance), placed into the eye's
  half or mixed like the anaglyph. `cpu::pick` / `pick_object` take
  the projection into account; `cpu::pick_center` marches along the camera's forward axis.

## Mesh export

- `rmb-core/src/mesh_export.rs`: `Grid` (samples of the box), `Field` (one slice of distance
  estimates; `CpuField`, or `rmb_gpu::GridField` with march.wgsl's `grid_main` entry, a
  pipeline of its own that only mesh exports compile; four values per accum entry),
  `surface_nets` (streams two planes at a time; the grid is padded with empty samples so the
  mesh is closed and caps the box faces; quads oriented outward), `refine` (Newton steps
  along the vertex normal, at most half a cell), `vertex_colors` (`Shader::surface_base`: the
  shading's base color without lighting), writers for PLY/OBJ/STL, `fit_bounds` (coarse search
  over growing boxes). App: `mesh_dialog.rs` (also the voxel mode); CLI: `rmb-cli mesh`.
- `voxel_export.rs`: the same `Grid`/`Field`, three rolling layers (shell = filled with an
  empty 6-neighbor), colors only on shell voxels (the inside gets the layer's mean), PNG slices
  or MagicaVoxel (`write_vox`, median-cut `palette`; resolution limited by `limited`).
- `mesh_simplify.rs`: `decimate` (Garland-Heckbert quadrics, heap with lazy deletion by
  vertex versions; link condition, no flips; vertices not on a single closed fan are frozen),
  after `refine`, before `vertex_colors`.

## Animation drivers

- `rmb_core::expr` (parser and evaluator, `noise`), `animation::Driver` (path, expression) in
  `Animation::drivers`; `AnimationPlan::scene_of` applies them last (`apply_drivers`: scene →
  JSON, `value_at` / `set_value_at`, back; formula parameters at their default come from the
  formula library, `param_default`). `drivable_values` lists the paths for the timeline's
  picker (`timeline.rs` › `drivers_ui`).
- Audio-reactive drivers: `rmb_core::audio` (ffmpeg decode, one-pole band split, RMS at 50 Hz,
  95th-percentile normalization, attack/release envelope) → `Animation::audio_levels` (stored
  in the scene); `AnimationPlan` feeds `audio/bass/mid/treble` (with the sound offset).
  `Animation::ensure_audio_levels` runs before CLI animate, queue jobs, network jobs and the
  export window; the timeline analyses a newly chosen sound in the background.
- Stills: `Animation::still_drivers` → `animation::still_scene` (frame 0, `value` = the scene's
  own value; the result has the flag off, so it is never applied twice). Called in
  `GpuRenderer::set_scene`, `CpuRenderer::new` / `set_scene_keep`, `save_render_ex`, the
  mesh/voxel exports and `GridField::new`.

## Render queue

- `rmb_core::queue` (Queue, QueueJob with a scene snapshot or file, JobWhat image/animation,
  JobState; JSON), `rmb_gpu::batch::run_job` (one job: tiles, CPU fallback, animation through
  `render_animation`). App: `queue_window.rs` (file `prefs::config_dir()/queue.json`, one job
  at a time on a thread, polled in `App::update`; export window "Add to queue" fills
  `Export::queued`). CLI: `rmb-cli batch`.

## Network rendering

- Clients (`rmb-cli --network`) broadcast an `Announce` (UDP 47810, also to 127.0.0.1) every
  2 s and listen on TCP 47811; the server (`server::run_job`) connects to the chosen ones.
  Messages: length-prefixed JSON, file/result messages followed by raw bytes (`proto.rs`).
  Version strings must match (`rmb_net::version`, bump `PROTOCOL` when messages change).
- Files: `rmb_core::files::visit` lists every file name of a scene (and its keyframes);
  `assets::prepare` makes them absolute and adds model sidecars (`files::model_files`) and the
  formulas' maps; clients store blobs by SHA-256 and recreate the server paths below
  `jobs/<id>/root` (`assets::mirror`, no `..`), maps in `jobs/<id>/maps` (`maps::set_dirs`).
- Work: pull-based (`Work::next`), tail duplication when an idle client is much faster
  (measured seconds per sample, hardware guess before), `Progress` replies may `Cancel`.
  Frames: `rmb_gpu::FrameRenderer` (one shader per job). Stills: `Samples { start, count }`
  with `start_at_sample` on the GPU/CPU renderer, summed on the server (`StillSum`), so the
  split render equals a single one. Results stay in the client's spool folder until `Ack`.
- Tiled stills (`JobRequest::tiles`, auto above `io::image::auto_tile`): `Unit::Tile { i }`
  over `render::denoise::TileLayout` (tile size and margin in `JobSpec::tiles`); clients
  render the region with all samples (`rmb_gpu::TileRenderer`, `CpuRenderer::set_tile`) and
  send it undenoised; the server's `TiledStill` denoises and pastes (`HdrImage::paste_tile`).
- `client::start_local` runs a client in-process ("this computer too", `--local`).

## Tiled renders

- `rmb_gpu::render_tiled_hdr` (layout `render::denoise::TileLayout`, regions by
  `TileRenderer`): equal-size tiles (edge tiles moved inward; one shader), uniform
  `tile` = left, top, full width, full height (`GpuRenderer::set_tile`). The kernel passes
  full-image pixel coordinates to `sample_image` and `eye_screen` uses the full size, so the
  rays and random numbers equal a whole-image render. Tile offsets are never baked
  (`uniform_words_never_baked`). With the denoiser, tiles get a `TILE_MARGIN` overlap and are
  denoised one by one; `io::image::save_render_ex(…, denoised = true)` finishes without
  denoising again. `io::image::auto_tile` decides Auto.

## Image formats

- `io::image::save_render` with `ImageKind` (PNG 8 / 16 bit, EXR). EXR (`write_exr`, `exr`
  crate) takes `HdrImage::finish_linear` (denoised, display space decoded, no post). The GPU
  `aux` buffer's w components carry the summed hit distance (`aux_depth * aux_hit`) and hit
  count (`aux_hit`); `read_hdr` / `CpuRenderer::hdr_image` turn them into `HdrImage::depth`
  and `alpha`.

## Glossy reflections

- `Material::gloss`: `bounce()` (cpu.rs, march.wgsl) draws two random numbers for a GGX micro
  facet (`ggx_normal`) only when gloss > 0, reflects and refracts on it; reflections below
  the surface are mirrored back above it. The weight stays the reflectance.

## Water

- `Primitive::Water` (`compose.rs` `water_wave01` / `water_height` / `water_distance` /
  `water_look`, mirrored in march.wgsl): `detail` waves exp(sin - 1) from 2.5 × the wave
  length down to 1/80 (height ∝ wave length, `WATER_FALLOFF`), random phases and directions
  around the wind from `hash3`, deep-water speeds (√frequency), drag along the slope
  (`choppy` × 0.38); the sum is standardized (mean 0.4658, spread 0.3025 of exp(sin - 1)),
  so the height range does not depend on the number of waves. Slow domain warping (scale
  5 wave lengths) and wave groups (scale 7) keep wide views from repeating. The distance
  divides by `water_steepness` (measured; test `water_slopes_stay_below_the_distance_bound`).
  Parameters on the GPU in `Obj.prim` / `prim2`. Water reflects by itself:
  `Surface::mirror` / `Traced.mirror` = Fresnel (`water_fresnel`), used instead of the
  material's reflection amount in the bounce loop; `Program::has_water` turns on the bounce
  loop (GPU `WATER`, `is_water(j)` from codegen, `REFLECT`). Glints: shininess at least
  `WATER_SHININESS`. Animations add `speed` per second to `phase` (`color_speeds` key
  `o{k}.water`).
- Shore interaction: `Evaluator::orbit` (and codegen `compose_objects`) combines water
  objects after all others and passes the distance so far (`dry`, divided by the object's
  scale) as the shore distance `s`; the winning water orbit keeps it in `Orbit::shore` (GPU
  `Orbit.shore`) for `water_look`. `water_shore`: waves exp(sin - 1) running in along `s`
  (length `SHORE_LEN`, reach `SHORE_RANGE` wave lengths, crests bent by noise), steeper and
  higher toward the coast, collapsing at it; foam belt and breaking foam in `water_look`.
  The distance bound adds `water_shore_steepness` within reach and blends out beyond it
  (`dz / steep + x (1 - open / steep)`, no min/max switch: a kink there tilts normals).
  `water_spray`: one drop per cell (3 × 3 cells checked, margin inside the cell, so the
  bound is ≥ a cell where no drop flies), flight after each crest reaches the coast
  (`water_shore_phase`), radius falling with `s`, size `a[10]`, variation `a[11]`, stretched
  along its velocity. Sideways flight: `water_spray_drift` warps the grid by a smooth field
  (direction and distance from noise, out during the flights and back while none fly), so
  the bound divides by 1 + `water_spray_warp` (its measured rate of change); its zone bound is combined with `max`, never
  returned alone near the zone (a bound that goes to 0 at a zone's edge renders as a
  surface). Parameters `a[8]` shore, `a[9]` spray (`Obj.prim3`); test
  `water_with_a_shore_stays_a_distance_bound` checks balls of the bound for sign changes.

- Water medium (`compose::WaterMedium` in `Primitive::Water::medium`, `Program::medium` =
  the first see-through water, part of the signature; GPU `MEDIUM`, `MEDIUM_OBJ`, uniform
  `wmed` from `renderer::medium_gpu`): `Evaluator::water_mode` / private `water_mode`
  (`WATER_SOLID`, `WATER_INSIDE`: the negated `water_body_distance` without spray, the air is
  the solid; `WATER_IGNORED`: 3e38, used by `soft_shadow` / `ambient_occlusion`, and by
  `distance_at_camera` / `zoom_ratio`, so navigation speed and Auto precision ignore the
  medium; picks start in `WATER_INSIDE` when the camera is under the water). The bounce
  loop keeps `in_water` (camera: `MediumFrame::depth` > 0, open sea without shore waves) and
  passes it to `trace` (`Seg::water`); at the medium's surface `water_at` gives reflection
  (`water_fresnel` above, `fresnel_exact` with TIR below) and `see` (passing share; foam is
  the only diffuse part), `water_bounce` picks one by share and refracts; the start point
  after a crossing steps on until `MediumFrame::clearance` ≥ 1.05 eps (no extra fractal call
  site: compile time). In water: no fog, background or visible lights; `medium_path` fades by
  `exp(-ext t)` and adds the water color lit from above (`water_top`) darkened by
  `exp(-fade depth)`, depth linear along the ray (closed-form integral, exponents kept ≥ 0
  for f32). Lit points under water: ambient × `exp(-fade depth)`, each light × `water_light`
  (Fresnel at a flat surface, `fade depth / cos`; lights under the water: `exp(-ext dist)`,
  bits in `wmed[1].w`). Tests: `render::cpu::tests`, `gpu_matches_cpu_under_water`; demo
  scenes: `cargo run --release -p rmb-core --example underwater_demo -- dir`.
- Caustics (`compose::water_caustic`, GPU same name, ten heights from one loop = one call
  site): the surface point a flat surface would refract toward the point, three points
  `caustic_step` apart refracted through the real wave normals (differences) down to the
  point's depth, brightness = 1 / |area ratio| (≤ `CAUSTIC_MAX`), contrast × strength ×
  exp(-depth / 4 wave lengths); jitter (two random numbers per wet light in the light loop)
  against aliasing. Light rays: `water_ray_distance` (by the mean extinction) +
  `water_ray_tint`, sharing the fog-scatter block's single `soft_shadow` call (a second call
  site doubled the compile time); their caustic lands at `RAYS_SHAFT` wave lengths below
  the entry point (`shaft` argument: the pattern stays constant along the light → shafts)
  with `RAYS_CONTRAST`. Scattering `WaterMedium::scattering` (murk + a little), phase HG
  `RAYS_FORWARD`. Medium scenes compile in ~14 s (11 s without caustics and rays).
- Bubbles and particles (`DropKind` in `MediumFrame::drops`, GPU `wmed[5..7]`):
  `water_drop` walks a 3D grid (Amanatides-Woo, `DROP_CELLS`) in the water's rigid frame
  (rotation + position, no scale: the GPU's deep zoom shifts world positions relative to
  the camera, so a world-anchored grid did not match the CPU); `drop_sphere` = one sphere
  per cell by `hash3` (well inside, so the first hit is the nearest; test against brute
  force). The grid rises along the water's z by `time × speed`. Plumes
  (`BubbleSource::Vents`, `MediumFrame::plumes`, GPU `wmed[8..14]`, `plume_density`):
  random vents per `vent_spacing` cell (3 × 3 checked) plus `Program::bubble_sources`
  (objects with `SceneObject::bubble_source`, in the water's frame), axes winding with the
  water-frame height; `plume_cell_hit` splits a plume cell into 2 × 2 × 2 sub-cells with a
  bubble each (inside its sub-cell, so the walk stays nearest-first). Ray-sphere by the closest
  approach (f32). `water_drops` composites after `medium_path` (farther first): bubbles by
  `fresnel_exact` (TIR rim), light from above + glint; particles 0.7 opaque. No random
  numbers. `WaterMedium::time` advances 1 per second in animations (phase `o{k}.medium`).
- Waterline: the ray starts on the camera's front glass (`waterline_point`, `lens` wide;
  only within two glass sizes of the surface, blended in over one, as its width shifts the
  view; test `water_far_below_leaves_the_fractal_alone`), in the water when that point is
  below the open sea; near the surface the first march
  leaves it; `waterline_shade` darkens the meniscus. `flat_lens`: rays starting in the water
  refract at the glass (normal = -forward, 1/ior) before the first trace. Lens drops
  (`lens_drop`, `lens_drop_shade`): screen-space grid (`LENS_DROP_CELL`, 3 × 3 cells, each
  column sliding down by its own speed × `time`) on glass points above the water, share =
  `lens_drops` × `lens_wet` (drops shrink near their own threshold, `LENS_DROP_SHRINK`, so
  they vanish one by one while drying). `cpu::lens_wetness` (1 when the camera left the
  water, linear to 0 over `dry_time`; spray ticks by `compose::water_spray_zone`) raises
  `lens_wet` in `AnimationPlan::scene_of` (from the frames' path) and in the preview
  (`viewport.rs` `LensWet`, camera history; quantized, as each change restarts the samples); the drop bends the ray (inverted view), darkens
  the rim and adds a sky-colored highlight after the bounce loop. `lens_drop_look` with
  `drop_focus`: out of focus a soft edge (three random numbers per sample inside a drop:
  look through it by the edge share, blur jitter) and a bokeh-like brighter center.
  Trails (`lens_trail`, GPU same, `wmed[9].y`): above each drop (in its column's sliding
  frame) up to the way it came (`time × speed`, at most most of a cell), narrower and
  fading upward; outside drops only, the same three random numbers for a small smear.
- `Primitive::Seabed` (`SEABED_KIND` 10, `seabed_height` / `seabed_distance`, GPU same):
  4 octaves of value noise + warped sine ripples; bound by the measured slope
  (`seabed_steepness`, test `seabed_stays_a_distance_bound`). Parts (`seabed_parts`, params
  a[5..12], GPU `prim2`/`prim3`, so `prim_de` takes `prim3`): in the ground-following frame
  (x, y, height above the ground; the shear stretches distances by ≤ 1 + steepness):
  `seabed_stones` (rocks and pebbles: one turned, partly buried ellipsoid per 2D cell, 3 × 3
  cells, bumps by a sine product) and `seabed_weed` (two ribbon blades per cell in noise
  meadows; exact strip distance divided by the axis/taper/twist rate). Far cells are covered
  by a fallback bound from the cell size. `Pattern::Seabed` (index 5, `seabed_color`) maps
  the nearest part to gradient ranges (`seabed_gradient`); `phase` += `sway_speed` per second
  (`color_speeds` key `o{k}.seabed`). Test `seabed_parts_stay_a_distance_bound`.
  Bounds must stay tight away from the parts: soft shadows read the estimate as the
  distance to the nearest surface (`d / (softness t)`) and have 160 steps, so a small
  fallback darkens and bands shadows. Each part's bound is at least its height gap
  (`max`), seaweed searches 5 × 5 cells with a cylinder cull and an edge-based fallback.
- Plants (`plants.rs`, `SceneObject::plants` on a seabed, `ObjectProgram::plants` =
  `PlantField`): stochastic L-system `expand` → turtle `draw` (round-cone branches, flat
  leaves) → `PlantTemplate` (height 1, BVH; cached by the growth settings, `template`) →
  `PlantField::nearest` (per species 5 × 5 cells of the seabed's local x/y, root on the
  ground under it, turn/size/sway per plant, cylinder cull, block-edge fallback; never the
  bare height gap, which is a flat plane to the soft shadows). Box distances in the BVH are
  divided by `PlantTemplate::norm` (the segments' bounds are divided by their slope).
  Colors: `color_value` returns `PLANT_COLOR + …` (`decode_color`: object, species, leaf,
  height), `Shader::gradient` / march.wgsl `gradient` decode it. GPU: `plant_de` /
  `plant_tpl` / `plant_seg`, data in `vols` (`renderer::plants_gpu`: object header word 0
  species count, 1 block offset; blocks of `PLANT_BLOCK` words), switch `PLANTS`, codegen
  `plant_de` after `prim_de` for seabeds with plants. Editor `rmb-app/src/plants_editor.rs`.
  Tests: `plants::tests`, `gpu_matches_cpu_under_water` (plants view). WGSL reserved words
  bit again: `patch`.
- Parametric L-systems: `expand` returns `Vec<Module>` (symbol + f64 arguments) or an error;
  `parse_modules` reads `F(l*0.8)`, arguments are `expr::Expr`; `PlantRule::params` /
  `condition`; a rule applies when its parameter count fits (or it has none) and its
  condition holds; the random pick draws only when some rule applies (classic systems keep
  their random sequence: test `classic_presets_grow_as_before` checks segment fingerprints,
  example `plant_fingerprint`). `draw`: an argument replaces the default length, angle
  (degrees, variation still added), radius (`!`) or leaf length. Templates only (CPU), so
  the GPU is unchanged. `PlantSpecies::check` (cached) for editor errors.
- Plants on any surface (`plant_cover.rs`; `Scene::plant_cover`, `SceneObject::cover`):
  roots baked per host (0 main fractal in its own coordinates, i + 1 object i in local
  coordinates) by an octree over the region (cells kept while `host_distance` < half
  diagonal), one candidate per spacing cell (share, 3D-noise patches), moved onto the
  surface along the gradient, kept when it stays in its cell (even density) and passes
  `max_slope`; axis = normal blended to up by `upright`, never closer than `MIN_OUT` to the
  surface. Background bakes like `mesh::request` (`request`/`get`/`generation`, key =
  host shape JSON + placement fields; a newer request for the same host cancels the older
  bake); `compose::load_models` also bakes plants. `CoverField` sorts roots into a CSR grid
  (cell ≥ 2 × largest reach, 3 × 3 × 3 search `CELL27`, block-edge fallback);
  `Program::covers` (`HostCover`); `Evaluator::orbit` joins them after the objects, before
  the water; a winning plant sets `Orbit::plant` (its color value, object
  `MAX_OBJECTS + cover`) and the host as `object`. GPU: `covers_gpu` (directory at
  `vols[31]`), `cover_union` / `cover_de` (one call site in `compose_objects`), switch
  `COVER`, uniform `cover` (sway phases). Deep mode drops main-fractal covers. Tests:
  `plant_cover::tests`, `gpu_matches_cpu_with_plant_cover`; demo scenes: example
  `plants_demo`. Plant color values keep the height below `PLANT_HEIGHT_MAX` (0.98): f32
  near 1e5 has steps of 1/128, 0.999 rounded up to the next species.
  Speed: one `SpeciesGrid` per species (cell 2 × its reach), the scan covers only the cells
  within best + reach (at most `COVER_SPAN` 1 beyond the point's cell); a shared grid sized
  by the largest species scanned hundreds of small roots per estimate (bulb close-up 1247 →
  562 ms). Level of detail `March::plant_detail` (pixels; `Program::plant_lod` = camera and
  detail × pixel angle, set by `CpuRenderer` incl. tiles; GPU uniform `cover[2].w`, 0 in mesh
  exports): `PlantNode::size` (thickest segment, `seg_size`), walks skip thinner nodes and
  segments; node stride 9 on the GPU (`PLANT_NODE`, `pnode_d2`; models keep `node_d2`).
  Measured 2026-11-27: 0.25 px 1.3x on the trees demo but twigs vanish (mean diff 2.7), so
  off by default.
- Speed (bench example: `--only plants,seabed` toggles; 2026-11-27, 640 x 360, baked): the
  searches skip cells by `cell_gap` before any hashing, scan `CELL_ORDER` nearest first,
  test the share before the patch noise, stop at the ground's distance (`limit`); the sway
  bound uses the rate at the highest reachable height. Underwater floor view 2040 → 1295 ms
  per pass, plant garden 1113 → 660 ms; plants are still ~70 % of it (GPU threads wait for
  the slowest lane in the BVH walks). `PLANT_STACK` 16 covers 4000 segments.

## Sky

- `rmb-core/src/sky.rs` (`Scene::sky`, `BackgroundMode::Sky`): analytic single scattering
  per channel (Rayleigh/Mie optical depths as RGB, Kasten-Young air mass, HG Mie phase per
  channel, isotropic `MULTI` fill). `SkyFrame::new` precomputes per frame: the sun's light at
  the ground (`sun_light`, full air mass) and at the scattering air (`sun_air`: half the air
  mass, at most `SUN_PATH_MAX`, so low suns do not black out hazy skies); ozone (`OZONE` ×
  `Sky::ozone`) absorbs along the full sun path in both (blue dusk); twilight fades over 18°
  below the horizon. `radiance` = air + sun disk + stars (`star`: hash grid, `STAR_CELLS`);
  `air` leaves out disk and stars and is the fog color with `Fog::from_background` (else the
  sun shines through the fog on surfaces in front of it). Ground below the horizon: haze
  toward the horizon + ground color × `sun_light` × `GROUND_LIGHT`.
- GPU: uniform `atmo` (8 vec4, `renderer::sky_gpu`), march.wgsl `sky_radiance(d, sources)`
  mirrors `radiance_ex`; background mode 3 in `bg_top.w`. No compile switch (small code).
- `sync_sun` writes the sun into light 1 (directional, world-relative, color normalized,
  intensity from the ground light × `sun_light_strength`; adds light 1 when there is none)
  and the ambient light (`hemispheres`: cosine-weighted averages of `air` above and below,
  48 fixed directions; amount = the brightest sky channel × `sky_light_strength`). A stand-in
  for the World item "the sky lights the scene like an HDRI". Called by the app every frame,
  by `AnimationPlan::scene_of`, and through `animation::still_scene` (returns a synced copy
  when it changes light 1), so every renderer and export sees the same light.
- `Sky::day_speed` → phase `sky.time` (hours per second). Editor `rmb-app/src/sky_editor.rs`
  (CPU strip preview, cached in egui temp data). `cargo run --release -p rmb-gpu --example
  sky_sheet -- out.png [--panorama]` renders all presets at six hours (judge model changes);
  `rmb-core` example `sky_demo` writes the manual's sunset scene. Test
  `gpu_matches_cpu_with_the_sky`.

## Light through glass

- `Material::glass_shadows` (`Program::glass_shadows`, GPU switch `GLASS_SHADOW`):
  `soft_shadow` returns a color; at a hit it takes the surface's gradient transparency × the
  glass amount, marches through the solid with `flip` toggled and multiplies the absorption by
  the surface color over that length (up to four crossings; no refraction).

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
  the colors match the full image. Quality filter: `explore::thumb_quality` compares the
  thumbnail at half and all samples (dust and grain change, detail stays; `MAX_NOISE` in
  explorer.rs, tuned with `cargo run --release -p rmb-gpu --example explore_quality -- dir`).
  Snapshots: a fourth strip, session only. Presets are `crates/rmb-app/presets/*.rmb`, written by
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
