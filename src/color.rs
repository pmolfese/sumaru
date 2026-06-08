use anyhow::{Result, ensure};

#[derive(Debug, Clone, PartialEq)]
pub enum ColorMap {
    Continuous(ContinuousColorMap),
    Labels(LabelTable),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContinuousColorMap {
    pub name: String,
    pub stops: Vec<ColorStop>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStop {
    pub position: f32,
    pub color: Rgba,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelTable {
    pub name: Option<String>,
    pub source: LabelTableSource,
    pub labels: Vec<LabelEntry>,
    pub unlabeled_color: Rgba,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelEntry {
    pub key: i32,
    pub label: String,
    pub color: Rgba,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelTableSource {
    Manual,
    Gifti,
    FreeSurfer,
    Unknown,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FreeSurferLabelEntry {
    pub key: i32,
    pub label: String,
    pub color: Rgba,
}

impl ColorMap {
    pub fn blue_white_red() -> Self {
        Self::Continuous(ContinuousColorMap::blue_white_red())
    }

    pub fn grayscale() -> Self {
        Self::Continuous(ContinuousColorMap::grayscale())
    }

    pub fn labels(label_table: LabelTable) -> Self {
        Self::Labels(label_table)
    }

    pub fn as_continuous(&self) -> Option<&ContinuousColorMap> {
        match self {
            Self::Continuous(colormap) => Some(colormap),
            Self::Labels(_) => None,
        }
    }

    pub fn label_table(&self) -> Option<&LabelTable> {
        match self {
            Self::Continuous(_) => None,
            Self::Labels(label_table) => Some(label_table),
        }
    }
}

impl ContinuousColorMap {
    pub fn new(name: impl Into<String>, stops: Vec<ColorStop>) -> Result<Self> {
        let name = name.into();
        ensure!(!name.trim().is_empty(), "color map name is empty");
        ensure!(!stops.is_empty(), "color map has no color stops");

        for stop in &stops {
            ensure!(
                stop.position.is_finite(),
                "color stop position must be finite"
            );
            ensure!(
                (0.0..=1.0).contains(&stop.position),
                "color stop position {} is outside 0..1",
                stop.position
            );
        }

        ensure!(
            stops
                .windows(2)
                .all(|window| window[0].position <= window[1].position),
            "color stops must be sorted by position"
        );

        Ok(Self { name, stops })
    }

    pub fn blue_white_red() -> Self {
        Self {
            name: "Blue-White-Red".to_string(),
            stops: vec![
                ColorStop {
                    position: 0.0,
                    color: Rgba::new_unchecked(0.1, 0.22, 0.85, 1.0),
                },
                ColorStop {
                    position: 0.5,
                    color: Rgba::new_unchecked(1.0, 1.0, 1.0, 1.0),
                },
                ColorStop {
                    position: 1.0,
                    color: Rgba::new_unchecked(0.86, 0.08, 0.08, 1.0),
                },
            ],
        }
    }

    pub fn grayscale() -> Self {
        Self {
            name: "Grayscale".to_string(),
            stops: vec![
                ColorStop {
                    position: 0.0,
                    color: Rgba::new_unchecked(0.0, 0.0, 0.0, 1.0),
                },
                ColorStop {
                    position: 1.0,
                    color: Rgba::new_unchecked(1.0, 1.0, 1.0, 1.0),
                },
            ],
        }
    }

    pub fn sample(&self, position: f32) -> Rgba {
        let position = position.clamp(0.0, 1.0);

        if position <= self.stops[0].position {
            return self.stops[0].color;
        }

        for window in self.stops.windows(2) {
            let left = window[0];
            let right = window[1];

            if position <= right.position {
                let span = right.position - left.position;
                let t = if span.abs() <= f32::EPSILON {
                    0.0
                } else {
                    (position - left.position) / span
                };

                return left.color.lerp(right.color, t);
            }
        }

        self.stops[self.stops.len() - 1].color
    }
}

impl ColorStop {
    pub fn new(position: f32, color: Rgba) -> Result<Self> {
        ensure!(position.is_finite(), "color stop position must be finite");
        ensure!(
            (0.0..=1.0).contains(&position),
            "color stop position {position} is outside 0..1"
        );

        Ok(Self { position, color })
    }
}

impl Rgba {
    pub const TRANSPARENT: Self = Self::new_unchecked(0.0, 0.0, 0.0, 0.0);
    pub const OPAQUE_BLACK: Self = Self::new_unchecked(0.0, 0.0, 0.0, 1.0);

    pub const fn new_unchecked(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    pub fn new(red: f32, green: f32, blue: f32, alpha: f32) -> Result<Self> {
        for (name, channel) in [
            ("red", red),
            ("green", green),
            ("blue", blue),
            ("alpha", alpha),
        ] {
            ensure!(channel.is_finite(), "{name} color channel must be finite");
            ensure!(
                (0.0..=1.0).contains(&channel),
                "{name} color channel {channel} is outside 0..1"
            );
        }

        Ok(Self {
            red,
            green,
            blue,
            alpha,
        })
    }

    pub fn clamped(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self {
            red: finite_or_zero(red).clamp(0.0, 1.0),
            green: finite_or_zero(green).clamp(0.0, 1.0),
            blue: finite_or_zero(blue).clamp(0.0, 1.0),
            alpha: finite_or_zero(alpha).clamp(0.0, 1.0),
        }
    }

    pub fn from_u8(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red: red as f32 / 255.0,
            green: green as f32 / 255.0,
            blue: blue as f32 / 255.0,
            alpha: alpha as f32 / 255.0,
        }
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.red, self.green, self.blue, self.alpha]
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            red: self.red + (other.red - self.red) * t,
            green: self.green + (other.green - self.green) * t,
            blue: self.blue + (other.blue - self.blue) * t,
            alpha: self.alpha + (other.alpha - self.alpha) * t,
        }
    }
}

impl LabelTable {
    pub fn new(source: LabelTableSource, labels: Vec<LabelEntry>) -> Result<Self> {
        Self::with_name(None, source, labels)
    }

    pub fn with_name(
        name: Option<String>,
        source: LabelTableSource,
        mut labels: Vec<LabelEntry>,
    ) -> Result<Self> {
        labels.sort_by_key(|label| label.key);
        ensure!(!labels.is_empty(), "label table has no entries");
        ensure!(
            labels
                .windows(2)
                .all(|window| window[0].key != window[1].key),
            "label table contains duplicate keys"
        );

        Ok(Self {
            name,
            source,
            labels,
            unlabeled_color: Rgba::TRANSPARENT,
        })
    }

    pub fn from_gifti(label_table: &gifti_rs::LabelTable) -> Result<Self> {
        let labels = label_table
            .labels
            .iter()
            .map(|label| {
                let color = Rgba::clamped(
                    label.red.unwrap_or(0.0),
                    label.green.unwrap_or(0.0),
                    label.blue.unwrap_or(0.0),
                    label.alpha.unwrap_or(1.0),
                );
                LabelEntry::new(label.key, label.text.clone(), color)
            })
            .collect::<Result<Vec<_>>>()?;

        Self::new(LabelTableSource::Gifti, labels)
    }

    pub fn from_freesurfer_entries(entries: Vec<FreeSurferLabelEntry>) -> Result<Self> {
        let labels = entries
            .into_iter()
            .map(|entry| LabelEntry::new(entry.key, entry.label, entry.color))
            .collect::<Result<Vec<_>>>()?;

        Self::new(LabelTableSource::FreeSurfer, labels)
    }

    pub fn label(&self, key: i32) -> Option<&LabelEntry> {
        self.labels
            .binary_search_by_key(&key, |entry| entry.key)
            .ok()
            .map(|index| &self.labels[index])
    }

    pub fn color_for_key(&self, key: i32) -> Rgba {
        self.label(key)
            .map_or(self.unlabeled_color, |label| label.color)
    }
}

impl LabelEntry {
    pub fn new(key: i32, label: impl Into<String>, color: Rgba) -> Result<Self> {
        let label = label.into();
        ensure!(!label.trim().is_empty(), "label text is empty");

        Ok(Self { key, label, color })
    }
}

impl FreeSurferLabelEntry {
    pub fn from_rgba_u8(
        key: i32,
        label: impl Into<String>,
        red: u8,
        green: u8,
        blue: u8,
        alpha: u8,
    ) -> Result<Self> {
        let label = label.into();
        ensure!(!label.trim().is_empty(), "FreeSurfer label text is empty");

        Ok(Self {
            key,
            label,
            color: Rgba::from_u8(red, green, blue, alpha),
        })
    }
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::{
        ColorMap, ColorStop, ContinuousColorMap, FreeSurferLabelEntry, LabelEntry, LabelTable,
        LabelTableSource, Rgba,
    };

    #[test]
    fn continuous_colormap_interpolates_between_stops() {
        let colormap = ContinuousColorMap::new(
            "two-stop",
            vec![
                ColorStop::new(0.0, Rgba::new(0.0, 0.0, 0.0, 1.0).unwrap()).unwrap(),
                ColorStop::new(1.0, Rgba::new(1.0, 1.0, 1.0, 1.0).unwrap()).unwrap(),
            ],
        )
        .unwrap();

        assert_eq!(
            colormap.sample(0.25),
            Rgba::new(0.25, 0.25, 0.25, 1.0).unwrap()
        );
    }

    #[test]
    fn continuous_colormap_rejects_unsorted_stops() {
        let error = ContinuousColorMap::new(
            "bad",
            vec![
                ColorStop::new(1.0, Rgba::OPAQUE_BLACK).unwrap(),
                ColorStop::new(0.0, Rgba::OPAQUE_BLACK).unwrap(),
            ],
        )
        .unwrap_err();

        assert!(error.to_string().contains("sorted"));
    }

    #[test]
    fn builtin_colormaps_can_be_wrapped_as_display_maps() {
        let colormap = ColorMap::grayscale();
        let continuous = colormap.as_continuous().unwrap();

        assert_eq!(continuous.sample(0.0), Rgba::OPAQUE_BLACK);
        assert_eq!(
            ColorMap::blue_white_red()
                .as_continuous()
                .unwrap()
                .sample(0.5),
            Rgba::new(1.0, 1.0, 1.0, 1.0).unwrap()
        );
    }

    #[test]
    fn label_table_sorts_and_finds_integer_labels() {
        let table = LabelTable::new(
            LabelTableSource::Manual,
            vec![
                LabelEntry::new(2, "V2", Rgba::from_u8(0, 255, 0, 255)).unwrap(),
                LabelEntry::new(1, "V1", Rgba::from_u8(255, 0, 0, 255)).unwrap(),
            ],
        )
        .unwrap();

        assert_eq!(table.labels[0].key, 1);
        assert_eq!(table.label(2).unwrap().label, "V2");
        assert_eq!(table.color_for_key(99), Rgba::TRANSPARENT);
    }

    #[test]
    fn label_table_rejects_duplicate_keys() {
        let error = LabelTable::new(
            LabelTableSource::Manual,
            vec![
                LabelEntry::new(1, "first", Rgba::OPAQUE_BLACK).unwrap(),
                LabelEntry::new(1, "second", Rgba::OPAQUE_BLACK).unwrap(),
            ],
        )
        .unwrap_err();

        assert!(error.to_string().contains("duplicate"));
    }

    #[test]
    fn gifti_label_table_import_preserves_keys_names_and_rgba() {
        let gifti = gifti_rs::LabelTable {
            labels: vec![gifti_rs::Label {
                key: 42,
                red: Some(0.1),
                green: Some(0.2),
                blue: Some(0.3),
                alpha: Some(0.4),
                text: "area 42".to_string(),
            }],
        };

        let table = LabelTable::from_gifti(&gifti).unwrap();

        assert_eq!(table.source, LabelTableSource::Gifti);
        assert_eq!(table.label(42).unwrap().label, "area 42");
        assert_eq!(
            table.color_for_key(42),
            Rgba::new(0.1, 0.2, 0.3, 0.4).unwrap()
        );
    }

    #[test]
    fn freesurfer_label_entries_use_u8_rgba_channels() {
        let entry = FreeSurferLabelEntry::from_rgba_u8(17, "bankssts", 10, 20, 30, 255).unwrap();
        let table = LabelTable::from_freesurfer_entries(vec![entry]).unwrap();

        assert_eq!(table.source, LabelTableSource::FreeSurfer);
        assert_eq!(table.label(17).unwrap().label, "bankssts");
        assert_eq!(table.color_for_key(17), Rgba::from_u8(10, 20, 30, 255));
    }
}
