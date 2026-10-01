# DriveSuma Compatibility

Sumaru can listen for the NIML `EngineCommand` messages emitted by AFNI's
`DriveSuma` program. This makes it possible to reuse a useful subset of SUMA
automation, including the commands used by the FATCAT demo scripts, without
making Sumaru imitate SUMA's application architecture.

The compatibility boundary is intentional:

- A Sumaru process owns one view. SUMA viewer letters such as `A` through `F`
  are not mapped to several views inside one process.
- Start separate Sumaru processes on separate NIML port blocs when a script
  needs independently controlled views.
- Sumaru keeps its own screenshot, montage, ROI, overlay-workbench, and local
  keyboard behavior. Compatibility translation happens only when requested.
- Only the active overlay is rendered. Loaded overlays remain available in the
  workbench and can be selected by DriveSuma, but they are not composited.

## Choosing a command mode

The three listener spellings are:

| Option | Command interpretation |
| --- | --- |
| `-niml` / `--niml` | Listen for `EngineCommand` messages, interpreting keys with Sumaru's native keyboard meanings. This is useful for exact Sumaru remote control. |
| `-niml-suma` / `--niml-suma` | Translate the supported DriveSuma/SUMA command subset described below. |
| `-niml-compat` / `--niml-compat` | Alias for `-niml-suma`. |

`-niml` and `-niml-suma` are mutually exclusive. They are also separate from
`--talk-afni`: the DriveSuma listener accepts short-lived inbound command
connections, while AFNI NIML talk is Sumaru's long-lived bidirectional session
with AFNI. A viewer may use both when a workflow needs both channels.

For an existing SUMA script, start with `-niml-suma`. Use `-niml` only when the
sender is intended to drive Sumaru according to Sumaru's own key bindings.

## Starting Sumaru and DriveSuma

Start the viewer first. The same `-npb` or `-np` selection must be supplied to
both programs:

```sh
sumaru -spec Demo.spec -sv Demo_SurfVol+orig. -niml-suma -npb 1000
```

Then send commands from another shell:

```sh
DriveSuma -npb 1000 \
  -com surf_cont -surf_label Net_000.gii \
  -load_dset Net_000.cols.niml.dset \
  -switch_cmap ROI_i32 -Dim 0.3
```

Port selection follows AFNI's named-port convention for
`SUMA_DRIVESUMA_NIML`. If neither `-np` nor `-npb` is supplied, both programs
must resolve to the same default environment and port settings.

For two independent views, run two Sumaru processes instead of addressing SUMA
viewer `A` and viewer `B` inside one application:

```sh
sumaru -spec left.spec  -niml-suma -npb 1000
sumaru -spec right.spec -niml-suma -npb 1001

DriveSuma -npb 1000 -com viewer_cont -key 'ctrl+left'
DriveSuma -npb 1001 -com viewer_cont -key 'ctrl+right'
```

DriveSuma resolves relative dataset paths using the caller working directory
included on the wire, so commands launched from demo directories can continue
to use relative paths.

## Supported `surf_cont` commands

The following translations are active only in `-niml-suma` /
`-niml-compat` mode.

When one `surf_cont` command contains several options, Sumaru applies them in a
stable dependency order: select the surface, load/select the dataset, choose
the color map and columns, then apply ranges, thresholds, appearance, and
visibility. Scripts therefore do not need to split a normal dataset setup into
several connections.

| DriveSuma option | Sumaru behavior | Notes |
| --- | --- | --- |
| `-surf_label LABEL` | Select a matching surface or paired state already loaded in this process. | Matches display names, component names, and filenames. It does not load a new surface file. |
| `-view_surf_cont y/n` | Show or hide the Sumaru surface controller. | Both `View_Surf_Cont` and `view_surf_cont` wire spellings are accepted. |
| `-load_dset PATH` | Load the overlay and make it active. | Reopening a known path refreshes its workbench entry. |
| `-switch_dset LABEL` | Select a loaded overlay. | Labels, full paths, filenames, and common SUMA dataset stems are matched. |
| `-switch_cmap amber_monochrome` | Select Sumaru's AFNI-compatible amber map. | Other continuous map names are not translated yet. |
| `-switch_cmap ROI_i32` | Select the discrete integer-label palette. | Zero normally remains the unlabeled background for ROI label tables. |
| `-I_sb INDEX` | Select the numeric intensity sub-brick. | Indices are zero-based. Missing or nonnumeric columns are reported and ignored. |
| `-I_range MAX` | Set the symmetric range `-abs(MAX)..abs(MAX)`. | Also enables symmetric range behavior. |
| `-I_range MIN MAX` | Set an explicit intensity range. | Reversed endpoints are normalized. A symmetric pair is recognized as symmetric. |
| `-T_sb INDEX` | Select and enable the threshold sub-brick. | `-1` disables the threshold column. |
| `-T_val VALUE` | Set a numeric threshold. | Enables thresholding. |
| `-T_val Pp` | Convert p-value `P` using the selected sub-brick's AFNI statistical metadata. | For example, `0.01p`. The command is ignored with a status message when compatible metadata are unavailable. |
| `-T_val P%` or `P%%` | Use percentile `P` of the selected threshold column. | Zero and nonfinite values are excluded, following SUMA's percentile behavior. |
| `-B_sb INDEX` | Select the brightness-modulation sub-brick. | `-1` disables brightness modulation. The first compatibility command that enables it adopts SUMA's `0.3..0.8` brightness-scale default; native Sumaru keeps its `0..1` default. |
| `-B_range MAX` | Set the symmetric brightness input range `-abs(MAX)..abs(MAX)`. | The same control is available in the overlay UI. |
| `-B_range MIN MAX` | Set an explicit brightness input range. | Reversed endpoints are normalized. |
| `-B_scale LOW HIGH` | Set the RGB multiplier at the low and high ends of `B_range`. | The order is preserved, so a decreasing scale is allowed. |
| `-Dim VALUE` | Multiply the displayed overlay color by the dim factor. | Clamped to the viewer's supported `0..1.5` range. |
| `-Opa VALUE` | Set overlay opacity. | Clamped to `0..1`. |
| `-view_dset y/n` | Set overlay visibility absolutely. | Unlike a toggle, repeated `n` commands remain off. |
| `-Dsp XXX` | Hide the active dataset display. | The overlay remains loaded. |
| `-Dsp Col` | Draw filled overlay colors. | This is Sumaru's normal display mode. |
| `-Dsp Con` | Draw dataset contours without filled colors. | This is separate from Sumaru's threshold-boundary `B` control. |
| `-Dsp 'C&C'` | Draw filled colors and dataset contours. | Quote `C&C` in the shell. The overlay UI exposes the same four display modes. |
| `-Clst RAD MIN` | Set cluster connectivity and minimum size. | Detailed sign rules are below. |
| `-UseClst y/n` | Enable or disable cluster filtering. | A threshold must also be active for clustering to retain nodes. |
| `-shw_0 y/n` | Show or mask exact zero intensity values. | Independent of thresholding. Label-table zero may still be the table's transparent unlabeled value. |
| `-1_only y` | Keep only the active overlay visible. | This is already Sumaru's overlay-workbench model. |
| `-1_only n` | Accepted, with a status message. | Multiple simultaneous foreground overlay compositing is not implemented; Sumaru remains active-overlay-only. |

### Dataset contours

SUMA's continuous color maps are pane-based, so `Dsp Con` can outline the
palette panes directly. Sumaru's continuous color maps are smoothly
interpolated and therefore have no intrinsic pane boundaries. The
compatibility translation divides the active intensity range into ten bands
and draws the nine internal isolines. Discrete label maps instead contour the
boundaries between their actual label values. Contour width, halo, and color
remain under Sumaru's normal contour controls.

This is behaviorally useful and stable, but a continuous `Con` rendering is not
expected to be pixel-identical to a particular SUMA palette with a different
number of panes.

### Cluster sign rules

`Clst` follows SUMA/`SurfClust` conventions:

- `RAD < 0`: use `abs(trunc(RAD))` mesh-edge rings. `-1` is ordinary direct
  adjacency; wider rings can bridge a small subthreshold gap.
- `RAD >= 0`: use accumulated mesh-edge length as a surface-distance
  approximation, in millimeters.
- `MIN >= 0`: require at least this much surface area in mm².
- `MIN < 0`: require at least `ceil(abs(MIN))` nodes.

For example, this uses one-edge adjacency and requires 20 nodes:

```sh
DriveSuma -npb 1000 \
  -com surf_cont -Clst -1 -20 -UseClst y
```

This uses a 2.5 mm surface radius and requires 40 mm²:

```sh
DriveSuma -npb 1000 \
  -com surf_cont -Clst 2.5 40 -UseClst y
```

The Sumaru cluster UI can switch between edge rings and millimeter radius and
can reconstruct a corresponding `SurfClust` command.

## Supported `viewer_cont -key` commands

In compatibility mode, DriveSuma keys are translated as follows:

| DriveSuma key | Sumaru action |
| --- | --- |
| `space` | Reset the camera. |
| `up`, `down`, `left`, `right` | Nudge the camera. |
| `ctrl+left`, `ctrl+right`, `ctrl+up`, `ctrl+down` | Select left, right, top, or bottom preset view. |
| `m` | Toggle camera momentum. |
| `p`, `P` | Cycle surface render style forward or backward. |
| `o`, `O` | Cycle/lower or raise Sumaru surface opacity. |
| `F5` | Toggle the background between black and white. |
| `r` | Save a screenshot using Sumaru's normal image action. |
| `ctrl+r` | Save a screenshot, matching SUMA's captured-frame result. Local Sumaru `Control+R` remains the ROI-controller shortcut outside compatibility translation. |
| `g`, `G` | Open the graph for the current pick. |
| `t` | Toggle AFNI NIML talk. |
| `[`, `]` | Toggle the left or right component. Compatibility mode permits both components to be hidden, matching SUMA. |
| `,`, `comma` | Select the previous loaded surface/state. |
| `.`, `period` | Select the next loaded surface/state. |

`Key_rep_N` repeat counts are honored. `Key_pause_N` and `Key_redis_N` are
recognized as wire metadata but currently do not introduce sleeps or force an
intermediate redraw. Keys carrying `Key_strval_N`, such as SUMA's go-to-node or
go-to-coordinate forms, are not translated yet.

Some deliberate nontranslations are worth calling out:

- `b` is not translated; `F5` is the supported background command.
- `R` is not translated in compatibility mode. Sumaru does not turn it into a
  different recorder action.
- Sumaru's local `r` screenshot, `R`/Shift-`R` montage, and `Control+R` ROI
  behavior remain unchanged when using the application directly or `-niml`.

## Native `-niml` key behavior

Native mode interprets remotely supplied keys exactly like Sumaru's own
bindings where possible. Important differences from compatibility mode include:

- preset views use `alt+arrow`, not `ctrl+arrow`;
- `R` or `shift+r` saves a montage;
- `ctrl+r` opens the ROI controller;
- `[` and `]` use Sumaru's normal paired-hemisphere visibility rule;
- `viewer_cont -bkg_col` selects a coarse black or white background;
- `surf_cont -view_dset n` follows the older native toggle behavior, while
  `view_dset y` is not translated in native mode;
- SUMA dataset-controller attributes such as `I_sb`, `T_val`, and `Clst` are
  not translated.

Use compatibility mode for existing DriveSuma scripts so the last two native
behaviors do not surprise them.

## Process control

`DriveSuma -com kill_suma` requests a controlled close of the compatibility
listener's Sumaru process. It is implemented only in compatibility mode.

## Known limitations

The following SUMA facilities are not currently translated:

- viewer letters, multiple viewers in one process, viewer size/position, and
  controller-window position;
- loading or replacing native surface geometry with `show_surf`, `switch_surf`,
  or `node_xyz` messages;
- `Key_strval_N` node/coordinate navigation;
- saved viewer-state files (`load_view` / `VVS_FileName`);
- arbitrary colormap loading and most named `switch_cmap` values;
- the `-show_0` spelling accepted by SUMA; use `-shw_0` with Sumaru;
- SUMA alpha-mode and boxed-color commands beyond Sumaru's own threshold and
  contour controls;
- native display objects/NIDO commands, masks, object-controller commands, and
  tract loading through `object_cont` (tracts should be supplied to Sumaru with
  `-tract` / `--tract`);
- `recorder_cont`, animation ranges, and autorecord. Use Sumaru's screenshot or
  montage actions instead;
- `get_label`, `set_outplug`, help writers, and SUMA widget-snapshot commands.

Window/viewer-placement options are intentionally outside the current design:
automation should launch and address separate Sumaru processes rather than
construct several lettered viewers inside one process.

## Diagnostics and testing

Add `--verbose` to a Sumaru launch to see accepted connections, unmatched
surface/dataset labels, invalid column selections, unresolved statistical
thresholds, and compatibility limitations in the status output.

NIML recording also applies to the listener:

```sh
sumaru -spec Demo.spec -niml-suma -npb 1000 \
  --niml-record drivesuma_session.nimlrec --verbose
```

Inspect or replay the result with the normal debug commands:

```sh
sumaru niml inspect drivesuma_session.nimlrec
sumaru niml replay drivesuma_session.nimlrec
```

The repository includes wire captures produced by the installed AFNI
`DriveSuma` executable in `tests/fixtures/drivesuma/`. Integration tests replay
those captures, while parser and viewer tests cover command ordering, native
versus compatibility key mappings, ranges, thresholds, brightness, clustering,
display modes, and process shutdown.
