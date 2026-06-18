# Refactor Plan

A streamlining pass focused on **combining types that pull single duty**,
**collapsing repeated field groups**, and **making `viewer/mod.rs` navigable** —
without cutting any functionality. Every item preserves behavior; the goal is
fewer parallel representations of the same concept and a smaller, more readable
viewer.

The original Tier 2 (value-type merges) and the core of Tier 1 (field-group
collapses) are **done** — see [Completed work](#completed-work) at the bottom.
What follows is the remaining work, reordered so the **biggest
interpretability / maintainability wins come first**.

## Where the leverage is now

The type-layer cleanup is finished (Tiers 1–2), and the `viewer/mod.rs` method
split, `ui.rs`/`input.rs` extraction, and most struct relocation have landed.
`mod.rs` is down to **~7,800 lines** (from ~12.2k). The remaining leverage,
ranked by payoff:

1. **Split `src/surface.rs`** — *new top item.* At ~4,180 lines it is now the
   single largest god-file in the tree (the viewer is already split). It bundles
   ~40 types and 31 impls across unrelated concerns: mesh core, topology, masks,
   spatial transforms, volume space, and surface↔surface mapping. The `io/`
   split is the proven template. (Item E)
2. **Finish struct relocation (A1 loose ends)** — the scene/ROI/overlay clusters
   moved, but a handful of single-owner types still sit in `mod.rs`
   (graph/afni/capture/pairing types, plus `InputResponse`→`input.rs` and the
   UI-only `ControlUiOutput`/`LaunchButtonIcon`/`paint_launch_button`/`stat_row`
   →`ui.rs`). Small, mechanical, and completes the "everything about X in one
   file" story. (Item A1)
3. **Group `ViewerState`'s 56 fields** — still a wide struct. Cluster the loose
   fields (volume + slice-drag, ROI editing state, scene/buffers/caches) into
   nested sub-structs the way overlay state already was. (Item F)
4. **Deepen `OverlayState` / tidy remaining ROI types** — the deferred reaches
   from Items C and D (strong enum for overlay content, move `OverlayAppearance`
   out of `mesh.rs`). Lower urgency. (Items C/D leftovers)

Do them roughly in this order: **E is the big structural payoff** now that the
viewer is tamed, **A1 is a quick co-location completion**, then **F**, then the
C/D leftovers.

---

## Remaining work

### A. Split `viewer/mod.rs` along its existing seams — method split ✅ DONE (branch `refactorWindowPaneB`)

`impl ViewerState` was ~6,000 lines in a 12.2k-line file.

- [x] Moved six cohesive method clusters into `impl ViewerState` blocks in topical
      submodules, each with a module doc-comment and a brief `///` on every moved
      method. Methods are exposed `pub(super)` so the parent (and sibling
      submodules) keep calling them unchanged:
      - `viewer/afni.rs` — AFNI/SUMA NIML talk (18 methods, ~636 lines)
      - `viewer/capture.rs` — screenshot/montage capture + camera framing (~411)
      - `viewer/overlay_load.rs` — overlay load + column/appearance refresh (~383)
      - `viewer/roi.rs` — drawn-ROI editing, fill, save, `load_roi_path` (~465)
      - `viewer/pairing.rs` — paired-hemisphere drag/transform/layout (~230)
      - `viewer/graph.rs` — graph window/dock + snapshot (~145)
      Net: `mod.rs` 12,167 → 10,021 lines (~2,150 moved). All ~216 tests pass,
      fmt clean, clippy unchanged.
- [ ] **Still open:** relocate the ~50 local structs to their owning module, and
      split the `draw_*` UI cluster into `viewer/ui.rs`. These are deferred
      navigability follow-ons — see [A-follow-up](#a-follow-up--struct-relocation--uirs-split-deferred)
      below for the full outline, rationale, and reduction estimate.

*Risk:* low per move (pure relocation, no logic change). Done one cluster at a
time, compiling + testing after each.

*Why first among the structural items:* every later edit to the viewer pays the
"scroll through 11.8k lines" tax. This is the change that compounds.

### B. Finish `WindowPane` — dedupe per-window logic + constructor args — ✅ DONE (branch `refactorWindowPaneB`)

The field collapse was already done (four `WindowPane`s, `ViewerState` 80 → 59
fields). Both remaining pieces are now complete:

- [x] **Deduplicated resize/redraw/repaint** into `WindowPane` methods:
      - `WindowPane::resize(device, size) -> bool` — the four `resize_*` methods
        are now one-line delegates (view additionally rebuilds the depth buffer
        on `true`).
      - `WindowPane::take_egui_input()` — the shared viewport-sync + raw-input
        head of every egui render path.
      - `WindowPane::present_egui_frame(device, queue, jobs, descriptor, label)`
        — the identical GPU tail (acquire texture → upload textures → render
        pass → free → present, returning `RenderStatus`) that `render_control`,
        `render_roi_control`, and `render_graph` each duplicated (~55 lines ×3).
      Net `mod.rs` −114 lines.
- [x] **Bundled `ViewerState::new`'s 17 arguments** into `ViewerWindows` (the
      four windows) and `InitialScene` (the eight `initial_*` load options),
      taking the constructor to 7 params. The clippy `too_many_arguments`
      warning on `new` is cleared.

All ~210 tests pass, fmt clean. (Remaining lib clippy warnings are pre-existing
on this branch — `desired_panel_size`/`pick.rs` arg counts, two `io.rs` lints,
and a `label_dataset` expect from the lookup-table commit — none introduced
here.)

### C. Deepen `OverlayState` — separate the four lifetimes (rename + grouping ✅ DONE, branch `refactorWindowPaneB`)

`ViewerState`'s `ViewerOverlayState` was a flat eight-field struct mixing source
identity, canonical dataset, derived scalars, and the render cache.

- [x] **Rename pass:** `model` → `render_model`, `values` → `node_values`,
      `dataset` → `canonical_dataset`. Clarifies intent at every read site
      (~101 accesses across the viewer submodules).
- [x] **Minimal grouping:** `ViewerOverlayState` is now three nested structs by
      lifetime, each documented field-by-field:
      - `OverlaySourceInfo { path, pair_paths, display_name }` — provenance.
      - `DatasetOverlayState { canonical_dataset, columns, node_values }` —
        canonical data + the scalars derived from it (they recompute together).
      - `OverlayRenderCache { render_model, appearance }` — what the GPU upload
        consumes.
      Access paths are now `self.overlay.source.*` / `.data.*` / `.render.*`.
      All three groups `#[derive(Default)]` (render cache keeps a manual
      `Default` for the seeded appearance range). All ~216 tests pass, clippy
      unchanged, fmt clean.

Still open (deferred, higher reach):

- [ ] **Strong enum:** make loaded content explicit as either canonical-dataset
      overlay data *or* AFNI-baked RGBA cache. Encodes the real invariant but
      touches the most call sites.
- [ ] **Move display state:** promote `OverlayAppearance` out of `viewer/mesh.rs`
      into a reusable overlay/display module so viewer UI, AFNI interop, and
      future GPU shader recoloring share one display-state type. (Pairs well with
      the GPU/shader work in `ROADMAP.md`.)
- [ ] Audit which sub-parts must be `Option` independently vs. which always live
      and die together (`render_model`/`canonical_dataset`/`node_values` likely a
      unit — candidates for the strong-enum step).

*Risk:* moderate — touched the load/unload path and many readers; done as a
mechanical anchored rewrite with the test suite as the safety net.

### D. Tidy the ROI render-side type cluster — ✅ DONE (branch `refactorWindowPaneB`)

`RoiLayer`, `RoiWorkspace`, `RoiSlot`, `RoiDraft`, `RoiDraftTarget`,
`RoiDraftSnapshot`, `RoiPickTarget`, `RoiAppearanceBuild`, `RoiComponentRange`
sat alongside the domain `roi::Roi`.

- [x] **Merged `RoiDraftSnapshot` into a nested `RoiDraftState`.** It was an exact
      copy of `RoiDraft`'s seven editable fields; `snapshot()`/`restore()`
      hand-copied each. Now `RoiDraft` holds `state: RoiDraftState` plus
      `history`/`redo_history: Vec<RoiDraftState>`; `snapshot()` is
      `self.state.clone()` and `restore()` is `self.state = snapshot`. Removes the
      "add a field in three places or undo silently drops it" hazard — the real
      maintainability win. ~106 field accesses moved under `.state` (anchored
      rewrite; the 24 ROI undo/redo/fill tests are the safety net).
- [x] **Confirmed the rest earn their keep:** `RoiAppearanceBuild` (4-field
      builder return), `RoiComponentRange` (multi-field, 11 uses), and
      `RoiPickTarget` (mesh + target + local node) are all genuine multi-field
      bundles, not single-field wrappers — kept as-is.

*Risk:* was low; done as a mechanical rewrite with the ROI test suite as the
safety net. All ~216 tests pass, clippy unchanged, fmt clean.

---

## A-follow-up — struct relocation + `ui.rs` split (deferred)

After the method split, `mod.rs` is ~10,089 lines and the single `impl
ViewerState` still runs ~956→4740 (~3,800 lines). The six submodules use
`use super::*`, so they currently *borrow* ~50 type definitions that still
physically live in `mod.rs`. Two follow-ons finish the job. Both are **pure
navigability** — no behavior change, no hazard removed — which is why they sit
below the correctness-bearing items.

### A1 — Relocate the local structs to their owning module — 🟡 PARTIAL

The big clusters have moved: the ROI types live in `roi.rs`, the overlay-load
types in `overlay_load.rs`, the geometry/transform types in `transform.rs`, and
the 14-type scene/render cluster now lives in `scene.rs`. **Remaining loose
ends** (single-owner types still in `mod.rs`):

| Target module | Types still in `mod.rs` |
|---|---|
| `graph.rs` | `GraphSnapshot`, `GraphPoint` |
| `afni.rs` | `AfniSurfaceTarget`, `AfniViewerOptions` |
| `capture.rs` | `MontageShot`, `MontageLayout`, `MontageCamera` |
| `pairing.rs` | `ComponentTransform` |
| `input.rs` | `InputResponse` |
| `ui.rs` | `ControlUiOutput`, `LaunchButtonIcon` + `paint_launch_button`, `stat_row`, the `pick_*_file` dialogs |

Stays in `mod.rs` (the genuine core): `ViewerState`, `ViewerApp`,
`WindowPane`/`EguiPane`, `ViewerWindows`/`InitialScene`, `ViewerEvent`,
`RenderStatus`, `LaunchOptions`, `PreloadTask`/`PreloadResult`.

*Mechanic:* each move is "cut the `struct` + its `impl` blocks, paste into the
target module." The only friction is **visibility** — a type used by `mod.rs`
*and* a sibling now needs `pub(super)` on the type and any fields the other
module touches. Unlike the method moves, this is per-field, not a blind pass.
Now that `input.rs`/`ui.rs` exist, their helpers are the lowest-friction moves.

---

## E. Split `src/surface.rs` into a `surface/` directory module

`surface.rs` is ~4,180 lines — the largest god-file left now that the viewer is
split. It holds ~40 types and 31 `impl` blocks across concerns that rarely
change together. The `io/` split (a thin `mod.rs` re-export over topical
submodules) is the exact template; internals stay internal.

Natural seams:

| Submodule | Types |
|---|---|
| `surface/mesh.rs` | `SurfaceMesh` (+ its ~820-line impl), `SurfaceMetadata`, `SurfaceLineage`, `SphereMetadata`, `Bounds`, `SurfaceGeometryMetrics` |
| `surface/identity.rs` | `SurfaceId`, `SurfaceDomainId`, `SurfaceDomain`, `SurfaceDomainIdentity`, `SurfaceDomainKind`, `SurfaceKinship`, `SurfaceSide`, `SurfaceKind`, `AnatomicalCorrectness` |
| `surface/topology.rs` | `SurfaceTopology`, `EdgeRecord`, `EdgeFaces`, `WindingReport`, `MeshValidationReport`, `MeshValidationIssue`, `NormalDirection` |
| `surface/mask.rs` | `NodeMask`, `FaceMask`, `BitMask`, `FaceMaskMode`, `SurfacePatch`, `SurfacePath` |
| `surface/transform.rs` | `SurfaceTransform`, `VolumeSpace`, `VoxelIndex`, `VolumeSamplePoint`, `ClipPlane`, `LineSegment` |
| `surface/mapping.rs` | `SurfaceToSurfaceMap`, `SurfaceMappingKind`, `NodeWeights`, `SmoothingWeights` |
| `surface/mod.rs` | thin re-export facade; keep `OverlayDataset`, `ValueRange` here (or hoist later) |

*Risk:* low — pure relocation behind a `pub use` facade, exactly like `io/`.
Do it one submodule at a time, compiling + testing after each. `VolumeSpace`
and `SurfaceTransform` are the natural first cut (self-contained, well-tested,
and the volume feature already leans on them).

*Why now:* it is the single biggest remaining "can a new reader find anything"
file, and it is *not* viewer-coupled, so it is the cleanest large win left.

## F. Group `ViewerState`'s remaining loose fields

`ViewerState` is still ~56 fields. The overlay/window groupings already proved
the pattern (`self.overlay.source.*`, the four `WindowPane`s). Remaining loose
clusters worth nesting:

- **Volume:** `volume_view`, `volume_slice_drag` → a small `VolumeUiState`.
- **ROI editing:** `roi_path`, `roi_layer`, `roi_workspace` → one group.
- **Scene/render:** `surface_scene`, `surface_buffers`, `surface_render_set`,
  `prepared_geometry_cache`, `anatomical_shading_cache`, `mesh` are a related
  set touched together on load/switch.

*Risk:* moderate (many readers); do it as anchored rewrites with the test suite
as the net, the way `OverlayState` grouping was done.

### A2 — Split the `draw_*` UI cluster into `viewer/ui.rs` — ✅ DONE (branch `refactorRS`)

- [x] Moved all 13 `draw_*` methods into an `impl ViewerState` block in
      `viewer/ui.rs` (~1,108 lines): `draw_ui`, `draw_view_overlay_ui`,
      `draw_overlay_workbench`, `draw_overlay_range_controls`,
      `draw_surface_dataset_section`, `draw_scene_section`, `draw_pick_section`,
      `draw_roi_control_contents`, `draw_graph_dock_ui`, `draw_graph_ui`,
      `draw_graph_contents`, `draw_roi_control_ui`, `draw_view_transient_label`.
      Entry points (`draw_ui`/`draw_roi_control_ui`/`draw_graph_ui`) and
      `selected_threshold_range` are `pub(super)`; the rest are private to
      `ui.rs`. UI-only helpers (`ControlUiOutput`, `LaunchButtonIcon`,
      `paint_launch_button`, `stat_row`, the `pick_*_file` dialogs) were left in
      `mod.rs` and are reached via `use super::*` — co-locating them is an A1
      loose end.
- The cross-cutting-reads concern resolved cleanly: only one method
  (`selected_threshold_range`) had a still-in-`mod.rs` caller and needed a
  visibility bump — the compiler surfaced it immediately, which is the seam
  working as intended.

### A3 — Split window input handling into `viewer/input.rs` — ✅ DONE (branch `refactorRS`)

Not in the original plan, but the volume work made it obvious: every interaction
feature edits the `WindowEvent` match.

- [x] Moved `view_input` (mouse/keyboard/camera/pair-drag/ROI-pick/volume-drag
      routing) plus the `control_input`/`roi_control_input`/`graph_input` egui
      passthroughs into `viewer/input.rs` (~272 lines), all `pub(super)`.
      Net `mod.rs` −263 lines.

### A4 — Volume feature glue → `viewer/volume_view.rs` — ✅ DONE (branch `refactorRS`)

- [x] The six `ViewerState` volume handlers (`load_volume_path`,
      `add_volume_slice`, `remove_selected_volume_slice`,
      `select_volume_plane_at_cursor`, `try_begin_volume_slice_drag`,
      `update_volume_slice_drag`) moved into an `impl ViewerState` block next to
      `VolumeView`, so the whole `--volume` feature (data, GPU, handlers) lives
      in `volume.rs` + `volume_view.rs`.

*Session net (`refactorRS`):* `mod.rs` 9,278 → ~7,809 lines. All tests pass,
fmt clean, no new warnings. Pure relocation throughout.

### Why — clarity

- **Co-location.** Today ROI drawing methods live in `roi.rs` but `RoiDraft`
  lives in `mod.rs`. After A1, the type, its `impl`, and the `ViewerState`
  methods that use it share a file — "everything about `RoiDraft`" is one answer.
- **`mod.rs` becomes a table of contents, not an encyclopedia** — its job
  collapses to construct the app, own the windows, run the event loop, and
  orchestrate render/update.
- **Visibility documents the seams.** Marking a field `pub(super)` makes
  cross-module coupling explicit and greppable; today it is invisible because
  everything shares one privacy scope.
- **`ui.rs` isolates the egui layer** so the render/event core is no longer
  interrupted by ~1,050 lines of widget layout (and vice-versa).

### Why — reduction (estimate)

- A2 (`ui.rs`): ~1,050 lines out of `mod.rs`.
- A1 (structs + their impls): ~2,000–2,500 lines (scene/ROI/overlay carry most
  of the `impl` weight).
- Net: `mod.rs` plausibly lands ~6,000–6,500 lines; `impl ViewerState` itself
  shrinks from ~3,800 to ~2,000 lines of genuine lifecycle/orchestration. No
  logic is deleted — this is redistribution, but the largest file roughly halves.

### Sequencing

1. **`scene.rs` first** (new module) — the 14 scene/render types are the densest
   cluster with the clearest boundary; biggest single clarity win.
2. **Relocate the topical structs** into the existing `roi.rs` / `overlay_load.rs`
   / `capture.rs` / `graph.rs` / `pairing.rs` — small, one commit each.
3. **`ui.rs` last** — the big draw split, once overlay/ROI/scene types are
   already `pub(super)` and co-located.

Default to `pub(super)` (not `pub(crate)`) so coupling stays scoped to the
`viewer` module tree. Each step compiles + tests independently, as the method
split did.

---

## Explicitly *not* recommended (yet)

- **`Rgba` vs `[f32; 4]`.** `[f32; 4]` is the GPU/vertex-buffer currency; forcing
  `Rgba` everywhere would add conversions in the hot path for little gain. Keep
  `Rgba` at the color/colormap boundary and `[f32; 4]` at the GPU boundary, with
  conversions centralized (they mostly already are).
- **The `*Id(String)` newtypes** (`SurfaceId`, `SurfaceDomainId`, `RoiId`). These
  do real type-safety work; leave them.
- **Merging the overlay-column-selection types.** Examined and intentionally
  kept separate — see Completed item 6.

---

## Suggested sequencing

1. ✅ **B** — finish `WindowPane` (dedupe resize/redraw + bundle constructor
   args). Done on `refactorWindowPaneB`.
2. ✅ **A** — split `viewer/mod.rs` into topical submodules (method clusters).
   Done on `refactorWindowPaneB`; struct relocation mostly done since.
3. ✅ **C** — deepen `OverlayState`: rename + minimal grouping done on
   `refactorWindowPaneB`. Strong-enum / move-`OverlayAppearance` steps deferred.
4. ✅ **D** — tidy the ROI type cluster (merged `RoiDraftSnapshot` into
   `RoiDraftState`). Done on `refactorWindowPaneB`.
5. ✅ **A2/A3/A4** — `ui.rs` + `input.rs` split, volume glue co-located. Done on
   `refactorRS`.

Remaining, in suggested order:

6. **E** — split `src/surface.rs` into a `surface/` directory module (biggest
   structural win left, low risk, not viewer-coupled).
7. **A1** — relocate the remaining single-owner viewer structs (start with the
   `input.rs`/`ui.rs` helpers — lowest friction).
8. **F** — group `ViewerState`'s remaining loose fields.
9. **C/D leftovers** — overlay strong-enum, move `OverlayAppearance` out of
   `mesh.rs`.

Run `cargo test && cargo clippy --lib && cargo fmt --all -- --check` after each
step; the suite (~210 tests) is the safety net for "no functionality cut."

---

## Completed work

Archived for reference. Branches: `refactor2` (Tier 2), `refactorT1`
(WindowPane/EguiPane), `refactorOverlay` (OverlayState + single source of truth).
All landed with ~210 tests passing, fmt clean.

### 1. Collapse per-window field groups into `WindowPane` — ✅ done (`refactorT1`)

~36 window/egui fields collapsed into four `WindowPane`s, dropping `ViewerState`
from ~80 fields to 59. Two compile-green milestones: `EguiPane` (20 egui
fields → 4) then `WindowPane` (window/surface/config/size/last_requested_size/
repaint_at/frame_rendered/egui). `upload_pending_egui_textures` /
`free_pending_egui_textures` became `EguiPane` methods; `repaint_delay_to_instant`
stayed a free function (operates on egui `FullOutput`, not pane state). The four
`*_window()` accessors were kept (bodies now read `&self.view.window` etc.).

*Remaining follow-through tracked as Item B above* (dedupe resize/redraw, bundle
the 17 constructor args).

**Smoke-test follow-ups (separate commit):** verifying the rename surfaced two
*pre-existing* graph-dock bugs (not caused by the refactor), both fixed: (1) the
docked graph only refreshed on `g` — now follows node picks live; (2) the dock
snapped back when resized because egui's panel-resize state didn't persist —
height is now owned in `graph_dock_height_points` with a self-managed drag
handle, and the 3D viewport reserves that height.

### 2. Unify the overlay state into `ViewerOverlayState` — ✅ wrapper done (`refactorOverlay`)

Eight parallel `overlay_*` fields (`overlay`, `overlay_values`,
`overlay_dataset`, `overlay_columns`, `overlay_appearance`, `overlay_path`,
`overlay_pair_paths`, `overlay_display_name`) grouped into one
`ViewerOverlayState`. `reset_scene_state` became one assignment instead of eight.
Kept mechanical: direct file overlays, paired overlays, AFNI `SUMA_irgba`
overlays, picking, graphing, and controller display all unchanged.

*Deeper restructure (separate the four lifetimes) tracked as Item C above.*

### 3. One source of truth for overlay scalars — ✅ done (`refactorOverlay`)

The same attributes previously lived in three hand-synced structs
(`overlay::Overlay`, `viewer/mesh.rs::OverlayAppearance`,
`command.rs::OverlayCommandState`). Now `ViewerOverlayState.appearance` owns the
display scalars (range/colormap/threshold/opacity/dim + symmetric-range toggle);
`OverlayCommandState` keeps only `visible`. Incoming AFNI overlay updates route
to the viewer as `AfniRouteAction::OverlayState`; outgoing state serializes from
an explicit `AfniOverlayState` snapshot, not controller mirror fields. Removed a
class of "the two copies drifted" bugs.

### 4. One min/max range type instead of five encodings — ✅ done (`refactor2`)

Collapsed `ValueRange` (f32), `OverlayRange` (f64), `ColumnRange` (f64),
`Option<[f32; 2]>`, and ad-hoc tuples down to **two** types along the
domain/render boundary: `ValueRange` (f32, render/UI) and `ColumnRange` (f64,
domain). `OverlayRange` merged into `ColumnRange` (its `contains`/`normalized`/
`validate` methods moved over; the `From` glue impl deleted).
`OverlayCommandState.intensity_range` became `Option<ValueRange>`.

### 5. One threshold type instead of three — ✅ done (`refactor2`)

Merged `OverlayThresholdCommandState` into a single `command::OverlayThreshold`
`{ enabled, absolute, value, hide_failed }`, used by the render appearance and
AFNI wire protocol. (`AfniOverlayState` still wraps it in `Option`, but there the
`Option` means "field present in this partial wire update" — a different concept,
correctly kept.) The richer domain `Threshold`/`ThresholdMode`
(Above/Below/Between/Outside) was left alone — merging it would lose the mode
enum, i.e. cut functionality.

### 6. Overlay-column-selection types — ⏭️ examined, intentionally not merged

`overlay.rs::OverlayColumns` + `ColumnSelection { index, label }` (carries
`String`, not `Copy`) vs. `viewer/mod.rs::OverlayColumnSelections { intensity,
threshold, brightness }` (index-only, `Copy`). A legitimate layer boundary:
`OverlayColumnSelections` is a `Copy` index bundle bound directly to egui
`&mut usize` dropdowns and converted to the labeled domain type at a single
boundary (`canonical_overlay_columns`). Merging would lose `Copy`, force `.index`
at ~15 sites, and complicate UI binding — i.e. *de*-streamline. Left as-is.
