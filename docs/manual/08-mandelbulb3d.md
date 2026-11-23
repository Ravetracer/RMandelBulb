# Mandelbulb3D parameters and formulas

RMandelbulb3D reads the parameters of Mandelbulb3D: the `.m3p` files it saves, and the text
parameters its users share in forums and galleries. It converts them into an RMandelbulb3D
scene with the same formulas, camera, lights, colors and effects, as far as they exist in
RMandelbulb3D. This chapter explains how to bring Mandelbulb3D work over and what to look out
for.

## Opening Mandelbulb3D parameters

There are three ways:

- **File › Open…** (<kbd>Ctrl</kbd>+<kbd>O</kbd>) opens `.m3p` files, and `.txt` files that hold
  text parameters. You can also drop such a file onto the window, or pass it to the program at
  start.
- **Edit › Paste scene** (<kbd>Ctrl</kbd>+<kbd>V</kbd>) reads text parameters from the
  clipboard. In Mandelbulb3D they are copied as a block of text that starts with
  `Mandelbulb3Dv18{`. Pasting is one undo step.
- The text parameters can be pasted while no text field in RMandelbulb3D has the keyboard focus;
  otherwise <kbd>Ctrl</kbd>+<kbd>V</kbd> pastes into the text field.

Parameter versions 20 to 45 are supported. Files of older versions are refused with a
message.

The imported scene is a new, unsaved scene: **File › Save** asks for a name for the `.rmb` file.

### Import notes

The **Notes** tab receives the title of the parameters (from the text format), the authors
stored in the file, and the parameter and program version. When something could not be
converted, the notes end with a list headed "Import notes:", the Notes tab opens by itself and
the status bar says "Opened … with import notes". Typical notes:

- formulas that are not available, by name;
- a color map or background image that was not found;
- Mandelbulb3D features that RMandelbulb3D shows differently (see below).

## What is imported

| Area | What RMandelbulb3D takes over |
|------|-------------------------------|
| Image | Width and height. |
| Camera | Position, direction and field of view. Mandelbulb3D starts its rays on a plane in front of the eye; the import places a normal camera where those rays meet, and sets **Near** (Ray marching tab) so that nothing in front of Mandelbulb3D's start plane shows. The far end of the view becomes **Max distance**. |
| Formulas | Every used slot with its formula, iteration count and option values; **Repeat from**; maximum iterations and bailout; Julia mode with its constant; the 4D rotation. |
| Hybrid modes | Alternating hybrids; interpolation hybrids (slots 1 and 2 with their weights); DE combination with its mode (including "mix"), the second chain, its iteration limit and blend distance (chapter 5). |
| Inside rendering | Inside rendering; "inside and outside" becomes inside rendering when the camera starts within the set. |
| Ray marching | The surface threshold (DEstop, as **Detail**), the ray step, the refinement steps and the smoothed normals. Because Mandelbulb3D marches without a step limit, **Max steps** is set high enough for small steps. |
| Cutting planes | The X, Y and Z cuts; the side between the camera and the plane is removed. |
| Lights | Directional and positional lights with color, intensity, highlights and Mandelbulb3D's diffuse functions; shadows; visible light sources with their glow shape (soft, sharp, ball, halo). |
| Ambient light | The two ambient colors (upper and lower); the ambient shadow as ray traced ambient occlusion with its strength and reach; the roughness factor. |
| Colors | The ten color stops with their surface and highlight colors and transparency; Mandelbulb3D's color index (color mode **Iterations (MB3D)**) with the start and end sliders and color cycling; the second color choice; color on iteration; color variation with depth; interior colors; color maps. |
| Background and fog | The vertical background gradient, the depth fog with its curve and "far fog", and the dynamic fog as **Glow** with its offset, second color, blend option and "on iteration" setting. |
| Background image | The panorama with its rotation, screen coordinates, brightness, "convert to spherical", border fitting, fog added on top, and the image as ambient light. |
| Depth of field | Focus distance and aperture. |
| Reflections and glass | Reflection amount and bounces; for transmission, the refraction index, absorption and scattering. |
| Output | Mandelbulb3D's gamma. Mandelbulb3D shades with the colors as shown on screen, without tone mapping; the import turns on **Display space** (Effects › Image) and switches off filmic tone mapping and the vignette. |

The imported values land in the normal settings, so you can change everything afterwards, for
example add a lens flare, switch to path tracing or export with the denoiser.

## Formulas

Mandelbulb3D formulas are found by their Mandelbulb3D name. RMandelbulb3D has:

- Mandelbulb3D's internal formulas (Integer Power, Real Power, Quaternion, Tricorn, Amazing Box,
  Bulbox, Folding Int Pow, Aexion C);
- more than 470 of Mandelbulb3D's formula files, translated into the formula language: 3D and
  4D formulas, transforms and dIFS formulas, also the Pascal-source "JIT" formulas. They appear in
  the formula list under **MB3D 3D**, **MB3D 4D**, **MB3D dIFS** and **MB3D transforms**.

The translations were checked against the original formula code. Each imported option value goes
to the matching parameter, in Mandelbulb3D's option order.

A formula that is not available stays in the hybrid as a slot with a warning sign and is
skipped when rendering; the view shows "Formulas not available" with its name. To add formulas
from your own Mandelbulb3D installation, use **Import .m3f…** in **Edit › Preferences… ›
Formulas** (chapter 6). Formulas that cannot be translated are counted in the status message.

## The maps folder

Some Mandelbulb3D formulas read images (height maps), and scenes can use color maps and
background images. Mandelbulb3D keeps these in its **M3Maps** folder, as files named by their
number (for example `10.jpg`). Tell RMandelbulb3D where that folder is:

1. Open **Edit › Preferences… › Maps**.
2. Click **Choose folder…** and pick Mandelbulb3D's `M3Maps` folder.

**Clear** forgets the folder again. Besides the chosen folder, RMandelbulb3D looks in the folder
named by the environment variable `RMB_MAPS_DIR` and in a `maps` or `M3Maps` folder next to the
program. Background images are also found by their full path.

!!! tip
    If an import reports a missing color map or background image, set the maps folder and open
    the parameters again.

## What is not imported

- **(K/L)IFS hybrids**, Mandelbulb3D's fourth hybrid mode, are not supported.
- **Formulas that switch the hybrid's formulas** (the `_Cond…` formulas) are not available.
- The **panorama camera** is shown as a normal perspective camera.
- A **DE combination "mix" with the second chain first** is shown with the first chain first.
- **Reflections and transmission only on dIFS formulas** are shown on all surfaces.
- An **interpolation hybrid** uses only its first two formulas, like in Mandelbulb3D; further
  formulas in the file are reported.
- The **smoothing of background images on load** is not done.

Each of these produces an import note when a file uses it.

## Differences to watch for

- **Ambient occlusion.** Mandelbulb3D's ambient shadow becomes ray traced occlusion. It is
  noisy at first and smooths out with more samples. Some scenes with strong lights can look
  brighter than in Mandelbulb3D.
- **Progressive rendering.** Soft shadows, ambient occlusion and depth of field refine over the
  samples. Use the **Denoised** preview or the denoiser for saved images to get clean results
  with fewer samples.
- **Sliders and units.** Mandelbulb3D's light and color sliders are converted to
  RMandelbulb3D's settings, which use other units, so the numbers differ from the ones you know
  from Mandelbulb3D.
- **Display space.** Imported scenes shade in display space like Mandelbulb3D. Switching
  **Effects › Image › Display space** off gives the linear lighting of new RMandelbulb3D scenes,
  with softer light; the colors then need adjusting.
- **Holes and noise.** If the surface shows holes that Mandelbulb3D did not, lower **Step** in
  the Ray marching tab or **DE scale** in the Iteration section.
