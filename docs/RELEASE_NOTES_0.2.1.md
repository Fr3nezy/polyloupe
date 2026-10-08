# PolyLoupe 0.2.1

GLB files compressed with meshoptimizer now open. Thanks to shinodem for the report and the file
([#2](https://github.com/Fr3nezy/polyloupe/issues/2)).

## Fixed

- **meshopt-compressed glTF/GLB** (`EXT_meshopt_compression`), as exported by Tripo and by
  gltfpack, failed with "missing binary portion of binary glTF". They're now decoded with the
  official meshoptimizer library, Explorer thumbnails included.
- **Quantized vertex data** (`KHR_mesh_quantization`): positions, normals, UVs and tangents stored
  as 8 or 16-bit integers are read correctly.

## Known limitations

- Draco-compressed meshes still open without their geometry.

## In breve (IT)

- I GLB compressi con meshopt (esportati da Tripo, gltfpack) ora si aprono, anche nelle miniature
  di Esplora file. Supportati anche i vertici quantizzati (`KHR_mesh_quantization`).
