#!/usr/bin/env python3
"""Packs the Blender add-on as an extension zip (Blender 4.2 and later).

    tools/blender/build-extension.py -o dist [--wheel dist/doctor_toolkit-*.whl ...]

With wheels, they go into the package and the manifest lists them with the
platforms they are for: Blender installs them into its user extension
folder when the extension is installed (Edit > Preferences > Get
Extensions > Install from Disk). Without wheels, the add-on needs the
"Module folder" preference. The zip is also a plain add-on for "Install…"
of older Blender versions. Standard library only.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
SOURCE = os.path.join(HERE, "doctor_toolkit_blender")

# Wheel platform tag -> Blender platform.
PLATFORMS = [
    (re.compile(r"manylinux.*_x86_64|linux_x86_64"), "linux-x64"),
    (re.compile(r"manylinux.*_aarch64|linux_aarch64"), "linux-arm64"),
    (re.compile(r"win_amd64"), "windows-x64"),
    (re.compile(r"win_arm64"), "windows-arm64"),
    (re.compile(r"macosx_.*_(arm64|universal2)"), "macos-arm64"),
    (re.compile(r"macosx_.*_(x86_64|universal2)"), "macos-x64"),
]


def platforms_of(wheel: str) -> list[str]:
    tag = os.path.basename(wheel)[: -len(".whl")].split("-")[-1]
    found = [name for pattern, name in PLATFORMS if pattern.search(tag)]
    if not found:
        sys.exit(f"unknown platform of wheel {wheel}")
    return found


def manifest(wheels: list[str]) -> tuple[str, str]:
    """The manifest with the wheels and platforms; and the version."""
    with open(os.path.join(SOURCE, "blender_manifest.toml"), encoding="utf-8") as f:
        text = f.read()
    version = re.search(r'^version = "([^"]+)"', text, re.M).group(1)
    if wheels:
        platforms = sorted({p for w in wheels for p in platforms_of(w)})
        listed = ", ".join(f'"./wheels/{os.path.basename(w)}"' for w in wheels)
        extra = f"platforms = [{', '.join(repr(p).replace(chr(39), chr(34)) for p in platforms)}]\n"
        extra += f"wheels = [{listed}]\n"
        text = text.replace("\n[permissions]", "\n" + extra + "\n[permissions]", 1)
    return text, version


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("-o", "--output", required=True, help="folder for the zip")
    p.add_argument("--wheel", action="append", default=[], help="doctor_toolkit wheel to bundle")
    a = p.parse_args()
    text, version = manifest(a.wheel)
    suffix = "-".join(sorted({p for w in a.wheel for p in platforms_of(w)})) or "no-module"
    os.makedirs(a.output, exist_ok=True)
    out = os.path.join(a.output, f"doctor_toolkit_blender-{version}-{suffix}.zip")
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for name in sorted(os.listdir(SOURCE)):
            path = os.path.join(SOURCE, name)
            if name == "blender_manifest.toml":
                z.writestr(name, text)
            elif name.endswith(".py"):
                z.write(path, name)
        for wheel in a.wheel:
            z.write(wheel, f"wheels/{os.path.basename(wheel)}")
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
