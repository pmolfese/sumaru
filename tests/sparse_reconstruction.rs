use std::sync::Arc;
use sumaru::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind};
use sumaru::sparse_reconstruction::{
    SparseFillMethod as Method, SparseProjectionCache, reconstruct_dataset,
};
use sumaru::surface::SurfaceMesh;
use sumaru::surface_align::{ParentDomainIdentity, ParentVertexMapping};

fn mesh() -> SurfaceMesh {
    SurfaceMesh::new(
        vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [5., 5., 5.]],
        vec![[0, 1, 2]],
    )
    .unwrap()
}
fn data(mesh: &SurfaceMesh) -> Dataset {
    Dataset::sparse(
        DatasetKind::SurfaceScalar,
        &mesh.domain,
        vec![2, 0],
        vec![
            DataColumn::new(
                "value",
                ColumnRole::Intensity,
                None,
                ColumnData::Float32(vec![4., 0.]),
            )
            .unwrap(),
        ],
    )
    .unwrap()
}
#[test]
fn scalar_reconstruction_keeps_samples_zero_and_missing_distinct() {
    let mesh = mesh();
    let source = data(&mesh);
    let original = source.clone();
    let mut cache = SparseProjectionCache::default();
    for method in [Method::Nearest, Method::Smooth] {
        let projection = cache
            .for_dataset(&mesh, &source, method, 1)
            .unwrap()
            .unwrap();
        let display = reconstruct_dataset(&source, 4, &projection).unwrap();
        let ColumnData::Float32(values) = &display.columns[0].values else {
            panic!()
        };
        assert_eq!(
            &values[..3],
            if method == Method::Nearest {
                &[0., 0., 4.]
            } else {
                &[2., 2., 2.]
            }
        );
        assert!(values[3].is_nan());
        assert_eq!(source, original);
        assert!(display.node_indices.is_none());
    }
    assert!(
        cache
            .for_dataset(&mesh, &source, Method::SampledOnly, 0)
            .unwrap()
            .is_none()
    );
}
#[test]
fn cache_keys_include_order_topology_and_settings() {
    let mut cache = SparseProjectionCache::default();
    let edges = vec![vec![1], vec![0, 2], vec![1]];
    let first = cache
        .get(&edges, &[0, 2], Method::Smooth, 1)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &cache
            .get(&edges, &[0, 2], Method::Smooth, 1)
            .unwrap()
            .unwrap()
    ));
    for (ids, method, steps) in [
        (vec![2, 0], Method::Smooth, 1),
        (vec![0], Method::Smooth, 1),
        (vec![0, 2], Method::Smooth, 2),
        (vec![0, 2], Method::Nearest, 1),
    ] {
        assert!(!Arc::ptr_eq(
            &first,
            &cache.get(&edges, &ids, method, steps).unwrap().unwrap()
        ));
    }
    let disconnected = vec![vec![], vec![], vec![]];
    let changed = cache
        .get(&disconnected, &[0, 2], Method::Smooth, 1)
        .unwrap()
        .unwrap();
    assert!(!Arc::ptr_eq(&first, &changed));
    assert!(changed.apply_dense_seeds(&[0., f32::NAN, 4.]).unwrap()[1].is_nan());
}
#[test]
fn categorical_and_child_targets_do_not_use_parent_smoothing() {
    let mut mesh = mesh();
    let mut source = data(&mesh);
    let mut cache = SparseProjectionCache::default();
    source.kind = DatasetKind::SurfaceLabel;
    assert!(
        cache
            .for_dataset(&mesh, &source, Method::Smooth, 10)
            .unwrap()
            .is_none()
    );
    source.kind = DatasetKind::SurfaceScalar;
    mesh.metadata.parent_mapping = Some(
        ParentVertexMapping::new(
            ParentDomainIdentity {
                vertex_count: Some(10),
                ..Default::default()
            },
            vec![0, 2, 4, 6],
            4,
        )
        .unwrap(),
    );
    assert!(
        cache
            .for_dataset(&mesh, &source, Method::Smooth, 10)
            .unwrap_err()
            .to_string()
            .contains("full parent connectivity")
    );
    assert!(
        cache
            .for_dataset(&mesh, &source, Method::SampledOnly, 10)
            .unwrap()
            .is_none()
    );
}
#[test]
fn malformed_connectivity_and_indices_are_errors() {
    let mut cache = SparseProjectionCache::default();
    for method in [Method::Nearest, Method::Smooth] {
        assert!(cache.get(&[vec![9]], &[0], method, 1).is_err());
        assert!(cache.get(&[vec![]], &[0, 0], method, 1).is_err());
        assert!(cache.get(&[vec![]], &[1], method, 1).is_err());
        assert!(cache.get(&[vec![]], &[], method, 1).is_err());
    }
}

#[test]
#[ignore = "requires private MNE examples via SUMARU_MNE_FIXTURE_DIR"]
fn local_fullres_scalar_and_time_series_reconstruction() {
    let directory = std::path::PathBuf::from(std::env::var("SUMARU_MNE_FIXTURE_DIR").unwrap());
    for (parent, data) in [
        (
            "sub-101_hemi-lh_desc-fullres_white.surf.gii",
            "sub-101_desc-allruns_cond-sighted_space-surface_method-LCMV-lh.time.gii",
        ),
        (
            "fsaverage_hemi-lh_desc-fullres_white.surf.gii",
            "sub-101_cond-sighted_space-fsaverage_method-LCMV-lh.time.gii",
        ),
    ] {
        let mesh = SurfaceMesh::from_path(directory.join(parent)).unwrap();
        let source = sumaru::io::read_gifti_dataset_for_surface(
            directory.join(data),
            &mesh,
            &Default::default(),
        )
        .unwrap();
        let mut scalar = source.clone();
        scalar.kind = DatasetKind::SurfaceScalar;
        scalar.columns.truncate(1);
        let before = scalar.clone();
        let mut cache = SparseProjectionCache::default();
        for method in [Method::Nearest, Method::Smooth] {
            let projection = cache
                .for_dataset(&mesh, &source, method, 10)
                .unwrap()
                .unwrap();
            let scalar_projection = cache
                .for_dataset(&mesh, &scalar, method, 10)
                .unwrap()
                .unwrap();
            assert!(Arc::ptr_eq(&projection, &scalar_projection));
            let dense = reconstruct_dataset(&scalar, mesh.domain.node_count, &projection).unwrap();
            let ColumnData::Float32(values) = &dense.columns[0].values else {
                panic!()
            };
            assert_eq!(values.len(), mesh.domain.node_count);
            assert!(values.iter().all(|v| v.is_finite()));
            for &node in source.node_indices.as_ref().unwrap().iter().step_by(200) {
                let from_rows = projection
                    .sample_node(node as usize, |row| match &scalar.columns[0].values {
                        ColumnData::Float32(v) => Some(v[row]),
                        _ => panic!(),
                    })
                    .unwrap();
                assert_eq!(values[node as usize], from_rows);
            }
            eprintln!(
                "{parent}: {:?}, {} samples -> {} finite display values",
                method,
                source.row_count,
                values.len()
            );
        }
        assert_eq!(scalar, before);
    }
}
