use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};

use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind, DatasetParentIds};
use crate::surface::SurfaceDomain;

const STC_ORIGINATOR_PREFIX: &str = "MNE_STC:";

#[derive(Debug, Clone, PartialEq)]
pub struct StcData {
    pub tmin_seconds: f64,
    pub tstep_seconds: f64,
    pub vertices: Vec<u32>,
    /// One source-value vector per time point.
    pub time_points: Vec<Vec<f32>>,
}

pub fn read_stc(path: impl AsRef<Path>) -> Result<StcData> {
    let path = path.as_ref();
    let bytes =
        fs::read(path).with_context(|| format!("failed to read STC file {}", path.display()))?;
    parse_stc(&bytes).with_context(|| format!("failed to parse STC file {}", path.display()))
}

pub fn read_stc_dataset(path: impl AsRef<Path>, domain: &SurfaceDomain) -> Result<Dataset> {
    let path = path.as_ref();
    let stc = read_stc(path)?;
    let columns = stc
        .time_points
        .into_iter()
        .enumerate()
        .map(|(index, values)| {
            let time = stc.tmin_seconds + index as f64 * stc.tstep_seconds;
            DataColumn::new(
                format!("time={:.6} s", time),
                ColumnRole::TimePoint,
                None,
                ColumnData::Float32(values),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let source_name = path
        .file_name()
        .map(|name| format!("{STC_ORIGINATOR_PREFIX}{}", name.to_string_lossy()))
        .or_else(|| Some(STC_ORIGINATOR_PREFIX.to_string()));
    let parent_ids = DatasetParentIds {
        originator_id: source_name,
        ..DatasetParentIds::default()
    };

    Dataset::sparse(
        DatasetKind::SurfaceTimeSeries,
        domain,
        stc.vertices,
        columns,
    )
    .map(|dataset| {
        dataset
            .with_time_start_seconds(Some(stc.tmin_seconds))
            .with_time_step_seconds(Some(stc.tstep_seconds))
            .with_parent_ids(parent_ids)
    })
}

pub fn dataset_is_stc(dataset: &Dataset) -> bool {
    dataset
        .parent_ids
        .originator_id
        .as_deref()
        .is_some_and(|originator| originator.starts_with(STC_ORIGINATOR_PREFIX))
}

pub fn mark_dataset_as_paired_stc(dataset: Dataset) -> Dataset {
    let mut parent_ids = dataset.parent_ids.clone();
    parent_ids.originator_id = Some(format!("{STC_ORIGINATOR_PREFIX}paired"));
    dataset.with_parent_ids(parent_ids)
}

pub fn parse_stc(bytes: &[u8]) -> Result<StcData> {
    let mut reader = BigEndianReader::new(bytes);
    let tmin_ms = reader.read_f32("start time")?;
    let tstep_ms = reader.read_f32("time step")?;
    ensure!(tmin_ms.is_finite(), "STC start time is not finite");
    ensure!(
        tstep_ms.is_finite() && tstep_ms > 0.0,
        "STC time step must be finite and positive"
    );

    let vertex_count = reader.read_u32("vertex count")? as usize;
    ensure!(vertex_count > 0, "STC file has no vertices");
    let mut vertices = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        vertices.push(reader.read_u32("vertex index")?);
    }

    let time_count = reader.read_u32("time-point count")? as usize;
    ensure!(time_count > 0, "STC file has no time points");
    let value_count = vertex_count
        .checked_mul(time_count)
        .context("STC data dimensions overflow")?;
    let byte_count = value_count
        .checked_mul(4)
        .context("STC byte count overflow")?;
    ensure!(
        reader.remaining() == byte_count,
        "STC data has {} bytes after its header, expected {byte_count}",
        reader.remaining()
    );

    // Classic STC stores time-major data. Keeping each time point contiguous
    // matches Sumaru's column-oriented Dataset without a full transpose buffer.
    let mut time_points = Vec::with_capacity(time_count);
    for _ in 0..time_count {
        let mut values = Vec::with_capacity(vertex_count);
        for _ in 0..vertex_count {
            values.push(reader.read_f32("source value")?);
        }
        time_points.push(values);
    }

    Ok(StcData {
        tmin_seconds: f64::from(tmin_ms) / 1000.0,
        tstep_seconds: f64::from(tstep_ms) / 1000.0,
        vertices,
        time_points,
    })
}

struct BigEndianReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> BigEndianReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take<const N: usize>(&mut self, label: &str) -> Result<[u8; N]> {
        let end = self.offset.checked_add(N).context("file offset overflow")?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .with_context(|| format!("truncated STC file while reading {label}"))?;
        self.offset = end;
        Ok(slice.try_into().expect("slice length checked"))
    }

    fn read_u32(&mut self, label: &str) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(label)?))
    }

    fn read_f32(&mut self, label: &str) -> Result<f32> {
        Ok(f32::from_be_bytes(self.take(label)?))
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_stc;

    #[test]
    fn parses_time_major_stc_and_converts_milliseconds() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(-100.0_f32).to_be_bytes());
        bytes.extend_from_slice(&(10.0_f32).to_be_bytes());
        bytes.extend_from_slice(&2_u32.to_be_bytes());
        bytes.extend_from_slice(&3_u32.to_be_bytes());
        bytes.extend_from_slice(&8_u32.to_be_bytes());
        bytes.extend_from_slice(&2_u32.to_be_bytes());
        for value in [1.0_f32, 2.0, 3.0, 4.0] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }

        let stc = parse_stc(&bytes).unwrap();
        assert_eq!(stc.tmin_seconds, -0.1);
        assert_eq!(stc.tstep_seconds, 0.01);
        assert_eq!(stc.vertices, vec![3, 8]);
        assert_eq!(stc.time_points, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    }

    #[test]
    fn rejects_truncated_data() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0.0_f32.to_be_bytes());
        bytes.extend_from_slice(&1.0_f32.to_be_bytes());
        bytes.extend_from_slice(&1_u32.to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&1_u32.to_be_bytes());
        assert!(parse_stc(&bytes).is_err());
    }
}
