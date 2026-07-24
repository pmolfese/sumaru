use crate::command::LightingMode;
use glam::{Mat3, Mat4, Quat, Vec3};
use std::time::Duration;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};

pub(super) const CAMERA_FOV_Y_RADIANS: f32 = std::f32::consts::FRAC_PI_4;
const KEYBOARD_NUDGE_RADIANS: f32 = std::f32::consts::PI / 36.0;
const MOMENTUM_MIN_DELTA_PIXELS: f32 = 0.01;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PresetOrientation {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CameraNudgeDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone)]
pub(super) struct Camera {
    mode: CameraMode,
    orientation: Quat,
    yaw: f32,
    pitch: f32,
    pub(super) distance: f32,
    rotating: bool,
    last_cursor: Option<(f64, f64)>,
    momentum_enabled: bool,
    momentum_delta: (f32, f32),
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
            momentum_enabled: false,
            momentum_delta: (0.0, 0.0),
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
                } else {
                    self.momentum_delta = (0.0, 0.0);
                }
                true
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.rotating {
                    if let Some((last_x, last_y)) = self.last_cursor {
                        let dx = position.x - last_x;
                        let dy = position.y - last_y;
                        self.drag(dx as f32, dy as f32);
                        if self.momentum_enabled
                            && (dx.abs() > f64::EPSILON || dy.abs() > f64::EPSILON)
                        {
                            self.momentum_delta = (dx as f32, dy as f32);
                        }
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
                self.stop_momentum();
                true
            }
            _ => false,
        }
    }

    pub(super) fn mode(&self) -> CameraMode {
        self.mode
    }

    pub(super) fn toggle_mode(&mut self) -> CameraMode {
        self.stop_momentum();
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
        self.stop_momentum();
        match preset {
            PresetOrientation::Left => self.set_view_direction(Vec3::NEG_X, Vec3::Z),
            PresetOrientation::Right => self.set_view_direction(Vec3::X, Vec3::Z),
            PresetOrientation::Top => self.set_view_direction(Vec3::Z, Vec3::Y),
            PresetOrientation::Bottom => self.set_view_direction(Vec3::NEG_Z, Vec3::Y),
        }
    }

    pub(super) fn nudge(&mut self, direction: CameraNudgeDirection) {
        self.stop_momentum();
        match direction {
            CameraNudgeDirection::Left => self.drag_angle(-KEYBOARD_NUDGE_RADIANS, 0.0),
            CameraNudgeDirection::Right => self.drag_angle(KEYBOARD_NUDGE_RADIANS, 0.0),
            CameraNudgeDirection::Up => self.drag_angle(0.0, -KEYBOARD_NUDGE_RADIANS),
            CameraNudgeDirection::Down => self.drag_angle(0.0, KEYBOARD_NUDGE_RADIANS),
        }
    }

    pub(super) fn momentum_enabled(&self) -> bool {
        self.momentum_enabled
    }

    pub(super) fn momentum_active(&self) -> bool {
        self.momentum_enabled
            && !self.rotating
            && self.momentum_delta_magnitude() >= MOMENTUM_MIN_DELTA_PIXELS
    }

    pub(super) fn toggle_momentum(&mut self) -> bool {
        self.momentum_enabled = !self.momentum_enabled;
        if !self.momentum_enabled {
            self.stop_momentum();
        }
        self.momentum_enabled
    }

    pub(super) fn tick_momentum(&mut self, _elapsed: Duration) -> bool {
        if !self.momentum_active() {
            return false;
        }

        self.drag(self.momentum_delta.0, self.momentum_delta.1);
        true
    }

    fn stop_momentum(&mut self) {
        self.momentum_delta = (0.0, 0.0);
    }

    fn momentum_delta_magnitude(&self) -> f32 {
        self.momentum_delta.0.hypot(self.momentum_delta.1)
    }

    #[cfg(test)]
    fn set_momentum_delta_for_test(&mut self, dx: f32, dy: f32) {
        self.momentum_delta = (dx, dy);
    }

    fn drag(&mut self, dx: f32, dy: f32) {
        let sensitivity = 0.01;
        self.drag_angle(dx * sensitivity, dy * sensitivity);
    }

    fn drag_angle(&mut self, dx_radians: f32, dy_radians: f32) {
        match self.mode {
            CameraMode::Orbit => {
                let yaw = Quat::from_axis_angle(Vec3::Z, -dx_radians);
                let right = self.orientation * Vec3::X;
                let pitch = Quat::from_axis_angle(right.normalize(), -dy_radians);
                self.orientation = (yaw * pitch * self.orientation).normalize();
                self.sync_angles_from_orientation();
            }
            CameraMode::Turntable => {
                self.yaw -= dx_radians;
                self.pitch = (self.pitch - dy_radians).clamp(-1.45, 1.45);
                self.sync_orientation_from_angles();
            }
        }
    }

    /// Build the camera + lighting uniform block, with an explicit model
    /// matrix used to draw each acorn hemisphere with its own transform while
    /// dragging or in paired renders.
    pub(super) fn uniform_bytes_with_model(
        &self,
        aspect: f32,
        model: Mat4,
        lighting_mode: LightingMode,
        surface_opacity: f32,
    ) -> Vec<u8> {
        let view_projection = self.view_projection(aspect);
        let lighting = self.lighting_uniforms(lighting_mode);
        let surface_color = [0.76, 0.78, 0.74, surface_opacity.clamp(0.0, 1.0)];
        let floats = [
            view_projection.to_cols_array().as_slice(),
            model.to_cols_array().as_slice(),
            &lighting.primary_direction,
            &lighting.secondary_direction,
            &lighting.tertiary_direction,
            &lighting.weights,
            &lighting.params,
            &surface_color,
        ]
        .concat();

        super::f32_bytes(&floats)
    }

    /// The view-projection matrix, exposed for non-surface scene content (e.g.
    /// volume slice planes) that draws in the same camera space.
    pub(super) fn view_projection_matrix(&self, aspect: f32) -> Mat4 {
        self.view_projection(aspect)
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

    pub(super) fn set_view_direction(&mut self, eye_direction: Vec3, up: Vec3) {
        self.orientation = orientation_for(eye_direction, up);
        self.sync_angles_from_orientation();
    }

    fn lighting_uniforms(&self, lighting_mode: LightingMode) -> LightingUniforms {
        let world = Vec3::new(0.35, 0.8, 0.45).normalize();
        let (eye_direction, up) = self.view_axes();
        let eye = eye_direction.normalize();
        let up = up.normalize();
        let right = up.cross(eye).normalize_or_zero();
        let primary = |direction: Vec3| [direction.x, direction.y, direction.z, 0.0];

        match lighting_mode {
            LightingMode::Directional => LightingUniforms {
                primary_direction: primary(world),
                secondary_direction: primary(world),
                tertiary_direction: primary(world),
                weights: [1.0, 0.0, 0.0, 0.0],
                params: [0.28, 0.72, 0.0, 0.0],
            },
            LightingMode::DirectionalSoft => LightingUniforms {
                primary_direction: primary(world),
                secondary_direction: primary(world),
                tertiary_direction: primary(world),
                weights: [1.0, 0.0, 0.0, 0.0],
                params: [0.58, 0.42, 0.0, 0.0],
            },
            LightingMode::Headlight => LightingUniforms {
                primary_direction: primary(eye),
                secondary_direction: primary(eye),
                tertiary_direction: primary(eye),
                weights: [1.0, 0.0, 0.0, 0.0],
                params: [0.40, 0.60, 0.0, 0.0],
            },
            LightingMode::Studio => {
                let fill_a = (eye + right * 0.85 + up * 0.35).normalize_or_zero();
                let fill_b = (eye - right * 0.65 - up * 0.20).normalize_or_zero();
                LightingUniforms {
                    primary_direction: primary(eye),
                    secondary_direction: primary(fill_a),
                    tertiary_direction: primary(fill_b),
                    weights: [0.55, 0.25, 0.20, 0.0],
                    params: [0.34, 0.66, 0.0, 0.0],
                }
            }
            LightingMode::Flat => LightingUniforms {
                primary_direction: primary(world),
                secondary_direction: primary(world),
                tertiary_direction: primary(world),
                weights: [0.0, 0.0, 0.0, 0.0],
                params: [1.0, 0.0, 0.0, 0.0],
            },
        }
    }
}

struct LightingUniforms {
    primary_direction: [f32; 4],
    secondary_direction: [f32; 4],
    tertiary_direction: [f32; 4],
    weights: [f32; 4],
    params: [f32; 4],
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

#[cfg(test)]
mod tests {
    use super::{Camera, CameraMode, CameraNudgeDirection, PresetOrientation};
    use std::time::Duration;

    const FIVE_DEGREES: f32 = std::f32::consts::PI / 36.0;

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

    #[test]
    fn arrow_nudges_rotate_camera_by_five_degrees() {
        let mut camera = Camera::default();
        let start_yaw = camera.yaw;
        let start_pitch = camera.pitch;

        camera.nudge(CameraNudgeDirection::Right);
        assert!((camera.yaw - (start_yaw + FIVE_DEGREES)).abs() < 0.000_001);
        assert!((camera.pitch - start_pitch).abs() < 0.000_001);

        camera.nudge(CameraNudgeDirection::Left);
        assert!((camera.yaw - start_yaw).abs() < 0.000_001);

        camera.nudge(CameraNudgeDirection::Up);
        assert!((camera.pitch - (start_pitch - FIVE_DEGREES)).abs() < 0.000_001);

        camera.nudge(CameraNudgeDirection::Down);
        assert!((camera.pitch - start_pitch).abs() < 0.000_001);
    }

    #[test]
    fn momentum_ticks_continue_last_drag_direction_until_disabled() {
        let mut camera = Camera::default();
        let (before, _) = camera.view_axes();

        assert!(camera.toggle_momentum());
        camera.set_momentum_delta_for_test(12.0, 0.0);
        assert!(camera.tick_momentum(Duration::from_millis(16)));

        let (after, _) = camera.view_axes();
        assert!(before.distance(after) > 0.001);

        assert!(!camera.toggle_momentum());
        let (disabled_before, _) = camera.view_axes();
        assert!(!camera.tick_momentum(Duration::from_millis(16)));
        let (disabled_after, _) = camera.view_axes();
        assert!(disabled_before.distance(disabled_after) < 0.000_001);
    }
}
