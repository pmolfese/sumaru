//! Paired (`both`) hemisphere scenes: pair detection, interactive pair drag,
//! transform/layout adjustment, render-set matrix refresh, and per-hemisphere
//! visibility. Extracted from `viewer/mod.rs`; all methods stay on `ViewerState`.

use super::*;

impl ViewerState {
    /// True when the active scene is a paired left+right hemisphere layout.
    pub(super) fn has_both_scene(&self) -> bool {
        self.surface_scene
            .as_ref()
            .is_some_and(|scene| scene.hemisphere == SpecHemisphere::Both)
    }

    /// The two scene components of the active pair, if both are present.
    pub(super) fn active_paired_components(
        &self,
    ) -> Option<(&SceneSurfaceComponent, &SceneSurfaceComponent)> {
        let scene = self.surface_scene.as_ref()?;
        if scene.hemisphere != SpecHemisphere::Both {
            return None;
        }
        let surface = scene.surfaces.get(scene.active_index)?;
        let left = surface
            .components
            .iter()
            .find(|component| component.side == SurfaceSide::Left)?;
        let right = surface
            .components
            .iter()
            .find(|component| component.side == SurfaceSide::Right)?;

        Some((left, right))
    }

    /// Half-width reference used to space the two hemispheres apart.
    pub(super) fn active_pair_reference_width(&self) -> Option<f32> {
        let (left, right) = self.active_paired_components()?;
        Some(pair_reference_width(
            left.mesh.as_ref()?,
            right.mesh.as_ref()?,
        ))
    }

    /// Per-component node-index ranges within the combined paired mesh.
    pub(super) fn roi_component_ranges(&self, mesh: &SurfaceMesh) -> Vec<RoiComponentRange> {
        if let Some((left, right)) = self.active_paired_components()
            && let (Some(left_mesh), Some(right_mesh)) = (left.mesh.as_ref(), right.mesh.as_ref())
        {
            return vec![
                RoiComponentRange {
                    side: SurfaceSide::Left,
                    node_offset: 0,
                    node_count: left_mesh.vertices.len(),
                    triangle_offset: 0,
                    triangle_count: left_mesh.triangles.len(),
                },
                RoiComponentRange {
                    side: SurfaceSide::Right,
                    node_offset: left_mesh.vertices.len() as u32,
                    node_count: right_mesh.vertices.len(),
                    triangle_offset: left_mesh.triangles.len(),
                    triangle_count: right_mesh.triangles.len(),
                },
            ];
        }

        vec![RoiComponentRange {
            side: mesh.metadata.side.clone(),
            node_offset: 0,
            node_count: mesh.vertices.len(),
            triangle_offset: 0,
            triangle_count: mesh.triangles.len(),
        }]
    }

    /// Start a Control+drag gesture adjusting the pair open-angle/gap.
    pub(super) fn begin_pair_drag(&mut self) {
        self.pair_dragging = true;
        self.pair_drag_last_cursor = self.view_cursor_position;
        self.pair_drag_changed = false;
        if self.surface_render_set.is_none() {
            self.upload_surface_buffers();
        }
        if let Err(error) = self.refresh_active_pair_render_geometry() {
            self.set_error(error);
        }
    }

    /// Update pair open-angle (x) and hemisphere gap (y) from drag delta.
    pub(super) fn update_pair_drag(&mut self, cursor: (f64, f64)) {
        if let Some((last_x, last_y)) = self.pair_drag_last_cursor {
            let dx = (cursor.0 - last_x) as f32;
            let dy = (cursor.1 - last_y) as f32;
            if dx.hypot(dy) as f64 >= PAIR_DRAG_PREVIEW_MIN_DELTA_PIXELS {
                if let Err(error) = self.adjust_pair_transform(dx, dy) {
                    self.set_error(error);
                }
                self.pair_drag_last_cursor = Some(cursor);
            }
        } else {
            self.pair_drag_last_cursor = Some(cursor);
        }
    }

    /// End the pair-drag gesture and commit the resulting transform.
    pub(super) fn finish_pair_drag(&mut self) {
        self.pair_dragging = false;
        self.pair_drag_last_cursor = None;
        if self.pair_drag_changed {
            self.log_status(format!(
                "Hemisphere layout: open {}, angle {:.1} deg, gap {:.1}.",
                pair_open_percent_label(self.controller.display.pair_state.open_angle_degrees),
                self.controller.display.pair_state.open_angle_degrees,
                self.controller.display.pair_state.separation_distance
            ));
        }
        self.pair_drag_changed = false;
        if let Err(error) = self.refresh_active_pair_render_geometry() {
            self.set_error(error);
        }
    }

    /// Nudge the pair open-angle/gap by the given deltas and rebuild geometry.
    pub(super) fn adjust_pair_transform(&mut self, dx: f32, dy: f32) -> Result<()> {
        let Some(pair_width) = self.active_pair_reference_width() else {
            return Ok(());
        };
        let vertical_scale = (pair_width / 700.0).max(0.05);
        self.controller.display.pair_state.open_angle_degrees =
            (self.controller.display.pair_state.open_angle_degrees
                + dx * PAIR_OPEN_DEGREES_PER_PIXEL)
                .clamp(-PAIR_MAX_OPEN_DEGREES, PAIR_MAX_OPEN_DEGREES);
        self.controller.display.pair_state.separation_distance =
            (self.controller.display.pair_state.separation_distance + -dy * vertical_scale)
                .clamp(0.0, pair_width * PAIR_MAX_DRAG_GAP_FACTOR);
        self.controller.display.pair_layout =
            if self.controller.display.pair_state.open_angle_degrees.abs() <= f32::EPSILON
                && self.controller.display.pair_state.separation_distance <= f32::EPSILON
            {
                HemisphereLayout::Closed
            } else {
                HemisphereLayout::Open
            };
        self.pair_drag_changed = true;
        self.show_transient_label(format!(
            "Open {}",
            pair_open_percent_label(self.controller.display.pair_state.open_angle_degrees)
        ));
        self.preview_active_pair_transform()
    }

    /// Apply the in-progress pair transform without persisting it.
    pub(super) fn preview_active_pair_transform(&mut self) -> Result<()> {
        self.refresh_active_pair_render_geometry()
    }

    /// Rebuild paired render geometry after a transform change.
    pub(super) fn refresh_active_pair_render_geometry(&mut self) -> Result<()> {
        if self.surface_render_set.is_none() {
            self.upload_surface_buffers();
        }
        self.refresh_surface_render_set_matrices();
        let camera = self.camera.clone();
        self.update_render_uniforms_for_camera(&camera);

        Ok(())
    }

    /// Refresh per-hemisphere model matrices on the resident render set.
    pub(super) fn refresh_surface_render_set_matrices(&mut self) {
        let matrices = self.active_pair_matrices_for_layout(
            self.controller.display.pair_state,
            self.controller.display.pair_visibility,
        );
        let Some(render_set) = self.surface_render_set.as_mut() else {
            return;
        };

        for instance in &mut render_set.instances {
            if let Some((_, matrix)) = matrices.iter().find(|(side, _)| *side == instance.side) {
                instance.model_matrix = *matrix;
            }
        }
    }

    /// Compute the two hemisphere model matrices for a given layout.
    pub(super) fn active_pair_matrices_for_layout(
        &self,
        layout: HemisphereLayoutState,
        visibility: PairVisibility,
    ) -> Vec<(SurfaceSide, Mat4)> {
        let Some(scene) = self.surface_scene.as_ref() else {
            return Vec::new();
        };
        let Some(surface) = scene.surfaces.get(scene.active_index) else {
            return Vec::new();
        };

        pair_hemisphere_matrices(&surface.components, layout, visibility)
    }

    /// Show/hide one hemisphere of the active pair.
    pub(super) fn toggle_pair_hemisphere_visibility(&mut self, side: SurfaceSide) -> Result<()> {
        if !self.has_both_scene() {
            self.log_status("Load a both-hemisphere spec before toggling hemisphere visibility.");
            return Ok(());
        }
        let Some(next) = self
            .controller
            .display
            .pair_visibility
            .toggled(side.clone())
        else {
            return Ok(());
        };
        if next == self.controller.display.pair_visibility {
            return Ok(());
        }

        self.controller.display.pair_visibility = next;
        self.refresh_active_pair_render_geometry()?;
        self.update_scene_stats();
        self.log_status(format!(
            "{} hemisphere toggled; visible hemispheres: {}.",
            surface_side_label(&side),
            self.controller.display.pair_visibility.label()
        ));

        Ok(())
    }
}
