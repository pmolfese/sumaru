use anyhow::{Context, Result, ensure};

use crate::color::{ColorMap, ContinuousColorMap, LabelTable};
use crate::dataset::{ColumnData, ColumnRange, DataColumn, Dataset};
use crate::surface::{SurfaceDomain, SurfaceDomainId};

#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    pub dataset_id: Option<String>,
    pub domain_id: SurfaceDomainId,
    pub columns: OverlayColumns,
    pub colormap: ColorMap,
    pub intensity_range: RangeSelection,
    pub threshold: Threshold,
    pub mask_mode: MaskMode,
    pub clip_mode: ClipMode,
    pub symmetric_range: bool,
    pub opacity: f32,
    pub plane_order: i32,
    pub layer_role: OverlayLayerRole,
    pub color_cache: PerNodeColorCache,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayColumns {
    pub intensity: ColumnSelection,
    pub threshold: Option<ColumnSelection>,
    pub brightness: Option<ColumnSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSelection {
    pub index: usize,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RangeSelection {
    Auto,
    Manual(ColumnRange),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Threshold {
    pub mode: ThresholdMode,
    pub range: Option<ColumnRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdMode {
    Off,
    Above,
    Below,
    Between,
    Outside,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaskMode {
    None,
    HideFailedThreshold,
    DimFailedThreshold(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipMode {
    ClampToIntensityRange,
    HideOutsideIntensityRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayLayerRole {
    Foreground,
    Background,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PerNodeColorCache {
    pub colors: Vec<[f32; 4]>,
}

impl Overlay {
    pub fn from_dataset(
        dataset: &Dataset,
        domain: &SurfaceDomain,
        columns: OverlayColumns,
    ) -> Result<Self> {
        let mut overlay = Self::without_color_cache(dataset, domain, columns)?;
        overlay.rebuild_color_cache(dataset, domain)?;

        Ok(overlay)
    }

    /// Builds the overlay with default display settings but an empty color
    /// cache, leaving `rebuild_color_cache` to the caller. Use this when you
    /// will immediately apply display settings (colormap, range, threshold,
    /// opacity) so the cache is only computed once rather than once here with
    /// defaults and again after the settings are applied.
    pub fn without_color_cache(
        dataset: &Dataset,
        domain: &SurfaceDomain,
        mut columns: OverlayColumns,
    ) -> Result<Self> {
        ensure!(
            dataset.domain_id == domain.id,
            "overlay dataset domain does not match target surface domain"
        );
        columns.attach_labels(dataset);

        Ok(Self {
            dataset_id: dataset.parent_ids.source_dataset_id.clone(),
            domain_id: dataset.domain_id.clone(),
            columns,
            colormap: ColorMap::blue_white_red(),
            intensity_range: RangeSelection::Auto,
            threshold: Threshold::off(),
            mask_mode: MaskMode::None,
            clip_mode: ClipMode::ClampToIntensityRange,
            symmetric_range: false,
            opacity: 1.0,
            plane_order: 0,
            layer_role: OverlayLayerRole::Foreground,
            color_cache: PerNodeColorCache::transparent(domain.node_count),
        })
    }

    /// Builds an overlay directly from already-computed per-node colors.
    ///
    /// AFNI's live `SUMA_irgba` messages arrive as sparse RGBA color updates,
    /// not as a full dataset table. Keeping this constructor explicit lets the
    /// live AFNI path display those colors without pretending they are a
    /// canonical `Dataset`.
    pub fn from_color_cache(
        domain: &SurfaceDomain,
        colors: Vec<[f32; 4]>,
        dataset_id: Option<String>,
    ) -> Result<Self> {
        ensure!(
            colors.len() == domain.node_count,
            "overlay color cache length {} does not match domain node count {}",
            colors.len(),
            domain.node_count
        );

        Ok(Self {
            dataset_id,
            domain_id: domain.id.clone(),
            columns: OverlayColumns::new(0),
            colormap: ColorMap::blue_white_red(),
            intensity_range: RangeSelection::Auto,
            threshold: Threshold::off(),
            mask_mode: MaskMode::None,
            clip_mode: ClipMode::ClampToIntensityRange,
            symmetric_range: false,
            opacity: 1.0,
            plane_order: 0,
            layer_role: OverlayLayerRole::Foreground,
            color_cache: PerNodeColorCache { colors },
        })
    }

    pub fn rebuild_color_cache(&mut self, dataset: &Dataset, domain: &SurfaceDomain) -> Result<()> {
        ensure!(
            self.domain_id == dataset.domain_id && dataset.domain_id == domain.id,
            "overlay, dataset, and domain ids do not match"
        );
        ensure!(
            self.opacity.is_finite(),
            "overlay opacity must be a finite value"
        );

        let intensity_column = selected_numeric_column(dataset, &self.columns.intensity)
            .context("overlay intensity column is invalid")?;
        let threshold_column = self
            .columns
            .threshold
            .as_ref()
            .map(|selection| selected_numeric_column(dataset, selection))
            .transpose()
            .context("overlay threshold column is invalid")?;
        let brightness_column = self
            .columns
            .brightness
            .as_ref()
            .map(|selection| selected_numeric_column(dataset, selection))
            .transpose()
            .context("overlay brightness column is invalid")?;
        let brightness_range: Option<ColumnRange> =
            brightness_column.and_then(|column| column.range);
        self.threshold.validate()?;
        let intensity_mapping = match &self.colormap {
            ColorMap::Continuous(colormap) => IntensityColorMapping::Continuous {
                colormap,
                range: self.resolved_intensity_range(intensity_column)?,
            },
            ColorMap::Labels(label_table) => IntensityColorMapping::Labels(label_table),
        };

        let mut colors = vec![[0.0, 0.0, 0.0, 0.0]; domain.node_count];
        let opacity = self.opacity.clamp(0.0, 1.0);

        for row in 0..dataset.row_count {
            let Some(node) = dataset.node_for_row(row) else {
                continue;
            };
            let node = node as usize;
            if node >= colors.len() {
                continue;
            }

            let Some(value) = numeric_value(intensity_column, row) else {
                colors[node] = [0.35, 0.35, 0.35, opacity];
                continue;
            };

            let threshold_value = threshold_column.and_then(|column| numeric_value(column, row));
            let passes_threshold = self.threshold.passes(threshold_value);
            let clipped_out = matches!(
                intensity_mapping,
                IntensityColorMapping::Continuous { range, .. }
                    if self.clip_mode == ClipMode::HideOutsideIntensityRange
                        && !range.contains(value)
            );
            let mut color = match intensity_mapping {
                IntensityColorMapping::Continuous { colormap, range } => {
                    map_value(value, range, colormap)
                }
                IntensityColorMapping::Labels(label_table) => {
                    map_label_value(intensity_column, row, label_table)
                        .unwrap_or([0.35, 0.35, 0.35, opacity])
                }
            };

            if let (Some(column), Some(range)) = (brightness_column, brightness_range)
                && let Some(brightness) = numeric_value(column, row)
            {
                let factor = range.normalized(brightness).clamp(0.0, 1.0) as f32;
                color[0] *= factor;
                color[1] *= factor;
                color[2] *= factor;
            }

            if clipped_out {
                color[3] = 0.0;
            } else {
                color[3] = color[3].clamp(0.0, 1.0) * opacity;
            }

            if !passes_threshold {
                match self.mask_mode {
                    MaskMode::None => {}
                    MaskMode::HideFailedThreshold => color[3] = 0.0,
                    MaskMode::DimFailedThreshold(factor) => {
                        let factor = factor.clamp(0.0, 1.0);
                        color[0] *= factor;
                        color[1] *= factor;
                        color[2] *= factor;
                    }
                }
            }

            colors[node] = color;
        }

        self.color_cache = PerNodeColorCache { colors };

        Ok(())
    }

    pub fn with_colormap(mut self, colormap: ColorMap) -> Self {
        self.colormap = colormap;
        self
    }

    pub fn with_intensity_range(mut self, intensity_range: RangeSelection) -> Self {
        self.intensity_range = intensity_range;
        self
    }

    pub fn with_symmetric_range(mut self, symmetric_range: bool) -> Self {
        self.symmetric_range = symmetric_range;
        self
    }

    pub fn with_threshold(mut self, threshold: Threshold, mask_mode: MaskMode) -> Self {
        self.threshold = threshold;
        self.mask_mode = mask_mode;
        self
    }

    pub fn with_clip_mode(mut self, clip_mode: ClipMode) -> Self {
        self.clip_mode = clip_mode;
        self
    }

    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity.clamp(0.0, 1.0);
        self
    }

    pub fn with_plane_order(mut self, plane_order: i32) -> Self {
        self.plane_order = plane_order;
        self
    }

    pub fn with_layer_role(mut self, layer_role: OverlayLayerRole) -> Self {
        self.layer_role = layer_role;
        self
    }

    fn resolved_intensity_range(&self, column: &DataColumn) -> Result<ColumnRange> {
        let mut range = match self.intensity_range {
            RangeSelection::Auto => column
                .range
                .with_context(|| format!("column {} has no numeric range", column.label))?,
            RangeSelection::Manual(range) => range,
        };
        range.validate("intensity range")?;

        if self.symmetric_range {
            let extent = range.min.abs().max(range.max.abs());
            range = ColumnRange {
                min: -extent,
                max: extent,
            };
        }

        Ok(range)
    }
}

impl OverlayColumns {
    pub fn new(intensity_index: usize) -> Self {
        Self {
            intensity: ColumnSelection::new(intensity_index),
            threshold: None,
            brightness: None,
        }
    }

    pub fn with_threshold(mut self, threshold_index: usize) -> Self {
        self.threshold = Some(ColumnSelection::new(threshold_index));
        self
    }

    pub fn with_brightness(mut self, brightness_index: usize) -> Self {
        self.brightness = Some(ColumnSelection::new(brightness_index));
        self
    }

    fn attach_labels(&mut self, dataset: &Dataset) {
        self.intensity.attach_label(dataset);
        if let Some(selection) = &mut self.threshold {
            selection.attach_label(dataset);
        }
        if let Some(selection) = &mut self.brightness {
            selection.attach_label(dataset);
        }
    }
}

impl ColumnSelection {
    pub fn new(index: usize) -> Self {
        Self { index, label: None }
    }

    fn attach_label(&mut self, dataset: &Dataset) {
        self.label = dataset
            .columns
            .get(self.index)
            .map(|column| column.label.clone());
    }
}

impl Threshold {
    pub fn off() -> Self {
        Self {
            mode: ThresholdMode::Off,
            range: None,
        }
    }

    pub fn above(min: f64) -> Self {
        Self {
            mode: ThresholdMode::Above,
            range: Some(ColumnRange { min, max: min }),
        }
    }

    pub fn below(max: f64) -> Self {
        Self {
            mode: ThresholdMode::Below,
            range: Some(ColumnRange { min: max, max }),
        }
    }

    pub fn between(min: f64, max: f64) -> Self {
        Self {
            mode: ThresholdMode::Between,
            range: Some(ColumnRange { min, max }),
        }
    }

    pub fn outside(min: f64, max: f64) -> Self {
        Self {
            mode: ThresholdMode::Outside,
            range: Some(ColumnRange { min, max }),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.mode == ThresholdMode::Off {
            return Ok(());
        }

        let range = self
            .range
            .context("threshold mode requires a threshold range")?;
        range.validate("threshold range")
    }

    fn passes(&self, value: Option<f64>) -> bool {
        let Some(value) = value else {
            return self.mode == ThresholdMode::Off;
        };
        let Some(range) = self.range else {
            return self.mode == ThresholdMode::Off;
        };

        match self.mode {
            ThresholdMode::Off => true,
            ThresholdMode::Above => value >= range.min,
            ThresholdMode::Below => value <= range.max,
            ThresholdMode::Between => range.contains(value),
            ThresholdMode::Outside => value <= range.min || value >= range.max,
        }
    }
}

impl PerNodeColorCache {
    fn transparent(node_count: usize) -> Self {
        Self {
            colors: vec![[0.0, 0.0, 0.0, 0.0]; node_count],
        }
    }
}

fn selected_numeric_column<'a>(
    dataset: &'a Dataset,
    selection: &ColumnSelection,
) -> Result<&'a DataColumn> {
    let column = dataset
        .columns
        .get(selection.index)
        .with_context(|| format!("column index {} is outside dataset", selection.index))?;
    ensure!(
        column.values.is_numeric(),
        "column {} is not numeric",
        column.label
    );

    Ok(column)
}

fn numeric_value(column: &DataColumn, row: usize) -> Option<f64> {
    match &column.values {
        ColumnData::UInt32(values) => values.get(row).map(|value| *value as f64),
        ColumnData::Int32(values) => values.get(row).map(|value| *value as f64),
        ColumnData::Float32(values) => values
            .get(row)
            .copied()
            .filter(|value| value.is_finite())
            .map(|value| value as f64),
        ColumnData::Float64(values) => values.get(row).copied().filter(|value| value.is_finite()),
        ColumnData::Text(_) => None,
    }
}

fn map_value(value: f64, range: ColumnRange, colormap: &ContinuousColorMap) -> [f32; 4] {
    let normalized = range.normalized(value) as f32;
    colormap.sample(normalized).to_array()
}

fn map_label_value(column: &DataColumn, row: usize, label_table: &LabelTable) -> Option<[f32; 4]> {
    integer_value(column, row).map(|value| label_table.color_for_key(value).to_array())
}

fn integer_value(column: &DataColumn, row: usize) -> Option<i32> {
    match &column.values {
        ColumnData::UInt32(values) => values.get(row).and_then(|value| i32::try_from(*value).ok()),
        ColumnData::Int32(values) => values.get(row).copied(),
        ColumnData::Float32(values) => values
            .get(row)
            .and_then(|value| finite_integer(*value as f64)),
        ColumnData::Float64(values) => values.get(row).and_then(|value| finite_integer(*value)),
        ColumnData::Text(_) => None,
    }
}

fn finite_integer(value: f64) -> Option<i32> {
    (value.is_finite() && value.fract() == 0.0)
        .then_some(value as i64)
        .and_then(|value| i32::try_from(value).ok())
}

#[derive(Clone, Copy)]
enum IntensityColorMapping<'a> {
    Continuous {
        colormap: &'a ContinuousColorMap,
        range: ColumnRange,
    },
    Labels(&'a LabelTable),
}

trait ColumnDataKind {
    fn is_numeric(&self) -> bool;
}

impl ColumnDataKind for ColumnData {
    fn is_numeric(&self) -> bool {
        !matches!(self, Self::Text(_))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClipMode, MaskMode, Overlay, OverlayColumns, OverlayLayerRole, RangeSelection, Threshold,
    };
    use crate::color::{ColorMap, LabelEntry, LabelTable, LabelTableSource, Rgba};
    use crate::dataset::{ColumnData, ColumnRange, ColumnRole, DataColumn, Dataset, DatasetKind};
    use crate::surface::SurfaceDomain;

    #[test]
    fn overlay_builds_dense_color_cache_from_intensity_column() {
        let domain = triangle_domain();
        let dataset = scalar_dataset(&domain);
        let overlay = Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0)).unwrap();

        assert_eq!(overlay.color_cache.colors.len(), domain.node_count);
        assert_color_close(overlay.color_cache.colors[0], [0.1, 0.22, 0.85, 1.0]);
        assert_color_close(overlay.color_cache.colors[1], [1.0, 1.0, 1.0, 1.0]);
        assert_color_close(overlay.color_cache.colors[2], [0.86, 0.08, 0.08, 1.0]);
    }

    #[test]
    fn overlay_keeps_sparse_missing_nodes_transparent() {
        let domain = SurfaceDomain::from_triangles(5, vec![[0, 1, 2]]).unwrap();
        let dataset = Dataset::sparse(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![1, 4],
            vec![
                DataColumn::new(
                    "effect",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![0.0, 1.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let overlay = Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0)).unwrap();

        assert_eq!(overlay.color_cache.colors[0], [0.0, 0.0, 0.0, 0.0]);
        assert_ne!(overlay.color_cache.colors[1][3], 0.0);
        assert_ne!(overlay.color_cache.colors[4][3], 0.0);
    }

    #[test]
    fn overlay_rejects_non_numeric_intensity_column() {
        let domain = SurfaceDomain::from_triangles(2, vec![[0, 1, 0]]).unwrap();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceLabel,
            &domain,
            vec![
                DataColumn::new(
                    "label",
                    ColumnRole::Label,
                    None,
                    ColumnData::Text(vec!["a".to_string(), "b".to_string()]),
                )
                .unwrap(),
            ],
        )
        .unwrap();

        let error = Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0)).unwrap_err();

        assert!(error.to_string().contains("intensity column is invalid"));
    }

    #[test]
    fn overlay_label_colormap_maps_integer_values_to_label_colors() {
        let domain = triangle_domain();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceLabel,
            &domain,
            vec![
                DataColumn::new(
                    "label",
                    ColumnRole::Label,
                    None,
                    ColumnData::Int32(vec![1, 2, 4]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let table = LabelTable::new(
            LabelTableSource::Manual,
            vec![
                LabelEntry::new(1, "1", Rgba::from_u8(0, 194, 255, 255)).unwrap(),
                LabelEntry::new(2, "2", Rgba::from_u8(255, 242, 0, 255)).unwrap(),
                LabelEntry::new(4, "4", Rgba::from_u8(255, 117, 24, 255)).unwrap(),
            ],
        )
        .unwrap();
        let mut overlay = Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0))
            .unwrap()
            .with_colormap(ColorMap::labels(table));

        overlay.rebuild_color_cache(&dataset, &domain).unwrap();

        assert_color_close(
            overlay.color_cache.colors[0],
            [0.0, 194.0 / 255.0, 1.0, 1.0],
        );
        assert_color_close(
            overlay.color_cache.colors[1],
            [1.0, 242.0 / 255.0, 0.0, 1.0],
        );
        assert_color_close(
            overlay.color_cache.colors[2],
            [1.0, 117.0 / 255.0, 24.0 / 255.0, 1.0],
        );
    }

    #[test]
    fn overlay_label_colormap_leaves_unlabeled_zero_values_transparent() {
        let domain = triangle_domain();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceLabel,
            &domain,
            vec![
                DataColumn::new(
                    "label",
                    ColumnRole::Label,
                    None,
                    ColumnData::Int32(vec![0, 1, 2]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let table = LabelTable::new(
            LabelTableSource::Manual,
            vec![
                LabelEntry::new(1, "1", Rgba::from_u8(0, 194, 255, 255)).unwrap(),
                LabelEntry::new(2, "2", Rgba::from_u8(255, 242, 0, 255)).unwrap(),
            ],
        )
        .unwrap();
        let mut overlay = Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0))
            .unwrap()
            .with_colormap(ColorMap::labels(table));

        overlay.rebuild_color_cache(&dataset, &domain).unwrap();

        assert_color_close(overlay.color_cache.colors[0], [0.0, 0.0, 0.0, 0.0]);
        assert_color_close(
            overlay.color_cache.colors[1],
            [0.0, 194.0 / 255.0, 1.0, 1.0],
        );
        assert_color_close(
            overlay.color_cache.colors[2],
            [1.0, 242.0 / 255.0, 0.0, 1.0],
        );
    }

    #[test]
    fn overlay_threshold_can_hide_failed_nodes() {
        let domain = triangle_domain();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![
                DataColumn::new(
                    "effect",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![0.0, 0.5, 1.0]),
                )
                .unwrap(),
                DataColumn::new(
                    "stat",
                    ColumnRole::Threshold,
                    None,
                    ColumnData::Float32(vec![1.0, 3.0, 5.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let mut overlay =
            Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0).with_threshold(1))
                .unwrap()
                .with_threshold(Threshold::above(3.0), MaskMode::HideFailedThreshold);

        overlay.rebuild_color_cache(&dataset, &domain).unwrap();

        assert_eq!(overlay.color_cache.colors[0][3], 0.0);
        assert_eq!(overlay.color_cache.colors[1][3], 1.0);
        assert_eq!(overlay.color_cache.colors[2][3], 1.0);
    }

    #[test]
    fn outside_threshold_includes_boundary_values() {
        let domain = triangle_domain();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![
                DataColumn::new(
                    "effect",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![-1.0, 0.0, 1.0]),
                )
                .unwrap(),
                DataColumn::new(
                    "stat",
                    ColumnRole::Threshold,
                    None,
                    ColumnData::Float32(vec![-2.0, 0.0, 2.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let mut overlay =
            Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0).with_threshold(1))
                .unwrap()
                .with_threshold(Threshold::outside(-2.0, 2.0), MaskMode::HideFailedThreshold);

        overlay.rebuild_color_cache(&dataset, &domain).unwrap();

        assert_eq!(overlay.color_cache.colors[0][3], 1.0);
        assert_eq!(overlay.color_cache.colors[1][3], 0.0);
        assert_eq!(overlay.color_cache.colors[2][3], 1.0);
    }

    #[test]
    fn overlay_can_clip_outside_manual_intensity_range() {
        let domain = triangle_domain();
        let dataset = scalar_dataset(&domain);
        let mut overlay = Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0))
            .unwrap()
            .with_intensity_range(RangeSelection::Manual(ColumnRange {
                min: -0.5,
                max: 0.5,
            }))
            .with_clip_mode(ClipMode::HideOutsideIntensityRange);

        overlay.rebuild_color_cache(&dataset, &domain).unwrap();

        assert_eq!(overlay.color_cache.colors[0][3], 0.0);
        assert_eq!(overlay.color_cache.colors[1][3], 1.0);
        assert_eq!(overlay.color_cache.colors[2][3], 0.0);
    }

    #[test]
    fn overlay_symmetric_range_centers_signed_values() {
        let domain = triangle_domain();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![
                DataColumn::new(
                    "effect",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![-2.0, 0.0, 1.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let overlay = Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0))
            .unwrap()
            .with_symmetric_range(true);
        let mut overlay = overlay;

        overlay.rebuild_color_cache(&dataset, &domain).unwrap();

        assert_color_close(overlay.color_cache.colors[1], [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn overlay_brightness_column_modulates_rgb() {
        let domain = triangle_domain();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![
                DataColumn::new(
                    "effect",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![0.0, 0.5, 1.0]),
                )
                .unwrap(),
                DataColumn::new(
                    "brightness",
                    ColumnRole::Brightness,
                    None,
                    ColumnData::Float32(vec![0.0, 0.5, 1.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let mut overlay =
            Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0).with_brightness(1))
                .unwrap();

        overlay.rebuild_color_cache(&dataset, &domain).unwrap();

        assert_color_close(overlay.color_cache.colors[0], [0.0, 0.0, 0.0, 1.0]);
        assert!(overlay.color_cache.colors[2][0] > overlay.color_cache.colors[1][0]);
    }

    #[test]
    fn overlay_opacity_plane_order_and_layer_role_are_state() {
        let domain = triangle_domain();
        let dataset = scalar_dataset(&domain);
        let mut overlay = Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0))
            .unwrap()
            .with_opacity(0.25)
            .with_plane_order(5)
            .with_layer_role(OverlayLayerRole::Background)
            .with_colormap(ColorMap::grayscale());

        overlay.rebuild_color_cache(&dataset, &domain).unwrap();

        assert_eq!(overlay.opacity, 0.25);
        assert_eq!(overlay.plane_order, 5);
        assert_eq!(overlay.layer_role, OverlayLayerRole::Background);
        assert_eq!(overlay.color_cache.colors[0][3], 0.25);
    }

    #[test]
    fn overlay_validates_domain_match() {
        let first = triangle_domain();
        let second = SurfaceDomain::from_triangles(3, vec![[0, 2, 1]]).unwrap();
        let dataset = scalar_dataset(&first);

        let error = Overlay::from_dataset(&dataset, &second, OverlayColumns::new(0)).unwrap_err();

        assert!(error.to_string().contains("domain does not match"));
    }

    fn scalar_dataset(domain: &SurfaceDomain) -> Dataset {
        Dataset::dense(
            DatasetKind::SurfaceScalar,
            domain,
            vec![
                DataColumn::new(
                    "effect",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![-1.0, 0.0, 1.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    fn triangle_domain() -> SurfaceDomain {
        SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap()
    }

    fn assert_color_close(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.0001);
        }
    }
}
