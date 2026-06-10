# sumaru Completed Roadmap

This is the completed-work ledger for `sumaru`. The active to-do plan lives in
[`ROADMAP.md`](ROADMAP.md).

## Bootstrap And Build

- [x] Created the `sumaru` Cargo package.
- [x] Added GIFTI support from `PennLINC/gifti-rs`.
- [x] Added NIFTI support from `Enet4/nifti-rs`.
- [x] Added Cargo aliases, README quickstart documentation, and the first
  staged roadmap.
- [x] Added `sumaru inspect` to prove the reader path.
- [x] Added `sumaru -i/--surface` to launch a native viewer.
- [x] Added Linux CircleCI build/test coverage as the minimal first CI path.
- [x] Added `cargo fmt-all`, `cargo check-all`, and `cargo test-all` workflow
  aliases.

## Canonical Data Model

- [x] Defined file-neutral `SurfaceMesh`, `Bounds`, and early overlay dataset
  types.
- [x] Converted GIFTI pointset and triangle arrays into `SurfaceMesh`.
- [x] Separated durable model types from render-prep types so mesh, dataset,
  overlay, ROI, and scene state can be used outside `wgpu`.
- [x] Expanded `SurfaceMesh` metadata toward SUMA's `SUMA_SurfaceObject`:
  stable id, label, source file, node count/dimension, embedding dimension,
  face dimension, side, group/subject label, state name, surface kind,
  anatomical-correct flag, and sphere radius/center where applicable.
- [x] Added explicit surface lineage: local domain parent, local curvature
  parent, domain grandparent, node parent, parent volume id, originator id, and
  domain-kinship checks for topology, geometry, standard-mesh compatibility,
  and mapping.
- [x] Added `SurfaceDomain` for topology/geometry domains, optional node ids,
  row-index-to-node-index mapping, sorted-node metadata, and triangle topology
  independent of one coordinate set.
- [x] Defined `Dataset` as a domain-attached table with dense/sparse rows,
  typed columns, labels, roles, ranges, units, stat metadata, and parent ids.
- [x] Defined `Overlay` as display state layered on top of `Dataset`: selected
  intensity/threshold/brightness columns, colormap, intensity range, threshold
  mode/range, masking/clipping, symmetric range, opacity, plane order,
  foreground/background role, and color cache.
- [x] Defined `LabelTable` and `ColorMap` models for integer label datasets,
  continuous maps, RGBA label colors, and imported GIFTI/FreeSurfer label
  tables.
- [x] Defined `Roi`/`RoiDatum` for surface regions from drawing, imported
  `.niml.roi`, datasets, thresholded overlays, or future tools.
- [x] Added starter fixture-backed/local-reference tests for representative
  `.gii`, `.gii.dset`, `.niml.roi`, and `.spec` files, plus focused malformed
  surface/dataset tests.

## Surface Geometry Core

- [x] Computed bounding boxes, centers, and radius for loaded surfaces.
- [x] Computed vertex normals for triangle meshes.
- [x] Computed face normals, polygon areas, node areas, and total mesh area.
- [x] Detected normal direction and triangle winding, with utilities to flip or
  orient triangles consistently.
- [x] Built topology caches analogous to SUMA's `MF`, `FN`, and `EL`: member
  faces, first-order neighbors, neighbor distances, unique edge list,
  edge-to-host-face mapping, and boundary edges/triangles.
- [x] Added mesh validation for empty geometry, duplicate or degenerate
  triangles, non-manifold edges, invalid dimensions, disconnected components,
  boundary edges/loops, and winding diagnostics.
- [x] Added robust node/row lookup helpers so sparse datasets, overlays, ROI
  paths, and full-node arrays agree on node ids versus row indices.
- [x] Implemented masks and patches: node masks, face masks, patch extraction,
  patch bounds, and mask composition.
- [x] Added ROI geometry operations: node paths, edge paths, triangle paths,
  contour edges, fill-to-mask behavior, path lengths, and basic geodesic
  distance reporting.
- [x] Implemented Dijkstra shortest path, k-ring neighborhoods,
  distance-limited neighborhoods, and approximate spherical neighborhoods.
- [x] Added curvature and shape metrics including convexity and
  curvature-style scalar fields.
- [x] Added nearest-neighbor smoothing, weighted smoothing, mask-respecting
  smoothing, and vertex smoothing.
- [x] Added coordinate-space transforms and affine composition for load-time and
  interactive/display transforms.
- [x] Added surface-volume geometry primitives: voxel/index/world conversion,
  nearest surface node to voxel/world position, surface voxelization,
  voxel-to-surface distance, and volume-to-surface sampling hooks.
- [x] Added clipping and intersection geometry: plane/surface intersections,
  clipped contours, visible patch extraction, and render masks.
- [x] Added surface-to-surface mapping support: same-topology transfer,
  nearest-neighbor transfer, barycentric/triangle transfer, and domain-kinship
  checks before values move between surfaces.

## AFNI/SUMA File And Session Support

- [x] Added the first native NIML I/O foundation: ASCII NIML element
  parser/writer, `.niml.dset` and `.niml.roi` payload extraction, and reference
  checks against C, MATLAB, and SUMAvista assumptions.
- [x] Extended NIML I/O with fixed-width binary numeric payloads, mixed
  numeric/string row tables, and canonical `Dataset` conversion.
- [x] Parsed `.spec` files into surface groups, states, hemispheres, labels,
  topology/coordinate file pairs, local domain parents, curvature parents, and
  surface-volume parent references.
- [x] Parsed `.niml.dset` and AFNI-converted `.gii.dset` into canonical
  `Dataset` values with column labels, roles, typed values, node-index columns,
  ranges, stat metadata, and parent/domain ids.
- [x] Parsed `.niml.roi` files into ROI data used by the viewer.
- [x] Loaded single-hemisphere `.spec` scenes and switched the active surface
  with `.` and `,`.
- [x] Loaded `both` specs as paired left/right surfaces rendered together.
- [x] Added `-spec/--spec` and `-sv/--sv` launch arguments for spec sessions.
- [x] Added strict on-demand spec loading by default and optional `--preload`
  for background loading.
- [x] Automatically paired opposite-hemisphere overlays where matching files are
  available.

## Viewer, Controllers, And Interaction

- [x] Built a desktop viewer using `winit` for windowing and `wgpu` for
  rendering.
- [x] Rendered a single GIFTI mesh in a native window.
- [x] Uploaded vertices, normals, colors, and triangle indices to `wgpu`.
- [x] Added depth testing and basic directional lighting.
- [x] Normalized loaded surfaces for the first viewer camera.
- [x] Added mouse orbit control, scroll-wheel zoom, Space camera reset, and
  `C` camera-mode switching between orbit and turntable.
- [x] Added temporary in-view labels for camera mode and acorn-open percentage.
- [x] Added Option/Alt + arrow preset orientations.
- [x] Added `F5` background switching between black and white.
- [x] Added first scalar overlay coloring from a GIFTI data array.
- [x] Introduced `egui`, `egui-winit`, and `egui-wgpu` for native controller
  panels.
- [x] Added a first viewer workbench for loading surfaces and overlays, showing
  errors, displaying scene stats, and driving camera/background controls.
- [x] Moved the workbench into a separate native controls window so it no longer
  consumes viewport space or intercepts surface clicks.
- [x] Added native file picker support for surfaces, overlays, specs, surface
  volumes, and ROI files.
- [x] Added selectable color maps, threshold controls, p-value/stat conversion,
  symmetric ranges, dimming, and opacity controls for scalar overlays.
- [x] Added first right-click node/triangle inspection with surface coordinates,
  overlay values, and threshold values.
- [x] Added persistent node/triangle selection highlighting, crosshair state,
  and crosshair display.
- [x] Added screenshot export with `r` and montage export with `R`.
- [x] Added paired-hemisphere screenshot montage behavior for closed top,
  closed bottom, open medial-in, and open outer-out views.
- [x] Added menu entries for viewer actions and controller windows.
- [x] Added keyboard shortcuts for showing/hiding surface and ROI controllers.

## ROI Workflows

- [x] Loaded and displayed `.niml.roi` regions over the surface.
- [x] Added a separate ROI controller window.
- [x] Added ROI drawing, joining, filling, undo/redo, finalizing, editing, and
  deleting individual ROI slots.
- [x] Added multi-ROI controller behavior with separate ROI slots and a
  `Save All` path for combined ROI files.
- [x] Saved individual ROI files and combined multi-ROI `.niml.roi` files.
- [x] Matched ROI fill colors to ROI values closely enough for SUMA round-trip
  checks.
- [x] Confirmed outside-fill behavior and SUMA display round-trips on local
  examples.

## Multi-Surface SUMA Workflows

- [x] Added multi-surface scenes for pial, smoothwm, inflated, sphere, and
  registered template states.
- [x] Added pial/inflated/sphere/spec-state switching.
- [x] Added an active-surface controller dropdown for direct state selection.
- [x] Added closed/acorn paired-hemisphere view presets.
- [x] Added Control-drag controls for paired-hemisphere opening angle and
  hemisphere gap.
- [x] Added signed acorn opening, including negative opening direction.
- [x] Added `[` and `]` to show/hide left and right hemispheres.
- [x] Added curvature/convexity-style gray surface shading compatible with
  common SUMA display expectations.

## Rendering Performance Wins

- [x] Cached geometry-derived scene stats per surface id so recoloring no
  longer recomputes whole-mesh topology.
- [x] Computed overlay color caches once per appearance change instead of
  building them once with defaults and again with real settings.
- [x] Drew `both`-hemisphere scenes as two resident per-hemisphere GPU mesh
  instances instead of one rebuilt composite mesh.
- [x] Updated acorn opening/closing with per-hemisphere model matrices so open
  angle, signed direction, gap, and visibility update by writing small uniforms
  and changing the draw list.
- [x] Made picking layout-aware for resident hemispheres by transforming the
  pick ray into each hemisphere's current model matrix.
- [x] Added in-view acorn feedback for signed open percentage while
  Control-dragging.
