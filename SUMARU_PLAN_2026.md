# SUMARU Feature Plan 2026

This document maps the proposed Sumaru features against the current Sumaru architecture and the corresponding implementations in the original AFNI/SUMA codebase:

1. Multiple overlay switching
2. Direct p-value and q-value threshold entry
3. InstaCorr (`D` dot product)
4. Tractography and graph datasets
5. Crosshair synchronization between cloned viewers

## Status at a Glance

Status labels in this document mean:

- **Complete:** the agreed initial user-facing feature is implemented and tested.
- **Partial:** a usable implementation exists, but specifically listed follow-up work remains.
- **Not started:** design decisions exist, but there is no user-facing implementation yet.
- **Deferred:** deliberately outside the initial AFNI/SUMA-focused release.

| Feature | Status | Usable now | Work still to do |
|---|---|---|---|
| Multiple overlay switching | **Complete for active-only use** | Ordered overlay list; add, activate, refresh, remove, and keyboard-cycle; paired hemispheres stay together; configurable threshold transfer | Stable ID cleanup instead of positional identities; true simultaneous plane compositing is deferred |
| `~/.sumaru` preferences | **Complete for current settings** | Self-documenting defaults are created on first launch and existing files are preserved | Add new preference keys only when future features need them |
| Typed p-value entry | **Not started** | P-values are calculated and displayed; command-line `--p-val` conversion works | Click-to-edit UI, parsing/validation, presets, and semantic threshold intent |
| Typed q-value entry | **Not started** | Q-values are calculated and displayed when an AFNI FDR curve exists | Invert the FDR curve, add click-to-edit UI, validation, and tests |
| InstaCorr | **Partial; core workflow is usable** | `D` panel, SUMA preprocessing controls, background preprocessing, cached seed updates, generated overlays, paired hemispheres | Numerical comparison with reference SUMA output; nuisance-regressor input; result export; active cancellation rather than stale-result rejection |
| Tractography | **Partial; initial viewer is usable** | AFNI `.niml.tract` parsing, large-data GPU rendering, adjustable ribbons, three SUMA color modes, per-bundle visibility/opacity, BVH picking, surface/volume/standalone scenes | Coordinate metadata validation and override UI; per-bundle color editing; filters/masks; selection highlighting; joins/caps/LOD polish |
| Graph datasets | **Partial; initial viewer and matrix are usable** | `Graph_Bucket` parsing, nodes/edges, measures, threshold/color controls, labels, picking, linked tract bundles, matrix view | Node group/color metadata; edge-width/stipple mapping; unconnected-node control; external coordinate sources; export |
| Crosshair synchronization between cloned viewers | **Not started** | Each viewer can navigate by vertex or XYZ locally; AFNI crosshair integration exists separately | Local viewer-session transport, Auto/Vertex/XYZ modes, topology checks, loop prevention, and session UI |
| Additional tract/graph formats | **Deferred** | AFNI/SUMA formats only | TrackVis, MRtrix, CIFTI, and CSV after the native workflow is mature |

## Recommended Next Work

The next implementation slices, in priority order, are:

1. Implement typed p-value entry using the existing statistic conversion.
2. Implement conservative q-value inversion and typed q-value entry.
3. Add cloned-viewer sessions and `Auto` crosshair synchronization.
4. Validate InstaCorr numerically against reference SUMA output, then add AFNI `.1D` nuisance regressors and result export.
5. Finish tract coordinate metadata validation/override and graph node group/color support.
6. Add true simultaneous overlay compositing after the active-only workflow has been exercised further.

The remaining sections preserve the original design rationale. Their status
headings and checklists are authoritative when older proposal text describes
work that has since been implemented.

## Architecture Baseline and Changes

Sumaru's core overlay model is already richer than the viewer UI suggests. `src/overlay.rs` defines an `Overlay` with thresholding, opacity, foreground/background role, and plane order.

The original viewer limitation was a single `DatasetOverlayState`. The active-only
overlay milestone now stores multiple such states through
`src/viewer/overlay_stack.rs`, while keeping one active state connected to the
existing renderer. Consequently:

- Loading no longer discards earlier overlays; the active entry is swapped into
  the existing render path.
- Overlay commands still use positional indices, so stable overlay identities
  remain an internal cleanup item.
- Rendering is intentionally still active-only; visible-plane compositing is a
  separate later feature.
- `src/viewer/mesh.rs::OverlayAppearance` contains the reusable display state: threshold, range, colormap, opacity, contours, clustering, and related controls.
- `src/viewer/graph.rs` is a picked-node column plot, not support for SUMA graph datasets. It should be treated, and perhaps eventually named, as the plot dock to avoid ambiguity.

The canonical surface NIML path remains surface-oriented. Dedicated parsers now
handle `Graph_Bucket` matrix metadata/node coordinates and ASCII or binary
`TAYLOR_TRACT_DATUM` rows without forcing either format through the surface
dataset model.

## 1. Multiple Overlay Switching

### Implementation status (2026-09-26)

The first active-only milestone is implemented:

- Opening a new overlay adds and activates an ordered entry.
- Reopening an existing source selects and refreshes its existing entry.
- Left/right pairs remain one logical entry.
- The overlay workbench provides a selector, previous/next controls, and removal.
- Control-PageUp and Control-PageDown cycle the list.
- Only the active overlay is rendered.
- Threshold switching defaults to preserving the current numeric value.
- Incompatible statistical metadata leaves the numeric threshold in place and displays `p --` and `q --`.
- Settings > Preferences offers current-value, p-value-when-possible, and per-overlay policies.
- Preferences serialize automatically to a self-documenting `~/.sumaru`
  modeled on `~/.sumarc`: an `***ENVIRONMENT` section, stable `SUMARU_...`
  keys, and comments beside every setting that explain its values and default.
  Sumaru creates the documented default file on first launch when it is absent
  and never overwrites an existing file during initialization.
  The first setting is `SUMARU_OverlayThresholdSync`; the earlier minimal
  `overlay_threshold_sync` spelling remains readable for migration.

True visible-plane compositing remains a later milestone.

Remaining work in this area is limited to replacing positional overlay command
identities with stable IDs and, later, rendering more than one visible plane at
once. The active-only user workflow itself is complete.

### Original SUMA behavior

Original SUMA separates three concepts:

- The collection of color planes.
- The currently selected plane being edited.
- Which planes are visible and mixed.

The popup and selection behavior live around `SUMA_SelectSwitchDsetCol` in `SUMA_xColBar.c`. Existing planes retain their individual settings. When loading a related dataset, SUMA can copy settings if its column types are compatible, as implemented by `SUMA_PreserveOverlaySettings` in `SUMA_Color.c`.

This is an important distinction: selecting the active overlay and compositing multiple visible overlays are related, but separate, features.

### Proposed Sumaru design

Introduce stable overlay identities and a stack:

```text
OverlayStack
|-- entries: Vec<OverlayEntry>
|-- active_overlay: OverlayId
`-- display_policy
    |-- ActiveOnly
    `-- CompositeVisible
```

Each `OverlayEntry` should own its source information, canonical dataset, render cache, and appearance. A logical overlay entry may contain both left- and right-hemisphere sources so paired overlays always switch together.

Move or promote `OverlayAppearance` from viewer mesh-specific code into reusable overlay display state. Threshold transfer is preference-driven: the default copies the current numeric slider value, another mode converts through p-value metadata when possible, and a third restores per-overlay values. A later direct p/q-entry milestone can retain semantic threshold intent rather than only the converted statistic:

```text
ThresholdIntent
|-- Statistic(value)
|-- PValue(value)
`-- QValue(value)
```

For example, a shared `p <= 0.05` intent should be converted independently through each overlay's statistical metadata. Copying the resulting T statistic between datasets with different degrees of freedom would not preserve the same statistical threshold.

Extend viewer commands with operations such as:

- `AddOverlay`
- `SelectOverlay`
- `CycleOverlay`
- `RemoveOverlay`
- `SetOverlayVisibility`

### First milestone

- Loading an overlay adds it instead of replacing the current one.
- Add an overlay popup with active selection, visibility, and remove actions.
- Add next-overlay and previous-overlay commands.
- Render only the active overlay initially.
- Transfer threshold settings according to an explicit shared-threshold policy.
- Update picking and the plot dock against the active overlay.
- Preserve stable `OverlayId` values; never identify overlays solely by list position.
- Keep paired hemisphere overlays synchronized.

Later, `CompositeVisible` can expose true multi-plane mixing. The existing core model's `plane_order` and foreground/background roles already point in that direction.

### Keyboard considerations

`O` currently lowers surface opacity and Shift-O raises it in `src/viewer/input.rs`; Shift-V toggles overlay visibility. The README's older description of `O` as an overlay toggle should not determine the new binding.

A conflict-free pair such as Ctrl-PageUp/Ctrl-PageDown could cycle overlays, but the final choice should be made alongside the UI design.

### Tests

- Overlay identity and ordering
- Add, select, remove, and cycle operations
- Paired-hemisphere selection coherence
- Behavior when the next overlay lacks a compatible threshold column
- Threshold-profile propagation
- Pick and plot-dock refresh after switching
- State restoration and AFNI-compatible display behavior

### Direct p-value and q-value threshold entry

#### Implementation status: not started

The readout and underlying forward p/q calculations exist, but the labels are
still read-only. Typed p-value UI, q-value inversion, semantic threshold intent,
presets, and editor tests below remain to be implemented.

The viewer already does most of the forward calculation:

- `src/viewer/ui.rs` displays the current statistic threshold and derives its p-value through `AfniStatSpec::two_sided_p_value`.
- When an AFNI FDR curve is available, it derives and displays a q-value through `AfniFdrCurve::q_value`.
- `src/stats.rs::AfniStatSpec::statistic_for_p_value` already converts a typed p-value back into a statistic threshold.
- `src/viewer/overlay_load.rs::apply_initial_overlay_p_value` already uses that conversion for the command-line `--p-val` option.

The current p-value and q-value labels are read-only. The proposed interaction is:

- Show the statistic, p-value, and available q-value as a compact threshold readout.
- Give the p-value and q-value labels a hover treatment and tooltip that explains both interactions.
- Clicking a label opens a small numeric editor anchored to that label.
- Right-clicking a label opens presets and advanced threshold choices.
- Accept decimal and scientific notation, for example `0.05`, `.001`, or `1e-6`.
- Enter applies the value; Escape cancels; an Apply action remains available for pointer use.
- Offer common presets such as 0.05, 0.01, 0.005, and 0.001 in the same menu.
- Display validation errors in the editor without changing the active threshold.
- Applying a p-value or q-value enables thresholding and updates the underlying statistic threshold immediately.
- Dragging the threshold bar afterward recomputes the displayed p-value and q-value, so there is still only one authoritative statistic threshold.

The labels should also support keyboard focus and activation so numeric entry is not pointer-only.

#### P-value conversion

Typed p-values can reuse `statistic_for_p_value`. Signed T, correlation, Fisher-Z, and Z-score statistics should retain Sumaru's current two-tailed semantics; F and chi-square statistics use their supported upper-tail calculation. The editor should be disabled with a clear explanation when the selected threshold column lacks supported statistical metadata.

#### Q-value conversion

Typed q-values require a new inverse operation, such as `AfniFdrCurve::threshold_for_q_value`. It should:

- Validate that q is finite and in `(0, 1]`.
- Search the FDR curve's supported statistic domain.
- Return the least stringent absolute statistic threshold whose estimated q-value is less than or equal to the requested q-value.
- Be conservative in the presence of interpolation noise by enforcing or searching a monotone q-value envelope.
- Report an unavailable or out-of-range request without changing the existing threshold.

Q-value entry should only be enabled when the selected threshold column carries an AFNI FDR curve. No q-value should be synthesized from p-value metadata alone.

#### Threshold-entry tests

- Decimal and scientific-notation parsing
- Invalid, zero, negative, greater-than-one, NaN, and infinite inputs
- Round trips from p to statistic to p for each supported statistic family
- Q-value inversion at curve samples and between samples
- Conservative behavior for a slightly non-monotonic FDR curve
- Missing stat metadata and missing FDR curves
- Editor Apply, Enter, Escape, focus loss, and unchanged-value behavior
- Semantic p/q threshold preservation when switching overlays with different metadata

## 2. InstaCorr

### Implementation status (2026-09-26)

The first SUMA-parity implementation is now in place:

- `D` creates or toggles an InstaCorr session for the active surface time-series overlay and opens a dedicated floating controls panel.
- Draft SUMA preprocessing, TR, polort, and bandpass settings do not affect the live result until **Recalculate** is pressed.
- Defaults match SUMA: preprocessing enabled, polort 2, 0.01–0.1 Hz, and L2 normalization.
- The preprocessing switch reproduces SUMA's broad `normalize_dset` gate. When enabled, preprocessing follows `SUMA_dot.c`/`THD_bandpass_vectors`: linear detrending, Legendre polynomial regressors, next-even-length FFT filtering, AFNI boundary tapering, filtered-regressor projection, and L2 normalization. When disabled, all dependent controls are disabled and InstaCorr computes the raw seed-by-node dot product without detrending, filtering, or normalization.
- NIML `ni_timestep` and GIFTI `TimeStep`/`AFNI_TimeStep` metadata now populate the editable TR field. Missing TR opens the panel and waits for valid user input.
- Preprocessing runs in a background worker with generation IDs; seed changes reuse the cached matrix.
- The result is a generated overlay in the normal overlay selector, updated in place with symmetric display and AFNI `Correl(samples,1,nort)` metadata.
- Ordinary right-clicks, Go to Vertex/XYZ, and AFNI crosshair updates recalculate the active session. Shift-right-click moves only the crosshair.
- Paired left/right time series already share the combined canonical dataset, so one session calculates both hemispheres.
- Suspending leaves the result available; removing the generated result ends its session.

Still pending for later iterations: nuisance-regressor file input, result export, cancellation of a running FFT beyond stale-generation rejection, and numerical fixture comparison against an AFNI/SUMA run.

### Original SUMA behavior

The uppercase-D handler in `SUMA_input.c` does more than calculate a correlation once:

- It creates a Dot/InstaCorr transform the first time.
- It makes the result a new current overlay.
- It registers a node-selection callback.
- Each subsequently picked node becomes the seed and refreshes the result overlay.
- Pressing `D` again toggles the live behavior.
- The result defaults to symmetric correlation coloring.

The central operation in `SUMA_dot.c` is a matrix-vector dot product after preprocessing and normalization. This produces Pearson-like correlation coefficients when the node and seed series are normalized. Full SUMA also supports detrending, temporal filtering, and nuisance regressors.

### Proposed Sumaru design

Implement the numerical work in a viewer-independent analysis module, such as `src/analysis/instacorr.rs`:

```text
InstaCorrSession
|-- source_overlay_id
|-- seed_node
|-- enabled
|-- preprocessing_options
|-- preprocessed_matrix_cache
`-- result_overlay_id
```

The multi-overlay stack is an enabling dependency: an InstaCorr result should be a synthetic overlay entry rather than special-case viewer state.

### First milestone (implemented)

- Require a surface time-series dataset.
- Normalize every node's series and cache the resulting node-by-time matrix.
- Normalize the selected seed series.
- Calculate all correlations with one matrix-vector operation.
- Store the result in a synthetic one-column `Dataset`.
- Add that dataset to `OverlayStack`.
- Update the same result overlay whenever the selected node changes.
- Use uppercase `D` for SUMA-compatible enable/disable behavior.
- Default to a symmetric correlation range and correlation-stat metadata.
- Handle dense and sparse node indexing, missing data, NaNs, and constant time series explicitly.

Sumaru already recognizes surface time-series datasets, and `src/stats.rs` understands AFNI correlation-stat metadata.

### Full parity follow-up

- [x] Retain time-axis and TR metadata during import.
- [x] Polynomial detrending.
- [x] Bandpass filtering.
- [ ] Orthogonal nuisance-regressor input.
- [x] Paired-hemisphere source selection.
- [x] Degrees-of-freedom and correlation-stat metadata.
- [x] Background calculation with generation IDs and stale-result rejection.
- [ ] Active cancellation of superseded preprocessing work.
- [ ] Numerical comparison against saved original-SUMA results.
- [ ] Export of the generated correlation dataset.

The preprocessed and normalized node matrix should be cached. Each new seed then requires an operation proportional to nodes times time points. This calculation should not block rendering or input for large datasets.

### Tests

- Small known matrices with self-correlation of 1
- Inverse signals with correlation of -1
- Constant, missing, and NaN-valued series
- Dense and sparse node mappings
- Cache invalidation when the source data or preprocessing changes
- Result refresh after node selection
- Reference outputs generated by original SUMA

## 3. Tractography and Graph Datasets

Tractography and graph data should use separate canonical data models rather than being forced into the surface `Dataset` or `SurfaceMesh` types.

Implementation status (2026-09-26): the first working slice is now present.
Sumaru reads ASCII and endian-aware binary `TAYLOR_TRACT_DATUM` networks,
parses full/`tri`/`tri_diag`/sparse `Graph_Bucket` encodings and embedded node
coordinates, preserves relative `network_file` links, and loads either format
as an independent scene object. The viewer provides startup flags and file
pickers, duplicate-path refresh, an Objects-style control list, visibility,
opacity, removal, three tract color modes, per-bundle visibility/opacity, and
adjustable screen-space ribbon width. Segment data stays compact on the GPU and
is split across buffers when an object exceeds the adapter's per-buffer limit.
Graph nodes use shaded billboard-sphere glyphs and straight ribbon edges. The
graph inspector selects one named edge measure, applies an absolute-value
threshold while retaining sign for coloring, selects signed or magnitude
coloring, and controls node size. Graph labels are projected into the view,
graph picks report the selected measure and value, and tract picks use a
per-streamline BVH before testing segments. Relative `network_file` links are
now resolved automatically. Matching `Bundle_Tag` and `Bundle_Alt_Tag` against
SUMA edge indices allows the graph inspector to switch between straight edges
and linked tract bundles; the graph measure still supplies threshold, color,
and pick values, and unmatched edges fall back to straight segments. A floating,
scrollable matrix view now shares those measure, threshold, and color controls;
it supports full/lower-triangle presentation, adjustable cells, exact tooltips,
and edge selection synchronized with the normal graph selection readout.

### Tractography

#### Original SUMA format and behavior

The old model in `src/ptaylor/TrackIO.h` is a network containing bundles, with each bundle containing variable-length polylines. AFNI `.niml.tract` uses the custom variable-length `TAYLOR_TRACT_DATUM` row type.

SUMA supports large line collections, selection, line styles and widths, length filters, and hide/gray/dim/hair mask modes. On load it generates three RGB color datasets: local segment orientation, whole-tract midpoint orientation, and bundle ID. The orientation modes map the absolute X, Y, and Z direction components to red, green, and blue. The supplied SUMA screenshot appears to use local-orientation coloring over an anatomical volume slice.

Coordinate behavior is intentionally simple in the original implementation. `3dTrackID` temporarily reorients its working datasets to RAI, and `Create_Tract_NEW` writes world-space points using the AFNI dataset origin and a half-voxel offset. `NIel_2_Tracts` later copies those stored coordinates without changing them, and the SUMA renderer submits the tract points directly to OpenGL. A tract display object is assigned the `ANY_ANATOMICAL` state, allowing it to appear with either surface or volume objects. The optional embedded grid and `atlas_space` describe and validate the coordinate context; they are not an instruction for an additional display-time axis flip.

#### Proposed data model

```text
Tractogram
|-- bundles
|   |-- id, labels, and color
|   `-- streamline range
|-- streamline_offsets
|-- points: Vec<[f32; 3]>
`-- coordinate_space_metadata
```

Flat point and offset arrays should be preferred over one allocation per streamline for efficient upload and rendering.

#### Initial tractography milestone

- [x] Parse AFNI `.niml.tract`, including custom ASCII and binary rows.
- [x] Render streamlines as adjustable-width screen-space ribbons.
- [x] Avoid permanently expanded CPU triangle geometry; upload compact segment instances and split buffers at adapter limits.
- [x] Expose width in screen points with high-DPI scaling.
- [x] Provide SUMA-compatible local-orientation, midpoint-orientation, and bundle color modes.
- [x] Add per-bundle visibility and opacity.
- [ ] Add explicit per-bundle color editing; bundle-ID colors are currently automatic.
- [x] Add BVH picking that reports object, bundle, streamline, length, and nearest position.
- [x] Add repeatable `--tract PATH` CLI arguments and a file-picker action.
- [ ] Retain embedded grid/atlas coordinate metadata and expose compatibility warnings plus an override.

WebGPU does not expose general line-width state. The initial renderer should therefore expand each segment in clip/screen space in the shader, using the original point buffer plus a compact segment-index buffer. This keeps width stable while zooming and avoids the memory cost of storing expanded ribbon triangles for large tractograms.

#### Later tractography work

- Refined joins and caps, optional tube rendering, and data-driven per-streamline thickness
- Length and bundle filtering
- SUMA-like hide/dim/gray/hair masks
- Per-streamline data coloring
- Selection highlighting and richer tract-pick metadata
- Decimation or level-of-detail handling for very large data

For the AFNI-only first release, `Auto` coordinate mode means treating stored `.niml.tract` points as AFNI RAI/DICOM world coordinates, matching SUMA rather than applying a guessed axis conversion. Embedded grid and `atlas_space` metadata should be retained and used for compatibility checks. If metadata conflicts with the loaded surface or volume, warn rather than silently transforming the tract. An object-level coordinate override remains available for ambiguous or externally modified files.

The FATCAT demo directory `/Users/molfesepj/FATCAT_DEMO/DTI` supplies realistic validation data: thirteen binary `.niml.tract` files ranging from 944 to 205,402 streamlines and from approximately 400 KB to 81 MB, seventeen full-matrix `Graph_Bucket` datasets, node coordinates, grids, volumes, and relative `network_file` links. Compact test fixtures should be derived from these files for the repository, while the whole-brain file should be used as a local stress test. Additional synthetic fixtures remain necessary for triangular and sparse graph encodings because this demo's graph matrices are all marked `full`. Acceptance tests must cover tracts over a surface, tracts over an anatomical volume slice, and a tract-only scene.

### Graph datasets

#### Original SUMA format and behavior

Original SUMA recognizes `Graph_Bucket` datasets. Matrix/index metadata is interpreted in `suma_datasets.c`. Sparse graph datasets contain an edge index plus separate P1 and P2 endpoint columns. Graph-node positions, names, groups, and colors come from `NODE_COORDS`.

SUMA supports:

- 3D node and edge display
- Matrix display
- Edge and node selection
- Thresholding and coloring of edge values
- Edge thickness and stippling
- Showing or hiding unconnected nodes
- Rendering linked tract bundles in place of straight edges

#### Proposed data model

```text
GraphDataset
|-- nodes: id, position, label, group, color
|-- edges: id, source, target
|-- edge_value_columns
|-- matrix_shape_metadata
`-- optional_tract_bundle_references
```

Do not overload the existing picked-node graph/plot dock with this model.

#### Initial graph milestone

- [x] Parse full, triangular, triangular-with-diagonal, and sparse encodings.
- [x] Preserve matrix shape and named measure columns for inspection.
- [x] Render 3D nodes and adjustable-width straight edges.
- [x] Reuse threshold, signed/magnitude color, opacity, and selected-measure concepts.
- [x] Add edge and node selection.
- [x] Add `--graph`/`--gdset` CLI arguments and file-picker actions.
- [x] Resolve relative `network_file` links and offer SUMA-compatible bundle-backed edges.
- [x] Add a separate floating full/lower-triangle matrix view with shared measure, threshold, color, and selection state.
- [ ] Preserve and present graph node group/color metadata beyond labels and positions.

#### Later graph work

- Label collision handling and graph group coloring
- Edge-width mapping
- Showing or hiding unconnected nodes
- Export support

### Scene architecture

Implementation status: **partial**. Tract and graph objects now share an
independent scene-object list and renderer, but surfaces and volumes have not
been migrated into that same abstraction. Such a migration is optional unless
a future feature needs one lifecycle across every object type.

Generalize the viewer toward independent scene objects instead of placing tract and graph geometry into `SurfaceMesh`:

```text
SceneObject
|-- Surface
|-- Volume
|-- Tractogram
`-- Graph
```

Each object type should have its own canonical data and render buffers while sharing transforms, visibility, selection, and scene lifecycle where appropriate.

Tractograms and graphs must work in a standalone scene and alongside either surface or volume objects. Opening another object adds and activates it; reopening the same canonical path refreshes its existing object. A dedicated Objects panel owns visibility, activation, removal, and type-specific inspectors.

Graph edge data may reuse a generic mapped-data appearance containing selected column, range, threshold, colormap, and opacity. It should not be forced to become a surface overlay.

## 4. Crosshair Synchronization Between Cloned Viewers

### Current Sumaru behavior

The duplicate-window action does not create another view inside the same process. `ViewerState::spawn_duplicate_instance` launches a separate Sumaru process containing the current surface or spec and surface volume, but no overlay or ROI. Consequently, cloned viewers do not share `InteractionState` or `CrosshairState` in memory.

The existing crosshair state already carries the useful payload:

- Vertex index
- Triangle index
- Untransformed surface position

The existing Go to Location implementation in `src/viewer/edit.rs` can already move a viewer either to a vertex index or to the vertex nearest an XYZ coordinate. Remote synchronization should reuse the same selection-application path so the crosshair, picked values, plot dock, AFNI communication, and eventually InstaCorr callbacks update consistently.

### Original SUMA behavior

Original SUMA exposes three crosshair lock modes in `SUMA_display.h`:

- No lock
- Node-index lock, which is topology based and selects the same node on related surfaces
- Coordinate lock, which is geometry based and selects the same XYZ location

Sumaru should retain those explicit choices and add an Auto mode that handles the common cloned-view case safely.

### Vertex versus XYZ synchronization

Neither representation is sufficient by itself:

- Vertex identity is correct for two representations with the same topology, such as pial, white, smoothwm, and inflated versions of the same hemisphere. The geometry can differ dramatically while vertex 1234 still represents the corresponding cortical location.
- XYZ is correct for geometrically registered objects that do not share topology, including a surface and volume slice, or two unrelated meshes in a common world space.
- XYZ alone is a poor default for pial-to-inflated synchronization.
- Vertex alone is unsafe when surfaces have different topology or refer to different hemispheres.

The recommended default is therefore `Auto`:

1. Use the vertex index when the target has the same hemisphere and a compatible topology identity.
2. Otherwise use world/anatomical XYZ and select the nearest target vertex or update the target slice location.
3. Warn or decline the coordinate fallback when the nearest target vertex is beyond a configurable distance tolerance.

Every synchronization event should carry both representations:

```text
CrosshairSyncEvent
|-- session_id
|-- origin_view_id
|-- sequence
|-- source_surface
|   |-- surface/domain lineage
|   |-- hemisphere
|   `-- topology_id
|-- vertex_index
|-- face_index
`-- world_xyz_ras
```

`topology_id` should be a stable identity derived from the mesh connectivity and node count, rather than trusting equal vertex counts. Surface/domain lineage can provide a faster known-related match when available.

### Viewer session and transport

Clones need a small local inter-process session channel:

- The originating viewer creates or joins a viewer session.
- A duplicate inherits the same session identifier and local endpoint.
- A duplicate created from another duplicate remains in that session.
- A fresh empty window remains independent unless explicitly joined.
- Crosshair messages remain local to the machine and use a per-session token.
- Each event has an origin and sequence number so receivers apply it once and never rebroadcast it.
- If the session broker disappears, viewers continue independently and show a disconnected state rather than exiting.

A loopback TCP or Unix-domain socket broker is sufficient; this does not require AFNI to be running. AFNI NIML crosshair support remains a separate external integration and can feed the same local `apply_crosshair` path.

### Proposed UI

Add a compact crosshair-lock control near the existing crosshair or clone controls:

```text
Crosshair lock:  Off | Auto | Vertex | XYZ       2 views
```

- `Off`: selection remains local.
- `Auto`: topology/vertex first, coordinate fallback.
- `Vertex`: synchronize only when compatible topology can be proven.
- `XYZ`: always synchronize by world coordinate.
- A small status indicator shows session membership and connection state.
- Camera orientation, zoom, overlay selection, and threshold settings remain independent. Camera/view locking can be considered separately.

Duplicated viewers join the same session automatically and start with `Auto` crosshair synchronization enabled. The lock mode remains explicitly visible so it is easy to change to Off, Vertex, or XYZ. A fresh empty viewer does not join the session automatically.

### Crosshair synchronization tests

- Clone joins the parent's session; a fresh window does not
- A clone made from a clone remains in the same session
- Same-topology pial/inflated views select the same vertex
- Different topology falls back to nearest world coordinate in Auto mode
- Hemisphere mismatch does not accidentally reuse a same-numbered vertex
- Coordinate-distance tolerance and warnings
- Off, Auto, Vertex, and XYZ behavior
- No echo loops or duplicate processing
- Clean behavior when a member or broker closes
- Right-click picks, Go to Vertex, Go to XYZ, pasted coordinates, and AFNI-originated crosshairs use the shared application path
- Synchronized picks trigger per-view plot and InstaCorr refresh only when those features are locally active

## Implementation Record and Remaining Queue

Completed foundation:

- [x] Active-only multiple-overlay stack, selector, refresh, removal, cycling, and threshold-transfer preferences.
- [x] Self-documenting `~/.sumaru` creation and preferences panel.
- [x] Live synthetic InstaCorr overlay with configurable SUMA preprocessing and cached background computation.
- [x] AFNI `.niml.tract` parsing, independent scene objects, ribbon rendering, controls, and picking.
- [x] `Graph_Bucket` parsing, 3D nodes/edges, graph controls, labels, and picking.
- [x] SUMA primary/alternate tag matching for linked graph tract bundles.
- [x] Floating graph matrix view.

Next required implementation work:

1. [ ] Typed p-value entry and semantic p-value threshold intent.
2. [ ] Conservative FDR inversion and typed q-value entry.
3. [ ] Viewer-session transport and Auto/Vertex/XYZ cloned-window crosshair locking.
4. [ ] Reference SUMA numerical fixtures for InstaCorr.
5. [ ] InstaCorr AFNI `.1D` nuisance regressors and result export.
6. [ ] Tract coordinate metadata validation, warnings, and override UI.
7. [ ] Graph node group/color metadata support.

Explicitly later or optional:

- [ ] True multi-plane overlay compositing.
- [ ] Advanced tract filtering, mask modes, selection styling, joins/caps, and level of detail.
- [ ] Graph edge-value width/stipple mapping, unconnected-node controls, and export.
- [ ] Joining an already-running independent viewer to a synchronization session.
- [ ] TrackVis, MRtrix, CIFTI, and CSV support.

## Resolved Design Decisions

- A normal click on the displayed p-value or q-value opens its numeric editor.
- Right-click on the p-value or q-value opens presets and advanced threshold choices.
- Multiple-overlay rendering starts in active-only mode; compositing is deferred.
- Opening a new source adds and activates it; reopening a source refreshes its existing entry.
- Left/right pairs are one logical overlay entry and list order follows load order.
- Threshold switching defaults to keeping the current numeric slider value.
- When statistical metadata is incompatible, keep that value and display `p --` and `q --`.
- The overlay selector lives in the overlay workbench, with Control-PageUp/Control-PageDown cycling.
- Duplicated viewers will automatically join the originating viewer's local session.
- Crosshair synchronization will start enabled in `Auto` mode for cloned viewers.
- A fresh empty viewer will remain independent unless explicitly joined later.
- InstaCorr preprocessing parity is required in the first release, not deferred.
- `D` opens a dedicated floating controls panel; docking can be reconsidered after the workflow has been exercised.
- The panel presents one `SUMA preprocessing` switch plus editable TR, polort, and bandpass controls. Turning the switch off reproduces SUMA's `normalize_dset` gate: the dependent controls are disabled and the result is a raw dot product.
- Parameter edits are staged until **Recalculate**; ordinary seed changes reuse the last applied settings.
- Ordinary right-click updates the active seed; Shift-right-click moves the crosshair without recalculating.
- One paired left/right logical overlay produces one paired result; unrelated overlays are not paired heuristically.
- Generated results update in place, remain available while suspended, and end their session when removed.
- Initial tract and graph support is limited to AFNI/SUMA `.niml.tract` and `Graph_Bucket .niml.dset`; TrackVis, MRtrix, CIFTI, and CSV are deferred.
- Tractography is implemented before graph rendering so both use the same compact ribbon and picking infrastructure.
- Tracts and graphs are independent scene objects that can display alone or alongside surfaces and volumes; reopening the same path refreshes the existing object.
- Tract coordinates default to AFNI RAI/DICOM world coordinates as stored, matching SUMA. Future metadata handling will provide compatibility warnings and an explicit override instead of guessed transforms.
- Adjustable-width screen-space tract ribbons are required in the initial milestone.
- The graph view uses embedded `NODE_COORDS` for 3D nodes and offers a separate floating matrix window; external coordinate sources remain deferred.
- Graph-to-tract links resolve relative to the graph dataset and can replace matching straight edges with bundles using SUMA's primary/alternate edge-tag lookup.

## Open Design Questions

1. Should crosshair synchronization remain limited to windows created through Duplicate, or should an existing independent window be able to join a named or selected local session?

2. For the next InstaCorr iteration, should nuisance regressors be loaded from an AFNI `.1D` file?

## Relevant Source Locations

### Current Sumaru

- `src/dataset.rs`: canonical surface dataset and column model
- `src/overlay.rs`: core overlay state
- `src/viewer/mod.rs`: viewer state, active render cache, and command application
- `src/viewer/overlay_load.rs`: overlay loading and refresh behavior
- `src/viewer/overlay_stack.rs`: ordered active-overlay selection and threshold transfer
- `src/viewer/mesh.rs`: `OverlayAppearance`
- `src/viewer/input.rs`: keyboard bindings
- `src/viewer/graph.rs`: current picked-node plot dock
- `src/viewer/edit.rs`: Go to Vertex/XYZ and nearest-node application path
- `src/io/niml.rs`: NIML parsing and canonical payload construction
- `src/spec.rs`: surface-oriented spec parsing
- `src/stats.rs`: AFNI statistical metadata, including correlation
- `src/instacorr.rs`: SUMA-compatible preprocessing and cached dot products
- `src/viewer/instacorr.rs`: sessions, generated overlays, background work, and controls
- `src/preferences.rs`: persistent `~/.sumaru` preferences
- `src/tractography.rs`: AFNI `.niml.tract` model and parser
- `src/graph_dataset.rs`: `Graph_Bucket` model, parser, and matrix access
- `src/viewer/scene_objects.rs`: tract/graph GPU rendering, linked bundles, and picking
- `src/viewer/ui.rs`: tract/graph inspectors and floating graph matrix window
- `src/command.rs`: `InteractionState`, `SurfacePick`, and `CrosshairState`

### Original AFNI/SUMA

- `src/SUMA/SUMA_xColBar.c`: overlay popup and color-plane switching
- `src/SUMA/SUMA_Color.c`: overlay preservation, ordering, and mixing
- `src/SUMA/SUMA_input.c`: uppercase-D InstaCorr interaction
- `src/SUMA/SUMA_dot.c`: preprocessing and dot-product calculation
- `src/ptaylor/TrackIO.h` and `TrackIO.c`: tract network format
- `src/suma_datasets.c`: graph dataset recognition and conversion
- `src/SUMA/SUMA_CreateDO.c`: tract and graph display objects and rendering
- `src/SUMA/SUMA_Load_Surface_Object.c`: tract display-object loading
- `src/SUMA/SUMA_display.h`: topology-based and coordinate-based crosshair lock semantics
