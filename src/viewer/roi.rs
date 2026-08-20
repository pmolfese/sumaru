//! Drawn-ROI editing: click-to-draw, seed fill, draft point/join management,
//! draft anchor sync, and saving ROIs to layers/files. Extracted from
//! `viewer/mod.rs`; all methods stay on `ViewerState`.

use std::collections::BTreeMap;

use crate::io::{NimlDatasetPayload, NimlNumericMatrix, NimlValueType, write_niml_ascii};

use super::*;

/// A loaded set of ROIs rendered together as one overlay layer: the source
/// name, the ROIs themselves, their drawing appearance, and the per-node label
/// strings plus a tally of how many nodes mapped onto the surface versus were
/// skipped (e.g. out-of-range node indices).
#[derive(Debug, Clone)]
pub(super) struct RoiLayer {
    pub(super) display_name: String,
    pub(super) rois: Vec<Roi>,
    pub(super) appearance: RoiAppearance,
    pub(super) node_labels: HashMap<u32, Vec<String>>,
    pub(super) mapped_nodes: usize,
    pub(super) skipped_nodes: usize,
}

/// The editable ROI bench: one slot per ROI plus a trailing blank slot, which
/// slot is active, and the next integer label to hand out when a new slot is
/// created.
#[derive(Debug, Clone)]
pub(super) struct RoiWorkspace {
    pub(super) slots: Vec<RoiSlot>,
    pub(super) active_index: usize,
    pub(super) next_integer_label: i32,
}

/// One ROI in the workspace. While `editing`, the live `draft` is the source of
/// truth; once finalized it is frozen into `finalized_roi`. `visible` toggles
/// whether the slot is drawn.
#[derive(Debug, Clone)]
pub(super) struct RoiSlot {
    pub(super) draft: RoiDraft,
    pub(super) finalized_roi: Option<Roi>,
    pub(super) editing: bool,
    pub(super) visible: bool,
}

/// The undoable editing state of an ROI draft: the target surface, the drawn
/// anchor nodes and stroke segments, and the fill state. Factored out of
/// `RoiDraft` so it is also the unit stored on the undo/redo stacks — capturing
/// or restoring a draft is a single clone of this struct rather than a
/// hand-maintained field-by-field copy. (Was the separate `RoiDraftSnapshot`.)
#[derive(Debug, Clone, Default)]
pub(super) struct RoiDraftState {
    /// Surface/domain/side this draft is bound to, once a first point is placed.
    pub(super) target: Option<RoiDraftTarget>,
    /// Ordered anchor (click) nodes of the path.
    pub(super) anchor_nodes: Vec<u32>,
    /// Stroke segments connecting consecutive anchors (last may close the loop).
    pub(super) segments: Vec<Vec<u32>>,
    /// Filled-interior nodes, once a closed path is filled.
    pub(super) fill_nodes: Option<Vec<u32>>,
    /// Seed node a fill was started from.
    pub(super) fill_seed_node: Option<u32>,
    /// A fill is requested and awaiting the next click.
    pub(super) fill_pending: bool,
    /// Free-draw mode is active (each click extends the path).
    pub(super) draw_enabled: bool,
}

/// An in-progress drawn ROI: its label/color identity, the editable `state`,
/// and the undo/redo history of prior states.
#[derive(Debug, Clone)]
pub(super) struct RoiDraft {
    pub(super) label: String,
    pub(super) integer_label: i32,
    /// The current editable state (target, path, fill).
    pub(super) state: RoiDraftState,
    /// Past states for undo, most recent last.
    pub(super) history: Vec<RoiDraftState>,
    /// States popped by undo, available to redo.
    pub(super) redo_history: Vec<RoiDraftState>,
}

/// The surface a draft is bound to, identified well enough to re-locate it
/// across reloads: the surface id, its domain id, and which hemisphere side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RoiDraftTarget {
    pub(super) surface_id: SurfaceId,
    pub(super) domain_id: SurfaceDomainId,
    pub(super) side: SurfaceSide,
}

/// The result of resolving a click into ROI space: the picked surface mesh, the
/// draft target it belongs to, and the node index in that surface's local
/// numbering.
#[derive(Debug, Clone)]
pub(super) struct RoiPickTarget {
    pub(super) mesh: SurfaceMesh,
    pub(super) target: RoiDraftTarget,
    pub(super) local_node: u32,
}

impl RoiLayer {
    pub(super) fn labels_for_node(&self, node: u32) -> &[String] {
        self.node_labels
            .get(&node)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

impl Default for RoiWorkspace {
    fn default() -> Self {
        let mut workspace = Self {
            slots: Vec::new(),
            active_index: 0,
            next_integer_label: 1,
        };
        workspace.push_blank_slot();
        workspace
    }
}

impl RoiWorkspace {
    pub(super) fn from_rois(rois: Vec<Roi>) -> Self {
        let next_integer_label = rois
            .iter()
            .map(|roi| roi.integer_label)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let mut workspace = Self {
            slots: rois.into_iter().map(RoiSlot::from_roi).collect(),
            active_index: 0,
            next_integer_label,
        };
        let active_index = workspace.push_blank_slot();
        workspace.active_index = active_index;
        workspace
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn has_saveable_rois(&self) -> bool {
        self.slots.iter().any(RoiSlot::has_roi)
    }

    pub(super) fn active_draft(&self) -> Option<&RoiDraft> {
        self.slots
            .get(self.active_index)
            .filter(|slot| slot.editing)
            .map(|slot| &slot.draft)
    }

    pub(super) fn active_draft_mut(&mut self) -> Option<&mut RoiDraft> {
        self.slots
            .get_mut(self.active_index)
            .filter(|slot| slot.editing)
            .map(|slot| &mut slot.draft)
    }

    pub(super) fn set_active(&mut self, index: usize) -> bool {
        if index >= self.slots.len() {
            return false;
        }
        self.active_index = index;
        true
    }

    pub(super) fn saveable_rois(&self) -> Result<Vec<Roi>> {
        self.slots
            .iter()
            .filter_map(RoiSlot::current_roi)
            .collect::<Result<Vec<_>>>()
    }

    pub(super) fn saveable_roi_at(&self, index: usize) -> Result<Option<Roi>> {
        self.slots
            .get(index)
            .and_then(RoiSlot::current_roi)
            .transpose()
    }

    pub(super) fn visible_rois(&self) -> Result<Vec<Roi>> {
        self.slots
            .iter()
            .filter(|slot| slot.visible)
            .filter_map(RoiSlot::current_roi)
            .collect::<Result<Vec<_>>>()
    }

    pub(super) fn finalize_slot(&mut self, index: usize) -> Result<bool> {
        let Some(slot) = self.slots.get_mut(index) else {
            return Ok(false);
        };
        let Some(roi) = slot.draft.to_roi()? else {
            return Ok(false);
        };
        slot.finalized_roi = Some(roi);
        slot.editing = false;
        slot.draft.state.draw_enabled = false;
        slot.draft.state.fill_pending = false;
        slot.visible = true;
        let next_index = self.push_blank_slot();
        self.active_index = next_index;

        Ok(true)
    }

    pub(super) fn edit_slot(&mut self, index: usize) -> Result<bool> {
        let Some(slot) = self.slots.get_mut(index) else {
            return Ok(false);
        };
        if slot.editing {
            self.active_index = index;
            return Ok(true);
        }
        let Some(roi) = slot.finalized_roi.as_ref() else {
            return Ok(false);
        };
        let Some(draft) = RoiDraft::from_roi(roi) else {
            return Ok(false);
        };
        slot.draft = draft;
        slot.finalized_roi = None;
        slot.editing = true;
        self.active_index = index;

        Ok(true)
    }

    pub(super) fn delete_slot(&mut self, index: usize) -> bool {
        if index >= self.slots.len() {
            return false;
        }

        self.slots.remove(index);
        if self.slots.is_empty() {
            self.active_index = self.push_blank_slot();
            return true;
        }

        if self.active_index > index {
            self.active_index -= 1;
        } else if self.active_index >= self.slots.len() {
            self.active_index = self.slots.len() - 1;
        }

        if !self.slots.iter().any(|slot| slot.editing) {
            self.active_index = self.push_blank_slot();
        }

        true
    }

    pub(super) fn push_blank_slot(&mut self) -> usize {
        let value = self.next_integer_label;
        self.next_integer_label = self.next_integer_label.saturating_add(1);
        self.slots
            .push(RoiSlot::blank(format!("roi_{value}"), value));
        self.slots.len() - 1
    }
}

impl RoiSlot {
    pub(super) fn blank(label: String, integer_label: i32) -> Self {
        Self {
            draft: RoiDraft::new(label, integer_label),
            finalized_roi: None,
            editing: true,
            visible: true,
        }
    }

    pub(super) fn from_roi(roi: Roi) -> Self {
        let draft = RoiDraft::from_roi(&roi)
            .unwrap_or_else(|| RoiDraft::new(roi.label.clone(), roi.integer_label));
        Self {
            draft,
            finalized_roi: Some(roi),
            editing: false,
            visible: true,
        }
    }

    pub(super) fn has_roi(&self) -> bool {
        self.finalized_roi.is_some() || !self.draft.is_empty()
    }

    pub(super) fn current_roi(&self) -> Option<Result<Roi>> {
        if self.editing {
            return self.draft.to_roi().transpose();
        }
        self.finalized_roi.clone().map(Ok)
    }

    pub(super) fn label(&self) -> &str {
        if self.editing {
            &self.draft.label
        } else {
            self.finalized_roi
                .as_ref()
                .map(|roi| roi.label.as_str())
                .unwrap_or(self.draft.label.as_str())
        }
    }

    pub(super) fn integer_label(&self) -> i32 {
        if self.editing {
            self.draft.integer_label
        } else {
            self.finalized_roi
                .as_ref()
                .map(|roi| roi.integer_label)
                .unwrap_or(self.draft.integer_label)
        }
    }
}

impl Default for RoiDraft {
    fn default() -> Self {
        Self::new("roi_1", 1)
    }
}

impl RoiDraft {
    pub(super) fn new(label: impl Into<String>, integer_label: i32) -> Self {
        Self {
            label: label.into(),
            integer_label,
            state: RoiDraftState::default(),
            history: Vec::new(),
            redo_history: Vec::new(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.state.anchor_nodes.is_empty()
            && self.state.segments.is_empty()
            && self.state.fill_nodes.is_none()
    }

    pub(super) fn is_joined(&self) -> bool {
        self.state.segments.last().is_some_and(|segment| {
            segment.len() >= 2
                && self.state.anchor_nodes.len() >= 3
                && segment.last().copied() == self.state.anchor_nodes.first().copied()
        })
    }

    pub(super) fn can_join(&self) -> bool {
        self.state.anchor_nodes.len() >= 3 && !self.is_joined()
    }

    pub(super) fn can_fill(&self) -> bool {
        self.is_joined()
    }

    pub(super) fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub(super) fn can_redo(&self) -> bool {
        !self.redo_history.is_empty()
    }

    pub(super) fn reopen_joined_path_for_append(&mut self) {
        if self.is_joined() {
            self.state.segments.pop();
        }
        self.state.fill_nodes = None;
        self.state.fill_seed_node = None;
        self.state.fill_pending = false;
    }

    pub(super) fn from_roi(roi: &Roi) -> Option<Self> {
        let mut draft = Self::new(roi.label.clone(), roi.integer_label);
        draft.state.target = match (&roi.parent_surface_id, &roi.parent_domain_id) {
            (Some(surface_id), Some(domain_id)) => Some(RoiDraftTarget {
                surface_id: surface_id.clone(),
                domain_id: domain_id.clone(),
                side: roi.parent_side.clone(),
            }),
            _ => None,
        };

        for datum in &roi.data {
            if datum.action == RoiBrushAction::FillArea {
                if !datum.node_path.is_empty() {
                    draft.state.fill_nodes = Some(datum.node_path.clone());
                }
                continue;
            }

            match datum.kind {
                RoiElementKind::NodeSegment | RoiElementKind::EdgeGroup => {
                    if datum.node_path.is_empty() {
                        continue;
                    }
                    if draft.state.anchor_nodes.is_empty()
                        && let Some(first) = datum.node_path.first().copied()
                    {
                        draft.state.anchor_nodes.push(first);
                    }
                    if datum.action != RoiBrushAction::JoinEnds
                        && let Some(last) = datum.node_path.last().copied()
                        && draft.state.anchor_nodes.last().copied() != Some(last)
                    {
                        draft.state.anchor_nodes.push(last);
                    }
                    draft.state.segments.push(datum.node_path.clone());
                }
                RoiElementKind::NodeGroup if !datum.node_path.is_empty() => {
                    if draft.state.segments.is_empty() && draft.state.anchor_nodes.is_empty() {
                        draft.state.anchor_nodes = datum.node_path.clone();
                    } else {
                        draft.state.fill_nodes = Some(datum.node_path.clone());
                    }
                }
                _ => {}
            }
        }

        (!draft.is_empty()).then_some(draft)
    }

    /// Capture the current editable state for the undo/redo stacks.
    pub(super) fn snapshot(&self) -> RoiDraftState {
        self.state.clone()
    }

    /// Replace the editable state with a previously captured snapshot.
    pub(super) fn restore(&mut self, snapshot: RoiDraftState) {
        self.state = snapshot;
    }

    pub(super) fn push_history(&mut self) {
        self.history.push(self.snapshot());
        self.redo_history.clear();
    }

    pub(super) fn undo(&mut self) -> bool {
        let Some(snapshot) = self.history.pop() else {
            return false;
        };
        self.redo_history.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    pub(super) fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo_history.pop() else {
            return false;
        };
        self.history.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    pub(super) fn boundary_nodes(&self) -> Vec<u32> {
        let mut nodes = Vec::new();
        for segment in &self.state.segments {
            if segment.is_empty() {
                continue;
            }
            let start = usize::from(nodes.last().copied() == segment.first().copied());
            nodes.extend(segment.iter().skip(start).copied());
        }
        nodes
    }

    pub(super) fn to_roi(&self) -> Result<Option<Roi>> {
        if self.is_empty() {
            return Ok(None);
        }

        let Some(target) = self.state.target.clone() else {
            return Ok(None);
        };
        let mut data = Vec::new();
        if self.state.segments.is_empty() && !self.state.anchor_nodes.is_empty() {
            data.push(RoiDatum::node_group(self.state.anchor_nodes.clone())?);
        }
        for (index, segment) in self.state.segments.iter().enumerate() {
            let action = if index == self.state.segments.len().saturating_sub(1) && self.is_joined()
            {
                RoiBrushAction::JoinEnds
            } else {
                RoiBrushAction::AppendStroke
            };
            data.push(RoiDatum::node_segment(segment.clone(), action)?);
        }
        if let Some(nodes) = &self.state.fill_nodes {
            data.push(RoiDatum::new(
                RoiElementKind::NodeGroup,
                RoiBrushAction::FillArea,
                nodes.clone(),
                Vec::new(),
            )?);
        }

        let drawing_type = if self.state.fill_nodes.is_some() {
            RoiDrawingType::FilledArea
        } else if self.is_joined() {
            RoiDrawingType::ClosedPath
        } else {
            RoiDrawingType::OpenPath
        };
        let roi = Roi::new(self.label.clone(), self.integer_label)?
            .with_parent_surface(target.surface_id, target.domain_id, target.side)
            .with_source(RoiSource::Drawn, None)?
            .with_style(
                roi_fill_color_for_label(self.integer_label),
                roi_edge_color_for_label(self.integer_label),
                2,
            )?
            .with_color_by_label(true)
            .with_draw_status(if self.is_joined() {
                RoiDrawStatus::Finished
            } else {
                RoiDrawStatus::InCreation
            })
            .with_drawing_type(drawing_type)
            .with_data(data)?;

        Ok(Some(roi))
    }
}

/// The product of turning a set of ROIs into a drawable appearance: the colors
/// to apply, the per-node label strings, and how many nodes were successfully
/// mapped versus skipped during the build.
#[derive(Debug, Clone)]
pub(super) struct RoiAppearanceBuild {
    pub(super) appearance: RoiAppearance,
    pub(super) node_labels: HashMap<u32, Vec<String>>,
    pub(super) mapped_nodes: usize,
    pub(super) skipped_nodes: usize,
}

/// Where one hemisphere's data lives inside a composite paired surface: which
/// side it is, and its node and triangle offsets/counts within the combined
/// buffers. Used to scatter ROI results back to the right slice of a pair.
#[derive(Debug, Clone)]
pub(super) struct RoiComponentRange {
    pub(super) side: SurfaceSide,
    pub(super) node_offset: u32,
    pub(super) node_count: usize,
    pub(super) triangle_offset: usize,
    pub(super) triangle_count: usize,
}

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
            .is_some_and(|draft| draft.state.fill_pending);
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
            .and_then(|draft| draft.state.target.as_ref())
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

        let new_segment = if let Some(previous) = draft.state.anchor_nodes.last().copied() {
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
        if draft.state.target.is_none() {
            draft.state.target = Some(target.target.clone());
        }
        draft.push_history();
        if was_joined {
            draft.reopen_joined_path_for_append();
        }
        draft.state.target = Some(target.target.clone());
        if let Some(segment) = new_segment {
            draft.state.segments.push(segment);
        }
        draft.state.anchor_nodes.push(target.local_node);
        draft.state.fill_nodes = None;
        draft.state.fill_seed_node = None;
        draft.state.fill_pending = false;
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
        let first = draft.state.anchor_nodes[0];
        let last = *draft
            .state
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
        draft.state.segments.push(closing.nodes);
        draft.state.fill_nodes = None;
        draft.state.fill_seed_node = None;
        draft.state.fill_pending = false;
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
        draft.state.fill_nodes = Some(nodes);
        draft.state.fill_seed_node = Some(target.local_node);
        draft.state.fill_pending = false;
        self.rebuild_roi_layer_from_state()?;
        self.log_status(format!(
            "ROI fill defined from node {} on {node_count} nodes.",
            target.local_node
        ));

        Ok(())
    }

    /// Mesh the active draft targets its nodes against, for fill/anchor math.
    pub(super) fn roi_draft_mesh(&self) -> Option<SurfaceMesh> {
        let target = self.roi_workspace.active_draft()?.state.target.as_ref()?;
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
        let target = draft.state.target.as_ref()?;
        let local_node = draft.state.anchor_nodes.last().copied()?;
        let mesh = self.mesh.as_ref()?;
        let node_index = self.display_node_for_roi_anchor(target, local_node)?;

        surface_pick_for_mesh_node(mesh, self.overlay.data.node_values(), node_index)
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
    /// Write the cluster map as a full-rank `.niml.dset`.
    ///
    /// This is the `SurfClust -out_roidset -out_fulllist` form: one value per
    /// node of the surface, carrying the node's cluster rank, and zero
    /// everywhere else. A `.niml.roi` instead stores only the nodes that belong
    /// to a cluster, so it is smaller but does not preserve row-to-node
    /// correspondence across datasets.
    pub(super) fn save_clusters_as_dataset(&mut self) -> Result<()> {
        let summaries = self.cluster_summaries().to_vec();
        if summaries.is_empty() {
            self.log_status("No surviving clusters to save.");
            return Ok(());
        }
        let labels = self
            .cluster_labels
            .clone()
            .context("no cluster labels are available")?;

        let Some(path) = save_niml_dset_file(
            "Save Cluster Map",
            "sumaru_clusters.niml.dset",
            self.overlay
                .source
                .path
                .as_ref()
                .or(self.surface_path.as_ref()),
        ) else {
            self.log_status("Cluster map save cancelled.");
            return Ok(());
        };

        let payload = cluster_dataset_payload(
            &labels,
            path.file_name()
                .map(|name| name.to_string_lossy().to_string()),
            self.surfclust_command_for_clusters(true),
        )?;
        write_niml_ascii(&path, &[payload.to_element()?])?;

        println!(
            "# Equivalent SurfClust command for these clusters:\n{}",
            self.surfclust_command_for_clusters(true)
        );
        self.log_status(format!(
            "Saved {} cluster(s) as a full-rank dataset to {}.",
            summaries.len(),
            path.display()
        ));

        Ok(())
    }

    /// The `SurfClust` command line that reproduces the current clusters.
    ///
    /// `full_list` selects between the full-rank dataset form and the sparse
    /// ROI form, matching which file the user is writing.
    pub(super) fn surfclust_command_for_clusters(&self, full_list: bool) -> String {
        let (threshold, _) = threshold_and_mask_from_appearance(self.overlay.render.appearance);
        let columns = self.overlay.data.columns();
        surfclust_command(
            self.surface_path.as_ref().and_then(|path| path.to_str()),
            self.overlay
                .source
                .path
                .as_ref()
                .and_then(|path| path.to_str()),
            columns.intensity,
            columns.threshold,
            threshold,
            self.overlay.render.appearance.cluster,
            full_list,
        )
    }

    /// Write every surviving cluster as its own ROI in a single `.niml.roi`.
    ///
    /// This is the sparse form: only nodes belonging to a cluster are stored.
    /// The file is written but deliberately not loaded — displaying it is the
    /// user's decision, not a side effect of exporting.
    pub(super) fn save_clusters_as_rois(&mut self) -> Result<()> {
        let summaries = self.cluster_summaries().to_vec();
        if summaries.is_empty() {
            self.log_status("No surviving clusters to convert.");
            return Ok(());
        }
        let Some(labels) = self.cluster_labels.clone() else {
            self.log_status("No cluster labels are available.");
            return Ok(());
        };

        // Clusters never span components, so each one maps onto exactly one
        // hemisphere. Translate combined node indices back to that component's
        // local indices, which is what an ROI on that surface expects.
        let components: Vec<(usize, usize, SurfaceSide)> = self
            .cluster_topology_cache
            .as_ref()
            .map(|cache| {
                cache
                    .components
                    .iter()
                    .map(|component| {
                        (
                            component.node_offset,
                            component.node_offset + component.neighbors.len(),
                            component.side.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut rois = Vec::new();
        for summary in &summaries {
            let nodes = labels
                .iter()
                .enumerate()
                .filter(|(_, value)| **value == summary.label)
                .map(|(node, _)| node as u32)
                .collect::<Vec<_>>();
            if nodes.is_empty() {
                continue;
            }

            let (offset, side) = components
                .iter()
                .find(|(start, end, _)| {
                    let first = nodes[0] as usize;
                    first >= *start && first < *end
                })
                .map(|(start, _, side)| (*start as u32, side.clone()))
                .unwrap_or((0, SurfaceSide::Unknown));
            let local_nodes = nodes.iter().map(|node| node - offset).collect::<Vec<_>>();

            // Cluster rank is the ROI's integer label, so the ROI numbering
            // matches the cluster table and the .niml.dset values.
            let label = format!(
                "Cluster {} ({:.1} mm2, {} nodes)",
                summary.label, summary.area, summary.node_count
            );
            let mut roi = Roi::from_nodes(label, summary.label as i32, local_nodes)?;
            roi.parent_side = side;
            rois.push(self.attach_roi_to_current_surface(roi));
        }

        if rois.is_empty() {
            self.log_status("No surviving clusters to save.");
            return Ok(());
        }

        let Some(path) = save_roi_file(
            "Save Cluster ROIs",
            "sumaru_clusters.niml.roi",
            self.roi_path.as_ref().or(self.surface_path.as_ref()),
        ) else {
            self.log_status("Cluster ROI save cancelled.");
            return Ok(());
        };

        write_niml_roi(&path, &rois)?;

        // AFNI's convention: show the command that reproduces this result, so a
        // clustering found by clicking can be rerun and scripted. ROIs are the
        // sparse form, so no -out_fulllist here.
        let command = self.surfclust_command_for_clusters(false);
        println!("# Equivalent SurfClust command for these clusters:\n{command}");

        // Written, not loaded. Displaying it is the user's call, so the ROI
        // workspace, the current ROI path, and visibility are all left alone.
        self.log_status(format!(
            "Saved {} cluster ROI(s) to {}.",
            rois.len(),
            path.display()
        ));

        Ok(())
    }

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

/// Builds the full-rank cluster dataset payload.
///
/// Every node of the surface gets a row carrying its cluster rank, zero for
/// nodes in no surviving cluster. This is what preserves row-to-node
/// correspondence across datasets, which the sparse ROI form cannot.
fn cluster_dataset_payload(
    labels: &[u32],
    filename: Option<String>,
    history: String,
) -> Result<NimlDatasetPayload> {
    let values: Vec<f64> = labels.iter().map(|label| f64::from(*label)).collect();
    let node_indices: Vec<u32> = (0..labels.len() as u32).collect();
    let matrix = NimlNumericMatrix::new(vec![NimlValueType::Int32], labels.len(), values)?;

    Ok(NimlDatasetPayload {
        dset_type: "Node_Bucket".to_string(),
        self_idcode: None,
        filename,
        label: Some("sumaru clusters".to_string()),
        sparse_data: Some(matrix),
        node_indices: Some(node_indices),
        column_ranges: Vec::new(),
        column_labels: vec!["Cluster".to_string()],
        column_types: vec!["Generic_Int".to_string()],
        column_stats: Vec::new(),
        fdr_curves: BTreeMap::new(),
        history: Some(history),
    })
}

#[cfg(test)]
mod tests {
    use super::cluster_dataset_payload;
    use crate::io::{read_niml_dset_str, serialize_niml_ascii};

    #[test]
    fn cluster_dataset_is_full_rank_and_round_trips() {
        // Two clusters over six nodes, with gaps. The gaps are what the ROI
        // form would omit and the dataset form must keep as explicit zeros.
        let labels = vec![1u32, 1, 0, 2, 0, 0];
        let payload = cluster_dataset_payload(
            &labels,
            Some("clusters.niml.dset".to_string()),
            "SurfClust -i s.gii".to_string(),
        )
        .unwrap();

        let element = payload.to_element().unwrap();
        let text = serialize_niml_ascii(&[element]);
        let parsed = read_niml_dset_str(&text).unwrap();

        // Every node is present, not just the clustered ones.
        let data = parsed.sparse_data.expect("dataset has SPARSE_DATA");
        assert_eq!(data.rows, labels.len());
        assert_eq!(
            data.values,
            labels
                .iter()
                .map(|label| f64::from(*label))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            parsed.node_indices.expect("dataset has INDEX_LIST"),
            (0..labels.len() as u32).collect::<Vec<_>>()
        );
        assert_eq!(parsed.column_labels, vec!["Cluster".to_string()]);

        // The reproducing command travels with the file.
        assert!(parsed.history.unwrap_or_default().contains("SurfClust"));
    }
}
