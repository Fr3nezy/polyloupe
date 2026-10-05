# PolyLoupe 0.2.0

Two workspaces, STEP files, and a Move tool. This release turns PolyLoupe from a viewer for
3D art into a viewer for 3D parts as well.

## What's new

- **STEP / STP files** open through OpenCASCADE. The CAD kernel lives in its own DLL
  (`polyloupe_step.dll`), loaded only when a STEP file is opened, so startup and every other
  format are unaffected. Colors are kept; the meshed result is cached per file in
  `%LOCALAPPDATA%\PolyLoupe\cache\step`, so the second open is instant. STEP files also get
  Explorer thumbnails.
- **Manufacturing and 3D Art workspaces.** Manufacturing (STL, 3MF, STEP, PLY) shows millimeters
  and print tools; 3D Art (glTF, FBX, OBJ, DAE) keeps meters, textures, UVs and animation. The
  workspace follows the file type, and the new switch sits at the left of the viewport
  toolbar, with an icon and a name.
- **Plain plastic in Manufacturing.** Parts show as neutral satin plastic (light grey, roughness
  0.5, non-metallic) and ignore the file's materials and textures. Shading > Part Material
  changes the color, or switches to "File materials". 3D Art keeps each file's own materials.
- **Surface imperfection** (Shading > Part Material, default 25%): a subtle global relief and uneven gloss on the plastic, in millimeters and world space, so STL and STEP parts look less like a perfect CG surface. 0% turns it off; it applies to the plastic material only, not to "File materials" or 3D Art.
- **Print finishes** for quick renders: PLA, PETG, Silk PLA, resin, nylon SLS, metal SLM, with
  layer lines that follow the bed.
- **Move tool (W)** with a transform gizmo, Lay on face, Auto orient, Drop to bed, and Export
  Model to STL or 3MF. It works without a selection and shows the face to lay on.
- **Cleaner modes.** Texture and Attribute colors, the Materials tab, the channel strip and the
  UV pane belong to 3D Art only; Manufacturing no longer offers them.
- **Update check**: once a day PolyLoupe asks GitHub whether a newer release exists and shows a banner with a Download button (Preferences > Check for updates turns it off). Nothing is downloaded or installed by the app.
- **Side panel button** at the top right (tooltip: "Show/Hide the side panel (N)").
- Measure: Delete removes only the last measurement. Models whose buffers exceed 256 MB open.

## Notes

- STEP loading is dominated by OpenCASCADE building the exact geometry; the meshing already
  runs on all cores and the result is cached. Big assemblies can still take several seconds the
  first time.
- The old "filament color" and "model colors" options are replaced by Part Material; a color
  chosen in 0.1.x is not carried over.
- PolyLoupe is GPL-3.0-or-later; OpenCASCADE is LGPL 2.1 with its exception, and its notice is
  installed with the app.

## In breve (IT)

- **File STEP/STP** tramite OpenCASCADE, in una DLL caricata solo quando serve; risultato in
  cache, la seconda apertura è immediata. Miniature in Esplora file anche per STEP.
- **Due aree di lavoro**: Manifattura (STL, 3MF, STEP, PLY: millimetri e strumenti di stampa) e
  3D Art (glTF, FBX, OBJ, DAE: texture, UV, animazione). Il selettore è a sinistra della barra
  del viewport, con icona e nome.
- **Plastica neutra in Manifattura**: grigio chiaro satinato, ignora materiali e texture del file.
  In Shading > Materiale del pezzo puoi cambiare colore o usare i "Materiali del file". In 3D Art
  restano i materiali del file.
- **Imperfezioni superficie** (Shading > Materiale del pezzo, 25% di default): lieve rilievo e lucentezza non uniforme sulla plastica; 0% la spegne.
- **Finiture di stampa** (PLA, PETG, Silk PLA, resina, nylon, metallo) con layer che seguono il piano.
- **Strumento Sposta (W)** con gizmo, Appoggia su faccia, Orienta automaticamente, Export STL/3MF.
- **Modalità più pulite**: Texture, Attribute, tab Materiali, canali e UV solo in 3D Art.
- **Pulsante del pannello laterale** in alto a destra (tasto N).
