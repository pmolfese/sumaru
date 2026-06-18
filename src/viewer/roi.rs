//! Drawn-ROI editing: click-to-draw, seed fill, draft point/join management,
//! draft anchor sync, and saving ROIs to layers/files. Extracted from
//! `viewer/mod.rs`; all methods stay on `ViewerState`.

use super::*;

impl ViewerState {
    /// Route a right-click during ROI drawing to add-point or seed-fill.
    pub(super) fn handle_roi_draw_click_at_cursor(&mut self) -> Result<()> {
        let Some(pick) = self.pick_surface_at_cursor() else {
            self.log_status("No surface under the cursor for ROI drawing.");
            return Ok(());
        };
        let target = self.roi_pick_target(pick)?;
        self.controller.interaction.set_pick(Some(pick));
        self.send_afni_crosshair_for_pick(pick)?;

        let fill_pending = self
            .roi_workspace
            .active_draft()
            .is_some_and(|draft| draft.fill_pending);
        if fill_pending {
            self.fill_roi_draft_from_seed(&target)?;
        } else {
            self.add_roi_draft_point(&target)?;
        }

        self.upload_surface_buffers();
        self.update_scene_stats();
        self.control.window.request_redraw();
        if self.controller.panels.roi_controller_open {
            self.roi_control.window.request_redraw();
        }

        Ok(())
    }

    /// Resolve a pick into the ROI draft target (surface/side/local node).
    pub(super) fn roi_pick_target(&self, pick: SurfacePick) -> Result<RoiPickTarget> {
        if let Some((left, right)) = self.active_paired_components() {
            let left_mesh = left
                .mesh
                .as_ref()
                .context("left hemisphere surface is still loading")?;
            let right_mesh = right
                .mesh
                .as_ref()
                .context("right hemisphere surface is still loading")?;
            let left_node_count = left_mesh.vertices.len() as u32;
            if pick.node_index < left_node_count {
                return Ok(RoiPickTarget {
                    mesh: left_mesh.clone(),
                    target: RoiDraftTarget {
                        surface_id: left_mesh.metadata.id.clone(),
                        domain_id: left_mesh.domain.id.clone(),
                        side: SurfaceSide::Left,
                    },
                    local_node: pick.node_index,
                });
            }
            let local_node = pick
                .node_index
                .checked_sub(left_node_count)
                .context("picked node index is outside the left hemisphere")?;
            ensure!(
                (local_node as usize) < right_mesh.vertices.len(),
                "picked node {} is outside the right hemisphere node count {}",
                local_node,
                right_mesh.vertices.len()
            );
            return Ok(RoiPickTarget {
                mesh: right_mesh.clone(),
                target: RoiDraftTarget {
                    surface_id: right_mesh.metadata.id.clone(),
                    domain_id: right_mesh.domain.id.clone(),
                    side: SurfaceSide::Right,
                },
                local_node,
            });
        }

        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before drawing an ROI")?;
        Ok(RoiPickTarget {
            mesh: mesh.clone(),
            target: RoiDraftTarget {
                surface_id: mesh.metadata.id.clone(),
                domain_id: mesh.domain.id.clone(),
                side: mesh.metadata.side.clone(),
            },
            local_node: pick.node_index,
        })
    }

    /// Ensure an active draft exists for the given target, creating one if needed.
    pub(super) fn ensure_roi_draft_target(&self, target: &RoiDraftTarget) -> Result<()> {
        if let Some(existing) = self
            .roi_workspace
            .active_draft()
            .and_then(|draft| draft.target.as_ref())
        {
            ensure!(
                existing == target,
                "ROI draft is tied to the {:?} surface; clear or save it before drawing on {:?}",
                existing.side,
                target.side
            );
        }

        Ok(())
    }

    /// Append a node to the active ROI draft path.
    pub(super) fn add_roi_draft_point(&mut self, target: &RoiPickTarget) -> Result<()> {
        self.ensure_roi_draft_target(&target.target)?;
        let Some(draft) = self.roi_workspace.active_draft() else {
            self.log_status("No editable ROI slot is active.");
            return Ok(());
        };
        let was_joined = draft.is_joined();

        let new_segment = if let Some(previous) = draft.anchor_nodes.last().copied() {
            Some(
                target
                    .mesh
                    .shortest_node_path(previous, target.local_node)?
                    .context("no mesh path connects the selected ROI points")?
                    .nodes,
            )
        } else {
            None
        };
        let draft = self
            .roi_workspace
            .active_draft_mut()
            .context("no editable ROI slot is active")?;
        if draft.target.is_none() {
            draft.target = Some(target.target.clone());
        }
        draft.push_history();
        if was_joined {
            draft.reopen_joined_path_for_append();
        }
        draft.target = Some(target.target.clone());
        if let Some(segment) = new_segment {
            draft.segments.push(segment);
        }
        draft.anchor_nodes.push(target.local_node);
        draft.fill_nodes = None;
        draft.fill_seed_node = None;
        draft.fill_pending = false;
        self.rebuild_roi_layer_from_state()?;
        if was_joined {
            self.log_status(format!(
                "ROI reopened and point added at node {}.",
                target.local_node
            ));
        } else {
            self.log_status(format!("ROI point added at node {}.", target.local_node));
        }

        Ok(())
    }

    /// Close the current ROI draft path into a loop.
    pub(super) fn join_roi_draft(&mut self) -> Result<()> {
        let Some(draft) = self.roi_workspace.active_draft() else {
            self.log_status("No editable ROI slot is active.");
            return Ok(());
        };
        if !draft.can_join() {
            self.log_status("Need at least three ROI points before joining.");
            return Ok(());
        }
        let mesh = self
            .roi_draft_mesh()
            .context("active surface for the ROI draft is not available")?;
        let first = draft.anchor_nodes[0];
        let last = *draft
            .anchor_nodes
            .last()
            .context("ROI draft has no last point")?;
        let closing = mesh
            .shortest_node_path(last, first)?
            .context("no mesh path connects the ROI endpoints")?;

        let draft = self
            .roi_workspace
            .active_draft_mut()
            .context("no editable ROI slot is active")?;
        draft.push_history();
        draft.segments.push(closing.nodes);
        draft.fill_nodes = None;
        draft.fill_seed_node = None;
        draft.fill_pending = false;
        self.rebuild_roi_layer_from_state()?;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status("ROI loop joined. Press Fill, then right-click a seed point.");

        Ok(())
    }

    /// Flood-fill the ROI interior from a seed node within the joined boundary.
    pub(super) fn fill_roi_draft_from_seed(&mut self, target: &RoiPickTarget) -> Result<()> {
        self.ensure_roi_draft_target(&target.target)?;
        let draft = self
            .roi_workspace
            .active_draft()
            .context("no editable ROI slot is active")?;
        ensure!(draft.can_fill(), "join the ROI before filling it");
        let boundary = draft.boundary_nodes();
        let nodes = roi_fill_nodes_from_seed(&target.mesh, &boundary, target.local_node)?;
        let node_count = nodes.len();

        let draft = self
            .roi_workspace
            .active_draft_mut()
            .context("no editable ROI slot is active")?;
        draft.push_history();
        draft.fill_nodes = Some(nodes);
        draft.fill_seed_node = Some(target.local_node);
        draft.fill_pending = false;
        self.rebuild_roi_layer_from_state()?;
        self.log_status(format!(
            "ROI fill defined from node {} on {node_count} nodes.",
            target.local_node
        ));

        Ok(())
    }

    /// Mesh the active draft targets its nodes against, for fill/anchor math.
    pub(super) fn roi_draft_mesh(&self) -> Option<SurfaceMesh> {
        let target = self.roi_workspace.active_draft()?.target.as_ref()?;
        if let Some((left, right)) = self.active_paired_components() {
            return match &target.side {
                SurfaceSide::Left => left.mesh.clone(),
                SurfaceSide::Right => right.mesh.clone(),
                _ => None,
            };
        }

        self.mesh.as_ref().cloned()
    }

    /// Move the current pick to the draft's anchor node.
    pub(super) fn sync_pick_to_roi_draft_anchor(&mut self) {
        let pick = self.roi_draft_anchor_pick();
        self.controller.interaction.set_pick(pick);
    }

    /// The pick representing the active draft's anchor node, if any.
    pub(super) fn roi_draft_anchor_pick(&self) -> Option<SurfacePick> {
        let draft = self.roi_workspace.active_draft()?;
        let target = draft.target.as_ref()?;
        let local_node = draft.anchor_nodes.last().copied()?;
        let mesh = self.mesh.as_ref()?;
        let node_index = self.display_node_for_roi_anchor(target, local_node)?;

        surface_pick_for_mesh_node(mesh, self.overlay.values.as_ref(), node_index)
    }

    /// Map a target-local node to its combined-mesh display node.
    pub(super) fn display_node_for_roi_anchor(
        &self,
        target: &RoiDraftTarget,
        local_node: u32,
    ) -> Option<u32> {
        if let Some((left, right)) = self.active_paired_components() {
            let left_nodes = left.mesh.as_ref()?.vertices.len() as u32;
            let right_nodes = right.mesh.as_ref()?.vertices.len() as u32;
            return match &target.side {
                SurfaceSide::Left => (local_node < left_nodes).then_some(local_node),
                SurfaceSide::Right => (local_node < right_nodes)
                    .then(|| left_nodes.checked_add(local_node))
                    .flatten(),
                _ => None,
            };
        }

        self.mesh
            .as_ref()
            .and_then(|mesh| ((local_node as usize) < mesh.vertices.len()).then_some(local_node))
    }

    /// Save one workspace ROI slot to its `.niml.roi` file.
    pub(super) fn save_roi_slot(&mut self, index: usize) -> Result<()> {
        let Some(roi) = self.roi_workspace.saveable_roi_at(index)? else {
            self.log_status("No ROI is available to save in this slot.");
            return Ok(());
        };

        let default_name = roi_save_default_name(&roi, self.surface_path.as_ref());
        let Some(path) = save_roi_file(
            "Save ROI",
            &default_name,
            self.roi_path.as_ref().or(self.surface_path.as_ref()),
        ) else {
            self.log_status("ROI save cancelled.");
            return Ok(());
        };

        write_niml_roi(&path, std::slice::from_ref(&roi))?;
        self.log_status(format!(
            "Saved ROI {} to {}.",
            roi_display_label(&roi),
            path.display()
        ));

        Ok(())
    }

    /// Save every saveable ROI in the workspace to disk.
    pub(super) fn save_all_rois(&mut self) -> Result<()> {
        let rois = self.roi_workspace.saveable_rois()?;
        if rois.is_empty() {
            self.log_status("No ROI is available to save.");
            return Ok(());
        }

        let default_name = roi_save_all_default_name(self.roi_path.as_ref());
        let Some(path) = save_roi_file(
            "Save All ROIs",
            &default_name,
            self.roi_path.as_ref().or(self.surface_path.as_ref()),
        ) else {
            self.log_status("ROI save cancelled.");
            return Ok(());
        };

        write_niml_roi(&path, &rois)?;
        self.roi_path = Some(path.clone());
        self.controller.surface.current_roi_path = Some(path.clone());
        self.rebuild_roi_layer_from_state()?;
        self.controller.roi.visible = true;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Saved {} ROI object(s) to {}.",
            rois.len(),
            path.display()
        ));

        Ok(())
    }

    /// Build a render-side ROI layer from a path and a set of ROIs.
    pub(super) fn build_roi_layer(&self, path: PathBuf, rois: Vec<Roi>) -> Result<RoiLayer> {
        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before building ROI display")?;
        let ranges = self.roi_component_ranges(mesh);
        let build = roi_appearance_for_mesh(&rois, mesh, &ranges)?;

        Ok(RoiLayer {
            display_name: file_name_display(&path),
            rois,
            appearance: build.appearance,
            node_labels: build.node_labels,
            mapped_nodes: build.mapped_nodes,
            skipped_nodes: build.skipped_nodes,
        })
    }

    /// Stamp the current surface's domain/parent ids onto an ROI.
    pub(super) fn attach_roi_to_current_surface(&self, roi: Roi) -> Roi {
        if let Some(target) = self.roi_draft_target_for_side(&roi.parent_side) {
            roi.with_parent_surface(target.surface_id, target.domain_id, target.side)
        } else {
            roi
        }
    }

    /// Resolve the draft target for a hemisphere side in the active scene.
    pub(super) fn roi_draft_target_for_side(&self, side: &SurfaceSide) -> Option<RoiDraftTarget> {
        if let Some((left, right)) = self.active_paired_components() {
            let component = match side {
                SurfaceSide::Left => left,
                SurfaceSide::Right => right,
                _ => return None,
            };
            let mesh = component.mesh.as_ref()?;
            return Some(RoiDraftTarget {
                surface_id: mesh.metadata.id.clone(),
                domain_id: mesh.domain.id.clone(),
                side: component.side.clone(),
            });
        }

        let mesh = self.mesh.as_ref()?;
        let side = if matches!(side, SurfaceSide::Unknown) {
            mesh.metadata.side.clone()
        } else {
            side.clone()
        };
        Some(RoiDraftTarget {
            surface_id: mesh.metadata.id.clone(),
            domain_id: mesh.domain.id.clone(),
            side,
        })
    }

    /// Rebuild the ROI render layer after workspace edits.
    pub(super) fn rebuild_roi_layer_from_state(&mut self) -> Result<()> {
        let rois = self.roi_workspace.visible_rois()?;

        if rois.is_empty() {
            self.roi_layer = None;
            return Ok(());
        }

        let path = self
            .roi_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("sumaru_draft.niml.roi"));
        self.roi_layer = Some(self.build_roi_layer(path, rois)?);

        Ok(())
    }

    /// Load ROIs from a `.niml.roi` file and attach them to the current surface.
    pub(super) fn load_roi_path(&mut self, path: PathBuf) -> Result<()> {
        self.mesh
            .as_ref()
            .context("load a surface before loading an ROI")?;
        let payloads = read_niml_roi(&path)
            .with_context(|| format!("failed to read ROI {}", path.display()))?;
        ensure!(
            !payloads.is_empty(),
            "ROI file {} did not contain any Node_ROI payloads",
            path.display()
        );
        let rois = payloads
            .into_iter()
            .map(|payload| payload.to_roi())
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("failed to convert ROI {}", path.display()))?;
        let rois = rois
            .into_iter()
            .map(|roi| self.attach_roi_to_current_surface(roi))
            .collect::<Vec<_>>();
        self.roi_path = Some(path.clone());
        self.controller.surface.current_roi_path = Some(path.clone());
        self.roi_workspace = RoiWorkspace::from_rois(rois);
        self.rebuild_roi_layer_from_state()
            .with_context(|| format!("failed to map ROI {}", path.display()))?;
        let layer = self
            .roi_layer
            .as_ref()
            .context("loaded ROI did not produce a display layer")?;
        let roi_count = layer.rois.len();
        let mapped_nodes = layer.mapped_nodes;

        self.controller.roi.visible = true;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Loaded {roi_count} ROI object(s) from {} on {mapped_nodes} nodes.",
            path.display()
        ));

        Ok(())
    }
}
