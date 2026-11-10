# Surface mapping and resampling

Sumaru supports two distinct ways to put data on another surface.

## Known parent vertex IDs

MNE exports with `MNEParentVertexIndices` can load on their full-resolution parent
or a child with matching parent correspondence. Rows are matched by parent ID,
including reordered subsets. For older exports whose metadata live on the
anatomical child, use `--companion-surface child.gii`.

```sh
sumaru -i fullres_white.surf.gii --overlay estimate.time.gii --sparse-fill mne-smooth
```

Sparse-fill choices are `sampled-only`, `mesh-nearest`, and `mne-smooth`.
The default smoothing uses 10 steps. Mesh-nearest uses edge-hop distance.
Smoothing may change values at sampled vertices; disconnected regions without
samples remain missing. Categorical labels stay sampled-only.

Parent identity checks are separate from index validity. The default mismatch
policy warns; `--strict-parent` rejects mismatched or incomplete identities.
Valid metadata are not independent proof of geometric registration.

Export without opening a viewer:

```sh
sumaru resample --input estimate.time.gii --target fullres_white.surf.gii   --method mne-smooth --smoothing-steps 10 --output estimate.parent.time.gii
```

## Independently tessellated surfaces

Build an explicit geometric map when parent IDs do not establish correspondence:

```sh
sumaru surface-map --source child.gii --target target.gii   --method normal-projection --centroid --output mapping.json
sumaru resample --input child.time.gii --source-surface child.gii   --target target.gii --map mapping.json --output target.time.gii --preview
```

Mapping methods include nearest-node, closest-triangle, normal-projection,
registered-sphere, area-average, ancestry, and afni-m2m. Registered spheres must
already be registered. Missing values remain NaN; categorical interpolation is
rejected.

## Inspect correspondence

Use `--qc-child child.gii` with a parent surface for the parent/child QC view.
For a geometric map, use `--qc-source SOURCE --qc-map MAP`.
Check coordinate conventions and the original versus transformed geometry before
interpreting a resampled result.

Follow [Surface QC and resampling](surface_alignment_walkthrough.md) for worked
commands, limitations, and export previews.
