# TODO

## Mandelbulb3D import fidelity

- [x] Shading in display space like MB3D (no linear workflow, no tone mapping)
- [x] Depth fog with MB3D's depth curve, fog color from the vertical background gradient
- [x] Dynamic fog (glow) with offset, second color and blend option
- [x] Ambient light relative to the camera, diffuse/specular multipliers
- [x] Color index like MB3D (smooth iterations from Rout/Rold, start/end sliders, color cycling)
- [x] Depth of field import (focus distance, aperture)
- [x] Interpolation hybrids (two formulas blended per iteration)
- [x] Background images (panorama with MB3D's rotation and B-spline sampling)
  - [x] Screen-coordinate images, "convert to spherical" and border fitting on load, ambient
        light from the image, image brightness, fog added on top
  - [ ] Smoothing on load (TBoptions bit 19; no sample uses it)
- [x] Reflections (MB3D's reflection amount and bounces, weighted by the highlight colors)
- [x] DE combination "mix" (dIFS measured where the first chain ends; "Dodeca Torus mix")
  - [ ] Unverified: "genQuat helix mix" renders very small
- [x] Transparency / transmission (MB3D's glass: refraction, Fresnel, absorption, scattering;
      "toricaleggs", "Torii")
- [x] Positional lights (point lights with 1/d² falloff)
- [x] Specular colors per gradient stop, MB3D's diffuse light functions, lights darkened by AO
- [x] Dynamic fog on one iteration, AO strength above 1, AO reach like MB3D
- [x] Visible light sources (glow shapes soft, sharp, ball, halo; occlusion and depth fog)
- [x] Smoothed normals and roughness (dull highlights, softer diffuse on rough parts)
- [x] Color variation with depth (VarColZpos), interior colors (cut faces, camera inside)
- [x] Color maps on the surface (orbit trap × iterations, normals, 3D waves, 3D projection;
      4 of the 7 maps the samples use are not in this MB3D copy)
- [ ] Unverified (no reference render): "KentPress - Parameter 3 tweak fogits" renders almost
      white (dynamic fog on iteration 31), "Waste-and-tragedy - Mandalex" overexposed (strong
      point light; MB3D's screen-space AO probably darkens more)
- [x] Missing formulas of the samples: Bulbox and Aexion C (built-in), Amazing Surf 2,
      GeneralQuat (by hand), Mandalex, _Transform, TilingIFS, ABoxVSShapes (translator)
- [x] Height map / image formulas (9; Mandelbulb3D maps with its B-spline sampling, CPU and GPU)
- [ ] `_Cond*` formulas that switch the hybrid's formulas (5)
- [ ] Check "material colors" and "Nothing left but bones" (render empty, no reference renders)

## Features

- [x] Animation rendering into single images or movie formats (PNG, MP4, ProRes, WebM, GIF)
- [x] Camera tracking with splines (centripetal Catmull-Rom, quaternion squad) and tweening
      between key frames; flight recording with automatic keyframes
- [x] Smoothing of camera paths so every path looks professional, without shaking or
      unnatural movement (steady speed, smoothed recorded speed, "cameraman" pass)
  - [x] Optional: automatic, or as a correction after defining the path over the key frames
  - [ ] Motion blur, audio track
- [x] More light sources: spot, point, area, line (up to eight lights; area and line lights
      sampled per pass for soft shadows, visible as panel and tube)
- [x] Lens flares for light sources with extensive options (glow and haze, diffraction star and
      spikes, anamorphic streak with sparkles, aperture ghosts with coating tints and color
      fringes, rainbow ring, lens dirt; presets; fade when hidden, in fog or off screen)
  - [ ] Compare with the reference images in `../_dev` once more after use; possible
        additions: light rays through fog (god rays), bokeh-textured ghosts, custom ghost layouts
- [x] Glowing palette colors that produce a shimmering/glow effect (glow collected along the
      rays near emissive parts)
- [x] Palette colors that act as light sources in that color (emission per gradient color,
      light cast by a hemisphere ray per sample, reach, glow and glow size)
  - [x] Like emission in MagicaVoxel:
        https://www.megavoxels.com/learn/how-to-create-lights-in-magicavoxel-using-emission/
  - [x] Glow bands (up to 4 per material, exact width), palette rotation and band speed in
        animations, several rays for the emitted light, bloom (preview and export)
  - [x] Streaks (directional bloom: 1-4 directions, length, angle, tint) and steam (glowing
        haze shaped by 3D noise, lit by the emissive surfaces through one scattered ray per
        sample, drifting in animations)
  - [ ] Possible: animated shimmer (flicker over time), emission from color maps
- [x] Scene composition like Mandelbulber's boolean mode (as used by Julius Horsthuis): up to
      eight objects after the main fractal (fractals or plane, box, sphere, cylinder, torus,
      water), each with position, rotation, scale, operation (add, cut away, intersect, smooth
      variants) and its own material (gradient with transparency/emission, patterns for shapes)
  - [ ] Possible: transform of the main fractal, repetition (infinite grids of an object),
        more primitives (capsule, cone, prism), object fractals with DE combination
- [x] Loading 3D objects into the scene: OBJ (+MTL), STL, PLY, glTF/GLB as composition objects,
      baked into a signed distance grid (16-bit, up to 256 points per side) with exact triangle
      distances near the surface; per-material colors, color textures, bump and specular maps
      (images near the model are assigned by name); move, rotate, scale and stretch in the view
  - [ ] Possible: FBX import, a disk cache of baked models, normal maps of glTF models,
        emissive materials (glTF emissive factor), faster near-surface search (models make
        renders about 2x slower than the grid alone)
- [x] Texture layers instead of colors, or as masks over the colors with layer blend modes
      (color, bump and glow layers; triplanar, planar, spherical, cylindrical, orbit mapping)
  - [ ] Possible: alpha channel masks, normal maps, roughness/specular layers, texture
        animation (offset over time)
- [x] AI denoiser (Intel Open Image Denoise, from RWildFire) with albedo and normal helper images
- [ ] Monte Carlo rendering (path tracing) as an option of the renderer, in the app (progressive
      preview) and in the export (images and movies), for more realistic light: global
      illumination with diffuse and glossy bounces (light from all surfaces, not only the
      emissive ones), importance sampling (cosine / BRDF), next-event estimation toward the
      lights, Russian roulette to end paths, progressive accumulation with the existing sample
      loop and the denoiser. The renderer already samples stochastically (AO and emission rays,
      soft shadows, area lights, DOF); this extends it to full multi-bounce paths.
      References: scratchapixel.com "Monte Carlo Methods in Practice: Monte Carlo Rendering
      Practical Example"
      (https://www.scratchapixel.com/lessons/mathematics-physics-for-computer-graphics/monte-carlo-methods-in-practice/monte-carlo-rendering-practical-example.html),
      Ghete, "Monte Carlo Rendering" (TU Wien seminar 2007,
      https://www.cg.tuwien.ac.at/courses/Seminar/SS2007/MontecarloRendering-Ghete.pdf)

## Performance

A 1920 × 1080 GPU render (64 samples, the CLI default) takes about 2 minutes on the RTX A2000
Laptop GPU (about 1.9 s per pass). Every pass marches the primary ray (up to 400 steps), normal,
raytraced AO, shadows and area lights from scratch. Ordered by expected gain:

- [ ] Profile first: split the pass time into primary march, normal, AO, shadows, area lights
      and emission (e.g. timestamp queries or feature toggles on a test scene) before optimizing
- [ ] Fewer samples plus OIDN (e.g. 16 instead of 64, about 4×); check frame-to-frame flicker
      of the per-frame denoiser in animations
- [ ] Adaptive sampling: stop sampling pixels whose variance has converged (typically 2-3×)
- [ ] Hit-distance caching: start the primary ray of later passes at the previous hit distance
      minus a margin (minimum over neighbor pixels to stay safe at silhouettes and with jitter;
      not with DOF); saves most of the primary march
- [ ] Final-render shader specialization: iteration counts and feature flags as constants
      (maybe 10-30%, costs a recompile per render; keep the buffer path for the preview)
- [ ] Animation pipelining: denoise and PNG/ffmpeg encoding of frame N on the CPU while the GPU
      renders frame N+1 (check whether `animate` / export already overlap)
- [ ] Optional "deep zoom" mode: f32 breaks down at roughly 1e5-1e6 zoom. Consumer GPUs run
      FP64 at 1/64 rate (wgpu `SHADER_F64` not worth it); use double-float emulation (df64, two
      f32 per value) for the position and the first iterations instead
- [ ] CPU renderer: replace the `Box<dyn Fn>` closure tree in `formula/cpu.rs` with a JIT
      (Cranelift) that compiles `.mbf` formulas to native code, plus SIMD ray packets
      (AVX2, 4 × f64 via `core::arch`); plausibly 5-20× on the CPU. Hand-written assembly is
      not worth it (478 formulas; LLVM matches it)
- [ ] Low priority: split rows between CPU and GPU (parity exists); expected gain only 5-10%
