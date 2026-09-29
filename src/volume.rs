//! Volume loading for the `--volume` viewer mode: read a NIfTI or AFNI
//! HEAD/BRIK dataset into a
//! dense scalar grid plus the voxel<->world transform, ready for orthogonal
//! slice-plane rendering. The coordinate math lives in
//! [`crate::surface::VolumeSpace`]; this module owns the voxel data itself.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use nifti::{IntoNdArray, NiftiObject, NiftiVolume, ReaderOptions};

use crate::surface::{SurfaceTransform, VolumeSpace};

/// A loaded scalar volume: the voxel samples, their grid<->world placement, and
/// the intensity range used for window/level display.
///
/// `data` is stored in i-fastest order (`index = i + nx * (j + ny * k)`), which
/// matches the row-major layout a `wgpu` 3D texture expects (x along the row, y
/// down the image, z through the stack), so the render path can upload it
/// without reshuffling.
#[derive(Debug, Clone)]
pub struct Volume {
    /// Grid dimensions `[nx, ny, nz]` in voxels.
    pub dimensions: [usize; 3],
    /// Per-voxel scalar values, i-fastest.
    pub data: Vec<f32>,
    /// Voxel<->world coordinate transforms for placing slice planes in the scene.
    pub space: VolumeSpace,
    /// Smallest scalar value in `data`.
    pub min_value: f32,
    /// Largest scalar value in `data`.
    pub max_value: f32,
}

impl Volume {
    /// Read any volume format supported by the viewer.
    ///
    /// AFNI datasets may be named by their `.HEAD`, `.BRIK`, `.BRIK.gz`, or
    /// AFNI prefix (for example `anat+tlrc.`). All other inputs are passed to
    /// the NIfTI reader.
    pub fn read(path: &Path) -> Result<Self> {
        if let Some(paths) = AfniPaths::resolve(path)? {
            return Self::read_afni(&paths);
        }
        Self::read_nifti(path)
    }

    /// Read a NIfTI (`.nii`/`.nii.gz`) file into a `Volume`.
    ///
    /// The voxel->world transform comes from the file's sform/qform affine (the
    /// same one AFNI reports as its geometry matrix), so slice planes land in
    /// the world space the surfaces share.
    pub fn read_nifti(path: &std::path::Path) -> Result<Self> {
        let object = ReaderOptions::new()
            .read_file(path)
            .with_context(|| format!("failed to read volume {}", path.display()))?;

        let affine = object.header().affine::<f32>();
        let dim = object.volume().dim().to_vec();
        anyhow::ensure!(
            dim.len() >= 3,
            "volume {} has fewer than 3 dimensions ({dim:?})",
            path.display()
        );
        let dimensions = [dim[0] as usize, dim[1] as usize, dim[2] as usize];
        let [nx, ny, nz] = dimensions;

        // Pull the first 3D frame as f32 in canonical (i,j,k) ndarray order, then
        // copy into our i-fastest buffer.
        let array = object
            .into_volume()
            .into_ndarray::<f32>()
            .with_context(|| format!("failed to decode volume data in {}", path.display()))?;

        let mut data = vec![0.0_f32; nx * ny * nz];
        let mut min_value = f32::INFINITY;
        let mut max_value = f32::NEG_INFINITY;
        for (idx, &value) in array.indexed_iter() {
            let (i, j, k) = (idx[0], idx[1], idx[2]);
            if i >= nx || j >= ny || k >= nz {
                continue;
            }
            let value = if value.is_finite() { value } else { 0.0 };
            data[i + nx * (j + ny * k)] = value;
            min_value = min_value.min(value);
            max_value = max_value.max(value);
        }
        if !min_value.is_finite() || !max_value.is_finite() {
            min_value = 0.0;
            max_value = 0.0;
        }

        let voxel_to_world = SurfaceTransform::from_matrix(nalgebra_affine_to_cols(&affine));
        let space = VolumeSpace::new(dimensions, voxel_to_world)?;

        Ok(Self {
            dimensions,
            data,
            space,
            min_value,
            max_value,
        })
    }

    fn read_afni(paths: &AfniPaths) -> Result<Self> {
        let header_bytes = std::fs::read(&paths.head)
            .with_context(|| format!("failed to read AFNI header {}", paths.head.display()))?;
        let header = AfniHeader::parse(&header_bytes)
            .with_context(|| format!("failed to parse AFNI header {}", paths.head.display()))?;

        let dimensions_values = header.ints("DATASET_DIMENSIONS")?;
        ensure!(
            dimensions_values.len() >= 3,
            "AFNI DATASET_DIMENSIONS must contain at least 3 values"
        );
        let mut dimensions = [0_usize; 3];
        for axis in 0..3 {
            dimensions[axis] = usize::try_from(dimensions_values[axis]).with_context(|| {
                format!("AFNI dimension {} is negative", dimensions_values[axis])
            })?;
            ensure!(dimensions[axis] > 0, "AFNI dimensions must be positive");
        }
        let voxel_count = dimensions
            .into_iter()
            .try_fold(1_usize, |size, dim| size.checked_mul(dim))
            .context("AFNI volume dimensions overflow addressable memory")?;

        // AFNI's own loader defaults old datasets without BRICK_TYPES to short.
        let brick_type = match header.ints_optional("BRICK_TYPES") {
            Some(values) => *values.first().context("AFNI BRICK_TYPES is empty")?,
            None => 1,
        };
        let scale = header
            .floats_optional("BRICK_FLOAT_FACS")
            .and_then(|values| values.first().copied())
            .filter(|factor| *factor > 0.0)
            .unwrap_or(1.0);
        let little_endian = match header.string_optional("BYTEORDER_STRING") {
            Some(value) if value.starts_with("LSB_FIRST") => true,
            Some(value) if value.starts_with("MSB_FIRST") => false,
            Some(value) => bail!("unsupported AFNI BYTEORDER_STRING {value:?}"),
            // This is also AFNI's compatibility behavior for older headers.
            None => cfg!(target_endian = "little"),
        };

        let reader: Box<dyn Read> =
            if paths.brik.extension().is_some_and(|ext| ext == "gz") {
                Box::new(GzDecoder::new(File::open(&paths.brik).with_context(
                    || format!("failed to open AFNI brick {}", paths.brik.display()),
                )?))
            } else {
                Box::new(File::open(&paths.brik).with_context(|| {
                    format!("failed to open AFNI brick {}", paths.brik.display())
                })?)
            };
        let data = decode_afni_brick(
            BufReader::new(reader),
            brick_type,
            voxel_count,
            little_endian,
            scale,
        )
        .with_context(|| format!("failed to decode AFNI brick {}", paths.brik.display()))?;
        let (min_value, max_value) = finite_range(&data);

        let voxel_to_world = SurfaceTransform::from_matrix(header.voxel_to_ras()?);
        let space = VolumeSpace::new(dimensions, voxel_to_world)?;
        Ok(Self {
            dimensions,
            data,
            space,
            min_value,
            max_value,
        })
    }

    /// Sample the scalar value at integer voxel coordinates, if in range.
    pub fn sample(&self, i: usize, j: usize, k: usize) -> Option<f32> {
        let [nx, ny, nz] = self.dimensions;
        if i >= nx || j >= ny || k >= nz {
            return None;
        }
        Some(self.data[i + nx * (j + ny * k)])
    }
}

#[derive(Debug)]
struct AfniPaths {
    head: PathBuf,
    brik: PathBuf,
}

impl AfniPaths {
    fn resolve(path: &Path) -> Result<Option<Self>> {
        let text = path.as_os_str().to_string_lossy();
        let explicit_afni = text.ends_with(".HEAD")
            || text.ends_with(".BRIK")
            || text.ends_with(".BRIK.gz")
            || text.ends_with("+orig")
            || text.ends_with("+orig.")
            || text.ends_with("+acpc")
            || text.ends_with("+acpc.")
            || text.ends_with("+tlrc")
            || text.ends_with("+tlrc.");
        if !explicit_afni {
            return Ok(None);
        }

        let base = text
            .strip_suffix(".BRIK.gz")
            .or_else(|| text.strip_suffix(".BRIK"))
            .or_else(|| text.strip_suffix(".HEAD"))
            .unwrap_or(&text);
        let base = base.strip_suffix('.').unwrap_or(base);
        let head = PathBuf::from(format!("{base}.HEAD"));
        if !head.is_file() {
            bail!("AFNI header {} does not exist", head.display());
        }

        let uncompressed = PathBuf::from(format!("{base}.BRIK"));
        let compressed = PathBuf::from(format!("{base}.BRIK.gz"));
        let input_was_compressed = text.ends_with(".BRIK.gz");
        let brik = if input_was_compressed && compressed.is_file() {
            compressed
        } else if uncompressed.is_file() {
            uncompressed
        } else if compressed.is_file() {
            compressed
        } else {
            bail!(
                "AFNI dataset {} has no matching .BRIK or .BRIK.gz file",
                head.display()
            );
        };
        Ok(Some(Self { head, brik }))
    }
}

#[derive(Debug)]
enum AfniAttribute {
    Ints(Vec<i32>),
    Floats(Vec<f32>),
    String(String),
}

#[derive(Debug)]
struct AfniHeader {
    attributes: HashMap<String, AfniAttribute>,
}

impl AfniHeader {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let mut scanner = HeaderScanner::new(bytes);
        let mut attributes = HashMap::new();
        while scanner.has_more() {
            scanner.expect_token("type")?;
            scanner.expect_token("=")?;
            let kind = scanner.token()?.to_owned();
            scanner.expect_token("name")?;
            scanner.expect_token("=")?;
            let name = scanner.token()?.to_owned();
            scanner.expect_token("count")?;
            scanner.expect_token("=")?;
            let count: usize = scanner
                .token()?
                .parse()
                .context("invalid AFNI attribute count")?;
            if count == 0 {
                continue;
            }
            let value = match kind.as_str() {
                "integer-attribute" => AfniAttribute::Ints(
                    (0..count)
                        .map(|_| {
                            scanner
                                .token()?
                                .parse()
                                .context("invalid AFNI integer value")
                        })
                        .collect::<Result<_>>()?,
                ),
                "float-attribute" => AfniAttribute::Floats(
                    (0..count)
                        .map(|_| scanner.token()?.parse().context("invalid AFNI float value"))
                        .collect::<Result<_>>()?,
                ),
                "string-attribute" => {
                    scanner.skip_whitespace();
                    scanner.expect_byte(b'\'')?;
                    let raw = scanner.take(count)?;
                    // AFNI uses '~' as an on-disk stand-in for embedded NULs.
                    AfniAttribute::String(
                        String::from_utf8_lossy(raw)
                            .replace('~', "\0")
                            .trim_end_matches('\0')
                            .to_owned(),
                    )
                }
                _ => bail!("unsupported AFNI attribute type {kind:?}"),
            };
            attributes.insert(name, value);
        }
        Ok(Self { attributes })
    }

    fn ints(&self, name: &str) -> Result<&[i32]> {
        match self.attributes.get(name) {
            Some(AfniAttribute::Ints(values)) => Ok(values),
            Some(_) => bail!("AFNI attribute {name} has the wrong type"),
            None => bail!("AFNI header is missing {name}"),
        }
    }

    fn ints_optional(&self, name: &str) -> Option<&[i32]> {
        match self.attributes.get(name) {
            Some(AfniAttribute::Ints(values)) => Some(values),
            _ => None,
        }
    }

    fn floats_optional(&self, name: &str) -> Option<&[f32]> {
        match self.attributes.get(name) {
            Some(AfniAttribute::Floats(values)) => Some(values),
            _ => None,
        }
    }

    fn string_optional(&self, name: &str) -> Option<&str> {
        match self.attributes.get(name) {
            Some(AfniAttribute::String(value)) => Some(value),
            _ => None,
        }
    }

    fn voxel_to_ras(&self) -> Result<[[f32; 4]; 4]> {
        let mut rows = [[0.0_f32; 4]; 4];
        rows[3][3] = 1.0;
        if let Some(values) = self.floats_optional("IJK_TO_DICOM_REAL") {
            ensure!(
                values.len() >= 12,
                "AFNI IJK_TO_DICOM_REAL must contain 12 values"
            );
            for row in 0..3 {
                rows[row].copy_from_slice(&values[row * 4..row * 4 + 4]);
            }
        } else {
            let orientations = self.ints("ORIENT_SPECIFIC")?;
            let origins = self
                .floats_optional("ORIGIN")
                .context("AFNI header is missing ORIGIN")?;
            let deltas = self
                .floats_optional("DELTA")
                .context("AFNI header is missing DELTA")?;
            ensure!(
                orientations.len() >= 3 && origins.len() >= 3 && deltas.len() >= 3,
                "AFNI orientation geometry attributes must contain 3 values"
            );
            for axis in 0..3 {
                let dicom_axis = match orientations[axis] {
                    0 | 1 => 0,
                    2 | 3 => 1,
                    4 | 5 => 2,
                    code => bail!("unsupported AFNI orientation code {code}"),
                };
                rows[dicom_axis][axis] = deltas[axis];
                rows[dicom_axis][3] = origins[axis];
            }
        }
        // AFNI stores DICOM LPS (called RAI in AFNI); NIfTI and Sumaru use RAS.
        for row in rows.iter_mut().take(2) {
            for value in row {
                *value = -*value;
            }
        }
        let mut columns = [[0.0_f32; 4]; 4];
        for row in 0..4 {
            for column in 0..4 {
                columns[column][row] = rows[row][column];
            }
        }
        Ok(columns)
    }
}

struct HeaderScanner<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> HeaderScanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    fn skip_whitespace(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
    }
    fn has_more(&mut self) -> bool {
        self.skip_whitespace();
        self.position < self.bytes.len()
    }
    fn token(&mut self) -> Result<&'a str> {
        self.skip_whitespace();
        let start = self.position;
        while self
            .bytes
            .get(self.position)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            self.position += 1;
        }
        ensure!(self.position > start, "unexpected end of AFNI header");
        std::str::from_utf8(&self.bytes[start..self.position])
            .context("AFNI header token is not UTF-8")
    }
    fn expect_token(&mut self, expected: &str) -> Result<()> {
        let actual = self.token()?;
        ensure!(
            actual == expected,
            "expected {expected:?} in AFNI header, found {actual:?}"
        );
        Ok(())
    }
    fn expect_byte(&mut self, expected: u8) -> Result<()> {
        ensure!(
            self.bytes.get(self.position) == Some(&expected),
            "malformed AFNI string attribute"
        );
        self.position += 1;
        Ok(())
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .context("AFNI string length overflow")?;
        ensure!(end <= self.bytes.len(), "truncated AFNI string attribute");
        let value = &self.bytes[self.position..end];
        self.position = end;
        Ok(value)
    }
}

fn decode_afni_brick(
    mut reader: impl Read,
    brick_type: i32,
    voxel_count: usize,
    little_endian: bool,
    scale: f32,
) -> Result<Vec<f32>> {
    let bytes_per_voxel = match brick_type {
        0 => 1,
        1 => 2,
        2 | 3 => 4,
        4 | 5 => 8,
        6 => 3,
        7 => 4,
        _ => bail!("unsupported AFNI BRICK_TYPES code {brick_type}"),
    };
    let byte_count = voxel_count
        .checked_mul(bytes_per_voxel)
        .context("AFNI brick size overflow")?;
    let mut bytes = vec![0_u8; byte_count];
    reader
        .read_exact(&mut bytes)
        .context("AFNI brick is shorter than its header declares")?;
    let order2 = |chunk: &[u8]| {
        if little_endian {
            [chunk[0], chunk[1]]
        } else {
            [chunk[1], chunk[0]]
        }
    };
    let order4 = |chunk: &[u8]| {
        if little_endian {
            [chunk[0], chunk[1], chunk[2], chunk[3]]
        } else {
            [chunk[3], chunk[2], chunk[1], chunk[0]]
        }
    };
    let order8 = |chunk: &[u8]| {
        if little_endian {
            [
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ]
        } else {
            [
                chunk[7], chunk[6], chunk[5], chunk[4], chunk[3], chunk[2], chunk[1], chunk[0],
            ]
        }
    };
    let mut data = Vec::with_capacity(voxel_count);
    for chunk in bytes.chunks_exact(bytes_per_voxel) {
        let value = match brick_type {
            0 => chunk[0] as f32,
            1 => i16::from_le_bytes(order2(chunk)) as f32,
            2 => i32::from_le_bytes(order4(chunk)) as f32,
            3 => f32::from_le_bytes(order4(chunk)),
            4 => f64::from_le_bytes(order8(chunk)) as f32,
            5 => {
                let real = f32::from_le_bytes(order4(&chunk[..4]));
                let imaginary = f32::from_le_bytes(order4(&chunk[4..]));
                real.hypot(imaginary)
            }
            6 => 0.299 * chunk[0] as f32 + 0.587 * chunk[1] as f32 + 0.114 * chunk[2] as f32,
            7 => {
                (0.299 * chunk[0] as f32 + 0.587 * chunk[1] as f32 + 0.114 * chunk[2] as f32)
                    * (chunk[3] as f32 / 255.0)
            }
            _ => unreachable!(),
        } * scale;
        data.push(if value.is_finite() { value } else { 0.0 });
    }
    Ok(data)
}

fn finite_range(data: &[f32]) -> (f32, f32) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for &value in data {
        min = min.min(value);
        max = max.max(value);
    }
    if min.is_finite() && max.is_finite() {
        (min, max)
    } else {
        (0.0, 0.0)
    }
}

/// Convert a `(row, col)`-indexed 4x4 affine (e.g. nalgebra's `Matrix4`) into
/// the column-major `[[f32; 4]; 4]` (outer index = column) that
/// [`SurfaceTransform::from_matrix`] expects. Generic over the matrix type so we
/// don't take a direct dependency on `nalgebra`.
fn nalgebra_affine_to_cols<M>(affine: &M) -> [[f32; 4]; 4]
where
    M: std::ops::Index<(usize, usize), Output = f32>,
{
    let mut cols = [[0.0_f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            cols[col][row] = affine[(row, col)];
        }
    }
    cols
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_volume_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("testing/SUMA/sub-3_SurfVol.nii")
    }

    #[test]
    fn loads_surfvol_geometry_and_data() {
        let path = sample_volume_path();
        if !path.exists() {
            eprintln!("skipping: sample volume not present at {}", path.display());
            return;
        }

        let volume = Volume::read_nifti(&path).expect("load sample volume");
        assert_eq!(volume.dimensions, [256, 256, 256]);
        assert_eq!(volume.data.len(), 256 * 256 * 256);

        // Byte-valued anatomical: range sits within 0..=255 and is non-trivial.
        assert!(volume.min_value >= 0.0);
        assert!(volume.max_value > volume.min_value);
        assert!(volume.max_value <= 255.0);

        // Voxel [0,0,0] maps near AFNI's reported geometry origin
        // (RAS world ~ (-124.15, +125.15, +122.45) up to RAS/LPI sign convention).
        let origin = volume.space.voxel_to_world([0.0, 0.0, 0.0]);
        assert!(
            origin.iter().any(|c| c.abs() > 100.0),
            "unexpected world origin {origin:?}"
        );
    }

    #[test]
    fn loads_afni_prefix_with_scaling_byte_order_and_ras_geometry() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "sumaru-afni-volume-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create temporary dataset directory");
        let prefix = directory.join("synthetic+tlrc");
        let head = prefix.with_extension("HEAD");
        let brik = prefix.with_extension("BRIK");
        let header = "\
type = integer-attribute
name = DATASET_DIMENSIONS
count = 3
 2 2 1
type = integer-attribute
name = BRICK_TYPES
count = 1
 1
type = float-attribute
name = BRICK_FLOAT_FACS
count = 1
 0.5
type = string-attribute
name = BYTEORDER_STRING
count = 10
'MSB_FIRST~
type = float-attribute
name = IJK_TO_DICOM_REAL
count = 12
 1 0 0 10  0 2 0 20  0 0 3 30
";
        fs::write(&head, header).expect("write synthetic HEAD");
        let brick_bytes: Vec<u8> = [-1_i16, 2, 3, 4]
            .into_iter()
            .flat_map(i16::to_be_bytes)
            .collect();
        fs::write(&brik, &brick_bytes).expect("write synthetic BRIK");

        let requested_prefix = PathBuf::from(format!("{}.", prefix.display()));
        let volume = Volume::read(&requested_prefix).expect("load AFNI prefix");
        assert_eq!(volume.dimensions, [2, 2, 1]);
        assert_eq!(volume.data, [-0.5, 1.0, 1.5, 2.0]);
        assert_eq!(volume.min_value, -0.5);
        assert_eq!(volume.max_value, 2.0);
        assert_eq!(
            volume.space.voxel_to_world([1.0, 1.0, 0.0]),
            [-11.0, -22.0, 30.0]
        );

        // Either member of the pair resolves to the same dataset too.
        assert_eq!(Volume::read(&head).unwrap().data, volume.data);
        assert_eq!(Volume::read(&brik).unwrap().data, volume.data);

        let compressed_brick = prefix.with_extension("BRIK.gz");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&brick_bytes).unwrap();
        fs::write(&compressed_brick, encoder.finish().unwrap()).unwrap();
        fs::remove_file(&brik).unwrap();
        assert_eq!(Volume::read(&requested_prefix).unwrap().data, volume.data);
        assert_eq!(Volume::read(&compressed_brick).unwrap().data, volume.data);
        fs::remove_dir_all(directory).expect("remove temporary dataset directory");
    }

    #[test]
    fn builds_geometry_from_legacy_orientation_attributes() {
        let header = AfniHeader::parse(
            b"type = integer-attribute\nname = ORIENT_SPECIFIC\ncount = 3\n0 3 4\n\
              type = float-attribute\nname = ORIGIN\ncount = 3\n10 20 30\n\
              type = float-attribute\nname = DELTA\ncount = 3\n1 -2 3\n",
        )
        .unwrap();
        let transform = SurfaceTransform::from_matrix(header.voxel_to_ras().unwrap());
        assert_eq!(
            transform.transform_point([1.0, 1.0, 1.0]),
            [-11.0, -18.0, 33.0]
        );
    }
}
