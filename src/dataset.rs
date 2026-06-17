use anyhow::{Result, ensure};

use crate::stats::normal_two_tailed_p_value;
use crate::surface::{SurfaceDomain, SurfaceDomainId};

#[derive(Debug, Clone, PartialEq)]
pub struct Dataset {
    pub kind: DatasetKind,
    pub domain_id: SurfaceDomainId,
    pub row_count: usize,
    pub node_indices: Option<Vec<u32>>,
    pub columns: Vec<DataColumn>,
    pub parent_ids: DatasetParentIds,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatasetKind {
    SurfaceScalar,
    SurfaceLabel,
    SurfaceTimeSeries,
    Roi,
    Unknown,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DatasetParentIds {
    pub source_dataset_id: Option<String>,
    pub domain_parent_id: Option<String>,
    pub surface_parent_id: Option<String>,
    pub volume_parent_id: Option<String>,
    pub originator_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataColumn {
    pub label: String,
    pub role: ColumnRole,
    pub units: Option<String>,
    pub stat: Option<String>,
    pub fdr_curve: Option<AfniFdrCurve>,
    pub values: ColumnData,
    pub range: Option<ColumnRange>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AfniFdrCurve {
    pub x0: f64,
    pub dx: f64,
    pub samples: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnRole {
    NodeIndex,
    Intensity,
    Threshold,
    Brightness,
    Label,
    Statistic,
    TimePoint,
    Mask,
    Unknown,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnData {
    UInt32(Vec<u32>),
    Int32(Vec<i32>),
    Float32(Vec<f32>),
    Float64(Vec<f64>),
    Text(Vec<String>),
}

/// A closed `[min, max]` interval over data values. Shared by data columns and
/// the overlay intensity/threshold ranges (formerly a separate `OverlayRange`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnRange {
    pub min: f64,
    pub max: f64,
}

impl ColumnRange {
    pub(crate) fn contains(&self, value: f64) -> bool {
        value >= self.min && value <= self.max
    }

    pub(crate) fn normalized(&self, value: f64) -> f64 {
        if (self.max - self.min).abs() <= f64::EPSILON {
            0.5
        } else {
            ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        }
    }

    pub(crate) fn validate(&self, label: &str) -> Result<()> {
        ensure!(
            self.min.is_finite() && self.max.is_finite(),
            "{label} must contain finite values"
        );
        ensure!(self.min <= self.max, "{label} min is greater than max");
        Ok(())
    }
}

impl Dataset {
    pub fn dense(
        kind: DatasetKind,
        domain: &SurfaceDomain,
        columns: Vec<DataColumn>,
    ) -> Result<Self> {
        let row_count = dataset_row_count(&columns)?;
        ensure!(
            row_count == domain.node_count,
            "dense dataset has {} rows but domain has {} nodes",
            row_count,
            domain.node_count
        );

        Ok(Self {
            kind,
            domain_id: domain.id.clone(),
            row_count,
            node_indices: None,
            columns,
            parent_ids: DatasetParentIds::default(),
        })
    }

    pub fn sparse(
        kind: DatasetKind,
        domain: &SurfaceDomain,
        node_indices: Vec<u32>,
        columns: Vec<DataColumn>,
    ) -> Result<Self> {
        let row_count = dataset_row_count(&columns)?;
        ensure!(
            row_count == node_indices.len(),
            "sparse dataset has {} rows but {} node indices",
            row_count,
            node_indices.len()
        );

        for node in &node_indices {
            ensure!(
                (*node as usize) < domain.node_count,
                "dataset references node {} outside domain node count {}",
                node,
                domain.node_count
            );
        }

        Ok(Self {
            kind,
            domain_id: domain.id.clone(),
            row_count,
            node_indices: Some(node_indices),
            columns,
            parent_ids: DatasetParentIds::default(),
        })
    }

    pub fn with_parent_ids(mut self, parent_ids: DatasetParentIds) -> Self {
        self.parent_ids = parent_ids;
        self
    }

    pub fn is_sparse(&self) -> bool {
        self.node_indices.is_some()
    }

    pub fn node_for_row(&self, row: usize) -> Option<u32> {
        if row >= self.row_count {
            return None;
        }

        self.node_indices
            .as_ref()
            .map_or(Some(row as u32), |indices| indices.get(row).copied())
    }

    pub fn column(&self, label: &str) -> Option<&DataColumn> {
        self.columns.iter().find(|column| column.label == label)
    }

    pub fn columns_for_role(&self, role: ColumnRole) -> impl Iterator<Item = &DataColumn> {
        self.columns
            .iter()
            .filter(move |column| column.role == role)
    }
}

impl DataColumn {
    pub fn new(
        label: impl Into<String>,
        role: ColumnRole,
        units: Option<String>,
        values: ColumnData,
    ) -> Result<Self> {
        let label = label.into();
        ensure!(!label.trim().is_empty(), "dataset column label is empty");
        ensure!(!values.is_empty(), "dataset column {label} has no rows");

        let range = values.range();

        Ok(Self {
            label,
            role,
            units,
            stat: None,
            fdr_curve: None,
            values,
            range,
        })
    }

    pub fn with_stat(mut self, stat: Option<String>) -> Self {
        self.stat = stat.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("none"))
                .then(|| trimmed.to_string())
        });
        self
    }

    pub fn with_fdr_curve(mut self, curve: Option<AfniFdrCurve>) -> Self {
        self.fdr_curve = curve;
        self
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl AfniFdrCurve {
    pub fn new(x0: f64, dx: f64, samples: Vec<f64>) -> Result<Self> {
        ensure!(x0.is_finite(), "FDR curve x0 must be finite");
        ensure!(
            dx.is_finite() && dx.abs() > f64::EPSILON,
            "FDR curve dx must be non-zero"
        );
        ensure!(!samples.is_empty(), "FDR curve has no samples");
        ensure!(
            samples.iter().all(|value| value.is_finite()),
            "FDR curve contains non-finite samples"
        );

        Ok(Self { x0, dx, samples })
    }

    pub fn from_afni_values(values: &[f64]) -> Result<Self> {
        ensure!(
            values.len() >= 3,
            "AFNI FDR curve needs x0, dx, and at least one sample"
        );
        Self::new(values[0], values[1], values[2..].to_vec())
    }

    pub fn to_afni_values(&self) -> Vec<f64> {
        let mut values = Vec::with_capacity(self.samples.len() + 2);
        values.push(self.x0);
        values.push(self.dx);
        values.extend_from_slice(&self.samples);
        values
    }

    pub fn z_value(&self, threshold: f64) -> Option<f64> {
        if !threshold.is_finite() {
            return None;
        }
        let last = self.samples.len().checked_sub(1)?;
        if last == 0 {
            return self.samples.first().copied();
        }

        let x = threshold.abs();
        let position = (x - self.x0) / self.dx;
        if position <= 0.0 {
            return self.samples.first().copied();
        }
        if position >= last as f64 {
            return self.samples.last().copied();
        }

        let ix = position.floor() as usize;
        let t = position - ix as f64;
        let left = self.samples[ix];
        let right = self.samples[ix + 1];
        let lo = left.min(right);
        let hi = left.max(right);

        let y0 = self.samples[ix.saturating_sub(1)];
        let y1 = left;
        let y2 = right;
        let y3 = self.samples[(ix + 2).min(last)];
        let t2 = t * t;
        let t3 = t2 * t;
        let cubic = 0.5
            * ((2.0 * y1)
                + (-y0 + y2) * t
                + (2.0 * y0 - 5.0 * y1 + 4.0 * y2 - y3) * t2
                + (-y0 + 3.0 * y1 - 3.0 * y2 + y3) * t3);

        Some(cubic.clamp(lo, hi))
    }

    pub fn q_value(&self, threshold: f64) -> Option<f64> {
        let z = self.z_value(threshold)?;
        (z > 0.0)
            .then(|| normal_two_tailed_p_value(z))
            .flatten()
            .or(Some(1.0))
    }
}

impl ColumnData {
    pub fn len(&self) -> usize {
        match self {
            Self::UInt32(values) => values.len(),
            Self::Int32(values) => values.len(),
            Self::Float32(values) => values.len(),
            Self::Float64(values) => values.len(),
            Self::Text(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn range(&self) -> Option<ColumnRange> {
        match self {
            Self::UInt32(values) => values
                .iter()
                .copied()
                .map(|value| value as f64)
                .fold(None, range_step),
            Self::Int32(values) => values
                .iter()
                .copied()
                .map(|value| value as f64)
                .fold(None, range_step),
            Self::Float32(values) => values
                .iter()
                .copied()
                .filter(|value| value.is_finite())
                .map(|value| value as f64)
                .fold(None, range_step),
            Self::Float64(values) => values
                .iter()
                .copied()
                .filter(|value| value.is_finite())
                .fold(None, range_step),
            Self::Text(_) => None,
        }
    }
}

fn dataset_row_count(columns: &[DataColumn]) -> Result<usize> {
    ensure!(!columns.is_empty(), "dataset has no columns");
    let row_count = columns[0].len();

    for column in columns {
        ensure!(
            column.len() == row_count,
            "dataset column {} has {} rows but expected {}",
            column.label,
            column.len(),
            row_count
        );
    }

    Ok(row_count)
}

fn range_step(range: Option<ColumnRange>, value: f64) -> Option<ColumnRange> {
    Some(match range {
        Some(range) => ColumnRange {
            min: range.min.min(value),
            max: range.max.max(value),
        },
        None => ColumnRange {
            min: value,
            max: value,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ColumnData, ColumnRange, ColumnRole, DataColumn, Dataset, DatasetKind, DatasetParentIds,
    };
    use crate::surface::SurfaceDomain;

    #[test]
    fn dense_dataset_attaches_to_full_surface_domain() {
        let domain = triangle_domain();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![
                DataColumn::new(
                    "beta",
                    ColumnRole::Intensity,
                    Some("a.u.".to_string()),
                    ColumnData::Float32(vec![1.0, 2.0, 3.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap();

        assert_eq!(dataset.domain_id, domain.id);
        assert_eq!(dataset.row_count, 3);
        assert!(!dataset.is_sparse());
        assert_eq!(dataset.node_for_row(2), Some(2));
        assert_eq!(
            dataset.column("beta").unwrap().range,
            Some(ColumnRange { min: 1.0, max: 3.0 })
        );
    }

    #[test]
    fn sparse_dataset_maps_rows_to_nodes() {
        let domain = SurfaceDomain::from_triangles(10, vec![[0, 1, 2]]).unwrap();
        let dataset = Dataset::sparse(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![2, 5],
            vec![
                DataColumn::new(
                    "t-stat",
                    ColumnRole::Statistic,
                    None,
                    ColumnData::Float64(vec![4.0, -3.5]),
                )
                .unwrap(),
            ],
        )
        .unwrap();

        assert!(dataset.is_sparse());
        assert_eq!(dataset.row_count, 2);
        assert_eq!(dataset.node_for_row(0), Some(2));
        assert_eq!(dataset.node_for_row(1), Some(5));
        assert_eq!(dataset.node_for_row(2), None);
    }

    #[test]
    fn sparse_dataset_rejects_node_indices_outside_domain() {
        let domain = triangle_domain();
        let error = Dataset::sparse(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![0, 5],
            vec![
                DataColumn::new(
                    "value",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![1.0, 2.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap_err();

        assert!(error.to_string().contains("outside domain node count"));
    }

    #[test]
    fn dataset_requires_columns_to_have_same_row_count() {
        let domain = triangle_domain();
        let error = Dataset::dense(
            DatasetKind::SurfaceTimeSeries,
            &domain,
            vec![
                DataColumn::new(
                    "time_001",
                    ColumnRole::TimePoint,
                    None,
                    ColumnData::Float32(vec![1.0, 2.0, 3.0]),
                )
                .unwrap(),
                DataColumn::new(
                    "time_002",
                    ColumnRole::TimePoint,
                    None,
                    ColumnData::Float32(vec![1.0, 2.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap_err();

        assert!(error.to_string().contains("expected 3"));
    }

    #[test]
    fn dataset_supports_multiple_roles_and_parent_ids() {
        let domain = triangle_domain();
        let parents = DatasetParentIds {
            source_dataset_id: Some("stats.niml.dset".to_string()),
            domain_parent_id: Some(domain.id.as_str().to_string()),
            surface_parent_id: Some("surface-abc".to_string()),
            volume_parent_id: None,
            originator_id: Some("afni".to_string()),
        };
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![
                DataColumn::new(
                    "effect",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![0.1, 0.2, 0.3]),
                )
                .unwrap(),
                DataColumn::new(
                    "threshold",
                    ColumnRole::Threshold,
                    None,
                    ColumnData::Float32(vec![2.1, 2.2, 2.3]),
                )
                .unwrap(),
                DataColumn::new(
                    "label",
                    ColumnRole::Label,
                    None,
                    ColumnData::UInt32(vec![1, 2, 2]),
                )
                .unwrap(),
            ],
        )
        .unwrap()
        .with_parent_ids(parents.clone());

        assert_eq!(dataset.parent_ids, parents);
        assert_eq!(dataset.columns_for_role(ColumnRole::Threshold).count(), 1);
        assert_eq!(
            dataset.column("label").unwrap().range,
            Some(ColumnRange { min: 1.0, max: 2.0 })
        );
    }

    #[test]
    fn text_columns_have_no_numeric_range() {
        let column = DataColumn::new(
            "region",
            ColumnRole::Label,
            None,
            ColumnData::Text(vec!["V1".to_string(), "V2".to_string()]),
        )
        .unwrap();

        assert_eq!(column.range, None);
    }

    #[test]
    fn float_ranges_ignore_non_finite_values() {
        let column = DataColumn::new(
            "value",
            ColumnRole::Intensity,
            None,
            ColumnData::Float32(vec![f32::NAN, -1.0, f32::INFINITY, 3.0]),
        )
        .unwrap();

        assert_eq!(
            column.range,
            Some(ColumnRange {
                min: -1.0,
                max: 3.0
            })
        );
    }

    fn triangle_domain() -> SurfaceDomain {
        SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap()
    }
}
