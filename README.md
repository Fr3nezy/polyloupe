<img src="assets/brand/polyloupe-lockup-white.svg" alt="Poly Loupe" height="64">

# Poly Loupe

The 3D viewer for people who make assets. Opens in a blink, moves like Blender (or Maya,
ZBrush, Unity...), shows what's really in the model. Native, free and open source.

- Opens glTF/GLB, FBX, OBJ (with MTL textures) and STL; files load on a background thread.
- Wireframe, Solid and Rendered shading, like Blender:
  - Solid: Studio, MatCap or Flat lighting; Material, Single, Random, Texture or Attribute color.
  - Texture pass viewer: Base Color, Roughness, Metallic, Normal, AO, Emission, Alpha, UV grid.
  - Rendered: PBR with image-based lighting, AgX. Blender's own HDRIs built in (Forest, the
    Material Preview default, Studio, Sunset), plus your `.hdr` / `.exr` library with a default.
- Click to select, outliner sidebar (N), hide/reveal, frame selected.
- Per-object texture channel view (Base Color, Roughness, Normal…) from the header's Channel picker.
- Animation: skinning, morph targets and node animation (glTF, FBX) with a Blender-style timeline.
- Explorer thumbnails for .glb .gltf .fbx .obj .stl .ply .3mf .dae, "Open with" and Default apps entries.
- Image export (F12): PNG at 1×/2×/4× the viewport, optionally transparent.
- English and Italian UI (follows Windows by default), Preferences window (Ctrl+,).
- Optional "Open files in the same window".
- Navigation presets: Blender, Maya · Cinema 4D · Substance, ZBrush, 3ds Max, Unity · Unreal
  (RMB + WASD fly), CAD (SolidWorks); picked on first run, changeable in Preferences.
- Textures an exporter referenced but never shipped are skipped (with a warning), so the model
  shows its plain material instead of pink.
- X-ray, infinite floor grid with metric units, navigation gizmo, statistics overlay.
- Renders only when something changes: no GPU usage while idle, no 60 FPS cap.

## Controls

Blender's by default (the mouse part changes with the navigation preset):

| Input | Action |
|---|---|
| MMB drag / Alt+LMB drag | Orbit |
| Shift+MMB drag | Pan |
| Wheel / Ctrl+MMB drag | Zoom |
| LMB / Shift+LMB | Select / extend selection |
| A / Alt+A | Select all / none |
| H / Shift+H / Alt+H | Hide selected / hide others / reveal |
| N | Sidebar (outliner, object info, texture maps) |
| Home / `.` | Frame all / frame selected |
| 1 / 3 / 7 (Ctrl for opposite) | Front / Right / Top view |
| 2 4 6 8 | Orbit in 15° steps |
| 5 | Perspective / orthographic |
| Z | Shading pie menu |
| Shift+Z | Toggle wireframe |
| Alt+Z | Toggle X-ray |
| Shift+Alt+Z | Toggle overlays |
| Space | Play / pause animation |
| ← / → (Shift for start/end) | Step frames |
| Ctrl+O | Open file |
| F12 | Export image |
| Ctrl+, | Preferences |
| F11 | Fullscreen |

Top-row digits work as numpad keys, so laptops without a numpad are covered.

## Build

Requires Rust (stable) and, on Windows, the MSVC build tools.

```bash
cargo run --release -- path/to/model.glb
```

## Installer and Explorer thumbnails

```bash
powershell -ExecutionPolicy Bypass -File scripts/build-installer.ps1
```

Builds everything and writes `target\installer\PolyLoupe-Setup-<version>.exe` (English or Italian
wizard, built with [Inno Setup](https://jrsoftware.org/isinfo.php)). It installs to
`Program Files`, registers the thumbnail handler, adds Poly Loupe to "Open with" and Default apps
for .glb .gltf .fbx .obj .stl .ply .3mf .dae, and offers to open Default apps at the end (Windows
doesn't let installers pick the default app themselves).

The thumbnail handler works like Blender's handler for .blend files: Windows runs it in its
isolated thumbnail process and hands it the file's bytes, and the DLL parses the model and renders
it on the CPU (no GPU, no helper process), in about 30 ms for a typical model. Files next to the
model are out of its reach there: an .obj renders without its .mtl, and a .gltf with external
buffers keeps the normal icon. `polyloupe --thumbnail model.glb out.png 256` renders exactly what
Explorer shows.

Without the installer:

```bash
powershell -ExecutionPolicy Bypass -File scripts/install-thumbnails.ps1 -AllUsers
```

Drop `-AllUsers` to install for the current user only (`%LOCALAPPDATA%\Programs\Poly Loupe`, no
admin), though Windows may take a while to notice a per-user handler; add `-Uninstall` to remove
either.

Explorer keeps the thumbnails it already made: clear "Thumbnails" in Disk Cleanup to see files you
browsed before an update with the new renderer.

## Screenshots from the command line

```bash
cargo run --release -- model.glb --capture out.png --size 1280x800 --shading rendered --view front
```

Flags: `--shading wireframe|solid|rendered`, `--lighting studio|matcap|flat`,
`--color material|single|random|texture|attribute`, `--pass basecolor|roughness|metallic|normal|ao|emission|alpha|uvgrid`,
`--matcap N`, `--env forest|studio|sunset|PATH.hdr|PATH.exr`, `--env-bg`, `--xray`, `--wire-overlay`,
`--no-grid`, `--no-outline`, `--view front|back|right|left|top|bottom`, `--ortho`,
`--popover shading|overlays|channel|preferences|welcome`, `--nav blender|maya|zbrush|max|gameengine|cad`, `--pie`, `--sidebar`, `--select N`, `--click X,Y`,
`--channel PASS` (on the selection), `--clip N`, `--frame N`, `--fps`, `--popover preferences`.
`--export OUT.png [--export-scale 1|2|4] [--transparent]` runs File > Export Image instead of a
window screenshot.

Capture runs never change your saved preferences.

See [PLAN.md](PLAN.md) for the roadmap.
