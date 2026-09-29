//! SUMA-compatible seed correlation preprocessing.
//!
//! SUMA's InstaCorr path removes a polynomial baseline, applies an FFT
//! bandpass to both the data and the polynomial regressors, projects the
//! filtered regressors out, and L2-normalizes every node's time series. SUMA's
//! broad `normalize_dset` switch bypasses that entire pipeline when disabled,
//! leaving a raw matrix-vector dot product. Once prepared, changing the seed
//! is only another matrix-vector dot product.

use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

use crate::dataset::{ColumnData, ColumnRole, Dataset, DatasetKind};

const MIN_BANDPASS_SAMPLES: usize = 9;
const EPSILON: f64 = 1.0e-12;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstaCorrOptions {
    pub tr_seconds: Option<f64>,
    pub bandpass_enabled: bool,
    pub low_hz: f64,
    pub high_hz: f64,
    pub polort: i32,
    pub normalize: bool,
}

impl Default for InstaCorrOptions {
    fn default() -> Self {
        Self {
            tr_seconds: None,
            bandpass_enabled: true,
            low_hz: 0.01,
            high_hz: 0.1,
            polort: 2,
            normalize: true,
        }
    }
}

impl InstaCorrOptions {
    pub fn validate(self, sample_count: usize) -> Result<()> {
        ensure!(sample_count >= 2, "InstaCorr needs at least 2 time points");
        // This deliberately reproduces SUMA's `normalize_dset` gate: when it
        // is off, the raw series are dotted and every preprocessing option is
        // ignored, so none of those options should block calculation.
        if !self.normalize {
            return Ok(());
        }
        ensure!(self.polort >= -1, "polort must be -1 or greater");
        ensure!(
            (self.polort as isize) < sample_count as isize,
            "polort {} leaves no data dimensions for {sample_count} time points",
            self.polort
        );
        if self.bandpass_enabled {
            ensure!(
                sample_count >= MIN_BANDPASS_SAMPLES,
                "SUMA bandpass preprocessing needs at least {MIN_BANDPASS_SAMPLES} time points"
            );
            let tr = self
                .tr_seconds
                .filter(|value| value.is_finite() && *value > 0.0)
                .ok_or_else(|| anyhow::anyhow!("enter a positive TR before recalculating"))?;
            ensure!(
                self.low_hz.is_finite() && self.low_hz >= 0.0,
                "low cutoff must be zero or positive"
            );
            ensure!(
                self.high_hz.is_finite() && self.high_hz > self.low_hz,
                "high cutoff must be greater than the low cutoff"
            );
            let _ = tr; // Values above Nyquist are accepted and clipped, like AFNI.
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PreparedInstaCorr {
    rows: Arc<Vec<Vec<f32>>>,
    valid_rows: Arc<Vec<bool>>,
    pub sample_count: usize,
    pub removed_dof: usize,
    pub options: InstaCorrOptions,
}

impl PreparedInstaCorr {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn correlate(&self, seed_row: usize) -> Result<Vec<f32>> {
        ensure!(
            seed_row < self.rows.len(),
            "seed vertex is not present in the time-series dataset"
        );
        ensure!(
            self.valid_rows[seed_row],
            "seed vertex has an invalid time series"
        );
        let seed = &self.rows[seed_row];
        Ok(self
            .rows
            .iter()
            .zip(self.valid_rows.iter())
            .map(|(row, valid)| {
                if !valid {
                    f32::NAN
                } else {
                    row.iter()
                        .zip(seed.iter())
                        .map(|(left, right)| left * right)
                        .sum()
                }
            })
            .collect())
    }
}

/// Extract and preprocess one row per dataset node. Sparse datasets retain
/// their sparse row layout; the viewer maps the resulting values back through
/// `Dataset::node_for_row`.
pub fn prepare_dataset(dataset: &Dataset, options: InstaCorrOptions) -> Result<PreparedInstaCorr> {
    ensure!(
        dataset.kind == DatasetKind::SurfaceTimeSeries,
        "active overlay is not a surface time-series dataset"
    );
    let time_columns = dataset
        .columns
        .iter()
        .filter(|column| column.role == ColumnRole::TimePoint)
        .collect::<Vec<_>>();
    ensure!(
        time_columns.len() >= 2,
        "surface time-series dataset has fewer than 2 time-point columns"
    );
    options.validate(time_columns.len())?;

    let mut rows = Vec::with_capacity(dataset.row_count);
    for row_index in 0..dataset.row_count {
        let mut row = Vec::with_capacity(time_columns.len());
        for column in &time_columns {
            row.push(column_value(&column.values, row_index)?);
        }
        rows.push(row);
    }
    prepare_rows(rows, options)
}

pub fn prepare_rows(
    mut rows: Vec<Vec<f64>>,
    options: InstaCorrOptions,
) -> Result<PreparedInstaCorr> {
    let sample_count = rows.first().map_or(0, Vec::len);
    options.validate(sample_count)?;
    ensure!(
        rows.iter().all(|row| row.len() == sample_count),
        "InstaCorr time-series rows have different lengths"
    );

    if !options.normalize {
        let mut valid_rows = Vec::with_capacity(rows.len());
        let prepared = rows
            .into_iter()
            .map(|mut row| {
                let valid = row.iter().all(|value| value.is_finite());
                valid_rows.push(valid);
                if !valid {
                    row.fill(0.0);
                }
                row.into_iter().map(|value| value as f32).collect()
            })
            .collect();
        return Ok(PreparedInstaCorr {
            rows: Arc::new(prepared),
            valid_rows: Arc::new(valid_rows),
            sample_count,
            removed_dof: 0,
            options,
        });
    }

    let regressors = legendre_regressors(sample_count, options.polort);
    let mut filtered_regressors = regressors.clone();
    // SUMA asks THD_bandpass_vectors for qdet=1 in addition to the selected
    // polynomial regressors. This always removes a linear trend from the
    // whole-dataset cache, including when the passband spans the full spectrum.
    for row in &mut rows {
        linear_detrend(row);
    }
    let mut removed_dof = 1 + regressors.len();
    if options.bandpass_enabled {
        let tr = options.tr_seconds.expect("validated bandpass TR");
        for row in &mut rows {
            bandpass(
                row,
                tr,
                options.low_hz,
                options.high_hz,
                !regressors.is_empty(),
            );
        }
        for regressor in &mut filtered_regressors {
            linear_detrend(regressor);
            bandpass(regressor, tr, options.low_hz, options.high_hz, false);
        }
        removed_dof += bandpass_removed_dof(sample_count, tr, options.low_hz, options.high_hz);
    }

    let basis = orthonormal_basis(filtered_regressors);
    for row in &mut rows {
        if !options.bandpass_enabled && !basis.is_empty() {
            // Without the filter, this is the usual polynomial detrend.
            project_out(row, &basis);
        } else if options.bandpass_enabled {
            // Mirrors THD_bandpass_vectors: filtered polynomial regressors are
            // projected out after bandpassing.
            project_out(row, &basis);
        }
    }

    let mut valid_rows = Vec::with_capacity(rows.len());
    let mut prepared = Vec::with_capacity(rows.len());
    for mut row in rows {
        let finite = row.iter().all(|value| value.is_finite());
        let norm = row.iter().map(|value| value * value).sum::<f64>().sqrt();
        let valid = finite && norm > EPSILON;
        if valid {
            for value in &mut row {
                *value /= norm;
            }
        }
        if !valid {
            row.fill(0.0);
        }
        prepared.push(row.into_iter().map(|value| value as f32).collect());
        valid_rows.push(valid);
    }

    Ok(PreparedInstaCorr {
        rows: Arc::new(prepared),
        valid_rows: Arc::new(valid_rows),
        sample_count,
        removed_dof: removed_dof.min(sample_count.saturating_sub(1)),
        options,
    })
}

fn column_value(values: &ColumnData, row: usize) -> Result<f64> {
    let value = match values {
        ColumnData::UInt32(values) => values.get(row).copied().map(f64::from),
        ColumnData::Int32(values) => values.get(row).copied().map(f64::from),
        ColumnData::Float32(values) => values.get(row).copied().map(f64::from),
        ColumnData::Float64(values) => values.get(row).copied(),
        ColumnData::Text(_) => bail!("InstaCorr time-point columns must be numeric"),
    };
    value.ok_or_else(|| anyhow::anyhow!("time-point column is missing row {row}"))
}

fn legendre_regressors(sample_count: usize, polort: i32) -> Vec<Vec<f64>> {
    if polort < 0 {
        return Vec::new();
    }
    let x = (0..sample_count)
        .map(|index| 2.0 * index as f64 / (sample_count - 1) as f64 - 1.0)
        .collect::<Vec<_>>();
    let mut regressors = vec![vec![1.0; sample_count]];
    if polort >= 1 {
        regressors.push(x.clone());
    }
    for degree in 2..=polort as usize {
        let previous = &regressors[degree - 1];
        let before_previous = &regressors[degree - 2];
        let degree_f = degree as f64;
        regressors.push(
            (0..sample_count)
                .map(|index| {
                    ((2.0 * degree_f - 1.0) * x[index] * previous[index]
                        - (degree_f - 1.0) * before_previous[index])
                        / degree_f
                })
                .collect(),
        );
    }
    regressors
}

fn remove_mean(values: &mut [f64]) {
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    for value in values {
        *value -= mean;
    }
}

fn linear_detrend(values: &mut [f64]) {
    if values.len() < 3 {
        remove_mean(values);
        return;
    }
    let basis = orthonormal_basis(legendre_regressors(values.len(), 1));
    project_out(values, &basis);
}

fn next_even(value: usize) -> usize {
    value + value % 2
}

fn bandpass(values: &mut [f64], tr: f64, low_hz: f64, high_hz: f64, has_orts: bool) {
    let nfft = next_even(values.len());
    let nby2 = nfft / 2;
    let nhalf = nby2.saturating_sub(1);
    let df = 1.0 / (nfft as f64 * tr);
    let mut jbot = (low_hz / df).round_ties_even() as usize;
    let mut jtop = (high_hz / df).round_ties_even() as usize;
    jbot = if jbot < nhalf { jbot } else { 0 };
    jtop = jtop.min(nhalf);
    if jbot > jtop {
        jbot = 0;
        jtop = nhalf;
    }

    let mut buffer = vec![Complex::new(0.0, 0.0); nfft];
    for (slot, value) in buffer.iter_mut().zip(values.iter()) {
        slot.re = *value;
    }
    let mut planner = FftPlanner::new();
    planner.plan_fft_forward(nfft).process(&mut buffer);
    buffer[0] = Complex::ZERO;
    buffer[nby2] = Complex::ZERO;
    let taper = if has_orts { 0.05 } else { 0.5 };
    if jbot >= 1 {
        buffer[jbot] *= taper;
        buffer[nfft - jbot] *= taper;
        for index in 1..jbot {
            buffer[index] = Complex::ZERO;
            buffer[nfft - index] = Complex::ZERO;
        }
    }
    buffer[jtop] *= taper;
    buffer[nfft - jtop] *= taper;
    for index in (jtop + 1)..nby2 {
        buffer[index] = Complex::ZERO;
        buffer[nfft - index] = Complex::ZERO;
    }
    planner.plan_fft_inverse(nfft).process(&mut buffer);
    for (value, transformed) in values.iter_mut().zip(buffer.iter()) {
        *value = transformed.re / nfft as f64;
    }
}

fn bandpass_removed_dof(sample_count: usize, tr: f64, low_hz: f64, high_hz: f64) -> usize {
    let nfft = next_even(sample_count);
    let nby2 = nfft / 2;
    let nhalf = nby2.saturating_sub(1);
    let df = 1.0 / (nfft as f64 * tr);
    let qbot = (low_hz / df).round_ties_even() as usize;
    let qtop = (high_hz / df).round_ties_even() as usize;
    let mut jbot = if qbot < nhalf { qbot } else { 0 };
    let mut jtop = qtop.min(nhalf);
    if jbot > jtop {
        jbot = 0;
        jtop = nhalf;
    }
    let mut removed = 2;
    if jbot >= 1 {
        removed += 2 * jbot - 1;
    }
    removed += 2 * (nby2 - jtop) - 1;
    ((sample_count as f64 / nfft as f64) * removed as f64).round() as usize
}

fn orthonormal_basis(regressors: Vec<Vec<f64>>) -> Vec<Vec<f64>> {
    let mut basis: Vec<Vec<f64>> = Vec::new();
    for mut vector in regressors {
        project_out(&mut vector, &basis);
        let norm = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
        if norm > EPSILON {
            for value in &mut vector {
                *value /= norm;
            }
            basis.push(vector);
        }
    }
    basis
}

fn project_out(values: &mut [f64], basis: &[Vec<f64>]) {
    for vector in basis {
        let weight = values
            .iter()
            .zip(vector.iter())
            .map(|(left, right)| left * right)
            .sum::<f64>();
        for (value, component) in values.iter_mut().zip(vector.iter()) {
            *value -= weight * component;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> InstaCorrOptions {
        InstaCorrOptions {
            tr_seconds: Some(1.0),
            bandpass_enabled: false,
            low_hz: 0.01,
            high_hz: 0.1,
            polort: 0,
            normalize: true,
        }
    }

    #[test]
    fn normalized_dot_product_is_correlation() {
        let prepared = prepare_rows(
            vec![
                vec![1.0, 3.0, 2.0, 5.0, 4.0],
                vec![-1.0, -3.0, -2.0, -5.0, -4.0],
            ],
            options(),
        )
        .unwrap();
        let correlations = prepared.correlate(0).unwrap();
        assert!((correlations[0] - 1.0).abs() < 1.0e-5);
        assert!((correlations[1] + 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn constant_rows_are_invalid() {
        let prepared = prepare_rows(
            vec![vec![1.0, 3.0, 2.0, 5.0], vec![4.0, 4.0, 4.0, 4.0]],
            options(),
        )
        .unwrap();
        assert!(prepared.correlate(1).is_err());
        assert!(prepared.correlate(0).unwrap()[1].is_nan());
    }

    #[test]
    fn bandpass_requires_tr() {
        let error = InstaCorrOptions::default().validate(100).unwrap_err();
        assert!(error.to_string().contains("positive TR"));
    }

    #[test]
    fn suma_default_bandpass_keeps_normalized_dot_products_bounded() {
        let signal = (0..40)
            .map(|index| {
                let time = index as f64;
                (time * 0.2).sin() + 0.25 * (time * 0.7).cos()
            })
            .collect::<Vec<_>>();
        let inverse = signal.iter().map(|value| -*value).collect::<Vec<_>>();
        let prepared = prepare_rows(
            vec![signal, inverse],
            InstaCorrOptions {
                tr_seconds: Some(1.0),
                ..InstaCorrOptions::default()
            },
        )
        .unwrap();
        let correlations = prepared.correlate(0).unwrap();
        assert!((correlations[0] - 1.0).abs() < 1.0e-5);
        assert!((correlations[1] + 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn linear_polort_removes_linear_signal() {
        let mut selected = options();
        selected.polort = 1;
        let prepared = prepare_rows(
            vec![vec![0.0, 1.0, 2.0, 4.0, 4.0], vec![2.0, 3.0, 4.0, 6.0, 6.0]],
            selected,
        )
        .unwrap();
        assert!((prepared.correlate(0).unwrap()[1] - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn normalization_off_matches_suma_raw_dot_product_gate() {
        let options = InstaCorrOptions {
            tr_seconds: None,
            bandpass_enabled: true,
            low_hz: 10.0,
            high_hz: 1.0,
            polort: 999,
            normalize: false,
        };
        let prepared = prepare_rows(
            vec![
                vec![1.0, 2.0, 3.0],
                vec![4.0, 5.0, 6.0],
                vec![0.0, 0.0, 0.0],
            ],
            options,
        )
        .unwrap();

        assert_eq!(prepared.removed_dof, 0);
        assert_eq!(prepared.correlate(0).unwrap(), vec![14.0, 32.0, 0.0]);
        assert_eq!(prepared.correlate(2).unwrap(), vec![0.0, 0.0, 0.0]);
    }
}
