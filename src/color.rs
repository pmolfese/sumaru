use anyhow::{Result, ensure};

const STABLE_LABEL_RGB_PALETTE: [[u8; 3]; 10] = [
    [0, 194, 255],
    [255, 242, 0],
    [57, 255, 20],
    [255, 117, 24],
    [255, 0, 255],
    [0, 255, 255],
    [255, 49, 49],
    [157, 0, 255],
    [180, 255, 0],
    [255, 0, 170],
];
const ZERO_LABEL_RGB: [u8; 3] = [128, 128, 128];

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
    pub fn spectrum_red_to_blue() -> Self {
        Self::Continuous(ContinuousColorMap::spectrum_red_to_blue())
    }

    pub fn spectrum_yellow_to_red() -> Self {
        Self::Continuous(ContinuousColorMap::spectrum_yellow_to_red())
    }

    pub fn color_circle_ajj() -> Self {
        Self::Continuous(ContinuousColorMap::color_circle_ajj())
    }

    pub fn spectrum_red_to_blue_gap() -> Self {
        Self::Continuous(ContinuousColorMap::spectrum_red_to_blue_gap())
    }

    pub fn spectrum_yellow_to_cyan() -> Self {
        Self::Continuous(ContinuousColorMap::spectrum_yellow_to_cyan())
    }

    pub fn spectrum_yellow_to_cyan_gap() -> Self {
        Self::Continuous(ContinuousColorMap::spectrum_yellow_to_cyan_gap())
    }

    pub fn color_circle_zss() -> Self {
        Self::Continuous(ContinuousColorMap::color_circle_zss())
    }

    pub fn reds_and_blues() -> Self {
        Self::Continuous(ContinuousColorMap::reds_and_blues())
    }

    pub fn reds_and_blues_with_green() -> Self {
        Self::Continuous(ContinuousColorMap::reds_and_blues_with_green())
    }

    pub fn afni_p2_spanned() -> Self {
        Self::Continuous(ContinuousColorMap::afni_p2_spanned())
    }

    pub fn blue_white_red() -> Self {
        Self::Continuous(ContinuousColorMap::blue_white_red())
    }

    pub fn fire() -> Self {
        Self::Continuous(ContinuousColorMap::fire())
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

pub fn stable_label_rgb(key: i32) -> [u8; 3] {
    if key == 0 {
        return ZERO_LABEL_RGB;
    }

    let normalized = if key > 0 {
        key - 1
    } else {
        key.saturating_abs() - 1
    };
    let index = normalized.rem_euclid(STABLE_LABEL_RGB_PALETTE.len() as i32) as usize;
    STABLE_LABEL_RGB_PALETTE[index]
}

pub fn stable_label_color(key: i32, alpha: u8) -> Rgba {
    let [red, green, blue] = stable_label_rgb(key);
    Rgba::from_u8(red, green, blue, alpha)
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

    /// AFNI's `Spectrum:red_to_blue` colorscale (`bigmap[0]` in display.c),
    /// reproduced exactly from `DC_spectrum_AJJ`. Oriented so position 0 is
    /// blue (low) and position 1 is red (high), matching AFNI's value mapping.
    pub fn spectrum_red_to_blue() -> Self {
        // AFNI sweeps hue = ii*(248/255) - 4 for ii in 0..=255 (red -> blue).
        // Position p corresponds to ii = (1 - p) * 255, so hue = (1-p)*248 - 4.
        Self::ajj_colorscale("Spectrum:red_to_blue", 0.8, |p| (1.0 - p) * 248.0 - 4.0)
    }

    /// AFNI's `Spectrum:yellow_to_red` colorscale (`bigmap[4]`), reproduced
    /// from `DC_spectrum_AJJ` with gamma 0.7. Position 0 is red (low),
    /// position 1 is yellow (high).
    pub fn spectrum_yellow_to_red() -> Self {
        // AFNI sweeps hue = 60 - ii*(60/255) for ii in 0..=255 (yellow -> red).
        // With ii = (1 - p) * 255 this simplifies to hue = 60 * p.
        Self::ajj_colorscale("Spectrum:yellow_to_red", 0.7, |p| p * 60.0)
    }

    /// AFNI's `Color_circle_AJJ` colorscale (`bigmap[5]`), a full hue wheel
    /// from `DC_spectrum_AJJ` with gamma 0.8. Useful for cyclic data such as
    /// phase/angle maps.
    pub fn color_circle_ajj() -> Self {
        // AFNI sweeps the full circle hue = ii*(360/255) for ii in 0..=255.
        Self::ajj_colorscale("Color_circle_AJJ", 0.8, |p| p * 360.0)
    }

    /// AFNI's `Spectrum:red_to_blue+gap` colorscale (`bigmap[1]`). Red->yellow
    /// in the lower half, a black gap across the center, cyan->blue in the
    /// upper half. The gap reads cleanly for thresholded two-tailed stats.
    pub fn spectrum_red_to_blue_gap() -> Self {
        Self::ajj_indexed("Spectrum:red_to_blue+gap", |index| {
            if index < BIGMAP_MBOT {
                spectrum_ajj(index as f64 * (AJJ_YEL / (BIGMAP_MBOT - 1) as f64), 0.8)
            } else if index > BIGMAP_MTOP {
                spectrum_ajj(
                    AJJ_CYN + (index - BIGMAP_MTOP - 1) as f64 * (60.0 / BIGMAP_HALF_SPAN),
                    0.8,
                )
            } else {
                Rgba::OPAQUE_BLACK
            }
        })
    }

    /// AFNI's `Spectrum:yellow_to_cyan` colorscale (`bigmap[2]`). Yellow->red
    /// lower, a magenta/purple bridge across the center, blue->cyan upper.
    pub fn spectrum_yellow_to_cyan() -> Self {
        Self::ajj_indexed("Spectrum:yellow_to_cyan", |index| {
            spectrum_ajj(yellow_to_cyan_hue(index), 0.8)
        })
    }

    /// AFNI's `Spectrum:yellow_to_cyan+gap` colorscale (`bigmap[3]`). Same as
    /// `yellow_to_cyan` but with a black gap replacing the center bridge.
    pub fn spectrum_yellow_to_cyan_gap() -> Self {
        Self::ajj_indexed("Spectrum:yellow_to_cyan+gap", |index| {
            if (BIGMAP_MBOT..=BIGMAP_MTOP).contains(&index) {
                Rgba::OPAQUE_BLACK
            } else {
                spectrum_ajj(yellow_to_cyan_hue(index), 0.8)
            }
        })
    }

    /// AFNI's `Color_circle_ZSS` colorscale (`bigmap[6]`), a full hue wheel
    /// built from `DC_spectrum_ZSS`.
    pub fn color_circle_zss() -> Self {
        Self::ajj_indexed("Color_circle_ZSS", |index| {
            spectrum_zss(360.0 - index as f64 * (360.0 / (BIGMAP_N - 1) as f64), 1.0)
        })
    }

    /// AFNI's `Reds_and_Blues` colorscale (`bigmap[7]`). Yellow->red across the
    /// lower half, blue->cyan across the upper half, with no green.
    pub fn reds_and_blues() -> Self {
        Self::ajj_indexed("Reds_and_Blues", reds_and_blues_color)
    }

    /// AFNI's `Reds_and_Blues_w_Green` colorscale (`bigmap[8]`). Identical to
    /// `Reds_and_Blues` with a green band at the center. AFNI paints only two
    /// entries (127, 128) green, which is an invisible sliver once the 256-entry
    /// LUT is interpolated, so we widen the seam to `BIGMAP_GREEN_SEAM_HALF`
    /// entries on each side of center while keeping AFNI's exact green hue.
    pub fn reds_and_blues_with_green() -> Self {
        let green_band =
            (BIGMAP_HALF - BIGMAP_GREEN_SEAM_HALF)..(BIGMAP_HALF + BIGMAP_GREEN_SEAM_HALF);
        Self::ajj_indexed("Reds_and_Blues_w_Green", move |index| {
            if green_band.contains(&index) {
                spectrum_ajj(
                    BIGMAP_HALF as f64 * ((AJJ_BLU + 8.0) / (BIGMAP_N - 1) as f64) - 4.0,
                    0.8,
                )
            } else {
                reds_and_blues_color(index)
            }
        })
    }

    /// Builds a 256-entry colorscale by evaluating `DC_spectrum_AJJ` at the hue
    /// produced by `hue_at` for each stop position. Mirrors the per-channel
    /// gamma and byte quantization of AFNI's `NJ_bigmaps_init`.
    fn ajj_colorscale(name: &str, gamma: f64, hue_at: impl Fn(f64) -> f64) -> Self {
        const COUNT: usize = 256;
        let stops = (0..COUNT)
            .map(|index| {
                let position = index as f32 / (COUNT - 1) as f32;
                ColorStop {
                    position,
                    color: spectrum_ajj(hue_at(position as f64), gamma),
                }
            })
            .collect();
        Self {
            name: name.to_string(),
            stops,
        }
    }

    /// Builds a colorscale from an AFNI per-index color function. AFNI draws
    /// array index 0 at the top (maximum value), so index `ii` maps to position
    /// `(N-1-ii)/(N-1)`; the returned stops are sorted ascending by position.
    fn ajj_indexed(name: &str, color_at: impl Fn(usize) -> Rgba) -> Self {
        let mut stops: Vec<ColorStop> = (0..BIGMAP_N)
            .map(|index| ColorStop {
                position: (BIGMAP_N - 1 - index) as f32 / (BIGMAP_N - 1) as f32,
                color: color_at(index),
            })
            .collect();
        stops.reverse();
        Self {
            name: name.to_string(),
            stops,
        }
    }

    pub fn afni_p2_spanned() -> Self {
        Self {
            name: "afni_p2spanned".to_string(),
            stops: vec![
                ColorStop {
                    position: 0.0,
                    color: Rgba::new_unchecked(0.02, 0.12, 0.32, 1.0),
                },
                ColorStop {
                    position: 0.22,
                    color: Rgba::new_unchecked(0.08, 0.38, 0.68, 1.0),
                },
                ColorStop {
                    position: 0.42,
                    color: Rgba::new_unchecked(0.52, 0.74, 0.92, 1.0),
                },
                ColorStop {
                    position: 0.5,
                    color: Rgba::new_unchecked(0.98, 0.96, 0.86, 1.0),
                },
                ColorStop {
                    position: 0.66,
                    color: Rgba::new_unchecked(0.96, 0.68, 0.28, 1.0),
                },
                ColorStop {
                    position: 0.82,
                    color: Rgba::new_unchecked(0.80, 0.24, 0.16, 1.0),
                },
                ColorStop {
                    position: 1.0,
                    color: Rgba::new_unchecked(0.45, 0.09, 0.07, 1.0),
                },
            ],
        }
    }

    pub fn fire() -> Self {
        Self {
            name: "nih_fire".to_string(),
            stops: vec![
                ColorStop {
                    position: 0.0,
                    color: Rgba::new_unchecked(0.02, 0.0, 0.0, 1.0),
                },
                ColorStop {
                    position: 0.28,
                    color: Rgba::new_unchecked(0.42, 0.02, 0.02, 1.0),
                },
                ColorStop {
                    position: 0.58,
                    color: Rgba::new_unchecked(0.90, 0.24, 0.02, 1.0),
                },
                ColorStop {
                    position: 0.82,
                    color: Rgba::new_unchecked(1.0, 0.74, 0.12, 1.0),
                },
                ColorStop {
                    position: 1.0,
                    color: Rgba::new_unchecked(1.0, 1.0, 0.88, 1.0),
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

// Geometry of AFNI's 256-entry "bigmap" colorscales (display.c). NBIG_GAP is
// NPANE_BIG/32 = 8, so the center gap spans MBOT..=MTOP.
const BIGMAP_N: usize = 256;
const BIGMAP_HALF: usize = BIGMAP_N / 2; // NPANE_BIG/2 = 128
const BIGMAP_MBOT: usize = 120; // NPANE_BIG/2 - NBIG_GAP
const BIGMAP_MTOP: usize = 136; // NPANE_BIG/2 + NBIG_GAP
const BIGMAP_HALF_SPAN: f64 = (BIGMAP_N - BIGMAP_MTOP - 2) as f64; // 118
// Half-width (in LUT entries) of the green center band for Reds_and_Blues_w_Green.
// AFNI uses an effective half-width of 1 (entries 127, 128); we widen it so the
// band survives interpolation and stays visible in colorbars and on the surface.
const BIGMAP_GREEN_SEAM_HALF: usize = 4;

// AFNI hue fiducials (display.h).
const AJJ_YEL: f64 = 60.0;
const AJJ_CYN: f64 = 180.0;
const AJJ_BLU: f64 = 240.0;

/// Hue sweep shared by `yellow_to_cyan` and its gapped variant (`bigmap[2]`).
fn yellow_to_cyan_hue(index: usize) -> f64 {
    if index < BIGMAP_MBOT {
        AJJ_YEL - index as f64 * (AJJ_YEL / (BIGMAP_MBOT - 1) as f64)
    } else if index > BIGMAP_MTOP {
        AJJ_BLU - (index - BIGMAP_MTOP - 1) as f64 * (60.0 / BIGMAP_HALF_SPAN)
    } else {
        let denom = (BIGMAP_MTOP - BIGMAP_MBOT + 2) as f64; // 18
        360.0 - (index - BIGMAP_MBOT + 1) as f64 * (120.0 / denom)
    }
}

/// Per-index color for AFNI's `Reds_and_Blues` (`bigmap[7]`).
fn reds_and_blues_color(index: usize) -> Rgba {
    if index < BIGMAP_HALF {
        spectrum_ajj(
            AJJ_YEL - index as f64 * (AJJ_YEL / (BIGMAP_HALF - 1) as f64),
            0.8,
        )
    } else {
        let span = (BIGMAP_N - BIGMAP_HALF - 2) as f64; // 126
        let offset = index as f64 - (BIGMAP_MTOP as f64 + 1.0); // can be negative, as in C
        spectrum_ajj(AJJ_BLU - offset * (60.0 / span), 0.8)
    }
}

/// Faithful port of AFNI's `DC_spectrum_ZSS` (display.c). Maps a hue angle in
/// degrees to RGB through four quadrants, with the same byte quantization as
/// `spectrum_ajj`.
fn spectrum_zss(hue: f64, gamma: f64) -> Rgba {
    let gamma = if gamma <= 0.0 { 1.0 } else { gamma };

    let mut an = hue;
    while an < 0.0 {
        an += 360.0;
    }
    while an > 360.0 {
        an -= 360.0;
    }
    an /= 90.0;

    let channel = |value: f64| -> i32 {
        let powered = if value <= 0.0 { 0.0 } else { value.powf(gamma) };
        (255.0 * powered + 0.5) as i32
    };

    let (red, green, blue);
    if an <= 1.0 {
        red = channel(1.0 - an);
        green = channel(0.5 * an);
        blue = channel(an);
    } else if an <= 2.0 {
        red = 0;
        green = channel(0.5 * an);
        blue = channel(2.0 - an);
    } else if an <= 3.0 {
        red = channel(an - 2.0);
        green = 255;
        blue = 0;
    } else {
        red = 255;
        green = channel(4.0 - an);
        blue = 0;
    }

    let to_unit = |value: i32| value.clamp(0, 255) as f32 / 255.0;
    Rgba::new_unchecked(to_unit(red), to_unit(green), to_unit(blue), 1.0)
}

/// Faithful port of AFNI's `DC_spectrum_AJJ` (display.c). Maps a hue angle in
/// degrees to an RGB color using the "RWC" constants and a per-channel gamma,
/// quantizing to bytes exactly as AFNI does before normalizing back to 0..1.
fn spectrum_ajj(hue: f64, gamma: f64) -> Rgba {
    let gamma = if gamma <= 0.0 { 1.0 } else { gamma };

    // RWC's choices: ak == ab == 5, so s == sb == 250 and c == cb.
    let ak = 5.0_f64;
    let s = 255.0 - ak;
    let c = s / 60.0;
    let ab = 5.0_f64;
    let sb = 255.0 - ab;
    let cb = sb / 60.0;

    let mut an = hue;
    while an < 0.0 {
        an += 360.0;
    }
    while an > 360.0 {
        an -= 360.0;
    }

    // mypow: x <= 0 -> 0, else x^gamma. Output is 255*pow(num/255) + 0.5 cast
    // to int, matching AFNI's truncating byte quantization.
    let channel = |numerator: f64| -> i32 {
        let normalized = numerator / 255.0;
        let powered = if normalized <= 0.0 {
            0.0
        } else {
            normalized.powf(gamma)
        };
        (255.0 * powered + 0.5) as i32
    };

    let (red, green, blue);
    if an < 120.0 {
        red = channel(ak + s.min((120.0 - an) * c));
        green = channel(ak + s.min(an * c));
        blue = 0;
    } else if an < 240.0 {
        red = 0;
        green = channel(ak + s.min((240.0 - an) * c));
        blue = channel(ab + sb.min((an - 120.0) * cb));
    } else {
        red = channel(ak + s.min((an - 240.0) * c));
        green = 0;
        blue = channel(ab + s.min((360.0 - an) * cb));
    }

    let to_unit = |value: i32| value.clamp(0, 255) as f32 / 255.0;
    Rgba::new_unchecked(to_unit(red), to_unit(green), to_unit(blue), 1.0)
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
        LabelTableSource, Rgba, stable_label_rgb,
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
        // Exact DC_spectrum_AJJ endpoints are slightly tinted by the +/-4 deg
        // hue padding: blue is (35, 0, 255), red is (255, 0, 35) in bytes.
        let red_to_blue = ColorMap::spectrum_red_to_blue();
        let red_to_blue = red_to_blue.as_continuous().unwrap();
        assert_eq!(
            red_to_blue.sample(0.0),
            Rgba::new(35.0 / 255.0, 0.0, 1.0, 1.0).unwrap()
        );
        assert_eq!(
            red_to_blue.sample(1.0),
            Rgba::new(1.0, 0.0, 35.0 / 255.0, 1.0).unwrap()
        );
        assert_eq!(
            ColorMap::afni_p2_spanned()
                .as_continuous()
                .unwrap()
                .sample(0.5),
            Rgba::new(0.98, 0.96, 0.86, 1.0).unwrap()
        );
        assert_eq!(
            ColorMap::blue_white_red()
                .as_continuous()
                .unwrap()
                .sample(0.5),
            Rgba::new(1.0, 1.0, 1.0, 1.0).unwrap()
        );
        assert_eq!(
            ColorMap::fire().as_continuous().unwrap().sample(1.0),
            Rgba::new(1.0, 1.0, 0.88, 1.0).unwrap()
        );
    }

    fn assert_bytes_close(actual: Rgba, expected: [u8; 3]) {
        let to_byte = |channel: f32| (channel * 255.0).round() as i32;
        assert_eq!(
            [
                to_byte(actual.red),
                to_byte(actual.green),
                to_byte(actual.blue),
            ],
            [expected[0] as i32, expected[1] as i32, expected[2] as i32],
        );
    }

    #[test]
    fn ajj_colorscales_match_afni_byte_values() {
        // Reference bytes computed directly from AFNI's DC_spectrum_AJJ.
        let red_to_blue = ColorMap::spectrum_red_to_blue();
        let red_to_blue = red_to_blue.as_continuous().unwrap();
        assert_eq!(red_to_blue.stops.len(), 256);
        // Position 1.0 is AFNI index 0 (red); the yellow plateau sits at index 64.
        assert_bytes_close(red_to_blue.sample(1.0), [255, 0, 35]);
        assert_bytes_close(red_to_blue.sample(191.0 / 255.0), [255, 249, 0]);
        assert_bytes_close(red_to_blue.sample(0.0), [35, 0, 255]);

        // Gamma 0.7 lifts the dark end, so "red" carries a little green.
        let yellow_to_red = ColorMap::spectrum_yellow_to_red();
        let yellow_to_red = yellow_to_red.as_continuous().unwrap();
        assert_bytes_close(yellow_to_red.sample(1.0), [255, 255, 0]);
        assert_bytes_close(yellow_to_red.sample(0.0), [255, 16, 0]);

        // Full hue wheel: green near the middle, near-red at both ends.
        let circle = ColorMap::color_circle_ajj();
        let circle = circle.as_continuous().unwrap();
        assert_bytes_close(circle.sample(0.0), [255, 11, 0]);
        assert_bytes_close(circle.sample(85.0 / 255.0), [0, 255, 11]);
        assert_bytes_close(circle.sample(1.0), [255, 0, 11]);
    }

    #[test]
    fn gap_and_two_sided_colorscales_match_afni_byte_values() {
        // For ajj_indexed maps, AFNI index ii maps to position (255-ii)/255.
        let pos = |index: usize| (255 - index) as f32 / 255.0;

        let red_to_blue_gap = ColorMap::spectrum_red_to_blue_gap();
        let red_to_blue_gap = red_to_blue_gap.as_continuous().unwrap();
        assert_bytes_close(red_to_blue_gap.sample(pos(0)), [255, 11, 0]);
        assert_bytes_close(red_to_blue_gap.sample(pos(128)), [0, 0, 0]);
        assert_bytes_close(red_to_blue_gap.sample(pos(255)), [11, 0, 255]);

        let yellow_to_cyan = ColorMap::spectrum_yellow_to_cyan();
        let yellow_to_cyan = yellow_to_cyan.as_continuous().unwrap();
        assert_bytes_close(yellow_to_cyan.sample(pos(0)), [255, 255, 0]);
        assert_bytes_close(yellow_to_cyan.sample(pos(128)), [255, 0, 255]);
        assert_bytes_close(yellow_to_cyan.sample(pos(255)), [0, 255, 255]);

        let yellow_to_cyan_gap = ColorMap::spectrum_yellow_to_cyan_gap();
        let yellow_to_cyan_gap = yellow_to_cyan_gap.as_continuous().unwrap();
        assert_bytes_close(yellow_to_cyan_gap.sample(pos(128)), [0, 0, 0]);

        let circle_zss = ColorMap::color_circle_zss();
        let circle_zss = circle_zss.as_continuous().unwrap();
        assert_bytes_close(circle_zss.sample(pos(0)), [255, 0, 0]);
        assert_bytes_close(circle_zss.sample(pos(64)), [254, 255, 0]);
        assert_bytes_close(circle_zss.sample(pos(128)), [0, 254, 2]);

        let reds_and_blues = ColorMap::reds_and_blues();
        let reds_and_blues = reds_and_blues.as_continuous().unwrap();
        assert_bytes_close(reds_and_blues.sample(pos(0)), [255, 255, 0]);
        assert_bytes_close(reds_and_blues.sample(pos(128)), [37, 0, 255]);

        // The "w_Green" variant inserts a widened green band centered on the
        // seam (indices 124..132); just outside the band returns to red/blue.
        let with_green = ColorMap::reds_and_blues_with_green();
        let with_green = with_green.as_continuous().unwrap();
        assert_bytes_close(with_green.sample(pos(124)), [0, 255, 14]);
        assert_bytes_close(with_green.sample(pos(128)), [0, 255, 14]);
        assert_bytes_close(with_green.sample(pos(131)), [0, 255, 14]);
        let below = with_green.sample(pos(123));
        let above = with_green.sample(pos(132));
        assert!(below.red > below.green, "index 123 should stay reddish");
        assert!(above.blue > above.green, "index 132 should stay bluish");
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

    #[test]
    fn stable_label_palette_matches_expected_first_four_labels() {
        assert_eq!(stable_label_rgb(1), [0, 194, 255]);
        assert_eq!(stable_label_rgb(2), [255, 242, 0]);
        assert_eq!(stable_label_rgb(3), [57, 255, 20]);
        assert_eq!(stable_label_rgb(4), [255, 117, 24]);
    }
}
