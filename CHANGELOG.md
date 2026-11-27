# Changelog

All notable changes of RMandelbulb3D. Versions follow `MAJOR.FEATURE.FIX`:

- **MAJOR**: big changes, for example scene files or the network protocol that older versions
  cannot read any more.
- **FEATURE**: new features.
- **FIX**: bug fixes and small improvements of existing functions.

Every release has a section `## [x.y.z] - YYYY-MM-DD` here; `tools/release.py` checks for it
and the release workflow uses it as the release notes.

## [Unreleased]

## [1.2.1] - 2026-11-27

- Water: the camera flies into see-through water like into air. The surface no longer slows
  the flight down, and under the water navigation keeps working (no "camera inside the
  fractal" message, no switch to double precision).
- Water: lens drops appear when the camera comes out of the water and dry up within the new
  **Drying time** (shrinking and vanishing one by one); spray near a shore leaves a few.
  **Wet lens** sets them for still images.

## [1.2.0] - 2026-11-26

- Water: new Underwater settings. See-through water bends rays into it (refraction) and the
  camera can dive below the surface; light and sight fade into the water's color with
  distance and depth, red first (filter color, visibility, murkiness, depth darkening). From
  below, the sky shows through Snell's window and the surface mirrors the depths beyond it.
  Water in older scenes stays opaque.
- Water: caustics from the real waves on everything under the water, and light rays (god
  rays) in the water with shafts from the waves and the shadows of objects (settings
  Caustics, Caustic sharpness, Light rays).
- Water: rising air bubbles and floating particles in the water; with the camera at the
  surface the picture splits into the views above and below the water (Lens size).
- Water: flat camera lens (the view under the water magnifies, like underwater cameras) and
  water drops on the lens above the water that run down in animations, soft and out of focus
  or sharp like rain on a window (Drop focus).
- New object Seabed: an ocean floor with rolling hills and sand ripples (goes below the
  water when the scene has one).
- Ray marching: new option "Step minus detail" (Mandelbulb3D's "step sub DEstop"), imported.
- Glow: new setting Offset per distance (Mandelbulb3D's fog offset Z), imported; several
  Mandelbulb3D scenes get their depth glow back.
- Ray marching: new setting Step limit (Mandelbulb3D's ray step limiter), set by Mandelbulb3D
  imports. Fixes "genQuat helix mix" rendering tiny and removes dark speckles in many
  imported scenes; the imported samples also render faster.
- PNG files keep the scene when its texts have characters outside Latin-1 (several
  Mandelbulb3D imports could not be saved as PNG).
- Fixed the GPU shader of scenes with a very large bailout and color on iteration ("spineJulia").
- Mandelbulb3D's `_Cond…` formulas (`_CondItcountR`, `_CondItcountSwitch`, `_CondItcountSwX2`,
  `_CondLyapunov`, `_CondLyapunovEx`), which change the iteration counts of other hybrid slots,
  are available and imported. The formula language has a new statement `setcount(slot, count)`.
- Deep zoom (double precision) with an interpolation hybrid no longer fails to compile its
  shader.
- Background images: new option Smooth (softens the image on load); Mandelbulb3D parameters
  with "smooth background image" now import it.

## [1.1.1] - 2026-11-24

- Water spray: drops fly in arcs, sideways with a direction shared by nearby drops; new
  settings Drop size and Drop size variation.

## [1.1.0] - 2026-11-24

- Water: no more even, repeating pattern in wide views. Waves from long swells down to
  small ripples with random phases, wave speeds like deep water, slowly bending crests and
  wave groups; rarer, higher foam; deeper default color.
- Water breaks on the other objects of the scene: shore waves run in toward fractals,
  primitives and models, steepen and collapse into foam, a foam belt lines the coast and
  drops splash up with every wave (settings Shore waves and Spray; objects just below the
  surface act as shallows).

## [1.0.0] - 2026-11-24

First release.

- 3D fractal explorer and renderer after Mandelbulb3D: GPU ray marching (wgpu, Vulkan /
  DirectX 12 / Metal) with a CPU reference renderer (formulas compiled to native code).
- Hybrid formulas with up to nine slots, a formula language and editor, 478 translated
  Mandelbulb3D formulas, Kleinian and custom distance estimates; import of Mandelbulb3D
  parameters (`.m3p`, clipboard text) and formulas (`.m3f`).
- Scene composition with primitives, object fractals and 3D models (OBJ, STL, PLY, glTF,
  FBX), texture layers, glass, glossy reflections, path tracing, fog with light scattering,
  glow, lens flares, bloom, depth of field with aperture blades.
- Camera projections (fisheye, 360° and 180° panoramas) and stereo (anaglyph, side by side,
  over / under).
- Animation: keyframes, recorded flights, cameraman smoothing, motion blur, sound tracks,
  drivers (expressions, audio-reactive, also for stills).
- Output: PNG 8/16 bit, OpenEXR with passes, tiled renders up to 32768 px, movies through
  ffmpeg, mesh export (PLY, OBJ, STL, with decimation), voxel export (PNG slices,
  MagicaVoxel).
- Render queue, network rendering with automatic discovery and fair load balancing (frames,
  sample ranges or tiles of stills), Intel Open Image Denoise.
- Water as an ocean surface: waves around the wind direction with sharp crests, Fresnel
  reflection of the sky and the scene, sun glints, foam; the waves move in animations.
- Explore lane with presets, random scenes (quality filter), mutations and snapshots.
- User manual in the program and as PDF.
- Packages for Linux (tar.gz, AppImage), Windows (zip) and macOS (arm64, x86_64; unsigned).
