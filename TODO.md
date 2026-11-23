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
  - [x] Audio track: a sound file under exported movies (AAC/Opus/PCM by format), with an
        offset; cut or padded with silence to the movie's length
  - [x] Motion blur: shutter as a fraction of the frame time; each pass renders a slice of the
        exposure (settings at its middle), every sample a random camera time within the slice
        (no ghost copies in fast moves)
- [x] More light sources: spot, point, area, line (up to eight lights; area and line lights
      sampled per pass for soft shadows, visible as panel and tube)
- [x] Lens flares for light sources with extensive options (glow and haze, diffraction star and
      spikes, anamorphic streak with sparkles, aperture ghosts with coating tints and color
      fringes, rainbow ring, lens dirt; presets; fade when hidden, in fog or off screen)
  - [x] Compared with the reference images in `../_dev` (presets via `flare_presets`): the haze
        now fades out within about its size (it covered the whole image), about a third of the
        generated ghosts are large, faint and soft. Hand-placed ghost layouts (Ghosts › Place
        ghosts by hand) and bokeh images that shape the ghosts; god rays come from light
        scattering in fog
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
  - [x] Shimmer (emitted light flickers in noise patches; new patterns per second in
        animations), emission from color maps (lights only itself)
- [x] Scene composition like Mandelbulber's boolean mode (as used by Julius Horsthuis): up to
      eight objects after the main fractal (fractals or plane, box, sphere, cylinder, torus,
      water), each with position, rotation, scale, operation (add, cut away, intersect, smooth
      variants) and its own material (gradient with transparency/emission, patterns for shapes)
  - [x] Placement of the main fractal (position, rotation, scale, stretch; deep zoom folds it
        into the reference point), repetition of objects (spacing per own axis, endless or a
        number of copies on each side), capsule, cone, hexagonal and triangular prism, object
        fractals with DE combination (incl. dIFS mix) and interpolation hybrids
  - [ ] Possible: 4D rotation for object fractals, gizmos for the main fractal's placement
- [x] Loading 3D objects into the scene: OBJ (+MTL), STL, PLY, glTF/GLB as composition objects,
      baked into a signed distance grid (16-bit, up to 256 points per side) with exact triangle
      distances near the surface; per-material colors, color textures, bump and specular maps
      (images near the model are assigned by name); move, rotate, scale and stretch in the view
  - [x] FBX import (ufbx: binary and ASCII), a disk cache of the baked distance grids, normal
        maps (glTF, FBX, or assigned per material), emissive materials (glTF/FBX emission,
        OBJ `Ke`, or set per material)
  - [ ] Possible: faster near-surface search (models make renders about 2x slower than the grid
        alone)
- [x] Texture layers instead of colors, or as masks over the colors with layer blend modes
      (color, bump and glow layers; triplanar, planar, spherical, cylindrical, orbit mapping)
  - [x] Image alpha as opacity, normal map layers (tangent frame from the mapping's u/v
        directions), specular and roughness layers, drift (texture offset per second in
        animations)
- [x] AI denoiser (Intel Open Image Denoise, from RWildFire) with albedo and normal helper images
- [x] Monte Carlo rendering (path tracing) as an option of the renderer, in the app (progressive
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
  - Done: Lighting › Global illumination › Path tracing (optional; classic stays the default):
    diffuse and reflection/glass bounces, next-event estimation through the existing light
    loop, ambient colors as sky light, Russian roulette, firefly clamp.
  - [x] Light scattering in fog (Fog › Light scattering, classic and path tracing): one point
        per ray segment and one random light with a shadow ray; equiangular sampling around
        positional lights, fog-weighted for directional ones; Henyey-Greenstein phase. Light
        beams through gaps (god rays) and lit haze. Costs one shadow march per sample
  - [x] Glossy reflections and frosted glass (Material › Gloss: GGX micro facets in the
        bounce, classic and path tracing)
  - [x] ~~Multiple importance sampling for emissive surfaces~~ (not applicable): the emitters
        are fractal surfaces without a list of their geometry, so they cannot be sampled
        directly; only BSDF sampling finds them (next-event estimation covers the lights)
  - [x] ~~Adaptive sampling~~: measured and dropped, see Performance

- [x] Random palette generator (harmonies incl. "dark + neon", number of colors, hue,
      saturation and lightness ranges, order, hardness, seamless, emissive share; OKLCh colors,
      `palette.rs`) and a better palette editor: draggable stops on the strip (click adds,
      right-click removes), saved palettes, tools (reverse, shift, mirror, repeat, space evenly,
      sort, invert, hard steps / smooth), adjustments (hue, saturation, lightness, contrast,
      hardness)
- [x] Volumetric haze shaped by the fractal (Fog › Fractal haze): glows by itself and hides what
      lies behind it, one color or the gradient's colors by the iteration count. Shapes:
      Distance (falls off from the surfaces, hugs every structure; default) and Iterations
      (Mandelbulber's iteration fog with a from/to share of the maximum iterations). Iteration
      fog alone depends a lot on the formula: on a Mandelbulb the high counts only occur in a
      thin shell at the surface, on scenes with large regions that never escape it covers
      everything
- [x] Post effects (Image): chromatic aberration, white balance (temperature, tint), split
      toning (shadow and highlight colors)

## Performance

A 1920 × 1080 GPU render (64 samples, the CLI default) took about 2 minutes on the RTX A2000
Laptop GPU (about 1.9 s per pass). Every pass marches the primary ray (up to 400 steps), normal,
raytraced AO, shadows and area lights from scratch. After the items below, passes of the MB3D
test scenes are 1.8-4.3x faster than at the start (640 x 360: Hydra 727 -> 169 ms,
Curvichrome 1052 -> 254 ms, AboxShapes-Misty 44 -> 17 ms, temple 71 -> 35 ms, Housing problem
1697 -> 949 ms).

- [x] Profile first (`rmb-gpu` example `bench`: feature toggles per scene). Points at the
      surface run the full iteration count, so the evaluations there dominate: the binary hit
      refinement cost 20-60 % of a pass (up to 15 evaluations), raytraced AO 35-70 % (its hit
      refinement included; random directions also make whole warps wait for the longest ray),
      shadows 3-12 %
  - [x] Refinement stops at steps below threshold/128 (`refine_steps` is the maximum);
        AO, emission and scatter rays skip it. Same image (mean difference below 1/255 at 64
        samples); passes 1.15-2.6x faster (MB3D test scenes at 640 x 360)
- [x] Fewer samples plus OIDN, measured on two 12-frame dollies (640 x 360, error and flicker
      against 512 samples, gray levels): stills at 16-32 samples + OIDN are cleaner than 64 raw
      (2-4x faster). In animations the per-frame denoiser shimmers on fine detail: flicker 2x
      (16 samples) and 1.4x (32) of 64 raw; 64 + OIDN equals 64 raw. Raw noise does not
      flicker since the random seeds do not depend on the frame. Recommendation in README
  - [ ] Possible: temporal denoising (OIDN has none; e.g. blend with the reprojected previous
        frame), sample defaults per output kind (stills vs. animations)
- [x] ~~Adaptive sampling~~ (estimated, dropped): stopping pixels whose noise (standard error
      of the luminance) is below 0.2-1 % after at least 8 of 32 samples. A GPU warp (8 x 4
      pixels) only saves time when all its pixels stop: that leaves 99-100 % of the work on
      AboxShapes-Misty and 75 % on the temple, where the savings are mostly sky (cheap misses).
      Fractal surfaces stay noisy (AO, soft shadows) until the end
- [x] ~~Hit-distance caching~~ (tried, dropped): starting the camera ray of later passes
      shortly before the hits of the first pass saves only 10-20 % (the skipped steps far from
      the surface bail out after few iterations). With inexact distance estimates (common in
      MB3D scenes, step factor 0.2-0.5) the hit depends on the step sequence from the camera:
      thin parts that rays only hit at some step phases disappear, and overshoots into the
      surface land elsewhere (shadowed vs. lit). Keeping that would need the arrival step and a
      random phase per pixel; not worth the state and the risk
- [x] Formula parameters and composed objects in uniform instead of storage buffers: passes
      1.3-1.8x faster (same image)
- [x] Final-render shader specialization: renders without a window (CLI, export, render
      window) write the formula parameters into their shader as constants, animations the ones
      that stay the same in all frames (`GpuRenderer::set_baking`). They compile their own
      shader anyway; another 8-38 % (Hydra 173 -> 108 ms, AboxShapes-Misty 16 -> 14 ms)
  - [x] Uniform settings as constants too (feature switches, iteration counts, light count,
        colors; `codegen::bake_uniforms`): switched-off features drop out of the shader.
        Passes another 1-28 % faster (640 x 360: Curvichrome 282 -> 204 ms, Housing problem
        910 -> 733 ms, AboxShapes-Misty 19.3 -> 14.5 ms, Hydra and temple within noise), and
        the shaders compile 2-5x faster (Housing problem 7.9 -> 1.5 s). Animations bake the
        settings that stay the same in all frames
  - [x] The preview switches to such a shader when the view and settings rest for 0.7 s
        (compiled in the background, continues the accumulation; Preferences › Optimize at
        rest)
- [x] Animation pipelining: encoding and writing of frame N on a second thread while frame N+1
      renders (PNG sequences about 17 % faster at 1280 x 720, 16 samples; movies unchanged, as
      ffmpeg already encodes in its own process). The denoiser stays on the render thread.
      Renders without a window also use 50 ms submissions instead of 12 ms (0-5 %)
- [x] Deep zoom with double-float precision (df64, two f32 per value, ~48 bit): f32 gets grainy
      at zoom ~3e4 (default bulb). Precision setting Auto (switches at ~5e3) / Single / Double.
      The GPU renders relative to the camera, only the main fractal iterates in df64 (until the
      derivative makes f32 rounding negligible, then f32). Same image as the CPU (f64) at 1e7.
      Costs (640 x 360): Mandelbulb 17.5 -> 430 ms per pass (trig in df64), ABox 37 -> 90 ms;
      the shader compiles 10-18 s instead of ~1 s (once per mode, the driver caches it).
      Both improved since, see below
  - [x] Faster df64 functions (`examples/df64_bench` times each one): sin/cos, exp and the
        series behind atan2/acos/pow keep only the leading terms in double-float, the small
        rest in f32 (same accuracy, `tests/df64.rs`). sincos 2.2x, atan2 1.8x, exp/log 2.5x,
        non-integer pow 2.8x faster; deep zoom passes (zoom ~1e6, 640 x 360) Mandelbulb
        136 -> 62 ms, power 7.5 430 -> 180 ms; compiles about 2x faster. Measured and
        dropped: the sin/cos table in a uniform buffer (the table read costs nothing), one
        shared `dd_sincos` for sin and cos of the same angle (the driver already merges them),
        an f32 quotient in atan2's Newton step (3x less accurate), switching to f32 earlier
        (noisier surface, slower)
  - [ ] Possible: fewer inlined copies of the fractal (compile time), composed fractal objects
        in df64, zooms beyond ~1e11 (needs perturbation or more precision)
- [ ] CPU renderer: replace the `Box<dyn Fn>` closure tree in `formula/cpu.rs` with a JIT
      (Cranelift) that compiles `.mbf` formulas to native code, plus SIMD ray packets
      (AVX2, 4 × f64 via `core::arch`); plausibly 5-20× on the CPU. Hand-written assembly is
      not worth it (478 formulas; LLVM matches it)
- [x] ~~Split rows between CPU and GPU~~ (measured, dropped): the CPU renderer is 700-1900x
      slower than the GPU per pass (8 threads, MB3D test scenes), so it would add 0.1-0.3 %.
      Only worth another look after a CPU JIT
