//! Ordered multi-overlay selection while preserving the existing active-overlay
//! render path.

use super::*;

impl OverlayKey {
    pub(super) fn single(path: &Path) -> Self {
        Self::Single(canonical_or_original_path(path.to_path_buf()))
    }

    pub(super) fn pair(pair: &ExplicitOverlayPair) -> Self {
        Self::Pair {
            left: pair.left_path.clone().map(canonical_or_original_path),
            right: pair.right_path.clone().map(canonical_or_original_path),
        }
    }
}

impl ViewerOverlayState {
    pub(super) fn key(&self) -> Option<OverlayKey> {
        if let Some(id) = self.source.generated_id {
            Some(OverlayKey::Generated(id))
        } else if let Some(pair) = self.source.pair_paths.as_ref() {
            Some(OverlayKey::pair(pair))
        } else {
            self.source.path.as_deref().map(OverlayKey::single)
        }
    }
}

impl ViewerState {
    pub(super) fn overlay_count(&self) -> usize {
        self.overlay_stack.slots.len()
    }

    pub(super) fn active_overlay_index(&self) -> Option<usize> {
        self.overlay_stack.active
    }

    pub(super) fn overlay_label_at(&self, index: usize) -> Option<String> {
        if self.overlay_stack.active == Some(index) {
            return Some(self.overlay.display_text());
        }
        self.overlay_stack
            .slots
            .get(index)
            .and_then(Option::as_ref)
            .map(ViewerOverlayState::display_text)
    }

    /// Reserve/select the ordered slot for a file that has already parsed
    /// successfully. Returns the active threshold state to transfer to the
    /// freshly installed dataset according to user preferences.
    pub(super) fn prepare_overlay_install(
        &mut self,
        key: OverlayKey,
    ) -> Option<OverlayThresholdTransfer> {
        let transfer = self.capture_overlay_threshold_transfer();
        if let Some(existing) = self.overlay_index_for_key(&key) {
            self.swap_active_overlay(existing);
            return if self.preferences.overlay_threshold_sync == OverlayThresholdSync::PerOverlay {
                self.capture_overlay_threshold_transfer()
            } else {
                transfer
            };
        }

        if let Some(active) = self.overlay_stack.active {
            debug_assert!(self.overlay_stack.slots[active].is_none());
            self.overlay_stack.slots[active] = Some(std::mem::take(&mut self.overlay));
        } else {
            // An AFNI live-color overlay is intentionally exclusive and is not
            // added to the file-backed selector.
            self.overlay = ViewerOverlayState::default();
        }
        let next = self.overlay_stack.slots.len();
        self.overlay_stack.slots.push(None);
        self.overlay_stack.active = Some(next);
        if self.preferences.overlay_threshold_sync == OverlayThresholdSync::PerOverlay {
            None
        } else {
            transfer
        }
    }

    pub(super) fn apply_overlay_install_threshold(
        &mut self,
        transfer: Option<OverlayThresholdTransfer>,
    ) {
        let Some(transfer) = transfer else {
            return;
        };
        if self.preferences.overlay_threshold_sync == OverlayThresholdSync::PerOverlay {
            self.overlay.render.appearance.threshold = transfer.threshold;
        } else {
            self.apply_overlay_threshold_transfer(transfer);
        }
    }

    pub(super) fn select_overlay(&mut self, index: usize) -> Result<bool> {
        if self.overlay_stack.active == Some(index) {
            return Ok(false);
        }
        ensure!(
            index < self.overlay_stack.slots.len(),
            "overlay index {index} is out of range"
        );
        let transfer = self.capture_overlay_threshold_transfer();
        self.swap_active_overlay(index);
        if let Some(transfer) = transfer {
            self.apply_overlay_threshold_transfer(transfer);
        }
        self.finish_overlay_switch()?;
        Ok(true)
    }

    pub(super) fn cycle_overlay(&mut self, step: isize) -> Result<bool> {
        let len = self.overlay_stack.slots.len();
        if len <= 1 {
            self.log_status(if len == 0 {
                "No overlays are loaded."
            } else {
                "Only one overlay is loaded."
            });
            return Ok(false);
        }
        let active = self.overlay_stack.active.unwrap_or(0) as isize;
        let next = (active + step).rem_euclid(len as isize) as usize;
        self.select_overlay(next)
    }

    pub(super) fn remove_active_overlay(&mut self) -> Result<bool> {
        let Some(active) = self.overlay_stack.active else {
            return Ok(false);
        };
        let removed = self.overlay.display_text();
        let removed_generated_id = self.overlay.source.generated_id;
        let transfer = self.capture_overlay_threshold_transfer();
        self.overlay_stack.slots.remove(active);

        if self.overlay_stack.slots.is_empty() {
            self.overlay_stack.active = None;
            self.overlay.clear();
            self.controller.surface.current_overlay_path = None;
            self.controller.overlay.visible = false;
            self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
            self.refresh_pick_overlay_value();
            self.upload_surface_buffers();
            self.update_scene_stats();
        } else {
            let next = active.min(self.overlay_stack.slots.len() - 1);
            self.overlay = self.overlay_stack.slots[next]
                .take()
                .expect("inactive overlay slot must own its state");
            self.overlay_stack.active = Some(next);
            if let Some(transfer) = transfer {
                self.apply_overlay_threshold_transfer(transfer);
            }
            self.finish_overlay_switch()?;
        }
        if let Some(output_id) = removed_generated_id {
            self.end_instacorr_for_output(output_id);
        }
        self.log_status(format!("Removed overlay {removed}."));
        Ok(true)
    }

    pub(super) fn reset_overlay_stack_storage(&mut self) {
        self.overlay_stack = ViewerOverlayStack::default();
    }

    pub(super) fn overlay_index_for_key(&self, key: &OverlayKey) -> Option<usize> {
        if self.overlay.key().as_ref() == Some(key) {
            return self.overlay_stack.active;
        }
        self.overlay_stack
            .slots
            .iter()
            .enumerate()
            .find_map(|(index, entry)| {
                (entry.as_ref().and_then(ViewerOverlayState::key).as_ref() == Some(key))
                    .then_some(index)
            })
    }

    fn swap_active_overlay(&mut self, index: usize) {
        if self.overlay_stack.active == Some(index) {
            return;
        }
        let incoming = self.overlay_stack.slots[index]
            .take()
            .expect("inactive overlay slot must own its state");
        if let Some(active) = self.overlay_stack.active {
            self.overlay_stack.slots[active] = Some(std::mem::replace(&mut self.overlay, incoming));
        } else {
            self.overlay = incoming;
        }
        self.overlay_stack.active = Some(index);
    }

    fn capture_overlay_threshold_transfer(&self) -> Option<OverlayThresholdTransfer> {
        self.overlay.is_loaded().then(|| OverlayThresholdTransfer {
            threshold: self.overlay.render.appearance.threshold,
            p_value: self.selected_threshold_stat_spec().and_then(|stat| {
                stat.two_sided_p_value(self.overlay.render.appearance.threshold.value as f64)
            }),
        })
    }

    fn apply_overlay_threshold_transfer(&mut self, transfer: OverlayThresholdTransfer) {
        let destination = self.overlay.render.appearance.threshold;
        let destination_stat = self.selected_threshold_stat_spec();
        self.overlay.render.appearance.threshold = overlay_threshold_after_switch(
            self.preferences.overlay_threshold_sync,
            transfer,
            destination,
            destination_stat.as_ref(),
        );
    }

    fn finish_overlay_switch(&mut self) -> Result<()> {
        self.auto_niml_overlay_active = false;
        self.afni_live_overlay_active = false;
        self.controller.surface.current_overlay_path = self.overlay.source.path.clone();
        self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
        self.refresh_overlay_appearance()?;
        self.refresh_graph_snapshot_if_open();
        self.log_status(format!(
            "Selected overlay {} of {}: {}.",
            self.overlay_stack.active.unwrap_or(0) + 1,
            self.overlay_stack.slots.len(),
            self.overlay.display_text()
        ));
        Ok(())
    }

    pub(super) fn save_preferences(&mut self) {
        let Some(path) = self.preferences_path.as_deref() else {
            self.preferences_status =
                Some("Home directory unavailable; preferences were not saved.".into());
            return;
        };
        self.preferences_status = Some(match self.preferences.save(path) {
            Ok(()) => format!("Saved to {}", path.display()),
            Err(error) => format!("Could not save {}: {error}", path.display()),
        });
    }
}

fn overlay_threshold_after_switch(
    preference: OverlayThresholdSync,
    transfer: OverlayThresholdTransfer,
    destination: OverlayThreshold,
    destination_stat: Option<&AfniStatSpec>,
) -> OverlayThreshold {
    match preference {
        OverlayThresholdSync::PerOverlay => destination,
        OverlayThresholdSync::CurrentValue => transfer.threshold,
        OverlayThresholdSync::MatchPValue => {
            let mut threshold = transfer.threshold;
            if let (Some(p_value), Some(stat)) = (transfer.p_value, destination_stat)
                && let Some(value) = stat.statistic_for_p_value(p_value)
            {
                threshold.value = value as f32;
            }
            threshold
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threshold(value: f32) -> OverlayThreshold {
        OverlayThreshold {
            enabled: true,
            absolute: true,
            value,
            hide_failed: true,
        }
    }

    #[test]
    fn current_value_policy_copies_the_numeric_threshold() {
        let transferred = overlay_threshold_after_switch(
            OverlayThresholdSync::CurrentValue,
            OverlayThresholdTransfer {
                threshold: threshold(2.5),
                p_value: Some(0.05),
            },
            threshold(7.0),
            AfniStatSpec::parse("Ttest(10)").as_ref(),
        );
        assert_eq!(transferred.value, 2.5);
    }

    #[test]
    fn p_value_policy_converts_for_the_destination_stat() {
        let destination_stat = AfniStatSpec::parse("Ttest(48)").unwrap();
        let transferred = overlay_threshold_after_switch(
            OverlayThresholdSync::MatchPValue,
            OverlayThresholdTransfer {
                threshold: threshold(9.0),
                p_value: Some(0.05),
            },
            threshold(7.0),
            Some(&destination_stat),
        );
        assert!((transferred.value - 2.010_635).abs() < 0.000_2);
    }

    #[test]
    fn p_value_policy_falls_back_to_numeric_when_metadata_is_missing() {
        let transferred = overlay_threshold_after_switch(
            OverlayThresholdSync::MatchPValue,
            OverlayThresholdTransfer {
                threshold: threshold(3.25),
                p_value: Some(0.01),
            },
            threshold(7.0),
            None,
        );
        assert_eq!(transferred.value, 3.25);
    }

    #[test]
    fn per_overlay_policy_keeps_the_destination_threshold() {
        let transferred = overlay_threshold_after_switch(
            OverlayThresholdSync::PerOverlay,
            OverlayThresholdTransfer {
                threshold: threshold(3.25),
                p_value: Some(0.01),
            },
            threshold(7.0),
            None,
        );
        assert_eq!(transferred.value, 7.0);
    }
}
