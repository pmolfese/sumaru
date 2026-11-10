// Developed by P. Molfese, National Institutes of Health (NIH).
// See LICENSE for the public-domain notice and disclaimer.
//! Build, inspect, recover, and apply explicit geometric/ancestry maps.
use crate::{
    io::SurfaceDatasetOptions,
    surface::SurfaceMesh,
    surface_mapping::{
        GeometricMap, MappingMethod, MappingOptions, centroid_translation, identity, read_affine,
    },
};
use anyhow::{Context, Result, ensure};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(clap::Args, Debug)]
pub struct MapArgs {
    #[arg(long)]
    pub source: PathBuf,
    #[arg(long)]
    pub target: PathBuf,
    #[arg(long,value_parser=["nearest-node","closest-triangle","normal-projection","registered-sphere","area-average","ancestry","afni-m2m"])]
    pub method: String,
    #[arg(short, long)]
    pub output: PathBuf,
    #[arg(long, conflicts_with = "affine")]
    pub centroid: bool,
    /// Row-major 3x4/4x4 source-to-target XYZ matrix. No implicit RAS/LPS conversion.
    #[arg(long)]
    pub affine: Option<PathBuf>,
    /// Convert an AFNI DICOM-basis affine to RAS (requires RAS input geometry).
    #[arg(long, requires = "affine")]
    pub affine_afni: bool,
    #[arg(long)]
    pub maximum_distance: Option<f64>,
    #[arg(long)]
    pub nearest_fallback: bool,
    #[arg(long)]
    pub source_sphere: Option<PathBuf>,
    #[arg(long)]
    pub target_sphere: Option<PathBuf>,
    /// Checked sumaru-weighted-map-1 file whose rows gather parent values at child nodes.
    #[arg(long)]
    pub weights: Option<PathBuf>,
    /// AFNI M2M import explicitly binds this file to --source (M2) and --target (M1).
    #[arg(long)]
    pub m2m: Option<PathBuf>,
    /// Summary report (default OUTPUT.report.json); per-node diagnostics live in the map.
    #[arg(long)]
    pub report: Option<PathBuf>,
}
#[derive(clap::Args, Debug)]
pub struct RecoverArgs {
    #[arg(long)]
    pub child: PathBuf,
    #[arg(long)]
    pub parent: PathBuf,
    #[arg(short, long)]
    pub output: PathBuf,
    #[arg(long, default_value_t = 0.00001)]
    pub tolerance: f64,
    #[arg(long, conflicts_with = "affine")]
    pub centroid: bool,
    #[arg(long)]
    pub affine: Option<PathBuf>,
    /// Convert an AFNI DICOM-basis affine to RAS (requires RAS input geometry).
    #[arg(long, requires = "affine")]
    pub affine_afni: bool,
}
pub fn read_affine_for_loaded_coordinates(path: &Path, afni: bool) -> Result<[[f64; 4]; 4]> {
    let matrix = read_affine(path)?;
    if !afni {
        return Ok(matrix);
    }
    use afni_core::affine::{AffineTransform, CoordinateConvention};
    Ok(
        *AffineTransform::new(matrix, CoordinateConvention::AfniDICOM)?
            .convert_coordinates(CoordinateConvention::Ras)?
            .matrix(),
    )
}
pub fn new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("refusing overwrite or unable to create {}", path.display()))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}
pub fn read_map(path: &Path, source: &SurfaceMesh, target: &SurfaceMesh) -> Result<GeometricMap> {
    let map: GeometricMap = serde_json::from_slice(&std::fs::read(path)?)?;
    map.validate(source, target)?;
    Ok(map)
}
pub fn run_map(args: &MapArgs) -> Result<()> {
    ensure!(!args.output.exists(), "map output already exists");
    let source = SurfaceMesh::from_path(&args.source)?;
    let target = SurfaceMesh::from_path(&args.target)?;
    ensure!(
        args.m2m.is_none() || args.method == "afni-m2m",
        "--m2m requires afni-m2m method"
    );
    let transform = if args.centroid {
        centroid_translation(&source, &target)
    } else if let Some(path) = &args.affine {
        read_affine_for_loaded_coordinates(path, args.affine_afni)?
    } else {
        identity()
    };
    let map = if args.method == "afni-m2m" {
        ensure!(
            !args.centroid
                && args.affine.is_none()
                && args.maximum_distance.is_none()
                && !args.nearest_fallback
                && args.weights.is_none()
                && args.source_sphere.is_none()
                && args.target_sphere.is_none(),
            "M2M import cannot be combined with geometric construction settings"
        );
        crate::surface_mapping::import_afni_m2m(
            args.m2m.as_deref().context("--m2m required")?,
            &source,
            &target,
        )?
    } else if args.method == "ancestry" {
        ensure!(
            !args.centroid
                && args.affine.is_none()
                && args.maximum_distance.is_none()
                && !args.nearest_fallback
                && args.source_sphere.is_none()
                && args.target_sphere.is_none(),
            "ancestry imports cannot also specify geometric search settings"
        );
        let mut map = read_map(
            args.weights
                .as_deref()
                .context("ancestry requires --weights")?,
            &source,
            &target,
        )?;
        map.method = "WeightedAncestry".into();
        map.notes.push(
            "Rows gather parent/source values at child/target nodes. This is not an inverse map."
                .into(),
        );
        map
    } else if args.method == "registered-sphere" {
        ensure!(
            !args.centroid
                && args.affine.is_none()
                && args.maximum_distance.is_none()
                && !args.nearest_fallback
                && args.weights.is_none(),
            "registered-sphere uses supplied registered spheres, not anatomical transforms/fallbacks"
        );
        GeometricMap::registered_spheres(
            &source,
            &target,
            &SurfaceMesh::from_path(
                args.source_sphere
                    .as_ref()
                    .context("--source-sphere required")?,
            )?,
            &SurfaceMesh::from_path(
                args.target_sphere
                    .as_ref()
                    .context("--target-sphere required")?,
            )?,
        )?
    } else {
        ensure!(
            args.source_sphere.is_none() && args.target_sphere.is_none() && args.weights.is_none(),
            "sphere/ancestry inputs require their corresponding method"
        );
        let method = match args.method.as_str() {
            "nearest-node" => MappingMethod::NearestNode,
            "closest-triangle" => MappingMethod::ClosestTriangle,
            "normal-projection" => MappingMethod::NormalProjection,
            "area-average" => MappingMethod::AreaAverage,
            _ => anyhow::bail!("unknown method"),
        };
        GeometricMap::build(
            &source,
            &target,
            &MappingOptions {
                method,
                transform,
                maximum_distance: args.maximum_distance,
                nearest_fallback: args.nearest_fallback,
            },
        )?
    };
    let report = args
        .report
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{}.report.json", args.output.display())));
    ensure!(
        report != args.output && !report.exists(),
        "report path must be new and differ from map"
    );
    new_file(&args.output, &serde_json::to_vec(&map)?)?;
    if let Err(error) = new_file(&report, &serde_json::to_vec_pretty(&map.summary())?) {
        let _ = std::fs::remove_file(&args.output);
        return Err(error);
    }
    println!(
        "{}\nSaved map {}",
        serde_json::to_string_pretty(&map.summary())?,
        args.output.display()
    );
    Ok(())
}
pub fn recover(args: &RecoverArgs) -> Result<()> {
    ensure!(
        args.output.extension().is_some_and(|e| e == "gii"),
        "recovery output must be anatomical .gii"
    );
    let child = SurfaceMesh::from_path(&args.child)?;
    let parent = SurfaceMesh::from_path(&args.parent)?;
    ensure!(
        crate::resample::is_original_parent(&parent),
        "parent must be in original full vertex order"
    );
    ensure!(
        child.metadata.parent_mapping.is_none(),
        "child already declares parent IDs; recovery never overrides existing metadata"
    );
    let transform = if args.centroid {
        centroid_translation(&child, &parent)
    } else if let Some(path) = &args.affine {
        read_affine_for_loaded_coordinates(path, args.affine_afni)?
    } else {
        identity()
    };
    let ids = GeometricMap::recover_parent_ids(&child, &parent, transform, args.tolerance)?;
    let mut image = geometry_image(&child);
    let identity = crate::resample::parent_identity(&parent);
    image.meta.push((
        "MNEParentVertexIndices".into(),
        ids.iter().map(u32::to_string).collect::<Vec<_>>().join(" "),
    ));
    image.meta.push((
        "MNEParentVertexCount".into(),
        parent.vertices.len().to_string(),
    ));
    if let Some(subject) = identity.subject {
        image.meta.push(("MNEParentSubject".into(), subject));
    }
    if let Some(side) = identity.hemisphere {
        let label = match side {
            crate::surface::SurfaceSide::Left => Some("lh"),
            crate::surface::SurfaceSide::Right => Some("rh"),
            _ => None,
        };
        if let Some(label) = label {
            image
                .meta
                .push(("MNEParentHemisphere".into(), label.into()));
        }
    }
    image.meta.push(("SumaruParentRecovery".into(),serde_json::json!({"transform":transform,"tolerance":args.tolerance,"parent_fingerprint":crate::surface_mapping::mesh_fingerprint(&parent),"coordinates":"original loaded child XYZ retained"}).to_string()));
    new_file(&args.output, gifti_rs::serialize(&image)?.as_bytes())?;
    println!(
        "Recovered {} unique parent IDs into {}",
        ids.len(),
        args.output.display()
    );
    Ok(())
}
pub fn geometry_image(mesh: &SurfaceMesh) -> gifti_rs::GiftiImage {
    use gifti_rs::*;
    let array = |intent, data: ArrayData, dims| DataArray {
        intent,
        datatype: data.dtype() as i32,
        array_index_order: ArrayIndexOrder::RowMajor,
        dims,
        encoding: Encoding::GZipBase64Binary,
        endian: Endian::Little,
        ext_filename: None,
        ext_offset: None,
        coordsys: vec![],
        meta: vec![],
        data,
    };
    GiftiImage {
        version: "1.0".into(),
        num_data_arrays: 2,
        meta: vec![],
        label_table: None,
        data_arrays: vec![
            array(
                intent::POINTSET,
                ArrayData::Float32(mesh.vertices.iter().flatten().copied().collect()),
                vec![mesh.vertices.len(), 3],
            ),
            array(
                intent::TRIANGLE,
                ArrayData::Int32(mesh.triangles.iter().flatten().map(|v| *v as i32).collect()),
                vec![mesh.triangles.len(), 3],
            ),
        ],
    }
}
pub fn apply_map(
    input: &Path,
    source_path: &Path,
    target_path: &Path,
    map_path: &Path,
    output_path: &Path,
    report_path: &Path,
    loading: &SurfaceDatasetOptions,
) -> Result<()> {
    ensure!(
        output_path != report_path && !output_path.exists() && !report_path.exists(),
        "output and report must be distinct new paths"
    );
    let source = SurfaceMesh::from_path(source_path)?;
    let target = SurfaceMesh::from_path(target_path)?;
    let map = read_map(map_path, &source, &target)?;
    let dataset = if input.extension().is_some_and(|e| e == "stc") {
        crate::io::read_stc_dataset(input, &source.domain)?
    } else if input.to_string_lossy().ends_with(".niml.dset") {
        crate::io::read_niml_dataset(input, &source.domain)?
    } else {
        crate::io::read_gifti_dataset_for_surface(input, &source, loading)?
    };
    let output = map.apply(&dataset, &target)?;
    let report = serde_json::json!({"schema":"sumaru-geometric-resample-1","version":env!("CARGO_PKG_VERSION"),"input":std::fs::canonicalize(input)?,"source":std::fs::canonicalize(source_path)?,"target":std::fs::canonicalize(target_path)?,"map":std::fs::canonicalize(map_path)?,"mapping":map.summary(),"warnings":dataset.mapping_warnings,"time_start_seconds":output.time_start_seconds,"time_step_seconds":output.time_step_seconds,"columns":output.columns.iter().map(|c|serde_json::json!({"name":c.label,"units":c.units})).collect::<Vec<_>>(),"validity":"NaN means unmapped or missing contributor; zeros are valid"});
    // Independent meshes have target-local numbering. Never carry source parent IDs.
    let identity = crate::surface_align::ParentDomainIdentity {
        vertex_count: Some(target.vertices.len()),
        ..Default::default()
    };
    let xml = if output_path.to_string_lossy().ends_with(".niml.dset") {
        crate::resample::export_niml(&output, &report)?
    } else {
        ensure!(
            output_path.extension().is_some_and(|e| e == "gii"),
            "expected .gii or .niml.dset output"
        );
        let mut image = crate::resample::export_image(&output, &identity, &report)?;
        image.meta.retain(|(key, _)| {
            !key.starts_with("MNEParent") && key != "SumaruResamplingProvenance"
        });
        image
            .meta
            .push(("SumaruGeometricProvenance".into(), report.to_string()));
        gifti_rs::serialize(&image)?
    };
    new_file(output_path, xml.as_bytes())?;
    if let Err(error) = new_file(report_path, &serde_json::to_vec_pretty(&report)?) {
        let _ = std::fs::remove_file(output_path);
        return Err(error);
    }
    println!(
        "Saved {}\nReport {}",
        output_path.display(),
        report_path.display()
    );
    Ok(())
}
