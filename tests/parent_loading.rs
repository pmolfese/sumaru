use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use sumaru::dataset::ColumnData;
use sumaru::io::{SurfaceDatasetOptions, read_gifti_dataset_for_surface};
use sumaru::surface::SurfaceMesh;
use sumaru::surface_align::{ParentMismatchPolicy, ParentVertexMapping};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/parent_mapping")
        .join(name)
}
fn child() -> SurfaceMesh {
    SurfaceMesh::from_path(fixture("lh.gii")).unwrap()
}
fn parent() -> SurfaceMesh {
    let mut mesh = SurfaceMesh::new(
        (0..10).map(|i| [i as f32, 0., 0.]).collect(),
        vec![[0, 1, 2]],
    )
    .unwrap();
    mesh.metadata.parent_identity = child().metadata.parent_identity;
    mesh
}
fn load(path: impl AsRef<Path>, mesh: &SurfaceMesh) -> sumaru::dataset::Dataset {
    read_gifti_dataset_for_surface(path, mesh, &SurfaceDatasetOptions::default()).unwrap()
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "sumaru-parent-loading-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn put(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn same_time_series_loads_on_child_parent_and_inflated_parent() {
    let local = load(fixture("lh.time.gii"), &child());
    assert!(local.node_indices.is_none());
    let mut full = parent();
    let sparse = load(fixture("lh.time.gii"), &full);
    assert!(sparse.mapping_warnings.is_empty());
    let strict = SurfaceDatasetOptions {
        parent_policy: ParentMismatchPolicy::Strict,
        ..Default::default()
    };
    assert!(read_gifti_dataset_for_surface(fixture("lh.time.gii"), &full, &strict).is_ok());
    assert_eq!(sparse.node_indices, Some(vec![8, 1, 4]));
    assert_eq!(sparse.columns, local.columns);
    assert_eq!(sparse.time_step_seconds, local.time_step_seconds);
    for vertex in &mut full.vertices {
        vertex[0] *= 100.;
        vertex[1] = 200.;
    }
    let inflated = load(fixture("lh.time.gii"), &full);
    assert_eq!(inflated.node_indices, sparse.node_indices);
    assert_eq!(inflated.columns, sparse.columns);
}

#[test]
fn differently_ordered_children_use_ids_not_row_positions() {
    let mut target = child();
    target.metadata.parent_mapping.as_mut().unwrap().indices = vec![4, 8, 1];
    let data = load(fixture("lh.time.gii"), &target);
    assert_eq!(data.node_indices, Some(vec![1, 2, 0]));
    assert_eq!(
        data.columns[0].values,
        ColumnData::Float32(vec![10., 20., 30.])
    );
    target.metadata.parent_mapping.as_mut().unwrap().indices = vec![4, 9, 1];
    let data = load(fixture("lh.time.gii"), &target);
    assert_eq!(data.node_indices, Some(vec![2, 0]));
    assert_eq!(data.columns[0].values, ColumnData::Float32(vec![20., 30.]));
    assert_eq!(data.parent_mapping.unwrap().indices, vec![1, 4]);
    assert!(data.mapping_warnings.iter().any(|s| s.contains("2 of 3")));
}

#[test]
fn children_with_no_shared_samples_require_reconstruction() {
    let mut target = child();
    target.metadata.parent_mapping.as_mut().unwrap().indices = vec![0, 2, 3];
    let error = read_gifti_dataset_for_surface(
        fixture("lh.time.gii"),
        &target,
        &SurfaceDatasetOptions::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("no shared"));
}

#[test]
fn full_parent_data_selects_child_samples_and_sparse_indices_still_work() {
    let temp = Temp::new();
    let xml = std::fs::read_to_string(fixture("legacy.time.gii"))
        .unwrap()
        .replace("Dim0=\"3\"", "Dim0=\"10\"")
        .replace("10 20 30", "0 1 2 3 4 5 6 7 8 9")
        .replace("11 21 31", "10 11 12 13 14 15 16 17 18 19");
    let data = load(temp.put("full.time.gii", &xml), &child());
    assert_eq!(data.node_indices, Some(vec![1, 2, 0]));
    assert_eq!(
        data.columns[0].values,
        ColumnData::Float32(vec![1., 4., 8.])
    );
    let sparse = load(fixture("sparse.time.gii"), &parent());
    assert_eq!(sparse.node_indices, Some(vec![8, 1, 4]));
    let on_child = load(fixture("sparse.time.gii"), &child());
    assert_eq!(on_child.node_indices, None);
    assert_eq!(on_child.columns[0].values, sparse.columns[0].values);
}

#[test]
fn legacy_companion_discovery_is_only_used_when_needed() {
    let temp = Temp::new();
    let path = temp.put(
        "older.time.gii",
        &std::fs::read_to_string(fixture("legacy.time.gii")).unwrap(),
    );
    temp.put(
        "older.gii",
        &std::fs::read_to_string(fixture("lh.gii")).unwrap(),
    );
    let data = load(&path, &parent());
    assert_eq!(data.node_indices, Some(vec![8, 1, 4]));
    temp.put(
        "older.gii",
        "invalid companion should not affect an existing direct load",
    );
    assert!(load(&path, &child()).node_indices.is_none());
    let enriched = temp.put(
        "older.time.gii",
        &std::fs::read_to_string(fixture("lh.time.gii")).unwrap(),
    );
    assert_eq!(load(enriched, &parent()).node_indices, Some(vec![8, 1, 4]));
}

#[test]
fn explicit_companions_are_validated_even_for_self_contained_data() {
    let options = SurfaceDatasetOptions {
        companion_surfaces: vec![fixture("lh.gii")],
        ..Default::default()
    };
    let result =
        read_gifti_dataset_for_surface(fixture("legacy.time.gii"), &parent(), &options).unwrap();
    assert_eq!(result.node_indices, Some(vec![8, 1, 4]));
    let bad = SurfaceDatasetOptions {
        companion_surfaces: vec![fixture("rh.gii")],
        ..Default::default()
    };
    assert!(read_gifti_dataset_for_surface(fixture("lh.time.gii"), &parent(), &bad).is_err());
    let pair = SurfaceDatasetOptions {
        companion_surfaces: vec![fixture("rh.gii"), fixture("lh.gii")],
        ..Default::default()
    };
    assert!(read_gifti_dataset_for_surface(fixture("lh.time.gii"), &parent(), &pair).is_ok());
}

#[test]
fn warn_and_strict_policies_do_not_bypass_structural_validation() {
    let mut wrong = parent();
    wrong.metadata.parent_identity.as_mut().unwrap().subject = Some("other".into());
    let loaded = load(fixture("lh.time.gii"), &wrong);
    assert!(loaded.mapping_warnings.iter().any(|s| s.contains("other")));
    let strict = SurfaceDatasetOptions {
        parent_policy: ParentMismatchPolicy::Strict,
        ..Default::default()
    };
    assert!(read_gifti_dataset_for_surface(fixture("lh.time.gii"), &wrong, &strict).is_err());
    let tiny = SurfaceMesh::new(vec![[0., 0., 0.]; 3], vec![[0, 1, 2]]).unwrap();
    assert!(
        read_gifti_dataset_for_surface(
            fixture("lh.time.gii"),
            &tiny,
            &SurfaceDatasetOptions::default()
        )
        .is_err()
    );
}

#[test]
fn malformed_timing_is_rejected_and_milliseconds_are_converted() {
    let temp = Temp::new();
    let xml = std::fs::read_to_string(fixture("lh.time.gii")).unwrap();
    for bad in [
        xml.replace("<Value>2</Value>", "<Value>3</Value>"),
        xml.replace("<Value>0.01</Value>", "<Value>nan</Value>"),
        xml.replacen(
            "<MetaData />",
            "<MetaData><MD><Name>TimeIndex</Name><Value>1</Value></MD></MetaData>",
            1,
        ),
        xml.replacen(
            "<MetaData />",
            "<MetaData><MD><Name>TimeValue</Name><Value>99</Value></MD></MetaData>",
            1,
        ),
    ] {
        assert!(
            read_gifti_dataset_for_surface(
                temp.put("bad.time.gii", &bad),
                &parent(),
                &SurfaceDatasetOptions::default()
            )
            .is_err()
        );
    }
    let ms = xml
        .replace("<Value>s</Value>", "<Value>ms</Value>")
        .replace("<Value>-0.2</Value>", "<Value>-200</Value>")
        .replace("<Value>0.01</Value>", "<Value>10</Value>");
    let data = load(temp.put("ms.time.gii", &ms), &parent());
    assert_eq!(data.time_start_seconds, Some(-0.2));
    assert_eq!(data.time_step_seconds, Some(0.01));
}

#[test]
fn no_metadata_stays_legacy_and_missing_time_or_units_are_not_invented() {
    let temp = Temp::new();
    let xml = std::fs::read_to_string(fixture("legacy.time.gii")).unwrap();
    let start = xml.find("  <MetaData>").unwrap();
    let end = xml.find("  </MetaData>").unwrap() + "  </MetaData>".len();
    let plain = format!("{}<MetaData />{}", &xml[..start], &xml[end..]);
    let data = load(temp.put("plain.gii", &plain), &child());
    assert!(data.parent_mapping.is_none());
    assert!(data.time_start_seconds.is_none());
    assert!(data.time_step_seconds.is_none());
    assert!(data.columns.iter().all(|c| c.units.is_none()));
}

#[test]
fn selected_rows_keep_physical_units() {
    let temp = Temp::new();
    let xml = std::fs::read_to_string(fixture("lh.time.gii"))
        .unwrap()
        .replacen(
            "<MetaData>",
            "<MetaData><MD><Name>DataUnits</Name><Value>Am</Value></MD>",
            1,
        );
    assert!(
        load(temp.put("units.time.gii", &xml), &parent())
            .columns
            .iter()
            .all(|c| c.units.as_deref() == Some("Am"))
    );
}

#[test]
fn conflicting_node_index_and_parent_metadata_are_rejected() {
    let temp = Temp::new();
    let xml = std::fs::read_to_string(fixture("sparse.time.gii")).unwrap()
        .replacen("<MetaData>", "<MetaData><MD><Name>MNEParentVertexIndices</Name><Value>7 2 5</Value></MD><MD><Name>MNEParentVertexCount</Name><Value>10</Value></MD>", 1);
    let error = read_gifti_dataset_for_surface(
        temp.put("conflict.gii", &xml),
        &parent(),
        &SurfaceDatasetOptions::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("conflicting row mappings"));
}

#[test]
fn changing_child_subset_is_not_mistaken_for_same_size_direct_loading() {
    let mut target = child();
    target.metadata.parent_mapping = Some(
        ParentVertexMapping::new(
            target.metadata.parent_identity.clone().unwrap(),
            vec![8, 4, 9],
            3,
        )
        .unwrap(),
    );
    let data = load(fixture("lh.time.gii"), &target);
    assert_eq!(data.row_count, 2);
    assert_eq!(data.node_indices, Some(vec![0, 1]));
}
