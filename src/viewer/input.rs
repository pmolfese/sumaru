//! Window input handling for the viewer windows: the view window's mouse and
//! keyboard routing (camera, pair-drag, ROI picking, volume slice scrubbing,
//! and keyboard shortcuts) plus the egui passthrough for the control, ROI, and
//! graph windows. Extracted from `viewer/mod.rs`; all handlers stay on
//! `ViewerState`.

use super::*;

impl ViewerState {
    pub(super) fn view_input(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::ModifiersChanged(modifiers) = event {
            self.modifiers = modifiers.state();
            if !self.modifiers.control_key() && self.pair_dragging {
                self.finish_pair_drag();
            }
        }

        let egui_response = self
            .view
            .egui
            .state
            .on_window_event(&self.view.window, event);
        if egui_response.repaint {
            self.view.window.request_redraw();
        }
        if egui_response.consumed {
            return true;
        }

        match event {
            WindowEvent::ModifiersChanged(_) => false,
            WindowEvent::CursorMoved { position, .. } => {
                let cursor = (position.x, position.y);
                self.view_cursor_position = Some(cursor);
                if self.pair_dragging {
                    self.update_pair_drag(cursor);
                    return true;
                }
                if self.volume_slice_drag.is_some() {
                    self.update_volume_slice_drag();
                    return true;
                }

                self.camera.pointer_input(event)
            }
            WindowEvent::MouseInput { state, button, .. }
                if self.pair_dragging
                    && matches!(*button, MouseButton::Left | MouseButton::Right) =>
            {
                if *state == ElementState::Released {
                    self.finish_pair_drag();
                }
                true
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.volume_slice_drag.is_some() => {
                self.volume_slice_drag = None;
                true
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if self.volume_view.is_some()
                && !self.modifiers.control_key()
                && self.try_begin_volume_slice_drag() =>
            {
                true
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if self.modifiers.control_key()
                && self.has_both_scene()
                && matches!(*button, MouseButton::Left | MouseButton::Right) =>
            {
                self.begin_pair_drag();
                true
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } if self.volume_view.is_some() => {
                self.select_volume_plane_at_cursor();
                true
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                let roi_draw_active = self
                    .roi_workspace
                    .active_draft()
                    .is_some_and(|draft| draft.state.draw_enabled || draft.state.fill_pending);
                if roi_draw_active {
                    if let Err(error) = self.handle_roi_draw_click_at_cursor() {
                        self.set_error(error);
                    }
                } else {
                    self.inspect_surface_at_cursor();
                }
                true
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyR) if self.modifiers.control_key() => {
                        self.set_roi_controller_open(true);
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyS) if self.modifiers.control_key() => {
                        self.set_surface_controller_visible(
                            !self.controller.panels.surface_controller_visible,
                        );
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyT) if self.modifiers.control_key() => {
                        if let Err(error) = self.force_resend_afni_surfaces() {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyT) => {
                        if let Err(error) = self.toggle_afni_talk() {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyC) => {
                        let mode = self.camera.toggle_mode();
                        self.controller.camera.mode = mode.into();
                        self.show_mode_label(mode);
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyL)
                        if !self.modifiers.shift_key()
                            && !self.modifiers.control_key()
                            && !self.modifiers.alt_key() =>
                    {
                        self.cycle_lighting_mode();
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyM) => {
                        self.toggle_camera_momentum();
                        true
                    }
                    PhysicalKey::Code(KeyCode::Space) => {
                        self.camera.reset();
                        self.controller.camera.note_reset();
                        true
                    }
                    PhysicalKey::Code(KeyCode::F5) => {
                        self.apply_commands(vec![ViewerCommand::ToggleBackground]);
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyR) if self.modifiers.shift_key() => {
                        if let Err(error) = self.save_preset_montage_screenshot() {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyR) => {
                        if let Err(error) = self.save_current_view_screenshot() {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyP)
                        if !self.modifiers.shift_key()
                            && !self.modifiers.control_key()
                            && !self.modifiers.alt_key() =>
                    {
                        self.apply_commands(vec![ViewerCommand::ToggleSurfaceRenderStyle]);
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyO)
                        if !self.modifiers.shift_key()
                            && !self.modifiers.control_key()
                            && !self.modifiers.alt_key() =>
                    {
                        self.apply_commands(vec![ViewerCommand::CycleSurfaceOpacity]);
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyO) if self.modifiers.shift_key() => {
                        self.toggle_overlay_visibility();
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyG) => {
                        if let Err(error) = self.open_graph_for_current_pick() {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::BracketLeft) => {
                        if let Err(error) =
                            self.toggle_pair_hemisphere_visibility(SurfaceSide::Left)
                        {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::BracketRight) => {
                        if let Err(error) =
                            self.toggle_pair_hemisphere_visibility(SurfaceSide::Right)
                        {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::Period) => match self.cycle_scene_surface(1) {
                        Ok(changed) => changed,
                        Err(error) => {
                            self.set_error(error);
                            true
                        }
                    },
                    PhysicalKey::Code(KeyCode::Comma) => match self.cycle_scene_surface(-1) {
                        Ok(changed) => changed,
                        Err(error) => {
                            self.set_error(error);
                            true
                        }
                    },
                    PhysicalKey::Code(KeyCode::ArrowLeft) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Left);
                        self.controller.camera.set_preset(ViewPreset::Left);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowRight) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Right);
                        self.controller.camera.set_preset(ViewPreset::Right);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowUp) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Top);
                        self.controller.camera.set_preset(ViewPreset::Top);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowDown) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Bottom);
                        self.controller.camera.set_preset(ViewPreset::Bottom);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowLeft) => {
                        self.camera.nudge(CameraNudgeDirection::Left);
                        self.controller.camera.note_manual_motion();
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowRight) => {
                        self.camera.nudge(CameraNudgeDirection::Right);
                        self.controller.camera.note_manual_motion();
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowUp) => {
                        self.camera.nudge(CameraNudgeDirection::Up);
                        self.controller.camera.note_manual_motion();
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowDown) => {
                        self.camera.nudge(CameraNudgeDirection::Down);
                        self.controller.camera.note_manual_motion();
                        true
                    }
                    _ => false,
                }
            }
            _ => self.camera.pointer_input(event),
        }
    }

    pub(super) fn control_input(&mut self, event: &WindowEvent) -> InputResponse {
        let egui_response = self
            .control
            .egui
            .state
            .on_window_event(&self.control.window, event);

        InputResponse {
            consumed: egui_response.consumed,
            repaint: egui_response.repaint,
        }
    }

    pub(super) fn roi_control_input(&mut self, event: &WindowEvent) -> InputResponse {
        let egui_response = self
            .roi_control
            .egui
            .state
            .on_window_event(&self.roi_control.window, event);

        InputResponse {
            consumed: egui_response.consumed,
            repaint: egui_response.repaint,
        }
    }

    pub(super) fn graph_input(&mut self, event: &WindowEvent) -> InputResponse {
        let egui_response = self
            .graph
            .egui
            .state
            .on_window_event(&self.graph.window, event);

        InputResponse {
            consumed: egui_response.consumed,
            repaint: egui_response.repaint,
        }
    }
}
