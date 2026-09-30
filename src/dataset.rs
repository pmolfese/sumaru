use anyhow::{Result, ensure};

use crate::stats::{AfniStatSpec, normal_two_tailed_p_value};
use crate::surface::{SurfaceDomain, SurfaceDomainId};

#[derive(Debug, Clone, PartialEq)]
pub struct Dataset {
    pub kind: DatasetKind,
    pub domain_id: SurfaceDomainId,
    pub row_count: usize,
    pub node_indices: Option<Vec<u32>>,
    pub columns: Vec<DataColumn>,
    /// Sampling interval for time-series columns, in seconds when known.
    pub time_step_seconds: Option<f64>,
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
            time_step_seconds: None,
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
            time_step_seconds: None,
            parent_ids: DatasetParentIds::default(),
        })
    }

    pub fn with_parent_ids(mut self, parent_ids: DatasetParentIds) -> Self {
        self.parent_ids = parent_ids;
        self
    }

    pub fn with_time_step_seconds(mut self, time_step_seconds: Option<f64>) -> Self {
        self.time_step_seconds =
            time_step_seconds.filter(|value| value.is_finite() && *value > 0.0);
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

    /// Rebuild the FDR curve AFNI normally stores alongside a statistical
    /// sub-brick. This is needed for formats such as GIFTI that preserve the
    /// statistic intent but can discard AFNI's `FDRCURVE_*` header fields.
    pub fn from_statistics(stat: &AfniStatSpec, values: &ColumnData) -> Option<Self> {
        const PMAX: f64 = 0.9999;
        const PBOT: f64 = 1.0e-15;
        const ZTOP: f64 = 9.0;
        const CURVE_SAMPLES: usize = 101;

        let statistics = numeric_column_values(values)?;
        let mut ranked = statistics
            .into_iter()
            .filter(|value| value.is_finite() && *value != 0.0)
            .filter_map(|value| {
                stat.two_sided_p_value(value.abs())
                    .filter(|p| *p >= 0.0 && *p < PMAX)
                    .map(|p| (p.max(PBOT), value.abs()))
            })
            .collect::<Vec<_>>();
        if ranked.len() <= 19 {
            return None;
        }
        ranked.sort_by(|left, right| left.0.total_cmp(&right.0));

        let count = ranked.len();
        let mut q_values = vec![1.0; count];
        let mut q_min = 1.0_f64;
        let mut has_small_q_after_first = false;
        for index in (0..count).rev() {
            let q = ((count as f64 * ranked[index].0) / (index as f64 + 1.0)).min(q_min);
            q_min = q;
            q_values[index] = q;
            has_small_q_after_first |= index > 0 && q <= 0.15;
        }

        if has_small_q_after_first
            && ranked[0].0 > 0.0
            && let Some(true_positive_count) = estimate_afni_true_positive_count(&ranked)
            && true_positive_count > 0
        {
            let mut factor = (count - true_positive_count) as f64 / count as f64;
            if factor < 0.5 {
                factor = 0.25 + factor * factor;
            }
            for q in &mut q_values {
                *q *= factor;
            }
        }

        let mut curve_points = ranked
            .into_iter()
            .zip(q_values)
            .filter_map(|((_, statistic), q)| {
                let z = normal_statistic_for_two_tailed_p_value(q)?;
                (z > 0.0).then_some((z, statistic))
            })
            .collect::<Vec<_>>();
        if curve_points.len() < 9 {
            return None;
        }
        curve_points.sort_by(|left, right| left.0.total_cmp(&right.0));

        let mut last = curve_points.len() - 1;
        while last > 0 && curve_points[last].0 >= ZTOP {
            last -= 1;
        }
        if last == 0 {
            return None;
        }
        if last < curve_points.len() - 1 {
            last += 1;
        }

        let x0 = curve_points[0].1;
        let top = curve_points[last].1;
        let dx = (top - x0) / (CURVE_SAMPLES - 1) as f64;
        if !dx.is_finite() || dx.abs() <= f64::EPSILON {
            return None;
        }

        let mut samples = Vec::with_capacity(CURVE_SAMPLES);
        samples.push(curve_points[0].0);
        let mut point = 1;
        for sample_index in 1..CURVE_SAMPLES - 1 {
            let threshold = x0 + sample_index as f64 * dx;
            while point < curve_points.len() && curve_points[point].1 < threshold {
                point += 1;
            }
            if point >= curve_points.len() {
                return None;
            }
            let (left_z, left_stat) = curve_points[point - 1];
            let (right_z, right_stat) = curve_points[point];
            let fraction = if (right_stat - left_stat).abs() <= f64::EPSILON {
                0.0
            } else {
                (threshold - left_stat) / (right_stat - left_stat)
            };
            samples.push(left_z + fraction * (right_z - left_z));
        }
        samples.push(curve_points[last].0);

        Self::new(x0, dx, samples).ok()
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

fn numeric_column_values(values: &ColumnData) -> Option<Vec<f64>> {
    match values {
        ColumnData::UInt32(values) => Some(values.iter().map(|value| *value as f64).collect()),
        ColumnData::Int32(values) => Some(values.iter().map(|value| *value as f64).collect()),
        ColumnData::Float32(values) => Some(values.iter().map(|value| *value as f64).collect()),
        ColumnData::Float64(values) => Some(values.clone()),
        ColumnData::Text(_) => None,
    }
}

fn estimate_afni_true_positive_count(ranked: &[(f64, f64)]) -> Option<usize> {
    if ranked.len() < 233 {
        return None;
    }

    let mut histogram = [0_usize; 16];
    let mut histogram_count = 0;
    for (p, _) in ranked {
        let bin = ((*p - 0.15) * 20.0) as isize;
        if (0..16).contains(&bin) {
            histogram[bin as usize] += 1;
            histogram_count += 1;
        }
    }
    if histogram_count < 160 {
        return None;
    }
    histogram.sort_unstable();

    let count = ranked.len() as f64;
    let estimate_four = count
        - 20.0 * (histogram[6] + 2 * histogram[7] + 2 * histogram[8] + histogram[9]) as f64 / 6.0;
    let estimate_six = count
        - 20.0
            * (histogram[5]
                + 2 * histogram[6]
                + 2 * histogram[7]
                + 2 * histogram[8]
                + 2 * histogram[9]
                + histogram[10]) as f64
            / 10.0;
    let estimate = estimate_four.min(estimate_six).trunc();
    (estimate >= 0.0).then_some(estimate as usize)
}

fn normal_statistic_for_two_tailed_p_value(p: f64) -> Option<f64> {
    if !p.is_finite() || !(0.0..=1.0).contains(&p) {
        return None;
    }
    if p == 1.0 {
        return Some(0.0);
    }
    if p == 0.0 {
        return Some(9.0);
    }

    let mut low = 0.0_f64;
    let mut high = 9.0_f64;
    for _ in 0..64 {
        let midpoint = (low + high) * 0.5;
        if normal_two_tailed_p_value(midpoint)? <= p {
            high = midpoint;
        } else {
            low = midpoint;
        }
    }
    Some(high)
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
