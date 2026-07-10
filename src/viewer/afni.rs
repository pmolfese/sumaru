//! AFNI/SUMA NIML talk: connection lifecycle, surface registration, incoming
//! message routing, and crosshair/overlay sync. Extracted from `viewer/mod.rs`
//! as part of the topical-submodule split; all methods stay on `ViewerState`.

use super::*;

impl ViewerState {
    /// Connect if disconnected, disconnect if connected.
    pub(super) fn toggle_afni_talk(&mut self) -> Result<()> {
        if self.afni_connection.is_some() {
            self.disconnect_afni_talk();
            return Ok(());
        }

        self.connect_afni_talk()
    }

    /// Open the AFNI/SUMA NIML socket and register current surfaces.
    pub(super) fn connect_afni_talk(&mut self) -> Result<()> {
        if self.afni_connection.is_some() {
            self.log_status("AFNI/SUMA NIML talk is already connected.");
            return Ok(());
        }

        let config = self.afni_options.port_config.clone();
        let event_proxy = self.event_proxy.clone();
        let connection = AfniConnection::connect(
            &config,
            self.verbose,
            self.afni_recorder.clone(),
            move || {
                let _ = event_proxy.send_event(ViewerEvent::AfniMessagesReady);
            },
        )
        .with_context(|| {
            format!(
                "failed to connect to AFNI/SUMA NIML talk at {}:{}",
                config.host, config.port
            )
        })?;
        self.afni_connection = Some(connection);
        self.afni_session = AfniNimlSession::new();
        self.log_status(format!(
            "Connected AFNI/SUMA NIML talk at {}:{}.",
            config.host, config.port
        ));
        self.send_afni_surfaces(false)
    }

    /// Tear down the AFNI connection and clear talk state.
    pub(super) fn disconnect_afni_talk(&mut self) {
        if let Some(mut connection) = self.afni_connection.take() {
            connection.disconnect();
            self.afni_session = AfniNimlSession::new();
            self.pending_afni_surface_registrations.clear();
            self.pending_afni_redraw_after_registrations = false;
            self.pending_afni_elements.clear();
            self.pending_cell_color_upload = None;
            self.deferred_afni_rgba_overlays.clear();
            self.log_status("Disconnected AFNI/SUMA NIML talk.");
        }
    }

    /// Re-send all surface geometry to AFNI (Control+T), ignoring caches.
    pub(super) fn force_resend_afni_surfaces(&mut self) -> Result<()> {
        if self.afni_connection.is_none() {
            self.connect_afni_talk()?;
            return Ok(());
        }

        self.afni_session = AfniNimlSession::new();
        self.send_afni_surfaces(true)
    }

    /// Send pending surface registrations; `force` bypasses the sent-cache.
    pub(super) fn send_afni_surfaces(&mut self, force: bool) -> Result<()> {
        if self.afni_connection.is_none() {
            return Ok(());
        }
        if self.mesh.is_none() {
            self.log_status("AFNI/SUMA NIML talk connected; no surface is loaded yet.");
            return Ok(());
        }

        if force {
            self.afni_session = AfniNimlSession::new();
            self.pending_afni_surface_registrations.clear();
            self.pending_afni_redraw_after_registrations = false;
        }

        self.ensure_scene_surfaces_loaded_for_afni()?;
        let exports = self.afni_surface_exports()?;
        let mut queued_count = 0usize;
        for (mesh, info) in exports {
            if !self.afni_session.reserve_surface_registration(&info) {
                continue;
            }
            self.pending_afni_surface_registrations
                .push_back(PendingAfniSurfaceRegistration {
                    mesh: Arc::new(mesh),
                    info,
                    next_part: 0,
                });
            queued_count += 1;
        }

        if queued_count > 0 {
            self.log_status(format!(
                "Queued {queued_count} surface registration{} for AFNI/SUMA.",
                if queued_count == 1 { "" } else { "s" }
            ));
            self.pending_afni_redraw_after_registrations = true;
            self.process_pending_afni_surface_registrations()?;
        } else if force {
            self.log_status("No new surface geometry needed to be sent to AFNI/SUMA.");
        }

        Ok(())
    }

    /// Send a `SUMA_crosshair_xyz` to prompt AFNI to resend its colorization for
    /// the active surfaces. Targets, in order: the current selection, the last
    /// crosshair we sent, AFNI's most recently reported crosshair, and finally
    /// the node nearest the brain's center to trigger an initial draw when none
    /// of those exist yet.
    pub(super) fn send_afni_redraw_crosshair(&mut self) {
        if self.afni_connection.is_none() {
            return;
        }
        let Some(mesh) = self.mesh.as_ref() else {
            return;
        };
        if mesh.vertices.is_empty() {
            return;
        }
        let node = self
            .controller
            .interaction
            .pick
            .map(|pick| pick.node_index)
            .or(self.sent_crosshair_node)
            .or(self.afni_crosshair_node)
            .or_else(|| node_nearest_bounds_center(mesh))
            .unwrap_or(0);
        let Some(pick) = surface_pick_for_mesh_node(mesh, self.overlay.data.node_values(), node)
        else {
            return;
        };
        if let Err(error) = self.send_afni_crosshair_for_pick(pick) {
            self.set_error(error);
        }
    }

    /// Collect the per-surface ixyz/normals/ijk registration payloads to send.
    pub(super) fn afni_surface_exports(&self) -> Result<Vec<(SurfaceMesh, AfniSurfaceInfo)>> {
        if let Some(scene) = self.surface_scene.as_ref() {
            let mut exports = Vec::new();
            for surface in &scene.surfaces {
                for component in &surface.components {
                    let Some(mesh) = component.mesh.as_ref() else {
                        continue;
                    };
                    if !afni_component_is_sendable(component, Some(mesh)) {
                        continue;
                    }
                    let mut info = AfniSurfaceInfo::from_mesh(mesh);
                    decorate_afni_surface_info(&mut info, Some(scene), Some(component));
                    exports.push((mesh.clone(), info));
                }
            }
            if !exports.is_empty() {
                return Ok(exports);
            }
        }

        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before connecting to AFNI/SUMA NIML talk")?;
        let mut info = AfniSurfaceInfo::from_mesh(mesh);
        decorate_afni_surface_info(&mut info, self.surface_scene.as_ref(), None);
        decorate_afni_surface_volume_info(
            &mut info,
            self.surface_volume_path.as_ref(),
            self.surface_volume_idcode.as_deref(),
        );
        Ok(vec![(mesh.clone(), info)])
    }

    /// Pull and dispatch any NIML messages waiting on the connection.
    pub(super) fn drain_afni_events(&mut self) -> bool {
        let drain_start = Instant::now();
        let mut changed = false;
        let mut event_count = 0usize;
        let queued_before = self.pending_afni_elements.len();
        while event_count < AFNI_EVENTS_PER_DRAIN {
            let event = self
                .afni_connection
                .as_ref()
                .and_then(AfniConnection::try_recv);
            let Some(event) = event else {
                break;
            };
            match event {
                AfniConnectionEvent::Elements(elements) => {
                    if self.verbose {
                        let names = elements
                            .iter()
                            .map(|element| element.name.as_str())
                            .collect::<Vec<_>>()
                            .join(",");
                        self.log_status(format!(
                            "AFNI drain queued {} element{} [{}]; pending before={}.",
                            elements.len(),
                            if elements.len() == 1 { "" } else { "s" },
                            names,
                            self.pending_afni_elements.len()
                        ));
                    }
                    self.pending_afni_elements.extend(elements);
                }
                AfniConnectionEvent::Error(message) => {
                    self.log_status(format!("AFNI/SUMA NIML talk error: {message}"));
                }
                AfniConnectionEvent::Disconnected => {
                    self.afni_connection = None;
                    self.afni_session = AfniNimlSession::new();
                    self.pending_afni_surface_registrations.clear();
                    self.pending_afni_redraw_after_registrations = false;
                    self.pending_afni_elements.clear();
                    self.pending_cell_color_upload = None;
                    self.deferred_afni_rgba_overlays.clear();
                    self.log_status("AFNI/SUMA NIML talk disconnected.");
                    changed = true;
                    break;
                }
            }
            event_count += 1;
        }
        if self.verbose && (event_count > 0 || queued_before > 0) {
            self.log_status(format!(
                "AFNI drain pulled {event_count} connection event{} in {:.1} ms; pending elements={}.",
                if event_count == 1 { "" } else { "s" },
                drain_start.elapsed().as_secs_f64() * 1000.0,
                self.pending_afni_elements.len()
            ));
        }

        let mut elements = Vec::new();
        while elements.len() < AFNI_ELEMENTS_PER_DRAIN {
            let Some(element) = self.pending_afni_elements.pop_front() else {
                break;
            };
            elements.push(element);
        }
        if !elements.is_empty() {
            let handle_start = Instant::now();
            if self.verbose {
                let names = elements
                    .iter()
                    .map(|element| element.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                self.log_status(format!(
                    "AFNI handling {} element{} [{}]; pending after pop={}.",
                    elements.len(),
                    if elements.len() == 1 { "" } else { "s" },
                    names,
                    self.pending_afni_elements.len()
                ));
            }
            match self.handle_afni_elements(elements) {
                Ok(event_changed) => changed |= event_changed,
                Err(error) => self.set_error(error),
            }
            if self.verbose {
                self.log_status(format!(
                    "AFNI handled element batch in {:.1} ms.",
                    handle_start.elapsed().as_secs_f64() * 1000.0
                ));
            }
        }

        let registration_start = Instant::now();
        match self.process_pending_afni_surface_registrations() {
            Ok(sent) => {
                if self.verbose && sent {
                    self.log_status(format!(
                        "AFNI surface registration tick took {:.1} ms.",
                        registration_start.elapsed().as_secs_f64() * 1000.0
                    ));
                }
                changed |= sent;
            }
            Err(error) => self.set_error(error),
        }
        let upload_start = Instant::now();
        changed |= self.process_pending_cell_color_upload();
        if self.verbose && self.pending_cell_color_upload.is_some() {
            self.log_status(format!(
                "AFNI color chunk upload tick took {:.1} ms; chunks remain.",
                upload_start.elapsed().as_secs_f64() * 1000.0
            ));
        }
        let deferred_start = Instant::now();
        match self.apply_deferred_afni_rgba_overlays() {
            Ok(applied) => {
                if self.verbose && applied {
                    self.log_status(format!(
                        "AFNI applied deferred RGBA overlays in {:.1} ms.",
                        deferred_start.elapsed().as_secs_f64() * 1000.0
                    ));
                }
                changed |= applied;
            }
            Err(error) => self.set_error(error),
        }

        if event_count == AFNI_EVENTS_PER_DRAIN
            || !self.pending_afni_surface_registrations.is_empty()
            || !self.pending_afni_elements.is_empty()
            || self.pending_cell_color_upload.is_some()
            || !self.deferred_afni_rgba_overlays.is_empty()
        {
            self.request_afni_work();
        }

        changed
    }

    /// Route a batch of parsed incoming NIML elements to their handlers.
    pub(super) fn handle_afni_elements(&mut self, elements: Vec<NimlElement>) -> Result<bool> {
        let mut actions = Vec::new();
        let mut changed = false;
        for element in elements {
            let Some(outcome) = self
                .afni_session
                .receive_element(&mut self.controller, &element)?
            else {
                if self.verbose {
                    self.log_status(format!("Ignored AFNI/SUMA NIML element {}.", element.name));
                }
                continue;
            };
            changed |= outcome.applied_state;
            actions.extend(outcome.actions);
        }

        let action_count = actions.len();
        let actions = coalesce_afni_route_actions(actions);
        if self.verbose && actions.len() < action_count {
            self.log_status(format!(
                "Coalesced {} redundant AFNI/SUMA action{} before applying.",
                action_count - actions.len(),
                if action_count - actions.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }

        for action in actions {
            let label = afni_route_action_label(&action);
            let apply_start = Instant::now();
            changed |= self.apply_afni_route_action(action)?;
            if self.verbose {
                self.log_status(format!(
                    "AFNI action {label} applied in {:.1} ms.",
                    apply_start.elapsed().as_secs_f64() * 1000.0
                ));
            }
        }

        Ok(changed)
    }

    /// Apply one routed AFNI action (overlay, crosshair, rgba) to viewer state.
    pub(super) fn apply_afni_route_action(&mut self, action: AfniRouteAction) -> Result<bool> {
        match action {
            AfniRouteAction::ViewerCommand(command) => {
                self.apply_commands(vec![command]);
                Ok(true)
            }
            AfniRouteAction::LoadDataset(path) => {
                self.load_overlay_path(path)?;
                Ok(true)
            }
            AfniRouteAction::RgbaOverlay(overlay) => self.apply_afni_rgba_overlay(overlay),
            AfniRouteAction::OverlayState(state) => self.apply_afni_overlay_state(state),
            AfniRouteAction::SurfaceCrosshair(crosshair) => {
                self.apply_afni_surface_crosshair(crosshair)
            }
            AfniRouteAction::RoiUpdate(update) => {
                if let Some(visible) = update.visible {
                    self.apply_commands(vec![ViewerCommand::SetRoiVisible(visible)]);
                }
                if let Some(path) = update.path {
                    self.load_roi_path(path)?;
                }
                Ok(true)
            }
        }
    }

    /// Apply an AFNI overlay/threshold settings update to the active overlay.
    pub(super) fn apply_afni_overlay_state(&mut self, state: AfniOverlayState) -> Result<bool> {
        let mut changed = false;

        if let Some(symmetric_range) = state.symmetric_range {
            self.overlay.render.appearance.symmetric_range = symmetric_range;
            changed = true;
        }
        if let Some(range) = state.intensity_range {
            self.overlay.render.appearance.range = range;
            changed = true;
        }
        if let Some(threshold) = state.threshold {
            self.overlay.render.appearance.threshold = threshold;
            changed = true;
        }
        if let Some(opacity) = state.opacity {
            self.overlay.render.appearance.opacity = opacity.clamp(0.0, 1.0);
            changed = true;
        }

        if changed {
            self.refresh_overlay_appearance()?;
        }

        Ok(changed)
    }

    /// Apply a sparse SUMA_irgba node-color overlay sent from AFNI.
    pub(super) fn apply_afni_rgba_overlay(&mut self, overlay: AfniRgbaOverlay) -> Result<bool> {
        let apply_start = Instant::now();
        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before applying AFNI/SUMA RGBA overlay")?;
        let Some(target) = self.afni_surface_target_for_message(
            &overlay.surface_idcode,
            overlay.local_domain_parent_id.as_deref(),
        ) else {
            self.log_status(format!(
                "Ignored AFNI/SUMA RGBA overlay for unknown surface {}{}.",
                overlay.surface_idcode,
                overlay
                    .local_domain_parent_id
                    .as_deref()
                    .map(|parent| format!(" (domain parent {parent})"))
                    .unwrap_or_default()
            ));
            return Ok(false);
        };

        // AFNI resends an identical irgba colorization on every redraw. When the
        // payload for this surface matches the one we last applied, skip the
        // whole O(n) recolor + vertex re-upload and report "no change" so we
        // don't even trigger a redraw.
        let signature = afni_rgba_overlay_signature(&overlay);
        let previous_signature = self
            .afni_rgba_signatures
            .get(&overlay.surface_idcode)
            .copied();
        if self.afni_rgba_colors.is_some() && previous_signature == Some(signature) {
            if self.verbose {
                self.log_status(format!(
                    "Skipped unchanged AFNI/SUMA RGBA overlay for {}.",
                    overlay.surface_idcode
                ));
            }
            return Ok(false);
        }

        if self.pending_cell_color_upload.is_some() {
            self.defer_afni_rgba_overlay(overlay);
            return Ok(false);
        }

        let (colors, applied, skipped) =
            apply_afni_rgba_to_color_cache(self.afni_rgba_colors.take(), mesh, target, &overlay);
        if self.verbose {
            self.log_status(format!(
                "AFNI RGBA color cache updated in {:.1} ms for {} nodes ({} skipped).",
                apply_start.elapsed().as_secs_f64() * 1000.0,
                applied,
                skipped
            ));
        }

        let dataset_id = overlay
            .function_idcode
            .clone()
            .or_else(|| Some("AFNI SUMA_irgba".to_string()));
        let overlay_model = Overlay::from_color_cache(&mesh.domain, colors.clone(), dataset_id)?;
        self.afni_rgba_colors = Some(colors);
        self.overlay.render.render_model = Some(overlay_model);
        self.overlay.data = DatasetOverlayState::None;
        self.overlay.source.path = None;
        self.overlay.source.pair_paths = None;
        self.controller.surface.current_overlay_path = None;
        self.overlay.source.display_name = Some("AFNI SUMA_irgba".to_string());
        if let Some(threshold) = overlay
            .threshold
            .as_deref()
            .and_then(|value| value.parse::<f32>().ok())
        {
            self.overlay.render.appearance.threshold = OverlayThreshold {
                enabled: true,
                absolute: true,
                value: threshold,
                hide_failed: true,
            };
        }
        self.controller.overlay.visible = true;
        // AFNI bakes its threshold into the colors it sends (sub-threshold nodes
        // are simply absent from the sparse list), so we do not re-apply a
        // scalar threshold to this already-resolved color cache.
        self.refresh_pick_overlay_value();
        let upload_start = Instant::now();
        self.upload_surface_buffers();
        if self.verbose {
            self.log_status(format!(
                "AFNI RGBA surface buffer update requested in {:.1} ms.",
                upload_start.elapsed().as_secs_f64() * 1000.0
            ));
        }
        self.update_scene_stats();
        self.afni_rgba_signatures
            .insert(overlay.surface_idcode.clone(), signature);
        self.log_status(format!(
            "Applied AFNI/SUMA RGBA overlay to {applied} nodes{}.",
            if skipped > 0 {
                format!(" ({skipped} skipped)")
            } else {
                String::new()
            }
        ));
        if self.verbose {
            self.log_status(match previous_signature {
                Some(_) => format!(
                    "AFNI/SUMA RGBA overlay for {} changed; re-applied.",
                    overlay.surface_idcode
                ),
                None => format!(
                    "AFNI/SUMA RGBA overlay for {} applied for the first time.",
                    overlay.surface_idcode
                ),
            });
        }

        Ok(true)
    }

    fn defer_afni_rgba_overlay(&mut self, overlay: AfniRgbaOverlay) {
        let key = (
            overlay.surface_idcode.clone(),
            overlay.local_domain_parent_id.clone(),
        );
        self.deferred_afni_rgba_overlays.retain(|candidate| {
            (
                candidate.surface_idcode.clone(),
                candidate.local_domain_parent_id.clone(),
            ) != key
        });
        self.deferred_afni_rgba_overlays.push(overlay);
        if self.verbose {
            self.log_status(format!(
                "Deferred AFNI/SUMA RGBA overlay while GPU chunks are still uploading; deferred={}.",
                self.deferred_afni_rgba_overlays.len()
            ));
        }
    }

    pub(super) fn apply_deferred_afni_rgba_overlays(&mut self) -> Result<bool> {
        if self.pending_cell_color_upload.is_some() || self.deferred_afni_rgba_overlays.is_empty() {
            return Ok(false);
        }

        let overlays = std::mem::take(&mut self.deferred_afni_rgba_overlays);
        let mut changed = false;
        for overlay in overlays {
            changed |= self.apply_afni_rgba_overlay(overlay)?;
        }
        Ok(changed)
    }

    /// Move the local crosshair/pick to AFNI's reported surface node.
    pub(super) fn apply_afni_surface_crosshair(
        &mut self,
        crosshair: AfniSurfaceCrosshair,
    ) -> Result<bool> {
        let Some(local_node) = crosshair.node_index else {
            self.log_status(format!(
                "AFNI/SUMA crosshair at {} did not include a surface node id.",
                coordinate_label(crosshair.surface_position)
            ));
            return Ok(false);
        };
        let node_offset = crosshair
            .surface_idcode
            .as_deref()
            .and_then(|surface_idcode| self.afni_node_offset_for_surface(surface_idcode))
            .unwrap_or(0);
        let node_index = node_offset
            .checked_add(local_node as usize)
            .and_then(|node| u32::try_from(node).ok())
            .context("AFNI/SUMA crosshair node index is outside Sumaru node range")?;
        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before applying AFNI/SUMA crosshair")?;
        let pick = surface_pick_for_mesh_node(mesh, self.overlay.data.node_values(), node_index)
            .with_context(|| {
                format!("AFNI/SUMA crosshair references unavailable node {node_index}")
            })?;

        self.controller.interaction.set_pick(Some(pick));
        self.afni_crosshair_node = Some(node_index);
        self.refresh_pick_overlay_value();
        self.refresh_graph_snapshot_if_open();
        if self.has_both_scene() {
            self.upload_surface_buffers();
        } else {
            self.upload_selection_highlight_buffer();
        }
        self.control.window.request_redraw();
        if self.controller.panels.roi_controller_open {
            self.roi_control.window.request_redraw();
        }
        self.log_status(format!(
            "Applied AFNI/SUMA crosshair to node {}{}.",
            node_index,
            crosshair
                .surface_idcode
                .as_deref()
                .map(|idcode| format!(" on {idcode}"))
                .unwrap_or_default()
        ));

        Ok(true)
    }

    /// Send a node pick to AFNI as a crosshair update.
    pub(super) fn send_afni_crosshair_for_pick(&mut self, pick: SurfacePick) -> Result<()> {
        if self.afni_connection.is_none() {
            return Ok(());
        }
        let Some(element) = self.afni_crosshair_element_for_pick(pick)? else {
            if self.verbose {
                self.log_status(format!(
                    "Could not map node {} to an AFNI/SUMA surface crosshair.",
                    pick.node_index
                ));
            }
            return Ok(());
        };

        let connection = self
            .afni_connection
            .as_mut()
            .context("AFNI/SUMA NIML talk is not connected")?;
        if let Err(error) = connection.send_elements(std::slice::from_ref(&element)) {
            self.disconnect_afni_talk();
            return Err(error.context("AFNI/SUMA crosshair write failed"));
        }
        self.sent_crosshair_node = Some(pick.node_index);
        if self.verbose {
            self.log_status(format!(
                "Sent AFNI/SUMA crosshair for node {}.",
                pick.node_index
            ));
        }

        Ok(())
    }

    /// Build the NIML crosshair element for a pick, mapping to the source surface.
    pub(super) fn afni_crosshair_element_for_pick(
        &self,
        pick: SurfacePick,
    ) -> Result<Option<NimlElement>> {
        if let Some(scene) = self.surface_scene.as_ref() {
            let Some(surface) = scene.surfaces.get(scene.active_index) else {
                return Ok(None);
            };
            let mut node_offset = 0u32;
            for component in &surface.components {
                let Some(mesh) = component.mesh.as_ref() else {
                    continue;
                };
                let node_count = u32::try_from(mesh.vertices.len())
                    .context("surface has too many vertices for AFNI/SUMA node ids")?;
                let Some(node_limit) = node_offset.checked_add(node_count) else {
                    return Ok(None);
                };
                if (node_offset..node_limit).contains(&pick.node_index) {
                    let local_node = pick.node_index - node_offset;
                    let mut info = AfniSurfaceInfo::from_mesh(mesh);
                    decorate_afni_surface_info(&mut info, Some(scene), Some(component));
                    return surface_crosshair_element(
                        mesh,
                        &info,
                        local_node,
                        pick.surface_position,
                    )
                    .map(Some);
                }
                node_offset = node_limit;
            }
        }

        let Some(mesh) = self.mesh.as_ref() else {
            return Ok(None);
        };
        if (pick.node_index as usize) >= mesh.vertices.len() {
            return Ok(None);
        }
        let mut info = AfniSurfaceInfo::from_mesh(mesh);
        decorate_afni_surface_info(&mut info, self.surface_scene.as_ref(), None);
        decorate_afni_surface_volume_info(
            &mut info,
            self.surface_volume_path.as_ref(),
            self.surface_volume_idcode.as_deref(),
        );
        surface_crosshair_element(mesh, &info, pick.node_index, pick.surface_position).map(Some)
    }

    /// Combined-mesh node offset for a given source surface idcode.
    pub(super) fn afni_node_offset_for_surface(&self, surface_idcode: &str) -> Option<usize> {
        self.afni_surface_target_for_message(surface_idcode, None)
            .map(|target| target.node_offset)
    }

    /// Resolve which source surface a node index belongs to for outgoing messages.
    pub(super) fn afni_surface_target_for_message(
        &self,
        surface_idcode: &str,
        local_domain_parent_id: Option<&str>,
    ) -> Option<AfniSurfaceTarget> {
        if let Some(scene) = self.surface_scene.as_ref() {
            if let Some(target) = afni_surface_target_in_scene_surface(
                scene,
                scene.active_index,
                |component, mesh| {
                    afni_component_matches_surface_id(component, mesh, surface_idcode)
                },
            ) {
                return Some(target);
            }

            if let Some(parent_id) = local_domain_parent_id
                && let Some(target) = afni_surface_target_in_scene_surface(
                    scene,
                    scene.active_index,
                    |component, mesh| {
                        afni_component_matches_domain_parent(component, mesh, parent_id)
                    },
                )
            {
                return Some(target);
            }
        }

        let mesh = self.mesh.as_ref()?;
        if mesh.metadata.id.as_str() == surface_idcode
            || local_domain_parent_id
                .is_some_and(|parent_id| afni_mesh_matches_domain_parent(mesh, parent_id))
        {
            return Some(AfniSurfaceTarget {
                node_offset: 0,
                node_count: mesh.vertices.len(),
            });
        }

        None
    }

    /// Lazily load any spec surface meshes needed before AFNI registration.
    pub(super) fn ensure_scene_surfaces_loaded_for_afni(&mut self) -> Result<()> {
        let Some(scene) = self.surface_scene.as_ref() else {
            return Ok(());
        };
        let mut tasks = Vec::new();
        for (surface_index, surface) in scene.surfaces.iter().enumerate() {
            for (component_index, component) in surface.components.iter().enumerate() {
                if !afni_component_is_sendable(component, component.mesh.as_ref()) {
                    continue;
                }
                if component.mesh.is_none() {
                    tasks.push((
                        surface_index,
                        component_index,
                        component.spec_surface.clone(),
                    ));
                }
            }
        }
        if tasks.is_empty() {
            return Ok(());
        }

        let spec = scene.spec.clone();
        let surface_volume_idcode = scene.surface_volume_idcode.clone();
        self.log_status(format!(
            "Loading {} spec surface component{} for AFNI/SUMA registration.",
            tasks.len(),
            if tasks.len() == 1 { "" } else { "s" }
        ));

        for (surface_index, component_index, surface) in tasks {
            let mesh = load_spec_component_mesh(&spec, &surface, surface_volume_idcode.as_deref())?;
            if let Some(scene) = self.surface_scene.as_mut()
                && let Some(component) = scene
                    .surfaces
                    .get_mut(surface_index)
                    .and_then(|surface| surface.components.get_mut(component_index))
                && component.mesh.is_none()
            {
                component.mesh = Some(mesh);
            }
        }

        Ok(())
    }
}

impl ViewerState {
    pub(super) fn request_afni_work(&self) {
        let _ = self.event_proxy.send_event(ViewerEvent::AfniMessagesReady);
    }

    pub(super) fn process_pending_afni_surface_registrations(&mut self) -> Result<bool> {
        if self.pending_afni_surface_registrations.is_empty() {
            return Ok(false);
        }

        let mut sent_any = false;
        let mut completed_surfaces = 0usize;
        let mut sent_parts = 0usize;
        while sent_parts < AFNI_SURFACE_REGISTRATION_PARTS_PER_TICK {
            let Some(registration) = self.pending_afni_surface_registrations.front_mut() else {
                break;
            };
            let Some(part) = AFNI_SURFACE_REGISTRATION_PARTS
                .get(registration.next_part)
                .copied()
            else {
                self.pending_afni_surface_registrations.pop_front();
                completed_surfaces += 1;
                continue;
            };
            let connection = self
                .afni_connection
                .as_mut()
                .context("AFNI/SUMA NIML talk is not connected")?;
            if let Err(error) = connection.send_surface_registration_part(
                Arc::clone(&registration.mesh),
                registration.info.clone(),
                part,
            ) {
                self.disconnect_afni_talk();
                return Err(error.context("AFNI/SUMA NIML write failed"));
            }

            registration.next_part += 1;
            sent_parts += 1;
            sent_any = true;
            if registration.next_part >= AFNI_SURFACE_REGISTRATION_PARTS.len() {
                self.pending_afni_surface_registrations.pop_front();
                completed_surfaces += 1;
            }
        }

        if completed_surfaces > 0 && self.verbose {
            self.log_status(format!(
                "Sent {completed_surfaces} queued AFNI/SUMA surface registration{}.",
                if completed_surfaces == 1 { "" } else { "s" }
            ));
        }

        if self.pending_afni_surface_registrations.is_empty() {
            if self.pending_afni_redraw_after_registrations {
                self.pending_afni_redraw_after_registrations = false;
                // Nudge AFNI to redraw the overlay for the freshly registered
                // surfaces so the connection is visibly live without waiting
                // for the user to click.
                self.send_afni_redraw_crosshair();
            }
        } else {
            self.request_afni_work();
        }

        Ok(sent_any)
    }

    pub(super) fn process_pending_cell_color_upload(&mut self) -> bool {
        let Some(mut upload) = self.pending_cell_color_upload.take() else {
            return false;
        };

        let surface_color_slice = upload.surface_colors.as_deref().map(Vec::as_slice);
        let roi_color_slice = upload.roi_colors.as_deref();
        let mut instances = Vec::new();
        let mut processed_chunks = 0usize;
        let tick_start = Instant::now();

        while processed_chunks < AFNI_CELL_COLOR_UPLOAD_CHUNKS_PER_TICK {
            let Some(range) = upload.ranges.pop_front() else {
                break;
            };
            let chunk_start = Instant::now();
            let prepared_surface = PreparedSurface::from_geometry_cell_color_range(
                &upload.geometry,
                surface_color_slice,
                roi_color_slice,
                range,
            );
            upload.chunks_uploaded += 1;
            processed_chunks += 1;
            if !prepared_surface.is_empty() {
                instances.push(self.surface_render_instance_from_prepared(
                    &prepared_surface,
                    upload.side.clone(),
                    upload.model_matrix,
                    upload.labels,
                ));
            }
            if self.verbose {
                self.log_status(format!(
                    "AFNI colorized surface {} prepared/uploaded chunk {}/{} in {:.1} ms.",
                    upload.surface_id.as_str(),
                    upload.chunks_uploaded,
                    upload.mesh_chunk_count,
                    chunk_start.elapsed().as_secs_f64() * 1000.0
                ));
            }
        }

        let changed = !instances.is_empty();
        if let Some(render_set) = self.surface_render_set.as_mut() {
            render_set.instances.extend(instances);
        }

        if upload.ranges.is_empty() {
            if self.verbose {
                self.log_status(format!(
                    "Finished AFNI colorized surface {} progressive upload \
                     ({} chunks) in this tick after {:.1} ms.",
                    upload.surface_id.as_str(),
                    upload.mesh_chunk_count,
                    tick_start.elapsed().as_secs_f64() * 1000.0
                ));
            }
            self.upload_selection_highlight_buffer();
        } else {
            if self.verbose {
                self.log_status(format!(
                    "AFNI colorized surface {} uploaded {}/{} chunks.",
                    upload.surface_id.as_str(),
                    upload.chunks_uploaded,
                    upload.mesh_chunk_count
                ));
            }
            self.pending_cell_color_upload = Some(upload);
            self.request_afni_work();
        }

        changed
    }
}

fn afni_route_action_label(action: &AfniRouteAction) -> &'static str {
    match action {
        AfniRouteAction::ViewerCommand(_) => "viewer_command",
        AfniRouteAction::LoadDataset(_) => "load_dataset",
        AfniRouteAction::RgbaOverlay(_) => "rgba_overlay",
        AfniRouteAction::OverlayState(_) => "overlay_state",
        AfniRouteAction::SurfaceCrosshair(_) => "surface_crosshair",
        AfniRouteAction::RoiUpdate(_) => "roi_update",
    }
}

pub(super) fn coalesce_afni_route_actions(actions: Vec<AfniRouteAction>) -> Vec<AfniRouteAction> {
    let mut latest_rgba = HashMap::<(String, Option<String>), usize>::new();
    for (index, action) in actions.iter().enumerate() {
        if let AfniRouteAction::RgbaOverlay(overlay) = action {
            latest_rgba.insert(
                (
                    overlay.surface_idcode.clone(),
                    overlay.local_domain_parent_id.clone(),
                ),
                index,
            );
        }
    }

    actions
        .into_iter()
        .enumerate()
        .filter_map(|(index, action)| {
            if let AfniRouteAction::RgbaOverlay(overlay) = &action {
                let key = (
                    overlay.surface_idcode.clone(),
                    overlay.local_domain_parent_id.clone(),
                );
                if latest_rgba.get(&key) != Some(&index) {
                    return None;
                }
            }
            Some(action)
        })
        .collect()
}
