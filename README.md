# sumaru

`sumaru` is a ground-up Rust rebuild of AFNI's SUMA surface tooling.
The first milestone is a small, testable command-line core that can read the
neuroimaging file types SUMA workflows depend on.

## Current Scope

- GIFTI surface/shape/dataset I/O through `gifti-rs` from `PennLINC/gifti-rs`
- NIFTI volume I/O through `nifti` from `Enet4/nifti-rs`
- SUMA `.spec` parsing for single-hemisphere multi-surface scenes
- A first surface viewer through `winit`, `wgpu`, and `egui`
- Headless file inspection:

```sh
cargo run
cargo run -- -i /path/to/surface.gii
cargo run -- --surface /path/to/surface.gii
cargo run -- --surface /path/to/surface.gii --overlay /path/to/overlay.shape.gii
cargo run -- --surface /path/to/surface.gii --overlay /path/to/stats.niml.dset
cargo run -- --surface /path/to/surface.gii --overlay /path/to/stats.gii.dset
cargo run -- --surface /path/to/surface.gii --overlay /path/to/stats.niml.dset --verbose
cargo run -- -spec /path/to/subj_rh.spec -sv /path/to/subj_SurfVol.nii
cargo run -- -spec /path/to/subj_rh.spec -sv /path/to/subj_SurfVol.nii --preload
cargo run -- inspect /path/to/file.nii.gz
```

## Cargo Commands

The project defines a few Cargo aliases in `.cargo/config.toml`:

```sh
cargo check-all
cargo test-all
cargo fmt-all
cargo surface /path/to/surface.gii
cargo inspect -- /path/to/file.gii
```

## Overlays

`--overlay` accepts a GIFTI file with one numeric value per surface vertex, an
AFNI/SUMA `.niml.dset`, or an AFNI-converted `.gii.dset`. Multi-column datasets
are parsed into the canonical `Dataset` table first; the controller can then
choose intensity, threshold, and brightness columns from the dataset. If the
selected threshold column carries an AFNI stat label such as `Ttest(48)`, the
threshold control can operate in p-value mode.

## AFNI NIML Talk

`sumaru` now has a non-`wgpu` AFNI/SUMA NIML talk layer in the library crate and
a first live viewer bridge. The first concrete AFNI-compatible message subset
is the same practical path used by SUMA and PySuma:

- `SUMA_ixyz`: surface node index and XYZ coordinates sent to AFNI
- `SUMA_node_normals`: per-node normals sent to AFNI
- `SUMA_ijk`: triangle indices sent to AFNI
- `SUMA_irgba`: sparse node RGBA colors and optional threshold/function/volume
  metadata sent from AFNI back to the surface viewer

Launch with `--talk-afni` to connect on startup, or press `T` in the viewer to
toggle AFNI/SUMA NIML talk. Press `Control+T` to force-resend the active surface
geometry. Port selection follows AFNI/SUMA conventions: `--afni-port PORT` uses
an explicit port, while `-np OFFSET`/`--np OFFSET` and `-npb BLOC`/`--npb BLOC`
resolve the same AFNI-style port offsets. `--afni-host` defaults to `127.0.0.1`.
AFNI must be listening for NIML before Sumaru can connect: launch AFNI with
`-niml` (and usually `-yesplugouts` for SUMA-style sessions), or press the
`NIML+PO` button in the AFNI GUI after launch.

For a quick look at any supported file, use the generic inspector. It covers
GIFTI, NIFTI, raw NIML datasets/ROIs/label tables, and recorded NIML traces:

```sh
cargo run -- inspect path/to/file
```

For reproducible AFNI talk debugging, add
`--niml-record path/to/session.nimlrec` to a viewer launch. Sumaru records each
sent and received NIML event with direction, timestamp, and the serialized
payload. Recording is intentionally plain `.nimlrec` for live-session speed;
gzip the file afterward if you want to archive or share it. The debug readers
accept both `.nimlrec` and `.nimlrec.gz`:

```sh
cargo run -- niml inspect path/to/session.nimlrec
cargo run -- niml replay path/to/session.nimlrec.gz
```

Small test messages can be sent directly to an AFNI/SUMA NIML socket:

```sh
cargo run -- --afni-port 53211 niml send raw path/to/message.niml
cargo run -- --afni-port 53211 niml send crosshair --surface-id SURF_ID --node 42 --xyz 1,2,3
cargo run -- --afni-port 53211 niml send command reset-camera
```

Example:

```sh
cargo run -- --spec path/to/fsaverage_lh.spec --sv path/to/SurfVol.nii --talk-afni --niml-record afni_session.nimlrec
```

The same module also defines Sumaru-side NIML state messages for active surface,
crosshair and selected node/triangle, dataset loading, overlay/threshold
settings, controller commands, and ROI state. Those messages route through
shared controller/command state rather than directly mutating viewer-only
fields, so they can be tested without launching the GUI.

## Viewer Controls

- Launch with `cargo run` to open an empty viewer and a separate controls
  window, then use the `Open:` buttons for a surface, overlay, spec, or surface
  volume. The controls window auto-fits to its current contents, capped by
  the monitor size.
- Add `--verbose` to print viewer status messages to the terminal.
- Spec scenes load only the active display state by default. Add `--preload`
  to load the remaining spec surfaces in the background after launch.
- Left-drag to orbit.
- Right-click the surface to inspect the nearest node, triangle, and loaded
  overlay value.
- Scroll to zoom.
- Press Space to reset the camera.
- Press `C` to switch camera mode between `orbit` and `turntable`.
- Press `O` to toggle a loaded overlay on or off.
- Press `.` to advance to the next surface in a loaded single-hemisphere
  `.spec` scene, or the next matched left/right state pair in a `both` scene.
  Press `,` to move backward.
- In a `both` spec scene, use `Open` and `Close` in the VIEW section to
  persistently switch between the closed and acorn paired-hemisphere layouts.
  Hold Control and left-drag in the viewer to fine-tune the pair: left/right
  adjusts the open angle, and up/down adjusts the gap between hemispheres.
- In a `both` spec scene, press `[` to show/hide the left hemisphere and `]` to
  show/hide the right hemisphere.
- Press `r` to save the current view as a PNG, or Shift-`R` to save a 1x4
  montage. Single-surface scenes use left/right/top/bottom views; `both` spec
  scenes use closed top, closed bottom, open medial-in, and open outer-out
  views. The VIEW section also has `Save` and `Montage` buttons. When a
  thresholded overlay is active, a second `_cmap`-suffixed file is saved
  alongside the screenshot with the colorbar rendered on the right side.
- Press `F5` to switch the background between black and white.
- Press `g` to open or close the graph dock at the bottom of the view window.
  When open, right-click picks update the graph live. Drag the handle at the
  top of the dock to resize it; the 3D viewport adjusts to match.
- Hold Option and press an arrow key for preset views:
  - Option-Left: left side view
  - Option-Right: right side view
  - Option-Up: top-down view
  - Option-Down: bottom-up view

## Design Direction

The binary crate should stay thin. Most behavior should live in the library
crate so future renderers, GUI experiments, batch tools, and tests can share
the same data model.

See `docs/ROADMAP.md` for the active to-do plan and `docs/COMPLETED.md` for
the completed-work ledger.

## Project File Guide

- `Cargo.toml` defines the `sumaru` package, Rust edition/toolchain floor,
  dependencies, and lint policy. This is where core libraries like `gifti-rs`,
  `nifti`, `winit`, `wgpu`, `egui`, `rfd`, `clap`, and `glam` are wired in.
- `Cargo.lock` records exact dependency versions so rebuilds use the same crate
  graph.
- `.cargo/config.toml` defines local Cargo aliases such as `cargo check-all`,
  `cargo surface`, and `cargo inspect`.
- `.gitignore` keeps Cargo build output in `target/` out of version control.
- `README.md` is the project-facing quickstart: scope, commands, controls,
  overlays, design direction, and this file guide.
- `docs/ROADMAP.md` is the active to-do plan, grouped by shared foundations
  such as AFNI interop, command state, everyday viewer use, GPU work, and
  volume support.
- `docs/COMPLETED.md` is the completed-work ledger for bootstrap, data model,
  geometry, viewer, ROI, spec, and rendering performance milestones.
- `src/lib.rs` is the library crate entry point. It exposes the reusable modules
  so the binary, tests, and future tools can share the same implementation.
- `src/afni.rs` contains the first AFNI/SUMA NIML talk layer. It resolves
  AFNI-style ports, builds `SUMA_ixyz`/`SUMA_node_normals`/`SUMA_ijk` surface
  registration elements, parses `SUMA_irgba` overlays, maps incoming NIML
  messages to shared controller actions, and emits Sumaru state messages.
- `src/command.rs` contains the shared controller and command state used to
  route viewer menus, keyboard shortcuts, controller panels, and AFNI messages
  through the same non-`wgpu` model. It owns the canonical `OverlayThreshold`
  type (used by both the render appearance and the controller command state) and
  `ValueRange` (`f32` render range), keeping the render/domain boundary explicit.
- `src/main.rs` is the command-line entry point. It parses `sumaru` arguments,
  launches the viewer with an initial surface or `.spec` scene, requires `-sv`
  surface-volume context for `.spec` launches, handles `--overlay`, passes
  through `--verbose` terminal logging, controls spec preloading with
  `--preload`, and runs the `inspect` subcommand.
- `src/color.rs` contains shared RGBA, continuous color-map, and label-table
  models for scalar maps and integer label datasets, including GIFTI and
  FreeSurfer import helpers. Continuous colormaps include 13 byte-exact AFNI
  colorscales ported from `DC_spectrum_AJJ` / `DC_spectrum_ZSS` (the
  same LUT engine SUMA uses): `Spectrum:red_to_blue`, both `+gap` variants,
  `Spectrum:yellow_to_red`, `Spectrum:yellow_to_cyan` and its `+gap` variant,
  `color_circle_AJJ`, `color_circle_ZSS`, `Reds_and_Blues`,
  `Reds_and_Blues_w_Green`, `afni_p2spanned`, `bwr`, and `Fire`.
- `src/dataset.rs` contains the canonical domain-attached dataset table model.
  It supports dense and sparse row-to-node data, typed columns, column labels
  and roles, numeric ranges (`ColumnRange`, the `f64` domain range type),
  units, and parent/provenance ids.
- `src/inspect.rs` contains headless file inspection. It detects GIFTI/NIFTI
  paths, reads them through the current external crates, and prints concise
  metadata summaries.
- `src/io.rs` contains the first native AFNI/SUMA I/O layer. It parses and
  writes NIML elements, handles ASCII and fixed-width binary numeric payloads,
  preserves mixed numeric/string rows, extracts `.niml.dset`/`.niml.roi`
  payloads, converts NIML and AFNI `.gii.dset` datasets into canonical
  `Dataset` values, and encodes compatibility checks against the AFNI C,
  MATLAB, and SUMAvista Python readers.
- `src/overlay.rs` contains display state layered on datasets. It selects
  intensity/threshold/brightness columns, stores color-map and range controls,
  and builds per-node RGBA color caches for rendering.
- `src/roi.rs` contains the shared ROI model for drawn, imported, dataset-born,
  and threshold-derived surface regions. It stores labels, styling,
  parent-surface/domain links, source/provenance, path history, domain
  validation, and conversion into sparse ROI datasets.
- `src/spec.rs` parses SUMA `.spec` files into surface groups, states,
  hemisphere labels, resolved surface paths, local domain/curvature parents,
  anatomical flags, and label-dataset references.
- `src/surface.rs` contains the current surface data model and GIFTI surface
  adapter. It loads vertices/triangles, validates indices, computes bounds and
  normals, records SUMA-inspired domain/metadata/lineage, and stores scalar
  overlay values/ranges without depending on viewer rendering details.
- `src/viewer/mod.rs` coordinates the desktop viewer. It sets up the `winit`
  event loop, owns the four windows (view, control, roi_control, graph) as
  `WindowPane` values, integrates `egui` via `EguiPane`, routes
  keyboard/mouse/UI actions, loads surfaces and overlays, and calls into the
  viewer submodules for camera, picking, GPU setup, and render-prep work.
- `src/viewer/camera.rs` contains the viewer camera model: orbit and turntable
  modes, preset orientations, scroll zoom, mouse drag handling, and camera
  uniform packing for the shader.
- `src/viewer/gpu.rs` contains small `wgpu` setup helpers such as surface
  format/present-mode selection and the depth-buffer texture used by the
  surface render pass.
- `src/viewer/mesh.rs` prepares durable surface/overlay data for the viewer. It
  normalizes positions, flattens triangle indices, assigns default or overlay
  colors, and packs vertex/index bytes for GPU upload.
- `src/viewer/pick.rs` contains right-click surface inspection. It builds a
  camera ray from the cursor, intersects it with normalized triangles, and
  reports the hit triangle, nearest node, and overlay value.
- `src/viewer/screenshot.rs` contains screenshot image helpers. It converts
  `wgpu` readback bytes into RGBA pixels, writes PNG files, and stitches the
  preset-view montage.
- `src/viewer/shader.wgsl` contains the GPU shader code used by `wgpu`,
  primarily the lit surface rendering path.
- `target/` is generated by Cargo when you build or run the project. It is not
  source code and can be regenerated at any time.
