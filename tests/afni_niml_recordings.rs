use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use sumaru::io::NimlData;
use sumaru::niml_debug::{NimlDirection, read_debug_records, replay_records};

fn afni_niml_fixture(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testing")
        .join("afni_niml")
        .join(name);
    path.exists().then_some(path)
}

fn afni_niml_recording_fixture(stem: &str) -> Option<PathBuf> {
    afni_niml_fixture(&format!("{stem}.nimlrec"))
        .or_else(|| afni_niml_fixture(&format!("{stem}.nimlrec.gz")))
}

fn running_on_ci() -> bool {
    std::env::var_os("CI").is_some()
}

fn running_large_recording_tests() -> bool {
    std::env::var_os("SUMARU_RUN_LARGE_AFNI_NIML_TESTS").is_some()
}

#[test]
fn basic_afni_session_recording_replays_and_preserves_expected_messages() -> Result<()> {
    if !running_large_recording_tests() {
        eprintln!(
            "skipping large AFNI NIML recording test; set \
             SUMARU_RUN_LARGE_AFNI_NIML_TESTS=1 to replay the local fixture"
        );
        return Ok(());
    }

    let Some(path) = afni_niml_recording_fixture("basic_func") else {
        if running_on_ci() {
            bail!("testing/afni_niml/basic_func.nimlrec[.gz] is missing on CI");
        }
        eprintln!(
            "skipping AFNI NIML recording test: testing/afni_niml/basic_func.nimlrec[.gz] is absent"
        );
        return Ok(());
    };

    let records = read_debug_records(&path)?;
    let report = replay_records(&records)?;

    assert!(records.len() >= 40);
    assert_eq!(report.records, records.len());
    assert_eq!(report.elements, records.len());
    assert!(report.routed >= 20);
    assert!(report.ignored >= 18);
    assert!(report.rgba_overlays >= 18);
    assert!(report.surface_crosshairs >= 2);
    assert!(
        records
            .iter()
            .any(|record| record.direction == NimlDirection::Tx)
    );
    assert!(
        records
            .iter()
            .any(|record| record.direction == NimlDirection::Rx)
    );

    let mut top_level_counts = BTreeMap::<&str, usize>::new();
    for record in &records {
        for element in &record.elements {
            *top_level_counts.entry(element.name.as_str()).or_default() += 1;
        }
    }
    assert!(top_level_counts.get("SUMA_ixyz").copied().unwrap_or(0) >= 6);
    assert!(
        top_level_counts
            .get("SUMA_node_normals")
            .copied()
            .unwrap_or(0)
            >= 6
    );
    assert!(top_level_counts.get("SUMA_ijk").copied().unwrap_or(0) >= 6);
    assert!(
        top_level_counts
            .get("SUMA_crosshair_xyz")
            .copied()
            .unwrap_or(0)
            >= 2
    );
    assert!(top_level_counts.get("SUMA_crosshair").copied().unwrap_or(0) >= 2);
    assert!(top_level_counts.get("SUMA_irgba").copied().unwrap_or(0) >= 18);

    let rgba_rows = records
        .iter()
        .flat_map(|record| &record.elements)
        .filter(|element| element.name == "SUMA_irgba")
        .map(|element| {
            let NimlData::Numeric(matrix) = &element.data else {
                panic!("SUMA_irgba must be numeric");
            };
            assert_eq!(matrix.column_count(), 5);
            assert!(element.attrs.contains_key("surface_idcode"));
            assert!(element.attrs.contains_key("local_domain_parent_ID"));
            assert!(element.attrs.contains_key("volume_idcode"));
            matrix.rows
        })
        .collect::<Vec<_>>();
    assert!(rgba_rows.contains(&134_631));
    assert!(rgba_rows.contains(&136_938));

    Ok(())
}
