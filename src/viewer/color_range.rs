// Developed by P. Molfese, National Institutes of Health (NIH).
//
// This software is a "work of the United States Government" prepared by a federal
// employee as part of official duties. As such, it is not subject to copyright
// protection within the United States (17 U.S.C. § 105). International copyrights
// may apply.
//
// See LICENSE for the full public-domain notice and disclaimer.

//! Shared colorbar and controller range editing. Percentiles use finite samples
//! from the selected intensity column, including zeros, without densifying sparse data.
use super::*;

pub(super) fn sanitize_color_range(appearance: &mut OverlayAppearance) {
    // Automatic ranges include zero for constant data; explicitly locked
    // endpoints remain exact, including a constant range's midpoint mapping.
    if !appearance.range_locked {
        appearance.range = sanitized_color_range(appearance.range, appearance.symmetric_range);
        return;
    }
    let range = &mut appearance.range;
    if !range.min.is_finite() || !range.max.is_finite() {
        *range = DEFAULT_OVERLAY_RANGE;
    }
    if appearance.symmetric_range {
        let extent = range.min.abs().max(range.max.abs());
        *range = ValueRange {
            min: -extent,
            max: extent,
        };
    } else if range.min > range.max {
        std::mem::swap(&mut range.min, &mut range.max);
    }
    // A constant column maps to the midpoint. Do not widen small intervals:
    // floating-point epsilon is not a minimum meaningful data magnitude.
}

/// Apply a column/timecourse update without disturbing a dataset's manual range.
pub(super) fn apply_automatic_range(
    appearance: &mut OverlayAppearance,
    range: ValueRange,
    symmetric: Option<bool>,
) {
    if !appearance.range_locked {
        appearance.range = range;
        if let Some(symmetric) = symmetric {
            appearance.symmetric_range = symmetric;
        }
        sanitize_color_range(appearance);
    }
}

pub(super) fn endpoint_label(value: f32) -> String {
    if value != 0.0 && (value.abs() < 0.001 || value.abs() >= 100_000.0) {
        format!("{value:e}")
    } else {
        value.to_string()
    }
}

fn percentile(column: &DataColumn, percent: f64) -> Option<f32> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return None;
    }
    let mut values: Vec<_> = (0..column.values.len())
        .filter_map(|row| numeric_column_value_as_f32(column, row))
        .collect();
    if values.is_empty() {
        return None;
    }
    let rank = (percent / 100.0 * values.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(values.len() - 1);
    Some(*values.select_nth_unstable_by(index, f32::total_cmp).1)
}

fn parse_endpoint(text: &str, column: Option<&DataColumn>) -> Option<f64> {
    let text = text.trim();
    let value = if let Some(percent) = text.strip_suffix('%') {
        f64::from(percentile(column?, percent.trim().parse().ok()?)?)
    } else {
        text.parse::<f64>().ok()?
    };
    (value.is_finite() && (value as f32).is_finite()).then_some(value)
}

fn data_range(column: &DataColumn) -> Option<ValueRange> {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for value in (0..column.values.len()).filter_map(|row| numeric_column_value_as_f32(column, row))
    {
        min = min.min(value);
        max = max.max(value);
    }
    min.is_finite().then_some(ValueRange { min, max })
}

fn set_range(appearance: &mut OverlayAppearance, range: ValueRange) {
    appearance.range = range;
    appearance.range_locked = true;
    sanitize_color_range(appearance);
}

pub(super) fn draw_range_presets(
    ui: &mut egui::Ui,
    appearance: &mut OverlayAppearance,
    column: Option<&DataColumn>,
) -> bool {
    let mut changed = false;
    ui.add_enabled_ui(column.is_some(), |ui| {
        if ui
            .button("Fit to data")
            .on_hover_text("Use the full range of the current intensity column")
            .clicked()
            && let Some(range) = column.and_then(data_range)
        {
            set_range(appearance, range);
            changed = true;
        }
        if ui
            .button("Robust 2–98%")
            .on_hover_text(
                "Use nearest-rank percentiles of finite intensity samples, including zeros",
            )
            .clicked()
            && let Some(column) = column
            && let (Some(min), Some(max)) = (percentile(column, 2.0), percentile(column, 98.0))
        {
            set_range(appearance, ValueRange { min, max });
            changed = true;
        }
    });
    changed
}

pub(super) fn draw_range_editor(
    ui: &mut egui::Ui,
    appearance: &mut OverlayAppearance,
    column: Option<&DataColumn>,
) -> bool {
    let mut edited = false;
    ui.label("Color range");
    ui.horizontal(|ui| {
        edited |= ui
            .checkbox(&mut appearance.symmetric_range, "Symmetric about zero")
            .changed();
    });
    let range = appearance.range;
    let span = (f64::from(range.max) - f64::from(range.min)).abs();
    let magnitude = f64::from(range.max).abs().max(f64::from(range.min).abs());
    let speed = if span > 0.0 {
        span / 200.0
    } else if magnitude > 0.0 {
        magnitude / 200.0
    } else {
        0.01
    };
    let endpoint = |ui: &mut egui::Ui, value: &mut f32, label: &str| {
        ui.add(egui::DragValue::new(value)
            .speed(speed)
            // egui otherwise caps drag rounding at 15 decimal places, which
            // turns small scientific values into zero even with a custom formatter.
            .min_decimals(45)
            .max_decimals(45)
            .update_while_editing(false)
            .prefix(label)
            .custom_formatter(|value, _| endpoint_label(value as f32))
            .custom_parser(|text| parse_endpoint(text, column)))
            .on_hover_text("Click to type a value, scientific notation (1e-8), or a percentile (98%). Drag to adjust.")
            .changed()
    };
    ui.horizontal(|ui| {
        if appearance.symmetric_range {
            let mut extent = range.min.abs().max(range.max.abs());
            if endpoint(ui, &mut extent, "+/- ") {
                let extent = extent.abs();
                appearance.range = ValueRange {
                    min: -extent,
                    max: extent,
                };
                edited = true;
            }
        } else {
            edited |= endpoint(ui, &mut appearance.range.min, "min ");
            edited |= endpoint(ui, &mut appearance.range.max, "max ");
        }
    });
    if edited {
        appearance.range_locked = true;
        sanitize_color_range(appearance);
    }
    let lock_changed = ui.checkbox(&mut appearance.range_locked, "Lock range for this dataset")
        .on_hover_text("Keep this range when changing intensity columns or playing timecourses. Unlock to resume automatic ranges on the next column or timecourse update.")
        .changed();
    edited || lock_changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(values: Vec<f32>) -> DataColumn {
        DataColumn::new(
            "intensity",
            ColumnRole::Intensity,
            None,
            ColumnData::Float32(values),
        )
        .unwrap()
    }

    #[test]
    fn colorbar_context_menu_fits_data_without_changing_threshold() {
        let ctx = egui::Context::default();
        let mut appearance = OverlayAppearance::from_range(ValueRange {
            min: 0.0,
            max: 10.0,
        });
        appearance.symmetric_range = false;
        appearance.threshold.value = 4.0;
        let column = column(vec![1e-9, 2e-9, 3e-9]);
        let mut frame = |events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 500.0),
                    )),
                    events,
                    ..Default::default()
                },
                |root_ui| {
                    egui::CentralPanel::default().show(root_ui, |ui| {
                        vertical_threshold_bar(
                            ui,
                            &mut appearance,
                            ValueRange {
                                min: 0.0,
                                max: 10.0,
                            },
                            Some(&column),
                        );
                    });
                },
            );
            // This headless test deliberately does not upload textures.
            output.textures_delta.clear();
            output
        };
        let click = |pos, button, pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let pos = egui::pos2(20.0, 50.0);
        frame(vec![]);
        frame(vec![
            egui::Event::PointerMoved(pos),
            click(pos, egui::PointerButton::Secondary, true),
        ]);
        frame(vec![click(pos, egui::PointerButton::Secondary, false)]);
        let output = frame(vec![]);
        fn find_label(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                egui::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| find_label(shape, label))
                }
                _ => None,
            }
        }
        let fit = output
            .shapes
            .iter()
            .find_map(|shape| find_label(&shape.shape, "Fit to data"))
            .expect("right-click menu exposes Fit to data");
        frame(vec![
            egui::Event::PointerMoved(fit),
            click(fit, egui::PointerButton::Primary, true),
        ]);
        frame(vec![click(fit, egui::PointerButton::Primary, false)]);
        assert_eq!(appearance.range.min, 1e-9);
        assert_eq!(appearance.range.max, 3e-9);
        assert!(appearance.range_locked);
        assert_eq!(appearance.threshold.value, 4.0);
        assert!(!appearance.threshold.enabled);
    }

    #[test]
    fn endpoint_entry_supports_scientific_notation_and_finite_percentiles() {
        let column = column(vec![f32::NAN, -2e-9, 0.0, 1e-9, 4e-9, f32::INFINITY]);
        assert_eq!(parse_endpoint(" 1e-12 ", None), Some(1e-12));
        assert_eq!(
            parse_endpoint("0%", Some(&column)),
            Some(f64::from(-2e-9_f32))
        );
        assert_eq!(parse_endpoint("50%", Some(&column)), Some(0.0));
        assert_eq!(
            parse_endpoint("98%", Some(&column)),
            Some(f64::from(4e-9_f32))
        );
        for text in ["nan", "inf", "1e100", "-1%", "101%", "oops"] {
            assert_eq!(parse_endpoint(text, Some(&column)), None, "{text}");
        }
        assert_eq!(parse_endpoint("50%", None), None);
        assert_eq!(percentile(&self::column(vec![f32::NAN]), 50.0), None);
    }

    #[test]
    fn locked_range_survives_columns_and_playback_until_unlocked() {
        let mut appearance = OverlayAppearance::from_range(ValueRange {
            min: -1.0,
            max: 1.0,
        });
        set_range(
            &mut appearance,
            ValueRange {
                min: -1e-9,
                max: 1e-9,
            },
        );
        for range in [
            ValueRange {
                min: 0.0,
                max: 42.0,
            },
            ValueRange {
                min: -7.0,
                max: 2.0,
            },
        ] {
            apply_automatic_range(&mut appearance, range, Some(false));
            assert_eq!(appearance.range.max, 1e-9);
            assert!(appearance.symmetric_range);
        }
        appearance.range_locked = false;
        apply_automatic_range(
            &mut appearance,
            ValueRange {
                min: 2e-20,
                max: 3e-20,
            },
            Some(false),
        );
        assert_eq!(appearance.range.min, 2e-20);
        assert_eq!(appearance.range.max, 3e-20);
        assert!(!appearance.symmetric_range);
    }

    #[test]
    fn automatic_constant_ranges_include_zero_but_manual_ranges_stay_exact() {
        let mut appearance = OverlayAppearance::from_range(ValueRange { min: 0.0, max: 1.0 });
        for value in [-1e-20_f32, 0.0, 1e-20] {
            let constant = ValueRange {
                min: value,
                max: value,
            };
            appearance.range_locked = false;
            apply_automatic_range(&mut appearance, constant, Some(false));
            assert!(appearance.range.min < appearance.range.max);
            assert!(appearance.range.min <= 0.0 && appearance.range.max >= 0.0);
            set_range(&mut appearance, constant);
            sanitize_color_range(&mut appearance);
            assert_eq!(appearance.range, constant);
        }
    }

    #[test]
    fn magnitude_scaling_respects_manual_range_and_resumes_when_unlocked() {
        let mut appearance = OverlayAppearance::from_range(ValueRange {
            min: -2.0,
            max: 2.0,
        });
        let signed = appearance.range;
        set_range(&mut appearance, signed);
        appearance.colormap = OverlayColorMap::Hot;
        let magnitude = ValueRange { min: 0.0, max: 5.0 };
        apply_automatic_range(&mut appearance, magnitude, Some(false));
        assert_eq!(appearance.range, signed);
        assert!(appearance.symmetric_range);
        appearance.range_locked = false;
        apply_automatic_range(&mut appearance, magnitude, Some(false));
        assert_eq!(appearance.range, magnitude);
        assert!(!appearance.symmetric_range);
        assert_eq!(appearance.colormap, OverlayColorMap::Hot);
    }

    #[test]
    fn endpoint_format_preserves_small_values_for_editing() {
        for value in [1.234567e-20_f32, -9.876543e-8, 0.00010001, 0.0, 1e30] {
            let text = endpoint_label(value);
            assert_eq!(parse_endpoint(&text, None).unwrap() as f32, value);
        }
    }

    #[test]
    fn tiny_manual_and_constant_ranges_are_not_widened() {
        let mut appearance = OverlayAppearance::from_range(ValueRange { min: 0.0, max: 1.0 });
        set_range(
            &mut appearance,
            ValueRange {
                min: -1e-20,
                max: 3e-20,
            },
        );
        assert_eq!(appearance.range.min, -3e-20);
        assert_eq!(appearance.range.max, 3e-20);
        assert!(appearance.range_locked);
        appearance.symmetric_range = false;
        set_range(
            &mut appearance,
            ValueRange {
                min: 2e-20,
                max: 1e-20,
            },
        );
        assert_eq!(appearance.range.min, 1e-20);
        assert_eq!(appearance.range.max, 2e-20);
        let range = data_range(&column(vec![1e-20, 1e-20])).unwrap();
        set_range(&mut appearance, range);
        assert_eq!(appearance.range.min, appearance.range.max);
    }
}
