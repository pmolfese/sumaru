//! AFNI/FATCAT `.niml.tract` data model and reader.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::io::{NimlData, read_niml};

#[derive(Debug, Clone, PartialEq)]
pub struct TractographyDataset {
    pub path: Option<PathBuf>,
    pub bundles: Vec<TractBundle>,
    pub bounds: SpatialBounds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TractBundle {
    pub tag: Option<i32>,
    pub alternate_tag: Option<i32>,
    pub ends: Option<String>,
    pub tracts: Vec<Tract>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tract {
    pub id: i32,
    pub points: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub center: [f32; 3],
    pub radius: f32,
}

impl SpatialBounds {
    pub fn from_points<'a>(points: impl IntoIterator<Item = &'a [f32; 3]>) -> Option<Self> {
        let mut points = points.into_iter();
        let first = *points.next()?;
        let mut min = first;
        let mut max = first;
        for point in points {
            for axis in 0..3 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        let center = [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ];
        let radius = [max[0] - center[0], max[1] - center[1], max[2] - center[2]]
            .into_iter()
            .map(f32::abs)
            .fold(0.0, f32::max)
            .max(f32::EPSILON);
        Some(Self {
            min,
            max,
            center,
            radius,
        })
    }
}

impl TractographyDataset {
    pub fn tract_count(&self) -> usize {
        self.bundles.iter().map(|bundle| bundle.tracts.len()).sum()
    }

    pub fn point_count(&self) -> usize {
        self.bundles
            .iter()
            .flat_map(|bundle| &bundle.tracts)
            .map(|tract| tract.points.len())
            .sum()
    }
}

pub fn read_niml_tract(path: impl AsRef<Path>) -> Result<TractographyDataset> {
    let path = path.as_ref();
    let elements = read_niml(path)
        .with_context(|| format!("failed to read tractography file {}", path.display()))?;
    ensure!(elements.len() == 1, "expected one top-level tract network");
    let root = &elements[0];
    ensure!(
        root.name == "network",
        "expected <network>, got <{}>",
        root.name
    );
    let NimlData::Group(children) = &root.data else {
        bail!("tract network is not a NIML group");
    };

    let mut bundles = Vec::new();
    for child in children.iter().filter(|child| child.name == "tracts") {
        let NimlData::TractDatums(records) = &child.data else {
            bail!("<tracts> does not contain TAYLOR_TRACT_DATUM rows");
        };
        let tag = parse_optional_i32(&child.attrs, "Bundle_Tag")?;
        let alternate_tag = parse_optional_i32(&child.attrs, "Bundle_Alt_Tag")?;
        let ends = child.attrs.get("Bundle_Ends").cloned();
        let tracts = records
            .iter()
            .map(|record| Tract {
                id: record.id,
                // FATCAT writes TAYLOR_TRACT_DATUM coordinates in AFNI's
                // RAI/DICOM world convention. Sumaru's surfaces and volume
                // renderer use NIfTI/GIFTI RAS (AFNI calls it LPI), whose x
                // and y axes have the opposite signs. AFNI's Create_Tract_NEW
                // likewise documents and enforces an RAI grid before writing
                // these values (ptaylor/TrackIO.c).
                points: record.points.iter().copied().map(afni_rai_to_ras).collect(),
            })
            .collect();
        bundles.push(TractBundle {
            tag,
            alternate_tag,
            ends,
            tracts,
        });
    }
    ensure!(!bundles.is_empty(), "tract network contains no bundles");
    if let Some(declared) = root.attrs.get("N_tracts") {
        let declared = declared
            .parse::<usize>()
            .with_context(|| format!("invalid network N_tracts value {declared:?}"))?;
        let actual = bundles
            .iter()
            .map(|bundle| bundle.tracts.len())
            .sum::<usize>();
        ensure!(
            declared == actual,
            "tract network declares {declared} tracts but contains {actual}"
        );
    }
    let bounds = SpatialBounds::from_points(
        bundles
            .iter()
            .flat_map(|bundle| &bundle.tracts)
            .flat_map(|tract| &tract.points),
    )
    .context("tract network contains no points")?;

    Ok(TractographyDataset {
        path: Some(path.to_path_buf()),
        bundles,
        bounds,
    })
}

fn afni_rai_to_ras([x, y, z]: [f32; 3]) -> [f32; 3] {
    [-x, -y, z]
}

fn parse_optional_i32(
    attrs: &std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<Option<i32>> {
    attrs
        .get(key)
        .map(|value| {
            value
                .parse::<i32>()
                .with_context(|| format!("invalid {key} value {value:?}"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::afni_rai_to_ras;
    use crate::io::{NimlData, parse_niml_bytes, parse_niml_str};

    #[test]
    fn converts_afni_rai_tract_coordinates_to_ras() {
        assert_eq!(afni_rai_to_ras([12.5, -7.0, 3.25]), [-12.5, 7.0, 3.25]);
    }

    #[test]
    fn reads_ascii_taylor_tract_rows() {
        let elements = parse_niml_str(
            r#"<tracts ni_type="TAYLOR_TRACT_DATUM" ni_dimen="2">
7 6 1 2 3 4 5 6
8 3 -1 -2 -3
</tracts>"#,
        )
        .unwrap();
        let NimlData::TractDatums(rows) = &elements[0].data else {
            panic!()
        };
        assert_eq!(rows[0].id, 7);
        assert_eq!(rows[0].points, vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        assert_eq!(rows[1].points, vec![[-1.0, -2.0, -3.0]]);
    }

    #[test]
    fn reads_binary_little_endian_taylor_tract_rows() {
        let mut bytes =
            b"<tracts ni_form=\"binary.lsbfirst\" ni_type=\"TAYLOR_TRACT_DATUM\" ni_dimen=\"1\">"
                .to_vec();
        bytes.extend_from_slice(&42_i32.to_le_bytes());
        bytes.extend_from_slice(&6_i32.to_le_bytes());
        for value in [1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(b"</tracts>");
        let elements = parse_niml_bytes(&bytes).unwrap();
        let NimlData::TractDatums(rows) = &elements[0].data else {
            panic!()
        };
        assert_eq!(rows[0].id, 42);
        assert_eq!(rows[0].points.len(), 2);
    }
}
