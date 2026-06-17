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

/// Inserts `suffix` between a path's file stem and its extension, e.g.
/// `sumaru_view.png` with suffix `_cmap` becomes `sumaru_view_cmap.png`.
pub(super) fn append_filename_suffix(path: &Path, suffix: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("screenshot");
    let mut file_name = format!("{stem}{suffix}");
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        file_name.push('.');
        file_name.push_str(extension);
    }
    path.with_file_name(file_name)
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

pub(super) fn stitch_horizontal_with_gap(
    images: &[ScreenshotImage],
    gap_width: u32,
    background: [u8; 4],
) -> Result<ScreenshotImage> {
    let first = images
        .first()
        .ok_or_else(|| anyhow!("cannot stitch an empty screenshot montage"))?;
    let image_width = images
        .iter()
        .try_fold(0_u32, |total, image| total.checked_add(image.width))
        .ok_or_else(|| anyhow!("montage image width overflowed"))?;
    let gap_total = gap_width
        .checked_mul(images.len().saturating_sub(1) as u32)
        .ok_or_else(|| anyhow!("montage gap width overflowed"))?;
    let width = image_width
        .checked_add(gap_total)
        .ok_or_else(|| anyhow!("montage image width overflowed"))?;
    let height = images
        .iter()
        .map(|image| image.height)
        .max()
        .unwrap_or(first.height);
    let mut rgba = repeated_pixel_buffer(width, height, background);

    let mut dst_x = 0_u32;
    for image in images {
        let dst_y = ((height - image.height) / 2) as usize;
        for y in 0..image.height as usize {
            let src_start = y * image.width as usize * 4;
            let src_end = src_start + image.width as usize * 4;
            let dst_start = ((dst_y + y) * width as usize + dst_x as usize) * 4;
            rgba[dst_start..dst_start + image.width as usize * 4]
                .copy_from_slice(&image.rgba[src_start..src_end]);
        }
        dst_x = dst_x
            .checked_add(image.width)
            .and_then(|x| x.checked_add(gap_width))
            .ok_or_else(|| anyhow!("montage image width overflowed"))?;
    }

    ScreenshotImage::new(width, height, rgba)
}

pub(super) fn crop_to_content(
    image: &ScreenshotImage,
    background: [u8; 4],
    tolerance: u8,
    padding: u32,
) -> Result<ScreenshotImage> {
    let Some((mut min_x, mut min_y, mut max_x, mut max_y)) =
        content_bounds(image, background, tolerance)
    else {
        return Ok(image.clone());
    };

    min_x = min_x.saturating_sub(padding);
    min_y = min_y.saturating_sub(padding);
    max_x = (max_x + padding).min(image.width - 1);
    max_y = (max_y + padding).min(image.height - 1);

    let width = max_x - min_x + 1;
    let height = max_y - min_y + 1;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in min_y..=max_y {
        let start = ((y * image.width + min_x) * 4) as usize;
        let end = start + width as usize * 4;
        rgba.extend_from_slice(&image.rgba[start..end]);
    }

    ScreenshotImage::new(width, height, rgba)
}

pub(super) fn pad_image(
    image: &ScreenshotImage,
    horizontal_padding: u32,
    vertical_padding: u32,
    background: [u8; 4],
) -> Result<ScreenshotImage> {
    let width = image
        .width
        .checked_add(horizontal_padding.saturating_mul(2))
        .ok_or_else(|| anyhow!("padded image width overflowed"))?;
    let height = image
        .height
        .checked_add(vertical_padding.saturating_mul(2))
        .ok_or_else(|| anyhow!("padded image height overflowed"))?;
    let mut rgba = repeated_pixel_buffer(width, height, background);

    for y in 0..image.height as usize {
        let src_start = y * image.width as usize * 4;
        let src_end = src_start + image.width as usize * 4;
        let dst_start = (((y as u32 + vertical_padding) * width + horizontal_padding) * 4) as usize;
        rgba[dst_start..dst_start + image.width as usize * 4]
            .copy_from_slice(&image.rgba[src_start..src_end]);
    }

    ScreenshotImage::new(width, height, rgba)
}

fn content_bounds(
    image: &ScreenshotImage,
    background: [u8; 4],
    tolerance: u8,
) -> Option<(u32, u32, u32, u32)> {
    let mut min_x = image.width;
    let mut min_y = image.height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;

    for y in 0..image.height {
        for x in 0..image.width {
            let index = ((y * image.width + x) * 4) as usize;
            let pixel = &image.rgba[index..index + 4];
            if pixel_differs_from_background(pixel, background, tolerance) {
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    found.then_some((min_x, min_y, max_x, max_y))
}

fn pixel_differs_from_background(pixel: &[u8], background: [u8; 4], tolerance: u8) -> bool {
    pixel
        .iter()
        .zip(background)
        .take(3)
        .any(|(channel, background)| channel.abs_diff(background) > tolerance)
}

fn repeated_pixel_buffer(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for _ in 0..width as usize * height as usize {
        rgba.extend_from_slice(&pixel);
    }

    rgba
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        ScreenshotImage, append_filename_suffix, append_png_extension, crop_to_content, pad_image,
        padded_bytes_per_row, stitch_horizontal, stitch_horizontal_with_gap, texture_bytes_to_rgba,
    };

    #[test]
    fn filename_suffix_is_inserted_before_extension() {
        assert_eq!(
            append_filename_suffix(&PathBuf::from("/tmp/sumaru_view.png"), "_cmap"),
            PathBuf::from("/tmp/sumaru_view_cmap.png")
        );
        assert_eq!(
            append_filename_suffix(&PathBuf::from("montage"), "_cmap"),
            PathBuf::from("montage_cmap")
        );
    }

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

    #[test]
    fn images_stitch_with_gap_and_vertical_centering() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let black = [0, 0, 0, 255];
        let left = ScreenshotImage::new(1, 3, red.repeat(3)).unwrap();
        let right = ScreenshotImage::new(1, 1, blue.to_vec()).unwrap();
        let montage = stitch_horizontal_with_gap(&[left, right], 2, black).unwrap();

        assert_eq!(montage.width, 4);
        assert_eq!(montage.height, 3);
        let middle_right_pixel = ((1 * montage.width + 3) * 4) as usize;
        assert_eq!(
            &montage.rgba[middle_right_pixel..middle_right_pixel + 4],
            &blue
        );
        let top_right_pixel = 3 * 4;
        assert_eq!(&montage.rgba[top_right_pixel..top_right_pixel + 4], &black);
    }

    #[test]
    fn crop_to_content_removes_background_border() {
        let black = [0, 0, 0, 255];
        let white = [255, 255, 255, 255];
        let mut rgba = black.repeat(9);
        rgba[4 * 4..4 * 4 + 4].copy_from_slice(&white);
        let image = ScreenshotImage::new(3, 3, rgba).unwrap();
        let cropped = crop_to_content(&image, black, 2, 0).unwrap();

        assert_eq!(cropped.width, 1);
        assert_eq!(cropped.height, 1);
        assert_eq!(cropped.rgba, white);
    }

    #[test]
    fn pad_image_adds_background_border() {
        let black = [0, 0, 0, 255];
        let white = [255, 255, 255, 255];
        let image = ScreenshotImage::new(1, 1, white.to_vec()).unwrap();
        let padded = pad_image(&image, 2, 1, black).unwrap();

        assert_eq!(padded.width, 5);
        assert_eq!(padded.height, 3);
        let center = ((1 * padded.width + 2) * 4) as usize;
        assert_eq!(&padded.rgba[center..center + 4], &white);
        assert_eq!(&padded.rgba[0..4], &black);
    }
}
