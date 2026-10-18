//! Independent tractography and graph scene objects plus their GPU ribbon renderer.

use super::*;
use crate::graph_dataset::{GraphDataset, GraphMatrixShape};
use crate::tractography::{SpatialBounds, TractographyDataset};

const SEGMENT_FLOATS: usize = 11;
const SEGMENT_STRIDE: wgpu::BufferAddress = (SEGMENT_FLOATS * 4) as u64;
const SEGMENT_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Uint32];
const NODE_FLOATS: usize = 7;
const NODE_STRIDE: wgpu::BufferAddress = (NODE_FLOATS * 4) as u64;
const NODE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
const RIBBON_FEATHER_POINTS: f32 = 1.0;
const MASK_INSTANCE_FLOATS: usize = 9;
const MASK_INSTANCE_STRIDE: wgpu::BufferAddress = (MASK_INSTANCE_FLOATS * 4) as u64;
const MASK_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
const MASK_INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![1 => Float32x3, 2 => Float32, 3 => Float32x4, 4 => Float32];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TractColorMode {
    LocalOrientation,
    MidpointOrientation,
    Bundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TractMaskFilter {
    AllTracts,
    AnyMask,
    Mask(usize),
    Pair(usize, usize),
}

impl TractMaskFilter {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::AllTracts => "all tracts",
            Self::AnyMask => "any mask",
            Self::Mask(_) => "one mask",
            Self::Pair(_, _) => "mask pair",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TractMask {
    pub(super) name: String,
    pub(super) center: [f32; 3],
    pub(super) radius: f32,
    pub(super) color: [f32; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TractMaskAnalysis {
    pub(super) mask_counts: Vec<usize>,
    pub(super) pair_counts: Vec<Vec<usize>>,
    hits: Vec<Vec<bool>>,
    display_flags: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphColorMode {
    Signed,
    Magnitude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphEdgeGeometry {
    Straight,
    LinkedBundles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphMatrixPlacement {
    Docked,
    Window,
}

impl GraphMatrixPlacement {
    pub const ALL: [Self; 2] = [Self::Docked, Self::Window];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Docked => "attached panel",
            Self::Window => "separate window",
        }
    }
}

impl GraphEdgeGeometry {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Straight => "straight edges",
            Self::LinkedBundles => "linked tract bundles",
        }
    }
}

impl GraphColorMode {
    pub(super) const ALL: [Self; 2] = [Self::Signed, Self::Magnitude];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Signed => "signed blue–red",
            Self::Magnitude => "magnitude",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct TractBundleAppearance {
    pub(super) visible: bool,
    pub(super) opacity: f32,
}

#[derive(Debug, Clone)]
struct TractPickIndex {
    entries: Vec<TractPickEntry>,
    nodes: Vec<TractBvhNode>,
}

#[derive(Debug, Clone, Copy)]
struct TractPickEntry {
    bundle_index: u32,
    tract_index: u32,
    flat_index: u32,
    min: [f32; 3],
    max: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
struct TractBvhNode {
    min: [f32; 3],
    max: [f32; 3],
    left: u32,
    right: u32,
    start: u32,
    count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum SceneObjectPick {
    GraphNode {
        object_index: usize,
        object_name: String,
        node_index: i32,
        label: String,
        position: [f32; 3],
    },
    GraphEdge {
        object_index: usize,
        object_name: String,
        source_index: usize,
        target_index: usize,
        source: String,
        target: String,
        measure: String,
        value: f32,
    },
    Tract {
        object_index: usize,
        object_name: String,
        bundle: String,
        tract_id: i32,
        length: f32,
        position: [f32; 3],
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TractMaskPick {
    pub(super) object_index: usize,
    pub(super) mask_index: usize,
    pub(super) distance: f32,
}

impl SceneObjectPick {
    pub(super) fn object_index(&self) -> usize {
        match self {
            Self::GraphNode { object_index, .. }
            | Self::GraphEdge { object_index, .. }
            | Self::Tract { object_index, .. } => *object_index,
        }
    }

    pub(super) fn status_text(&self) -> String {
        match self {
            Self::GraphNode {
                object_name,
                node_index,
                label,
                position,
                ..
            } => format!(
                "{object_name}: node {node_index} {label} at ({:.3}, {:.3}, {:.3}).",
                position[0], position[1], position[2]
            ),
            Self::GraphEdge {
                object_name,
                source,
                target,
                measure,
                value,
                ..
            } => format!("{object_name}: {source} ↔ {target}, {measure} = {value:.6}."),
            Self::Tract {
                object_name,
                bundle,
                tract_id,
                length,
                position,
                ..
            } => format!(
                "{object_name}: {bundle}, tract {tract_id}, length {length:.3}, nearest ({:.3}, {:.3}, {:.3}).",
                position[0], position[1], position[2]
            ),
        }
    }
}

impl TractColorMode {
    pub(super) const ALL: [Self; 3] = [
        Self::LocalOrientation,
        Self::MidpointOrientation,
        Self::Bundle,
    ];
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::LocalOrientation => "local orientation",
            Self::MidpointOrientation => "tract orientation",
            Self::Bundle => "bundle",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum SceneObjectPayload {
    Tracts(TractographyDataset),
    Graph(GraphDataset),
}

#[derive(Debug, Clone)]
pub(super) struct SceneObject {
    pub(super) path: PathBuf,
    pub(super) name: String,
    pub(super) visible: bool,
    pub(super) width_points: f32,
    pub(super) opacity: f32,
    pub(super) tract_color_mode: TractColorMode,
    pub(super) tract_bundles: Vec<TractBundleAppearance>,
    pub(super) tract_masks: Vec<TractMask>,
    pub(super) tract_mask_filter: TractMaskFilter,
    pub(super) tract_mask_selected: Option<usize>,
    pub(super) tract_mask_analysis: TractMaskAnalysis,
    pub(super) graph_measure: usize,
    pub(super) graph_threshold: f32,
    pub(super) graph_color_mode: GraphColorMode,
    pub(super) graph_node_size_points: f32,
    pub(super) graph_labels_visible: bool,
    pub(super) graph_endpoint_labels_visible: bool,
    pub(super) graph_edge_geometry: GraphEdgeGeometry,
    pub(super) graph_matrix_open: bool,
    pub(super) graph_matrix_placement: GraphMatrixPlacement,
    pub(super) graph_matrix_lower_triangle: bool,
    pub(super) graph_matrix_cell_points: f32,
    pub(super) graph_matrix_selected_cell: Option<(usize, usize)>,
    pub(super) linked_tracts: Option<TractographyDataset>,
    pub(super) payload: SceneObjectPayload,
    pub(super) bounds: SpatialBounds,
    tract_pick_index: Option<TractPickIndex>,
}

impl SceneObject {
    pub(super) fn from_tracts(path: PathBuf, data: TractographyDataset) -> Self {
        let name = display_name(&path);
        let bounds = data.bounds;
        let tract_pick_index = Some(TractPickIndex::build(&data));
        let tract_bundles = data
            .bundles
            .iter()
            .map(|_| TractBundleAppearance {
                visible: true,
                opacity: 1.0,
            })
            .collect();
        let tract_mask_analysis = TractMaskAnalysis::empty(data.tract_count());
        Self {
            path,
            name,
            visible: true,
            width_points: 1.75,
            opacity: 1.0,
            tract_color_mode: TractColorMode::LocalOrientation,
            tract_bundles,
            tract_masks: Vec::new(),
            tract_mask_filter: TractMaskFilter::AllTracts,
            tract_mask_selected: None,
            tract_mask_analysis,
            graph_measure: 0,
            graph_threshold: 0.0,
            graph_color_mode: GraphColorMode::Signed,
            graph_node_size_points: 11.0,
            graph_labels_visible: false,
            graph_endpoint_labels_visible: false,
            graph_edge_geometry: GraphEdgeGeometry::Straight,
            graph_matrix_open: false,
            graph_matrix_placement: GraphMatrixPlacement::Docked,
            graph_matrix_lower_triangle: false,
            graph_matrix_cell_points: 22.0,
            graph_matrix_selected_cell: None,
            linked_tracts: None,
            payload: SceneObjectPayload::Tracts(data),
            bounds,
            tract_pick_index,
        }
    }

    pub(super) fn from_graph_with_linked_tracts(
        path: PathBuf,
        data: GraphDataset,
        linked_tracts: Option<TractographyDataset>,
    ) -> Result<Self> {
        let graph_bounds = SpatialBounds::from_points(data.nodes.iter().map(|node| &node.position))
            .context("graph contains no positioned nodes")?;
        let bounds = linked_tracts
            .as_ref()
            .map(|tracts| union_bounds(graph_bounds, tracts.bounds))
            .unwrap_or(graph_bounds);
        let tract_pick_index = linked_tracts.as_ref().map(TractPickIndex::build);
        Ok(Self {
            name: display_name(&path),
            path,
            visible: true,
            width_points: 1.25,
            opacity: 0.85,
            tract_color_mode: TractColorMode::LocalOrientation,
            tract_bundles: Vec::new(),
            tract_masks: Vec::new(),
            tract_mask_filter: TractMaskFilter::AllTracts,
            tract_mask_selected: None,
            tract_mask_analysis: TractMaskAnalysis::empty(0),
            graph_measure: 0,
            graph_threshold: 0.0,
            graph_color_mode: GraphColorMode::Signed,
            graph_node_size_points: 11.0,
            graph_labels_visible: false,
            graph_endpoint_labels_visible: false,
            graph_edge_geometry: GraphEdgeGeometry::Straight,
            graph_matrix_open: false,
            graph_matrix_placement: GraphMatrixPlacement::Docked,
            graph_matrix_lower_triangle: false,
            graph_matrix_cell_points: 22.0,
            graph_matrix_selected_cell: None,
            linked_tracts,
            payload: SceneObjectPayload::Graph(data),
            bounds,
            tract_pick_index,
        })
    }

    pub(super) fn detail(&self) -> String {
        match &self.payload {
            SceneObjectPayload::Tracts(data) => format!(
                "{} bundles · {} tracts · {} points",
                data.bundles.len(),
                data.tract_count(),
                data.point_count()
            ),
            SceneObjectPayload::Graph(data) => format!(
                "{} nodes · {} measures · {} matrix{}",
                data.nodes.len(),
                data.edge_column_count,
                match data.matrix_shape {
                    GraphMatrixShape::Full => "full",
                    GraphMatrixShape::Triangle => "triangle",
                    GraphMatrixShape::TriangleWithDiagonal => "triangle + diagonal",
                    GraphMatrixShape::Sparse => "sparse",
                },
                self.linked_tracts
                    .as_ref()
                    .map_or("", |_| " · linked tracts")
            ),
        }
    }

    fn visit_segments(&self, visitor: impl FnMut(SceneSegment)) {
        match &self.payload {
            SceneObjectPayload::Tracts(data) => {
                visit_tract_segments(data, self.tract_color_mode, &self.tract_bundles, visitor)
            }
            SceneObjectPayload::Graph(data) => visit_graph_segments(
                data,
                self.linked_tracts.as_ref(),
                self.graph_edge_geometry,
                self.graph_measure,
                self.graph_threshold,
                self.graph_color_mode,
                self.graph_matrix_selected_cell,
                visitor,
            ),
        }
    }

    pub(super) fn add_tract_mask(&mut self, center: [f32; 3]) -> Option<usize> {
        let SceneObjectPayload::Tracts(data) = &self.payload else {
            return None;
        };
        let index = self.tract_masks.len();
        let color = stable_label_color(index as i32 + 1, 230).to_array();
        self.tract_masks.push(TractMask {
            name: format!("Mask {}", mask_letter(index)),
            center,
            radius: (data.bounds.radius * 0.08).clamp(1.0, 15.0),
            color,
        });
        self.tract_mask_selected = Some(index);
        if self.tract_mask_filter == TractMaskFilter::AllTracts {
            self.tract_mask_filter = TractMaskFilter::AnyMask;
        }
        self.refresh_tract_mask_analysis();
        Some(index)
    }

    pub(super) fn remove_tract_mask(&mut self, mask_index: usize) -> bool {
        if mask_index >= self.tract_masks.len() {
            return false;
        }
        self.tract_masks.remove(mask_index);
        for (index, mask) in self.tract_masks.iter_mut().enumerate() {
            mask.name = format!("Mask {}", mask_letter(index));
        }
        self.tract_mask_selected = None;
        self.tract_mask_filter = if self.tract_masks.is_empty() {
            TractMaskFilter::AllTracts
        } else {
            TractMaskFilter::AnyMask
        };
        self.refresh_tract_mask_analysis();
        true
    }

    pub(super) fn set_tract_mask_center(&mut self, mask_index: usize, center: [f32; 3]) -> bool {
        let Some(mask) = self.tract_masks.get_mut(mask_index) else {
            return false;
        };
        if mask.center == center || !center.iter().all(|value| value.is_finite()) {
            return false;
        }
        mask.center = center;
        self.refresh_tract_mask_analysis();
        true
    }

    pub(super) fn set_tract_mask_radius(&mut self, mask_index: usize, radius: f32) -> bool {
        let Some(mask) = self.tract_masks.get_mut(mask_index) else {
            return false;
        };
        let radius = radius.max(0.1);
        if !radius.is_finite() || (mask.radius - radius).abs() <= f32::EPSILON {
            return false;
        }
        mask.radius = radius;
        self.refresh_tract_mask_analysis();
        true
    }

    pub(super) fn set_tract_mask_filter(&mut self, filter: TractMaskFilter) {
        self.tract_mask_filter = sanitize_mask_filter(filter, self.tract_masks.len());
        self.refresh_tract_mask_display_flags();
    }

    fn refresh_tract_mask_analysis(&mut self) {
        let SceneObjectPayload::Tracts(data) = &self.payload else {
            self.tract_mask_analysis = TractMaskAnalysis::empty(0);
            return;
        };
        self.tract_mask_filter =
            sanitize_mask_filter(self.tract_mask_filter, self.tract_masks.len());
        self.tract_mask_analysis = analyze_tract_masks(
            data,
            self.tract_pick_index.as_ref(),
            &self.tract_masks,
            self.tract_mask_filter,
        );
    }

    fn refresh_tract_mask_display_flags(&mut self) {
        self.tract_mask_analysis.display_flags = display_flags_for_filter(
            &self.tract_mask_analysis.hits,
            self.tract_mask_filter,
            self.tract_mask_analysis.display_flags.len(),
        );
    }

    fn tract_is_displayed(&self, flat_index: usize) -> bool {
        self.tract_mask_analysis
            .display_flags
            .get(flat_index)
            .is_none_or(|flag| *flag != 0)
    }
}

impl TractMaskAnalysis {
    fn empty(tract_count: usize) -> Self {
        Self {
            mask_counts: Vec::new(),
            pair_counts: Vec::new(),
            hits: Vec::new(),
            display_flags: vec![1; tract_count.max(1)],
        }
    }
}

fn mask_letter(index: usize) -> String {
    if index < 26 {
        ((b'A' + index as u8) as char).to_string()
    } else {
        (index + 1).to_string()
    }
}

fn sanitize_mask_filter(filter: TractMaskFilter, mask_count: usize) -> TractMaskFilter {
    match filter {
        TractMaskFilter::Mask(index) if index >= mask_count => TractMaskFilter::AnyMask,
        TractMaskFilter::Pair(first, second) if first >= mask_count || second >= mask_count => {
            TractMaskFilter::AnyMask
        }
        other => other,
    }
}

fn union_bounds(first: SpatialBounds, second: SpatialBounds) -> SpatialBounds {
    let min = [
        first.min[0].min(second.min[0]),
        first.min[1].min(second.min[1]),
        first.min[2].min(second.min[2]),
    ];
    let max = [
        first.max[0].max(second.max[0]),
        first.max[1].max(second.max[1]),
        first.max[2].max(second.max[2]),
    ];
    SpatialBounds::from_points([min, max].iter()).expect("two bounds corners")
}

impl TractPickIndex {
    fn build(data: &TractographyDataset) -> Self {
        let mut entries = Vec::with_capacity(data.tract_count());
        let mut flat_index = 0_u32;
        for (bundle_index, bundle) in data.bundles.iter().enumerate() {
            for (tract_index, tract) in bundle.tracts.iter().enumerate() {
                let Some(bounds) = SpatialBounds::from_points(tract.points.iter()) else {
                    flat_index += 1;
                    continue;
                };
                entries.push(TractPickEntry {
                    bundle_index: bundle_index as u32,
                    tract_index: tract_index as u32,
                    flat_index,
                    min: bounds.min,
                    max: bounds.max,
                });
                flat_index += 1;
            }
        }
        let mut nodes = Vec::new();
        if !entries.is_empty() {
            build_tract_bvh(&mut entries, &mut nodes, 0);
        }
        Self { entries, nodes }
    }
}

fn analyze_tract_masks(
    data: &TractographyDataset,
    index: Option<&TractPickIndex>,
    masks: &[TractMask],
    filter: TractMaskFilter,
) -> TractMaskAnalysis {
    let tract_count = data.tract_count();
    let mut hits = vec![vec![false; tract_count]; masks.len()];
    if let Some(index) = index {
        for (mask_index, mask) in masks.iter().enumerate() {
            mark_sphere_intersections(data, index, mask, &mut hits[mask_index]);
        }
    }
    let mask_counts = hits
        .iter()
        .map(|mask_hits| mask_hits.iter().filter(|hit| **hit).count())
        .collect::<Vec<_>>();
    let mut pair_counts = vec![vec![0; masks.len()]; masks.len()];
    for first in 0..masks.len() {
        pair_counts[first][first] = mask_counts[first];
        for second in first + 1..masks.len() {
            let count = hits[first]
                .iter()
                .zip(&hits[second])
                .filter(|(a, b)| **a && **b)
                .count();
            pair_counts[first][second] = count;
            pair_counts[second][first] = count;
        }
    }
    let display_flags = display_flags_for_filter(&hits, filter, tract_count);
    TractMaskAnalysis {
        mask_counts,
        pair_counts,
        hits,
        display_flags,
    }
}

fn mark_sphere_intersections(
    data: &TractographyDataset,
    index: &TractPickIndex,
    mask: &TractMask,
    hits: &mut [bool],
) {
    if index.nodes.is_empty() || mask.radius <= 0.0 {
        return;
    }
    let center = Vec3::from_array(mask.center);
    let radius_squared = mask.radius * mask.radius;
    let mut stack = vec![0_usize];
    while let Some(node_index) = stack.pop() {
        let node = index.nodes[node_index];
        if point_aabb_distance_squared(center, node.min, node.max) > radius_squared {
            continue;
        }
        if node.count == 0 {
            stack.push(node.left as usize);
            stack.push(node.right as usize);
            continue;
        }
        for entry in &index.entries[node.start as usize..(node.start + node.count) as usize] {
            if point_aabb_distance_squared(center, entry.min, entry.max) > radius_squared {
                continue;
            }
            let tract =
                &data.bundles[entry.bundle_index as usize].tracts[entry.tract_index as usize];
            if tract_intersects_sphere(&tract.points, center, radius_squared)
                && let Some(hit) = hits.get_mut(entry.flat_index as usize)
            {
                *hit = true;
            }
        }
    }
}

fn point_aabb_distance_squared(point: Vec3, min: [f32; 3], max: [f32; 3]) -> f32 {
    let min = Vec3::from_array(min);
    let max = Vec3::from_array(max);
    let clamped = point.clamp(min, max);
    point.distance_squared(clamped)
}

fn tract_intersects_sphere(points: &[[f32; 3]], center: Vec3, radius_squared: f32) -> bool {
    if points.len() == 1 {
        return Vec3::from_array(points[0]).distance_squared(center) <= radius_squared;
    }
    points.windows(2).any(|pair| {
        let start = Vec3::from_array(pair[0]);
        let end = Vec3::from_array(pair[1]);
        let segment = end - start;
        let length_squared = segment.length_squared();
        let fraction = if length_squared <= f32::EPSILON {
            0.0
        } else {
            ((center - start).dot(segment) / length_squared).clamp(0.0, 1.0)
        };
        start.lerp(end, fraction).distance_squared(center) <= radius_squared
    })
}

fn display_flags_for_filter(
    hits: &[Vec<bool>],
    filter: TractMaskFilter,
    tract_count: usize,
) -> Vec<u32> {
    let mut flags = vec![1; tract_count.max(1)];
    if tract_count == 0 || filter == TractMaskFilter::AllTracts {
        return flags;
    }
    for flat_index in 0..tract_count {
        let visible = match filter {
            TractMaskFilter::AllTracts => true,
            TractMaskFilter::AnyMask => hits.iter().any(|mask_hits| mask_hits[flat_index]),
            TractMaskFilter::Mask(mask) => hits
                .get(mask)
                .is_some_and(|mask_hits| mask_hits[flat_index]),
            TractMaskFilter::Pair(first, second) => hits
                .get(first)
                .zip(hits.get(second))
                .is_some_and(|(a, b)| a[flat_index] && b[flat_index]),
        };
        flags[flat_index] = u32::from(visible);
    }
    flags
}

fn build_tract_bvh(
    entries: &mut [TractPickEntry],
    nodes: &mut Vec<TractBvhNode>,
    base: usize,
) -> usize {
    const LEAF_SIZE: usize = 8;
    let (min, max) = entry_bounds(entries);
    let node_index = nodes.len();
    nodes.push(TractBvhNode {
        min,
        max,
        left: u32::MAX,
        right: u32::MAX,
        start: base as u32,
        count: entries.len() as u32,
    });
    if entries.len() <= LEAF_SIZE {
        return node_index;
    }

    let extent = Vec3::from_array(max) - Vec3::from_array(min);
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    entries.sort_unstable_by(|a, b| {
        let a_center = a.min[axis] + a.max[axis];
        let b_center = b.min[axis] + b.max[axis];
        a_center.total_cmp(&b_center)
    });
    let middle = entries.len() / 2;
    let (left_entries, right_entries) = entries.split_at_mut(middle);
    let left = build_tract_bvh(left_entries, nodes, base);
    let right = build_tract_bvh(right_entries, nodes, base + middle);
    nodes[node_index].left = left as u32;
    nodes[node_index].right = right as u32;
    nodes[node_index].count = 0;
    node_index
}

fn entry_bounds(entries: &[TractPickEntry]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for entry in entries {
        for axis in 0..3 {
            min[axis] = min[axis].min(entry.min[axis]);
            max[axis] = max[axis].max(entry.max[axis]);
        }
    }
    (min, max)
}

pub(super) fn pick_scene_objects(
    objects: &[SceneObject],
    model: Mat4,
    ray_origin: Vec3,
    ray_direction: Vec3,
    tolerance: f32,
) -> Option<SceneObjectPick> {
    let mut best: Option<(f32, f32, SceneObjectPick)> = None;
    for (object_index, object) in objects.iter().enumerate() {
        if !object.visible || object.opacity <= 0.0 {
            continue;
        }
        match &object.payload {
            SceneObjectPayload::Graph(data) => {
                pick_graph_object(
                    object_index,
                    object,
                    data,
                    model,
                    ray_origin,
                    ray_direction,
                    tolerance,
                    &mut best,
                );
            }
            SceneObjectPayload::Tracts(data) => {
                pick_tract_object(
                    object_index,
                    object,
                    data,
                    model,
                    ray_origin,
                    ray_direction,
                    tolerance,
                    true,
                    &mut best,
                );
            }
        }
    }
    best.map(|(_, _, pick)| pick)
}

pub(super) fn pick_tract_on_object_unfiltered(
    objects: &[SceneObject],
    object_index: usize,
    model: Mat4,
    ray_origin: Vec3,
    ray_direction: Vec3,
    tolerance: f32,
) -> Option<SceneObjectPick> {
    let object = objects.get(object_index)?;
    let SceneObjectPayload::Tracts(data) = &object.payload else {
        return None;
    };
    if !object.visible || object.opacity <= 0.0 {
        return None;
    }
    let mut best = None;
    pick_tract_object(
        object_index,
        object,
        data,
        model,
        ray_origin,
        ray_direction,
        tolerance,
        false,
        &mut best,
    );
    best.map(|(_, _, pick)| pick)
}

pub(super) fn pick_tract_masks(
    objects: &[SceneObject],
    model: Mat4,
    ray_origin: Vec3,
    ray_direction: Vec3,
) -> Option<TractMaskPick> {
    let inverse = model.inverse();
    let local_origin = inverse.transform_point3(ray_origin);
    let local_direction = inverse.transform_vector3(ray_direction).normalize_or_zero();
    if local_direction.length_squared() <= f32::EPSILON {
        return None;
    }
    let mut best = None;
    for (object_index, object) in objects.iter().enumerate() {
        if !object.visible || !matches!(object.payload, SceneObjectPayload::Tracts(_)) {
            continue;
        }
        for (mask_index, mask) in object.tract_masks.iter().enumerate() {
            let Some(local_distance) = ray_sphere_intersection(
                local_origin,
                local_direction,
                Vec3::from_array(mask.center),
                mask.radius,
            ) else {
                continue;
            };
            let scene_hit = model.transform_point3(local_origin + local_direction * local_distance);
            let distance = scene_hit.distance(ray_origin);
            if best
                .as_ref()
                .is_none_or(|pick: &TractMaskPick| distance < pick.distance)
            {
                best = Some(TractMaskPick {
                    object_index,
                    mask_index,
                    distance,
                });
            }
        }
    }
    best
}

fn ray_sphere_intersection(
    origin: Vec3,
    direction: Vec3,
    center: Vec3,
    radius: f32,
) -> Option<f32> {
    let offset = origin - center;
    let half_b = offset.dot(direction);
    let c = offset.length_squared() - radius * radius;
    let discriminant = half_b * half_b - c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let near = -half_b - root;
    let far = -half_b + root;
    if near >= 0.0 {
        Some(near)
    } else if far >= 0.0 {
        Some(far)
    } else {
        None
    }
}

fn pick_graph_object(
    object_index: usize,
    object: &SceneObject,
    data: &GraphDataset,
    model: Mat4,
    ray_origin: Vec3,
    ray_direction: Vec3,
    tolerance: f32,
    best: &mut Option<(f32, f32, SceneObjectPick)>,
) {
    let node_tolerance = tolerance * (object.graph_node_size_points / 11.0).max(0.65);
    for node in &data.nodes {
        let position = model.transform_point3(Vec3::from_array(node.position));
        let (distance, ray_distance) = point_ray_distance(position, ray_origin, ray_direction);
        if distance <= node_tolerance {
            update_best_pick(
                best,
                distance / node_tolerance,
                ray_distance,
                SceneObjectPick::GraphNode {
                    object_index,
                    object_name: object.name.clone(),
                    node_index: node.index,
                    label: node.label.clone(),
                    position: node.position,
                },
            );
        }
    }

    if object.graph_edge_geometry == GraphEdgeGeometry::LinkedBundles {
        pick_graph_bundle_edges(
            object_index,
            object,
            data,
            model,
            ray_origin,
            ray_direction,
            tolerance,
            best,
        );
    }

    visit_graph_edges(
        data,
        object.graph_measure,
        |source, target, value, edge_ids| {
            if !value.is_finite() || value == 0.0 || value.abs() < object.graph_threshold.max(0.0) {
                return;
            }
            if object.graph_edge_geometry == GraphEdgeGeometry::LinkedBundles
                && object
                    .linked_tracts
                    .as_ref()
                    .and_then(|tracts| linked_bundle_index_for_edge(tracts, edge_ids))
                    .is_some()
            {
                return;
            }
            let start = model.transform_point3(Vec3::from_array(data.nodes[source].position));
            let end = model.transform_point3(Vec3::from_array(data.nodes[target].position));
            let (distance, ray_distance, _) =
                segment_ray_distance(start, end, ray_origin, ray_direction);
            if distance <= tolerance {
                let measure = data
                    .edge_labels
                    .get(object.graph_measure)
                    .cloned()
                    .unwrap_or_else(|| format!("Measure {}", object.graph_measure + 1));
                update_best_pick(
                    best,
                    distance / tolerance,
                    ray_distance,
                    SceneObjectPick::GraphEdge {
                        object_index,
                        object_name: object.name.clone(),
                        source_index: source,
                        target_index: target,
                        source: data.nodes[source].label.clone(),
                        target: data.nodes[target].label.clone(),
                        measure,
                        value,
                    },
                );
            }
        },
    );
}

#[derive(Debug, Clone, Copy)]
struct GraphBundlePickInfo {
    source: usize,
    target: usize,
    value: f32,
}

#[allow(clippy::too_many_arguments)]
fn pick_graph_bundle_edges(
    object_index: usize,
    object: &SceneObject,
    data: &GraphDataset,
    model: Mat4,
    ray_origin: Vec3,
    ray_direction: Vec3,
    tolerance: f32,
    best: &mut Option<(f32, f32, SceneObjectPick)>,
) {
    let (Some(tracts), Some(index)) = (&object.linked_tracts, &object.tract_pick_index) else {
        return;
    };
    if index.nodes.is_empty() {
        return;
    }
    let mut bundle_edges = vec![None; tracts.bundles.len()];
    visit_graph_edges(
        data,
        object.graph_measure,
        |source, target, value, edge_ids| {
            if value.is_finite()
                && value != 0.0
                && value.abs() >= object.graph_threshold.max(0.0)
                && let Some(bundle_index) = linked_bundle_index_for_edge(tracts, edge_ids)
            {
                bundle_edges[bundle_index] = Some(GraphBundlePickInfo {
                    source,
                    target,
                    value,
                });
            }
        },
    );

    let inverse = model.inverse();
    let local_origin = inverse.transform_point3(ray_origin);
    let local_direction = inverse.transform_vector3(ray_direction).normalize_or_zero();
    if local_direction.length_squared() <= f32::EPSILON {
        return;
    }
    let local_tolerance = inverse
        .transform_vector3(Vec3::X * tolerance)
        .length()
        .max(f32::EPSILON);
    let measure = data
        .edge_labels
        .get(object.graph_measure)
        .cloned()
        .unwrap_or_else(|| format!("Measure {}", object.graph_measure + 1));
    let mut stack = vec![0_usize];
    while let Some(node_index) = stack.pop() {
        let node = index.nodes[node_index];
        if !ray_intersects_aabb(
            local_origin,
            local_direction,
            node.min,
            node.max,
            local_tolerance,
        ) {
            continue;
        }
        if node.count == 0 {
            stack.push(node.left as usize);
            stack.push(node.right as usize);
            continue;
        }
        for entry in &index.entries[node.start as usize..(node.start + node.count) as usize] {
            let Some(edge) = bundle_edges
                .get(entry.bundle_index as usize)
                .copied()
                .flatten()
            else {
                continue;
            };
            if !ray_intersects_aabb(
                local_origin,
                local_direction,
                entry.min,
                entry.max,
                local_tolerance,
            ) {
                continue;
            }
            let tract =
                &tracts.bundles[entry.bundle_index as usize].tracts[entry.tract_index as usize];
            for pair in tract.points.windows(2) {
                let start = Vec3::from_array(pair[0]);
                let end = Vec3::from_array(pair[1]);
                let (distance, _, segment_fraction) =
                    segment_ray_distance(start, end, local_origin, local_direction);
                if distance > local_tolerance {
                    continue;
                }
                let scene_position = model.transform_point3(start.lerp(end, segment_fraction));
                let (scene_distance, scene_ray_distance) =
                    point_ray_distance(scene_position, ray_origin, ray_direction);
                update_best_pick(
                    best,
                    scene_distance / tolerance,
                    scene_ray_distance,
                    SceneObjectPick::GraphEdge {
                        object_index,
                        object_name: object.name.clone(),
                        source_index: edge.source,
                        target_index: edge.target,
                        source: data.nodes[edge.source].label.clone(),
                        target: data.nodes[edge.target].label.clone(),
                        measure: measure.clone(),
                        value: edge.value,
                    },
                );
            }
        }
    }
}

fn pick_tract_object(
    object_index: usize,
    object: &SceneObject,
    data: &TractographyDataset,
    model: Mat4,
    ray_origin: Vec3,
    ray_direction: Vec3,
    tolerance: f32,
    respect_mask_filter: bool,
    best: &mut Option<(f32, f32, SceneObjectPick)>,
) {
    let Some(index) = &object.tract_pick_index else {
        return;
    };
    if index.nodes.is_empty() {
        return;
    }
    let inverse = model.inverse();
    let local_origin = inverse.transform_point3(ray_origin);
    let local_direction = inverse.transform_vector3(ray_direction).normalize_or_zero();
    if local_direction.length_squared() <= f32::EPSILON {
        return;
    }
    let local_tolerance = inverse
        .transform_vector3(Vec3::X * tolerance)
        .length()
        .max(f32::EPSILON);
    let mut stack = vec![0_usize];
    while let Some(node_index) = stack.pop() {
        let node = index.nodes[node_index];
        if !ray_intersects_aabb(
            local_origin,
            local_direction,
            node.min,
            node.max,
            local_tolerance,
        ) {
            continue;
        }
        if node.count == 0 {
            stack.push(node.left as usize);
            stack.push(node.right as usize);
            continue;
        }
        for entry in &index.entries[node.start as usize..(node.start + node.count) as usize] {
            let Some(appearance) = object.tract_bundles.get(entry.bundle_index as usize) else {
                continue;
            };
            if !appearance.visible || appearance.opacity <= 0.0 {
                continue;
            }
            if respect_mask_filter && !object.tract_is_displayed(entry.flat_index as usize) {
                continue;
            }
            if !ray_intersects_aabb(
                local_origin,
                local_direction,
                entry.min,
                entry.max,
                local_tolerance,
            ) {
                continue;
            }
            let bundle = &data.bundles[entry.bundle_index as usize];
            let tract = &bundle.tracts[entry.tract_index as usize];
            let tract_length = tract
                .points
                .windows(2)
                .map(|pair| Vec3::from_array(pair[0]).distance(Vec3::from_array(pair[1])))
                .sum();
            let bundle_label = bundle
                .ends
                .clone()
                .or_else(|| bundle.tag.map(|tag| format!("bundle {tag}")))
                .unwrap_or_else(|| format!("bundle {}", entry.bundle_index + 1));
            for pair in tract.points.windows(2) {
                let start = Vec3::from_array(pair[0]);
                let end = Vec3::from_array(pair[1]);
                let (distance, _, segment_fraction) =
                    segment_ray_distance(start, end, local_origin, local_direction);
                if distance > local_tolerance {
                    continue;
                }
                let local_position = start.lerp(end, segment_fraction);
                let scene_position = model.transform_point3(local_position);
                let (_, scene_ray_distance) =
                    point_ray_distance(scene_position, ray_origin, ray_direction);
                let scene_distance =
                    (scene_position - (ray_origin + ray_direction * scene_ray_distance)).length();
                update_best_pick(
                    best,
                    scene_distance / tolerance,
                    scene_ray_distance,
                    SceneObjectPick::Tract {
                        object_index,
                        object_name: object.name.clone(),
                        bundle: bundle_label.clone(),
                        tract_id: tract.id,
                        length: tract_length,
                        position: local_position.to_array(),
                    },
                );
            }
        }
    }
}

fn update_best_pick(
    best: &mut Option<(f32, f32, SceneObjectPick)>,
    normalized_distance: f32,
    ray_distance: f32,
    pick: SceneObjectPick,
) {
    if ray_distance < 0.0 {
        return;
    }
    let replace = best.as_ref().is_none_or(|(best_distance, best_ray, _)| {
        normalized_distance < *best_distance - 0.05
            || ((normalized_distance - *best_distance).abs() <= 0.05 && ray_distance < *best_ray)
    });
    if replace {
        *best = Some((normalized_distance, ray_distance, pick));
    }
}

fn point_ray_distance(point: Vec3, origin: Vec3, direction: Vec3) -> (f32, f32) {
    let ray_distance = (point - origin).dot(direction).max(0.0);
    let closest = origin + direction * ray_distance;
    (point.distance(closest), ray_distance)
}

fn segment_ray_distance(
    start: Vec3,
    end: Vec3,
    ray_origin: Vec3,
    ray_direction: Vec3,
) -> (f32, f32, f32) {
    let segment = end - start;
    let segment_length_squared = segment.length_squared();
    if segment_length_squared <= f32::EPSILON {
        let (distance, ray_distance) = point_ray_distance(start, ray_origin, ray_direction);
        return (distance, ray_distance, 0.0);
    }
    let origin_to_start = ray_origin - start;
    let b = ray_direction.dot(segment);
    let d = ray_direction.dot(origin_to_start);
    let e = segment.dot(origin_to_start);
    let denominator = segment_length_squared - b * b;
    let mut segment_fraction = if denominator.abs() > f32::EPSILON {
        ((e - b * d) / denominator).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut ray_distance = (b * segment_fraction - d).max(0.0);
    segment_fraction = ((e + b * ray_distance) / segment_length_squared).clamp(0.0, 1.0);
    ray_distance = (b * segment_fraction - d).max(0.0);
    let segment_point = start + segment * segment_fraction;
    let ray_point = ray_origin + ray_direction * ray_distance;
    (
        segment_point.distance(ray_point),
        ray_distance,
        segment_fraction,
    )
}

fn ray_intersects_aabb(
    origin: Vec3,
    direction: Vec3,
    min: [f32; 3],
    max: [f32; 3],
    padding: f32,
) -> bool {
    let min = Vec3::from_array(min) - Vec3::splat(padding);
    let max = Vec3::from_array(max) + Vec3::splat(padding);
    let mut near: f32 = 0.0;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        if direction[axis].abs() <= f32::EPSILON {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return false;
            }
            continue;
        }
        let inverse = 1.0 / direction[axis];
        let mut first = (min[axis] - origin[axis]) * inverse;
        let mut second = (max[axis] - origin[axis]) * inverse;
        if first > second {
            std::mem::swap(&mut first, &mut second);
        }
        near = near.max(first);
        far = far.min(second);
        if far < near {
            return false;
        }
    }
    far >= 0.0
}

pub(super) struct SceneObjectRenderer {
    ribbon_pipeline: wgpu::RenderPipeline,
    node_pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    uniform_layout: wgpu::BindGroupLayout,
    mask_vertex_buffer: wgpu::Buffer,
    mask_index_buffer: wgpu::Buffer,
    mask_index_count: u32,
}

pub(super) struct SceneObjectGpu {
    chunks: Vec<SceneObjectChunk>,
    node_buffer: Option<wgpu::Buffer>,
    node_count: u32,
    mask_instance_buffer: Option<wgpu::Buffer>,
    mask_instance_count: u32,
    tract_mask_buffer: wgpu::Buffer,
    pub(super) uniform_buffer: wgpu::Buffer,
    pub(super) bind_group: wgpu::BindGroup,
}

struct SceneObjectChunk {
    segment_buffer: wgpu::Buffer,
    segment_count: u32,
}

#[derive(Debug, Clone, Copy)]
struct SceneSegment {
    start: [f32; 3],
    end: [f32; 3],
    color: [f32; 4],
    tract_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GraphEdgeIds {
    primary: i32,
    alternate: Option<i32>,
}

impl SceneObjectRenderer {
    pub(super) fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene object ribbon uniform layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene object ribbon pipeline layout"),
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene object ribbon shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("scene_objects.wgsl"))),
        });
        let ribbon_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene object ribbon pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: SEGMENT_STRIDE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &SEGMENT_ATTRIBUTES,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let node_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene object node pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("node_vs"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: NODE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &NODE_ATTRIBUTES,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("node_fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let mask_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tract mask sphere pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("mask_vs"),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &MASK_VERTEX_ATTRIBUTES,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: MASK_INSTANCE_STRIDE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &MASK_INSTANCE_ATTRIBUTES,
                    }),
                ],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("mask_fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let (mask_vertices, mask_indices) = unit_sphere_mesh(16, 24);
        let mask_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tract mask sphere vertices"),
            contents: &f32_bytes(&mask_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mask_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tract mask sphere indices"),
            contents: &u32_bytes(&mask_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            ribbon_pipeline,
            node_pipeline,
            mask_pipeline,
            uniform_layout,
            mask_vertex_buffer,
            mask_index_buffer,
            mask_index_count: mask_indices.len() as u32,
        }
    }

    pub(super) fn upload(&self, device: &wgpu::Device, object: &SceneObject) -> SceneObjectGpu {
        // Keep CPU staging bounded even for the 81 MB whole-brain FATCAT
        // fixture. The object model and one modest upload chunk are resident;
        // we never build a second, fully expanded triangle copy.
        const TARGET_CHUNK_BYTES: usize = 32 * 1024 * 1024;
        let max_chunk_bytes = (device.limits().max_buffer_size as usize)
            .min(TARGET_CHUNK_BYTES)
            .max(SEGMENT_FLOATS * 4);
        let max_segments = (max_chunk_bytes / (SEGMENT_FLOATS * 4)).max(1);
        let mut chunks = Vec::new();
        let mut floats = Vec::with_capacity(max_segments * SEGMENT_FLOATS);
        object.visit_segments(|segment| {
            floats.extend_from_slice(&segment.start);
            floats.extend_from_slice(&segment.end);
            floats.extend_from_slice(&segment.color);
            floats.push(f32::from_bits(segment.tract_index));
            if floats.len() >= max_segments * SEGMENT_FLOATS {
                chunks.push(upload_segment_chunk(device, &floats));
                floats.clear();
            }
        });
        if !floats.is_empty() {
            chunks.push(upload_segment_chunk(device, &floats));
        }
        let (node_buffer, node_count) = match &object.payload {
            SceneObjectPayload::Graph(data) => {
                let mut nodes = Vec::with_capacity(data.nodes.len() * NODE_FLOATS);
                for (index, node) in data.nodes.iter().enumerate() {
                    nodes.extend_from_slice(&node.position);
                    nodes.extend_from_slice(
                        &stable_label_color(node.index.max(index as i32 + 1), 255).to_array(),
                    );
                }
                let buffer = (!nodes.is_empty()).then(|| {
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("scene object graph node buffer"),
                        contents: &f32_bytes(&nodes),
                        usage: wgpu::BufferUsages::VERTEX,
                    })
                });
                (buffer, data.nodes.len() as u32)
            }
            SceneObjectPayload::Tracts(_) => (None, 0),
        };
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene object ribbon uniform buffer"),
            size: 160,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mask_flags = object_mask_flags(object);
        let tract_mask_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene object tract mask flags"),
            contents: &u32_bytes(mask_flags),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene object ribbon bind group"),
            layout: &self.uniform_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: tract_mask_buffer.as_entire_binding(),
                },
            ],
        });
        let (mask_instance_buffer, mask_instance_count) = upload_mask_instances(device, object);
        SceneObjectGpu {
            chunks,
            node_buffer,
            node_count,
            mask_instance_buffer,
            mask_instance_count,
            tract_mask_buffer,
            uniform_buffer,
            bind_group,
        }
    }

    pub(super) fn update(
        queue: &wgpu::Queue,
        gpu: &SceneObjectGpu,
        object: &SceneObject,
        view_projection: Mat4,
        model: Mat4,
        viewport: [f32; 2],
        scale_factor: f32,
    ) {
        let mut floats = Vec::with_capacity(40);
        floats.extend_from_slice(&view_projection.to_cols_array());
        floats.extend_from_slice(&model.to_cols_array());
        floats.extend_from_slice(&viewport);
        floats.extend_from_slice(&[
            object.width_points.max(0.25) * scale_factor,
            RIBBON_FEATHER_POINTS * scale_factor,
        ]);
        floats.extend_from_slice(&[
            object.opacity.clamp(0.0, 1.0),
            object.graph_node_size_points.max(1.0) * scale_factor,
            0.0,
            0.0,
        ]);
        queue.write_buffer(&gpu.uniform_buffer, 0, &f32_bytes(&floats));
    }

    pub(super) fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, gpu: &'a SceneObjectGpu) {
        pass.set_pipeline(&self.ribbon_pipeline);
        pass.set_bind_group(0, &gpu.bind_group, &[]);
        for chunk in &gpu.chunks {
            pass.set_vertex_buffer(0, chunk.segment_buffer.slice(..));
            pass.draw(0..6, 0..chunk.segment_count);
        }
        if let Some(buffer) = &gpu.node_buffer {
            pass.set_pipeline(&self.node_pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..gpu.node_count);
        }
        if let Some(buffer) = &gpu.mask_instance_buffer {
            pass.set_pipeline(&self.mask_pipeline);
            pass.set_vertex_buffer(0, self.mask_vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, buffer.slice(..));
            pass.set_index_buffer(self.mask_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.mask_index_count, 0, 0..gpu.mask_instance_count);
        }
    }

    pub(super) fn refresh_masks(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        gpu: &mut SceneObjectGpu,
        object: &SceneObject,
    ) {
        let (buffer, count) = upload_mask_instances(device, object);
        gpu.mask_instance_buffer = buffer;
        gpu.mask_instance_count = count;
        queue.write_buffer(
            &gpu.tract_mask_buffer,
            0,
            &u32_bytes(object_mask_flags(object)),
        );
    }
}

fn object_mask_flags(object: &SceneObject) -> &[u32] {
    if matches!(object.payload, SceneObjectPayload::Tracts(_)) {
        &object.tract_mask_analysis.display_flags
    } else {
        &[1]
    }
}

fn upload_mask_instances(
    device: &wgpu::Device,
    object: &SceneObject,
) -> (Option<wgpu::Buffer>, u32) {
    let mut floats = Vec::with_capacity(object.tract_masks.len() * MASK_INSTANCE_FLOATS);
    for (index, mask) in object.tract_masks.iter().enumerate() {
        floats.extend_from_slice(&mask.center);
        floats.push(mask.radius);
        floats.extend_from_slice(&mask.color);
        floats.push(f32::from(object.tract_mask_selected == Some(index)));
    }
    let buffer = (!floats.is_empty()).then(|| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tract mask sphere instances"),
            contents: &f32_bytes(&floats),
            usage: wgpu::BufferUsages::VERTEX,
        })
    });
    (buffer, object.tract_masks.len() as u32)
}

fn unit_sphere_mesh(stacks: u32, slices: u32) -> (Vec<f32>, Vec<u32>) {
    let mut vertices = Vec::new();
    for stack in 0..=stacks {
        let latitude = std::f32::consts::PI * stack as f32 / stacks as f32;
        let y = latitude.cos();
        let ring = latitude.sin();
        for slice in 0..=slices {
            let longitude = std::f32::consts::TAU * slice as f32 / slices as f32;
            vertices.extend_from_slice(&[ring * longitude.cos(), y, ring * longitude.sin()]);
        }
    }
    let mut indices = Vec::new();
    let row = slices + 1;
    for stack in 0..stacks {
        for slice in 0..slices {
            let first = stack * row + slice;
            let second = first + row;
            indices.extend_from_slice(&[first, second, first + 1, first + 1, second, second + 1]);
        }
    }
    (vertices, indices)
}

fn u32_bytes(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect()
}

fn upload_segment_chunk(device: &wgpu::Device, floats: &[f32]) -> SceneObjectChunk {
    let segment_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("scene object segment buffer"),
        contents: &f32_bytes(floats),
        usage: wgpu::BufferUsages::VERTEX,
    });
    SceneObjectChunk {
        segment_buffer,
        segment_count: (floats.len() / SEGMENT_FLOATS) as u32,
    }
}

fn visit_tract_segments(
    data: &TractographyDataset,
    mode: TractColorMode,
    appearances: &[TractBundleAppearance],
    mut visitor: impl FnMut(SceneSegment),
) {
    let mut flat_index = 0_u32;
    for (bundle_index, bundle) in data.bundles.iter().enumerate() {
        let appearance = appearances.get(bundle_index);
        let bundle_visible = !appearance.is_some_and(|appearance| !appearance.visible);
        let bundle_opacity = appearance.map_or(1.0, |appearance| appearance.opacity);
        let bundle_color =
            stable_label_color(bundle.tag.unwrap_or(bundle_index as i32 + 1), 255).to_array();
        for tract in &bundle.tracts {
            let tract_index = flat_index;
            flat_index += 1;
            if !bundle_visible {
                continue;
            }
            let tract_color = tract
                .points
                .first()
                .zip(tract.points.last())
                .map(|(a, b)| orientation_color(*a, *b))
                .unwrap_or([0.8, 0.8, 0.8, 1.0]);
            for pair in tract.points.windows(2) {
                let mut color = match mode {
                    TractColorMode::LocalOrientation => orientation_color(pair[0], pair[1]),
                    TractColorMode::MidpointOrientation => tract_color,
                    TractColorMode::Bundle => bundle_color,
                };
                color[3] *= bundle_opacity;
                visitor(SceneSegment {
                    start: pair[0],
                    end: pair[1],
                    color,
                    tract_index,
                });
            }
        }
    }
}

fn visit_graph_segments(
    data: &GraphDataset,
    linked_tracts: Option<&TractographyDataset>,
    geometry: GraphEdgeGeometry,
    measure: usize,
    threshold: f32,
    color_mode: GraphColorMode,
    selected_cell: Option<(usize, usize)>,
    mut visitor: impl FnMut(SceneSegment),
) {
    if measure >= data.edge_column_count {
        return;
    }
    let max_abs = data
        .column_range(measure)
        .map(|(min, max)| min.abs().max(max.abs()))
        .unwrap_or(1.0)
        .max(f32::EPSILON);
    visit_graph_edges(data, measure, |source, target, value, edge_ids| {
        let selected = selected_cell.is_some_and(|(row, column)| {
            (row == source && column == target) || (row == target && column == source)
        });
        if !value.is_finite() || (!selected && (value == 0.0 || value.abs() < threshold.max(0.0))) {
            return;
        }
        let mut color = if selected {
            [1.0, 0.88, 0.08, 1.0]
        } else {
            graph_value_color(value, max_abs, color_mode)
        };
        if selected_cell.is_some() && !selected {
            color[3] *= 0.28;
        }
        let linked_bundle = (geometry == GraphEdgeGeometry::LinkedBundles)
            .then(|| linked_tracts.and_then(|tracts| linked_bundle_for_edge(tracts, edge_ids)))
            .flatten();
        if let Some(bundle) = linked_bundle {
            for tract in &bundle.tracts {
                for pair in tract.points.windows(2) {
                    visitor(SceneSegment {
                        start: pair[0],
                        end: pair[1],
                        color,
                        tract_index: u32::MAX,
                    });
                }
            }
        } else {
            visitor(SceneSegment {
                start: data.nodes[source].position,
                end: data.nodes[target].position,
                color,
                tract_index: u32::MAX,
            });
        }
    });
}

fn linked_bundle_for_edge<'a>(
    tracts: &'a TractographyDataset,
    edge_ids: GraphEdgeIds,
) -> Option<&'a crate::tractography::TractBundle> {
    linked_bundle_index_for_edge(tracts, edge_ids).map(|index| &tracts.bundles[index])
}

fn linked_bundle_index_for_edge(
    tracts: &TractographyDataset,
    edge_ids: GraphEdgeIds,
) -> Option<usize> {
    tracts.bundles.iter().position(|bundle| {
        [bundle.tag, bundle.alternate_tag]
            .into_iter()
            .flatten()
            .any(|tag| tag == edge_ids.primary || edge_ids.alternate == Some(tag))
    })
}

fn visit_graph_edges(
    data: &GraphDataset,
    measure: usize,
    mut visitor: impl FnMut(usize, usize, f32, GraphEdgeIds),
) {
    if measure >= data.edge_column_count {
        return;
    }
    if data.matrix_shape == GraphMatrixShape::Full {
        for source in 0..data.nodes.len() {
            for target in source + 1..data.nodes.len() {
                let value = data
                    .full_edge_row(source, target)
                    .and_then(|row| row.get(measure))
                    .copied()
                    .into_iter()
                    .chain(
                        data.full_edge_row(target, source)
                            .and_then(|row| row.get(measure))
                            .copied(),
                    )
                    .filter(|value| value.is_finite())
                    .max_by(|a, b| a.abs().total_cmp(&b.abs()))
                    .unwrap_or(0.0);
                visitor(
                    source,
                    target,
                    value,
                    GraphEdgeIds {
                        primary: (target * data.nodes.len() + source) as i32,
                        alternate: Some((source * data.nodes.len() + target) as i32),
                    },
                );
            }
        }
    } else {
        for row in 0..data.edge_values.len() / data.edge_column_count.max(1) {
            let Some((source, target)) = data.edge_endpoints(row) else {
                continue;
            };
            if source >= data.nodes.len() || target >= data.nodes.len() || source == target {
                continue;
            }
            let value = data
                .edge_row(row)
                .and_then(|values| values.get(measure))
                .copied()
                .unwrap_or(0.0);
            let primary = if data.matrix_shape == GraphMatrixShape::Sparse {
                data.edge_indices
                    .get(row)
                    .map(|indices| indices[0])
                    .unwrap_or(row as i32)
            } else {
                row as i32
            };
            visitor(
                source,
                target,
                value,
                GraphEdgeIds {
                    primary,
                    alternate: None,
                },
            );
        }
    }
}

pub(super) fn graph_value_color(value: f32, max_abs: f32, mode: GraphColorMode) -> [f32; 4] {
    let strength = (value.abs() / max_abs.max(f32::EPSILON)).clamp(0.0, 1.0);
    match mode {
        GraphColorMode::Signed if value < 0.0 => [0.12 + 0.45 * (1.0 - strength), 0.35, 1.0, 0.78],
        GraphColorMode::Signed => [1.0, 0.2 + 0.45 * (1.0 - strength), 0.12, 0.78],
        GraphColorMode::Magnitude => [0.12 + 0.88 * strength, 0.78, 1.0 - 0.7 * strength, 0.78],
    }
}

fn orientation_color(start: [f32; 3], end: [f32; 3]) -> [f32; 4] {
    let direction = (Vec3::from_array(end) - Vec3::from_array(start))
        .normalize_or_zero()
        .abs();
    if direction.length_squared() <= f32::EPSILON {
        [0.65, 0.65, 0.65, 1.0]
    } else {
        [direction.x, direction.y, direction.z, 1.0]
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub(super) fn normalization_model(bounds: SpatialBounds) -> Mat4 {
    Mat4::from_scale(Vec3::splat(1.0 / bounds.radius.max(f32::EPSILON)))
        * Mat4::from_translation(-Vec3::from_array(bounds.center))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tractography::{Tract, TractBundle};

    #[test]
    fn local_orientation_uses_absolute_xyz_as_rgb() {
        assert_eq!(
            orientation_color([0.0; 3], [-2.0, 0.0, 0.0]),
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(
            orientation_color([0.0; 3], [0.0, 0.0, 4.0]),
            [0.0, 0.0, 1.0, 1.0]
        );
    }

    #[test]
    fn hidden_and_translucent_bundles_affect_generated_segments() {
        let data = TractographyDataset {
            path: None,
            bundles: vec![
                TractBundle {
                    tag: Some(1),
                    alternate_tag: None,
                    ends: None,
                    tracts: vec![Tract {
                        id: 1,
                        points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                    }],
                },
                TractBundle {
                    tag: Some(2),
                    alternate_tag: None,
                    ends: None,
                    tracts: vec![Tract {
                        id: 2,
                        points: vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    }],
                },
            ],
            bounds: SpatialBounds::from_points([[0.0, 0.0, 0.0], [1.0, 1.0, 0.0]].iter()).unwrap(),
        };
        let appearances = vec![
            TractBundleAppearance {
                visible: false,
                opacity: 1.0,
            },
            TractBundleAppearance {
                visible: true,
                opacity: 0.25,
            },
        ];
        let mut segments = Vec::new();
        visit_tract_segments(
            &data,
            TractColorMode::LocalOrientation,
            &appearances,
            |segment| segments.push(segment),
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].color, [0.0, 1.0, 0.0, 0.25]);
    }

    #[test]
    fn graph_uses_selected_measure_and_absolute_threshold() {
        let data = GraphDataset {
            path: None,
            nodes: vec![
                crate::graph_dataset::GraphNode {
                    index: 0,
                    position: [0.0, 0.0, 0.0],
                    label: "A".into(),
                },
                crate::graph_dataset::GraphNode {
                    index: 1,
                    position: [1.0, 0.0, 0.0],
                    label: "B".into(),
                },
            ],
            // Full matrices are target-major. Only the 1 -> 0 row has values.
            edge_values: vec![0.0, 0.0, 0.2, -0.8, 0.0, 0.0, 0.0, 0.0],
            edge_column_count: 2,
            edge_labels: vec!["weak".into(), "strong".into()],
            matrix_shape: GraphMatrixShape::Full,
            edge_indices: Vec::new(),
            edge_positions: Vec::new(),
            network_file: None,
        };
        let mut weak = Vec::new();
        visit_graph_segments(
            &data,
            None,
            GraphEdgeGeometry::Straight,
            0,
            0.5,
            GraphColorMode::Signed,
            None,
            |segment| weak.push(segment),
        );
        assert!(weak.is_empty());

        let mut strong = Vec::new();
        visit_graph_segments(
            &data,
            None,
            GraphEdgeGeometry::Straight,
            1,
            0.5,
            GraphColorMode::Signed,
            None,
            |segment| strong.push(segment),
        );
        assert_eq!(strong.len(), 1);
        assert!(strong[0].color[2] > strong[0].color[0]);

        let mut selected = Vec::new();
        visit_graph_segments(
            &data,
            None,
            GraphEdgeGeometry::Straight,
            0,
            0.5,
            GraphColorMode::Signed,
            Some((1, 0)),
            |segment| selected.push(segment),
        );
        assert_eq!(
            selected.len(),
            1,
            "selection overrides the display threshold"
        );
        assert_eq!(selected[0].color, [1.0, 0.88, 0.08, 1.0]);
    }

    #[test]
    fn graph_pick_reports_the_displayed_measure_value() {
        let data = GraphDataset {
            path: None,
            nodes: vec![
                crate::graph_dataset::GraphNode {
                    index: 10,
                    position: [0.0, 0.0, 0.0],
                    label: "A".into(),
                },
                crate::graph_dataset::GraphNode {
                    index: 20,
                    position: [1.0, 0.0, 0.0],
                    label: "B".into(),
                },
            ],
            edge_values: vec![0.0, 0.0, 0.25, -0.75, 0.0, 0.0, 0.0, 0.0],
            edge_column_count: 2,
            edge_labels: vec!["weak".into(), "FA".into()],
            matrix_shape: GraphMatrixShape::Full,
            edge_indices: Vec::new(),
            edge_positions: Vec::new(),
            network_file: None,
        };
        let mut object = SceneObject::from_graph_with_linked_tracts(
            PathBuf::from("network.niml.dset"),
            data,
            None,
        )
        .unwrap();
        object.graph_measure = 1;
        let pick = pick_scene_objects(
            &[object],
            Mat4::IDENTITY,
            Vec3::new(0.5, 0.0, 3.0),
            -Vec3::Z,
            0.02,
        )
        .unwrap();
        assert!(matches!(
            pick,
            SceneObjectPick::GraphEdge {
                source,
                target,
                measure,
                value,
                ..
            } if source == "A" && target == "B" && measure == "FA" && value == -0.75
        ));
    }

    #[test]
    fn tract_bvh_pick_reports_bundle_and_tract() {
        let data = TractographyDataset {
            path: None,
            bundles: vec![TractBundle {
                tag: Some(7),
                alternate_tag: None,
                ends: Some("A-B".into()),
                tracts: vec![Tract {
                    id: 42,
                    points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                }],
            }],
            bounds: SpatialBounds::from_points([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]].iter()).unwrap(),
        };
        let object = SceneObject::from_tracts(PathBuf::from("tract.niml.tract"), data);
        assert_eq!(object.tract_pick_index.as_ref().unwrap().entries.len(), 1);
        let pick = pick_scene_objects(
            &[object],
            Mat4::IDENTITY,
            Vec3::new(0.5, 0.0, 3.0),
            -Vec3::Z,
            0.02,
        )
        .unwrap();
        assert!(matches!(
            pick,
            SceneObjectPick::Tract {
                bundle,
                tract_id: 42,
                ..
            } if bundle == "A-B"
        ));
    }

    #[test]
    fn hidden_bundle_is_not_pickable() {
        let data = TractographyDataset {
            path: None,
            bundles: vec![TractBundle {
                tag: Some(1),
                alternate_tag: None,
                ends: None,
                tracts: vec![Tract {
                    id: 2,
                    points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                }],
            }],
            bounds: SpatialBounds::from_points([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]].iter()).unwrap(),
        };
        let mut object = SceneObject::from_tracts(PathBuf::from("tract.niml.tract"), data);
        object.tract_bundles[0].visible = false;
        assert!(
            pick_scene_objects(
                &[object],
                Mat4::IDENTITY,
                Vec3::new(0.5, 0.0, 3.0),
                -Vec3::Z,
                0.02,
            )
            .is_none()
        );
    }

    #[test]
    fn mask_snap_can_pick_dimmed_tracts_outside_the_active_filter() {
        let data = TractographyDataset {
            path: None,
            bundles: vec![TractBundle {
                tag: Some(1),
                alternate_tag: None,
                ends: None,
                tracts: vec![Tract {
                    id: 2,
                    points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                }],
            }],
            bounds: SpatialBounds::from_points([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]].iter()).unwrap(),
        };
        let mut object = SceneObject::from_tracts(PathBuf::from("tract.niml.tract"), data);
        object.add_tract_mask([20.0, 0.0, 0.0]);
        assert_eq!(object.tract_mask_analysis.display_flags[0], 0);
        assert!(
            pick_scene_objects(
                std::slice::from_ref(&object),
                Mat4::IDENTITY,
                Vec3::new(0.5, 0.0, 3.0),
                -Vec3::Z,
                0.02,
            )
            .is_none()
        );
        assert!(
            pick_tract_on_object_unfiltered(
                &[object],
                0,
                Mat4::IDENTITY,
                Vec3::new(0.5, 0.0, 3.0),
                -Vec3::Z,
                0.02,
            )
            .is_some()
        );
    }

    #[test]
    fn sphere_intersection_detects_a_crossing_segment_with_endpoints_outside() {
        assert!(tract_intersects_sphere(
            &[[-2.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            Vec3::ZERO,
            0.25,
        ));
        assert!(!tract_intersects_sphere(
            &[[-2.0, 2.0, 0.0], [2.0, 2.0, 0.0]],
            Vec3::ZERO,
            0.25,
        ));
    }

    #[test]
    fn mask_connection_matrix_uses_inclusive_pairwise_counts() {
        let data = TractographyDataset {
            path: None,
            bundles: vec![TractBundle {
                tag: Some(1),
                alternate_tag: None,
                ends: None,
                tracts: vec![
                    Tract {
                        id: 0,
                        points: vec![[-1.0, 0.0, 0.0], [11.0, 0.0, 0.0]],
                    },
                    Tract {
                        id: 1,
                        points: vec![[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                    },
                    Tract {
                        id: 2,
                        points: vec![[9.0, 0.0, 0.0], [21.0, 0.0, 0.0]],
                    },
                    Tract {
                        id: 3,
                        points: vec![[-1.0, 0.0, 0.0], [21.0, 0.0, 0.0]],
                    },
                ],
            }],
            bounds: SpatialBounds::from_points([[-1.0, 0.0, 0.0], [21.0, 0.0, 0.0]].iter())
                .unwrap(),
        };
        let index = TractPickIndex::build(&data);
        let masks = [0.0, 10.0, 20.0].map(|x| TractMask {
            name: format!("{x}"),
            center: [x, 0.0, 0.0],
            radius: 1.1,
            color: [1.0; 4],
        });
        let analysis =
            analyze_tract_masks(&data, Some(&index), &masks, TractMaskFilter::Pair(0, 2));
        assert_eq!(analysis.mask_counts, vec![3, 3, 2]);
        assert_eq!(analysis.pair_counts[0], vec![3, 2, 1]);
        assert_eq!(analysis.pair_counts[1], vec![2, 3, 2]);
        assert_eq!(analysis.pair_counts[2], vec![1, 2, 2]);
        assert_eq!(analysis.display_flags[..4], [0, 0, 0, 1]);
    }

    #[test]
    fn tract_mask_pick_returns_the_nearest_sphere() {
        let data = TractographyDataset {
            path: None,
            bundles: vec![TractBundle {
                tag: None,
                alternate_tag: None,
                ends: None,
                tracts: vec![Tract {
                    id: 1,
                    points: vec![[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                }],
            }],
            bounds: SpatialBounds::from_points([[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]].iter()).unwrap(),
        };
        let mut object = SceneObject::from_tracts(PathBuf::from("tract.niml.tract"), data);
        object.add_tract_mask([0.0, 0.0, 0.0]);
        object.tract_masks[0].radius = 0.5;
        let pick = pick_tract_masks(
            &[object],
            Mat4::IDENTITY,
            Vec3::new(0.0, 0.0, 3.0),
            -Vec3::Z,
        )
        .unwrap();
        assert_eq!(pick.object_index, 0);
        assert_eq!(pick.mask_index, 0);
        assert!((pick.distance - 2.5).abs() < 1.0e-5);
    }

    fn linked_graph_fixture(bundle_tag: i32) -> (GraphDataset, TractographyDataset) {
        let graph = GraphDataset {
            path: None,
            nodes: vec![
                crate::graph_dataset::GraphNode {
                    index: 0,
                    position: [0.0, 0.0, 0.0],
                    label: "A".into(),
                },
                crate::graph_dataset::GraphNode {
                    index: 1,
                    position: [1.0, 0.0, 0.0],
                    label: "B".into(),
                },
            ],
            edge_values: vec![0.0, 1.0, 0.0, 0.0],
            edge_column_count: 1,
            edge_labels: vec!["NT".into()],
            matrix_shape: GraphMatrixShape::Full,
            edge_indices: Vec::new(),
            edge_positions: Vec::new(),
            network_file: Some(PathBuf::from("tract.niml.tract")),
        };
        let tracts = TractographyDataset {
            path: None,
            bundles: vec![TractBundle {
                tag: Some(bundle_tag),
                alternate_tag: Some(2),
                ends: Some("A<->B".into()),
                tracts: vec![Tract {
                    id: 9,
                    points: vec![[0.0, 0.0, 0.0], [0.5, 0.5, 0.0], [1.0, 0.0, 0.0]],
                }],
            }],
            bounds: SpatialBounds::from_points(
                [[0.0, 0.0, 0.0], [0.5, 0.5, 0.0], [1.0, 0.0, 0.0]].iter(),
            )
            .unwrap(),
        };
        (graph, tracts)
    }

    #[test]
    fn linked_graph_bundles_match_suma_primary_and_alternate_edge_tags() {
        for tag in [1, 2] {
            let (graph, tracts) = linked_graph_fixture(tag);
            let mut segments = Vec::new();
            visit_graph_segments(
                &graph,
                Some(&tracts),
                GraphEdgeGeometry::LinkedBundles,
                0,
                0.0,
                GraphColorMode::Signed,
                None,
                |segment| segments.push(segment),
            );
            assert_eq!(segments.len(), 2);
            assert_eq!(segments[0].end, [0.5, 0.5, 0.0]);
        }
    }

    #[test]
    fn linked_graph_falls_back_to_straight_edge_without_matching_bundle() {
        let (graph, mut tracts) = linked_graph_fixture(99);
        tracts.bundles[0].alternate_tag = None;
        let mut segments = Vec::new();
        visit_graph_segments(
            &graph,
            Some(&tracts),
            GraphEdgeGeometry::LinkedBundles,
            0,
            0.0,
            GraphColorMode::Signed,
            None,
            |segment| segments.push(segment),
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start, [0.0, 0.0, 0.0]);
        assert_eq!(segments[0].end, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn linked_graph_bundle_pick_reports_graph_edge_value() {
        let (graph, tracts) = linked_graph_fixture(1);
        let mut object = SceneObject::from_graph_with_linked_tracts(
            PathBuf::from("network.niml.dset"),
            graph,
            Some(tracts),
        )
        .unwrap();
        object.graph_edge_geometry = GraphEdgeGeometry::LinkedBundles;
        let pick = pick_scene_objects(
            &[object],
            Mat4::IDENTITY,
            Vec3::new(0.5, 0.5, 3.0),
            -Vec3::Z,
            0.02,
        )
        .unwrap();
        assert!(matches!(
            pick,
            SceneObjectPick::GraphEdge {
                source,
                target,
                measure,
                value: 1.0,
                ..
            } if source == "A" && target == "B" && measure == "NT"
        ));
    }

    #[test]
    fn local_fatcat_linked_graph_tags_cover_nonzero_edges_when_available() {
        let graph_path = Path::new("/Users/molfesepj/FATCAT_DEMO/DTI/o.NETS_AND_000.niml.dset");
        let tract_path = Path::new("/Users/molfesepj/FATCAT_DEMO/DTI/o.NETS_AND_000.niml.tract");
        if !graph_path.exists() || !tract_path.exists() {
            return;
        }
        let graph = crate::graph_dataset::read_graph_bucket(graph_path).unwrap();
        let tracts = crate::tractography::read_niml_tract(tract_path).unwrap();
        let mut nonzero_edges = 0;
        let mut matched_edges = 0;
        visit_graph_edges(&graph, 0, |_, _, value, edge_ids| {
            if value != 0.0 {
                nonzero_edges += 1;
                if linked_bundle_index_for_edge(&tracts, edge_ids).is_some() {
                    matched_edges += 1;
                }
            }
        });
        assert_eq!(nonzero_edges, tracts.bundles.len());
        assert_eq!(matched_edges, nonzero_edges);
    }
}
