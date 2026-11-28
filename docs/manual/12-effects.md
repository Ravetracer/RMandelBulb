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
- **Sky**: an atmosphere lit by the sun (below).

### Sky

The sky mode computes the light of an atmosphere for every direction: a blue sky with a paler
horizon by day, red and orange sunsets with a blue dusk above them, twilight after sunset and
stars at night. The colors of the air and its haze are settings, so Mars (butterscotch by day,
blue at sunset), Titan or invented planets are as easy as Earth. Below the horizon lies the
ground, hazed toward the horizon. A strip at the top of the section shows the sky all around
(north, east, south, west, from just below the horizon to the zenith).

![The presets (rows: Earth clear, Earth hazy, Mars, Titan, Alien green, Alien violet) at 7, 10, 13, 18:30, 19:48 and 21:30](images/sky-presets.jpg)

![The sky settings](images/inspector-sky.png){: .half}

**Preset** sets the atmosphere: colors, haze, density, ozone, ground and sun color. The sun's
place, the stars and the brightness stay.

The sun is placed by **Time of day** (real solar geometry) or **Manual**:

| Setting | What it does |
|---------|--------------|
| **Time** | Local solar time in hours; at 12 the sun is highest (in the south for northern latitudes). |
| **Latitude** | Degrees north (negative: south). Near the poles the sun stays low or never sets. |
| **Day of year** | 1 = January 1st, 172 = the longest day in the north. |
| **North** | Direction of north in the world, in degrees from the x axis (like a light's direction). |
| **Direction**, **Height** | Manual: the sun's direction from the x axis and its height above the horizon (negative: below it; the twilight lasts to -18°). |
| **Time-lapse** | Hours per second in animations (0: the time stands still). |

| Atmosphere | What it does |
|------------|--------------|
| **Sky color**, **Scattering** | The colors the air scatters most (Rayleigh scattering) and how much: a deeper sky, a whiter horizon, redder sunsets. |
| **Haze color**, **Haze** | Haze or dust (Mie scattering): a paler sky and a glow around the sun. |
| **Sun glow** | How tightly the haze gathers around the sun (0: evenly, 0.95: a small bright halo). The Mars preset sets it per color (blue scatters more forward: blue sunsets); the slider moves all three. |
| **Density** | Thickness of the atmosphere (multiplies scattering and haze). |
| **Ozone** | Ozone high above absorbs orange and green along the low sun's long path: it keeps the sky blue at dusk (Earth: 1). |
| **Ground** | Color of the ground below the horizon. |
| **Brightness** | Overall brightness of the sky and the stars. |

| Sun and stars | What it does |
|---------------|--------------|
| **Sun color** | Color of the sunlight before the atmosphere. |
| **Sun size**, **Disk brightness** | Angular diameter of the sun disk in degrees (ours: 0.53) and its brightness (0: no disk, only its light). |
| **Stars**, **Star brightness** | How many stars show where the sky is dark, and how bright. |
| **Sun lights the scene** | Light 1 follows the sun: its direction, its color (redder when low) and its intensity, which fades out at sunset. The ambient light takes the average colors of the sky and the ground: shadows are filled bluish by day, warm at sunset, and the night is dark. If the scene has no light, the sky adds light 1. The Light tab shows notes; these settings are then set by the sky. |
| **Sunlight** | Intensity of light 1 relative to the sun. |
| **Skylight** | Ambient light from the sky relative to its brightness. |

With **Fog › Background color**, the fog takes the sky's air in each direction (without the
sun disk and the stars): distant things fade into the horizon's color, and water melts into
the sky at the horizon.

!!! tip
    For a quick time-lapse, set **Time-lapse** to 1–2 hours per second and export an
    animation: the sky, the sun light and its shadows follow the time.

### Background image

**Image…** puts a panorama image (latitude/longitude) behind the scene. The × removes it. A file
name that is not found is shown as a warning: give the full path or put the file in the maps folder
(Preferences › Maps).

| Setting | What it does |
|---------|--------------|
| **Brightness** | Brightness of the image, and of the ambient light taken from it (0–4, default 1). |
| **Smooth** | Softens the image when it loads, against noise and hard pixel edges (Mandelbulb3D's "smooth" option). |
| **Fill the screen** | The image fills the screen instead of surrounding the scene. Reflections still see it by angle. |
| **Rotate X/Y/Z** | Turns the panorama (degrees). Not with Fill the screen. |
| **Make spherical** | Squeezes a flat photo toward the poles so it wraps around the scene with less distortion (Mandelbulb3D's "convert to spherical"). |
| **Blend borders** | Blends the left and right edges into each other, so the panorama has no seam. |
| **Fog on top** | The fog is added on top of the image instead of covering it. |
| **Ambient light** | The image lights the scene: a blurred copy replaces the sky and ground colors of the ambient light (Lighting › Ambient). |

!!! note
    The fog keeps using the background *colors* above (or the sky), also when an image is
    shown.

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
| **Offset per distance** | Ray steps added to the glow offset per unit of distance from the camera; negative values give more glow farther away (Mandelbulb3D's fog offset Z). |
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
| **Blades** | The shape of the lens opening, which out-of-focus highlights take on (bokeh): 0 is round, 3 to 9 give a polygon with that many corners (5 or 6 look like a real camera lens). | 0 |
| **Blade rotation** | Turns the polygon (degrees; with 3 or more blades). | 0° |

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
