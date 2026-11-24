# Scene composition

A scene can hold more than the main fractal: further fractals, basic shapes such as a ground
plane, boxes and spheres, a water surface, and 3D models from other programs. Each of these
**objects** has its own placement and colors, and is combined with the rest of the scene: added
to it, cut away from it or intersected with it, also with smooth blends. This works like
Mandelbulber's "boolean mode", the technique behind many architectural fractal scenes.

![A composed scene](images/composition.png)

## The scene list

The **SCENE** list at the top of the scene panel shows the **Main fractal** and, below it, up to
eight objects. Each object row shows:

- an icon for its kind (fractal, basic shape or 3D model) and its name;
- a sign for its operation: `+` add, `−` cut away, `∩` intersect, `~+`, `~−`, `~∩` for the
  smooth variants;
- an eye button to show or hide the object. Hidden objects are dimmed.

Click a row to edit the object; its settings replace the formulas in the panel below. Click
**Main fractal** to return to the fractal's formulas. In the view, <kbd>Ctrl</kbd>+click selects
the object under the pointer.

With an object selected, the buttons below the list move it up or down, duplicate it or remove
it.

### The order matters

The objects are combined in list order: each one is added to, cut away from or intersected
with **everything above it** in the list. A box that cuts a window into a wall must come after
the wall; a sphere added after that fills the window again.

## Adding objects

The **+** button next to the SCENE title opens a menu:

| Entry | What it adds |
|-------|--------------|
| Fractal | Another fractal with its own formulas, placement and colors. It starts as a small Amazing Box. |
| Plane | A ground: everything below it is solid. Turn it for walls. It starts below the scene with a checker pattern. |
| Box, Sphere, Cylinder, Capsule, Cone, Torus, Hexagonal prism, Triangular prism | Basic shapes. |
| Water | An ocean surface: waves with foam that reflect the sky and the scene and move in animations. |
| 3D model… | A model file: OBJ (with its MTL materials), STL, PLY, glTF/GLB or FBX. |

New objects appear in front of the camera (the plane and the water below the scene). The
menu is disabled once the scene has eight objects.

## Object settings

| Setting | What it does |
|---------|--------------|
| Name | The name in the scene list. |
| Visible | Includes the object in the scene. |
| Operation | How the object is combined with everything above it (see below). |
| Blend | For the smooth operations: the distance over which the object melts into the others, in world units. Default 0.1. |

### Operations

| Operation | Result |
|-----------|--------|
| Add | The object joins the scene. |
| Cut away | The object is cut out of everything before it. |
| Intersect | Only what is inside both the object and everything before it remains. |
| Add smoothly | Like Add, with a rounded, melted seam. |
| Cut away smoothly | Like Cut away, with rounded edges at the cut. |
| Intersect smoothly | Like Intersect, with rounded edges. |

!!! tip
    A large sphere set to **Intersect** keeps only a ball-shaped piece of the fractal. A box set
    to **Cut away** opens the fractal like a cross section.

## Placement and gizmos

The **Placement** section of an object:

| Setting | What it does |
|---------|--------------|
| X, Y, Z | Position along the world axes. **In front of the camera** moves the object to the middle of the view. |
| Turn X, Y, Z | Rotation in degrees, about x, then y, then z. A double-click sets 0. |
| Scale | Size as a whole. |
| Stretch X, Y, Z | Extra size along the object's own axes (1: none). A double-click sets 1. |

The selected object also has **handles** in the view. **Move**, **Rotate** and **Scale** in the
top bar choose which:

![Object handles in the view](images/gizmos.png)

| Mode | Handles |
|------|---------|
| Move | Arrows along the world axes; drag one to move along it. Drag the center square to move in the plane facing the camera; with <kbd>Shift</kbd> toward or away from the camera. |
| Rotate | Rings about the object's own axes; drag around the center to turn. |
| Scale | Handles on the object's own axes stretch it along that axis. Drag the center to scale it as a whole: right or up makes it larger, left or down smaller. |

While you drag, a small label shows the values. The main fractal is placed with numbers only,
in its own **Placement** section (chapter 5).

## Repetition

The **Repetition** section turns an object into a grid of copies:

| Setting | What it does |
|---------|--------------|
| Spacing X, Y, Z | Distance between the copies along the object's own axes, in its own units (before the scale). 0: no copies along that axis. |
| × (next to a spacing) | Copies on each side of the original. 0: endless. |

Keep the spacing larger than the object, or the copies cut into each other. Endless repetition
of a box or a column gives whole cities or forests at the cost of a single object.

## Shapes

Each basic shape has a section with its size, measured before the object's scale. Shapes
stand upright along their own z axis.

| Shape | Settings |
|-------|----------|
| Plane | None: everything below it is solid. |
| Box | Width, Depth, Height, Rounding (radius of the rounded edges) |
| Sphere | Radius |
| Cylinder | Radius, Height, Rounding |
| Capsule | Radius, Height (from end to end, the round ends included) |
| Cone | Bottom radius, Top radius (0: pointed), Height |
| Torus | Ring radius, Thickness (radius of the tube) |
| Hexagonal prism | Radius (from the axis to the middle of a side), Height |
| Triangular prism | Side, Height |
| Water | See below. |

### Water

The water is an ocean surface made of many waves that run around the wind direction, each a
little shorter and faster than the one before; their crests are pulled sharp like those of
the sea. It reflects the sky and the scene by itself, like real water: little when you look
straight down, almost a mirror at grazing angles. The material's **Reflection** (Color tab)
is not needed for it, and the rest of the scene keeps its own setting.

![Calm water (left) and a rough sea with foam (right)](images/water.jpg)

| Setting | What it does | Start value |
|---------|--------------|-------------|
| Wave height | Height of the largest waves. | 0.06 |
| Wave length | Length of the largest waves (across the wind a bit shorter). | 2 |
| Choppiness | Shape of the crests: 0 = round swell, 1 = like the sea, 2 = very choppy. | 1 |
| Detail | Number of waves: more give finer ripples on the swell (and cost a little speed). | 16 |
| Direction | Wind direction in degrees; the waves run mostly along it. | 0 |
| Foam | White, broken foam on the highest crests, streaked along the wind. | 0.4 |
| Reflection | How strongly the water reflects (1 = like water, 2 = twice as much). | 1 |
| Speed | How fast the waves move in animations (per second; 0 = still). | 1 |
| Time | The moment of the waves: another value shows another wave pattern in a still image. Animations add the speed per second to it. | 0 |

The water's own colors (its material) are the color of the water body: it gets brighter on
the crests, where the light shines through the waves. Dark blue-green colors look like deep
sea; lighter, greener ones like shallow water. The sun shows as sharp glints on the waves.

!!! tip
    For water you can see into, give its colors some transparency and turn on **Glass** in
    the Color tab (chapter 11): rays then also bend into the water. **Bounces** in the Color
    tab sets how many reflections of reflections the water shows (default 2).

## Fractal objects

A fractal object has the same hybrid chain, formula parameters, **Iteration** settings and
**Two chains** (DE combination and interpolation) as the main fractal (chapter 5). It does not
have the slot settings, **Inside**, the 4D rotation, **Min iterations** or the **C factor**. The
formula editor works on the selected slot of the fractal you are editing.

## Materials

The **Material** section of an object:

- **Own colors** off: the object uses the main fractal's colors (Color tab).
- **Own colors** on: the object gets its own gradient, glow bands, mapping and texture layers.
  The gradient and texture layers work like those of the main fractal (chapter 11).

| Setting | What it does |
|---------|--------------|
| Pattern | Shapes and models: how the colors are picked from the gradient. **Solid** (one color, picked by **Offset**), **Checker** (alternating squares in the gradient's end colors), **By height**, **From the center**. Models also have **Model colors**: the colors and textures of the model file. |
| Pattern size | Size of the checker cells, or the height or distance per gradient length. |
| Color by | Fractals: which value of the orbit picks the color, as in the Color tab. |
| Scale, Offset | How fast the colors change, and where the gradient starts. |
| Rotation | In animations: the gradient turns by this many gradient lengths per second. |
| Repeat | Repeats the gradient instead of holding its end colors. |

## 3D models

A 3D model is turned into a distance field, so the renderer can treat it like any other
object. This **baking** runs in the background when you add the model; a progress bar shows it,
and the model appears when it is done. The section **3D model** then shows the number of
triangles, materials and grid points.

| Setting | What it does | Default |
|---------|--------------|---------|
| Change… | Picks another model file. | |
| Resolution | Grid points along the longest side of the distance grid (64, 96, 128, 192, 256). Higher renders faster near complex models, but bakes longer and needs more memory (256: about 32 MB). | 128 |
| Up axis | Which axis of the file points up. Auto: Y for glTF and OBJ (Blender's export default), Z for STL and PLY. | Auto |
| Keep file units | Off: the model is centered and sized to 2 units, then scaled by the placement. On: the file's own coordinates. | off |

The grid is used away from the model; near its surface the triangles themselves are used, so
thin parts and fine detail stay sharp at any distance.

### Model materials

The **Materials** section lists the materials of the model file. For each one:

- the **color button** sets the base color (a right-click returns to the file's color);
- the **texture menu** picks the color texture: the one from the model file, none, an image
  found near the model, or any image (**Browse…**);
- clicking the **name** opens more maps: **Bump** (a height map, with **Strength**),
  **Specular** (bright parts shine, dark parts stay matte), **Normal map** (with **Normal
  strength**) and **Emission** (the material shines by itself, with **Emission color** and
  **Emission strength**).

Names in bold were changed here; a layers icon marks materials with maps or emission. The
changes are saved with the scene; the model file stays untouched.

Above the list:

| Control | What it does |
|---------|--------------|
| Filter | Shows only the materials whose name contains this text. |
| Auto-assign | Assigns the images near the model file to the materials by name (replaces the image choices, keeps the colors). |
| Rescan | Looks for images near the model again. |
| Reset | Uses the materials exactly as the model file has them. |
| Color all shown | Gives every material in the (filtered) list the color of the button next to it. |

"Near the model" means: in the model's folder, in the folders inside it (two levels deep) and in
a `textures` folder next to the model's folder. When a model is added, RMandelbulb3D runs
**Auto-assign** once by itself. glTF and FBX files bring their normal maps and emission along,
OBJ files their emission (`Ke`).

### The model cache

Baked distance grids are kept on disk, in `rmandelbulb/models/` of your cache folder
(`~/.cache` on Linux). The next time you open a scene with the same model and settings, it loads
without baking. The oldest files are removed when the cache grows beyond about 2 GB. The
environment variable `RMB_MODEL_CACHE` sets another folder; `off` switches the cache off.

!!! note
    A lower **Resolution** bakes faster and needs less memory, but renders slower near the
    model.
