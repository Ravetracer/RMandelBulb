# TODO

## Mandelbulb3D import fidelity

- [x] Shading in display space like MB3D (no linear workflow, no tone mapping)
- [x] Depth fog with MB3D's depth curve, fog color from the vertical background gradient
- [x] Dynamic fog (glow) with offset, second color and blend option
- [x] Ambient light relative to the camera, diffuse/specular multipliers
- [x] Color index like MB3D (smooth iterations from Rout/Rold, start/end sliders, color cycling)
- [x] Depth of field import (focus distance, aperture)
- [ ] Interpolation hybrids (currently imported as alternating)
- [ ] Background images
- [ ] Positional lights (currently directional)
- [ ] Specular colors per gradient stop (currently their average), MB3D's diffuse light functions,
      visible light sources, color variation with depth (VarColZpos)
- [ ] Height map / image formulas (9)
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
- [ ] Loading 3D objects into the scene (standard formats, Blender, …)
- [ ] Textures instead of colors, or as masks over the colors with layer blend modes
