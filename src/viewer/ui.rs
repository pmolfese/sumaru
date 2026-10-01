//! egui drawing for the viewer's panels: the control window's surface/overlay
//! and ROI/pick sections, the view window's menu bar and transient overlays,
//! and the graph window/dock. Extracted from `viewer/mod.rs`; all drawing stays
//! on `ViewerState` and reaches its helpers (file pickers, `stat_row`,
//! `paint_launch_button`, `ControlUiOutput`) through `use super::*`.

use super::*;

/// Seeds the explicit fade width when the user switches away from fade-to-zero,
/// so the display does not jump. Half the threshold magnitude is a visible but
/// not drastic change from the fade-to-zero ramp.
fn fade_width_seed(threshold_value: f32) -> f64 {
    let magnitude = f64::from(threshold_value).abs();
    if magnitude.is_finite() && magnitude > 0.0 {
        magnitude * 0.5
    } else {
        1.0
    }
}

impl ViewerState {
    pub(super) fn draw_ui(&mut self, ctx: &egui::Context) -> ControlUiOutput {
        let mut actions = Vec::new();
        let panel_height = (self.control.size.height as f32 - 24.0).max(240.0);
        let mut desired_control_size_points = egui::vec2(
            CONTROL_CONTENT_WIDTH_POINTS + 32.0,
            CONTROL_MIN_INNER_HEIGHT as f32,
        );

        #[allow(deprecated)]
        egui::CentralPanel::default().show(ctx, |ui| {
            let scroll_output = egui::ScrollArea::vertical()
                .max_height(panel_height)
                // Shrink horizontally around the controls. Keeping horizontal
                // auto-shrink disabled makes the content width inherit the
                // current window width, which feeds that width back into
                // `fit_control_window` and prevents an oversized window from
                // returning to its natural content width.
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    // The controller is deliberately designed around this
                    // width. Constrain both bounds so full-width frames cannot
                    // inherit an oversized native window and report that size
                    // as their content width.
                    ui.set_width(CONTROL_CONTENT_WIDTH_POINTS);
                    self.draw_surface_dataset_section(ui, &mut actions);
                    self.draw_overlay_workbench(ui, &mut actions);
                    self.draw_scene_objects_section(ui, &mut actions);
                    self.draw_scene_section(ui);
                    self.draw_pick_section(ui);
                });
            desired_control_size_points = egui::vec2(
                // Keep width independent of the current native window. The
                // height remains content-driven because sections can expand
                // and collapse.
                CONTROL_CONTENT_WIDTH_POINTS + 32.0,
                scroll_output.content_size.y + 32.0,
            );
        });

        ControlUiOutput {
            actions,
            desired_control_size_points,
        }
    }

    pub(super) fn draw_view_overlay_ui(&mut self, ctx: &egui::Context) -> Vec<ViewerCommand> {
        let mut actions = Vec::new();

        #[allow(deprecated)]
        egui::TopBottomPanel::top("main_menu_bar")
            .resizable(false)
            .show(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Open Surface...").clicked() {
                            actions.push(ViewerCommand::PickSurface);
                            ui.close();
                        }
                        if ui.button("Open Spec...").clicked() {
                            actions.push(ViewerCommand::PickSpec);
                            ui.close();
                        }
                        if ui.button("Open Surface Volume...").clicked() {
                            actions.push(ViewerCommand::PickSurfaceVolume);
                            ui.close();
                        }
                        if ui.button("Open Tractography...").clicked() {
                            actions.push(ViewerCommand::PickTract);
                            ui.close();
                        }
                        if ui.button("Open Graph Dataset...").clicked() {
                            actions.push(ViewerCommand::PickGraphDataset);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(self.mesh.is_some(), egui::Button::new("Open Overlay..."))
                            .clicked()
                        {
                            actions.push(ViewerCommand::PickOverlay);
                            ui.close();
                        }
                        if ui
                            .add_enabled(self.mesh.is_some(), egui::Button::new("Open ROI..."))
                            .clicked()
                        {
                            actions.push(ViewerCommand::PickRoi);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                self.has_renderable_surface()
                                    || self.volume_view.is_some()
                                    || !self.scene_objects.is_empty(),
                                egui::Button::new("Save View..."),
                            )
                            .clicked()
                        {
                            actions.push(ViewerCommand::SaveScreenshot);
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                self.surface_buffers.is_some(),
                                egui::Button::new("Save Montage..."),
                            )
                            .clicked()
                        {
                            actions.push(ViewerCommand::SaveMontage);
                            ui.close();
                        }
                    });

                    /*
                    ui.menu_button("Edit", |ui| {
                        let has_pick = self.controller.interaction.pick.is_some();
                        if ui
                            .add_enabled(has_pick, egui::Button::new("Copy Vertex Index"))
                            .clicked()
                        {
                            actions.push(ViewerCommand::CopyVertexIndex);
                            ui.close();
                        }
                        if ui
                            .add_enabled(has_pick, egui::Button::new("Copy XYZ (RAS)"))
                            .clicked()
                        {
                            actions.push(ViewerCommand::CopyXyzRas);
                            ui.close();
                        }
                        if ui
                            .add_enabled(has_pick, egui::Button::new("Copy XYZ (RAI, AFNI)"))
                            .clicked()
                        {
                            actions.push(ViewerCommand::CopyXyzRai);
                            ui.close();
                        }
                        ui.separator();
                        let has_surface = self.mesh.is_some();
                        if ui
                            .add_enabled(has_surface, egui::Button::new("Paste Location"))
                            .clicked()
                        {
                            actions.push(ViewerCommand::PasteLocation);
                            ui.close();
                        }
                        if ui
                            .add_enabled(has_surface, egui::Button::new("Go to Location..."))
                            .clicked()
                        {
                            actions.push(ViewerCommand::SetGoToLocationOpen(true));
                            ui.close();
                        }
                    });
                    */

                    ui.menu_button("View", |ui| {
                        ui.label(format!("Mode: {}", self.camera.mode().label()));
                        ui.label(format!(
                            "Surface: {}",
                            self.controller.display.surface_render_style.label()
                        ));
                        ui.label(format!(
                            "Opacity: {}%",
                            self.controller.display.surface_opacity_percent
                        ));
                        ui.label(format!(
                            "Lighting: {}",
                            self.controller.display.lighting_mode.label()
                        ));
                        ui.separator();
                        if ui.button("Reset").clicked() {
                            actions.push(ViewerCommand::ResetCamera);
                            ui.close();
                        }
                        if ui.button("Cycle Camera").clicked() {
                            actions.push(ViewerCommand::ToggleCameraMode);
                            ui.close();
                        }
                        if ui.button("Cycle Surface Style").clicked() {
                            actions.push(ViewerCommand::ToggleSurfaceRenderStyle);
                            ui.close();
                        }
                        if ui.button("Lower Surface Opacity").clicked() {
                            actions.push(ViewerCommand::CycleSurfaceOpacity);
                            ui.close();
                        }
                        if ui.button("Cycle Lighting").clicked() {
                            actions.push(ViewerCommand::ToggleLightingMode);
                            ui.close();
                        }
                        if ui
                            .button(if self.camera.momentum_enabled() {
                                "Momentum Off"
                            } else {
                                "Momentum On"
                            })
                            .clicked()
                        {
                            actions.push(ViewerCommand::ToggleCameraMomentum);
                            ui.close();
                        }
                        if ui
                            .button(self.controller.display.background.next_label())
                            .clicked()
                        {
                            actions.push(ViewerCommand::ToggleBackground);
                            ui.close();
                        }
                        let mut anatomical_shading_visible =
                            self.controller.display.anatomical_shading_visible;
                        if ui
                            .add_enabled_ui(self.mesh.is_some(), |ui| {
                                ui.checkbox(&mut anatomical_shading_visible, "Anatomical Shading")
                            })
                            .inner
                            .changed()
                        {
                            actions.push(ViewerCommand::SetAnatomicalShadingVisible(
                                anatomical_shading_visible,
                            ));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Left").clicked() {
                            actions.push(ViewerCommand::Preset(ViewPreset::Left));
                            ui.close();
                        }
                        if ui.button("Right").clicked() {
                            actions.push(ViewerCommand::Preset(ViewPreset::Right));
                            ui.close();
                        }
                        if ui.button("Top").clicked() {
                            actions.push(ViewerCommand::Preset(ViewPreset::Top));
                            ui.close();
                        }
                        if ui.button("Bottom").clicked() {
                            actions.push(ViewerCommand::Preset(ViewPreset::Bottom));
                            ui.close();
                        }
                        ui.separator();
                        let mut overlay_visible = self.controller.overlay.visible;
                        if ui
                            .add_enabled_ui(self.overlay.is_loaded(), |ui| {
                                ui.checkbox(&mut overlay_visible, "Overlay Visible")
                            })
                            .inner
                            .changed()
                        {
                            actions.push(ViewerCommand::SetOverlayVisible(overlay_visible));
                            ui.close();
                        }
                        let can_layout_hemispheres = self.has_both_scene();
                        if ui
                            .add_enabled(can_layout_hemispheres, egui::Button::new("Close Pair"))
                            .clicked()
                        {
                            actions.push(ViewerCommand::HemisphereLayout(HemisphereLayout::Closed));
                            ui.close();
                        }
                        if ui
                            .add_enabled(can_layout_hemispheres, egui::Button::new("Open Pair"))
                            .clicked()
                        {
                            actions.push(ViewerCommand::HemisphereLayout(HemisphereLayout::Open));
                            ui.close();
                        }
                    });

                    ui.menu_button("Controllers", |ui| {
                        let mut surface_visible = self.controller.panels.surface_controller_visible;
                        if ui
                            .checkbox(
                                &mut surface_visible,
                                "Surface / Overlay Controller    Ctrl+S",
                            )
                            .changed()
                        {
                            actions
                                .push(ViewerCommand::SetSurfaceControllerVisible(surface_visible));
                            ui.close();
                        }
                        let mut roi_open = self.controller.panels.roi_controller_open;
                        if ui
                            .checkbox(&mut roi_open, "ROI Drawing Controller    Ctrl+R")
                            .changed()
                        {
                            actions.push(ViewerCommand::SetRoiControllerOpen(roi_open));
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                self.controller.interaction.pick.is_some(),
                                egui::Button::new("Graph Pick    G"),
                            )
                            .clicked()
                        {
                            actions.push(ViewerCommand::OpenGraphForPick);
                            ui.close();
                        }
                    });

                    ui.menu_button("Settings", |ui| {
                        if ui.button("Preferences...").clicked() {
                            self.preferences_open = true;
                            ui.close();
                        }
                    });

                    if let Some(volume_view) = self.volume_view.as_ref() {
                        let selected_label = volume_view.selected_label();
                        let volume_opacity = volume_view.opacity();
                        ui.menu_button("Volume", |ui| {
                            let mut opacity = volume_opacity;
                            if ui
                                .add(
                                    egui::Slider::new(&mut opacity, 0.0..=1.0)
                                        .text("Slice opacity"),
                                )
                                .changed()
                            {
                                actions.push(ViewerCommand::SetVolumeSliceOpacity(opacity));
                            }
                            ui.separator();
                            if ui.button("Add Axial slice").clicked() {
                                actions.push(ViewerCommand::AddVolumeAxial);
                                ui.close();
                            }
                            if ui.button("Add Coronal slice").clicked() {
                                actions.push(ViewerCommand::AddVolumeCoronal);
                                ui.close();
                            }
                            if ui.button("Add Sagittal slice").clicked() {
                                actions.push(ViewerCommand::AddVolumeSagittal);
                                ui.close();
                            }
                            ui.separator();
                            let remove_label = match selected_label {
                                Some(label) => format!("Remove selected {label} slice"),
                                None => "Remove selected slice".to_string(),
                            };
                            if ui
                                .add_enabled(
                                    selected_label.is_some(),
                                    egui::Button::new(remove_label),
                                )
                                .clicked()
                            {
                                actions.push(ViewerCommand::RemoveSelectedVolumeSlice);
                                ui.close();
                            }
                            ui.separator();
                            ui.label("Right-click a slice to select; left-drag to move.");
                        });
                    }

                    // New / duplicate launch buttons, right-aligned as painted
                    // icons so they read as window controls rather than menus.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_duplicate = self.mesh.is_some();
                        if paint_launch_button(
                            ui,
                            LaunchButtonIcon::Duplicate,
                            can_duplicate,
                            "Duplicate sumaru window (same surface, no overlay)",
                        ) {
                            actions.push(ViewerCommand::LaunchDuplicateInstance);
                        }
                        if paint_launch_button(
                            ui,
                            LaunchButtonIcon::New,
                            true,
                            "New blank sumaru window",
                        ) {
                            actions.push(ViewerCommand::LaunchNewInstance);
                        }
                    });
                });
            });

        if self.graph_matrix_dock_open() {
            self.draw_graph_matrix_dock_ui(ctx, &mut actions);
        } else if self.controller.panels.graph_window_open {
            self.draw_graph_dock_ui(ctx, &mut actions);
        }

        self.draw_go_to_location(ctx, &mut actions);
        self.draw_preferences_window(ctx);
        self.draw_instacorr_window(ctx);
        self.draw_scene_object_labels(ctx);
        self.draw_view_transient_label(ctx);

        actions
    }

    fn draw_graph_matrix_dock_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<ViewerCommand>) {
        let Some(index) = self.active_scene_object else {
            return;
        };
        let current_height = self.graph_dock_height_points;
        #[allow(deprecated)]
        let response = egui::TopBottomPanel::bottom("graph_matrix_dock")
            .resizable(false)
            .exact_height(current_height)
            .show(ctx, |ui| {
                let mut next_height = current_height;
                let full = ui.max_rect();
                let handle_rect = egui::Rect::from_min_max(
                    full.left_top(),
                    egui::pos2(full.right(), full.top() + GRAPH_DOCK_HANDLE_HEIGHT_POINTS),
                );
                let handle = ui.interact(
                    handle_rect,
                    ui.id().with("graph_matrix_dock_resize"),
                    egui::Sense::drag(),
                );
                if handle.hovered() || handle.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
                if handle.dragged() {
                    next_height -= handle.drag_delta().y;
                }
                let stroke = if handle.hovered() || handle.dragged() {
                    ui.visuals().widgets.active.bg_stroke
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke
                };
                ui.painter()
                    .hline(handle_rect.x_range(), handle_rect.center().y, stroke);
                ui.add_space(GRAPH_DOCK_HANDLE_HEIGHT_POINTS);

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Graph Matrix")
                            .strong()
                            .color(accent_color()),
                    );
                    if let Some(object) = self.scene_objects.get(index) {
                        ui.separator();
                        ui.label(egui::RichText::new(&object.name).color(muted_color()));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            actions.push(ViewerCommand::SetGraphMatrixOpen(index, false));
                        }
                        if ui.button("Separate window").clicked() {
                            actions.push(ViewerCommand::SetGraphMatrixPlacement(
                                index,
                                GraphMatrixPlacement::Window,
                            ));
                        }
                    });
                });
                ui.separator();
                self.draw_graph_matrix_contents(ui, index, actions);

                next_height
            });

        let window_height_points = self.view.size.height as f32 / ctx.pixels_per_point().max(0.01);
        let max_height = (window_height_points - GRAPH_DOCK_MIN_SCENE_HEIGHT_POINTS)
            .max(GRAPH_DOCK_MIN_HEIGHT_POINTS);
        let clamped = response
            .inner
            .clamp(GRAPH_DOCK_MIN_HEIGHT_POINTS, max_height);
        if (clamped - current_height).abs() > f32::EPSILON {
            self.graph_dock_height_points = clamped;
            self.view.window.request_redraw();
        }
    }

    pub(super) fn draw_graph_matrix_window_ui(&self, ctx: &egui::Context) -> Vec<ViewerCommand> {
        let mut actions = Vec::new();
        #[allow(deprecated)]
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(index) = self.active_scene_object else {
                ui.centered_and_justified(|ui| ui.label("No graph matrix is open."));
                return;
            };
            ui.horizontal(|ui| {
                if ui.button("Attach to view").clicked() {
                    actions.push(ViewerCommand::SetGraphMatrixPlacement(
                        index,
                        GraphMatrixPlacement::Docked,
                    ));
                }
                if ui.button("Close").clicked() {
                    actions.push(ViewerCommand::SetGraphMatrixOpen(index, false));
                }
            });
            ui.separator();
            self.draw_graph_matrix_contents(ui, index, &mut actions);
        });
        actions
    }

    fn draw_graph_matrix_contents(
        &self,
        ui: &mut egui::Ui,
        index: usize,
        actions: &mut Vec<ViewerCommand>,
    ) {
        let Some(object) = self.scene_objects.get(index) else {
            return;
        };
        let SceneObjectPayload::Graph(data) = &object.payload else {
            return;
        };
        let mut measure = object
            .graph_measure
            .min(data.edge_column_count.saturating_sub(1));
        let measure_label = data
            .edge_labels
            .get(measure)
            .cloned()
            .unwrap_or_else(|| format!("Measure {}", measure + 1));
        ui.horizontal_wrapped(|ui| {
            ui.label("Measure");
            egui::ComboBox::from_id_salt(("graph_matrix_measure", index))
                .selected_text(measure_label)
                .show_ui(ui, |ui| {
                    for measure_index in 0..data.edge_column_count {
                        let label = data
                            .edge_labels
                            .get(measure_index)
                            .cloned()
                            .unwrap_or_else(|| format!("Measure {}", measure_index + 1));
                        ui.selectable_value(&mut measure, measure_index, label);
                    }
                });
            if measure != object.graph_measure {
                actions.push(ViewerCommand::SetGraphMeasure(index, measure));
            }

            ui.separator();
            let mut lower_triangle = object.graph_matrix_lower_triangle;
            ui.selectable_value(&mut lower_triangle, false, "Full");
            ui.selectable_value(&mut lower_triangle, true, "Lower triangle");
            if lower_triangle != object.graph_matrix_lower_triangle {
                actions.push(ViewerCommand::SetGraphMatrixLowerTriangle(
                    index,
                    lower_triangle,
                ));
            }
        });

        let max_abs = data
            .column_range(measure)
            .map(|(min, max)| min.abs().max(max.abs()))
            .unwrap_or(1.0)
            .max(f32::EPSILON);
        ui.horizontal(|ui| {
            let mut threshold = object.graph_threshold.min(max_abs);
            if ui
                .add(egui::Slider::new(&mut threshold, 0.0..=max_abs).text("|value| threshold"))
                .changed()
            {
                actions.push(ViewerCommand::SetGraphThreshold(index, threshold));
            }
            let mut cell_size = object.graph_matrix_cell_points;
            if ui
                .add(egui::Slider::new(&mut cell_size, 12.0..=42.0).text("Cell size"))
                .changed()
            {
                actions.push(ViewerCommand::SetGraphMatrixCellSize(index, cell_size));
            }
        });

        ui.horizontal(|ui| {
            graph_matrix_legend_swatch(ui, -max_abs, max_abs, object.graph_color_mode);
            ui.small(format!("−{max_abs:.3}"));
            graph_matrix_legend_swatch(ui, 0.0, max_abs, object.graph_color_mode);
            ui.small("0");
            graph_matrix_legend_swatch(ui, max_abs, max_abs, object.graph_color_mode);
            ui.small(format!("+{max_abs:.3}"));
            ui.separator();
            ui.weak("Dim cells are below the 3D threshold");
        });
        ui.separator();

        let cell_size = object.graph_matrix_cell_points.clamp(12.0, 42.0);
        let matrix_values = data.matrix_values(measure);
        let node_count = data.nodes.len();
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new(("graph_matrix_grid", index))
                    .spacing(egui::vec2(2.0, 2.0))
                    .show(ui, |ui| {
                        ui.add_sized([68.0, cell_size], egui::Label::new(""));
                        for (column, node) in data.nodes.iter().enumerate() {
                            let label = compact_matrix_label(&node.label, column);
                            ui.add_sized(
                                [cell_size, cell_size],
                                egui::Label::new(egui::RichText::new(label).small()),
                            )
                            .on_hover_text(format!("{} · node {}", node.label, node.index));
                        }
                        ui.end_row();

                        for (row, row_node) in data.nodes.iter().enumerate() {
                            let row_label = compact_matrix_label(&row_node.label, row);
                            ui.add_sized(
                                [68.0, cell_size],
                                egui::Label::new(egui::RichText::new(row_label).small()),
                            )
                            .on_hover_text(format!("{} · node {}", row_node.label, row_node.index));
                            for (column, column_node) in data.nodes.iter().enumerate() {
                                if object.graph_matrix_lower_triangle && column > row {
                                    ui.add_sized([cell_size, cell_size], egui::Label::new(""));
                                    continue;
                                }
                                let value = matrix_values
                                    .get(row * node_count + column)
                                    .copied()
                                    .flatten();
                                let below_threshold = value.is_none_or(|value| {
                                    !value.is_finite()
                                        || value.abs() < object.graph_threshold.max(0.0)
                                });
                                let fill = value
                                    .filter(|value| value.is_finite())
                                    .map(|value| {
                                        let rgba = scene_objects::graph_value_color(
                                            value,
                                            max_abs,
                                            object.graph_color_mode,
                                        );
                                        graph_matrix_color32(rgba, below_threshold)
                                    })
                                    .unwrap_or_else(|| ui.visuals().faint_bg_color);
                                let selected =
                                    object.graph_matrix_selected_cell == Some((row, column));
                                let stroke = if selected {
                                    egui::Stroke::new(2.0_f32, ui.visuals().strong_text_color())
                                } else {
                                    egui::Stroke::NONE
                                };
                                let response = ui
                                    .add_sized(
                                        [cell_size, cell_size],
                                        egui::Button::new("")
                                            .fill(fill)
                                            .stroke(stroke)
                                            .corner_radius(1.0),
                                    )
                                    .on_hover_text(match value {
                                        Some(value) => format!(
                                            "{} → {}\n{} = {value:.6}",
                                            row_node.label,
                                            column_node.label,
                                            data.edge_labels
                                                .get(measure)
                                                .map(String::as_str)
                                                .unwrap_or("Value")
                                        ),
                                        None => format!(
                                            "{} → {}\nNo edge value",
                                            row_node.label, column_node.label
                                        ),
                                    });
                                if response.clicked() && value.is_some() {
                                    actions.push(ViewerCommand::SelectGraphMatrixCell(
                                        index, row, column,
                                    ));
                                }
                            }
                            ui.end_row();
                        }
                    });
            });

        if let Some((row, column)) = object.graph_matrix_selected_cell
            && let (Some(source), Some(target), Some(value)) = (
                data.nodes.get(row),
                data.nodes.get(column),
                data.matrix_value(row, column, measure),
            )
        {
            ui.separator();
            ui.label(
                egui::RichText::new(format!("{} → {}    {value:.6}", source.label, target.label))
                    .strong(),
            );
        }
    }

    fn draw_scene_object_labels(&self, ctx: &egui::Context) {
        if !self.scene_objects.iter().any(|object| {
            object.visible
                && (object.graph_labels_visible
                    || (object.graph_endpoint_labels_visible
                        && object.graph_matrix_selected_cell.is_some()))
                && matches!(object.payload, SceneObjectPayload::Graph(_))
        }) {
            return;
        }
        let scene_size = self.scene_viewport_size();
        let pixels_per_point = ctx.pixels_per_point().max(0.01);
        let width_points = scene_size.width as f32 / pixels_per_point;
        let height_points = scene_size.height as f32 / pixels_per_point;
        let transform = self
            .camera
            .view_projection_matrix(self.scene_viewport_aspect())
            * self.scene_object_model();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("scene_object_node_labels"),
        ));
        for object in &self.scene_objects {
            if !object.visible
                || (!object.graph_labels_visible
                    && (!object.graph_endpoint_labels_visible
                        || object.graph_matrix_selected_cell.is_none()))
            {
                continue;
            }
            let SceneObjectPayload::Graph(data) = &object.payload else {
                continue;
            };
            let selected_endpoints = object.graph_matrix_selected_cell;
            for (index, node) in data.nodes.iter().enumerate() {
                let selected_endpoint = selected_endpoints
                    .is_some_and(|(source, target)| index == source || index == target);
                if !object.graph_labels_visible
                    && !(object.graph_endpoint_labels_visible && selected_endpoint)
                {
                    continue;
                }
                let clip = transform * Vec3::from_array(node.position).extend(1.0);
                if clip.w <= 0.0 {
                    continue;
                }
                let ndc = clip.truncate() / clip.w;
                if ndc.x.abs() > 1.0 || ndc.y.abs() > 1.0 || !(0.0..=1.0).contains(&ndc.z) {
                    continue;
                }
                let position = egui::pos2(
                    (ndc.x + 1.0) * 0.5 * width_points + 7.0,
                    (1.0 - ndc.y) * 0.5 * height_points,
                );
                let color = if selected_endpoint {
                    egui::Color32::from_rgb(255, 225, 20)
                } else {
                    let [red, green, blue, _] =
                        stable_label_color(node.index.max(index as i32 + 1), 255).to_array();
                    egui::Color32::from_rgb(
                        (red * 255.0) as u8,
                        (green * 255.0) as u8,
                        (blue * 255.0) as u8,
                    )
                };
                let text = if node.label.is_empty() {
                    node.index.to_string()
                } else {
                    node.label.clone()
                };
                let font = egui::FontId::proportional(12.0);
                painter.text(
                    position + egui::vec2(1.0, 1.0),
                    egui::Align2::LEFT_CENTER,
                    &text,
                    font.clone(),
                    egui::Color32::BLACK,
                );
                painter.text(position, egui::Align2::LEFT_CENTER, text, font, color);
            }
        }
    }

    pub(super) fn draw_graph_dock_ui(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<ViewerCommand>,
    ) {
        let current_height = self.graph_dock_height_points;
        #[allow(deprecated)]
        let response = egui::TopBottomPanel::bottom("graph_dock")
            .resizable(false)
            .exact_height(current_height)
            .show(ctx, |ui| {
                let mut next_height = current_height;

                // Self-managed resize handle along the dock's top edge. egui's own
                // panel-resize state did not persist here, so the dock height is
                // owned by `graph_dock_height_points` and adjusted directly.
                let full = ui.max_rect();
                let handle_rect = egui::Rect::from_min_max(
                    full.left_top(),
                    egui::pos2(full.right(), full.top() + GRAPH_DOCK_HANDLE_HEIGHT_POINTS),
                );
                let handle = ui.interact(
                    handle_rect,
                    ui.id().with("graph_dock_resize"),
                    egui::Sense::drag(),
                );
                if handle.hovered() || handle.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
                if handle.dragged() {
                    // Dragging up (negative y) grows the dock.
                    next_height -= handle.drag_delta().y;
                }
                let stroke = if handle.hovered() || handle.dragged() {
                    ui.visuals().widgets.active.bg_stroke
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke
                };
                ui.painter()
                    .hline(handle_rect.x_range(), handle_rect.center().y, stroke);
                ui.add_space(GRAPH_DOCK_HANDLE_HEIGHT_POINTS);

                ui.horizontal(|ui| {
                    let (title, subtitle) = if self.timecourse.is_some() {
                        (
                            "Timecourse",
                            "cursor, baseline, response, and activation map",
                        )
                    } else {
                        ("Graph", "picked node overlay values")
                    };
                    ui.label(egui::RichText::new(title).strong().color(accent_color()));
                    ui.separator();
                    ui.label(egui::RichText::new(subtitle).color(muted_color()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            actions.push(ViewerCommand::SetGraphWindowOpen(false));
                        }
                    });
                });
                ui.separator();
                if self.timecourse.is_some() {
                    self.draw_timecourse_contents(ui, actions);
                } else {
                    self.draw_graph_contents(ui);
                }

                next_height
            });

        let window_height_points = self.view.size.height as f32 / ctx.pixels_per_point().max(0.01);
        let max_height = (window_height_points - GRAPH_DOCK_MIN_SCENE_HEIGHT_POINTS)
            .max(GRAPH_DOCK_MIN_HEIGHT_POINTS);
        let clamped = response
            .inner
            .clamp(GRAPH_DOCK_MIN_HEIGHT_POINTS, max_height);
        if (clamped - current_height).abs() > f32::EPSILON {
            self.graph_dock_height_points = clamped;
            self.view.window.request_redraw();
        }
    }

    pub(super) fn draw_view_transient_label(&mut self, ctx: &egui::Context) {
        if let Some((text, remaining)) = self.active_mode_label() {
            // Ensure the label is cleared on time even with no further input.
            ctx.request_repaint_after(remaining);
            egui::Area::new(egui::Id::new("view_transient_label"))
                .anchor(egui::Align2::CENTER_TOP, [0.0, 48.0])
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(180))
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_min_width(128.0);
                            ui.vertical_centered(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(text)
                                            .size(18.0)
                                            .strong()
                                            .color(egui::Color32::WHITE),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Extend),
                                );
                            });
                        });
                });
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    pub(super) fn draw_roi_control_ui(&mut self, ctx: &egui::Context) -> ControlUiOutput {
        let mut actions = Vec::new();
        let panel_height = (self.roi_control.size.height as f32 - 24.0).max(160.0);
        let mut desired_control_size_points = egui::vec2(
            ROI_CONTROL_CONTENT_WIDTH_POINTS + 24.0,
            ROI_CONTROL_MIN_INNER_HEIGHT as f32,
        );

        #[allow(deprecated)]
        egui::CentralPanel::default().show(ctx, |ui| {
            let scroll_output = egui::ScrollArea::vertical()
                .max_height(panel_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_min_width(ROI_CONTROL_CONTENT_WIDTH_POINTS);
                    self.draw_roi_control_contents(ui, &mut actions);
                });
            desired_control_size_points = egui::vec2(
                scroll_output
                    .content_size
                    .x
                    .max(ROI_CONTROL_CONTENT_WIDTH_POINTS)
                    + 32.0,
                scroll_output.content_size.y + 32.0,
            );
        });

        ControlUiOutput {
            actions,
            desired_control_size_points,
        }
    }

    pub(super) fn draw_graph_contents(&self, ui: &mut egui::Ui) {
        ui.set_min_width(GRAPH_MIN_PLOT_WIDTH_POINTS);
        let Some(snapshot) = self.graph_snapshot.as_ref() else {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.35).max(24.0));
                ui.label(
                    egui::RichText::new("Pick a node, then press G")
                        .size(18.0)
                        .color(muted_color()),
                );
            });
            return;
        };

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Node").color(accent_color()));
            ui.monospace(snapshot.node_index.to_string());
            ui.separator();
            ui.label(egui::RichText::new("Surf x,y,z").color(accent_color()));
            ui.monospace(coordinate_label(snapshot.surface_position));
        });
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Surface").color(accent_color()));
            ui.monospace(truncate_middle(&snapshot.surface_label, 44));
            ui.separator();
            ui.label(egui::RichText::new("Overlay").color(accent_color()));
            ui.monospace(truncate_middle(&snapshot.overlay_label, 44));
        });
        ui.add_space(6.0);

        if snapshot.points.is_empty() {
            ui.label(
                egui::RichText::new("No numeric overlay columns are available for this node.")
                    .color(muted_color()),
            );
            return;
        }

        let _ = draw_graph_snapshot(ui, snapshot, self.overlay.data.columns(), None);
    }

    pub(super) fn draw_roi_control_contents(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<ViewerCommand>,
    ) {
        controller_section(ui, "ROI", true, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.mesh.is_some(), egui::Button::new("Open ROI"))
                    .clicked()
                {
                    actions.push(ViewerCommand::PickRoi);
                }
                if ui
                    .add_enabled(
                        self.roi_layer.is_some() || self.roi_workspace.has_saveable_rois(),
                        egui::Button::new("Clear"),
                    )
                    .clicked()
                {
                    actions.push(ViewerCommand::ClearRoi);
                }
                if ui
                    .add_enabled(
                        self.roi_workspace.has_saveable_rois(),
                        egui::Button::new("Save All"),
                    )
                    .on_hover_text("Save every ROI object in one .niml.roi file")
                    .clicked()
                {
                    actions.push(ViewerCommand::SaveAllRois);
                }
                let mut visible = self.controller.roi.visible;
                if ui
                    .add_enabled_ui(self.roi_layer.is_some(), |ui| {
                        ui.checkbox(&mut visible, "Visible")
                    })
                    .inner
                    .changed()
                {
                    actions.push(ViewerCommand::SetRoiVisible(visible));
                }
            });

            ui.add_space(8.0);
            egui::Grid::new("roi_controller_summary_grid")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    stat_row(ui, "ROI", self.roi_display_text());
                    stat_row(ui, "Slots", self.roi_workspace.slots.len().to_string());
                    if let Some(layer) = self.roi_layer.as_ref() {
                        stat_row(ui, "Objects", layer.rois.len().to_string());
                        stat_row(ui, "Nodes", layer.mapped_nodes.to_string());
                    }
                });
        });

        ui.add_space(10.0);
        controller_section(ui, "ROI OBJECTS", true, |ui| {
            let slot_count = self.roi_workspace.slots.len();
            for index in 0..slot_count {
                ui.push_id(("roi_slot", index), |ui| {
                    let is_active = self.roi_workspace.active_index == index;
                    let slot = &mut self.roi_workspace.slots[index];
                    egui::Frame::new()
                        .stroke(egui::Stroke::new(1.0_f32, border_color()))
                        .fill(panel_fill_color())
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let title = format!("ROI {}", index + 1);
                                let title = if is_active {
                                    format!("{title}  editing")
                                } else if slot.editing {
                                    title
                                } else {
                                    format!("{title}  finalized")
                                };
                                ui.label(egui::RichText::new(title).color(accent_color()));
                                ui.add_space(8.0);
                                let mut visible = slot.visible;
                                if ui.checkbox(&mut visible, "Visible").changed() {
                                    actions.push(ViewerCommand::SetRoiSlotVisible(index, visible));
                                }
                            });

                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label("Label");
                                if slot.editing {
                                    ui.text_edit_singleline(&mut slot.draft.label);
                                } else {
                                    ui.monospace(slot.label());
                                }
                                ui.label("Value");
                                if slot.editing {
                                    ui.add(
                                        egui::DragValue::new(&mut slot.draft.integer_label)
                                            .speed(1),
                                    );
                                } else {
                                    ui.monospace(slot.integer_label().to_string());
                                }
                            });

                            ui.add_space(6.0);
                            egui::Grid::new("roi_slot_summary_grid")
                                .num_columns(2)
                                .spacing([10.0, 4.0])
                                .show(ui, |ui| {
                                    stat_row(ui, "State", roi_slot_state_text(slot));
                                    stat_row(ui, "Draft", roi_draft_status_text(&slot.draft));
                                });

                            ui.add_space(8.0);
                            ui.horizontal_wrapped(|ui| {
                                if slot.editing {
                                    let draw_clicked = ui
                                        .add_enabled(
                                            self.mesh.is_some(),
                                            egui::Button::new("Draw")
                                                .selected(is_active && slot.draft.state.draw_enabled),
                                        )
                                        .on_hover_text(
                                            "Right-click the surface to add ROI anchor points",
                                        )
                                        .clicked();
                                    if draw_clicked {
                                        actions.push(ViewerCommand::ToggleRoiDraw(
                                            index,
                                            !slot.draft.state.draw_enabled,
                                        ));
                                    }
                                    if ui
                                        .add_enabled(slot.draft.can_join(), egui::Button::new("Join"))
                                        .on_hover_text(
                                            "Close the ROI by joining the last point back to the first",
                                        )
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::JoinRoiDraft(index));
                                    }
                                    if ui
                                        .add_enabled(slot.draft.can_fill(), egui::Button::new("Fill"))
                                        .on_hover_text(
                                            "Right-click inside or outside the closed ROI to define the fill",
                                        )
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::ArmRoiFill(index));
                                    }
                                    if ui
                                        .add_enabled(slot.draft.can_undo(), egui::Button::new("Undo"))
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::UndoRoiDraft(index));
                                    }
                                    if ui
                                        .add_enabled(slot.draft.can_redo(), egui::Button::new("Redo"))
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::RedoRoiDraft(index));
                                    }
                                    if ui
                                        .add_enabled(!slot.draft.is_empty(), egui::Button::new("Finalize"))
                                        .on_hover_text("Finish this ROI and start a new one")
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::FinalizeRoiSlot(index));
                                    }
                                } else {
                                    if ui.button("Edit").clicked() {
                                        actions.push(ViewerCommand::EditRoiSlot(index));
                                    }
                                    if ui
                                        .add_enabled(slot.has_roi(), egui::Button::new("Delete"))
                                        .on_hover_text("Remove only this ROI object")
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::DeleteRoiSlot(index));
                                    }
                                }

                                if ui
                                    .add_enabled(slot.has_roi(), egui::Button::new("Save"))
                                    .on_hover_text("Save only this ROI object")
                                    .clicked()
                                {
                                    actions.push(ViewerCommand::SaveRoiSlot(index));
                                }
                            });
                        });
                    ui.add_space(8.0);
                });
            }
        });
    }

    pub(super) fn draw_surface_dataset_section(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<ViewerCommand>,
    ) {
        controller_section(ui, "SURFACE / DATASET", true, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Open:");
                if ui
                    .button("Surf")
                    .on_hover_text("Open GIFTI surface")
                    .clicked()
                {
                    actions.push(ViewerCommand::PickSurface);
                }
                if ui
                    .add_enabled(self.mesh.is_some(), egui::Button::new("Olay"))
                    .on_hover_text("Open overlay dataset")
                    .clicked()
                {
                    actions.push(ViewerCommand::PickOverlay);
                }
                if ui
                    .add_enabled(self.mesh.is_some(), egui::Button::new("ROI"))
                    .on_hover_text("Open SUMA ROI")
                    .clicked()
                {
                    actions.push(ViewerCommand::PickRoi);
                }
                if ui.button("Spec").on_hover_text("Open SUMA spec").clicked() {
                    actions.push(ViewerCommand::PickSpec);
                }
                if ui
                    .button("SV")
                    .on_hover_text("Open surface volume")
                    .clicked()
                {
                    actions.push(ViewerCommand::PickSurfaceVolume);
                }
            });

            ui.add_space(8.0);
            if let Some(scene) = self.surface_scene.as_ref() {
                egui::Grid::new("spec_scene_grid")
                    .num_columns(2)
                    .spacing([8.0, 5.0])
                    .show(ui, |ui| {
                        stat_row(ui, "Spec", file_display(Some(&scene.spec_path)));
                        stat_row(
                            ui,
                            "SurfVol",
                            file_display(scene.surface_volume_path.as_ref()),
                        );
                        let active = scene.active_index + 1;
                        let total = scene.surfaces.len();
                        let surface = &scene.surfaces[scene.active_index];
                        let mut selected_index = scene.active_index;
                        let selected_text =
                            scene_surface_display_label(scene.active_index, total, surface);
                        ui.label("Active");
                        let mut changed = false;
                        egui::ComboBox::from_id_salt("spec_active_surface")
                            .selected_text(selected_text)
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                for (index, surface) in scene.surfaces.iter().enumerate() {
                                    changed |= ui
                                        .selectable_value(
                                            &mut selected_index,
                                            index,
                                            scene_surface_display_label(index, total, surface),
                                        )
                                        .changed();
                                }
                            });
                        ui.end_row();
                        if changed && selected_index + 1 != active {
                            actions.push(ViewerCommand::SelectSceneSurface(selected_index));
                        }
                        stat_row(ui, "Overlay", self.overlay_display_text());
                        stat_row(ui, "ROI", self.roi_display_text());
                        if scene.skipped_surfaces > 0 {
                            stat_row(ui, "Skipped files", scene.skipped_surfaces.to_string());
                        }
                        if scene.skipped_states > 0 {
                            stat_row(ui, "Skipped states", scene.skipped_states.to_string());
                        }
                    });
            } else {
                egui::Grid::new("surface_file_grid")
                    .num_columns(2)
                    .spacing([8.0, 5.0])
                    .show(ui, |ui| {
                        stat_row(ui, "Surface", file_display(self.surface_path.as_ref()));
                        stat_row(ui, "Overlay", self.overlay_display_text());
                        stat_row(ui, "ROI", self.roi_display_text());
                    });
            }
        });
    }

    pub(super) fn draw_scene_objects_section(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<ViewerCommand>,
    ) {
        controller_section(ui, "TRACTS / GRAPHS", true, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open tracts...").clicked() {
                    actions.push(ViewerCommand::PickTract);
                }
                if ui.button("Open graph...").clicked() {
                    actions.push(ViewerCommand::PickGraphDataset);
                }
            });

            if self.scene_objects.is_empty() {
                ui.weak("No tractography or graph objects loaded.");
                return;
            }

            ui.add_space(5.0);
            for (index, object) in self.scene_objects.iter().enumerate() {
                let selected = self.active_scene_object == Some(index);
                let mut visible = object.visible;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut visible, "").changed() {
                        actions.push(ViewerCommand::SetSceneObjectVisible(index, visible));
                    }
                    if ui.selectable_label(selected, &object.name).clicked() {
                        actions.push(ViewerCommand::SelectSceneObject(index));
                    }
                });
            }

            let Some(index) = self.active_scene_object else {
                return;
            };
            let Some(object) = self.scene_objects.get(index) else {
                return;
            };
            ui.add_space(6.0);
            ui.small(object.detail());
            let mut width = object.width_points;
            if ui
                .add(egui::Slider::new(&mut width, 0.25..=12.0).text("Ribbon width"))
                .changed()
            {
                actions.push(ViewerCommand::SetSceneObjectWidth(index, width));
            }
            let mut opacity = object.opacity;
            if ui
                .add(egui::Slider::new(&mut opacity, 0.0..=1.0).text("Opacity"))
                .changed()
            {
                actions.push(ViewerCommand::SetSceneObjectOpacity(index, opacity));
            }
            match &object.payload {
                SceneObjectPayload::Tracts(data) => {
                    let mut mode = object.tract_color_mode;
                    egui::ComboBox::from_id_salt("tract_color_mode")
                        .selected_text(mode.label())
                        .show_ui(ui, |ui| {
                            for option in TractColorMode::ALL {
                                ui.selectable_value(&mut mode, option, option.label());
                            }
                        });
                    if mode != object.tract_color_mode {
                        actions.push(ViewerCommand::SetTractColorMode(index, mode));
                    }
                    ui.add_space(3.0);
                    ui.strong("Bundles");
                    egui::ScrollArea::vertical()
                        .id_salt("tract_bundle_list")
                        .max_height(170.0)
                        .show(ui, |ui| {
                            for (bundle_index, bundle) in data.bundles.iter().enumerate() {
                                let Some(appearance) = object.tract_bundles.get(bundle_index)
                                else {
                                    continue;
                                };
                                let label = bundle
                                    .ends
                                    .as_deref()
                                    .map(str::to_owned)
                                    .or_else(|| bundle.tag.map(|tag| format!("Bundle {tag}")))
                                    .unwrap_or_else(|| format!("Bundle {}", bundle_index + 1));
                                let mut visible = appearance.visible;
                                ui.horizontal(|ui| {
                                    if ui.checkbox(&mut visible, "").changed() {
                                        actions.push(ViewerCommand::SetTractBundleVisible(
                                            index,
                                            bundle_index,
                                            visible,
                                        ));
                                    }
                                    ui.label(format!("{label} ({} tracts)", bundle.tracts.len()));
                                });
                                let mut bundle_opacity = appearance.opacity;
                                if ui
                                    .add(
                                        egui::Slider::new(&mut bundle_opacity, 0.0..=1.0)
                                            .text("opacity"),
                                    )
                                    .changed()
                                {
                                    actions.push(ViewerCommand::SetTractBundleOpacity(
                                        index,
                                        bundle_index,
                                        bundle_opacity,
                                    ));
                                }
                            }
                        });
                }
                SceneObjectPayload::Graph(data) => {
                    if let Some(linked) = object.linked_tracts.as_ref() {
                        let mut geometry = object.graph_edge_geometry;
                        egui::ComboBox::from_id_salt("graph_edge_geometry")
                            .selected_text(geometry.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut geometry,
                                    GraphEdgeGeometry::Straight,
                                    GraphEdgeGeometry::Straight.label(),
                                );
                                ui.selectable_value(
                                    &mut geometry,
                                    GraphEdgeGeometry::LinkedBundles,
                                    GraphEdgeGeometry::LinkedBundles.label(),
                                );
                            });
                        if geometry != object.graph_edge_geometry {
                            actions.push(ViewerCommand::SetGraphEdgeGeometry(index, geometry));
                        }
                        ui.small(format!(
                            "Linked network: {} bundles · {} tracts",
                            linked.bundles.len(),
                            linked.tract_count()
                        ));
                    } else if let Some(path) = data.network_file.as_ref() {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            format!("Linked network unavailable: {}", path.display()),
                        );
                    }
                    let mut measure = object
                        .graph_measure
                        .min(data.edge_column_count.saturating_sub(1));
                    let measure_label = data
                        .edge_labels
                        .get(measure)
                        .cloned()
                        .unwrap_or_else(|| format!("Measure {}", measure + 1));
                    egui::ComboBox::from_id_salt("graph_measure")
                        .selected_text(measure_label)
                        .show_ui(ui, |ui| {
                            for measure_index in 0..data.edge_column_count {
                                let label =
                                    data.edge_labels.get(measure_index).cloned().unwrap_or_else(
                                        || format!("Measure {}", measure_index + 1),
                                    );
                                ui.selectable_value(&mut measure, measure_index, label);
                            }
                        });
                    if measure != object.graph_measure {
                        actions.push(ViewerCommand::SetGraphMeasure(index, measure));
                    }
                    let max_abs = data
                        .column_range(measure)
                        .map(|(min, max)| min.abs().max(max.abs()))
                        .unwrap_or(1.0)
                        .max(f32::EPSILON);
                    let mut threshold = object.graph_threshold.min(max_abs);
                    if ui
                        .add(
                            egui::Slider::new(&mut threshold, 0.0..=max_abs)
                                .text("|value| threshold"),
                        )
                        .changed()
                    {
                        actions.push(ViewerCommand::SetGraphThreshold(index, threshold));
                    }
                    let mut color_mode = object.graph_color_mode;
                    egui::ComboBox::from_id_salt("graph_color_mode")
                        .selected_text(color_mode.label())
                        .show_ui(ui, |ui| {
                            for option in GraphColorMode::ALL {
                                ui.selectable_value(&mut color_mode, option, option.label());
                            }
                        });
                    if color_mode != object.graph_color_mode {
                        actions.push(ViewerCommand::SetGraphColorMode(index, color_mode));
                    }
                    let mut node_size = object.graph_node_size_points;
                    if ui
                        .add(egui::Slider::new(&mut node_size, 2.0..=40.0).text("Node size"))
                        .changed()
                    {
                        actions.push(ViewerCommand::SetGraphNodeSize(index, node_size));
                    }
                    let mut labels_visible = object.graph_labels_visible;
                    if ui
                        .checkbox(&mut labels_visible, "Show node labels")
                        .changed()
                    {
                        actions.push(ViewerCommand::SetGraphLabelsVisible(index, labels_visible));
                    }
                    let mut endpoint_labels_visible = object.graph_endpoint_labels_visible;
                    if ui
                        .checkbox(
                            &mut endpoint_labels_visible,
                            "Show selected endpoint labels",
                        )
                        .changed()
                    {
                        actions.push(ViewerCommand::SetGraphEndpointLabelsVisible(
                            index,
                            endpoint_labels_visible,
                        ));
                    }
                    let mut matrix_placement = object.graph_matrix_placement;
                    ui.horizontal(|ui| {
                        ui.label("Matrix");
                        egui::ComboBox::from_id_salt(("graph_matrix_placement", index))
                            .selected_text(matrix_placement.label())
                            .show_ui(ui, |ui| {
                                for placement in GraphMatrixPlacement::ALL {
                                    ui.selectable_value(
                                        &mut matrix_placement,
                                        placement,
                                        placement.label(),
                                    );
                                }
                            });
                    });
                    if matrix_placement != object.graph_matrix_placement {
                        actions.push(ViewerCommand::SetGraphMatrixPlacement(
                            index,
                            matrix_placement,
                        ));
                    }
                    if ui
                        .button(if object.graph_matrix_open {
                            "Close matrix view"
                        } else {
                            "Open matrix view"
                        })
                        .clicked()
                    {
                        actions.push(ViewerCommand::SetGraphMatrixOpen(
                            index,
                            !object.graph_matrix_open,
                        ));
                    }
                }
            }
            if let Some(pick) = self
                .scene_object_pick
                .as_ref()
                .filter(|pick| pick.object_index() == index)
            {
                ui.add_space(5.0);
                ui.label(egui::RichText::new("Selection").strong());
                ui.small(pick.status_text());
            } else {
                ui.weak("Right-click a tract, graph node, or graph edge to inspect it.");
            }
            if ui.button("Remove object").clicked() {
                actions.push(ViewerCommand::RemoveSceneObject(index));
            }
        });
    }

    pub(super) fn draw_overlay_workbench(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<ViewerCommand>,
    ) {
        let overlay_loaded = self.overlay.is_loaded();
        let column_options = self
            .overlay
            .data
            .dataset()
            .map(overlay_column_options)
            .unwrap_or_default();
        // Edit a local copy of the column selection; the egui dropdowns bind to
        // it and we write it back through `set_columns` only if it changed. The
        // copy avoids borrowing into the `Loaded` variant across the closures.
        let mut columns = self.overlay.data.columns();
        let mut columns_changed = false;
        let mut changed = false;

        controller_section(ui, "OVERLAY WORKBENCH", true, |ui| {
            if self.overlay_count() > 0 {
                let active = self.active_overlay_index().unwrap_or(0);
                let mut requested = active;
                ui.horizontal(|ui| {
                    ui.label("Dataset");
                    egui::ComboBox::from_id_salt("active_overlay_selector")
                        .selected_text(
                            self.overlay_label_at(active)
                                .unwrap_or_else(|| "none".to_string()),
                        )
                        .width(OVERLAY_SELECTOR_WIDTH_POINTS)
                        .show_ui(ui, |ui| {
                            for index in 0..self.overlay_count() {
                                let label = self
                                    .overlay_label_at(index)
                                    .unwrap_or_else(|| format!("Overlay {}", index + 1));
                                ui.selectable_value(&mut requested, index, label);
                            }
                        });
                    if ui
                        .add_enabled(self.overlay_count() > 1, egui::Button::new("◀"))
                        .on_hover_text("Previous overlay (Ctrl+PageUp)")
                        .clicked()
                    {
                        actions.push(ViewerCommand::CycleOverlay(-1));
                    }
                    if ui
                        .add_enabled(self.overlay_count() > 1, egui::Button::new("▶"))
                        .on_hover_text("Next overlay (Ctrl+PageDown)")
                        .clicked()
                    {
                        actions.push(ViewerCommand::CycleOverlay(1));
                    }
                    if ui
                        .button("Remove")
                        .on_hover_text("Unload the active overlay")
                        .clicked()
                    {
                        actions.push(ViewerCommand::RemoveActiveOverlay);
                    }
                });
                if requested != active {
                    actions.push(ViewerCommand::SelectOverlay(requested));
                }
                ui.separator();
            }
            if let Some((state, detail)) = self.instacorr_readout() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("InstaCorr").color(accent_color()));
                    ui.label(state);
                    if ui.small_button("Controls").clicked() {
                        self.instacorr_window_open = true;
                        self.view_window().request_redraw();
                    }
                });
                ui.label(egui::RichText::new(detail).small().color(muted_color()));
                ui.separator();
            }
            if !overlay_loaded {
                ui.label(egui::RichText::new("No overlay loaded").color(muted_color()));
                return;
            }

            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(
                        OVERLAY_THRESHOLD_COLUMN_WIDTH_POINTS,
                        OVERLAY_THRESHOLD_RAIL_HEIGHT_POINTS,
                    ),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.label("Thresh");
                        let threshold_range = self.selected_threshold_range();
                        changed |= vertical_threshold_bar(
                            ui,
                            &mut self.overlay.render.appearance,
                            threshold_range,
                        );
                        let threshold_detail_available = self.overlay.data.is_loaded()
                            && self.overlay.render.appearance.threshold.enabled;
                        let button_row_size = egui::vec2(
                            ui.available_width(),
                            ui.spacing().interact_size.y,
                        );
                        ui.allocate_ui_with_layout(
                            button_row_size,
                            egui::Layout::left_to_right(egui::Align::Center)
                                .with_main_align(egui::Align::Center),
                            |ui| {
                            let fade_button = ui.add_enabled(
                                threshold_detail_available,
                                egui::Button::new("A").selected(
                                    self.overlay.render.appearance.transparent_threshold,
                                ),
                            );
                            let fade_button = if threshold_detail_available {
                                fade_button.on_hover_text(
                                    "Fade sub-threshold values by opacity; passing values stay at maximum opacity",
                                )
                            } else if self.afni_live_overlay_active {
                                fade_button.on_hover_text(
                                    "Unavailable for live AFNI RGBA: the packet does not contain per-node threshold scalars",
                                )
                            } else {
                                fade_button.on_hover_text(
                                    "Enable thresholding on a scalar overlay before using transparent thresholding",
                                )
                            };
                            if fade_button.clicked() {
                                self.overlay.render.appearance.transparent_threshold =
                                    !self.overlay.render.appearance.transparent_threshold;
                                changed = true;
                            }

                            let boxed_button = ui.add_enabled(
                                threshold_detail_available,
                                egui::Button::new("B")
                                    .selected(self.overlay.render.appearance.boxed_threshold),
                            );
                            let boxed_button = if threshold_detail_available {
                                boxed_button.on_hover_text(
                                    "Outline the interpolated boundary of values that pass the threshold",
                                )
                            } else if self.afni_live_overlay_active {
                                boxed_button.on_hover_text(
                                    "Unavailable for live AFNI RGBA: a true contour requires per-node threshold scalars",
                                )
                            } else {
                                boxed_button.on_hover_text(
                                    "Enable thresholding on a scalar overlay before drawing its contour",
                                )
                            };
                            if boxed_button.clicked() {
                                self.overlay.render.appearance.boxed_threshold =
                                    !self.overlay.render.appearance.boxed_threshold;
                                changed = true;
                            }

                            let cluster_button = ui.add_enabled(
                                threshold_detail_available,
                                egui::Button::new("C")
                                    .selected(self.overlay.render.appearance.clusterize),
                            );
                            let cluster_button = if threshold_detail_available {
                                cluster_button.on_hover_text(
                                    "Clusterize: keep only connected suprathreshold clusters that meet a minimum size",
                                )
                            } else if self.afni_live_overlay_active {
                                cluster_button.on_hover_text(
                                    "Unavailable for live AFNI RGBA: the packet does not contain per-node threshold scalars",
                                )
                            } else {
                                cluster_button.on_hover_text(
                                    "Enable thresholding on a scalar overlay before clusterizing",
                                )
                            };
                            if cluster_button.clicked() {
                                self.overlay.render.appearance.clusterize =
                                    !self.overlay.render.appearance.clusterize;
                                changed = true;
                            }
                            },
                        );
                        ui.monospace(threshold_value_display(
                            self.overlay.render.appearance.threshold.value,
                        ));
                        ui.label(
                            egui::RichText::new(threshold_p_value_display(
                                self.selected_threshold_p_value(),
                            ))
                            .color(muted_color()),
                        );
                        ui.label(
                            egui::RichText::new(threshold_q_value_display(
                                self.selected_threshold_q_value(),
                            ))
                            .color(muted_color()),
                        );
                    },
                );

                ui.add_space(12.0);
                ui.vertical(|ui| {
                    egui::Grid::new("overlay_mapping_grid")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .show(ui, |ui| {
                            if column_options.is_empty() {
                                stat_row(ui, "I", "scalar column 0");
                                stat_row(ui, "T", "scalar column 0");
                                stat_row(ui, "B", "none");
                            } else {
                                columns_changed |= draw_intensity_column_selector(
                                    ui,
                                    &column_options,
                                    &mut columns.intensity,
                                );
                                columns_changed |= draw_threshold_column_selector(
                                    ui,
                                    &column_options,
                                    &mut columns.threshold,
                                    self.overlay.render.appearance.threshold.value,
                                );
                                columns_changed |= draw_optional_column_selector(
                                    ui,
                                    "B",
                                    "brightness_column",
                                    &column_options,
                                    &mut columns.brightness,
                                );
                            }
                        });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label("Map");
                        egui::ComboBox::from_id_salt("overlay_colormap")
                            .selected_text(self.overlay.render.appearance.colormap.label())
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                for colormap in OverlayColorMap::ALL {
                                    changed |= ui
                                        .selectable_value(
                                            &mut self.overlay.render.appearance.colormap,
                                            colormap,
                                            colormap.label(),
                                        )
                                        .changed();
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Display");
                        egui::ComboBox::from_id_salt("overlay_display_mode")
                            .selected_text(self.overlay.render.appearance.display_mode.label())
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                for mode in [
                                    OverlayDisplayMode::Hidden,
                                    OverlayDisplayMode::Color,
                                    OverlayDisplayMode::Contour,
                                    OverlayDisplayMode::ColorAndContour,
                                ] {
                                    changed |= ui
                                        .selectable_value(
                                            &mut self.overlay.render.appearance.display_mode,
                                            mode,
                                            mode.label(),
                                        )
                                        .changed();
                                }
                            });
                    });
                    changed |= ui
                        .checkbox(
                            &mut self.overlay.render.appearance.show_zero,
                            "Show intensity zero",
                        )
                        .on_hover_text(
                            "Display nodes whose intensity is exactly zero (SUMA shw_0)",
                        )
                        .changed();
                    ui.add_space(8.0);
                    if self
                        .overlay
                        .render
                        .appearance
                        .colormap
                        .uses_continuous_range()
                    {
                        changed |= self.draw_overlay_range_controls(ui);
                    } else {
                        ui.label(
                            egui::RichText::new(
                                "Discrete integer labels use a fixed value-to-color palette.",
                            )
                            .color(muted_color()),
                        );
                    }
                    if let Some(brightness_index) = columns.brightness {
                        ui.add_space(6.0);
                        changed |=
                            self.draw_brightness_mapping_controls(ui, brightness_index);
                    }
                    ui.add_space(6.0);
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut self.overlay.render.appearance.dim, 0.0..=1.5)
                                .text("Dim"),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(
                                &mut self.overlay.render.appearance.opacity,
                                0.0..=1.0,
                            )
                            .text("Opacity"),
                        )
                        .changed();

                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        changed |= ui
                            .checkbox(
                                &mut self.overlay.render.appearance.threshold.absolute,
                                "Abs",
                            )
                            .changed();
                    });
                    if self.overlay.render.appearance.transparent_threshold {
                        ui.horizontal(|ui| {
                            let fade = &mut self.overlay.render.appearance.fade;
                            ui.label("Fade");
                            egui::ComboBox::from_id_salt("overlay_threshold_fade_curve")
                                .selected_text(fade.curve.label())
                                .show_ui(ui, |ui| {
                                    for curve in FadeCurve::ALL {
                                        changed |= ui
                                            .selectable_value(
                                                &mut fade.curve,
                                                curve,
                                                curve.label(),
                                            )
                                            .changed();
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "Shape of the sub-threshold opacity ramp. Quadratic matches AFNI; \
                                     steeper curves pull the near-threshold band down harder, which is \
                                     where the ramp is otherwise nearly flat",
                                );

                            let mut fade_to_zero =
                                matches!(fade.width, FadeWidth::BoundaryMagnitude);
                            if ui
                                .checkbox(&mut fade_to_zero, "To zero")
                                .on_hover_text(
                                    "Fade across the full distance from the threshold to zero (AFNI behavior). \
                                     Uncheck to fade across an explicit width instead, which sharpens the \
                                     distinction between passing and failing values at the cost of context",
                                )
                                .changed()
                            {
                                fade.width = if fade_to_zero {
                                    FadeWidth::BoundaryMagnitude
                                } else {
                                    FadeWidth::Absolute(fade_width_seed(
                                        self.overlay.render.appearance.threshold.value,
                                    ))
                                };
                                changed = true;
                            }

                            if let FadeWidth::Absolute(width) = fade.width {
                                let mut width = width;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut width)
                                            .speed(0.05)
                                            .range(0.0..=f64::INFINITY),
                                    )
                                    .on_hover_text(
                                        "Fade distance in threshold data units. Values this far below \
                                         the threshold are fully transparent",
                                    )
                                    .changed()
                                {
                                    fade.width = FadeWidth::Absolute(width.max(0.0));
                                    changed = true;
                                }
                            }
                        });
                        // One full-width slider per row, matching the Dim and
                        // Opacity rows above. Two sliders share a row cleanly
                        // only at panel widths this window does not guarantee.
                        let fade = &mut self.overlay.render.appearance.fade;
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut fade.max_alpha, 0.0..=1.0)
                                    .fixed_decimals(2)
                                    .text("Step"),
                            )
                            .on_hover_text(
                                "Ceiling on sub-threshold opacity. The fade curve is nearly flat just \
                                 below the threshold, so lowering this is what creates a visible step \
                                 between passing and failing values. AFNI uses 0.87",
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut fade.desaturate, 0.0..=1.0)
                                    .fixed_decimals(2)
                                    .text("Desat"),
                            )
                            .on_hover_text(
                                "How far failing values drain toward grey as they fade. Opacity alone \
                                 is a weak cue; draining color with it separates the two populations \
                                 much more strongly. 0 matches AFNI",
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut fade.darken, 0.0..=1.0)
                                    .fixed_decimals(2)
                                    .text("Dark"),
                            )
                            .on_hover_text(
                                "How far failing values darken as they fade. Fading a bright color \
                                 toward a light surface barely changes its brightness, so this is \
                                 what makes sub-threshold regions recede rather than just thin out. \
                                 0 matches AFNI",
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut fade.boost, 0.0..=1.0)
                                    .fixed_decimals(2)
                                    .text("Boost"),
                            )
                            .on_hover_text(
                                "Saturation added to values that pass, widening the gap from the \
                                 other side. Has no effect on colors already at full saturation, \
                                 where Dark and the B contour do the work instead. 0 matches AFNI",
                            )
                            .changed();
                    }
                    if self.overlay.render.appearance.boxed_threshold
                        || self
                            .overlay
                            .render
                            .appearance
                            .display_mode
                            .shows_contour()
                    {
                        let contour = &mut self.overlay.render.appearance.contour;
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut contour.width_px, 0.5..=8.0)
                                    .fixed_decimals(1)
                                    .suffix(" px")
                                    .text("Border"),
                            )
                            .on_hover_text(
                                "Thickness of the contour line. Widths are in screen pixels, so \
                                 the line stays the same thickness at any zoom and matches in \
                                 saved screenshots",
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut contour.halo_px, 0.0..=6.0)
                                    .fixed_decimals(1)
                                    .suffix(" px")
                                    .text("Halo"),
                            )
                            .on_hover_text(
                                "Width of the casing drawn behind the contour, in the opposing \
                                 shade. A casing is what keeps the line readable over both dark \
                                 sulci and saturated overlay colors. Set to 0 for a plain line",
                            )
                            .changed();
                        ui.horizontal(|ui| {
                            let contour = &mut self.overlay.render.appearance.contour;
                            ui.label("Color");
                            changed |= ui
                                .selectable_value(
                                    &mut contour.color_mode,
                                    ContourColorMode::AutoContrast,
                                    "Auto",
                                )
                                .on_hover_text(
                                    "Pick black or white from the overlay color under the contour, so \
                                     the line never disappears into the colormap",
                                )
                                .changed();
                            changed |= ui
                                .selectable_value(
                                    &mut contour.color_mode,
                                    ContourColorMode::Fixed,
                                    "Fixed",
                                )
                                .on_hover_text("Use one chosen color for the contour")
                                .changed();
                            if contour.color_mode == ContourColorMode::Fixed {
                                changed |= ui
                                    .color_edit_button_rgb(&mut contour.color)
                                    .on_hover_text(
                                        "Contour color. The casing automatically takes the opposing \
                                         shade",
                                    )
                                    .changed();
                            }
                        });
                    }
                    if self.overlay.render.appearance.clusterize {
                        let cluster = &mut self.overlay.render.appearance.cluster;
                        ui.horizontal(|ui| {
                            ui.label("Size by");
                            changed |= ui
                                .selectable_value(
                                    &mut cluster.metric,
                                    ClusterSizeMetric::Area,
                                    "Area",
                                )
                                .on_hover_text(
                                    "Measure clusters by surface area in mm2. Comparable between \
                                     surfaces, unlike node count, which depends on mesh density",
                                )
                                .changed();
                            changed |= ui
                                .selectable_value(
                                    &mut cluster.metric,
                                    ClusterSizeMetric::Nodes,
                                    "Nodes",
                                )
                                .on_hover_text(
                                    "Measure clusters by node count. Use this when matching a \
                                     threshold from a cluster-size simulation reported in nodes",
                                )
                                .changed();
                        });
                        match cluster.metric {
                            ClusterSizeMetric::Area => {
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut cluster.min_area)
                                            .speed(1.0)
                                            .range(0.0..=f32::INFINITY)
                                            .prefix("Min area ")
                                            .suffix(" mm2"),
                                    )
                                    .on_hover_text(
                                        "Clusters smaller than this total surface area are hidden",
                                    )
                                    .changed();
                            }
                            ClusterSizeMetric::Nodes => {
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut cluster.min_nodes)
                                            .speed(1.0)
                                            .range(0..=u32::MAX)
                                            .prefix("Min nodes "),
                                    )
                                    .on_hover_text(
                                        "Clusters with fewer nodes than this are hidden",
                                    )
                                    .changed();
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.label("Connect by");
                            if ui
                                .selectable_label(cluster.radius_mm.is_none(), "Rings")
                                .clicked()
                            {
                                cluster.radius_mm = None;
                                changed = true;
                            }
                            if ui
                                .selectable_label(cluster.radius_mm.is_some(), "mm")
                                .clicked()
                            {
                                cluster.radius_mm = Some(cluster.radius_mm.unwrap_or(2.0));
                                changed = true;
                            }
                        });
                        if let Some(radius_mm) = &mut cluster.radius_mm {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(radius_mm)
                                        .speed(0.1)
                                        .range(0.0..=f32::INFINITY)
                                        .prefix("Radius ")
                                        .suffix(" mm"),
                                )
                                .on_hover_text(
                                    "Maximum accumulated distance along mesh edges between cluster members",
                                )
                                .changed();
                        } else {
                            changed |= ui
                                .add(egui::Slider::new(&mut cluster.rings, 1..=4).text("Rings"))
                                .on_hover_text(
                                    "How many edges apart two suprathreshold nodes may be and still \
                                     join the same cluster. 1 is plain edge adjacency; larger values \
                                     bridge small gaps",
                                )
                                .changed();
                        }
                        ui.horizontal(|ui| {
                            ui.label("Tails");
                            changed |= ui
                                .selectable_value(
                                    &mut cluster.tails,
                                    ClusterTails::Bisided,
                                    "Bisided",
                                )
                                .on_hover_text(
                                    "Cluster each tail separately, so a positive blob touching a \
                                     negative one stays two clusters. Matches 3dClusterize -bisided",
                                )
                                .changed();
                            changed |= ui
                                .selectable_value(
                                    &mut cluster.tails,
                                    ClusterTails::Merged,
                                    "Merged",
                                )
                                .on_hover_text(
                                    "Let opposite-signed regions merge into one cluster where they touch",
                                )
                                .changed();
                        });

                        let summaries = self.cluster_summaries();
                        let cluster_count = summaries.len();
                        let total_area: f32 = summaries.iter().map(|summary| summary.area).sum();
                        ui.label(
                            egui::RichText::new(if cluster_count == 0 {
                                "No clusters survive".to_string()
                            } else {
                                format!("{cluster_count} cluster(s), {total_area:.1} mm2 total")
                            })
                            .color(muted_color()),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    cluster_count > 0,
                                    egui::Button::new("Save .niml.roi"),
                                )
                                .on_hover_text(
                                    "Write each surviving cluster as its own ROI. Sparse: only \
                                     nodes belonging to a cluster are stored. The file is not \
                                     loaded, so open it when you want to see it",
                                )
                                .clicked()
                                && let Err(error) = self.save_clusters_as_rois()
                            {
                                self.log_status(format!("Cluster ROI save failed: {error}"));
                            }
                            if ui
                                .add_enabled(
                                    cluster_count > 0,
                                    egui::Button::new("Save .niml.dset"),
                                )
                                .on_hover_text(
                                    "Write the cluster map as a full-rank dataset: one value per \
                                     node of the surface carrying its cluster rank, zero \
                                     elsewhere. Preserves row-to-node correspondence across \
                                     datasets, unlike the sparse ROI form",
                                )
                                .clicked()
                                && let Err(error) = self.save_clusters_as_dataset()
                            {
                                self.log_status(format!("Cluster dataset save failed: {error}"));
                            }
                        });
                    }
                    if let Some(stat) = self.selected_threshold_stat_label() {
                        ui.label(egui::RichText::new(format!("Stat: {stat}")).color(muted_color()));
                    }
                });
            });
        });

        if columns_changed {
            self.overlay.data.set_columns(columns);
            actions.push(ViewerCommand::RefreshOverlayColumns);
        }
        if changed {
            self.sanitize_overlay_appearance();
            actions.push(ViewerCommand::RefreshOverlayAppearance);
        }
    }

    fn draw_preferences_window(&mut self, ctx: &egui::Context) {
        if !self.preferences_open {
            return;
        }
        let mut open = true;
        let mut threshold_sync = self.preferences.overlay_threshold_sync;
        let mut gifti_dset_auto_qcalc = self.preferences.gifti_dset_auto_qcalc;
        let mut changed = false;
        egui::Window::new("Preferences")
            .open(&mut open)
            .resizable(false)
            .default_width(430.0)
            .show(ctx, |ui| {
                ui.heading("Overlay switching");
                ui.label("When the active overlay changes:");
                ui.add_space(4.0);
                for option in OverlayThresholdSync::ALL {
                    changed |= ui
                        .radio_value(&mut threshold_sync, option, option.label())
                        .on_hover_text(option.description())
                        .changed();
                    if threshold_sync == option {
                        ui.indent(option.label(), |ui| {
                            ui.label(
                                egui::RichText::new(option.description()).color(muted_color()),
                            );
                        });
                    }
                }
                ui.separator();
                ui.heading("GIFTI datasets");
                changed |= ui
                    .checkbox(
                        &mut gifti_dset_auto_qcalc,
                        "Calculate q-values when FDR metadata is missing",
                    )
                    .on_hover_text(
                        "Applies when a statistical .gii.dset is next loaded. Embedded FDR curves are always used; this only reconstructs a missing curve.",
                    )
                    .changed();
                ui.label(
                    egui::RichText::new(
                        "Off by default because reconstruction adds work while loading each statistical column.",
                    )
                    .color(muted_color()),
                );
                ui.separator();
                if let Some(path) = self.preferences_path.as_ref() {
                    ui.label(
                        egui::RichText::new(format!("Saved automatically to {}", path.display()))
                            .color(muted_color()),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("Preferences cannot be persisted: no home directory")
                            .color(egui::Color32::LIGHT_RED),
                    );
                }
                if let Some(status) = self.preferences_status.as_deref() {
                    ui.label(egui::RichText::new(status).color(muted_color()));
                }
            });
        self.preferences_open = open;
        if changed {
            self.preferences.overlay_threshold_sync = threshold_sync;
            self.preferences.gifti_dset_auto_qcalc = gifti_dset_auto_qcalc;
            self.save_preferences();
        }
    }

    pub(super) fn draw_overlay_range_controls(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            changed |= ui
                .checkbox(
                    &mut self.overlay.render.appearance.symmetric_range,
                    "Symmetric",
                )
                .changed();

            if self.overlay.render.appearance.symmetric_range {
                let mut extent = self
                    .overlay
                    .render
                    .appearance
                    .range
                    .min
                    .abs()
                    .max(self.overlay.render.appearance.range.max.abs())
                    .max(0.0001);
                let speed = (extent / 100.0).max(0.001);
                if ui
                    .add(
                        egui::DragValue::new(&mut extent)
                            .speed(speed)
                            .prefix("+/- "),
                    )
                    .changed()
                {
                    let extent = extent.abs().max(0.0001);
                    self.overlay.render.appearance.range = ValueRange {
                        min: -extent,
                        max: extent,
                    };
                    changed = true;
                }
            } else {
                let speed = range_drag_speed(self.overlay.render.appearance.range);
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.overlay.render.appearance.range.min)
                            .speed(speed)
                            .prefix("min "),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.overlay.render.appearance.range.max)
                            .speed(speed)
                            .prefix("max "),
                    )
                    .changed();
            }
        });

        changed
    }

    fn draw_brightness_mapping_controls(
        &mut self,
        ui: &mut egui::Ui,
        brightness_index: usize,
    ) -> bool {
        let automatic_seed = self
            .overlay
            .data
            .dataset()
            .and_then(|dataset| dataset.columns.get(brightness_index))
            .and_then(|column| column.range)
            .map(|range| ValueRange {
                min: range.min as f32,
                max: range.max as f32,
            })
            .unwrap_or(DEFAULT_OVERLAY_RANGE);
        let appearance = &mut self.overlay.render.appearance;
        let mut automatic = appearance.brightness_range.is_none();
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label("B range");
            if ui.checkbox(&mut automatic, "Auto").changed() {
                appearance.brightness_range = (!automatic).then_some(automatic_seed);
                changed = true;
            }
            if let Some(range) = appearance.brightness_range.as_mut() {
                let speed = range_drag_speed(*range);
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut range.min)
                            .speed(speed)
                            .prefix("min "),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut range.max)
                            .speed(speed)
                            .prefix("max "),
                    )
                    .changed();
            }
        });
        ui.horizontal(|ui| {
            ui.label("B scale");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut appearance.brightness_scale[0])
                        .speed(0.01)
                        .prefix("low "),
                )
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut appearance.brightness_scale[1])
                        .speed(0.01)
                        .prefix("high "),
                )
                .changed();
        });

        changed
    }

    fn selected_threshold_stat_label(&self) -> Option<String> {
        let dataset = self.overlay.data.dataset()?;
        let index = self.overlay.data.columns().threshold?;
        dataset.columns.get(index)?.stat.clone()
    }

    pub(super) fn selected_threshold_stat_spec(&self) -> Option<AfniStatSpec> {
        self.selected_threshold_stat_label()
            .as_deref()
            .and_then(AfniStatSpec::parse)
    }

    pub(super) fn selected_threshold_range(&self) -> ValueRange {
        self.overlay
            .data
            .dataset()
            .and_then(|dataset| {
                self.overlay
                    .data
                    .columns()
                    .threshold
                    .and_then(|index| dataset.columns.get(index))
                    .and_then(|column| column.range)
            })
            .map(|range| ValueRange {
                min: range.min as f32,
                max: range.max as f32,
            })
            .or_else(|| self.overlay.data.node_values().map(|overlay| overlay.range))
            .unwrap_or(DEFAULT_OVERLAY_RANGE)
    }

    fn selected_threshold_p_value(&self) -> Option<f64> {
        self.selected_threshold_stat_spec().and_then(|stat| {
            stat.two_sided_p_value(self.overlay.render.appearance.threshold.value as f64)
        })
    }

    fn selected_threshold_q_value(&self) -> Option<f64> {
        let dataset = self.overlay.data.dataset()?;
        let index = self.overlay.data.columns().threshold?;
        let column = dataset.columns.get(index)?;
        column
            .fdr_curve
            .as_ref()?
            .q_value(self.overlay.render.appearance.threshold.value as f64)
    }

    pub(super) fn draw_scene_section(&self, ui: &mut egui::Ui) {
        controller_section(ui, "SCENE", false, |ui| {
            if let Some(stats) = self.scene_stats.as_ref() {
                egui::Grid::new("scene_stats_grid")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .show(ui, |ui| {
                        stat_row(ui, "Nodes", stats.geometry.node_count.to_string());
                        stat_row(ui, "Triangles", stats.geometry.face_count.to_string());
                        stat_row(ui, "Area", format!("{:.4}", stats.geometry.total_area));
                        stat_row(
                            ui,
                            "Normals",
                            normal_direction_label(stats.geometry.normal_direction),
                        );
                        if stats.geometry.boundary_edges > 0 {
                            stat_row(
                                ui,
                                "Boundary edges",
                                stats.geometry.boundary_edges.to_string(),
                            );
                        }
                        if stats.geometry.non_manifold_edges > 0 {
                            stat_row(
                                ui,
                                "Non-manifold",
                                stats.geometry.non_manifold_edges.to_string(),
                            );
                        }
                        if let Some(range) = stats.overlay_range {
                            stat_row(ui, "Overlay range", value_range_label(range));
                        }
                    });
            } else {
                ui.label(egui::RichText::new("No surface loaded").color(muted_color()));
            }
        });
    }

    pub(super) fn draw_pick_section(&self, ui: &mut egui::Ui) {
        controller_section(ui, "PICK", true, |ui| {
            egui::Grid::new("pick_grid")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    stat_row(ui, "Surface file", self.pick_surface_display_text());
                    stat_row(ui, "Overlay file", self.pick_overlay_display_text());
                    if let Some(pick) = self.controller.interaction.pick {
                        stat_row(ui, "Node", pick.node_index.to_string());
                        if let Some(region) = self.pick_region_display_text(pick) {
                            stat_row(ui, "Region", region);
                        }
                        stat_row(ui, "Triangle", pick.face_index.to_string());
                        stat_row(ui, "Surf x,y,z", coordinate_label(pick.surface_position));
                        stat_row(ui, "Overlay Value", picked_overlay_value_label(pick));
                        stat_row(ui, "ROI", self.pick_roi_display_text(pick));
                    }
                });
            if self.controller.interaction.pick.is_none() {
                ui.label(egui::RichText::new("No pick").color(muted_color()));
            }
        });
    }
}

fn compact_matrix_label(label: &str, index: usize) -> String {
    let label = label.trim();
    if label.is_empty() {
        return (index + 1).to_string();
    }
    let mut chars = label.chars();
    let compact: String = chars.by_ref().take(7).collect();
    if chars.next().is_some() {
        format!("{compact}…")
    } else {
        compact
    }
}

fn graph_matrix_color32(rgba: [f32; 4], below_threshold: bool) -> egui::Color32 {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    let color = egui::Color32::from_rgba_unmultiplied(
        channel(rgba[0]),
        channel(rgba[1]),
        channel(rgba[2]),
        channel(rgba[3]),
    );
    if below_threshold {
        color.gamma_multiply(0.28)
    } else {
        color
    }
}

fn graph_matrix_legend_swatch(ui: &mut egui::Ui, value: f32, max_abs: f32, mode: GraphColorMode) {
    let color = graph_matrix_color32(
        scene_objects::graph_value_color(value, max_abs, mode),
        false,
    );
    let (rect, _) = ui.allocate_exact_size(egui::vec2(15.0, 10.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 1.0, color);
}
