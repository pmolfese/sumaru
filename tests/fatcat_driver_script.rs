use std::collections::BTreeSet;

const SCRIPT: &str = include_str!("../scripts/fatcat/Do_09_VISdti_sumaru_visual_ex2.tcsh");
const RS_SCRIPT: &str =
    include_str!("../scripts/fatcat/Do_09_VISdti_sumaru_visual_ex2_drivesuma_rs.tcsh");

#[test]
fn fatcat_driver_uses_explicit_suma_compatibility_mode_for_every_viewer() {
    let launches = SCRIPT
        .lines()
        .filter(|line| line.trim_start().starts_with("\"$sumaru_bin\" -npb"))
        .collect::<Vec<_>>();

    assert_eq!(
        launches.len(),
        4,
        "the four FATCAT examples must stay covered"
    );
    assert!(launches.iter().all(|line| line.contains("-niml-suma")));
    assert!(!launches.iter().any(|line| line.contains(" -niml ")));
}

#[test]
fn fatcat_driver_keeps_each_supported_scene_shape() {
    for required in [
        "-spec SUMA/std.60.FATCAT_both.spec",
        "-tract DTI/o.WB_000.niml.tract",
        "-tract DTI/o.NETS_AND_000.niml.tract",
        "-tract DTI/o.NETS_AND_MINIP_000.niml.tract",
        "-gdset o.NETS_AND_MINIP_000.niml.dset",
        "-vol mprage+orig.",
    ] {
        assert!(
            SCRIPT.contains(required),
            "missing FATCAT launch input: {required}"
        );
    }
}

#[test]
fn fatcat_driver_commands_match_the_recorded_compatibility_surface() {
    let expected = BTreeSet::from([
        "kill_suma",
        "viewer_cont",
        "surf_cont",
        "view_surf_cont",
        "surf_label",
        "load_dset",
        "switch_cmap",
        "ROI_i32",
        "Dim",
    ]);
    let present = expected
        .iter()
        .filter(|token| SCRIPT.contains(**token))
        .copied()
        .collect::<BTreeSet<_>>();

    assert_eq!(present, expected);
}

#[test]
fn fatcat_driver_uses_suma_drive_for_commands_and_native_prompts() {
    assert!(RS_SCRIPT.contains("set suma_drive_bin = suma-drive"));
    assert!(RS_SCRIPT.contains("$?SUMA_DRIVE_BIN"));
    assert!(RS_SCRIPT.contains("$?DRIVESUMA_BIN"));
    assert!(RS_SCRIPT.contains("\"$suma_drive_bin\" prompt --pause"));
    assert!(!RS_SCRIPT.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("DriveSuma ") || line.starts_with("prompt_user ")
    }));
    assert!(
        SCRIPT
            .lines()
            .any(|line| line.trim_start().starts_with("DriveSuma "))
    );
    assert!(SCRIPT.contains("prompt_user -pause"));
}
