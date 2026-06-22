//! egui drawing for the viewer's panels: the control window's surface/overlay
//! and ROI/pick sections, the view window's menu bar and transient overlays,
//! and the graph window/dock. Extracted from `viewer/mod.rs`; all drawing stays
//! on `ViewerState` and reaches its helpers (file pickers, `stat_row`,
//! `paint_launch_button`, `ControlUiOutput`) through `use super::*`.

use super::*;

impl ViewerState {
    pub(super) fn draw_ui(&mut self, ctx: &egui::Context) -> ControlUiOutput {
        let mut actions = Vec::new();
        let panel_height = (self.control.size.height as f32 - 24.0).max(240.0);
        let mut desired_control_size_points = egui::vec2(
            CONTROL_CONTENT_WIDTH_POINTS + 24.0,
            CONTROL_MIN_INNER_HEIGHT as f32,
        );

        #[allow(deprecated)]
        egui::CentralPanel::default().show(ctx, |ui| {
            let scroll_output = egui::ScrollArea::vertical()
                .max_height(panel_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_min_width(CONTROL_CONTENT_WIDTH_POINTS);
                    self.draw_surface_dataset_section(ui, &mut actions);
                    self.draw_overlay_workbench(ui, &mut actions);
                    self.draw_scene_section(ui);
                    self.draw_pick_section(ui);
                });
            desired_control_size_points = egui::vec2(
                scroll_output
                    .content_size
                    .x
                    .max(CONTROL_CONTENT_WIDTH_POINTS)
                    + 32.0,
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
                                self.surface_buffers.is_some(),
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
                        ui.separator();
                        if ui.button("Reset").clicked() {
                            actions.push(ViewerCommand::ResetCamera);
                            ui.close();
                        }
                        if ui.button("Cycle Camera").clicked() {
                            actions.push(ViewerCommand::ToggleCameraMode);
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

                    if let Some(volume_view) = self.volume_view.as_ref() {
                        let selected_label = volume_view.selected_label();
                        ui.menu_button("Volume", |ui| {
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

        if self.controller.panels.graph_window_open {
            self.draw_graph_dock_ui(ctx, &mut actions);
        }

        self.draw_go_to_location(ctx, &mut actions);
        self.draw_view_transient_label(ctx);

        actions
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
                    ui.label(egui::RichText::new("Graph").strong().color(accent_color()));
                    ui.separator();
                    ui.label(
                        egui::RichText::new("picked node overlay values").color(muted_color()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            actions.push(ViewerCommand::SetGraphWindowOpen(false));
                        }
                    });
                });
                ui.separator();
                self.draw_graph_contents(ui);

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

    pub(super) fn draw_graph_ui(&self, ctx: &egui::Context) {
        #[allow(deprecated)]
        egui::CentralPanel::default().show(ctx, |ui| {
            self.draw_graph_contents(ui);
        });
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

        draw_graph_snapshot(ui, snapshot, self.overlay.data.columns());
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
                        .stroke(egui::Stroke::new(1.0, border_color()))
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
                        ui.monospace(threshold_value_display(
                            self.overlay.render.appearance.threshold.value,
                        ));
                        ui.label(
                            egui::RichText::new(threshold_p_value_display(
                                self.selected_threshold_p_value(),
                            ))
                            .color(muted_color()),
                        );
                        if let Some(q_value) = self.selected_threshold_q_value() {
                            ui.label(
                                egui::RichText::new(threshold_q_value_display(q_value))
                                    .color(muted_color()),
                            );
                        }
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
                    ui.add_space(8.0);
                    changed |= self.draw_overlay_range_controls(ui);
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

    fn selected_threshold_stat_label(&self) -> Option<String> {
        let dataset = self.overlay.data.dataset()?;
        let index = self.overlay.data.columns().threshold?;
        dataset.columns.get(index)?.stat.clone()
    }

    fn selected_threshold_stat_spec(&self) -> Option<AfniStatSpec> {
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
                            stat_row(
                                ui,
                                "Overlay range",
                                format!("{:.4} to {:.4}", range.min, range.max),
                            );
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
