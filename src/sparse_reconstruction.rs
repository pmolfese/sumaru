// Developed by P. Molfese, National Institutes of Health (NIH).
// See LICENSE for the public-domain notice, warranty disclaimer, and attribution.

//! Sparse reconstruction on a supplied full parent mesh. NaN represents missing data.
//! Nearest uses edge hops; smoothing averages sampled vertices too and uses nearest
//! fallback for reachable vertices outside the requested smoothing rings.
use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind};
use crate::surface::SurfaceMesh;
use anyhow::{Result, ensure};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SparseFillMethod {
    SampledOnly,
    Nearest,
    #[default]
    Smooth,
}

#[derive(Debug)]
pub enum SparseProjection {
    Smooth(SparseUpsamplingMatrix),
    Nearest(NearestSourceProjection),
}
impl SparseProjection {
    pub fn source_nodes(&self) -> &[u32] {
        match self {
            Self::Smooth(matrix) => &matrix.source_nodes,
            Self::Nearest(matrix) => &matrix.source_nodes,
        }
    }

    pub fn sample_node(
        &self,
        node: usize,
        mut source: impl FnMut(usize) -> Option<f32>,
    ) -> Option<f32> {
        match self {
            Self::Smooth(matrix) => matrix.sample_node(node, source),
            Self::Nearest(matrix) => {
                source((*matrix.source_for_node.get(node)?)? as usize).filter(|v| v.is_finite())
            }
        }
    }
    pub fn apply_dense_seeds(&self, values: &[f32]) -> Result<Vec<f32>> {
        match self {
            Self::Smooth(matrix) => matrix.apply_dense_seeds(values),
            Self::Nearest(matrix) => matrix.apply_dense_seeds(values),
        }
    }
}

fn validate_inputs(neighbors: &[Vec<u32>], sources: &[u32]) -> Result<()> {
    ensure!(!sources.is_empty(), "reconstruction has no source nodes");
    let mut seen = HashSet::new();
    for &source in sources {
        ensure!(
            (source as usize) < neighbors.len(),
            "source node outside parent topology"
        );
        ensure!(seen.insert(source), "duplicate source node {source}");
    }
    ensure!(
        neighbors
            .iter()
            .flatten()
            .all(|&n| (n as usize) < neighbors.len()),
        "neighbor outside parent topology"
    );
    Ok(())
}

/// A declared child cannot stand in for full parent connectivity. A complete
/// mapped mesh is usable because the loader has already resolved IDs locally.
pub fn has_full_parent_connectivity(mesh: &SurfaceMesh) -> bool {
    mesh.metadata.parent_mapping.as_ref().is_none_or(|mapping| {
        mapping.parent.vertex_count == Some(mesh.domain.node_count)
            && mapping.indices.len() == mesh.domain.node_count
    })
}

/// Bounded operator cache. Exact topology comparison prevents reuse after a mesh
/// change; ordered source IDs and method settings distinguish condition mappings.
#[derive(Default)]
pub struct SparseProjectionCache {
    neighbors: Vec<Vec<u32>>,
    entries: Vec<(Vec<u32>, SparseFillMethod, usize, Arc<SparseProjection>)>,
}
impl SparseProjectionCache {
    pub fn get(
        &mut self,
        neighbors: &[Vec<u32>],
        sources: &[u32],
        method: SparseFillMethod,
        steps: usize,
    ) -> Result<Option<Arc<SparseProjection>>> {
        if method == SparseFillMethod::SampledOnly {
            return Ok(None);
        }
        if self.neighbors != neighbors {
            self.entries.clear();
            self.neighbors = neighbors.to_vec();
        }
        let steps = if method == SparseFillMethod::Smooth {
            steps
        } else {
            0
        };
        if let Some(entry) = self
            .entries
            .iter()
            .find(|e| e.0 == sources && e.1 == method && e.2 == steps)
        {
            return Ok(Some(entry.3.clone()));
        }
        let projection = Arc::new(match method {
            SparseFillMethod::Smooth => {
                SparseProjection::Smooth(SparseUpsamplingMatrix::build(neighbors, sources, steps)?)
            }
            SparseFillMethod::Nearest => {
                SparseProjection::Nearest(NearestSourceProjection::build(neighbors, sources)?)
            }
            SparseFillMethod::SampledOnly => unreachable!(),
        });
        if self.entries.len() == 8 {
            self.entries.remove(0);
        }
        self.entries
            .push((sources.to_vec(), method, steps, projection.clone()));
        Ok(Some(projection))
    }

    pub fn for_dataset(
        &mut self,
        mesh: &SurfaceMesh,
        dataset: &Dataset,
        method: SparseFillMethod,
        steps: usize,
    ) -> Result<Option<Arc<SparseProjection>>> {
        ensure!(
            dataset.domain_id == mesh.domain.id,
            "dataset does not belong to target domain"
        );
        if method == SparseFillMethod::SampledOnly
            || !dataset.is_sparse()
            || dataset.row_count >= mesh.domain.node_count
            || matches!(dataset.kind, DatasetKind::SurfaceLabel | DatasetKind::Roi)
            || dataset
                .columns
                .iter()
                .any(|c| matches!(c.role, ColumnRole::Label | ColumnRole::Mask))
        {
            return Ok(None);
        }
        ensure!(
            has_full_parent_connectivity(mesh),
            "parent-based filling requires full parent connectivity; choose sampled-only for a decimated target"
        );
        self.get(
            &mesh.topology().node_neighbors,
            dataset.node_indices.as_deref().unwrap(),
            method,
            steps,
        )
    }
}

/// Build a separate display copy. The caller retains the original indexed samples.
/// Non-numeric columns are unsupported; categorical datasets must remain sampled-only.
pub fn reconstruct_dataset(
    dataset: &Dataset,
    node_count: usize,
    projection: &SparseProjection,
) -> Result<Dataset> {
    ensure!(
        !matches!(dataset.kind, DatasetKind::SurfaceLabel | DatasetKind::Roi),
        "categorical data cannot be reconstructed"
    );
    let nodes = dataset
        .node_indices
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("expected indexed samples"))?;
    ensure!(
        nodes == projection.source_nodes(),
        "projection source order differs from dataset indices"
    );
    let mut output = dataset.clone();
    for column in &mut output.columns {
        ensure!(
            !matches!(column.role, ColumnRole::Label | ColumnRole::Mask),
            "categorical columns cannot be reconstructed"
        );
        let values: Vec<f32> = match &column.values {
            ColumnData::Float32(v) => v.clone(),
            ColumnData::Float64(v) => v.iter().map(|v| *v as f32).collect(),
            ColumnData::Int32(v) => v.iter().map(|v| *v as f32).collect(),
            ColumnData::UInt32(v) => v.iter().map(|v| *v as f32).collect(),
            ColumnData::Text(_) => anyhow::bail!("cannot reconstruct text columns"),
        };
        let mut dense = vec![f32::NAN; node_count];
        ensure!(
            nodes.len() == values.len(),
            "sample count differs from indices"
        );
        for (&node, value) in nodes.iter().zip(values) {
            ensure!((node as usize) < node_count, "sample outside target");
            dense[node as usize] = value;
        }
        let values = if column.role == ColumnRole::NodeIndex {
            (0..node_count).map(|n| n as f32).collect()
        } else {
            projection.apply_dense_seeds(&dense)?
        };
        *column = DataColumn::new(
            column.label.clone(),
            column.role.clone(),
            column.units.clone(),
            ColumnData::Float32(values),
        )?
        .with_stat(column.stat.clone())
        .with_fdr_curve(column.fdr_curve.clone());
    }
    output.row_count = node_count;
    output.node_indices = None;
    output.parent_mapping = None;
    Ok(output)
}

/// CSR source-to-surface operator matching MNE's `_surf_upsampling_mat`:
/// adjacency includes each node itself, active rows expand one ring per step,
/// and every output row is normalized after each multiplication.
#[derive(Debug)]
pub struct SparseUpsamplingMatrix {
    /// Requested ring expansion count. Uncovered reachable nodes use nearest fallback.
    pub smoothing_steps: usize,
    /// Number of output vertices assigned by the nearest fallback.
    pub nearest_fallback_nodes: usize,
    source_nodes: Vec<u32>,
    row_offsets: Vec<usize>,
    column_indices: Vec<u32>,
    weights: Vec<f32>,
}

impl SparseUpsamplingMatrix {
    pub fn build(neighbors: &[Vec<u32>], source_nodes: &[u32], steps: usize) -> Result<Self> {
        validate_inputs(neighbors, source_nodes)?;
        ensure!(steps > 0, "sparse smoothing requires at least one step");
        ensure!(
            !source_nodes.is_empty(),
            "sparse smoothing has no source nodes"
        );
        let mut seen = HashSet::with_capacity(source_nodes.len());
        let mut rows = vec![Vec::<(u32, f32)>::new(); neighbors.len()];
        for (column, node) in source_nodes.iter().copied().enumerate() {
            ensure!(
                (node as usize) < neighbors.len(),
                "source node {node} is outside the display surface"
            );
            ensure!(
                seen.insert(node),
                "source node {node} appears more than once"
            );
            rows[node as usize].push((column as u32, 1.0));
        }

        let mut accumulator = vec![0.0_f32; source_nodes.len()];
        let mut marks = vec![0_u32; source_nodes.len()];
        let mut generation = 0_u32;
        let mut touched = Vec::new();
        for _ in 0..steps {
            let mut next = vec![Vec::<(u32, f32)>::new(); neighbors.len()];
            for node in 0..neighbors.len() {
                generation = generation.wrapping_add(1);
                if generation == 0 {
                    marks.fill(0);
                    generation = 1;
                }
                touched.clear();
                let mut contributing_rows = 0_usize;
                for adjacent in std::iter::once(node)
                    .chain(neighbors[node].iter().map(|neighbor| *neighbor as usize))
                {
                    if rows[adjacent].is_empty() {
                        continue;
                    }
                    contributing_rows += 1;
                    for (column, weight) in &rows[adjacent] {
                        let column = *column as usize;
                        if marks[column] != generation {
                            marks[column] = generation;
                            accumulator[column] = 0.0;
                            touched.push(column);
                        }
                        accumulator[column] += *weight;
                    }
                }
                if contributing_rows == 0 {
                    continue;
                }
                let normalization = 1.0 / contributing_rows as f32;
                next[node].reserve(touched.len());
                for column in &touched {
                    next[node].push((*column as u32, accumulator[*column] * normalization));
                }
            }
            rows = next;
        }

        let mut nearest_fallback_nodes = 0;
        // Ten smoothing steps normally cover the mesh. Fill any remaining
        // reachable vertices without mixing disconnected hemispheres.
        if rows.iter().any(Vec::is_empty) {
            let nearest = NearestSourceProjection::build(neighbors, source_nodes)?;
            for (row, source) in rows.iter_mut().zip(nearest.source_for_node) {
                if row.is_empty()
                    && let Some(source) = source
                {
                    nearest_fallback_nodes += 1;
                    row.push((source, 1.0));
                }
            }
        }

        let mut row_offsets = Vec::with_capacity(rows.len() + 1);
        let mut column_indices = Vec::new();
        let mut weights = Vec::new();
        row_offsets.push(0);
        for row in rows {
            column_indices.extend(row.iter().map(|(column, _)| *column));
            weights.extend(row.iter().map(|(_, weight)| *weight));
            row_offsets.push(column_indices.len());
        }
        Ok(Self {
            smoothing_steps: steps,
            nearest_fallback_nodes,
            source_nodes: source_nodes.to_vec(),
            row_offsets,
            column_indices,
            weights,
        })
    }

    pub fn sample_node(
        &self,
        node: usize,
        mut source: impl FnMut(usize) -> Option<f32>,
    ) -> Option<f32> {
        let start = *self.row_offsets.get(node)?;
        let end = *self.row_offsets.get(node + 1)?;
        if start == end {
            return None;
        }
        let mut value = 0.0;
        for entry in start..end {
            value += source(self.column_indices[entry] as usize)? * self.weights[entry];
        }
        value.is_finite().then_some(value)
    }

    pub fn apply_dense_seeds(&self, values: &[f32]) -> Result<Vec<f32>> {
        ensure!(
            values.len() + 1 == self.row_offsets.len(),
            "sparse smoothing input has {} nodes but matrix targets {}",
            values.len(),
            self.row_offsets.len().saturating_sub(1)
        );
        let source_values = self
            .source_nodes
            .iter()
            .map(|node| values[*node as usize])
            .collect::<Vec<_>>();
        let mut output = vec![f32::NAN; values.len()];
        for (node, slot) in output.iter_mut().enumerate() {
            *slot = self
                .sample_node(node, |row| Some(source_values[row]))
                .unwrap_or(f32::NAN);
        }
        Ok(output)
    }
}

#[derive(Debug)]
pub struct NearestSourceProjection {
    source_nodes: Vec<u32>,
    source_for_node: Vec<Option<u32>>,
}

impl NearestSourceProjection {
    pub fn build(neighbors: &[Vec<u32>], source_nodes: &[u32]) -> Result<Self> {
        validate_inputs(neighbors, source_nodes)?;
        let mut source_for_node = vec![None; neighbors.len()];
        let mut queue = VecDeque::new();
        let mut ordered_sources = source_nodes.iter().copied().enumerate().collect::<Vec<_>>();
        ordered_sources.sort_by_key(|(_, node)| *node);
        for (column, node) in ordered_sources {
            ensure!(
                (node as usize) < neighbors.len(),
                "source node {node} is outside the display surface"
            );
            source_for_node[node as usize] = Some(column as u32);
            queue.push_back(node as usize);
        }
        while let Some(node) = queue.pop_front() {
            let source = source_for_node[node];
            for neighbor in &neighbors[node] {
                let neighbor = *neighbor as usize;
                if source_for_node[neighbor].is_none() {
                    source_for_node[neighbor] = source;
                    queue.push_back(neighbor);
                }
            }
        }
        Ok(Self {
            source_nodes: source_nodes.to_vec(),
            source_for_node,
        })
    }

    pub fn apply_dense_seeds(&self, values: &[f32]) -> Result<Vec<f32>> {
        ensure!(
            values.len() == self.source_for_node.len(),
            "nearest projection input has {} nodes but projection targets {}",
            values.len(),
            self.source_for_node.len()
        );
        let source_values = self
            .source_nodes
            .iter()
            .map(|node| values[*node as usize])
            .collect::<Vec<_>>();
        Ok(self
            .source_for_node
            .iter()
            .map(|source| source.map_or(f32::NAN, |source| source_values[source as usize]))
            .collect())
    }
}
