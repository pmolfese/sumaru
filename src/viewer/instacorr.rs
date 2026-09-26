//! Viewer-side InstaCorr sessions, background preprocessing, and controls.

use super::*;

pub(super) struct InstaCorrSession {
    id: u64,
    output_id: u64,
    source_key: OverlayKey,
    source_label: String,
    source_dataset: Dataset,
    draft: InstaCorrOptions,
    tr_text: String,
    applied: Option<InstaCorrOptions>,
    prepared: Option<Arc<PreparedInstaCorr>>,
    enabled: bool,
    busy: bool,
    generation: u64,
    seed_node: Option<u32>,
    pending_seed_node: Option<u32>,
    status: String,
}

pub(super) struct InstaCorrWorkerResult {
    session_id: u64,
    generation: u64,
    options: InstaCorrOptions,
    seed_node: u32,
    result: std::result::Result<InstaCorrComputation, String>,
}

struct InstaCorrComputation {
    prepared: Arc<PreparedInstaCorr>,
    values: Vec<f32>,
}

impl ViewerState {
    /// Toggle (or create) the InstaCorr session associated with the active
    /// time-series source or generated result overlay.
    pub(super) fn toggle_instacorr(&mut self) {
        let active_key = self.overlay.key();
        let existing = active_key.as_ref().and_then(|key| {
            self.instacorr_sessions.iter().position(|session| {
                session.source_key == *key || OverlayKey::Generated(session.output_id) == *key
            })
        });

        if let Some(index) = existing {
            let id = self.instacorr_sessions[index].id;
            let enabled = if self.active_instacorr_session == Some(id) {
                !self.instacorr_sessions[index].enabled
            } else {
                true
            };
            for session in &mut self.instacorr_sessions {
                session.enabled = false;
            }
            self.instacorr_sessions[index].enabled = enabled;
            self.active_instacorr_session = Some(id);
            self.instacorr_window_open = true;
            self.log_status(if enabled {
                "InstaCorr active; ordinary selections recalculate the seed."
            } else {
                "InstaCorr suspended; its result overlay remains available."
            });
            if enabled {
                if self.instacorr_sessions[index].prepared.is_some() {
                    if let Some(node) = self.current_pick_node() {
                        self.request_instacorr_seed_update(id, node);
                    }
                } else if self.instacorr_sessions[index].draft.tr_seconds.is_some()
                    && self.current_pick_node().is_some()
                {
                    self.request_instacorr_recalculate(id);
                }
            }
            self.view_window().request_redraw();
            self.control_window().request_redraw();
            return;
        }

        let Some(source_key) = active_key else {
            self.log_status("Load a surface time-series overlay before activating InstaCorr.");
            return;
        };
        let Some(dataset) = self.overlay.data.dataset().cloned() else {
            self.log_status("The active overlay has no time-series dataset for InstaCorr.");
            return;
        };
        if dataset.kind != DatasetKind::SurfaceTimeSeries {
            self.log_status("The active overlay is not a surface time-series dataset.");
            return;
        }

        let id = self.next_instacorr_id;
        self.next_instacorr_id = self.next_instacorr_id.wrapping_add(1).max(1);
        for session in &mut self.instacorr_sessions {
            session.enabled = false;
        }
        let draft = InstaCorrOptions {
            tr_seconds: dataset.time_step_seconds,
            ..InstaCorrOptions::default()
        };
        let source_has_tr = dataset.time_step_seconds.is_some();
        let tr_text = draft
            .tr_seconds
            .map(|value| value.to_string())
            .unwrap_or_default();
        let source_label = self.overlay.display_text();
        self.instacorr_sessions.push(InstaCorrSession {
            id,
            output_id: id,
            source_key,
            source_label,
            source_dataset: dataset,
            draft,
            tr_text,
            applied: None,
            prepared: None,
            enabled: true,
            busy: false,
            generation: 0,
            seed_node: None,
            pending_seed_node: None,
            status: "Ready to calculate.".to_string(),
        });
        self.active_instacorr_session = Some(id);
        self.instacorr_window_open = true;

        if self
            .instacorr_sessions
            .last()
            .is_some_and(|session| session.draft.tr_seconds.is_some())
            && self.current_pick_node().is_some()
        {
            self.request_instacorr_recalculate(id);
        } else if !source_has_tr {
            self.log_status("InstaCorr controls opened; enter TR, then press Recalculate.");
        } else {
            self.log_status("InstaCorr controls opened; select a seed, then press Recalculate.");
        }
        self.view_window().request_redraw();
    }

    pub(super) fn note_instacorr_pick(&mut self, node: u32) {
        let Some(id) = self.active_instacorr_session else {
            return;
        };
        let Some(session) = self
            .instacorr_sessions
            .iter()
            .find(|session| session.id == id)
        else {
            return;
        };
        if !session.enabled {
            return;
        }
        if session.busy {
            if let Some(session) = self
                .instacorr_sessions
                .iter_mut()
                .find(|session| session.id == id)
            {
                session.pending_seed_node = Some(node);
                session.status = format!("Queued seed vertex {node}…");
            }
        } else if session.prepared.is_some() {
            self.request_instacorr_seed_update(id, node);
        }
    }

    pub(super) fn draw_instacorr_window(&mut self, ctx: &egui::Context) {
        if !self.instacorr_window_open {
            return;
        }
        let Some(id) = self.active_instacorr_session else {
            self.instacorr_window_open = false;
            return;
        };
        let Some(index) = self
            .instacorr_sessions
            .iter()
            .position(|session| session.id == id)
        else {
            self.instacorr_window_open = false;
            return;
        };

        let mut open = true;
        let mut recalculate = false;
        let mut seed_now = false;
        egui::Window::new("InstaCorr")
            .open(&mut open)
            .default_width(390.0)
            .resizable(false)
            .show(ctx, |ui| {
                let session = &mut self.instacorr_sessions[index];
                ui.heading(&session.source_label);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut session.enabled, "Active");
                    if session.busy {
                        ui.spinner();
                        ui.label("Calculating…");
                    }
                });
                ui.separator();
                egui::Grid::new("instacorr_options")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("SUMA preprocessing");
                        ui.checkbox(&mut session.draft.normalize, "Enabled")
                        .on_hover_text(
                            "SUMA-compatible normalize_dset behavior. Off calculates a raw dot product and ignores the options below.",
                        );
                        ui.end_row();
                        ui.label("TR (seconds)");
                        ui.add_enabled(
                            session.draft.normalize && session.draft.bandpass_enabled,
                            egui::TextEdit::singleline(&mut session.tr_text),
                        );
                        ui.end_row();
                        ui.label("Polynomial (polort)");
                        ui.add_enabled(
                            session.draft.normalize,
                            egui::DragValue::new(&mut session.draft.polort).range(-1..=20),
                        );
                        ui.end_row();
                        ui.label("Bandpass");
                        ui.add_enabled_ui(session.draft.normalize, |ui| {
                            ui.checkbox(&mut session.draft.bandpass_enabled, "Enabled")
                        });
                        ui.end_row();
                        ui.label("Low cutoff (Hz)");
                        ui.add_enabled(
                            session.draft.normalize && session.draft.bandpass_enabled,
                            egui::DragValue::new(&mut session.draft.low_hz)
                                .speed(0.001)
                                .range(0.0..=1000.0),
                        );
                        ui.end_row();
                        ui.label("High cutoff (Hz)");
                        ui.add_enabled(
                            session.draft.normalize && session.draft.bandpass_enabled,
                            egui::DragValue::new(&mut session.draft.high_hz)
                                .speed(0.001)
                                .range(0.0..=1000.0),
                        );
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    recalculate = ui
                        .add_enabled(!session.busy, egui::Button::new("Recalculate"))
                        .on_hover_text("Apply these settings and rebuild the preprocessed cache")
                        .clicked();
                    seed_now = ui
                        .add_enabled(
                            !session.busy && session.prepared.is_some(),
                            egui::Button::new("Update seed"),
                        )
                        .on_hover_text("Recalculate only the dot product for the current vertex")
                        .clicked();
                });
                ui.separator();
                ui.label(egui::RichText::new(&session.status).color(egui::Color32::GRAY));
                if let Some(applied) = session.applied {
                    let applied_summary = if applied.normalize {
                        format!(
                            "Applied: mode=SUMA correlation, TR={} s, polort={}, bandpass={}",
                            applied
                                .tr_seconds
                                .map(|value| format!("{value:.6}"))
                                .unwrap_or_else(|| "--".to_string()),
                            applied.polort,
                            if applied.bandpass_enabled {
                                format!("{:.6}–{:.6} Hz", applied.low_hz, applied.high_hz)
                            } else {
                                "off".to_string()
                            }
                        )
                    } else {
                        "Applied: mode=raw dot product (preprocessing options ignored)".to_string()
                    };
                    ui.label(
                        egui::RichText::new(applied_summary)
                        .small()
                        .color(egui::Color32::GRAY),
                    );
                }
                ui.label(
                    egui::RichText::new(
                        "D toggles this session. Right-click updates the seed; Shift-right-click moves the crosshair only.",
                    )
                    .small()
                    .color(egui::Color32::GRAY),
                );
            });
        self.instacorr_window_open = open;
        if recalculate {
            self.request_instacorr_recalculate(id);
        } else if seed_now && let Some(node) = self.current_pick_node() {
            self.request_instacorr_seed_update(id, node);
        }
    }

    pub(super) fn instacorr_readout(&self) -> Option<(String, String)> {
        let id = self.active_instacorr_session?;
        let session = self
            .instacorr_sessions
            .iter()
            .find(|session| session.id == id)?;
        let state = if session.busy {
            "calculating"
        } else if session.enabled {
            "active"
        } else {
            "suspended"
        };
        let detail = session.applied.map_or_else(
            || "not calculated".to_string(),
            |options| {
                let seed = session
                    .seed_node
                    .map_or_else(|| "--".to_string(), |node| node.to_string());
                if options.normalize {
                    format!(
                        "seed {seed}; mode SUMA correlation; TR {} s; polort {}; BP {}",
                        options
                            .tr_seconds
                            .map_or_else(|| "--".to_string(), |value| format!("{value:.6}")),
                        options.polort,
                        if options.bandpass_enabled {
                            format!("{:.6}–{:.6}", options.low_hz, options.high_hz)
                        } else {
                            "off".to_string()
                        }
                    )
                } else {
                    format!("seed {seed}; mode raw dot; preprocessing ignored")
                }
            },
        );
        Some((format!("{state}: {}", session.source_label), detail))
    }

    fn current_pick_node(&self) -> Option<u32> {
        self.controller.interaction.pick.map(|pick| pick.node_index)
    }

    fn request_instacorr_recalculate(&mut self, id: u64) {
        let Some(index) = self
            .instacorr_sessions
            .iter()
            .position(|session| session.id == id)
        else {
            return;
        };
        let Some(seed_node) = self.current_pick_node() else {
            self.instacorr_sessions[index].status = "Select a seed vertex first.".to_string();
            return;
        };
        let parsed_tr = self.instacorr_sessions[index]
            .tr_text
            .trim()
            .parse::<f64>()
            .ok();
        let session = &mut self.instacorr_sessions[index];
        session.draft.tr_seconds = parsed_tr.filter(|value| value.is_finite() && *value > 0.0);
        let sample_count = session
            .source_dataset
            .columns
            .iter()
            .filter(|column| column.role == ColumnRole::TimePoint)
            .count();
        if let Err(error) = session.draft.validate(sample_count) {
            session.status = error.to_string();
            return;
        }
        let options = session.draft;
        let dataset = session.source_dataset.clone();
        let Some(seed_row) = dataset_row_for_node(&dataset, seed_node) else {
            session.status = format!("Vertex {seed_node} is not present in the source dataset.");
            return;
        };
        session.generation = session.generation.wrapping_add(1);
        let generation = session.generation;
        session.busy = true;
        session.pending_seed_node = None;
        session.status = if options.normalize {
            "Preprocessing all time series…"
        } else {
            "Preparing raw dot product…"
        }
        .to_string();
        let sender = self.instacorr_sender.clone();
        let proxy = self.event_proxy.clone();
        thread::spawn(move || {
            let result = prepare_dataset(&dataset, options)
                .and_then(|prepared| {
                    let prepared = Arc::new(prepared);
                    let values = prepared.correlate(seed_row)?;
                    Ok(InstaCorrComputation { prepared, values })
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(InstaCorrWorkerResult {
                session_id: id,
                generation,
                options,
                seed_node,
                result,
            });
            let _ = proxy.send_event(ViewerEvent::InstaCorrComputed);
        });
    }

    fn request_instacorr_seed_update(&mut self, id: u64, seed_node: u32) {
        let Some(session) = self
            .instacorr_sessions
            .iter_mut()
            .find(|session| session.id == id)
        else {
            return;
        };
        let Some(seed_row) = dataset_row_for_node(&session.source_dataset, seed_node) else {
            session.status = format!("Vertex {seed_node} is not present in the source dataset.");
            return;
        };
        let Some(prepared) = session.prepared.clone() else {
            return;
        };
        let Some(options) = session.applied else {
            return;
        };
        session.generation = session.generation.wrapping_add(1);
        let generation = session.generation;
        session.busy = true;
        session.pending_seed_node = None;
        session.status = format!("Updating seed vertex {seed_node}…");
        let sender = self.instacorr_sender.clone();
        let proxy = self.event_proxy.clone();
        thread::spawn(move || {
            let result = prepared
                .correlate(seed_row)
                .map(|values| InstaCorrComputation { prepared, values })
                .map_err(|error| error.to_string());
            let _ = sender.send(InstaCorrWorkerResult {
                session_id: id,
                generation,
                options,
                seed_node,
                result,
            });
            let _ = proxy.send_event(ViewerEvent::InstaCorrComputed);
        });
    }

    pub(super) fn drain_instacorr_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.instacorr_receiver.try_recv() {
            let Some(index) = self
                .instacorr_sessions
                .iter()
                .position(|session| session.id == result.session_id)
            else {
                continue;
            };
            if self.instacorr_sessions[index].generation != result.generation {
                continue;
            }
            self.instacorr_sessions[index].busy = false;
            match result.result {
                Ok(computation) => {
                    self.instacorr_sessions[index].prepared = Some(computation.prepared.clone());
                    self.instacorr_sessions[index].applied = Some(result.options);
                    self.instacorr_sessions[index].seed_node = Some(result.seed_node);
                    self.instacorr_sessions[index].status = format!(
                        "Calculated vertex {} across {} nodes.",
                        result.seed_node,
                        computation.values.len()
                    );
                    if let Err(error) = self.install_instacorr_result(
                        result.session_id,
                        result.seed_node,
                        &computation.prepared,
                        computation.values,
                    ) {
                        self.instacorr_sessions[index].status = error.to_string();
                        self.set_error(error);
                    }
                    if let Some(node) = self.instacorr_sessions[index].pending_seed_node.take()
                        && self.instacorr_sessions[index].enabled
                        && node != result.seed_node
                    {
                        self.request_instacorr_seed_update(result.session_id, node);
                    }
                    changed = true;
                }
                Err(error) => {
                    self.instacorr_sessions[index].status = error.clone();
                    self.log_status(format!("InstaCorr: {error}"));
                    changed = true;
                }
            }
        }
        changed
    }

    fn install_instacorr_result(
        &mut self,
        session_id: u64,
        seed_node: u32,
        prepared: &PreparedInstaCorr,
        mut values: Vec<f32>,
    ) -> Result<()> {
        let session = self
            .instacorr_sessions
            .iter()
            .find(|session| session.id == session_id)
            .context("InstaCorr session ended before its result arrived")?;
        if prepared.options.normalize {
            for value in &mut values {
                if value.is_finite() {
                    *value = value.clamp(-1.0, 1.0);
                }
            }
        }
        let source_label = session.source_label.clone();
        let source_dataset = session.source_dataset.clone();
        let output_id = session.output_id;
        let stat = prepared.options.normalize.then(|| {
            format!(
                "Correl({},{},{})",
                prepared.sample_count, 1, prepared.removed_dof
            )
        });
        let role = if stat.is_some() {
            ColumnRole::Statistic
        } else {
            ColumnRole::Intensity
        };
        let column = DataColumn::new(
            format!("InstaCorr seed {seed_node}"),
            role,
            None,
            ColumnData::Float32(values),
        )?
        .with_stat(stat);
        let dataset = Dataset {
            kind: DatasetKind::SurfaceScalar,
            domain_id: source_dataset.domain_id.clone(),
            row_count: source_dataset.row_count,
            node_indices: source_dataset.node_indices.clone(),
            columns: vec![column],
            time_step_seconds: None,
            parent_ids: DatasetParentIds {
                source_dataset_id: source_dataset.parent_ids.source_dataset_id.clone(),
                domain_parent_id: source_dataset.parent_ids.domain_parent_id.clone(),
                surface_parent_id: source_dataset.parent_ids.surface_parent_id.clone(),
                volume_parent_id: source_dataset.parent_ids.volume_parent_id.clone(),
                originator_id: Some("sumaru InstaCorr".to_string()),
            },
        };
        let node_count = self
            .mesh
            .as_ref()
            .context("surface was unloaded while InstaCorr was calculating")?
            .domain
            .node_count;
        let columns = OverlayColumnSelections::default();
        let node_values = overlay_dataset_from_canonical_dataset(&dataset, node_count, columns)?;
        let key = OverlayKey::Generated(output_id);
        if let Some(index) = self.overlay_index_for_key(&key) {
            self.select_overlay(index)?;
        } else {
            self.prepare_overlay_install(key);
        }

        self.overlay.clear();
        self.overlay.source.generated_id = Some(output_id);
        self.overlay.source.display_name =
            Some(format!("InstaCorr — {source_label} — seed {seed_node}"));
        self.overlay.data = DatasetOverlayState::Loaded {
            canonical_dataset: dataset,
            columns,
            node_values,
        };
        self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
        self.controller.overlay.visible = true;
        let range = if prepared.options.normalize {
            ValueRange {
                min: -0.5,
                max: 0.5,
            }
        } else {
            self.overlay
                .data
                .node_values()
                .map(|values| symmetric_value_range(values.range))
                .unwrap_or(DEFAULT_OVERLAY_RANGE)
        };
        self.overlay.render.appearance = OverlayAppearance::from_range(range);
        self.overlay.render.appearance.symmetric_range = true;
        self.overlay.render.appearance.threshold = OverlayThreshold {
            enabled: true,
            absolute: true,
            value: 0.0,
            hide_failed: true,
        };
        self.controller.surface.current_overlay_path = None;
        self.sanitize_overlay_appearance();
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!("InstaCorr updated from seed vertex {seed_node}."));
        Ok(())
    }

    pub(super) fn end_instacorr_for_output(&mut self, output_id: u64) {
        self.instacorr_sessions
            .retain(|session| session.output_id != output_id);
        if self.active_instacorr_session.is_some_and(|id| {
            !self
                .instacorr_sessions
                .iter()
                .any(|session| session.id == id)
        }) {
            self.active_instacorr_session = None;
            self.instacorr_window_open = false;
        }
    }

    pub(super) fn clear_instacorr_sessions(&mut self) {
        self.instacorr_sessions.clear();
        self.active_instacorr_session = None;
        self.instacorr_window_open = false;
    }
}

fn dataset_row_for_node(dataset: &Dataset, node: u32) -> Option<usize> {
    match dataset.node_indices.as_ref() {
        Some(indices) => indices.iter().position(|candidate| *candidate == node),
        None => ((node as usize) < dataset.row_count).then_some(node as usize),
    }
}
