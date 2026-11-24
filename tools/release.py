#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Versions and releases of RMandelbulb3D.

Versions are MAJOR.FEATURE.FIX: MAJOR for big changes (files or the network protocol older
versions cannot read), FEATURE for new features, FIX for bug fixes and small improvements.
The version lives in Cargo.toml ([workspace.package] version); every crate uses it.

    python3 tools/release.py fix            # 1.0.0 -> 1.0.1
    python3 tools/release.py feature        # 1.0.1 -> 1.1.0
    python3 tools/release.py major          # 1.1.0 -> 2.0.0
    python3 tools/release.py 1.2.3          # an exact version
    ... --push                              # also push the commit and the tag

A release moves the entries under "## [Unreleased]" in CHANGELOG.md into a new section
"## [x.y.z] - date", sets the version, commits "Release x.y.z" and tags vx.y.z. Pushing the
tag starts .github/workflows/release.yml, which builds the packages and publishes the GitHub
release with that changelog section as its notes.

    python3 tools/release.py --print-version     # the current version
    python3 tools/release.py --notes 1.2.3       # the changelog section of 1.2.3
"""

import argparse
import datetime
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGO = ROOT / "Cargo.toml"
CHANGELOG = ROOT / "CHANGELOG.md"
VERSION_RE = re.compile(r'(\[workspace\.package\][^\[]*?version\s*=\s*")([^"]+)(")', re.S)


def current() -> str:
    m = VERSION_RE.search(CARGO.read_text())
    if not m:
        sys.exit("no version in Cargo.toml [workspace.package]")
    return m.group(2)


def parse(v: str) -> tuple[int, int, int]:
    m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", v)
    if not m:
        sys.exit(f"not a version MAJOR.FEATURE.FIX: {v}")
    return int(m.group(1)), int(m.group(2)), int(m.group(3))


def bumped(v: str, how: str) -> str:
    major, feature, fix = parse(v)
    if how == "major":
        return f"{major + 1}.0.0"
    if how == "feature":
        return f"{major}.{feature + 1}.0"
    if how == "fix":
        return f"{major}.{feature}.{fix + 1}"
    parse(how)
    return how


def notes(v: str) -> str:
    """The body of the changelog section of version `v`."""
    text = CHANGELOG.read_text()
    m = re.search(rf"^## \[{re.escape(v)}\][^\n]*\n(.*?)(?=^## \[|\Z)", text, re.S | re.M)
    if not m:
        sys.exit(f"CHANGELOG.md has no section '## [{v}]'")
    return m.group(1).strip() + "\n"


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


def release(new: str, push: bool) -> None:
    old = current()
    if parse(new) <= parse(old):
        sys.exit(f"{new} is not newer than {old}")
    if git("status", "--porcelain", "--untracked-files=no").strip():
        sys.exit("commit or stash your changes first")
    if git("tag", "--list", f"v{new}").strip():
        sys.exit(f"the tag v{new} exists already")
    text = CHANGELOG.read_text()
    m = re.search(r"^## \[Unreleased\][^\n]*\n(.*?)(?=^## \[|\Z)", text, re.S | re.M)
    if not m or not m.group(1).strip():
        sys.exit("CHANGELOG.md: write the changes under '## [Unreleased]' first")
    today = datetime.date.today().isoformat()
    section = f"## [Unreleased]\n\n## [{new}] - {today}\n\n{m.group(1).strip()}\n\n"
    CHANGELOG.write_text(text[: m.start()] + section + text[m.end():])
    CARGO.write_text(VERSION_RE.sub(lambda m: m.group(1) + new + m.group(3), CARGO.read_text(), count=1))
    # Cargo.lock carries the crates' versions.
    subprocess.run(["cargo", "update", "--workspace", "--offline"], cwd=ROOT, check=True)
    git("add", "Cargo.toml", "Cargo.lock", "CHANGELOG.md")
    git("commit", "-m", f"Release {new}")
    git("tag", "-a", f"v{new}", "-m", f"RMandelbulb3D {new}")
    print(f"{old} -> {new}: committed and tagged v{new}")
    if push:
        git("push", "origin", "HEAD")
        git("push", "origin", f"v{new}")
        print("pushed; the release workflow builds and publishes the packages")
    else:
        print(f"publish with: git push origin HEAD && git push origin v{new}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("version", nargs="?", help="fix, feature, major or an exact version x.y.z")
    ap.add_argument("--push", action="store_true", help="push the commit and the tag")
    ap.add_argument("--print-version", action="store_true")
    ap.add_argument("--notes", metavar="VERSION")
    a = ap.parse_args()
    if a.print_version:
        print(current())
    elif a.notes:
        sys.stdout.write(notes(a.notes))
    elif a.version:
        release(bumped(current(), a.version), a.push)
    else:
        ap.print_help()


if __name__ == "__main__":
    main()
