"""Writes target/THIRD-PARTY-NOTICES.txt: the license texts of every crate and asset that ships
inside PolyLoupe's binaries, as their licenses ask for in binary distributions.

    python scripts/third-party-notices.py

Crates come from `cargo metadata` (normal and build dependencies of the workspace, Windows
target); identical license texts are printed once with the list of crates they cover.
"""

import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "unlicense")
ASSETS = [
    ("Space Grotesk font (assets/fonts)", "assets/fonts/OFL.txt"),
    ("Blender HDRIs: Forest, Studio, Sunset (assets/hdri)", "assets/hdri/LICENSE.txt"),
    ("ambientCG surface wear maps (assets/surface)", "assets/surface/LICENSE.txt"),
    # Built from source by the cadrum crate into target/occt (see .cargo/config.toml).
    ("Open CASCADE Technology 8.0.1 (polyloupe_step.dll): LGPL 2.1", "target/occt/LICENSE_LGPL_21.txt"),
    ("Open CASCADE Technology: LGPL exception", "target/occt/OCCT_LGPL_EXCEPTION.txt"),
]


def shipped_packages(meta):
    members = set(meta["workspace_members"])
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    seen, stack = set(), list(members)
    while stack:
        pkg = stack.pop()
        if pkg in seen:
            continue
        seen.add(pkg)
        for dep in nodes[pkg]["deps"]:
            if any(k["kind"] in (None, "build") for k in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    packages = {p["id"]: p for p in meta["packages"]}
    return sorted((packages[i] for i in seen - members), key=lambda p: (p["name"], p["version"]))


def license_texts(package):
    folder = os.path.dirname(package["manifest_path"])
    names = sorted(f for f in os.listdir(folder) if f.lower().startswith(LICENSE_PREFIXES))
    if package.get("license_file"):
        names.append(os.path.relpath(os.path.join(folder, package["license_file"]), folder))
    texts = []
    for name in dict.fromkeys(names):
        path = os.path.join(folder, name)
        if os.path.isfile(path):
            with open(path, encoding="utf-8", errors="replace") as f:
                texts.append(f.read().strip())
    return texts


def main():
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--filter-platform", "x86_64-pc-windows-msvc"],
            cwd=ROOT, check=True, capture_output=True, text=True, encoding="utf-8",
        ).stdout
    )
    by_text = {}
    missing = []
    for package in shipped_packages(meta):
        label = f"{package['name']} {package['version']} ({package.get('license') or 'see text'})"
        texts = license_texts(package)
        if not texts:
            missing.append(label)
        for text in texts:
            by_text.setdefault(text, []).append(label)

    out = [
        "PolyLoupe is free software under the GNU General Public License, version 3 or later",
        "(see LICENSE). It includes the third-party components below, under their own licenses.",
        "",
    ]
    for title, rel in ASSETS:
        with open(os.path.join(ROOT, rel), encoding="utf-8") as f:
            out += ["=" * 78, title, "=" * 78, "", f.read().strip(), ""]
    for text, labels in sorted(by_text.items(), key=lambda kv: kv[1][0]):
        out += ["=" * 78, *labels, "=" * 78, "", text, ""]
    if missing:
        out += ["=" * 78, "Crates without a license file in their package (license from their metadata;", "the standard texts of these licenses appear above):", "=" * 78, ""]
        out += missing
    target = os.path.join(ROOT, "target", "THIRD-PARTY-NOTICES.txt")
    os.makedirs(os.path.dirname(target), exist_ok=True)
    with open(target, "w", encoding="utf-8", newline="\r\n") as f:
        f.write("\n".join(out) + "\n")
    print(f"{target}: {len(by_text)} license texts, {len(missing)} crates without one")


if __name__ == "__main__":
    sys.exit(main())
