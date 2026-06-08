use glam::{Mat3, Mat4, Quat, Vec3};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};

pub(super) const CAMERA_FOV_Y_RADIANS: f32 = std::f32::consts::FRAC_PI_4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CameraMode {
    Orbit,
    Turntable,
}

impl CameraMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            CameraMode::Orbit => "orbit",
            CameraMode::Turntable => "turntable",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PresetOrientation {
    Left,
    Right,
    Top,
    Bottom,
}

pub(super) struct Camera {
    mode: CameraMode,
    orientation: Quat,
    yaw: f32,
    pitch: f32,
    pub(super) distance: f32,
    rotating: bool,
    last_cursor: Option<(f64, f64)>,
}

impl Default for Camera {
    fn default() -> Self {
        let mut camera = Self {
            mode: CameraMode::Orbit,
            orientation: Quat::IDENTITY,
            yaw: 0.0,
            pitch: 0.25,
            distance: 3.0,
            rotating: false,
            last_cursor: None,
        };
        camera.sync_orientation_from_angles();
        camera
    }
}

impl Camera {
    pub(super) fn pointer_input(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.rotating = *state == ElementState::Pressed;
                if !self.rotating {
                    self.last_cursor = None;
                }
                true
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.rotating {
                    if let Some((last_x, last_y)) = self.last_cursor {
                        let dx = position.x - last_x;
                        let dy = position.y - last_y;
                        self.drag(dx as f32, dy as f32);
                    }
                    self.last_cursor = Some((position.x, position.y));
                    return true;
                }

                false
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32 / 120.0,
                };
                self.distance = (self.distance * 0.9_f32.powf(scroll)).clamp(0.75, 25.0);
                true
            }
            _ => false,
        }
    }

    pub(super) fn mode(&self) -> CameraMode {
        self.mode
    }

    pub(super) fn toggle_mode(&mut self) -> CameraMode {
        self.mode = match self.mode {
            CameraMode::Orbit => {
                self.sync_angles_from_orientation();
                CameraMode::Turntable
            }
            CameraMode::Turntable => {
                self.sync_orientation_from_angles();
                CameraMode::Orbit
            }
        };
        self.mode
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn set_preset(&mut self, preset: PresetOrientation) {
        match preset {
            PresetOrientation::Left => self.set_view_direction(Vec3::NEG_X, Vec3::Z),
            PresetOrientation::Right => self.set_view_direction(Vec3::X, Vec3::Z),
            PresetOrientation::Top => self.set_view_direction(Vec3::Z, Vec3::Y),
            PresetOrientation::Bottom => self.set_view_direction(Vec3::NEG_Z, Vec3::Y),
        }
    }

    fn drag(&mut self, dx: f32, dy: f32) {
        let sensitivity = 0.01;

        match self.mode {
            CameraMode::Orbit => {
                let yaw = Quat::from_axis_angle(Vec3::Z, -dx * sensitivity);
                let right = self.orientation * Vec3::X;
                let pitch = Quat::from_axis_angle(right.normalize(), -dy * sensitivity);
                self.orientation = (yaw * pitch * self.orientation).normalize();
                self.sync_angles_from_orientation();
            }
            CameraMode::Turntable => {
                self.yaw -= dx * sensitivity;
                self.pitch = (self.pitch - dy * sensitivity).clamp(-1.45, 1.45);
                self.sync_orientation_from_angles();
            }
        }
    }

    pub(super) fn uniform_bytes(&self, aspect: f32) -> Vec<u8> {
        let view_projection = self.view_projection(aspect);
        let model = Mat4::IDENTITY;
        let light_direction = Vec3::new(0.35, 0.8, 0.45).normalize();
        let surface_color = [0.76, 0.78, 0.74, 1.0];
        let floats = [
            view_projection.to_cols_array().as_slice(),
            model.to_cols_array().as_slice(),
            &[light_direction.x, light_direction.y, light_direction.z, 0.0],
            &surface_color,
        ]
        .concat();

        f32_bytes(&floats)
    }

    fn view_projection(&self, aspect: f32) -> Mat4 {
        let (eye_direction, up) = self.view_axes();
        let eye = eye_direction * self.distance;
        let view = Mat4::look_at_rh(eye, Vec3::ZERO, up);
        let projection = Mat4::perspective_rh(CAMERA_FOV_Y_RADIANS, aspect.max(0.01), 0.01, 100.0);

        projection * view
    }

    pub(super) fn view_axes(&self) -> (Vec3, Vec3) {
        match self.mode {
            CameraMode::Orbit => (self.orientation * Vec3::Z, self.orientation * Vec3::Y),
            CameraMode::Turntable => {
                let eye_direction = self.eye_direction_from_angles();
                let up = stable_up_for_direction(eye_direction);
                (eye_direction, up)
            }
        }
    }

    fn eye_direction_from_angles(&self) -> Vec3 {
        let pitch_cos = self.pitch.cos();
        Vec3::new(
            self.yaw.sin() * pitch_cos,
            self.yaw.cos() * pitch_cos,
            self.pitch.sin(),
        )
        .normalize()
    }

    fn sync_orientation_from_angles(&mut self) {
        let eye_direction = self.eye_direction_from_angles();
        self.orientation = orientation_for(eye_direction, stable_up_for_direction(eye_direction));
    }

    fn sync_angles_from_orientation(&mut self) {
        let eye_direction = (self.orientation * Vec3::Z).normalize();
        self.pitch = eye_direction.z.asin().clamp(-1.45, 1.45);
        self.yaw = eye_direction.x.atan2(eye_direction.y);
    }

    fn set_view_direction(&mut self, eye_direction: Vec3, up: Vec3) {
        self.orientation = orientation_for(eye_direction, up);
        self.sync_angles_from_orientation();
    }
}

fn orientation_for(eye_direction: Vec3, up_hint: Vec3) -> Quat {
    let eye_direction = eye_direction.normalize();
    let mut right = up_hint.cross(eye_direction);

    if right.length_squared() <= f32::EPSILON {
        right = Vec3::X;
    }

    let right = right.normalize();
    let up = eye_direction.cross(right).normalize();

    Quat::from_mat3(&Mat3::from_cols(right, up, eye_direction)).normalize()
}

pub(super) fn stable_up_for_direction(eye_direction: Vec3) -> Vec3 {
    if eye_direction.normalize().dot(Vec3::Z).abs() > 0.95 {
        Vec3::Y
    } else {
        Vec3::Z
    }
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
    use super::{Camera, CameraMode, PresetOrientation};

    #[test]
    fn camera_mode_toggles_between_orbit_and_turntable() {
        let mut camera = Camera::default();

        assert_eq!(camera.toggle_mode(), CameraMode::Turntable);
        assert_eq!(camera.toggle_mode(), CameraMode::Orbit);
    }

    #[test]
    fn option_up_preset_points_camera_from_top() {
        let mut camera = Camera::default();

        camera.set_preset(PresetOrientation::Top);
        let (eye_direction, _) = camera.view_axes();

        assert!(eye_direction.z > 0.99);
    }
}
