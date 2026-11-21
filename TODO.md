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
- [ ] Glowing palette colors that produce a shimmering/glow effect
- [ ] Palette colors that act as light sources in that color (strength, glow, …)
  - [ ] Like emission in MagicaVoxel:
        https://www.megavoxels.com/learn/how-to-create-lights-in-magicavoxel-using-emission/
- [ ] Loading 3D objects into the scene (standard formats, Blender, …)
- [ ] Textures instead of colors, or as masks over the colors with layer blend modes
- [x] AI denoiser (Intel Open Image Denoise, from RWildFire) with albedo and normal helper images
