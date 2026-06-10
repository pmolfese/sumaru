use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use gifti_rs::{ArrayData, DataArray, GiftiImage, Meta};

use crate::color::Rgba;
use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind, DatasetParentIds};
use crate::roi::{Roi, RoiBrushAction, RoiDatum, RoiDrawingType, RoiElementKind, RoiSource};
use crate::surface::{SurfaceDomain, SurfaceSide};

const NIFTI_INTENT_CORREL: i32 = 2;
const NIFTI_INTENT_TTEST: i32 = 3;
const NIFTI_INTENT_FTEST: i32 = 4;
const NIFTI_INTENT_ZSCORE: i32 = 5;
const NIFTI_INTENT_CHISQ: i32 = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct NimlElement {
    pub name: String,
    pub attrs: BTreeMap<String, String>,
    pub data: NimlData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NimlData {
    None,
    Text(String),
    Numeric(NimlNumericMatrix),
    Mixed(NimlMixedTable),
    RoiDatums(Vec<NimlRoiDatumRecord>),
    Group(Vec<NimlElement>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NimlNumericMatrix {
    pub column_types: Vec<NimlValueType>,
    pub rows: usize,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NimlMixedTable {
    pub column_types: Vec<NimlValueType>,
    pub rows: usize,
    pub values: Vec<NimlValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NimlValue {
    Integer(i64),
    Float(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NimlValueType {
    UInt8,
    Int16,
    Int32,
    Float32,
    Float64,
    String,
    CString,
    SumaRoiDatum,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NimlRoiDatumRecord {
    pub action_code: i32,
    pub element_type_code: i32,
    pub node_path: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NimlDatasetPayload {
    pub dset_type: String,
    pub self_idcode: Option<String>,
    pub filename: Option<String>,
    pub label: Option<String>,
    pub sparse_data: Option<NimlNumericMatrix>,
    pub node_indices: Option<Vec<u32>>,
    pub column_ranges: Vec<String>,
    pub column_labels: Vec<String>,
    pub column_types: Vec<String>,
    pub column_stats: Vec<String>,
    pub history: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NimlRoiPayload {
    pub self_idcode: Option<String>,
    pub domain_parent_idcode: Option<String>,
    pub parent_side: SurfaceSide,
    pub label: String,
    pub integer_label: i32,
    pub roi_type_code: Option<i32>,
    pub drawing_type: RoiDrawingType,
    pub color_plane_name: Option<String>,
    pub fill_color: Option<Rgba>,
    pub edge_color: Option<Rgba>,
    pub edge_thickness: Option<u32>,
    pub records: Vec<NimlRoiDatumRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NimlReferenceCheck {
    pub name: &'static str,
    pub references: &'static [&'static str],
    pub passed: bool,
    pub detail: String,
}

pub fn read_niml(path: impl AsRef<Path>) -> Result<Vec<NimlElement>> {
    let path = path.as_ref();
    let bytes =
        fs::read(path).with_context(|| format!("failed to read NIML file {}", path.display()))?;
    parse_niml_bytes(&bytes)
}

pub fn parse_niml_str(text: &str) -> Result<Vec<NimlElement>> {
    let cleaned = strip_niml_comment_prefixes(text);
    parse_niml_bytes(cleaned.as_bytes())
}

pub fn parse_niml_bytes(bytes: &[u8]) -> Result<Vec<NimlElement>> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        if text
            .lines()
            .any(|line| line.trim_start().starts_with("# <Node_ROI"))
        {
            let cleaned = strip_niml_comment_prefixes(text);
            let mut parser = NimlByteParser::new(cleaned.as_bytes());
            return parser.parse();
        }
    }

    let mut parser = NimlByteParser::new(bytes);
    parser.parse()
}

pub fn write_niml_ascii(path: impl AsRef<Path>, elements: &[NimlElement]) -> Result<()> {
    let path = path.as_ref();
    fs::write(path, serialize_niml_ascii(elements))
        .with_context(|| format!("failed to write NIML file {}", path.display()))
}

pub fn serialize_niml_ascii(elements: &[NimlElement]) -> String {
    let mut out = String::new();
    for element in elements {
        serialize_element(element, &mut out);
    }
    out
}

pub fn expand_niml_type(ni_type: &str) -> Result<Vec<NimlValueType>> {
    let mut types = Vec::new();

    for piece in ni_type
        .split(',')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
    {
        let (count, type_name) = match piece.split_once('*') {
            Some((count, type_name))
                if count.trim().chars().all(|value| value.is_ascii_digit()) =>
            {
                let count = count
                    .trim()
                    .parse::<usize>()
                    .with_context(|| format!("invalid NIML type repeat count in {piece:?}"))?;
                ensure!(count > 0, "NIML type repeat count must be positive");
                (count, type_name.trim())
            }
            _ => (1, piece),
        };
        let value_type = NimlValueType::from_name(type_name);
        types.extend(std::iter::repeat_n(value_type, count));
    }

    ensure!(!types.is_empty(), "NIML ni_type is empty");
    Ok(types)
}

pub fn read_niml_dset_str(text: &str) -> Result<NimlDatasetPayload> {
    let elements = parse_niml_str(text)?;
    ensure!(
        elements.len() == 1,
        "expected exactly one top-level NIML dataset element"
    );
    NimlDatasetPayload::from_element(&elements[0])
}

pub fn read_niml_dset(path: impl AsRef<Path>) -> Result<NimlDatasetPayload> {
    let elements = read_niml(path)?;
    ensure!(
        elements.len() == 1,
        "expected exactly one top-level NIML dataset element"
    );
    NimlDatasetPayload::from_element(&elements[0])
}

pub fn read_niml_dataset(path: impl AsRef<Path>, domain: &SurfaceDomain) -> Result<Dataset> {
    read_niml_dset(path)?.to_dataset(domain)
}

pub fn read_gifti_dataset(path: impl AsRef<Path>, domain: &SurfaceDomain) -> Result<Dataset> {
    let path = path.as_ref();
    let image = read_gifti_image(path)
        .with_context(|| format!("failed to read GIFTI dataset {}", path.display()))?;
    gifti_image_to_dataset(&image, domain, path)
}

pub fn read_gifti_image(path: impl AsRef<Path>) -> Result<GiftiImage> {
    read_gifti_compat(path.as_ref())
}

pub fn read_niml_roi_str(text: &str) -> Result<Vec<NimlRoiPayload>> {
    parse_niml_str(text)?
        .iter()
        .filter(|element| element.name == "Node_ROI")
        .map(NimlRoiPayload::from_element)
        .collect()
}

pub fn read_niml_roi(path: impl AsRef<Path>) -> Result<Vec<NimlRoiPayload>> {
    read_niml(path)?
        .iter()
        .filter(|element| element.name == "Node_ROI")
        .map(NimlRoiPayload::from_element)
        .collect()
}

pub fn write_niml_roi(path: impl AsRef<Path>, rois: &[Roi]) -> Result<()> {
    let elements = rois
        .iter()
        .map(|roi| NimlRoiPayload::from_roi(roi).to_element())
        .collect::<Vec<_>>();
    write_niml_ascii(path, &elements)
}

pub fn niml_reference_checks() -> Vec<NimlReferenceCheck> {
    let mut checks = Vec::new();

    let type_check = expand_niml_type("int,3*float,String").map(|types| {
        types
            == vec![
                NimlValueType::Int32,
                NimlValueType::Float32,
                NimlValueType::Float32,
                NimlValueType::Float32,
                NimlValueType::String,
            ]
    });
    checks.push(NimlReferenceCheck {
        name: "ni_type repeat expansion",
        references: &[
            "SUMAvista/src/pysuma/niml.py:_expand_ni_type",
            "afni/src/matlab/afni_niml_parse.m:afni_nel_getvectype",
        ],
        passed: type_check.unwrap_or(false),
        detail: "Supports comma-separated row types and N*type repeat syntax.".to_string(),
    });

    let dset_check = read_niml_dset_str(MINIMAL_DSET_SAMPLE).map(|payload| {
        payload
            .sparse_data
            .as_ref()
            .is_some_and(|data| data.rows == 2)
            && payload.node_indices == Some(vec![10, 12])
            && payload.column_labels == vec!["effect".to_string(), "stat".to_string()]
    });
    checks.push(NimlReferenceCheck {
        name: "AFNI_dataset sparse table layout",
        references: &[
            "afni/src/matlab/afni_niml_writesimple.m",
            "SUMAvista/src/pysuma/niml.py:read_niml_dset",
            "afni/src/SUMA/SUMA_Surface_IO.c:SUMA_WriteDset",
        ],
        passed: dset_check.unwrap_or(false),
        detail:
            "Recognizes AFNI_dataset groups with SPARSE_DATA, INDEX_LIST, and AFNI_atr metadata."
                .to_string(),
    });

    let roi_check = read_niml_roi_str(MINIMAL_ROI_SAMPLE).map(|payloads| {
        payloads.len() == 1
            && payloads[0].records.len() == 2
            && payloads[0].records[0].node_path == vec![1, 2, 3]
            && payloads[0].fill_color == Some(Rgba::new_unchecked(0.5, 0.1, 0.9, 1.0))
    });
    checks.push(NimlReferenceCheck {
        name: "Node_ROI datum layout",
        references: &[
            "afni/src/SUMA/SUMA_define.h:SUMA_ROI_DATUM",
            "afni/src/SUMA/SUMA_Surface_IO.c:SUMA_OpenDrawnROI_NIML",
            "SUMAvista/src/pysuma/niml.py:read_niml_roi",
        ],
        passed: roi_check.unwrap_or(false),
        detail: "Recognizes commented Node_ROI headers and SUMA_NIML_ROI_DATUM rows as action/type/count/node-path records."
            .to_string(),
    });

    checks
}

impl NimlElement {
    pub fn group(
        name: impl Into<String>,
        attrs: BTreeMap<String, String>,
        children: Vec<NimlElement>,
    ) -> Self {
        Self {
            name: name.into(),
            attrs,
            data: NimlData::Group(children),
        }
    }

    pub fn text(
        name: impl Into<String>,
        attrs: BTreeMap<String, String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            attrs,
            data: NimlData::Text(text.into()),
        }
    }

    pub fn numeric(
        name: impl Into<String>,
        attrs: BTreeMap<String, String>,
        matrix: NimlNumericMatrix,
    ) -> Self {
        Self {
            name: name.into(),
            attrs,
            data: NimlData::Numeric(matrix),
        }
    }
}

impl NimlNumericMatrix {
    pub fn new(column_types: Vec<NimlValueType>, rows: usize, values: Vec<f64>) -> Result<Self> {
        ensure!(
            !column_types.is_empty(),
            "NIML numeric matrix has no columns"
        );
        ensure!(
            column_types.iter().all(NimlValueType::is_numeric),
            "NIML numeric matrix contains a non-numeric column type"
        );
        ensure!(
            values.len() == rows * column_types.len(),
            "NIML numeric matrix has {} values but expected {}",
            values.len(),
            rows * column_types.len()
        );

        Ok(Self {
            column_types,
            rows,
            values,
        })
    }

    pub fn from_rows(column_types: Vec<NimlValueType>, rows: Vec<Vec<f64>>) -> Result<Self> {
        let column_count = column_types.len();
        let row_count = rows.len();
        let mut values = Vec::with_capacity(row_count * column_count);
        for row in rows {
            ensure!(
                row.len() == column_count,
                "NIML row has {} columns but expected {}",
                row.len(),
                column_count
            );
            values.extend(row);
        }
        Self::new(column_types, row_count, values)
    }

    pub fn column_count(&self) -> usize {
        self.column_types.len()
    }

    pub fn get(&self, row: usize, column: usize) -> Option<f64> {
        if row >= self.rows || column >= self.column_count() {
            return None;
        }
        self.values.get(row * self.column_count() + column).copied()
    }
}

impl NimlMixedTable {
    pub fn new(
        column_types: Vec<NimlValueType>,
        rows: usize,
        values: Vec<NimlValue>,
    ) -> Result<Self> {
        ensure!(!column_types.is_empty(), "NIML mixed table has no columns");
        ensure!(
            values.len() == rows * column_types.len(),
            "NIML mixed table has {} values but expected {}",
            values.len(),
            rows * column_types.len()
        );

        Ok(Self {
            column_types,
            rows,
            values,
        })
    }

    pub fn column_count(&self) -> usize {
        self.column_types.len()
    }

    pub fn get(&self, row: usize, column: usize) -> Option<&NimlValue> {
        if row >= self.rows || column >= self.column_count() {
            return None;
        }
        self.values.get(row * self.column_count() + column)
    }
}

impl NimlValueType {
    fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "byte" | "uint8" => Self::UInt8,
            "short" | "int16" => Self::Int16,
            "int" | "int32" => Self::Int32,
            "float" | "float32" => Self::Float32,
            "double" | "float64" => Self::Float64,
            "string" => Self::String,
            "cstring" => Self::CString,
            "suma_niml_roi_datum" => Self::SumaRoiDatum,
            _ => Self::Other(name.trim().to_string()),
        }
    }

    fn canonical_name(&self) -> &str {
        match self {
            Self::UInt8 => "byte",
            Self::Int16 => "short",
            Self::Int32 => "int",
            Self::Float32 => "float",
            Self::Float64 => "double",
            Self::String => "String",
            Self::CString => "CString",
            Self::SumaRoiDatum => "SUMA_NIML_ROI_DATUM",
            Self::Other(value) => value,
        }
    }

    fn is_numeric(&self) -> bool {
        matches!(
            self,
            Self::UInt8 | Self::Int16 | Self::Int32 | Self::Float32 | Self::Float64
        )
    }

    fn is_integer(&self) -> bool {
        matches!(self, Self::UInt8 | Self::Int16 | Self::Int32)
    }
}

impl NimlDatasetPayload {
    pub fn from_element(element: &NimlElement) -> Result<Self> {
        ensure!(
            element.name == "AFNI_dataset",
            "expected AFNI_dataset element, got {}",
            element.name
        );
        let NimlData::Group(children) = &element.data else {
            bail!("AFNI_dataset element is not a NIML group");
        };

        let mut payload = Self {
            dset_type: element
                .attrs
                .get("dset_type")
                .cloned()
                .unwrap_or_else(|| "Node_Bucket".to_string()),
            self_idcode: element.attrs.get("self_idcode").cloned(),
            filename: element.attrs.get("filename").cloned(),
            label: element.attrs.get("label").cloned(),
            sparse_data: None,
            node_indices: None,
            column_ranges: Vec::new(),
            column_labels: Vec::new(),
            column_types: Vec::new(),
            column_stats: Vec::new(),
            history: None,
        };

        for child in children {
            match child.name.as_str() {
                "SPARSE_DATA" => {
                    let NimlData::Numeric(matrix) = &child.data else {
                        bail!("SPARSE_DATA payload is not numeric");
                    };
                    payload.sparse_data = Some(matrix.clone());
                }
                "INDEX_LIST" => {
                    let NimlData::Numeric(matrix) = &child.data else {
                        bail!("INDEX_LIST payload is not numeric");
                    };
                    let mut indices = Vec::with_capacity(matrix.rows);
                    for row in 0..matrix.rows {
                        let value = matrix
                            .get(row, 0)
                            .context("INDEX_LIST row has no first column")?;
                        ensure!(
                            value.is_finite() && value >= 0.0 && value.fract() == 0.0,
                            "INDEX_LIST contains non-node-index value {value}"
                        );
                        indices.push(value as u32);
                    }
                    payload.node_indices = Some(indices);
                }
                "AFNI_atr" => {
                    let Some(atr_name) = child.attrs.get("atr_name") else {
                        continue;
                    };
                    let text = match &child.data {
                        NimlData::Text(text) => text.as_str(),
                        _ => "",
                    };
                    match atr_name.as_str() {
                        "COLMS_RANGE" => payload.column_ranges = split_semicolons(text),
                        "COLMS_LABS" => payload.column_labels = split_semicolons(text),
                        "COLMS_TYPE" => payload.column_types = split_semicolons(text),
                        "COLMS_STATSYM" => payload.column_stats = split_semicolons(text),
                        "HISTORY_NOTE" => payload.history = Some(text.trim().to_string()),
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        Ok(payload)
    }

    pub fn to_element(&self) -> Result<NimlElement> {
        let sparse_data = self
            .sparse_data
            .clone()
            .context("NIML dataset payload has no SPARSE_DATA")?;
        let node_indices = self.node_indices.clone().unwrap_or_else(|| {
            (0..sparse_data.rows)
                .map(|index| index as u32)
                .collect::<Vec<_>>()
        });
        ensure!(
            node_indices.len() == sparse_data.rows,
            "node index count does not match SPARSE_DATA rows"
        );

        let mut root_attrs = BTreeMap::new();
        root_attrs.insert("dset_type".to_string(), self.dset_type.clone());
        if let Some(value) = &self.self_idcode {
            root_attrs.insert("self_idcode".to_string(), value.clone());
        }
        if let Some(value) = &self.filename {
            root_attrs.insert("filename".to_string(), value.clone());
        }
        if let Some(value) = &self.label {
            root_attrs.insert("label".to_string(), value.clone());
        }

        let mut sparse_attrs = BTreeMap::new();
        sparse_attrs.insert("data_type".to_string(), "Node_Bucket_data".to_string());

        let mut index_attrs = BTreeMap::new();
        index_attrs.insert(
            "data_type".to_string(),
            "Node_Bucket_node_indices".to_string(),
        );
        index_attrs.insert("COLMS_LABS".to_string(), "Node Indices".to_string());
        index_attrs.insert("COLMS_TYPE".to_string(), "Node_Index".to_string());
        index_attrs.insert(
            "sorted_node_def".to_string(),
            if node_indices.windows(2).all(|window| window[0] <= window[1]) {
                "Yes"
            } else {
                "No"
            }
            .to_string(),
        );
        let index_matrix = NimlNumericMatrix::new(
            vec![NimlValueType::Int32],
            node_indices.len(),
            node_indices.iter().map(|value| *value as f64).collect(),
        )?;

        let mut children = vec![
            NimlElement::numeric("SPARSE_DATA", sparse_attrs, sparse_data),
            NimlElement::numeric("INDEX_LIST", index_attrs, index_matrix),
        ];
        push_atr(
            &mut children,
            "COLMS_RANGE",
            &join_semicolons(&self.column_ranges),
        );
        push_atr(
            &mut children,
            "COLMS_LABS",
            &join_semicolons(&self.column_labels),
        );
        push_atr(
            &mut children,
            "COLMS_TYPE",
            &join_semicolons(&self.column_types),
        );
        push_atr(
            &mut children,
            "COLMS_STATSYM",
            &join_semicolons(&self.column_stats),
        );
        if let Some(history) = &self.history {
            push_atr(&mut children, "HISTORY_NOTE", history);
        }

        Ok(NimlElement::group("AFNI_dataset", root_attrs, children))
    }

    pub fn to_dataset(&self, domain: &SurfaceDomain) -> Result<Dataset> {
        let sparse_data = self
            .sparse_data
            .as_ref()
            .context("NIML dataset payload has no SPARSE_DATA")?;
        ensure!(
            sparse_data.rows > 0,
            "NIML dataset payload has no SPARSE_DATA rows"
        );

        let columns = (0..sparse_data.column_count())
            .map(|column| self.data_column_for_sparse_column(sparse_data, column))
            .collect::<Result<Vec<_>>>()?;

        let mut dataset = if let Some(node_indices) = &self.node_indices {
            Dataset::sparse(
                DatasetKind::from_niml_payload(self),
                domain,
                node_indices.clone(),
                columns,
            )?
        } else {
            Dataset::dense(DatasetKind::from_niml_payload(self), domain, columns)?
        };

        dataset.parent_ids = DatasetParentIds {
            source_dataset_id: self.self_idcode.clone(),
            domain_parent_id: Some(domain.id.as_str().to_string()),
            surface_parent_id: None,
            volume_parent_id: None,
            originator_id: self.filename.clone(),
        };

        Ok(dataset)
    }

    fn data_column_for_sparse_column(
        &self,
        matrix: &NimlNumericMatrix,
        column: usize,
    ) -> Result<DataColumn> {
        let label = self
            .column_labels
            .get(column)
            .cloned()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("col_{column}"));
        let afni_type = self.column_types.get(column).map(String::as_str);
        let stat = self.column_stats.get(column).map(String::as_str);
        let role = column_role_from_niml_metadata(afni_type, stat, &label);

        let physical_type = matrix
            .column_types
            .get(column)
            .context("SPARSE_DATA column has no NIML physical type")?;
        let values = match physical_type {
            NimlValueType::UInt8 => ColumnData::UInt32(
                (0..matrix.rows)
                    .map(|row| {
                        numeric_matrix_integer_value(matrix, row, column).map(|value| value as u32)
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            NimlValueType::Int16 | NimlValueType::Int32 => ColumnData::Int32(
                (0..matrix.rows)
                    .map(|row| {
                        numeric_matrix_integer_value(matrix, row, column).map(|value| value as i32)
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            NimlValueType::Float64 => ColumnData::Float64(
                (0..matrix.rows)
                    .map(|row| {
                        matrix
                            .get(row, column)
                            .context("SPARSE_DATA column value is missing")
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            NimlValueType::Float32 => ColumnData::Float32(
                (0..matrix.rows)
                    .map(|row| {
                        matrix
                            .get(row, column)
                            .context("SPARSE_DATA column value is missing")
                            .map(|value| value as f32)
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            value_type => bail!("cannot convert non-numeric NIML type {value_type:?} to Dataset"),
        };

        DataColumn::new(label, role, None, values).map(|column| {
            column.with_stat(
                stat.map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            )
        })
    }
}

fn read_gifti_compat(path: &Path) -> Result<GiftiImage> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read GIFTI file {}", path.display()))?;
    let xml = std::str::from_utf8(&bytes)
        .with_context(|| format!("GIFTI file {} is not valid UTF-8", path.display()))?;
    let normalized = normalize_gifti_intent_names(xml);

    gifti_rs::parse_str(&normalized)
        .with_context(|| format!("failed to parse GIFTI file {}", path.display()))
}

fn normalize_gifti_intent_names(xml: &str) -> String {
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

fn gifti_image_to_dataset(
    image: &GiftiImage,
    domain: &SurfaceDomain,
    path: &Path,
) -> Result<Dataset> {
    let columns = image
        .data_arrays
        .iter()
        .enumerate()
        .filter(|(_, array)| {
            array.intent != gifti_rs::intent::POINTSET
                && array.intent != gifti_rs::intent::TRIANGLE
                && array.data.len() == domain.node_count
        })
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

    Dataset::dense(DatasetKind::SurfaceScalar, domain, columns)
        .map(|dataset| dataset.with_parent_ids(parent_ids))
}

fn gifti_array_to_data_column(array: &DataArray, index: usize) -> Result<DataColumn> {
    let label = gifti_meta_value(&array.meta, "Name").unwrap_or_else(|| format!("col_{index}"));
    let role = column_role_from_gifti_intent(array.intent);
    let stat = gifti_stat_from_array(array);

    DataColumn::new(label, role, None, column_data_from_gifti_array(array)?)
        .map(|column| column.with_stat(stat))
}

fn column_data_from_gifti_array(array: &DataArray) -> Result<ColumnData> {
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

fn column_role_from_gifti_intent(intent: i32) -> ColumnRole {
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

fn gifti_stat_from_array(array: &DataArray) -> Option<String> {
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

fn format_stat_parameter(value: f64) -> String {
    if value.fract().abs() < 1.0e-9 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn gifti_meta_value(meta: &Meta, key: &str) -> Option<String> {
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

impl DatasetKind {
    fn from_niml_payload(payload: &NimlDatasetPayload) -> Self {
        let dset_type = compact_lower(&payload.dset_type);
        if dset_type.contains("roi") {
            return Self::Roi;
        }

        let types = payload
            .column_types
            .iter()
            .map(|value| compact_lower(value))
            .collect::<Vec<_>>();

        if types
            .iter()
            .any(|value| value.contains("roi") || value.contains("label"))
        {
            Self::SurfaceLabel
        } else if types.iter().any(|value| value.contains("time"))
            || payload
                .column_labels
                .iter()
                .map(|value| compact_lower(value))
                .any(|value| value.contains("timepoint"))
        {
            Self::SurfaceTimeSeries
        } else if dset_type.is_empty() || dset_type == "nodebucket" {
            Self::SurfaceScalar
        } else {
            Self::Other(payload.dset_type.clone())
        }
    }
}

impl NimlRoiPayload {
    pub fn from_roi(roi: &Roi) -> Self {
        Self {
            self_idcode: Some(roi.id.as_str().to_string()),
            domain_parent_idcode: roi
                .parent_domain_id
                .as_ref()
                .map(|id| id.as_str().to_string()),
            parent_side: roi.parent_side.clone(),
            label: roi.label.clone(),
            integer_label: roi.integer_label,
            roi_type_code: Some(drawing_type_to_code(roi.drawing_type)),
            drawing_type: roi.drawing_type,
            color_plane_name: Some(roi_plane_name(&roi.parent_side)),
            fill_color: Some(roi.fill_color),
            edge_color: Some(roi.edge_color),
            edge_thickness: Some(roi.edge_thickness),
            records: roi
                .data
                .iter()
                .map(NimlRoiDatumRecord::from_roi_datum)
                .collect(),
        }
    }

    pub fn from_element(element: &NimlElement) -> Result<Self> {
        ensure!(
            element.name == "Node_ROI",
            "expected Node_ROI element, got {}",
            element.name
        );
        let NimlData::RoiDatums(records) = &element.data else {
            bail!("Node_ROI payload is not SUMA_NIML_ROI_DATUM data");
        };

        let label = element
            .attrs
            .get("Label")
            .cloned()
            .unwrap_or_else(|| "ROI".to_string());
        let integer_label = parse_i32_attr(&element.attrs, "iLabel")?.unwrap_or(0);
        let roi_type_code = parse_i32_attr(&element.attrs, "Type")?;
        let fill_color = element
            .attrs
            .get("FillColor")
            .map(|value| parse_rgba(value))
            .transpose()?;
        let edge_color = element
            .attrs
            .get("EdgeColor")
            .map(|value| parse_rgba(value))
            .transpose()?;
        let edge_thickness = parse_u32_attr(&element.attrs, "EdgeThickness")?;

        Ok(Self {
            self_idcode: element
                .attrs
                .get("self_idcode")
                .or_else(|| element.attrs.get("idcode_str"))
                .or_else(|| element.attrs.get("Object_ID"))
                .cloned(),
            domain_parent_idcode: element
                .attrs
                .get("domain_parent_idcode")
                .or_else(|| element.attrs.get("Parent_idcode_str"))
                .or_else(|| element.attrs.get("Parent_ID"))
                .cloned(),
            parent_side: element
                .attrs
                .get("Parent_side")
                .map_or(SurfaceSide::Unknown, |value| side_from_text(value)),
            label,
            integer_label,
            roi_type_code,
            drawing_type: roi_type_code
                .map(drawing_type_from_code)
                .unwrap_or(RoiDrawingType::Collection),
            color_plane_name: element.attrs.get("ColPlaneName").cloned(),
            fill_color,
            edge_color,
            edge_thickness,
            records: records.clone(),
        })
    }

    pub fn to_roi(&self) -> Result<Roi> {
        let mut roi = Roi::new(self.label.clone(), self.integer_label)?
            .with_source(RoiSource::NimlRoi, self.self_idcode.clone())?
            .with_drawing_type(self.drawing_type);

        if let Some(id) = &self.self_idcode {
            roi = roi.with_id(id.clone())?;
        }
        if let (Some(fill), Some(edge), Some(thickness)) =
            (self.fill_color, self.edge_color, self.edge_thickness)
        {
            roi = roi.with_style(fill, edge, thickness)?;
        }
        roi.parent_side = self.parent_side.clone();
        roi = roi.with_data(
            self.records
                .iter()
                .map(NimlRoiDatumRecord::to_roi_datum)
                .collect::<Result<Vec<_>>>()?,
        )?;

        Ok(roi)
    }

    pub fn to_element(&self) -> NimlElement {
        let mut attrs = BTreeMap::new();
        if let Some(value) = &self.self_idcode {
            attrs.insert("self_idcode".to_string(), value.clone());
        }
        if let Some(value) = &self.domain_parent_idcode {
            attrs.insert("domain_parent_idcode".to_string(), value.clone());
        }
        attrs.insert(
            "Parent_side".to_string(),
            side_to_niml(&self.parent_side).to_string(),
        );
        attrs.insert("Label".to_string(), self.label.clone());
        attrs.insert("iLabel".to_string(), self.integer_label.to_string());
        attrs.insert(
            "Type".to_string(),
            self.roi_type_code
                .unwrap_or_else(|| drawing_type_to_code(self.drawing_type))
                .to_string(),
        );
        if let Some(value) = &self.color_plane_name {
            attrs.insert("ColPlaneName".to_string(), value.clone());
        }
        if let Some(value) = self.fill_color {
            attrs.insert("FillColor".to_string(), rgba_to_string(value));
        }
        if let Some(value) = self.edge_color {
            attrs.insert("EdgeColor".to_string(), rgba_to_string(value));
        }
        if let Some(value) = self.edge_thickness {
            attrs.insert("EdgeThickness".to_string(), value.to_string());
        }

        NimlElement {
            name: "Node_ROI".to_string(),
            attrs,
            data: NimlData::RoiDatums(self.records.clone()),
        }
    }
}

impl NimlRoiDatumRecord {
    fn from_roi_datum(datum: &RoiDatum) -> Self {
        let node_path = if datum.kind == RoiElementKind::FaceGroup {
            datum.triangle_path.clone()
        } else {
            datum.node_path.clone()
        };

        Self {
            action_code: brush_action_to_code(datum.action),
            element_type_code: roi_element_kind_to_code(datum.kind),
            node_path,
        }
    }

    fn to_roi_datum(&self) -> Result<RoiDatum> {
        let action = brush_action_from_code(self.action_code);
        match roi_element_kind_from_code(self.element_type_code) {
            RoiElementKind::FaceGroup => RoiDatum::face_group(self.node_path.clone()),
            kind => RoiDatum::new(kind, action, self.node_path.clone(), Vec::new()),
        }
    }
}

struct NimlByteParser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> NimlByteParser<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    fn parse(&mut self) -> Result<Vec<NimlElement>> {
        let mut elements = Vec::new();
        loop {
            self.skip_whitespace();
            if self.is_end() {
                return Ok(elements);
            }
            if self.peek(b"<?xml") {
                self.consume_until(b">")?;
                continue;
            }
            elements.push(self.parse_element()?);
        }
    }

    fn parse_element(&mut self) -> Result<NimlElement> {
        self.skip_whitespace();
        self.expect(b"<")?;
        ensure!(
            !self.peek(b"/"),
            "unexpected closing tag at byte {}",
            self.pos
        );

        let name = self.read_name()?;
        let raw_header = self.read_header()?;
        let header = raw_header.trim();
        let self_closing = header.ends_with('/');
        let header = if self_closing {
            header.trim_end_matches('/').trim_end()
        } else {
            header
        };
        let attrs = parse_attrs(header)?;

        if self_closing {
            return Ok(NimlElement {
                name,
                attrs,
                data: NimlData::None,
            });
        }

        let data = if attrs
            .get("ni_form")
            .is_some_and(|value| value == "ni_group")
        {
            let mut children = Vec::new();
            loop {
                self.skip_whitespace();
                let end_marker = format!("</{name}>");
                if self.peek(end_marker.as_bytes()) {
                    break;
                }
                children.push(self.parse_element()?);
            }
            self.expect(format!("</{name}>").as_bytes())?;
            NimlData::Group(children)
        } else {
            let end_marker = format!("</{name}>");
            if element_is_binary(&attrs) {
                let payload_len = binary_payload_len(&attrs)?;
                ensure!(
                    self.pos + payload_len <= self.input.len(),
                    "binary NIML payload for {name} ended early"
                );
                let body = &self.input[self.pos..self.pos + payload_len];
                self.pos += payload_len;
                self.expect(end_marker.as_bytes())?;
                parse_element_data_bytes(&attrs, body)?
            } else {
                let Some(relative_end) = find_bytes(&self.input[self.pos..], end_marker.as_bytes())
                else {
                    bail!("missing closing tag for NIML element {name}");
                };
                let end = self.pos + relative_end;
                let body = &self.input[self.pos..end];
                self.pos = end + end_marker.len();
                parse_element_data_bytes(&attrs, body)?
            }
        };

        Ok(NimlElement { name, attrs, data })
    }

    fn is_end(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn skip_whitespace(&mut self) {
        while self
            .input
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }

    fn peek(&self, token: &[u8]) -> bool {
        self.input[self.pos..].starts_with(token)
    }

    fn expect(&mut self, token: &[u8]) -> Result<()> {
        ensure!(
            self.peek(token),
            "expected {:?} at byte {}, found {:?}",
            String::from_utf8_lossy(token),
            self.pos,
            self.input
                .get(self.pos..self.pos.saturating_add(token.len()))
                .map(String::from_utf8_lossy)
        );
        self.pos += token.len();
        Ok(())
    }

    fn consume_until(&mut self, token: &[u8]) -> Result<()> {
        let Some(index) = find_bytes(&self.input[self.pos..], token) else {
            bail!("did not find marker {:?}", String::from_utf8_lossy(token));
        };
        self.pos += index + token.len();
        Ok(())
    }

    fn read_name(&mut self) -> Result<String> {
        let start = self.pos;
        while let Some(byte) = self.input.get(self.pos) {
            if byte.is_ascii_whitespace() || *byte == b'>' || *byte == b'/' {
                break;
            }
            self.pos += 1;
        }
        ensure!(self.pos > start, "NIML element has no name");
        String::from_utf8(self.input[start..self.pos].to_vec())
            .context("NIML tag name is not UTF-8")
    }

    fn read_header(&mut self) -> Result<String> {
        let start = self.pos;
        let mut in_quote = false;
        while let Some(byte) = self.input.get(self.pos) {
            if *byte == b'"' {
                in_quote = !in_quote;
            } else if *byte == b'>' && !in_quote {
                let header = String::from_utf8(self.input[start..self.pos].to_vec())
                    .context("NIML element header is not UTF-8")?;
                self.pos += 1;
                return Ok(header);
            }
            self.pos += 1;
        }
        bail!("unterminated NIML element header")
    }
}

fn parse_element_data_bytes(attrs: &BTreeMap<String, String>, body: &[u8]) -> Result<NimlData> {
    let Some(ni_type) = attrs.get("ni_type") else {
        let body = std::str::from_utf8(body).context("NIML text payload is not UTF-8")?;
        return Ok(NimlData::Text(unescape_niml(body.trim())));
    };
    let column_types = expand_niml_type(ni_type)?;
    let rows = attrs
        .get("ni_dimen")
        .map(|value| value.parse::<usize>())
        .transpose()
        .context("invalid NIML ni_dimen")?
        .unwrap_or(0);

    if element_is_binary(attrs) {
        if column_types == [NimlValueType::SumaRoiDatum] {
            bail!("binary SUMA_NIML_ROI_DATUM payloads are not supported yet");
        }
        ensure!(
            column_types.iter().all(NimlValueType::is_numeric),
            "binary NIML payloads with string or mixed columns are not supported"
        );
        return Ok(NimlData::Numeric(parse_binary_numeric_matrix(
            body,
            column_types,
            rows,
            attrs.get("ni_form").map(String::as_str),
        )?));
    }

    let body = std::str::from_utf8(body).context("NIML text payload is not UTF-8")?;

    if column_types == [NimlValueType::SumaRoiDatum] {
        return Ok(NimlData::RoiDatums(parse_roi_datum_records(body, rows)?));
    }

    if column_types.iter().all(NimlValueType::is_numeric) {
        return Ok(NimlData::Numeric(parse_numeric_matrix(
            body,
            column_types,
            rows,
        )?));
    }

    if column_types.len() == 1
        && matches!(
            column_types[0],
            NimlValueType::String | NimlValueType::CString
        )
    {
        return Ok(NimlData::Text(unescape_niml(body.trim())));
    }

    Ok(NimlData::Mixed(parse_mixed_table(
        body,
        column_types,
        rows,
    )?))
}

fn parse_numeric_matrix(
    body: &str,
    column_types: Vec<NimlValueType>,
    rows: usize,
) -> Result<NimlNumericMatrix> {
    let expected = rows * column_types.len();
    let values = body
        .split_whitespace()
        .map(|token| {
            token
                .parse::<f64>()
                .with_context(|| format!("invalid NIML numeric value {token:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        values.len() == expected,
        "NIML numeric payload has {} values but expected {}",
        values.len(),
        expected
    );

    NimlNumericMatrix::new(column_types, rows, values)
}

fn parse_binary_numeric_matrix(
    body: &[u8],
    column_types: Vec<NimlValueType>,
    rows: usize,
    ni_form: Option<&str>,
) -> Result<NimlNumericMatrix> {
    let row_stride = column_types
        .iter()
        .map(NimlValueType::byte_width)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .sum::<usize>();
    let expected = rows * row_stride;
    ensure!(
        body.len() == expected,
        "binary NIML payload has {} bytes but expected {}",
        body.len(),
        expected
    );

    let byte_order = BinaryByteOrder::from_ni_form(ni_form.unwrap_or("binary"))?;
    let mut values = Vec::with_capacity(rows * column_types.len());
    let mut offset = 0;
    for _row in 0..rows {
        for column_type in &column_types {
            let width = column_type.byte_width()?;
            let chunk = &body[offset..offset + width];
            offset += width;
            values.push(decode_binary_value(column_type, chunk, byte_order)?);
        }
    }

    NimlNumericMatrix::new(column_types, rows, values)
}

fn parse_mixed_table(
    body: &str,
    column_types: Vec<NimlValueType>,
    rows: usize,
) -> Result<NimlMixedTable> {
    let mut parser = MixedValueParser::new(body);
    let mut values = Vec::with_capacity(rows * column_types.len());

    for _row in 0..rows {
        for column_type in &column_types {
            values.push(parser.parse_value(column_type)?);
        }
    }
    parser.skip_delimiters();
    ensure!(
        parser.is_done_or_at_closing_text(),
        "NIML mixed payload contains trailing data"
    );

    NimlMixedTable::new(column_types, rows, values)
}

#[derive(Debug, Clone, Copy)]
enum BinaryByteOrder {
    Native,
    Little,
    Big,
}

impl BinaryByteOrder {
    fn from_ni_form(ni_form: &str) -> Result<Self> {
        match ni_form.to_ascii_lowercase().as_str() {
            "binary" => Ok(Self::Native),
            "binary.lsbfirst" => Ok(Self::Little),
            "binary.msbfirst" => Ok(Self::Big),
            _ => bail!("unrecognized binary NIML form {ni_form:?}"),
        }
    }

    fn is_little(self) -> bool {
        match self {
            Self::Native => cfg!(target_endian = "little"),
            Self::Little => true,
            Self::Big => false,
        }
    }
}

impl NimlValueType {
    fn byte_width(&self) -> Result<usize> {
        match self {
            Self::UInt8 => Ok(1),
            Self::Int16 => Ok(2),
            Self::Int32 | Self::Float32 => Ok(4),
            Self::Float64 => Ok(8),
            value_type => bail!("NIML type {value_type:?} has no fixed numeric byte width"),
        }
    }
}

fn decode_binary_value(
    value_type: &NimlValueType,
    bytes: &[u8],
    byte_order: BinaryByteOrder,
) -> Result<f64> {
    Ok(match value_type {
        NimlValueType::UInt8 => bytes[0] as f64,
        NimlValueType::Int16 => {
            let raw = [bytes[0], bytes[1]];
            (if byte_order.is_little() {
                i16::from_le_bytes(raw)
            } else {
                i16::from_be_bytes(raw)
            }) as f64
        }
        NimlValueType::Int32 => {
            let raw = [bytes[0], bytes[1], bytes[2], bytes[3]];
            (if byte_order.is_little() {
                i32::from_le_bytes(raw)
            } else {
                i32::from_be_bytes(raw)
            }) as f64
        }
        NimlValueType::Float32 => {
            let raw = [bytes[0], bytes[1], bytes[2], bytes[3]];
            (if byte_order.is_little() {
                f32::from_le_bytes(raw)
            } else {
                f32::from_be_bytes(raw)
            }) as f64
        }
        NimlValueType::Float64 => {
            let raw = [
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ];
            if byte_order.is_little() {
                f64::from_le_bytes(raw)
            } else {
                f64::from_be_bytes(raw)
            }
        }
        value_type => bail!("cannot decode non-numeric binary NIML type {value_type:?}"),
    })
}

fn binary_payload_len(attrs: &BTreeMap<String, String>) -> Result<usize> {
    let ni_type = attrs
        .get("ni_type")
        .context("binary NIML element has no ni_type")?;
    let rows = attrs
        .get("ni_dimen")
        .context("binary NIML element has no ni_dimen")?
        .parse::<usize>()
        .context("invalid binary NIML ni_dimen")?;
    let column_types = expand_niml_type(ni_type)?;
    ensure!(
        column_types.iter().all(NimlValueType::is_numeric),
        "binary NIML element has non-numeric fixed-width type"
    );
    let row_width = column_types
        .iter()
        .map(NimlValueType::byte_width)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .sum::<usize>();
    Ok(rows * row_width)
}

fn element_is_binary(attrs: &BTreeMap<String, String>) -> bool {
    attrs
        .get("ni_form")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("binary"))
}

struct MixedValueParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> MixedValueParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn parse_value(&mut self, value_type: &NimlValueType) -> Result<NimlValue> {
        self.skip_delimiters();
        match value_type {
            NimlValueType::String | NimlValueType::CString => {
                self.parse_token().map(NimlValue::Text)
            }
            value_type if value_type.is_integer() => {
                let token = self.parse_token()?;
                token
                    .parse::<i64>()
                    .map(NimlValue::Integer)
                    .with_context(|| format!("invalid NIML integer value {token:?}"))
            }
            value_type if value_type.is_numeric() => {
                let token = self.parse_token()?;
                token
                    .parse::<f64>()
                    .map(NimlValue::Float)
                    .with_context(|| format!("invalid NIML float value {token:?}"))
            }
            value_type => bail!("cannot parse NIML mixed value type {value_type:?}"),
        }
    }

    fn parse_token(&mut self) -> Result<String> {
        self.skip_delimiters();
        ensure!(
            self.pos < self.input.len(),
            "NIML mixed payload ended early"
        );
        let bytes = self.input.as_bytes();
        let first = bytes[self.pos];

        if first == b'"' || first == b'\'' {
            let quote = first;
            self.pos += 1;
            let start = self.pos;
            while self.pos < bytes.len() && bytes[self.pos] != quote {
                self.pos += 1;
            }
            ensure!(
                self.pos < bytes.len(),
                "quoted NIML mixed value is unterminated"
            );
            let token = unescape_niml(&self.input[start..self.pos]);
            self.pos += 1;
            return Ok(token);
        }

        let start = self.pos;
        while self.pos < bytes.len()
            && !bytes[self.pos].is_ascii_whitespace()
            && bytes[self.pos] != b';'
        {
            self.pos += 1;
        }
        ensure!(self.pos > start, "NIML mixed payload has empty token");
        Ok(unescape_niml(&self.input[start..self.pos]))
    }

    fn skip_delimiters(&mut self) {
        while let Some(byte) = self.input.as_bytes().get(self.pos) {
            if byte.is_ascii_whitespace() || *byte == b';' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn is_done_or_at_closing_text(&self) -> bool {
        self.pos >= self.input.len()
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_roi_datum_records(body: &str, rows: usize) -> Result<Vec<NimlRoiDatumRecord>> {
    let values = body
        .split_whitespace()
        .map(|token| {
            token
                .parse::<i32>()
                .with_context(|| format!("invalid SUMA_NIML_ROI_DATUM value {token:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut records = Vec::new();
    let mut pos = 0;

    while pos < values.len() {
        ensure!(
            pos + 3 <= values.len(),
            "malformed SUMA_NIML_ROI_DATUM record header"
        );
        let action_code = values[pos];
        let element_type_code = values[pos + 1];
        let node_count = values[pos + 2];
        ensure!(
            node_count >= 0,
            "SUMA_NIML_ROI_DATUM record has negative node count"
        );
        pos += 3;
        let node_count = node_count as usize;
        ensure!(
            pos + node_count <= values.len(),
            "malformed SUMA_NIML_ROI_DATUM node path"
        );
        let mut node_path = Vec::with_capacity(node_count);
        for value in &values[pos..pos + node_count] {
            ensure!(
                *value >= 0,
                "SUMA_NIML_ROI_DATUM node path contains negative node index"
            );
            node_path.push(*value as u32);
        }
        pos += node_count;
        records.push(NimlRoiDatumRecord {
            action_code,
            element_type_code,
            node_path,
        });
    }

    if rows > 0 {
        ensure!(
            records.len() == rows,
            "SUMA_NIML_ROI_DATUM contains {} records but ni_dimen says {}",
            records.len(),
            rows
        );
    }

    Ok(records)
}

fn parse_attrs(header: &str) -> Result<BTreeMap<String, String>> {
    let mut attrs = BTreeMap::new();
    let bytes = header.as_bytes();
    let mut pos = 0;

    while pos < bytes.len() {
        while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }

        let key_start = pos;
        while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() && bytes[pos] != b'=' {
            pos += 1;
        }
        let key = header[key_start..pos].trim();
        ensure!(!key.is_empty(), "NIML attribute has empty name");
        while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        if bytes.get(pos) != Some(&b'=') {
            attrs.insert(key.to_string(), String::new());
            continue;
        }
        pos += 1;
        while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        ensure!(
            bytes.get(pos) == Some(&b'"'),
            "NIML attribute {key} value is not quoted"
        );
        pos += 1;
        let value_start = pos;
        while pos < bytes.len() && bytes[pos] != b'"' {
            pos += 1;
        }
        ensure!(
            pos < bytes.len(),
            "NIML attribute {key} has no closing quote"
        );
        let value = unescape_niml(&header[value_start..pos]);
        pos += 1;
        attrs.insert(key.to_string(), value);
    }

    Ok(attrs)
}

fn serialize_element(element: &NimlElement, out: &mut String) {
    let mut attrs = element.attrs.clone();
    match &element.data {
        NimlData::Group(_) => {
            attrs.insert("ni_form".to_string(), "ni_group".to_string());
        }
        NimlData::Numeric(matrix) => {
            attrs.insert("ni_type".to_string(), ni_type_string(&matrix.column_types));
            attrs.insert("ni_dimen".to_string(), matrix.rows.to_string());
        }
        NimlData::Mixed(table) => {
            attrs.insert("ni_type".to_string(), ni_type_string(&table.column_types));
            attrs.insert("ni_dimen".to_string(), table.rows.to_string());
        }
        NimlData::RoiDatums(records) => {
            attrs.insert(
                "ni_type".to_string(),
                NimlValueType::SumaRoiDatum.canonical_name().to_string(),
            );
            attrs.insert("ni_dimen".to_string(), records.len().to_string());
        }
        NimlData::Text(_) => {
            attrs
                .entry("ni_type".to_string())
                .or_insert_with(|| "String".to_string());
            attrs
                .entry("ni_dimen".to_string())
                .or_insert_with(|| "1".to_string());
        }
        NimlData::None => {}
    }

    out.push('<');
    out.push_str(&element.name);
    for (key, value) in attrs {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&key);
        out.push_str("=\"");
        out.push_str(&escape_niml(&value));
        out.push('"');
    }
    out.push_str(" >");

    match &element.data {
        NimlData::Group(children) => {
            out.push('\n');
            for child in children {
                serialize_element(child, out);
            }
        }
        NimlData::Numeric(matrix) => {
            for row in 0..matrix.rows {
                out.push('\n');
                for column in 0..matrix.column_count() {
                    if column > 0 {
                        out.push(' ');
                    }
                    let value = matrix.get(row, column).unwrap_or(0.0);
                    if matrix.column_types[column].is_integer() {
                        out.push_str(&(value as i64).to_string());
                    } else {
                        out.push_str(&format_float(value));
                    }
                }
            }
            out.push('\n');
        }
        NimlData::Mixed(table) => {
            for row in 0..table.rows {
                out.push('\n');
                for column in 0..table.column_count() {
                    if column > 0 {
                        out.push(' ');
                    }
                    if let Some(value) = table.get(row, column) {
                        out.push_str(&format_mixed_value(value));
                    }
                }
            }
            out.push('\n');
        }
        NimlData::RoiDatums(records) => {
            for record in records {
                out.push('\n');
                out.push_str(&format!(
                    "{} {} {}",
                    record.action_code,
                    record.element_type_code,
                    record.node_path.len()
                ));
                for node in &record.node_path {
                    out.push(' ');
                    out.push_str(&node.to_string());
                }
            }
            out.push('\n');
        }
        NimlData::Text(text) => {
            out.push('\n');
            out.push_str(&escape_niml(text));
            out.push('\n');
        }
        NimlData::None => {}
    }

    out.push_str("</");
    out.push_str(&element.name);
    out.push_str(">\n");
}

fn push_atr(children: &mut Vec<NimlElement>, atr_name: &str, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let mut attrs = BTreeMap::new();
    attrs.insert("atr_name".to_string(), atr_name.to_string());
    children.push(NimlElement::text("AFNI_atr", attrs, text.to_string()));
}

fn split_semicolons(text: &str) -> Vec<String> {
    strip_outer_quotes(text.trim())
        .split(';')
        .map(|piece| strip_outer_quotes(piece.trim()))
        .filter(|piece| !piece.is_empty())
        .map(str::to_string)
        .collect()
}

fn join_semicolons(values: &[String]) -> String {
    if values.is_empty() {
        return String::new();
    }
    format!("{};", values.join(";"))
}

fn strip_outer_quotes(text: &str) -> &str {
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        &text[1..text.len() - 1]
    } else {
        text
    }
}

fn ni_type_string(types: &[NimlValueType]) -> String {
    types
        .iter()
        .map(NimlValueType::canonical_name)
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_i32_attr(attrs: &BTreeMap<String, String>, key: &str) -> Result<Option<i32>> {
    attrs
        .get(key)
        .map(|value| {
            value
                .parse::<i32>()
                .with_context(|| format!("invalid {key}"))
        })
        .transpose()
}

fn parse_u32_attr(attrs: &BTreeMap<String, String>, key: &str) -> Result<Option<u32>> {
    attrs
        .get(key)
        .map(|value| {
            value
                .parse::<u32>()
                .with_context(|| format!("invalid {key}"))
        })
        .transpose()
}

fn parse_rgba(value: &str) -> Result<Rgba> {
    let components = value
        .split_whitespace()
        .map(|piece| {
            piece
                .parse::<f32>()
                .with_context(|| format!("invalid color component {piece:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        components.len() == 3 || components.len() == 4,
        "NIML color must have three or four components"
    );
    let alpha = components.get(3).copied().unwrap_or(1.0);
    Ok(Rgba::clamped(
        components[0],
        components[1],
        components[2],
        alpha,
    ))
}

fn rgba_to_string(value: Rgba) -> String {
    format!(
        "{} {} {} {}",
        format_float(value.red as f64),
        format_float(value.green as f64),
        format_float(value.blue as f64),
        format_float(value.alpha as f64)
    )
}

fn side_from_text(value: &str) -> SurfaceSide {
    match value.trim().to_ascii_lowercase().as_str() {
        "l" | "lh" | "left" => SurfaceSide::Left,
        "r" | "rh" | "right" => SurfaceSide::Right,
        "lr" | "both" | "bilateral" => SurfaceSide::Both,
        "" | "no_side" | "none" | "unknown" => SurfaceSide::Unknown,
        _ => SurfaceSide::Other(value.trim().to_string()),
    }
}

fn side_to_niml(side: &SurfaceSide) -> &str {
    match side {
        SurfaceSide::Left => "L",
        SurfaceSide::Right => "R",
        SurfaceSide::Both => "LR",
        SurfaceSide::Unknown => "no_side",
        SurfaceSide::Other(value) => value,
    }
}

fn drawing_type_from_code(value: i32) -> RoiDrawingType {
    match value {
        0 => RoiDrawingType::OpenPath,
        1 => RoiDrawingType::ClosedPath,
        2 => RoiDrawingType::FilledArea,
        _ => RoiDrawingType::Collection,
    }
}

fn drawing_type_to_code(value: RoiDrawingType) -> i32 {
    match value {
        RoiDrawingType::OpenPath => 0,
        RoiDrawingType::ClosedPath => 1,
        RoiDrawingType::FilledArea => 2,
        RoiDrawingType::Collection => 4,
    }
}

fn roi_element_kind_to_code(value: RoiElementKind) -> i32 {
    match value {
        RoiElementKind::NodeGroup => 1,
        RoiElementKind::EdgeGroup => 2,
        RoiElementKind::FaceGroup => 3,
        RoiElementKind::NodeSegment => 4,
        RoiElementKind::Unknown => 0,
    }
}

fn roi_element_kind_from_code(value: i32) -> RoiElementKind {
    match value {
        1 => RoiElementKind::NodeGroup,
        2 => RoiElementKind::EdgeGroup,
        3 => RoiElementKind::FaceGroup,
        4 => RoiElementKind::NodeSegment,
        _ => RoiElementKind::Unknown,
    }
}

fn brush_action_from_code(value: i32) -> RoiBrushAction {
    match value {
        1 => RoiBrushAction::AppendStroke,
        2 => RoiBrushAction::AppendStrokeOrFill,
        3 => RoiBrushAction::JoinEnds,
        4 => RoiBrushAction::FillArea,
        _ => RoiBrushAction::Unknown,
    }
}

fn brush_action_to_code(value: RoiBrushAction) -> i32 {
    match value {
        RoiBrushAction::AppendStroke => 1,
        RoiBrushAction::AppendStrokeOrFill => 2,
        RoiBrushAction::JoinEnds => 3,
        RoiBrushAction::FillArea => 4,
        RoiBrushAction::Unknown => 0,
    }
}

fn roi_plane_name(side: &SurfaceSide) -> String {
    match side {
        SurfaceSide::Left => "ROI.L.iS_0".to_string(),
        SurfaceSide::Right => "ROI.R.iS_0".to_string(),
        _ => "Sumaru_ROI".to_string(),
    }
}

fn strip_niml_comment_prefixes(text: &str) -> String {
    let mut cleaned = String::new();
    for line in text.lines() {
        let stripped = line.trim_start();
        if let Some(rest) = stripped.strip_prefix('#') {
            let uncommented = rest.strip_prefix(' ').unwrap_or(rest);
            cleaned.push_str(uncommented);
        } else {
            cleaned.push_str(line);
        }
        cleaned.push('\n');
    }
    cleaned
}

fn format_float(value: f64) -> String {
    let mut formatted = format!("{value:.10}");
    while formatted.contains('.') && formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.push('0');
    }
    formatted
}

fn format_mixed_value(value: &NimlValue) -> String {
    match value {
        NimlValue::Integer(value) => value.to_string(),
        NimlValue::Float(value) => format_float(*value),
        NimlValue::Text(value) => {
            if value.chars().any(|ch| ch.is_whitespace() || ch == ';') {
                format!("\"{}\"", escape_niml(value))
            } else {
                escape_niml(value)
            }
        }
    }
}

fn numeric_matrix_integer_value(
    matrix: &NimlNumericMatrix,
    row: usize,
    column: usize,
) -> Result<i64> {
    let value = matrix
        .get(row, column)
        .context("SPARSE_DATA column value is missing")?;
    ensure!(
        value.is_finite() && value.fract() == 0.0,
        "SPARSE_DATA column value {value} is not an integer"
    );
    Ok(value as i64)
}

fn column_role_from_niml_metadata(
    afni_type: Option<&str>,
    stat: Option<&str>,
    label: &str,
) -> ColumnRole {
    let type_text = afni_type.map(compact_lower).unwrap_or_default();
    let stat_text = stat.map(compact_lower).unwrap_or_default();
    let label_text = compact_lower(label);

    if type_text.contains("nodeindex") || label_text.contains("nodeindex") {
        ColumnRole::NodeIndex
    } else if type_text.contains("label") || type_text.contains("roi") {
        ColumnRole::Label
    } else if type_text.contains("mask") {
        ColumnRole::Mask
    } else if type_text.contains("time") {
        ColumnRole::TimePoint
    } else if type_text.contains("threshold") || label_text.contains("threshold") {
        ColumnRole::Threshold
    } else if type_text.contains("brightness") || label_text.contains("brightness") {
        ColumnRole::Brightness
    } else if !stat_text.is_empty() && stat_text != "none" {
        ColumnRole::Statistic
    } else if type_text.contains("stat") || label_text.contains("stat") {
        ColumnRole::Statistic
    } else {
        ColumnRole::Intensity
    }
}

fn compact_lower(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn escape_niml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
        .replace('>', "&gt;")
        .replace('<', "&lt;")
}

fn unescape_niml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

const MINIMAL_DSET_SAMPLE: &str = r#"
<AFNI_dataset
  ni_form="ni_group"
  dset_type="Node_Bucket"
  self_idcode="XYZ_TEST"
  filename="toy.niml.dset"
  label="toy.niml.dset" >
<SPARSE_DATA
  ni_type="float,float"
  ni_dimen="2"
  data_type="Node_Bucket_data" >
1.5 2.5
3.5 4.5
</SPARSE_DATA>
<INDEX_LIST
  ni_type="int"
  ni_dimen="2"
  data_type="Node_Bucket_node_indices" >
10
12
</INDEX_LIST>
<AFNI_atr
  ni_type="String"
  ni_dimen="1"
  atr_name="COLMS_LABS" >
effect;stat;
</AFNI_atr>
</AFNI_dataset>
"#;

const MINIMAL_ROI_SAMPLE: &str = r#"
# <Node_ROI
#  ni_type = "SUMA_NIML_ROI_DATUM"
#  ni_dimen = "2"
#  self_idcode = "XYZ_ROI"
#  domain_parent_idcode = "DOMAIN"
#  Parent_side = "L"
#  Label = "V1"
#  iLabel = "7"
#  Type = "4"
#  ColPlaneName = "ROI.L.iS_0"
#  FillColor = "0.5 0.1 0.9 1.0"
#  EdgeColor = "0.0 0.0 1.0 1.0"
#  EdgeThickness = "2"
# >
 1 4 3 1 2 3
 4 1 2 8 9
# </Node_ROI>
"#;

#[cfg(test)]
mod tests {
    use super::{
        NimlData, NimlDatasetPayload, NimlElement, NimlMixedTable, NimlNumericMatrix, NimlValue,
        NimlValueType, expand_niml_type, niml_reference_checks, parse_niml_bytes, parse_niml_str,
        read_niml_dset_str, read_niml_roi_str, serialize_niml_ascii,
    };
    use crate::color::Rgba;
    use crate::dataset::{ColumnData, ColumnRole, DatasetKind};
    use crate::roi::{Roi, RoiBrushAction, RoiDatum, RoiDrawingType, RoiElementKind, RoiSource};
    use crate::surface::{SurfaceDomain, SurfaceSide};

    #[test]
    fn niml_type_expansion_matches_reference_repeat_syntax() {
        let types = expand_niml_type("int,3*float,String,SUMA_NIML_ROI_DATUM").unwrap();

        assert_eq!(
            types,
            vec![
                NimlValueType::Int32,
                NimlValueType::Float32,
                NimlValueType::Float32,
                NimlValueType::Float32,
                NimlValueType::String,
                NimlValueType::SumaRoiDatum,
            ]
        );
    }

    #[test]
    fn ascii_niml_parser_reads_dataset_group_contract() {
        let elements = parse_niml_str(super::MINIMAL_DSET_SAMPLE).unwrap();

        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].name, "AFNI_dataset");
        let NimlData::Group(children) = &elements[0].data else {
            panic!("expected group");
        };
        assert_eq!(children[0].name, "SPARSE_DATA");
        let NimlData::Numeric(matrix) = &children[0].data else {
            panic!("expected numeric sparse data");
        };
        assert_eq!(matrix.rows, 2);
        assert_eq!(matrix.column_count(), 2);
        assert_eq!(matrix.get(1, 1), Some(4.5));
    }

    #[test]
    fn dataset_payload_roundtrips_sparse_data_and_attributes() {
        let payload = read_niml_dset_str(super::MINIMAL_DSET_SAMPLE).unwrap();

        assert_eq!(payload.dset_type, "Node_Bucket");
        assert_eq!(payload.node_indices, Some(vec![10, 12]));
        assert_eq!(payload.column_labels, vec!["effect", "stat"]);

        let element = payload.to_element().unwrap();
        let serialized = serialize_niml_ascii(&[element]);
        let reparsed = read_niml_dset_str(&serialized).unwrap();

        assert_eq!(reparsed.node_indices, Some(vec![10, 12]));
        assert_eq!(
            reparsed.sparse_data.unwrap().values,
            vec![1.5, 2.5, 3.5, 4.5]
        );
    }

    #[test]
    fn binary_little_endian_numeric_payload_reads_fixed_width_values() {
        let mut bytes =
            b"<SPARSE_DATA ni_type=\"float,int\" ni_dimen=\"2\" ni_form=\"binary.lsbfirst\" >"
                .to_vec();
        bytes.extend_from_slice(&1.5_f32.to_le_bytes());
        bytes.extend_from_slice(&10_i32.to_le_bytes());
        bytes.extend_from_slice(&2.5_f32.to_le_bytes());
        bytes.extend_from_slice(&12_i32.to_le_bytes());
        bytes.extend_from_slice(b"</SPARSE_DATA>");

        let elements = parse_niml_bytes(&bytes).unwrap();
        let NimlData::Numeric(matrix) = &elements[0].data else {
            panic!("expected binary numeric matrix");
        };

        assert_eq!(matrix.rows, 2);
        assert_eq!(
            matrix.column_types,
            vec![NimlValueType::Float32, NimlValueType::Int32]
        );
        assert_eq!(matrix.get(0, 0), Some(1.5));
        assert_eq!(matrix.get(1, 1), Some(12.0));
    }

    #[test]
    fn bare_niml_attributes_are_preserved_as_empty_values() {
        let text = r#"<AFNI_dataset
            dset_type="Node_Bucket"
            domain_parent_idcode
            geometry_parent_idcode
            ni_form="ni_group">
            </AFNI_dataset>"#;

        let elements = parse_niml_str(text).unwrap();

        assert_eq!(
            elements[0].attrs.get("domain_parent_idcode"),
            Some(&String::new())
        );
        assert_eq!(
            elements[0].attrs.get("geometry_parent_idcode"),
            Some(&String::new())
        );
        assert_eq!(
            elements[0].attrs.get("dset_type").map(String::as_str),
            Some("Node_Bucket")
        );
    }

    #[test]
    fn binary_big_endian_double_payload_reads_values() {
        let mut bytes =
            b"<SPARSE_DATA ni_type=\"double\" ni_dimen=\"2\" ni_form=\"binary.msbfirst\" >"
                .to_vec();
        bytes.extend_from_slice(&1.25_f64.to_be_bytes());
        bytes.extend_from_slice(&2.75_f64.to_be_bytes());
        bytes.extend_from_slice(b"</SPARSE_DATA>");

        let elements = parse_niml_bytes(&bytes).unwrap();
        let NimlData::Numeric(matrix) = &elements[0].data else {
            panic!("expected binary numeric matrix");
        };

        assert_eq!(matrix.column_types, vec![NimlValueType::Float64]);
        assert_eq!(matrix.values, vec![1.25, 2.75]);
    }

    #[test]
    fn mixed_ascii_rows_preserve_string_and_numeric_columns() {
        let elements = parse_niml_str(
            r#"<MIXED ni_type="int,String,float" ni_dimen="2" >
1 "alpha beta" 2.5
2 gamma 3.5
</MIXED>"#,
        )
        .unwrap();
        let NimlData::Mixed(table) = &elements[0].data else {
            panic!("expected mixed table");
        };

        assert_eq!(table.rows, 2);
        assert_eq!(table.get(0, 0), Some(&NimlValue::Integer(1)));
        assert_eq!(
            table.get(0, 1),
            Some(&NimlValue::Text("alpha beta".to_string()))
        );
        assert_eq!(table.get(1, 2), Some(&NimlValue::Float(3.5)));

        let serialized = serialize_niml_ascii(&elements);
        let reparsed = parse_niml_str(&serialized).unwrap();
        assert_eq!(reparsed, elements);
    }

    #[test]
    fn canonical_dataset_conversion_builds_sparse_dataset() {
        let domain = SurfaceDomain::from_triangles(20, vec![[0, 1, 2]]).unwrap();
        let payload = read_niml_dset_str(
            r#"
<AFNI_dataset ni_form="ni_group" dset_type="Node_Bucket" self_idcode="XYZ_DATA" filename="toy.niml.dset" >
<SPARSE_DATA ni_type="float,float" ni_dimen="2" data_type="Node_Bucket_data" >
1.5 2.5
3.5 4.5
</SPARSE_DATA>
<INDEX_LIST ni_type="int" ni_dimen="2" data_type="Node_Bucket_node_indices" >
10
12
</INDEX_LIST>
<AFNI_atr ni_type="String" ni_dimen="1" atr_name="COLMS_LABS" >effect;Tstat;</AFNI_atr>
<AFNI_atr ni_type="String" ni_dimen="1" atr_name="COLMS_TYPE" >Generic_Float;Generic_Float;</AFNI_atr>
<AFNI_atr ni_type="String" ni_dimen="1" atr_name="COLMS_STATSYM" >none;Ttest(10);</AFNI_atr>
</AFNI_dataset>
"#,
        )
        .unwrap();

        let dataset = payload.to_dataset(&domain).unwrap();

        assert_eq!(dataset.kind, DatasetKind::SurfaceScalar);
        assert_eq!(dataset.node_indices, Some(vec![10, 12]));
        assert_eq!(
            dataset.parent_ids.source_dataset_id.as_deref(),
            Some("XYZ_DATA")
        );
        assert_eq!(dataset.columns[0].label, "effect");
        assert_eq!(dataset.columns[0].role, ColumnRole::Intensity);
        assert_eq!(dataset.columns[1].role, ColumnRole::Statistic);
        assert_eq!(dataset.columns[1].stat.as_deref(), Some("Ttest(10)"));
        match &dataset.columns[0].values {
            ColumnData::Float32(values) => assert_eq!(values, &vec![1.5, 3.5]),
            values => panic!("unexpected column data: {values:?}"),
        }
    }

    #[test]
    fn canonical_dataset_conversion_builds_dense_dataset_without_index_list() {
        let domain = SurfaceDomain::from_triangles(2, vec![[0, 1, 0]]).unwrap();
        let payload = NimlDatasetPayload {
            dset_type: "Node_Bucket".to_string(),
            self_idcode: Some("XYZ_DENSE".to_string()),
            filename: None,
            label: None,
            sparse_data: Some(
                NimlNumericMatrix::from_rows(
                    vec![NimlValueType::Int32],
                    vec![vec![1.0], vec![2.0]],
                )
                .unwrap(),
            ),
            node_indices: None,
            column_ranges: Vec::new(),
            column_labels: vec!["roi".to_string()],
            column_types: vec!["ROI_Label".to_string()],
            column_stats: vec!["none".to_string()],
            history: None,
        };

        let dataset = payload.to_dataset(&domain).unwrap();

        assert_eq!(dataset.kind, DatasetKind::SurfaceLabel);
        assert!(!dataset.is_sparse());
        assert_eq!(dataset.columns[0].role, ColumnRole::Label);
        match &dataset.columns[0].values {
            ColumnData::Int32(values) => assert_eq!(values, &vec![1, 2]),
            values => panic!("unexpected column data: {values:?}"),
        }
    }

    #[test]
    fn mixed_table_constructor_validates_value_count() {
        let error = NimlMixedTable::new(
            vec![NimlValueType::Int32, NimlValueType::String],
            2,
            vec![NimlValue::Integer(1)],
        )
        .unwrap_err();

        assert!(error.to_string().contains("expected 4"));
    }

    #[test]
    fn commented_node_roi_reads_suma_datum_records() {
        let rois = read_niml_roi_str(super::MINIMAL_ROI_SAMPLE).unwrap();

        assert_eq!(rois.len(), 1);
        assert_eq!(rois[0].self_idcode.as_deref(), Some("XYZ_ROI"));
        assert_eq!(rois[0].parent_side, SurfaceSide::Left);
        assert_eq!(rois[0].label, "V1");
        assert_eq!(rois[0].integer_label, 7);
        assert_eq!(
            rois[0].fill_color,
            Some(Rgba::new_unchecked(0.5, 0.1, 0.9, 1.0))
        );
        assert_eq!(rois[0].records[0].action_code, 1);
        assert_eq!(rois[0].records[0].element_type_code, 4);
        assert_eq!(rois[0].records[0].node_path, vec![1, 2, 3]);
    }

    #[test]
    fn roi_payload_converts_to_roi_model() {
        let payload = read_niml_roi_str(super::MINIMAL_ROI_SAMPLE)
            .unwrap()
            .remove(0);
        let roi = payload.to_roi().unwrap();

        assert_eq!(roi.id.as_str(), "XYZ_ROI");
        assert_eq!(roi.label, "V1");
        assert_eq!(roi.parent_side, SurfaceSide::Left);
        assert_eq!(roi.drawing_type, RoiDrawingType::Collection);
        assert_eq!(roi.data[0].kind, RoiElementKind::NodeSegment);
        assert_eq!(roi.data[0].action, RoiBrushAction::AppendStroke);
        assert_eq!(roi.unique_nodes(), vec![1, 2, 3, 8, 9]);
    }

    #[test]
    fn roi_payload_roundtrips_through_ascii_niml_element() {
        let payload = read_niml_roi_str(super::MINIMAL_ROI_SAMPLE)
            .unwrap()
            .remove(0);
        let serialized = serialize_niml_ascii(&[payload.to_element()]);
        let reparsed = read_niml_roi_str(&serialized).unwrap();

        assert_eq!(reparsed[0].label, "V1");
        assert_eq!(reparsed[0].records, payload.records);
    }

    #[test]
    fn drawn_roi_exports_suma_edge_join_and_fill_records() {
        let roi = Roi::new("drawn", 3)
            .unwrap()
            .with_source(RoiSource::Drawn, None)
            .unwrap()
            .with_drawing_type(RoiDrawingType::FilledArea)
            .with_data(vec![
                RoiDatum::node_segment(vec![1, 2, 3], RoiBrushAction::AppendStroke).unwrap(),
                RoiDatum::node_segment(vec![3, 4, 1], RoiBrushAction::JoinEnds).unwrap(),
                RoiDatum::new(
                    RoiElementKind::NodeGroup,
                    RoiBrushAction::FillArea,
                    vec![1, 2, 3, 4, 5],
                    Vec::new(),
                )
                .unwrap(),
            ])
            .unwrap();
        let payload = super::NimlRoiPayload::from_roi(&roi);

        assert_eq!(payload.records.len(), 3);
        assert_eq!(
            payload
                .records
                .iter()
                .map(|record| (record.action_code, record.element_type_code))
                .collect::<Vec<_>>(),
            vec![(1, 4), (3, 4), (4, 1)]
        );

        let serialized = serialize_niml_ascii(&[payload.to_element()]);
        let reparsed = read_niml_roi_str(&serialized).unwrap();
        assert_eq!(reparsed[0].label, "drawn");
        assert_eq!(reparsed[0].records[1].action_code, 3);
        assert_eq!(reparsed[0].records[2].node_path, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn reference_contract_checks_cover_c_matlab_python_alignment() {
        let checks = niml_reference_checks();

        assert_eq!(checks.len(), 3);
        assert!(checks.iter().all(|check| check.passed), "{checks:?}");
        assert!(checks.iter().all(|check| !check.references.is_empty()));
    }

    #[test]
    fn numeric_matrix_rejects_wrong_value_count() {
        let error = NimlNumericMatrix::new(vec![NimlValueType::Float32], 2, vec![1.0]).unwrap_err();

        assert!(error.to_string().contains("expected 2"));
    }

    #[test]
    fn dataset_payload_requires_sparse_data_before_writing() {
        let payload = NimlDatasetPayload {
            dset_type: "Node_Bucket".to_string(),
            self_idcode: None,
            filename: None,
            label: None,
            sparse_data: None,
            node_indices: None,
            column_ranges: Vec::new(),
            column_labels: Vec::new(),
            column_types: Vec::new(),
            column_stats: Vec::new(),
            history: None,
        };

        assert!(
            payload
                .to_element()
                .unwrap_err()
                .to_string()
                .contains("SPARSE_DATA")
        );
    }

    #[test]
    fn parser_preserves_unknown_text_elements() {
        let elements = parse_niml_str(
            r#"<AFNI_atr ni_type="String" ni_dimen="1" atr_name="NOTE" >hello &lt;sumaru&gt;</AFNI_atr>"#,
        )
        .unwrap();

        let NimlData::Text(text) = &elements[0].data else {
            panic!("expected text");
        };
        assert_eq!(text, "hello <sumaru>");
    }

    #[test]
    fn explicit_element_constructors_make_serializable_groups() {
        let matrix =
            NimlNumericMatrix::from_rows(vec![NimlValueType::Int32], vec![vec![1.0]]).unwrap();
        let child = NimlElement::numeric("INDEX_LIST", Default::default(), matrix);
        let root = NimlElement::group("AFNI_dataset", Default::default(), vec![child]);
        let text = serialize_niml_ascii(&[root]);

        assert!(text.contains("ni_form=\"ni_group\""));
        assert!(text.contains("ni_type=\"int\""));
    }
}
