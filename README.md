# sumaru

`sumaru` is a ground-up Rust rebuild of AFNI's SUMA surface tooling.
The first milestone is a small, testable command-line core that can read the
neuroimaging file types SUMA workflows depend on.

## Current Scope

- GIFTI surface/shape I/O through `gifti-rs` from `PennLINC/gifti-rs`
- NIFTI volume I/O through `nifti` from `Enet4/nifti-rs`
- A first surface viewer through `winit`, `wgpu`, and `egui`
- Headless file inspection:

```sh
cargo run
cargo run -- -i /path/to/surface.gii
cargo run -- --surface /path/to/surface.gii
cargo run -- --surface /path/to/surface.gii --overlay /path/to/overlay.shape.gii
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

`--overlay` accepts a GIFTI file with one numeric value per surface vertex.
The shared color and label-table model now supports scalar ramps and integer
label colors. The current viewer still maps the prototype scalar overlay with a
blue-white-red ramp until richer overlay controls catch up.

## Viewer Controls

- Launch with `cargo run` to open an empty viewer and a separate controls
  window, then browse for a surface and optional overlay or load them by
  pasted path. The controls window auto-fits to its current contents, capped by
  the monitor size.
- Left-drag to orbit.
- Right-click the surface to inspect the nearest node, triangle, and loaded
  overlay value.
- Scroll to zoom.
- Press Space to reset the camera.
- Press `C` to switch camera mode between `orbit` and `turntable`.
- Press `O` to toggle a loaded overlay on or off.
- Press `F5` to switch the background between black and white.
- Hold Option and press an arrow key for preset views:
  - Option-Left: left side view
  - Option-Right: right side view
  - Option-Up: top-down view
  - Option-Down: bottom-up view

## Design Direction

The binary crate should stay thin. Most behavior should live in the library
crate so future renderers, GUI experiments, batch tools, and tests can share
the same data model.

See `docs/ROADMAP.md` for the staged build plan.

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
- `docs/ROADMAP.md` is the staged development plan for growing from the current
  reader-backed viewer into a SUMA-class surface, dataset, AFNI interop, and
  eventual volume-rendering tool.
- `src/lib.rs` is the library crate entry point. It exposes the reusable modules
  so the binary, tests, and future tools can share the same implementation.
- `src/main.rs` is the command-line entry point. It parses `sumaru` arguments,
  launches the viewer with or without an initial surface, handles `--overlay`,
  and runs the `inspect` subcommand.
- `src/color.rs` contains shared RGBA, continuous color-map, and label-table
  models for scalar maps and integer label datasets, including GIFTI and
  FreeSurfer import helpers.
- `src/dataset.rs` contains the canonical domain-attached dataset table model.
  It supports dense and sparse row-to-node data, typed columns, column labels
  and roles, numeric ranges, units, and parent/provenance ids.
- `src/inspect.rs` contains headless file inspection. It detects GIFTI/NIFTI
  paths, reads them through the current external crates, and prints concise
  metadata summaries.
- `src/io.rs` contains the first native AFNI/SUMA I/O layer. It parses and
  writes NIML elements, handles ASCII and fixed-width binary numeric payloads,
  preserves mixed numeric/string rows, extracts `.niml.dset`/`.niml.roi`
  payloads, converts NIML datasets into canonical `Dataset` values, and encodes
  compatibility checks against the AFNI C, MATLAB, and SUMAvista Python
  readers.
- `src/overlay.rs` contains display state layered on datasets. It selects
  intensity/threshold/brightness columns, stores color-map and range controls,
  and builds per-node RGBA color caches for rendering.
- `src/roi.rs` contains the shared ROI model for drawn, imported, dataset-born,
  and threshold-derived surface regions. It stores labels, styling,
  parent-surface/domain links, source/provenance, path history, domain
  validation, and conversion into sparse ROI datasets.
- `src/surface.rs` contains the current surface data model and GIFTI surface
  adapter. It loads vertices/triangles, validates indices, computes bounds and
  normals, records SUMA-inspired domain/metadata/lineage, and stores scalar
  overlay values/ranges without depending on viewer rendering details.
- `src/viewer/mod.rs` contains the desktop viewer. It sets up the `winit` event
  loop, initializes shared `wgpu` state for a surface window and separate
  controls window, integrates `egui`, uploads reloadable surface geometry,
  handles camera controls, renders the mesh, and shows loading/camera controls.
- `src/viewer/mesh.rs` prepares durable surface/overlay data for the viewer. It
  normalizes positions, flattens triangle indices, assigns default or overlay
  colors, and packs vertex/index bytes for GPU upload.
- `src/viewer/shader.wgsl` contains the GPU shader code used by `wgpu`,
  primarily the lit surface rendering path.
- `target/` is generated by Cargo when you build or run the project. It is not
  source code and can be regenerated at any time.
