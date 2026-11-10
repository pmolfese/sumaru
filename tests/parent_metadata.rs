use std::path::Path;

use sumaru::io::{gifti_parent_mapping, read_gifti_dataset, read_gifti_image};
use sumaru::surface::{SurfaceMesh, SurfaceSide};
use sumaru::surface_align::{ParentDomainIdentity, ParentMismatchPolicy, ParentVerification};

fn fixture(hemi: &str, times: bool) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/parent_mapping")
        .join(format!("{hemi}{}.gii", if times { ".time" } else { "" }))
}

#[test]
fn child_geometry_and_data_keep_local_order_and_parent_provenance() {
    for (hemi, side, ids) in [
        ("lh", SurfaceSide::Left, vec![8, 1, 4]),
        ("rh", SurfaceSide::Right, vec![7, 2, 5]),
    ] {
        let mesh = SurfaceMesh::from_gifti_path(fixture(hemi, false)).unwrap();
        let mapping = mesh.metadata.parent_mapping.as_ref().unwrap();
        assert_eq!(mapping.indices, ids);
        assert_eq!(mapping.parent.hemisphere, Some(side));
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
        assert_eq!(mesh.domain.node_count, 3);
        let report = sumaru::inspect::inspect_path(fixture(hemi, false)).unwrap();
        assert!(
            report
                .summary
                .contains("parent mapping: 3 ordered vertex IDs")
        );
        assert!(report.summary.contains("parent subject: synthetic"));
        let data = read_gifti_dataset(fixture(hemi, true), &mesh.domain).unwrap();
        assert_eq!(data.parent_mapping.as_ref(), Some(mapping));
        assert_eq!(data.node_indices, None); // still dense on its child
        assert_eq!(
            data.columns[0].values,
            sumaru::dataset::ColumnData::Float32(vec![10., 20., 30.])
        );
        assert_eq!(data.time_start_seconds, Some(-0.2));
        assert_eq!(data.time_step_seconds, Some(0.01));
        assert_eq!(
            data.vertex_mapping(ParentDomainIdentity::default())
                .unwrap(),
            Some(mapping.clone())
        );
    }
}

#[test]
fn missing_metadata_preserves_legacy_dense_loading() {
    let mut image = read_gifti_image(fixture("lh", true)).unwrap();
    image.meta.retain(|(key, _)| !key.starts_with("MNEParent"));
    assert!(gifti_parent_mapping(&image).unwrap().is_none());
    let mesh = SurfaceMesh::from_gifti_path(fixture("lh", false)).unwrap();
    let dataset = read_gifti_dataset(fixture("legacy", true), &mesh.domain).unwrap();
    assert!(dataset.parent_mapping.is_none());
    assert!(dataset.node_indices.is_none());
    assert_eq!(dataset.row_count, 3);
    assert_eq!(dataset.columns.len(), 2);
}

#[test]
fn stc_and_node_index_normalize_without_changing_existing_sparse_loading() {
    let domain = sumaru::surface::SurfaceDomain::from_triangles(10, vec![[0, 1, 2]]).unwrap();
    let identity = ParentDomainIdentity {
        vertex_count: Some(10),
        ..Default::default()
    };
    let gifti = read_gifti_dataset(fixture("sparse", true), &domain).unwrap();
    let stc = sumaru::io::read_stc_dataset(
        fixture("lh", false).with_file_name("sparse-lh.stc"),
        &domain,
    )
    .unwrap();
    for dataset in [gifti, stc] {
        assert_eq!(dataset.node_indices, Some(vec![8, 1, 4]));
        assert!(dataset.parent_mapping.is_none());
        let mapping = dataset.vertex_mapping(identity.clone()).unwrap().unwrap();
        assert_eq!(mapping.indices, vec![8, 1, 4]);
        assert_eq!(mapping.parent, identity);
        assert_eq!(
            dataset.columns[1].values,
            sumaru::dataset::ColumnData::Float32(vec![11., 21., 31.])
        );
    }
}

#[test]
fn array_metadata_is_supported_but_conflicting_copies_fail() {
    let mut image = read_gifti_image(fixture("lh", true)).unwrap();
    let expected = gifti_parent_mapping(&image).unwrap();
    image.data_arrays[0].meta = image.meta.clone();
    image.meta.clear();
    assert_eq!(gifti_parent_mapping(&image).unwrap(), expected);
    image
        .meta
        .push(("MNEParentVertexIndices".into(), "8\n1  4".into()));
    assert_eq!(gifti_parent_mapping(&image).unwrap(), expected);
    image.meta[0].1 = "1 8 4".into();
    assert!(
        gifti_parent_mapping(&image)
            .unwrap_err()
            .to_string()
            .contains("conflicting")
    );
}

#[test]
fn malformed_parent_metadata_is_never_silently_ignored() {
    for (key, value) in [
        ("MNEParentVertexIndices", ""),
        ("MNEParentVertexIndices", "8 8 4"),
        ("MNEParentVertexIndices", "8 -1 4"),
        ("MNEParentVertexIndices", "8 1.0 4"),
        ("MNEParentVertexIndices", "8 1 10"),
        ("MNEParentVertexIndices", "8 1"),
        ("MNEParentVertexCount", "0"),
        ("MNEParentHemisphere", "both"),
    ] {
        let mut image = read_gifti_image(fixture("lh", true)).unwrap();
        image.meta.iter_mut().find(|(k, _)| k == key).unwrap().1 = value.into();
        assert!(gifti_parent_mapping(&image).is_err(), "{key}={value}");
    }
    let mut image = read_gifti_image(fixture("lh", true)).unwrap();
    image.meta.retain(|(k, _)| k != "MNEParentVertexIndices");
    assert!(gifti_parent_mapping(&image).is_err());
}

#[test]
fn all_time_rows_are_checked_even_if_a_reader_would_skip_a_column() {
    let mut image = read_gifti_image(fixture("lh", true)).unwrap();
    image.data_arrays[1].dims = vec![2];
    image.data_arrays[1].data = gifti_rs::ArrayData::Float32(vec![1., 2.]);
    assert!(gifti_parent_mapping(&image).is_err());
}

#[test]
fn full_parent_geometry_can_declare_identity_without_a_redundant_index_list() {
    let mut image = read_gifti_image(fixture("lh", false)).unwrap();
    image
        .meta
        .retain(|(key, _)| key != "MNEParentVertexIndices");
    image
        .meta
        .iter_mut()
        .find(|(key, _)| key == "MNEParentVertexCount")
        .unwrap()
        .1 = "3".into();
    let metadata = sumaru::io::gifti_parent_metadata(&image).unwrap();
    assert!(metadata.mapping.is_none());
    assert_eq!(
        metadata.identity.unwrap().subject.as_deref(),
        Some("synthetic")
    );
}

#[test]
fn identity_mismatch_warns_by_default_but_invalid_addressing_always_fails() {
    let mapping = sumaru::io::read_gifti_parent_mapping(fixture("lh", true))
        .unwrap()
        .unwrap();
    let mut target = mapping.parent.clone();
    let report = mapping
        .assess_parent(&target, 10, ParentMismatchPolicy::default())
        .unwrap();
    assert_eq!(report.verification, ParentVerification::MetadataMatched);
    assert!(report.warnings.is_empty());
    target.subject = Some("another-subject".into());
    target.hemisphere = Some(SurfaceSide::Right);
    target.vertex_count = Some(11);
    let report = mapping
        .assess_parent(&target, 11, ParentMismatchPolicy::Warn)
        .unwrap();
    assert_eq!(report.verification, ParentVerification::Mismatch);
    assert!(report.warnings.iter().any(|s| s.contains("subject")));
    assert!(report.warnings.iter().any(|s| s.contains("hemisphere")));
    assert!(report.warnings.iter().any(|s| s.contains("count")));
    assert!(
        mapping
            .assess_parent(&target, 11, ParentMismatchPolicy::Strict)
            .is_err()
    );
    target.vertex_count = Some(8);
    assert!(
        mapping
            .assess_parent(&target, 8, ParentMismatchPolicy::Warn)
            .is_err()
    );
}

#[test]
fn explicit_identity_can_be_assessed_without_inventing_a_subject() {
    let mut mapping = sumaru::io::read_gifti_parent_mapping(fixture("lh", true))
        .unwrap()
        .unwrap();
    mapping.parent.subject = None;
    mapping.parent.vertex_order_fingerprint = Some("documented-test-fingerprint".into());
    assert_eq!(
        mapping
            .assess_parent(&mapping.parent, 10, ParentMismatchPolicy::Strict)
            .unwrap()
            .verification,
        ParentVerification::Verified
    );
}

#[test]
fn incomplete_metadata_warns_and_conflicting_fingerprints_override_matching_labels() {
    let mut mapping = sumaru::io::read_gifti_parent_mapping(fixture("lh", true))
        .unwrap()
        .unwrap();
    let mut target = mapping.parent.clone();
    target.subject = None;
    let result = mapping
        .assess_parent(&target, 10, ParentMismatchPolicy::Warn)
        .unwrap();
    assert_eq!(result.verification, ParentVerification::Unverified);
    assert!(result.warnings[0].contains("incomplete"));
    assert!(
        mapping
            .assess_parent(&target, 10, ParentMismatchPolicy::Strict)
            .is_err()
    );
    target = mapping.parent.clone();
    mapping.parent.vertex_order_fingerprint = Some("first".into());
    target.vertex_order_fingerprint = Some("different".into());
    assert_eq!(
        mapping
            .assess_parent(&target, 10, ParentMismatchPolicy::Warn)
            .unwrap()
            .verification,
        ParentVerification::Mismatch
    );
    assert!(
        mapping
            .assess_parent(&target, 10, ParentMismatchPolicy::Strict)
            .is_err()
    );
}

/// Local research data remain outside the repository. Run explicitly with
/// SUMARU_MNE_FIXTURE_DIR pointing to a directory of anatomy/time-series pairs.
#[test]
#[ignore = "requires external MNE exports"]
fn local_mne_exports_load_on_their_child_geometry() {
    let root = std::env::var("SUMARU_MNE_FIXTURE_DIR").expect("set SUMARU_MNE_FIXTURE_DIR");
    let mut pairs = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if let Some(stem) = name.strip_suffix(".time.gii") {
            let mesh =
                SurfaceMesh::from_gifti_path(path.with_file_name(format!("{stem}.gii"))).unwrap();
            let data = read_gifti_dataset(&path, &mesh.domain).unwrap();
            assert!(data.parent_mapping.is_some());
            assert_eq!(data.parent_mapping, mesh.metadata.parent_mapping);
            assert_eq!(data.row_count, mesh.vertices.len());
            assert!(data.time_step_seconds.is_some());
            let on_child = sumaru::io::read_gifti_dataset_for_surface(
                &path,
                &mesh,
                &sumaru::io::SurfaceDatasetOptions::default(),
            )
            .unwrap();
            assert!(on_child.node_indices.is_none());
            assert_eq!(on_child.columns, data.columns);
            let mapping = data.parent_mapping.as_ref().unwrap();
            let subject = mapping.parent.subject.as_ref().unwrap();
            let hemi = match mapping.parent.hemisphere {
                Some(SurfaceSide::Left) => "lh",
                Some(SurfaceSide::Right) => "rh",
                _ => panic!("expected a hemisphere"),
            };
            let parent_path =
                path.with_file_name(format!("{subject}_hemi-{hemi}_desc-fullres_white.surf.gii"));
            if parent_path.exists() {
                let parent = SurfaceMesh::from_gifti_path(parent_path).unwrap();
                assert!(parent.metadata.parent_mapping.is_none());
                let identity = parent.metadata.parent_identity.as_ref().unwrap();
                assert_eq!(mapping.parent, *identity);
                let assessment = mapping
                    .assess_parent(identity, parent.vertices.len(), ParentMismatchPolicy::Warn)
                    .unwrap();
                assert_eq!(assessment.verification, ParentVerification::MetadataMatched);
                assert!(assessment.warnings.is_empty());
                let on_parent = sumaru::io::read_gifti_dataset_for_surface(
                    &path,
                    &parent,
                    &sumaru::io::SurfaceDatasetOptions::default(),
                )
                .unwrap();
                assert_eq!(on_parent.node_indices.as_ref(), Some(&mapping.indices));
                assert!(on_parent.mapping_warnings.is_empty());
                assert_eq!(on_parent.columns, data.columns);
                assert_eq!(on_parent.time_start_seconds, data.time_start_seconds);
                assert_eq!(on_parent.time_step_seconds, data.time_step_seconds);
                // These fixtures are white surfaces in the same coordinate frame.
                // This check is not a general requirement for inflated/pial views.
                for (local, id) in mapping.indices.iter().enumerate() {
                    for axis in 0..3 {
                        assert!(
                            (mesh.vertices[local][axis] - parent.vertices[*id as usize][axis])
                                .abs()
                                < 1e-4
                        );
                    }
                }
            }
            pairs += 1;
        }
    }
    assert!(pairs > 0, "no anatomy/time-series pairs found");
    eprintln!("validated {pairs} local MNE export pairs");
}
