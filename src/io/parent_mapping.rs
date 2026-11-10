//! MNE parent provenance is separate from GIFTI's local geometry and sparse
//! NODE_INDEX addressing. Read all copies so conflicting array metadata cannot
//! silently override file-level metadata.

use super::*;
use crate::surface_align::{ParentDomainIdentity, ParentVertexMapping};

pub fn read_gifti_parent_mapping(path: impl AsRef<Path>) -> Result<Option<ParentVertexMapping>> {
    let image = read_gifti_image(path)?;
    gifti_parent_mapping(&image)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GiftiParentMetadata {
    pub identity: Option<ParentDomainIdentity>,
    pub mapping: Option<ParentVertexMapping>,
}

/// Inspect correspondence before binding a dataset to any display domain.
pub fn gifti_parent_mapping(image: &GiftiImage) -> Result<Option<ParentVertexMapping>> {
    Ok(gifti_parent_metadata(image)?.mapping)
}

pub fn gifti_parent_metadata(image: &GiftiImage) -> Result<GiftiParentMetadata> {
    let metas: Vec<&Meta> = std::iter::once(&image.meta)
        .chain(image.data_arrays.iter().map(|array| &array.meta))
        .collect();
    let ids = consistent(&metas, "MNEParentVertexIndices", |value| {
        value
            .split_whitespace()
            .map(|id| {
                id.parse::<u32>().with_context(|| {
                    format!("invalid parent vertex ID {id:?}; expected a nonnegative integer")
                })
            })
            .collect::<Result<Vec<_>>>()
    })?;
    let vertex_count = consistent(&metas, "MNEParentVertexCount", |value| {
        let count = value
            .parse::<usize>()
            .context("invalid MNEParentVertexCount")?;
        ensure!(count > 0, "MNEParentVertexCount must be positive");
        Ok(count)
    })?;
    let hemisphere = consistent(&metas, "MNEParentHemisphere", |value| {
        match value.to_ascii_lowercase().as_str() {
            "lh" => Ok(SurfaceSide::Left),
            "rh" => Ok(SurfaceSide::Right),
            _ => bail!("MNEParentHemisphere must be lh or rh, got {value:?}"),
        }
    })?;
    let subject = consistent(&metas, "MNEParentSubject", |value| {
        ensure!(
            !value.is_empty(),
            "MNEParentSubject is empty; omit unknown subjects"
        );
        Ok(value.to_owned())
    })?;
    let identity = ParentDomainIdentity {
        vertex_count,
        hemisphere,
        subject,
        vertex_order_fingerprint: None,
    };
    let Some(indices) = ids else {
        if identity == ParentDomainIdentity::default() {
            return Ok(GiftiParentMetadata::default());
        }
        // Full parent geometry may declare its own identity without a redundant
        // 0..N index list. Do not infer this for sparse data or child geometry.
        let rows = mapping_row_count(image)?;
        ensure!(
            identity.vertex_count == Some(rows),
            "MNE parent metadata lacks MNEParentVertexIndices and does not describe a complete parent"
        );
        ensure!(
            image
                .data_arrays
                .iter()
                .any(|a| a.intent == gifti_rs::intent::POINTSET && a.dims.len() == 2),
            "MNE parent metadata without indices requires full parent geometry"
        );
        for array in &image.data_arrays {
            if let Some(n) = parent_rows(array)? {
                ensure!(
                    n == rows,
                    "full parent metadata has inconsistent row counts"
                );
            }
        }
        return Ok(GiftiParentMetadata {
            identity: Some(identity),
            mapping: None,
        });
    };
    let mapping = ParentVertexMapping::new(identity.clone(), indices, mapping_row_count(image)?)?;
    // Check every data/coordinate array, not only the first matching column.
    for array in &image.data_arrays {
        if let Some(rows) = parent_rows(array)? {
            mapping.validate(rows)?;
        }
    }
    Ok(GiftiParentMetadata {
        identity: Some(identity),
        mapping: Some(mapping),
    })
}

fn mapping_row_count(image: &GiftiImage) -> Result<usize> {
    for array in &image.data_arrays {
        if let Some(rows) = parent_rows(array)? {
            return Ok(rows);
        }
    }
    bail!("parent mapping has no geometry or data rows")
}

fn parent_rows(array: &DataArray) -> Result<Option<usize>> {
    if array.intent == gifti_rs::intent::TRIANGLE {
        return Ok(None);
    }
    if array.intent == gifti_rs::intent::POINTSET && array.dims.len() == 2 {
        ensure!(
            array.dims[1] == 3 && array.data.len() == array.dims[0] * 3,
            "parent-mapped geometry must contain N x 3 coordinates"
        );
        return Ok(Some(array.dims[0]));
    }
    ensure!(
        array.dims.len() == 1 && array.data.len() == array.dims[0],
        "parent-mapped data must contain one-dimensional arrays matching the parent IDs"
    );
    Ok(Some(array.data.len()))
}

fn consistent<T: PartialEq>(
    metas: &[&Meta],
    key: &str,
    parse: impl Fn(&str) -> Result<T>,
) -> Result<Option<T>> {
    let mut result = None;
    for meta in metas {
        for (name, value) in *meta {
            if name.eq_ignore_ascii_case(key) {
                let parsed = parse(value.trim()).with_context(|| format!("invalid {key}"))?;
                if let Some(existing) = &result {
                    ensure!(*existing == parsed, "conflicting {key} metadata copies");
                } else {
                    result = Some(parsed);
                }
            }
        }
    }
    Ok(result)
}
