# DriveSuma wire captures

These NIML streams were captured from the installed AFNI `DriveSuma` binary,
not synthesized by the tests. A temporary TCP listener replaced SUMA while the
commands below were run from `/Users/molfesepj/FATCAT_DEMO`.

```text
DriveSuma -npb 1000 -com surf_cont -surf_label Net_000.gii \
  -load_dset Net_000.cols.niml.dset -switch_cmap ROI_i32 -Dim 0.3

DriveSuma -npb 1001 -com surf_cont -surf_label Net_000.gii \
  -switch_cmap amber_monochrome -Dim 1.0 \
  -com viewer_cont -key '.' -key 't'

DriveSuma -npb 1002 -com surf_cont -view_surf_cont y \
  -com viewer_cont -viewer_size 900 900 -viewer_position 100 200 \
  -com viewer_cont -key '[' -key ']' -key 'F5'

DriveSuma -npb 1003 -com kill_suma
```

The captures intentionally retain the emitted port and absolute working
directory. They revealed that the wire attributes are `Dset_FileName` and
`View_Surf_Cont`, which differ from the corresponding command-line option
names.
