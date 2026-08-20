use std::collections::BTreeSet;
use std::ops::Range;

use glam::Vec3;

use crate::color::{ColorMap, stable_label_color};
use crate::command::OverlayThreshold;
#[cfg(test)]
use crate::overlay::Overlay;
use crate::overlay::{FadeSettings, Threshold, ThresholdMode};
use crate::surface::{SurfaceMesh, ValueRange};

pub(super) const DEFAULT_SURFACE_COLOR: [f32; 4] = [0.76, 0.78, 0.74, 1.0];
const SELECTED_FACE_COLOR: [f32; 4] = [0.1, 0.85, 1.0, 1.0];
const CROSSHAIR_COLOR: [f32; 4] = [1.0, 0.92, 0.12, 1.0];
const SELECTED_FACE_OFFSET: f32 = 0.003;
const CROSSHAIR_RADIUS: f32 = 0.025;

#[derive(Debug, Clone)]
pub(super) struct PreparedSurface {
    pub(super) vertices: Vec<PreparedVertex>,
    pub(super) indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedGeometry {
    pub(super) vertices: Vec<PreparedGeometryVertex>,
    pub(super) indices: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PreparedGeometryVertex {
    pub(super) position: [f32; 3],
    pub(super) normal: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PreparedVertex {
    pub(super) position: [f32; 3],
    pub(super) normal: [f32; 3],
    pub(super) color: [f32; 4],
}

/// Threshold contour geometry, expanded into screen-facing quads.
///
/// wgpu cannot draw lines wider than one physical pixel — WebGPU has no
/// line-width state at all — so an adjustable border has to be built from
/// triangles and widened in the vertex shader, where the segment direction can
/// be measured in screen space.
#[derive(Debug, Clone)]
pub(super) struct PreparedThresholdContour {
    pub(super) vertices: Vec<ContourVertex>,
    pub(super) indices: Vec<u32>,
}

/// One corner of a contour quad. Both segment endpoints travel with every
/// vertex so the shader can derive the screen-space perpendicular; `params`
/// carries which side of the centerline this corner sits on, which endpoint it
/// belongs to, and the background luminance used by auto-contrast coloring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ContourVertex {
    pub(super) segment_start: [f32; 3],
    pub(super) segment_end: [f32; 3],
    /// `[side, at_end, luminance]`, with `side` in `{-1, +1}` and `at_end` in
    /// `{0, 1}`.
    pub(super) params: [f32; 3],
}

/// User-facing appearance of the "B" contour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ThresholdContourStyle {
    /// Width of the inner line, in physical pixels.
    pub(super) width_px: f32,
    /// Extra width of the casing drawn behind the inner line, per side, in
    /// physical pixels. Zero draws a plain single line.
    pub(super) halo_px: f32,
    pub(super) color_mode: ContourColorMode,
    /// Inner line color, used when `color_mode` is [`ContourColorMode::Fixed`].
    pub(super) color: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContourColorMode {
    /// Choose black or white per fragment from the overlay color the contour
    /// sits on, so the line never vanishes into the colormap.
    AutoContrast,
    /// Use [`ThresholdContourStyle::color`] for the inner line.
    Fixed,
}

impl ThresholdContourStyle {
    pub(super) fn new() -> Self {
        Self {
            width_px: 2.0,
            halo_px: 1.5,
            color_mode: ContourColorMode::AutoContrast,
            color: [1.0, 1.0, 1.0],
        }
    }

    /// Width of the casing pass, which is the inner line widened on both sides.
    pub(super) fn halo_width_px(self) -> f32 {
        self.width_px.max(0.0) + self.halo_px.max(0.0) * 2.0
    }

    pub(super) fn draws_halo(self) -> bool {
        self.halo_px > 0.0
    }

    /// Color of the casing. It is the opposite pole from the inner line, so the
    /// pair reads against any background regardless of which line the eye
    /// catches first.
    pub(super) fn halo_color(self) -> [f32; 3] {
        if luminance(self.color) < 0.5 {
            [1.0, 1.0, 1.0]
        } else {
            [0.0, 0.0, 0.0]
        }
    }
}

impl Default for ThresholdContourStyle {
    fn default() -> Self {
        Self::new()
    }
}

/// Rec. 709 luminance.
pub(super) fn luminance(color: [f32; 3]) -> f32 {
    0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ThresholdContourPoint {
    position: [f32; 3],
    normal: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ThresholdContourSegment {
    start: ThresholdContourPoint,
    end: ThresholdContourPoint,
}

#[derive(Debug, Clone, Copy)]
struct ThresholdBoundary {
    value: f64,
    exact_is_above: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ContourPointKey {
    Vertex(u32),
    Edge(u32, u32),
}

const THRESHOLD_CONTOUR_OFFSET_FACTOR: f32 = 1.0e-4;
/// Fallback background luminance when the overlay colormap cannot be sampled
/// (discrete label overlays), chosen so auto-contrast picks a black line.
const THRESHOLD_CONTOUR_DEFAULT_LUMINANCE: f32 = 1.0;

#[derive(Debug, Clone)]
pub(super) struct RoiAppearance {
    pub(super) node_colors: Vec<Option<[f32; 4]>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SelectionHighlight {
    pub(super) node_index: u32,
    pub(super) face_index: usize,
    pub(super) crosshair_position: [f32; 3],
    pub(super) marker_radius: f32,
    pub(super) face_offset: f32,
}

impl SelectionHighlight {
    pub(super) fn normalized(
        node_index: u32,
        face_index: usize,
        crosshair_position: [f32; 3],
    ) -> Self {
        Self::scaled(node_index, face_index, crosshair_position, 1.0)
    }

    pub(super) fn scaled(
        node_index: u32,
        face_index: usize,
        crosshair_position: [f32; 3],
        scale: f32,
    ) -> Self {
        let scale = if scale.is_finite() && scale > f32::EPSILON {
            scale
        } else {
            1.0
        };

        Self {
            node_index,
            face_index,
            crosshair_position,
            marker_radius: CROSSHAIR_RADIUS * scale,
            face_offset: SELECTED_FACE_OFFSET * scale,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OverlayAppearance {
    pub(super) range: ValueRange,
    pub(super) symmetric_range: bool,
    pub(super) colormap: OverlayColorMap,
    pub(super) threshold: OverlayThreshold,
    pub(super) transparent_threshold: bool,
    pub(super) boxed_threshold: bool,
    pub(super) fade: FadeSettings,
    pub(super) contour: ThresholdContourStyle,
    pub(super) opacity: f32,
    pub(super) dim: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverlayColorMap {
    DiscreteLabels,
    SpectrumRedToBlue,
    SpectrumRedToBlueGap,
    SpectrumYellowToRed,
    SpectrumYellowToCyan,
    SpectrumYellowToCyanGap,
    ColorCircleAjj,
    ColorCircleZss,
    RedsAndBlues,
    RedsAndBluesWithGreen,
    AfniP2Spanned,
    BlueWhiteRed,
    Fire,
    Grayscale,
}

impl PreparedSurface {
    #[cfg(test)]
    pub(super) fn from_surface(
        surface: &SurfaceMesh,
        overlay: Option<&Overlay>,
        overlay_dim: f32,
    ) -> Self {
        let geometry = PreparedGeometry::from_surface(surface);
        Self::from_geometry_with_selection(&geometry, None, overlay, overlay_dim, None, None)
    }

    #[cfg(test)]
    pub(super) fn from_geometry_with_selection(
        geometry: &PreparedGeometry,
        surface_colors: Option<&[[f32; 4]]>,
        overlay: Option<&Overlay>,
        overlay_dim: f32,
        roi: Option<&RoiAppearance>,
        selection: Option<SelectionHighlight>,
    ) -> Self {
        Self::from_geometry_color_slices(
            geometry,
            surface_colors,
            overlay.map(|overlay| overlay.color_cache.colors.as_slice()),
            overlay_dim,
            roi.map(|roi| roi.node_colors.as_slice()),
            selection,
        )
    }

    pub(super) fn from_geometry_color_slices(
        geometry: &PreparedGeometry,
        surface_colors: Option<&[[f32; 4]]>,
        overlay_colors: Option<&[[f32; 4]]>,
        overlay_dim: f32,
        roi_colors: Option<&[Option<[f32; 4]>]>,
        selection: Option<SelectionHighlight>,
    ) -> Self {
        let mut vertices = geometry
            .vertices
            .iter()
            .enumerate()
            .map(|(index, vertex)| PreparedVertex {
                position: vertex.position,
                normal: vertex.normal,
                color: compose_vertex_color(
                    surface_colors
                        .and_then(|colors| colors.get(index))
                        .copied()
                        .unwrap_or(DEFAULT_SURFACE_COLOR),
                    overlay_colors.and_then(|colors| colors.get(index)).copied(),
                    overlay_dim,
                    roi_colors
                        .and_then(|colors| colors.get(index))
                        .copied()
                        .flatten(),
                ),
            })
            .collect();
        let mut indices = geometry.indices.clone();
        if let Some(selection) = selection {
            append_selection_highlight(&mut vertices, &mut indices, geometry, selection);
        }

        Self { vertices, indices }
    }

    pub(super) fn from_geometry_cell_colors(
        geometry: &PreparedGeometry,
        surface_colors: Option<&[[f32; 4]]>,
        roi_colors: Option<&[Option<[f32; 4]>]>,
        selection: Option<SelectionHighlight>,
    ) -> Self {
        let mut prepared = Self::from_geometry_cell_color_range(
            geometry,
            surface_colors,
            roi_colors,
            0..geometry.triangle_count(),
        );
        if let Some(selection) = selection {
            append_selection_highlight(
                &mut prepared.vertices,
                &mut prepared.indices,
                geometry,
                selection,
            );
        }

        prepared
    }

    pub(super) fn from_geometry_cell_color_range(
        geometry: &PreparedGeometry,
        surface_colors: Option<&[[f32; 4]]>,
        roi_colors: Option<&[Option<[f32; 4]>]>,
        triangle_range: Range<usize>,
    ) -> Self {
        let triangle_count = geometry.triangle_count();
        let start_triangle = triangle_range.start.min(triangle_count);
        let end_triangle = triangle_range.end.min(triangle_count).max(start_triangle);
        let triangle_indices = &geometry.indices[start_triangle * 3..end_triangle * 3];
        let mut vertices = Vec::with_capacity(triangle_indices.len());
        let mut indices = Vec::with_capacity(triangle_indices.len());

        for triangle in triangle_indices.chunks_exact(3) {
            let face_color = cell_color_for_triangle(triangle, surface_colors, roi_colors);
            let start = vertices.len() as u32;
            for index in triangle {
                if let Some(vertex) = geometry.vertices.get(*index as usize) {
                    vertices.push(PreparedVertex {
                        position: vertex.position,
                        normal: vertex.normal,
                        color: face_color,
                    });
                }
            }
            if vertices.len() >= start as usize + 3 {
                indices.extend_from_slice(&[start, start + 1, start + 2]);
            }
        }

        Self { vertices, indices }
    }

    pub(super) fn selection_highlight(
        geometry: &PreparedGeometry,
        selection: SelectionHighlight,
    ) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        append_selection_highlight(&mut vertices, &mut indices, geometry, selection);

        Self { vertices, indices }
    }

    pub(super) fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }

    pub(super) fn vertex_bytes(&self) -> Vec<u8> {
        let mut floats = Vec::with_capacity(self.vertices.len() * 6);

        for vertex in &self.vertices {
            floats.extend_from_slice(&vertex.position);
            floats.extend_from_slice(&vertex.normal);
        }

        super::f32_bytes(&floats)
    }

    pub(super) fn color_bytes(&self) -> Vec<u8> {
        prepared_vertex_color_bytes(self.vertices.iter().map(|vertex| vertex.color))
    }

    pub(super) fn cell_color_bytes_for_range(
        geometry: &PreparedGeometry,
        surface_colors: Option<&[[f32; 4]]>,
        roi_colors: Option<&[Option<[f32; 4]>]>,
        triangle_range: Range<usize>,
    ) -> Vec<u8> {
        let triangle_count = geometry.triangle_count();
        let start_triangle = triangle_range.start.min(triangle_count);
        let end_triangle = triangle_range.end.min(triangle_count).max(start_triangle);
        let triangle_indices = &geometry.indices[start_triangle * 3..end_triangle * 3];
        let colors = triangle_indices.chunks_exact(3).flat_map(|triangle| {
            let face_color = cell_color_for_triangle(triangle, surface_colors, roi_colors);
            [face_color; 3]
        });

        prepared_vertex_color_bytes(colors)
    }

    pub(super) fn index_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(self.indices.as_slice()));

        for index in &self.indices {
            bytes.extend_from_slice(&index.to_ne_bytes());
        }

        bytes
    }

    pub(super) fn line_index_count(&self) -> u32 {
        self.line_indices().len() as u32
    }

    pub(super) fn line_index_bytes(&self) -> Vec<u8> {
        indices_to_bytes(&self.line_indices())
    }

    pub(super) fn point_index_count(&self) -> u32 {
        self.vertices.len() as u32
    }

    pub(super) fn point_index_bytes(&self) -> Vec<u8> {
        let indices = progressive_point_indices(self.vertices.len() as u32);
        indices_to_bytes(&indices)
    }

    fn line_indices(&self) -> Vec<u32> {
        let mut seen = BTreeSet::new();
        let mut indices = Vec::new();

        for triangle in self.indices.chunks_exact(3) {
            for &(a, b) in &[
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                let edge = normalized_edge(a, b);
                if seen.insert(edge) {
                    indices.extend_from_slice(&[a, b]);
                }
            }
        }

        indices
    }

    pub(super) fn is_empty(&self) -> bool {
        self.vertices.is_empty() || self.indices.is_empty()
    }
}

impl PreparedThresholdContour {
    /// Builds contour quads for `threshold`.
    ///
    /// `boundary_luminances` supplies, per threshold boundary, the luminance of
    /// the overlay color the contour will sit on. It is only consulted by
    /// auto-contrast coloring; a missing entry falls back to a light
    /// background.
    pub(super) fn from_geometry(
        geometry: &PreparedGeometry,
        threshold_values: &[f32],
        threshold: Threshold,
        boundary_luminances: &[f32],
    ) -> Self {
        let segments = threshold_contour_segments(geometry, threshold_values, threshold);
        let normal_offset = contour_normal_offset(geometry);
        let mut vertices = Vec::with_capacity(segments.len() * 4);
        let mut indices = Vec::with_capacity(segments.len() * 6);

        for (boundary_index, segment) in segments {
            let luminance = boundary_luminances
                .get(boundary_index)
                .copied()
                .filter(|luminance| luminance.is_finite())
                .unwrap_or(THRESHOLD_CONTOUR_DEFAULT_LUMINANCE);

            // Lift the line off the surface along the local normal. This is a
            // world-space nudge; the render pass adds a slope-scaled depth bias
            // on top, which is what actually keeps the line solid on faces
            // angled away from the camera.
            let start = (Vec3::from_array(segment.start.position)
                + Vec3::from_array(segment.start.normal) * normal_offset)
                .to_array();
            let end = (Vec3::from_array(segment.end.position)
                + Vec3::from_array(segment.end.normal) * normal_offset)
                .to_array();

            let base = vertices.len() as u32;
            for (side, at_end) in [(1.0, 0.0), (-1.0, 0.0), (1.0, 1.0), (-1.0, 1.0)] {
                vertices.push(ContourVertex {
                    segment_start: start,
                    segment_end: end,
                    params: [side, at_end, luminance],
                });
            }
            // Two triangles per segment. Winding is irrelevant because the
            // contour pipeline disables culling.
            indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        }

        Self { vertices, indices }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.vertices.is_empty() || self.indices.is_empty()
    }

    pub(super) fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }

    pub(super) fn vertex_bytes(&self) -> Vec<u8> {
        let mut floats = Vec::with_capacity(self.vertices.len() * CONTOUR_VERTEX_FLOATS);
        for vertex in &self.vertices {
            floats.extend_from_slice(&vertex.segment_start);
            floats.extend_from_slice(&vertex.segment_end);
            floats.extend_from_slice(&vertex.params);
        }
        super::f32_bytes(&floats)
    }

    pub(super) fn index_bytes(&self) -> Vec<u8> {
        indices_to_bytes(&self.indices)
    }
}

/// Floats per contour vertex: two endpoints plus the packed params triple.
pub(super) const CONTOUR_VERTEX_FLOATS: usize = 9;

fn threshold_contour_segments(
    geometry: &PreparedGeometry,
    threshold_values: &[f32],
    threshold: Threshold,
) -> Vec<(usize, ThresholdContourSegment)> {
    if geometry.vertices.len() != threshold_values.len() {
        return Vec::new();
    }

    let boundaries = threshold_boundaries(threshold);
    let mut segments = Vec::new();
    let mut emitted = BTreeSet::new();
    for (boundary_index, boundary) in boundaries.into_iter().enumerate() {
        for triangle in geometry.indices.chunks_exact(3) {
            let nodes = [triangle[0], triangle[1], triangle[2]];
            let Some(values) = nodes
                .map(|node| threshold_values.get(node as usize).copied())
                .into_iter()
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if values.iter().any(|value| !value.is_finite()) {
                continue;
            }

            let mut crossings = Vec::with_capacity(2);
            for (a_index, b_index) in [(0, 1), (1, 2), (2, 0)] {
                let a_value = f64::from(values[a_index]);
                let b_value = f64::from(values[b_index]);
                let a_above = if boundary.exact_is_above {
                    a_value >= boundary.value
                } else {
                    a_value > boundary.value
                };
                let b_above = if boundary.exact_is_above {
                    b_value >= boundary.value
                } else {
                    b_value > boundary.value
                };
                if a_above == b_above {
                    continue;
                }

                let a_node = nodes[a_index];
                let b_node = nodes[b_index];
                let key = if a_value == boundary.value {
                    ContourPointKey::Vertex(a_node)
                } else if b_value == boundary.value {
                    ContourPointKey::Vertex(b_node)
                } else {
                    ContourPointKey::Edge(a_node.min(b_node), a_node.max(b_node))
                };
                if crossings.iter().any(|(existing, _)| *existing == key) {
                    continue;
                }

                let denominator = b_value - a_value;
                if !denominator.is_finite() || denominator.abs() <= f64::EPSILON {
                    continue;
                }
                let t = ((boundary.value - a_value) / denominator).clamp(0.0, 1.0) as f32;
                let Some(a_vertex) = geometry.vertices.get(a_node as usize) else {
                    continue;
                };
                let Some(b_vertex) = geometry.vertices.get(b_node as usize) else {
                    continue;
                };
                let position = Vec3::from_array(a_vertex.position)
                    .lerp(Vec3::from_array(b_vertex.position), t)
                    .to_array();
                let normal = Vec3::from_array(a_vertex.normal)
                    .lerp(Vec3::from_array(b_vertex.normal), t)
                    .try_normalize()
                    .unwrap_or(Vec3::Z)
                    .to_array();
                crossings.push((key, ThresholdContourPoint { position, normal }));
            }

            if crossings.len() != 2 {
                continue;
            }
            let (first_key, first) = crossings[0];
            let (second_key, second) = crossings[1];
            let ordered_keys = if first_key <= second_key {
                (first_key, second_key)
            } else {
                (second_key, first_key)
            };
            if emitted.insert((boundary_index, ordered_keys)) {
                segments.push((
                    boundary_index,
                    ThresholdContourSegment {
                        start: first,
                        end: second,
                    },
                ));
            }
        }
    }

    segments
}

fn threshold_boundaries(threshold: Threshold) -> Vec<ThresholdBoundary> {
    let Some(range) = threshold.range else {
        return Vec::new();
    };
    match threshold.mode {
        ThresholdMode::Off => Vec::new(),
        ThresholdMode::Above => vec![ThresholdBoundary {
            value: range.min,
            exact_is_above: true,
        }],
        ThresholdMode::Below => vec![ThresholdBoundary {
            value: range.max,
            exact_is_above: false,
        }],
        ThresholdMode::Between if range.min == range.max => vec![ThresholdBoundary {
            value: range.min,
            exact_is_above: true,
        }],
        ThresholdMode::Between => vec![
            ThresholdBoundary {
                value: range.min,
                exact_is_above: true,
            },
            ThresholdBoundary {
                value: range.max,
                exact_is_above: false,
            },
        ],
        ThresholdMode::Outside if range.min == range.max => Vec::new(),
        ThresholdMode::Outside => vec![
            ThresholdBoundary {
                value: range.min,
                exact_is_above: false,
            },
            ThresholdBoundary {
                value: range.max,
                exact_is_above: true,
            },
        ],
    }
}

/// Luminance of the overlay color at each threshold boundary, in the same order
/// [`threshold_boundaries`] returns them.
///
/// The contour sits exactly on the threshold, so the color it has to stand out
/// against is the colormap sampled at the boundary value. Two-sided thresholds
/// get one entry per tail, which matters because the two tails are usually
/// opposite ends of the colormap.
pub(super) fn threshold_boundary_luminances(
    threshold: Threshold,
    range: ValueRange,
    colormap: OverlayColorMap,
) -> Vec<f32> {
    let Some(ColorMap::Continuous(map)) = colormap.continuous_color_map() else {
        return Vec::new();
    };
    let span = f64::from(range.max) - f64::from(range.min);
    threshold_boundaries(threshold)
        .into_iter()
        .map(|boundary| {
            if !span.is_finite() || span == 0.0 {
                return THRESHOLD_CONTOUR_DEFAULT_LUMINANCE;
            }
            let normalized = ((boundary.value - f64::from(range.min)) / span) as f32;
            let color = map.sample(normalized.clamp(0.0, 1.0)).to_array();
            luminance([color[0], color[1], color[2]])
        })
        .collect()
}

fn contour_normal_offset(geometry: &PreparedGeometry) -> f32 {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for vertex in &geometry.vertices {
        let position = Vec3::from_array(vertex.position);
        min = min.min(position);
        max = max.max(position);
    }
    let diagonal = (max - min).length();
    if diagonal.is_finite() {
        diagonal.max(1.0) * THRESHOLD_CONTOUR_OFFSET_FACTOR
    } else {
        THRESHOLD_CONTOUR_OFFSET_FACTOR
    }
}

fn prepared_vertex_color_bytes(colors: impl Iterator<Item = [f32; 4]>) -> Vec<u8> {
    let mut floats = Vec::new();
    for color in colors {
        floats.extend_from_slice(&color);
    }

    super::f32_bytes(&floats)
}

pub(super) fn color_bytes(colors: impl Iterator<Item = [f32; 4]>) -> Vec<u8> {
    prepared_vertex_color_bytes(colors)
}

impl RoiAppearance {
    pub(super) fn empty(node_count: usize) -> Self {
        Self {
            node_colors: vec![None; node_count],
        }
    }

    pub(super) fn set_node_color(&mut self, node: u32, color: [f32; 4]) -> bool {
        let Some(slot) = self.node_colors.get_mut(node as usize) else {
            return false;
        };
        *slot = Some(color);
        true
    }
}

fn append_selection_highlight(
    vertices: &mut Vec<PreparedVertex>,
    indices: &mut Vec<u32>,
    geometry: &PreparedGeometry,
    selection: SelectionHighlight,
) {
    append_selected_face(
        vertices,
        indices,
        geometry,
        selection.face_index,
        selection.face_offset,
    );
    append_crosshair_marker(
        vertices,
        indices,
        selection.crosshair_position,
        selection.marker_radius,
    );
    append_selected_node_marker(
        vertices,
        indices,
        geometry,
        selection.node_index,
        selection.marker_radius,
    );
}

fn append_selected_face(
    vertices: &mut Vec<PreparedVertex>,
    indices: &mut Vec<u32>,
    geometry: &PreparedGeometry,
    face_index: usize,
    face_offset: f32,
) -> Option<()> {
    let base_index = face_index.checked_mul(3)?;
    let face_indices = [
        *geometry.indices.get(base_index)?,
        *geometry.indices.get(base_index + 1)?,
        *geometry.indices.get(base_index + 2)?,
    ];
    let face_vertices = [
        *geometry.vertices.get(face_indices[0] as usize)?,
        *geometry.vertices.get(face_indices[1] as usize)?,
        *geometry.vertices.get(face_indices[2] as usize)?,
    ];
    let face_normal = (Vec3::from_array(face_vertices[0].normal)
        + Vec3::from_array(face_vertices[1].normal)
        + Vec3::from_array(face_vertices[2].normal))
    .normalize_or_zero();
    let offset = face_normal * face_offset;
    let start = vertices.len() as u32;
    for vertex in face_vertices {
        vertices.push(PreparedVertex {
            position: (Vec3::from_array(vertex.position) + offset).to_array(),
            normal: vertex.normal,
            color: SELECTED_FACE_COLOR,
        });
    }
    indices.extend_from_slice(&[start, start + 1, start + 2]);

    Some(())
}

fn append_selected_node_marker(
    vertices: &mut Vec<PreparedVertex>,
    indices: &mut Vec<u32>,
    geometry: &PreparedGeometry,
    node_index: u32,
    marker_radius: f32,
) -> Option<()> {
    let vertex = geometry.vertices.get(node_index as usize)?;
    append_crosshair_marker(vertices, indices, vertex.position, marker_radius);
    Some(())
}

fn append_crosshair_marker(
    vertices: &mut Vec<PreparedVertex>,
    indices: &mut Vec<u32>,
    position: [f32; 3],
    radius: f32,
) {
    let center = Vec3::from_array(position);
    let directions = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ];
    let start = vertices.len() as u32;
    for direction in directions {
        vertices.push(PreparedVertex {
            position: (center + direction * radius).to_array(),
            normal: direction.to_array(),
            color: CROSSHAIR_COLOR,
        });
    }
    indices.extend_from_slice(&[
        start,
        start + 2,
        start + 4,
        start + 2,
        start + 1,
        start + 4,
        start + 1,
        start + 3,
        start + 4,
        start + 3,
        start,
        start + 4,
        start + 2,
        start,
        start + 5,
        start + 1,
        start + 2,
        start + 5,
        start + 3,
        start + 1,
        start + 5,
        start,
        start + 3,
        start + 5,
    ]);
}

impl PreparedGeometry {
    pub(super) fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    #[cfg(test)]
    pub(super) fn flat_color_triangle_index_bytes(
        &self,
        surface_colors: Option<&[[f32; 4]]>,
        roi_colors: Option<&[Option<[f32; 4]>]>,
    ) -> Vec<u8> {
        let mut indices = Vec::with_capacity(self.indices.len());
        for triangle in self.indices.chunks_exact(3) {
            match cell_color_source_slot_for_triangle(triangle, surface_colors, roi_colors) {
                // WGSL flat interpolation sources the first vertex of the
                // primitive, so rotate each triangle to put the chosen AFNI
                // face-color source first while preserving winding.
                0 => indices.extend_from_slice(triangle),
                1 => indices.extend_from_slice(&[triangle[1], triangle[2], triangle[0]]),
                2 => indices.extend_from_slice(&[triangle[2], triangle[0], triangle[1]]),
                _ => unreachable!("triangle color source slot must be 0..=2"),
            }
        }

        indices_to_bytes(&indices)
    }

    pub(super) fn from_surface(surface: &SurfaceMesh) -> Self {
        let normals = surface.vertex_normals();
        let center = Vec3::from_array(surface.bounds.center);
        let scale = if surface.bounds.radius > f32::EPSILON {
            1.0 / surface.bounds.radius
        } else {
            1.0
        };

        let vertices = surface
            .vertices
            .iter()
            .zip(normals)
            .map(|(position, normal)| PreparedGeometryVertex {
                position: ((Vec3::from_array(*position) - center) * scale).to_array(),
                normal,
            })
            .collect();
        let indices = surface
            .triangles
            .iter()
            .flat_map(|triangle| triangle.iter().copied())
            .collect();

        Self { vertices, indices }
    }
}

pub(super) fn cell_color_chunk_ranges(
    triangle_count: usize,
    max_triangles_per_chunk: usize,
) -> Vec<Range<usize>> {
    if triangle_count == 0 {
        return Vec::new();
    }

    let max_triangles_per_chunk = max_triangles_per_chunk.max(1);
    let mut ranges = Vec::with_capacity(triangle_count.div_ceil(max_triangles_per_chunk));
    let mut start = 0;
    while start < triangle_count {
        let end = start
            .saturating_add(max_triangles_per_chunk)
            .min(triangle_count);
        ranges.push(start..end);
        start = end;
    }

    ranges
}

impl OverlayAppearance {
    pub(super) fn from_range(range: ValueRange) -> Self {
        Self {
            range: super::symmetric_value_range(range),
            symmetric_range: true,
            colormap: OverlayColorMap::SpectrumRedToBlue,
            threshold: OverlayThreshold {
                enabled: false,
                absolute: true,
                value: 0.0,
                hide_failed: true,
            },
            transparent_threshold: false,
            boxed_threshold: false,
            fade: FadeSettings::new(),
            contour: ThresholdContourStyle::new(),
            opacity: 1.0,
            dim: 1.0,
        }
    }
}

impl OverlayColorMap {
    pub(super) const ALL: [Self; 14] = [
        Self::DiscreteLabels,
        Self::SpectrumRedToBlue,
        Self::SpectrumRedToBlueGap,
        Self::SpectrumYellowToRed,
        Self::SpectrumYellowToCyan,
        Self::SpectrumYellowToCyanGap,
        Self::ColorCircleAjj,
        Self::ColorCircleZss,
        Self::RedsAndBlues,
        Self::RedsAndBluesWithGreen,
        Self::AfniP2Spanned,
        Self::BlueWhiteRed,
        Self::Fire,
        Self::Grayscale,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::DiscreteLabels => "discrete labels",
            Self::SpectrumRedToBlue => "Spectrum:red_to_blue",
            Self::SpectrumRedToBlueGap => "Spectrum:red_to_blue+gap",
            Self::SpectrumYellowToRed => "Spectrum:yellow_to_red",
            Self::SpectrumYellowToCyan => "Spectrum:yellow_to_cyan",
            Self::SpectrumYellowToCyanGap => "Spectrum:yellow_to_cyan+gap",
            Self::ColorCircleAjj => "Color_circle_AJJ",
            Self::ColorCircleZss => "Color_circle_ZSS",
            Self::RedsAndBlues => "Reds_and_Blues",
            Self::RedsAndBluesWithGreen => "Reds_and_Blues_w_Green",
            Self::AfniP2Spanned => "afni_p2spanned",
            Self::BlueWhiteRed => "blue-white-red",
            Self::Fire => "nih_fire",
            Self::Grayscale => "grayscale",
        }
    }

    pub(super) fn continuous_color_map(self) -> Option<ColorMap> {
        match self {
            Self::DiscreteLabels => None,
            Self::SpectrumRedToBlue => Some(ColorMap::spectrum_red_to_blue()),
            Self::SpectrumRedToBlueGap => Some(ColorMap::spectrum_red_to_blue_gap()),
            Self::SpectrumYellowToRed => Some(ColorMap::spectrum_yellow_to_red()),
            Self::SpectrumYellowToCyan => Some(ColorMap::spectrum_yellow_to_cyan()),
            Self::SpectrumYellowToCyanGap => Some(ColorMap::spectrum_yellow_to_cyan_gap()),
            Self::ColorCircleAjj => Some(ColorMap::color_circle_ajj()),
            Self::ColorCircleZss => Some(ColorMap::color_circle_zss()),
            Self::RedsAndBlues => Some(ColorMap::reds_and_blues()),
            Self::RedsAndBluesWithGreen => Some(ColorMap::reds_and_blues_with_green()),
            Self::AfniP2Spanned => Some(ColorMap::afni_p2_spanned()),
            Self::BlueWhiteRed => Some(ColorMap::blue_white_red()),
            Self::Fire => Some(ColorMap::fire()),
            Self::Grayscale => Some(ColorMap::grayscale()),
        }
    }

    pub(super) fn uses_continuous_range(self) -> bool {
        !matches!(self, Self::DiscreteLabels)
    }
}

fn indices_to_bytes(indices: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(indices));
    for index in indices {
        bytes.extend_from_slice(&index.to_ne_bytes());
    }
    bytes
}

fn progressive_point_indices(total: u32) -> Vec<u32> {
    const RESIDUE_ORDER: [u32; 8] = [0, 4, 2, 6, 1, 5, 3, 7];
    let mut indices = Vec::with_capacity(total as usize);
    for residue in RESIDUE_ORDER {
        let mut index = residue;
        while index < total {
            indices.push(index);
            index += 8;
        }
    }

    indices
}

fn normalized_edge(a: u32, b: u32) -> (u32, u32) {
    if a <= b { (a, b) } else { (b, a) }
}

pub(super) fn compose_vertex_color(
    surface_color: [f32; 4],
    overlay_color: Option<[f32; 4]>,
    overlay_dim: f32,
    roi_color: Option<[f32; 4]>,
) -> [f32; 4] {
    let base = overlay_color.map_or(surface_color, |color| {
        compose_overlay_color_over_base(surface_color, color, overlay_dim)
    });
    roi_color.map_or(base, |color| compose_annotation_color(base, color))
}

fn compose_overlay_color_over_base(base: [f32; 4], color: [f32; 4], dim: f32) -> [f32; 4] {
    let alpha = finite_or(color[3], 0.0).clamp(0.0, 1.0);
    let dim = dim.clamp(0.0, 1.5);
    [
        finite_or(base[0], DEFAULT_SURFACE_COLOR[0]) * (1.0 - alpha)
            + finite_or(color[0], 0.35) * dim * alpha,
        finite_or(base[1], DEFAULT_SURFACE_COLOR[1]) * (1.0 - alpha)
            + finite_or(color[1], 0.35) * dim * alpha,
        finite_or(base[2], DEFAULT_SURFACE_COLOR[2]) * (1.0 - alpha)
            + finite_or(color[2], 0.35) * dim * alpha,
        1.0,
    ]
}

fn compose_annotation_color(base: [f32; 4], annotation: [f32; 4]) -> [f32; 4] {
    let alpha = finite_or(annotation[3], 0.0).clamp(0.0, 1.0);
    [
        finite_or(base[0], DEFAULT_SURFACE_COLOR[0]) * (1.0 - alpha)
            + finite_or(annotation[0], DEFAULT_SURFACE_COLOR[0]) * alpha,
        finite_or(base[1], DEFAULT_SURFACE_COLOR[1]) * (1.0 - alpha)
            + finite_or(annotation[1], DEFAULT_SURFACE_COLOR[1]) * alpha,
        finite_or(base[2], DEFAULT_SURFACE_COLOR[2]) * (1.0 - alpha)
            + finite_or(annotation[2], DEFAULT_SURFACE_COLOR[2]) * alpha,
        1.0,
    ]
}

fn cell_color_for_triangle(
    triangle: &[u32],
    surface_colors: Option<&[[f32; 4]]>,
    roi_colors: Option<&[Option<[f32; 4]>]>,
) -> [f32; 4] {
    cell_color_source_for_triangle(triangle, surface_colors, roi_colors).1
}

#[cfg(test)]
fn cell_color_source_slot_for_triangle(
    triangle: &[u32],
    surface_colors: Option<&[[f32; 4]]>,
    roi_colors: Option<&[Option<[f32; 4]>]>,
) -> usize {
    cell_color_source_for_triangle(triangle, surface_colors, roi_colors).0
}

fn cell_color_source_for_triangle(
    triangle: &[u32],
    surface_colors: Option<&[[f32; 4]]>,
    roi_colors: Option<&[Option<[f32; 4]>]>,
) -> (usize, [f32; 4]) {
    let color = |index: u32| {
        let index = index as usize;
        compose_vertex_color(
            surface_colors
                .and_then(|colors| colors.get(index))
                .copied()
                .unwrap_or(DEFAULT_SURFACE_COLOR),
            None,
            1.0,
            roi_colors
                .and_then(|colors| colors.get(index))
                .copied()
                .flatten(),
        )
    };

    let v0 = color(triangle[0]);
    let v1 = color(triangle[1]);
    let v2 = color(triangle[2]);
    if colors_match(v1, v2) {
        (1, v1)
    } else {
        (0, v0)
    }
}

fn colors_match(left: [f32; 4], right: [f32; 4]) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| (*left - right).abs() <= 1.0e-6)
}

pub(super) fn sample_colormap(colormap: OverlayColorMap, t: f32) -> [f32; 4] {
    if let Some(colormap) = colormap.continuous_color_map() {
        return colormap
            .as_continuous()
            .expect("viewer color maps are continuous")
            .sample(t)
            .to_array();
    }

    let index = (t.clamp(0.0, 1.0) * 9.0).round() as i32 + 1;
    stable_label_color(index, 255).to_array()
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::{
        CONTOUR_VERTEX_FLOATS, DEFAULT_SURFACE_COLOR, OverlayColorMap, PreparedGeometry,
        PreparedSurface, PreparedThresholdContour, RoiAppearance, SelectionHighlight,
        ThresholdContourStyle, cell_color_chunk_ranges, threshold_boundary_luminances,
        threshold_contour_segments,
    };
    use crate::color::ColorMap;
    use crate::dataset::{ColumnData, ColumnRange, ColumnRole, DataColumn, Dataset, DatasetKind};
    use crate::overlay::{MaskMode, Overlay, OverlayColumns, RangeSelection, Threshold};
    use crate::surface::{SurfaceMesh, ValueRange};
    use glam::Vec3;

    #[test]
    fn prepared_surface_flattens_indices_and_computes_normals() {
        let mesh = triangle_mesh();

        let prepared = PreparedSurface::from_surface(&mesh, None, 1.0);

        assert_eq!(prepared.indices, vec![0, 1, 2]);
        assert_eq!(prepared.vertices.len(), 3);
        for vertex in prepared.vertices {
            assert_eq!(vertex.normal, [0.0, 0.0, 1.0]);
            assert_eq!(vertex.color, DEFAULT_SURFACE_COLOR);
        }
    }

    #[test]
    fn prepared_surface_normalizes_positions_for_viewing() {
        let mesh = triangle_mesh();

        let prepared = PreparedSurface::from_surface(&mesh, None, 1.0);

        for vertex in prepared.vertices {
            let length = Vec3::from_array(vertex.position).length();
            assert!(length <= 1.0 + f32::EPSILON);
        }
    }

    #[test]
    fn prepared_surface_appends_selection_highlight_geometry() {
        let mesh = triangle_mesh();
        let geometry = PreparedGeometry::from_surface(&mesh);

        let prepared = PreparedSurface::from_geometry_with_selection(
            &geometry,
            None,
            None,
            1.0,
            None,
            Some(SelectionHighlight::normalized(2, 0, [0.0, 0.0, 0.0])),
        );

        assert_eq!(prepared.vertices.len(), geometry.vertices.len() + 15);
        assert_eq!(prepared.indices.len(), geometry.indices.len() + 51);
        assert_eq!(&prepared.indices[..3], geometry.indices.as_slice());
        assert!(
            prepared.vertices[geometry.vertices.len()..]
                .iter()
                .all(|vertex| vertex.color != DEFAULT_SURFACE_COLOR)
        );
    }

    #[test]
    fn prepared_surface_maps_overlay_values_to_vertex_colors() {
        let mesh = triangle_mesh();
        let (dataset, overlay) = scalar_overlay(&mesh, vec![-1.0, 0.0, 1.0]);

        let prepared = PreparedSurface::from_surface(&mesh, Some(&overlay), 1.0);

        assert_color_close(prepared.vertices[0].color, [0.02, 0.12, 0.32, 1.0]);
        assert_color_close(prepared.vertices[1].color, [0.98, 0.96, 0.86, 1.0]);
        assert_color_close(prepared.vertices[2].color, [0.45, 0.09, 0.07, 1.0]);

        assert_eq!(overlay.color_cache.colors.len(), dataset.row_count);
    }

    #[test]
    fn prepared_surface_cell_colors_use_triangle_face_color() {
        let mesh = triangle_mesh();
        let geometry = PreparedGeometry::from_surface(&mesh);
        let colors = vec![
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        ];

        let prepared =
            PreparedSurface::from_geometry_cell_colors(&geometry, Some(&colors), None, None);

        assert_eq!(prepared.indices, vec![0, 1, 2]);
        assert_eq!(prepared.vertices.len(), 3);
        for vertex in prepared.vertices {
            assert_eq!(vertex.color, [0.0, 0.0, 1.0, 1.0]);
        }
    }

    #[test]
    fn flat_color_triangle_indices_rotate_to_the_chosen_face_color_vertex() {
        let mesh = triangle_mesh();
        let geometry = PreparedGeometry::from_surface(&mesh);
        let colors = vec![
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        ];

        let bytes = geometry.flat_color_triangle_index_bytes(Some(&colors), None);
        let indices = bytes
            .chunks_exact(std::mem::size_of::<u32>())
            .map(|chunk| u32::from_ne_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();

        assert_eq!(indices, vec![1, 2, 0]);
    }

    #[test]
    fn cell_color_chunk_ranges_cover_all_triangles() {
        assert_eq!(
            cell_color_chunk_ranges(0, 3),
            Vec::<std::ops::Range<usize>>::new()
        );
        assert_eq!(cell_color_chunk_ranges(5, 2), vec![0..2, 2..4, 4..5]);
        assert_eq!(cell_color_chunk_ranges(3, 0), vec![0..1, 1..2, 2..3]);
    }

    #[test]
    fn prepared_surface_cell_color_chunks_use_local_indices() {
        let mesh = square_mesh();
        let geometry = PreparedGeometry::from_surface(&mesh);
        let colors = vec![
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
        ];
        let chunks = cell_color_chunk_ranges(geometry.triangle_count(), 1)
            .into_iter()
            .map(|range| {
                PreparedSurface::from_geometry_cell_color_range(
                    &geometry,
                    Some(&colors),
                    None,
                    range,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(chunks.len(), 2);
        for chunk in &chunks {
            assert_eq!(chunk.indices, vec![0, 1, 2]);
            assert_eq!(chunk.vertices.len(), 3);
            assert_eq!(chunk.index_count(), 3);
        }
    }

    #[test]
    fn prepared_surface_selection_highlight_can_be_its_own_chunk() {
        let mesh = triangle_mesh();
        let geometry = PreparedGeometry::from_surface(&mesh);
        let chunk = PreparedSurface::selection_highlight(
            &geometry,
            SelectionHighlight::normalized(2, 0, [0.0, 0.0, 0.0]),
        );

        assert_eq!(chunk.vertices.len(), 15);
        assert_eq!(chunk.indices.len(), 51);
        assert!(!chunk.is_empty());
    }

    #[test]
    fn prepared_surface_can_threshold_with_stat_values() {
        let mesh = triangle_mesh();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &mesh.domain,
            vec![
                DataColumn::new(
                    "effect",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![-1.0, 0.0, 1.0]),
                )
                .unwrap(),
                DataColumn::new(
                    "stat",
                    ColumnRole::Threshold,
                    None,
                    ColumnData::Float32(vec![4.0, 0.0, 4.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let mut overlay = afni_overlay(&dataset, &mesh, OverlayColumns::new(0).with_threshold(1))
            .with_threshold(Threshold::outside(-2.0, 2.0), MaskMode::HideFailedThreshold);
        overlay.rebuild_color_cache(&dataset, &mesh.domain).unwrap();

        let prepared = PreparedSurface::from_surface(&mesh, Some(&overlay), 1.0);

        assert_color_close(prepared.vertices[0].color, [0.02, 0.12, 0.32, 1.0]);
        assert_eq!(prepared.vertices[1].color, DEFAULT_SURFACE_COLOR);
        assert_color_close(prepared.vertices[2].color, [0.45, 0.09, 0.07, 1.0]);

        let mut dimmed_overlay =
            afni_overlay(&dataset, &mesh, OverlayColumns::new(0).with_threshold(1)).with_threshold(
                Threshold::outside(-2.0, 2.0),
                MaskMode::DimFailedThreshold(0.25),
            );
        dimmed_overlay
            .rebuild_color_cache(&dataset, &mesh.domain)
            .unwrap();
        let dimmed_prepared = PreparedSurface::from_surface(&mesh, Some(&dimmed_overlay), 1.0);

        assert_color_close(dimmed_prepared.vertices[1].color, [0.245, 0.24, 0.215, 1.0]);
    }

    #[test]
    fn prepared_surface_composes_roi_color_over_overlay_color() {
        let mesh = triangle_mesh();
        let geometry = PreparedGeometry::from_surface(&mesh);
        let (_, overlay) = scalar_overlay(&mesh, vec![-1.0, 0.0, 1.0]);
        let mut roi = RoiAppearance::empty(mesh.vertices.len());
        assert!(roi.set_node_color(1, [0.0, 1.0, 0.0, 0.5]));

        let prepared = PreparedSurface::from_geometry_with_selection(
            &geometry,
            None,
            Some(&overlay),
            1.0,
            Some(&roi),
            None,
        );

        assert_color_close(prepared.vertices[1].color, [0.49, 0.98, 0.43, 1.0]);
    }

    #[test]
    fn prepared_surface_uses_surface_colors_below_roi_colors() {
        let mesh = triangle_mesh();
        let geometry = PreparedGeometry::from_surface(&mesh);
        let surface_colors = vec![
            [0.4, 0.4, 0.4, 1.0],
            [0.6, 0.6, 0.6, 1.0],
            [0.8, 0.8, 0.8, 1.0],
        ];
        let mut roi = RoiAppearance::empty(mesh.vertices.len());
        assert!(roi.set_node_color(1, [1.0, 0.0, 0.0, 0.5]));

        let prepared = PreparedSurface::from_geometry_with_selection(
            &geometry,
            Some(surface_colors.as_slice()),
            None,
            1.0,
            Some(&roi),
            None,
        );

        assert_color_close(prepared.vertices[0].color, [0.4, 0.4, 0.4, 1.0]);
        assert_color_close(prepared.vertices[1].color, [0.8, 0.3, 0.3, 1.0]);
        assert_color_close(prepared.vertices[2].color, [0.8, 0.8, 0.8, 1.0]);
    }

    #[test]
    fn prepared_surface_packs_vertex_bytes() {
        let mesh = triangle_mesh();
        let prepared = PreparedSurface::from_surface(&mesh, None, 1.0);

        assert_eq!(
            prepared.vertex_bytes().len(),
            prepared.vertices.len() * 6 * 4
        );
        assert_eq!(
            prepared.color_bytes().len(),
            prepared.vertices.len() * 4 * 4
        );
        assert_eq!(prepared.index_bytes().len(), prepared.indices.len() * 4);
    }

    #[test]
    fn marching_triangles_interpolates_one_threshold_segment() {
        let geometry = PreparedGeometry::from_surface(&triangle_mesh());
        let segments =
            threshold_contour_segments(&geometry, &[0.0, 2.0, 2.0], Threshold::above(1.0));

        assert_eq!(segments.len(), 1);
        let (boundary_index, segment) = segments[0];
        assert_eq!(boundary_index, 0);
        assert!((segment.start.position[2]).abs() <= f32::EPSILON);
        assert!((segment.end.position[2]).abs() <= f32::EPSILON);
        assert_eq!(segment.start.normal, [0.0, 0.0, 1.0]);
        assert_eq!(segment.end.normal, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn marching_triangles_emits_both_between_and_outside_boundaries() {
        let geometry = PreparedGeometry::from_surface(&triangle_mesh());
        let values = [0.0, 2.0, 4.0];

        assert_eq!(
            threshold_contour_segments(&geometry, &values, Threshold::between(1.0, 3.0)).len(),
            2
        );
        assert_eq!(
            threshold_contour_segments(&geometry, &values, Threshold::outside(1.0, 3.0)).len(),
            2
        );
    }

    #[test]
    fn marching_triangles_handles_exact_vertices_and_deduplicates_shared_edges() {
        let mesh = SurfaceMesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ],
            vec![[0, 1, 2], [1, 0, 3]],
        )
        .unwrap();
        let geometry = PreparedGeometry::from_surface(&mesh);

        assert_eq!(
            threshold_contour_segments(&geometry, &[1.0, 1.0, 0.0, 0.0], Threshold::above(1.0),)
                .len(),
            1
        );
        assert!(
            threshold_contour_segments(&geometry, &[1.0, 2.0, 2.0, 2.0], Threshold::above(1.0),)
                .is_empty()
        );
    }

    #[test]
    fn marching_triangles_respects_below_threshold_boundary_inclusivity() {
        let geometry = PreparedGeometry::from_surface(&triangle_mesh());
        assert_eq!(
            threshold_contour_segments(&geometry, &[1.0, 1.0, 2.0], Threshold::below(1.0),).len(),
            1
        );
        assert!(
            threshold_contour_segments(&geometry, &[1.0, 0.0, 0.0], Threshold::below(1.0),)
                .is_empty()
        );
    }

    #[test]
    fn paired_component_contours_use_their_local_threshold_slices() {
        let left = PreparedGeometry::from_surface(&triangle_mesh());
        let right = PreparedGeometry::from_surface(&triangle_mesh());
        let combined_values = [0.0, 2.0, 2.0, 2.0, 0.0, 0.0];

        assert_eq!(
            threshold_contour_segments(&left, &combined_values[..3], Threshold::above(1.0),).len(),
            1
        );
        assert_eq!(
            threshold_contour_segments(&right, &combined_values[3..], Threshold::above(1.0),).len(),
            1
        );
    }

    #[test]
    fn marching_triangles_skips_triangles_with_sparse_or_non_finite_values() {
        let geometry = PreparedGeometry::from_surface(&triangle_mesh());
        assert!(
            threshold_contour_segments(&geometry, &[0.0, f32::NAN, 2.0], Threshold::above(1.0),)
                .is_empty()
        );
        assert!(
            threshold_contour_segments(&geometry, &[0.0, 2.0], Threshold::above(1.0)).is_empty()
        );
    }

    #[test]
    fn prepared_threshold_contour_builds_quads_with_surface_offset() {
        let geometry = PreparedGeometry::from_surface(&triangle_mesh());
        let contour = PreparedThresholdContour::from_geometry(
            &geometry,
            &[0.0, 2.0, 2.0],
            Threshold::above(1.0),
            &[0.9],
        );

        // One crossing segment becomes one quad: four corners, two triangles.
        assert_eq!(contour.vertices.len(), 4);
        assert_eq!(contour.indices, vec![0, 1, 2, 1, 3, 2]);

        // Both endpoints travel with every corner so the shader can measure the
        // segment direction in screen space, and both are lifted off the
        // surface along its normal.
        for vertex in &contour.vertices {
            assert!(vertex.segment_start[2] > 0.0);
            assert!(vertex.segment_end[2] > 0.0);
            assert_eq!(vertex.params[2], 0.9);
        }

        // The four corners cover both sides of the centerline at both ends.
        let corners: Vec<(f32, f32)> = contour
            .vertices
            .iter()
            .map(|vertex| (vertex.params[0], vertex.params[1]))
            .collect();
        assert_eq!(
            corners,
            vec![(1.0, 0.0), (-1.0, 0.0), (1.0, 1.0), (-1.0, 1.0)]
        );

        assert_eq!(
            contour.vertex_bytes().len(),
            contour.vertices.len() * CONTOUR_VERTEX_FLOATS * 4
        );
    }

    #[test]
    fn contour_style_casing_widens_and_opposes_the_inner_line() {
        let mut style = ThresholdContourStyle::new();
        style.width_px = 2.0;
        style.halo_px = 1.5;
        // The casing extends past the inner line on both sides.
        assert_eq!(style.halo_width_px(), 5.0);
        assert!(style.draws_halo());

        // A white line gets a black casing and vice versa, so the pair reads on
        // any background.
        style.color = [1.0, 1.0, 1.0];
        assert_eq!(style.halo_color(), [0.0, 0.0, 0.0]);
        style.color = [0.0, 0.0, 0.0];
        assert_eq!(style.halo_color(), [1.0, 1.0, 1.0]);

        style.halo_px = 0.0;
        assert!(!style.draws_halo());
        assert_eq!(style.halo_width_px(), style.width_px);
    }

    #[test]
    fn boundary_luminances_follow_the_colormap_at_each_tail() {
        // A two-sided threshold outlines both tails, which sit at opposite ends
        // of the colormap and so need their own contrast decisions.
        let range = ValueRange {
            min: -5.0,
            max: 5.0,
        };
        let luminances = threshold_boundary_luminances(
            Threshold::outside(-2.0, 2.0),
            range,
            OverlayColorMap::BlueWhiteRed,
        );
        assert_eq!(luminances.len(), 2);
        assert!(luminances.iter().all(|value| (0.0..=1.0).contains(value)));

        // Label overlays have no continuous colormap to sample; callers fall
        // back to the default rather than getting a wrong answer.
        assert!(
            threshold_boundary_luminances(
                Threshold::above(2.0),
                range,
                OverlayColorMap::DiscreteLabels,
            )
            .is_empty()
        );

        // A degenerate range must not produce NaN luminance.
        let flat = ValueRange { min: 1.0, max: 1.0 };
        for luminance in
            threshold_boundary_luminances(Threshold::above(2.0), flat, OverlayColorMap::Fire)
        {
            assert!(luminance.is_finite());
        }
    }

    #[test]
    fn point_indices_are_progressively_spread_for_sparse_prefix_draws() {
        let mesh = octagon_strip_mesh();
        let geometry = PreparedGeometry::from_surface(&mesh);
        let prepared =
            PreparedSurface::from_geometry_with_selection(&geometry, None, None, 1.0, None, None);
        let indices = prepared
            .point_index_bytes()
            .chunks_exact(std::mem::size_of::<u32>())
            .map(|chunk| u32::from_ne_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();

        assert_eq!(indices, vec![0, 4, 2, 6, 1, 5, 3, 7]);
    }

    fn triangle_mesh() -> SurfaceMesh {
        let vertices = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

        SurfaceMesh::new(vertices, vec![[0, 1, 2]]).unwrap()
    }

    fn square_mesh() -> SurfaceMesh {
        let vertices = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];

        SurfaceMesh::new(vertices, vec![[0, 1, 2], [0, 2, 3]]).unwrap()
    }

    fn octagon_strip_mesh() -> SurfaceMesh {
        let vertices = (0..8)
            .map(|index| [index as f32, 0.0, 0.0])
            .collect::<Vec<_>>();
        let triangles = (1..7)
            .map(|index| [0, index as u32, index as u32 + 1])
            .collect::<Vec<_>>();

        SurfaceMesh::new(vertices, triangles).unwrap()
    }

    fn scalar_overlay(mesh: &SurfaceMesh, values: Vec<f32>) -> (Dataset, Overlay) {
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &mesh.domain,
            vec![
                DataColumn::new(
                    "effect",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(values),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let overlay = afni_overlay(&dataset, mesh, OverlayColumns::new(0));

        (dataset, overlay)
    }

    fn afni_overlay(dataset: &Dataset, mesh: &SurfaceMesh, columns: OverlayColumns) -> Overlay {
        let mut overlay = Overlay::from_dataset(dataset, &mesh.domain, columns)
            .unwrap()
            .with_colormap(ColorMap::afni_p2_spanned())
            .with_intensity_range(RangeSelection::Manual(ColumnRange {
                min: -1.0,
                max: 1.0,
            }))
            .with_symmetric_range(true);
        overlay.rebuild_color_cache(dataset, &mesh.domain).unwrap();
        overlay
    }

    fn assert_color_close(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.0001);
        }
    }
}
