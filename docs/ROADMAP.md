# sumaru Roadmap

This is a working plan for growing `sumaru` from a reader-backed CLI into a
SUMA-class surface viewer and analysis environment.

## Principle

Build a Rust-native surface/volume model first, then hang CLIs and viewers from
that model. Avoid copying AFNI's implementation; use public file formats,
behavioral tests, and user-visible compatibility as the guide.

Once the model is rich enough to support a real task, prefer building the
thinnest end-to-end slice that is actually usable (load → view → adjust →
export), dogfood it, and let real use pull depth into the model — rather than
pre-building model depth on spec. The "Daily Driver" phase below exists for
exactly this: it pulls a small set of items forward out of later phases so the
tool becomes usable day-to-day before those phases are finished in order.

AFNI/SUMA reference points for the early model work include
`SUMA_define.h` (`SUMA_SurfaceObject`, `SUMA_OVERLAYS`, `SUMA_VOLPAR`, ROI
records), `suma_datasets.h`, `surface_domain.h`, `SUMA_GeomComp.h`, and
`SUMA_Color.h`.

## Phase: Daily Driver (current focus)

The near-term goal: make `sumaru` usable for the two most common day-to-day
SUMA tasks — viewing statistical overlays and drawing/editing ROIs — on real
AFNI data (`.niml.dset`, `.spec`). Items here are pulled forward from Phases 3,
4, and 5; the canonical descriptions still live in those phases. Much of this is
wiring already-built model code (the `Overlay`/`ColorMap` model, the `Roi`
model, and the Phase 2 path/fill/geodesic geometry) into the viewer, not new
foundation.

Deliberately *not* in scope yet: a standalone controller/command-routing layer
(wire controls directly to viewer state for now and extract a controller only
once scripts or AFNI become a second consumer), atlas/label coloring, live AFNI
interop, and volume rendering.

### Tier 1 — Statistical overlay loop

- [x] Parse `.niml.dset` and AFNI-converted `.gii.dset` into the canonical
  `Dataset` (from Phase 3) so real AFNI overlays load, not just one-column
  GIFTI shape files.
- [x] Wire overlay display controls into the viewer using the existing
  `color.rs`/`overlay.rs` model: selectable colormap, intensity range,
  threshold mode/range, symmetric range, and opacity (from Phase 4).
- [x] Add screenshot export (from Phase 4) for figures and QA.

### Tier 2 — ROI loop

- [ ] Add persistent node/triangle selection highlighting and crosshair state
  (from Phase 4) on top of the existing right-click pick.
- [ ] Load and display `.niml.roi` regions (from Phases 3/5), including a
  second color layer composited over the scalar overlay.
- [ ] Drawing, editing, undo/redo, and `.niml.roi` save (from Phase 5),
  exercising the Phase 2 node/edge/triangle paths, fill-to-mask, and geodesics.

### Tier 3 — Sessions and anatomy

- [ ] Parse `.spec` files and load multi-surface scenes with visibility toggles
  and pial/inflated/sphere state switching (from Phases 3/5), with required
  surface-volume parent context for spec launches.
  - [x] Parse single- and both-hemisphere `.spec` files into surface entries,
    states, groups, hemisphere labels, and SUMA parent fields.
  - [x] Add `-spec/--spec` plus required `-sv/--sv` launch arguments and store
    the surface-volume parent on loaded spec surfaces.
  - [x] Load single-hemisphere spec scenes and switch the active surface with
    `.` and `,`.
  - [x] Load `both` specs as paired left/right surfaces rendered together.
  - [x] Add closed/acorn paired-hemisphere view presets and Control-drag
    controls for opening angle and hemisphere gap.
  - [x] Load the first spec state immediately and use strict on-demand loading
    by default, with `--preload` for background loading of remaining spec
    surfaces.
  - [x] Update screenshot montage behavior for paired-hemisphere scenes.

### Continuous (cheap insurance, do early)

- [ ] Add CircleCI building and testing on macOS and Linux (a minimal version
  of the Phase 8 CI item), since cross-platform support is a core goal.

## Phase: Performance (viewer interaction)

Keeping overlay thresholding and surface/hemisphere toggling responsive on large
standard-mesh, both-hemisphere scenes. Guiding principle: **build durable data
once, then express interaction as cheap state on top** (recolor, transforms,
flags, draw-call selection, GPU uniforms) instead of rebuilding the canonical
model or re-uploading large per-vertex data on every change.

Always **measure before optimizing** — time the recolor, upload, and rebuild
paths on a real ~275k-vertex scene to confirm where the cost is before moving to
the next rung.

### Recolor path hygiene (done)

- [x] Cache geometry-derived scene stats (`winding_report`/`total_area`) per
  surface id so a recolor no longer recomputes whole-mesh topology; only the
  cheap overlay range is refreshed.
- [x] Compute the overlay color cache exactly once per appearance change
  (`Overlay::without_color_cache` + a single `rebuild_color_cache`) instead of
  building it once with default settings and again with the real settings.

### Thresholding — Level 2: separate color buffer

- [ ] Split the interleaved vertex buffer into a position+normal buffer
  (uploaded once) and a separate color buffer. A threshold/colormap change then
  re-uploads only ~4·N color bytes and never touches geometry. Split
  `upload_surface_buffers` into a geometry upload (on mesh/visibility change)
  and a color upload (on recolor). Still does an O(n) CPU recolor per change,
  but ~10× less upload; often enough on its own.

### Thresholding — Level 3: GPU/shader colormapping

- [ ] Upload the raw per-vertex scalar columns (intensity, threshold, optional
  brightness) once; put threshold/range/opacity/colormap-id in a small uniform;
  colormap in the shader against a 1-D LUT texture. A threshold change becomes a
  tiny uniform write — no CPU recolor, no per-vertex upload — so slider dragging
  is immediate even on large meshes. Naturally supports SUMA's independent
  intensity/threshold sub-bricks. Costs: porting threshold semantics (symmetric
  range, abs, hide-failed vs dim, p-value→stat, NaN) to WGSL, adding pixel/render
  tests since that logic leaves unit-testable Rust, and a node→vertex expansion
  for sparse overlays. The Level 2 multi-buffer/bind-group plumbing carries into
  this.

### Toggling and hemisphere layout — durable mesh residency

- [ ] Keep the durable `SurfaceMesh` immutable after load; never rebuild it for a
  view change. Express the `both`-hemisphere open-angle/gap layout as per-
  hemisphere transforms (model matrices) rather than baking transformed vertex
  positions into a rebuilt merged mesh, and make picking layout-aware (transform
  the pick ray) so layout changes never rebuild the durable mesh. Currently the
  open-angle drag is debounced (render-only per frame, durable rebuild once on
  release for picking correctness); this removes that release-time rebuild too.
- [ ] Keep each surface's geometry resident on the GPU and toggle visibility via
  draw-call selection instead of rebuilding/re-uploading filtered geometry, so
  hemisphere and state switches are a draw-list change rather than an upload.

## Phase 0: Bootstrap

- [x] Create the `sumaru` Cargo package.
- [x] Pull in GIFTI support from `PennLINC/gifti-rs`.
- [x] Pull in NIFTI support from `Enet4/nifti-rs`.
- [x] Add Cargo aliases, README quickstart, and this staged roadmap.
- [x] Add a first `sumaru inspect` command to prove the reader path works.
- [x] Add a first `sumaru -i/--surface` viewer path to prove native windowing
  and GPU rendering work.

## Phase 1: Canonical Data Model

- [x] Define the first file-neutral `SurfaceMesh`, `Bounds`, and
  `OverlayDataset` types needed by the prototype viewer.
- [x] Convert GIFTI pointset/triangle arrays into `SurfaceMesh`.
- [x] Separate durable model types from render-prep types so mesh, dataset,
  overlay, and scene state can be used by CLIs, tests, and future UIs without
  depending on `wgpu`.
- [x] Expand `SurfaceMesh` metadata toward SUMA's `SUMA_SurfaceObject`: stable
  id, label, source file, node count/dimension, embedding dimension, face
  dimension, side, group/subject label, state name, surface kind
  (`pial`, `smoothwm`, `inflated`, `sphere`, etc.), anatomical-correct flag,
  and sphere radius/center when applicable.
- [x] Add explicit surface lineage: local domain parent, local curvature
  parent, domain grandparent, node parent, parent volume id, originator id, and
  enough domain-kinship information to know when two surfaces share topology,
  share geometry, can use explicit standard/template node-count compatibility,
  or need mapping.
- [x] Add a `SurfaceDomain` concept for topology/geometry domains: optional
  node IDs, row-index-to-node-index mapping, sorted-node metadata, and
  triangle topology independent of any one coordinate set.
- [x] Define `Dataset` as a domain-attached table, not just one vector:
  dataset kind, domain id, row count, optional sparse node index column, typed
  data columns, column labels, column roles, ranges, units, and parent ids.
- [x] Define `Overlay` as display state layered on top of a `Dataset`: selected
  intensity/threshold/brightness columns, colormap, intensity range,
  threshold mode/range, masking/clipping, symmetric range, opacity, plane order,
  foreground/background role, and per-node color cache.
- [x] Define `LabelTable` and `ColorMap` models for integer label datasets,
  continuous maps, RGBA label colors, and imported GIFTI/FreeSurfer label
  tables.
- [x] Define `Roi`/`RoiDatum` for surface regions from drawing, imported
  `.niml.roi`, datasets, thresholded overlays, or future tools: parent surface
  id, side, label, integer label, fill/edge colors, edge thickness, draw
  status, source/provenance, and optional stroke history as node/triangle
  paths.
- [x] Add starter fixture-backed/local-reference tests for representative
  `.gii`, `.gii.dset`, `.niml.roi`, and `.spec` files, with malformed
  surface/dataset behavior covered by focused unit tests. Use
  AFNI/nibabel/Python readers as reference tools for future test expectations,
  not runtime dependencies.
  - Why: Neuroimaging files have many edge cases, and fixtures keep reader
    behavior from drifting silently.

## Phase 2: Surface Geometry Core

- [x] Compute bounding boxes, centers, and radius for loaded surfaces.
- [x] Compute vertex normals for triangle meshes.
- [x] Compute face normals, polygon areas, node areas, and total mesh area.
  - Why: These are basic mesh facts needed for lighting, validation, smoothing,
    statistics, and quality checks.
- [x] Detect normal direction and triangle winding; add utilities to flip or
  orient triangles consistently when source files disagree.
  - Why: Some files disagree about triangle order, and wrong winding makes
    lighting, picking, and inside/outside tests misleading.
- [x] Build topology caches analogous to SUMA's `MF`, `FN`, and `EL`: per-node
  member faces, first-order neighbors, neighbor distances, unique edge list,
  edge-to-host-face mapping, and boundary edges/triangles.
  - Why: Many surface operations need fast neighbor and edge lookup; rebuilding
    those lists every time would be slow.
- [x] Validate meshes beyond bounds checks: empty geometry, duplicate or
  degenerate triangles, non-manifold edges, invalid polygon dimensions where
  representable, disconnected components, boundary edges/loops, and winding
  diagnostics.
  - Why: Bad meshes should fail early with useful errors instead of producing
    strange overlays or viewer behavior later.
- [x] Add robust node/row lookup helpers so sparse datasets, overlays, ROI
  paths, and full-node arrays can all agree on node IDs versus row indices.
  - Why: Sparse data often says "this row is node 123"; we need that mapping to
    avoid coloring or editing the wrong node.
- [x] Implement masks and patches: node masks, face masks derived from node
  masks, patch extraction, patch bounds, and mask composition.
  - Why: Masks and patches are the practical building blocks for ROIs,
    thresholded views, focused analysis, and exports.
- [x] Add geometry operations for ROI workflows: node paths, edge paths,
  triangle paths, contour edges, fill-to-mask behavior, path lengths, and basic
  geodesic distance reporting.
  - Why: Drawing and editing ROIs depends on paths over the mesh, not just
    freehand screen coordinates.
- [x] Implement graph/geodesic operations used by SUMA workflows: Dijkstra
  shortest path, k-ring neighborhoods, distance-limited neighborhoods, and
  approximate spherical neighborhoods for fast region queries.
  - Why: Users often ask questions in surface distance, and Euclidean distance
    through the brain is the wrong measure.
- [x] Add curvature and shape metrics needed for common overlays: convexity,
  curvature-style scalar fields, and parent-aware shape data.
  - Why: Curvature-like maps are standard context for seeing folds, sulci, and
    anatomy on cortical surfaces.
- [x] Add first smoothing primitives for surface data and geometry:
  nearest-neighbor smoothing, weighted smoothing, mask-respecting smoothing,
  and vertex smoothing.
  - Why: Smoothing is a common preprocessing and visualization step, and doing
    it on the surface preserves cortical topology.
- [x] Add coordinate-space transforms and affine composition for surface
  geometry, including load-time transforms and interactive/display transforms.
  - Why: Surfaces, volumes, templates, and viewers need a shared way to move
    between coordinate spaces.
- [x] Add surface-volume geometry primitives needed later by AFNI interop and
  volume rendering: voxel/index/world conversion, nearest surface node to
  voxel/world position, surface voxelization, voxel-to-surface distance, and
  volume-to-surface sampling hooks.
  - Why: Surface and volume data often need to talk to each other, especially
    for AFNI-style workflows.
- [x] Add clipping and intersection geometry: plane/surface intersections,
  clipped contours, visible patch extraction, and screenshot/export-friendly
  render masks.
  - Why: Clipping and cuts make dense surfaces easier to inspect and export.
- [x] Add surface-to-surface mapping support: same-topology transfer,
  nearest-neighbor transfer, barycentric/triangle transfer, and domain-kinship
  checks before values move between surfaces.
  - Why: Data often needs to move between pial, inflated, sphere, native, and
    standard surfaces without unsafe assumptions.

## Phase 3: SUMA Compatibility Layer

> Top priority (pulled into Daily Driver): `.niml.dset` → `Dataset`, `.spec`
> parsing, and `.niml.roi` reading.

- [ ] Inventory the SUMA file formats and workflows we want first: `.spec`,
  GIFTI surfaces, `.niml.dset`, `.niml.roi`, labels, overlays, and AFNI
  session coordination.
  - Why: A clear inventory keeps compatibility work focused on real workflows
    instead of chasing every historical format at once.
- [ ] Implement native Rust parsers in priority order, starting with
  `.niml.dset` and `.niml.roi`, with round-trip or golden-output tests.
  - Why: Native parsers make `sumaru` portable, testable, and usable without a
    Python or AFNI runtime dependency.
- [x] Add the first native NIML I/O foundation: ASCII NIML element
  parser/writer, `.niml.dset`/`.niml.roi` payload extraction, and reference
  checks that encode the shared C, MATLAB, and SUMAvista Python layout
  assumptions.
- [x] Extend NIML I/O with fixed-width binary numeric payloads,
  mixed numeric/string row tables, and conversion from `.niml.dset` payloads
  into canonical `Dataset` values.
- [x] Parse `.spec` files into surface groups, states, hemispheres, labels,
  topology/coordinate file pairs, local domain parents, curvature parents, and
  surface-volume parent references.
  - Why: `.spec` files describe how a SUMA scene fits together, so they are the
    bridge from single-file loading to real sessions.
- [x] Parse `.niml.dset` and AFNI-converted `.gii.dset` into the canonical
  `Dataset` model with column labels, column roles, typed values, node-index
  columns, ranges, stat metadata, and parent/domain ids.
  - Why: AFNI datasets can arrive as NIML or GIFTI, and both carry the rich
    table structure that overlays, thresholds, and statistics need.
- [ ] Parse `.niml.roi` into `Roi`/`RoiDatum` with stroke history, node
  paths, triangle paths, labels, colors, and parent-surface metadata.
  - Why: Reading ROI files lets users bring existing SUMA annotations into
    `sumaru` without redrawing them.
- [ ] Add AFNI `.HEAD/.BRIK` metadata support when the shared `VolumeSpace`
  model is ready enough to represent AFNI orientation and warp attributes.
  - Why: AFNI volumes are common in SUMA workflows, but they should wait until
    the volume coordinate model can represent them correctly.
- [ ] Incubate AFNI/SUMA readers inside this workspace first, then split them
  into reusable Rust crates once the APIs and fixture coverage stabilize.
  - Why: Keeping readers local at first lets the design change quickly before
    we promise a public crate API.
- [ ] Expand the compatibility fixture corpus with more real AFNI/SUMA examples:
  dense and sparse `.niml.dset`, binary NIML variants, label tables,
  malformed files, multi-surface `.spec` sessions, and small shareable golden
  summaries generated from AFNI/nibabel/SUMAvista reference readers.
  - Why: The starter local files prove the pipeline, but broader data is what
    will catch format edge cases before users do.
- [ ] Keep compatibility code at the edges so the internal model remains clean.
  - Why: The core model should describe surfaces and datasets, not every quirk
    of every file format.

## Phase 4: Rendering Prototype

> Top priority (pulled into Daily Driver): overlay colormap/range/threshold/
> symmetric-range/opacity controls, persistent selection + crosshair, and
> screenshot export.

- [x] Start with a desktop viewer using `winit` for windowing and `wgpu` for
  rendering.
- [x] Render a single GIFTI mesh in a native window.
- [x] Upload vertices, normals, colors, and triangle indices to `wgpu`.
- [x] Add depth testing and basic directional lighting.
- [x] Normalize loaded surfaces for the first viewer camera.
- [x] Add mouse orbit camera control.
- [x] Add scroll-wheel zoom.
- [x] Add Space camera reset.
- [x] Add `C` camera-mode switching between orbit and turntable.
- [x] Add a temporary on-screen camera-mode label.
- [x] Add Option/Alt + arrow preset orientations.
- [x] Add `F5` background switching between black and white.
- [x] Add first scalar overlay coloring from a GIFTI data array.
- [x] Introduce `egui` with `egui-winit`/`egui-wgpu` for the first native
  viewer control panel.
  - Why: Panels are easier and more consistent with a real UI toolkit than
    with hand-drawn overlay text.
- [x] Add a first in-viewer workbench for loading surface and overlay paths,
  showing load errors, displaying scene stats, and driving camera/background
  controls.
  - Why: Faster visual testing makes it much easier to evaluate real surfaces
    and overlays while the deeper SUMA features are still being built.
- [x] Move the first workbench into a separate native controls window so it no
  longer consumes surface viewport space or intercepts surface clicks.
  - Why: Surface inspection needs as much uninterrupted viewport area as
    possible, while controls should remain available beside it.
- [ ] Move camera, background, overlay, and selection settings into a shared
  controller/command state instead of leaving them as viewer-only state.
  - Why: Shared command state lets keys, panels, scripts, and AFNI messages
    control the same viewer behavior.
- [ ] Add a controller layer for future UI panels and command routing before
  adding richer controls.
  - Why: A controller layer keeps interaction logic out of the renderer and
    makes future UI work less tangled.
- [ ] Define shared interaction state: selected node, selected face, crosshair
  location, current surface/object id, and pick results that can be driven from
  either keyboard shortcuts or a future controller UI.
  - Why: Keyboard shortcuts, mouse picking, panels, and AFNI messages should all
    change the same state instead of fighting each other, so this belongs near
    the controller and `egui` work rather than the file/model foundation.
- [ ] Split the first `egui` panel into controller-backed widgets once the
  command state exists.
  - Why: The current panel is useful for testing, but richer controls should
    share commands with shortcuts, scripts, and AFNI messages instead of
    calling viewer methods directly.
  - Reference: `docs/EGUI_CONTROLLER_REFERENCE.md` preserves the first surface
    controller mockup and implementation notes.
- [x] Add native file picker support for loading surfaces, overlays, specs, and
  surface volumes from the first viewer workbench.
  - Why: File dialogs make visual testing faster while exact fixture paths and
    scripted workflows remain command-line workflows.
- [ ] Add recent-file support if repeated manual loading becomes annoying.
  - Why: File dialogs are simple and reliable for testing, but repeated manual
    loading will eventually want remembered recent files and working folders.
- [ ] Add label-table-aware coloring.
  - Why: Label datasets should display named regions with their intended colors.
- [x] Add selectable color maps.
  - Why: Different scalar data needs different visual encodings, and users need
    control over that choice.
- [ ] Add threshold, clipping, opacity, and symmetric-range controls.
  - Why: These controls are how users turn dense scalar maps into interpretable
    surface overlays.
- [ ] Add multiple overlay planes and explicit foreground/background ordering.
  - Why: SUMA users often compare several datasets at once, and layer order
    should be predictable.
- [x] Add first right-click node/triangle inspection with scalar overlay
  values.
  - Why: Users need to click the surface and know exactly which node or face
    they are inspecting.
- [ ] Add persistent node/triangle selection highlighting.
  - Why: Inspection answers what is under the cursor; selection should also
    remember and visually mark the active node or face for later commands.
- [ ] Add crosshair state and display.
  - Why: Crosshairs link the viewer to coordinates, selections, and later AFNI
    or volume views.
- [x] Add screenshot export.
  - Why: Users need a direct way to save figures for notes, QA, and papers.
- [ ] Add viewer tests or screenshot/pixel checks for nonblank rendering once
  automated graphics verification is practical.
  - Why: Rendering bugs are easy to miss in code review, so pixel checks give
    us confidence that the window still draws real content.

## Phase 5: Interactive SUMA Workflows

> Top priority (pulled into Daily Driver): multi-surface scenes + state
> switching, and ROI display/drawing/editing/undo/save.

- [x] Add multi-surface scenes: pial, smoothwm, inflated, sphere, and
  registered template surfaces.
  - Why: SUMA's power comes from switching among related surfaces in one scene,
    not opening one mesh at a time.
- [ ] Add surface visibility toggles and current-surface focus.
  - Why: Multi-surface scenes need simple controls for hiding, showing, and
    choosing the active surface.
- [x] Add state switching across related surfaces, such as anatomical,
  inflated, spherical, and template states.
  - Why: State switching lets users inspect the same data on surfaces that make
    different anatomical features easier to see.
- [ ] Add node/triangle inspection panels backed by shared selection state.
  - Why: Clicking a node should reveal useful facts like coordinates, labels,
    overlay values, and topology.
- [ ] Add ROI loading, display, creation, editing, undo/redo, and save/export.
  - Why: ROIs are a core SUMA workflow, and users need to both view old ROIs
    and create new ones safely.
- [ ] Add threshold controls and color-map management for loaded overlays.
  - Why: Real analysis maps need quick adjustment to reveal signal without
    reloading data.
- [ ] Add cluster/connected-component views for thresholded overlays.
  - Why: Clusters help users summarize thresholded results as regions rather
    than thousands of separate nodes.
- [ ] Add volume-to-surface and surface-to-volume bridge operations where the
  data model can support them.
  - Why: Many workflows start in volume space and end on the surface, or the
    reverse.

## Phase 6: AFNI Interop

- [ ] Decide how much live AFNI communication matters for the first real
  release.
  - Why: Live AFNI sync is useful, but deciding its scope prevents it from
    delaying the standalone viewer.
- [ ] Document the subset of AFNI/SUMA messages needed for surface selection,
  crosshair updates, dataset loading, and controller state.
  - Why: A small documented protocol target is easier to implement and test
    than trying to mirror everything.
- [ ] Add AFNI/SUMA-compatible `BBox` threshold A/B semantics for future
  multi-threshold transparency and masking controls.
  - Why: Threshold failures should eventually be controlled as overlay
    transparency/masking state, not by unexpectedly changing the base surface
    appearance.
- [ ] If needed, implement a small protocol crate for AFNI/SUMA messaging
  rather than burying protocol details inside the viewer.
  - Why: Protocol code should be reusable by CLIs and tests, not locked inside
    window-rendering code.
- [ ] Add command-line conversion and inspection tools that work without the
  GUI.
  - Why: Headless tools make batch workflows, debugging, and test fixtures much
    easier.
- [ ] Add compatibility tests against AFNI-generated messages and representative
  session files.
  - Why: Interop only matters if it matches real AFNI output, so fixtures should
    define the contract.

## Phase 7: Volume Rendering

- [ ] Define `Volume` and `VolumeSpace` types from NIFTI/AFNI concepts:
  dimensions, voxel sizes, origin, orientation codes, qform/sform or AFNI
  matrix attributes, and transforms between voxel index/IJK, scanner/world, and
  AFNI-style coordinate spaces.
  - Why: Volume rendering and AFNI interop need reliable coordinate math before
    pixels are drawn.
- [ ] Convert NIFTI headers and affine metadata into the shared `VolumeSpace`
  model.
  - Why: NIFTI files store orientation and scaling in headers, and we need that
    information to place volumes correctly.
- [ ] Add fixture-backed snapshot/golden tests for representative `.nii.gz`,
  `.hdr/.img`, AFNI volume metadata, and malformed volume inputs.
  - Why: Volume headers are easy to misread, and fixtures catch orientation and
    datatype mistakes early.
- [ ] Add `-v/--volume` as a volume-only viewer mode for NIFTI `.nii` and
  `.nii.gz` inputs.
  - Why: A separate volume entry point lets us prove loading and display before
    mixing volumes with surfaces.
- [ ] Start with volume metadata in the viewer and orthogonal slice rendering
  to validate NIFTI loading, intensity normalization, voxel spacing, and
  orientation handling.
  - Why: Slices are the simplest honest way to check that the volume is loaded
    and oriented correctly.
- [ ] Add slice navigation, window/level controls, and crosshair-linked slice
  positions.
  - Why: Basic volume viewing requires moving through slices and adjusting
    contrast interactively.
- [ ] Upload volume data to GPU textures with a clear strategy for scalar
  datatype conversion and normalization.
  - Why: GPU rendering needs predictable texture formats even when source files
    use many numeric datatypes.
- [ ] Add true 3D volume rendering with a `wgpu` ray-marching shader, transfer
  functions, opacity/window controls, and 3D texture upload.
  - Why: Ray marching is the path to actual 3D volume visualization rather than
    only slice viewing.
- [ ] Decide how 4D NIFTI data maps into the viewer: first volume by default,
  selectable timepoints/bricks later.
  - Why: 4D files are common, and the viewer needs a clear rule instead of
    silently picking the wrong frame.
- [ ] Integrate surface + overlay + volume scenes once shared spatial
  transforms and crosshair state are reliable.
  - Why: Combined scenes are powerful only if the objects align correctly in
    the same space.

## Phase 8: Packaging and Reliability

- [ ] Ship binaries for macOS, Linux, and Windows.
  - Why: Users should not need a Rust toolchain just to try the application.
- [ ] Add continuous integration for `cargo fmt`, `cargo check`, and
  `cargo test` on supported platforms.
  - Why: CI catches breakage early and keeps the project buildable across
    machines.
- [ ] Add clippy once the codebase is large enough for lint policy to matter.
  - Why: Clippy helps catch common Rust mistakes once the code is stable enough
    for a consistent lint bar.
- [ ] Add fuzz tests for AFNI/SUMA/NIML parsers.
  - Why: Parsers face messy input, and fuzzing helps find crashes before users
    do.
- [ ] Add benchmark coverage for large surfaces, large overlays, and large
  datasets.
  - Why: Neuroimaging files can be large, and benchmarks show when changes make
    loading or rendering slower.
- [ ] Build a small public corpus of open neuroimaging fixtures for regression
  testing.
  - Why: Shared fixtures make bugs reproducible and give contributors a common
    test set.
- [ ] Add crash-report-friendly errors for parser failures and GPU setup
  failures.
  - Why: Clear errors help users report problems and help us diagnose failures
    quickly.
