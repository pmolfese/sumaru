//! GPU state for `--volume` mode: a loaded [`Volume`] uploaded to a 3D texture
//! plus the orthogonal slice planes drawn through it. Each plane is a world-space
//! quad; the slice shader reconstructs voxel coordinates per fragment, so the
//! planes stay correct under the volume's voxel<->world affine and co-register
//! with surfaces in the same scene.

use super::*;
use crate::volume::Volume;

/// The three anatomical slice orientations, indexed by the world axis their
/// plane is perpendicular to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlicePlane {
    /// Perpendicular to world X (left-right).
    Sagittal,
    /// Perpendicular to world Y (anterior-posterior).
    Coronal,
    /// Perpendicular to world Z (inferior-superior).
    Axial,
}

impl SlicePlane {
    pub(super) const ALL: [SlicePlane; 3] =
        [SlicePlane::Axial, SlicePlane::Coronal, SlicePlane::Sagittal];

    /// World axis (0=X, 1=Y, 2=Z) the plane is perpendicular to.
    fn axis(self) -> usize {
        match self {
            SlicePlane::Sagittal => 0,
            SlicePlane::Coronal => 1,
            SlicePlane::Axial => 2,
        }
    }

    /// The two world axes the plane spans, `(u, v)`. `v` is the tab edge.
    fn in_plane_axes(self) -> (usize, usize) {
        match self.axis() {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            SlicePlane::Axial => "Axial",
            SlicePlane::Coronal => "Coronal",
            SlicePlane::Sagittal => "Sagittal",
        }
    }

    /// Identity color for the plane's border and grab tab.
    fn color(self) -> [f32; 3] {
        match self {
            SlicePlane::Axial => [0.92, 0.26, 0.26],   // red
            SlicePlane::Coronal => [0.30, 0.85, 0.36], // green
            SlicePlane::Sagittal => [0.36, 0.55, 1.0], // blue
        }
    }
}

/// One slice plane in the scene: its orientation and where it sits along that
/// orientation's world axis (millimeters). Multiple slices may share an
/// orientation, giving parallel cuts at different depths.
#[derive(Debug, Clone, Copy)]
struct Slice {
    orientation: SlicePlane,
    world_position: f32,
}

const SLICE_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x3];

/// Floats per vertex: world position (3) + quad uv (2) + color (3).
const SLICE_VERTEX_FLOATS: usize = 8;
/// Bytes-per-vertex stride.
const SLICE_VERTEX_STRIDE: u64 = (SLICE_VERTEX_FLOATS * std::mem::size_of::<f32>()) as u64;

pub(super) struct VolumeView {
    volume: Volume,
    /// Path the volume was loaded from, for status messages.
    path: PathBuf,
    /// World-space axis-aligned bounds of the volume box.
    world_min: Vec3,
    world_max: Vec3,
    /// Scene normalization (world -> origin-centered unit space) so the volume
    /// sits inside the camera frustum, matching the surface normalization.
    scene_model: Mat4,
    /// Display window: (low, high) intensity mapped to black..white.
    window: (f32, f32),
    /// All slice planes in the scene; several may share an orientation.
    slices: Vec<Slice>,
    /// Index into `slices` the user has selected (right-click) for left-drag
    /// scrubbing.
    selected: Option<usize>,

    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    texture_bind_group: wgpu::BindGroup,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_count: u32,
}

impl VolumeView {
    /// Upload `volume` to the GPU and build the slice-plane pipeline. Axial is
    /// enabled by default, centered in the volume.
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
        volume: Volume,
        path: PathBuf,
    ) -> Self {
        let [nx, ny, nz] = volume.dimensions;

        // 3D intensity texture (R32Float, i-fastest matches our buffer layout).
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("volume texture"),
            size: wgpu::Extent3d {
                width: nx as u32,
                height: ny as u32,
                depth_or_array_layers: nz as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &super::f32_bytes(&volume.data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((nx * 4) as u32),
                rows_per_image: Some(ny as u32),
            },
            wgpu::Extent3d {
                width: nx as u32,
                height: ny as u32,
                depth_or_array_layers: nz as u32,
            },
        );
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // R32Float is not guaranteed filterable, so sample with nearest. This
        // keeps exact voxel values (and full portability) at the cost of slightly
        // blockier slices; a filterable normalized format could replace it later.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("volume sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("volume uniform layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("volume texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });

        // 2 mat4 (32) + 2 vec4 (8) = 40 floats.
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volume uniform buffer"),
            size: (40 * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("volume uniform bind group"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("volume texture bind group"),
            layout: &texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("volume slice shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("volume_slice.wgsl"))),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("volume slice pipeline layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("volume slice pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("slice_vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: SLICE_VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &SLICE_VERTEX_ATTRIBUTES,
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("slice_fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                // Slice planes are viewed from both sides.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let (world_min, world_max) = volume_world_bounds(&volume);
        let center = (world_min + world_max) * 0.5;
        let radius = ((world_max - world_min) * 0.5).length().max(f32::EPSILON);
        let scene_model =
            Mat4::from_scale(Vec3::splat(1.0 / radius)) * Mat4::from_translation(-center);

        let mid = (world_min + world_max) * 0.5;
        // One slice of each orientation shown by default, centered in the volume.
        let slices = SlicePlane::ALL
            .iter()
            .map(|&orientation| Slice {
                orientation,
                world_position: mid[orientation.axis()],
            })
            .collect();

        let mut view = Self {
            volume,
            path,
            world_min,
            world_max,
            scene_model,
            window: (0.0, 0.0),
            slices,
            selected: None,
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            texture_bind_group,
            vertex_buffer: None,
            vertex_count: 0,
        };
        view.window = view.default_window();
        view.rebuild_geometry(device);
        view
    }

    /// Default display window from the loaded intensity range.
    fn default_window(&self) -> (f32, f32) {
        let low = self.volume.min_value;
        let high = self.volume.max_value;
        if high > low {
            (low, high)
        } else {
            (low, low + 1.0)
        }
    }

    /// How many slices of a given orientation currently exist.
    pub(super) fn orientation_count(&self, orientation: SlicePlane) -> usize {
        self.slices
            .iter()
            .filter(|slice| slice.orientation == orientation)
            .count()
    }

    /// Add a new slice of the given orientation. New slices stagger along the
    /// axis (centered, then offset) so a second parallel slice is visible rather
    /// than hidden behind the first. The new slice becomes the selection.
    pub(super) fn add_slice(&mut self, device: &wgpu::Device, orientation: SlicePlane) {
        let axis = orientation.axis();
        let lo = self.world_min[axis];
        let hi = self.world_max[axis];
        // Spread successive slices across the axis: center, then 1/4, 3/4, ...
        let n = self.orientation_count(orientation);
        let fraction = match n {
            0 => 0.5,
            1 => 0.25,
            2 => 0.75,
            _ => ((n as f32 * 0.137) % 1.0).clamp(0.05, 0.95),
        };
        let world_position = lo + (hi - lo) * fraction;
        self.slices.push(Slice {
            orientation,
            world_position,
        });
        self.selected = Some(self.slices.len() - 1);
        self.rebuild_geometry(device);
    }

    /// Remove the selected slice, if any. Returns its orientation label.
    pub(super) fn remove_selected(&mut self, device: &wgpu::Device) -> Option<&'static str> {
        let index = self.selected?;
        if index >= self.slices.len() {
            self.selected = None;
            return None;
        }
        let label = self.slices[index].orientation.label();
        self.slices.remove(index);
        self.selected = None;
        self.rebuild_geometry(device);
        Some(label)
    }

    /// Label of the selected slice's orientation, for menus/status.
    pub(super) fn selected_label(&self) -> Option<&'static str> {
        self.selected
            .and_then(|index| self.slices.get(index))
            .map(|slice| slice.orientation.label())
    }

    /// World-space ray from a camera-space (scene-normalized) ray.
    fn world_ray(&self, origin: Vec3, direction: Vec3) -> (Vec3, Vec3) {
        let inverse = self.scene_model.inverse();
        let world_origin = inverse.transform_point3(origin);
        let world_dir = inverse.transform_vector3(direction).normalize_or_zero();
        (world_origin, world_dir)
    }

    fn center(&self) -> Vec3 {
        (self.world_min + self.world_max) * 0.5
    }

    /// Test a camera-space ray against the slices and return the index of the
    /// nearest one the ray passes through (anywhere on the quad). Used for
    /// right-click selection; with overlapping parallel slices, the closer one
    /// wins.
    pub(super) fn slice_at_ray(&self, origin: Vec3, direction: Vec3) -> Option<usize> {
        let (o, d) = self.world_ray(origin, direction);
        if d.length_squared() <= f32::EPSILON {
            return None;
        }

        let mut best: Option<(f32, usize)> = None;
        for (index, slice) in self.slices.iter().enumerate() {
            let axis = slice.orientation.axis();
            // Ray vs the slice's constant-axis surface.
            let denom = d[axis];
            if denom.abs() <= f32::EPSILON {
                continue;
            }
            let t = (slice.world_position - o[axis]) / denom;
            if t <= 0.0 {
                continue;
            }
            let hit = o + d * t;
            let (u, v) = slice.orientation.in_plane_axes();
            let uu = inverse_lerp(self.world_min[u], self.world_max[u], hit[u]);
            let vv = inverse_lerp(self.world_min[v], self.world_max[v], hit[v]);
            if (0.0..=1.0).contains(&uu) && (0.0..=1.0).contains(&vv) {
                match best {
                    Some((best_t, _)) if best_t <= t => {}
                    _ => best = Some((t, index)),
                }
            }
        }
        best.map(|(_, index)| index)
    }

    pub(super) fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Set (or clear) the selected slice and rebuild geometry so the highlight
    /// updates.
    pub(super) fn set_selected(&mut self, device: &wgpu::Device, index: Option<usize>) {
        let index = index.filter(|&i| i < self.slices.len());
        if self.selected == index {
            return;
        }
        self.selected = index;
        self.rebuild_geometry(device);
    }

    /// Move the slice at `index` to follow a camera-space ray, dragging along its
    /// world axis (closest point between the ray and the axis line through the
    /// center). Returns true if the position changed enough to warrant a redraw.
    pub(super) fn drag_slice_to_ray(
        &mut self,
        device: &wgpu::Device,
        index: usize,
        origin: Vec3,
        direction: Vec3,
    ) -> bool {
        let Some(slice) = self.slices.get(index) else {
            return false;
        };
        let (o, d) = self.world_ray(origin, direction);
        if d.length_squared() <= f32::EPSILON {
            return false;
        }
        let axis = slice.orientation.axis();
        let mut axis_dir = Vec3::ZERO;
        axis_dir[axis] = 1.0;
        let center = self.center();

        // Closest point between axis line (center + s*axis_dir) and ray (o + t*d).
        let w0 = center - o;
        let b = axis_dir.dot(d);
        let dd = axis_dir.dot(w0);
        let e = d.dot(w0);
        let denom = 1.0 - b * b; // |axis_dir|=|d|=1
        if denom.abs() <= 1e-5 {
            return false;
        }
        let s = (b * e - dd) / denom;
        let new_position = (center[axis] + s).clamp(self.world_min[axis], self.world_max[axis]);

        let slice = &mut self.slices[index];
        if (slice.world_position - new_position).abs() < 1e-4 {
            return false;
        }
        slice.world_position = new_position;
        self.rebuild_geometry(device);
        true
    }

    pub(super) fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Rebuild the combined vertex buffer for all enabled planes. Vertices are
    /// stored as flat `[x, y, z]` floats to match the codebase's bytemuck-free
    /// buffer style.
    fn rebuild_geometry(&mut self, device: &wgpu::Device) {
        let mut floats: Vec<f32> = Vec::new();
        for index in 0..self.slices.len() {
            let slice = self.slices[index];
            let selected = self.selected == Some(index);
            self.push_plane_quad(
                &mut floats,
                slice.orientation,
                slice.world_position,
                selected,
            );
        }

        self.vertex_count = (floats.len() / SLICE_VERTEX_FLOATS) as u32;
        self.vertex_buffer = if floats.is_empty() {
            None
        } else {
            Some(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("volume slice vertices"),
                    contents: &super::f32_bytes(&floats),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
            )
        };
    }

    /// Append the two triangles of one plane's quad, spanning the volume's world
    /// bounds along the two axes the plane is parallel to. Each vertex carries
    /// its parametric `(u, v)` in 0..1 and the plane's identity color.
    fn push_plane_quad(
        &self,
        floats: &mut Vec<f32>,
        plane: SlicePlane,
        position: f32,
        selected: bool,
    ) {
        let (u, v) = plane.in_plane_axes();
        let axis = plane.axis();
        let (u0, u1) = (self.world_min[u], self.world_max[u]);
        let (v0, v1) = (self.world_min[v], self.world_max[v]);
        // Brighten the border/tab of the selected slice toward white.
        let color = if selected {
            let c = plane.color();
            [
                c[0] + (1.0 - c[0]) * 0.55,
                c[1] + (1.0 - c[1]) * 0.55,
                c[2] + (1.0 - c[2]) * 0.55,
            ]
        } else {
            plane.color()
        };
        let vertex = |floats: &mut Vec<f32>, su: f32, sv: f32| {
            let mut p = [0.0_f32; 3];
            p[axis] = position;
            p[u] = u0 + su * (u1 - u0);
            p[v] = v0 + sv * (v1 - v0);
            floats.extend_from_slice(&p);
            floats.extend_from_slice(&[su, sv]);
            floats.extend_from_slice(&color);
        };
        // (u, v) parametric corners; two triangles.
        for (su, sv) in [
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
        ] {
            vertex(floats, su, sv);
        }
    }

    /// Draw the enabled slice planes into the active render pass.
    pub(super) fn render(
        &self,
        queue: &wgpu::Queue,
        render_pass: &mut wgpu::RenderPass<'_>,
        view_projection: Mat4,
    ) {
        let Some(vertex_buffer) = self.vertex_buffer.as_ref() else {
            return;
        };
        if self.vertex_count == 0 {
            return;
        }

        let clip_from_world = view_projection * self.scene_model;
        let world_to_voxel =
            Mat4::from_cols_array_2d(&self.volume.space.world_to_voxel.to_matrix());
        let [nx, ny, nz] = self.volume.dimensions;
        let floats: Vec<f32> = clip_from_world
            .to_cols_array()
            .into_iter()
            .chain(world_to_voxel.to_cols_array())
            .chain([1.0 / nx as f32, 1.0 / ny as f32, 1.0 / nz as f32, 0.0])
            .chain([self.window.0, self.window.1, 0.0, 0.0])
            .collect();
        queue.write_buffer(&self.uniform_buffer, 0, &super::f32_bytes(&floats));

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_bind_group(1, &self.texture_bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.draw(0..self.vertex_count, 0..1);
    }
}

/// Parametric position of `value` within `[a, b]` (unclamped).
fn inverse_lerp(a: f32, b: f32, value: f32) -> f32 {
    if (b - a).abs() <= f32::EPSILON {
        0.0
    } else {
        (value - a) / (b - a)
    }
}

/// World-space axis-aligned bounds of a volume's voxel box.
fn volume_world_bounds(volume: &Volume) -> (Vec3, Vec3) {
    let [nx, ny, nz] = volume.dimensions;
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let extents = [
        [0.0, (nx - 1) as f32],
        [0.0, (ny - 1) as f32],
        [0.0, (nz - 1) as f32],
    ];
    for &i in &extents[0] {
        for &j in &extents[1] {
            for &k in &extents[2] {
                let world = Vec3::from_array(volume.space.voxel_to_world([i, j, k]));
                min = min.min(world);
                max = max.max(world);
            }
        }
    }
    (min, max)
}
