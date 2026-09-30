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
            Self::Cursor => "Cursor",
            Self::Window => "Window result",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeCourseBaseline {
    Raw,
    SubtractMean,
    ZScore,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TimeCoursePlotInteraction {
    Cursor(usize),
    Baseline([usize; 2]),
    Response([usize; 2]),
}

pub(super) struct TimeCourseState {
    pub(super) source_dataset: Dataset,
    pub(super) time_columns: Vec<usize>,
    times_seconds: Vec<f64>,
    baseline_stats: Vec<BaselineStats>,
    pub(super) controls: TimeCourseControls,
    next_frame_at: Instant,
    frame_interval: Duration,
}

#[derive(Debug, Clone, Copy)]
struct BaselineStats {
    mean: f32,
    standard_deviation: Option<f32>,
}

impl TimeCourseState {
    fn new(dataset: Dataset) -> Result<Self> {
        let time_columns = dataset
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

        let step = dataset.time_step_seconds.unwrap_or(1.0);
        let start = dataset.time_start_seconds.unwrap_or(0.0);
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
            measure: TimeCourseMeasure::PositivePeak,
            threshold_enabled: false,
            threshold_absolute: true,
            threshold_value: 0.0,
            playing: false,
        };
        let baseline_stats = baseline_statistics(
            &dataset,
            &time_columns,
            controls.baseline_start,
            controls.baseline_end,
        );
        Ok(Self {
            source_dataset: dataset,
            time_columns,
            times_seconds,
            baseline_stats,
            controls,
            next_frame_at: Instant::now() + frame_interval,
            frame_interval,
        })
    }

    pub(super) fn sample_count(&self) -> usize {
        self.time_columns.len()
    }

    pub(super) fn sample_time(&self, index: usize) -> f64 {
        self.times_seconds[index.min(self.times_seconds.len() - 1)]
    }

    pub(super) fn sample_value(&self, row: usize, sample: usize) -> Option<f32> {
        let column = self
            .source_dataset
            .columns
            .get(*self.time_columns.get(sample)?)?;
        let value = numeric_column_value_as_f32(column, row)?;
        let stats = *self.baseline_stats.get(row)?;
        let value = match self.controls.baseline {
            TimeCourseBaseline::Raw => value,
            TimeCourseBaseline::SubtractMean => value - stats.mean,
            TimeCourseBaseline::ZScore => {
                let standard_deviation = stats
                    .standard_deviation
                    .filter(|standard_deviation| *standard_deviation > f32::EPSILON)?;
                (value - stats.mean) / standard_deviation
            }
        };
        value.is_finite().then_some(value)
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
        let dataset = self
            .overlay
            .data
            .dataset()
            .cloned()
            .context("timecourse mode requires a loaded 3D+time overlay")?;
        ensure!(
            dataset.kind == DatasetKind::SurfaceTimeSeries
                || dataset
                    .columns
                    .iter()
                    .any(|column| column.role == ColumnRole::TimePoint),
            "the overlay is not a surface time series"
        );
        self.timecourse = Some(TimeCourseState::new(dataset)?);
        self.set_graph_window_open(true);
        self.apply_timecourse_overlay()?;
        let state = self
            .timecourse
            .as_ref()
            .expect("timecourse was initialized");
        self.log_status(format!(
            "Timecourse mode: {} samples, {:.1} to {:.1} ms.",
            state.sample_count(),
            state.sample_time(0) * 1000.0,
            state.sample_time(state.sample_count() - 1) * 1000.0,
        ));
        Ok(())
    }

    pub(super) fn set_timecourse_controls(
        &mut self,
        mut controls: TimeCourseControls,
    ) -> Result<()> {
        let state = self
            .timecourse
            .as_mut()
            .context("timecourse mode is not active")?;
        state.sanitize_controls(&mut controls);
        let baseline_changed = controls.baseline_start != state.controls.baseline_start
            || controls.baseline_end != state.controls.baseline_end;
        let starting_playback = controls.playing && !state.controls.playing;
        state.controls = controls;
        if baseline_changed {
            state.baseline_stats = baseline_statistics(
                &state.source_dataset,
                &state.time_columns,
                state.controls.baseline_start,
                state.controls.baseline_end,
            );
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

    fn apply_timecourse_overlay(&mut self) -> Result<()> {
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

        self.overlay.data = DatasetOverlayState::Loaded {
            canonical_dataset: dataset,
            columns,
            node_values,
        };
        self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
        self.overlay.source.label_table = None;
        self.overlay.render.appearance.colormap = OverlayColorMap::SpectrumRedToBlue;
        self.overlay.render.appearance.range = symmetric_value_range(range);
        self.overlay.render.appearance.symmetric_range = range.min < 0.0 && range.max > 0.0;
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
            changed |= ui
                .add(
                    egui::Slider::new(&mut controls.cursor, 0..=last)
                        .show_value(false)
                        .text("Time"),
                )
                .changed();
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
                            "Cursor",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(
                            &mut controls.display,
                            TimeCourseDisplay::Window,
                            "Window result",
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
        });

        ui.horizontal(|ui| {
            ui.label("Baseline");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut controls.baseline_start)
                        .range(0..=state.sample_count() - 1),
                )
                .changed();
            ui.label("to");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut controls.baseline_end)
                        .range(0..=state.sample_count() - 1),
                )
                .changed();
            ui.monospace(format!(
                "({:.1}..{:.1} ms)",
                state.sample_time(controls.baseline_start) * 1000.0,
                state.sample_time(controls.baseline_end) * 1000.0
            ));
            ui.separator();
            ui.label("Response");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut controls.response_start)
                        .range(0..=state.sample_count() - 1),
                )
                .changed();
            ui.label("to");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut controls.response_end)
                        .range(0..=state.sample_count() - 1),
                )
                .changed();
            ui.monospace(format!(
                "({:.1}..{:.1} ms)",
                state.sample_time(controls.response_start) * 1000.0,
                state.sample_time(controls.response_end) * 1000.0
            ));
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
                        .speed(0.01)
                        .prefix("T "),
                )
                .changed();
            changed |= ui
                .add_enabled(
                    controls.threshold_enabled,
                    egui::Checkbox::new(&mut controls.threshold_absolute, "absolute"),
                )
                .changed();
            if ui.button("Show result").clicked() {
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
                    TimeCoursePlotInteraction::Baseline([start, end]) => {
                        controls.baseline_start = start;
                        controls.baseline_end = end;
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
            actions.push(ViewerCommand::SetTimeCourseControls(sanitized));
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
                value.map(|value| match controls.baseline {
                    TimeCourseBaseline::Raw => value,
                    TimeCourseBaseline::SubtractMean => value - stats.mean,
                    TimeCourseBaseline::ZScore => stats
                        .standard_deviation
                        .filter(|sd| *sd > f32::EPSILON)
                        .map_or(f32::NAN, |sd| (value - stats.mean) / sd),
                })
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
}
