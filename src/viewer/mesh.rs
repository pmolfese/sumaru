use std::collections::BTreeSet;

use glam::Vec3;

use crate::color::ColorMap;
use crate::command::OverlayThreshold;
use crate::overlay::Overlay;
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
    pub(super) opacity: f32,
    pub(super) dim: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverlayColorMap {
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
        let mut vertices = Vec::with_capacity(geometry.indices.len());
        let mut indices = Vec::with_capacity(geometry.indices.len());

        for triangle in geometry.indices.chunks_exact(3) {
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

        if let Some(selection) = selection {
            append_selection_highlight(&mut vertices, &mut indices, geometry, selection);
        }

        Self { vertices, indices }
    }

    pub(super) fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }

    pub(super) fn vertex_bytes(&self) -> Vec<u8> {
        let mut floats = Vec::with_capacity(self.vertices.len() * 10);

        for vertex in &self.vertices {
            floats.extend_from_slice(&vertex.position);
            floats.extend_from_slice(&vertex.normal);
            floats.extend_from_slice(&vertex.color);
        }

        super::f32_bytes(&floats)
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
        let indices = (0..self.vertices.len() as u32).collect::<Vec<_>>();
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
            opacity: 1.0,
            dim: 1.0,
        }
    }
}

impl OverlayColorMap {
    pub(super) const ALL: [Self; 13] = [
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

    pub(super) fn to_color_map(self) -> ColorMap {
        match self {
            Self::SpectrumRedToBlue => ColorMap::spectrum_red_to_blue(),
            Self::SpectrumRedToBlueGap => ColorMap::spectrum_red_to_blue_gap(),
            Self::SpectrumYellowToRed => ColorMap::spectrum_yellow_to_red(),
            Self::SpectrumYellowToCyan => ColorMap::spectrum_yellow_to_cyan(),
            Self::SpectrumYellowToCyanGap => ColorMap::spectrum_yellow_to_cyan_gap(),
            Self::ColorCircleAjj => ColorMap::color_circle_ajj(),
            Self::ColorCircleZss => ColorMap::color_circle_zss(),
            Self::RedsAndBlues => ColorMap::reds_and_blues(),
            Self::RedsAndBluesWithGreen => ColorMap::reds_and_blues_with_green(),
            Self::AfniP2Spanned => ColorMap::afni_p2_spanned(),
            Self::BlueWhiteRed => ColorMap::blue_white_red(),
            Self::Fire => ColorMap::fire(),
            Self::Grayscale => ColorMap::grayscale(),
        }
    }
}

fn indices_to_bytes(indices: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(indices));
    for index in indices {
        bytes.extend_from_slice(&index.to_ne_bytes());
    }
    bytes
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
    if colors_match(v1, v2) { v1 } else { v0 }
}

fn colors_match(left: [f32; 4], right: [f32; 4]) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| (*left - right).abs() <= 1.0e-6)
}

pub(super) fn sample_colormap(colormap: OverlayColorMap, t: f32) -> [f32; 4] {
    colormap
        .to_color_map()
        .as_continuous()
        .expect("viewer color maps are continuous")
        .sample(t)
        .to_array()
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_SURFACE_COLOR, PreparedGeometry, PreparedSurface, RoiAppearance, SelectionHighlight,
    };
    use crate::color::ColorMap;
    use crate::dataset::{ColumnData, ColumnRange, ColumnRole, DataColumn, Dataset, DatasetKind};
    use crate::overlay::{MaskMode, Overlay, OverlayColumns, RangeSelection, Threshold};
    use crate::surface::SurfaceMesh;
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
            prepared.vertices.len() * 10 * 4
        );
        assert_eq!(prepared.index_bytes().len(), prepared.indices.len() * 4);
    }

    fn triangle_mesh() -> SurfaceMesh {
        let vertices = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

        SurfaceMesh::new(vertices, vec![[0, 1, 2]]).unwrap()
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
