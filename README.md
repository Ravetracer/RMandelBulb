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
- **Rendering**: progressive anti-aliasing, up to eight lights (directional, point, spot, area
  and line lights; area and line lights give real soft shadows), ray-traced or
  fast ambient occlusion, sky/ground ambient, gradient coloring (iterations, orbit trap, radius,
  normal), depth fog, glow, depth of field, cutting planes, filmic tone mapping, vignette.
- **Navigation**:
  - Orbit around the point under the cursor; the wheel zooms toward the cursor.
  - Fly mode with WASD, at a speed that adapts to the distance from the surface; Space
    captures the mouse for free steering (full turns, no stop at the window border).
  - Double-click looks at a point and sets the focus there.
  - Low-resolution preview while moving.
  - Lens flares per light (star, anamorphic streak, ghosts, rings, lens dirt; presets),
    hidden when the fractal covers the light.
  - Ground grid (z = 0) with the colored x/y/z axes and an axis cross; click an axis to look
    along it.
  - Lights are drawn in the view with gizmos: move lights freely or along the x/y/z arrows
    (a dashed line shows the spot below them), drag aim points, the spot cone and the area
    size, and turn directional lights on a dial.
- **Files**:
  - Native `.rmb` (JSON).
  - Mandelbulb3D `.m3p` files, and MB3D text parameters pasted with Ctrl+V.
  - PNG export that embeds the scene, so opening the PNG restores it.
- **Scene composition** (Mandelbulber's boolean mode): add fractals and shapes (plane, box,
  sphere, cylinder, torus, water) to the scene, move, rotate, scale and stretch them with gizmos
  in the view (Move / Rotate / Scale in the toolbar), combine them by adding,
  cutting away or intersecting (also smoothly), each in its own colors. Ctrl+click selects the
  object under the cursor.
- **3D models**: OBJ, STL, PLY and glTF/GLB files (Blender exports all of them) as scene
  objects. A distance grid (64 to 256 points along the longest side) covers the space around
  them; near the surface the triangles themselves are used, so thin parts stay sharp at any
  distance. Each material gets a color, a color texture, a bump and a specular map; images near
  the model file (for example in a `textures` folder) are assigned to the materials by name.
- **Texture layers** (up to four per material): images, or a solid color through a mask image,
  over the gradient colors with blend modes (multiply, screen, overlay, soft light, …), as bumps
  or as glow. Triplanar, planar, spherical, cylindrical or orbit mapping, in object or world
  space.
- **Path tracing** (optional, Lighting › Global illumination): Monte Carlo global illumination
  with diffuse and reflection bounces; glowing colors light their surroundings, the ambient
  colors become sky light. Classic Mandelbulb3D-style shading stays the default.
- **Render window** (F12): a test render with everything the export applies; post processing
  changes show there at once.
- **Emissive colors**: gradient colors can shine by themselves, light their surroundings in
  their color and glow (MagicaVoxel-style emission). **Glow bands** lay thin glowing stripes
  over a gradient, independent of its colors; palette rotation and each band's own speed make
  them sweep through the fractal in animations. **Steam**: glowing haze around emissive parts,
  shaped by 3D noise and drifting in animations. **Bloom** and **streaks** add soft halos and
  lines of light around bright parts.
- **Animation** (Animation button, timeline under the view):
  - Keyframes are whole scenes. The camera follows a smooth path through them that never
    overshoots, and every other setting blends between them.
  - **Record flight**: fly in Fly mode and keyframes are created along the flight.
  - Three timings:
    - per keyframe (frames and easing);
    - as flown (with a smoothed speed);
    - steady (one even pace, measured relative to the distance to the surface, so zoom
      flights stay even).
  - An optional "cameraman" smooths moves and turns. It can also look ahead, level the
    horizon and bank into turns.
  - Playback in the view, with the camera path drawn over the image.
  - Export to MP4 (H.264/H.265), ProRes MOV, WebM, GIF (needs ffmpeg) or PNG images.
- **CLI**: `rmb-cli render|animate|convert|info|formulas`.

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

How many samples: for still images 16-32 samples with the denoiser look cleaner than 64 without
it, in a quarter to half the time. In animations the denoiser works frame by frame and
reconstructs fine fractal detail slightly differently each time, which shimmers: at 16 samples
about twice as much as 64 samples without the denoiser, at 32 samples 1.4 times as much. Use 64
samples (with or without the denoiser) for animations with much fine detail. Without the
denoiser the noise pattern stays the same from frame to frame and does not flicker.

## Status and limits

- Not yet translated: formulas that switch the hybrid's formulas (5), use random numbers (3)
  and a few others. Scenes that use them open with the missing slots listed and skipped. All 80
  Mandelbulb3D example parameter files are supported.
- Formulas that read Mandelbulb3D maps (height maps, map translations) find them by number in
  Mandelbulb3D's `M3Maps` folder: Preferences › Maps, `RMB_MAPS_DIR`, or a `maps` folder next
  to the program.
- Deep zooms: the GPU renders in single precision up to a zoom of about 10,000 and switches to
  double precision (two single numbers per value) beyond, to about 1e11 (Ray marching ›
  Precision). Double precision is several times slower (formulas with sine and cosine about
  25 times) and its shader takes 10-20 s to compile the first time.
- Inside rendering (views from within the set) works like Mandelbulb3D's: rays step through the
  set in small steps, so it is slower.
- Glass (MB3D's transmission) follows one branch (refracted or reflected) per sample and
  needs several samples to converge; rays inside glass step through it in small steps.
- Imported scenes shade like MB3D (display space, depth fog over the view range, dynamic fog,
  MB3D's color index, depth of field); results are close, not pixel-identical.
- The GPU renders in f32, so very deep zooms need the CPU renderer (`rmb-cli render --cpu`).

## License

GNU LGPL 2.1 or later, like Mandelbulb3D (see `LICENSE` and `NOTICE`).
