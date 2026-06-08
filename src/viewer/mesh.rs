use glam::Vec3;

use crate::surface::{OverlayDataset, SurfaceMesh, ValueRange};

pub(super) const DEFAULT_SURFACE_COLOR: [f32; 4] = [0.76, 0.78, 0.74, 1.0];

#[derive(Debug, Clone)]
pub(super) struct PreparedSurface {
    pub(super) vertices: Vec<PreparedVertex>,
    pub(super) indices: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PreparedVertex {
    pub(super) position: [f32; 3],
    pub(super) normal: [f32; 3],
    pub(super) color: [f32; 4],
}

impl PreparedSurface {
    pub(super) fn from_surface(surface: &SurfaceMesh, overlay: Option<&OverlayDataset>) -> Self {
        let normals = surface.vertex_normals();
        let center = Vec3::from_array(surface.bounds.center);
        let scale = if surface.bounds.radius > f32::EPSILON {
            1.0 / surface.bounds.radius
        } else {
            1.0
        };
        let colors = overlay.map(overlay_vertex_colors);

        let vertices = surface
            .vertices
            .iter()
            .zip(normals)
            .enumerate()
            .map(|(index, (position, normal))| PreparedVertex {
                position: ((Vec3::from_array(*position) - center) * scale).to_array(),
                normal,
                color: colors
                    .as_ref()
                    .map_or(DEFAULT_SURFACE_COLOR, |colors| colors[index]),
            })
            .collect();
        let indices = surface
            .triangles
            .iter()
            .flat_map(|triangle| triangle.iter().copied())
            .collect();

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

fn overlay_vertex_colors(overlay: &OverlayDataset) -> Vec<[f32; 4]> {
    overlay
        .values
        .iter()
        .map(|value| color_for_value(overlay.range, *value))
        .collect()
}

fn color_for_value(range: ValueRange, value: f32) -> [f32; 4] {
    if !value.is_finite() {
        return [0.35, 0.35, 0.35, 1.0];
    }

    let (min, max) = if range.min < 0.0 && range.max > 0.0 {
        let extent = range.min.abs().max(range.max.abs());
        (-extent, extent)
    } else {
        (range.min, range.max)
    };

    if (max - min).abs() <= f32::EPSILON {
        return [1.0, 1.0, 1.0, 1.0];
    }

    let normalized = ((value - min) / (max - min)).clamp(0.0, 1.0);
    if normalized < 0.5 {
        let t = normalized * 2.0;
        lerp_color([0.1, 0.22, 0.85, 1.0], [1.0, 1.0, 1.0, 1.0], t)
    } else {
        let t = (normalized - 0.5) * 2.0;
        lerp_color([1.0, 1.0, 1.0, 1.0], [0.86, 0.08, 0.08, 1.0], t)
    }
}

fn lerp_color(start: [f32; 4], end: [f32; 4], t: f32) -> [f32; 4] {
    [
        start[0] + (end[0] - start[0]) * t,
        start[1] + (end[1] - start[1]) * t,
        start[2] + (end[2] - start[2]) * t,
        start[3] + (end[3] - start[3]) * t,
    ]
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
    use crate::surface::{OverlayDataset, SurfaceMesh, ValueRange};
    use glam::Vec3;

    #[test]
    fn prepared_surface_flattens_indices_and_computes_normals() {
        let mesh = triangle_mesh();

        let prepared = PreparedSurface::from_surface(&mesh, None);

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

        let prepared = PreparedSurface::from_surface(&mesh, None);

        for vertex in prepared.vertices {
            let length = Vec3::from_array(vertex.position).length();
            assert!(length <= 1.0 + f32::EPSILON);
        }
    }

    #[test]
    fn prepared_surface_maps_overlay_values_to_vertex_colors() {
        let mesh = triangle_mesh();
        let overlay = OverlayDataset {
            values: vec![-1.0, 0.0, 1.0],
            range: ValueRange::from_values(&[-1.0, 0.0, 1.0]).unwrap(),
        };

        let prepared = PreparedSurface::from_surface(&mesh, Some(&overlay));

        assert_color_close(prepared.vertices[0].color, [0.1, 0.22, 0.85, 1.0]);
        assert_color_close(prepared.vertices[1].color, [1.0, 1.0, 1.0, 1.0]);
        assert_color_close(prepared.vertices[2].color, [0.86, 0.08, 0.08, 1.0]);
    }

    #[test]
    fn prepared_surface_packs_vertex_bytes() {
        let mesh = triangle_mesh();
        let prepared = PreparedSurface::from_surface(&mesh, None);

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

    fn assert_color_close(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.0001);
        }
    }
}
