use glam::DVec3;
use sumaru::{
    dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind},
    surface::SurfaceMesh,
    surface_mapping::{GeometricMap, MappingMethod, MappingOptions, closest_triangle, identity},
};
fn mesh(v: Vec<[f32; 3]>, t: Vec<[u32; 3]>) -> SurfaceMesh {
    SurfaceMesh::new(v, t).unwrap()
}
fn plane(z: f32) -> SurfaceMesh {
    mesh(vec![[0., 0., z], [2., 0., z], [0., 2., z]], vec![[0, 1, 2]])
}
fn dataset(m: &SurfaceMesh, v: Vec<f32>) -> Dataset {
    Dataset::dense(
        DatasetKind::SurfaceTimeSeries,
        &m.domain,
        vec![
            DataColumn::new(
                "t0",
                ColumnRole::TimePoint,
                Some("nAm".into()),
                ColumnData::Float32(v),
            )
            .unwrap(),
        ],
    )
    .unwrap()
    .with_time_start_seconds(Some(-0.2))
    .with_time_step_seconds(Some(0.01))
}
#[test]
fn true_closest_triangle_beats_centroid_and_handles_edges_degeneracy() {
    let source = mesh(
        vec![
            [-100., -100., 0.],
            [100., -100., 0.],
            [0., 100., 0.],
            [-1., -1., 2.],
            [1., -1., 2.],
            [0., 1., 2.],
        ],
        vec![[0, 1, 2], [3, 4, 5]],
    );
    let target = plane(0.);
    let map = GeometricMap::build(&source, &target, &Default::default()).unwrap();
    assert_eq!(map.rows[0].triangle, Some(0));
    assert_eq!(map.rows[0].distance, Some(0.));
    let (q, w) = closest_triangle(DVec3::new(2., 2., 0.), DVec3::ZERO, DVec3::X, DVec3::Y);
    assert_eq!(q, DVec3::new(0.5, 0.5, 0.));
    assert_eq!(w, [0., 0.5, 0.5]);
    let (q, _) = closest_triangle(DVec3::new(0.5, 1., 0.), DVec3::ZERO, DVec3::ZERO, DVec3::X);
    assert_eq!(q, DVec3::new(0.5, 0., 0.));
}
#[test]
fn normal_projection_bidirectional_missing_and_explicit_fallback() {
    let source = plane(0.);
    let target = plane(1.);
    let options = MappingOptions {
        method: MappingMethod::NormalProjection,
        ..Default::default()
    };
    let map = GeometricMap::build(&source, &target, &options).unwrap();
    assert!(map.rows.iter().all(|r| r.signed_distance == Some(-1.)));
    let target = mesh(
        vec![[5., 5., 1.], [6., 5., 1.], [5., 6., 1.]],
        vec![[0, 1, 2]],
    );
    let miss = GeometricMap::build(&source, &target, &options).unwrap();
    assert!(miss.rows.iter().all(|r| r.weights.is_empty()));
    let fill = GeometricMap::build(
        &source,
        &target,
        &MappingOptions {
            nearest_fallback: true,
            ..options.clone()
        },
    )
    .unwrap();
    assert!(fill.rows.iter().all(|r| r.fallback));
    let clipped = GeometricMap::build(
        &source,
        &plane(1.),
        &MappingOptions {
            maximum_distance: Some(0.5),
            ..options
        },
    )
    .unwrap();
    assert!(clipped.rows.iter().all(|r| r.weights.is_empty()));
}
#[test]
fn affine_application_fingerprints_and_missing_contributors() {
    let source = plane(0.);
    let target = plane(4.);
    let mut transform = identity();
    transform[2][3] = 4.;
    let map = GeometricMap::build(
        &source,
        &target,
        &MappingOptions {
            transform,
            ..Default::default()
        },
    )
    .unwrap();
    let data = dataset(&source, vec![0., 2., 4.]);
    let result = map.apply(&data, &target).unwrap();
    assert_eq!(result.columns[0].values, data.columns[0].values);
    assert_eq!(result.time_start_seconds, Some(-0.2));
    assert_eq!(result.columns[0].units, Some("nAm".into()));
    let mut wrong = source.clone();
    wrong.vertices.swap(0, 1);
    assert!(map.validate(&wrong, &target).is_err());
    let mut bad = map.clone();
    bad.rows[0].source_nodes[0] = 99;
    assert!(bad.validate(&source, &target).is_err());
    assert!(bad.apply(&data, &target).is_err());
    let mut nan = dataset(&source, vec![0., f32::NAN, 4.]);
    let result = map.apply(&nan, &target).unwrap();
    let ColumnData::Float32(v) = result.columns[0].values.clone() else {
        panic!()
    };
    assert_eq!(v[0], 0.);
    assert!(v[1].is_nan());
    nan.columns[0].values = ColumnData::Float32(vec![0., 1., 4.]);
    let json = serde_json::to_vec(&map).unwrap();
    let restored: GeometricMap = serde_json::from_slice(&json).unwrap();
    restored.validate(&source, &target).unwrap();
}
#[test]
fn recovery_requires_unique_exact_matches_in_transformed_coordinates() {
    let parent = plane(0.);
    let mut child = plane(2.);
    let mut transform = identity();
    transform[2][3] = -2.;
    assert_eq!(
        GeometricMap::recover_parent_ids(&child, &parent, transform, 1e-5).unwrap(),
        vec![0, 1, 2]
    );
    assert!(GeometricMap::recover_parent_ids(&child, &parent, identity(), 1e-5).is_err());
    child.vertices[1] = child.vertices[0];
    assert!(GeometricMap::recover_parent_ids(&child, &parent, transform, 1e-5).is_err());
    let duplicate = mesh(
        vec![[0., 0., 0.], [0., 0., 0.], [0., 2., 0.]],
        vec![[0, 1, 2]],
    );
    assert!(GeometricMap::recover_parent_ids(&plane(0.), &duplicate, identity(), 1e-5).is_err());
}
fn octa() -> SurfaceMesh {
    mesh(
        vec![
            [1., 0., 0.],
            [-1., 0., 0.],
            [0., 1., 0.],
            [0., -1., 0.],
            [0., 0., 1.],
            [0., 0., -1.],
        ],
        vec![
            [0, 2, 4],
            [2, 1, 4],
            [1, 3, 4],
            [3, 0, 4],
            [2, 0, 5],
            [1, 2, 5],
            [3, 1, 5],
            [0, 3, 5],
        ],
    )
}
#[test]
fn registered_spheres_use_radial_weights_not_vertex_count() {
    let source = octa();
    let mut sphere = octa();
    let angle = std::f32::consts::FRAC_PI_4;
    for p in &mut sphere.vertices {
        let x = p[0] * angle.cos() - p[1] * angle.sin();
        let y = p[0] * angle.sin() + p[1] * angle.cos();
        p[0] = x;
        p[1] = y;
    }
    let map = GeometricMap::registered_spheres(&source, &sphere, &source, &sphere).unwrap();
    assert_eq!(map.rows[0].source_nodes, vec![0, 2]);
    assert!((map.rows[0].weights[0] - 0.5).abs() < 1e-7);
    let mut wrong = sphere.clone();
    wrong.triangles.swap(0, 1);
    assert!(GeometricMap::registered_spheres(&source, &sphere, &source, &wrong).is_err());
}
#[test]
fn area_average_preserves_constants_and_differs_from_point_sampling() {
    let source = plane(0.);
    let target = mesh(
        vec![[0.5, 0.5, 0.], [100., 0., 0.], [0., 100., 0.]],
        vec![[0, 1, 2]],
    );
    let map = GeometricMap::build(
        &source,
        &target,
        &MappingOptions {
            method: MappingMethod::AreaAverage,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(map.rows[0].source_nodes, vec![0, 1, 2]);
    assert!(map.rows[1].weights.is_empty());
    let out = map
        .apply(&dataset(&source, vec![0., 3., 6.]), &target)
        .unwrap();
    let ColumnData::Float32(v) = &out.columns[0].values else {
        panic!()
    };
    assert_eq!(v[0], 3.);
    assert!(v[1].is_nan());
}
#[test]
fn bvh_queries_match_brute_force() {
    let mut vertices = vec![];
    let mut triangles = vec![];
    for i in 0..40 {
        let x = (i * 17 % 31) as f32;
        let y = (i * 13 % 29) as f32;
        let z = (i % 7) as f32;
        let n = vertices.len() as u32;
        vertices.extend([[x, y, z], [x + 2., y, z], [x, y + 1., z]]);
        triangles.push([n, n + 1, n + 2]);
    }
    let source = mesh(vertices, triangles);
    let target = mesh(
        (0..30)
            .map(|i| [(i * 7 % 33) as f32, (i * 11 % 25) as f32, 2.25])
            .collect(),
        vec![[0, 1, 2]],
    );
    for method in [MappingMethod::ClosestTriangle, MappingMethod::NearestNode] {
        let map = GeometricMap::build(
            &source,
            &target,
            &MappingOptions {
                method,
                ..Default::default()
            },
        )
        .unwrap();
        for (p, row) in target.vertices.iter().zip(&map.rows) {
            let p = DVec3::from_array(p.map(f64::from));
            let expected = if method == MappingMethod::NearestNode {
                source
                    .vertices
                    .iter()
                    .map(|v| p.distance(DVec3::from_array(v.map(f64::from))))
                    .fold(f64::INFINITY, f64::min)
            } else {
                source
                    .triangles
                    .iter()
                    .map(|t| {
                        let [a, b, c] = t
                            .map(|i| DVec3::from_array(source.vertices[i as usize].map(f64::from)));
                        p.distance(closest_triangle(p, a, b, c).0)
                    })
                    .fold(f64::INFINITY, f64::min)
            };
            assert!((row.distance.unwrap() - expected).abs() < 1e-10);
        }
    }
}

#[test]
fn map_io_exports_ancestry_area_sphere_and_rejects_overwrite() {
    use sumaru::surface_mapping_cli::{apply_map, geometry_image, read_map};
    let dir = std::env::temp_dir().join(format!("sumaru-geometry-io-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = octa();
    let target = octa();
    let sp = dir.join("source.gii");
    let tp = dir.join("target.gii");
    gifti_rs::write(&geometry_image(&source), &sp).unwrap();
    gifti_rs::write(&geometry_image(&target), &tp).unwrap();
    let data = dataset(&source, vec![0., 1., 2., 3., 4., 5.]);
    let mut image =
        sumaru::resample::export_image(&data, &Default::default(), &serde_json::json!({})).unwrap();
    image
        .meta
        .retain(|(k, _)| !k.starts_with("MNEParent") && k != "SumaruResamplingProvenance");
    let input = dir.join("data.gii");
    gifti_rs::write(&image, &input).unwrap();
    for method in ["sphere", "area", "ancestry"] {
        let mut map = if method == "sphere" {
            GeometricMap::registered_spheres(&source, &target, &source, &target).unwrap()
        } else {
            GeometricMap::build(
                &source,
                &target,
                &MappingOptions {
                    method: MappingMethod::AreaAverage,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        if method == "ancestry" {
            map.method = "WeightedAncestry".into();
            map.rows[0].source_nodes = vec![0, 2];
            map.rows[0].weights = vec![0.25, 0.75];
        }
        let mp = dir.join(format!("{method}.json"));
        std::fs::write(&mp, serde_json::to_vec(&map).unwrap()).unwrap();
        read_map(&mp, &source, &target).unwrap();
        for extension in ["gii", "niml.dset"] {
            let output = dir.join(format!("{method}.{extension}"));
            let report = dir.join(format!("{method}.{extension}.report"));
            apply_map(&input, &sp, &tp, &mp, &output, &report, &Default::default()).unwrap();
            let restored = if extension == "gii" {
                let im = sumaru::io::read_gifti_image(&output).unwrap();
                assert!(!im.meta.iter().any(|(k, _)| k.starts_with("MNEParent")));
                sumaru::io::read_gifti_dataset_for_surface(&output, &target, &Default::default())
                    .unwrap()
            } else {
                sumaru::io::read_niml_dataset(&output, &target.domain).unwrap()
            };
            assert_eq!(restored.row_count, 6);
            assert_eq!(restored.time_start_seconds, Some(-0.2));
            assert_eq!(restored.time_step_seconds, Some(0.01));
            assert_eq!(restored.columns[0].units, Some("nAm".into()));
            let ColumnData::Float32(v) = &restored.columns[0].values else {
                panic!()
            };
            assert_eq!(v[0], if method == "ancestry" { 1.5 } else { 0. });
            assert!(
                apply_map(&input, &sp, &tp, &mp, &output, &report, &Default::default()).is_err()
            );
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn binary_and_ascii_m2m_preserve_missing_fallback_and_weights() {
    let xml = r#"<M2M ni_form="ni_group" M1_N_Nodes="3" M2_N_Nodes="3" M1Nn="3">
<M1n ni_type="int" ni_dimen="3">0 1 2</M1n>
<M2Nne_M1n ni_type="int" ni_dimen="3">2 1 0</M2Nne_M1n>
<M2ne_M1n ni_type="int" ni_dimen="3">0 2 1</M2ne_M1n>
<M2we_M1n ni_type="float" ni_dimen="3">0.25 0.75 1</M2we_M1n>
<M2t_M1n ni_type="int" ni_dimen="3">0 -1 -1</M2t_M1n>
<PD ni_type="float" ni_dimen="3">-2 3 0</PD>
</M2M>"#;
    let path = std::env::temp_dir().join(format!("sumaru-m2m-{}.niml", std::process::id()));
    let mesh = plane(0.);
    for binary in [false, true] {
        let elements = afni_io::niml::parse_bytes(xml.as_bytes()).unwrap();
        let bytes = if binary {
            afni_io::niml::serialize_binary(&elements)
        } else {
            xml.as_bytes().to_vec()
        };
        std::fs::write(&path, bytes).unwrap();
        let map = sumaru::surface_mapping::import_afni_m2m(&path, &mesh, &mesh).unwrap();
        assert_eq!(map.rows[0].weights, vec![0.25, 0.75]);
        assert_eq!(map.rows[0].signed_distance, Some(-2.));
        assert!(map.rows[1].fallback);
        assert!(map.rows[2].weights.is_empty());
    }
    for broken in [
        xml.replace("0.25 0.75 1", "-0.25 1.25 1"),
        xml.replace("0 2 1</M2ne", "0 99 1</M2ne"),
        xml.replace("0 1 2</M1n>", "0 0 2</M1n>"),
    ] {
        std::fs::write(&path, broken).unwrap();
        assert!(sumaru::surface_mapping::import_afni_m2m(&path, &mesh, &mesh).is_err());
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn affine_and_recovered_anatomy_roundtrip_without_replacing_coordinates() {
    use sumaru::surface_mapping_cli::{RecoverArgs, geometry_image, recover};
    let dir = std::env::temp_dir().join(format!("sumaru-recovery-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let parent = plane(0.);
    let child = plane(2.);
    let cp = dir.join("child.gii");
    let pp = dir.join("parent.gii");
    let out = dir.join("recovered.gii");
    let ap = dir.join("affine.1D");
    gifti_rs::write(&geometry_image(&child), &cp).unwrap();
    gifti_rs::write(&geometry_image(&parent), &pp).unwrap();
    let mut matrix = identity();
    matrix[2][3] = -2.;
    for text in [
        "1 0 0 0 0 1 0 0 0 0 1 -2\n",
        "1 0 0 0\n0 1 0 0\n0 0 1 -2\n",
        "1 0 0 0\n0 1 0 0\n0 0 1 -2\n0 0 0 1\n",
    ] {
        std::fs::write(&ap, text).unwrap();
        assert_eq!(sumaru::surface_mapping::read_affine(&ap).unwrap(), matrix);
    }
    recover(&RecoverArgs {
        child: cp,
        parent: pp.clone(),
        output: out.clone(),
        tolerance: 1e-5,
        centroid: false,
        affine: Some(ap),
        affine_afni: false,
    })
    .unwrap();
    let restored = SurfaceMesh::from_path(&out).unwrap();
    assert_eq!(restored.vertices, child.vertices);
    assert_eq!(restored.triangles, child.triangles);
    assert_eq!(
        restored.metadata.parent_mapping.unwrap().indices,
        vec![0, 1, 2]
    );
    assert!(
        recover(&RecoverArgs {
            child: out,
            parent: pp,
            output: dir.join("again.gii"),
            tolerance: 1e-5,
            centroid: true,
            affine: None,
            affine_afni: false,
        })
        .is_err()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn afni_affine_coordinates_are_converted_only_on_request() {
    let path = std::env::temp_dir().join(format!("sumaru-affine-basis-{}.1D", std::process::id()));
    std::fs::write(&path, "1 0 0 2 0 1 0 3 0 0 1 4\n").unwrap();
    let raw =
        sumaru::surface_mapping_cli::read_affine_for_loaded_coordinates(&path, false).unwrap();
    let ras = sumaru::surface_mapping_cli::read_affine_for_loaded_coordinates(&path, true).unwrap();
    assert_eq!([raw[0][3], raw[1][3], raw[2][3]], [2., 3., 4.]);
    assert_eq!([ras[0][3], ras[1][3], ras[2][3]], [-2., -3., 4.]);
    std::fs::remove_file(path).unwrap();
}
