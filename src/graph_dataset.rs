//! AFNI `Graph_Bucket` NIML datasets used by FATCAT network results.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::io::{NimlData, NimlElement, NimlValue, read_niml};

#[derive(Debug, Clone, PartialEq)]
pub struct GraphDataset {
    pub path: Option<PathBuf>,
    pub nodes: Vec<GraphNode>,
    /// Row-major edge table. Full matrices use `nodes.len()²` rows.
    pub edge_values: Vec<f32>,
    pub edge_column_count: usize,
    pub edge_labels: Vec<String>,
    pub matrix_shape: GraphMatrixShape,
    /// Explicit `(edge index, first node, second node)` rows for sparse graphs.
    pub edge_indices: Vec<[i32; 3]>,
    pub network_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    pub index: i32,
    pub position: [f32; 3],
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphMatrixShape {
    Full,
    Triangle,
    TriangleWithDiagonal,
    Sparse,
}

impl GraphDataset {
    pub fn edge_row(&self, row: usize) -> Option<&[f32]> {
        let start = row.checked_mul(self.edge_column_count)?;
        self.edge_values.get(start..start + self.edge_column_count)
    }

    pub fn full_edge_row(&self, source: usize, target: usize) -> Option<&[f32]> {
        (self.matrix_shape == GraphMatrixShape::Full
            && source < self.nodes.len()
            && target < self.nodes.len())
        .then(|| target * self.nodes.len() + source)
        .and_then(|row| self.edge_row(row))
    }

    pub fn edge_endpoints(&self, row: usize) -> Option<(usize, usize)> {
        let count = self.nodes.len();
        match self.matrix_shape {
            GraphMatrixShape::Full => (row < count * count).then_some((row % count, row / count)),
            GraphMatrixShape::Triangle | GraphMatrixShape::TriangleWithDiagonal => {
                let include_diagonal = self.matrix_shape == GraphMatrixShape::TriangleWithDiagonal;
                let mut current = 0;
                for column in 0..count {
                    let first_row = if include_diagonal { column } else { column + 1 };
                    for matrix_row in first_row..count {
                        if current == row {
                            return Some((matrix_row, column));
                        }
                        current += 1;
                    }
                }
                None
            }
            GraphMatrixShape::Sparse => self.edge_indices.get(row).and_then(|indices| {
                let source = usize::try_from(indices[1]).ok()?;
                let target = usize::try_from(indices[2]).ok()?;
                Some((source, target))
            }),
        }
    }

    /// Finite value extent for one named edge-measure column.
    pub fn column_range(&self, column: usize) -> Option<(f32, f32)> {
        if column >= self.edge_column_count {
            return None;
        }
        self.edge_values
            .chunks_exact(self.edge_column_count)
            .filter_map(|row| row.get(column).copied())
            .filter(|value| value.is_finite())
            .fold(None, |range, value| {
                Some(match range {
                    Some((min, max)) => (min.min(value), max.max(value)),
                    None => (value, value),
                })
            })
    }
}

pub fn read_graph_bucket(path: impl AsRef<Path>) -> Result<GraphDataset> {
    let path = path.as_ref();
    let elements = read_niml(path)
        .with_context(|| format!("failed to read graph dataset {}", path.display()))?;
    ensure!(elements.len() == 1, "expected one top-level graph dataset");
    graph_from_element(&elements[0], Some(path))
}

pub fn graph_from_element(root: &NimlElement, source_path: Option<&Path>) -> Result<GraphDataset> {
    ensure!(root.name == "AFNI_dataset", "expected <AFNI_dataset>");
    ensure!(
        root.attrs
            .get("dset_type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("Graph_Bucket")),
        "AFNI dataset is not a Graph_Bucket"
    );
    let NimlData::Group(children) = &root.data else {
        bail!("Graph_Bucket is not a NIML group")
    };
    let sparse = children
        .iter()
        .find(|child| child.name == "SPARSE_DATA")
        .context("Graph_Bucket has no SPARSE_DATA")?;
    let NimlData::Numeric(matrix) = &sparse.data else {
        bail!("Graph_Bucket SPARSE_DATA is not numeric")
    };
    let matrix_shape = match sparse
        .attrs
        .get("matrix_shape")
        .map(|value| value.trim().to_ascii_lowercase())
    {
        Some(value) if value == "full" => GraphMatrixShape::Full,
        Some(value) if value == "tri" => GraphMatrixShape::Triangle,
        Some(value) if value == "tri_diag" => GraphMatrixShape::TriangleWithDiagonal,
        _ => GraphMatrixShape::Sparse,
    };
    let nodes_element = children
        .iter()
        .find(|child| child.name == "NODE_COORDS")
        .context("Graph_Bucket has no NODE_COORDS")?;
    let NimlData::Mixed(table) = &nodes_element.data else {
        bail!("Graph_Bucket NODE_COORDS is not mixed data")
    };
    ensure!(
        table.column_count() >= 5,
        "NODE_COORDS needs index, XYZ, and label columns"
    );
    let mut nodes = Vec::with_capacity(table.rows);
    for row in 0..table.rows {
        let integer = |column| match table.get(row, column) {
            Some(NimlValue::Integer(value)) => Ok(*value as i32),
            _ => bail!("NODE_COORDS row {row} column {column} is not an integer"),
        };
        let number = |column| match table.get(row, column) {
            Some(NimlValue::Float(value)) => Ok(*value as f32),
            Some(NimlValue::Integer(value)) => Ok(*value as f32),
            _ => bail!("NODE_COORDS row {row} column {column} is not numeric"),
        };
        let label = match table.get(row, 4) {
            Some(NimlValue::Text(value)) => value.clone(),
            _ => bail!("NODE_COORDS row {row} label is not text"),
        };
        nodes.push(GraphNode {
            index: integer(0)?,
            position: [number(1)?, number(2)?, number(3)?],
            label,
        });
    }
    let expected_rows = match matrix_shape {
        GraphMatrixShape::Full => Some(nodes.len() * nodes.len()),
        GraphMatrixShape::Triangle => Some(nodes.len() * nodes.len().saturating_sub(1) / 2),
        GraphMatrixShape::TriangleWithDiagonal => Some(nodes.len() * (nodes.len() + 1) / 2),
        GraphMatrixShape::Sparse => None,
    };
    if let Some(expected) = expected_rows {
        ensure!(
            matrix.rows == expected,
            "graph matrix has {} rows but its shape requires {expected}",
            matrix.rows
        );
    }
    let edge_indices = children
        .iter()
        .find(|child| child.name == "INDEX_LIST")
        .and_then(|child| match &child.data {
            NimlData::Numeric(values) => Some(values),
            _ => None,
        })
        .map(|values| {
            ensure!(
                values.column_count() >= 3,
                "sparse graph INDEX_LIST needs edge/source/target columns"
            );
            (0..values.rows)
                .map(|row| {
                    Ok([
                        values.get(row, 0).unwrap_or_default() as i32,
                        values.get(row, 1).unwrap_or_default() as i32,
                        values.get(row, 2).unwrap_or_default() as i32,
                    ])
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    if matrix_shape == GraphMatrixShape::Sparse {
        ensure!(
            edge_indices.len() == matrix.rows,
            "sparse graph edge index count does not match data rows"
        );
    }
    let edge_labels = children
        .iter()
        .find_map(|child| {
            (child.name == "AFNI_atr"
                && child
                    .attrs
                    .get("atr_name")
                    .is_some_and(|name| name == "COLMS_LABS"))
            .then(|| match &child.data {
                NimlData::Text(text) => text
                    .trim_matches(|c| c == ' ' || c == '\"')
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
                _ => Vec::new(),
            })
        })
        .unwrap_or_default();
    let network_file = children
        .iter()
        .find(|child| child.name == "network_link")
        .and_then(|child| child.attrs.get("network_file"))
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                source_path
                    .and_then(Path::parent)
                    .unwrap_or(Path::new("."))
                    .join(path)
            }
        });

    Ok(GraphDataset {
        path: source_path.map(Path::to_path_buf),
        nodes,
        edge_values: matrix.values.iter().map(|value| *value as f32).collect(),
        edge_column_count: matrix.column_count(),
        edge_labels,
        matrix_shape,
        edge_indices,
        network_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::parse_niml_str;

    #[test]
    fn reads_full_graph_bucket_and_relative_network_link() {
        let text = r#"<AFNI_dataset ni_form="ni_group" dset_type="Graph_Bucket">
<SPARSE_DATA ni_type="2*float" ni_dimen="4" matrix_size="2 2" matrix_shape="full">1 2 0 0 0 0 3 4</SPARSE_DATA>
<NODE_COORDS ni_type="int,3*float,String" ni_dimen="2">0 1 2 3 "A" 1 4 5 6 "B"</NODE_COORDS>
<network_link network_file="tracks.niml.tract" ni_form="ni_group"></network_link>
</AFNI_dataset>"#;
        let root = parse_niml_str(text).unwrap().remove(0);
        let graph = graph_from_element(&root, Some(Path::new("/tmp/net.niml.dset"))).unwrap();
        assert_eq!(graph.nodes[1].label, "B");
        assert_eq!(graph.full_edge_row(1, 1), Some(&[3.0, 4.0][..]));
        assert_eq!(
            graph.network_file,
            Some(PathBuf::from("/tmp/tracks.niml.tract"))
        );
    }

    #[test]
    fn triangular_endpoint_order_matches_suma_packing() {
        let graph = GraphDataset {
            path: None,
            nodes: (0..3)
                .map(|index| GraphNode {
                    index,
                    position: [0.0; 3],
                    label: index.to_string(),
                })
                .collect(),
            edge_values: vec![1.0; 3],
            edge_column_count: 1,
            edge_labels: Vec::new(),
            matrix_shape: GraphMatrixShape::Triangle,
            edge_indices: Vec::new(),
            network_file: None,
        };
        assert_eq!(graph.edge_endpoints(0), Some((1, 0)));
        assert_eq!(graph.edge_endpoints(1), Some((2, 0)));
        assert_eq!(graph.edge_endpoints(2), Some((2, 1)));
    }

    #[test]
    fn column_range_uses_only_the_requested_finite_measure() {
        let graph = GraphDataset {
            path: None,
            nodes: Vec::new(),
            edge_values: vec![1.0, -4.0, 2.0, f32::NAN, 3.0, 8.0],
            edge_column_count: 2,
            edge_labels: vec!["first".into(), "second".into()],
            matrix_shape: GraphMatrixShape::Sparse,
            edge_indices: Vec::new(),
            network_file: None,
        };
        assert_eq!(graph.column_range(0), Some((1.0, 3.0)));
        assert_eq!(graph.column_range(1), Some((-4.0, 8.0)));
        assert_eq!(graph.column_range(2), None);
    }
}
