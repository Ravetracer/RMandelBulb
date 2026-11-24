#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Packages built RMandelbulb3D programs for a release (used by .github/workflows/release.yml,
also runs locally).

    python3 tools/package.py linux    --bin target/release
    python3 tools/package.py appimage --bin target/release --appimagetool ./appimagetool
    python3 tools/package.py windows  --bin target/release
    python3 tools/package.py macos    --bin target/aarch64-apple-darwin/release --arch arm64

Every package gets the programs (rmb-app, rmb-cli), Intel Open Image Denoise (downloaded by
fetch_oidn.py, SHA-256 checked), INSTALL.txt, README, CHANGELOG, the licenses and, when
--manual names it (default: docs/manual/build/RMandelbulb3D-Manual-<version>.pdf if present),
the PDF manual. Output: dist/RMandelbulb3D-<version>-<platform>.<ext>.
"""

import argparse
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PKG = ROOT / "packaging"
DOCS = ["README.md", "CHANGELOG.md", "LICENSE", "NOTICE"]


def version() -> str:
    text = (ROOT / "Cargo.toml").read_text()
    m = re.search(r'\[workspace\.package\][^\[]*?version\s*=\s*"([^"]+)"', text, re.S)
    if not m:
        sys.exit("no version in Cargo.toml [workspace.package]")
    return m.group(1)


def fetch_oidn(platform: str, dest: Path) -> None:
    subprocess.run([sys.executable, str(ROOT / "tools" / "fetch_oidn.py"), "--platform", platform, "--dest", str(dest)], check=True)


def install_text(name: str, ver: str, arch: str = "") -> str:
    return (PKG / name).read_text().replace("@VERSION@", ver).replace("@ARCH@", arch)


def copy_exe(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)
    dst.chmod(dst.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def copy_docs(dest: Path, manual: Path | None) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    for d in DOCS:
        if (ROOT / d).is_file():
            shutil.copy2(ROOT / d, dest / d)
    if manual and manual.is_file():
        shutil.copy2(manual, dest / manual.name)


def tar_gz(src_dir: Path, out: Path) -> None:
    with tarfile.open(out, "w:gz") as t:
        t.add(src_dir, arcname=src_dir.name)


def zip_dir(src_dir: Path, out: Path) -> None:
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for p in sorted(src_dir.rglob("*")):
            arc = Path(src_dir.name) / p.relative_to(src_dir)
            info = zipfile.ZipInfo.from_file(p, str(arc))
            if p.is_file():
                info.compress_type = zipfile.ZIP_DEFLATED
                z.writestr(info, p.read_bytes())
            else:
                z.writestr(info, b"")


def linux(a, ver: str, out: Path, stage: Path) -> Path:
    name = f"RMandelbulb3D-{ver}-linux-x86_64"
    d = stage / name
    for exe in ("rmb-app", "rmb-cli"):
        copy_exe(a.bin / exe, d / exe)
    fetch_oidn("linux", d)
    copy_docs(d, a.manual)
    (d / "INSTALL.txt").write_text(install_text("INSTALL-linux.txt", ver))
    desktop = (PKG / "linux" / "rmandelbulb3d.desktop").read_text()
    (d / "rmandelbulb3d.desktop").write_text(desktop.replace("Exec=rmb-app", f"Exec={Path('/opt') / 'RMandelbulb3D' / 'rmb-app'}"))
    shutil.copy2(ROOT / "crates" / "rmb-app" / "assets" / "icon.png", d / "rmandelbulb3d.png")
    target = out / f"{name}.tar.gz"
    tar_gz(d, target)
    return target


def appimage(a, ver: str, out: Path, stage: Path) -> Path:
    if not a.appimagetool:
        sys.exit("--appimagetool is needed (https://github.com/AppImage/appimagetool/releases)")
    app = stage / "RMandelbulb3D.AppDir"
    for exe in ("rmb-app", "rmb-cli"):
        copy_exe(a.bin / exe, app / "usr" / "bin" / exe)
    oidn = stage / "oidn-linux"
    fetch_oidn("linux", oidn)
    shutil.copytree(oidn / "lib", app / "usr" / "lib", dirs_exist_ok=True)
    doc = app / "usr" / "share" / "doc" / "rmandelbulb3d"
    copy_docs(doc, a.manual)
    shutil.copytree(oidn / "oidn-license", doc / "oidn-license", dirs_exist_ok=True)
    copy_exe(PKG / "linux" / "AppRun", app / "AppRun")
    shutil.copy2(PKG / "linux" / "rmandelbulb3d.desktop", app / "rmandelbulb3d.desktop")
    icons = app / "usr" / "share" / "icons" / "hicolor" / "256x256" / "apps"
    icons.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "crates" / "rmb-app" / "assets" / "icon.png", icons / "rmandelbulb3d.png")
    shutil.copy2(ROOT / "crates" / "rmb-app" / "assets" / "icon.png", app / "rmandelbulb3d.png")
    apps = app / "usr" / "share" / "applications"
    apps.mkdir(parents=True, exist_ok=True)
    shutil.copy2(PKG / "linux" / "rmandelbulb3d.desktop", apps / "rmandelbulb3d.desktop")
    target = out / f"RMandelbulb3D-{ver}-x86_64.AppImage"
    env = dict(os.environ, ARCH="x86_64", APPIMAGE_EXTRACT_AND_RUN="1")
    subprocess.run([str(a.appimagetool), "--no-appstream", str(app), str(target)], check=True, env=env)
    (out / f"RMandelbulb3D-{ver}-x86_64.AppImage.txt").write_text(install_text("INSTALL-appimage.txt", ver))
    return target


def windows(a, ver: str, out: Path, stage: Path) -> Path:
    name = f"RMandelbulb3D-{ver}-windows-x64"
    d = stage / name
    for exe in ("rmb-app.exe", "rmb-cli.exe"):
        copy_exe(a.bin / exe, d / exe)
    fetch_oidn("windows", d)
    copy_docs(d, a.manual)
    (d / "INSTALL.txt").write_bytes(install_text("INSTALL-windows.txt", ver).replace("\n", "\r\n").encode())
    target = out / f"{name}.zip"
    zip_dir(d, target)
    return target


def icns(png: Path, dest: Path) -> bool:
    """macOS icon from the 256 px PNG (needs sips and iconutil, i.e. macOS)."""
    if not (shutil.which("sips") and shutil.which("iconutil")):
        return False
    with tempfile.TemporaryDirectory() as t:
        iconset = Path(t) / "RMandelbulb3D.iconset"
        iconset.mkdir()
        for size in (16, 32, 64, 128, 256):
            for scale in (1, 2):
                px = size * scale
                if px > 256 and size != 256:
                    continue
                suffix = "" if scale == 1 else "@2x"
                subprocess.run(["sips", "-z", str(px), str(px), str(png), "--out", str(iconset / f"icon_{size}x{size}{suffix}.png")], check=True, capture_output=True)
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(dest)], check=True)
    return True


def macos(a, ver: str, out: Path, stage: Path) -> Path:
    arch = a.arch or "arm64"
    name = f"RMandelbulb3D-{ver}-macos-{arch}"
    d = stage / name
    app = d / "RMandelbulb3D.app" / "Contents"
    for exe in ("rmb-app", "rmb-cli"):
        copy_exe(a.bin / exe, app / "MacOS" / exe)
    oidn = stage / f"oidn-{arch}"
    fetch_oidn("macos-arm64" if arch == "arm64" else "macos-x86_64", oidn)
    shutil.copytree(oidn / "lib", app / "Frameworks", dirs_exist_ok=True)
    res = app / "Resources"
    copy_docs(res, a.manual)
    shutil.copytree(oidn / "oidn-license", res / "oidn-license", dirs_exist_ok=True)
    (app / "Info.plist").write_text((PKG / "macos" / "Info.plist").read_text().replace("@VERSION@", ver))
    icns(ROOT / "crates" / "rmb-app" / "assets" / "icon.png", res / "RMandelbulb3D.icns")
    (d / "INSTALL.txt").write_text(install_text("INSTALL-macos.txt", ver, arch))
    # Ad hoc signature: Apple silicon runs only signed code (no developer identity needed).
    if shutil.which("codesign"):
        subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(d / "RMandelbulb3D.app")], check=True)
    target = out / f"{name}.zip"
    if shutil.which("ditto"):
        subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(d), str(target)], check=True)
    else:
        zip_dir(d, target)
    return target


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("kind", choices=["linux", "appimage", "windows", "macos"])
    ap.add_argument("--bin", type=Path, default=ROOT / "target" / "release", help="folder with the built programs")
    ap.add_argument("--out", type=Path, default=ROOT / "dist")
    ap.add_argument("--arch", choices=["arm64", "x86_64"], help="macOS architecture")
    ap.add_argument("--manual", type=Path, help="the PDF manual to include")
    ap.add_argument("--appimagetool", type=Path)
    a = ap.parse_args()
    ver = version()
    if a.manual is None:
        default = ROOT / "docs" / "manual" / "build" / f"RMandelbulb3D-Manual-{ver}.pdf"
        a.manual = default if default.is_file() else None
    a.out.mkdir(parents=True, exist_ok=True)
    stage = a.out / "stage"
    shutil.rmtree(stage, ignore_errors=True)
    stage.mkdir(parents=True)
    target = {"linux": linux, "appimage": appimage, "windows": windows, "macos": macos}[a.kind](a, ver, a.out, stage)
    shutil.rmtree(stage, ignore_errors=True)
    print(f"wrote {target}")


if __name__ == "__main__":
    main()
