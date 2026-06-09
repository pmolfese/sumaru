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
    pub(super) fn from_surface(
        surface: &SurfaceMesh,
        overlay: Option<&OverlayDataset>,
        overlay_appearance: Option<OverlayAppearance>,
    ) -> Self {
        let normals = surface.vertex_normals();
        let center = Vec3::from_array(surface.bounds.center);
        let scale = if surface.bounds.radius > f32::EPSILON {
            1.0 / surface.bounds.radius
        } else {
            1.0
        };
        let colors = overlay
            .zip(overlay_appearance)
            .map(|(overlay, appearance)| overlay_vertex_colors(overlay, appearance));

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
}

fn overlay_vertex_colors(overlay: &OverlayDataset, appearance: OverlayAppearance) -> Vec<[f32; 4]> {
    overlay
        .values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            color_for_value(
                appearance,
                *value,
                overlay
                    .threshold_values
                    .as_ref()
                    .and_then(|values| values.get(index))
                    .copied(),
                overlay
                    .brightness_values
                    .as_ref()
                    .and_then(|values| values.get(index))
                    .copied(),
                overlay.brightness_range,
            )
        })
        .collect()
}

fn color_for_value(
    appearance: OverlayAppearance,
    value: f32,
    threshold_value: Option<f32>,
    brightness_value: Option<f32>,
    brightness_range: Option<ValueRange>,
) -> [f32; 4] {
    if !value.is_finite() {
        return [0.35, 0.35, 0.35, appearance.opacity.clamp(0.0, 1.0)];
    }

    let min = appearance.range.min;
    let max = appearance.range.max;

    if (max - min).abs() <= f32::EPSILON {
        return [1.0, 1.0, 1.0, appearance.opacity.clamp(0.0, 1.0)];
    }

    let normalized = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let mut color = sample_colormap(appearance.colormap, normalized);
    apply_brightness(&mut color, brightness_value, brightness_range);
    let passes_threshold = appearance
        .threshold
        .passes(threshold_value.unwrap_or(value));

    if !passes_threshold && appearance.threshold.hide_failed {
        color = DEFAULT_SURFACE_COLOR;
    } else {
        let dim = appearance.dim.clamp(0.0, 1.5);
        let opacity = appearance.opacity.clamp(0.0, 1.0);
        let failed_factor = if passes_threshold { 1.0 } else { 0.25 };
        color[0] *= dim * failed_factor;
        color[1] *= dim * failed_factor;
        color[2] *= dim * failed_factor;
        color[0] = DEFAULT_SURFACE_COLOR[0] * (1.0 - opacity) + color[0] * opacity;
        color[1] = DEFAULT_SURFACE_COLOR[1] * (1.0 - opacity) + color[1] * opacity;
        color[2] = DEFAULT_SURFACE_COLOR[2] * (1.0 - opacity) + color[2] * opacity;
        color[3] = 1.0;
    }

    color
}

impl OverlayThreshold {
    fn passes(self, value: f32) -> bool {
        if !self.enabled {
            return true;
        }

        if self.absolute {
            value.abs() >= self.value.abs()
        } else {
            value >= self.value
        }
    }
}

fn apply_brightness(
    color: &mut [f32; 4],
    brightness_value: Option<f32>,
    brightness_range: Option<ValueRange>,
) {
    let (Some(value), Some(range)) = (brightness_value, brightness_range) else {
        return;
    };
    if !value.is_finite() || (range.max - range.min).abs() <= f32::EPSILON {
        return;
    }

    let factor = ((value - range.min) / (range.max - range.min)).clamp(0.0, 1.0);
    color[0] *= factor;
    color[1] *= factor;
    color[2] *= factor;
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
    match colormap {
        OverlayColorMap::AfniP2Spanned => sample_stops(
            t,
            &[
                (0.0, [0.02, 0.12, 0.32, 1.0]),
                (0.22, [0.08, 0.38, 0.68, 1.0]),
                (0.42, [0.52, 0.74, 0.92, 1.0]),
                (0.5, [0.98, 0.96, 0.86, 1.0]),
                (0.66, [0.96, 0.68, 0.28, 1.0]),
                (0.82, [0.80, 0.24, 0.16, 1.0]),
                (1.0, [0.45, 0.09, 0.07, 1.0]),
            ],
        ),
        OverlayColorMap::BlueWhiteRed => sample_stops(
            t,
            &[
                (0.0, [0.1, 0.22, 0.85, 1.0]),
                (0.5, [1.0, 1.0, 1.0, 1.0]),
                (1.0, [0.86, 0.08, 0.08, 1.0]),
            ],
        ),
        OverlayColorMap::Fire => sample_stops(
            t,
            &[
                (0.0, [0.02, 0.0, 0.0, 1.0]),
                (0.28, [0.42, 0.02, 0.02, 1.0]),
                (0.58, [0.90, 0.24, 0.02, 1.0]),
                (0.82, [1.0, 0.74, 0.12, 1.0]),
                (1.0, [1.0, 1.0, 0.88, 1.0]),
            ],
        ),
        OverlayColorMap::Grayscale => sample_stops(
            t,
            &[(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 1.0])],
        ),
    }
}

fn sample_stops(t: f32, stops: &[(f32, [f32; 4])]) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    if t <= stops[0].0 {
        return stops[0].1;
    }

    for window in stops.windows(2) {
        let [(left_t, left_color), (right_t, right_color)] = window else {
            continue;
        };
        if t <= *right_t {
            let span = *right_t - *left_t;
            let local_t = if span.abs() <= f32::EPSILON {
                0.0
            } else {
                (t - *left_t) / span
            };
            return lerp_color(*left_color, *right_color, local_t);
        }
    }

    stops[stops.len() - 1].1
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
    use super::{DEFAULT_SURFACE_COLOR, OverlayAppearance, PreparedSurface};
    use crate::surface::{OverlayDataset, SurfaceMesh, ValueRange};
    use glam::Vec3;

    #[test]
    fn prepared_surface_flattens_indices_and_computes_normals() {
        let mesh = triangle_mesh();

        let prepared = PreparedSurface::from_surface(&mesh, None, None);

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

        let prepared = PreparedSurface::from_surface(&mesh, None, None);

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
            threshold_values: None,
            threshold_pvalues: None,
            brightness_values: None,
            brightness_range: None,
        };

        let prepared = PreparedSurface::from_surface(
            &mesh,
            Some(&overlay),
            Some(OverlayAppearance::from_range(overlay.range)),
        );

        assert_color_close(prepared.vertices[0].color, [0.02, 0.12, 0.32, 1.0]);
        assert_color_close(prepared.vertices[1].color, [0.98, 0.96, 0.86, 1.0]);
        assert_color_close(prepared.vertices[2].color, [0.45, 0.09, 0.07, 1.0]);
    }

    #[test]
    fn prepared_surface_can_threshold_with_stat_values() {
        let mesh = triangle_mesh();
        let overlay = OverlayDataset {
            values: vec![-1.0, 0.0, 1.0],
            range: ValueRange::from_values(&[-1.0, 0.0, 1.0]).unwrap(),
            threshold_values: Some(vec![4.0, 0.0, 4.0]),
            threshold_pvalues: Some(vec![0.001, 1.0, 0.001]),
            brightness_values: None,
            brightness_range: None,
        };
        let mut appearance = OverlayAppearance::from_range(overlay.range);
        appearance.threshold.enabled = true;
        appearance.threshold.value = 2.0;

        let prepared = PreparedSurface::from_surface(&mesh, Some(&overlay), Some(appearance));

        assert_color_close(prepared.vertices[0].color, [0.02, 0.12, 0.32, 1.0]);
        assert_eq!(prepared.vertices[1].color, DEFAULT_SURFACE_COLOR);
        assert_color_close(prepared.vertices[2].color, [0.45, 0.09, 0.07, 1.0]);

        let mut dimmed_appearance = appearance;
        dimmed_appearance.threshold.hide_failed = false;
        let dimmed_prepared =
            PreparedSurface::from_surface(&mesh, Some(&overlay), Some(dimmed_appearance));

        assert_color_close(dimmed_prepared.vertices[1].color, [0.245, 0.24, 0.215, 1.0]);
    }

    #[test]
    fn prepared_surface_packs_vertex_bytes() {
        let mesh = triangle_mesh();
        let prepared = PreparedSurface::from_surface(&mesh, None, None);

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
