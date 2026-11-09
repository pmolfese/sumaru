# Surface alignment and data resampling

## Goal

Support transfer of surface data between resolutions of the same subject or
standard surface, through a headless command and an interactive quality-control
view. Start with the existing MNE anatomical GIFTI plus time-series GIFTI
workflow. Updated MNE exports must not be a prerequisite.

The first useful result is a reproducible replacement for
`suma_align_mne.py`: optionally translate the source to match target centroids,
build a source-to-target correspondence, transfer every time point, save the
result, and inspect the alignment with one surface drawn as a wireframe.

## Scope and terminology

- **Source:** mesh on which the input data are defined.
- **Target:** mesh on which the output data will be defined.
- **Alignment:** an explicit transform taking source coordinates into target
  coordinates. It does not alter data values or vertex identities.
- **Mapping:** target vertices' contributing source nodes and interpolation
  weights, plus validity and geometric diagnostics.
- **Resampling:** applying that mapping to data columns or time points.

Support low-to-high and high-to-low mapping. Neither operation is generally
invertible. Upsampling adds samples, not spatial information; downsampling by
interpolation is not area averaging or anti-aliasing.

Initial scope is scalar continuous data on triangular GIFTI or native
FreeSurfer geometry. Start data export with time-series/scalar GIFTI. Treat
labels explicitly with nearest-node transfer; never interpolate label IDs.
Defer vector/tensor transformations, automatic rigid registration, nonlinear
registration, cross-subject morphing, and conservative area-based resampling.

## Existing foundations and required corrections

- `src/surface.rs` provides `SurfaceMesh`, `SurfaceTransform`,
  `SurfaceToSurfaceMap`, `NodeWeights`, and nearest/barycentric mapping methods.
- `src/dataset.rs` provides dense and sparse domain-attached data and timing.
- `src/io/stc.rs` preserves original STC vertex IDs and time metadata.
- `src/viewer/timecourse.rs` contains sparse-display interpolation that may
  inform a later exact-parent workflow, but is not interchangeable with mapping
  between independently tessellated surfaces.
- The viewer has filled, triangle-edge, and point rendering modes. Rendering
  selection currently comes from shared controller state; comparison needs
  independent styles for source and target.

The current barycentric helper chooses a triangle by centroid distance and
clamps its weights. Replace this approximation for the new workflow with a
well-defined triangle search and interpolation algorithm. Avoid exhaustive
source-triangle scans for every target vertex at production resolutions.

## Architecture

Put the shared, headless alignment and mapping engine in
`src/surface_align.rs`, exported from `src/lib.rs`. Keep file loading/writing
in `src/io/`, command parsing in `src/main.rs`, and viewer integration in a
focused `src/viewer/surface_align.rs` module.

Proposed responsibilities and types (names may evolve during implementation):

- `AlignmentOptions`: no alignment, centroid translation, or supplied affine;
  explicit coordinate units and optional distance limits.
- `MappingOptions`: interpolation/search method and unmapped-node policy.
- `SurfaceAlignment`: source-to-target transform and before/after diagnostics.
- `SurfaceMap`: source/target identities, per-target weights and validity,
  search diagnostics, and provenance.
- `MappingReport`: mapped/unmapped counts, distance distribution, degeneracy
  counts, and relevant input warnings.
- Functions to align geometry, build a reusable map, apply it to a dataset,
  and validate a loaded map against its inputs.

Reuse or migrate existing map types rather than maintain two competing
implementations. Preserve existing callers with wrappers where appropriate.
Separate geometric search from dataset application so one map serves all time
points and conditions. Run GUI calculations in background workers and reject
stale results when inputs or options change.

## Alignment policy

1. Default to existing coordinates (`--align none`).
2. Offer `--align centroid` to reproduce the script's translation step.
   Report the transform explicitly. Uneven vertex sampling can shift a
   centroid, so this is a requested operation, not a presumed correction.
3. Accept a documented source-to-target affine convention. The first matrix
   import should support the script's AFNI-style 3x4 affine, with verified
   axis, multiplication, and unit semantics.
4. Inspect available coordinate metadata and units. Do not infer a conversion
   merely from similar shapes or silently fix scale/reflection discrepancies.
   Require explicit resolution of known incompatible coordinate frames.
5. Use one shared world-to-display transform for both surfaces. Viewer
   centering, camera fitting, and hemisphere layout must not alter the mapping
   geometry or independently normalize the two meshes.

Keep original geometry intact. Store the alignment transform with the map;
only write transformed geometry when explicitly requested.

## Mapping methods

### Initial methods

- **Nearest node:** accelerated nearest-neighbor lookup; useful for labels,
  baseline comparisons, and an explicit approximation for continuous data.
- **Closest-triangle barycentric:** locate the true closest point on the
  source mesh using a triangle spatial index, then use that point's barycentric
  weights. Handle edges, vertices, and degenerate triangles explicitly.
- **Normal-projection barycentric:** project each target vertex in both
  directions along its target normal, select the closest valid intersection
  with the source, and compute barycentric weights. This is the method to
  compare with the current `SurfToSurf` workflow.

Give these methods distinct names. Do not claim AFNI equivalence from the word
"barycentric" alone. Verify AFNI defaults, intersection selection, tolerances,
and failure behavior before choosing a compatibility preset.

For all methods, allow an explicit maximum mapping distance, record invalid
targets, and default to failing export when mappings are incomplete. An
explicit option may permit masked output with a documented fill value and a
validity mask. Never silently fill an unmapped target with a plausible zero.
Define treatment of missing/nonfinite source values separately from geometric
mapping failures; do not silently renormalize around missing samples.

Distance and normal diagnostics help identify wrong-bank matches but do not
prove anatomical correspondence. Avoid claiming that a small distance implies
a correct mapping in folded cortex.

### Explicit correspondence and later extensions

Use original parent vertex IDs when available and when parent identity is
verified. They give exact correspondence for retained samples on the matching
parent mesh; filling other parent vertices remains a separate operation.
Equal vertex counts or matching topology alone must not establish subject or
parent identity. Sparse datasets must be resolved by node IDs, not row numbers.

Later, allow correspondence to be computed on paired registered spherical
surfaces and applied to anatomical display surfaces sharing those vertex
orders. Extend to other MRI surface types through the same geometry interface,
with explicit assumptions about anatomical correspondence.

## Command-line design

Proposed syntax; these commands do not exist yet:

```sh
sumaru resample \
  --source-surface subject-lh.gii \
  --target-surface std.60.lh.smoothwm.gii \
  --data subject-lh.time.gii \
  --align centroid \
  --method normal-barycentric \
  --output subject-std60-lh.time.gii \
  --save-map subject-lh.smap \
  --report subject-lh.mapping.json \
  --preview

sumaru resample \
  --map subject-lh.smap \
  --source-surface subject-lh.gii \
  --target-surface std.60.lh.smoothwm.gii \
  --data another-condition-lh.time.gii \
  --output another-condition-std60-lh.time.gii

sumaru align \
  --source-surface subject-lh.gii \
  --target-surface std.60.lh.smoothwm.gii \
  --align centroid \
  --preview
```

`resample` must work without a graphics device or display unless preview is
requested. `align` offers geometry-only inspection without requiring data.
Source and target are always explicit; reversing direction builds a new map.
Start with one hemisphere per invocation so batch use is straightforward.

Validate all inputs before writing. Require an explicit overwrite option,
write outputs atomically where practical, and return nonzero status for
invalid inputs or failed mappings. Keep the script's fixed output prefix out
of the new interface.

## Data output and reusable map format

- Preserve time start, step, sample count, units, column labels, and applicable
  data metadata. Preserve unknown timing as unknown; offer explicit timing
  arguments for older exports rather than invent sample times.
- Attach output to the target domain and record source, transform, method,
  options, software version, and validity information.
- Do not blindly copy source-domain IDs or statistical calibration/FDR metadata
  onto interpolated output. Define which metadata remain valid.
- Use a versioned map format containing geometry/topology fingerprints,
  transform, options, weights, validity, and diagnostics. File paths are
  provenance, not sufficient identity checks.
- Include coordinates in geometric cache identity: inflated and anatomical
  meshes can share topology while requiring different geometric maps.
- Validate dimensions, finite weights, index ranges, and fingerprints on load.
- Choose the serialization after estimating realistic map sizes; avoid a large
  human-readable weight table as the only production representation.

## Visual alignment and QC

Default comparison: **target filled with the mapped data; source drawn as a
contrasting wireframe**. Provide a single control to swap those roles. For
geometry-only inspection, use a neutral filled target and colored source edges.
This provides the overlay contrast familiar from anatomical QC workflows.

Required controls and diagnostics:

- Independent source/target visibility, color, opacity, and filled/wireframe
  style; clear legend naming the meshes and source-to-target direction.
- Before/after alignment toggle using the same camera and display transform.
- Depth-tested edges with a small rendering-only depth bias to reduce
  z-fighting. An optional x-ray mode must be clearly identified.
- Mapping-distance coloring, unmapped-node highlighting, and mapped fraction
  plus median, high-percentile, and maximum mapping distances with units.
- Click a target vertex to show contributing source nodes/triangle, weights,
  distance, and validity. Draw correspondence lines for the selected point or
  a small subset rather than the entire mesh.
- For time-series input, a shared time cursor and color scale plus source
  contributor/target traces. Explain when a target trace is a weighted mixture.
- Save a reproducible QC screenshot or montage and machine-readable report.

Retain the application's one-viewer-per-process design. Add comparison within
that viewer; linked separate windows are not needed for the first release.

## Implementation milestones

### 1. Establish the numerical contract and fixtures — start here

- [ ] Inspect repository instructions and current surface/data I/O behavior.
- [ ] Add small synthetic source/target fixtures with known correspondences.
- [ ] Select a representative existing MNE/SUMA surface and time-series pair
  for local acceptance testing; do not commit private subject data.
- [ ] Capture the current script's transform, AFNI mapping options, mapped-node
  behavior, and output values as a comparison baseline.
- [ ] Specify source-to-target transform conventions, method names, missing
  data policy, and coordinate assumptions before implementing the command.

### 2. Build the shared engine

- [ ] Add `surface_align.rs` and migrate/reuse mapping primitives.
- [ ] Implement no alignment, centroid alignment, and supplied transforms.
- [ ] Implement accelerated nearest-node, true closest-triangle, and normal
  projection mapping with validity and diagnostics.
- [ ] Apply maps across dataset columns while preserving available timing.
- [ ] Validate geometry and dataset domains before mapping and application.

### 3. Deliver a usable headless workflow

- [ ] Add `resample` and geometry-only `align` commands.
- [ ] Add GIFTI result export, reusable map persistence, and JSON reports.
- [ ] Check both directions against synthetic fixtures and the real workflow.
- [ ] Document method differences from AFNI and incomplete-map behavior.

### 4. Add wireframe comparison and inspection

- [ ] Add independent source/target render styles in a shared scene.
- [ ] Add alignment toggle, distance/validity overlays, and correspondence picks.
- [ ] Integrate time-series comparison and background computation.
- [ ] Add QC screenshots and verify that preview matches headless output.

### 5. Extend interoperability

- [ ] Recognize future MNE parent-vertex metadata once its encoding is settled.
- [ ] Extend exact-parent/STC workflows with explicit sparse-fill semantics.
- [ ] Add further output formats, spherical correspondence, and area-aware
  downsampling as separately validated features.

## Validation and completion criteria

Meaningful numerical tests should cover identity mapping, known translations
and affines, analytic linear fields on planar triangles, constant preservation,
edge/vertex hits, normal-projection misses, categorical labels, degenerate and
empty meshes, malformed indices, nonfinite data, sparse row order, and rejected
map reuse with changed coordinates. Test downsampling independently; do not
expect a low-to-high-to-low round trip to be lossless in general.

Compare the normal-projection method with AFNI using identical transforms and
options, including unmapped vertices. Document tolerances and explain remaining
differences. Measure mapping construction and application on realistic mesh
sizes; reuse the map across frames rather than repeating geometric searches.

The first release is complete when the existing GIFTI workflow runs headlessly
without Python/AFNI runtime dependencies, saves reproducible results and map
provenance, preserves available timing, reports failures honestly, and opens a
wireframe-versus-filled preview that exposes the same map used for export.

## MNE coordination

The MNE exporter enhancement is separate work. Sumaru must continue to accept
existing exports containing only geometry and time-series arrays. As inspected
on 2026-11-09, the updated local MNE exporter lacks both parent vertex IDs and
the earlier timing metadata patch. New metadata will improve exact-parent
mapping and timing recovery, but must not block this feature.
