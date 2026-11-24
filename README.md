# RMandelbulb3D

A 3D fractal explorer and renderer in Rust, after [Mandelbulb3D](https://github.com/thargor6/mb3d).
Distance-estimated ray marching on the GPU (wgpu compute) with a CPU reference renderer, a
modern egui interface and import of Mandelbulb3D parameters.

## Features

- **Hybrid formulas**: up to nine formula slots with iteration counts and "repeat from", Julia
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
  normal), glossy reflections and frosted glass, depth fog with light scattering (light beams
  through gaps, lit haze), glow, depth of field, cutting planes, filmic tone mapping, vignette.
- **Mesh and voxel export**: the surface as a closed triangle mesh (PLY with vertex colors, OBJ,
  STL) for 3D printing and 3D programs, or as voxels (PNG slices, MagicaVoxel `.vox`), sampled
  on the GPU (File › Export mesh… / Export voxels…, `rmb-cli mesh` / `voxels`).
- **Image formats**: PNG 8 or 16 bit with the scene inside; OpenEXR with the linear image and
  coverage, depth, albedo and normal passes for compositing. Large images (up to 32768 px)
  render in tiles.
- **Projections and stereo**: fisheye (full frame or dome-master circle, up to 360°),
  equirectangular 360° and 180° panoramas; stereo as red/cyan anaglyph, side by side or
  over / under (omnidirectional stereo in the panoramas, for VR video).
- **Navigation**:
  - Orbit around the point under the cursor; the wheel zooms toward the cursor.
  - Fly mode with WASD, at a speed that adapts to the distance from the surface; Space
    captures the mouse for free steering (full turns, no stop at the window border).
  - Double-click looks at a point and sets the focus there.
  - Low-resolution preview while moving.
  - Lens flares per light (star, anamorphic streak, ghosts, rings, lens dirt; presets),
    hidden when the fractal covers the light. Ghosts can be placed by hand and shaped by a
    bokeh image.
  - Ground grid (z = 0) with the colored x/y/z axes and an axis cross; click an axis to look
    along it.
  - Lights are drawn in the view with gizmos: move lights freely or along the x/y/z arrows
    (a dashed line shows the spot below them), drag aim points, the spot cone and the area
    size, and turn directional lights on a dial.
- **Files**:
  - Native `.rmb` (JSON).
  - Mandelbulb3D `.m3p` files, and MB3D text parameters pasted with Ctrl+V.
  - PNG export that embeds the scene, so opening the PNG restores it.
- **Scene composition** (Mandelbulber's boolean mode): add fractals (with DE combinations and
  interpolation like the main fractal) and shapes (plane, box, sphere, cylinder, capsule, cone,
  torus, hexagonal and triangular prism, water) to the scene, move, rotate, scale and stretch
  them with gizmos in the view (Move / Rotate / Scale in the toolbar), combine them by adding,
  cutting away or intersecting (also smoothly), each in its own colors. Repetition turns an
  object into a grid of copies (endless or a set number). The main fractal has a placement too.
  Ctrl+click selects the object under the cursor.
- **3D models**: OBJ, STL, PLY, glTF/GLB and FBX files (Blender exports all of them) as scene
  objects. A distance grid (64 to 256 points along the longest side) covers the space around
  them; near the surface the triangles themselves are used, so thin parts stay sharp at any
  distance. Baked grids are kept on disk (`~/.cache/rmandelbulb/models`, up to 2 GB;
  `RMB_MODEL_CACHE` sets another folder, `off` turns it off), so a model loads fast the next
  time. Each material gets a color, a color texture, a bump, specular and normal map and an
  emission color (glTF and FBX bring normal maps and emission along); images near the model file
  (for example in a `textures` folder) are assigned to the materials by name.
- **Texture layers** (up to four per material): images, or a solid color through a mask image,
  over the gradient colors with blend modes (multiply, screen, overlay, soft light, …), as bumps,
  normal maps, glow, shine (specular) or roughness. The image's alpha channel cuts the layer
  out. Triplanar, planar, spherical, cylindrical or orbit mapping, in object or world space;
  in animations a layer can drift (flowing lava, moving clouds).
- **Path tracing** (optional, Lighting › Global illumination): Monte Carlo global illumination
  with diffuse and reflection bounces; glowing colors light their surroundings, the ambient
  colors become sky light. Classic Mandelbulb3D-style shading stays the default.
- **Render window** (F12): a test render with everything the export applies; post processing
  changes show there at once.
- **Emissive colors**: gradient colors can shine by themselves, light their surroundings in
  their color and glow (MagicaVoxel-style emission). **Glow bands** lay thin glowing stripes
  over a gradient, independent of its colors; palette rotation and each band's own speed make
  them sweep through the fractal in animations. **Steam**: glowing haze around emissive parts,
  shaped by 3D noise and drifting in animations. **Shimmer** lets the emitted light flicker in
  patches like embers. A color map can shine too. **Bloom** and **streaks** add soft halos and
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
  - Motion blur (Movie › Motion blur): the exposure as a fraction of the frame time (0.5 is a
    film camera's 180° shutter); camera moves and animated settings blur, every sample at its
    own time within the exposure (CLI: `--shutter`).
  - Sound track (Movie › Sound): a sound file under the exported movie, with an offset; the
    movie keeps its length (CLI: `--audio`, `--audio-offset`, `--no-audio`).
- **CLI**: `rmb-cli render|animate|convert|info|formulas`.

## Manual

The user manual is in `docs/manual` (Markdown with screenshots and how-tos). The app shows it
in its help window (F1); `python3 tools/build_manual.py` builds the PDF version
(`docs/manual/build/RMandelbulb3D-Manual-<version>.pdf`; needs Python 3 with `markdown`,
Google Chrome or Chromium and poppler-utils).

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
