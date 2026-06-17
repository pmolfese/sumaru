# Refactor Plan

A streamlining pass focused on **combining types that pull single duty** and
**collapsing repeated field groups** — without cutting any functionality. Every
item below preserves behavior; the goal is fewer parallel representations of the
same concept and a smaller, more navigable `viewer/mod.rs`.

Findings come from the current tree (notably the ~80-field `ViewerState` and the
~6,000-line `impl ViewerState`). Items are grouped by payoff vs. risk.

---

## Tier 1 — High payoff, mostly mechanical

### 1. Collapse the four per-window field groups into a `WindowPane` — ✅ DONE (branch `refactorT1`)

Outcome: the ~36 window/egui fields collapsed into four `WindowPane`s, dropping
`ViewerState` from ~80 fields to 59. Done as two compile-green milestones —
`EguiPane` (20 egui fields → 4) then `WindowPane` (wrapping window/surface/config/
size/last_requested_size/repaint_at/frame_rendered/egui). The `upload_pending_egui_textures`
and `free_pending_egui_textures` free functions became `EguiPane` methods. All
~210 tests pass, fmt clean, clippy unchanged at 4. The four `*_window()` accessor
methods were kept (bodies now read `&self.view.window` etc.).

Not yet done: `ViewerState::new` still takes 17 arguments (a separate clippy
warning) — that's about constructor *parameters*, not fields. Bundling the four
windows + the `initial_*` options into structs would clear it; deferred as an
optional follow-on so this milestone stays a pure field-collapse.

Original notes:

`ViewerState` carries four near-identical sets of fields, one per window (view,
control, roi_control, graph):

- `*_window`, `*_surface`, `*_config`, `*_size`, `last_requested_*_size`,
  `*_repaint_at`, `*_frame_rendered`
- the egui quintet: `*_egui_ctx`, `*_egui_state`, `*_egui_renderer`,
  `*_pending_egui_textures`, `*_allocated_egui_textures`

That's ~40 fields expressing the same shape four times
([`viewer/mod.rs:560`](src/viewer/mod.rs) onward).

- [ ] Introduce `struct EguiPane { ctx, state, renderer, pending_textures, allocated_textures }`
      and fold the existing free helpers (`upload_pending_egui_textures`,
      `free_pending_egui_textures`, `repaint_delay_to_instant`) into methods on it.
- [ ] Introduce `struct WindowPane { window, surface, config, size, last_requested_size, repaint_at, frame_rendered, egui: EguiPane }`.
- [ ] Replace the 4× field groups with `view`, `control`, `roi_control`, `graph`
      of type `WindowPane`. ~40 fields → 4.
- [ ] Resize/redraw/repaint logic that is currently copy-pasted three or four
      times becomes one method on `WindowPane`.

*Risk:* low-moderate (lots of call sites, but each change is a rename). No logic
changes. Biggest single readability win.

### 2. Unify the overlay state held on `ViewerState`
Eight parallel `overlay_*` fields describe one logical thing
([`viewer/mod.rs:609`](src/viewer/mod.rs)):
`overlay`, `overlay_values`, `overlay_dataset`, `overlay_columns`,
`overlay_appearance`, `overlay_path`, `overlay_pair_paths`,
`overlay_display_name`.

- [ ] Group them into a `struct OverlayState { … }` (kept as a single field, or
      `Option<OverlayState>` for the load/unload lifecycle). `reset_scene_state`
      becomes one assignment instead of eight.
- [ ] Audit which of these must be `Option` independently vs. which always travel
      together (e.g. `overlay`/`overlay_dataset`/`overlay_values` likely live and
      die as a unit).

*Risk:* moderate (touches load/unload and many readers). High clarity payoff.

### 3. Pick one source of truth for the overlay scalars that are currently mirrored
The same overlay attributes live in **three** structs and are hand-synced:

- `overlay::Overlay` (domain: `intensity_range`, `threshold`, `opacity`, `symmetric_range`, …)
- `viewer/mesh.rs::OverlayAppearance` (`range`, `colormap`, `threshold`, `opacity`, `dim`)
- `command.rs::OverlayCommandState` (`visible`, `symmetric_range`, `intensity_range`, `threshold`, `opacity`)

The cost shows up as manual mirroring, e.g.
[`viewer/mod.rs:3786`](src/viewer/mod.rs) copies `intensity_range`, `threshold`,
and `opacity` from `overlay_appearance` into `controller.overlay` by hand.

- [ ] Decide ownership: `OverlayAppearance` (render-facing) is the natural home
      for `range`/`colormap`/`threshold`/`opacity`/`dim`; `OverlayCommandState`
      should keep only what the controller uniquely needs (e.g. `visible`) and
      borrow the rest, instead of storing a second copy.
- [ ] Remove the hand-sync lines once the duplication is gone.

*Risk:* moderate — requires care to keep the command/controller boundary intact,
but eliminates a class of "the two copies drifted" bugs.

---

## Tier 2 — Merge duplicate value types ✅ DONE (branch `refactor2`)

Outcome: range encodings collapsed from five to two (one `f32` render type,
one `f64` domain type), and the two near-identical threshold types collapsed to
one. Item 6 was examined and intentionally left as-is — see its note. All ~210
tests pass, clippy unchanged, fmt clean.

### 4. One min/max range type instead of five encodings — ✅ done
The same `{ min, max }` pair appears as:

- `surface.rs::ValueRange` (`f32`)
- `overlay.rs::OverlayRange` (`f64`)
- `dataset.rs::ColumnRange` (`f64`)
- `OverlayCommandState.intensity_range: Option<[f32; 2]>`
- ad-hoc `[f32; 2]` / tuples in the viewer

- [x] Kept `ValueRange` (`f32`) as the render/UI range; added `PartialEq`.
- [x] Merged `OverlayRange` into `dataset::ColumnRange` (`f64`) — the audit showed
      the domain genuinely wants `f64` (column data is `f64`) and there was already
      a `From<ColumnRange> for OverlayRange` glue impl, now deleted. Its interval
      methods (`contains`/`normalized`/`validate`) moved onto `ColumnRange`.
- [x] Replaced `OverlayCommandState.intensity_range: Option<[f32; 2]>` with
      `Option<ValueRange>`, collapsing a hand-built array in the sync code into a
      direct copy of `overlay_appearance.range`.

Result: two range types (`ValueRange` f32 / `ColumnRange` f64) along the existing
domain/render boundary, instead of three named types plus `[f32; 2]` and tuples.

### 5. One threshold type instead of three — ✅ done (render/command merged)
- `overlay.rs::Threshold { mode: ThresholdMode, range: Option<OverlayRange> }`
- `viewer/mesh.rs::OverlayThreshold { enabled, absolute, value, hide_failed }`
- `command.rs::OverlayThresholdCommandState { value, absolute, hide_failed }`

The last two are identical except `OverlayThreshold` has an `enabled` flag where
the command form uses `Option<…>`.

- [x] Merged `OverlayThresholdCommandState` into a single `command::OverlayThreshold`
      `{ enabled, absolute, value, hide_failed }`, used as a plain field by both the
      render appearance and the controller command state. The `enabled` flag is the
      one on/off convention; `AfniOverlayState` still wraps it in `Option`, but there
      the `Option` means "field present in this partial wire update," which is a
      *different* concept — so that is correct, not a second convention. The
      hand-rolled field-by-field projection in `sync_controller_overlay_display_state`
      collapsed into a single struct copy.
- [x] Left the richer domain `Threshold`/`ThresholdMode` (Above/Below/Between/Outside)
      alone — merging it into the scalar render form would lose the mode enum, i.e.
      cut functionality. Documented here as an intentional split, not a duplicate.

### 6. One overlay-column-selection type — ⏭️ examined, intentionally not merged
- `overlay.rs::OverlayColumns` + `ColumnSelection { index, label }` (carries `String` labels, not `Copy`)
- `viewer/mod.rs::OverlayColumnSelections { intensity, threshold, brightness }` (index-only, `Copy`)

On inspection these are a legitimate layer boundary, not harmful duplication:
`OverlayColumnSelections` is a `Copy` index bundle bound directly to egui
`&mut usize` dropdowns and passed by value to ~10 functions, converted to the
labeled domain `OverlayColumns` at a single boundary
(`canonical_overlay_columns`). Merging would lose `Copy`, force `.index` at ~15
sites, and complicate the UI binding — i.e. it would *de*-streamline. Left as-is.

---

## Tier 3 — Structural / file-level

### 7. Split `viewer/mod.rs` (~11.8k lines) along its existing seams
`impl ViewerState` alone is ~6,000 lines. Method-name clustering already reveals
natural modules: `roi_*` (27), `draw_*` (18), `afni_*` (18), `pair*`/`paired_*`
(28), `graph_*` (12), `overlay_*`/`load_*` (20), `pick_*` (10), `capture_*`/`save_*`.

- [ ] Move cohesive method groups into `impl ViewerState` blocks in topical
      submodules (`viewer/afni.rs`, `viewer/roi.rs`, `viewer/overlay_load.rs`,
      `viewer/pairing.rs`, `viewer/graph.rs`). `screenshot.rs`, `mesh.rs`,
      `camera.rs`, `pick.rs`, `gpu.rs` already show the pattern works.
- [ ] Relocate the ~45 small local structs (`LoadedOverlay`, `SceneStats`,
      `GraphSnapshot`, `MontageShot`, …) to the module that owns them.

*Risk:* low per move (pure relocation), but high churn — do it in small,
verifiable commits after Tier 1/2 shrink the surface.

### 8. Tidy the ROI render-side type cluster
`RoiLayer`, `RoiWorkspace`, `RoiSlot`, `RoiDraft`, `RoiDraftTarget`,
`RoiDraftSnapshot`, `RoiPickTarget`, `RoiAppearanceBuild`, `RoiComponentRange`
([`viewer/mod.rs:7499`](src/viewer/mod.rs)+) sit alongside the domain `roi::Roi`.

- [ ] Audit `RoiDraft` / `RoiDraftTarget` / `RoiDraftSnapshot` for merging — a
      snapshot and its target are often the same data captured at two moments.
- [ ] Confirm each remaining type earns its keep; collapse any that are a struct
      wrapping a single field used in one place.

*Risk:* low, but needs the draw/undo paths re-read first.

---

## Explicitly *not* recommended (yet)

- **`Rgba` vs `[f32; 4]`.** `[f32; 4]` is the GPU/vertex-buffer currency; forcing
  `Rgba` everywhere would add conversions in the hot path for little gain. Better
  to keep `Rgba` at the color/colormap boundary and `[f32; 4]` at the GPU
  boundary, with conversions centralized (they mostly already are).
- **The `*Id(String)` newtypes** (`SurfaceId`, `SurfaceDomainId`, `RoiId`). These
  are doing real type-safety work; leave them.

---

## Suggested sequencing

1. Tier 2 first (items 4–6): small, self-contained type merges that *reduce the
   blast radius* of everything after them. Land each behind `From` shims.
2. Tier 1 next (items 1–3): the field-group collapses, now expressed in the
   unified types.
3. Tier 3 last (items 7–8): pure relocation once the type surface is smaller.

Run `cargo test && cargo clippy --lib && cargo fmt --all -- --check` after each
step; the suite (~210 tests) is the safety net for "no functionality cut."
