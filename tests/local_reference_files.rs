use std::path::PathBuf;

use anyhow::{Result, bail};
use sumaru::inspect::{FileKind, detect_file_kind, inspect_path};
use sumaru::io::read_niml_roi;
use sumaru::roi::{RoiBrushAction, RoiDrawingType, RoiElementKind, RoiSource};
use sumaru::surface::{OverlayDataset, SurfaceKind, SurfaceMesh, SurfaceSide};

fn local_fixture(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testing")
        .join(name);
    path.exists().then_some(path)
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
