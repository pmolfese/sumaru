//! Compact 3D+time mode built on the normal overlay renderer.
//!
//! The source dataset stays intact here while the active overlay is replaced by
//! a one-column scalar view for the cursor or selected response window. This
//! keeps timecourse-specific code out of the renderer and overlay stack.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeCourseDisplay {
    Cursor,
    Window,
}

impl TimeCourseDisplay {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cursor => "Time point",
            Self::Window => "Window summary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeCourseBaseline {
    Raw,
    SubtractMean,
    ZScore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeCourseScale {
    AutoRobust,
    FullRange,
}

impl TimeCourseScale {
    pub fn label(self) -> &'static str {
        match self {
            Self::AutoRobust => "Auto robust (99.5%)",
            Self::FullRange => "Full range",
        }
    }
}

impl TimeCourseBaseline {
    pub fn label(self) -> &'static str {
        match self {
            Self::Raw => "Raw",
            Self::SubtractMean => "Baseline subtract",
            Self::ZScore => "Baseline z-score",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeCourseMeasure {
    Mean,
    PositivePeak,
    NegativePeak,
    AbsolutePeak,
    AreaUnderCurve,
}

impl TimeCourseMeasure {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mean => "Mean",
            Self::PositivePeak => "Positive peak",
            Self::NegativePeak => "Negative peak",
            Self::AbsolutePeak => "Absolute peak",
            Self::AreaUnderCurve => "Area under curve",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimeCourseControls {
    pub cursor: usize,
    pub baseline_start: usize,
    pub baseline_end: usize,
    pub response_start: usize,
    pub response_end: usize,
    pub display: TimeCourseDisplay,
    pub baseline: TimeCourseBaseline,
    pub scale: TimeCourseScale,
    pub measure: TimeCourseMeasure,
    pub threshold_enabled: bool,
    pub threshold_absolute: bool,
    pub threshold_value: f32,
    pub playing: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TimeCoursePlotMarkers {
    pub(super) cursor: usize,
    pub(super) baseline: [usize; 2],
    pub(super) response: [usize; 2],
    pub(super) start_seconds: f64,
    pub(super) end_seconds: f64,
    pub(super) threshold: Option<TimeCoursePlotThreshold>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TimeCoursePlotThreshold {
    pub(super) value: f32,
    pub(super) absolute: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TimeCoursePlotInteraction {
    Cursor(usize),
    Baseline { range: [usize; 2], commit: bool },
    Response([usize; 2]),
}

pub(super) struct TimeCourseState {
    pub(super) source_dataset: Dataset,
    pub(super) time_columns: Vec<usize>,
    conditions: Vec<TimeCourseCondition>,
    active_condition: usize,
    times_seconds: Vec<f64>,
    baseline_stats: Vec<BaselineStats>,
    cursor_full_range: ValueRange,
    cursor_robust_range: ValueRange,
    cursor_scale_dirty: bool,
    pub(super) controls: TimeCourseControls,
    next_frame_at: Instant,
    frame_interval: Duration,
}

struct TimeCourseCondition {
    label: String,
    dataset: Dataset,
    time_columns: Vec<usize>,
    baseline_stats: Vec<BaselineStats>,
}

#[derive(Debug, Clone, Copy)]
struct BaselineStats {
    mean: f32,
    standard_deviation: Option<f32>,
}

impl TimeCourseState {
    #[cfg(test)]
    fn new(dataset: Dataset) -> Result<Self> {
        Self::from_conditions(vec![("Timecourse".to_string(), dataset)], 0)
    }

    fn from_conditions(datasets: Vec<(String, Dataset)>, active_condition: usize) -> Result<Self> {
        ensure!(!datasets.is_empty(), "timecourse mode requires an overlay");
        ensure!(
            active_condition < datasets.len(),
            "active timecourse condition is out of range"
        );
        let time_columns = datasets[0]
            .1
            .columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| {
                (column.role == ColumnRole::TimePoint && column_is_numeric(column)).then_some(index)
            })
            .collect::<Vec<_>>();
        ensure!(
            !time_columns.is_empty(),
            "timecourse mode requires GIFTI time-point data arrays"
        );

        let step = datasets[0].1.time_step_seconds.unwrap_or(1.0);
        let start = datasets[0].1.time_start_seconds.unwrap_or(0.0);
        let expected_domain = datasets[0].1.domain_id.clone();
        let expected_rows = datasets[0].1.row_count;
        let expected_nodes = datasets[0].1.node_indices.clone();
        let times_seconds = (0..time_columns.len())
            .map(|index| start + index as f64 * step)
            .collect::<Vec<_>>();
        let cursor = times_seconds
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.abs()
                    .partial_cmp(&right.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(0, |(index, _)| index);
        let pre_event_baseline_end = times_seconds
            .iter()
            .rposition(|time| *time < 0.0)
            .map(|index| index.min(time_columns.len() - 1));
        let baseline_end = pre_event_baseline_end.unwrap_or(cursor);
        let response_start = times_seconds
            .iter()
            .position(|time| *time >= 0.0)
            .unwrap_or(cursor);
        let response_end = times_seconds
            .iter()
            .rposition(|time| *time <= 0.2)
            .unwrap_or_else(|| {
                (response_start + time_columns.len() / 5).min(time_columns.len() - 1)
            })
            .max(response_start);
        let frame_interval = Duration::from_secs_f64(step.clamp(1.0 / 60.0, 0.25));

        let controls = TimeCourseControls {
            cursor,
            baseline_start: 0,
            baseline_end,
            response_start,
            response_end,
            display: TimeCourseDisplay::Cursor,
            baseline: if pre_event_baseline_end.is_some() {
                TimeCourseBaseline::SubtractMean
            } else {
                TimeCourseBaseline::Raw
            },
            scale: TimeCourseScale::AutoRobust,
            measure: TimeCourseMeasure::PositivePeak,
            threshold_enabled: false,
            threshold_absolute: true,
            threshold_value: 0.0,
            playing: false,
        };
        let mut conditions = Vec::with_capacity(datasets.len());
        for (label, dataset) in datasets {
            let condition_columns = dataset
                .columns
                .iter()
                .enumerate()
                .filter_map(|(index, column)| {
                    (column.role == ColumnRole::TimePoint && column_is_numeric(column))
                        .then_some(index)
                })
                .collect::<Vec<_>>();
            validate_timecourse_condition(
                &dataset,
                &condition_columns,
                time_columns.len(),
                start,
                step,
                &expected_domain,
                expected_rows,
                expected_nodes.as_deref(),
            )?;
            let condition_stats = baseline_statistics(
                &dataset,
                &condition_columns,
                controls.baseline_start,
                controls.baseline_end,
            );
            conditions.push(TimeCourseCondition {
                label,
                dataset,
                time_columns: condition_columns,
                baseline_stats: condition_stats,
            });
        }
        let (cursor_full_range, cursor_robust_range) =
            timecourse_condition_ranges(&conditions, controls.baseline)?;
        let active = &conditions[active_condition];
        Ok(Self {
            source_dataset: active.dataset.clone(),
            time_columns: active.time_columns.clone(),
            baseline_stats: active.baseline_stats.clone(),
            conditions,
            active_condition,
            times_seconds,
            cursor_full_range,
            cursor_robust_range,
            cursor_scale_dirty: false,
            controls,
            next_frame_at: Instant::now() + frame_interval,
            frame_interval,
        })
    }

    pub(super) fn activate_condition(&mut self, index: usize) -> Result<()> {
        let condition = self
            .conditions
            .get(index)
            .context("timecourse condition is out of range")?;
        self.active_condition = index;
        self.source_dataset = condition.dataset.clone();
        self.time_columns = condition.time_columns.clone();
        self.baseline_stats = condition.baseline_stats.clone();
        Ok(())
    }

    pub(super) fn condition_count(&self) -> usize {
        self.conditions.len()
    }

    pub(super) fn active_condition(&self) -> usize {
        self.active_condition
    }

    pub(super) fn condition_label(&self, index: usize) -> Option<&str> {
        self.conditions
            .get(index)
            .map(|condition| condition.label.as_str())
    }

    pub(super) fn condition_dataset(&self, index: usize) -> Option<&Dataset> {
        self.conditions
            .get(index)
            .map(|condition| &condition.dataset)
    }

    pub(super) fn condition_sample_value(
        &self,
        condition_index: usize,
        row: usize,
        sample: usize,
    ) -> Option<f32> {
        let condition = self.conditions.get(condition_index)?;
        let column = condition
            .dataset
            .columns
            .get(*condition.time_columns.get(sample)?)?;
        let value = numeric_column_value_as_f32(column, row)?;
        transform_timecourse_value(
            value,
            *condition.baseline_stats.get(row)?,
            self.controls.baseline,
        )
    }

    pub(super) fn sample_count(&self) -> usize {
        self.time_columns.len()
    }

    pub(super) fn sample_time(&self, index: usize) -> f64 {
        self.times_seconds[index.min(self.times_seconds.len() - 1)]
    }

    fn sample_time_ms(&self, index: usize) -> f64 {
        self.sample_time(index) * 1000.0
    }

    fn sample_index_for_time_ms(&self, time_ms: f64) -> usize {
        self.times_seconds
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                ((*left * 1000.0 - time_ms).abs())
                    .partial_cmp(&((*right * 1000.0 - time_ms).abs()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(0, |(index, _)| index)
    }

    fn sample_step_ms(&self) -> f64 {
        if self.times_seconds.len() >= 2 {
            ((self.times_seconds[1] - self.times_seconds[0]) * 1000.0).abs()
        } else {
            1.0
        }
    }

    pub(super) fn sample_value(&self, row: usize, sample: usize) -> Option<f32> {
        let column = self
            .source_dataset
            .columns
            .get(*self.time_columns.get(sample)?)?;
        let value = numeric_column_value_as_f32(column, row)?;
        let stats = *self.baseline_stats.get(row)?;
        let value = transform_timecourse_value(value, stats, self.controls.baseline)?;
        value.is_finite().then_some(value)
    }

    fn cursor_color_range(&self) -> ValueRange {
        match self.controls.scale {
            TimeCourseScale::AutoRobust => self.cursor_robust_range,
            TimeCourseScale::FullRange => self.cursor_full_range,
        }
    }

    pub(super) fn threshold_range(&self) -> ValueRange {
        self.cursor_full_range
    }

    fn sanitize_controls(&self, controls: &mut TimeCourseControls) {
        let last = self.sample_count() - 1;
        controls.cursor = controls.cursor.min(last);
        controls.baseline_start = controls.baseline_start.min(last);
        controls.baseline_end = controls.baseline_end.clamp(controls.baseline_start, last);
        controls.response_start = controls.response_start.min(last);
        controls.response_end = controls.response_end.clamp(controls.response_start, last);
        if !controls.threshold_value.is_finite() {
            controls.threshold_value = 0.0;
        }
    }

    fn plot_markers(&self) -> TimeCoursePlotMarkers {
        TimeCoursePlotMarkers {
            cursor: self.controls.cursor,
            baseline: [self.controls.baseline_start, self.controls.baseline_end],
            response: [self.controls.response_start, self.controls.response_end],
            start_seconds: self.sample_time(0),
            end_seconds: self.sample_time(self.sample_count() - 1),
            threshold: self
                .controls
                .threshold_enabled
                .then_some(TimeCoursePlotThreshold {
                    value: self.controls.threshold_value,
                    absolute: self.controls.threshold_absolute,
                }),
        }
    }

    fn display_dataset(&self) -> Result<Dataset> {
        let values = timecourse_values(
            &self.source_dataset,
            &self.time_columns,
            &self.baseline_stats,
            &self.controls,
            self.source_dataset.time_step_seconds.unwrap_or(1.0) as f32,
        );
        let label = match self.controls.display {
            TimeCourseDisplay::Cursor => {
                format!(
                    "t = {:.1} ms",
                    self.sample_time(self.controls.cursor) * 1000.0
                )
            }
            TimeCourseDisplay::Window => format!(
                "{} {:.1}..{:.1} ms",
                self.controls.measure.label(),
                self.sample_time(self.controls.response_start) * 1000.0,
                self.sample_time(self.controls.response_end) * 1000.0
            ),
        };
        let units = self
            .source_dataset
            .columns
            .get(self.time_columns[0])
            .and_then(|column| column.units.clone());
        let column = DataColumn::new(
            label,
            ColumnRole::Intensity,
            units,
            ColumnData::Float32(values),
        )?;
        Ok(Dataset {
            kind: DatasetKind::SurfaceScalar,
            domain_id: self.source_dataset.domain_id.clone(),
            row_count: self.source_dataset.row_count,
            node_indices: self.source_dataset.node_indices.clone(),
            columns: vec![column],
            time_step_seconds: None,
            time_start_seconds: None,
            parent_ids: self.source_dataset.parent_ids.clone(),
        })
    }
}

impl ViewerState {
    pub(super) fn initialize_timecourse_mode(&mut self) -> Result<()> {
        let active = self.active_overlay_index().unwrap_or(0);
        let mut conditions = Vec::with_capacity(self.overlay_count());
        for index in 0..self.overlay_count() {
            let overlay = if self.active_overlay_index() == Some(index) {
                &self.overlay
            } else {
                self.overlay_stack.slots[index]
                    .as_ref()
                    .context("timecourse overlay slot is empty")?
            };
            let dataset = overlay
                .data
                .dataset()
                .cloned()
                .context("timecourse mode requires loaded 3D+time overlays")?;
            ensure!(
                dataset.kind == DatasetKind::SurfaceTimeSeries
                    || dataset
                        .columns
                        .iter()
                        .any(|column| column.role == ColumnRole::TimePoint),
                "overlay {} is not a surface time series",
                overlay.display_text()
            );
            conditions.push((overlay.display_text(), dataset));
        }
        self.timecourse = Some(TimeCourseState::from_conditions(conditions, active)?);
        self.set_graph_window_open(true);
        self.apply_timecourse_overlay()?;
        let state = self
            .timecourse
            .as_ref()
            .expect("timecourse was initialized");
        self.log_status(format!(
            "Timecourse mode: {} condition(s), {} samples, {:.1} to {:.1} ms.",
            state.condition_count(),
            state.sample_count(),
            state.sample_time(0) * 1000.0,
            state.sample_time(state.sample_count() - 1) * 1000.0,
        ));
        Ok(())
    }

    pub(super) fn set_timecourse_controls(&mut self, controls: TimeCourseControls) -> Result<()> {
        self.update_timecourse_controls(controls, true)
    }

    pub(super) fn preview_timecourse_controls(
        &mut self,
        controls: TimeCourseControls,
    ) -> Result<()> {
        self.update_timecourse_controls(controls, false)
    }

    fn update_timecourse_controls(
        &mut self,
        mut controls: TimeCourseControls,
        recompute_scale: bool,
    ) -> Result<()> {
        let state = self
            .timecourse
            .as_mut()
            .context("timecourse mode is not active")?;
        state.sanitize_controls(&mut controls);
        let baseline_window_changed = controls.baseline_start != state.controls.baseline_start
            || controls.baseline_end != state.controls.baseline_end;
        let baseline_transform_changed = controls.baseline != state.controls.baseline;
        let starting_playback = controls.playing && !state.controls.playing;
        state.controls = controls;
        if baseline_window_changed {
            for condition in &mut state.conditions {
                condition.baseline_stats = baseline_statistics(
                    &condition.dataset,
                    &condition.time_columns,
                    state.controls.baseline_start,
                    state.controls.baseline_end,
                );
            }
            state.baseline_stats = state.conditions[state.active_condition]
                .baseline_stats
                .clone();
            state.cursor_scale_dirty = true;
        }
        if baseline_transform_changed || (recompute_scale && state.cursor_scale_dirty) {
            (state.cursor_full_range, state.cursor_robust_range) =
                timecourse_condition_ranges(&state.conditions, state.controls.baseline)?;
            state.cursor_scale_dirty = false;
        }
        if starting_playback {
            state.next_frame_at = Instant::now() + state.frame_interval;
        }
        self.apply_timecourse_overlay()
    }

    pub(super) fn update_timecourse_playback(&mut self, now: Instant) {
        let Some(state) = self.timecourse.as_mut() else {
            return;
        };
        if !state.controls.playing {
            return;
        }
        self.view.repaint_at = Some(now + state.frame_interval);
        if now < state.next_frame_at {
            return;
        }
        state.controls.cursor = (state.controls.cursor + 1) % state.sample_count();
        state.controls.display = TimeCourseDisplay::Cursor;
        state.next_frame_at = now + state.frame_interval;
        if let Err(error) = self.apply_timecourse_overlay() {
            self.set_error(error);
        }
    }

    pub(super) fn apply_timecourse_overlay(&mut self) -> Result<()> {
        let state = self
            .timecourse
            .as_ref()
            .context("timecourse mode is not active")?;
        let controls = state.controls.clone();
        let dataset = state.display_dataset()?;
        let node_count = self
            .mesh
            .as_ref()
            .context("load a surface before displaying a timecourse")?
            .domain
            .node_count;
        let columns = OverlayColumnSelections::default();
        let node_values = overlay_dataset_from_canonical_dataset(&dataset, node_count, columns)?;
        let range = node_values.range;
        let display_range = match controls.display {
            TimeCourseDisplay::Cursor => state.cursor_color_range(),
            TimeCourseDisplay::Window => symmetric_value_range(range),
        };

        self.overlay.data = DatasetOverlayState::Loaded {
            canonical_dataset: dataset,
            columns,
            node_values,
        };
        self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
        self.overlay.source.label_table = None;
        self.overlay.render.appearance.colormap = OverlayColorMap::SpectrumRedToBlue;
        self.overlay.render.appearance.range = display_range;
        self.overlay.render.appearance.symmetric_range =
            display_range.min < 0.0 && display_range.max > 0.0;
        self.overlay.render.appearance.threshold = OverlayThreshold {
            enabled: controls.threshold_enabled,
            absolute: controls.threshold_absolute,
            value: controls.threshold_value,
            hide_failed: true,
        };
        self.sanitize_overlay_appearance();
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.refresh_graph_snapshot_if_open();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.view.window.request_redraw();
        Ok(())
    }

    pub(super) fn draw_timecourse_contents(
        &self,
        ui: &mut egui::Ui,
        actions: &mut Vec<ViewerCommand>,
    ) {
        let Some(state) = self.timecourse.as_ref() else {
            return;
        };
        let mut controls = state.controls.clone();
        let mut changed = false;

        ui.horizontal(|ui| {
            let play_label = if controls.playing { "Pause" } else { "Play" };
            if ui.button(play_label).clicked() {
                controls.playing = !controls.playing;
                changed = true;
            }
            let last = state.sample_count() - 1;
            let cursor_changed = ui
                .add(
                    egui::Slider::new(&mut controls.cursor, 0..=last)
                        .show_value(false)
                        .text("Time"),
                )
                .changed();
            if cursor_changed {
                controls.display = TimeCourseDisplay::Cursor;
                controls.playing = false;
                changed = true;
            }
            ui.monospace(format!(
                "{:.1} ms",
                state.sample_time(controls.cursor) * 1000.0
            ));
            egui::ComboBox::from_id_salt("tc_display")
                .selected_text(controls.display.label())
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(
                            &mut controls.display,
                            TimeCourseDisplay::Cursor,
                            "Time point",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(
                            &mut controls.display,
                            TimeCourseDisplay::Window,
                            "Window summary",
                        )
                        .changed();
                });
            egui::ComboBox::from_id_salt("tc_baseline")
                .selected_text(controls.baseline.label())
                .show_ui(ui, |ui| {
                    for choice in [
                        TimeCourseBaseline::Raw,
                        TimeCourseBaseline::SubtractMean,
                        TimeCourseBaseline::ZScore,
                    ] {
                        changed |= ui
                            .selectable_value(&mut controls.baseline, choice, choice.label())
                            .changed();
                    }
                });
            egui::ComboBox::from_id_salt("tc_scale")
                .selected_text(controls.scale.label())
                .show_ui(ui, |ui| {
                    for choice in [TimeCourseScale::AutoRobust, TimeCourseScale::FullRange] {
                        changed |= ui
                            .selectable_value(&mut controls.scale, choice, choice.label())
                            .changed();
                    }
                });
        });

        ui.horizontal(|ui| {
            let time_min_ms = state.sample_time_ms(0);
            let time_max_ms = state.sample_time_ms(state.sample_count() - 1);
            let time_speed_ms = state.sample_step_ms();
            ui.label("Baseline");
            let mut baseline_start_ms = state.sample_time_ms(controls.baseline_start);
            let baseline_start_response = ui.add(
                egui::DragValue::new(&mut baseline_start_ms)
                    .range(time_min_ms..=time_max_ms)
                    .speed(time_speed_ms)
                    .suffix(" ms")
                    .max_decimals(1),
            );
            if baseline_start_response.changed() {
                controls.baseline_start = state.sample_index_for_time_ms(baseline_start_ms);
                changed = true;
            }
            ui.label("to");
            let mut baseline_end_ms = state.sample_time_ms(controls.baseline_end);
            let baseline_end_response = ui.add(
                egui::DragValue::new(&mut baseline_end_ms)
                    .range(time_min_ms..=time_max_ms)
                    .speed(time_speed_ms)
                    .suffix(" ms")
                    .max_decimals(1),
            );
            if baseline_end_response.changed() {
                controls.baseline_end = state.sample_index_for_time_ms(baseline_end_ms);
                changed = true;
            }
            let mut preview_only =
                baseline_start_response.dragged() || baseline_end_response.dragged();
            if baseline_start_response.drag_stopped() || baseline_end_response.drag_stopped() {
                changed = true;
                preview_only = false;
            }
            ui.separator();
            ui.label("Response");
            let mut response_start_ms = state.sample_time_ms(controls.response_start);
            let response_start = ui.add(
                egui::DragValue::new(&mut response_start_ms)
                    .range(time_min_ms..=time_max_ms)
                    .speed(time_speed_ms)
                    .suffix(" ms")
                    .max_decimals(1),
            );
            if response_start.changed() {
                controls.response_start = state.sample_index_for_time_ms(response_start_ms);
                changed = true;
            }
            ui.label("to");
            let mut response_end_ms = state.sample_time_ms(controls.response_end);
            let response_end = ui.add(
                egui::DragValue::new(&mut response_end_ms)
                    .range(time_min_ms..=time_max_ms)
                    .speed(time_speed_ms)
                    .suffix(" ms")
                    .max_decimals(1),
            );
            if response_end.changed() {
                controls.response_end = state.sample_index_for_time_ms(response_end_ms);
                changed = true;
            }
            ui.memory_mut(|memory| {
                memory
                    .data
                    .insert_temp(ui.id().with("tc_baseline_preview"), preview_only)
            });
        });

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("tc_measure")
                .selected_text(controls.measure.label())
                .show_ui(ui, |ui| {
                    for choice in [
                        TimeCourseMeasure::Mean,
                        TimeCourseMeasure::PositivePeak,
                        TimeCourseMeasure::NegativePeak,
                        TimeCourseMeasure::AbsolutePeak,
                        TimeCourseMeasure::AreaUnderCurve,
                    ] {
                        changed |= ui
                            .selectable_value(&mut controls.measure, choice, choice.label())
                            .changed();
                    }
                });
            changed |= ui
                .checkbox(&mut controls.threshold_enabled, "Activation threshold")
                .changed();
            changed |= ui
                .add_enabled(
                    controls.threshold_enabled,
                    egui::DragValue::new(&mut controls.threshold_value)
                        .speed(range_drag_speed(self.selected_threshold_range()))
                        .prefix("T ")
                        .custom_formatter(|value, _| scalar_value_label(value as f32))
                        .custom_parser(|text| text.trim().parse::<f64>().ok()),
                )
                .changed();
            changed |= ui
                .add_enabled(
                    controls.threshold_enabled,
                    egui::Checkbox::new(&mut controls.threshold_absolute, "absolute"),
                )
                .changed();
            if ui
                .button("Map window summary")
                .on_hover_text(
                    "Replace the current time-point map with the selected response-window measure",
                )
                .clicked()
            {
                controls.display = TimeCourseDisplay::Window;
                controls.playing = false;
                changed = true;
            }
            ui.label(
                egui::RichText::new("graph: click time · drag response · Shift-drag baseline")
                    .color(muted_color()),
            );
        });

        if controls.playing {
            ui.ctx().request_repaint_after(state.frame_interval);
        }

        ui.add_space(3.0);
        if let Some(snapshot) = self.graph_snapshot.as_ref() {
            if let Some(interaction) = draw_graph_snapshot(
                ui,
                snapshot,
                OverlayColumnSelections::default(),
                Some(state.plot_markers()),
            ) {
                match interaction {
                    TimeCoursePlotInteraction::Cursor(cursor) => {
                        controls.cursor = cursor;
                        controls.display = TimeCourseDisplay::Cursor;
                        controls.playing = false;
                    }
                    TimeCoursePlotInteraction::Baseline { range, commit } => {
                        let [start, end] = range;
                        controls.baseline_start = start;
                        controls.baseline_end = end;
                        ui.memory_mut(|memory| {
                            memory
                                .data
                                .insert_temp(ui.id().with("tc_baseline_preview"), !commit)
                        });
                    }
                    TimeCoursePlotInteraction::Response([start, end]) => {
                        controls.response_start = start;
                        controls.response_end = end;
                    }
                }
                changed = true;
            }
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(18.0);
                ui.label(
                    egui::RichText::new("Right-click a surface node to plot its timecourse")
                        .size(16.0)
                        .color(muted_color()),
                );
            });
        }
        if changed {
            let mut sanitized = controls;
            state.sanitize_controls(&mut sanitized);
            let preview_only = ui.memory(|memory| {
                memory
                    .data
                    .get_temp::<bool>(ui.id().with("tc_baseline_preview"))
                    .unwrap_or(false)
            });
            if preview_only {
                actions.push(ViewerCommand::PreviewTimeCourseControls(sanitized));
            } else {
                actions.push(ViewerCommand::SetTimeCourseControls(sanitized));
            }
        }
    }
}

fn timecourse_values(
    dataset: &Dataset,
    time_columns: &[usize],
    baseline_stats: &[BaselineStats],
    controls: &TimeCourseControls,
    step_seconds: f32,
) -> Vec<f32> {
    (0..dataset.row_count)
        .map(|row| {
            let stats = baseline_stats[row];
            let transform = |value: Option<f32>| {
                value.and_then(|value| transform_timecourse_value(value, stats, controls.baseline))
            };

            if controls.display == TimeCourseDisplay::Cursor {
                let column = &dataset.columns[time_columns[controls.cursor]];
                return transform(numeric_column_value_as_f32(column, row)).unwrap_or(f32::NAN);
            }
            let response = time_columns[controls.response_start..=controls.response_end]
                .iter()
                .filter_map(|index| {
                    transform(numeric_column_value_as_f32(&dataset.columns[*index], row))
                })
                .filter(|value| value.is_finite())
                .collect::<Vec<_>>();
            summarize_response(&response, controls.measure, step_seconds).unwrap_or(f32::NAN)
        })
        .collect()
}

fn transform_timecourse_value(
    value: f32,
    stats: BaselineStats,
    baseline: TimeCourseBaseline,
) -> Option<f32> {
    let value = match baseline {
        TimeCourseBaseline::Raw => value,
        TimeCourseBaseline::SubtractMean => value - stats.mean,
        TimeCourseBaseline::ZScore => {
            let standard_deviation = stats.standard_deviation.filter(|standard_deviation| {
                standard_deviation.is_finite() && *standard_deviation > 0.0
            })?;
            (value - stats.mean) / standard_deviation
        }
    };
    value.is_finite().then_some(value)
}

fn timecourse_condition_ranges(
    conditions: &[TimeCourseCondition],
    baseline: TimeCourseBaseline,
) -> Result<(ValueRange, ValueRange)> {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let capacity = conditions.iter().fold(0_usize, |total, condition| {
        total.saturating_add(
            condition
                .dataset
                .row_count
                .saturating_mul(condition.time_columns.len()),
        )
    });
    let mut magnitudes = Vec::with_capacity(capacity);
    for condition in conditions {
        for row in 0..condition.dataset.row_count {
            let Some(stats) = condition.baseline_stats.get(row).copied() else {
                continue;
            };
            for column_index in &condition.time_columns {
                let Some(value) =
                    numeric_column_value_as_f32(&condition.dataset.columns[*column_index], row)
                        .and_then(|value| transform_timecourse_value(value, stats, baseline))
                else {
                    continue;
                };
                min = min.min(value);
                max = max.max(value);
                magnitudes.push(value.abs());
            }
        }
    }
    ensure!(
        !magnitudes.is_empty(),
        "timecourse has no finite display values"
    );
    let robust_extent = nearest_rank_percentile(&mut magnitudes, 0.995)
        .filter(|extent| *extent > 0.0)
        .unwrap_or_else(|| min.abs().max(max.abs()).max(1.0));
    Ok((
        timecourse_color_range(min, max, min.abs().max(max.abs())),
        timecourse_color_range(min, max, robust_extent),
    ))
}

fn validate_timecourse_condition(
    dataset: &Dataset,
    time_columns: &[usize],
    expected_samples: usize,
    expected_start: f64,
    expected_step: f64,
    expected_domain: &crate::surface::SurfaceDomainId,
    expected_rows: usize,
    expected_nodes: Option<&[u32]>,
) -> Result<()> {
    ensure!(
        time_columns.len() == expected_samples,
        "timecourse conditions must have the same sample count (expected {expected_samples}, found {})",
        time_columns.len()
    );
    let start = dataset.time_start_seconds.unwrap_or(0.0);
    let step = dataset.time_step_seconds.unwrap_or(1.0);
    ensure!(
        (start - expected_start).abs() <= 1.0e-9,
        "timecourse conditions must have the same start time"
    );
    ensure!(
        (step - expected_step).abs() <= 1.0e-9,
        "timecourse conditions must have the same sample interval"
    );
    ensure!(
        &dataset.domain_id == expected_domain
            && dataset.row_count == expected_rows
            && dataset.node_indices.as_deref() == expected_nodes,
        "timecourse conditions must map to the same surface vertices"
    );
    Ok(())
}

fn nearest_rank_percentile(values: &mut [f32], quantile: f32) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let rank = (quantile.clamp(0.0, 1.0) * values.len() as f32).ceil() as usize;
    let index = rank.saturating_sub(1).min(values.len() - 1);
    Some(*values.select_nth_unstable_by(index, f32::total_cmp).1)
}

fn timecourse_color_range(min: f32, max: f32, extent: f32) -> ValueRange {
    if min < 0.0 && max > 0.0 {
        ValueRange {
            min: -extent,
            max: extent,
        }
    } else if max <= 0.0 {
        ValueRange {
            min: -extent,
            max: 0.0,
        }
    } else {
        ValueRange {
            min: 0.0,
            max: extent,
        }
    }
}

fn baseline_statistics(
    dataset: &Dataset,
    time_columns: &[usize],
    start: usize,
    end: usize,
) -> Vec<BaselineStats> {
    (0..dataset.row_count)
        .map(|row| {
            let values = time_columns[start..=end]
                .iter()
                .filter_map(|index| numeric_column_value_as_f32(&dataset.columns[*index], row))
                .collect::<Vec<_>>();
            let mean = finite_mean(&values).unwrap_or(0.0);
            BaselineStats {
                mean,
                standard_deviation: finite_standard_deviation(&values, mean),
            }
        })
        .collect()
}

fn finite_mean(values: &[f32]) -> Option<f32> {
    (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
}

fn finite_standard_deviation(values: &[f32], mean: f32) -> Option<f32> {
    (!values.is_empty()).then(|| {
        (values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f32>()
            / values.len() as f32)
            .sqrt()
    })
}

fn summarize_response(
    values: &[f32],
    measure: TimeCourseMeasure,
    step_seconds: f32,
) -> Option<f32> {
    let first = *values.first()?;
    Some(match measure {
        TimeCourseMeasure::Mean => values.iter().sum::<f32>() / values.len() as f32,
        TimeCourseMeasure::PositivePeak => values.iter().copied().fold(first, f32::max),
        TimeCourseMeasure::NegativePeak => values.iter().copied().fold(first, f32::min),
        TimeCourseMeasure::AbsolutePeak => values.iter().copied().fold(first, |best, value| {
            if value.abs() > best.abs() {
                value
            } else {
                best
            }
        }),
        TimeCourseMeasure::AreaUnderCurve => values
            .windows(2)
            .map(|pair| (pair[0] + pair[1]) * 0.5 * step_seconds)
            .sum(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_locked_dataset() -> Dataset {
        let domain = SurfaceDomain::from_triangles(2, vec![[0, 1, 1]]).unwrap();
        let columns = vec![
            DataColumn::new(
                "-100 ms",
                ColumnRole::TimePoint,
                Some("nAm".to_string()),
                ColumnData::Float32(vec![-1.0, 0.0]),
            )
            .unwrap(),
            DataColumn::new(
                "0 ms",
                ColumnRole::TimePoint,
                Some("nAm".to_string()),
                ColumnData::Float32(vec![1.0, 2.0]),
            )
            .unwrap(),
            DataColumn::new(
                "100 ms",
                ColumnRole::TimePoint,
                Some("nAm".to_string()),
                ColumnData::Float32(vec![3.0, 4.0]),
            )
            .unwrap(),
        ];
        Dataset::dense(DatasetKind::SurfaceTimeSeries, &domain, columns)
            .unwrap()
            .with_time_start_seconds(Some(-0.1))
            .with_time_step_seconds(Some(0.1))
    }

    #[test]
    fn response_summaries_preserve_peak_sign_and_integrate() {
        let values = [-2.0, 1.0, 3.0];
        assert_eq!(
            summarize_response(&values, TimeCourseMeasure::PositivePeak, 0.1),
            Some(3.0)
        );
        assert_eq!(
            summarize_response(&values, TimeCourseMeasure::NegativePeak, 0.1),
            Some(-2.0)
        );
        assert_eq!(
            summarize_response(&values, TimeCourseMeasure::AbsolutePeak, 0.1),
            Some(3.0)
        );
        assert!(
            (summarize_response(&values, TimeCourseMeasure::AreaUnderCurve, 0.1).unwrap() - 0.15)
                .abs()
                < 1.0e-6
        );
    }

    #[test]
    fn robust_scale_rejects_a_single_extreme_sample() {
        let mut magnitudes = vec![1.0; 199];
        magnitudes.push(1_000.0);

        assert_eq!(nearest_rank_percentile(&mut magnitudes, 0.995), Some(1.0));
        assert_eq!(
            timecourse_color_range(-2.0, 4.0, 3.0),
            ValueRange {
                min: -3.0,
                max: 3.0
            }
        );
    }

    #[test]
    fn baseline_z_score_accepts_meg_sized_standard_deviations() {
        let value = transform_timecourse_value(
            1.4e-10,
            BaselineStats {
                mean: 1.0e-10,
                standard_deviation: Some(2.0e-11),
            },
            TimeCourseBaseline::ZScore,
        )
        .unwrap();

        assert!((value - 2.0).abs() < 1.0e-5);
    }

    #[test]
    fn event_locked_defaults_use_pre_event_baseline_and_zero_cursor() {
        let mut state = TimeCourseState::new(event_locked_dataset()).unwrap();
        assert_eq!(state.controls.cursor, 1);
        assert_eq!(state.controls.baseline_start, 0);
        assert_eq!(state.controls.baseline_end, 0);
        assert_eq!(state.controls.baseline, TimeCourseBaseline::SubtractMean);
        assert_eq!(state.sample_value(0, 1), Some(2.0));
        assert_eq!(state.sample_value(0, 2), Some(4.0));

        let cursor = state.display_dataset().unwrap();
        assert_eq!(
            cursor.columns[0].values,
            ColumnData::Float32(vec![2.0, 2.0])
        );

        state.controls.display = TimeCourseDisplay::Window;
        state.controls.measure = TimeCourseMeasure::Mean;
        state.controls.response_start = 1;
        state.controls.response_end = 2;
        let response = state.display_dataset().unwrap();
        assert_eq!(
            response.columns[0].values,
            ColumnData::Float32(vec![3.0, 3.0])
        );
    }

    #[test]
    fn millisecond_controls_snap_to_the_nearest_sample() {
        let state = TimeCourseState::new(event_locked_dataset()).unwrap();

        assert_eq!(state.sample_time_ms(0), -100.0);
        assert_eq!(state.sample_time_ms(2), 100.0);
        assert_eq!(state.sample_step_ms(), 100.0);
        assert_eq!(state.sample_index_for_time_ms(-200.0), 0);
        assert_eq!(state.sample_index_for_time_ms(-51.0), 0);
        assert_eq!(state.sample_index_for_time_ms(-49.0), 1);
        assert_eq!(state.sample_index_for_time_ms(75.0), 2);
        assert_eq!(state.sample_index_for_time_ms(200.0), 2);
    }

    #[test]
    fn multiple_conditions_share_controls_and_can_change_the_active_map() {
        let first = event_locked_dataset();
        let mut second = event_locked_dataset();
        for column in &mut second.columns {
            if let ColumnData::Float32(values) = &mut column.values {
                for value in values {
                    *value *= 10.0;
                }
            }
        }

        let mut state = TimeCourseState::from_conditions(
            vec![
                ("condition A".to_string(), first),
                ("condition B".to_string(), second),
            ],
            0,
        )
        .unwrap();

        assert_eq!(state.condition_count(), 2);
        assert_eq!(state.condition_label(1), Some("condition B"));
        assert_eq!(state.condition_sample_value(0, 0, 1), Some(2.0));
        assert_eq!(state.condition_sample_value(1, 0, 1), Some(20.0));
        state.activate_condition(1).unwrap();
        assert_eq!(state.active_condition(), 1);
        assert_eq!(state.sample_value(0, 1), Some(20.0));
        assert!(state.cursor_full_range.max >= 40.0);
    }

    #[test]
    fn multiple_conditions_reject_mismatched_timing() {
        let first = event_locked_dataset();
        let second = event_locked_dataset().with_time_step_seconds(Some(0.2));

        let error = TimeCourseState::from_conditions(
            vec![
                ("condition A".to_string(), first),
                ("condition B".to_string(), second),
            ],
            0,
        )
        .err()
        .expect("mismatched timing should fail");

        assert!(error.to_string().contains("sample interval"));
    }
}
