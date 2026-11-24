# Changelog

All notable changes of RMandelbulb3D. Versions follow `MAJOR.FEATURE.FIX`:

- **MAJOR**: big changes, for example scene files or the network protocol that older versions
  cannot read any more.
- **FEATURE**: new features.
- **FIX**: bug fixes and small improvements of existing functions.

Every release has a section `## [x.y.z] - YYYY-MM-DD` here; `tools/release.py` checks for it
and the release workflow uses it as the release notes.

## [Unreleased]

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
