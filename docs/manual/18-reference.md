# Reference

## Keyboard shortcuts

On macOS, <kbd>Ctrl</kbd> is <kbd>Cmd</kbd>.

| Keys | Action |
|------|--------|
| <kbd>Ctrl</kbd>+<kbd>N</kbd> | New scene |
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | Open a scene, Mandelbulb3D parameters or a PNG with a scene |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | Save |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> | Save as |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>S</kbd> | Quick save (no dialog; folder in the Preferences) |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | Export image |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | Undo |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> or <kbd>Ctrl</kbd>+<kbd>Y</kbd> | Redo |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> | Copy the scene as text (when no text field has the focus) |
| <kbd>Ctrl</kbd>+<kbd>V</kbd> | Paste a scene or Mandelbulb3D text parameters (when no text field has the focus) |
| <kbd>Ctrl</kbd>+<kbd>Q</kbd> | Quit |
| <kbd>F1</kbd> | Help |
| <kbd>F12</kbd> | Render window: test render of the current scene |

### In the view

The view takes the keyboard when you click it, or when the mouse is over it and nothing else has
the focus.

| Input | Orbit mode | Fly mode |
|-------|------------|----------|
| <kbd>Tab</kbd> | Switch to Fly | Switch to Orbit |
| Left drag | Orbit around the point under the cursor | Look around |
| <kbd>Shift</kbd>+left drag | Look around | Look around |
| Middle drag | Look around | Look around |
| Right drag | Pan | Pan |
| Mouse wheel | Move toward the point under the cursor | Change the flying speed |
| Double-click | Look at the point and set the focus distance there | same |
| <kbd>Ctrl</kbd>+click | Select the object under the cursor | same |

Keys while the view has the focus:

| Keys | Action |
|------|--------|
| <kbd>W</kbd> / <kbd>S</kbd> | Forward / backward |
| <kbd>A</kbd> / <kbd>D</kbd> | Left / right |
| <kbd>R</kbd> / <kbd>F</kbd> | Up / down |
| <kbd>Q</kbd> / <kbd>E</kbd> | Roll |
| <kbd>←</kbd> / <kbd>→</kbd> | Turn left / right |
| <kbd>↑</kbd> / <kbd>↓</kbd> | Tilt up / down |
| <kbd>Shift</kbd> (held) | Move four times faster |
| <kbd>Space</kbd> | *Fly mode:* capture the mouse for free steering (mouse look) |
| <kbd>Esc</kbd> | Release the mouse |

The flying speed follows the distance to the surface, so flying stays controllable at any zoom.

### Other mouse actions

- **Axis cross** (with **Grid** on): click an axis end to look along that axis.
- **Light handles** (with **Lights** on): click a light to select it; drag handles to move and
  aim lights.
- **Object handles** (Move, Rotate, Scale in the top bar): drag to change the selected object;
  <kbd>Shift</kbd> while dragging the center moves it toward or away from the camera.
- **Gradient strip**: click to add a color, drag a marker to move it, right-click or
  double-click a marker to remove it.
- **Any slider or number field**: double-click resets it to its default. The ↻ button on a
  section title resets the whole section.
- **Dropping a file** onto the window opens it.

## File types

| Extension | What it is |
|-----------|------------|
| `.rmb` | RMandelbulb3D scene (JSON text). Carries the code of the non-built-in formulas it uses. |
| `.png` | Images saved by RMandelbulb3D contain the scene; opening one restores it. |
| `.m3p`, `.txt` | Mandelbulb3D parameters (files, or text copied from Mandelbulb3D). |
| `.mbf` | Formulas in RMandelbulb3D's formula language. |
| `.m3f` | Mandelbulb3D formula files (translated on import). |
| `.obj`, `.stl`, `.ply`, `.gltf`, `.glb`, `.fbx` | 3D models for the scene composition. |
| `.jpg`, `.png`, `.bmp` | Images for background, color maps, texture layers and bokeh (texture layers also read `.pgm`). |
| `.wav`, `.mp3`, `.flac`, `.ogg`, `.opus`, `.m4a`, `.aac` | Sound tracks for movies. |
| `.mp4`, `.mov`, `.webm`, `.gif` | Exported movies. |

## Where files are kept

| What | Linux | Windows |
|------|-------|---------|
| Configuration folder | `~/.config/rmandelbulb3d/` (or `$XDG_CONFIG_HOME/rmandelbulb3d/`) | `%APPDATA%\rmandelbulb3d\` |
| Preferences | `prefs.json` in the configuration folder | same |
| Saved palettes | `palettes.json` in the configuration folder | same |
| Your formulas | `formulas/` in the configuration folder | same |
| Cache of baked 3D models | `~/.cache/rmandelbulb/models/` | `%LOCALAPPDATA%\rmandelbulb\models\` |

On macOS the model cache is `~/Library/Caches/rmandelbulb/models/`. It holds one file per model
and bake setting, and the oldest files are removed when it grows above 2 GB.

### Environment variables

| Variable | What it does |
|----------|--------------|
| `RMB_OIDN_DIR` | Folder of the denoiser library (Intel Open Image Denoise). |
| `RMB_MAPS_DIR` | An extra folder for Mandelbulb3D maps. |
| `RMB_MODEL_CACHE` | Another folder for the model cache; empty or `off` switches it off. |

The denoiser library is searched in this order: `RMB_OIDN_DIR`, next to the program (`lib/` on
Linux and macOS), the `oidn/` folder of the source tree, and the system library path.

## FAQ

**My image is noisy. What helps?**
Raise the samples, and switch on **Denoise** (Effects › Image). For still images 16–32 samples
with the denoiser usually beat 64 without. Noise comes from ray traced occlusion, soft shadows,
area and line lights, depth of field, gloss, glass, light scattering, emissive light and path
tracing; each needs more samples. For a quick preview, **Ambient occlusion › Fast** has no noise.

**There are holes or speckles in the surface.**
The rays overstep the surface. Lower **Step** in the Ray marching tab (try 0.5, then 0.3) and
raise **Max steps** if distant parts vanish. See chapter "Ray marching".

**Distant parts are missing or cut off.**
Raise **Max distance** and **Max steps**. Check **Near** too: it hides everything closer than its
distance.

**Rendering is slow.**
Lower the preview **Resolution** in the preferences, switch ambient occlusion to *Fast*, set
**Smooth normals** to 0, raise **Detail** a little while exploring, and switch off light
scattering, steam and extra emission **Rays** until the final render. Deep zooms in double
precision are several times slower. Keep **Optimize at rest** on.

**The surface gets grainy and blocky when I zoom in deep.**
Single precision is used up. Set **Precision** to *Auto* or *Double* (Ray marching tab). The view
shows "Deep zoom: double precision (slower)" while it is used. The first switch compiles a new
shader, which takes 10–20 seconds. Zooms beyond about 1e11 need the CPU renderer
(`rmb-cli render --cpu`).

**A Mandelbulb3D scene opens with "Formulas not available".**
Not every Mandelbulb3D formula is translated: formulas that switch the hybrid's formulas or use
random numbers, and a few others, are missing. The scene opens without those slots and the
**Notes** tab lists what could not be converted. If you have the formula's `.m3f` file, try
**Preferences › Formulas › Import .m3f…**. See chapter "Mandelbulb3D import".

**A formula, color map or background image says "not found".**
Formulas and images that use Mandelbulb3D map numbers need the `M3Maps` folder: set it in
**Preferences › Maps**. For other images, give the full path.

**The view shows a warning about the GPU, or the export says "Needs a GPU".**
Without a usable graphics card the preview renders on the CPU, which is much slower, and the
render and export windows are not available. Render with `rmb-cli render --cpu` instead. The
status bar shows which graphics card is used.

**The denoiser is not available.**
The hover text of **Effects › Image › Denoise** and of **Denoised** in the top bar says why. The
library must be next to the program or found through `RMB_OIDN_DIR`.

**My animation flickers.**
With the denoiser at low sample counts fine detail shimmers from frame to frame. Use 64 samples
for animations with much fine detail, or switch the denoiser off: raw noise does not flicker.

**Movie formats are grayed out in the export window.**
ffmpeg was not found. Install it or choose the program in **Preferences › Movies**. PNG images
always work.

**How do I get a scene back from a rendered image?**
Open the PNG (or drop it onto the window): images saved by RMandelbulb3D contain their scene.
