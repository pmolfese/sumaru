use std::sync::Arc;

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
    /// Per-node cluster labels from [`crate::cluster::label_clusters`], zero
    /// meaning "not in a surviving cluster".
    ///
    /// Held as a plain array rather than a mesh reference so this module stays
    /// free of geometry: the viewer owns the topology and does the labeling.
    pub cluster_labels: Option<Arc<Vec<u32>>>,
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
    FadeFailedThreshold(FadeSettings),
}

/// Settings for transparent thresholding ("A"). `curve` and `width` define the
/// opacity ramp; `max_alpha` and `desaturate` control how legible the threshold
/// boundary itself is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FadeSettings {
    pub curve: FadeCurve,
    pub width: FadeWidth,
    /// Ceiling on sub-threshold opacity. The opacity ramp is continuous at the
    /// boundary and nearly flat just below it (quadratic fade puts 0.9T at
    /// alpha 0.81 and 0.99T at 0.98), so without a ceiling there is no visible
    /// step between passing and failing nodes. Capping sub-threshold alpha
    /// reintroduces one. AFNI does the same in its volumetric path with a fixed
    /// 222/255; here it is adjustable.
    pub max_alpha: f32,
    /// How far to pull failing colors toward their own luminance, scaled by how
    /// far they fell below threshold. Opacity alone is a weak cue because a
    /// faded warm color over a grey surface still reads as that color; moving
    /// saturation in step with opacity separates the two populations much more
    /// strongly. `0.0` reproduces AFNI, which fades opacity only.
    pub desaturate: f32,
    /// How far to darken failing colors, scaled by how far they fell below
    /// threshold.
    ///
    /// This is the third perceptual channel, independent of both opacity and
    /// saturation, and it matters most where opacity is weakest: fading a
    /// bright color toward a light anatomical surface barely moves luminance,
    /// so a near-threshold value stays as loud as one that passed. Darkening
    /// makes failing regions recede rather than merely thin out.
    pub darken: f32,
    /// How far to push *passing* colors away from their own luminance, widening
    /// the same gap from the other side.
    ///
    /// This can only do work where the color has saturation headroom left. A
    /// color already at the edge of the gamut cannot be pushed further, and
    /// there `darken` and the contour carry the separation instead.
    pub boost: f32,
}

/// AFNI's volumetric sub-threshold alpha ceiling, 222/255.
pub const AFNI_SUBTHRESHOLD_MAX_ALPHA: f32 = 222.0 / 255.0;

impl FadeSettings {
    /// Sumaru's default: an AFNI-shaped ramp plus a slightly firmer ceiling and
    /// moderate desaturation, which makes the threshold readable without `B`.
    pub fn new() -> Self {
        Self {
            curve: FadeCurve::Quadratic,
            width: FadeWidth::BoundaryMagnitude,
            max_alpha: 0.85,
            desaturate: 0.5,
            darken: 0.35,
            boost: 0.0,
        }
    }

    /// Exactly AFNI's transparent thresholding: quadratic fade to zero, the
    /// 222/255 ceiling, and no desaturation.
    pub fn afni() -> Self {
        Self {
            curve: FadeCurve::Quadratic,
            width: FadeWidth::BoundaryMagnitude,
            max_alpha: AFNI_SUBTHRESHOLD_MAX_ALPHA,
            desaturate: 0.0,
            darken: 0.0,
            boost: 0.0,
        }
    }
}

impl Default for FadeSettings {
    fn default() -> Self {
        Self::new()
    }
}

/// Exponent applied to the sub-threshold opacity ramp. Steeper curves pull the
/// near-threshold band down harder, which is where the quadratic default is
/// nearly flat and hardest to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeCurve {
    Linear,
    Quadratic,
    Cubic,
    Quartic,
}

impl FadeCurve {
    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Quadratic => "Quadratic",
            Self::Cubic => "Cubic",
            Self::Quartic => "Quartic",
        }
    }

    /// Steepest first, so the strongest separation reads at the top of the list.
    pub const ALL: [Self; 4] = [Self::Quartic, Self::Cubic, Self::Quadratic, Self::Linear];

    fn apply(self, ratio: f32) -> f32 {
        match self {
            Self::Linear => ratio,
            Self::Quadratic => ratio * ratio,
            Self::Cubic => ratio * ratio * ratio,
            Self::Quartic => (ratio * ratio) * (ratio * ratio),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FadeWidth {
    /// Fade over the magnitude of the nearest threshold boundary. This is
    /// equivalent to AFNI's fade-to-zero behavior for Above(T) and
    /// Outside(-T, T).
    BoundaryMagnitude,
    /// Fade over an explicit distance in threshold data units.
    Absolute(f64),
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
            cluster_labels: None,
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
            cluster_labels: None,
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

            if passes_threshold {
                if let MaskMode::FadeFailedThreshold(fade) = self.mask_mode {
                    apply_threshold_boost(&mut color, fade.boost);
                }
            } else {
                match self.mask_mode {
                    MaskMode::None => {}
                    MaskMode::HideFailedThreshold => color[3] = 0.0,
                    MaskMode::DimFailedThreshold(factor) => {
                        let factor = factor.clamp(0.0, 1.0);
                        color[0] *= factor;
                        color[1] *= factor;
                        color[2] *= factor;
                    }
                    MaskMode::FadeFailedThreshold(fade) => {
                        apply_threshold_fade(
                            &mut color,
                            self.threshold
                                .opacity_factor(threshold_value, fade.curve, fade.width),
                            fade,
                        );
                    }
                }
            }

            // Cluster rejection is applied last and unconditionally. A node
            // dropped for being in a too-small cluster *passed* the node-wise
            // threshold, so its fade ramp is 1.0 and the fade path would leave
            // it fully opaque. It needs its own rule, not the ramp.
            if let Some(labels) = self.cluster_labels.as_ref()
                && labels.get(node).copied().unwrap_or(0) == 0
            {
                color[3] = 0.0;
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

    /// Restricts the overlay to surviving clusters. `None` clears the
    /// restriction.
    pub fn with_cluster_labels(mut self, labels: Option<Arc<Vec<u32>>>) -> Self {
        self.cluster_labels = labels;
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

    /// Public form of [`Threshold::passes`] for callers holding a plain value,
    /// such as cluster labeling over a per-node scalar array.
    pub fn passes_value(&self, value: f64) -> bool {
        self.passes(Some(value))
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

    /// Returns the threshold opacity multiplier for transparent thresholding.
    /// Passing values always return 1. Failed values fade with their distance
    /// from the nearest boundary of the passing region.
    pub fn opacity_factor(&self, value: Option<f64>, curve: FadeCurve, width: FadeWidth) -> f32 {
        if self.mode == ThresholdMode::Off {
            return 1.0;
        }
        let Some(value) = value.filter(|value| value.is_finite()) else {
            return 0.0;
        };
        let Some(range) = self.range else {
            return 0.0;
        };
        if self.passes(Some(value)) {
            return 1.0;
        }

        let (distance, boundary) = match self.mode {
            ThresholdMode::Off => return 1.0,
            ThresholdMode::Above => (range.min - value, range.min),
            ThresholdMode::Below => (value - range.max, range.max),
            ThresholdMode::Between if value < range.min => (range.min - value, range.min),
            ThresholdMode::Between => (value - range.max, range.max),
            ThresholdMode::Outside => {
                let to_min = value - range.min;
                let to_max = range.max - value;
                if to_min <= to_max {
                    (to_min, range.min)
                } else {
                    (to_max, range.max)
                }
            }
        };
        let fade_width = match width {
            FadeWidth::BoundaryMagnitude => boundary.abs(),
            FadeWidth::Absolute(width) => width,
        };
        if !distance.is_finite() || distance < 0.0 || !fade_width.is_finite() || fade_width <= 0.0 {
            return 0.0;
        }

        let ratio = (1.0 - distance / fade_width).clamp(0.0, 1.0) as f32;
        curve.apply(ratio)
    }
}

/// Applies a sub-threshold fade to `color` in place.
///
/// `factor` is the raw opacity ramp from [`Threshold::opacity_factor`], which is
/// deliberately kept pure so it can be tested against AFNI's formula directly.
/// The ceiling and desaturation are layered on here.
fn apply_threshold_fade(color: &mut [f32; 4], factor: f32, fade: FadeSettings) {
    let max_alpha = if fade.max_alpha.is_finite() {
        fade.max_alpha.clamp(0.0, 1.0)
    } else {
        1.0
    };
    // Only failing nodes reach this path, so the ceiling never touches a value
    // that passed the threshold.
    let factor = factor.clamp(0.0, 1.0).min(max_alpha);
    color[3] *= factor;

    // How far this node fell short, which scales both color adjustments.
    let shortfall = 1.0 - factor;

    let desaturate = clamped_unit(fade.desaturate);
    if desaturate > 0.0 {
        // Rec. 709 luminance, so desaturating on its own preserves perceived
        // brightness rather than dragging everything toward mid grey.
        let luminance = rec709_luminance(*color);
        let amount = shortfall * desaturate;
        for channel in &mut color[..3] {
            *channel += (luminance - *channel) * amount;
        }
    }

    let darken = clamped_unit(fade.darken);
    if darken > 0.0 {
        let scale = 1.0 - shortfall * darken;
        for channel in &mut color[..3] {
            *channel *= scale;
        }
    }
}

/// Pushes a passing color away from its own luminance, so suprathreshold nodes
/// gain separation from the faded context around them.
///
/// Saturation is the only axis with headroom here — passing nodes are already
/// fully opaque and at full brightness — so a color sitting at the edge of the
/// gamut is left unchanged by design.
fn apply_threshold_boost(color: &mut [f32; 4], boost: f32) {
    let boost = clamped_unit(boost);
    if boost <= 0.0 {
        return;
    }

    let luminance = rec709_luminance(*color);
    for channel in &mut color[..3] {
        *channel = (luminance + (*channel - luminance) * (1.0 + boost)).clamp(0.0, 1.0);
    }
}

fn rec709_luminance(color: [f32; 4]) -> f32 {
    0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
}

fn clamped_unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
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
        AFNI_SUBTHRESHOLD_MAX_ALPHA, ClipMode, FadeCurve, FadeSettings, FadeWidth, MaskMode,
        Overlay, OverlayColumns, OverlayLayerRole, RangeSelection, Threshold,
        apply_threshold_boost, apply_threshold_fade,
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
    fn transparent_threshold_matches_afni_for_supported_threshold_shapes() {
        let above = Threshold::above(2.0);
        let outside = Threshold::outside(-2.0, 2.0);

        for (value, linear, quadratic) in [
            (0.0, 0.0, 0.0),
            (0.5, 0.25, 0.0625),
            (1.0, 0.5, 0.25),
            (2.0, 1.0, 1.0),
            (3.0, 1.0, 1.0),
        ] {
            assert_close(
                above.opacity_factor(Some(value), FadeCurve::Linear, FadeWidth::BoundaryMagnitude),
                linear,
            );
            assert_close(
                above.opacity_factor(
                    Some(value),
                    FadeCurve::Quadratic,
                    FadeWidth::BoundaryMagnitude,
                ),
                quadratic,
            );
            assert_close(
                outside.opacity_factor(
                    Some(value),
                    FadeCurve::Quadratic,
                    FadeWidth::BoundaryMagnitude,
                ),
                quadratic,
            );
            assert_close(
                outside.opacity_factor(
                    Some(-value),
                    FadeCurve::Quadratic,
                    FadeWidth::BoundaryMagnitude,
                ),
                quadratic,
            );
        }
    }

    #[test]
    fn transparent_threshold_extends_to_between_and_asymmetric_outside_modes() {
        let between = Threshold::between(2.0, 4.0);
        assert_close(
            between.opacity_factor(Some(1.0), FadeCurve::Linear, FadeWidth::BoundaryMagnitude),
            0.5,
        );
        assert_close(
            between.opacity_factor(Some(5.0), FadeCurve::Linear, FadeWidth::BoundaryMagnitude),
            0.75,
        );
        assert_close(
            between.opacity_factor(Some(3.0), FadeCurve::Linear, FadeWidth::BoundaryMagnitude),
            1.0,
        );

        let outside = Threshold::outside(-2.0, 4.0);
        assert_close(
            outside.opacity_factor(Some(0.0), FadeCurve::Linear, FadeWidth::BoundaryMagnitude),
            0.0,
        );
        assert_close(
            outside.opacity_factor(Some(3.0), FadeCurve::Linear, FadeWidth::BoundaryMagnitude),
            0.75,
        );
    }

    #[test]
    fn transparent_threshold_hides_missing_non_finite_and_zero_width_failures() {
        let threshold = Threshold::above(0.0);
        for value in [None, Some(f64::NAN), Some(-1.0)] {
            assert_eq!(
                threshold
                    .opacity_factor(value, FadeCurve::Quadratic, FadeWidth::BoundaryMagnitude,),
                0.0
            );
        }
        assert_eq!(
            threshold.opacity_factor(Some(-1.0), FadeCurve::Linear, FadeWidth::Absolute(0.0),),
            0.0
        );
    }

    #[test]
    fn transparent_threshold_multiplies_colormap_and_overlay_alpha() {
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
                    ColumnData::Float32(vec![0.0, 1.0, 2.0]),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let mut overlay =
            Overlay::from_dataset(&dataset, &domain, OverlayColumns::new(0).with_threshold(1))
                .unwrap()
                .with_threshold(
                    Threshold::above(2.0),
                    MaskMode::FadeFailedThreshold(FadeSettings::afni()),
                )
                .with_opacity(0.4);

        overlay.rebuild_color_cache(&dataset, &domain).unwrap();

        assert_close(overlay.color_cache.colors[0][3], 0.0);
        assert_close(overlay.color_cache.colors[1][3], 0.1);
        assert_close(overlay.color_cache.colors[2][3], 0.4);
    }

    #[test]
    fn subthreshold_alpha_ceiling_creates_a_step_at_the_threshold() {
        // The quadratic ramp is nearly flat just below the threshold, so
        // without a ceiling a failing value is visually identical to a passing
        // one. Guard the step that makes the boundary readable.
        let threshold = Threshold::above(10.0);
        let just_below = threshold.opacity_factor(
            Some(9.9),
            FadeCurve::Quadratic,
            FadeWidth::BoundaryMagnitude,
        );
        assert!(
            just_below > 0.97,
            "raw ramp should be nearly opaque just below threshold, got {just_below}"
        );

        let mut color = [1.0, 0.0, 0.0, 1.0];
        apply_threshold_fade(
            &mut color,
            just_below,
            FadeSettings {
                desaturate: 0.0,
                ..FadeSettings::afni()
            },
        );
        assert_close(color[3], AFNI_SUBTHRESHOLD_MAX_ALPHA);

        // A ceiling of 1.0 disables the step and restores the raw ramp.
        let mut color = [1.0, 0.0, 0.0, 1.0];
        apply_threshold_fade(
            &mut color,
            just_below,
            FadeSettings {
                max_alpha: 1.0,
                desaturate: 0.0,
                ..FadeSettings::afni()
            },
        );
        assert_close(color[3], just_below);
    }

    #[test]
    fn ceiling_never_raises_alpha_and_is_bounded() {
        // Deep sub-threshold values must keep their low ramp alpha; the ceiling
        // is a cap, not an assignment.
        let mut color = [1.0, 0.0, 0.0, 1.0];
        apply_threshold_fade(
            &mut color,
            0.1,
            FadeSettings {
                max_alpha: 0.85,
                desaturate: 0.0,
                ..FadeSettings::new()
            },
        );
        assert_close(color[3], 0.1);

        // Out-of-range and non-finite settings must not produce invalid alpha.
        for max_alpha in [-1.0, 2.0, f32::NAN] {
            let mut color = [1.0, 0.0, 0.0, 1.0];
            apply_threshold_fade(
                &mut color,
                0.5,
                FadeSettings {
                    max_alpha,
                    desaturate: 0.0,
                    ..FadeSettings::new()
                },
            );
            assert!(
                (0.0..=1.0).contains(&color[3]),
                "alpha left valid range for max_alpha {max_alpha}: {}",
                color[3]
            );
        }
    }

    #[test]
    fn desaturation_scales_with_the_fade_and_preserves_luminance() {
        let saturated = [1.0f32, 0.0, 0.0, 1.0];
        let luminance = 0.2126 * saturated[0] + 0.7152 * saturated[1] + 0.0722 * saturated[2];

        // Fully faded and fully desaturated lands on the luminance itself.
        let mut color = saturated;
        apply_threshold_fade(
            &mut color,
            0.0,
            FadeSettings {
                desaturate: 1.0,
                max_alpha: 1.0,
                darken: 0.0,
                ..FadeSettings::new()
            },
        );
        assert_close(color[0], luminance);
        assert_close(color[1], luminance);
        assert_close(color[2], luminance);

        // A value near the threshold keeps most of its color.
        let mut color = saturated;
        apply_threshold_fade(
            &mut color,
            0.9,
            FadeSettings {
                desaturate: 1.0,
                max_alpha: 1.0,
                darken: 0.0,
                ..FadeSettings::new()
            },
        );
        assert!(
            color[0] > 0.9,
            "near-threshold color should stay saturated, got {}",
            color[0]
        );

        // Zero desaturation is AFNI behavior: opacity moves, color does not.
        let mut color = saturated;
        apply_threshold_fade(&mut color, 0.25, FadeSettings::afni());
        assert_close(color[0], saturated[0]);
        assert_close(color[1], saturated[1]);
        assert_close(color[2], saturated[2]);
    }

    #[test]
    fn darkening_scales_with_the_fade_and_is_independent_of_desaturation() {
        // Darkening is the third channel: it must move brightness even when
        // saturation is left alone, which is the case that opacity handles
        // badly (a bright color over a light surface).
        let mut color = [1.0, 1.0, 1.0, 1.0];
        apply_threshold_fade(
            &mut color,
            0.0,
            FadeSettings {
                desaturate: 0.0,
                darken: 1.0,
                max_alpha: 1.0,
                ..FadeSettings::new()
            },
        );
        assert_close(color[0], 0.0);

        // Half-faded with full darkening lands halfway down.
        let mut color = [1.0, 1.0, 1.0, 1.0];
        apply_threshold_fade(
            &mut color,
            0.5,
            FadeSettings {
                desaturate: 0.0,
                darken: 1.0,
                max_alpha: 1.0,
                ..FadeSettings::new()
            },
        );
        assert_close(color[0], 0.5);

        // A value at the threshold is untouched regardless of the setting.
        let mut color = [1.0, 0.25, 0.75, 1.0];
        let original = color;
        apply_threshold_fade(
            &mut color,
            1.0,
            FadeSettings {
                desaturate: 1.0,
                darken: 1.0,
                max_alpha: 1.0,
                ..FadeSettings::new()
            },
        );
        assert_eq!(color, original);

        // Out-of-range settings must leave channels in gamut.
        for darken in [-1.0, 2.0, f32::NAN] {
            let mut color = [0.5, 0.5, 0.5, 1.0];
            apply_threshold_fade(
                &mut color,
                0.25,
                FadeSettings {
                    darken,
                    ..FadeSettings::new()
                },
            );
            assert!(color[..3].iter().all(|c| (0.0..=1.0).contains(c)));
        }
    }

    #[test]
    fn boost_saturates_passing_colors_but_respects_the_gamut() {
        // A color with headroom gains saturation around its own luminance.
        let mut color = [0.6, 0.5, 0.5, 1.0];
        let luminance = 0.2126 * 0.6 + 0.7152 * 0.5 + 0.0722 * 0.5;
        apply_threshold_boost(&mut color, 1.0);
        assert_close(color[0], luminance + (0.6 - luminance) * 2.0);

        // A color already at the edge of the gamut cannot be pushed further.
        // This is why Dark and the contour carry the separation for saturated
        // colormaps rather than Boost.
        let mut saturated = [0.0, 1.0, 0.0, 1.0];
        apply_threshold_boost(&mut saturated, 1.0);
        assert_eq!(saturated, [0.0, 1.0, 0.0, 1.0]);

        // Zero and invalid boosts are no-ops.
        for boost in [0.0, -1.0, f32::NAN] {
            let mut color = [0.6, 0.5, 0.4, 1.0];
            let original = color;
            apply_threshold_boost(&mut color, boost);
            assert_eq!(color, original);
        }

        // Boost never touches alpha.
        let mut color = [0.6, 0.5, 0.4, 0.5];
        apply_threshold_boost(&mut color, 1.0);
        assert_close(color[3], 0.5);
    }

    #[test]
    fn steeper_curves_pull_down_the_near_threshold_band() {
        // The whole point of the extra curves: at 0.9T the quadratic ramp is
        // still nearly opaque, and each steeper exponent drops it further.
        let threshold = Threshold::above(10.0);
        let factors: Vec<f32> = [
            FadeCurve::Linear,
            FadeCurve::Quadratic,
            FadeCurve::Cubic,
            FadeCurve::Quartic,
        ]
        .into_iter()
        .map(|curve| threshold.opacity_factor(Some(9.0), curve, FadeWidth::BoundaryMagnitude))
        .collect();

        assert_close(factors[0], 0.9);
        assert_close(factors[1], 0.81);
        assert_close(factors[2], 0.729);
        assert_close(factors[3], 0.6561);
        assert!(factors.windows(2).all(|pair| pair[0] > pair[1]));

        // Every curve still pins both ends of the ramp.
        for curve in FadeCurve::ALL {
            assert_close(
                threshold.opacity_factor(Some(0.0), curve, FadeWidth::BoundaryMagnitude),
                0.0,
            );
            assert_close(
                threshold.opacity_factor(Some(10.0), curve, FadeWidth::BoundaryMagnitude),
                1.0,
            );
        }

        // The UI list is ordered steepest first.
        assert_eq!(FadeCurve::ALL[0], FadeCurve::Quartic);
        assert_eq!(FadeCurve::ALL.len(), 4);
    }

    #[test]
    fn absolute_fade_width_is_narrower_than_fade_to_zero() {
        // The explicit width is the context-versus-contrast knob: at the same
        // value it must fade harder than the fade-to-zero ramp.
        let threshold = Threshold::above(10.0);
        let to_zero =
            threshold.opacity_factor(Some(5.0), FadeCurve::Linear, FadeWidth::BoundaryMagnitude);
        let narrow =
            threshold.opacity_factor(Some(5.0), FadeCurve::Linear, FadeWidth::Absolute(2.0));
        assert_close(to_zero, 0.5);
        assert_close(narrow, 0.0);

        // An absolute width equal to the boundary magnitude is identical to
        // fade-to-zero for Above(T).
        for value in [0.0, 2.5, 5.0, 7.5, 9.9] {
            assert_close(
                threshold.opacity_factor(
                    Some(value),
                    FadeCurve::Quadratic,
                    FadeWidth::Absolute(10.0),
                ),
                threshold.opacity_factor(
                    Some(value),
                    FadeCurve::Quadratic,
                    FadeWidth::BoundaryMagnitude,
                ),
            );
        }
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

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "expected {expected}, got {actual}"
        );
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
