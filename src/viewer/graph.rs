//! Picked-node graph window and bottom dock: opening from a pick, dock
//! show/hide and view-window growth, dock height, and building the graph
//! snapshot. Extracted from `viewer/mod.rs`; all methods stay on `ViewerState`.

use super::*;

impl ViewerState {
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
        let was_open = self.controller.panels.graph_window_open;
        if open && !was_open {
            self.grow_view_window_for_graph_dock();
        } else if !open && was_open {
            self.shrink_view_window_after_graph_dock();
        }
        self.controller.panels.graph_window_open = open;
        self.graph.window.set_visible(false);
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
        if let Some(dataset) = self.overlay.data.canonical_dataset.as_ref()
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
                    column_index: self.overlay.data.columns.intensity,
                    label: "I".to_string(),
                    value,
                });
            }
            if let Some(value) = pick.threshold_value {
                points.push(GraphPoint {
                    column_index: self.overlay.data.columns.threshold.unwrap_or(1),
                    label: "T".to_string(),
                    value,
                });
            }
        }

        if points.is_empty() {
            return None;
        }

        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for point in &points {
            min = min.min(point.value);
            max = max.max(point.value);
        }
        if !min.is_finite() || !max.is_finite() {
            return None;
        }
        if (max - min).abs() <= f32::EPSILON {
            min -= 1.0;
            max += 1.0;
        } else {
            let padding = (max - min) * 0.08;
            min -= padding;
            max += padding;
        }

        Some(GraphSnapshot {
            node_index: pick.node_index,
            surface_position: pick.surface_position,
            surface_label: self.pick_surface_display_text(),
            overlay_label: self.pick_overlay_display_text(),
            points,
            y_range: ValueRange { min, max },
        })
    }
}
