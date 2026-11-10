// Developed by P. Molfese, National Institutes of Health (NIH).
// See LICENSE for the public-domain notice and disclaimer.
//! Exact parent-ID placement and explicit reconstruction, with reproducible export.
use crate::{
    dataset::{ColumnData, ColumnRole, Dataset, DatasetKind},
    io::{SurfaceDatasetOptions, read_gifti_dataset_for_surface},
    sparse_reconstruction::{
        SparseFillMethod, SparseProjection, SparseProjectionCache, reconstruct_dataset,
    },
    surface::{SurfaceMesh, SurfaceSide},
    surface_align::ParentDomainIdentity,
};
use anyhow::{Context, Result, bail, ensure};
use gifti_rs::{ArrayData, ArrayIndexOrder, DataArray, Encoding, Endian, GiftiImage};
use serde_json::{Value, json};
use std::{fs::OpenOptions, io::Write, path::PathBuf};

pub struct ResampleOptions {
    pub input: PathBuf,
    pub target: PathBuf,
    pub output: PathBuf,
    pub report: PathBuf,
    pub method: SparseFillMethod,
    pub smoothing_steps: usize,
    pub columns: Vec<usize>,
    pub loading: SurfaceDatasetOptions,
}

pub fn parent_identity(mesh: &SurfaceMesh) -> ParentDomainIdentity {
    mesh.metadata
        .parent_identity
        .clone()
        .unwrap_or_else(|| ParentDomainIdentity {
            vertex_count: Some(mesh.vertices.len()),
            hemisphere: match mesh.metadata.side {
                SurfaceSide::Left | SurfaceSide::Right => Some(mesh.metadata.side.clone()),
                _ => None,
            },
            subject: mesh.metadata.subject_label.clone(),
            ..Default::default()
        })
}

pub fn identity_json(identity: &ParentDomainIdentity) -> Value {
    json!({"subject":identity.subject,"vertex_count":identity.vertex_count,
        "hemisphere":identity.hemisphere.as_ref().map(|h| format!("{h:?}")),
        "vertex_order_fingerprint":identity.vertex_order_fingerprint})
}

/// Writes new paths only, including the report. Existing files are never replaced.
pub fn run(options: &ResampleOptions) -> Result<Value> {
    ensure!(
        options.output != options.report,
        "output and report must have different paths"
    );
    for path in [&options.output, &options.report] {
        ensure!(!path.exists(), "refusing to overwrite {}", path.display());
    }
    let niml_output = options.output.to_string_lossy().ends_with(".niml.dset");
    ensure!(
        niml_output || options.output.extension().is_some_and(|e| e == "gii"),
        "export requires .gii or .niml.dset output"
    );
    let target = SurfaceMesh::from_path(&options.target)?;
    // Exports use the target's own node numbering; child export is a later route.
    ensure!(
        is_original_parent(&target),
        "resample requires a full parent target in original vertex order"
    );
    let mut source = if options.input.extension().is_some_and(|e| e == "stc") {
        crate::io::read_stc_dataset(&options.input, &target.domain)?
    } else {
        read_gifti_dataset_for_surface(&options.input, &target, &options.loading)?
    };
    if source.parent_mapping.is_none() {
        let mapping = crate::surface_align::ParentVertexMapping::new(
            ParentDomainIdentity::default(),
            source
                .node_indices
                .clone()
                .unwrap_or_else(|| (0..source.row_count as u32).collect()),
            source.row_count,
        )?;
        let assessment = mapping.assess_parent(
            &parent_identity(&target),
            target.vertices.len(),
            options.loading.parent_policy,
        )?;
        source.mapping_warnings.extend(assessment.warnings);
        source.parent_mapping = Some(mapping);
    }
    ensure!(
        !matches!(source.kind, DatasetKind::SurfaceLabel | DatasetKind::Roi),
        "resample exports scalar maps and time series; categorical export is not supported"
    );
    if !options.columns.is_empty() {
        ensure!(
            options.columns.windows(2).all(|w| w[0] < w[1]),
            "columns must be unique and increasing"
        );
        if source.kind == DatasetKind::SurfaceTimeSeries {
            ensure!(
                options.columns.windows(2).all(|w| w[1] == w[0] + 1),
                "time-series selection must be contiguous to preserve regular timing"
            );
            if let (Some(start), Some(step)) = (source.time_start_seconds, source.time_step_seconds)
            {
                source.time_start_seconds = Some(start + options.columns[0] as f64 * step);
            }
        }
        source.columns = options
            .columns
            .iter()
            .map(|&i| {
                source
                    .columns
                    .get(i)
                    .cloned()
                    .with_context(|| format!("column {i} is out of range"))
            })
            .collect::<Result<_>>()?;
    }
    ensure!(
        source.columns.iter().all(|c| !matches!(
            c.role,
            ColumnRole::Label | ColumnRole::Mask | ColumnRole::NodeIndex
        ) && !matches!(c.values, ColumnData::Text(_))),
        "export supports numeric scalar/time-point columns, not categorical, mask, index, or text columns"
    );
    for warning in &source.mapping_warnings {
        eprintln!("Warning: {warning}");
    }
    let mut cache = SparseProjectionCache::default();
    let projection =
        cache.for_dataset(&target, &source, options.method, options.smoothing_steps)?;
    if options.method != SparseFillMethod::SampledOnly && source.is_sparse() {
        ensure!(
            projection.is_some(),
            "selected columns cannot be reconstructed"
        );
    }
    let output = match &projection {
        Some(p) => reconstruct_dataset(&source, target.vertices.len(), p)?,
        None => source.clone(),
    };
    let identity = parent_identity(&target);
    let ids: Vec<u32> = source
        .node_indices
        .clone()
        .unwrap_or_else(|| (0..source.row_count as u32).collect());
    let method = method_name(options.method);
    let (steps, fallback) = match projection.as_deref() {
        Some(SparseProjection::Smooth(p)) => (Some(p.smoothing_steps), p.nearest_fallback_nodes),
        _ => (None, 0),
    };
    let source_identity = source
        .parent_mapping
        .as_ref()
        .map(|m| identity_json(&m.parent));
    let verification = source
        .parent_mapping
        .as_ref()
        .map(|mapping| {
            mapping.assess_parent(
                &identity,
                target.vertices.len(),
                options.loading.parent_policy,
            )
        })
        .transpose()?;
    let finite_counts: Vec<usize> = output
        .columns
        .iter()
        .map(|column| finite_count(&column.values))
        .collect::<Result<_>>()?;
    let report = json!({"schema":"sumaru-parent-resample-1", "sumaru_version":env!("CARGO_PKG_VERSION"),
        "input":std::fs::canonicalize(&options.input)?,"target":std::fs::canonicalize(&options.target)?,"output":options.output,
        "companion_surfaces":options.loading.companion_surfaces.iter().map(std::fs::canonicalize).collect::<std::io::Result<Vec<_>>>()?,
        "method":method,"reconstruction_applied":projection.is_some(),"smoothing_steps_requested":if options.method==SparseFillMethod::Smooth {Some(options.smoothing_steps)} else {None},"smoothing_steps":steps,"nearest_fallback_nodes":fallback,
        "nearest_distance":"mesh edge hops; ties use lowest source node ID", "validity":"finite values are valid; NaN is missing, zero is a real value",
        "target_parent":identity_json(&identity),"source_parent":source_identity,
        "parent_verification":verification.map(|a| format!("{:?}",a.verification)).unwrap_or_else(|| "Unverified".into()),
        "warnings":source.mapping_warnings,"ordered_source_vertex_ids":ids,
        "raw_source_fingerprint_fnv1a64":dataset_fingerprint(&source),"selected_columns":options.columns,"source_rows":source.row_count,"target_vertices":target.vertices.len(),"output_rows":output.row_count,
        "finite_values_per_column":finite_counts,"time_start_seconds":output.time_start_seconds,"time_step_seconds":output.time_step_seconds,
        "columns":output.columns.iter().map(|c| json!({"name":c.label,"units":c.units})).collect::<Vec<_>>()});
    let xml = if niml_output {
        export_niml(&output, &report)?
    } else {
        gifti_rs::serialize(&export_image(&output, &identity, &report)?)?
    };
    let report_bytes = serde_json::to_vec_pretty(&report)?;
    // Acquire both paths before writing. create_new also rejects symlinks/races.
    let mut data_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&options.output)?;
    let mut report_file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&options.report)
    {
        Ok(file) => file,
        Err(error) => {
            drop(data_file);
            let _ = std::fs::remove_file(&options.output);
            return Err(error.into());
        }
    };
    let result = (|| -> Result<()> {
        data_file.write_all(xml.as_bytes())?;
        data_file.sync_all()?;
        report_file.write_all(&report_bytes)?;
        report_file.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        drop(data_file);
        drop(report_file);
        let _ = std::fs::remove_file(&options.output);
        let _ = std::fs::remove_file(&options.report);
    }
    result?;
    Ok(report)
}

pub fn method_name(method: SparseFillMethod) -> &'static str {
    match method {
        SparseFillMethod::SampledOnly => "sampled-only",
        SparseFillMethod::Nearest => "mesh-nearest",
        SparseFillMethod::Smooth => "mne-smooth",
    }
}

fn finite_count(values: &ColumnData) -> Result<usize> {
    Ok(match values {
        ColumnData::Float32(v) => v.iter().filter(|v| v.is_finite()).count(),
        ColumnData::Float64(v) => v.iter().filter(|v| v.is_finite()).count(),
        ColumnData::Int32(v) => v.len(),
        ColumnData::UInt32(v) => v.len(),
        ColumnData::Text(_) => bail!("non-numeric export column"),
    })
}

fn array(intent: i32, data: ArrayData, count: usize, meta: Vec<(String, String)>) -> DataArray {
    DataArray {
        intent,
        datatype: data.dtype() as i32,
        array_index_order: ArrayIndexOrder::RowMajor,
        dims: vec![count],
        encoding: Encoding::GZipBase64Binary,
        endian: Endian::Little,
        ext_filename: None,
        ext_offset: None,
        coordsys: vec![],
        meta,
        data,
    }
}

pub fn export_image(
    dataset: &Dataset,
    identity: &ParentDomainIdentity,
    report: &Value,
) -> Result<GiftiImage> {
    let mut meta = vec![
        (
            "SumaruResamplingProvenance".into(),
            serde_json::to_string(report)?,
        ),
        (
            "SumaruValidity".into(),
            "Finite values valid; NaN missing; zero is a real value".into(),
        ),
    ];
    if let Some(n) = identity.vertex_count {
        meta.push(("MNEParentVertexCount".into(), n.to_string()));
    }
    if let Some(subject) = &identity.subject {
        meta.push(("MNEParentSubject".into(), subject.clone()));
    }
    if let Some(side) = &identity.hemisphere {
        let side = match side {
            SurfaceSide::Left => Some("lh"),
            SurfaceSide::Right => Some("rh"),
            _ => None,
        };
        if let Some(side) = side {
            meta.push(("MNEParentHemisphere".into(), side.into()));
        }
    }
    if let Some(start) = dataset.time_start_seconds {
        meta.push(("TimeStart".into(), start.to_string()));
    }
    if let Some(step) = dataset.time_step_seconds {
        meta.push(("TimeStep".into(), step.to_string()));
    }
    if dataset.kind == DatasetKind::SurfaceTimeSeries {
        meta.push(("TimeUnits".into(), "s".into()));
        meta.push(("TimeSamples".into(), dataset.columns.len().to_string()));
    }
    let mut arrays = Vec::new();
    if dataset.node_indices.is_none() {
        meta.push((
            "MNEParentVertexIndices".into(),
            (0..dataset.row_count)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        ));
    }
    if let Some(ids) = &dataset.node_indices {
        meta.push((
            "MNEParentVertexIndices".into(),
            ids.iter().map(u32::to_string).collect::<Vec<_>>().join(" "),
        ));
        let indices = ids
            .iter()
            .map(|&id| i32::try_from(id).context("GIFTI node index exceeds int32"))
            .collect::<Result<Vec<_>>>()?;
        arrays.push(array(
            gifti_rs::intent::NODE_INDEX,
            ArrayData::Int32(indices),
            dataset.row_count,
            vec![],
        ));
    }
    for (index, column) in dataset.columns.iter().enumerate() {
        ensure!(
            !matches!(
                column.role,
                ColumnRole::Label | ColumnRole::Mask | ColumnRole::NodeIndex
            ),
            "unsupported categorical/index data column"
        );
        let mut cm = vec![("Name".into(), column.label.clone())];
        if let Some(units) = &column.units {
            cm.push(("Units".into(), units.clone()));
        }
        let intent = if column.role == ColumnRole::TimePoint {
            cm.push(("TimeIndex".into(), index.to_string()));
            gifti_rs::intent::TIME_SERIES
        } else {
            gifti_rs::intent::NONE
        };
        // Preserve exact scalar types for sampled-only output. Reconstruction is float32.
        let data = match &column.values {
            ColumnData::Float32(v) => ArrayData::Float32(v.clone()),
            ColumnData::Float64(v) => ArrayData::Float64(v.clone()),
            ColumnData::Int32(v) => ArrayData::Int32(v.clone()),
            ColumnData::UInt32(v) => ArrayData::UInt32(v.clone()),
            ColumnData::Text(_) => bail!("text export unsupported"),
        };
        arrays.push(array(intent, data, dataset.row_count, cm));
    }
    Ok(GiftiImage {
        version: "1.0".into(),
        num_data_arrays: arrays.len() as i32,
        meta,
        label_table: None,
        data_arrays: arrays,
    })
}

/// AFNI-compatible numeric/index arrays; Sumaru-specific timing/units/provenance
/// live in HISTORY_NOTE and the JSON sidecar, not invented AFNI attributes.
pub fn export_niml(dataset: &Dataset, report: &Value) -> Result<String> {
    use crate::io::{NimlDatasetPayload, NimlNumericMatrix, NimlValueType};
    let column_types = dataset
        .columns
        .iter()
        .map(|c| match c.values {
            ColumnData::Int32(_) => NimlValueType::Int32,
            ColumnData::Float32(_) => NimlValueType::Float32,
            _ => NimlValueType::Float64,
        })
        .collect();
    let mut values = Vec::with_capacity(dataset.row_count * dataset.columns.len());
    for row in 0..dataset.row_count {
        for column in &dataset.columns {
            values.push(match &column.values {
                ColumnData::Float32(v) => v[row] as f64,
                ColumnData::Float64(v) => v[row],
                ColumnData::Int32(v) => v[row] as f64,
                ColumnData::UInt32(v) => v[row] as f64,
                ColumnData::Text(_) => bail!("text export unsupported"),
            });
        }
    }
    let payload = NimlDatasetPayload {
        dset_type: if dataset.kind == DatasetKind::SurfaceTimeSeries {
            "Node_Bucket_TimeSeries"
        } else {
            "Node_Bucket"
        }
        .into(),
        self_idcode: None,
        filename: None,
        label: Some("Sumaru parent resampling".into()),
        sparse_data: Some(NimlNumericMatrix::new(
            column_types,
            dataset.row_count,
            values,
        )?),
        time_step_seconds: dataset.time_step_seconds,
        node_indices: dataset.node_indices.clone(),
        column_ranges: vec![],
        column_labels: dataset.columns.iter().map(|c| c.label.clone()).collect(),
        column_types: dataset
            .columns
            .iter()
            .map(|c| {
                if c.role == ColumnRole::TimePoint {
                    "TimePoint"
                } else {
                    "Generic_Float"
                }
                .into()
            })
            .collect(),
        column_stats: vec![],
        fdr_curves: Default::default(),
        history: Some(format!(
            "SUMARU_RESAMPLE_JSON\n{}",
            serde_json::to_string(report)?
        )),
    };
    Ok(crate::io::serialize_niml_ascii(&[payload.to_element()?]))
}

pub fn read_provenance(path: &std::path::Path) -> Result<Option<Value>> {
    let text = if path.extension().is_some_and(|e| e == "gii") {
        crate::io::read_gifti_image(path)?
            .meta
            .into_iter()
            .find(|(k, _)| k == "SumaruResamplingProvenance")
            .map(|(_, v)| v)
    } else if path.to_string_lossy().ends_with(".niml.dset") {
        let elements = crate::io::read_niml(path)?;
        elements
            .iter()
            .find(|e| e.name == "AFNI_dataset")
            .map(crate::io::NimlDatasetPayload::from_element)
            .transpose()?
            .and_then(|p| p.history)
            .and_then(|h| h.strip_prefix("SUMARU_RESAMPLE_JSON\n").map(str::to_owned))
    } else {
        None
    };
    text.map(|text| -> Result<Value> {
        let report: Value = serde_json::from_str(&text).context("invalid export provenance")?;
        ensure!(
            report["schema"] == "sumaru-parent-resample-1",
            "unsupported export provenance schema"
        );
        let target = report["target_vertices"]
            .as_u64()
            .context("missing export target count")?;
        let ids = report["ordered_source_vertex_ids"]
            .as_array()
            .context("missing export source IDs")?;
        ensure!(
            report["source_rows"].as_u64() == Some(ids.len() as u64),
            "export source count mismatch"
        );
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            let id = id.as_u64().context("invalid export source ID")?;
            ensure!(
                id < target && seen.insert(id),
                "out-of-range or duplicate export source ID"
            );
        }
        Ok(report)
    })
    .transpose()
}

pub fn load_raw_export_source(report: &Value, parent: &SurfaceMesh) -> Result<Dataset> {
    let path = std::path::Path::new(
        report["input"]
            .as_str()
            .context("export has no source path")?,
    );
    let companions = report["companion_surfaces"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|v| v.as_str().map(PathBuf::from))
                .collect()
        })
        .unwrap_or_default();
    let mut source = if path.extension().is_some_and(|e| e == "stc") {
        crate::io::read_stc_dataset(path, &parent.domain)?
    } else {
        read_gifti_dataset_for_surface(
            path,
            parent,
            &SurfaceDatasetOptions {
                companion_surfaces: companions,
                ..Default::default()
            },
        )?
    };
    let ids: Vec<u32> = source
        .node_indices
        .clone()
        .unwrap_or_else(|| (0..source.row_count as u32).collect());
    ensure!(
        serde_json::to_value(&ids)? == report["ordered_source_vertex_ids"],
        "source IDs no longer match exported provenance"
    );
    if let Some(columns) = report["selected_columns"]
        .as_array()
        .filter(|v| !v.is_empty())
    {
        source.columns = columns
            .iter()
            .map(|index| {
                source
                    .columns
                    .get(index.as_u64().context("invalid column index")? as usize)
                    .cloned()
                    .context("source columns no longer match export")
            })
            .collect::<Result<_>>()?;
        source.time_start_seconds = report["time_start_seconds"].as_f64();
    }
    if let Some(expected) = report["raw_source_fingerprint_fnv1a64"].as_str() {
        ensure!(
            dataset_fingerprint(&source) == expected,
            "original sampled data changed since export; raw comparison is unavailable"
        );
    }
    Ok(source)
}

/// Non-cryptographic content fingerprint for detecting changed raw trace inputs.
/// Uses exact typed value bytes and local node order; not a mesh identity claim.
pub fn dataset_fingerprint(dataset: &Dataset) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    let mut add = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    for row in 0..dataset.row_count {
        add(&dataset.node_for_row(row).unwrap_or(u32::MAX).to_le_bytes());
    }
    for column in &dataset.columns {
        add(column.label.as_bytes());
        add(&[0]);
        match &column.values {
            ColumnData::Float32(v) => {
                add(b"f32");
                for x in v {
                    add(&x.to_le_bytes());
                }
            }
            ColumnData::Float64(v) => {
                add(b"f64");
                for x in v {
                    add(&x.to_le_bytes());
                }
            }
            ColumnData::Int32(v) => {
                add(b"i32");
                for x in v {
                    add(&x.to_le_bytes());
                }
            }
            ColumnData::UInt32(v) => {
                add(b"u32");
                for x in v {
                    add(&x.to_le_bytes());
                }
            }
            ColumnData::Text(v) => {
                for x in v {
                    add(x.as_bytes());
                    add(&[0]);
                }
            }
        }
    }
    format!("{hash:016x}")
}

pub fn is_original_parent(mesh: &SurfaceMesh) -> bool {
    mesh.metadata.parent_mapping.as_ref().is_none_or(|mapping| {
        mapping.parent.vertex_count == Some(mesh.vertices.len())
            && mapping.indices.len() == mesh.vertices.len()
            && mapping
                .indices
                .iter()
                .enumerate()
                .all(|(i, &id)| i == id as usize)
    })
}
