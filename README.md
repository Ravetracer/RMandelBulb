# RMandelbulb3D

A 3D fractal explorer and renderer in Rust, after [Mandelbulb3D](https://github.com/thargor6/mb3d).
Distance-estimated ray marching on the GPU (wgpu compute) with a CPU reference renderer, a
modern egui interface and import of Mandelbulb3D parameters.

## Features

- **Hybrid formulas**: up to six formula slots with iteration counts and "repeat from", Julia
  mode, 4D slice and rotation, analytic or numeric distance estimation, and Mandelbulb3D's DE
  combination (two chains joined by union, intersection, subtraction or a smooth blend).
- **Formula language**: formulas are small text files (`*.mbf`). Each one compiles to WGSL
  for the GPU and to closures for the f64 CPU renderer. User formulas live in
  `~/.config/rmandelbulb3d/formulas`.
- **Built-in formulas**: Mandelbulb variants, Mandelbulb3D's Integer/Real Power, Tricorn,
  Quaternion, Folding Int Pow, Amazing Box/Surf, ABox Mod, Kali box, Kaliset, Mandelbox 4D,
  Menger, Sierpinski, Octahedron, kaleidoscopic IFS, hypercomplex, and transforms (rotate,
  scale, folds, inversion, …).
- **Mandelbulb3D formulas**: 478 of Mandelbulb3D's 497 formulas plus the built-in ones:
  machine-code formulas (`.m3f`, dIFS included) and the Pascal-source JIT formulas (also the EM
  collection) are translated into the formula language (`rmb-cli m3f`) and built in; a few are
  written by hand (`lib/mb3d_builtin.mbf`). Translations and hand-written versions are checked
  against the original code running in an x86 emulator (`tools/m3f-check`).
- **Rendering**: progressive anti-aliasing, up to six lights with soft shadows, ray-traced or
  fast ambient occlusion, sky/ground ambient, gradient coloring (iterations, orbit trap, radius,
  normal), depth fog, glow, depth of field, cutting planes, filmic tone mapping, vignette.
- **Navigation**:
  - Orbit around the point under the cursor; the wheel zooms toward the cursor.
  - Fly mode with WASD, at a speed that adapts to the distance from the surface.
  - Double-click looks at a point and sets the focus there.
  - Low-resolution preview while moving.
- **Files**:
  - Native `.rmb` (JSON).
  - Mandelbulb3D `.m3p` files, and MB3D text parameters pasted with Ctrl+V.
  - PNG export that embeds the scene, so opening the PNG restores it.
- **CLI**: `rmb-cli render|convert|info|formulas`.

## Build

```sh
cargo build --release -p rmb-app -p rmb-cli
./target/release/rmb-app [scene.rmb | params.m3p]
./target/release/rmb-cli render params.m3p -o out.png -W 1920 -s 128
```

## Denoiser

Intel Open Image Denoise (OIDN 2.5) cleans up the noise of ray-traced occlusion, soft shadows
and depth of field, so far fewer samples are needed. It gets the averaged image before post
processing plus helper images from the ray marcher (surface color and normal of the first hit),
runs on NVIDIA (CUDA) or AMD (HIP) GPUs or the CPU, and is applied to saved images when
Effects › Image › Denoise is on (`rmb-cli render --denoise [blend]` / `--no-denoise`). "Denoised"
in the toolbar shows the preview through it. The library is loaded at runtime, so the program
builds and runs without it: `python3 tools/fetch_oidn.py` downloads the official release
(SHA-256 checked) into `oidn/`. Search order: `RMB_OIDN_DIR`, next to the executable (`lib/` on
Linux and macOS), `oidn/` of the source tree, the system library path.

## Status and limits

- Not yet translated: formulas that switch the hybrid's formulas (5), use random numbers (3)
  and a few others. Scenes that use them open with the missing slots listed and skipped. All 80
  Mandelbulb3D example parameter files are supported.
- Formulas that read Mandelbulb3D maps (height maps, map translations) find them by number in
  Mandelbulb3D's `M3Maps` folder: Preferences › Maps, `RMB_MAPS_DIR`, or a `maps` folder next
  to the program.
- Inside rendering (views from within the set) works like Mandelbulb3D's: rays step through the
  set in small steps, so it is slower.
- Not yet supported from MB3D: interpolation hybrids (imported as alternating), the dIFS "mix"
  combination, reflections, ambient light from background images.
- Imported scenes shade like MB3D (display space, depth fog over the view range, dynamic fog,
  MB3D's color index, depth of field); results are close, not pixel-identical.
- The GPU renders in f32, so very deep zooms need the CPU renderer (`rmb-cli render --cpu`).

## License

GNU LGPL 2.1 or later, like Mandelbulb3D (see `LICENSE` and `NOTICE`).
