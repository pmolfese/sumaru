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

The type-layer cleanup is largely finished, so the dominant remaining problem is
**file/structure size**, not duplicate types. Ranked by payoff:

1. **Split `viewer/mod.rs`** — the single largest readability win left. ~11.8k
   lines / ~6k-line `impl ViewerState`. Nothing else moves the "can a new reader
   find anything" needle as much. (Item A)
2. **Finish `WindowPane`** — the field collapse landed, but the four copies of
   resize/redraw/repaint logic are still inline. Folding them into methods is
   low-risk, deletes real duplication, and is the natural completion of work
   already in the tree. (Item B)
3. **Deepen `OverlayState`** — the wrapper groups the fields, but still mixes
   four lifetimes (source identity, dataset, derived scalars, render cache).
   Splitting them clarifies the most-touched data in the app. (Item C)
4. **Tidy the ROI render-side type cluster** — nine `Roi*` view structs; some
   are a snapshot and its target holding the same data. Smaller, more localized
   win. (Item D)

Do them roughly in this order: **B is a quick completion**, **A is the big
structural payoff** (and is much easier *after* B shrinks the per-window noise),
then **C**, then **D**.

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
- [ ] **Still open:** relocate the ~45 small local structs (`LoadedOverlay`,
      `SceneStats`, `GraphSnapshot`, `MontageShot`, …) to the module that owns
      them. Deferred — structs are referenced across modules and would need
      `pub(super)`/`pub(crate)` visibility bumps; lower value than the method
      split and best done as its own pass. The `draw_*` UI cluster and
      `pick_*` methods also remain in `mod.rs` (the draw methods are heavily
      interleaved; a `viewer/ui.rs` split is a reasonable future step).

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

### D. Tidy the ROI render-side type cluster

`RoiLayer`, `RoiWorkspace`, `RoiSlot`, `RoiDraft`, `RoiDraftTarget`,
`RoiDraftSnapshot`, `RoiPickTarget`, `RoiAppearanceBuild`, `RoiComponentRange`
([`viewer/mod.rs:7499`](src/viewer/mod.rs)+) sit alongside the domain `roi::Roi`.

- [ ] Audit `RoiDraft` / `RoiDraftTarget` / `RoiDraftSnapshot` for merging — a
      snapshot and its target are often the same data captured at two moments.
- [ ] Confirm each remaining type earns its keep; collapse any that are a struct
      wrapping a single field used in one place.

*Risk:* low, but re-read the draw/undo paths first. Most localized of the
remaining items — good "small commit" filler between the larger ones.

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
   Done on `refactorWindowPaneB`; struct relocation still open.
3. ✅ **C** — deepen `OverlayState`: rename + minimal grouping done on
   `refactorWindowPaneB`. Strong-enum / move-`OverlayAppearance` steps deferred.
4. **D** — tidy the ROI type cluster as small-commit filler.

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
