use std::path::PathBuf;

use anyhow::{Result, bail};
use sumaru::dataset::{ColumnData, ColumnRole, DatasetKind};
use sumaru::inspect::{FileKind, detect_file_kind, inspect_path};
use sumaru::io::{read_gifti_dataset, read_niml_dataset, read_niml_roi};
use sumaru::roi::{RoiBrushAction, RoiDrawingType, RoiElementKind, RoiSource};
use sumaru::surface::{OverlayDataset, SurfaceKind, SurfaceMesh, SurfaceSide};

fn local_fixture(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testing")
        .join(name);
    path.exists().then_some(path)
}

/// Most CI providers (CircleCI included) set `CI` in the environment. When set,
/// missing fixtures are a hard error rather than a silent skip.
fn running_on_ci() -> bool {
    std::env::var_os("CI").is_some()
}

#[test]
fn local_surface_fixture_loads_with_expected_counts_and_metadata() -> Result<()> {
    let Some(path) = local_fixture("rh.white.gii") else {
        eprintln!("skipping local fixture test: testing/rh.white.gii is absent");
        return Ok(());
    };

    let mesh = SurfaceMesh::from_gifti_path(path)?;

    assert_eq!(mesh.vertices.len(), 136_938);
    assert_eq!(mesh.triangles.len(), 273_872);
    assert_eq!(mesh.metadata.node_count, 136_938);
    assert_eq!(mesh.metadata.face_count, 273_872);
    assert_eq!(mesh.metadata.node_dimension, 3);
    assert_eq!(mesh.metadata.face_dimension, 3);
    assert_eq!(mesh.metadata.side, SurfaceSide::Right);
    assert_eq!(mesh.metadata.surface_kind, SurfaceKind::WhiteMatter);
    assert_eq!(mesh.metadata.state_name.as_deref(), Some("white"));
    assert!(mesh.bounds.radius.is_finite());
    assert!(mesh.bounds.radius > 0.0);

    Ok(())
}

#[test]
fn local_gifti_dataset_fixture_is_detected_and_loads_as_overlay() -> Result<()> {
    let Some(path) = local_fixture("rh.thickness.gii.dset") else {
        eprintln!("skipping local fixture test: testing/rh.thickness.gii.dset is absent");
        return Ok(());
    };

    assert_eq!(detect_file_kind(&path), Some(FileKind::Gifti));

    let report = inspect_path(&path)?;
    assert_eq!(report.kind, FileKind::Gifti);
    assert!(report.summary.contains("data arrays: 1"));

    let overlay = OverlayDataset::from_gifti_path(path, 136_938)?;
    assert_eq!(overlay.values.len(), 136_938);
    assert!(overlay.range.min.is_finite());
    assert!(overlay.range.max.is_finite());
    assert!(overlay.range.min < overlay.range.max);

    Ok(())
}

#[test]
fn local_niml_roi_fixture_loads_comment_wrapped_afni_roi() -> Result<()> {
    let Some(path) = local_fixture("suma_clickmiddle_joined.finished.niml.roi") else {
        eprintln!(
            "skipping local fixture test: testing/suma_clickmiddle_joined.finished.niml.roi is absent"
        );
        return Ok(());
    };

    let payloads = read_niml_roi(path)?;
    assert_eq!(payloads.len(), 1);

    let payload = &payloads[0];
    assert_eq!(payload.parent_side, SurfaceSide::Left);
    assert_eq!(payload.label, "12");
    assert_eq!(payload.integer_label, 2);
    assert_eq!(payload.drawing_type, RoiDrawingType::FilledArea);
    assert_eq!(payload.records.len(), 3);
    assert_eq!(payload.records[0].action_code, 1);
    assert_eq!(payload.records[0].element_type_code, 4);
    assert_eq!(payload.records[0].node_path.len(), 228);
    assert_eq!(payload.records[1].node_path.len(), 45);
    assert_eq!(payload.records[2].node_path.len(), 1139);

    let roi = payload.to_roi()?;
    assert_eq!(roi.source, RoiSource::NimlRoi);
    assert_eq!(roi.parent_side, SurfaceSide::Left);
    assert_eq!(roi.data.len(), 3);
    assert_eq!(roi.data[0].action, RoiBrushAction::AppendStroke);
    assert_eq!(roi.data[0].kind, RoiElementKind::NodeSegment);

    Ok(())
}

#[test]
fn local_binary_niml_dset_fixtures_load_as_canonical_datasets() -> Result<()> {
    let fixture_sets = [
        (
            "fs_lowres_std-lh.gii",
            ["ISC_lh_theta_neg.niml.dset", "ISC_lh_theta_pos.niml.dset"],
        ),
        (
            "fs_lowres_std-rh.gii",
            ["ISC_rh_theta_neg.niml.dset", "ISC_rh_theta_pos.niml.dset"],
        ),
    ];

    for (surface_name, dataset_names) in fixture_sets {
        let Some(surface_path) = local_fixture(surface_name) else {
            eprintln!("skipping local NIML dataset test: testing/{surface_name} is absent");
            return Ok(());
        };
        let mesh = SurfaceMesh::from_gifti_path(surface_path)?;
        assert_eq!(mesh.vertices.len(), 10_242);

        for dataset_name in dataset_names {
            let Some(dataset_path) = local_fixture(dataset_name) else {
                eprintln!("skipping local NIML dataset test: testing/{dataset_name} is absent");
                return Ok(());
            };

            let dataset = read_niml_dataset(dataset_path, &mesh.domain)?;

            assert_eq!(dataset.kind, DatasetKind::SurfaceScalar);
            assert!(!dataset.is_sparse());
            assert_eq!(dataset.row_count, 10_242);
            assert_eq!(dataset.columns.len(), 12);
            assert_eq!(dataset.columns[0].label, "Grp_HV");
            assert_eq!(dataset.columns[1].label, "Grp_HV t");
            assert_eq!(dataset.columns[0].role, ColumnRole::Intensity);
            assert_eq!(dataset.columns[1].role, ColumnRole::Statistic);
            assert_eq!(dataset.columns[1].stat.as_deref(), Some("Ttest(48)"));
            assert!(
                dataset
                    .parent_ids
                    .source_dataset_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with("XYZ_"))
            );

            match &dataset.columns[0].values {
                ColumnData::Float32(values) => {
                    assert_eq!(values.len(), 10_242);
                    assert!(values.iter().all(|value| value.is_finite()));
                }
                values => panic!("unexpected NIML column data: {values:?}"),
            }

            let range = dataset.columns[0].range.expect("first column has range");
            assert!(range.min < range.max);
            assert!(range.min.is_finite());
            assert!(range.max.is_finite());
        }
    }

    Ok(())
}

#[test]
fn local_lh_isc_niml_dset_preserves_reference_labels_and_ranges() -> Result<()> {
    let Some(surface_path) = local_fixture("fs_lowres_std-lh.gii") else {
        eprintln!("skipping local NIML dataset test: testing/fs_lowres_std-lh.gii is absent");
        return Ok(());
    };
    let Some(dataset_path) = local_fixture("ISC_lh_theta_neg.niml.dset") else {
        eprintln!("skipping local NIML dataset test: testing/ISC_lh_theta_neg.niml.dset is absent");
        return Ok(());
    };

    let mesh = SurfaceMesh::from_gifti_path(surface_path)?;
    let dataset = read_niml_dataset(dataset_path, &mesh.domain)?;

    assert_eq!(
        dataset
            .columns
            .iter()
            .map(|column| column.label.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Grp_HV",
            "Grp_HV t",
            "Grp_HVMD",
            "Grp_HVMD t",
            "Grp_MD",
            "Grp_MD t",
            "Grp_HV-MD",
            "Grp_HV-MD t",
            "Grp_HV-Mix",
            "Grp_HV-Mix t",
            "Grp_Mix-MD",
            "Grp_Mix-MD t",
        ]
    );

    let first_range = dataset.columns[0].range.expect("first column has range");
    assert_close(first_range.min, -0.001777, 0.000001);
    assert_close(first_range.max, 0.021278, 0.000001);

    Ok(())
}

#[test]
fn local_afni_gifti_dset_fixture_matches_converted_niml_dataset() -> Result<()> {
    let Some(surface_path) = local_fixture("fs_lowres_std-lh.gii") else {
        eprintln!("skipping local GIFTI dataset test: testing/fs_lowres_std-lh.gii is absent");
        return Ok(());
    };
    let Some(niml_path) = local_fixture("ISC_lh_theta_neg.niml.dset") else {
        eprintln!(
            "skipping local GIFTI dataset test: testing/ISC_lh_theta_neg.niml.dset is absent"
        );
        return Ok(());
    };
    let Some(gifti_path) = local_fixture("ISC_lh_theta_neg.gii.dset") else {
        eprintln!("skipping local GIFTI dataset test: testing/ISC_lh_theta_neg.gii.dset is absent");
        return Ok(());
    };

    let mesh = SurfaceMesh::from_gifti_path(surface_path)?;
    let niml = read_niml_dataset(niml_path, &mesh.domain)?;
    let report = inspect_path(&gifti_path)?;
    let gifti = read_gifti_dataset(&gifti_path, &mesh.domain)?;

    assert_eq!(report.kind, FileKind::Gifti);
    assert!(report.summary.contains("data arrays: 12"));
    assert_eq!(gifti.kind, DatasetKind::SurfaceScalar);
    assert!(!gifti.is_sparse());
    assert_eq!(gifti.row_count, 10_242);
    assert_eq!(gifti.columns.len(), niml.columns.len());
    assert_eq!(
        gifti
            .columns
            .iter()
            .map(|column| column.label.as_str())
            .collect::<Vec<_>>(),
        niml.columns
            .iter()
            .map(|column| column.label.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        gifti
            .columns
            .iter()
            .map(|column| column.stat.as_deref())
            .collect::<Vec<_>>(),
        niml.columns
            .iter()
            .map(|column| column.stat.as_deref())
            .collect::<Vec<_>>()
    );
    assert_eq!(gifti.columns[1].role, ColumnRole::Statistic);
    assert_eq!(gifti.columns[1].stat.as_deref(), Some("Ttest(48)"));

    let first_range = gifti.columns[0]
        .range
        .expect("first GIFTI column has range");
    assert_close(first_range.min, -0.001777, 0.000001);
    assert_close(first_range.max, 0.021278, 0.000001);

    let ColumnData::Float32(gifti_values) = &gifti.columns[0].values else {
        panic!(
            "unexpected GIFTI column data: {:?}",
            gifti.columns[0].values
        );
    };
    let ColumnData::Float32(niml_values) = &niml.columns[0].values else {
        panic!("unexpected NIML column data: {:?}", niml.columns[0].values);
    };
    for index in [0, 1, 12, 128, 1024, 10_241] {
        assert_close(
            gifti_values[index] as f64,
            niml_values[index] as f64,
            0.000001,
        );
    }

    Ok(())
}

#[test]
fn local_spec_fixture_exposes_expected_surface_entries() -> Result<()> {
    let Some(path) = local_fixture("sub-3_rh.spec") else {
        eprintln!("skipping local fixture test: testing/sub-3_rh.spec is absent");
        return Ok(());
    };

    let text = std::fs::read_to_string(path)?;
    let surface_count = text
        .lines()
        .filter(|line| line.trim() == "NewSurface")
        .count();
    let state_count = text
        .lines()
        .filter(|line| line.trim_start().starts_with("StateDef ="))
        .count();

    assert_eq!(surface_count, 7);
    assert_eq!(state_count, 7);
    assert!(text.contains("Group = sub-3"));
    assert!(text.contains("SurfaceName = rh.white.gii"));
    assert!(text.contains("LocalDomainParent = rh.smoothwm.gii"));

    Ok(())
}

#[test]
fn local_reference_folder_contains_expected_starter_files() -> Result<()> {
    let Some(testing_dir) = local_fixture(".").and_then(|path| path.canonicalize().ok()) else {
        // On CI the fixtures must be present, otherwise every other test in this
        // file silently skips and the build goes green without exercising real
        // files. Locally we stay permissive so contributors without the fixtures
        // are not blocked.
        if running_on_ci() {
            bail!(
                "testing/ fixtures are missing on CI; they must be committed so the \
                 integration tests actually run instead of skipping"
            );
        }
        eprintln!("skipping local fixture test: testing/ is absent");
        return Ok(());
    };

    let mut names = std::fs::read_dir(testing_dir)?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(Into::into)
        })
        .collect::<Result<Vec<String>>>()?;
    names.sort();

    let expected = [
        "ISC_lh_theta_neg.gii.dset",
        "ISC_lh_theta_neg.niml.dset",
        "ISC_lh_theta_pos.niml.dset",
        "ISC_rh_theta_neg.niml.dset",
        "ISC_rh_theta_pos.niml.dset",
        "fs_lowres_std-lh.gii",
        "fs_lowres_std-rh.gii",
        "rh.thickness.gii.dset",
        "rh.white.gii",
        "sub-3_rh.spec",
        "suma_clickmiddle_joined.finished.niml.roi",
    ];

    for expected_name in expected {
        if !names.iter().any(|name| name == expected_name) {
            bail!("missing expected local testing file {expected_name:?}; found {names:?}");
        }
    }

    Ok(())
}

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {actual} to be within {tolerance} of {expected}"
    );
}
