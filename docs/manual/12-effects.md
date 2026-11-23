# Effects

The **Effects** tab (the fog icon) holds what lies around and between the surfaces: the
background, fog and glow, the fractal haze, depth of field and cutting planes. Its last section,
**Image**, has its own chapter ("Image").

![The Effects tab](images/inspector-effects.png)

Each section has a ↻ button that resets it to the defaults.

## Background

The background shows where rays hit nothing. Choose its kind:

- **Solid**: one **Color**.
- **Gradient** (default): a sky gradient with **Top**, **Horizon** and **Bottom** colors.
- **Vertical**: from **Top** to **Bottom** over the screen height, like Mandelbulb3D. **Curve**
  bends it: 1 is even, above 1 more of the top color, below 1 more of the bottom color.

### Background image

**Image…** puts a panorama image (latitude/longitude) behind the scene. The × removes it. A file
name that is not found is shown as a warning: give the full path or put the file in the maps folder
(Preferences › Maps).

| Setting | What it does |
|---------|--------------|
| **Brightness** | Brightness of the image, and of the ambient light taken from it (0–4, default 1). |
| **Fill the screen** | The image fills the screen instead of surrounding the scene. Reflections still see it by angle. |
| **Rotate X/Y/Z** | Turns the panorama (degrees). Not with Fill the screen. |
| **Make spherical** | Squeezes a flat photo toward the poles so it wraps around the scene with less distortion (Mandelbulb3D's "convert to spherical"). |
| **Blend borders** | Blends the left and right edges into each other, so the panorama has no seam. |
| **Fog on top** | The fog is added on top of the image instead of covering it. |
| **Ambient light** | The image lights the scene: a blurred copy replaces the sky and ground colors of the ambient light (Lighting › Ambient). |

!!! note
    The fog keeps using the background *colors* above, also when an image is shown.

## Fog and glow

### Depth fog

Depth fog lets the scene fade into a color with the distance. Two kinds:

- **Exponential** (default): the fog grows exponentially with the distance. **Fog density** is
  the fog per unit of distance (0 = off).
- **Range**: the fog grows over the view range, from the **Near** to the **Max distance** of the
  Ray marching tab (Mandelbulb3D's depth fog).

| Setting (Range) | What it does |
|-----------------|--------------|
| **Fog strength** | Fog amount at the far end (0 = off). |
| **Fog start** | Where in the range the fog begins; before it the image gets brighter. |
| **Fog curve** | 0: even over the range; higher values put more fog close to the camera. |
| **Far fog** | Squares the fog amount: less fog close by (Mandelbulb3D's far fog). |

**Background color** makes the fog take the background color behind each pixel; otherwise it
uses **Fog color**.

### Light scattering

**Light scattering** makes the fog scatter light toward the camera: beams of light through gaps
in the fractal ("god rays") and lit haze around lights (0 = off). It needs fog. **Forward
scattering** above 0 makes the fog shine most when you look toward a light, like real haze; 0
scatters evenly; below 0 shines back toward the light.

It casts one more shadow ray per sample, so it needs more samples to get smooth.

### Glow

**Glow** adds light along rays that pass close to the surface: Mandelbulb3D's *dynamic fog*.
Negative values darken instead.

| Setting | What it does |
|---------|--------------|
| **Glow** | Strength (−4–4, default 0). |
| **Glow color** | The first glow color. |
| **Glow offset** | Ray steps that give no glow yet. |
| **Second color after** | Ray steps until the glow has the **Second glow color** (0 = only the first color). |
| **Cover** | The glow covers the image instead of adding light. |
| **Glow iteration** | Only ray steps where the fractal escaped after this many iterations glow (0 = all steps). |

The glow grows with the number of ray steps, so it also depends on **Step** and **Max steps**.

## Fractal haze

**Fractal haze** is a glowing volume shaped by the main fractal. It shines by itself and hides
what lies behind it (0 = off). Its **Haze shape** decides where it is:

- **Distance** (default): thickest at the surfaces, fading with the distance from them. It hugs
  every structure and fills narrow gaps. **Haze reach** sets how far it reaches, relative to the
  distance from the camera.
- **Iterations**: where the fractal's orbits take long to escape, which gives shells around the
  fractal (Mandelbulber's iteration fog). **Haze from** is the share of the maximum iterations
  from which on there is haze (higher: closer to the fractal); **Haze to** is the share above
  which there is none again (lower values keep it out of regions that never escape).

| Setting | What it does |
|---------|--------------|
| **Haze glow** | Brightness of the haze's own light (0: a dark smoke that only hides). |
| **Haze from gradient** | The haze takes the gradient's colors by the iteration count instead of one **Haze color**. |

!!! tip
    *Iterations* depends a lot on the formula. On a Mandelbulb the high counts occur only in a
    thin shell at the surface; on scenes with large regions that never escape it covers
    everything. Start with *Distance* and a small reach.

## Depth of field

Depth of field blurs everything outside the focus distance, like a camera lens.

| Setting | What it does | Range / default |
|---------|--------------|-----------------|
| **On** | Switches the blur on. | off |
| **Aperture** | Lens size: more blur. | 0.0005–1, default 0.02 |
| **Focus distance** | Distance from the camera that is sharp. | default 2.5 |

**Double-click** into the view to look at a point and set the focus distance there. The blur is
sampled, so it refines with the samples.

## Cutting planes

Each of the three cutting planes, **Cut X**, **Cut Y** and **Cut Z**, removes everything on one
side of a plane across that world axis:

- the check box switches the plane on;
- the number is the position along the axis;
- **keep +** keeps the part above the position (else the part below).

Cuts open the fractal to show its inside. Scenes imported from Mandelbulb3D may bring
**Interior colors** for the cut faces (Color › Mapping).
