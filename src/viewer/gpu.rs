pub(super) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

pub(super) fn choose_surface_format(
    view_caps: &wgpu::SurfaceCapabilities,
    control_caps: &wgpu::SurfaceCapabilities,
) -> wgpu::TextureFormat {
    preferred_surface_formats()
        .into_iter()
        .find(|format| view_caps.formats.contains(format) && control_caps.formats.contains(format))
        .or_else(|| {
            view_caps
                .formats
                .iter()
                .copied()
                .find(|format| control_caps.formats.contains(format))
        })
        .unwrap_or(view_caps.formats[0])
}

fn preferred_surface_formats() -> [wgpu::TextureFormat; 4] {
    [
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ]
}

pub(super) fn choose_present_mode(
    view_caps: &wgpu::SurfaceCapabilities,
    control_caps: &wgpu::SurfaceCapabilities,
) -> wgpu::PresentMode {
    [wgpu::PresentMode::Fifo]
        .into_iter()
        .find(|mode| {
            view_caps.present_modes.contains(mode) && control_caps.present_modes.contains(mode)
        })
        .or_else(|| {
            view_caps
                .present_modes
                .iter()
                .copied()
                .find(|mode| control_caps.present_modes.contains(mode))
        })
        .unwrap_or(view_caps.present_modes[0])
}

pub(super) fn choose_alpha_mode(
    view_caps: &wgpu::SurfaceCapabilities,
    control_caps: &wgpu::SurfaceCapabilities,
) -> wgpu::CompositeAlphaMode {
    view_caps
        .alpha_modes
        .iter()
        .copied()
        .find(|mode| control_caps.alpha_modes.contains(mode))
        .unwrap_or(view_caps.alpha_modes[0])
}

pub(super) struct DepthBuffer {
    _texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
}

impl DepthBuffer {
    pub(super) fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            _texture: texture,
            view,
        }
    }
}
