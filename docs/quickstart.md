# Quickstart

Build and install Sumaru using the [installation guide](installation.md).
The paths below are placeholders for your own files.

## Open a surface

```sh
sumaru --surface /path/to/lh.white.gii
```

Or run `sumaru` without arguments and use the **Open:** controls.
Left-drag rotates the surface, scroll zooms, and right-click selects a vertex.
Press Space to reset the camera.

## Add an overlay

```sh
sumaru -i /path/to/lh.white.gii --overlay /path/to/activation.niml.dset
```

Use the overlay workbench to choose a dataset and intensity column, adjust the
threshold, and set the color range. Color range and threshold are independent.
See [Overlays](overlays.md) for range locking and percentile presets.

## Explore a timecourse

```sh
sumaru tc -i /path/to/lh.white.gii --overlay /path/to/estimate.time.gii
```

Select a vertex to inspect its trace and use the timecourse dock for playback.
Check the time units and baseline settings before interpreting the display.
See [Timecourse mode](timecourses.md) and [InstaCorr](instacorr.md).

## Work with an ROI

Press **Control-R** to open the ROI controller. See [Viewer controls](viewer.md)
for the rest of the keyboard and mouse controls.

## Get command help

```sh
sumaru --help
sumaru tc --help
sumaru resample --help
sumaru surface-map --help
```
