//! Overlay loading and column/appearance refresh: loading single and paired
//! overlay files, applying initial CLI overlay options, resolving column
//! selections, and rebuilding the overlay render model. Extracted from
//! `viewer/mod.rs`; all methods stay on `ViewerState`.

use super::*;

/// A freshly loaded overlay before it is installed onto the viewer: the
/// per-node values ready for rendering, the canonical dataset they were derived
/// from (kept so columns can be re-resolved without re-reading the file), and
/// the column selections used to build them.
#[derive(Debug, Clone)]
pub(super) struct LoadedOverlay {
    pub(super) overlay_values: OverlayDataset,
    pub(super) dataset: Dataset,
    pub(super) columns: OverlayColumnSelections,
}

/// A `LoadedOverlay` paired with the human-facing name to show for it (the file
/// stem, or a combined label for a left/right hemisphere pair).
#[derive(Debug, Clone)]
pub(super) struct LoadedOverlaySelection {
    pub(super) overlay: LoadedOverlay,
    pub(super) display_name: String,
}

/// The two hemisphere files that make up a paired overlay, plus the display
/// name inferred for the pair as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PairedOverlayPaths {
    pub(super) left_path: PathBuf,
    pub(super) right_path: PathBuf,
    pub(super) display_name: String,
}

/// Which dataset sub-bricks drive each overlay channel: `intensity` is the
/// colored value (always present), while `threshold` and `brightness` are
/// optional masking and modulation columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct OverlayColumnSelections {
    pub(super) intensity: usize,
    pub(super) threshold: Option<usize>,
    pub(super) brightness: Option<usize>,
}

/// One entry in an overlay column picker: the dataset column index, its label,
/// and whether it holds numeric (selectable) data.
#[derive(Debug, Clone)]
pub(super) struct OverlayColumnOption {
    pub(super) index: usize,
    pub(super) label: String,
    pub(super) is_numeric: bool,
}

impl ViewerState {
    /// Load a single overlay file onto the current surface.
    pub(super) fn load_overlay_path(&mut self, path: PathBuf) -> Result<()> {
        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before loading an overlay")?;
        let loaded_selection = self
            .load_overlay_selection(&path, mesh)
            .with_context(|| format!("failed to load overlay {}", path.display()))?;
        let loaded_overlay = loaded_selection.overlay;
        let column_summary =
            overlay_column_summary(&loaded_overlay.dataset, loaded_overlay.columns);
        let overlay_values = loaded_overlay.overlay_values;
        let range = overlay_values.range;

        self.overlay.clear();
        self.afni_rgba_colors = None;
        self.afni_rgba_signatures.clear();
        self.overlay.data = DatasetOverlayState::Loaded {
            canonical_dataset: loaded_overlay.dataset,
            columns: loaded_overlay.columns,
            node_values: overlay_values,
        };
        self.controller.overlay.visible = true;
        self.overlay.render.appearance = OverlayAppearance::from_range(range);
        self.overlay.render.appearance.symmetric_range = range.min < 0.0 && range.max > 0.0;
        self.overlay.source.path = Some(path.clone());
        self.overlay.source.pair_paths = self.explicit_overlay_pair_for_loaded_path(&path);
        self.controller.surface.current_overlay_path = Some(path.clone());
        self.overlay.source.display_name = Some(loaded_selection.display_name);
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Loaded overlay range {}. {column_summary}",
            value_range_label(range)
        ));

        Ok(())
    }

    /// Load an explicit hemisphere overlay selection onto a paired scene.
    pub(super) fn load_overlay_pair_paths(&mut self, pair: ExplicitOverlayPair) -> Result<()> {
        let mesh = self
            .mesh
            .as_ref()
            .context("load a both-hemisphere scene before loading hemisphere overlays")?;
        let loaded_selection = self
            .load_explicit_paired_overlay_selection(&pair, mesh)
            .with_context(|| {
                format!(
                    "failed to load hemisphere overlays {}",
                    explicit_overlay_pair_display_name(&pair)
                )
            })?;
        let loaded_overlay = loaded_selection.overlay;
        let column_summary =
            overlay_column_summary(&loaded_overlay.dataset, loaded_overlay.columns);
        let overlay_values = loaded_overlay.overlay_values;
        let range = overlay_values.range;

        self.overlay.clear();
        self.afni_rgba_colors = None;
        self.afni_rgba_signatures.clear();
        self.overlay.data = DatasetOverlayState::Loaded {
            canonical_dataset: loaded_overlay.dataset,
            columns: loaded_overlay.columns,
            node_values: overlay_values,
        };
        self.controller.overlay.visible = true;
        self.overlay.render.appearance = OverlayAppearance::from_range(range);
        self.overlay.render.appearance.symmetric_range = range.min < 0.0 && range.max > 0.0;
        let primary_path = pair
            .primary_path()
            .context("explicit hemisphere overlay selection is empty")?
            .to_path_buf();
        self.overlay.source.path = Some(primary_path.clone());
        self.overlay.source.pair_paths = Some(pair.clone());
        self.controller.surface.current_overlay_path = Some(primary_path);
        self.overlay.source.display_name = Some(loaded_selection.display_name);
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Loaded hemisphere overlay range {}. {column_summary}",
            value_range_label(range)
        ));

        Ok(())
    }

    /// Apply CLI-provided sub-brick selectors and p-value to the loaded overlay.
    pub(super) fn apply_initial_overlay_options(
        &mut self,
        subs: Option<&[String]>,
        p_value: Option<f64>,
    ) -> Result<()> {
        if let Some(subs) = subs {
            let dataset = self
                .overlay
                .data
                .dataset()
                .context("no overlay dataset is loaded")?;
            let resolved = resolve_overlay_subs(dataset, subs)?;
            self.overlay.data.set_columns(resolved);
            self.refresh_overlay_columns()?;
        }

        if let Some(p_value) = p_value {
            self.apply_initial_overlay_p_value(p_value)?;
            self.refresh_overlay_appearance()?;
        }

        Ok(())
    }

    /// Set the initial threshold from a p-value if the column carries a stat.
    pub(super) fn apply_initial_overlay_p_value(&mut self, p_value: f64) -> Result<()> {
        let Some(dataset) = self.overlay.data.dataset() else {
            return Ok(());
        };
        let Some(threshold_index) = self.overlay.data.columns().threshold else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} requested, but no T sub-brick is selected"
            ));
            return Ok(());
        };
        let Some(column) = dataset.columns.get(threshold_index) else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} requested, but T sub-brick #{threshold_index} does not exist"
            ));
            return Ok(());
        };
        let Some(stat_label) = column.stat.as_deref() else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} requested, but T sub-brick #{} '{}' has no stat metadata",
                threshold_index, column.label
            ));
            return Ok(());
        };
        let Some(stat) = AfniStatSpec::parse(stat_label) else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} requested, but stat metadata '{stat_label}' is not supported"
            ));
            return Ok(());
        };
        let Some(threshold_value) = stat.statistic_for_p_value(p_value) else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} could not be converted with stat metadata '{stat_label}'"
            ));
            return Ok(());
        };

        self.overlay.render.appearance.threshold.enabled = true;
        self.overlay.render.appearance.threshold.absolute = true;
        self.overlay.render.appearance.threshold.value = threshold_value as f32;
        self.sanitize_overlay_appearance();
        self.log_status(format!(
            "Initial threshold p <= {p_value:.4} -> T {:.4}.",
            self.overlay.render.appearance.threshold.value
        ));

        Ok(())
    }

    /// Log a warning and leave thresholding off when an initial option fails.
    pub(super) fn warn_and_disable_initial_threshold(&mut self, message: String) {
        eprintln!("sumaru warning: {message}; threshold disabled.");
        self.overlay.render.appearance.threshold.enabled = false;
    }

    /// Build the overlay model from the current column selections.
    pub(super) fn load_overlay_selection(
        &self,
        path: &Path,
        mesh: &SurfaceMesh,
    ) -> Result<LoadedOverlaySelection> {
        if let Some((left, right)) = self.active_paired_components()
            && let Some(paths) = paired_overlay_paths(path)
        {
            let left_mesh = left
                .mesh
                .as_ref()
                .context("left hemisphere surface is still loading")?;
            let right_mesh = right
                .mesh
                .as_ref()
                .context("right hemisphere surface is still loading")?;
            ensure!(
                paths.left_path.exists(),
                "left hemisphere overlay {} does not exist",
                paths.left_path.display()
            );
            ensure!(
                paths.right_path.exists(),
                "right hemisphere overlay {} does not exist",
                paths.right_path.display()
            );

            let left_dataset = load_dataset_from_path(&paths.left_path, left_mesh)
                .with_context(|| format!("failed to load {}", paths.left_path.display()))?;
            let right_dataset = load_dataset_from_path(&paths.right_path, right_mesh)
                .with_context(|| format!("failed to load {}", paths.right_path.display()))?;
            let dataset = paired_overlay_dataset(
                left_dataset,
                right_dataset,
                &mesh.domain,
                left_mesh.vertices.len() as u32,
            )?;
            let overlay = loaded_overlay_from_dataset(dataset, mesh.vertices.len(), "paired NIML")?;

            return Ok(LoadedOverlaySelection {
                overlay,
                display_name: paths.display_name,
            });
        }

        Ok(LoadedOverlaySelection {
            overlay: load_overlay_from_path(path, mesh)?,
            display_name: file_name_display(path),
        })
    }

    /// Build the overlay model for an explicit hemisphere selection.
    pub(super) fn load_explicit_paired_overlay_selection(
        &self,
        pair: &ExplicitOverlayPair,
        mesh: &SurfaceMesh,
    ) -> Result<LoadedOverlaySelection> {
        let (left, right) = self
            .active_paired_components()
            .context("--overlay-lh/--overlay-rh require an active both-hemisphere spec")?;
        let left_mesh = left
            .mesh
            .as_ref()
            .context("left hemisphere surface is still loading")?;
        let right_mesh = right
            .mesh
            .as_ref()
            .context("right hemisphere surface is still loading")?;
        let dataset = match (&pair.left_path, &pair.right_path) {
            (Some(left_path), Some(right_path)) => {
                ensure!(
                    left_path.exists(),
                    "left hemisphere overlay {} does not exist",
                    left_path.display()
                );
                ensure!(
                    right_path.exists(),
                    "right hemisphere overlay {} does not exist",
                    right_path.display()
                );

                let left_dataset = load_dataset_from_path(left_path, left_mesh)
                    .with_context(|| format!("failed to load {}", left_path.display()))?;
                let right_dataset = load_dataset_from_path(right_path, right_mesh)
                    .with_context(|| format!("failed to load {}", right_path.display()))?;
                paired_overlay_dataset(
                    left_dataset,
                    right_dataset,
                    &mesh.domain,
                    left_mesh.vertices.len() as u32,
                )?
            }
            (Some(left_path), None) => {
                ensure!(
                    left_path.exists(),
                    "left hemisphere overlay {} does not exist",
                    left_path.display()
                );
                let left_dataset = load_dataset_from_path(left_path, left_mesh)
                    .with_context(|| format!("failed to load {}", left_path.display()))?;
                single_hemisphere_overlay_dataset(left_dataset, &mesh.domain, 0)?
            }
            (None, Some(right_path)) => {
                ensure!(
                    right_path.exists(),
                    "right hemisphere overlay {} does not exist",
                    right_path.display()
                );
                let right_dataset = load_dataset_from_path(right_path, right_mesh)
                    .with_context(|| format!("failed to load {}", right_path.display()))?;
                single_hemisphere_overlay_dataset(
                    right_dataset,
                    &mesh.domain,
                    left_mesh.vertices.len() as u32,
                )?
            }
            (None, None) => bail!("no hemisphere overlay path was provided"),
        };
        let overlay =
            loaded_overlay_from_dataset(dataset, mesh.vertices.len(), "hemisphere overlay")?;

        Ok(LoadedOverlaySelection {
            overlay,
            display_name: explicit_overlay_pair_display_name(pair),
        })
    }

    /// Infer the opposite-hemisphere overlay file for a loaded path.
    pub(super) fn explicit_overlay_pair_for_loaded_path(
        &self,
        path: &Path,
    ) -> Option<ExplicitOverlayPair> {
        self.active_paired_components()?;
        let paths = paired_overlay_paths(path)?;
        Some(ExplicitOverlayPair {
            left_path: Some(paths.left_path),
            right_path: Some(paths.right_path),
        })
    }

    /// Re-resolve intensity/threshold/brightness columns after a change.
    pub(super) fn refresh_overlay_columns(&mut self) -> Result<()> {
        let dataset = self
            .overlay
            .data
            .dataset()
            .context("no canonical overlay dataset is loaded")?;
        let domain = &self
            .mesh
            .as_ref()
            .context("load a surface before selecting overlay columns")?
            .domain;
        let overlay = overlay_dataset_from_canonical_dataset(
            dataset,
            domain.node_count,
            self.overlay.data.columns(),
        )?;
        let range = overlay.range;
        let column_summary = overlay_column_summary(dataset, self.overlay.data.columns());
        self.overlay.data.set_node_values(overlay);
        self.overlay.render.appearance.range = if self.overlay.render.appearance.symmetric_range {
            symmetric_value_range(range)
        } else {
            range
        };
        self.sanitize_overlay_appearance();
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!("Overlay columns: {column_summary}"));

        Ok(())
    }

    /// Recompute overlay appearance defaults from the selected columns.
    pub(super) fn refresh_overlay_appearance(&mut self) -> Result<()> {
        if !self.overlay.data.is_loaded() {
            return Ok(());
        }

        self.sanitize_overlay_appearance();
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();

        Ok(())
    }

    /// Rebuild the per-node overlay color model and re-upload colors.
    pub(super) fn rebuild_overlay_model(&mut self) -> Result<()> {
        let dataset = self
            .overlay
            .data
            .dataset()
            .context("no canonical overlay dataset is loaded")?;
        let domain = &self
            .mesh
            .as_ref()
            .context("load a surface before rebuilding overlay colors")?
            .domain;
        let columns = canonical_overlay_columns(
            self.overlay.data.columns(),
            self.overlay.render.appearance.threshold.enabled,
        );
        let (threshold, mask_mode) =
            threshold_and_mask_from_appearance(self.overlay.render.appearance);
        // Build with an empty cache, apply the real display settings, then
        // compute the color cache exactly once (from_dataset would compute it a
        // first time with default settings and throw that away).
        let mut overlay = Overlay::without_color_cache(dataset, domain, columns)?
            .with_colormap(self.overlay.render.appearance.colormap.to_color_map())
            .with_intensity_range(RangeSelection::Manual(overlay_range_from_value_range(
                self.overlay.render.appearance.range,
            )))
            .with_symmetric_range(self.overlay.render.appearance.symmetric_range)
            .with_threshold(threshold, mask_mode)
            .with_opacity(self.overlay.render.appearance.opacity);

        overlay.rebuild_color_cache(dataset, domain)?;
        self.overlay.render.render_model = Some(overlay);

        Ok(())
    }

    /// Toggle the active overlay on or off (key `O`).
    pub(super) fn toggle_overlay_visibility(&mut self) {
        if !self.overlay.is_loaded() {
            self.log_status("No overlay is loaded.");
            return;
        }

        self.controller.overlay.visible = !self.controller.overlay.visible;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(if self.controller.overlay.visible {
            "Overlay visible."
        } else {
            "Overlay hidden."
        });
    }
}
