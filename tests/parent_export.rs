use std::path::{Path, PathBuf};
use sumaru::{
    dataset::ColumnData,
    io::{SurfaceDatasetOptions, read_gifti_dataset_for_surface},
    resample::{self, ResampleOptions},
    sparse_reconstruction::SparseFillMethod,
    surface::SurfaceMesh,
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "sumaru-export-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/parent_mapping")
        .join(name)
}
fn parent(path: &Path) {
    let mut image = sumaru::io::read_gifti_image(fixture("lh.gii")).unwrap();
    image.meta.retain(|(k, _)| k != "MNEParentVertexIndices");
    image.data_arrays[0].dims = vec![10, 3];
    image.data_arrays[0].data = gifti_rs::ArrayData::Float32(
        (0..10)
            .flat_map(|i| [i as f32, (i % 2) as f32, 0.])
            .collect(),
    );
    image.data_arrays[1].dims = vec![6, 3];
    image.data_arrays[1].data =
        gifti_rs::ArrayData::Int32((0..6).flat_map(|i| [i, i + 1, i + 2]).collect());
    gifti_rs::write(&image, path).unwrap();
}
#[test]
fn exports_roundtrip_sparse_dense_missing_timing_units_and_provenance() {
    let temp = Temp::new();
    let target = temp.0.join("parent.gii");
    parent(&target);
    let mesh = SurfaceMesh::from_path(&target).unwrap();
    let source =
        read_gifti_dataset_for_surface(fixture("lh.time.gii"), &mesh, &Default::default()).unwrap();
    for method in [
        SparseFillMethod::SampledOnly,
        SparseFillMethod::Nearest,
        SparseFillMethod::Smooth,
    ] {
        let output = temp.0.join(format!("{method:?}.gii"));
        let report = output.with_extension("json");
        let options = ResampleOptions {
            input: fixture("lh.time.gii"),
            target: target.clone(),
            output: output.clone(),
            report: report.clone(),
            method,
            smoothing_steps: 2,
            columns: vec![],
            loading: SurfaceDatasetOptions::default(),
        };
        let provenance = resample::run(&options).unwrap();
        let loaded = read_gifti_dataset_for_surface(&output, &mesh, &Default::default()).unwrap();
        assert_eq!(loaded.time_start_seconds, source.time_start_seconds);
        assert_eq!(loaded.time_step_seconds, source.time_step_seconds);
        assert_eq!(loaded.columns[0].units, source.columns[0].units);
        assert_eq!(
            provenance["ordered_source_vertex_ids"],
            serde_json::json!([8, 1, 4])
        );
        if method == SparseFillMethod::SampledOnly {
            assert_eq!(loaded.node_indices, source.node_indices);
            assert_eq!(loaded.columns, source.columns);
        } else {
            assert!(!loaded.is_sparse());
            let ColumnData::Float32(v) = &loaded.columns[0].values else {
                panic!()
            };
            assert!(v[9].is_nan());
            assert!(v[0].is_finite());
        }
        assert!(
            sumaru::io::read_gifti_image(&output)
                .unwrap()
                .meta
                .iter()
                .any(|(key, _)| key == "SumaruResamplingProvenance")
        );
        let before = std::fs::read(&output).unwrap();
        assert!(resample::run(&options).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), before);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(report).unwrap()).unwrap(),
            provenance
        );
    }
}
#[test]
fn failed_report_creation_does_not_leave_output_and_column_selection_preserves_time() {
    let temp = Temp::new();
    let target = temp.0.join("parent.gii");
    parent(&target);
    let mut options = ResampleOptions {
        input: fixture("lh.time.gii"),
        target: target.clone(),
        output: temp.0.join("out.gii"),
        report: temp.0.join("absent/report.json"),
        method: SparseFillMethod::SampledOnly,
        smoothing_steps: 10,
        columns: vec![1],
        loading: Default::default(),
    };
    assert!(resample::run(&options).is_err());
    assert!(!options.output.exists());
    options.report = temp.0.join("report.json");
    resample::run(&options).unwrap();
    let mesh = SurfaceMesh::from_path(target).unwrap();
    let loaded =
        read_gifti_dataset_for_surface(options.output, &mesh, &Default::default()).unwrap();
    assert_eq!(loaded.columns.len(), 1);
    assert!((loaded.time_start_seconds.unwrap() + 0.19).abs() < 1e-10);
}
#[test]
fn qc_mapping_keeps_local_connectivity_and_original_coordinates() {
    let temp = Temp::new();
    let target = temp.0.join("parent.gii");
    parent(&target);
    let mut mesh = SurfaceMesh::from_path(target).unwrap();
    let child = SurfaceMesh::from_path(fixture("lh.gii")).unwrap();
    let qc =
        sumaru::surface_qc::SurfaceCorrespondence::new(child.clone(), &mesh, Default::default())
            .unwrap();
    assert_eq!(qc.mapped.triangles, child.triangles);
    assert_eq!(qc.original.vertices, child.vertices);
    assert_eq!(qc.mapped.vertices[0], mesh.vertices[8]);
    assert_eq!(qc.child_for_parent(4), Some(2));
    assert_eq!(qc.child_for_parent(9), None);
    mesh.vertices[8] = [9., 8., 7.];
    let changed =
        sumaru::surface_qc::SurfaceCorrespondence::new(child, &mesh, Default::default()).unwrap();
    assert_eq!(changed.mapped.vertices[0], [9., 8., 7.]);
}

#[test]
fn niml_export_preserves_indices_values_timing_units_and_history() {
    let temp = Temp::new();
    let target = temp.0.join("parent.gii");
    parent(&target);
    let mesh = SurfaceMesh::from_path(&target).unwrap();
    for method in [SparseFillMethod::SampledOnly, SparseFillMethod::Nearest] {
        let options = ResampleOptions {
            input: fixture("lh.time.gii"),
            target: target.clone(),
            output: temp.0.join(format!("{method:?}.niml.dset")),
            report: temp.0.join(format!("{method:?}.json")),
            method,
            smoothing_steps: 10,
            columns: vec![],
            loading: Default::default(),
        };
        let report = resample::run(&options).unwrap();
        let loaded = sumaru::io::read_niml_dataset(&options.output, &mesh.domain).unwrap();
        let original =
            read_gifti_dataset_for_surface(fixture("lh.time.gii"), &mesh, &Default::default())
                .unwrap();
        assert_eq!(loaded.time_start_seconds, original.time_start_seconds);
        assert_eq!(loaded.time_step_seconds, original.time_step_seconds);
        assert_eq!(loaded.columns[0].units, original.columns[0].units);
        assert_eq!(loaded.kind, original.kind);
        if method == SparseFillMethod::SampledOnly {
            assert_eq!(loaded.node_indices, original.node_indices);
            assert_eq!(loaded.columns[0].values, original.columns[0].values);
        }
        assert_eq!(
            resample::read_provenance(&options.output).unwrap().unwrap(),
            report
        );
    }
}

#[test]
fn raw_qc_trace_requires_unchanged_source_and_strict_mode_checks_identity() {
    let temp = Temp::new();
    let target = temp.0.join("parent.gii");
    parent(&target);
    let mesh = SurfaceMesh::from_path(&target).unwrap();
    let input = temp.0.join("input.gii");
    std::fs::copy(fixture("lh.time.gii"), &input).unwrap();
    let options = ResampleOptions {
        input: input.clone(),
        target,
        output: temp.0.join("out.gii"),
        report: temp.0.join("report.json"),
        method: SparseFillMethod::Nearest,
        smoothing_steps: 10,
        columns: vec![1],
        loading: Default::default(),
    };
    let report = resample::run(&options).unwrap();
    let raw = resample::load_raw_export_source(&report, &mesh).unwrap();
    assert_eq!(raw.columns.len(), 1);
    let mut image = sumaru::io::read_gifti_image(&input).unwrap();
    if let gifti_rs::ArrayData::Float32(v) = &mut image.data_arrays[1].data {
        v[0] += 1.;
    } else {
        panic!()
    };
    gifti_rs::write(&image, &input).unwrap();
    assert!(
        resample::load_raw_export_source(&report, &mesh)
            .unwrap_err()
            .to_string()
            .contains("changed since export")
    );
    image
        .meta
        .iter_mut()
        .find(|(key, _)| key == "MNEParentSubject")
        .unwrap()
        .1 = "different".into();
    gifti_rs::write(&image, &input).unwrap();
    let strict = ResampleOptions {
        output: temp.0.join("strict.gii"),
        report: temp.0.join("strict.json"),
        loading: SurfaceDatasetOptions {
            parent_policy: sumaru::surface_align::ParentMismatchPolicy::Strict,
            ..Default::default()
        },
        ..options
    };
    assert!(resample::run(&strict).is_err());
    assert!(!strict.output.exists());
}
