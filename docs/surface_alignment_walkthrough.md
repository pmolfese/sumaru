# Surface QC and resampling: stages 4–7

Start with geometry-only QC when you want to compare high- and low-resolution
surfaces. No time series, resampling, or output file is required. The commands
below use `cargo run --` from the Sumaru repository; an installed binary can use
`sumaru` instead.

## Choose the workflow

| Goal | Command / stage | What it changes |
| --- | --- | --- |
| Compare a full parent and its indexed decimated child | `-i PARENT --qc-child CHILD` (5) | Display only |
| Add a sparse time series to that comparison | Add `--overlay DATA` (5) | Display only |
| Save sparse samples or reconstructed full-parent data | `resample --method ...` (4) | New data file and report |
| Compare independently tessellated meshes or meshes without parent IDs | `surface-map`, then `--qc-source ... --qc-map ...` (6) | New map; QC itself is display only |
| Transfer data using a saved map | `resample --map ... --source-surface ...` (6–7) | New data file and report |
| Recover essentially exact IDs in legacy anatomy | `recover-parent-ids` (6) | New anatomical GIFTI |
| Use registered spheres, AFNI M2M, ancestry, or area averages | Additional `surface-map --method` choices (7) | New explicit map |

A **surface** supplies XYZ coordinates and triangles; a **data overlay** supplies
values or time samples at vertices. QC can compare two surfaces with no data.
It does not automatically compare two separately supplied data arrays or compute
a high-resolution versus low-resolution data-error map. Parent-export QC can
show original sampled traces alongside the exported data when its provenance
can resolve the unchanged original input.

## Set up your working example

These are the actual native sub-101 files used in the successful preview. The
folder also contains fsaverage files; do not select a parent by folder name alone.
Run this setup and the commands in the same terminal session:

```sh
EXAMPLE_DIR="/Users/molfesepj/Library/CloudStorage/OneDrive-NationalInstitutesofHealth/Projects/Sharif/SFIM_Closed_Loop/bids/derivatives/mne-source-sighted-blind-straight-average-gifti-fsaverage-20261110"
PARENT="$EXAMPLE_DIR/sub-101_hemi-lh_desc-fullres_white.surf.gii"
CHILD="$EXAMPLE_DIR/sub-101_desc-allruns_cond-sighted_space-surface_method-LCMV-lh.gii"
DATA="$EXAMPLE_DIR/sub-101_desc-allruns_cond-sighted_space-surface_method-LCMV-lh.time.gii"
```

Names such as `source.gii`, `target.gii`, and `source.sphere.reg.gii` in later
examples are placeholders: substitute real files. Output names must be new;
Sumaru does not overwrite existing data, map, or report files.

## Stage 5: just QC the high- and low-resolution surfaces

```sh
cargo run -- -i "$PARENT" --qc-child "$CHILD"
```

The parent uses the main surface renderer and the child starts as an amber
wireframe. In **Parent / Child QC**:

- Toggle parent/child visibility and choose filled, wire, or nodes for the child.
- Use **Swap parent / child styles** to reverse the visual contrast.
- Turn **Child connectivity at mapped parent positions** off to inspect the
  child's original XYZ coordinates. Both surfaces still share the parent camera.
- Turn it on to put the child's vertices at the positions declared by its parent
  IDs. This checks the correspondence/topology display; it is not independent
  evidence that the original coordinates align and is not registration.
- Disable **Show child wire through parent** for a depth-occluded view; enable it
  to see the entire wireframe through the parent. Dense crossing lines can be a
  consequence of this display setting.
- Pick child vertices to follow their parent IDs. With data loaded, this links
  to the parent's trace controls.
- Save screenshots or montages; a `.qc.json` sidecar records the QC settings.

This route requires explicit parent IDs on the child and the original full
parent numbering. It supports one hemisphere per window. For files without
those IDs, use stage 6 below rather than assume equal counts imply correspondence.

Add the original time series without creating any resampled files:

```sh
cargo run -- -i "$PARENT" --qc-child "$CHILD" --overlay "$DATA" \
  --sparse-fill sampled-only
```

Use `--sparse-fill mesh-nearest` or `--sparse-fill mne-smooth` to change display
filling. The source file is unchanged. For timecourse controls:

```sh
cargo run -- tc -i "$PARENT" --overlay "$DATA" --qc-child "$CHILD"
```

QC disables automatic opposite-hemisphere discovery in `tc`.

## Stage 4: save full-parent data

Choose the output behavior explicitly:

| `resample --method` | Result |
| --- | --- |
| `sampled-only` | Preserve measured samples and their parent IDs; no filling |
| `mesh-nearest` | Fill reachable parent vertices from the nearest sample by mesh edge hops |
| `mne-smooth` | Iterative mesh smoothing/filling; measured values can change too |

```sh
cargo run -- resample --input "$DATA" --target "$PARENT" \
  --method mne-smooth --smoothing-steps 10 \
  --output sub101.lh.smoothed.time.gii --preview --qc-child "$CHILD"
```

`--preview` opens the saved output without applying a second reconstruction.
Omit it for headless export. The report defaults to
`sub101.lh.smoothed.time.gii.json`; use `--report PATH` to choose another path.
Use an output ending in `.niml.dset` for NIML. Repeat `--column N` to select
zero-based columns; time-series selections must be contiguous and increasing.
Omitting `--column` exports all columns. Known timing/units are preserved; missing
locations remain missing rather than becoming zero.

If export succeeds but preview fails because the QC child path was wrong, keep
the export and reopen it directly:

```sh
cargo run -- -i "$PARENT" --overlay sub101.lh.smoothed.time.gii \
  --qc-child "$CHILD"
```

The stage 4 route targets the full parent using known IDs. For an independent
mesh, or a general reverse-direction transfer, build a map instead.

## Stage 6: geometry QC without parent IDs

For two already aligned surfaces, create an explicit closest-triangle map:

```sh
cargo run -- surface-map --source source.gii --target target.gii \
  --method closest-triangle --output source-to-target.map.json
cargo run -- -i target.gii --qc-source source.gii \
  --qc-map source-to-target.map.json
```

No data overlay is needed. The map gathers source values at target vertices;
source can be low resolution and target high resolution, or vice versa.
Swapping direction requires a new map—it is not an inverse of the saved weights.

**Geometric Map QC** offers source wireframe visibility, original versus
transformed source coordinates, and distance-colored target points. Blue→red
means increasing distance; magenta means missing and gray means no distance is
available. Pick a target node to inspect its source IDs, weights, distance and
fallback flag. The transform toggle changes the display only. Maps are checked
against both meshes' geometry/order fingerprints and become invalid if the
target geometry changes.

Available geometry methods:

| Method | Meaning |
| --- | --- |
| `nearest-node` | Euclidean nearest source vertex |
| `closest-triangle` | True closest point on a source triangle, with barycentric weights |
| `normal-projection` | Nearest intersection in either direction along the target normal |

No alignment is applied by default. Add `--centroid` only when a center
translation is intended. Use `--affine matrix.1D` for a supplied source-to-target
matrix in the loaded XYZ basis. Add `--affine-afni` for an AFNI DICOM-basis matrix
acting on RAS geometry. Do not use `--centroid` and `--affine` together.
`--maximum-distance D` rejects distant matches in loaded coordinate units.
`--nearest-fallback` is an explicit option for normal-projection ray misses.
These settings do not establish anatomical registration.

### Replace the old script's workflow

```sh
cargo run -- surface-map --source source.gii --target target.gii \
  --method normal-projection --centroid --output script-replacement.map.json
cargo run -- resample --input source.time.gii --source-surface source.gii \
  --target target.gii --map script-replacement.map.json \
  --output target.time.gii --preview
```

This uses the old script's centroid-plus-normal-projection approach. Numerical
results need not be bit-identical to AFNI. For a comparison using the script's
exact matrix, replace `--centroid` with
`--affine script-centroid.1D --affine-afni` on RAS inputs. See the
[AFNI baseline](../align_surfaces.md#stage-6-afni-baseline-and-validation-2026-11-10).

`resample --map` automatically opens geometric QC when `--preview` is present.
Do not also specify the parent-fill `--method`. It transfers all scalar/time-series
columns; `--column` is currently unavailable on this route. GIFTI, NIML and STC
inputs are supported, with GIFTI or NIML output. Labels/masks/text are rejected;
a missing positive-weight contributor produces NaN.

Inspect an existing map without a GUI:

```sh
cargo run -- map-info --source source.gii --target target.gii \
  --map source-to-target.map.json
```

`surface-map` writes the map and `MAP.report.json`. The map contains per-target
weights and diagnostics; the report and `map-info` summarize coverage/distances.

### Recover exact IDs for older anatomical exports

```sh
cargo run -- recover-parent-ids --child legacy-child.gii --parent parent.gii \
  --tolerance 0.00001 --output indexed-child.gii
cargo run -- -i parent.gii --qc-child indexed-child.gii
```

Recovery requires a unique, essentially exact parent match for every child
vertex, with no reused IDs. It preserves original loaded child coordinates and
triangles and writes new mapping metadata. Optional centroid/affine flags apply
only to the matching calculation. It does not overwrite existing IDs or infer
IDs from a smoothed/independent mesh. Increasing tolerance weakens the match.

## Stage 7: other mapping routes

Each command below creates a map. Inspect it with `map-info`, view it with
`--qc-source`/`--qc-map`, or apply it with the stage 6 `resample --map` command.

### Already registered spheres

```sh
cargo run -- surface-map --source source.white.gii --target target.white.gii \
  --method registered-sphere --source-sphere source.sphere.reg.gii \
  --target-sphere target.sphere.reg.gii --output sphere.map.json
```

Spheres must already share a registered, origin-centered frame and must match
the ordered topology of their respective anatomical meshes. Sumaru computes
radial barycentric correspondence; it does not register the spheres. Reported
distances are unit-sphere radial gaps, not anatomical millimeters.

### Import an existing AFNI M2M

```sh
cargo run -- surface-map --source source.gii --target target.gii \
  --method afni-m2m --m2m existing.niml.M2M --output imported.map.json
```

Both ASCII and binary M2M are supported. **AFNI M2 is source; M1 is target.**
The user associates the file with the correct meshes; matching counts/AFNI IDs
alone cannot prove correspondence. For QC, supply the already aligned geometry:
M2M does not store the original affine. Imported maps then use Sumaru fingerprints
for subsequent reuse. Do not combine imports with new geometric search settings.

### Weighted ancestry supplied by a decimator

```sh
cargo run -- surface-map --source parent.gii --target child.gii \
  --method ancestry --weights ancestry.json --output parent-to-child.map.json
```

`ancestry.json` must already use the validated `sumaru-weighted-map-1` schema,
including matching mesh fingerprints. This is not an importer for arbitrary
decimator formats. Row j gathers weighted parent/source values at child/target
vertex j; it does not scatter child data back to the parent.

### Area-weighted downsampling

```sh
cargo run -- surface-map --source highres.gii --target lowres.gii \
  --method area-average --output highres-to-lowres.map.json
cargo run -- resample --input highres.time.gii --source-surface highres.gii \
  --target lowres.gii --map highres-to-lowres.map.json \
  --output lowres.time.gii --preview
```

Each source vertex contributes its lumped triangle area to its nearest target
vertex's bucket. Weights normalize within each bucket; empty buckets remain
missing. This is area-weighted averaging, not conservative triangle-overlap
integration. Use it deliberately rather than treating it as interchangeable
with point sampling or exact parent-ID selection.

## Validation status

The user's 2026-11-10 screenshots confirm that parent/child surfaces render
together and that the two style combinations provide useful contrast on the
native dataset. They do not verify every QC control, screenshot sidecar,
geometric-map display, or every stage 7 route. Numerical/IO validation and AFNI
comparison details are recorded in [the implementation plan](../align_surfaces.md).
