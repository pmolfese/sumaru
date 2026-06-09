use glam::Vec3;

use crate::color::ColorMap;
use crate::overlay::Overlay;
use crate::surface::{SurfaceMesh, ValueRange};

pub(super) const DEFAULT_SURFACE_COLOR: [f32; 4] = [0.76, 0.78, 0.74, 1.0];

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

#[derive(Debug, Clone, Copy)]
pub(super) struct OverlayAppearance {
    pub(super) range: ValueRange,
    pub(super) colormap: OverlayColorMap,
    pub(super) threshold: OverlayThreshold,
    pub(super) opacity: f32,
    pub(super) dim: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverlayColorMap {
    AfniP2Spanned,
    BlueWhiteRed,
    Fire,
    Grayscale,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct OverlayThreshold {
    pub(super) enabled: bool,
    pub(super) absolute: bool,
    pub(super) value: f32,
    pub(super) hide_failed: bool,
}

impl PreparedSurface {
    #[cfg(test)]
    pub(super) fn from_surface(
        surface: &SurfaceMesh,
        overlay: Option<&Overlay>,
        overlay_dim: f32,
    ) -> Self {
        let geometry = PreparedGeometry::from_surface(surface);
        Self::from_geometry(&geometry, overlay, overlay_dim)
    }

    pub(super) fn from_geometry(
        geometry: &PreparedGeometry,
        overlay: Option<&Overlay>,
        overlay_dim: f32,
    ) -> Self {
        let vertices = geometry
            .vertices
            .iter()
            .enumerate()
            .map(|(index, vertex)| PreparedVertex {
                position: vertex.position,
                normal: vertex.normal,
                color: overlay
                    .and_then(|overlay| overlay.color_cache.colors.get(index))
                    .copied()
                    .map_or(DEFAULT_SURFACE_COLOR, |color| {
                        compose_overlay_color(color, overlay_dim)
                    }),
            })
            .collect();

        Self {
            vertices,
            indices: geometry.indices.clone(),
        }
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

        f32_bytes(&floats)
    }

    pub(super) fn index_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(self.indices.as_slice()));

        for index in &self.indices {
            bytes.extend_from_slice(&index.to_ne_bytes());
        }

        bytes
    }
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
            range: symmetric_range_if_signed(range),
            colormap: OverlayColorMap::AfniP2Spanned,
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
    pub(super) const ALL: [Self; 4] = [
        Self::AfniP2Spanned,
        Self::BlueWhiteRed,
        Self::Fire,
        Self::Grayscale,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::AfniP2Spanned => "afni_p2spanned",
            Self::BlueWhiteRed => "blue-white-red",
            Self::Fire => "nih_fire",
            Self::Grayscale => "grayscale",
        }
    }

    pub(super) fn to_color_map(self) -> ColorMap {
        match self {
            Self::AfniP2Spanned => ColorMap::afni_p2_spanned(),
            Self::BlueWhiteRed => ColorMap::blue_white_red(),
            Self::Fire => ColorMap::fire(),
            Self::Grayscale => ColorMap::grayscale(),
        }
    }
}

fn compose_overlay_color(color: [f32; 4], dim: f32) -> [f32; 4] {
    let alpha = finite_or(color[3], 0.0).clamp(0.0, 1.0);
    let dim = dim.clamp(0.0, 1.5);
    [
        DEFAULT_SURFACE_COLOR[0] * (1.0 - alpha) + finite_or(color[0], 0.35) * dim * alpha,
        DEFAULT_SURFACE_COLOR[1] * (1.0 - alpha) + finite_or(color[1], 0.35) * dim * alpha,
        DEFAULT_SURFACE_COLOR[2] * (1.0 - alpha) + finite_or(color[2], 0.35) * dim * alpha,
        1.0,
    ]
}

fn symmetric_range_if_signed(range: ValueRange) -> ValueRange {
    if range.min < 0.0 && range.max > 0.0 {
        let extent = range.min.abs().max(range.max.abs());
        ValueRange {
            min: -extent,
            max: extent,
        }
    } else {
        range
    }
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

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));

    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }

    bytes
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SURFACE_COLOR, PreparedSurface};
    use crate::color::ColorMap;
    use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind};
    use crate::overlay::{
        MaskMode, Overlay, OverlayColumns, OverlayRange, RangeSelection, Threshold,
    };
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
            .with_intensity_range(RangeSelection::Manual(OverlayRange {
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
