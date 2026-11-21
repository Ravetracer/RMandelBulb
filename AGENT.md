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
| `crates/rmb-core` | `scene.rs` (model), `formula/` (language `lang.rs`, `cpu.rs` closure compiler, `wgsl.rs` emitter, `lib/*.mbf` built-ins, `m3f/` MB3D machine-code translator, `lib/mb3d.mbf` its generated output), `fractal.rs` (hybrid `Program`, CPU evaluator), `render/` (`cpu.rs` reference renderer, `mod.rs` frame setup and post processing), `io/` (`.rmb` JSON, `m3p.rs` MB3D import, PNG with scene) |
| `crates/rmb-gpu` | `march.wgsl` (kernel, mirrors `render/cpu.rs`), `codegen.rs` (hybrid `iterate` function), `display.wgsl` (post), `renderer.rs` (progressive bands, async shader compile) |
| `crates/rmb-app` | egui app: `app.rs`, `viewport.rs` (preview and navigation), `formulas_panel.rs`, `inspector.rs`, `export.rs`, `doc.rs` (undo), `prefs.rs`, `theme.rs` (from RWildFire) |
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
- Regenerate from both folders: `rmb-cli m3f ../mb3d/M3Formulas ../mb3d/EM_JIT_M3Formulas -o …`.
- Regenerate without piping the output into `head`: a closed pipe kills `rmb-cli` before it
  writes the file.

## Testing the app visually

- Xvfb on `:99` (1600×1000), separate `XDG_CONFIG_HOME`; `xdotool` + `import -window root`.
  The NVIDIA GPU works through Xvfb (compute plus egui). Stop with `pkill -x rmb-app`.
