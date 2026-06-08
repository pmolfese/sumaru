use glam::Vec3;
use winit::dpi::PhysicalSize;

use crate::surface::{OverlayDataset, SurfaceMesh};

use super::SurfacePick;
use super::camera::{CAMERA_FOV_Y_RADIANS, Camera};

const PICK_EPSILON: f32 = 1.0e-6;

#[derive(Debug, Clone, Copy, PartialEq)]
struct PickRay {
    origin: Vec3,
    direction: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RayTriangleHit {
    distance: f32,
}

pub(super) fn pick_surface(
    mesh: &SurfaceMesh,
    overlay: Option<&OverlayDataset>,
    camera: &Camera,
    view_size: PhysicalSize<u32>,
    cursor: (f64, f64),
) -> Option<SurfacePick> {
    let ray = screen_ray_for_camera(camera, view_size, cursor)?;
    let center = Vec3::from_array(mesh.bounds.center);
    let scale = if mesh.bounds.radius > f32::EPSILON {
        1.0 / mesh.bounds.radius
    } else {
        1.0
    };
    let mut best_pick = None;
    let mut best_distance = f32::INFINITY;

    for (face_index, triangle) in mesh.triangles.iter().copied().enumerate() {
        let Some(positions) = normalized_triangle_positions(mesh, triangle, center, scale) else {
            continue;
        };
        let Some(hit) = ray_triangle_intersection(
            ray.origin,
            ray.direction,
            positions[0],
            positions[1],
            positions[2],
        ) else {
            continue;
        };

        if hit.distance < best_distance {
            let hit_position = ray.origin + ray.direction * hit.distance;
            let node_index = closest_triangle_node(triangle, positions, hit_position);
            let overlay_value = overlay
                .and_then(|overlay| overlay.values.get(node_index as usize))
                .copied();

            best_distance = hit.distance;
            best_pick = Some(SurfacePick {
                node_index,
                face_index,
                overlay_value,
            });
        }
    }

    best_pick
}

fn screen_ray_for_camera(
    camera: &Camera,
    view_size: PhysicalSize<u32>,
    cursor: (f64, f64),
) -> Option<PickRay> {
    if view_size.width == 0 || view_size.height == 0 {
        return None;
    }

    let cursor_x = cursor.0 as f32;
    let cursor_y = cursor.1 as f32;
    if !cursor_x.is_finite() || !cursor_y.is_finite() {
        return None;
    }

    let width = view_size.width as f32;
    let height = view_size.height as f32;
    let ndc_x = (cursor_x / width) * 2.0 - 1.0;
    let ndc_y = 1.0 - (cursor_y / height) * 2.0;
    let aspect = (width / height).max(0.01);
    let (eye_direction, up) = camera.view_axes();
    let eye_direction = eye_direction.normalize();
    let up = up.normalize();
    let right = up.cross(eye_direction).normalize();
    let forward = -eye_direction;
    let tan_half_fov = (CAMERA_FOV_Y_RADIANS * 0.5).tan();
    let direction =
        (forward + right * ndc_x * aspect * tan_half_fov + up * ndc_y * tan_half_fov).normalize();

    Some(PickRay {
        origin: eye_direction * camera.distance,
        direction,
    })
}

fn normalized_triangle_positions(
    mesh: &SurfaceMesh,
    triangle: [u32; 3],
    center: Vec3,
    scale: f32,
) -> Option<[Vec3; 3]> {
    Some([
        normalized_vertex_position(mesh, triangle[0], center, scale)?,
        normalized_vertex_position(mesh, triangle[1], center, scale)?,
        normalized_vertex_position(mesh, triangle[2], center, scale)?,
    ])
}

fn normalized_vertex_position(
    mesh: &SurfaceMesh,
    node_index: u32,
    center: Vec3,
    scale: f32,
) -> Option<Vec3> {
    mesh.vertices
        .get(node_index as usize)
        .map(|position| (Vec3::from_array(*position) - center) * scale)
}

fn ray_triangle_intersection(
    origin: Vec3,
    direction: Vec3,
    a: Vec3,
    b: Vec3,
    c: Vec3,
) -> Option<RayTriangleHit> {
    let edge_ab = b - a;
    let edge_ac = c - a;
    let p = direction.cross(edge_ac);
    let determinant = edge_ab.dot(p);

    if determinant.abs() <= PICK_EPSILON {
        return None;
    }

    let inverse_determinant = 1.0 / determinant;
    let origin_to_a = origin - a;
    let u = origin_to_a.dot(p) * inverse_determinant;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }

    let q = origin_to_a.cross(edge_ab);
    let v = direction.dot(q) * inverse_determinant;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }

    let distance = edge_ac.dot(q) * inverse_determinant;
    (distance > PICK_EPSILON).then_some(RayTriangleHit { distance })
}

fn closest_triangle_node(triangle: [u32; 3], positions: [Vec3; 3], point: Vec3) -> u32 {
    let mut closest_node = triangle[0];
    let mut closest_distance = positions[0].distance_squared(point);

    for (node_index, position) in triangle.into_iter().zip(positions).skip(1) {
        let distance = position.distance_squared(point);
        if distance < closest_distance {
            closest_node = node_index;
            closest_distance = distance;
        }
    }

    closest_node
}

#[cfg(test)]
mod tests {
    use super::super::camera::{Camera, PresetOrientation};
    use super::{
        closest_triangle_node, pick_surface, ray_triangle_intersection, screen_ray_for_camera,
    };
    use crate::surface::{OverlayDataset, SurfaceMesh, ValueRange};
    use glam::Vec3;
    use winit::dpi::PhysicalSize;

    #[test]
    fn center_screen_ray_points_toward_camera_target() {
        let camera = Camera::default();
        let (eye_direction, _) = camera.view_axes();

        let ray =
            screen_ray_for_camera(&camera, PhysicalSize::new(100, 100), (50.0, 50.0)).unwrap();

        assert_vec3_close(ray.origin, eye_direction * camera.distance);
        assert!(ray.direction.dot(-eye_direction) > 0.999);
    }

    #[test]
    fn ray_triangle_intersection_hits_triangle() {
        let hit = ray_triangle_intersection(
            Vec3::new(0.25, 0.25, 1.0),
            Vec3::NEG_Z,
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
        )
        .unwrap();

        assert!((hit.distance - 1.0).abs() < 0.0001);
    }

    #[test]
    fn closest_triangle_node_uses_hit_position() {
        let triangle = [10, 11, 12];
        let positions = [Vec3::ZERO, Vec3::X, Vec3::Y];

        assert_eq!(
            closest_triangle_node(triangle, positions, Vec3::new(0.1, 0.8, 0.0)),
            12
        );
    }

    #[test]
    fn surface_pick_reports_node_triangle_and_overlay_value() {
        let mesh = SurfaceMesh::new(
            vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let overlay = OverlayDataset {
            values: vec![10.0, 20.0, 30.0],
            range: ValueRange {
                min: 10.0,
                max: 30.0,
            },
        };
        let mut camera = Camera::default();
        camera.set_preset(PresetOrientation::Top);

        let pick = pick_surface(
            &mesh,
            Some(&overlay),
            &camera,
            PhysicalSize::new(100, 100),
            (50.0, 50.0),
        )
        .unwrap();

        assert_eq!(pick.node_index, 2);
        assert_eq!(pick.face_index, 0);
        assert_eq!(pick.overlay_value, Some(30.0));
    }

    fn assert_vec3_close(actual: Vec3, expected: Vec3) {
        assert!((actual - expected).length() < 0.0001);
    }
}
