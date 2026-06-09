use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nifti::{NiftiObject, NiftiVolume, ReaderOptions};

use crate::io::read_gifti_image;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Gifti,
    Nifti,
}

impl fmt::Display for FileKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileKind::Gifti => f.write_str("GIFTI"),
            FileKind::Nifti => f.write_str("NIFTI"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InspectReport {
    pub path: PathBuf,
    pub kind: FileKind,
    pub summary: String,
}

impl fmt::Display for InspectReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} ({})", self.path.display(), self.kind)?;
        write!(f, "{}", self.summary)
    }
}

pub fn inspect_path(path: impl AsRef<Path>) -> Result<InspectReport> {
    let path = path.as_ref();

    if !path.exists() {
        bail!("{} does not exist", path.display());
    }

    match detect_file_kind(path) {
        Some(FileKind::Gifti) => inspect_gifti(path),
        Some(FileKind::Nifti) => inspect_nifti(path),
        None => bail!("unsupported file type: {}", path.display()),
    }
}

pub fn detect_file_kind(path: &Path) -> Option<FileKind> {
    let filename = path.file_name()?.to_string_lossy().to_ascii_lowercase();

    if filename.ends_with(".gii")
        || filename.ends_with(".gii.gz")
        || filename.ends_with(".gii.dset")
        || filename.ends_with(".gii.dset.gz")
    {
        return Some(FileKind::Gifti);
    }

    if filename.ends_with(".nii")
        || filename.ends_with(".nii.gz")
        || filename.ends_with(".hdr")
        || filename.ends_with(".hdr.gz")
        || filename.ends_with(".img")
        || filename.ends_with(".img.gz")
    {
        return Some(FileKind::Nifti);
    }

    None
}

fn inspect_gifti(path: &Path) -> Result<InspectReport> {
    let image = read_gifti_image(path)
        .with_context(|| format!("failed to read GIFTI file {}", path.display()))?;

    let pointsets = image
        .data_arrays
        .iter()
        .filter(|array| array.intent == gifti_rs::intent::POINTSET)
        .count();
    let triangles = image
        .data_arrays
        .iter()
        .filter(|array| array.intent == gifti_rs::intent::TRIANGLE)
        .count();
    let labels = image
        .label_table
        .as_ref()
        .map_or(0, |table| table.labels.len());

    Ok(InspectReport {
        path: path.to_path_buf(),
        kind: FileKind::Gifti,
        summary: format!(
            "version: {}\ndata arrays: {}\npointset arrays: {}\ntriangle arrays: {}\nlabels: {}",
            image.version,
            image.data_arrays.len(),
            pointsets,
            triangles,
            labels
        ),
    })
}

fn inspect_nifti(path: &Path) -> Result<InspectReport> {
    let object = ReaderOptions::new()
        .read_file(path)
        .with_context(|| format!("failed to read NIFTI file {}", path.display()))?;
    let header = object.header();
    let dims = object.volume().dim().to_vec();

    Ok(InspectReport {
        path: path.to_path_buf(),
        kind: FileKind::Nifti,
        summary: format!(
            "dimensions: {:?}\ndatatype code: {}\nintent code: {}\nqform code: {}\nsform code: {}",
            dims, header.datatype, header.intent_code, header.qform_code, header.sform_code
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::{FileKind, detect_file_kind};
    use std::path::Path;

    #[test]
    fn detects_gifti_files() {
        assert_eq!(
            detect_file_kind(Path::new("lh.pial.surf.gii")),
            Some(FileKind::Gifti)
        );
        assert_eq!(
            detect_file_kind(Path::new("rh.shape.gii.gz")),
            Some(FileKind::Gifti)
        );
        assert_eq!(
            detect_file_kind(Path::new("rh.thickness.gii.dset")),
            Some(FileKind::Gifti)
        );
        assert_eq!(
            detect_file_kind(Path::new("rh.thickness.gii.dset.gz")),
            Some(FileKind::Gifti)
        );
    }

    #[test]
    fn detects_nifti_files() {
        assert_eq!(
            detect_file_kind(Path::new("bold.nii")),
            Some(FileKind::Nifti)
        );
        assert_eq!(
            detect_file_kind(Path::new("bold.nii.gz")),
            Some(FileKind::Nifti)
        );
        assert_eq!(
            detect_file_kind(Path::new("struct.hdr.gz")),
            Some(FileKind::Nifti)
        );
        assert_eq!(
            detect_file_kind(Path::new("struct.img.gz")),
            Some(FileKind::Nifti)
        );
    }

    #[test]
    fn rejects_unknown_files() {
        assert_eq!(detect_file_kind(Path::new("surface.obj")), None);
    }
}
