# RMandelbulb3D

A 3D fractal explorer and renderer in Rust, after [Mandelbulb3D](https://github.com/thargor6/mb3d).
Distance-estimated ray marching on the GPU (wgpu compute) with a CPU reference renderer, a
modern egui interface and import of Mandelbulb3D parameters.

## Features

- **Hybrid formulas**: up to six formula slots with iteration counts and "repeat from", Julia
  mode, 4D slice, analytic or numeric distance estimation.
- **Formula language**: formulas are small text files (`*.mbf`). Each one compiles to WGSL
  for the GPU and to closures for the f64 CPU renderer. User formulas live in
  `~/.config/rmandelbulb3d/formulas`.
- **Built-in formulas**: Mandelbulb variants, Mandelbulb3D's Integer/Real Power, Tricorn,
  Quaternion, Folding Int Pow, Amazing Box/Surf, ABox Mod, Kali box, Kaliset, Mandelbox 4D,
  Menger, Sierpinski, Octahedron, kaleidoscopic IFS, hypercomplex, and transforms (rotate,
  scale, folds, inversion, …).
- **Mandelbulb3D formulas**: about 250 of Mandelbulb3D's machine-code formulas (`.m3f`) are
  translated into the formula language (`rmb-cli m3f`) and built in.
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

## Status and limits

- Not yet translated: dIFS formulas (107), Pascal-source JIT formulas, and formulas with loops,
  bit operations or map lookups (about 100). Scenes that use them open with the missing slots
  listed and skipped. 34 of the 80 Mandelbulb3D example parameter files are fully supported.
- Not yet supported from MB3D: interpolation and DE-combination hybrids (imported as
  alternating), inside rendering, 4D rotations, background images, positional lights.
  Lighting and colors are converted approximately.
- The GPU renders in f32, so very deep zooms need the CPU renderer (`rmb-cli render --cpu`).

## License

GNU LGPL 2.1 or later, like Mandelbulb3D (see `LICENSE` and `NOTICE`).
