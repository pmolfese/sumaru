use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScreenshotImage {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

impl ScreenshotImage {
    pub(super) fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected_len = width as usize * height as usize * 4;
        if rgba.len() != expected_len {
            bail!(
                "screenshot buffer has {} bytes, expected {expected_len}",
                rgba.len()
            );
        }

        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

pub(super) fn append_png_extension(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|extension| {
        extension
            .to_str()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    }) {
        return path;
    }

    let mut path = path;
    path.set_extension("png");
    path
}

pub(super) fn save_png(path: &Path, image: &ScreenshotImage) -> Result<()> {
    image::save_buffer_with_format(
        path,
        &image.rgba,
        image.width,
        image.height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .with_context(|| format!("failed to save screenshot {}", path.display()))
}

pub(super) fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = unpadded_bytes_per_row(width);
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

    unpadded.div_ceil(alignment) * alignment
}

pub(super) fn unpadded_bytes_per_row(width: u32) -> u32 {
    width * 4
}

pub(super) fn texture_bytes_to_rgba(
    texture_bytes: &[u8],
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    format: wgpu::TextureFormat,
) -> Result<Vec<u8>> {
    let unpadded_bytes_per_row = unpadded_bytes_per_row(width);
    let expected_len = padded_bytes_per_row as usize * height as usize;
    if texture_bytes.len() < expected_len {
        bail!(
            "screenshot readback returned {} bytes, expected at least {expected_len}",
            texture_bytes.len()
        );
    }

    let channel_order = match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => [0, 1, 2, 3],
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => [2, 1, 0, 3],
        _ => bail!("surface format {format:?} is not supported for screenshot export"),
    };

    let mut rgba = Vec::with_capacity(unpadded_bytes_per_row as usize * height as usize);
    for row in texture_bytes
        .chunks(padded_bytes_per_row as usize)
        .take(height as usize)
    {
        let row = row
            .get(..unpadded_bytes_per_row as usize)
            .ok_or_else(|| anyhow!("screenshot row is shorter than expected"))?;
        for pixel in row.chunks_exact(4) {
            rgba.extend_from_slice(&[
                pixel[channel_order[0]],
                pixel[channel_order[1]],
                pixel[channel_order[2]],
                pixel[channel_order[3]],
            ]);
        }
    }

    Ok(rgba)
}

pub(super) fn stitch_horizontal(images: &[ScreenshotImage]) -> Result<ScreenshotImage> {
    let first = images
        .first()
        .ok_or_else(|| anyhow!("cannot stitch an empty screenshot montage"))?;
    if images.iter().any(|image| image.height != first.height) {
        bail!("all montage images must have the same height");
    }

    let width = images
        .iter()
        .try_fold(0_u32, |total, image| total.checked_add(image.width))
        .ok_or_else(|| anyhow!("montage image width overflowed"))?;
    let height = first.height;
    let mut rgba = vec![0; width as usize * height as usize * 4];

    for y in 0..height as usize {
        let mut dst_x = 0_usize;
        for image in images {
            let row_len = image.width as usize * 4;
            let src_start = y * row_len;
            let dst_start = (y * width as usize + dst_x) * 4;
            rgba[dst_start..dst_start + row_len]
                .copy_from_slice(&image.rgba[src_start..src_start + row_len]);
            dst_x += image.width as usize;
        }
    }

    ScreenshotImage::new(width, height, rgba)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        ScreenshotImage, append_png_extension, padded_bytes_per_row, stitch_horizontal,
        texture_bytes_to_rgba,
    };

    #[test]
    fn png_extension_is_added_when_missing() {
        assert_eq!(
            append_png_extension(PathBuf::from("sumaru_view"))
                .display()
                .to_string(),
            "sumaru_view.png"
        );
        assert_eq!(
            append_png_extension(PathBuf::from("sumaru_view.PNG"))
                .display()
                .to_string(),
            "sumaru_view.PNG"
        );
    }

    #[test]
    fn padded_rows_align_to_wgpu_copy_alignment() {
        let padded = padded_bytes_per_row(17);

        assert!(padded >= 17 * 4);
        assert_eq!(padded % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
    }

    #[test]
    fn bgra_texture_bytes_are_converted_to_rgba() {
        let rgba =
            texture_bytes_to_rgba(&[30, 20, 10, 255], 1, 1, 4, wgpu::TextureFormat::Bgra8Unorm)
                .unwrap();

        assert_eq!(rgba, vec![10, 20, 30, 255]);
    }

    #[test]
    fn images_stitch_left_to_right() {
        let left = ScreenshotImage::new(1, 1, vec![255, 0, 0, 255]).unwrap();
        let right = ScreenshotImage::new(1, 1, vec![0, 0, 255, 255]).unwrap();
        let montage = stitch_horizontal(&[left, right]).unwrap();

        assert_eq!(montage.width, 2);
        assert_eq!(montage.height, 1);
        assert_eq!(montage.rgba, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }
}
