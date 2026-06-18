//! Volume loading for the `--volume` viewer mode: read a NIfTI file into a
//! dense scalar grid plus the voxel<->world transform, ready for orthogonal
//! slice-plane rendering. The coordinate math lives in
//! [`crate::surface::VolumeSpace`]; this module owns the voxel data itself.

use anyhow::{Context, Result};
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

    /// Sample the scalar value at integer voxel coordinates, if in range.
    pub fn sample(&self, i: usize, j: usize, k: usize) -> Option<f32> {
        let [nx, ny, nz] = self.dimensions;
        if i >= nx || j >= ny || k >= nz {
            return None;
        }
        Some(self.data[i + nx * (j + ny * k)])
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
    use std::path::Path;

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
}
