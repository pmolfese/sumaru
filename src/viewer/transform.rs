//! Paired-hemisphere layout math: the matrices that place each hemisphere
//! in the scene, the auto-spread/clearance heuristics that keep a left/right
//! pair from overlapping, and the open-angle bookkeeping behind the drag-to-
//! open-book gesture. Pure geometry on `SurfaceMesh` and `Mat4` with no
//! borrow of `ViewerState`; moved out of `viewer/mod.rs`. All items are
//! `pub(super)` so the parent viewer module and its siblings keep using them.

use super::*;

/// O(1) framing (center + radius) for the acorn pair while dragging. It
/// approximates the exact transformed bounds with per-hemisphere bounding
/// spheres (transformed component center + the mesh's bounding-sphere radius),
/// so no per-vertex work runs per frame. It slightly over-estimates the exact
/// fit the baked release path computes, which can show as a small scale change
/// when the drag ends.
pub(super) fn pair_framing(
    components: &[SceneSurfaceComponent],
    transforms: &[ComponentTransform],
    visibility: PairVisibility,
) -> Option<(Vec3, f32)> {
    let mut bounds = TransformedBounds::empty();
    let mut any_visible = false;
    for (component, transform) in components.iter().zip(transforms) {
        if !visibility.is_visible(&component.side) {
            continue;
        }
        let Some(mesh) = component.mesh.as_ref() else {
            continue;
        };
        let corner = transformed_corner_bounds(mesh, *transform);
        bounds.include(corner.min);
        bounds.include(corner.max);
        any_visible = true;
    }
    if !any_visible {
        return None;
    }

    let center = bounds.center();
    let radius = components
        .iter()
        .zip(transforms)
        .filter(|(component, _)| visibility.is_visible(&component.side))
        .filter_map(|(component, transform)| component.mesh.as_ref().map(|mesh| (mesh, transform)))
        .map(|(mesh, transform)| {
            let transformed_center =
                transform_point(mesh, *transform, Vec3::from_array(mesh.bounds.center));
            transformed_center.distance(center) + mesh.bounds.radius
        })
        .fold(0.0_f32, f32::max)
        .max(1.0);

    Some((center, radius))
}

/// Per-hemisphere display model matrices for the current layout. Cheap (no
/// per-vertex allocation, transform, or upload) so dragging only writes small
/// uniforms.
pub(super) fn pair_hemisphere_matrices(
    components: &[SceneSurfaceComponent],
    layout: HemisphereLayoutState,
    visibility: PairVisibility,
) -> Vec<(SurfaceSide, Mat4)> {
    let transforms = component_transforms(components, layout);
    let Some((center, radius)) = pair_framing(components, &transforms, visibility) else {
        return Vec::new();
    };
    components
        .iter()
        .zip(transforms)
        .filter_map(|(component, transform)| {
            let mesh = component.mesh.as_ref()?;
            Some((
                component.side.clone(),
                hemisphere_model_matrix(
                    transform
                        .rotation_pivot
                        .unwrap_or_else(|| Vec3::from_array(mesh.bounds.center)),
                    transform.rotation_z_degrees,
                    transform.offset,
                    center,
                    radius,
                ),
            ))
        })
        .collect()
}

pub(super) fn component_transforms(
    components: &[SceneSurfaceComponent],
    layout: HemisphereLayoutState,
) -> Vec<ComponentTransform> {
    let mut transforms = vec![ComponentTransform::default(); components.len()];
    if components.len() != 2 {
        return transforms;
    }

    let Some(left_index) = components
        .iter()
        .position(|component| component.side == SurfaceSide::Left)
    else {
        return transforms;
    };
    let Some(right_index) = components
        .iter()
        .position(|component| component.side == SurfaceSide::Right)
    else {
        return transforms;
    };

    let Some(left_mesh) = components[left_index].mesh.as_ref() else {
        return transforms;
    };
    let Some(right_mesh) = components[right_index].mesh.as_ref() else {
        return transforms;
    };

    let clearance = pair_default_clearance(left_mesh, right_mesh);
    let auto_spread = pair_auto_spread_distance(left_mesh, right_mesh, layout.open_angle_degrees);
    let mut half_shift = ((clearance + layout.separation_distance) * 0.5) + auto_spread;

    transforms[left_index].offset.x -= half_shift;
    transforms[left_index].rotation_pivot =
        Some(pair_medial_rotation_pivot(left_mesh, &SurfaceSide::Left));
    transforms[left_index].rotation_z_degrees = layout.open_angle_degrees;
    transforms[right_index].offset.x += half_shift;
    transforms[right_index].rotation_pivot =
        Some(pair_medial_rotation_pivot(right_mesh, &SurfaceSide::Right));
    transforms[right_index].rotation_z_degrees = -layout.open_angle_degrees;

    let extra_spacing = pair_bounds_overlap_extra_spacing(
        left_mesh,
        right_mesh,
        transforms[left_index],
        transforms[right_index],
    );
    if extra_spacing > 0.0 {
        half_shift += extra_spacing * 0.5;
        transforms[left_index].offset.x = -half_shift;
        transforms[right_index].offset.x = half_shift;
    }

    transforms
}

/// Builds the affine model matrix that moves one raw hemisphere into the active
/// paired layout, so the GPU can apply a small uniform update instead of the CPU
/// re-transforming and re-uploading every vertex on each drag frame.
///
/// The baked transform is `p -> (pivot + R*(p - pivot) + offset - center) /
/// radius`, where `pivot` is the hemisphere hinge point, `R` the Z rotation,
/// and `center`/`radius` the scene normalization. The uniform scale keeps
/// normals correct under the same matrix (the shader renormalizes them).
pub(super) fn hemisphere_model_matrix(
    rotation_pivot: Vec3,
    rotation_z_degrees: f32,
    offset: Vec3,
    scene_center: Vec3,
    radius: f32,
) -> Mat4 {
    let inv_radius = 1.0 / radius;
    Mat4::from_scale(Vec3::splat(inv_radius))
        * Mat4::from_translation(rotation_pivot + offset - scene_center)
        * Mat4::from_rotation_z(rotation_z_degrees.to_radians())
        * Mat4::from_translation(-rotation_pivot)
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TransformedBounds {
    pub(super) min: Vec3,
    pub(super) max: Vec3,
}

impl TransformedBounds {
    pub(super) fn empty() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }

    pub(super) fn include(&mut self, point: Vec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    pub(super) fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

pub(super) fn transform_point(mesh: &SurfaceMesh, transform: ComponentTransform, point: Vec3) -> Vec3 {
    let pivot = transform
        .rotation_pivot
        .unwrap_or_else(|| Vec3::from_array(mesh.bounds.center));
    let rotation = Quat::from_rotation_z(transform.rotation_z_degrees.to_radians());
    pivot + rotation * (point - pivot) + transform.offset
}

pub(super) fn pair_medial_rotation_pivot(mesh: &SurfaceMesh, side: &SurfaceSide) -> Vec3 {
    let center = Vec3::from_array(mesh.bounds.center);
    let medial_x = match side {
        SurfaceSide::Left => mesh.bounds.max[0],
        SurfaceSide::Right => mesh.bounds.min[0],
        _ => center.x,
    };

    Vec3::new(medial_x, center.y, center.z)
}

pub(super) fn transformed_corner_bounds(
    mesh: &SurfaceMesh,
    transform: ComponentTransform,
) -> TransformedBounds {
    let min = Vec3::from_array(mesh.bounds.min);
    let max = Vec3::from_array(mesh.bounds.max);
    let mut bounds = TransformedBounds::empty();
    for x in [min.x, max.x] {
        for y in [min.y, max.y] {
            for z in [min.z, max.z] {
                bounds.include(transform_point(mesh, transform, Vec3::new(x, y, z)));
            }
        }
    }

    bounds
}

pub(super) fn pair_bounds_overlap_extra_spacing(
    left_mesh: &SurfaceMesh,
    right_mesh: &SurfaceMesh,
    left_transform: ComponentTransform,
    right_transform: ComponentTransform,
) -> f32 {
    let left = transformed_corner_bounds(left_mesh, left_transform);
    let right = transformed_corner_bounds(right_mesh, right_transform);
    let x_overlap = left.max.x.min(right.max.x) - left.min.x.max(right.min.x);
    let y_overlap = left.max.y.min(right.max.y) - left.min.y.max(right.min.y);
    let z_overlap = left.max.z.min(right.max.z) - left.min.z.max(right.min.z);
    if x_overlap <= 0.0 || y_overlap <= 0.0 || z_overlap <= 0.0 {
        return 0.0;
    }

    x_overlap + PAIR_MIN_SURFACE_CLEARANCE
}

pub(super) fn pair_reference_width(left_mesh: &SurfaceMesh, right_mesh: &SurfaceMesh) -> f32 {
    let min_x = left_mesh.bounds.min[0].min(right_mesh.bounds.min[0]);
    let max_x = left_mesh.bounds.max[0].max(right_mesh.bounds.max[0]);
    (max_x - min_x).abs().max(1.0)
}

pub(super) fn pair_default_clearance(left_mesh: &SurfaceMesh, right_mesh: &SurfaceMesh) -> f32 {
    let desired_gap = pair_reference_width(left_mesh, right_mesh) * PAIR_MIN_CLEARANCE_FRACTION;
    let current_gap = right_mesh.bounds.min[0] - left_mesh.bounds.max[0];
    (desired_gap - current_gap).max(0.0)
}

pub(super) fn pair_auto_spread_distance(
    left_mesh: &SurfaceMesh,
    right_mesh: &SurfaceMesh,
    open_angle_degrees: f32,
) -> f32 {
    let left_half_width = ((left_mesh.bounds.max[0] - left_mesh.bounds.min[0]) * 0.5).max(0.0);
    let right_half_width = ((right_mesh.bounds.max[0] - right_mesh.bounds.min[0]) * 0.5).max(0.0);
    let mean_half_width = (left_half_width + right_half_width) * 0.5;
    mean_half_width * open_angle_degrees.abs().to_radians().sin() * 0.9
}

pub(super) fn pair_open_percent(open_angle_degrees: f32) -> f32 {
    (open_angle_degrees / PAIR_MAX_OPEN_DEGREES) * 100.0
}

pub(super) fn pair_open_percent_label(open_angle_degrees: f32) -> String {
    let percent = pair_open_percent(open_angle_degrees);
    if percent.abs() < 0.5 {
        "0%".to_string()
    } else {
        format!("{percent:+.0}%")
    }
}

