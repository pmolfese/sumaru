# DriveSuma Compatibility

This list is based on AFNI's `SUMA_DriveSuma.c` and `SUMA_driver.c`.
DriveSuma converts most `-com ...` command lines into native SUMA NIML
`EngineCommand` groups. Sumaru now parses a useful subset of those groups and
keeps the remaining gaps listed below.

## EngineCommand Wire Format

DriveSuma sends most controller commands as a NIML group named
`EngineCommand`. The command kind is stored in the `Command` attribute, and the
parsed command-line options become additional attributes on the same group.

For example:

```xml
<EngineCommand
  Command="viewer_cont"
  N_Key="2"
  Key_0="R"
  Key_rep_0="1"
  Key_pause_0="0"
  Key_redis_0="0"
  Key_1="right"
  Key_rep_1="3"
  Key_pause_1="0"
  Key_redis_1="0" />
```

Sumaru receives this through the AFNI/SUMA NIML reader, routes it through
`parse_incoming_message`, and expands supported DriveSuma keys into normal
`ViewerCommand` actions. Unsupported attributes are detected by
`drivesuma_unsupported_attributes`, so fixtures can distinguish "unknown on the
wire" from "known but not implemented yet".

## Supported Today

### `viewer_cont -key`

Sumaru supports these DriveSuma key commands when they do not include a
`Key_strval_*` value qualifier:

- `R`, `space`: reset camera.
- `up`, `down`, `left`, `right`: nudge the camera.
- `ctrl+left`, `ctrl+right`, `ctrl+up`, `ctrl+down`: switch to left, right,
  top, or bottom view presets.
- `F5`, `b`: toggle background.
- `p`: cycle surface render style forward.
- `P`: cycle surface render style backward.
- `o`: lower/cycle surface opacity.
- `O`: raise surface opacity.
- `m`: toggle camera momentum.
- `r`: save screenshot.
- `ctrl+r`: save montage.
- `G`: open graph for the current pick.
- `comma`, `,`: cycle scene surface backward.
- `period`, `.`: cycle scene surface forward.

DriveSuma repeat counts are honored through `Key_rep_N`. For example,
`-key:r3 right` becomes three camera-nudge viewer commands.

The timing/display modifiers `Key_pause_N` and `Key_redis_N` are accepted as
known key metadata, but Sumaru does not currently sleep, pause, or force an
intermediate redraw for each repeated key.

### `viewer_cont -bkg_col`

Sumaru supports `bkg_col` as a coarse black/white background command. The RGB
values are summed; values above `1.5` select white, and lower values select
black.

### `surf_cont -view_dset n`

Sumaru supports `Command="surf_cont"` with `view_dset="n"` by toggling the
overlay off through the existing overlay visibility path.

## Unsupported Commands

## Native Surface Geometry

- `show_surf`: DriveSuma sends native SUMA `SurfaceObject` / new-surface
  messages. Sumaru does not yet parse native `SurfaceObject` groups.
- `node_xyz`: DriveSuma sends node-coordinate updates for an existing surface.
  Sumaru does not yet replace live mesh coordinates from native SUMA node XYZ
  messages.

## Viewer Controller

- `viewer_cont -key` with `Key_strval_*`, such as `-key:v54R j` or
  `-key:v"0.8 0 10.3" ctrl+j`, is recognized as unsupported. Sumaru needs a
  direct "go to node/coordinate" command path before this can be faithful.
- `viewer_cont -viewer`, `-viewer_width`, `-viewer_height`, `-viewer_size`,
  `-viewer_position`, `-controller_position`, and `DoViewerSetup` are not
  implemented. Sumaru does not currently expose DriveSuma-style multi-viewer or
  window placement control.
- `viewer_cont -load_view` / `VVS_FileName` is not implemented.
- `viewer_cont -load_do`, `-fixed_do`, `-mobile_do`, and related native NIDO
  display-object loading are not implemented.
- `viewer_cont -do_draw_mask`, `-autorecord`, `-N_foreg_smooth`,
  `-N_final_smooth`, and `-inout_notify` are not implemented.

## Surface/Object Controller

- Dataset loading and switching: `Dset_FileName`, `switch_dset`, `dset_label`.
- Dataset column controls: `I_sb`, `T_sb`, `B_sb`.
- Dataset value controls: `I_range`, `T_val`, `B_range`, `B_scale`, `Dim`,
  `Opa`, `Dsp`, `Clst`, `UseClst`, `1_only`, `shw_0`.
- Color controls: `switch_cmap`, `switch_cmode`, `load_cmap`, `Col_FileName`.
- Surface controls: `SO_label`, `switch_surf`, `view_surf`, `RenderMode`,
  `TransMode`.
- Alpha/boxed controls: `SET_FUNC_ALPHA`, `SET_FUNC_ALPHA_MODE`,
  `SET_FUNC_BOXED`.
- Controller visibility beyond the basic surface controller toggle is not fully
  implemented.

## Tracts, Masks, And Other Objects

- `object_cont` and tract/mask controls are not implemented:
  `Masks`, `2xMasks`, `Delete_All_Masks`, `Load_Masks`, `Save_Masks`.

## Recorder

- `recorder_cont` is not implemented:
  `Save_As`, `Save_From`, `Save_To`, `Anim_Dup`, `Caller_Working_Dir`.
- Sumaru has screenshot and montage commands, so still-image compatibility is a
  good candidate for a future partial implementation.

## Queries And Process Control

- `get_label` is not implemented. Sumaru would need a response path back to the
  DriveSuma stream.
- `set_outplug` is not implemented.
- `kill_suma` is not implemented. Sumaru should probably map this to a
  controlled close or disconnect rather than immediate process exit.

## Help/Snapshot Writer Commands

The controller help and widget snapshot commands are not implemented:

- `Write_*_Cont_Help`
- `Write_*_Cont_Sphinx_Help`
- `Snap_*_Cont_Widgets`
- `Write_Mouse_Keyb_Help`
- `Write_Mouse_Keyb_Sphinx_Help`
- `Write_Mouse_Cmap_Keyb_Help`
- `Write_Mouse_Cmap_Keyb_Sphinx_Help`

These are SUMA GUI documentation/snapshot facilities and do not currently have
Sumaru equivalents.
