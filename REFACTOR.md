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

### A. Split `viewer/mod.rs` along its existing seams — biggest win

`impl ViewerState` alone is ~6,000 lines. Method-name clustering already reveals
natural modules: `roi_*` (27), `draw_*` (18), `afni_*` (18), `pair*`/`paired_*`
(28), `graph_*` (12), `overlay_*`/`load_*` (20), `pick_*` (10), `capture_*`/`save_*`.

- [ ] Move cohesive method groups into `impl ViewerState` blocks in topical
      submodules (`viewer/afni.rs`, `viewer/roi.rs`, `viewer/overlay_load.rs`,
      `viewer/pairing.rs`, `viewer/graph.rs`). `screenshot.rs`, `mesh.rs`,
      `camera.rs`, `pick.rs`, `gpu.rs` already show the pattern works.
- [ ] Relocate the ~45 small local structs (`LoadedOverlay`, `SceneStats`,
      `GraphSnapshot`, `MontageShot`, …) to the module that owns them.

*Risk:* low per move (pure relocation, no logic change), but high churn — do it
in small, individually-verifiable commits, one method cluster at a time. Easiest
after Item B removes the per-window resize boilerplate.

*Why first among the structural items:* every later edit to the viewer pays the
"scroll through 11.8k lines" tax. This is the change that compounds.

### B. Finish `WindowPane` — dedupe per-window logic + constructor args

The field collapse is done (four `WindowPane`s, `ViewerState` 80 → 59 fields).
Two pieces of the original item remain:

- [ ] **Deduplicate resize/redraw/repaint** into `WindowPane` methods. The four
      windows (view, control, roi_control, graph) still have their resize and
      redraw bodies written out separately; with the fields now bundled, these
      collapse into shared methods on `WindowPane`. Removes the last of the 4×
      duplication this item set out to kill.
- [ ] **Bundle `ViewerState::new`'s 17 arguments** (a standing clippy
      `too_many_arguments` warning). Group the four windows + the `initial_*`
      options into structs. Clears the warning and makes the constructor
      readable.

*Risk:* low. The hard part (field bundling) is already merged; this is
mechanical follow-through with an obvious test signal.

*Why second:* small, finishes work already in flight, and shrinks `mod.rs`
before the big split (Item A) so there's less to relocate.

### C. Deepen `OverlayState` — separate the four lifetimes

`ViewerState` now owns one `ViewerOverlayState`, but it still mixes loaded source
identity, the canonical dataset, derived per-node scalar values, and the
render-ready color cache in one flat struct. Options, lowest-risk first:

- [ ] **Minimal grouping (recommended start):** split the wrapper into nested
      `OverlaySourceInfo`, `DatasetOverlayState`, and `OverlayRenderCache`
      structs, preserving current behavior. Lowest-risk readability pass.
- [ ] **Rename-only pass** (can land first or instead): `model` → `render_model`,
      `values` → `node_values`, `dataset` → `canonical_dataset`. Tiny, clarifies
      intent before any reshape.
- [ ] **Strong enum** (higher payoff, more reach): make loaded content explicit
      as either canonical-dataset overlay data *or* AFNI-baked RGBA cache. Gives
      real invariants but touches more call sites.
- [ ] **Move display state:** promote `OverlayAppearance` out of `viewer/mesh.rs`
      into a reusable overlay/display module so viewer UI, AFNI interop, and
      future GPU shader recoloring share one display-state type. (Pairs well with
      the GPU/shader work in `ROADMAP.md`.)
- [ ] Audit which sub-parts must be `Option` independently vs. which always live
      and die together (`overlay`/`dataset`/`values` likely a unit).

*Risk:* moderate — touches the load/unload path and many readers. High clarity
payoff because this is the most frequently-touched data in the app.

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

1. **B** — finish `WindowPane` (dedupe resize/redraw + bundle constructor args).
   Quick, completes in-flight work, shrinks `mod.rs`.
2. **A** — split `viewer/mod.rs` into topical submodules. The big structural
   payoff; easier once B is done.
3. **C** — deepen `OverlayState` (start with the minimal grouping / rename pass).
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
