#!/bin/tcsh -f

# Sumaru adaptation of FATCAT_DEMO/Do_09_VISdti_SUMA_visual_ex2.tcsh.
#
# Usage:
#   Do_09_VISdti_sumaru_visual_ex2.tcsh [--check] [FATCAT_DEMO]
#
# SUMARU_BIN may name a non-default Sumaru executable. The script preserves
# the original four-example flow, but uses Sumaru's explicit DriveSuma
# compatibility listener instead of SUMA's native -niml behavior.

set stat = 0
set check_only = 0
set demo_root = "$cwd"
set sumaru_bin = sumaru

if ($?SUMARU_BIN) then
    set sumaru_bin = "$SUMARU_BIN"
endif

foreach arg ($argv)
    if ("$arg" == "--check") then
        set check_only = 1
    else
        set demo_root = "$arg"
    endif
end

if (! -d "$demo_root") then
    echo "FATCAT demo directory does not exist: $demo_root"
    exit 1
endif

cd "$demo_root"

set required_files = ( \
    DTI/o.NETS_OR_000.niml.tract \
    DTI/o.WB_000.niml.tract \
    DTI/o.NETS_AND_000.niml.tract \
    DTI/o.NETS_AND_MINIP_000.niml.tract \
    DTI/o.NETS_AND_MINIP_000.niml.dset \
    SUMA/std.60.FATCAT_both.spec \
    SUMA/std.60.lh.smoothwm.gii \
    SUMA/std.60.rh.smoothwm.gii \
    Net_000.gii \
    Net_000.cols.niml.dset \
    mprage+orig.HEAD \
)

foreach required ($required_files)
    if (! -e "$required") then
        echo "Missing FATCAT demo input: $demo_root/$required"
        set stat = 1
    endif
end

set required_commands = ( "$sumaru_bin" DriveSuma afni plugout_drive prompt_user ccalc @Quiet_Talkers )
foreach required ($required_commands)
    which "$required" >& /dev/null
    if ($status != 0) then
        echo "Required command is unavailable: $required"
        set stat = 1
    endif
end

if ($stat != 0) then
    exit $stat
endif

if ($check_only) then
    echo "FATCAT/DriveSuma prerequisites are available in $demo_root"
    echo "Sumaru executable: $sumaru_bin"
    exit 0
endif

setenv AFNI_ENVIRON_WARNINGS NO
setenv SUMA_DriveSumaMaxCloseWait 15
setenv SUMA_DriveSumaMaxWait 15

set sport = 12
set sport2 = `ccalc -i $sport+1`

@Quiet_Talkers -npb_val $sport -npb_val $sport2

SET0:
# Whole-brain tractography with surfaces and an anatomical volume.
"$sumaru_bin" -npb $sport -niml-suma \
    -spec SUMA/std.60.FATCAT_both.spec \
    -sv mprage+orig \
    -vol mprage+orig. \
    -tract DTI/o.WB_000.niml.tract &

sleep 5
set l = `prompt_user -pause "\
Example 0/4: Whole-brain deterministic tractography with surfaces\n\
\n\
Sumaru should show the tractogram, both cortical surfaces, and anatomical\n\
slice planes in one scene. Use the TRACTS / GRAPHS and volume controls to\n\
inspect visibility, tract coloring, bundle visibility, and slice positions.\n\
Tract masks and surface prying from the original SUMA walkthrough are not yet\n\
implemented.\n\
\n\
Hit OK when ready for Example 1."`
if ("$l" != "1") goto BEND
DriveSuma -npb $sport -com kill_suma
sleep 5

SET1:
# Deterministic pairwise connections between DMN ROIs.
afni -npb $sport -niml -yesplugouts -layout demo_layout *.HEAD DTI/*.HEAD &
"$sumaru_bin" -npb $sport -niml-suma \
    -onestate \
    -i SUMA/std.60.lh.smoothwm.gii \
    -i SUMA/std.60.rh.smoothwm.gii \
    -i Net_000.gii \
    -sv mprage+orig \
    -vol mprage+orig. \
    -tract DTI/o.NETS_AND_000.niml.tract &

DriveSuma -npb $sport \
    -com viewer_cont -key 't' -key '.' \
    -com surf_cont -view_surf_cont y
DriveSuma -npb $sport \
    -com surf_cont -surf_label Net_000.gii \
    -load_dset Net_000.cols.niml.dset \
    -switch_cmap ROI_i32 -Dim 0.3

sleep 5
set l = `prompt_user -pause "\
Example 1/4: Deterministic pairwise connections between DMN ROIs\n\
\n\
The DriveSuma commands should select Net_000.gii, load its color dataset,\n\
apply the ROI_i32 palette, dim the overlay, and connect Sumaru to AFNI.\n\
Use the tract inspector to switch among local-orientation, midpoint-\n\
orientation, and bundle coloring. '[' and ']' control hemisphere visibility.\n\
\n\
Hit OK when ready for Example 2."`
if ("$l" != "1") goto BEND

SET2:
# MINIPROB variant of the same network on a second NIML port bloc.
"$sumaru_bin" -npb $sport2 -niml-suma \
    -onestate \
    -i SUMA/std.60.lh.smoothwm.gii \
    -i SUMA/std.60.rh.smoothwm.gii \
    -i Net_000.gii \
    -sv mprage+orig \
    -vol mprage+orig. \
    -tract DTI/o.NETS_AND_MINIP_000.niml.tract &

sleep 5
set l = `prompt_user -pause "\
Example 2/4: MINIPROB pairwise connections between DMN ROIs\n\
\n\
Compare this second Sumaru process with the deterministic tract result. The\n\
two viewers use separate DriveSuma port blocs, matching Sumaru's one-view-per-\n\
process compatibility model.\n\
\n\
Hit OK when ready for Example 3."`
if ("$l" != "1") goto BEND

DriveSuma -npb $sport -com kill_suma
DriveSuma -npb $sport2 -com kill_suma
plugout_drive -npb $sport -com 'QUIT' -quit
sleep 5

SET3:
# Connectivity graph and matrix view.
afni -npb $sport -niml -yesplugouts -layout demo_layout *.HEAD DTI/*.HEAD &
cd DTI
"$sumaru_bin" -npb $sport -niml-suma \
    -onestate \
    -i ../SUMA/std.60.lh.smoothwm.gii \
    -i ../SUMA/std.60.rh.smoothwm.gii \
    -i ../Net_000.gii \
    -sv ../mprage+orig \
    -vol ../mprage+orig. \
    -gdset o.NETS_AND_MINIP_000.niml.dset &
cd ..

DriveSuma -npb $sport \
    -com viewer_cont -key 't' \
    -com surf_cont -view_surf_cont y

sleep 5
set l = `prompt_user -pause "\
Example 3/4: Connectivity graph and matrix\n\
\n\
Open the graph object's matrix view from the TRACTS / GRAPHS inspector. The\n\
matrix and 3D graph share their selected measure and edge selection. Enable\n\
linked tract bundles to replace matching straight edges with the network's\n\
tract bundles.\n\
\n\
Hit OK to close Sumaru and AFNI."`
if ("$l" != "1") goto BEND

DriveSuma -npb $sport -com kill_suma
plugout_drive -npb $sport -com 'QUIT' -quit
goto END

BEND:
set stat = 1
@Quiet_Talkers -npb_val $sport -npb_val $sport2

END:
exit $stat
