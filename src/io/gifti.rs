//! GIFTI surface-dataset loading: turning a `gifti-rs` image into sumaru's
//! domain `Dataset`, including intent-name normalization and the
//! NIFTI-intent -> column-role / stat metadata mapping.

use super::*;

pub(crate) const NIFTI_INTENT_CORREL: i32 = 2;

pub(crate) const NIFTI_INTENT_TTEST: i32 = 3;

pub(crate) const NIFTI_INTENT_FTEST: i32 = 4;

pub(crate) const NIFTI_INTENT_ZSCORE: i32 = 5;

pub(crate) const NIFTI_INTENT_CHISQ: i32 = 6;

pub fn read_gifti_dataset(path: impl AsRef<Path>, domain: &SurfaceDomain) -> Result<Dataset> {
    let path = path.as_ref();
    let image = read_gifti_image(path)
        .with_context(|| format!("failed to read GIFTI dataset {}", path.display()))?;
    gifti_image_to_dataset(&image, domain, path)
}

pub fn read_gifti_image(path: impl AsRef<Path>) -> Result<GiftiImage> {
    read_gifti_compat(path.as_ref())
}

pub(crate) fn read_gifti_compat(path: &Path) -> Result<GiftiImage> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read GIFTI file {}", path.display()))?;
    let xml = std::str::from_utf8(&bytes)
        .with_context(|| format!("GIFTI file {} is not valid UTF-8", path.display()))?;
    let normalized = normalize_gifti_intent_names(xml);

    gifti_rs::parse_str(&normalized)
        .with_context(|| format!("failed to parse GIFTI file {}", path.display()))
}

pub(crate) fn normalize_gifti_intent_names(xml: &str) -> String {
    let mut normalized = xml.to_string();
    for (name, code) in [
        ("NIFTI_INTENT_CORREL", NIFTI_INTENT_CORREL),
        ("NIFTI_INTENT_TTEST", NIFTI_INTENT_TTEST),
        ("NIFTI_INTENT_FTEST", NIFTI_INTENT_FTEST),
        ("NIFTI_INTENT_ZSCORE", NIFTI_INTENT_ZSCORE),
        ("NIFTI_INTENT_CHISQ", NIFTI_INTENT_CHISQ),
    ] {
        normalized =
            normalized.replace(&format!("Intent=\"{name}\""), &format!("Intent=\"{code}\""));
        normalized = normalized.replace(&format!("Intent='{name}'"), &format!("Intent='{code}'"));
    }
    normalized
}

pub(crate) fn gifti_image_to_dataset(
    image: &GiftiImage,
    domain: &SurfaceDomain,
    path: &Path,
) -> Result<Dataset> {
    let columns = image
        .data_arrays
        .iter()
        .enumerate()
        .filter(|(_, array)| gifti_array_is_dataset_column(array, domain.node_count))
        .map(|(index, array)| gifti_array_to_data_column(array, index))
        .collect::<Result<Vec<_>>>()?;

    ensure!(
        !columns.is_empty(),
        "GIFTI dataset has no scalar data arrays matching {} surface nodes",
        domain.node_count
    );

    let parent_ids = DatasetParentIds {
        source_dataset_id: gifti_meta_value(&image.meta, "UniqueID"),
        domain_parent_id: Some(domain.id.as_str().to_string()),
        surface_parent_id: None,
        volume_parent_id: None,
        originator_id: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
    };

    let kind = if columns
        .iter()
        .all(|column| column.role == ColumnRole::TimePoint)
    {
        DatasetKind::SurfaceTimeSeries
    } else {
        DatasetKind::SurfaceScalar
    };

    Dataset::dense(kind, domain, columns).map(|dataset| dataset.with_parent_ids(parent_ids))
}

pub(crate) fn gifti_array_to_data_column(array: &DataArray, index: usize) -> Result<DataColumn> {
    let label = gifti_meta_value(&array.meta, "Name").unwrap_or_else(|| format!("col_{index}"));
    let role = column_role_from_gifti_array(array);
    let stat = gifti_stat_from_array(array);

    DataColumn::new(label, role, None, column_data_from_gifti_array(array)?)
        .map(|column| column.with_stat(stat))
}

fn gifti_array_is_dataset_column(array: &DataArray, node_count: usize) -> bool {
    array.intent != gifti_rs::intent::TRIANGLE
        && !gifti_array_is_surface_pointset(array)
        && array.data.len() == node_count
}

pub(crate) fn gifti_array_is_surface_pointset(array: &DataArray) -> bool {
    array.intent == gifti_rs::intent::POINTSET && array.dims.len() == 2 && array.dims[1] == 3
}

pub(crate) fn column_data_from_gifti_array(array: &DataArray) -> Result<ColumnData> {
    match &array.data {
        ArrayData::UInt8(values) => Ok(ColumnData::UInt32(
            values.iter().map(|value| u32::from(*value)).collect(),
        )),
        ArrayData::Int8(values) => Ok(ColumnData::Int32(
            values.iter().map(|value| i32::from(*value)).collect(),
        )),
        ArrayData::UInt16(values) => Ok(ColumnData::UInt32(
            values.iter().map(|value| u32::from(*value)).collect(),
        )),
        ArrayData::Int16(values) => Ok(ColumnData::Int32(
            values.iter().map(|value| i32::from(*value)).collect(),
        )),
        ArrayData::UInt32(values) => Ok(ColumnData::UInt32(values.clone())),
        ArrayData::Int32(values) => Ok(ColumnData::Int32(values.clone())),
        ArrayData::UInt64(values) => Ok(ColumnData::UInt32(
            values
                .iter()
                .map(|value| {
                    u32::try_from(*value).with_context(|| {
                        format!("GIFTI UInt64 value {value} does not fit in Dataset UInt32")
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        ArrayData::Int64(values) => Ok(ColumnData::Int32(
            values
                .iter()
                .map(|value| {
                    i32::try_from(*value).with_context(|| {
                        format!("GIFTI Int64 value {value} does not fit in Dataset Int32")
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        ArrayData::Float32(values) => Ok(ColumnData::Float32(values.clone())),
        ArrayData::Float64(values) => Ok(ColumnData::Float64(values.clone())),
    }
}

pub(crate) fn column_role_from_gifti_intent(intent: i32) -> ColumnRole {
    match intent {
        NIFTI_INTENT_TTEST | NIFTI_INTENT_FTEST | NIFTI_INTENT_ZSCORE | NIFTI_INTENT_CHISQ
        | NIFTI_INTENT_CORREL => ColumnRole::Statistic,
        gifti_rs::intent::LABEL => ColumnRole::Label,
        gifti_rs::intent::TIME_SERIES => ColumnRole::TimePoint,
        gifti_rs::intent::NODE_INDEX => ColumnRole::NodeIndex,
        gifti_rs::intent::SHAPE | gifti_rs::intent::NONE => ColumnRole::Intensity,
        _ => ColumnRole::Unknown,
    }
}

pub(crate) fn column_role_from_gifti_array(array: &DataArray) -> ColumnRole {
    if array.intent == gifti_rs::intent::POINTSET && !gifti_array_is_surface_pointset(array) {
        ColumnRole::TimePoint
    } else {
        column_role_from_gifti_intent(array.intent)
    }
}

pub(crate) fn gifti_stat_from_array(array: &DataArray) -> Option<String> {
    let params = ["intent_p1", "intent_p2", "intent_p3"]
        .iter()
        .filter_map(|key| gifti_meta_value(&array.meta, key))
        .filter_map(|value| value.parse::<f64>().ok())
        .map(format_stat_parameter)
        .collect::<Vec<_>>();

    match array.intent {
        NIFTI_INTENT_TTEST => params
            .first()
            .map(|df| format!("Ttest({df})"))
            .or_else(|| Some("Ttest".to_string())),
        NIFTI_INTENT_FTEST => {
            if params.len() >= 2 {
                Some(format!("Ftest({},{})", params[0], params[1]))
            } else {
                Some("Ftest".to_string())
            }
        }
        NIFTI_INTENT_ZSCORE => Some("Zscore".to_string()),
        NIFTI_INTENT_CHISQ => params
            .first()
            .map(|df| format!("ChiSq({df})"))
            .or_else(|| Some("ChiSq".to_string())),
        NIFTI_INTENT_CORREL => params
            .first()
            .map(|df| format!("Correlation({df})"))
            .or_else(|| Some("Correlation".to_string())),
        _ => None,
    }
}

pub(crate) fn format_stat_parameter(value: f64) -> String {
    if value.fract().abs() < 1.0e-9 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

pub(crate) fn gifti_meta_value(meta: &Meta, key: &str) -> Option<String> {
    meta.iter().find_map(|(name, value)| {
        name.eq_ignore_ascii_case(key)
            .then(|| value.trim())
            .and_then(|trimmed| {
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gifti_rs::{ArrayIndexOrder, DataType, Encoding, Endian};

    fn triangle_domain() -> SurfaceDomain {
        SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap()
    }

    fn float_array(intent: i32, dims: Vec<usize>, values: Vec<f32>) -> DataArray {
        DataArray {
            intent,
            datatype: DataType::Float32 as i32,
            array_index_order: ArrayIndexOrder::RowMajor,
            dims,
            encoding: Encoding::Ascii,
            endian: Endian::Little,
            ext_filename: None,
            ext_offset: None,
            coordsys: Vec::new(),
            meta: Vec::new(),
            data: ArrayData::Float32(values),
        }
    }

    fn int_array(intent: i32, dims: Vec<usize>, values: Vec<i32>) -> DataArray {
        DataArray {
            intent,
            datatype: DataType::Int32 as i32,
            array_index_order: ArrayIndexOrder::RowMajor,
            dims,
            encoding: Encoding::Ascii,
            endian: Endian::Little,
            ext_filename: None,
            ext_offset: None,
            coordsys: Vec::new(),
            meta: Vec::new(),
            data: ArrayData::Int32(values),
        }
    }

    #[test]
    fn scalar_pointset_arrays_load_as_time_series_columns() {
        let image = GiftiImage {
            version: "1.0".to_string(),
            num_data_arrays: 3,
            meta: Vec::new(),
            label_table: None,
            data_arrays: vec![
                float_array(gifti_rs::intent::POINTSET, vec![3], vec![1.0, 2.0, 3.0]),
                float_array(gifti_rs::intent::POINTSET, vec![3], vec![4.0, 5.0, 6.0]),
                int_array(gifti_rs::intent::TRIANGLE, vec![1, 3], vec![0, 1, 2]),
            ],
        };

        let dataset = gifti_image_to_dataset(&image, &triangle_domain(), Path::new("time.gii"))
            .expect("scalar pointsets should load as overlay columns");

        assert_eq!(dataset.kind, DatasetKind::SurfaceTimeSeries);
        assert_eq!(dataset.columns.len(), 2);
        assert!(
            dataset
                .columns
                .iter()
                .all(|column| column.role == ColumnRole::TimePoint)
        );
    }

    #[test]
    fn geometry_pointsets_are_not_loaded_as_dataset_columns() {
        let image = GiftiImage {
            version: "1.0".to_string(),
            num_data_arrays: 2,
            meta: Vec::new(),
            label_table: None,
            data_arrays: vec![
                float_array(
                    gifti_rs::intent::POINTSET,
                    vec![3, 3],
                    vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                ),
                int_array(gifti_rs::intent::TRIANGLE, vec![1, 3], vec![0, 1, 2]),
            ],
        };

        let error = gifti_image_to_dataset(&image, &triangle_domain(), Path::new("surface.gii"))
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("no scalar data arrays matching 3 surface nodes")
        );
    }
}
