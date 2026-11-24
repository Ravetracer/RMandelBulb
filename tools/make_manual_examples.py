#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Writes the example scenes of the manual's how-to chapter (docs/manual/examples/*.rmb) and
renders their images (docs/manual/images/howto-*.jpg) with rmb-cli.

    cargo build --release -p rmb-cli -p rmb-core --example compose_demo
    python3 tools/make_manual_examples.py [neon_steps hybrid_ranges …]  (default: all)

Needs ImageMagick (`convert`) for the JPEG conversion.
"""

import copy
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PRESETS = ROOT / "crates" / "rmb-app" / "presets"
EXAMPLES = ROOT / "docs" / "manual" / "examples"
IMAGES = ROOT / "docs" / "manual" / "images"
CLI = ROOT / "target" / "release" / "rmb-cli"

TWISTED_BULB = """formula "Twisted bulb" {
    category "My formulas"
    description "A power bulb that twists around the z axis, more with the height."
    de log
    param power "Power" = 8 [2, 16]
    param twist "Twist" = 1.5 [-4, 4]
    iter {
        let a = z * twist
        let tx = x * cos(a) - y * sin(a)
        y = x * sin(a) + y * cos(a)
        x = tx
        let r = max(sqrt(x * x + y * y + z * z), 1.0e-20)
        let theta = acos(clamp(z / r, -1.0, 1.0)) * power
        let phi = atan2(y, x) * power
        dr = power * pow(r, power - 1.0) * dr + 1.0
        let zr = pow(r, power)
        x = zr * sin(theta) * cos(phi) + cx
        y = zr * sin(theta) * sin(phi) + cy
        z = zr * cos(theta) + cz
    }
}
"""


def load(name):
    return json.loads((PRESETS / f"{name}.rmb").read_text())


def save(scene, name):
    EXAMPLES.mkdir(parents=True, exist_ok=True)
    path = EXAMPLES / f"{name}.rmb"
    path.write_text(json.dumps(scene, indent=2))
    return path


def render(scene_path, image, width=640, height=360, samples=48):
    with tempfile.TemporaryDirectory() as tmp:
        png = Path(tmp) / "out.png"
        r = subprocess.run([str(CLI), "render", str(scene_path), "-o", str(png), "-W", str(width), "-H", str(height), "-s", str(samples), "--denoise"], capture_output=True, text=True)
        if not png.exists():
            sys.exit(f"render of {scene_path} failed:\n{r.stderr}")
        subprocess.run(["convert", str(png), "-quality", "88", str(IMAGES / image)], check=True)
    print(image)


def neon_steps():
    """Neon lines in a blue haze, built up in four steps."""
    final = load("neon-box-city")
    # 1: the dark structure alone (no bands, no haze, no bloom).
    s1 = copy.deepcopy(final)
    s1["material"]["bands"] = []
    s1["fog"]["haze"] = 0.0
    s1["post"]["bloom"] = 0.0
    s1["post"]["chromatic"] = 0.0
    # 2: glow bands.
    s2 = copy.deepcopy(s1)
    s2["material"]["bands"] = final["material"]["bands"]
    # 3: fractal haze.
    s3 = copy.deepcopy(s2)
    s3["fog"]["haze"] = final["fog"]["haze"]
    for i, s in enumerate([s1, s2, s3, final], 1):
        render(save(s, f"neon-step{i}"), f"howto-neon-{i}.jpg", 480, 270)


def hybrid_ranges():
    """An Amazing Box that turns into a Mandelbulb after three iterations."""
    base = load("mandelbulb")
    box = {"formula": "Amazing Box", "iterations": 1, "params": {"scale": -1.8, "min_r": 0.5}}
    bulb = {"formula": "Mandelbulb", "iterations": 1, "params": {"power": 8.0}}
    plain = copy.deepcopy(base)
    plain["fractal"]["slots"] = [copy.deepcopy(box), copy.deepcopy(bulb)]
    plain["fractal"]["max_iterations"] = 14
    plain["fractal"]["bailout"] = 16.0
    # The box is bigger than the bulb: the camera steps back along its view.
    pos = plain["camera"]["position"]
    plain["camera"]["position"] = {k: v * 2.4 for k, v in pos.items()}
    ranged = copy.deepcopy(plain)
    ranged["fractal"]["slots"][0]["stop"] = 2
    ranged["fractal"]["slots"][1]["start"] = 3
    render(save(plain, "hybrid-alternating"), "howto-hybrid-alternating.jpg", 480, 270)
    render(save(ranged, "hybrid-ranges"), "howto-hybrid-ranges.jpg", 480, 270)


def own_formula():
    """A scene with an own formula: the scene carries the formula's code."""
    s = load("mandelbulb")
    s["fractal"]["slots"] = [{"formula": "Twisted bulb", "iterations": 1, "params": {"power": 8.0, "twist": 1.5}}]
    s["formulas"] = {"Twisted bulb": TWISTED_BULB}
    render(save(s, "twisted-bulb"), "howto-twisted-bulb.jpg")


def glass():
    s = load("glass-bulb")
    opaque = copy.deepcopy(s)
    opaque["material"]["transmission"] = False
    opaque["material"]["reflection"] = 0.0
    render(save(opaque, "glass-before"), "howto-glass-before.jpg", 480, 270)
    render(save(s, "glass-after"), "howto-glass-after.jpg", 480, 270)


def composition():
    out = EXAMPLES / "composition.rmb"
    EXAMPLES.mkdir(parents=True, exist_ok=True)
    demo = ROOT / "target" / "release" / "examples" / "compose_demo"
    subprocess.run([str(demo), str(out)], check=True, capture_output=True)
    render(out, "howto-composition.jpg")


def look_at(scene, target):
    """Turns the camera toward `target` (z up)."""
    p = scene["camera"]["position"]
    f = [target[k] - p[a] for k, a in enumerate("xyz")]
    n = sum(v * v for v in f) ** 0.5
    f = [v / n for v in f]
    r = [f[1], -f[0], 0.0]
    n = sum(v * v for v in r) ** 0.5
    r = [v / n for v in r]
    u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]]
    for name, v in (("forward", f), ("right", r), ("up", u)):
        scene["camera"][name] = dict(zip("xyz", v))


def deep_zoom():
    s = load("mandelbulb")
    s["march"]["precision"] = "Double"
    # Close up, the surface needs more iterations to show its detail.
    s["fractal"]["max_iterations"] = 40
    look_at(s, [0.7, -0.5, 0.5])
    path = save(s, "deep-zoom-start")
    out = EXAMPLES / "deep-zoom.rmb"
    tool = ROOT / "target" / "release" / "examples" / "deep_scene"
    subprocess.run([str(tool), str(path), str(out), "1e5", "double"], check=True, capture_output=True)
    path.unlink()
    render(out, "howto-deep-zoom.jpg")


def projections():
    """The projections and stereo layouts of the Camera tab side by side (images/projections.jpg)."""
    base = load("mandelbulb")
    modes = [
        ("Perspective", {}, 320, 180),
        ("Fisheye", {"projection": "Fisheye"}, 320, 180),
        ("Fisheye circle", {"projection": "FisheyeCircle", "fisheye_fov": 220}, 180, 180),
        ("Panorama 360°", {"projection": "Panorama"}, 360, 180),
        ("Panorama 180°", {"projection": "HalfPanorama"}, 180, 180),
        ("Anaglyph", {"stereo": "Anaglyph", "convergence": 2.5, "eye_ratio": 0.05}, 320, 180),
        ("Side by side", {"stereo": "SideBySide", "convergence": 2.5}, 360, 180),
        ("360° over / under", {"projection": "Panorama", "stereo": "OverUnder", "convergence": 2.5}, 180, 180),
    ]
    with tempfile.TemporaryDirectory() as tmp:
        tiles = []
        for i, (label, view, w, h) in enumerate(modes):
            s = copy.deepcopy(base)
            s["view"] = view
            scene = Path(tmp) / f"m{i}.rmb"
            scene.write_text(json.dumps(s))
            png = Path(tmp) / f"m{i}.png"
            subprocess.run([str(CLI), "render", str(scene), "-o", str(png), "-W", str(w), "-H", str(h), "-s", "32", "--denoise"], check=True, capture_output=True)
            tiles += ["-label", label, str(png)]
        subprocess.run(["montage", *tiles, "-tile", "4x2", "-geometry", "220x124>+6+6", "-background", "#202124", "-fill", "#e8eaed", "-pointsize", "13", "-quality", "88", str(IMAGES / "projections.jpg")], check=True)
    print("projections.jpg")


def vr_panorama():
    """A 360° stereo panorama from inside the Amazing Box (over / under)."""
    s = load("amazing-box")
    # Inside the structure, level, so it surrounds the camera.
    s["camera"]["position"] = {"x": 0.0, "y": -1.5, "z": 0.2}
    look_at(s, [0.0, 0.0, 0.2])
    s["view"] = {"projection": "Panorama", "stereo": "OverUnder", "convergence": 1.0, "eye_ratio": 1.0 / 30.0}
    s["width"], s["height"] = 2048, 2048
    path = save(s, "vr-panorama")
    render(path, "howto-vr-panorama.jpg", 480, 480)


if __name__ == "__main__":
    only = sys.argv[1:]
    for f in (neon_steps, hybrid_ranges, own_formula, glass, composition, deep_zoom, projections, vr_panorama):
        if only and f.__name__ not in only:
            continue
        f()
