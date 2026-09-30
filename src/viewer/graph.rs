//! Picked-node graph window and bottom dock: opening from a pick, dock
//! show/hide and view-window growth, dock height, and building the graph
//! snapshot. Extracted from `viewer/mod.rs`; all methods stay on `ViewerState`.

use super::*;

impl ViewerState {
    pub(super) fn graph_matrix_dock_open(&self) -> bool {
        self.active_scene_object
            .and_then(|index| self.scene_objects.get(index))
            .is_some_and(|object| {
                object.graph_matrix_open
                    && object.graph_matrix_placement == GraphMatrixPlacement::Docked
                    && matches!(object.payload, SceneObjectPayload::Graph(_))
            })
    }

    pub(super) fn graph_matrix_window_open(&self) -> bool {
        self.active_scene_object
            .and_then(|index| self.scene_objects.get(index))
            .is_some_and(|object| {
                object.graph_matrix_open
                    && object.graph_matrix_placement == GraphMatrixPlacement::Window
                    && matches!(object.payload, SceneObjectPayload::Graph(_))
            })
    }

    pub(super) fn bottom_dock_open(&self) -> bool {
        self.controller.panels.graph_window_open || self.graph_matrix_dock_open()
    }

    pub(super) fn set_graph_matrix_open(&mut self, index: usize, open: bool) {
        let dock_was_open = self.bottom_dock_open();
        if open {
            for object in &mut self.scene_objects {
                object.graph_matrix_open = false;
            }
        }
        let Some(object) = self.scene_objects.get_mut(index) else {
            return;
        };
        if !matches!(object.payload, SceneObjectPayload::Graph(_)) {
            return;
        }
        object.graph_matrix_open = open;
        if open {
            self.active_scene_object = Some(index);
        }
        self.sync_graph_matrix_presentation(dock_was_open);
    }

    pub(super) fn set_graph_matrix_placement(
        &mut self,
        index: usize,
        placement: GraphMatrixPlacement,
    ) {
        let dock_was_open = self.bottom_dock_open();
        let Some(object) = self.scene_objects.get_mut(index) else {
            return;
        };
        if !matches!(object.payload, SceneObjectPayload::Graph(_)) {
            return;
        }
        object.graph_matrix_placement = placement;
        self.active_scene_object = Some(index);
        self.sync_graph_matrix_presentation(dock_was_open);
    }

    pub(super) fn sync_graph_matrix_presentation(&mut self, dock_was_open: bool) {
        let dock_is_open = self.bottom_dock_open();
        if dock_is_open && !dock_was_open {
            self.grow_view_window_for_graph_dock();
        } else if !dock_is_open && dock_was_open {
            self.shrink_view_window_after_graph_dock();
        }

        let window_open = self.graph_matrix_window_open();
        if window_open {
            let title = self
                .active_scene_object
                .and_then(|index| self.scene_objects.get(index))
                .map(|object| format!("Graph Matrix — {}", object.name))
                .unwrap_or_else(|| "Graph Matrix".to_string());
            self.graph.window.set_title(&title);
        }
        self.graph.window.set_visible(window_open);
        if window_open {
            self.graph.window.request_redraw();
        }
        self.control.window.request_redraw();
        self.view.window.request_redraw();
    }

    /// Open the graph for the current pick, growing the view to fit the dock.
    pub(super) fn open_graph_for_current_pick(&mut self) -> Result<()> {
        let Some(pick) = self.controller.interaction.pick else {
            self.log_status("Pick a surface node before opening a graph.");
            return Ok(());
        };
        let snapshot = self
            .graph_snapshot_for_pick(pick)
            .context("no plottable overlay values are available for the picked node")?;
        self.graph_snapshot = Some(snapshot);
        self.set_graph_window_open(true);
        self.log_status(format!("Opened graph for node {}.", pick.node_index));
        Ok(())
    }

    /// Toggle the graph dock open/closed and resize the view window to match.
    pub(super) fn set_graph_window_open(&mut self, open: bool) {
        let dock_was_open = self.bottom_dock_open();
        self.controller.panels.graph_window_open = open;
        let dock_is_open = self.bottom_dock_open();
        if dock_is_open && !dock_was_open {
            self.grow_view_window_for_graph_dock();
        } else if !dock_is_open && dock_was_open {
            self.shrink_view_window_after_graph_dock();
        }
        if open {
            self.view.window.request_redraw();
        }
        self.view.window.request_redraw();
    }

    /// Enlarge the view window to make room for the opening dock.
    pub(super) fn grow_view_window_for_graph_dock(&mut self) {
        if self.graph_dock_pre_open_size.is_some() {
            return;
        }

        let growth = self.graph_dock_height_pixels();
        if growth == 0 {
            return;
        }

        let desired_size = PhysicalSize::new(
            self.view.size.width.max(1),
            self.view.size.height.saturating_add(growth).max(1),
        );
        self.graph_dock_pre_open_size = Some(self.view.size);
        if let Some(actual_size) = self.view.window.request_inner_size(desired_size) {
            self.resize_view(actual_size);
        }
    }

    /// Restore the view window size after the dock closes.
    pub(super) fn shrink_view_window_after_graph_dock(&mut self) {
        let Some(previous_size) = self.graph_dock_pre_open_size.take() else {
            return;
        };
        let desired_height = previous_size.height.max(1);
        let desired_size = PhysicalSize::new(self.view.size.width.max(1), desired_height);
        if let Some(actual_size) = self.view.window.request_inner_size(desired_size) {
            self.resize_view(actual_size);
        }
    }

    /// Current dock height in physical pixels from the owned point height.
    pub(super) fn graph_dock_height_pixels(&self) -> u32 {
        (self.graph_dock_height_points * self.view.egui.ctx.pixels_per_point())
            .round()
            .max(1.0) as u32
    }

    /// Build the per-column value series plotted for a given node pick.
    pub(super) fn graph_snapshot_for_pick(&self, pick: SurfacePick) -> Option<GraphSnapshot> {
        let mut points = Vec::new();
        if let Some(timecourse) = self.timecourse.as_ref()
            && let Some(row) = dataset_row_for_node(&timecourse.source_dataset, pick.node_index)
        {
            for sample_index in 0..timecourse.time_columns.len() {
                let Some(value) = timecourse.sample_value(row, sample_index) else {
                    continue;
                };
                points.push(GraphPoint {
                    column_index: sample_index,
                    label: format!("{:.1} ms", timecourse.sample_time(sample_index) * 1000.0),
                    value,
                });
            }
        } else if let Some(dataset) = self.overlay.data.dataset()
            && let Some(row) = dataset_row_for_node(dataset, pick.node_index)
        {
            for (column_index, column) in dataset.columns.iter().enumerate() {
                let Some(value) = numeric_column_value_as_f32(column, row) else {
                    continue;
                };
                points.push(GraphPoint {
                    column_index,
                    label: graph_column_label(column_index, column),
                    value,
                });
            }
        }

        if points.is_empty() {
            if let Some(value) = pick.overlay_value {
                points.push(GraphPoint {
                    column_index: self.overlay.data.columns().intensity,
                    label: "I".to_string(),
                    value,
                });
            }
            if let Some(value) = pick.threshold_value {
                points.push(GraphPoint {
                    column_index: self.overlay.data.columns().threshold.unwrap_or(1),
                    label: "T".to_string(),
                    value,
                });
            }
        }

        if points.is_empty() {
            return None;
        }

        let y_range = graph_y_range_for_points(&points)?;

        Some(GraphSnapshot {
            node_index: pick.node_index,
            surface_position: pick.surface_position,
            surface_label: self.pick_surface_display_text(),
            overlay_label: self.pick_overlay_display_text(),
            points,
            y_range,
        })
    }
}

fn graph_y_range_for_points(points: &[GraphPoint]) -> Option<ValueRange> {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for point in points {
        min = min.min(point.value);
        max = max.max(point.value);
    }
    if !min.is_finite() || !max.is_finite() {
        return None;
    }

    Some(padded_graph_y_range(min, max))
}

fn padded_graph_y_range(min: f32, max: f32) -> ValueRange {
    let span = max - min;
    let magnitude = min.abs().max(max.abs());
    let padding = if span > 0.0 {
        graph_y_padding(span, magnitude)
    } else if magnitude > 0.0 {
        graph_y_padding(magnitude, magnitude)
    } else {
        1.0
    };
    let padded = ValueRange {
        min: min - padding,
        max: max + padding,
    };
    if padded.min < padded.max {
        padded
    } else if magnitude > 0.0 {
        let fallback_padding = graph_y_padding(magnitude, magnitude).max(magnitude);
        ValueRange {
            min: min - fallback_padding,
            max: max + fallback_padding,
        }
    } else {
        ValueRange {
            min: -1.0,
            max: 1.0,
        }
    }
}

fn graph_y_padding(base: f32, magnitude: f32) -> f32 {
    let padding = (base * 0.08).max(magnitude * f32::EPSILON);
    if padding > 0.0 {
        padding
    } else {
        base.max(magnitude)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_point(value: f32) -> GraphPoint {
        GraphPoint {
            column_index: 0,
            label: "value".to_string(),
            value,
        }
    }

    #[test]
    fn graph_y_range_tracks_small_nonzero_values() {
        let range =
            graph_y_range_for_points(&[graph_point(1.0e-10), graph_point(1.2e-10)]).unwrap();

        assert!(range.min < 1.0e-10);
        assert!(range.max > 1.2e-10);
        assert!(range.min.abs() < 1.0e-8);
        assert!(range.max.abs() < 1.0e-8);
    }

    #[test]
    fn graph_y_range_for_constant_small_value_stays_local() {
        let range = graph_y_range_for_points(&[graph_point(-2.0e-12)]).unwrap();

        assert!(range.min < -2.0e-12);
        assert!(range.max > -2.0e-12);
        assert!(range.min.abs() < 1.0e-10);
        assert!(range.max.abs() < 1.0e-10);
    }

    #[test]
    fn graph_y_range_for_all_zero_values_uses_visible_fallback() {
        let range = graph_y_range_for_points(&[graph_point(0.0), graph_point(0.0)]).unwrap();

        assert_eq!(
            range,
            ValueRange {
                min: -1.0,
                max: 1.0
            }
        );
    }
}
