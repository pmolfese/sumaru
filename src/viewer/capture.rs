//! Screenshot and montage capture on `ViewerState`: saving the current view,
//! preset montages, the colorbar companion image, camera framing, and offscreen
//! surface capture. Extracted from `viewer/mod.rs`. Distinct from
//! `screenshot.rs`, which owns the `ScreenshotImage` pixel type and PNG I/O.

use super::*;

impl ViewerState {
    /// Capture the current 3D view to a PNG (key `r`), plus colorbar companion.
    pub(super) fn save_current_view_screenshot(&mut self) -> Result<()> {
        if !self.has_renderable_surface() {
            self.log_status("Load a surface before saving a screenshot.");
            return Ok(());
        }

        let Some(path) = save_screenshot_file(
            "Save current view",
            &timestamped_png_name("sumaru_view"),
            self.surface_path.as_ref(),
        ) else {
            self.log_status("Screenshot save cancelled.");
            return Ok(());
        };

        let camera = self.camera.clone();
        let image = self.capture_surface_view(&camera)?;
        screenshot::save_png(&path, &image)?;
        self.log_status(format!("Saved screenshot {}.", path.display()));
        self.save_colormap_companion(&image, &path)?;

        Ok(())
    }

    /// Capture the 1x4 preset montage to a PNG (Shift+`R`).
    pub(super) fn save_preset_montage_screenshot(&mut self) -> Result<()> {
        if !self.has_renderable_surface() {
            self.log_status("Load a surface before saving a montage.");
            return Ok(());
        }

        let title = if self.has_both_scene() {
            "Save paired top/bottom/acorn montage"
        } else {
            "Save left/right/top/bottom montage"
        };
        let Some(path) = save_screenshot_file(
            title,
            &timestamped_png_name("sumaru_montage"),
            self.surface_path.as_ref(),
        ) else {
            self.log_status("Montage save cancelled.");
            return Ok(());
        };

        let result = if self.has_both_scene() {
            self.capture_paired_spec_montage()
        } else {
            self.capture_standard_montage()
        };
        self.update();

        let montage = result?;
        screenshot::save_png(&path, &montage)?;
        self.log_status(format!("Saved montage {}.", path.display()));
        self.save_colormap_companion(&montage, &path)?;

        Ok(())
    }

    /// When a thresholded overlay is active, writes a second copy of `base`
    /// with the active colormap drawn along the right edge. The companion file
    /// reuses `path` with `_cmap` inserted before the extension. No-op when no
    /// thresholded overlay is shown.
    pub(super) fn save_colormap_companion(
        &self,
        base: &ScreenshotImage,
        path: &Path,
    ) -> Result<()> {
        if !self.has_thresholded_overlay() {
            return Ok(());
        }

        let background = self.controller.display.background.rgba8();
        let panel = self.colorbar_panel_image(base, background);
        let with_colorbar = screenshot::stitch_horizontal(&[base.clone(), panel])?;
        let cmap_path = screenshot::append_filename_suffix(path, "_cmap");
        screenshot::save_png(&cmap_path, &with_colorbar)?;
        self.log_status(format!(
            "Saved screenshot with colormap {}.",
            cmap_path.display()
        ));

        Ok(())
    }

    /// True when an overlay is loaded and its threshold is enabled.
    pub(super) fn has_thresholded_overlay(&self) -> bool {
        self.overlay.is_loaded() && self.overlay.render.appearance.threshold.enabled
    }

    /// Builds a right-side panel the same height as `base` containing a vertical
    /// colorbar for the active overlay colormap (max at top, min at bottom).
    pub(super) fn colorbar_panel_image(
        &self,
        base: &ScreenshotImage,
        background: [u8; 4],
    ) -> ScreenshotImage {
        let height = base.height.max(1);
        let bar_width = (base.width / 25).clamp(20, 60);
        let left_margin = bar_width / 2;
        let right_margin = bar_width;
        let panel_width = left_margin + bar_width + right_margin;

        let vertical_margin = (height / 12).max(4).min(height / 2);
        let bar_top = vertical_margin;
        let bar_bottom = height.saturating_sub(vertical_margin).max(bar_top + 1);
        let bar_height = bar_bottom - bar_top;
        let border = contrasting_border(background);

        let mut rgba = vec![0_u8; panel_width as usize * height as usize * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&background);
        }

        let mut set_pixel = |x: u32, y: u32, color: [u8; 4]| {
            if x < panel_width && y < height {
                let index = ((y * panel_width + x) * 4) as usize;
                rgba[index..index + 4].copy_from_slice(&color);
            }
        };

        for y in bar_top..bar_bottom {
            let t = if bar_height > 1 {
                1.0 - (y - bar_top) as f32 / (bar_height - 1) as f32
            } else {
                1.0
            };
            let color = sample_colormap(self.overlay.render.appearance.colormap, t);
            let rgba8 = [
                (color[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                (color[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                (color[2] * 255.0).round().clamp(0.0, 255.0) as u8,
                255,
            ];
            for x in left_margin..left_margin + bar_width {
                set_pixel(x, y, rgba8);
            }
        }

        // 1px border framing the bar.
        let frame_left = left_margin.saturating_sub(1);
        let frame_right = left_margin + bar_width;
        let frame_top = bar_top.saturating_sub(1);
        let frame_bottom = bar_bottom;
        for x in frame_left..=frame_right {
            set_pixel(x, frame_top, border);
            set_pixel(x, frame_bottom, border);
        }
        for y in frame_top..=frame_bottom {
            set_pixel(frame_left, y, border);
            set_pixel(frame_right, y, border);
        }

        ScreenshotImage::new(panel_width, height, rgba)
            .expect("colorbar panel dimensions match its buffer")
    }

    /// Capture the single-surface left/right/top/bottom montage.
    pub(super) fn capture_standard_montage(&mut self) -> Result<ScreenshotImage> {
        let shots = standard_montage_shots();
        self.capture_montage_shots(&shots)
    }

    /// Capture the paired-hemisphere closed/open montage.
    pub(super) fn capture_paired_spec_montage(&mut self) -> Result<ScreenshotImage> {
        let original_layout = self.controller.display.pair_layout;
        let original_state = self.controller.display.pair_state;
        let shots = paired_spec_montage_shots();
        let result = self.capture_paired_montage_shots(&shots);

        self.apply_hemisphere_layout_state(original_layout, original_state)?;
        result
    }

    /// Capture and stitch a given list of paired-scene montage shots.
    pub(super) fn capture_paired_montage_shots(
        &mut self,
        shots: &[MontageShot],
    ) -> Result<ScreenshotImage> {
        let mut images = Vec::with_capacity(shots.len());
        let background = self.controller.display.background.rgba8();
        for shot in shots {
            if let Some(layout) = shot.layout {
                self.apply_hemisphere_layout_state(layout.layout, layout.state)?;
            }
            let mut camera = self.camera.clone();
            match shot.camera {
                MontageCamera::Preset(preset) => camera.set_preset(preset),
                MontageCamera::Direction { eye_direction, up } => {
                    camera.set_view_direction(eye_direction, up);
                }
            }
            self.fit_camera_to_current_geometry(&mut camera, shot.padding);
            let image = self.capture_surface_view(&camera)?;
            images.push(screenshot::crop_to_content(
                &image,
                background,
                MONTAGE_CONTENT_CROP_TOLERANCE,
                MONTAGE_CONTENT_CROP_PADDING,
            )?);
        }

        let montage =
            screenshot::stitch_horizontal_with_gap(&images, MONTAGE_PAIRED_GAP_PIXELS, background)?;
        screenshot::pad_image(
            &montage,
            MONTAGE_OUTER_PADDING_PIXELS,
            MONTAGE_OUTER_PADDING_PIXELS,
            background,
        )
    }

    /// Capture and stitch a given list of single-surface montage shots.
    pub(super) fn capture_montage_shots(
        &mut self,
        shots: &[MontageShot],
    ) -> Result<ScreenshotImage> {
        let mut images = Vec::with_capacity(shots.len());
        for shot in shots {
            if let Some(layout) = shot.layout {
                self.apply_hemisphere_layout_state(layout.layout, layout.state)?;
            }
            let mut camera = self.camera.clone();
            match shot.camera {
                MontageCamera::Preset(preset) => camera.set_preset(preset),
                MontageCamera::Direction { eye_direction, up } => {
                    camera.set_view_direction(eye_direction, up);
                }
            }
            self.fit_camera_to_current_geometry(&mut camera, shot.padding);
            images.push(self.capture_surface_view(&camera)?);
        }

        screenshot::stitch_horizontal(&images)
    }

    /// Frame the camera to the active surface bounds with padding.
    pub(super) fn fit_camera_to_current_geometry(&self, camera: &mut Camera, padding: f32) {
        if self.has_both_scene() && self.fit_camera_to_active_pair(camera, padding) {
            return;
        }

        let Some(geometry) = self
            .prepared_geometry_cache
            .as_ref()
            .map(|cache| cache.geometry.as_ref())
        else {
            return;
        };
        if geometry.vertices.is_empty() {
            return;
        }

        let aspect = self.scene_viewport_aspect();
        let tan_y = (camera::CAMERA_FOV_Y_RADIANS * 0.5).tan();
        let tan_x = tan_y * aspect.max(0.01);
        let (eye_direction, up) = camera.view_axes();
        let eye_direction = eye_direction.normalize();
        let up = up.normalize();
        let right = up.cross(eye_direction).normalize_or_zero();
        let mut required_distance = 0.75_f32;

        for vertex in &geometry.vertices {
            let point = Vec3::from_array(vertex.position);
            let depth = point.dot(eye_direction);
            let horizontal = point.dot(right).abs() / tan_x;
            let vertical = point.dot(up).abs() / tan_y;
            required_distance = required_distance.max(depth + horizontal.max(vertical));
        }

        camera.distance = (required_distance * padding.max(1.0)).clamp(0.75, 25.0);
    }

    /// Frame the camera to both hemispheres of the active pair.
    pub(super) fn fit_camera_to_active_pair(&self, camera: &mut Camera, padding: f32) -> bool {
        let Some(scene) = self.surface_scene.as_ref() else {
            return false;
        };
        let Some(surface) = scene.surfaces.get(scene.active_index) else {
            return false;
        };
        let matrices = pair_hemisphere_matrices(
            &surface.components,
            self.controller.display.pair_state,
            self.controller.display.pair_visibility,
        );
        if matrices.is_empty() {
            return false;
        }

        let aspect = self.scene_viewport_aspect();
        let tan_y = (camera::CAMERA_FOV_Y_RADIANS * 0.5).tan();
        let tan_x = tan_y * aspect.max(0.01);
        let (eye_direction, up) = camera.view_axes();
        let eye_direction = eye_direction.normalize();
        let up = up.normalize();
        let right = up.cross(eye_direction).normalize_or_zero();
        let mut required_distance = 0.75_f32;
        let mut any = false;

        for component in &surface.components {
            if !self
                .controller
                .display
                .pair_visibility
                .is_visible(&component.side)
            {
                continue;
            }
            let Some(mesh) = component.mesh.as_ref() else {
                continue;
            };
            let Some((_, model)) = matrices.iter().find(|(side, _)| *side == component.side) else {
                continue;
            };
            for vertex in &mesh.vertices {
                let point = model.transform_point3(Vec3::from_array(*vertex));
                let depth = point.dot(eye_direction);
                let horizontal = point.dot(right).abs() / tan_x;
                let vertical = point.dot(up).abs() / tan_y;
                required_distance = required_distance.max(depth + horizontal.max(vertical));
                any = true;
            }
        }

        if any {
            camera.distance = (required_distance * padding.max(1.0)).clamp(0.75, 25.0);
        }

        any
    }

    /// Render the surface offscreen with a given camera and read back pixels.
    pub(super) fn capture_surface_view(&mut self, camera: &Camera) -> Result<ScreenshotImage> {
        let width = self.view.config.width.max(1);
        let height = self.view.config.height.max(1);
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let screenshot_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.view.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let screenshot_view =
            screenshot_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_buffer = DepthBuffer::new(&self.device, width, height);
        let padded_bytes_per_row = screenshot::padded_bytes_per_row(width);
        let readback_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback buffer"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        self.update_render_uniforms_for_camera(camera);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("screenshot render encoder"),
            });
        self.encode_surface_render_pass(&mut encoder, &screenshot_view, &depth_buffer.view);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &screenshot_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            extent,
        );
        self.queue.submit([encoder.finish()]);

        let buffer_slice = readback_buffer.slice(..);
        let (sender, receiver) = mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("failed to wait for screenshot readback")?;
        receiver
            .recv()
            .context("screenshot readback callback did not run")?
            .context("failed to map screenshot readback buffer")?;

        let mapped = buffer_slice.get_mapped_range();
        let rgba = screenshot::texture_bytes_to_rgba(
            &mapped,
            width,
            height,
            padded_bytes_per_row,
            self.view.config.format,
        )?;
        drop(mapped);
        readback_buffer.unmap();

        ScreenshotImage::new(width, height, rgba)
    }
}
