use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

const TRIANGLE_FILE_MAGIC: u32 = 16_777_214;
const QUAD_FILE_MAGIC: u32 = 16_777_215;
const NEW_QUAD_FILE_MAGIC: u32 = 16_777_213;

#[derive(Debug, Clone, PartialEq)]
pub struct FreeSurferSurface {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

pub fn read_freesurfer_surface(path: impl AsRef<Path>) -> Result<FreeSurferSurface> {
    let path = path.as_ref();
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read FreeSurfer surface {}", path.display()))?;
    parse_freesurfer_surface(&bytes)
        .with_context(|| format!("failed to parse FreeSurfer surface {}", path.display()))
}

pub fn parse_freesurfer_surface(bytes: &[u8]) -> Result<FreeSurferSurface> {
    let mut reader = BigEndianReader::new(bytes);
    let magic = reader.read_u24("surface magic")?;
    match magic {
        TRIANGLE_FILE_MAGIC => {}
        QUAD_FILE_MAGIC | NEW_QUAD_FILE_MAGIC => {
            bail!(
                "legacy FreeSurfer quad surfaces are not supported; convert the surface to the triangular format"
            )
        }
        _ => bail!("not a FreeSurfer binary surface (magic {magic})"),
    }

    reader.read_line("creation stamp")?;
    reader.read_line("comment")?;
    let vertex_count = reader.read_count("vertex count")?;
    let face_count = reader.read_count("face count")?;
    ensure!(vertex_count > 0, "FreeSurfer surface has no vertices");
    ensure!(face_count > 0, "FreeSurfer surface has no faces");

    let mut vertices = Vec::with_capacity(vertex_count);
    for vertex in 0..vertex_count {
        let point = [
            reader.read_f32("vertex coordinate")?,
            reader.read_f32("vertex coordinate")?,
            reader.read_f32("vertex coordinate")?,
        ];
        ensure!(
            point.iter().all(|value| value.is_finite()),
            "FreeSurfer vertex {vertex} contains a non-finite coordinate"
        );
        vertices.push(point);
    }

    let mut triangles = Vec::with_capacity(face_count);
    for face in 0..face_count {
        let mut triangle = [0_u32; 3];
        for node in &mut triangle {
            let index = reader.read_i32("face index")?;
            ensure!(
                index >= 0,
                "FreeSurfer face {face} has negative index {index}"
            );
            *node = index as u32;
            ensure!(
                (*node as usize) < vertex_count,
                "FreeSurfer face {face} references node {} outside vertex count {vertex_count}",
                *node
            );
        }
        triangles.push(triangle);
    }

    // FreeSurfer may append volume-geometry metadata after the face array. It
    // is not needed to display a surface, so intentionally allow trailing data.
    Ok(FreeSurferSurface {
        vertices,
        triangles,
    })
}

struct BigEndianReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> BigEndianReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take<const N: usize>(&mut self, label: &str) -> Result<[u8; N]> {
        let end = self.offset.checked_add(N).context("file offset overflow")?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .with_context(|| format!("truncated FreeSurfer surface while reading {label}"))?;
        self.offset = end;
        Ok(slice.try_into().expect("slice length checked"))
    }

    fn read_u24(&mut self, label: &str) -> Result<u32> {
        let bytes = self.take::<3>(label)?;
        Ok(u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]))
    }

    fn read_i32(&mut self, label: &str) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(label)?))
    }

    fn read_count(&mut self, label: &str) -> Result<usize> {
        let value = self.read_i32(label)?;
        ensure!(value >= 0, "FreeSurfer {label} is negative ({value})");
        Ok(value as usize)
    }

    fn read_f32(&mut self, label: &str) -> Result<f32> {
        Ok(f32::from_be_bytes(self.take(label)?))
    }

    fn read_line(&mut self, label: &str) -> Result<&'a [u8]> {
        let rest = self
            .bytes
            .get(self.offset..)
            .context("file offset outside FreeSurfer surface")?;
        let length = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .with_context(|| format!("truncated FreeSurfer surface while reading {label}"))?;
        let line = &rest[..length];
        self.offset += length + 1;
        Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use super::{TRIANGLE_FILE_MAGIC, parse_freesurfer_surface};

    fn push_u24(bytes: &mut Vec<u8>, value: u32) {
        let encoded = value.to_be_bytes();
        bytes.extend_from_slice(&encoded[1..]);
    }

    #[test]
    fn parses_triangle_surface_and_ignores_footer() {
        let mut bytes = Vec::new();
        push_u24(&mut bytes, TRIANGLE_FILE_MAGIC);
        bytes.extend_from_slice(b"created by test\n\n");
        bytes.extend_from_slice(&3_i32.to_be_bytes());
        bytes.extend_from_slice(&1_i32.to_be_bytes());
        for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        for index in [0_i32, 1, 2] {
            bytes.extend_from_slice(&index.to_be_bytes());
        }
        bytes.extend_from_slice(b"footer");

        let surface = parse_freesurfer_surface(&bytes).unwrap();
        assert_eq!(surface.vertices.len(), 3);
        assert_eq!(surface.triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn rejects_out_of_bounds_face() {
        let mut bytes = Vec::new();
        push_u24(&mut bytes, TRIANGLE_FILE_MAGIC);
        bytes.extend_from_slice(b"stamp\ncomment\n");
        bytes.extend_from_slice(&1_i32.to_be_bytes());
        bytes.extend_from_slice(&1_i32.to_be_bytes());
        for _ in 0..3 {
            bytes.extend_from_slice(&0_f32.to_be_bytes());
        }
        for index in [0_i32, 0, 1] {
            bytes.extend_from_slice(&index.to_be_bytes());
        }
        assert!(parse_freesurfer_surface(&bytes).is_err());
    }
}
