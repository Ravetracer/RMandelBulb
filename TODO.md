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
  - [x] Smoothing on load (TBoptions bit 19; no sample uses it)
- [x] Reflections (MB3D's reflection amount and bounces, weighted by the highlight colors)
- [x] DE combination "mix" (dIFS measured where the first chain ends; "Dodeca Torus mix")
  - [x] "genQuat helix mix" rendered very small: MB3D's ray step limiter was missing
        (`March::step_limit`; also removes dark speckles in many samples)
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
      white (dynamic fog on iteration 31; 45-58 counted steps × 0.045 per step by MB3D's own
      factors, so its fog saturates there too unless MB3D counts fewer steps),
      "Waste-and-tragedy - Mandalex" overexposed (strong point light; MB3D's screen-space AO
      probably darkens more). Checked 2026-11-25: step limiter, step sub DEstop and the fog's
      depth offset are imported now.
- [x] Missing formulas of the samples: Bulbox and Aexion C (built-in), Amazing Surf 2,
      GeneralQuat (by hand), Mandalex, _Transform, TilingIFS, ABoxVSShapes (translator)
- [x] Height map / image formulas (9; Mandelbulb3D maps with its B-spline sampling, CPU and GPU)
- [x] `_Cond*` formulas that switch the hybrid's formulas (5): `setcount` statement, counts
      reset per point
- [x] Check "material colors" and "Nothing left but bones" (both render; no reference renders)

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
  - [x] Compared with the lens-flare reference images (formerly `../_dev`, since removed) (presets via `flare_presets`): the haze
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

- [x] Formula editing like Mandelbulb3D and Mandelbulber 2 (gap analysis 2026-11-23):
  - [x] In-app formula editor (syntax colors, line numbers, errors with line, copy from any
        formula, apply into the slot, save to the formula folder); edited formulas recompile
  - [x] Scenes carry the code of their non-built-in formulas
  - [x] Mandelbulber per-slot settings: iteration range, weight, add C, escape test; minimum
        iterations and constant factor; up to 9 slots
  - [x] .m3f import from the app (Preferences › Formulas › Import .m3f…)
  - [x] More DE kinds: `de maxaxis`, `de custom` (the formula writes `dist`; no escape test),
        Kleinian formulas (Pseudo Kleinian, Jos Kleinian) with presets. Inside rendering
        (Mandelbulber's interior mode) is covered by the existing MB3D inside rendering
  - [ ] Possible later: nested boolean groups (Mandelbulber's object tree), per-slot
        settings and 4D rotation for object fractals, language helpers (functions, vectors,
        enum parameters)
- [x] Explore lane: own presets (thumbnails), random batches, mutations (like RWildFire)
  - [x] Quality filter (`explore::thumb_quality`: brightness change between half and all
        samples, relative to the contrast; above 0.22 the scene is made again, up to four
        tries; tuned with `rmb-gpu --example explore_quality`), Snapshots strip
  - [ ] Possible: more presets, snapshots kept across sessions
- [x] User manual: docs/manual (18 chapters, screenshots, how-tos with example scenes), shown
      in the app (Help › Manual), offline PDF (tools/build_manual.py)
- [x] Preferences like RWildFire: UI scale, font size, preview time limit, always denoise,
      free CPU cores, new scene size, folders per file kind, quick save, explore batch size
  - [x] The top bar no longer overlaps: icons only when the labels do not fit, the document
        name only where there is room
- [x] Underwater: see-through water medium (refraction, underwater camera, absorption and
      color filtering, depth profile), caustics, light rays, bubbles, particles, seabed
      object, waterline view, flat lens, lens drops
  - [x] Bubble sources: plumes from random vents and from objects marked as bubble sources
  - [x] Richer seabed: rocks, pebbles, swaying seaweed ribbons, "Sand, stones, seaweed"
        color pattern
  - [x] Plants grown by L-systems on the seabed: presets (kelp, sea fan, staghorn coral,
        algae tuft, fern weed) and an editor (start string, stochastic rules, turtle
        settings, preview)
  - [ ] Possible: plants on other objects than the seabed (on fractal surfaces), parametric
        L-systems
  - [x] Wet trails behind the lens drops running down (Drop trails)
  - [x] Faster seabed parts and plants (cell skipping, nearest-first scan, local sway
        bound): 1.6-1.7x on the GPU
  - [ ] Possible: plants are still ~70 % of a plant scene's time: level of detail (skip
        branches thinner than a pixel), fewer segments in the presets

## World: sky, weather, landscape (owner's wish 2026-11-27)

Complete scenes the fractals live in. Everything configurable in colors and amounts, so
alien planets are possible too. At most one water, one seabed, one landscape and one sky
per scene.

- [x] One water and one seabed per scene (the app offers no second one, no duplicate)
- [ ] Sky: an atmosphere around the scene (sun position, time of day, scattering colors
      from Earth-like to alien), as the background and the horizon behind the water
- [ ] Volumetric clouds in the sky: blue sky with fair-weather clouds, overcast, dark
      storm clouds; coverage, density, height, wind (moving in animations)
- [ ] Thunderstorms: lightning bolts and flashes that light the clouds and the scene
- [ ] The sky lights the scene like an HDRI box image (sky and clouds as ambient and
      reflection light, the sun as the main light), optional instead of the added lights
- [ ] Weather: rain (streaks, splashes on the water and the ground, wet surfaces, drops on
      the lens), snow (flakes, snow cover), mist, hail; amounts and colors configurable
- [ ] Landscape object: mountains and valleys (eroded-looking noise), materials by height
      and slope (rock, grass, sand, snow), meeting the water at the shore
- [ ] Vegetation on land: bushes, trees, grass and other plants grown by L-systems (as
      the underwater plants), placed by height, slope and patches

## Gaps to Mandelbulb3D and Mandelbulber 2 (analysis 2026-11-24)

Features the two programs have that RMandelbulb3D does not have yet, in the planned order.

- [x] Camera projections: fisheye (equidistant, full frame or circle, up to 360°),
      equirectangular 360° panorama, 180° half panorama (dome / VR video). Mandelbulber
      `projection_3d`
- [x] Stereo rendering: anaglyph (red/cyan, color or half color), side-by-side and over-under
      pairs, eye distance and convergence; omnidirectional stereo in the panoramas (eye
      distance fades out toward the poles). Mandelbulber `stereo`, MB3D stereo
  - [x] Lens flares per eye in stereo images (perspective; anaglyphs mix them red/cyan)
  - [ ] Possible: gizmos and guides in the other projections / per eye, VR video metadata in
        exported movies
- [x] Mesh export: distance field sampled on a grid (GPU `grid_main`, or CPU), surface nets
      (closed, caps on the box faces), vertex refinement, vertex colors, PLY/OBJ/STL; File ›
      Export mesh…, `rmb-cli mesh`. Mandelbulber `mesh_export`, MB3D BulbTracer2
  - [x] The box is shown in the view while the window is open; loose pieces below a share of
        the triangles are removed (Remove pieces under, `--min-piece`)
  - [x] Mesh decimation: quadric edge collapse (`mesh_simplify.rs`; Keep triangles,
        `--reduce`), non-manifold spots of the surface nets frozen, closed meshes stay closed;
        about 5 s per million triangles (heap traffic dominates)
  - [ ] Possible: sparse sampling (skip empty blocks; 2048³ grids), texture-baked colors
- [x] Voxel export: PNG slices per z layer (colors with transparency or white on black, hollow
      option) and MagicaVoxel `.vox` (≤256³, median-cut palette); File › Export voxels…,
      `rmb-cli voxels`. MB3D `VoxelExport`, Mandelbulber `voxel_export`
- [x] 16-bit PNG and EXR output (linear, denoised, with coverage, Z, albedo and normal passes;
      depth and coverage in the w components of the aux buffer). MB3D `zbuf16bit`, Mandelbulber
  - [x] EXR / 16-bit PNG frame sequences for animations (export window, `animate --exr /
        --16bit`, render queue, network renders: `JobSpec::frame_kind`, protocol 2)
  - [ ] Possible: an emission pass, larger MagicaVoxel scenes (several 256³ models)
- [x] Big renders in tiles (sizes beyond the GPU memory, for prints; up to 32768 px): tiles
      render the full image's pixels (identical without the denoiser), denoised per tile with a
      64 px overlap, post processing on the stitched image. Export window › Tiles,
      `rmb-cli render --tiles`. MB3D `Tiling`
  - [ ] Possible: tiled renders in the CPU path of `render`/queue (`CpuRenderer::set_tile`
        exists), writing PNG rows while tiles finish (less memory
        for 32K images), tiled animation frames
- [x] Batch rendering / render queue (several scenes and animations overnight): File › Render
      queue… (jobs from "Add to queue" in the export window with a scene snapshot, or scene
      files; saved in queue.json), `rmb-cli batch`. MB3D `BatchForm`, Mandelbulber
      `render_queue`
  - [ ] Possible: run queue jobs on the network clients, mesh/voxel jobs in the queue
- [x] Script expressions: animation drivers (path + expression of t, frame, u, value, @other
      values; `expr.rs`, `animation::Driver`), timeline › DRIVERS with a value picker.
      Mandelbulber `scripts`
  - [x] Audio-reactive variables: audio, bass, mid, treble (levels 0..1 from the sound track,
        analysed once with ffmpeg and stored in the animation; `audio.rs`)
  - [x] Curve view of each driver's value over the animation (timeline)
  - [x] Drivers on stills (DRIVERS › Also in stills, `Animation::still_drivers`,
        `animation::still_scene`, applied in the renderers, image saving and mesh/voxel export)
- [x] Kleinian DE and inside rendering (see Formula editing above)
- [x] Compared with MB3D's own Monte Carlo renderer (`mb3d/CalcMonteCarlo.pas`, 2026-11-24):
      it branches at every hit (direct light, mirror, refraction and one cosine ambient ray),
      caps ambient rays at the AO length (capped = open sky), uses light maps as image-based
      light, Halton sampling, variance-adaptive passes, 6 bokeh shapes. Taken over: Sky reach
      (Lighting › Global illumination), aperture blades (Depth of field › Blades), import of the
      ambient depth as path bounces (+ notes; every file carries MC defaults, so no switch to
      path tracing)
  - [x] Low-discrepancy sampling (like MB3D's Halton): the first six random numbers of every
        sample (pixel jitter, lens, first directions) follow the R2 sequence over the sample
        index with a random offset per pixel (`Rng::next_f64`, march.wgsl `rnd`). Against a
        common 4096-sample reference (320 x 180, no denoiser) the error drops by 34 % / 56 %
        (Mandelbulb, 16 / 64 samples) and 29 % / 44 % (Amazing Box); flicker of a 12-frame
        dolly at 16 samples 1.17 -> 0.80 levels of 255. About the quality of 2-4x the samples
  - [x] Shadows tinted through glass (Material › Light through glass: transparency × glass
        amount, absorption by the surface color along the straight path; up to four crossings;
        GPU switch `GLASS_SHADOW`)
  - [ ] Possible: light maps as image-based light (needs the light map import)
- [x] Network rendering for long, high-quality animations (owner's wish 2026-11-24). Easy to
      set up, no configuration files (done: crate `rmb-net`, `rmb-cli --network`,
      `net-render`, `net-clients`, File › Network render…):
  - In the CLI: render clients just run `rmb-cli` with one option (e.g. `rmb-cli --network`)
  - The server starts a job from the CLI (faster) or from the GUI
  - Automatic discovery: clients announce themselves on the local network (broadcast /
    multicast), the server finds them without entering addresses
  - The server shows the number of active clients; render on all of them or on selected ones
  - Clients render frames and send them back; the server stores them. A client deletes its
    copy only after the transfer to the server succeeded
  - Fair load balancing: slower machines get less work than faster ones. Clients send hardware
    specs only (CPU, RAM, GPU; no user data); the measured frame times refine the share
  - CLI shows progress bars for the current image and the whole job
  - Mandelbulber has a netrender (reference only)
  - Also done: stills split by sample ranges (identical to one computer's render), tail
    duplication of the slowest frame, reconnects, files shipped by hash, version check,
    optional shared key, "this computer too"
  - [x] Tiled stills over the network (`Unit::Tile`, `JobSpec::tiles`, protocol 3): clients
        render tile regions (GPU `TileRenderer`, CPU `CpuRenderer::set_tile`), the server
        denoises each with its margin and stitches (identical to a local tiled render)
  - [ ] Possible: subnet broadcast addresses per network interface (macOS/Windows may need them),
        resuming a job after the server restarts
- Low value (covered by raytraced AO and bloom): SSAO, HDR blur post effects

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
  - [x] ~~Temporal denoising~~ (tried, dropped): each frame blended with the previous one,
        reprojected through the depth pass and both cameras (rejected where depth or normal
        differ, clamped to the 3 x 3 neighbourhood), before or after OIDN. On a 12-frame dolly
        of the Mandelbulb (640 x 360, 16 samples; `examples/flicker_bench`, levels of 255
        against 512 samples) it was always worse: denoised alone error 0.78 / flicker 1.14,
        blended 0.5 with bilinear history 1.15 / 1.61, after OIDN 1.18 / 1.66, Catmull-Rom
        history 1.03 / 1.44 (64 raw samples: 0.40 / 0.58). Every pixel of a fractal is fine
        detail, so the resampled history blurs more than it removes noise; in moving shots the
        flicker comes mostly from the noise itself (raw 1.17 vs denoised 1.14), which only more
        samples lower
  - [ ] Possible: sample defaults per output kind (stills vs. animations)
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
- [x] CPU renderer JIT: `formula/jit.rs` compiles each formula's `iter` with Cranelift
      (registers in SSA variables, libm helpers for transcendentals; bit-identical to the
      closures, which stay as fallback and behind `RMB_NO_JIT=1`). All 512 library formulas
      compile in 0.24 s together (worst 12 ms), only on first use. `rmb-cli render --cpu
      -W 320 -H 180 -t 8`, closures → JIT: presets (32 samples) mandelbulb 7.7 → 5.5 s
      (1.4x, libm-bound), amazing-box 18.8 → 4.4 s (4.3x), hybrid 13.7 → 3.5 s (3.9x),
      kifs 4.6 → 1.1 s, menger 1.9 → 0.8 s, quaternion-julia 3.9 → 1.3 s,
      pseudo-kleinian 17.8 → 3.9 s (4.6x); MB3D imports (4 samples) Aexion 10bulbs
      25.2 → 2.5 s (10x), Torii temple 181 → 32 s (5.6x), ABoxScale2Start 4.4 → 1.1 s (4x),
      ApolloBalloons dIFS 515 → 54 s (9.5x). Load/store per register access instead of
      SSA variables was 5-50 % slower.
  - [ ] Possible: SIMD ray packets (AVX2, 4 × f64 via `core::arch`; needs a vectorized
        formula variant, masks for branches and vector libm); compiling the whole hybrid
        iteration loop (state copies, slot switching) into one function; native
        polynomial sin/cos/atan2 would help the bulbs but break bit-identity with the closures
- [x] ~~Split rows between CPU and GPU~~ (measured, dropped): the CPU renderer was 700-1900x
      slower than the GPU per pass (8 threads, MB3D test scenes), so it would add 0.1-0.3 %.
      With the JIT (4-10x on MB3D scenes) it is still about 100-400x slower: below 1 %, still
      not worth it

## Windows and macOS

- [x] Windows and macOS versions of the app and the CLI: platform folders (`rmb_core::paths`),
      OIDN in the packages (`lib/`, AppImage `usr/lib`, macOS `Contents/Frameworks`), ffmpeg
      next to the program or in Homebrew/MacPorts folders, network announcements to every
      subnet's broadcast address, Windows program icon, version 1.0.0 (`rmb-cli --version`)
- [x] GitHub Actions: `release.yml` (tag vX.Y.Z → packages for Linux tar.gz + AppImage,
      Windows zip, macOS arm64 + x86_64 zips, GitHub release with the changelog section),
      `ci.yml` (clippy, tests without GPU); `tools/release.py`, `tools/package.py`
- [x] Unsigned builds: INSTALL.txt per package (macOS: `xattr -dr com.apple.quarantine`,
      ad hoc signature; Windows: SmartScreen; Linux: needed libraries)
- [ ] Not tested on real Windows and macOS machines yet (only built on CI)
