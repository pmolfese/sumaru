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
        self.auto_niml_overlay_active = false;
        self.afni_live_overlay_active = false;
        self.afni_rgba_signatures.clear();
        self.overlay.data = DatasetOverlayState::Loaded {
            canonical_dataset: loaded_overlay.dataset,
            columns: loaded_overlay.columns,
            node_values: overlay_values,
        };
        self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
        self.controller.overlay.visible = true;
        self.overlay.render.appearance = OverlayAppearance::from_range(range);
        self.auto_select_label_colormap();
        self.overlay.render.appearance.symmetric_range = range.min < 0.0 && range.max > 0.0;
        let auto_discrete_labels = self.maybe_apply_discrete_overlay_palette();
        self.overlay.source.path = Some(path.clone());
        self.overlay.source.pair_paths = self.explicit_overlay_pair_for_loaded_path(&path);
        self.overlay.source.label_table = None;
        self.controller.surface.current_overlay_path = Some(path.clone());
        self.overlay.source.display_name = Some(loaded_selection.display_name);
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Loaded overlay range {}. {column_summary}{}",
            value_range_label(range),
            if auto_discrete_labels {
                " Using discrete integer label colors."
            } else {
                ""
            }
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
        self.auto_niml_overlay_active = false;
        self.afni_live_overlay_active = false;
        self.afni_rgba_signatures.clear();
        self.overlay.data = DatasetOverlayState::Loaded {
            canonical_dataset: loaded_overlay.dataset,
            columns: loaded_overlay.columns,
            node_values: overlay_values,
        };
        self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
        self.controller.overlay.visible = true;
        self.overlay.render.appearance = OverlayAppearance::from_range(range);
        self.auto_select_label_colormap();
        self.overlay.render.appearance.symmetric_range = range.min < 0.0 && range.max > 0.0;
        let auto_discrete_labels = self.maybe_apply_discrete_overlay_palette();
        let primary_path = pair
            .primary_path()
            .context("explicit hemisphere overlay selection is empty")?
            .to_path_buf();
        self.overlay.source.path = Some(primary_path.clone());
        self.overlay.source.pair_paths = Some(pair.clone());
        self.overlay.source.label_table = None;
        self.controller.surface.current_overlay_path = Some(primary_path);
        self.overlay.source.display_name = Some(loaded_selection.display_name);
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Loaded hemisphere overlay range {}. {column_summary}{}",
            value_range_label(range),
            if auto_discrete_labels {
                " Using discrete integer label colors."
            } else {
                ""
            }
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

    /// Load same-stem `.niml.dset` files beside the active surface components.
    pub(super) fn load_auto_niml_overlay_for_active_surface(&mut self) -> Result<bool> {
        if !self.auto_color_niml {
            return Ok(false);
        }

        let Some(mesh) = self.mesh.as_ref() else {
            return Ok(false);
        };
        let domain = mesh.domain.clone();
        let node_count = mesh.vertices.len();
        let components = self.active_auto_niml_components()?;
        if components.is_empty() {
            return Ok(false);
        }

        let mut datasets = Vec::new();
        let mut display_paths = Vec::new();
        let mut label_table = None;
        let mut node_offset = 0u32;
        let mut missing = 0usize;
        for (surface_path, component_mesh) in components {
            let Some(overlay_path) = matching_niml_dset_path(&surface_path) else {
                node_offset = node_offset.saturating_add(component_mesh.vertices.len() as u32);
                missing += 1;
                continue;
            };
            let (dataset, component_label_table) =
                read_niml_dataset_with_label_table(&overlay_path, &component_mesh.domain)
                    .with_context(|| format!("failed to load {}", overlay_path.display()))?;
            if label_table.is_none() {
                label_table = component_label_table;
            }
            datasets.push(single_hemisphere_overlay_dataset(
                dataset,
                &domain,
                node_offset,
            )?);
            display_paths.push(overlay_path);
            node_offset = node_offset.saturating_add(component_mesh.vertices.len() as u32);
        }

        if datasets.is_empty() {
            if self.auto_niml_overlay_active {
                self.clear_auto_niml_overlay();
            }
            self.log_status("No matching same-stem .niml.dset overlays found.");
            return Ok(false);
        }

        let dataset = combine_auto_niml_datasets(datasets, &domain)?;
        let loaded_overlay = loaded_overlay_from_dataset(dataset, node_count, "auto NIML")?;
        self.install_auto_niml_overlay(loaded_overlay, display_paths, label_table, missing)?;

        Ok(true)
    }

    fn active_auto_niml_components(&self) -> Result<Vec<(PathBuf, SurfaceMesh)>> {
        if let Some(scene) = self.surface_scene.as_ref() {
            let surface = scene
                .surfaces
                .get(scene.active_index)
                .context("active scene surface is outside loaded scene")?;
            return surface
                .components
                .iter()
                .map(|component| {
                    Ok((
                        component.path.clone(),
                        component.mesh.clone().with_context(|| {
                            format!("surface {} is still loading", component.name)
                        })?,
                    ))
                })
                .collect();
        }

        let path = self
            .surface_path
            .clone()
            .context("no active surface path is loaded")?;
        let mesh = self
            .mesh
            .clone()
            .context("no active surface mesh is loaded")?;

        Ok(vec![(path, mesh)])
    }

    fn install_auto_niml_overlay(
        &mut self,
        loaded_overlay: LoadedOverlay,
        display_paths: Vec<PathBuf>,
        label_table: Option<LabelTable>,
        missing: usize,
    ) -> Result<()> {
        let column_summary =
            overlay_column_summary(&loaded_overlay.dataset, loaded_overlay.columns);
        let overlay_values = loaded_overlay.overlay_values;
        let range = overlay_values.range;
        let primary_path = display_paths.first().cloned();
        let display_name = auto_niml_display_name(&display_paths);

        self.overlay.clear();
        self.auto_niml_overlay_active = true;
        self.afni_live_overlay_active = false;
        self.afni_rgba_signatures.clear();
        self.overlay.data = DatasetOverlayState::Loaded {
            canonical_dataset: loaded_overlay.dataset,
            columns: loaded_overlay.columns,
            node_values: overlay_values,
        };
        self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
        self.controller.overlay.visible = true;
        self.overlay.render.appearance = OverlayAppearance::from_range(range);
        self.auto_select_label_colormap();
        self.overlay.render.appearance.symmetric_range = range.min < 0.0 && range.max > 0.0;
        self.overlay.source.path = primary_path.clone();
        self.overlay.source.pair_paths = None;
        self.overlay.source.label_table = label_table;
        self.controller.surface.current_overlay_path = primary_path;
        self.overlay.source.display_name = Some(display_name);
        let auto_discrete_labels = self.maybe_apply_discrete_overlay_palette();
        self.rebuild_overlay_model()?;
        self.apply_initial_overlay_options(
            self.auto_niml_overlay_subs.clone().as_deref(),
            self.auto_niml_overlay_p_value,
        )?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Auto-loaded {} matching .niml.dset overlay{} range {}. {column_summary}{}{}",
            display_paths.len(),
            if display_paths.len() == 1 { "" } else { "s" },
            value_range_label(range),
            if auto_discrete_labels {
                " Using discrete integer label colors."
            } else {
                ""
            },
            if missing > 0 {
                format!(
                    " ({missing} surface{} had no match.)",
                    if missing == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            }
        ));

        Ok(())
    }

    fn clear_auto_niml_overlay(&mut self) {
        self.overlay.clear();
        self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
        self.auto_niml_overlay_active = false;
        self.afni_live_overlay_active = false;
        self.afni_rgba_signatures.clear();
        self.controller.surface.current_overlay_path = None;
        self.controller.overlay.visible = false;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
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
        self.overlay_data_generation = self.overlay_data_generation.wrapping_add(1);
        self.overlay.render.appearance.range = if self.overlay.render.appearance.symmetric_range {
            symmetric_value_range(range)
        } else {
            range
        };
        let auto_discrete_labels = self.maybe_apply_discrete_overlay_palette();
        self.sanitize_overlay_appearance();
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Overlay columns: {column_summary}{}",
            if auto_discrete_labels {
                " Using discrete integer label colors."
            } else {
                ""
            }
        ));

        Ok(())
    }

    fn maybe_apply_discrete_overlay_palette(&mut self) -> bool {
        let Some(dataset) = self.overlay.data.dataset() else {
            return false;
        };
        let intensity_index = self.overlay.data.columns().intensity;
        if resolved_overlay_label_table(
            dataset,
            intensity_index,
            self.overlay.source.label_table.as_ref(),
        )
        .is_some()
        {
            self.overlay.render.appearance.colormap = OverlayColorMap::DiscreteLabels;
            self.overlay.render.appearance.symmetric_range = false;
            true
        } else {
            if self.overlay.render.appearance.colormap == OverlayColorMap::DiscreteLabels {
                self.overlay.render.appearance.colormap = OverlayColorMap::SpectrumRedToBlue;
            }
            false
        }
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

    /// Selects the discrete-label colormap when a freshly loaded overlay looks
    /// like a label map rather than a continuous statistic.
    ///
    /// Cluster maps — whether written here, by `SurfClust`, or by
    /// `3dClusterize` — are integer ranks, and a continuous ramp renders them
    /// badly: adjacent ranks get near-identical colors and rank 0 paints the
    /// whole background. Discrete labels give each cluster its own color, and
    /// the label table's unlabeled color is transparent, so rank 0 drops out
    /// and the anatomy shows through.
    ///
    /// `auto_overlay_label_table` supplies the detection, including its cap on
    /// distinct values, so a genuinely continuous integer column stays on a
    /// continuous map. This runs only on load, never on rebuild, so it cannot
    /// override a colormap the user picked.
    fn auto_select_label_colormap(&mut self) {
        let intensity = self.overlay.data.columns().intensity;
        let Some(dataset) = self.overlay.data.dataset() else {
            return;
        };
        if auto_overlay_label_table(dataset, intensity).is_some() {
            self.overlay.render.appearance.colormap = OverlayColorMap::DiscreteLabels;
        }
    }

    /// Rebuild the per-node overlay color model and re-upload colors.
    pub(super) fn rebuild_overlay_model(&mut self) -> Result<()> {
        // Labels have to be current before colors are built, since cluster
        // rejection is applied inside the color cache.
        self.refresh_cluster_labels();
        let cluster_labels = self.cluster_labels.clone();
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
        let intensity_index = columns.intensity.index;
        let (threshold, mask_mode) =
            threshold_and_mask_from_appearance(self.overlay.render.appearance);
        // Build with an empty cache, apply the real display settings, then
        // compute the color cache exactly once (from_dataset would compute it a
        // first time with default settings and throw that away).
        let mut overlay = Overlay::without_color_cache(dataset, domain, columns)?
            .with_colormap(resolved_overlay_color_map(
                dataset,
                intensity_index,
                self.overlay.render.appearance.colormap,
                self.overlay.source.label_table.as_ref(),
            ))
            .with_intensity_range(RangeSelection::Manual(overlay_range_from_value_range(
                self.overlay.render.appearance.range,
            )))
            .with_symmetric_range(self.overlay.render.appearance.symmetric_range)
            .with_threshold(threshold, mask_mode)
            .with_cluster_labels(cluster_labels)
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

fn matching_niml_dset_path(surface_path: &Path) -> Option<PathBuf> {
    if !is_gifti_path(surface_path) {
        return None;
    }
    let candidate = surface_path.with_extension("niml.dset");
    candidate.exists().then_some(candidate)
}

fn combine_auto_niml_datasets(
    mut datasets: Vec<Dataset>,
    domain: &SurfaceDomain,
) -> Result<Dataset> {
    let mut combined = datasets
        .drain(..1)
        .next()
        .context("no auto NIML datasets were loaded")?;
    for dataset in datasets {
        combined = append_sparse_overlay_dataset(combined, dataset, domain)?;
    }

    Ok(combined)
}

fn append_sparse_overlay_dataset(
    left: Dataset,
    right: Dataset,
    domain: &SurfaceDomain,
) -> Result<Dataset> {
    ensure!(
        left.columns.len() == right.columns.len(),
        "auto NIML overlays have different column counts: {} vs {}",
        left.columns.len(),
        right.columns.len()
    );
    let kind = if left.kind == right.kind {
        left.kind.clone()
    } else {
        DatasetKind::Unknown
    };
    let parent_ids = if left.parent_ids == right.parent_ids {
        left.parent_ids.clone()
    } else {
        DatasetParentIds::default()
    };
    let left_row_count = left.row_count;
    let right_row_count = right.row_count;
    let left_node_indices = left.node_indices.clone();
    let right_node_indices = right.node_indices.clone();
    let columns = left
        .columns
        .into_iter()
        .zip(right.columns)
        .map(|(left, right)| paired_data_column(left, right))
        .collect::<Result<Vec<_>>>()?;
    let mut node_indices = Vec::with_capacity(left_row_count + right_row_count);
    if let Some(indices) = left_node_indices {
        node_indices.extend(indices);
    } else {
        node_indices.extend(0..left_row_count as u32);
    }
    if let Some(indices) = right_node_indices {
        node_indices.extend(indices);
    } else {
        node_indices.extend(0..right_row_count as u32);
    }

    Dataset::sparse(kind, domain, node_indices, columns)
        .map(|dataset| dataset.with_parent_ids(parent_ids))
        .context("failed to combine auto NIML overlay datasets")
}

fn auto_niml_display_name(paths: &[PathBuf]) -> String {
    match paths {
        [] => "auto .niml.dset".to_string(),
        [path] => file_name_display(path),
        paths => format!("{} auto .niml.dset overlays", paths.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_niml_dset_path_finds_same_stem_gifti_match() {
        let dir =
            std::env::temp_dir().join(format!("sumaru_auto_niml_match_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let surface = dir.join("region.k9.gii");
        let overlay = dir.join("region.k9.niml.dset");
        std::fs::write(&surface, b"").unwrap();
        std::fs::write(&overlay, b"").unwrap();

        assert_eq!(matching_niml_dset_path(&surface), Some(overlay.clone()));
        assert_eq!(matching_niml_dset_path(&dir.join("region.k9.txt")), None);

        let _ = std::fs::remove_file(surface);
        let _ = std::fs::remove_file(overlay);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn combine_auto_niml_datasets_preserves_component_node_offsets() {
        let mesh = SurfaceMesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
                [4.0, 0.0, 0.0],
            ],
            vec![[0, 1, 2], [2, 3, 4]],
        )
        .unwrap();
        let left = Dataset::sparse(
            DatasetKind::SurfaceLabel,
            &mesh.domain,
            vec![0, 1],
            vec![
                DataColumn::new(
                    "label",
                    ColumnRole::Label,
                    None,
                    ColumnData::Int32(vec![9, 9]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let right = Dataset::sparse(
            DatasetKind::SurfaceLabel,
            &mesh.domain,
            vec![3, 4],
            vec![
                DataColumn::new(
                    "label",
                    ColumnRole::Label,
                    None,
                    ColumnData::Int32(vec![10, 10]),
                )
                .unwrap(),
            ],
        )
        .unwrap();

        let combined = combine_auto_niml_datasets(vec![left, right], &mesh.domain).unwrap();

        assert_eq!(combined.node_indices.as_deref(), Some(&[0, 1, 3, 4][..]));
        assert_eq!(combined.row_count, 4);
        assert_eq!(combined.columns.len(), 1);
        assert_eq!(
            combined.columns[0].values,
            ColumnData::Int32(vec![9, 9, 10, 10])
        );
    }
}
