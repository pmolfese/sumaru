//! Resolve GIFTI rows onto a child or parent before display-domain attachment.
//! No coordinate matching or interpolation is performed here.

use super::*;
use crate::surface::SurfaceMesh;
use crate::surface_align::{ParentDomainIdentity, ParentMismatchPolicy, ParentVertexMapping};

#[derive(Debug, Clone, Default)]
pub struct SurfaceDatasetOptions {
    pub companion_surfaces: Vec<std::path::PathBuf>,
    pub parent_policy: ParentMismatchPolicy,
    pub auto_qcalc: bool,
}

pub fn read_gifti_dataset_for_surface(
    path: impl AsRef<Path>,
    target: &SurfaceMesh,
    options: &SurfaceDatasetOptions,
) -> Result<Dataset> {
    let path = path.as_ref();
    let image = read_gifti_image(path)?;
    let mut mapping = gifti_parent_mapping(&image)?;
    let rows = image
        .data_arrays
        .iter()
        .find(|a| {
            a.intent != gifti_rs::intent::NODE_INDEX
                && a.intent != gifti_rs::intent::TRIANGLE
                && !(a.intent == gifti_rs::intent::POINTSET && a.dims.len() == 2)
        })
        .context("GIFTI dataset has no data arrays")?
        .data
        .len();
    let has_indices = image
        .data_arrays
        .iter()
        .any(|a| a.intent == gifti_rs::intent::NODE_INDEX);
    let expected_side = mapping
        .as_ref()
        .and_then(|m| m.parent.hemisphere.as_ref())
        .or(target
            .metadata
            .parent_identity
            .as_ref()
            .and_then(|p| p.hemisphere.as_ref()))
        .unwrap_or(&target.metadata.side);
    let mut companion = None;
    for candidate in &options.companion_surfaces {
        let mesh = SurfaceMesh::from_path(candidate)?;
        let side = mesh
            .metadata
            .parent_identity
            .as_ref()
            .and_then(|p| p.hemisphere.as_ref())
            .unwrap_or(&mesh.metadata.side);
        if options.companion_surfaces.len() == 1 || side == expected_side {
            ensure!(
                companion.is_none(),
                "multiple companion surfaces match this hemisphere"
            );
            companion = Some(mesh);
        }
    }
    ensure!(
        options.companion_surfaces.is_empty() || companion.is_some(),
        "no explicit companion geometry matches this hemisphere"
    );
    // Legacy direct loads and self-contained exports do not depend on a sibling.
    let full_parent_rows = target
        .metadata
        .parent_mapping
        .as_ref()
        .is_some_and(|child| child.parent.vertex_count == Some(rows));
    if companion.is_none()
        && mapping.is_none()
        && !has_indices
        && rows != target.vertices.len()
        && !full_parent_rows
    {
        if let Some(name) = path.file_name().and_then(|s| s.to_str())
            && let Some(stem) = name.strip_suffix(".time.gii")
        {
            let candidate = path.with_file_name(format!("{stem}.gii"));
            if candidate.exists() {
                companion = Some(SurfaceMesh::from_path(candidate)?);
            }
        }
    }
    if let Some(companion) = companion {
        ensure!(
            companion.vertices.len() == rows,
            "companion geometry has {} vertices but data has {rows} rows",
            companion.vertices.len()
        );
        let companion_mapping = if let Some(mapping) = companion.metadata.parent_mapping {
            mapping
        } else if let Some(identity) = companion.metadata.parent_identity {
            ensure!(
                identity.vertex_count == Some(rows),
                "companion has incomplete parent identity"
            );
            ParentVertexMapping::new(identity, (0..u32::try_from(rows)?).collect(), rows)?
        } else {
            bail!("companion geometry has no parent vertex mapping or complete parent identity");
        };
        if let Some(existing) = mapping.as_mut() {
            ensure!(
                existing.indices == companion_mapping.indices,
                "time series and companion geometry have conflicting parent vertex IDs"
            );
            merge_identity(&mut existing.parent, &companion_mapping.parent)?;
        } else {
            mapping = Some(companion_mapping);
        }
    }

    // Preserve legacy dense/sparse behavior when no change of domain is needed.
    if mapping.is_none()
        && (target.metadata.parent_mapping.is_none()
            || (!has_indices && rows == target.vertices.len()))
    {
        let mut dataset = gifti_image_to_dataset(&image, &target.domain, path, options.auto_qcalc)?;
        apply_time_metadata(&image, &mut dataset)?;
        return Ok(dataset);
    }

    // Parse in the source's own row/index domain, never the target's node count.
    let index_max = image
        .data_arrays
        .iter()
        .filter(|a| a.intent == gifti_rs::intent::NODE_INDEX)
        .map(column_data_from_gifti_array)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|values| match values {
            ColumnData::UInt32(v) => Ok(v.into_iter().max().unwrap_or(0) as usize + 1),
            ColumnData::Int32(v) => {
                ensure!(v.iter().all(|i| *i >= 0), "negative NODE_INDEX");
                Ok(v.into_iter().max().unwrap_or(0) as usize + 1)
            }
            _ => bail!("NODE_INDEX must contain integers"),
        })
        .collect::<Result<Vec<usize>>>()?
        .into_iter()
        .max();
    let source_domain = SurfaceDomain::unbound_data(index_max.unwrap_or(rows))?;
    let mut dataset = gifti_image_to_dataset(&image, &source_domain, path, options.auto_qcalc)?;
    ensure!(
        dataset.columns.len()
            == image
                .data_arrays
                .iter()
                .filter(|a| a.intent != gifti_rs::intent::NODE_INDEX
                    && a.intent != gifti_rs::intent::TRIANGLE
                    && !(a.intent == gifti_rs::intent::POINTSET && a.dims.len() == 2))
                .count(),
        "data arrays have inconsistent row counts"
    );
    apply_time_metadata(&image, &mut dataset)?;
    if mapping.is_none() {
        let target_parent = target.metadata.parent_mapping.as_ref().unwrap();
        let ids = if let Some(ids) = &dataset.node_indices {
            ids.clone()
        } else {
            ensure!(
                target_parent.parent.vertex_count == Some(rows),
                "data does not match child geometry or declared full parent; supply --companion-surface"
            );
            (0..u32::try_from(rows).context("too many source rows")?).collect()
        };
        mapping = Some(ParentVertexMapping::new(
            ParentDomainIdentity {
                vertex_count: if has_indices { None } else { Some(rows) },
                ..Default::default()
            },
            ids,
            rows,
        )?);
    }
    let mapping = mapping.unwrap();
    mapping.validate(dataset.row_count)?;
    if let Some(indices) = &dataset.node_indices {
        let local_order = indices
            .iter()
            .enumerate()
            .all(|(row, id)| row == *id as usize);
        ensure!(
            local_order || *indices == mapping.indices,
            "NODE_INDEX and parent metadata specify conflicting row mappings"
        );
    }
    resolve_parent_dataset(dataset, mapping, target, options.parent_policy)
}

fn merge_identity(a: &mut ParentDomainIdentity, b: &ParentDomainIdentity) -> Result<()> {
    macro_rules! merge {
        ($field:ident) => {
            if let (Some(x), Some(y)) = (&a.$field, &b.$field) {
                ensure!(
                    x == y,
                    "time series and companion geometry have conflicting {}",
                    stringify!($field)
                );
            } else if a.$field.is_none() {
                a.$field = b.$field.clone();
            }
        };
    }
    merge!(vertex_count);
    merge!(hemisphere);
    merge!(subject);
    merge!(vertex_order_fingerprint);
    Ok(())
}

pub fn resolve_parent_dataset(
    source: Dataset,
    mapping: ParentVertexMapping,
    target: &SurfaceMesh,
    policy: ParentMismatchPolicy,
) -> Result<Dataset> {
    mapping.validate(source.row_count)?;
    let (identity, parent_count) = if let Some(child) = &target.metadata.parent_mapping {
        child.validate(target.vertices.len())?;
        let count = child
            .parent
            .vertex_count
            .or(mapping.parent.vertex_count)
            .unwrap_or_else(|| {
                child
                    .indices
                    .iter()
                    .chain(&mapping.indices)
                    .copied()
                    .max()
                    .unwrap_or(0) as usize
                    + 1
            });
        (child.parent.clone(), count)
    } else {
        (
            target
                .metadata
                .parent_identity
                .clone()
                .unwrap_or_else(|| ParentDomainIdentity {
                    vertex_count: Some(target.vertices.len()),
                    hemisphere: match target.metadata.side {
                        SurfaceSide::Left | SurfaceSide::Right => {
                            Some(target.metadata.side.clone())
                        }
                        _ => None,
                    },
                    subject: target.metadata.subject_label.clone(),
                    ..Default::default()
                }),
            target.vertices.len(),
        )
    };
    let assessment = mapping.assess_parent(&identity, parent_count, policy)?;
    let mut source_rows = Vec::new();
    let mut target_ids = Vec::new();
    if let Some(child) = &target.metadata.parent_mapping {
        let inverse: std::collections::HashMap<_, _> = child
            .indices
            .iter()
            .enumerate()
            .map(|(local, parent)| (*parent, local as u32))
            .collect();
        for (row, id) in mapping.indices.iter().enumerate() {
            if let Some(local) = inverse.get(id) {
                source_rows.push(row);
                target_ids.push(*local);
            }
        }
    } else {
        source_rows.extend(0..source.row_count);
        target_ids.clone_from(&mapping.indices);
    }
    ensure!(
        !source_rows.is_empty(),
        "source and target child have no shared parent vertices; reconstruction is required"
    );
    let columns = source
        .columns
        .iter()
        .map(|column| {
            let mut selected = column.clone();
            macro_rules! select {
                ($values:expr, $variant:ident) => {
                    ColumnData::$variant(source_rows.iter().map(|i| $values[*i].clone()).collect())
                };
            }
            selected.values = match &column.values {
                ColumnData::UInt32(v) => select!(v, UInt32),
                ColumnData::Int32(v) => select!(v, Int32),
                ColumnData::Float32(v) => select!(v, Float32),
                ColumnData::Float64(v) => select!(v, Float64),
                ColumnData::Text(v) => select!(v, Text),
            };
            selected.range = selected.values.range();
            Ok(selected)
        })
        .collect::<Result<Vec<_>>>()?;
    let dense = target_ids.len() == target.vertices.len()
        && target_ids
            .iter()
            .enumerate()
            .all(|(i, id)| i == *id as usize);
    let mut result = if dense {
        Dataset::dense(source.kind, &target.domain, columns)?
    } else {
        Dataset::sparse(source.kind, &target.domain, target_ids, columns)?
    };
    result.parent_mapping = Some(ParentVertexMapping::new(
        mapping.parent,
        source_rows.iter().map(|i| mapping.indices[*i]).collect(),
        source_rows.len(),
    )?);
    result.time_start_seconds = source.time_start_seconds;
    result.time_step_seconds = source.time_step_seconds;
    result.parent_ids = source.parent_ids;
    result.parent_ids.domain_parent_id = Some(target.domain.id.as_str().to_owned());
    result.mapping_warnings = assessment.warnings;
    if result.row_count < source.row_count {
        result.mapping_warnings.push(format!("{} of {} target vertices have samples; others remain missing before any display interpolation", result.row_count, target.vertices.len()));
    }
    Ok(result)
}

fn apply_time_metadata(image: &GiftiImage, dataset: &mut Dataset) -> Result<()> {
    let arrays: Vec<_> = image
        .data_arrays
        .iter()
        .filter(|a| {
            a.intent != gifti_rs::intent::NODE_INDEX
                && a.intent != gifti_rs::intent::TRIANGLE
                && !(a.intent == gifti_rs::intent::POINTSET && a.dims.len() == 2)
        })
        .collect();
    if let Some(count) = gifti_meta_value(&image.meta, "TimeSamples") {
        ensure!(
            count.parse::<usize>().context("invalid TimeSamples")? == arrays.len(),
            "TimeSamples does not match data array count"
        );
        ensure!(
            dataset.columns.len() == arrays.len(),
            "time-series arrays have inconsistent row counts"
        );
    }
    let units = gifti_meta_value(&image.meta, "TimeUnits");
    let scale = match units.as_deref() {
        None | Some("s") => 1.,
        Some("ms") => 0.001,
        Some(other) => bail!("unsupported TimeUnits {other:?}"),
    };
    for (key, slot) in [
        ("TimeStart", &mut dataset.time_start_seconds),
        ("TimeStep", &mut dataset.time_step_seconds),
    ] {
        if let Some(value) = gifti_meta_value(&image.meta, key) {
            let value = value
                .parse::<f64>()
                .with_context(|| format!("invalid {key}"))?
                * scale;
            ensure!(
                value.is_finite() && (key != "TimeStep" || value > 0.),
                "invalid {key}"
            );
            *slot = Some(value);
        }
    }
    for (i, array) in arrays.iter().enumerate() {
        if let Some(index) = gifti_meta_value(&array.meta, "TimeIndex") {
            ensure!(
                index.parse::<usize>().context("invalid TimeIndex")? == i,
                "TimeIndex does not match array order"
            );
        }
        if let Some(time) = gifti_meta_value(&array.meta, "TimeValue") {
            let array_units = gifti_meta_value(&array.meta, "TimeUnits");
            let factor = match array_units.as_deref().or(units.as_deref()) {
                None | Some("s") => 1.,
                Some("ms") => 0.001,
                _ => bail!("unsupported per-array TimeUnits"),
            };
            let time = time.parse::<f64>().context("invalid TimeValue")? * factor;
            ensure!(time.is_finite(), "nonfinite TimeValue");
            if let (Some(start), Some(step)) =
                (dataset.time_start_seconds, dataset.time_step_seconds)
            {
                ensure!(
                    (time - (start + i as f64 * step)).abs() <= 1e-9_f64.max(step.abs() * 1e-6),
                    "TimeValue disagrees with TimeStart/TimeStep"
                );
            }
        }
    }
    for (column, array) in dataset.columns.iter_mut().zip(arrays) {
        column.units = gifti_meta_value(&array.meta, "Units")
            .or_else(|| gifti_meta_value(&image.meta, "Units"))
            .or_else(|| gifti_meta_value(&image.meta, "DataUnits"));
    }
    Ok(())
}
