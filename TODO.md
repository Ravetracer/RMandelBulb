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
  - [ ] Screen-coordinate images, MB3D's "convert to spherical" on load, ambient light from the
        image (samples: material colors, Mengerplus for MC)
- [x] Reflections (MB3D's reflection amount and bounces, weighted by the highlight colors)
- [ ] Transparency / transmission (MB3D's glass: "material colors", "toricaleggs", "Torii")
- [x] Positional lights (point lights with 1/d² falloff)
- [x] Specular colors per gradient stop, MB3D's diffuse light functions, lights darkened by AO
- [x] Dynamic fog on one iteration, AO strength above 1, AO reach like MB3D
- [x] Visible light sources (glow shapes soft, sharp, ball, halo; occlusion and depth fog)
- [x] Smoothed normals and roughness (dull highlights, softer diffuse on rough parts)
- [x] Color variation with depth (VarColZpos), interior colors (cut faces, camera inside)
- [ ] Color maps on the surface (diffuse color from a map: on normals, 3D wrap, iterations/trap;
      8 samples)
- [ ] Unverified (no reference render): "KentPress - Parameter 3 tweak fogits" renders almost
      white (dynamic fog on iteration 31), "Waste-and-tragedy - Mandalex" overexposed (strong
      point light; MB3D's screen-space AO probably darkens more)
- [x] Missing formulas of the samples: Bulbox and Aexion C (built-in), Amazing Surf 2,
      GeneralQuat (by hand), Mandalex, _Transform, TilingIFS, ABoxVSShapes (translator)
- [x] Height map / image formulas (9; Mandelbulb3D maps with its B-spline sampling, CPU and GPU)
- [ ] `_Cond*` formulas that switch the hybrid's formulas (5)
- [ ] Check "material colors" and "Nothing left but bones" (render empty, no reference renders)

## Features

- [ ] Animation rendering into single images or movie formats
- [ ] Camera tracking with Bezier splines and tweening between key frames
- [ ] Smoothing of camera paths so every path looks professional, without shaking or
      unnatural movement
  - [ ] Optional: automatic, or as a correction after defining the path over the key frames
- [ ] More light sources: spot, point, area, line, …
- [ ] Lens flares for light sources with extensive options (lens reflections, shape, color,
      size, main flare shape such as anamorphic or standard, …)
  - [ ] Must look professional, not like a cheap implementation
  - [ ] Reference images: `../_dev`
- [ ] Glowing palette colors that produce a shimmering/glow effect
- [ ] Palette colors that act as light sources in that color (strength, glow, …)
  - [ ] Like emission in MagicaVoxel:
        https://www.megavoxels.com/learn/how-to-create-lights-in-magicavoxel-using-emission/
- [ ] Loading 3D objects into the scene (standard formats, Blender, …)
- [ ] Textures instead of colors, or as masks over the colors with layer blend modes
- [x] AI denoiser (Intel Open Image Denoise, from RWildFire) with albedo and normal helper images
