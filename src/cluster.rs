//! Surface cluster labeling ("C").
//!
//! This is the surface analog of AFNI's `3dClusterize` and SUMA's `SurfClust`:
//! having thresholded node-wise, keep only connected blobs that are large
//! enough to be worth believing. It is deliberately free of geometry, GPU, and
//! dataset types — it takes a mask plus adjacency and returns labels — so it can
//! be tested directly.

use std::collections::VecDeque;

use crate::overlay::{Threshold, ThresholdMode};

/// Which measure a cluster has to satisfy to survive.
///
/// Node count is what a volumetric voxel count maps onto most directly, but it
/// is mesh-density dependent and so is not comparable between surfaces. Area is
/// the honest metric. Both are offered because anyone matching a threshold from
/// a cluster-size simulation has to use whichever measure that simulation
/// produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterSizeMetric {
    Area,
    Nodes,
}

/// How the two tails of a two-sided threshold are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterTails {
    /// Cluster each tail separately, so a positive blob touching a negative
    /// blob stays two clusters. This matches `3dClusterize -bisided`, and is
    /// the behavior AFNI recommends for two-sided thresholds.
    Bisided,
    /// Cluster on suprathreshold status alone, letting opposite-signed regions
    /// merge where they touch.
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterParams {
    pub metric: ClusterSizeMetric,
    /// Minimum surface area in mm^2. Used when `metric` is
    /// [`ClusterSizeMetric::Area`].
    pub min_area: f32,
    /// Minimum node count. Used when `metric` is [`ClusterSizeMetric::Nodes`].
    pub min_nodes: u32,
    pub tails: ClusterTails,
    /// How many edges apart two suprathreshold nodes may be and still join the
    /// same cluster.
    ///
    /// One is plain edge adjacency, the surface equivalent of a voxel `NN`
    /// setting and the same as `SurfClust -rmm -1`. Larger values bridge gaps,
    /// at the cost of a bounded search per node instead of a single hop.
    pub rings: u32,
}

impl ClusterParams {
    pub fn new() -> Self {
        Self {
            metric: ClusterSizeMetric::Area,
            min_area: 50.0,
            min_nodes: 20,
            tails: ClusterTails::Bisided,
            rings: 1,
        }
    }
}

impl Default for ClusterParams {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-cluster summary, in the spirit of the `SurfClust` cluster table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterSummary {
    /// One-based label, matching the value stored in [`ClusterLabels::labels`].
    pub label: u32,
    pub node_count: u32,
    pub area: f32,
    /// Node carrying the largest absolute intensity in this cluster.
    pub peak_node: u32,
    pub peak_value: f32,
    pub min_value: f32,
    pub max_value: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClusterLabels {
    /// One entry per node. Zero means the node is not part of any surviving
    /// cluster, either because it failed the threshold or because its cluster
    /// was too small.
    pub labels: Vec<u32>,
    /// Surviving clusters, largest first.
    pub clusters: Vec<ClusterSummary>,
}

impl ClusterLabels {
    pub fn empty(node_count: usize) -> Self {
        Self {
            labels: vec![0; node_count],
            clusters: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    /// Nodes belonging to `label`, in ascending order.
    pub fn nodes_for(&self, label: u32) -> Vec<u32> {
        if label == 0 {
            return Vec::new();
        }
        self.labels
            .iter()
            .enumerate()
            .filter(|(_, value)| **value == label)
            .map(|(node, _)| node as u32)
            .collect()
    }
}

/// Inputs to [`label_clusters`], grouped so the call site stays readable.
pub struct ClusterInput<'a> {
    /// Nodes that passed the node-wise threshold.
    pub passes: &'a [bool],
    /// Threshold-column values. Only their sign is used, to keep the tails
    /// apart under [`ClusterTails::Bisided`].
    pub tail_values: &'a [f32],
    /// Adjacency, one neighbor list per node.
    pub neighbors: &'a [Vec<u32>],
    /// Per-node surface area in mm^2, as produced by `SurfaceMesh::node_areas`.
    pub node_areas: &'a [f32],
    /// Intensity values, used only for the reported peak and range.
    pub values: &'a [f32],
}

impl ClusterInput<'_> {
    fn node_count(&self) -> usize {
        self.passes.len()
    }

    /// Every input array has to describe the same node domain, or the labels
    /// would silently refer to different nodes than the caller believes.
    fn is_consistent(&self) -> bool {
        let count = self.node_count();
        self.tail_values.len() == count
            && self.neighbors.len() == count
            && self.node_areas.len() == count
            && self.values.len() == count
    }
}

/// Groups suprathreshold nodes into connected clusters and drops those below
/// the size minimum.
///
/// Returned labels are one-based and ordered largest cluster first, matching
/// `SurfClust`'s default sort.
pub fn label_clusters(input: ClusterInput<'_>, params: ClusterParams) -> ClusterLabels {
    let node_count = input.node_count();
    if !input.is_consistent() {
        return ClusterLabels::empty(node_count);
    }

    let rings = params.rings.max(1);
    let mut visited = vec![false; node_count];
    let mut found: Vec<(Vec<u32>, ClusterSummary)> = Vec::new();

    for seed in 0..node_count {
        if visited[seed] || !input.passes[seed] {
            continue;
        }

        let seed_sign = tail_sign(input.tail_values[seed]);
        let mut members = Vec::new();
        let mut queue = VecDeque::from([seed]);
        visited[seed] = true;

        while let Some(node) = queue.pop_front() {
            members.push(node as u32);
            for neighbor in reachable_within(node, rings, input.neighbors) {
                let neighbor = neighbor as usize;
                if visited[neighbor] || !input.passes[neighbor] {
                    continue;
                }
                if params.tails == ClusterTails::Bisided
                    && tail_sign(input.tail_values[neighbor]) != seed_sign
                {
                    continue;
                }
                visited[neighbor] = true;
                queue.push_back(neighbor);
            }
        }

        let summary = summarize(&members, &input);
        if survives(&summary, params) {
            found.push((members, summary));
        }
    }

    // Largest first, so label 1 is the biggest cluster.
    found.sort_by(|a, b| match params.metric {
        ClusterSizeMetric::Area => b.1.area.total_cmp(&a.1.area),
        ClusterSizeMetric::Nodes => b.1.node_count.cmp(&a.1.node_count),
    });

    let mut labels = vec![0u32; node_count];
    let mut clusters = Vec::with_capacity(found.len());
    for (index, (members, mut summary)) in found.into_iter().enumerate() {
        let label = index as u32 + 1;
        summary.label = label;
        for node in members {
            labels[node as usize] = label;
        }
        clusters.push(summary);
    }

    ClusterLabels { labels, clusters }
}

/// Sign of the tail a node belongs to. Zero counts as positive; a node with a
/// threshold value of exactly zero cannot pass a two-sided threshold anyway.
fn tail_sign(value: f32) -> i8 {
    if value < 0.0 { -1 } else { 1 }
}

/// Nodes within `rings` edges of `node`, excluding `node` itself.
///
/// Intermediate nodes need not be suprathreshold, matching `SurfClust -rmm -N`,
/// so a wider ring setting bridges small gaps between blobs.
fn reachable_within(node: usize, rings: u32, neighbors: &[Vec<u32>]) -> Vec<u32> {
    if rings <= 1 {
        return neighbors[node].clone();
    }

    let mut seen = vec![node as u32];
    let mut frontier = vec![node as u32];
    for _ in 0..rings {
        let mut next = Vec::new();
        for current in &frontier {
            for neighbor in &neighbors[*current as usize] {
                if !seen.contains(neighbor) {
                    seen.push(*neighbor);
                    next.push(*neighbor);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    seen.remove(0);
    seen
}

fn summarize(members: &[u32], input: &ClusterInput<'_>) -> ClusterSummary {
    let mut area = 0.0f32;
    let mut peak_node = members.first().copied().unwrap_or(0);
    let mut peak_magnitude = f32::NEG_INFINITY;
    let mut peak_value = 0.0f32;
    let mut min_value = f32::INFINITY;
    let mut max_value = f32::NEG_INFINITY;

    for node in members {
        let index = *node as usize;
        let node_area = input.node_areas[index];
        if node_area.is_finite() {
            area += node_area;
        }
        let value = input.values[index];
        if !value.is_finite() {
            continue;
        }
        min_value = min_value.min(value);
        max_value = max_value.max(value);
        if value.abs() > peak_magnitude {
            peak_magnitude = value.abs();
            peak_value = value;
            peak_node = *node;
        }
    }

    ClusterSummary {
        // Assigned once the clusters are sorted.
        label: 0,
        node_count: members.len() as u32,
        area,
        peak_node,
        peak_value,
        min_value: if min_value.is_finite() {
            min_value
        } else {
            0.0
        },
        max_value: if max_value.is_finite() {
            max_value
        } else {
            0.0
        },
    }
}

fn survives(summary: &ClusterSummary, params: ClusterParams) -> bool {
    match params.metric {
        ClusterSizeMetric::Area => {
            let minimum = if params.min_area.is_finite() {
                params.min_area.max(0.0)
            } else {
                0.0
            };
            summary.area >= minimum
        }
        ClusterSizeMetric::Nodes => summary.node_count >= params.min_nodes,
    }
}

/// Reconstructs the `SurfClust` command line that would reproduce the current
/// clusters outside the GUI.
///
/// AFNI's convention is to show users the equivalent command for what the
/// interface just did, so a result found by clicking can be rerun, scripted,
/// and put in a methods section.
///
/// Returns the command plus any caveats where the GUI settings have no exact
/// `SurfClust` equivalent.
pub fn surfclust_command(
    surface: Option<&str>,
    dataset: Option<&str>,
    intensity_column: usize,
    threshold_column: Option<usize>,
    threshold: Threshold,
    params: ClusterParams,
    full_list: bool,
) -> String {
    let mut command = String::from("SurfClust");
    command.push_str(&format!(" -i {}", surface.unwrap_or("SURFACE")));
    command.push_str(&format!(
        " -input {} {intensity_column}",
        dataset.unwrap_or("DATASET")
    ));

    // The GUI's ring count is edge connectivity, which SurfClust spells as a
    // negative radius.
    command.push_str(&format!(" -rmm -{}", params.rings.max(1)));

    match params.metric {
        ClusterSizeMetric::Area => command.push_str(&format!(" -amm2 {}", params.min_area)),
        ClusterSizeMetric::Nodes => command.push_str(&format!(" -n {}", params.min_nodes)),
    }

    if let Some(column) = threshold_column {
        command.push_str(&format!(" -thresh_col {column}"));
    }

    let mut caveats: Vec<String> = Vec::new();
    match (threshold.mode, threshold.range) {
        (ThresholdMode::Off, _) | (_, None) => {}
        (ThresholdMode::Above, Some(range)) => {
            command.push_str(&format!(" -thresh {}", range.min));
        }
        (ThresholdMode::Outside, Some(range)) if range.min == -range.max => {
            // The symmetric case is what the GUI's Abs checkbox produces, and
            // SurfClust spells it as an absolute threshold.
            command.push_str(&format!(" -athresh {}", range.max));
        }
        (ThresholdMode::Outside, Some(range)) => {
            command.push_str(&format!(" -ex_range {} {}", range.min, range.max));
        }
        (ThresholdMode::Between, Some(range)) => {
            command.push_str(&format!(" -ir_range {} {}", range.min, range.max));
        }
        (ThresholdMode::Below, Some(range)) => {
            command.push_str(&format!(" -ir_range -inf {}", range.max));
            caveats.push(
                "SurfClust has no one-sided lower threshold; -ir_range is an approximation."
                    .to_string(),
            );
        }
    }

    match params.metric {
        ClusterSizeMetric::Area => command.push_str(" -sort_area"),
        ClusterSizeMetric::Nodes => command.push_str(" -sort_n_nodes"),
    }
    // -out_roidset writes the cluster-rank dataset; -out_fulllist widens it to
    // every node of the surface, which is the full-rank form.
    command.push_str(" -out_roidset");
    if full_list {
        command.push_str(" -out_fulllist");
    }

    if params.tails == ClusterTails::Bisided {
        // Worth stating plainly: SurfClust clusters the thresholded mask without
        // regard to sign, so it cannot reproduce bisided separation. A user who
        // runs this command on a two-sided threshold may get fewer, larger
        // clusters than the GUI shows.
        caveats.push(
            "SurfClust has no bisided option: it clusters the thresholded mask without \
             separating tails, so touching positive and negative blobs merge there but \
             stay separate here."
                .to_string(),
        );
    }

    if surface.is_none() || dataset.is_none() {
        caveats.push(
            "Placeholders above stand in for paths that are not known to the viewer.".to_string(),
        );
    }

    if caveats.is_empty() {
        command
    } else {
        let notes = caveats
            .iter()
            .map(|caveat| format!("# note: {caveat}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{command}\n{notes}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClusterInput, ClusterParams, ClusterSizeMetric, ClusterTails, label_clusters,
        reachable_within, surfclust_command,
    };
    use crate::overlay::Threshold;

    /// A path graph: 0-1-2 ... n-1, which makes connectivity easy to reason
    /// about while still exercising the traversal.
    fn path_neighbors(count: usize) -> Vec<Vec<u32>> {
        (0..count)
            .map(|node| {
                let mut neighbors = Vec::new();
                if node > 0 {
                    neighbors.push(node as u32 - 1);
                }
                if node + 1 < count {
                    neighbors.push(node as u32 + 1);
                }
                neighbors
            })
            .collect()
    }

    #[test]
    fn separates_disconnected_blobs_and_drops_the_small_one() {
        // Nodes 0,1,2 form one blob; node 5 is an isolated singleton. With a
        // three-node minimum only the first survives.
        let neighbors = path_neighbors(7);
        let passes = vec![true, true, true, false, false, true, false];
        let areas = vec![1.0; 7];
        let values = vec![1.0, 5.0, 2.0, 0.0, 0.0, 9.0, 0.0];
        let tails = vec![1.0; 7];

        let labels = label_clusters(
            ClusterInput {
                passes: &passes,
                tail_values: &tails,
                neighbors: &neighbors,
                node_areas: &areas,
                values: &values,
            },
            ClusterParams {
                metric: ClusterSizeMetric::Nodes,
                min_nodes: 3,
                rings: 1,
                ..ClusterParams::new()
            },
        );

        assert_eq!(labels.clusters.len(), 1);
        assert_eq!(labels.labels, vec![1, 1, 1, 0, 0, 0, 0]);
        assert_eq!(labels.clusters[0].node_count, 3);
        assert_eq!(labels.nodes_for(1), vec![0, 1, 2]);

        // The rejected singleton had the largest value in the whole dataset,
        // so the peak must come from the surviving cluster only.
        assert_eq!(labels.clusters[0].peak_node, 1);
        assert_eq!(labels.clusters[0].peak_value, 5.0);
        assert_eq!(labels.clusters[0].min_value, 1.0);
        assert_eq!(labels.clusters[0].max_value, 5.0);
    }

    #[test]
    fn area_metric_sums_node_areas_rather_than_counting_nodes() {
        // Two blobs with equal node counts but different node areas: the
        // dense-but-small one must fail an area threshold that the other
        // passes. This is the case where node count would mislead.
        let neighbors = path_neighbors(5);
        let passes = vec![true, true, false, true, true];
        let areas = vec![10.0, 10.0, 0.0, 1.0, 1.0];
        let values = vec![1.0; 5];
        let tails = vec![1.0; 5];

        let labels = label_clusters(
            ClusterInput {
                passes: &passes,
                tail_values: &tails,
                neighbors: &neighbors,
                node_areas: &areas,
                values: &values,
            },
            ClusterParams {
                metric: ClusterSizeMetric::Area,
                min_area: 5.0,
                rings: 1,
                ..ClusterParams::new()
            },
        );

        assert_eq!(labels.clusters.len(), 1);
        assert_eq!(labels.clusters[0].area, 20.0);
        assert_eq!(labels.labels, vec![1, 1, 0, 0, 0]);
    }

    #[test]
    fn bisided_keeps_touching_opposite_tails_apart() {
        // Nodes 0,1 are a negative blob directly adjacent to a positive blob at
        // 2,3. Merged clustering joins them into one four-node cluster;
        // bisided must keep them separate, matching 3dClusterize -bisided.
        let neighbors = path_neighbors(4);
        let passes = vec![true; 4];
        let areas = vec![1.0; 4];
        let values = vec![-3.0, -4.0, 3.0, 4.0];
        let tails = values.clone();

        let bisided = label_clusters(
            ClusterInput {
                passes: &passes,
                tail_values: &tails,
                neighbors: &neighbors,
                node_areas: &areas,
                values: &values,
            },
            ClusterParams {
                metric: ClusterSizeMetric::Nodes,
                min_nodes: 1,
                tails: ClusterTails::Bisided,
                rings: 1,
                ..ClusterParams::new()
            },
        );
        assert_eq!(bisided.clusters.len(), 2);
        assert!(bisided.clusters.iter().all(|c| c.node_count == 2));
        // The two tails carry different labels.
        assert_ne!(bisided.labels[1], bisided.labels[2]);

        let merged = label_clusters(
            ClusterInput {
                passes: &passes,
                tail_values: &tails,
                neighbors: &neighbors,
                node_areas: &areas,
                values: &values,
            },
            ClusterParams {
                metric: ClusterSizeMetric::Nodes,
                min_nodes: 1,
                tails: ClusterTails::Merged,
                rings: 1,
                ..ClusterParams::new()
            },
        );
        assert_eq!(merged.clusters.len(), 1);
        assert_eq!(merged.clusters[0].node_count, 4);
    }

    #[test]
    fn rings_bridge_gaps_between_otherwise_separate_blobs() {
        // 0,1 and 3,4 are separated by one failing node. Single-ring
        // adjacency keeps them apart; two rings bridges the gap, because the
        // intermediate node need not itself be suprathreshold.
        let neighbors = path_neighbors(5);
        let passes = vec![true, true, false, true, true];
        let areas = vec![1.0; 5];
        let values = vec![1.0; 5];
        let tails = vec![1.0; 5];
        let input = || ClusterInput {
            passes: &passes,
            tail_values: &tails,
            neighbors: &neighbors,
            node_areas: &areas,
            values: &values,
        };

        let single = label_clusters(
            input(),
            ClusterParams {
                metric: ClusterSizeMetric::Nodes,
                min_nodes: 1,
                rings: 1,
                ..ClusterParams::new()
            },
        );
        assert_eq!(single.clusters.len(), 2);

        let double = label_clusters(
            input(),
            ClusterParams {
                metric: ClusterSizeMetric::Nodes,
                min_nodes: 1,
                rings: 2,
                ..ClusterParams::new()
            },
        );
        assert_eq!(double.clusters.len(), 1);
        assert_eq!(double.clusters[0].node_count, 4);
        // The bridging node itself never joins, since it failed the threshold.
        assert_eq!(double.labels[2], 0);
    }

    #[test]
    fn clusters_are_labeled_largest_first() {
        // 0,1,2 is larger than 4,5. Label 1 must be the larger, matching
        // SurfClust's default area sort.
        let neighbors = path_neighbors(6);
        let passes = vec![true, true, true, false, true, true];
        let areas = vec![1.0; 6];
        let values = vec![1.0; 6];
        let tails = vec![1.0; 6];

        let labels = label_clusters(
            ClusterInput {
                passes: &passes,
                tail_values: &tails,
                neighbors: &neighbors,
                node_areas: &areas,
                values: &values,
            },
            ClusterParams {
                metric: ClusterSizeMetric::Nodes,
                min_nodes: 1,
                rings: 1,
                ..ClusterParams::new()
            },
        );

        assert_eq!(labels.clusters[0].label, 1);
        assert_eq!(labels.clusters[0].node_count, 3);
        assert_eq!(labels.clusters[1].node_count, 2);
        assert_eq!(labels.nodes_for(1), vec![0, 1, 2]);
        assert_eq!(labels.nodes_for(2), vec![4, 5]);
        assert!(labels.nodes_for(0).is_empty());
    }

    #[test]
    fn mismatched_inputs_and_empty_masks_yield_no_clusters() {
        let neighbors = path_neighbors(4);
        let areas = vec![1.0; 4];
        let values = vec![1.0; 4];
        let tails = vec![1.0; 4];

        // Nothing passes.
        let none = label_clusters(
            ClusterInput {
                passes: &[false; 4],
                tail_values: &tails,
                neighbors: &neighbors,
                node_areas: &areas,
                values: &values,
            },
            ClusterParams::new(),
        );
        assert!(none.is_empty());
        assert_eq!(none.labels, vec![0; 4]);

        // A short array must not panic or produce labels for the wrong nodes.
        let ragged = label_clusters(
            ClusterInput {
                passes: &[true; 4],
                tail_values: &tails[..2],
                neighbors: &neighbors,
                node_areas: &areas,
                values: &values,
            },
            ClusterParams::new(),
        );
        assert!(ragged.is_empty());
        assert_eq!(ragged.labels.len(), 4);
    }

    #[test]
    fn surfclust_command_maps_gui_settings_onto_flags() {
        // Edge connectivity is a negative radius; the Abs checkbox produces a
        // symmetric Outside threshold, which SurfClust spells as -athresh.
        let command = surfclust_command(
            Some("lh.inflated.gii"),
            Some("stats.niml.dset"),
            0,
            Some(1),
            Threshold::outside(-2.5, 2.5),
            ClusterParams {
                metric: ClusterSizeMetric::Area,
                min_area: 40.0,
                tails: ClusterTails::Merged,
                rings: 1,
                ..ClusterParams::new()
            },
            true,
        );

        assert!(command.starts_with("SurfClust -i lh.inflated.gii"));
        assert!(command.contains("-input stats.niml.dset 0"));
        assert!(command.contains("-rmm -1"));
        assert!(command.contains("-amm2 40"));
        assert!(command.contains("-thresh_col 1"));
        assert!(command.contains("-athresh 2.5"));
        assert!(command.contains("-sort_area"));
        // Merged clustering is what SurfClust does natively, so no caveat.
        assert!(!command.contains("# note:"));
    }

    #[test]
    fn surfclust_command_flags_settings_it_cannot_reproduce() {
        // Bisided has no SurfClust equivalent. Emitting the command without
        // saying so would hand the user something that silently disagrees with
        // what they see on screen.
        let command = surfclust_command(
            Some("lh.gii"),
            Some("d.dset"),
            0,
            Some(1),
            Threshold::outside(-2.0, 2.0),
            ClusterParams {
                tails: ClusterTails::Bisided,
                ..ClusterParams::new()
            },
            true,
        );
        assert!(command.contains("# note:"));
        assert!(command.contains("bisided"));

        // Unknown paths are flagged rather than silently emitted as blanks.
        let unknown = surfclust_command(
            None,
            None,
            0,
            None,
            Threshold::above(3.0),
            ClusterParams {
                tails: ClusterTails::Merged,
                ..ClusterParams::new()
            },
            false,
        );
        assert!(unknown.contains("SURFACE"));
        assert!(unknown.contains("DATASET"));
        assert!(unknown.contains("# note:"));
        // No threshold column selected means no -thresh_col flag at all.
        assert!(!unknown.contains("-thresh_col"));
        assert!(unknown.contains("-thresh 3"));
    }

    #[test]
    fn surfclust_command_uses_node_flags_for_the_node_metric() {
        let command = surfclust_command(
            Some("s.gii"),
            Some("d.dset"),
            2,
            Some(3),
            Threshold::above(1.5),
            ClusterParams {
                metric: ClusterSizeMetric::Nodes,
                min_nodes: 75,
                tails: ClusterTails::Merged,
                rings: 3,
                ..ClusterParams::new()
            },
            true,
        );
        assert!(command.contains("-n 75"));
        assert!(command.contains("-sort_n_nodes"));
        assert!(!command.contains("-amm2"));
        // A wider ring setting is a larger negative radius.
        assert!(command.contains("-rmm -3"));
    }

    #[test]
    fn full_list_flag_distinguishes_dataset_from_roi_output() {
        // The full-rank .niml.dset form is -out_fulllist; the sparse .niml.roi
        // form is not, so the emitted command has to match which file the user
        // actually wrote.
        let build = |full_list| {
            surfclust_command(
                Some("s.gii"),
                Some("d.dset"),
                0,
                Some(1),
                Threshold::above(2.0),
                ClusterParams {
                    tails: ClusterTails::Merged,
                    ..ClusterParams::new()
                },
                full_list,
            )
        };

        let dataset = build(true);
        assert!(dataset.contains("-out_roidset"));
        assert!(dataset.contains("-out_fulllist"));

        let roi = build(false);
        assert!(roi.contains("-out_roidset"));
        assert!(!roi.contains("-out_fulllist"));
    }

    #[test]
    fn ring_expansion_excludes_the_origin_and_grows_with_radius() {
        let neighbors = path_neighbors(5);
        assert_eq!(reachable_within(2, 1, &neighbors), vec![1, 3]);

        let mut two = reachable_within(2, 2, &neighbors);
        two.sort_unstable();
        assert_eq!(two, vec![0, 1, 3, 4]);
        assert!(!two.contains(&2));
    }
}
