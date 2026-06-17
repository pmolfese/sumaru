# sumaru To-Do Roadmap

This is the active work plan for growing `sumaru` into a SUMA-class Rust
surface viewer and analysis environment.

Completed work has moved to [`COMPLETED.md`](COMPLETED.md). This file should
only contain unfinished work, grouped by the foundations they lean on rather
than by the order the ideas originally appeared.

## Current Priority: AFNI Interop And File Formats

The next major push is to make `sumaru` communicate with AFNI/SUMA and handle
AFNI/SUMA file formats with enough fidelity that real sessions can move between
AFNI, SUMA, SUMAvista, and `sumaru`.

### Format Inventory And Fixtures

- [ ] Inventory the AFNI/SUMA file formats and workflows needed for the first
  interop-capable release: `.spec`, GIFTI surfaces, `.niml.dset`,
  `.gii.dset`, `.niml.roi`, label tables, color maps, `.HEAD/.BRIK`, surface
  volumes, and live AFNI/SUMA messages.
- [ ] Build a broader compatibility fixture matrix with dense and sparse
  datasets, ASCII and binary NIML, mixed numeric/string tables, label tables,
  multi-surface `.spec` sessions, malformed files, and recorded AFNI/SUMA
  message examples.
- [ ] Cross-check file semantics against AFNI C code, SUMAvista Python,
  MATLAB readers where useful, and the external
  `/Users/molfesepj/Documents/Programming/afni_rust` crate before hardening
  public APIs.

### NIML Talk With AFNI

- [x] Document the minimal AFNI/SUMA message subset for first interop: surface
  selection, crosshair updates, selected node/triangle, dataset loading,
  overlay/threshold state, controller commands, and ROI updates.
- [x] Add a small NIML communication/session layer that is independent of
  `wgpu`, so tests and command-line tools can exercise AFNI message handling
  without launching the viewer.
- [x] Route incoming AFNI messages through shared command/controller state
  rather than directly mutating viewer-only fields.
- [x] Emit the matching `sumaru` state back to AFNI where appropriate:
  crosshair location, selected node, active surface, current dataset, and
  overlay/threshold settings.
- [x] Wire the first live viewer bridge: launch with `--talk-afni`, toggle with
  `T`, force-resend active surface geometry with `Control+T`, and display
  incoming `SUMA_irgba` color overlays.
- [x] Add debug tools for AFNI interop: record message streams, replay message
  streams, inspect individual messages, and send small test commands from the
  CLI.
- [ ] Add compatibility tests against AFNI-generated messages and representative
  session files.
  - [ ] Replace the large local `sub-3_both.spec` AFNI talk recording with a
    compact committed `.nimlrec.gz` fixture.
  - [ ] Functional overlay `SUMA_irgba` recording.
  - [ ] Atlas/label overlay `SUMA_irgba` recording.

### File Format Polish In `sumaru`

- [ ] Tighten `.niml.roi` round-trip fidelity for multi-ROI files, finalized
  versus editable ROI states, stroke/fill metadata, outside fills, per-ROI
  colors, and SUMA-compatible save output.
- [ ] Expand `.niml.dset` and `.gii.dset` coverage for more statistical
  metadata, label-table payloads, sparse node-index conventions, and malformed
  inputs.
- [ ] Add AFNI `.HEAD/.BRIK` metadata support once `VolumeSpace` can represent
  AFNI orientation, transform, and warp attributes accurately.
- [ ] Add command-line conversion and inspection tools that work without the
  GUI for surfaces, datasets, ROI files, specs, and recorded NIML messages.
- [ ] Keep compatibility code at the edges so the internal surface, dataset,
  overlay, ROI, and scene models remain file-neutral.

### Move Toward `afni_rust`

- [ ] Review `/Users/molfesepj/Documents/Programming/afni_rust` for existing
  format models, parser/writer APIs, error types, fixture strategy, and places
  where `sumaru` and the crate disagree.
- [ ] Decide the boundary between `sumaru`'s canonical runtime models and
  reusable AFNI/SUMA I/O crate models.
- [ ] Add adapter traits so `sumaru` can swap local parsers for `afni_rust`
  readers/writers without changing viewer or analysis code.
- [ ] Move stable NIML, spec, dataset, ROI, and future AFNI volume I/O into
  `afni_rust` once APIs and fixtures are stable enough to share.
- [ ] Keep shared fixtures and golden summaries aligned between `sumaru`,
  `afni_rust`, AFNI/SUMA, and SUMAvista.

## Shared Controller And Command State

This cluster supports AFNI interop, menus, keyboard shortcuts, controller
windows, scripts, and future automation. It should happen before wiring a lot
more UI or protocol behavior into viewer-only fields.

- [x] Move camera, background, overlay, ROI, surface selection, visibility,
  crosshair, and pick settings into shared command/controller state.
- [x] Add a controller layer for UI panels, keyboard shortcuts, CLI commands,
  and AFNI messages before adding richer controls.
- [x] Define shared interaction state: selected node, selected face, selected
  triangle, crosshair location, current surface/object id, current overlay id,
  current ROI id, and latest pick result.
- [x] Split the current `egui` panels into controller-backed widgets once the
  command state exists.
- [x] Add a lightweight status/log event stream so `--verbose`, controllers,
  and future AFNI communication can report the same events consistently.
- [ ] Add **new-sumaru** and **duplicate-sumaru** launch buttons to the
  controller panel, drawn as custom-painted icon buttons (painter API, no extra
  crate):
  - **new-sumaru** (`+` icon): spawns a blank sumaru process — no surface, no
    overlay, no session context carried over.
  - **duplicate-sumaru** (two-overlapping-rectangles / copy icon): spawns a new
    sumaru instance pre-loaded with the same surface (and spec state, if active)
    but no overlay, so the user gets a clean starting point for a second
    analysis view.
  - Wire both to the linked-session layer (see *Linked Sumaru Sessions*) once
    that infrastructure exists; for now, a plain subprocess launch is
    sufficient.

## Everyday Viewer Use

These are usability features that make `sumaru` easier to use day to day before
the larger GPU/shader optimization pass.

- [ ] Add recent-file and remembered-working-folder support for surface,
  overlay, spec, surface-volume, ROI, screenshot, and montage workflows.
- [ ] Add label-table-aware coloring for atlas/label datasets and imported
  GIFTI/FreeSurfer label tables.
- [ ] Add richer node/triangle inspection panels backed by the current pick and
  crosshair state.
- [ ] Promote picked-node graphing into a daily-driver workflow: persistent
  graph windows opened from `G`, clear column/timepoint labeling, multiple
  picked-node traces, graph export, and live refresh when the active overlay or
  selected node changes.
- [ ] Add a `sumaru graph` or `sumaru plot` command for non-GUI graphing and
  inspection of surface datasets by node, ROI, column range, or saved pick.
  Start with a compact form like `sumaru graph <vertex> <dataset>` and expand
  to optional surface/domain context, output image export, and multi-column
  plotting.
- [ ] Add explicit surface visibility and focus controls for multi-surface and
  `.spec` scenes.
- [ ] Add multiple overlay planes with explicit foreground/background ordering.
- [ ] Add AFNI/SUMA-compatible `BBox` threshold A/B semantics for future
  multi-threshold transparency and masking controls.
- [ ] Add cluster and connected-component views for thresholded overlays.
- [ ] Add automated GUI regression coverage once graphics verification is
  practical:
  state/command tests for shortcuts and controller actions, render-prep tests
  for markers/ROI/overlay data, headless offscreen render tests for visible
  pixels, and tolerant golden-image tests for montage/acorn/ROI scenes.

## Geometry, Mapping, And Analysis Extensions

The core geometry layer is strong enough for ROI work now. These items extend it
toward richer analysis workflows and cross-space operations.

- [ ] Add node/triangle inspection panels that expose topology, coordinates,
  labels, overlay values, ROI membership, and surface/domain lineage.
- [ ] Add volume-to-surface and surface-to-volume bridge operations where the
  data model can support them.
- [ ] Add richer cluster summaries for thresholded overlays: size, area,
  centroid, peak value, peak node, and ROI export.
- [ ] Expand surface-to-surface transfer tests for standard meshes, same-node
  standard-surface compatibility, nearest-neighbor transfer, and barycentric
  transfer.

## GPU/Shader Optimization Bundle

Do this as one coordinated rendering pass after the next everyday-use and AFNI
interop work. The guiding principle is: build durable data once, then express
interaction as cheap state on top.

Measure first on a real large `both`-hemisphere scene: recolor time, geometry
upload time, scalar/color upload time, threshold rebuild time, and frame time.

- [ ] Add a small viewer performance HUD or verbose timing hooks for mesh
  upload, color/scalar upload, threshold rebuild, spec-state switching, ROI
  drawing, and frame time.
- [ ] Split the interleaved vertex buffer into position+normal geometry buffers
  and separate compact color/scalar buffers.
- [ ] Upload raw per-vertex scalar columns once; put threshold, range, opacity,
  and colormap id in small uniforms; sample colormaps in the shader against a
  1-D LUT texture.
- [ ] Keep all loaded spec states resident as reusable GPU geometry buffers so
  pial/inflated/sphere switching becomes a draw-list or bind-group swap instead
  of a surface upload.
- [ ] Split ROI and selection highlighting out of baked vertex colors into
  lightweight GPU layers or buffers, so drawing and editing ROIs does not
  rebuild the full surface color stream.
- [ ] Preserve SUMA threshold semantics in WGSL: symmetric range, absolute
  thresholding, hide-failed versus dim, p-value-to-stat conversion, NaN
  handling, and sparse node-to-vertex expansion.
- [ ] Add focused pixel/render tests for shader-side threshold and color
  behavior.

## Volume And AFNI Volume Space

Volume work stays behind AFNI/file-format interop because AFNI coordinate
semantics need to be represented correctly before serious rendering begins.

- [ ] Define `Volume` and `VolumeSpace` types from NIFTI/AFNI concepts:
  dimensions, voxel sizes, origin, orientation codes, qform/sform or AFNI
  matrix attributes, and transforms between voxel index/IJK, scanner/world, and
  AFNI-style coordinate spaces.
- [ ] Convert NIFTI headers and affine metadata into the shared `VolumeSpace`
  model.
- [ ] Add fixture-backed snapshot/golden tests for representative `.nii.gz`,
  `.hdr/.img`, AFNI volume metadata, and malformed volume inputs.
- [ ] Add AFNI `.HEAD/.BRIK` metadata support once the shared `VolumeSpace`
  model is ready.
- [ ] Add `-v/--volume` as a volume-only viewer mode for NIFTI `.nii` and
  `.nii.gz` inputs.
- [ ] Start with volume metadata in the viewer and orthogonal slice rendering
  to validate NIFTI loading, intensity normalization, voxel spacing, and
  orientation handling.
- [ ] Add slice navigation, window/level controls, and crosshair-linked slice
  positions.
- [ ] Upload volume data to GPU textures with a clear scalar datatype
  conversion and normalization strategy.
- [ ] Add true 3D volume rendering with a `wgpu` ray-marching shader, transfer
  functions, opacity/window controls, and 3D texture upload.
- [ ] Decide how 4D NIFTI data maps into the viewer: first volume by default,
  selectable timepoints/bricks later.
- [ ] Integrate surface, overlay, ROI, and volume scenes once shared spatial
  transforms and crosshair state are reliable.

## Linked Sumaru Sessions And Session Restore

- [ ] Add Sumaru-to-Sumaru NIML/session communication so multiple viewer
  windows can share crosshair position, selected node, active surface, active
  overlays, ROI edits, graph windows, and controller state.
  - [ ] Define the first Sumaru-to-Sumaru message subset: crosshair position,
    picked node/face, active surface/state, active overlay identity, threshold
    state, and graph-open requests.
  - [ ] Add CLI launch options for linked sessions: join an existing Sumaru
    session, create a new session group, name an instance, and choose whether
    it starts as a master or follower.
  - [ ] Add controller/menu controls for selecting the active master for a
    window, following one master, temporarily unlinking, and reconnecting.
  - [ ] Support flexible group layouts, including one master with several
    followers and multiple master/follower groups in the same desktop session.
  - [ ] Add subtle per-instance color identity across the render window,
    surface controller, ROI controller, and graph overlay. Use the default
    current black theme for the original instance, then blue, orange, green,
    and generated fallback accents for additional windows.
  - [ ] Keep linked-session state in the shared controller/command layer so
    AFNI talk, keyboard shortcuts, graph windows, and future scripts can use
    the same event path.
  - [ ] Add verbose/debug logging and record/replay support for Sumaru-to-Sumaru
    messages, modeled after the AFNI NIML recording tools.
- [ ] Add session save/restore support for launch paths, spec state, active
  surfaces, surface-volume context, overlays, ROI files, camera/view state,
  thresholds, graph windows, and linked AFNI/NIML talk settings.
- [ ] Add `sumaru session save` and `sumaru session restore` commands, with
  matching menu/controller entries once the format is stable.

## Packaging, Reliability, And Public Use

- [ ] Ship binaries for macOS, Linux, and Windows.
- [ ] Add macOS and Windows CI when project resources make those runners
  practical.
- [ ] Add clippy once the codebase is stable enough for lint policy to matter.
- [ ] Add fuzz tests for AFNI/SUMA/NIML parsers.
- [ ] Add benchmark coverage for large surfaces, large overlays, large ROI
  files, large `.spec` scenes, and large datasets.
- [ ] Build a small public corpus of open neuroimaging fixtures for regression
  testing.
- [ ] Add crash-report-friendly errors for parser failures, AFNI interop
  failures, and GPU setup failures.
