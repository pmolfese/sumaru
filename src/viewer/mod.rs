use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use glam::{Mat4, Quat, Vec3};
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind};
use crate::io::{read_gifti_dataset, read_niml_dataset, read_niml_roi};
use crate::overlay::{
    ColumnSelection, MaskMode, Overlay, OverlayColumns, OverlayRange, RangeSelection, Threshold,
};
use crate::roi::{Roi, RoiBrushAction, RoiDatum, RoiElementKind};
use crate::spec::{SpecFile, SpecHemisphere, SpecSurface, read_spec};
use crate::stats::AfniStatSpec;
use crate::surface::{
    AnatomicalCorrectness, NormalDirection, OverlayDataset, SurfaceDomain, SurfaceId, SurfaceMesh,
    SurfaceSide, ValueRange,
};
use camera::{Camera, CameraMode, PresetOrientation};
use gpu::{
    DEPTH_FORMAT, DepthBuffer, choose_alpha_mode, choose_present_mode, choose_surface_format,
};
use mesh::{
    OverlayAppearance, OverlayColorMap, PreparedGeometry, PreparedGeometryVertex, PreparedSurface,
    RoiAppearance, SelectionHighlight, sample_colormap,
};
use pick::pick_surface;
use screenshot::ScreenshotImage;

mod camera;
mod gpu;
mod mesh;
mod pick;
mod screenshot;

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
const VERTEX_STRIDE: wgpu::BufferAddress = 40;
const MODE_LABEL_DURATION: Duration = Duration::from_secs(2);
const STARTUP_REDRAW_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_REDRAW_RETRY_INTERVAL: Duration = Duration::from_millis(16);
const CONTROL_CONTENT_WIDTH_POINTS: f32 = 560.0;
const CONTROL_MIN_INNER_WIDTH: u32 = 620;
const CONTROL_MIN_INNER_HEIGHT: u32 = 420;
const CONTROL_INITIAL_INNER_HEIGHT: u32 = 720;
const CONTROL_MAX_INNER_WIDTH: u32 = 900;
const CONTROL_RESIZE_THRESHOLD: u32 = 12;
const INITIAL_WINDOW_RAISE_PIXELS: i32 = 100;
const OVERLAY_THRESHOLD_COLUMN_WIDTH_POINTS: f32 = 96.0;
const OVERLAY_THRESHOLD_RAIL_HEIGHT_POINTS: f32 = 315.0;
const OVERLAY_THRESHOLD_BAR_HEIGHT_POINTS: f32 = 255.0;
const OVERLAY_SELECTOR_WIDTH_POINTS: f32 = 250.0;
const DEFAULT_OVERLAY_RANGE: ValueRange = ValueRange {
    min: -1.0,
    max: 1.0,
};
const PAIR_OPEN_DEGREES_PER_PIXEL: f32 = 0.18;
const PAIR_MAX_OPEN_DEGREES: f32 = 85.0;
const PAIR_ACORN_EXTRA_GAP: f32 = 50.0;
const PAIR_MIN_CLEARANCE_FRACTION: f32 = 0.02;
const PAIR_MIN_SURFACE_CLEARANCE: f32 = 2.0;
const PAIR_MAX_DRAG_GAP_FACTOR: f32 = 1.5;
const PAIR_DRAG_PREVIEW_MIN_DELTA_PIXELS: f64 = 2.0;
const MONTAGE_DEFAULT_PADDING: f32 = 1.08;
const MONTAGE_PAIRED_CLOSED_PADDING: f32 = 1.35;
const MONTAGE_OPEN_PADDING: f32 = 1.02;
const MONTAGE_PAIRED_GAP_PIXELS: u32 = 150;
const MONTAGE_OUTER_PADDING_PIXELS: u32 = 50;
const MONTAGE_CONTENT_CROP_TOLERANCE: u8 = 2;
const MONTAGE_CONTENT_CROP_PADDING: u32 = 4;
const BLACK_BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};
const WHITE_BACKGROUND: wgpu::Color = wgpu::Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};

#[derive(Debug, Default)]
pub struct LaunchOptions {
    pub surface_path: Option<PathBuf>,
    pub spec_path: Option<PathBuf>,
    pub surface_volume_path: Option<PathBuf>,
    pub overlay_path: Option<PathBuf>,
    pub roi_path: Option<PathBuf>,
    pub overlay_subs: Option<Vec<String>>,
    pub overlay_p_value: Option<f64>,
    pub verbose: bool,
    pub preload: bool,
}

pub fn run(options: LaunchOptions) -> Result<()> {
    let event_loop = EventLoop::<ViewerEvent>::with_user_event().build()?;
    // Render on demand rather than spinning at max FPS: the loop sleeps until an
    // input event, a requested redraw, or a scheduled animation deadline.
    event_loop.set_control_flow(ControlFlow::Wait);

    let event_proxy = event_loop.create_proxy();
    let mut app = ViewerApp::new(options, event_proxy);
    event_loop.run_app(&mut app)?;

    if let Some(error) = app.setup_error {
        return Err(error);
    }

    Ok(())
}

struct ViewerApp {
    initial_surface_path: Option<PathBuf>,
    initial_spec_path: Option<PathBuf>,
    initial_surface_volume_path: Option<PathBuf>,
    initial_overlay_path: Option<PathBuf>,
    initial_roi_path: Option<PathBuf>,
    initial_overlay_subs: Option<Vec<String>>,
    initial_overlay_p_value: Option<f64>,
    verbose: bool,
    preload: bool,
    event_proxy: EventLoopProxy<ViewerEvent>,
    state: Option<ViewerState>,
    setup_error: Option<anyhow::Error>,
}

impl ViewerApp {
    fn new(options: LaunchOptions, event_proxy: EventLoopProxy<ViewerEvent>) -> Self {
        Self {
            initial_surface_path: options.surface_path,
            initial_spec_path: options.spec_path,
            initial_surface_volume_path: options.surface_volume_path,
            initial_overlay_path: options.overlay_path,
            initial_roi_path: options.roi_path,
            initial_overlay_subs: options.overlay_subs,
            initial_overlay_p_value: options.overlay_p_value,
            verbose: options.verbose,
            preload: options.preload,
            event_proxy,
            state: None,
            setup_error: None,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let view_window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(window_title(self.initial_surface_path.as_ref()))
                    .with_inner_size(PhysicalSize::new(1280, 900)),
            )?,
        );
        let control_window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("sumaru controls")
                    .with_inner_size(PhysicalSize::new(
                        CONTROL_MIN_INNER_WIDTH,
                        CONTROL_INITIAL_INNER_HEIGHT,
                    )),
            )?,
        );
        if let Ok(position) = view_window.outer_position() {
            let raised_y = position.y.saturating_sub(INITIAL_WINDOW_RAISE_PIXELS);
            view_window.set_outer_position(PhysicalPosition::new(position.x, raised_y));
            control_window.set_outer_position(PhysicalPosition::new(position.x + 1320, raised_y));
        }
        self.state = Some(pollster::block_on(ViewerState::new(
            view_window,
            control_window,
            self.initial_surface_path.take(),
            self.initial_spec_path.take(),
            self.initial_surface_volume_path.take(),
            self.initial_overlay_path.take(),
            self.initial_roi_path.take(),
            self.initial_overlay_subs.take(),
            self.initial_overlay_p_value.take(),
            self.verbose,
            self.preload,
            self.event_proxy.clone(),
        ))?);

        Ok(())
    }
}

impl ApplicationHandler<ViewerEvent> for ViewerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() || self.setup_error.is_some() {
            return;
        }

        if let Err(error) = self.initialize(event_loop) {
            self.setup_error = Some(error);
            event_loop.exit();
            return;
        }

        // Under ControlFlow::Wait we must ask for the first frame explicitly.
        if let Some(state) = self.state.as_ref() {
            state.view_window().request_redraw();
            state.control_window().request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };

        if window_id == state.view_window().id() {
            if state.view_input(&event) {
                // View-window input can change state the control panel shows
                // (picks, camera mode label, overlay toggle), so refresh both.
                state.view_window().request_redraw();
                state.control_window().request_redraw();
                return;
            }

            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Resized(size) => {
                    state.resize_view(size);
                    state.view_window().request_redraw();
                }
                WindowEvent::RedrawRequested => {
                    state.update();

                    match state.render_view() {
                        RenderStatus::Rendered => state.view_frame_rendered = true,
                        RenderStatus::Skipped => {}
                        RenderStatus::Reconfigure => {
                            state.resize_view(state.view_size);
                            state.view_window().request_redraw();
                        }
                        RenderStatus::ValidationError => eprintln!("surface validation error"),
                    }
                }
                _ => {}
            }
            return;
        }

        if window_id != state.control_window().id() {
            return;
        }

        let input = state.control_input(&event);
        if input.repaint {
            state.control_window().request_redraw();
        }
        if input.consumed {
            state.control_window().request_redraw();
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                state.resize_control(size);
                state.control_window().request_redraw();
            }
            WindowEvent::RedrawRequested => match state.render_control() {
                RenderStatus::Rendered => state.control_frame_rendered = true,
                RenderStatus::Skipped => {}
                RenderStatus::Reconfigure => {
                    state.resize_control(state.control_size);
                    state.control_window().request_redraw();
                }
                RenderStatus::ValidationError => eprintln!("control validation error"),
            },
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: ViewerEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };

        match event {
            ViewerEvent::SpecPreloadReady => {
                if state.drain_preload_results() {
                    state.control_window().request_redraw();
                    state.view_window().request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_ref() else {
            return;
        };

        let now = Instant::now();
        if state.needs_startup_redraw(now) {
            state.request_missing_startup_redraws();
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                now.checked_add(STARTUP_REDRAW_RETRY_INTERVAL)
                    .unwrap_or(now),
            ));
            return;
        }

        // The only timed redraws come from egui (panel animations and the
        // transient camera-mode label, which schedules its own repaint).
        // Everything else is driven by input-triggered request_redraw calls.
        match state.control_repaint_at {
            Some(at) if at <= now => {
                state.control_window().request_redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

struct ViewerState {
    view_window: Arc<Window>,
    control_window: Arc<Window>,
    view_surface: wgpu::Surface<'static>,
    control_surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    view_config: wgpu::SurfaceConfiguration,
    control_config: wgpu::SurfaceConfiguration,
    view_size: PhysicalSize<u32>,
    control_size: PhysicalSize<u32>,
    last_requested_control_size: Option<PhysicalSize<u32>>,
    /// When the control window should next repaint for an egui animation, or
    /// `None` if it is idle. Drives `ControlFlow::WaitUntil`.
    control_repaint_at: Option<Instant>,
    view_frame_rendered: bool,
    control_frame_rendered: bool,
    startup_redraw_until: Instant,
    render_pipeline: wgpu::RenderPipeline,
    surface_buffers: Option<SurfaceBuffers>,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    uniform_bind_group_layout: wgpu::BindGroupLayout,
    /// Active only while Control-dragging an acorn pair: per-hemisphere resident
    /// buffers drawn with per-hemisphere model matrices, so the drag updates
    /// small uniforms instead of re-transforming and re-uploading geometry.
    pair_drag_render: Option<PairDragRender>,
    depth_buffer: DepthBuffer,
    mesh: Option<SurfaceMesh>,
    prepared_geometry_cache: Option<PreparedGeometryCache>,
    surface_scene: Option<SurfaceScene>,
    scene_generation: u64,
    overlay: Option<Overlay>,
    overlay_values: Option<OverlayDataset>,
    overlay_dataset: Option<Dataset>,
    overlay_columns: OverlayColumnSelections,
    overlay_visible: bool,
    overlay_appearance: OverlayAppearance,
    overlay_symmetric_range: bool,
    surface_path: Option<PathBuf>,
    overlay_path: Option<PathBuf>,
    roi_path: Option<PathBuf>,
    overlay_display_name: Option<String>,
    roi_layer: Option<RoiLayer>,
    roi_visible: bool,
    surface_volume_path: Option<PathBuf>,
    hemisphere_layout: HemisphereLayout,
    hemisphere_open_angle_degrees: f32,
    hemisphere_separation_distance: f32,
    pair_visibility: PairVisibility,
    scene_stats: Option<SceneStats>,
    /// Cached geometry-derived stats (winding/area/counts) keyed by surface id,
    /// so recolors do not recompute the expensive `winding_report`.
    scene_geometry_stats: Option<(SurfaceId, SceneGeometryStats)>,
    surface_pick: Option<SurfacePick>,
    verbose: bool,
    preload_enabled: bool,
    preload_sender: Sender<PreloadResult>,
    preload_receiver: Receiver<PreloadResult>,
    event_proxy: EventLoopProxy<ViewerEvent>,
    camera: Camera,
    view_cursor_position: Option<(f64, f64)>,
    pair_dragging: bool,
    pair_drag_last_cursor: Option<(f64, f64)>,
    pair_drag_changed: bool,
    background: BackgroundMode,
    modifiers: ModifiersState,
    mode_label: Option<ModeLabel>,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: Renderer,
    pending_egui_textures: egui::TexturesDelta,
    allocated_egui_textures: HashSet<egui::TextureId>,
}

impl ViewerState {
    async fn new(
        view_window: Arc<Window>,
        control_window: Arc<Window>,
        initial_surface_path: Option<PathBuf>,
        initial_spec_path: Option<PathBuf>,
        initial_surface_volume_path: Option<PathBuf>,
        initial_overlay_path: Option<PathBuf>,
        initial_roi_path: Option<PathBuf>,
        initial_overlay_subs: Option<Vec<String>>,
        initial_overlay_p_value: Option<f64>,
        verbose: bool,
        preload_enabled: bool,
        event_proxy: EventLoopProxy<ViewerEvent>,
    ) -> Result<Self> {
        let view_size = view_window.inner_size();
        let control_size = control_window.inner_size();
        let instance = wgpu::Instance::default();
        let view_surface = instance.create_surface(view_window.clone())?;
        let control_surface = instance.create_surface(control_window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&view_surface),
                force_fallback_adapter: false,
            })
            .await
            .context("failed to find a compatible GPU adapter")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("sumaru device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await?;
        let view_caps = view_surface.get_capabilities(&adapter);
        let control_caps = control_surface.get_capabilities(&adapter);
        let surface_format = choose_surface_format(&view_caps, &control_caps);
        let present_mode = choose_present_mode(&view_caps, &control_caps);
        let alpha_mode = choose_alpha_mode(&view_caps, &control_caps);
        let view_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: view_size.width.max(1),
            height: view_size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        let control_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: control_size.width.max(1),
            height: control_size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        view_surface.configure(&device, &view_config);
        control_surface.configure(&device, &control_config);

        let camera = Camera::default();
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera uniform buffer"),
            contents: &camera.uniform_bytes(view_config.width as f32 / view_config.height as f32),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("camera bind group layout"),
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
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("surface shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("shader.wgsl"))),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("surface pipeline layout"),
            bind_group_layouts: &[Some(&uniform_bind_group_layout)],
            immediate_size: 0,
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("surface render pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
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
        let depth_buffer = DepthBuffer::new(&device, view_config.width, view_config.height);
        let egui_ctx = egui::Context::default();
        egui_ctx.set_visuals(egui::Visuals::dark());
        let mut egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            control_window.as_ref(),
            None,
            None,
            None,
        );
        egui_state.set_max_texture_side(device.limits().max_texture_dimension_2d as usize);
        let egui_renderer = Renderer::new(&device, surface_format, RendererOptions::default());
        let initial_surface_volume_path =
            initial_surface_volume_path.map(canonical_or_original_path);
        let (preload_sender, preload_receiver) = mpsc::channel();

        let mut state = Self {
            view_window,
            control_window,
            view_surface,
            control_surface,
            device,
            queue,
            view_config,
            control_config,
            view_size,
            control_size,
            last_requested_control_size: None,
            control_repaint_at: None,
            view_frame_rendered: false,
            control_frame_rendered: false,
            startup_redraw_until: Instant::now(),
            render_pipeline,
            surface_buffers: None,
            uniform_buffer,
            uniform_bind_group,
            uniform_bind_group_layout,
            pair_drag_render: None,
            depth_buffer,
            mesh: None,
            prepared_geometry_cache: None,
            surface_scene: None,
            scene_generation: 0,
            overlay: None,
            overlay_values: None,
            overlay_dataset: None,
            overlay_columns: OverlayColumnSelections::default(),
            overlay_visible: true,
            overlay_appearance: OverlayAppearance::from_range(DEFAULT_OVERLAY_RANGE),
            overlay_symmetric_range: true,
            surface_path: None,
            overlay_path: None,
            roi_path: None,
            overlay_display_name: None,
            roi_layer: None,
            roi_visible: true,
            surface_volume_path: initial_surface_volume_path.clone(),
            hemisphere_layout: HemisphereLayout::Closed,
            hemisphere_open_angle_degrees: 0.0,
            hemisphere_separation_distance: 0.0,
            pair_visibility: PairVisibility::both(),
            scene_stats: None,
            scene_geometry_stats: None,
            surface_pick: None,
            verbose,
            preload_enabled,
            preload_sender,
            preload_receiver,
            event_proxy,
            camera,
            view_cursor_position: None,
            pair_dragging: false,
            pair_drag_last_cursor: None,
            pair_drag_changed: false,
            background: BackgroundMode::Black,
            modifiers: ModifiersState::empty(),
            mode_label: None,
            egui_ctx,
            egui_state,
            egui_renderer,
            pending_egui_textures: egui::TexturesDelta::default(),
            allocated_egui_textures: HashSet::new(),
        };

        if let Some(path) = initial_surface_path {
            state.load_surface_path(path)?;
        } else if let Some(path) = initial_spec_path {
            state.load_spec_path(path, initial_surface_volume_path)?;
        }
        if let Some(path) = initial_overlay_path {
            state.load_overlay_path(path)?;
            state.apply_initial_overlay_options(
                initial_overlay_subs.as_deref(),
                initial_overlay_p_value,
            )?;
        }
        if let Some(path) = initial_roi_path {
            state.load_roi_path(path)?;
        }
        state.arm_startup_redraw_guard();
        state.log_status("Viewer initialized.");

        Ok(state)
    }

    fn view_window(&self) -> &Window {
        &self.view_window
    }

    fn control_window(&self) -> &Window {
        &self.control_window
    }

    fn arm_startup_redraw_guard(&mut self) {
        self.view_frame_rendered = false;
        self.control_frame_rendered = false;
        self.startup_redraw_until = Instant::now()
            .checked_add(STARTUP_REDRAW_TIMEOUT)
            .unwrap_or_else(Instant::now);
    }

    fn needs_startup_redraw(&self, now: Instant) -> bool {
        now <= self.startup_redraw_until
            && (!self.view_frame_rendered || !self.control_frame_rendered)
    }

    fn request_missing_startup_redraws(&self) {
        if !self.view_frame_rendered {
            self.view_window.request_redraw();
        }
        if !self.control_frame_rendered {
            self.control_window.request_redraw();
        }
    }

    fn resize_view(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        self.view_size = size;
        self.view_config.width = size.width;
        self.view_config.height = size.height;
        self.view_surface.configure(&self.device, &self.view_config);
        self.depth_buffer = DepthBuffer::new(&self.device, size.width, size.height);
    }

    fn resize_control(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        self.control_size = size;
        self.last_requested_control_size = None;
        self.control_config.width = size.width;
        self.control_config.height = size.height;
        self.control_surface
            .configure(&self.device, &self.control_config);
    }

    fn view_input(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                if !self.modifiers.control_key() && self.pair_dragging {
                    self.finish_pair_drag();
                }
                false
            }
            WindowEvent::CursorMoved { position, .. } => {
                let cursor = (position.x, position.y);
                self.view_cursor_position = Some(cursor);
                if self.pair_dragging {
                    self.update_pair_drag(cursor);
                    return true;
                }

                self.camera.pointer_input(event)
            }
            WindowEvent::MouseInput { state, button, .. }
                if self.pair_dragging
                    && matches!(*button, MouseButton::Left | MouseButton::Right) =>
            {
                if *state == ElementState::Released {
                    self.finish_pair_drag();
                }
                true
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if self.modifiers.control_key()
                && self.has_both_scene()
                && matches!(*button, MouseButton::Left | MouseButton::Right) =>
            {
                self.begin_pair_drag();
                true
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                self.inspect_surface_at_cursor();
                true
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyC) => {
                        let mode = self.camera.toggle_mode();
                        self.show_mode_label(mode);
                        true
                    }
                    PhysicalKey::Code(KeyCode::Space) => {
                        self.camera.reset();
                        true
                    }
                    PhysicalKey::Code(KeyCode::F5) => {
                        self.background.toggle();
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyR) if self.modifiers.shift_key() => {
                        if let Err(error) = self.save_preset_montage_screenshot() {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyR) => {
                        if let Err(error) = self.save_current_view_screenshot() {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyO) => {
                        self.toggle_overlay_visibility();
                        true
                    }
                    PhysicalKey::Code(KeyCode::BracketLeft) => {
                        if let Err(error) =
                            self.toggle_pair_hemisphere_visibility(SurfaceSide::Left)
                        {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::BracketRight) => {
                        if let Err(error) =
                            self.toggle_pair_hemisphere_visibility(SurfaceSide::Right)
                        {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::Period) => match self.cycle_scene_surface(1) {
                        Ok(changed) => changed,
                        Err(error) => {
                            self.set_error(error);
                            true
                        }
                    },
                    PhysicalKey::Code(KeyCode::Comma) => match self.cycle_scene_surface(-1) {
                        Ok(changed) => changed,
                        Err(error) => {
                            self.set_error(error);
                            true
                        }
                    },
                    PhysicalKey::Code(KeyCode::ArrowLeft) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Left);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowRight) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Right);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowUp) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Top);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowDown) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Bottom);
                        true
                    }
                    _ => false,
                }
            }
            _ => self.camera.pointer_input(event),
        }
    }

    fn control_input(&mut self, event: &WindowEvent) -> InputResponse {
        let egui_response = self.egui_state.on_window_event(&self.control_window, event);

        InputResponse {
            consumed: egui_response.consumed,
            repaint: egui_response.repaint,
        }
    }

    fn update(&mut self) {
        let aspect = self.view_config.width as f32 / self.view_config.height as f32;
        self.queue
            .write_buffer(&self.uniform_buffer, 0, &self.camera.uniform_bytes(aspect));
    }

    fn render_view(&mut self) -> RenderStatus {
        let output = match self.view_surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return RenderStatus::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                return RenderStatus::Reconfigure;
            }
            wgpu::CurrentSurfaceTexture::Validation => return RenderStatus::ValidationError,
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("surface render encoder"),
            });

        self.encode_surface_render_pass(&mut encoder, &view, &self.depth_buffer.view);

        self.queue.submit([encoder.finish()]);
        output.present();

        RenderStatus::Rendered
    }

    fn encode_surface_render_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
    ) {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("surface render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(self.background.color()),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });

        if let Some(drag) = &self.pair_drag_render {
            // Drag mode: one draw per hemisphere, each with its own model matrix
            // bind group, over geometry uploaded once at drag start.
            render_pass.set_pipeline(&self.render_pipeline);
            for hemisphere in &drag.hemispheres {
                if !self.pair_visibility.is_visible(&hemisphere.side) {
                    continue;
                }
                render_pass.set_bind_group(0, &hemisphere.bind_group, &[]);
                render_pass.set_vertex_buffer(0, hemisphere.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(hemisphere.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..hemisphere.index_count, 0, 0..1);
            }
        } else if let Some(buffers) = &self.surface_buffers {
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_vertex_buffer(0, buffers.vertex_buffer.slice(..));
            render_pass.set_index_buffer(buffers.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..buffers.index_count, 0, 0..1);
        }
    }

    fn save_current_view_screenshot(&mut self) -> Result<()> {
        if self.surface_buffers.is_none() {
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

        Ok(())
    }

    fn save_preset_montage_screenshot(&mut self) -> Result<()> {
        if self.surface_buffers.is_none() {
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

        Ok(())
    }

    fn capture_standard_montage(&mut self) -> Result<ScreenshotImage> {
        let shots = standard_montage_shots();
        self.capture_montage_shots(&shots)
    }

    fn capture_paired_spec_montage(&mut self) -> Result<ScreenshotImage> {
        let original_geometry_cache = self.prepared_geometry_cache.clone();
        let shots = paired_spec_montage_shots();
        let result = self.capture_paired_montage_shots(&shots);

        self.prepared_geometry_cache = original_geometry_cache;
        self.upload_surface_buffers();
        result
    }

    fn capture_paired_montage_shots(&mut self, shots: &[MontageShot]) -> Result<ScreenshotImage> {
        let mut images = Vec::with_capacity(shots.len());
        let background = self.background.rgba8();
        for shot in shots {
            if let Some(layout) = shot.layout {
                self.prepare_paired_montage_render_geometry(layout.state)?;
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

    fn capture_montage_shots(&mut self, shots: &[MontageShot]) -> Result<ScreenshotImage> {
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

    fn prepare_paired_montage_render_geometry(
        &mut self,
        layout: HemisphereLayoutState,
    ) -> Result<()> {
        let visibility = self.pair_visibility;
        let indices = self
            .prepared_geometry_cache
            .as_ref()
            .filter(|cache| cache.pair_visibility == visibility)
            .map(|cache| cache.geometry.indices.clone());
        let geometry = {
            let scene = self
                .surface_scene
                .as_mut()
                .context("no SUMA spec scene is loaded")?;
            let surface = scene
                .surfaces
                .get_mut(scene.active_index)
                .context("active surface index is outside loaded scene")?;
            surface.preview_geometry(layout, visibility, indices)?
        };
        if let Some(mesh) = self.mesh.as_ref() {
            self.prepared_geometry_cache = Some(PreparedGeometryCache {
                surface_id: mesh.metadata.id.clone(),
                vertex_count: mesh.vertices.len(),
                face_count: mesh.triangles.len(),
                pair_visibility: visibility,
                geometry: Arc::new(geometry),
            });
            self.upload_surface_buffers();
        }

        Ok(())
    }

    fn fit_camera_to_current_geometry(&self, camera: &mut Camera, padding: f32) {
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

        let aspect = self.view_config.width.max(1) as f32 / self.view_config.height.max(1) as f32;
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

    fn capture_surface_view(&mut self, camera: &Camera) -> Result<ScreenshotImage> {
        let width = self.view_config.width.max(1);
        let height = self.view_config.height.max(1);
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
            format: self.view_config.format,
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

        let aspect = width as f32 / height as f32;
        self.queue
            .write_buffer(&self.uniform_buffer, 0, &camera.uniform_bytes(aspect));
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
            self.view_config.format,
        )?;
        drop(mapped);
        readback_buffer.unmap();

        ScreenshotImage::new(width, height, rgba)
    }

    fn render_control(&mut self) -> RenderStatus {
        egui_winit::update_viewport_info(
            self.egui_state
                .egui_input_mut()
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default(),
            &self.egui_ctx,
            &self.control_window,
            false,
        );
        let raw_input = self.egui_state.take_egui_input(&self.control_window);
        let egui_ctx = self.egui_ctx.clone();
        let mut ui_actions = Vec::new();
        let mut desired_control_size_points = egui::Vec2::ZERO;
        #[allow(deprecated)]
        let full_output = egui_ctx.run(raw_input, |ctx| {
            let output = self.draw_ui(ctx);
            ui_actions = output.actions;
            desired_control_size_points = output.desired_control_size_points;
        });
        // Schedule the next control-window repaint from egui's requested delay:
        // ZERO means "again next frame" (an active animation), MAX means idle.
        let repaint_delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.repaint_delay)
            .unwrap_or(Duration::MAX);
        self.control_repaint_at = if repaint_delay == Duration::ZERO {
            Some(Instant::now())
        } else if repaint_delay == Duration::MAX {
            None
        } else {
            Instant::now().checked_add(repaint_delay)
        };
        // A panel action (load, toggle, camera/background change) alters the
        // 3D scene, so the view window needs to repaint too.
        let actions_present = !ui_actions.is_empty();
        self.egui_state
            .handle_platform_output(&self.control_window, full_output.platform_output);
        if repaint_delay != Duration::ZERO {
            self.fit_control_window(desired_control_size_points, full_output.pixels_per_point);
        }
        self.apply_ui_actions(ui_actions);
        if actions_present {
            self.view_window.request_redraw();
            self.control_window.request_redraw();
        }
        let paint_jobs = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.control_config.width, self.control_config.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        self.pending_egui_textures
            .append(full_output.textures_delta);

        let output = match self.control_surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return RenderStatus::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                return RenderStatus::Reconfigure;
            }
            wgpu::CurrentSurfaceTexture::Validation => return RenderStatus::ValidationError,
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("control render encoder"),
            });

        let mut retained_textures = egui::TexturesDelta::default();
        let mut needs_texture_repaint = false;
        for (id, image_delta) in &self.pending_egui_textures.set {
            if image_delta.pos.is_some() && !self.allocated_egui_textures.contains(id) {
                retained_textures.set.push((*id, image_delta.clone()));
                needs_texture_repaint = true;
                continue;
            }

            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
            self.allocated_egui_textures.insert(*id);
        }
        let mut command_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let egui_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.06,
                            g: 0.07,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            self.egui_renderer.render(
                &mut egui_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }

        for id in &self.pending_egui_textures.free {
            if self.allocated_egui_textures.remove(id) {
                self.egui_renderer.free_texture(id);
            }
        }
        self.pending_egui_textures = retained_textures;
        if needs_texture_repaint {
            // Deferred texture upload: repaint next frame to finish it. Under
            // ControlFlow::Wait this scheduled wake is what actually drives it.
            self.control_repaint_at = Some(Instant::now());
        }

        command_buffers.push(encoder.finish());
        self.queue.submit(command_buffers);
        output.present();

        RenderStatus::Rendered
    }

    fn draw_ui(&mut self, ctx: &egui::Context) -> ControlUiOutput {
        let mut actions = Vec::new();
        let panel_height = (self.control_size.height as f32 - 24.0).max(240.0);
        let mut desired_control_size_points = egui::vec2(
            CONTROL_CONTENT_WIDTH_POINTS + 24.0,
            CONTROL_MIN_INNER_HEIGHT as f32,
        );

        #[allow(deprecated)]
        egui::CentralPanel::default().show(ctx, |ui| {
            let scroll_output = egui::ScrollArea::vertical()
                .max_height(panel_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_min_width(CONTROL_CONTENT_WIDTH_POINTS);
                    self.draw_surface_dataset_section(ui, &mut actions);
                    self.draw_overlay_workbench(ui, &mut actions);
                    self.draw_view_section(ui, &mut actions);
                    self.draw_scene_section(ui);
                    self.draw_pick_section(ui);
                });
            desired_control_size_points = egui::vec2(
                scroll_output
                    .content_size
                    .x
                    .max(CONTROL_CONTENT_WIDTH_POINTS)
                    + 32.0,
                scroll_output.content_size.y + 32.0,
            );
        });

        if let Some((text, remaining)) = self.active_mode_label() {
            // Ensure the label is cleared on time even with no further input.
            ctx.request_repaint_after(remaining);
            egui::Area::new(egui::Id::new("camera_mode_label"))
                .anchor(egui::Align2::CENTER_TOP, [0.0, 18.0])
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(180))
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(text)
                                    .size(18.0)
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            );
                        });
                });
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        ControlUiOutput {
            actions,
            desired_control_size_points,
        }
    }

    fn draw_surface_dataset_section(&mut self, ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
        controller_section(ui, "SURFACE / DATASET", true, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Open:");
                if ui
                    .button("Surf")
                    .on_hover_text("Open GIFTI surface")
                    .clicked()
                {
                    actions.push(UiAction::PickSurface);
                }
                if ui
                    .add_enabled(self.mesh.is_some(), egui::Button::new("Olay"))
                    .on_hover_text("Open overlay dataset")
                    .clicked()
                {
                    actions.push(UiAction::PickOverlay);
                }
                if ui
                    .add_enabled(self.mesh.is_some(), egui::Button::new("ROI"))
                    .on_hover_text("Open SUMA ROI")
                    .clicked()
                {
                    actions.push(UiAction::PickRoi);
                }
                if ui.button("Spec").on_hover_text("Open SUMA spec").clicked() {
                    actions.push(UiAction::PickSpec);
                }
                if ui
                    .button("SV")
                    .on_hover_text("Open surface volume")
                    .clicked()
                {
                    actions.push(UiAction::PickSurfaceVolume);
                }
            });

            ui.add_space(8.0);
            if let Some(scene) = self.surface_scene.as_ref() {
                egui::Grid::new("spec_scene_grid")
                    .num_columns(2)
                    .spacing([8.0, 5.0])
                    .show(ui, |ui| {
                        stat_row(ui, "Spec", file_display(Some(&scene.spec_path)));
                        stat_row(
                            ui,
                            "SurfVol",
                            file_display(scene.surface_volume_path.as_ref()),
                        );
                        let active = scene.active_index + 1;
                        let total = scene.surfaces.len();
                        let surface = &scene.surfaces[scene.active_index];
                        let mut selected_index = scene.active_index;
                        let selected_text =
                            scene_surface_display_label(scene.active_index, total, surface);
                        ui.label("Active");
                        let mut changed = false;
                        egui::ComboBox::from_id_salt("spec_active_surface")
                            .selected_text(selected_text)
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                for (index, surface) in scene.surfaces.iter().enumerate() {
                                    changed |= ui
                                        .selectable_value(
                                            &mut selected_index,
                                            index,
                                            scene_surface_display_label(index, total, surface),
                                        )
                                        .changed();
                                }
                            });
                        ui.end_row();
                        if changed && selected_index + 1 != active {
                            actions.push(UiAction::SelectSceneSurface(selected_index));
                        }
                        stat_row(ui, "Overlay", self.overlay_display_text());
                        stat_row(ui, "ROI", self.roi_display_text());
                        if scene.skipped_surfaces > 0 {
                            stat_row(ui, "Skipped files", scene.skipped_surfaces.to_string());
                        }
                        if scene.skipped_states > 0 {
                            stat_row(ui, "Skipped states", scene.skipped_states.to_string());
                        }
                    });
            } else {
                egui::Grid::new("surface_file_grid")
                    .num_columns(2)
                    .spacing([8.0, 5.0])
                    .show(ui, |ui| {
                        stat_row(ui, "Surface", file_display(self.surface_path.as_ref()));
                        stat_row(ui, "Overlay", self.overlay_display_text());
                        stat_row(ui, "ROI", self.roi_display_text());
                    });
            }
        });
    }

    fn draw_overlay_workbench(&mut self, ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
        let overlay_loaded = self.overlay.is_some();
        let column_options = self
            .overlay_dataset
            .as_ref()
            .map(overlay_column_options)
            .unwrap_or_default();
        let mut columns_changed = false;
        let mut changed = false;

        controller_section(ui, "OVERLAY WORKBENCH", true, |ui| {
            if !overlay_loaded {
                ui.label(egui::RichText::new("No overlay loaded").color(muted_color()));
                return;
            }

            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(
                        OVERLAY_THRESHOLD_COLUMN_WIDTH_POINTS,
                        OVERLAY_THRESHOLD_RAIL_HEIGHT_POINTS,
                    ),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.label("Thresh");
                        let threshold_range = self.selected_threshold_range();
                        changed |= vertical_threshold_bar(
                            ui,
                            &mut self.overlay_appearance,
                            threshold_range,
                        );
                        ui.monospace(threshold_value_display(
                            self.overlay_appearance.threshold.value,
                        ));
                        ui.label(
                            egui::RichText::new(threshold_p_value_display(
                                self.selected_threshold_p_value(),
                            ))
                            .color(muted_color()),
                        );
                    },
                );

                ui.add_space(12.0);
                ui.vertical(|ui| {
                    egui::Grid::new("overlay_mapping_grid")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .show(ui, |ui| {
                            if column_options.is_empty() {
                                stat_row(ui, "I", "scalar column 0");
                                stat_row(ui, "T", "scalar column 0");
                                stat_row(ui, "B", "none");
                            } else {
                                columns_changed |= draw_intensity_column_selector(
                                    ui,
                                    &column_options,
                                    &mut self.overlay_columns.intensity,
                                );
                                columns_changed |= draw_threshold_column_selector(
                                    ui,
                                    &column_options,
                                    &mut self.overlay_columns.threshold,
                                    self.overlay_appearance.threshold.value,
                                );
                                columns_changed |= draw_optional_column_selector(
                                    ui,
                                    "B",
                                    "brightness_column",
                                    &column_options,
                                    &mut self.overlay_columns.brightness,
                                );
                            }
                        });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label("Map");
                        egui::ComboBox::from_id_salt("overlay_colormap")
                            .selected_text(self.overlay_appearance.colormap.label())
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                for colormap in OverlayColorMap::ALL {
                                    changed |= ui
                                        .selectable_value(
                                            &mut self.overlay_appearance.colormap,
                                            colormap,
                                            colormap.label(),
                                        )
                                        .changed();
                                }
                            });
                    });
                    ui.add_space(8.0);
                    changed |= self.draw_overlay_range_controls(ui);
                    ui.add_space(6.0);
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut self.overlay_appearance.dim, 0.0..=1.5)
                                .text("Dim"),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut self.overlay_appearance.opacity, 0.0..=1.0)
                                .text("Opacity"),
                        )
                        .changed();

                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        changed |= ui
                            .checkbox(&mut self.overlay_appearance.threshold.absolute, "Abs")
                            .changed();
                    });
                    if let Some(stat) = self.selected_threshold_stat_label() {
                        ui.label(egui::RichText::new(format!("Stat: {stat}")).color(muted_color()));
                    }
                });
            });
        });

        if columns_changed {
            actions.push(UiAction::RefreshOverlayColumns);
        }
        if changed {
            self.sanitize_overlay_appearance();
            actions.push(UiAction::RefreshOverlayAppearance);
        }
    }

    fn draw_overlay_range_controls(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            changed |= ui
                .checkbox(&mut self.overlay_symmetric_range, "Symmetric")
                .changed();

            if self.overlay_symmetric_range {
                let mut extent = self
                    .overlay_appearance
                    .range
                    .min
                    .abs()
                    .max(self.overlay_appearance.range.max.abs())
                    .max(0.0001);
                let speed = (extent / 100.0).max(0.001);
                if ui
                    .add(
                        egui::DragValue::new(&mut extent)
                            .speed(speed)
                            .prefix("+/- "),
                    )
                    .changed()
                {
                    let extent = extent.abs().max(0.0001);
                    self.overlay_appearance.range = ValueRange {
                        min: -extent,
                        max: extent,
                    };
                    changed = true;
                }
            } else {
                let speed = range_drag_speed(self.overlay_appearance.range);
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.overlay_appearance.range.min)
                            .speed(speed)
                            .prefix("min "),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.overlay_appearance.range.max)
                            .speed(speed)
                            .prefix("max "),
                    )
                    .changed();
            }
        });

        changed
    }

    fn selected_threshold_stat_label(&self) -> Option<String> {
        let dataset = self.overlay_dataset.as_ref()?;
        let index = self.overlay_columns.threshold?;
        dataset.columns.get(index)?.stat.clone()
    }

    fn selected_threshold_stat_spec(&self) -> Option<AfniStatSpec> {
        self.selected_threshold_stat_label()
            .as_deref()
            .and_then(AfniStatSpec::parse)
    }

    fn selected_threshold_range(&self) -> ValueRange {
        self.overlay_dataset
            .as_ref()
            .and_then(|dataset| {
                self.overlay_columns
                    .threshold
                    .and_then(|index| dataset.columns.get(index))
                    .and_then(|column| column.range)
            })
            .map(|range| ValueRange {
                min: range.min as f32,
                max: range.max as f32,
            })
            .or_else(|| self.overlay_values.as_ref().map(|overlay| overlay.range))
            .unwrap_or(DEFAULT_OVERLAY_RANGE)
    }

    fn selected_threshold_p_value(&self) -> Option<f64> {
        self.selected_threshold_stat_spec()
            .and_then(|stat| stat.two_sided_p_value(self.overlay_appearance.threshold.value as f64))
    }

    fn draw_view_section(&mut self, ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
        controller_section(ui, "VIEW", false, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Mode");
                ui.monospace(self.camera.mode().label());
                if ui.button("Cycle").clicked() {
                    actions.push(UiAction::ToggleCameraMode);
                }
                if ui.button("Reset").clicked() {
                    actions.push(UiAction::ResetCamera);
                }
                if ui.button(self.background.next_label()).clicked() {
                    actions.push(UiAction::ToggleBackground);
                }
                if ui
                    .button("Save")
                    .on_hover_text("Save current view")
                    .clicked()
                {
                    actions.push(UiAction::SaveScreenshot);
                }
                if ui
                    .button("Montage")
                    .on_hover_text(if self.has_both_scene() {
                        "Save closed top/bottom plus open paired-hemisphere views"
                    } else {
                        "Save left/right/top/bottom montage"
                    })
                    .clicked()
                {
                    actions.push(UiAction::SaveMontage);
                }
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Left").clicked() {
                    actions.push(UiAction::Preset(PresetOrientation::Left));
                }
                if ui.button("Right").clicked() {
                    actions.push(UiAction::Preset(PresetOrientation::Right));
                }
                if ui.button("Top").clicked() {
                    actions.push(UiAction::Preset(PresetOrientation::Top));
                }
                if ui.button("Bottom").clicked() {
                    actions.push(UiAction::Preset(PresetOrientation::Bottom));
                }
                let can_layout_hemispheres = self.has_both_scene();
                if ui
                    .add_enabled(
                        can_layout_hemispheres,
                        egui::Button::new("Close")
                            .selected(self.hemisphere_layout == HemisphereLayout::Closed),
                    )
                    .on_hover_text("Reset paired hemispheres to their closed alignment")
                    .clicked()
                {
                    actions.push(UiAction::HemisphereLayout(HemisphereLayout::Closed));
                }
                if ui
                    .add_enabled(
                        can_layout_hemispheres,
                        egui::Button::new("Open")
                            .selected(self.hemisphere_layout == HemisphereLayout::Open),
                    )
                    .on_hover_text("Open paired hemispheres into the acorn view")
                    .clicked()
                {
                    actions.push(UiAction::HemisphereLayout(HemisphereLayout::Open));
                }
            });
        });
    }

    fn draw_scene_section(&self, ui: &mut egui::Ui) {
        controller_section(ui, "SCENE", false, |ui| {
            if let Some(stats) = self.scene_stats.as_ref() {
                egui::Grid::new("scene_stats_grid")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .show(ui, |ui| {
                        stat_row(ui, "Nodes", stats.geometry.node_count.to_string());
                        stat_row(ui, "Triangles", stats.geometry.face_count.to_string());
                        stat_row(ui, "Area", format!("{:.4}", stats.geometry.total_area));
                        stat_row(
                            ui,
                            "Normals",
                            normal_direction_label(stats.geometry.normal_direction),
                        );
                        if stats.geometry.boundary_edges > 0 {
                            stat_row(
                                ui,
                                "Boundary edges",
                                stats.geometry.boundary_edges.to_string(),
                            );
                        }
                        if stats.geometry.non_manifold_edges > 0 {
                            stat_row(
                                ui,
                                "Non-manifold",
                                stats.geometry.non_manifold_edges.to_string(),
                            );
                        }
                        if let Some(range) = stats.overlay_range {
                            stat_row(
                                ui,
                                "Overlay range",
                                format!("{:.4} to {:.4}", range.min, range.max),
                            );
                        }
                    });
            } else {
                ui.label(egui::RichText::new("No surface loaded").color(muted_color()));
            }
        });
    }

    fn draw_pick_section(&self, ui: &mut egui::Ui) {
        controller_section(ui, "PICK", false, |ui| {
            egui::Grid::new("pick_grid")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    stat_row(ui, "Surface file", self.pick_surface_display_text());
                    stat_row(ui, "Overlay file", self.pick_overlay_display_text());
                    if let Some(pick) = self.surface_pick {
                        stat_row(ui, "Node", pick.node_index.to_string());
                        stat_row(ui, "Triangle", pick.face_index.to_string());
                        stat_row(ui, "Surf x,y,z", coordinate_label(pick.surface_position));
                        stat_row(ui, "Overlay Value", picked_overlay_value_label(pick));
                        stat_row(ui, "ROI", self.pick_roi_display_text(pick));
                    }
                });
            if self.surface_pick.is_none() {
                ui.label(egui::RichText::new("No pick").color(muted_color()));
            }
        });
    }

    fn sanitize_overlay_appearance(&mut self) {
        let range = &mut self.overlay_appearance.range;
        if !range.min.is_finite() || !range.max.is_finite() {
            *range = DEFAULT_OVERLAY_RANGE;
        }

        if self.overlay_symmetric_range {
            let extent = range.min.abs().max(range.max.abs()).max(0.0001);
            *range = ValueRange {
                min: -extent,
                max: extent,
            };
        } else if range.max < range.min {
            std::mem::swap(&mut range.min, &mut range.max);
        }

        if (range.max - range.min).abs() <= f32::EPSILON {
            range.max = range.min + 1.0;
        }

        self.overlay_appearance.dim = self.overlay_appearance.dim.clamp(0.0, 1.5);
        self.overlay_appearance.opacity = self.overlay_appearance.opacity.clamp(0.0, 1.0);

        let (threshold_min, threshold_max) = threshold_bounds(
            self.selected_threshold_range(),
            self.overlay_appearance.threshold.absolute,
        );
        self.overlay_appearance.threshold.value = self
            .overlay_appearance
            .threshold
            .value
            .clamp(threshold_min, threshold_max);
    }

    fn fit_control_window(
        &mut self,
        desired_control_size_points: egui::Vec2,
        pixels_per_point: f32,
    ) {
        if desired_control_size_points.x <= 0.0 || desired_control_size_points.y <= 0.0 {
            return;
        }

        let monitor_size = self
            .control_window
            .current_monitor()
            .map(|monitor| monitor.size());
        let max_width = monitor_size
            .map(|size| ((size.width as f32 * 0.55) as u32).min(CONTROL_MAX_INNER_WIDTH))
            .unwrap_or(CONTROL_MAX_INNER_WIDTH)
            .max(CONTROL_MIN_INNER_WIDTH);
        let max_height = monitor_size
            .map(|size| (size.height as f32 * 0.85) as u32)
            .unwrap_or(960)
            .max(CONTROL_MIN_INNER_HEIGHT);

        let desired_size = PhysicalSize::new(
            ((desired_control_size_points.x * pixels_per_point).ceil() as u32)
                .clamp(CONTROL_MIN_INNER_WIDTH, max_width),
            ((desired_control_size_points.y * pixels_per_point).ceil() as u32)
                .clamp(CONTROL_MIN_INNER_HEIGHT, max_height),
        );

        if size_is_close(self.control_size, desired_size) {
            return;
        }
        if self.last_requested_control_size == Some(desired_size) {
            return;
        }

        self.last_requested_control_size = Some(desired_size);
        if let Some(actual_size) = self.control_window.request_inner_size(desired_size) {
            self.resize_control(actual_size);
        }
        self.control_window.request_redraw();
    }

    fn apply_ui_actions(&mut self, actions: Vec<UiAction>) {
        for action in actions {
            match action {
                UiAction::PickSurface => {
                    if let Some(path) = pick_surface_file(self.surface_path.as_ref()) {
                        if let Err(error) = self.load_surface_path(path) {
                            self.set_error(error);
                        }
                    }
                }
                UiAction::PickOverlay => {
                    if let Some(path) =
                        pick_overlay_file(self.overlay_path.as_ref().or(self.surface_path.as_ref()))
                    {
                        if let Err(error) = self.load_overlay_path(path) {
                            self.set_error(error);
                        }
                    }
                }
                UiAction::PickRoi => {
                    if let Some(path) =
                        pick_roi_file(self.roi_path.as_ref().or(self.surface_path.as_ref()))
                    {
                        if let Err(error) = self.load_roi_path(path) {
                            self.set_error(error);
                        }
                    }
                }
                UiAction::PickSpec => {
                    let current_path = self
                        .surface_scene
                        .as_ref()
                        .map(|scene| &scene.spec_path)
                        .or(self.surface_path.as_ref());
                    if let Some(path) = pick_spec_file(current_path) {
                        if let Err(error) = self.load_spec_path(path, None) {
                            self.set_error(error);
                        }
                    }
                }
                UiAction::PickSurfaceVolume => {
                    let current_path = self
                        .surface_volume_path
                        .as_ref()
                        .or_else(|| {
                            self.surface_scene
                                .as_ref()
                                .and_then(|scene| scene.surface_volume_path.as_ref())
                        })
                        .or(self.surface_path.as_ref());
                    if let Some(path) = pick_surface_volume_file(current_path) {
                        if let Err(error) = self.set_surface_volume_path(path) {
                            self.set_error(error);
                        }
                    }
                }
                UiAction::RefreshOverlayColumns => {
                    if let Err(error) = self.refresh_overlay_columns() {
                        self.set_error(error);
                    }
                }
                UiAction::RefreshOverlayAppearance => {
                    if let Err(error) = self.refresh_overlay_appearance() {
                        self.set_error(error);
                    }
                }
                UiAction::ResetCamera => self.camera.reset(),
                UiAction::ToggleCameraMode => {
                    let mode = self.camera.toggle_mode();
                    self.show_mode_label(mode);
                }
                UiAction::ToggleBackground => self.background.toggle(),
                UiAction::Preset(preset) => self.camera.set_preset(preset),
                UiAction::HemisphereLayout(layout) => {
                    if let Err(error) = self.set_hemisphere_layout(layout) {
                        self.set_error(error);
                    }
                }
                UiAction::SelectSceneSurface(index) => {
                    if let Err(error) = self.activate_scene_surface(index) {
                        self.set_error(error);
                    }
                }
                UiAction::SaveScreenshot => {
                    if let Err(error) = self.save_current_view_screenshot() {
                        self.set_error(error);
                    }
                }
                UiAction::SaveMontage => {
                    if let Err(error) = self.save_preset_montage_screenshot() {
                        self.set_error(error);
                    }
                }
            }
        }
    }

    fn load_surface_path(&mut self, path: PathBuf) -> Result<()> {
        let mut mesh = SurfaceMesh::from_gifti_path(&path)
            .with_context(|| format!("failed to load surface {}", path.display()))?;
        apply_surface_volume_parent(&mut mesh, self.surface_volume_path.as_ref());
        let node_count = mesh.vertices.len();
        let face_count = mesh.triangles.len();

        self.set_active_mesh(mesh, None);
        self.scene_generation = self.scene_generation.wrapping_add(1);
        self.surface_scene = None;
        self.surface_path = Some(path.clone());
        self.overlay = None;
        self.overlay_values = None;
        self.overlay_dataset = None;
        self.overlay_columns = OverlayColumnSelections::default();
        self.overlay_visible = true;
        self.overlay_appearance = OverlayAppearance::from_range(DEFAULT_OVERLAY_RANGE);
        self.overlay_symmetric_range = true;
        self.overlay_path = None;
        self.overlay_display_name = None;
        self.roi_path = None;
        self.roi_layer = None;
        self.roi_visible = true;
        self.surface_pick = None;
        self.pair_visibility = PairVisibility::both();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.camera.reset();
        self.view_window
            .set_title(&window_title(self.surface_path.as_ref()));
        self.log_status(format!(
            "Loaded surface with {node_count} nodes and {face_count} triangles."
        ));

        Ok(())
    }

    fn load_spec_path(
        &mut self,
        spec_path: PathBuf,
        surface_volume_path: Option<PathBuf>,
    ) -> Result<()> {
        let spec = read_spec(&spec_path)
            .with_context(|| format!("failed to read SUMA spec {}", spec_path.display()))?;
        let surface_volume_path = surface_volume_path
            .or_else(|| self.surface_volume_path.clone())
            .map(canonical_or_original_path);
        let surface_volume_path =
            surface_volume_path.context("loading a SUMA spec requires -sv/--sv")?;
        let mut components = Vec::new();
        let mut skipped_surfaces = 0;

        for surface in &spec.surfaces {
            if !surface.path.exists() {
                skipped_surfaces += 1;
                self.log_status(format!(
                    "Skipping missing spec surface {}.",
                    surface.path.display()
                ));
                continue;
            }

            components.push(SceneSurfaceComponent {
                name: surface.name.clone(),
                state: surface.state.clone(),
                path: surface.path.clone(),
                side: surface.side.clone(),
                spec_surface: surface.clone(),
                mesh: None,
                normal_cache: None,
            });
        }

        let (surfaces, skipped_states, messages) =
            scene_surfaces_from_components(&spec, components);
        for message in messages {
            self.log_status(message);
        }

        ensure!(
            !surfaces.is_empty(),
            "SUMA spec {} did not contain any loadable GIFTI surfaces",
            spec.path.display()
        );

        let loaded_count = surfaces.len();
        self.scene_generation = self.scene_generation.wrapping_add(1);
        let generation = self.scene_generation;
        let loaded_label = if spec.hemisphere == SpecHemisphere::Both {
            "paired states"
        } else {
            "surfaces"
        };
        self.surface_volume_path = Some(surface_volume_path.clone());
        self.surface_scene = Some(SurfaceScene {
            spec: spec.clone(),
            spec_path: spec.path.clone(),
            surface_volume_path: Some(surface_volume_path.clone()),
            hemisphere: spec.hemisphere,
            surfaces,
            active_index: 0,
            skipped_surfaces,
            skipped_states,
        });
        self.overlay = None;
        self.overlay_values = None;
        self.overlay_dataset = None;
        self.overlay_columns = OverlayColumnSelections::default();
        self.overlay_visible = true;
        self.overlay_appearance = OverlayAppearance::from_range(DEFAULT_OVERLAY_RANGE);
        self.overlay_symmetric_range = true;
        self.overlay_path = None;
        self.overlay_display_name = None;
        self.roi_path = None;
        self.roi_layer = None;
        self.roi_visible = true;
        self.surface_pick = None;
        self.pair_visibility = PairVisibility::both();
        self.ensure_scene_surface_loaded(0)?;
        self.activate_scene_surface(0)?;
        self.start_scene_preload(generation);
        self.camera.reset();
        self.log_status(format!(
            "Loaded {loaded_count} {loaded_label} from spec {} (skipped {skipped_surfaces} files, {skipped_states} states).",
            spec.path.display()
        ));

        Ok(())
    }

    fn set_surface_volume_path(&mut self, path: PathBuf) -> Result<()> {
        let path = canonical_or_original_path(path);
        self.surface_volume_path = Some(path.clone());

        if let Some(scene) = self.surface_scene.as_mut() {
            scene.surface_volume_path = Some(path.clone());
            for surface in &mut scene.surfaces {
                surface.display_cache = None;
                for component in &mut surface.components {
                    if let Some(mesh) = component.mesh.as_mut() {
                        apply_surface_volume_parent(mesh, Some(&path));
                    }
                }
            }
        }

        if let Some(mesh) = self.mesh.as_mut() {
            apply_surface_volume_parent(mesh, Some(&path));
        }

        self.log_status(format!("Surface volume set to {}.", path.display()));

        Ok(())
    }

    fn ensure_scene_surface_loaded(&mut self, index: usize) -> Result<()> {
        let (spec, surface_volume_path, tasks) = {
            let scene = self
                .surface_scene
                .as_ref()
                .context("no SUMA spec scene is loaded")?;
            ensure!(
                index < scene.surfaces.len(),
                "surface index {index} is outside loaded scene"
            );
            let tasks = scene.surfaces[index]
                .components
                .iter()
                .enumerate()
                .filter(|(_, component)| component.mesh.is_none())
                .map(|(component_index, component)| {
                    (component_index, component.spec_surface.clone())
                })
                .collect::<Vec<_>>();

            (scene.spec.clone(), scene.surface_volume_path.clone(), tasks)
        };

        for (component_index, surface) in tasks {
            let mesh = load_spec_component_mesh(&spec, &surface, surface_volume_path.as_ref())?;
            if let Some(scene) = self.surface_scene.as_mut()
                && let Some(component) = scene
                    .surfaces
                    .get_mut(index)
                    .and_then(|surface| surface.components.get_mut(component_index))
                && component.mesh.is_none()
            {
                component.mesh = Some(mesh);
            }
        }

        Ok(())
    }

    fn start_scene_preload(&self, generation: u64) {
        if !self.preload_enabled {
            self.log_status("Spec preloading disabled.");
            return;
        }

        let Some(scene) = self.surface_scene.as_ref() else {
            return;
        };
        let mut tasks = Vec::new();
        for (surface_index, surface) in scene.surfaces.iter().enumerate() {
            for (component_index, component) in surface.components.iter().enumerate() {
                if component.mesh.is_none() {
                    tasks.push(PreloadTask {
                        generation,
                        surface_index,
                        component_index,
                        spec: scene.spec.clone(),
                        surface: component.spec_surface.clone(),
                        surface_volume_path: scene.surface_volume_path.clone(),
                    });
                }
            }
        }

        if tasks.is_empty() {
            return;
        }

        self.log_status(format!(
            "Preloading {} spec surface components in the background.",
            tasks.len()
        ));
        let sender = self.preload_sender.clone();
        let event_proxy = self.event_proxy.clone();
        thread::spawn(move || {
            for task in tasks {
                let result = load_spec_component_mesh(
                    &task.spec,
                    &task.surface,
                    task.surface_volume_path.as_ref(),
                )
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(PreloadResult {
                    generation: task.generation,
                    surface_index: task.surface_index,
                    component_index: task.component_index,
                    path: task.surface.path.clone(),
                    result,
                });
                let _ = event_proxy.send_event(ViewerEvent::SpecPreloadReady);
            }
        });
    }

    fn drain_preload_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.preload_receiver.try_recv() {
            changed |= self.apply_preload_result(result);
        }

        changed
    }

    fn apply_preload_result(&mut self, result: PreloadResult) -> bool {
        if result.generation != self.scene_generation {
            return false;
        }

        match result.result {
            Ok(mesh) => {
                let layout = self.hemisphere_layout_state();
                let mut warmed_cache = false;
                let mut cache_error = None;
                {
                    let Some(scene) = self.surface_scene.as_mut() else {
                        return false;
                    };
                    let Some(surface) = scene.surfaces.get_mut(result.surface_index) else {
                        return false;
                    };
                    let Some(component) = surface.components.get_mut(result.component_index) else {
                        return false;
                    };
                    if component.mesh.is_some() {
                        return false;
                    }
                    component.mesh = Some(mesh);
                    if surface
                        .components
                        .iter()
                        .all(|component| component.mesh.is_some())
                    {
                        match surface.warm_display_cache(layout) {
                            Ok(warmed) => warmed_cache = warmed,
                            Err(error) => cache_error = Some(format!("{error:#}")),
                        }
                    }
                }
                if let Some(error) = cache_error {
                    self.log_status(format!(
                        "Preloaded {}, but failed to warm display cache: {error}.",
                        result.path.display()
                    ));
                    return true;
                }
                if warmed_cache {
                    self.log_status(format!("Preloaded and cached {}.", result.path.display()));
                } else {
                    self.log_status(format!("Preloaded {}.", result.path.display()));
                }
                true
            }
            Err(error) => {
                self.log_status(format!(
                    "Failed to preload {}: {error}",
                    result.path.display()
                ));
                false
            }
        }
    }

    fn activate_scene_surface(&mut self, index: usize) -> Result<()> {
        self.ensure_scene_surface_loaded(index)?;
        let layout = self.hemisphere_layout_state();
        let (surface_count, name, state, path, snapshot) = {
            let Some(scene) = self.surface_scene.as_mut() else {
                bail!("no SUMA spec scene is loaded");
            };
            ensure!(
                index < scene.surfaces.len(),
                "surface index {index} is outside loaded scene"
            );

            scene.active_index = index;
            let surface = &mut scene.surfaces[index];
            let name = surface.name.clone();
            let state = surface.state.clone();
            let path = surface.path.clone();
            let snapshot = surface.display_mesh(layout)?;
            (scene.surfaces.len(), name, state, path, snapshot)
        };

        self.set_active_mesh(snapshot.mesh, snapshot.prepared_geometry);
        self.surface_path = Some(path.clone());
        self.surface_pick = None;
        if self.roi_layer.is_some() {
            self.refresh_roi_layer()?;
        }
        if self.has_both_scene() && self.pair_visibility != PairVisibility::both() {
            self.refresh_active_pair_render_geometry()?;
        }
        if self.overlay_dataset.is_some() {
            self.refresh_overlay_columns()?;
        } else {
            self.upload_surface_buffers();
            self.update_scene_stats();
        }
        self.view_window
            .set_title(&window_title(self.surface_path.as_ref()));
        self.log_status(format!(
            "Active surface {}/{}: {}{}.",
            index + 1,
            surface_count,
            name,
            state
                .as_ref()
                .map_or_else(String::new, |state| format!(" ({state})"))
        ));

        Ok(())
    }

    fn cycle_scene_surface(&mut self, step: isize) -> Result<bool> {
        let Some(scene) = self.surface_scene.as_ref() else {
            self.log_status("No SUMA spec scene is loaded.");
            return Ok(false);
        };
        let len = scene.surfaces.len();
        if len <= 1 {
            self.log_status("The loaded SUMA spec has only one loadable surface.");
            return Ok(false);
        }

        let active = scene.active_index as isize;
        let len = len as isize;
        let next = (active + step).rem_euclid(len) as usize;
        self.activate_scene_surface(next)?;

        Ok(true)
    }

    fn set_active_mesh(
        &mut self,
        mesh: SurfaceMesh,
        prepared_geometry: Option<Arc<PreparedGeometry>>,
    ) {
        self.prepared_geometry_cache = prepared_geometry.map(|geometry| PreparedGeometryCache {
            surface_id: mesh.metadata.id.clone(),
            vertex_count: mesh.vertices.len(),
            face_count: mesh.triangles.len(),
            pair_visibility: PairVisibility::both(),
            geometry,
        });
        self.mesh = Some(mesh);
    }

    fn has_both_scene(&self) -> bool {
        self.surface_scene
            .as_ref()
            .is_some_and(|scene| scene.hemisphere == SpecHemisphere::Both)
    }

    fn active_paired_components(&self) -> Option<(&SceneSurfaceComponent, &SceneSurfaceComponent)> {
        let scene = self.surface_scene.as_ref()?;
        if scene.hemisphere != SpecHemisphere::Both {
            return None;
        }
        let surface = scene.surfaces.get(scene.active_index)?;
        let left = surface
            .components
            .iter()
            .find(|component| component.side == SurfaceSide::Left)?;
        let right = surface
            .components
            .iter()
            .find(|component| component.side == SurfaceSide::Right)?;

        Some((left, right))
    }

    fn active_pair_reference_width(&self) -> Option<f32> {
        let (left, right) = self.active_paired_components()?;
        Some(pair_reference_width(
            left.mesh.as_ref()?,
            right.mesh.as_ref()?,
        ))
    }

    fn roi_component_ranges(&self, mesh: &SurfaceMesh) -> Vec<RoiComponentRange> {
        if let Some((left, right)) = self.active_paired_components() {
            if let (Some(left_mesh), Some(right_mesh)) = (left.mesh.as_ref(), right.mesh.as_ref()) {
                return vec![
                    RoiComponentRange {
                        side: SurfaceSide::Left,
                        node_offset: 0,
                        node_count: left_mesh.vertices.len(),
                        triangle_offset: 0,
                        triangle_count: left_mesh.triangles.len(),
                    },
                    RoiComponentRange {
                        side: SurfaceSide::Right,
                        node_offset: left_mesh.vertices.len() as u32,
                        node_count: right_mesh.vertices.len(),
                        triangle_offset: left_mesh.triangles.len(),
                        triangle_count: right_mesh.triangles.len(),
                    },
                ];
            }
        }

        vec![RoiComponentRange {
            side: mesh.metadata.side.clone(),
            node_offset: 0,
            node_count: mesh.vertices.len(),
            triangle_offset: 0,
            triangle_count: mesh.triangles.len(),
        }]
    }

    fn hemisphere_layout_state(&self) -> HemisphereLayoutState {
        HemisphereLayoutState {
            open_angle_degrees: self.hemisphere_open_angle_degrees,
            separation_distance: self.hemisphere_separation_distance,
        }
    }

    fn begin_pair_drag(&mut self) {
        self.pair_dragging = true;
        self.pair_drag_last_cursor = self.view_cursor_position;
        self.pair_drag_changed = false;
        // Upload each hemisphere's geometry once; the drag then only updates
        // model matrices. If this can't be set up, refresh falls back to the
        // old per-frame geometry rebuild.
        self.pair_drag_render = self.build_pair_drag_render();
        if let Err(error) = self.refresh_pair_drag_uniforms() {
            self.set_error(error);
        }
    }

    fn update_pair_drag(&mut self, cursor: (f64, f64)) {
        if let Some((last_x, last_y)) = self.pair_drag_last_cursor {
            let dx = (cursor.0 - last_x) as f32;
            let dy = (cursor.1 - last_y) as f32;
            if dx.hypot(dy) as f64 >= PAIR_DRAG_PREVIEW_MIN_DELTA_PIXELS {
                if let Err(error) = self.adjust_pair_transform(dx, dy) {
                    self.set_error(error);
                }
                self.pair_drag_last_cursor = Some(cursor);
            }
        } else {
            self.pair_drag_last_cursor = Some(cursor);
        }
    }

    fn finish_pair_drag(&mut self) {
        self.pair_dragging = false;
        self.pair_drag_last_cursor = None;
        if self.pair_drag_changed {
            self.log_status(format!(
                "Hemisphere layout: open {:.1} deg, gap {:.1}.",
                self.hemisphere_open_angle_degrees, self.hemisphere_separation_distance
            ));
        }
        self.pair_drag_changed = false;
        // Drop the drag-only resident buffers and resume the baked path, which
        // keeps picking and the static display correct.
        self.pair_drag_render = None;
        if let Err(error) = self.rebuild_active_scene_surface_mesh() {
            self.set_error(error);
        }
    }

    fn adjust_pair_transform(&mut self, dx: f32, dy: f32) -> Result<()> {
        let Some(pair_width) = self.active_pair_reference_width() else {
            return Ok(());
        };
        let vertical_scale = (pair_width / 700.0).max(0.05);
        self.hemisphere_open_angle_degrees = (self.hemisphere_open_angle_degrees
            + dx * PAIR_OPEN_DEGREES_PER_PIXEL)
            .clamp(0.0, PAIR_MAX_OPEN_DEGREES);
        self.hemisphere_separation_distance = (self.hemisphere_separation_distance
            + -dy * vertical_scale)
            .clamp(0.0, pair_width * PAIR_MAX_DRAG_GAP_FACTOR);
        self.hemisphere_layout = if self.hemisphere_open_angle_degrees <= f32::EPSILON
            && self.hemisphere_separation_distance <= f32::EPSILON
        {
            HemisphereLayout::Closed
        } else {
            HemisphereLayout::Open
        };
        self.pair_drag_changed = true;
        self.preview_active_pair_transform()
    }

    fn preview_active_pair_transform(&mut self) -> Result<()> {
        self.refresh_pair_drag_uniforms()
    }

    fn build_pair_drag_render(&mut self) -> Option<PairDragRender> {
        struct RawComponent {
            side: SurfaceSide,
            positions: Vec<[f32; 3]>,
            normals: Vec<[f32; 3]>,
            triangles: Vec<[u32; 3]>,
            vertex_count: usize,
        }

        let raw: Vec<RawComponent> = {
            let scene = self.surface_scene.as_mut()?;
            if scene.hemisphere != SpecHemisphere::Both {
                return None;
            }
            let index = scene.active_index;
            let surface = scene.surfaces.get_mut(index)?;
            let mut raw = Vec::with_capacity(surface.components.len());
            for component in &mut surface.components {
                let normals = ensure_component_normals(component).ok()?;
                let mesh = component.mesh.as_ref()?;
                raw.push(RawComponent {
                    side: component.side.clone(),
                    positions: mesh.vertices.clone(),
                    normals: (*normals).clone(),
                    triangles: mesh.triangles.clone(),
                    vertex_count: mesh.vertices.len(),
                });
            }
            raw
        };
        if raw.len() != 2 {
            return None;
        }

        // Slice the overlay color cache by cumulative node offset, matching the
        // merged-domain ordering the baked path uses.
        let overlay_colors = self
            .overlay
            .as_ref()
            .filter(|_| self.overlay_visible)
            .map(|overlay| overlay.color_cache.colors.as_slice());
        let roi_colors = self
            .visible_roi_layer()
            .map(|layer| layer.appearance.node_colors.as_slice());
        let dim = self.overlay_appearance.dim;
        let aspect = self.view_config.width as f32 / self.view_config.height as f32;
        let init_bytes = self.camera.uniform_bytes(aspect);

        let mut hemispheres = Vec::with_capacity(raw.len());
        let mut offset = 0usize;
        for component in &raw {
            let colors = overlay_colors.map(|colors| {
                let start = offset.min(colors.len());
                let end = (offset + component.vertex_count).min(colors.len());
                &colors[start..end]
            });
            let roi_colors = roi_colors.map(|colors| {
                let start = offset.min(colors.len());
                let end = (offset + component.vertex_count).min(colors.len());
                &colors[start..end]
            });
            let vertex_bytes = pair_drag_vertex_bytes(
                &component.positions,
                &component.normals,
                colors,
                dim,
                roi_colors,
            );
            let (index_bytes, index_count) = pair_drag_index_bytes(&component.triangles);
            let vertex_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("pair drag vertex buffer"),
                    contents: &vertex_bytes,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });
            let index_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("pair drag index buffer"),
                    contents: &index_bytes,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                });
            let uniform_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("pair drag uniform buffer"),
                        contents: &init_bytes,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    });
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("pair drag bind group"),
                layout: &self.uniform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                }],
            });
            hemispheres.push(PairDragHemisphere {
                side: component.side.clone(),
                vertex_buffer,
                index_buffer,
                index_count,
                uniform_buffer,
                bind_group,
            });
            offset += component.vertex_count;
        }

        Some(PairDragRender { hemispheres })
    }

    fn refresh_pair_drag_uniforms(&mut self) -> Result<()> {
        if self.pair_drag_render.is_none() {
            // GPU drag path unavailable; fall back to rebuilding render geometry.
            return self.refresh_active_pair_render_geometry();
        }

        let layout = self.hemisphere_layout_state();
        let visibility = self.pair_visibility;
        let aspect = self.view_config.width as f32 / self.view_config.height as f32;
        let matrices = {
            let Some(scene) = self.surface_scene.as_ref() else {
                return Ok(());
            };
            let Some(surface) = scene.surfaces.get(scene.active_index) else {
                return Ok(());
            };
            pair_hemisphere_matrices(&surface.components, layout, visibility)
        };

        let Some(drag) = self.pair_drag_render.as_ref() else {
            return Ok(());
        };
        for hemisphere in &drag.hemispheres {
            if let Some((_, matrix)) = matrices.iter().find(|(side, _)| *side == hemisphere.side) {
                let bytes = self.camera.uniform_bytes_with_model(aspect, *matrix);
                self.queue
                    .write_buffer(&hemisphere.uniform_buffer, 0, &bytes);
            }
        }

        Ok(())
    }

    fn refresh_active_pair_render_geometry(&mut self) -> Result<()> {
        let layout = self.hemisphere_layout_state();
        let visibility = self.pair_visibility;
        let indices = self
            .prepared_geometry_cache
            .as_ref()
            .filter(|cache| cache.pair_visibility == visibility)
            .map(|cache| cache.geometry.indices.clone());
        let geometry = {
            let scene = self
                .surface_scene
                .as_mut()
                .context("no SUMA spec scene is loaded")?;
            let surface = scene
                .surfaces
                .get_mut(scene.active_index)
                .context("active surface index is outside loaded scene")?;
            surface.preview_geometry(layout, visibility, indices)?
        };
        if let Some(mesh) = self.mesh.as_ref() {
            self.prepared_geometry_cache = Some(PreparedGeometryCache {
                surface_id: mesh.metadata.id.clone(),
                vertex_count: mesh.vertices.len(),
                face_count: mesh.triangles.len(),
                pair_visibility: visibility,
                geometry: Arc::new(geometry),
            });
            self.upload_surface_buffers();
        }

        Ok(())
    }

    fn toggle_pair_hemisphere_visibility(&mut self, side: SurfaceSide) -> Result<()> {
        if !self.has_both_scene() {
            self.log_status("Load a both-hemisphere spec before toggling hemisphere visibility.");
            return Ok(());
        }
        let Some(next) = self.pair_visibility.toggled(side.clone()) else {
            return Ok(());
        };
        if next == self.pair_visibility {
            return Ok(());
        }

        self.pair_visibility = next;
        self.refresh_active_pair_render_geometry()?;
        self.update_scene_stats();
        self.log_status(format!(
            "{} hemisphere toggled; visible hemispheres: {}.",
            surface_side_label(&side),
            self.pair_visibility.label()
        ));

        Ok(())
    }

    fn overlay_display_text(&self) -> String {
        self.overlay_display_name
            .clone()
            .or_else(|| self.overlay_path.as_deref().map(file_name_display))
            .unwrap_or_else(|| "none".to_string())
    }

    fn roi_display_text(&self) -> String {
        self.roi_layer
            .as_ref()
            .map(|layer| {
                let suffix = if layer.skipped_nodes > 0 {
                    format!(
                        " ({} mapped, {} skipped)",
                        layer.mapped_nodes, layer.skipped_nodes
                    )
                } else {
                    format!(" ({} nodes)", layer.mapped_nodes)
                };
                format!("{}{}", layer.display_name, suffix)
            })
            .or_else(|| self.roi_path.as_deref().map(file_name_display))
            .unwrap_or_else(|| "none".to_string())
    }

    fn pick_surface_display_text(&self) -> String {
        if let Some(pick) = self.surface_pick
            && let Some(component) = self.picked_paired_component(pick)
        {
            return file_name_display(&component.path);
        }

        if let Some((left, right)) = self.active_paired_components() {
            return format!(
                "{} + {}",
                file_name_display(&left.path),
                file_name_display(&right.path)
            );
        }

        self.surface_path
            .as_deref()
            .map(file_name_display)
            .unwrap_or_else(|| "none".to_string())
    }

    fn pick_overlay_display_text(&self) -> String {
        let Some(path) = self.overlay_path.as_deref() else {
            return "none".to_string();
        };

        if let Some(pick) = self.surface_pick
            && let Some(component) = self.picked_paired_component(pick)
            && let Some(path) = paired_overlay_path_for_side(path, &component.side)
        {
            return file_name_display(&path);
        }

        file_name_display(path)
    }

    fn pick_roi_display_text(&self, pick: SurfacePick) -> String {
        let Some(layer) = self.roi_layer.as_ref() else {
            return "none".to_string();
        };
        let labels = layer.labels_for_node(pick.node_index);
        if labels.is_empty() {
            return "none".to_string();
        }

        labels.join(", ")
    }

    fn picked_paired_component(&self, pick: SurfacePick) -> Option<&SceneSurfaceComponent> {
        let (left, right) = self.active_paired_components()?;
        paired_component_for_node(left, right, pick.node_index)
    }

    fn set_hemisphere_layout(&mut self, layout: HemisphereLayout) -> Result<()> {
        let target = match layout {
            HemisphereLayout::Closed => HemisphereLayoutState::closed(),
            HemisphereLayout::Open => HemisphereLayoutState::acorn(),
        };
        if self.hemisphere_layout == layout && self.hemisphere_layout_state() == target {
            return Ok(());
        }

        self.apply_hemisphere_layout_state(layout, target)?;
        self.log_status(format!("Hemisphere layout: {}.", layout.label()));

        Ok(())
    }

    fn apply_hemisphere_layout_state(
        &mut self,
        layout: HemisphereLayout,
        state: HemisphereLayoutState,
    ) -> Result<()> {
        self.hemisphere_layout = layout;
        self.hemisphere_open_angle_degrees = state.open_angle_degrees;
        self.hemisphere_separation_distance = state.separation_distance;
        if let Some(scene) = self.surface_scene.as_ref()
            && scene.hemisphere == SpecHemisphere::Both
        {
            self.rebuild_active_scene_surface_mesh()?;
        }

        Ok(())
    }

    fn rebuild_active_scene_surface_mesh(&mut self) -> Result<()> {
        let Some(index) = self.surface_scene.as_ref().map(|scene| scene.active_index) else {
            return Ok(());
        };
        self.ensure_scene_surface_loaded(index)?;
        let layout = self.hemisphere_layout_state();
        let (path, snapshot) = {
            let scene = self
                .surface_scene
                .as_mut()
                .context("no SUMA spec scene is loaded")?;
            let surface = scene
                .surfaces
                .get_mut(index)
                .context("active surface index is outside loaded scene")?;
            (surface.path.clone(), surface.display_mesh(layout)?)
        };

        self.set_active_mesh(snapshot.mesh, snapshot.prepared_geometry);
        self.surface_path = Some(path);
        self.surface_pick = None;
        if self.roi_layer.is_some() {
            self.refresh_roi_layer()?;
        }
        if self.has_both_scene() && self.pair_visibility != PairVisibility::both() {
            self.refresh_active_pair_render_geometry()?;
        }
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.view_window
            .set_title(&window_title(self.surface_path.as_ref()));

        Ok(())
    }

    fn load_overlay_path(&mut self, path: PathBuf) -> Result<()> {
        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before loading an overlay")?;
        let loaded_selection = self
            .load_overlay_selection(&path, mesh)
            .with_context(|| format!("failed to load overlay {}", path.display()))?;
        let loaded_overlay = loaded_selection.overlay;
        let column_summary =
            overlay_column_summary(&loaded_overlay.dataset, loaded_overlay.columns);
        let overlay_values = loaded_overlay.overlay_values;
        let range = overlay_values.range;

        self.overlay = None;
        self.overlay_values = Some(overlay_values);
        self.overlay_dataset = Some(loaded_overlay.dataset);
        self.overlay_columns = loaded_overlay.columns;
        self.overlay_visible = true;
        self.overlay_appearance = OverlayAppearance::from_range(range);
        self.overlay_symmetric_range = range.min < 0.0 && range.max > 0.0;
        self.overlay_path = Some(path.clone());
        self.overlay_display_name = Some(loaded_selection.display_name);
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Loaded overlay range {:.4} to {:.4}. {column_summary}",
            range.min, range.max
        ));

        Ok(())
    }

    fn load_roi_path(&mut self, path: PathBuf) -> Result<()> {
        self.mesh
            .as_ref()
            .context("load a surface before loading an ROI")?;
        let payloads = read_niml_roi(&path)
            .with_context(|| format!("failed to read ROI {}", path.display()))?;
        ensure!(
            !payloads.is_empty(),
            "ROI file {} did not contain any Node_ROI payloads",
            path.display()
        );
        let rois = payloads
            .into_iter()
            .map(|payload| payload.to_roi())
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("failed to convert ROI {}", path.display()))?;
        let layer = self
            .build_roi_layer(path.clone(), rois)
            .with_context(|| format!("failed to map ROI {}", path.display()))?;
        let roi_count = layer.rois.len();
        let mapped_nodes = layer.mapped_nodes;

        self.roi_path = Some(path.clone());
        self.roi_layer = Some(layer);
        self.roi_visible = true;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Loaded {roi_count} ROI object(s) from {} on {mapped_nodes} nodes.",
            path.display()
        ));

        Ok(())
    }

    fn build_roi_layer(&self, path: PathBuf, rois: Vec<Roi>) -> Result<RoiLayer> {
        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before building ROI display")?;
        let ranges = self.roi_component_ranges(mesh);
        let build = roi_appearance_for_mesh(&rois, mesh, &ranges)?;

        Ok(RoiLayer {
            path: path.clone(),
            display_name: file_name_display(&path),
            rois,
            appearance: build.appearance,
            node_labels: build.node_labels,
            mapped_nodes: build.mapped_nodes,
            skipped_nodes: build.skipped_nodes,
        })
    }

    fn refresh_roi_layer(&mut self) -> Result<()> {
        let Some(existing) = self.roi_layer.take() else {
            return Ok(());
        };
        let path = existing.path;
        let rois = existing.rois;
        self.roi_layer = Some(self.build_roi_layer(path, rois)?);

        Ok(())
    }

    fn apply_initial_overlay_options(
        &mut self,
        subs: Option<&[String]>,
        p_value: Option<f64>,
    ) -> Result<()> {
        if let Some(subs) = subs {
            let dataset = self
                .overlay_dataset
                .as_ref()
                .context("no overlay dataset is loaded")?;
            self.overlay_columns = resolve_overlay_subs(dataset, subs)?;
            self.refresh_overlay_columns()?;
        }

        if let Some(p_value) = p_value {
            self.apply_initial_overlay_p_value(p_value)?;
            self.refresh_overlay_appearance()?;
        }

        Ok(())
    }

    fn apply_initial_overlay_p_value(&mut self, p_value: f64) -> Result<()> {
        let Some(dataset) = self.overlay_dataset.as_ref() else {
            return Ok(());
        };
        let Some(threshold_index) = self.overlay_columns.threshold else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} requested, but no T sub-brick is selected"
            ));
            return Ok(());
        };
        let Some(column) = dataset.columns.get(threshold_index) else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} requested, but T sub-brick #{threshold_index} does not exist"
            ));
            return Ok(());
        };
        let Some(stat_label) = column.stat.as_deref() else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} requested, but T sub-brick #{} '{}' has no stat metadata",
                threshold_index, column.label
            ));
            return Ok(());
        };
        let Some(stat) = AfniStatSpec::parse(stat_label) else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} requested, but stat metadata '{stat_label}' is not supported"
            ));
            return Ok(());
        };
        let Some(threshold_value) = stat.statistic_for_p_value(p_value) else {
            self.warn_and_disable_initial_threshold(format!(
                "--p-val {p_value} could not be converted with stat metadata '{stat_label}'"
            ));
            return Ok(());
        };

        self.overlay_appearance.threshold.enabled = true;
        self.overlay_appearance.threshold.absolute = true;
        self.overlay_appearance.threshold.value = threshold_value as f32;
        self.sanitize_overlay_appearance();
        self.log_status(format!(
            "Initial threshold p <= {p_value:.4} -> T {:.4}.",
            self.overlay_appearance.threshold.value
        ));

        Ok(())
    }

    fn warn_and_disable_initial_threshold(&mut self, message: String) {
        eprintln!("sumaru warning: {message}; threshold disabled.");
        self.overlay_appearance.threshold.enabled = false;
    }

    fn load_overlay_selection(
        &self,
        path: &Path,
        mesh: &SurfaceMesh,
    ) -> Result<LoadedOverlaySelection> {
        if let Some((left, right)) = self.active_paired_components()
            && let Some(paths) = paired_overlay_paths(path)
        {
            let left_mesh = left
                .mesh
                .as_ref()
                .context("left hemisphere surface is still loading")?;
            let right_mesh = right
                .mesh
                .as_ref()
                .context("right hemisphere surface is still loading")?;
            ensure!(
                paths.left_path.exists(),
                "left hemisphere overlay {} does not exist",
                paths.left_path.display()
            );
            ensure!(
                paths.right_path.exists(),
                "right hemisphere overlay {} does not exist",
                paths.right_path.display()
            );

            let left_dataset = load_dataset_from_path(&paths.left_path, left_mesh)
                .with_context(|| format!("failed to load {}", paths.left_path.display()))?;
            let right_dataset = load_dataset_from_path(&paths.right_path, right_mesh)
                .with_context(|| format!("failed to load {}", paths.right_path.display()))?;
            let dataset = paired_overlay_dataset(
                left_dataset,
                right_dataset,
                &mesh.domain,
                left_mesh.vertices.len() as u32,
            )?;
            let overlay = loaded_overlay_from_dataset(dataset, mesh.vertices.len(), "paired NIML")?;

            return Ok(LoadedOverlaySelection {
                overlay,
                display_name: paths.display_name,
            });
        }

        Ok(LoadedOverlaySelection {
            overlay: load_overlay_from_path(path, mesh)?,
            display_name: file_name_display(path),
        })
    }

    fn refresh_overlay_columns(&mut self) -> Result<()> {
        let dataset = self
            .overlay_dataset
            .as_ref()
            .context("no canonical overlay dataset is loaded")?;
        let domain = &self
            .mesh
            .as_ref()
            .context("load a surface before selecting overlay columns")?
            .domain;
        let overlay = overlay_dataset_from_canonical_dataset(
            dataset,
            domain.node_count,
            self.overlay_columns,
        )?;
        let range = overlay.range;
        let column_summary = overlay_column_summary(dataset, self.overlay_columns);
        self.overlay_values = Some(overlay);
        self.overlay_appearance.range = if self.overlay_symmetric_range {
            symmetric_value_range(range)
        } else {
            range
        };
        self.sanitize_overlay_appearance();
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!("Overlay columns: {column_summary}"));

        Ok(())
    }

    fn refresh_overlay_appearance(&mut self) -> Result<()> {
        if self.overlay_dataset.is_none() {
            return Ok(());
        }

        self.sanitize_overlay_appearance();
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();

        Ok(())
    }

    fn rebuild_overlay_model(&mut self) -> Result<()> {
        let dataset = self
            .overlay_dataset
            .as_ref()
            .context("no canonical overlay dataset is loaded")?;
        let domain = &self
            .mesh
            .as_ref()
            .context("load a surface before rebuilding overlay colors")?
            .domain;
        let columns = canonical_overlay_columns(
            self.overlay_columns,
            self.overlay_appearance.threshold.enabled,
        );
        let (threshold, mask_mode) = threshold_and_mask_from_appearance(self.overlay_appearance);
        // Build with an empty cache, apply the real display settings, then
        // compute the color cache exactly once (from_dataset would compute it a
        // first time with default settings and throw that away).
        let mut overlay = Overlay::without_color_cache(dataset, domain, columns)?
            .with_colormap(self.overlay_appearance.colormap.to_color_map())
            .with_intensity_range(RangeSelection::Manual(overlay_range_from_value_range(
                self.overlay_appearance.range,
            )))
            .with_symmetric_range(self.overlay_symmetric_range)
            .with_threshold(threshold, mask_mode)
            .with_opacity(self.overlay_appearance.opacity);

        overlay.rebuild_color_cache(dataset, domain)?;
        self.overlay = Some(overlay);

        Ok(())
    }

    fn toggle_overlay_visibility(&mut self) {
        if self.overlay.is_none() {
            self.log_status("No overlay is loaded.");
            return;
        }

        self.overlay_visible = !self.overlay_visible;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(if self.overlay_visible {
            "Overlay visible."
        } else {
            "Overlay hidden."
        });
    }

    fn visible_overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref().filter(|_| self.overlay_visible)
    }

    fn visible_roi_layer(&self) -> Option<&RoiLayer> {
        self.roi_layer.as_ref().filter(|_| self.roi_visible)
    }

    fn inspect_surface_at_cursor(&mut self) {
        let Some(cursor) = self.view_cursor_position else {
            self.log_status("Move the cursor over the surface before inspecting.");
            return;
        };
        let Some(mesh) = self.mesh.as_ref() else {
            self.log_status("Load a surface before inspecting nodes.");
            return;
        };

        match pick_surface(
            mesh,
            self.overlay_values.as_ref(),
            &self.camera,
            self.view_size,
            cursor,
        ) {
            Some(pick) => {
                self.log_status(pick.status_text());
                self.surface_pick = Some(pick);
                self.upload_surface_buffers();
            }
            None => {
                self.surface_pick = None;
                self.upload_surface_buffers();
                self.log_status("No surface under the cursor.");
            }
        }
    }

    fn refresh_pick_overlay_value(&mut self) {
        if let Some(pick) = &mut self.surface_pick {
            pick.overlay_value = self
                .overlay_values
                .as_ref()
                .and_then(|overlay| overlay.values.get(pick.node_index as usize))
                .copied();
            pick.threshold_value = self
                .overlay_values
                .as_ref()
                .and_then(|overlay| overlay.threshold_values.as_ref())
                .and_then(|values| values.get(pick.node_index as usize))
                .copied();
        }
    }

    fn upload_surface_buffers(&mut self) {
        let Some(mesh) = self.mesh.as_ref() else {
            self.surface_buffers = None;
            self.prepared_geometry_cache = None;
            return;
        };

        if !self
            .prepared_geometry_cache
            .as_ref()
            .is_some_and(|cache| cache.matches(mesh))
        {
            self.prepared_geometry_cache = Some(PreparedGeometryCache {
                surface_id: mesh.metadata.id.clone(),
                vertex_count: mesh.vertices.len(),
                face_count: mesh.triangles.len(),
                pair_visibility: PairVisibility::both(),
                geometry: Arc::new(PreparedGeometry::from_surface(mesh)),
            });
        }

        let geometry = self
            .prepared_geometry_cache
            .as_ref()
            .expect("prepared geometry cache is populated above")
            .geometry
            .clone();
        let selection = self.selection_highlight();
        let prepared_surface = PreparedSurface::from_geometry_with_selection(
            &geometry,
            self.visible_overlay(),
            self.overlay_appearance.dim,
            self.visible_roi_layer().map(|layer| &layer.appearance),
            selection,
        );
        let vertex_bytes = prepared_surface.vertex_bytes();
        let index_bytes = prepared_surface.index_bytes();
        let surface_id = mesh.metadata.id.clone();
        let index_count = prepared_surface.index_count();

        if let Some(buffers) = self.surface_buffers.as_mut() {
            if buffers.vertex_bytes_len == vertex_bytes.len() {
                self.queue
                    .write_buffer(&buffers.vertex_buffer, 0, &vertex_bytes);
            } else {
                buffers.vertex_buffer =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("surface vertex buffer"),
                            contents: &vertex_bytes,
                            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        });
                buffers.vertex_bytes_len = vertex_bytes.len();
            }

            if buffers.surface_id != surface_id
                || buffers.index_bytes_len != index_bytes.len()
                || buffers.index_count != index_count
            {
                buffers.index_buffer =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("surface index buffer"),
                            contents: &index_bytes,
                            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                        });
                buffers.index_bytes_len = index_bytes.len();
                buffers.index_count = index_count;
            }
            buffers.surface_id = surface_id;
            return;
        }

        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("surface vertex buffer"),
                contents: &vertex_bytes,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("surface index buffer"),
                contents: &index_bytes,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            });

        self.surface_buffers = Some(SurfaceBuffers {
            surface_id,
            vertex_buffer,
            vertex_bytes_len: vertex_bytes.len(),
            index_buffer,
            index_bytes_len: index_bytes.len(),
            index_count,
        });
    }

    fn selection_highlight(&self) -> Option<SelectionHighlight> {
        let pick = self.surface_pick?;
        Some(SelectionHighlight {
            node_index: pick.node_index,
            face_index: pick.face_index,
            crosshair_position: pick.normalized_position,
        })
    }

    fn update_scene_stats(&mut self) {
        let Some(mesh) = self.mesh.as_ref() else {
            self.scene_stats = None;
            self.scene_geometry_stats = None;
            return;
        };

        // The expensive part (winding_report + total_area) only depends on
        // geometry, so cache it per surface id. Recolors keep the same id and
        // reuse it, recomputing only the cheap overlay range.
        let id = mesh.metadata.id.clone();
        let cache_hit = matches!(
            &self.scene_geometry_stats,
            Some((cached_id, _)) if *cached_id == id
        );
        let geometry = if cache_hit {
            self.scene_geometry_stats
                .as_ref()
                .expect("cache hit checked above")
                .1
        } else {
            let geometry = SceneGeometryStats::from_mesh(mesh);
            self.scene_geometry_stats = Some((id, geometry));
            geometry
        };

        self.scene_stats = Some(SceneStats {
            geometry,
            overlay_range: self.overlay_values.as_ref().map(|overlay| overlay.range),
        });
    }

    fn show_mode_label(&mut self, mode: CameraMode) {
        self.mode_label = Some(ModeLabel {
            text: mode.label(),
            until: Instant::now() + MODE_LABEL_DURATION,
        });
    }

    /// Returns the active mode-label text and the time remaining before it
    /// expires, so the caller can schedule a repaint to clear it.
    fn active_mode_label(&mut self) -> Option<(&'static str, Duration)> {
        let label = self.mode_label.as_ref()?;
        let now = Instant::now();
        if now >= label.until {
            self.mode_label = None;
            return None;
        }

        Some((label.text, label.until - now))
    }

    fn set_error(&mut self, error: anyhow::Error) {
        eprintln!("sumaru error: {error:#}");
    }

    fn log_status(&self, message: impl AsRef<str>) {
        if self.verbose {
            eprintln!("sumaru: {}", message.as_ref());
        }
    }
}

struct SurfaceBuffers {
    surface_id: SurfaceId,
    vertex_buffer: wgpu::Buffer,
    vertex_bytes_len: usize,
    index_buffer: wgpu::Buffer,
    index_bytes_len: usize,
    index_count: u32,
}

/// Per-hemisphere resident geometry used only while dragging an acorn pair.
/// The geometry is uploaded once (raw, untransformed); the layout drag updates
/// each hemisphere's `uniform_buffer` model matrix instead of the geometry.
struct PairDragRender {
    hemispheres: Vec<PairDragHemisphere>,
}

struct PairDragHemisphere {
    side: SurfaceSide,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

#[derive(Clone)]
struct PreparedGeometryCache {
    surface_id: SurfaceId,
    vertex_count: usize,
    face_count: usize,
    pair_visibility: PairVisibility,
    geometry: Arc<PreparedGeometry>,
}

impl PreparedGeometryCache {
    fn matches(&self, mesh: &SurfaceMesh) -> bool {
        self.surface_id == mesh.metadata.id
            && self.vertex_count == mesh.vertices.len()
            && self.face_count == mesh.triangles.len()
    }
}

#[derive(Debug, Clone)]
struct SurfaceScene {
    spec: SpecFile,
    spec_path: PathBuf,
    surface_volume_path: Option<PathBuf>,
    hemisphere: SpecHemisphere,
    surfaces: Vec<SceneSurface>,
    active_index: usize,
    skipped_surfaces: usize,
    skipped_states: usize,
}

#[derive(Debug, Clone)]
struct SceneSurface {
    name: String,
    state: Option<String>,
    path: PathBuf,
    components: Vec<SceneSurfaceComponent>,
    display_cache: Option<DisplayMeshCache>,
}

#[derive(Debug, Clone)]
struct DisplayMeshCache {
    layout: HemisphereLayoutState,
    mesh: SurfaceMesh,
    prepared_geometry: Arc<PreparedGeometry>,
}

struct DisplayMeshSnapshot {
    mesh: SurfaceMesh,
    prepared_geometry: Option<Arc<PreparedGeometry>>,
}

#[derive(Debug, Clone)]
struct SceneSurfaceComponent {
    name: String,
    state: Option<String>,
    path: PathBuf,
    side: SurfaceSide,
    spec_surface: SpecSurface,
    mesh: Option<SurfaceMesh>,
    normal_cache: Option<Arc<Vec<[f32; 3]>>>,
}

impl SceneSurface {
    fn single(component: SceneSurfaceComponent) -> Self {
        Self {
            name: component.name.clone(),
            state: component.state.clone(),
            path: component.path.clone(),
            components: vec![component],
            display_cache: None,
        }
    }

    fn paired(
        state: String,
        spec_path: PathBuf,
        left: SceneSurfaceComponent,
        right: SceneSurfaceComponent,
    ) -> Self {
        Self {
            name: state.clone(),
            state: Some(state),
            path: spec_path,
            components: vec![left, right],
            display_cache: None,
        }
    }

    fn display_mesh(&mut self, layout: HemisphereLayoutState) -> Result<DisplayMeshSnapshot> {
        ensure!(
            !self.components.is_empty(),
            "scene surface {} has no components",
            self.name
        );
        if self.components.len() == 1 {
            if let Some(cache) = self.display_cache.as_ref() {
                return Ok(DisplayMeshSnapshot {
                    mesh: cache.mesh.clone(),
                    prepared_geometry: Some(cache.prepared_geometry.clone()),
                });
            }
            let mesh = self.components[0]
                .mesh
                .clone()
                .with_context(|| format!("surface {} is still loading", self.name))?;
            let prepared_geometry = Arc::new(PreparedGeometry::from_surface(&mesh));
            self.display_cache = Some(DisplayMeshCache {
                layout,
                mesh: mesh.clone(),
                prepared_geometry: prepared_geometry.clone(),
            });
            return Ok(DisplayMeshSnapshot {
                mesh,
                prepared_geometry: Some(prepared_geometry),
            });
        }

        if let Some(cache) = self.display_cache.as_ref()
            && cache.layout == layout
        {
            return Ok(DisplayMeshSnapshot {
                mesh: cache.mesh.clone(),
                prepared_geometry: Some(cache.prepared_geometry.clone()),
            });
        }

        let mut mesh = composite_component_mesh(&self.components, layout)?;
        mesh.metadata.label = Some(self.name.clone());
        mesh.metadata.source_file = Some(self.path.clone());
        mesh.metadata.side = SurfaceSide::Both;
        mesh.metadata.state_name = self.state.clone();
        if let Some(first) = self.components.first() {
            let first_mesh = first
                .mesh
                .as_ref()
                .with_context(|| format!("surface component {} is still loading", first.name))?;
            mesh.metadata.group_label = first_mesh.metadata.group_label.clone();
            mesh.metadata.subject_label = first_mesh.metadata.subject_label.clone();
            mesh.metadata.surface_kind = first_mesh.metadata.surface_kind.clone();
            mesh.metadata.lineage.parent_volume_id =
                first_mesh.metadata.lineage.parent_volume_id.clone();
        }

        let prepared_geometry = Arc::new(PreparedGeometry::from_surface(&mesh));
        self.display_cache = Some(DisplayMeshCache {
            layout,
            mesh: mesh.clone(),
            prepared_geometry: prepared_geometry.clone(),
        });

        Ok(DisplayMeshSnapshot {
            mesh,
            prepared_geometry: Some(prepared_geometry),
        })
    }

    fn warm_display_cache(&mut self, layout: HemisphereLayoutState) -> Result<bool> {
        if self
            .display_cache
            .as_ref()
            .is_some_and(|cache| self.components.len() == 1 || cache.layout == layout)
        {
            return Ok(false);
        }

        self.display_mesh(layout)?;
        Ok(true)
    }

    fn preview_geometry(
        &mut self,
        layout: HemisphereLayoutState,
        visibility: PairVisibility,
        reusable_indices: Option<Vec<u32>>,
    ) -> Result<PreparedGeometry> {
        if self.components.len() <= 1 {
            let mesh = self.components[0]
                .mesh
                .as_ref()
                .with_context(|| format!("surface {} is still loading", self.name))?;
            return Ok(PreparedGeometry::from_surface(mesh));
        }

        paired_preview_geometry(&mut self.components, layout, visibility, reusable_indices)
    }
}

#[derive(Default)]
struct StatePair {
    left: Option<SceneSurfaceComponent>,
    right: Option<SceneSurfaceComponent>,
}

fn scene_surfaces_from_components(
    spec: &SpecFile,
    components: Vec<SceneSurfaceComponent>,
) -> (Vec<SceneSurface>, usize, Vec<String>) {
    if spec.hemisphere != SpecHemisphere::Both {
        return (
            components.into_iter().map(SceneSurface::single).collect(),
            0,
            Vec::new(),
        );
    }

    paired_scene_surfaces(spec, components)
}

fn paired_scene_surfaces(
    spec: &SpecFile,
    components: Vec<SceneSurfaceComponent>,
) -> (Vec<SceneSurface>, usize, Vec<String>) {
    let mut by_state = BTreeMap::<String, StatePair>::new();
    let mut skipped_states = 0;
    let mut messages = Vec::new();

    for component in components {
        let Some(state) = component.state.clone() else {
            skipped_states += 1;
            messages.push(format!(
                "Skipping both-spec surface {} because it has no SurfaceState.",
                component.path.display()
            ));
            continue;
        };

        let side = component.side.clone();
        let component_name = component.name.clone();
        let pair = by_state.entry(state.clone()).or_default();
        let slot = match side {
            SurfaceSide::Left => &mut pair.left,
            SurfaceSide::Right => &mut pair.right,
            _ => {
                skipped_states += 1;
                messages.push(format!(
                    "Skipping both-spec surface {} for state {state}: side is not left or right.",
                    component.path.display()
                ));
                continue;
            }
        };

        if slot.is_some() {
            skipped_states += 1;
            messages.push(format!(
                "Skipping duplicate both-spec surface {component_name} for state {state}."
            ));
            continue;
        }

        *slot = Some(component);
    }

    let mut ordered_states = Vec::new();
    let mut seen = HashSet::new();
    for state in &spec.states {
        if by_state.contains_key(state) && seen.insert(state.clone()) {
            ordered_states.push(state.clone());
        }
    }
    for state in by_state.keys() {
        if seen.insert(state.clone()) {
            ordered_states.push(state.clone());
        }
    }

    let mut surfaces = Vec::new();
    for state in ordered_states {
        let Some(pair) = by_state.remove(&state) else {
            continue;
        };
        match (pair.left, pair.right) {
            (Some(left), Some(right)) => {
                surfaces.push(SceneSurface::paired(state, spec.path.clone(), left, right));
            }
            (left, right) => {
                skipped_states += 1;
                let missing = match (left.is_some(), right.is_some()) {
                    (true, false) => "right",
                    (false, true) => "left",
                    _ => "left and right",
                };
                messages.push(format!(
                    "Skipping both-spec state {state}: missing {missing} hemisphere surface."
                ));
            }
        }
    }

    (surfaces, skipped_states, messages)
}

fn composite_component_mesh(
    components: &[SceneSurfaceComponent],
    layout: HemisphereLayoutState,
) -> Result<SurfaceMesh> {
    let transforms = component_transforms(components, layout);
    let vertex_count: usize = components
        .iter()
        .filter_map(|component| component.mesh.as_ref())
        .map(|mesh| mesh.vertices.len())
        .sum();
    let face_count: usize = components
        .iter()
        .filter_map(|component| component.mesh.as_ref())
        .map(|mesh| mesh.triangles.len())
        .sum();
    let mut vertices = Vec::with_capacity(vertex_count);
    let mut triangles = Vec::with_capacity(face_count);
    let mut node_offset = 0u32;

    for (component, transform) in components.iter().zip(transforms) {
        let mesh = component
            .mesh
            .as_ref()
            .with_context(|| format!("surface component {} is still loading", component.name))?;
        let center = Vec3::from_array(mesh.bounds.center);
        let rotation = Quat::from_rotation_z(transform.rotation_z_degrees.to_radians());
        vertices.extend(mesh.vertices.iter().map(|position| {
            let point = Vec3::from_array(*position);
            let rotated = center + rotation * (point - center);
            (rotated + transform.offset).to_array()
        }));
        triangles.extend(mesh.triangles.iter().map(|triangle| {
            [
                triangle[0] + node_offset,
                triangle[1] + node_offset,
                triangle[2] + node_offset,
            ]
        }));
        node_offset += u32::try_from(mesh.vertices.len())
            .context("paired surface has too many vertices for u32 indices")?;
    }

    SurfaceMesh::new(vertices, triangles)
}

fn paired_preview_geometry(
    components: &mut [SceneSurfaceComponent],
    layout: HemisphereLayoutState,
    visibility: PairVisibility,
    reusable_indices: Option<Vec<u32>>,
) -> Result<PreparedGeometry> {
    let normals = components
        .iter_mut()
        .map(ensure_component_normals)
        .collect::<Result<Vec<_>>>()?;
    let transforms = component_transforms(components, layout);
    let bounds = transformed_bounds(components, &transforms, visibility)?;
    let center = bounds.center();
    let radius = transformed_radius(components, &transforms, visibility, center).max(1.0);
    let vertex_count = components
        .iter()
        .filter_map(|component| component.mesh.as_ref())
        .map(|mesh| mesh.vertices.len())
        .sum();
    let face_count: usize = components
        .iter()
        .filter(|component| visibility.is_visible(&component.side))
        .filter_map(|component| component.mesh.as_ref())
        .map(|mesh| mesh.triangles.len())
        .sum();
    let mut vertices = Vec::with_capacity(vertex_count);

    for ((component, transform), normals) in components.iter().zip(transforms).zip(normals) {
        let mesh = component
            .mesh
            .as_ref()
            .with_context(|| format!("surface component {} is still loading", component.name))?;
        let component_center = Vec3::from_array(mesh.bounds.center);
        let rotation = Quat::from_rotation_z(transform.rotation_z_degrees.to_radians());
        vertices.extend(
            mesh.vertices
                .iter()
                .zip(normals.iter())
                .map(|(position, normal)| {
                    let point = Vec3::from_array(*position);
                    let rotated = component_center + rotation * (point - component_center);
                    let normal = (rotation * Vec3::from_array(*normal)).normalize_or_zero();
                    PreparedGeometryVertex {
                        position: ((rotated + transform.offset - center) / radius).to_array(),
                        normal: normal.to_array(),
                    }
                }),
        );
    }

    let expected_index_len = face_count * 3;
    let indices = reusable_indices
        .filter(|indices| indices.len() == expected_index_len)
        .unwrap_or_else(|| component_indices(components, visibility, expected_index_len));

    Ok(PreparedGeometry { vertices, indices })
}

fn paired_component_for_node<'a>(
    left: &'a SceneSurfaceComponent,
    right: &'a SceneSurfaceComponent,
    node_index: u32,
) -> Option<&'a SceneSurfaceComponent> {
    let left_nodes = left.mesh.as_ref()?.vertices.len() as u32;
    if node_index < left_nodes {
        return Some(left);
    }

    let right_nodes = right.mesh.as_ref()?.vertices.len() as u32;
    let right_limit = left_nodes.checked_add(right_nodes)?;
    (node_index < right_limit).then_some(right)
}

/// O(1) framing (center + radius) for the acorn pair while dragging. It
/// approximates the exact transformed bounds with per-hemisphere bounding
/// spheres (transformed component center + the mesh's bounding-sphere radius),
/// so no per-vertex work runs per frame. It slightly over-estimates the exact
/// fit the baked release path computes, which can show as a small scale change
/// when the drag ends.
fn pair_framing(
    components: &[SceneSurfaceComponent],
    transforms: &[ComponentTransform],
    visibility: PairVisibility,
) -> Option<(Vec3, f32)> {
    let mut bounds = TransformedBounds::empty();
    let mut any_visible = false;
    for (component, transform) in components.iter().zip(transforms) {
        if !visibility.is_visible(&component.side) {
            continue;
        }
        let Some(mesh) = component.mesh.as_ref() else {
            continue;
        };
        let corner = transformed_corner_bounds(mesh, *transform);
        bounds.include(corner.min);
        bounds.include(corner.max);
        any_visible = true;
    }
    if !any_visible {
        return None;
    }

    let center = bounds.center();
    let radius = components
        .iter()
        .zip(transforms)
        .filter(|(component, _)| visibility.is_visible(&component.side))
        .filter_map(|(component, transform)| component.mesh.as_ref().map(|mesh| (mesh, transform)))
        .map(|(mesh, transform)| {
            let transformed_center =
                transform_point(mesh, *transform, Vec3::from_array(mesh.bounds.center));
            transformed_center.distance(center) + mesh.bounds.radius
        })
        .fold(0.0_f32, f32::max)
        .max(1.0);

    Some((center, radius))
}

/// Per-hemisphere display model matrices for the current layout. Cheap (no
/// per-vertex allocation, transform, or upload) so dragging only writes small
/// uniforms.
fn pair_hemisphere_matrices(
    components: &[SceneSurfaceComponent],
    layout: HemisphereLayoutState,
    visibility: PairVisibility,
) -> Vec<(SurfaceSide, Mat4)> {
    let transforms = component_transforms(components, layout);
    let Some((center, radius)) = pair_framing(components, &transforms, visibility) else {
        return Vec::new();
    };
    components
        .iter()
        .zip(transforms)
        .filter_map(|(component, transform)| {
            let mesh = component.mesh.as_ref()?;
            let component_center = Vec3::from_array(mesh.bounds.center);
            Some((
                component.side.clone(),
                hemisphere_model_matrix(
                    component_center,
                    transform.rotation_z_degrees,
                    transform.offset,
                    center,
                    radius,
                ),
            ))
        })
        .collect()
}

/// Packs raw (untransformed) per-hemisphere vertices into the standard
/// position+normal+color vertex layout. The model matrix applies the layout
/// transform on the GPU, so positions and normals here stay in mesh space.
fn pair_drag_vertex_bytes(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    colors: Option<&[[f32; 4]]>,
    dim: f32,
    roi_colors: Option<&[Option<[f32; 4]>]>,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(positions.len() * 40);
    for (index, (position, normal)) in positions.iter().zip(normals).enumerate() {
        for value in position.iter().chain(normal.iter()) {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        let color = mesh::compose_vertex_color(
            colors.and_then(|colors| colors.get(index)).copied(),
            dim,
            roi_colors
                .and_then(|colors| colors.get(index))
                .copied()
                .flatten(),
        );
        for value in color {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
    }
    bytes
}

fn pair_drag_index_bytes(triangles: &[[u32; 3]]) -> (Vec<u8>, u32) {
    let mut bytes = Vec::with_capacity(triangles.len() * 12);
    for triangle in triangles {
        for index in triangle {
            bytes.extend_from_slice(&index.to_ne_bytes());
        }
    }
    (bytes, (triangles.len() * 3) as u32)
}

fn ensure_component_normals(component: &mut SceneSurfaceComponent) -> Result<Arc<Vec<[f32; 3]>>> {
    if component.normal_cache.is_none() {
        let mesh = component
            .mesh
            .as_ref()
            .with_context(|| format!("surface component {} is still loading", component.name))?;
        component.normal_cache = Some(Arc::new(mesh.vertex_normals()));
    }

    Ok(component
        .normal_cache
        .as_ref()
        .expect("component normal cache is populated above")
        .clone())
}

fn component_indices(
    components: &[SceneSurfaceComponent],
    visibility: PairVisibility,
    expected_len: usize,
) -> Vec<u32> {
    let mut indices = Vec::with_capacity(expected_len);
    let mut node_offset = 0_u32;
    for component in components {
        if let Some(mesh) = component.mesh.as_ref() {
            if visibility.is_visible(&component.side) {
                indices.extend(mesh.triangles.iter().flat_map(|triangle| {
                    [
                        triangle[0] + node_offset,
                        triangle[1] + node_offset,
                        triangle[2] + node_offset,
                    ]
                }));
            }
            node_offset += mesh.vertices.len() as u32;
        }
    }

    indices
}

fn component_transforms(
    components: &[SceneSurfaceComponent],
    layout: HemisphereLayoutState,
) -> Vec<ComponentTransform> {
    let mut transforms = vec![ComponentTransform::default(); components.len()];
    if components.len() != 2 {
        return transforms;
    }

    let Some(left_index) = components
        .iter()
        .position(|component| component.side == SurfaceSide::Left)
    else {
        return transforms;
    };
    let Some(right_index) = components
        .iter()
        .position(|component| component.side == SurfaceSide::Right)
    else {
        return transforms;
    };

    let Some(left_mesh) = components[left_index].mesh.as_ref() else {
        return transforms;
    };
    let Some(right_mesh) = components[right_index].mesh.as_ref() else {
        return transforms;
    };

    let clearance = pair_default_clearance(left_mesh, right_mesh);
    let auto_spread = pair_auto_spread_distance(left_mesh, right_mesh, layout.open_angle_degrees);
    let mut half_shift = ((clearance + layout.separation_distance) * 0.5) + auto_spread;

    transforms[left_index].offset.x -= half_shift;
    transforms[left_index].rotation_z_degrees = layout.open_angle_degrees;
    transforms[right_index].offset.x += half_shift;
    transforms[right_index].rotation_z_degrees = -layout.open_angle_degrees;

    let extra_spacing = pair_bounds_overlap_extra_spacing(
        left_mesh,
        right_mesh,
        transforms[left_index],
        transforms[right_index],
    );
    if extra_spacing > 0.0 {
        half_shift += extra_spacing * 0.5;
        transforms[left_index].offset.x = -half_shift;
        transforms[right_index].offset.x = half_shift;
    }

    transforms
}

/// Builds the affine model matrix that reproduces the per-hemisphere display
/// transform `paired_preview_geometry` bakes into vertices, so the GPU can
/// apply it through the shader's `model` uniform instead of the CPU
/// re-transforming and re-uploading every vertex on each drag frame.
///
/// The baked transform is `p -> (c + R*(p - c) + offset - center) / radius`,
/// where `c` is the component center, `R` the Z rotation, and `center`/`radius`
/// the scene normalization. The uniform scale keeps normals correct under the
/// same matrix (the shader renormalizes them).
fn hemisphere_model_matrix(
    component_center: Vec3,
    rotation_z_degrees: f32,
    offset: Vec3,
    scene_center: Vec3,
    radius: f32,
) -> Mat4 {
    let inv_radius = 1.0 / radius;
    Mat4::from_scale(Vec3::splat(inv_radius))
        * Mat4::from_translation(component_center + offset - scene_center)
        * Mat4::from_rotation_z(rotation_z_degrees.to_radians())
        * Mat4::from_translation(-component_center)
}

#[cfg(test)]
mod acorn_matrix_tests {
    use super::hemisphere_model_matrix;
    use glam::{Quat, Vec3};

    /// Mirrors the per-vertex transform baked by `paired_preview_geometry`.
    fn baked_position(
        p: Vec3,
        component_center: Vec3,
        rotation_z_degrees: f32,
        offset: Vec3,
        scene_center: Vec3,
        radius: f32,
    ) -> Vec3 {
        let rotation = Quat::from_rotation_z(rotation_z_degrees.to_radians());
        let rotated = component_center + rotation * (p - component_center);
        (rotated + offset - scene_center) / radius
    }

    #[test]
    fn model_matrix_matches_baked_vertex_transform() {
        let component_center = Vec3::new(2.0, -3.0, 1.5);
        let offset = Vec3::new(-12.0, 0.0, 0.0);
        let scene_center = Vec3::new(0.5, 0.5, 0.0);
        let radius = 47.0;
        let angle = 18.0;
        let matrix = hemisphere_model_matrix(component_center, angle, offset, scene_center, radius);

        for p in [
            Vec3::ZERO,
            Vec3::new(10.0, -5.0, 3.0),
            Vec3::new(-7.0, 8.0, -2.0),
            component_center,
        ] {
            let expected = baked_position(p, component_center, angle, offset, scene_center, radius);
            let actual = matrix.transform_point3(p);
            assert!(
                (actual - expected).length() < 1e-4,
                "matrix {actual:?} != baked {expected:?} for {p:?}"
            );
        }
    }

    #[test]
    fn closed_layout_is_plain_normalization() {
        let scene_center = Vec3::new(1.0, 2.0, 3.0);
        let radius = 10.0;
        let matrix = hemisphere_model_matrix(
            Vec3::new(4.0, 4.0, 4.0),
            0.0,
            Vec3::ZERO,
            scene_center,
            radius,
        );
        let p = Vec3::new(11.0, 12.0, 13.0);
        let expected = (p - scene_center) / radius;
        assert!((matrix.transform_point3(p) - expected).length() < 1e-4);
    }
}

#[derive(Debug, Clone, Copy)]
struct TransformedBounds {
    min: Vec3,
    max: Vec3,
}

impl TransformedBounds {
    fn empty() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }

    fn include(&mut self, point: Vec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

fn transformed_bounds(
    components: &[SceneSurfaceComponent],
    transforms: &[ComponentTransform],
    visibility: PairVisibility,
) -> Result<TransformedBounds> {
    let mut bounds = TransformedBounds::empty();
    for (component, transform) in components.iter().zip(transforms) {
        if !visibility.is_visible(&component.side) {
            continue;
        }
        let mesh = component
            .mesh
            .as_ref()
            .with_context(|| format!("surface component {} is still loading", component.name))?;
        for position in &mesh.vertices {
            bounds.include(transform_point(
                mesh,
                *transform,
                Vec3::from_array(*position),
            ));
        }
    }

    Ok(bounds)
}

fn transformed_radius(
    components: &[SceneSurfaceComponent],
    transforms: &[ComponentTransform],
    visibility: PairVisibility,
    center: Vec3,
) -> f32 {
    components
        .iter()
        .zip(transforms)
        .filter(|(component, _)| visibility.is_visible(&component.side))
        .filter_map(|(component, transform)| component.mesh.as_ref().map(|mesh| (mesh, transform)))
        .flat_map(|(mesh, transform)| {
            mesh.vertices
                .iter()
                .map(move |position| transform_point(mesh, *transform, Vec3::from_array(*position)))
        })
        .map(|point| (point - center).length())
        .fold(0.0, f32::max)
}

fn transform_point(mesh: &SurfaceMesh, transform: ComponentTransform, point: Vec3) -> Vec3 {
    let center = Vec3::from_array(mesh.bounds.center);
    let rotation = Quat::from_rotation_z(transform.rotation_z_degrees.to_radians());
    center + rotation * (point - center) + transform.offset
}

fn transformed_corner_bounds(
    mesh: &SurfaceMesh,
    transform: ComponentTransform,
) -> TransformedBounds {
    let min = Vec3::from_array(mesh.bounds.min);
    let max = Vec3::from_array(mesh.bounds.max);
    let mut bounds = TransformedBounds::empty();
    for x in [min.x, max.x] {
        for y in [min.y, max.y] {
            for z in [min.z, max.z] {
                bounds.include(transform_point(mesh, transform, Vec3::new(x, y, z)));
            }
        }
    }

    bounds
}

fn pair_bounds_overlap_extra_spacing(
    left_mesh: &SurfaceMesh,
    right_mesh: &SurfaceMesh,
    left_transform: ComponentTransform,
    right_transform: ComponentTransform,
) -> f32 {
    let left = transformed_corner_bounds(left_mesh, left_transform);
    let right = transformed_corner_bounds(right_mesh, right_transform);
    let x_overlap = left.max.x.min(right.max.x) - left.min.x.max(right.min.x);
    let y_overlap = left.max.y.min(right.max.y) - left.min.y.max(right.min.y);
    let z_overlap = left.max.z.min(right.max.z) - left.min.z.max(right.min.z);
    if x_overlap <= 0.0 || y_overlap <= 0.0 || z_overlap <= 0.0 {
        return 0.0;
    }

    x_overlap + PAIR_MIN_SURFACE_CLEARANCE
}

fn pair_reference_width(left_mesh: &SurfaceMesh, right_mesh: &SurfaceMesh) -> f32 {
    let min_x = left_mesh.bounds.min[0].min(right_mesh.bounds.min[0]);
    let max_x = left_mesh.bounds.max[0].max(right_mesh.bounds.max[0]);
    (max_x - min_x).abs().max(1.0)
}

fn pair_default_clearance(left_mesh: &SurfaceMesh, right_mesh: &SurfaceMesh) -> f32 {
    let desired_gap = pair_reference_width(left_mesh, right_mesh) * PAIR_MIN_CLEARANCE_FRACTION;
    let current_gap = right_mesh.bounds.min[0] - left_mesh.bounds.max[0];
    (desired_gap - current_gap).max(0.0)
}

fn pair_auto_spread_distance(
    left_mesh: &SurfaceMesh,
    right_mesh: &SurfaceMesh,
    open_angle_degrees: f32,
) -> f32 {
    let left_half_width = ((left_mesh.bounds.max[0] - left_mesh.bounds.min[0]) * 0.5).max(0.0);
    let right_half_width = ((right_mesh.bounds.max[0] - right_mesh.bounds.min[0]) * 0.5).max(0.0);
    let mean_half_width = (left_half_width + right_half_width) * 0.5;
    mean_half_width * open_angle_degrees.to_radians().sin() * 0.9
}

#[derive(Debug, Clone)]
struct LoadedOverlay {
    overlay_values: OverlayDataset,
    dataset: Dataset,
    columns: OverlayColumnSelections,
}

#[derive(Debug, Clone)]
struct LoadedOverlaySelection {
    overlay: LoadedOverlay,
    display_name: String,
}

#[derive(Debug, Clone)]
struct RoiLayer {
    path: PathBuf,
    display_name: String,
    rois: Vec<Roi>,
    appearance: RoiAppearance,
    node_labels: Vec<Vec<String>>,
    mapped_nodes: usize,
    skipped_nodes: usize,
}

impl RoiLayer {
    fn labels_for_node(&self, node: u32) -> &[String] {
        self.node_labels
            .get(node as usize)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

#[derive(Debug, Clone)]
struct RoiAppearanceBuild {
    appearance: RoiAppearance,
    node_labels: Vec<Vec<String>>,
    mapped_nodes: usize,
    skipped_nodes: usize,
}

#[derive(Debug, Clone)]
struct RoiComponentRange {
    side: SurfaceSide,
    node_offset: u32,
    node_count: usize,
    triangle_offset: usize,
    triangle_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PairedOverlayPaths {
    left_path: PathBuf,
    right_path: PathBuf,
    display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct OverlayColumnSelections {
    intensity: usize,
    threshold: Option<usize>,
    brightness: Option<usize>,
}

#[derive(Debug, Clone)]
struct OverlayColumnOption {
    index: usize,
    label: String,
    is_numeric: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SurfacePick {
    node_index: u32,
    face_index: usize,
    surface_position: [f32; 3],
    normalized_position: [f32; 3],
    overlay_value: Option<f32>,
    threshold_value: Option<f32>,
}

impl SurfacePick {
    fn status_text(self) -> String {
        format!(
            "Inspected node {}, triangle {}, {}.",
            self.node_index,
            self.face_index,
            picked_overlay_value_label(self)
        )
    }
}

#[derive(Debug, Clone)]
struct SceneStats {
    geometry: SceneGeometryStats,
    overlay_range: Option<ValueRange>,
}

/// Geometry-derived scene statistics. Computing these runs `winding_report`,
/// which builds topology and is O(n) with heavy allocation, so the viewer
/// caches them per surface id and only recomputes when the mesh changes.
#[derive(Debug, Clone, Copy)]
struct SceneGeometryStats {
    node_count: usize,
    face_count: usize,
    total_area: f32,
    boundary_edges: usize,
    non_manifold_edges: usize,
    normal_direction: NormalDirection,
}

impl SceneGeometryStats {
    fn from_mesh(mesh: &SurfaceMesh) -> Self {
        let winding = mesh.winding_report();

        Self {
            node_count: mesh.vertices.len(),
            face_count: mesh.triangles.len(),
            total_area: mesh.total_area(),
            boundary_edges: winding.boundary_edges,
            non_manifold_edges: winding.non_manifold_edges,
            normal_direction: winding.normal_direction,
        }
    }
}

struct InputResponse {
    consumed: bool,
    repaint: bool,
}

struct ControlUiOutput {
    actions: Vec<UiAction>,
    desired_control_size_points: egui::Vec2,
}

#[derive(Debug, Clone, Copy)]
enum ViewerEvent {
    SpecPreloadReady,
}

struct PreloadTask {
    generation: u64,
    surface_index: usize,
    component_index: usize,
    spec: SpecFile,
    surface: SpecSurface,
    surface_volume_path: Option<PathBuf>,
}

struct PreloadResult {
    generation: u64,
    surface_index: usize,
    component_index: usize,
    path: PathBuf,
    result: std::result::Result<SurfaceMesh, String>,
}

enum UiAction {
    PickSurface,
    PickOverlay,
    PickRoi,
    PickSpec,
    PickSurfaceVolume,
    RefreshOverlayColumns,
    RefreshOverlayAppearance,
    ResetCamera,
    ToggleCameraMode,
    ToggleBackground,
    Preset(PresetOrientation),
    HemisphereLayout(HemisphereLayout),
    SelectSceneSurface(usize),
    SaveScreenshot,
    SaveMontage,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MontageShot {
    layout: Option<MontageLayout>,
    camera: MontageCamera,
    padding: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MontageLayout {
    layout: HemisphereLayout,
    state: HemisphereLayoutState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MontageCamera {
    Preset(PresetOrientation),
    Direction { eye_direction: Vec3, up: Vec3 },
}

fn standard_montage_shots() -> [MontageShot; 4] {
    [
        MontageShot {
            layout: None,
            camera: MontageCamera::Preset(PresetOrientation::Left),
            padding: MONTAGE_DEFAULT_PADDING,
        },
        MontageShot {
            layout: None,
            camera: MontageCamera::Preset(PresetOrientation::Right),
            padding: MONTAGE_DEFAULT_PADDING,
        },
        MontageShot {
            layout: None,
            camera: MontageCamera::Preset(PresetOrientation::Top),
            padding: MONTAGE_DEFAULT_PADDING,
        },
        MontageShot {
            layout: None,
            camera: MontageCamera::Preset(PresetOrientation::Bottom),
            padding: MONTAGE_DEFAULT_PADDING,
        },
    ]
}

fn paired_spec_montage_shots() -> [MontageShot; 4] {
    let closed = Some(MontageLayout {
        layout: HemisphereLayout::Closed,
        state: HemisphereLayoutState::closed(),
    });
    let open = Some(MontageLayout {
        layout: HemisphereLayout::Open,
        state: HemisphereLayoutState::acorn(),
    });

    [
        MontageShot {
            layout: closed,
            camera: MontageCamera::Preset(PresetOrientation::Top),
            padding: MONTAGE_PAIRED_CLOSED_PADDING,
        },
        MontageShot {
            layout: closed,
            camera: MontageCamera::Preset(PresetOrientation::Bottom),
            padding: MONTAGE_PAIRED_CLOSED_PADDING,
        },
        MontageShot {
            layout: open,
            camera: MontageCamera::Direction {
                eye_direction: Vec3::Y,
                up: Vec3::Z,
            },
            padding: MONTAGE_OPEN_PADDING,
        },
        MontageShot {
            layout: open,
            camera: MontageCamera::Direction {
                eye_direction: Vec3::NEG_Y,
                up: Vec3::Z,
            },
            padding: MONTAGE_OPEN_PADDING,
        },
    ]
}

enum RenderStatus {
    Rendered,
    Skipped,
    Reconfigure,
    ValidationError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HemisphereLayout {
    Closed,
    Open,
}

impl HemisphereLayout {
    fn label(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PairVisibility {
    left: bool,
    right: bool,
}

impl PairVisibility {
    fn both() -> Self {
        Self {
            left: true,
            right: true,
        }
    }

    fn is_visible(self, side: &SurfaceSide) -> bool {
        match side {
            SurfaceSide::Left => self.left,
            SurfaceSide::Right => self.right,
            _ => true,
        }
    }

    fn toggled(self, side: SurfaceSide) -> Option<Self> {
        let mut next = self;
        match side {
            SurfaceSide::Left => next.left = !next.left,
            SurfaceSide::Right => next.right = !next.right,
            _ => return None,
        }
        (next.left || next.right).then_some(next)
    }

    fn label(self) -> &'static str {
        match (self.left, self.right) {
            (true, true) => "left+right",
            (true, false) => "left only",
            (false, true) => "right only",
            (false, false) => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct HemisphereLayoutState {
    open_angle_degrees: f32,
    separation_distance: f32,
}

impl HemisphereLayoutState {
    fn closed() -> Self {
        Self {
            open_angle_degrees: 0.0,
            separation_distance: 0.0,
        }
    }

    fn acorn() -> Self {
        Self {
            open_angle_degrees: PAIR_MAX_OPEN_DEGREES,
            separation_distance: PAIR_ACORN_EXTRA_GAP,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ComponentTransform {
    offset: Vec3,
    rotation_z_degrees: f32,
}

impl Default for ComponentTransform {
    fn default() -> Self {
        Self {
            offset: Vec3::ZERO,
            rotation_z_degrees: 0.0,
        }
    }
}

struct ModeLabel {
    text: &'static str,
    until: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundMode {
    Black,
    White,
}

impl BackgroundMode {
    fn toggle(&mut self) {
        *self = match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        };
    }

    fn color(self) -> wgpu::Color {
        match self {
            Self::Black => BLACK_BACKGROUND,
            Self::White => WHITE_BACKGROUND,
        }
    }

    fn next_label(self) -> &'static str {
        match self {
            Self::Black => "White background",
            Self::White => "Black background",
        }
    }

    fn rgba8(self) -> [u8; 4] {
        match self {
            Self::Black => [0, 0, 0, 255],
            Self::White => [255, 255, 255, 255],
        }
    }
}

fn window_title(surface_path: Option<&PathBuf>) -> String {
    surface_path.map_or_else(
        || "sumaru".to_string(),
        |path| format!("sumaru - {}", path.display()),
    )
}

fn apply_spec_surface_metadata(
    mesh: &mut SurfaceMesh,
    spec: &SpecFile,
    surface: &SpecSurface,
    surface_volume_path: Option<&PathBuf>,
) {
    mesh.metadata.label = Some(surface.name.clone());
    mesh.metadata.group_label = spec.group.clone();
    if mesh.metadata.subject_label.is_none() {
        mesh.metadata.subject_label = spec.group.clone();
    }
    if let Some(state) = &surface.state {
        mesh.metadata.state_name = Some(state.clone());
    }
    if surface.side != SurfaceSide::Unknown {
        mesh.metadata.side = surface.side.clone();
    }
    if let Some(anatomical) = surface.anatomical {
        mesh.metadata.anatomically_correct = if anatomical {
            AnatomicalCorrectness::Correct
        } else {
            AnatomicalCorrectness::Incorrect
        };
    }
    if let Some(embed_dimension) = surface.embed_dimension {
        mesh.metadata.embedding_dimension = embed_dimension;
    }
    mesh.metadata.lineage.local_domain_parent = surface.local_domain_parent.clone();
    mesh.metadata.lineage.local_curvature_parent = surface.local_curvature_parent.clone();
    apply_surface_volume_parent(mesh, surface_volume_path);
}

fn load_spec_component_mesh(
    spec: &SpecFile,
    surface: &SpecSurface,
    surface_volume_path: Option<&PathBuf>,
) -> Result<SurfaceMesh> {
    let mut mesh = SurfaceMesh::from_gifti_path(&surface.path)
        .with_context(|| format!("failed to load spec surface {}", surface.path.display()))?;
    apply_spec_surface_metadata(&mut mesh, spec, surface, surface_volume_path);

    Ok(mesh)
}

fn apply_surface_volume_parent(mesh: &mut SurfaceMesh, surface_volume_path: Option<&PathBuf>) {
    mesh.metadata.lineage.parent_volume_id =
        surface_volume_path.map(|path| path.display().to_string());
}

fn canonical_or_original_path(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn save_screenshot_file(
    title: &str,
    default_name: &str,
    current_path: Option<&PathBuf>,
) -> Option<PathBuf> {
    let dialog = dialog_with_start_directory(
        rfd::FileDialog::new()
            .set_title(title)
            .add_filter("PNG image", &["png"])
            .set_file_name(default_name),
        current_path,
    );

    dialog.save_file().map(screenshot::append_png_extension)
}

fn timestamped_png_name(prefix: &str) -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());

    timestamped_png_name_from_unix_seconds(prefix, seconds)
}

fn timestamped_png_name_from_unix_seconds(prefix: &str, seconds: u64) -> String {
    let timestamp = UtcTimestampParts::from_unix_seconds(seconds);

    format!(
        "{prefix}_{:04}-{:02}-{:02}_{:02}{:02}{:02}.png",
        timestamp.year,
        timestamp.month,
        timestamp.day,
        timestamp.hour,
        timestamp.minute,
        timestamp.second
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UtcTimestampParts {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl UtcTimestampParts {
    fn from_unix_seconds(seconds: u64) -> Self {
        let days = (seconds / SECONDS_PER_DAY) as i64;
        let seconds_of_day = seconds % SECONDS_PER_DAY;
        let (year, month, day) = civil_from_unix_days(days);

        Self {
            year,
            month,
            day,
            hour: (seconds_of_day / SECONDS_PER_HOUR) as u32,
            minute: ((seconds_of_day % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE) as u32,
            second: (seconds_of_day % SECONDS_PER_MINUTE) as u32,
        }
    }
}

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;

fn civil_from_unix_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    (year as i32, month as u32, day as u32)
}

fn pick_surface_file(current_path: Option<&PathBuf>) -> Option<PathBuf> {
    let dialog = dialog_with_start_directory(
        rfd::FileDialog::new()
            .set_title("Open surface")
            .add_filter("GIFTI surface", &["gii"]),
        current_path,
    );

    dialog.pick_file()
}

fn pick_overlay_file(current_path: Option<&PathBuf>) -> Option<PathBuf> {
    let dialog = dialog_with_start_directory(
        rfd::FileDialog::new()
            .set_title("Open overlay")
            .add_filter("GIFTI or SUMA dataset", &["gii", "dset", "niml.dset"]),
        current_path,
    );

    dialog.pick_file()
}

fn pick_roi_file(current_path: Option<&PathBuf>) -> Option<PathBuf> {
    let dialog = dialog_with_start_directory(
        rfd::FileDialog::new()
            .set_title("Open ROI")
            .add_filter("SUMA ROI", &["roi", "niml.roi"]),
        current_path,
    );

    dialog.pick_file()
}

fn pick_spec_file(current_path: Option<&PathBuf>) -> Option<PathBuf> {
    let dialog = dialog_with_start_directory(
        rfd::FileDialog::new()
            .set_title("Open SUMA spec")
            .add_filter("SUMA spec", &["spec"]),
        current_path,
    );

    dialog.pick_file()
}

fn pick_surface_volume_file(current_path: Option<&PathBuf>) -> Option<PathBuf> {
    let dialog = dialog_with_start_directory(
        rfd::FileDialog::new()
            .set_title("Open surface volume")
            .add_filter(
                "Surface volume",
                &["nii", "gz", "HEAD", "BRIK", "head", "brik"],
            ),
        current_path,
    );

    dialog.pick_file()
}

fn paired_overlay_paths(path: &Path) -> Option<PairedOverlayPaths> {
    let file_name = path.file_name()?.to_str()?;
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    for &pattern in HEMISPHERE_FILE_PATTERNS {
        if let Some(paths) = paired_overlay_paths_for_pattern(&parent, file_name, pattern) {
            return Some(paths);
        }
    }

    None
}

fn paired_overlay_path_for_side(path: &Path, side: &SurfaceSide) -> Option<PathBuf> {
    let paths = paired_overlay_paths(path)?;
    match side {
        SurfaceSide::Left => Some(paths.left_path),
        SurfaceSide::Right => Some(paths.right_path),
        _ => None,
    }
}

fn paired_overlay_paths_for_pattern(
    parent: &Path,
    file_name: &str,
    pattern: HemisphereFilePattern,
) -> Option<PairedOverlayPaths> {
    if file_name.contains(pattern.left) {
        let right_name = file_name.replacen(pattern.left, pattern.right, 1);
        let display_name = file_name.replacen(pattern.left, pattern.wildcard, 1);
        return Some(PairedOverlayPaths {
            left_path: parent.join(file_name),
            right_path: parent.join(right_name),
            display_name,
        });
    }
    if file_name.contains(pattern.right) {
        let left_name = file_name.replacen(pattern.right, pattern.left, 1);
        let display_name = file_name.replacen(pattern.right, pattern.wildcard, 1);
        return Some(PairedOverlayPaths {
            left_path: parent.join(left_name),
            right_path: parent.join(file_name),
            display_name,
        });
    }

    None
}

#[derive(Debug, Clone, Copy)]
struct HemisphereFilePattern {
    left: &'static str,
    right: &'static str,
    wildcard: &'static str,
}

const HEMISPHERE_FILE_PATTERNS: &[HemisphereFilePattern] = &[
    HemisphereFilePattern {
        left: "_lh_",
        right: "_rh_",
        wildcard: "_?h_",
    },
    HemisphereFilePattern {
        left: "_lh.",
        right: "_rh.",
        wildcard: "_?h.",
    },
    HemisphereFilePattern {
        left: ".lh.",
        right: ".rh.",
        wildcard: ".?h.",
    },
    HemisphereFilePattern {
        left: "-lh-",
        right: "-rh-",
        wildcard: "-?h-",
    },
    HemisphereFilePattern {
        left: "_lh-",
        right: "_rh-",
        wildcard: "_?h-",
    },
    HemisphereFilePattern {
        left: "-lh_",
        right: "-rh_",
        wildcard: "-?h_",
    },
];

fn paired_overlay_dataset(
    left: Dataset,
    right: Dataset,
    domain: &SurfaceDomain,
    right_node_offset: u32,
) -> Result<Dataset> {
    ensure!(
        left.columns.len() == right.columns.len(),
        "paired overlays have different column counts: {} vs {}",
        left.columns.len(),
        right.columns.len()
    );
    let left_is_dense = !left.is_sparse();
    let right_is_dense = !right.is_sparse();
    let left_row_count = left.row_count;
    let right_row_count = right.row_count;
    let left_node_indices = left.node_indices.clone();
    let right_node_indices = right.node_indices.clone();
    let kind = if left.kind == right.kind {
        left.kind.clone()
    } else {
        DatasetKind::Unknown
    };

    let columns = left
        .columns
        .into_iter()
        .zip(right.columns)
        .map(|(left, right)| paired_data_column(left, right))
        .collect::<Result<Vec<_>>>()?;

    if left_is_dense
        && right_is_dense
        && left_row_count as u32 == right_node_offset
        && left_row_count + right_row_count == domain.node_count
    {
        return Dataset::dense(kind, domain, columns)
            .context("failed to build paired dense overlay dataset");
    }

    let mut node_indices = Vec::with_capacity(left_row_count + right_row_count);
    if let Some(indices) = left_node_indices {
        node_indices.extend(indices);
    } else {
        node_indices.extend(0..left_row_count as u32);
    }
    if let Some(indices) = right_node_indices {
        node_indices.extend(indices.into_iter().map(|node| node + right_node_offset));
    } else {
        node_indices.extend((0..right_row_count as u32).map(|node| node + right_node_offset));
    }

    Dataset::sparse(kind, domain, node_indices, columns)
        .context("failed to build paired overlay dataset")
}

fn paired_data_column(left: DataColumn, right: DataColumn) -> Result<DataColumn> {
    let right_label = right.label;
    let right_role = right.role;
    let right_units = right.units;
    let right_stat = right.stat;
    ensure!(
        std::mem::discriminant(&left.values) == std::mem::discriminant(&right.values),
        "paired overlay column {} and {} have different data types",
        left.label,
        right_label
    );
    let stat = if left.stat == right_stat {
        left.stat.clone()
    } else {
        None
    };
    let units = if left.units == right_units {
        left.units.clone()
    } else {
        None
    };
    let role = if left.role == right_role {
        left.role.clone()
    } else {
        ColumnRole::Unknown
    };

    Ok(DataColumn::new(
        left.label,
        role,
        units,
        paired_column_data(left.values, right.values)?,
    )?
    .with_stat(stat))
}

fn paired_column_data(left: ColumnData, right: ColumnData) -> Result<ColumnData> {
    match (left, right) {
        (ColumnData::UInt32(mut left), ColumnData::UInt32(right)) => {
            left.extend(right);
            Ok(ColumnData::UInt32(left))
        }
        (ColumnData::Int32(mut left), ColumnData::Int32(right)) => {
            left.extend(right);
            Ok(ColumnData::Int32(left))
        }
        (ColumnData::Float32(mut left), ColumnData::Float32(right)) => {
            left.extend(right);
            Ok(ColumnData::Float32(left))
        }
        (ColumnData::Float64(mut left), ColumnData::Float64(right)) => {
            left.extend(right);
            Ok(ColumnData::Float64(left))
        }
        (ColumnData::Text(mut left), ColumnData::Text(right)) => {
            left.extend(right);
            Ok(ColumnData::Text(left))
        }
        _ => bail!("paired overlay columns have different data types"),
    }
}

fn load_dataset_from_path(path: &Path, mesh: &SurfaceMesh) -> Result<Dataset> {
    if is_niml_dset_path(path) {
        read_niml_dataset(path, &mesh.domain)
    } else if is_gifti_path(path) {
        read_gifti_dataset(path, &mesh.domain).or_else(|dataset_error| {
            let overlay_values =
                OverlayDataset::from_gifti_path(path, mesh.vertices.len()).with_context(|| {
                    format!(
                        "failed to load GIFTI as canonical dataset ({dataset_error:#}) or simple overlay"
                    )
                })?;
            dataset_from_simple_overlay(&mesh.domain, overlay_values.values)
        })
    } else {
        let overlay_values = OverlayDataset::from_gifti_path(path, mesh.vertices.len())?;
        dataset_from_simple_overlay(&mesh.domain, overlay_values.values)
    }
}

fn load_overlay_from_path(path: &Path, mesh: &SurfaceMesh) -> Result<LoadedOverlay> {
    let dataset = load_dataset_from_path(path, mesh)?;
    loaded_overlay_from_dataset(dataset, mesh.vertices.len(), "overlay")
}

fn loaded_overlay_from_dataset(
    dataset: Dataset,
    node_count: usize,
    source_label: &str,
) -> Result<LoadedOverlay> {
    let columns = default_overlay_columns(&dataset).with_context(|| {
        format!("{source_label} dataset has no numeric column that can be displayed as an overlay")
    })?;
    let overlay = overlay_dataset_from_canonical_dataset(&dataset, node_count, columns)?;

    Ok(LoadedOverlay {
        overlay_values: overlay,
        dataset,
        columns,
    })
}

fn roi_appearance_for_mesh(
    rois: &[Roi],
    mesh: &SurfaceMesh,
    ranges: &[RoiComponentRange],
) -> Result<RoiAppearanceBuild> {
    let mut appearance = RoiAppearance::empty(mesh.vertices.len());
    let mut node_labels = vec![Vec::new(); mesh.vertices.len()];
    let mut mapped = BTreeSet::new();
    let mut skipped_nodes = 0usize;

    for roi in rois {
        for datum in roi.data.iter().filter(|datum| !roi_datum_is_stroke(datum)) {
            skipped_nodes += paint_roi_datum(
                roi,
                datum,
                mesh,
                ranges,
                roi.fill_color.to_array(),
                &mut appearance,
                &mut node_labels,
                &mut mapped,
            );
        }
    }
    for roi in rois {
        for datum in roi.data.iter().filter(|datum| roi_datum_is_stroke(datum)) {
            skipped_nodes += paint_roi_datum(
                roi,
                datum,
                mesh,
                ranges,
                roi.edge_color.to_array(),
                &mut appearance,
                &mut node_labels,
                &mut mapped,
            );
        }
    }

    ensure!(
        !mapped.is_empty(),
        "ROI nodes did not overlap the active surface"
    );

    Ok(RoiAppearanceBuild {
        appearance,
        node_labels,
        mapped_nodes: mapped.len(),
        skipped_nodes,
    })
}

fn paint_roi_datum(
    roi: &Roi,
    datum: &RoiDatum,
    mesh: &SurfaceMesh,
    ranges: &[RoiComponentRange],
    color: [f32; 4],
    appearance: &mut RoiAppearance,
    node_labels: &mut [Vec<String>],
    mapped: &mut BTreeSet<u32>,
) -> usize {
    let mut skipped = 0usize;
    let label = roi_display_label(roi);

    for node in roi_datum_nodes(roi, datum, mesh, ranges) {
        match node {
            Some(node) if appearance.set_node_color(node, color) => {
                mapped.insert(node);
                if let Some(labels) = node_labels.get_mut(node as usize)
                    && !labels.contains(&label)
                {
                    labels.push(label.clone());
                }
            }
            _ => skipped += 1,
        }
    }

    skipped
}

fn roi_datum_nodes(
    roi: &Roi,
    datum: &RoiDatum,
    mesh: &SurfaceMesh,
    ranges: &[RoiComponentRange],
) -> Vec<Option<u32>> {
    if !datum.node_path.is_empty() {
        return datum
            .node_path
            .iter()
            .map(|node| roi_node_to_mesh_node(roi, *node, mesh.vertices.len(), ranges))
            .collect();
    }

    datum
        .triangle_path
        .iter()
        .flat_map(|face| {
            let Some(mesh_face) = roi_face_to_mesh_face(roi, *face, mesh.triangles.len(), ranges)
            else {
                return vec![None];
            };
            mesh.triangles
                .get(mesh_face)
                .map(|triangle| triangle.iter().copied().map(Some).collect())
                .unwrap_or_else(|| vec![None])
        })
        .collect()
}

fn roi_node_to_mesh_node(
    roi: &Roi,
    node: u32,
    mesh_node_count: usize,
    ranges: &[RoiComponentRange],
) -> Option<u32> {
    if let Some(range) = roi_component_range_for_side(&roi.parent_side, ranges) {
        return ((node as usize) < range.node_count).then_some(range.node_offset + node);
    }

    ((node as usize) < mesh_node_count).then_some(node)
}

fn roi_face_to_mesh_face(
    roi: &Roi,
    face: u32,
    mesh_face_count: usize,
    ranges: &[RoiComponentRange],
) -> Option<usize> {
    if let Some(range) = roi_component_range_for_side(&roi.parent_side, ranges) {
        let face = face as usize;
        return (face < range.triangle_count).then_some(range.triangle_offset + face);
    }

    let face = face as usize;
    (face < mesh_face_count).then_some(face)
}

fn roi_component_range_for_side<'a>(
    side: &SurfaceSide,
    ranges: &'a [RoiComponentRange],
) -> Option<&'a RoiComponentRange> {
    if ranges.len() == 1 {
        return ranges.first();
    }

    match side {
        SurfaceSide::Left | SurfaceSide::Right => ranges.iter().find(|range| range.side == *side),
        _ => None,
    }
}

fn roi_datum_is_stroke(datum: &RoiDatum) -> bool {
    matches!(
        datum.kind,
        RoiElementKind::NodeSegment | RoiElementKind::EdgeGroup
    ) || matches!(
        datum.action,
        RoiBrushAction::AppendStroke
            | RoiBrushAction::AppendStrokeOrFill
            | RoiBrushAction::JoinEnds
    )
}

fn roi_display_label(roi: &Roi) -> String {
    format!("{} ({})", roi.label, roi.integer_label)
}

fn dataset_from_simple_overlay(domain: &SurfaceDomain, values: Vec<f32>) -> Result<Dataset> {
    Dataset::dense(
        DatasetKind::SurfaceScalar,
        domain,
        vec![
            DataColumn::new(
                "scalar",
                ColumnRole::Intensity,
                None,
                ColumnData::Float32(values),
            )
            .context("failed to build scalar overlay column")?,
        ],
    )
    .context("failed to wrap scalar overlay as canonical dataset")
}

fn canonical_overlay_columns(
    selections: OverlayColumnSelections,
    threshold_enabled: bool,
) -> OverlayColumns {
    let threshold = selections
        .threshold
        .or_else(|| threshold_enabled.then_some(selections.intensity));
    let mut columns = OverlayColumns::new(selections.intensity);
    if let Some(index) = threshold {
        columns.threshold = Some(ColumnSelection::new(index));
    }
    if let Some(index) = selections.brightness {
        columns.brightness = Some(ColumnSelection::new(index));
    }

    columns
}

fn threshold_and_mask_from_appearance(appearance: OverlayAppearance) -> (Threshold, MaskMode) {
    if !appearance.threshold.enabled {
        return (Threshold::off(), MaskMode::None);
    }

    let value = appearance.threshold.value as f64;
    let threshold = if appearance.threshold.absolute {
        let extent = value.abs();
        Threshold::outside(-extent, extent)
    } else {
        Threshold::above(value)
    };
    let mask_mode = if appearance.threshold.hide_failed {
        MaskMode::HideFailedThreshold
    } else {
        MaskMode::DimFailedThreshold(0.25)
    };

    (threshold, mask_mode)
}

fn overlay_range_from_value_range(range: ValueRange) -> OverlayRange {
    OverlayRange {
        min: range.min as f64,
        max: range.max as f64,
    }
}

fn overlay_dataset_from_canonical_dataset(
    dataset: &Dataset,
    node_count: usize,
    columns: OverlayColumnSelections,
) -> Result<OverlayDataset> {
    let intensity_column = dataset
        .columns
        .get(columns.intensity)
        .filter(|column| column_is_numeric(column))
        .context("selected intensity column is not numeric")?;
    let threshold_column = columns
        .threshold
        .and_then(|index| dataset.columns.get(index))
        .filter(|column| column_is_numeric(column));
    let brightness_column = columns
        .brightness
        .and_then(|index| dataset.columns.get(index))
        .filter(|column| column_is_numeric(column));
    let threshold_stat =
        threshold_column.and_then(|column| column.stat.as_deref().and_then(AfniStatSpec::parse));
    let mut values = vec![f32::NAN; node_count];
    let mut threshold_values = threshold_column.map(|_| vec![f32::NAN; node_count]);
    let mut threshold_pvalues = threshold_stat.as_ref().map(|_| vec![f32::NAN; node_count]);
    let mut brightness_values = brightness_column.map(|_| vec![f32::NAN; node_count]);

    for row in 0..dataset.row_count {
        let Some(node) = dataset.node_for_row(row) else {
            continue;
        };
        let node = node as usize;
        if let (Some(value), Some(slot)) = (
            numeric_column_value_as_f32(intensity_column, row),
            values.get_mut(node),
        ) {
            *slot = value;
        }
        if let (Some(column), Some(slots)) = (threshold_column, threshold_values.as_mut()) {
            if let (Some(value), Some(slot)) = (
                numeric_column_value_as_f32(column, row),
                slots.get_mut(node),
            ) {
                *slot = value;
                if let (Some(stat), Some(pvalue_slots)) =
                    (threshold_stat.as_ref(), threshold_pvalues.as_mut())
                {
                    if let (Some(pvalue), Some(pvalue_slot)) = (
                        stat.two_sided_p_value(value as f64),
                        pvalue_slots.get_mut(node),
                    ) {
                        *pvalue_slot = pvalue as f32;
                    }
                }
            }
        }
        if let (Some(column), Some(slots)) = (brightness_column, brightness_values.as_mut()) {
            if let (Some(value), Some(slot)) = (
                numeric_column_value_as_f32(column, row),
                slots.get_mut(node),
            ) {
                *slot = value;
            }
        }
    }

    let range = ValueRange::from_values(&values)?;
    let brightness_range = brightness_values
        .as_ref()
        .map(|values| ValueRange::from_values(values))
        .transpose()?;
    Ok(OverlayDataset {
        values,
        range,
        threshold_values,
        threshold_pvalues,
        brightness_values,
        brightness_range,
    })
}

fn default_overlay_columns(dataset: &Dataset) -> Option<OverlayColumnSelections> {
    let intensity = preferred_overlay_column(dataset)?;
    let threshold = preferred_threshold_column(dataset, intensity);
    Some(OverlayColumnSelections {
        intensity,
        threshold,
        brightness: None,
    })
}

fn resolve_overlay_subs(dataset: &Dataset, specs: &[String]) -> Result<OverlayColumnSelections> {
    ensure!(
        (2..=3).contains(&specs.len()),
        "--subs expects I,T or I,T,B"
    );

    Ok(OverlayColumnSelections {
        intensity: resolve_required_overlay_column(dataset, &specs[0], "I")?,
        threshold: resolve_optional_overlay_column(dataset, &specs[1], "T")?,
        brightness: specs
            .get(2)
            .map(|spec| resolve_optional_overlay_column(dataset, spec, "B"))
            .transpose()?
            .flatten(),
    })
}

fn resolve_required_overlay_column(dataset: &Dataset, spec: &str, role: &str) -> Result<usize> {
    ensure!(
        !spec.trim().eq_ignore_ascii_case("none"),
        "{role} sub-brick cannot be none"
    );
    let index = resolve_overlay_column(dataset, spec)
        .with_context(|| format!("failed to resolve {role} sub-brick '{spec}'"))?;
    ensure!(
        dataset.columns.get(index).is_some_and(column_is_numeric),
        "{role} sub-brick '{spec}' resolved to non-numeric column #{index}"
    );

    Ok(index)
}

fn resolve_optional_overlay_column(
    dataset: &Dataset,
    spec: &str,
    role: &str,
) -> Result<Option<usize>> {
    if spec.trim().eq_ignore_ascii_case("none") {
        return Ok(None);
    }

    resolve_required_overlay_column(dataset, spec, role).map(Some)
}

fn resolve_overlay_column(dataset: &Dataset, spec: &str) -> Result<usize> {
    let spec = spec.trim();
    ensure!(!spec.is_empty(), "empty sub-brick selector");

    if let Some(index) = parse_column_index_selector(spec) {
        ensure!(
            index < dataset.columns.len(),
            "sub-brick index #{index} is outside dataset column count {}",
            dataset.columns.len()
        );
        return Ok(index);
    }

    let needle = normalized_column_selector(spec);
    let exact = matching_overlay_columns(dataset, |candidate| {
        normalized_column_selector(candidate) == needle
    });
    if let Some(index) = unique_overlay_column_match(spec, exact)? {
        return Ok(index);
    }

    let partial = matching_overlay_columns(dataset, |candidate| {
        normalized_column_selector(candidate).contains(&needle)
    });
    unique_overlay_column_match(spec, partial)?
        .with_context(|| format!("no dataset column matched '{spec}'"))
}

fn parse_column_index_selector(spec: &str) -> Option<usize> {
    spec.trim()
        .strip_prefix('#')
        .unwrap_or(spec.trim())
        .parse()
        .ok()
}

fn matching_overlay_columns(dataset: &Dataset, matches: impl Fn(&str) -> bool) -> Vec<usize> {
    let mut matched = Vec::new();
    for (index, column) in dataset.columns.iter().enumerate() {
        if overlay_column_match_labels(index, column)
            .iter()
            .any(|candidate| matches(candidate))
            && !matched.contains(&index)
        {
            matched.push(index);
        }
    }

    matched
}

fn unique_overlay_column_match(spec: &str, matches: Vec<usize>) -> Result<Option<usize>> {
    match matches.as_slice() {
        [] => Ok(None),
        [index] => Ok(Some(*index)),
        _ => bail!(
            "sub-brick selector '{}' is ambiguous; matched columns {:?}",
            spec,
            matches
        ),
    }
}

fn overlay_column_match_labels(index: usize, column: &DataColumn) -> Vec<String> {
    let mut labels = vec![
        index.to_string(),
        format!("#{index}"),
        column.label.clone(),
        format!("#{index} {}", column.label),
    ];
    if let Some(stat) = column.stat.as_ref() {
        labels.push(format!("{} [{stat}]", column.label));
        labels.push(format!("#{index} {} [{stat}]", column.label));
    }

    labels
}

fn normalized_column_selector(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn preferred_overlay_column(dataset: &Dataset) -> Option<usize> {
    dataset
        .columns
        .iter()
        .enumerate()
        .filter(|(_, column)| column_is_numeric(column))
        .min_by_key(|(_, column)| overlay_column_priority(&column.role))
        .map(|(index, _)| index)
}

fn preferred_threshold_column(dataset: &Dataset, intensity: usize) -> Option<usize> {
    let next = intensity + 1;
    if dataset
        .columns
        .get(next)
        .is_some_and(|column| column_is_numeric(column) && column.stat.is_some())
    {
        return Some(next);
    }

    dataset
        .columns
        .iter()
        .enumerate()
        .find(|(_, column)| column_is_numeric(column) && column.stat.is_some())
        .map(|(index, _)| index)
}

fn overlay_column_priority(role: &ColumnRole) -> u8 {
    match role {
        ColumnRole::Intensity => 0,
        ColumnRole::Statistic => 1,
        ColumnRole::TimePoint => 2,
        ColumnRole::Brightness => 3,
        ColumnRole::Label => 4,
        ColumnRole::Threshold => 5,
        ColumnRole::Mask => 6,
        ColumnRole::NodeIndex => 7,
        ColumnRole::Unknown | ColumnRole::Other(_) => 8,
    }
}

fn column_is_numeric(column: &DataColumn) -> bool {
    !matches!(column.values, ColumnData::Text(_))
}

fn numeric_column_value_as_f32(column: &DataColumn, row: usize) -> Option<f32> {
    let value = match &column.values {
        ColumnData::UInt32(values) => *values.get(row)? as f32,
        ColumnData::Int32(values) => *values.get(row)? as f32,
        ColumnData::Float32(values) => *values.get(row)?,
        ColumnData::Float64(values) => *values.get(row)? as f32,
        ColumnData::Text(_) => return None,
    };

    value.is_finite().then_some(value)
}

fn overlay_column_summary(dataset: &Dataset, columns: OverlayColumnSelections) -> String {
    format!(
        "I {}, T {}, B {}.",
        column_selection_label(dataset, Some(columns.intensity)),
        column_selection_label(dataset, columns.threshold),
        column_selection_label(dataset, columns.brightness)
    )
}

fn column_selection_label(dataset: &Dataset, selection: Option<usize>) -> String {
    selection
        .and_then(|index| dataset.columns.get(index).map(|column| (index, column)))
        .map_or_else(
            || "none".to_string(),
            |(index, column)| {
                column.stat.as_ref().map_or_else(
                    || format!("#{index} {}", column.label),
                    |stat| format!("#{index} {} [{stat}]", column.label),
                )
            },
        )
}

fn is_niml_dset_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".niml.dset"))
}

fn is_gifti_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            name.ends_with(".gii")
                || name.ends_with(".gii.gz")
                || name.ends_with(".gii.dset")
                || name.ends_with(".gii.dset.gz")
        })
}

fn dialog_with_start_directory(
    dialog: rfd::FileDialog,
    current_path: Option<&PathBuf>,
) -> rfd::FileDialog {
    if let Some(directory) = dialog_start_directory(current_path) {
        dialog.set_directory(directory)
    } else {
        dialog
    }
}

fn dialog_start_directory(current_path: Option<&PathBuf>) -> Option<PathBuf> {
    current_path
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
}

fn controller_section(
    ui: &mut egui::Ui,
    title: &str,
    default_open: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    ui.add_space(8.0);
    egui::CollapsingHeader::new(
        egui::RichText::new(title)
            .size(11.0)
            .strong()
            .color(egui::Color32::from_rgb(123, 184, 226)),
    )
    .id_salt(("controller_section", title))
    .default_open(default_open)
    .show_unindented(ui, |ui| {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(28, 32, 39))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 62, 74)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, add_contents);
    });
}

fn draw_intensity_column_selector(
    ui: &mut egui::Ui,
    options: &[OverlayColumnOption],
    selection: &mut usize,
) -> bool {
    ui.label("I");
    let mut changed = false;
    egui::ComboBox::from_id_salt("intensity_column")
        .selected_text(column_option_label(options, Some(*selection)))
        .width(OVERLAY_SELECTOR_WIDTH_POINTS)
        .show_ui(ui, |ui| {
            for option in options {
                changed |= ui
                    .selectable_value(selection, option.index, option.label.as_str())
                    .changed();
            }
        });
    ui.end_row();
    changed
}

fn draw_optional_column_selector(
    ui: &mut egui::Ui,
    label: &str,
    id: &'static str,
    options: &[OverlayColumnOption],
    selection: &mut Option<usize>,
) -> bool {
    ui.label(label);
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(column_option_label(options, *selection))
        .width(OVERLAY_SELECTOR_WIDTH_POINTS)
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(selection, None, "none").changed();
            for option in options {
                changed |= ui
                    .selectable_value(selection, Some(option.index), option.label.as_str())
                    .changed();
            }
        });
    ui.end_row();
    changed
}

fn draw_threshold_column_selector(
    ui: &mut egui::Ui,
    options: &[OverlayColumnOption],
    selection: &mut Option<usize>,
    threshold_value: f32,
) -> bool {
    ui.label("T");
    let mut changed = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("threshold_column")
            .selected_text(column_option_label(options, *selection))
            .width(OVERLAY_SELECTOR_WIDTH_POINTS)
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(selection, None, "none").changed();
                for option in options {
                    changed |= ui
                        .selectable_value(selection, Some(option.index), option.label.as_str())
                        .changed();
                }
            });
        ui.monospace(threshold_value_display(threshold_value));
    });
    ui.end_row();
    changed
}

fn overlay_column_options(dataset: &Dataset) -> Vec<OverlayColumnOption> {
    dataset
        .columns
        .iter()
        .enumerate()
        .filter(|(_, column)| column_is_numeric(column))
        .map(|(index, column)| OverlayColumnOption {
            index,
            label: column.stat.as_ref().map_or_else(
                || format!("#{index} {}", column.label),
                |stat| format!("#{index} {} [{stat}]", column.label),
            ),
            is_numeric: true,
        })
        .collect()
}

fn column_option_label(options: &[OverlayColumnOption], selection: Option<usize>) -> String {
    selection
        .and_then(|index| {
            options
                .iter()
                .find(|option| option.index == index && option.is_numeric)
        })
        .map_or_else(|| "none".to_string(), |option| option.label.clone())
}

fn vertical_threshold_bar(
    ui: &mut egui::Ui,
    appearance: &mut OverlayAppearance,
    threshold_range: ValueRange,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(54.0, OVERLAY_THRESHOLD_BAR_HEIGHT_POINTS),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    let bar_rect = rect.shrink2(egui::vec2(12.0, 4.0));
    let steps = 80;
    let mut changed = false;

    for step in 0..steps {
        let t0 = step as f32 / steps as f32;
        let t1 = (step + 1) as f32 / steps as f32;
        let y0 = bar_rect.bottom() - bar_rect.height() * t0;
        let y1 = bar_rect.bottom() - bar_rect.height() * t1;
        let color = color32_from_rgba(sample_colormap(appearance.colormap, (t0 + t1) * 0.5));
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(bar_rect.left(), y1),
                egui::pos2(bar_rect.right(), y0),
            ),
            0,
            color,
        );
    }

    painter.rect_stroke(
        bar_rect,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, egui::Color32::from_rgb(95, 104, 121)),
        egui::StrokeKind::Outside,
    );

    if response.clicked() || response.dragged() {
        if let Some(position) = response.interact_pointer_pos() {
            let (min, max) = threshold_bounds(threshold_range, appearance.threshold.absolute);
            appearance.threshold.value = threshold_value_from_bar_y(bar_rect, min, max, position.y);
            appearance.threshold.enabled = true;
            changed = true;
        }
    }

    let (min, max) = threshold_bounds(threshold_range, appearance.threshold.absolute);
    let value = appearance.threshold.value.clamp(min, max);
    let y = threshold_bar_y_for_value(bar_rect, min, max, value);
    let marker_color = if appearance.threshold.enabled {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_gray(145)
    };
    painter.line_segment(
        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(2.0, marker_color),
    );
    painter.circle_filled(egui::pos2(rect.center().x, y), 3.5, egui::Color32::BLACK);
    painter.circle_stroke(
        egui::pos2(rect.center().x, y),
        3.5,
        egui::Stroke::new(1.0, marker_color),
    );

    changed
}

fn file_display(path: Option<&PathBuf>) -> String {
    path.map_or_else(|| "none".to_string(), |path| file_name_display(path))
}

fn file_name_display(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "none".to_string(), ToString::to_string)
}

fn scene_surface_display_label(index: usize, total: usize, surface: &SceneSurface) -> String {
    format!(
        "{}/{} {}{}",
        index + 1,
        total,
        surface.name,
        surface
            .state
            .as_ref()
            .map_or_else(String::new, |state| format!(" ({state})"))
    )
}

fn surface_side_label(side: &SurfaceSide) -> &str {
    match side {
        SurfaceSide::Left => "left",
        SurfaceSide::Right => "right",
        SurfaceSide::Both => "both",
        SurfaceSide::Unknown => "unknown",
        SurfaceSide::Other(value) => value.as_str(),
    }
}

fn muted_color() -> egui::Color32 {
    egui::Color32::from_rgb(151, 160, 174)
}

fn color32_from_rgba(color: [f32; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        float_color_channel(color[0]),
        float_color_channel(color[1]),
        float_color_channel(color[2]),
        float_color_channel(color[3]),
    )
}

fn float_color_channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn range_drag_speed(range: ValueRange) -> f32 {
    ((range.max - range.min).abs() / 200.0).max(0.001)
}

fn symmetric_value_range(range: ValueRange) -> ValueRange {
    if range.min < 0.0 && range.max > 0.0 {
        let extent = range.min.abs().max(range.max.abs());
        ValueRange {
            min: -extent,
            max: extent,
        }
    } else {
        range
    }
}

fn threshold_bounds(range: ValueRange, absolute: bool) -> (f32, f32) {
    if absolute {
        let extent = range.min.abs().max(range.max.abs()).max(0.0001);
        (0.0, extent)
    } else {
        ordered_range(range)
    }
}

fn threshold_value_display(value: f32) -> String {
    format!("{value:.4}")
}

fn threshold_p_value_display(pvalue: Option<f64>) -> String {
    match pvalue {
        Some(value) if value < 0.001 => format!("p <= {value:.2e}"),
        Some(value) => format!("p <= {value:.4}"),
        None => "p --".to_string(),
    }
}

fn threshold_bar_y_for_value(rect: egui::Rect, min: f32, max: f32, value: f32) -> f32 {
    let span = (max - min).abs().max(f32::EPSILON);
    let t = ((value - min) / span).clamp(0.0, 1.0);
    rect.bottom() - rect.height() * t
}

fn threshold_value_from_bar_y(rect: egui::Rect, min: f32, max: f32, y: f32) -> f32 {
    let t = ((rect.bottom() - y) / rect.height().max(f32::EPSILON)).clamp(0.0, 1.0);
    min + (max - min) * t
}

fn ordered_range(range: ValueRange) -> (f32, f32) {
    if range.min <= range.max {
        (range.min, range.max)
    } else {
        (range.max, range.min)
    }
}

fn stat_row(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    ui.label(label);
    ui.monospace(value.into());
    ui.end_row();
}

fn normal_direction_label(direction: NormalDirection) -> &'static str {
    match direction {
        NormalDirection::Outward => "outward",
        NormalDirection::Inward => "inward",
        NormalDirection::Mixed => "mixed",
        NormalDirection::Unknown => "unknown",
    }
}

fn overlay_value_label(value: Option<f32>) -> String {
    value.map_or_else(|| "not loaded".to_string(), |value| format!("{value:.4}"))
}

fn picked_overlay_value_label(pick: SurfacePick) -> String {
    format!(
        "I {}; T {}",
        overlay_value_label(pick.overlay_value),
        overlay_value_label(pick.threshold_value)
    )
}

fn coordinate_label(position: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", position[0], position[1], position[2])
}

fn size_is_close(current: PhysicalSize<u32>, desired: PhysicalSize<u32>) -> bool {
    current.width.abs_diff(desired.width) <= CONTROL_RESIZE_THRESHOLD
        && current.height.abs_diff(desired.height) <= CONTROL_RESIZE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::{
        BackgroundMode, HemisphereLayout, HemisphereLayoutState, MontageCamera, OverlayAppearance,
        OverlayColumnSelections, PAIR_MAX_DRAG_GAP_FACTOR, PAIR_MAX_OPEN_DEGREES,
        PAIR_OPEN_DEGREES_PER_PIXEL, PairVisibility, PresetOrientation, RoiComponentRange,
        SceneSurface, SceneSurfaceComponent, canonical_overlay_columns, paired_component_for_node,
        paired_overlay_dataset, paired_overlay_path_for_side, paired_overlay_paths,
        paired_preview_geometry, paired_spec_montage_shots, resolve_overlay_subs,
        roi_appearance_for_mesh, scene_surface_display_label, scene_surfaces_from_components,
        standard_montage_shots, threshold_and_mask_from_appearance,
        timestamped_png_name_from_unix_seconds,
    };
    use crate::color::Rgba;
    use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind};
    use crate::overlay::{MaskMode, Threshold};
    use crate::roi::Roi;
    use crate::spec::{SpecFile, SpecHemisphere, SpecSurface};
    use crate::surface::{SurfaceDomain, SurfaceMesh, SurfaceSide, ValueRange};
    use glam::Vec3;
    use std::path::PathBuf;

    #[test]
    fn background_toggles_between_black_and_white() {
        let mut background = BackgroundMode::Black;

        background.toggle();
        assert_eq!(background, BackgroundMode::White);

        background.toggle();
        assert_eq!(background, BackgroundMode::Black);
    }

    #[test]
    fn close_control_sizes_do_not_trigger_resize_churn() {
        assert!(super::size_is_close(
            winit::dpi::PhysicalSize::new(420, 700),
            winit::dpi::PhysicalSize::new(428, 710)
        ));
        assert!(!super::size_is_close(
            winit::dpi::PhysicalSize::new(420, 700),
            winit::dpi::PhysicalSize::new(460, 700)
        ));
    }

    #[test]
    fn screenshot_default_names_use_readable_timestamps() {
        assert_eq!(
            timestamped_png_name_from_unix_seconds("sumaru_view", 0),
            "sumaru_view_1970-01-01_000000.png"
        );
        assert_eq!(
            timestamped_png_name_from_unix_seconds("sumaru_montage", 1_672_531_199),
            "sumaru_montage_2022-12-31_235959.png"
        );
        assert_eq!(
            timestamped_png_name_from_unix_seconds("sumaru_view", 1_709_251_200),
            "sumaru_view_2024-03-01_000000.png"
        );
    }

    #[test]
    fn paired_spec_montage_uses_closed_top_bottom_then_open_front_back() {
        let standard = standard_montage_shots();
        assert_eq!(
            standard.iter().map(|shot| shot.camera).collect::<Vec<_>>(),
            vec![
                MontageCamera::Preset(PresetOrientation::Left),
                MontageCamera::Preset(PresetOrientation::Right),
                MontageCamera::Preset(PresetOrientation::Top),
                MontageCamera::Preset(PresetOrientation::Bottom),
            ]
        );
        assert!(standard.iter().all(|shot| shot.layout.is_none()));

        let paired = paired_spec_montage_shots();
        assert_eq!(paired[0].layout.unwrap().layout, HemisphereLayout::Closed);
        assert_eq!(
            paired[0].layout.unwrap().state,
            HemisphereLayoutState::closed()
        );
        assert_eq!(
            paired[0].camera,
            MontageCamera::Preset(PresetOrientation::Top)
        );
        assert_eq!(paired[1].layout.unwrap().layout, HemisphereLayout::Closed);
        assert_eq!(
            paired[1].camera,
            MontageCamera::Preset(PresetOrientation::Bottom)
        );
        assert_eq!(paired[2].layout.unwrap().layout, HemisphereLayout::Open);
        assert_eq!(
            paired[2].layout.unwrap().state,
            HemisphereLayoutState::acorn()
        );
        assert_eq!(
            paired[2].camera,
            MontageCamera::Direction {
                eye_direction: Vec3::Y,
                up: Vec3::Z,
            }
        );
        assert_eq!(paired[3].layout.unwrap().layout, HemisphereLayout::Open);
        assert_eq!(
            paired[3].camera,
            MontageCamera::Direction {
                eye_direction: Vec3::NEG_Y,
                up: Vec3::Z,
            }
        );
    }

    #[test]
    fn canonical_overlay_columns_use_intensity_as_threshold_fallback() {
        let selections = OverlayColumnSelections {
            intensity: 2,
            threshold: None,
            brightness: Some(4),
        };

        let columns = canonical_overlay_columns(selections, true);
        assert_eq!(columns.intensity.index, 2);
        assert_eq!(columns.threshold.unwrap().index, 2);
        assert_eq!(columns.brightness.unwrap().index, 4);

        let columns = canonical_overlay_columns(selections, false);
        assert!(columns.threshold.is_none());
    }

    #[test]
    fn roi_appearance_offsets_right_hemisphere_nodes_in_paired_mesh() {
        let mesh = SurfaceMesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [10.0, 0.0, 0.0],
                [11.0, 0.0, 0.0],
                [10.0, 1.0, 0.0],
            ],
            vec![[0, 1, 2], [3, 4, 5]],
        )
        .unwrap();
        let ranges = vec![
            RoiComponentRange {
                side: SurfaceSide::Left,
                node_offset: 0,
                node_count: 3,
                triangle_offset: 0,
                triangle_count: 1,
            },
            RoiComponentRange {
                side: SurfaceSide::Right,
                node_offset: 3,
                node_count: 3,
                triangle_offset: 1,
                triangle_count: 1,
            },
        ];
        let mut left = Roi::from_nodes("left-roi", 1, vec![1]).unwrap();
        left.parent_side = SurfaceSide::Left;
        left = left
            .with_style(
                Rgba::from_u8(255, 0, 0, 255),
                Rgba::from_u8(0, 0, 255, 255),
                1,
            )
            .unwrap();
        let mut right = Roi::from_nodes("right-roi", 2, vec![1]).unwrap();
        right.parent_side = SurfaceSide::Right;
        right = right
            .with_style(
                Rgba::from_u8(0, 255, 0, 255),
                Rgba::from_u8(0, 0, 255, 255),
                1,
            )
            .unwrap();

        let build = roi_appearance_for_mesh(&[left, right], &mesh, &ranges).unwrap();

        assert_eq!(build.mapped_nodes, 2);
        assert!(build.appearance.node_colors[1].is_some());
        assert!(build.appearance.node_colors[4].is_some());
        assert_eq!(build.node_labels[1], vec!["left-roi (1)"]);
        assert_eq!(build.node_labels[4], vec!["right-roi (2)"]);
    }

    #[test]
    fn overlay_subs_resolve_numeric_and_label_selectors() {
        let domain = SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![
                DataColumn::new(
                    "Grp_HV",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![0.0, 1.0, 2.0]),
                )
                .unwrap(),
                DataColumn::new(
                    "Grp_HV t",
                    ColumnRole::Statistic,
                    None,
                    ColumnData::Float32(vec![2.0, 3.0, 4.0]),
                )
                .unwrap()
                .with_stat(Some("Ttest(48)".to_string())),
                DataColumn::new(
                    "Grp_HV beta",
                    ColumnRole::Brightness,
                    None,
                    ColumnData::Float32(vec![0.5, 0.6, 0.7]),
                )
                .unwrap(),
            ],
        )
        .unwrap();

        let numeric = resolve_overlay_subs(&dataset, &["0".into(), "1".into()]).unwrap();
        assert_eq!(
            numeric,
            OverlayColumnSelections {
                intensity: 0,
                threshold: Some(1),
                brightness: None
            }
        );

        let labels = resolve_overlay_subs(
            &dataset,
            &["Grp_HV".into(), "Grp_HV t".into(), "beta".into()],
        )
        .unwrap();
        assert_eq!(
            labels,
            OverlayColumnSelections {
                intensity: 0,
                threshold: Some(1),
                brightness: Some(2)
            }
        );
    }

    #[test]
    fn overlay_subs_reject_ambiguous_or_non_numeric_selectors() {
        let domain = SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap();
        let dataset = Dataset::dense(
            DatasetKind::SurfaceScalar,
            &domain,
            vec![
                DataColumn::new(
                    "Grp",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(vec![0.0, 1.0, 2.0]),
                )
                .unwrap(),
                DataColumn::new(
                    "Grp t",
                    ColumnRole::Statistic,
                    None,
                    ColumnData::Float32(vec![2.0, 3.0, 4.0]),
                )
                .unwrap(),
                DataColumn::new(
                    "Grp label",
                    ColumnRole::Label,
                    None,
                    ColumnData::Text(vec!["a".into(), "b".into(), "c".into()]),
                )
                .unwrap(),
            ],
        )
        .unwrap();

        assert!(resolve_overlay_subs(&dataset, &["Gr".into(), "Grp t".into()]).is_err());
        assert!(resolve_overlay_subs(&dataset, &["Grp label".into(), "Grp t".into()]).is_err());
    }

    #[test]
    fn viewer_threshold_slider_maps_to_canonical_threshold() {
        let mut appearance = OverlayAppearance::from_range(ValueRange {
            min: -5.0,
            max: 5.0,
        });
        appearance.threshold.enabled = true;
        appearance.threshold.absolute = true;
        appearance.threshold.value = 2.0;

        let (threshold, mask_mode) = threshold_and_mask_from_appearance(appearance);
        assert_eq!(threshold, Threshold::outside(-2.0, 2.0));
        assert_eq!(mask_mode, MaskMode::HideFailedThreshold);

        appearance.threshold.absolute = false;
        let (threshold, _) = threshold_and_mask_from_appearance(appearance);
        assert_eq!(threshold, Threshold::above(2.0));
    }

    #[test]
    fn both_spec_components_are_paired_by_normalized_state() {
        let spec = both_spec(["std.smoothwm", "std.pial"]);
        let (surfaces, skipped_states, messages) = scene_surfaces_from_components(
            &spec,
            vec![
                component("std.smoothwm", SurfaceSide::Left, 0.0),
                component("std.smoothwm", SurfaceSide::Right, 0.0),
                component("std.pial", SurfaceSide::Left, 0.0),
            ],
        );

        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].state.as_deref(), Some("std.smoothwm"));
        assert_eq!(surfaces[0].components.len(), 2);
        assert_eq!(skipped_states, 1);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("missing right hemisphere"))
        );
    }

    #[test]
    fn both_spec_state_selection_follows_spec_order_and_skips_incomplete_pairs() {
        let spec = both_spec(["std.inflated", "std.smoothwm", "std.pial"]);
        let (surfaces, skipped_states, messages) = scene_surfaces_from_components(
            &spec,
            vec![
                component("std.smoothwm", SurfaceSide::Right, 0.0),
                component("std.pial", SurfaceSide::Left, 0.0),
                component("std.inflated", SurfaceSide::Left, 0.0),
                component("std.smoothwm", SurfaceSide::Left, 0.0),
                component("std.inflated", SurfaceSide::Right, 0.0),
            ],
        );

        assert_eq!(
            surfaces
                .iter()
                .map(|surface| surface.state.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("std.inflated"), Some("std.smoothwm")]
        );
        assert_eq!(skipped_states, 1);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("missing right hemisphere"))
        );
    }

    #[test]
    fn scene_surface_dropdown_labels_include_index_name_and_state() {
        let surface = SceneSurface::paired(
            "std.inflated".to_string(),
            PathBuf::from("both.spec"),
            component("std.inflated", SurfaceSide::Left, 0.0),
            component("std.inflated", SurfaceSide::Right, 0.0),
        );

        assert_eq!(
            scene_surface_display_label(1, 4, &surface),
            "2/4 std.inflated (std.inflated)"
        );
    }

    #[test]
    fn single_hemi_spec_components_remain_independent_surfaces() {
        let spec = SpecFile {
            path: PathBuf::from("lh.spec"),
            group: None,
            states: vec!["smoothwm".to_string()],
            hemisphere: SpecHemisphere::Left,
            surfaces: Vec::new(),
        };
        let (surfaces, skipped_states, messages) = scene_surfaces_from_components(
            &spec,
            vec![
                component("smoothwm", SurfaceSide::Left, 0.0),
                component("pial", SurfaceSide::Left, 2.0),
            ],
        );

        assert_eq!(surfaces.len(), 2);
        assert_eq!(skipped_states, 0);
        assert!(messages.is_empty());
    }

    #[test]
    fn open_layout_spreads_and_rotates_paired_hemispheres() {
        let mut surface = SceneSurface::paired(
            "smoothwm".to_string(),
            PathBuf::from("both.spec"),
            component("smoothwm", SurfaceSide::Left, 0.0),
            component("smoothwm", SurfaceSide::Right, 0.0),
        );

        let closed = surface
            .display_mesh(HemisphereLayoutState::closed())
            .unwrap()
            .mesh;
        let open = surface
            .display_mesh(HemisphereLayoutState::acorn())
            .unwrap()
            .mesh;

        assert_eq!(closed.metadata.side, SurfaceSide::Both);
        assert_eq!(closed.vertices.len(), 6);
        assert!(component_x_gap(&closed) > 0.0);
        assert!(component_x_gap(&open) > component_x_gap(&closed));
        assert_ne!(open.vertices[1][1], closed.vertices[1][1]);
    }

    #[test]
    fn pair_visibility_keeps_at_least_one_hemisphere_visible() {
        let both = PairVisibility::both();
        assert_eq!(both.label(), "left+right");

        let left_only = both.toggled(SurfaceSide::Right).unwrap();
        assert_eq!(left_only.label(), "left only");
        assert!(left_only.toggled(SurfaceSide::Left).is_none());

        let right_only = both.toggled(SurfaceSide::Left).unwrap();
        assert_eq!(right_only.label(), "right only");
        assert!(right_only.toggled(SurfaceSide::Right).is_none());
    }

    #[test]
    fn paired_preview_geometry_visibility_preserves_node_offsets() {
        let mut components = vec![
            component("smoothwm", SurfaceSide::Left, 0.0),
            component("smoothwm", SurfaceSide::Right, 3.0),
        ];

        let both = paired_preview_geometry(
            &mut components,
            HemisphereLayoutState::closed(),
            PairVisibility::both(),
            None,
        )
        .unwrap();
        assert_eq!(both.indices, vec![0, 1, 2, 3, 4, 5]);

        let left_only = paired_preview_geometry(
            &mut components,
            HemisphereLayoutState::closed(),
            PairVisibility {
                left: true,
                right: false,
            },
            None,
        )
        .unwrap();
        assert_eq!(left_only.indices, vec![0, 1, 2]);

        let right_only = paired_preview_geometry(
            &mut components,
            HemisphereLayoutState::closed(),
            PairVisibility {
                left: false,
                right: true,
            },
            None,
        )
        .unwrap();
        assert_eq!(right_only.indices, vec![3, 4, 5]);
    }

    #[test]
    fn paired_component_lookup_maps_composite_nodes_to_exact_surface() {
        let left = component("smoothwm", SurfaceSide::Left, 0.0);
        let right = component("smoothwm", SurfaceSide::Right, 3.0);

        assert_eq!(
            paired_component_for_node(&left, &right, 2).unwrap().side,
            SurfaceSide::Left
        );
        assert_eq!(
            paired_component_for_node(&left, &right, 3).unwrap().side,
            SurfaceSide::Right
        );
        assert!(paired_component_for_node(&left, &right, 6).is_none());
    }

    #[test]
    fn pair_transform_drag_math_matches_pysuma_direction() {
        let mut open_angle = 0.0_f32;
        let mut separation = 0.0_f32;
        let pair_width = 140.0_f32;

        open_angle =
            (open_angle + 100.0 * PAIR_OPEN_DEGREES_PER_PIXEL).clamp(0.0, PAIR_MAX_OPEN_DEGREES);
        separation = (separation + -(-50.0) * (pair_width / 700.0).max(0.05))
            .clamp(0.0, pair_width * PAIR_MAX_DRAG_GAP_FACTOR);

        assert_eq!(open_angle, 18.0);
        assert_eq!(separation, 10.0);
    }

    #[test]
    fn paired_overlay_paths_find_opposite_hemisphere_and_wildcard_label() {
        let paths =
            paired_overlay_paths(&PathBuf::from("std.141.ISC_lh_alpha_neg.niml.dset")).unwrap();

        assert_eq!(
            paths.left_path,
            PathBuf::from("std.141.ISC_lh_alpha_neg.niml.dset")
        );
        assert_eq!(
            paths.right_path,
            PathBuf::from("std.141.ISC_rh_alpha_neg.niml.dset")
        );
        assert_eq!(paths.display_name, "std.141.ISC_?h_alpha_neg.niml.dset");

        let paths = paired_overlay_paths(&PathBuf::from("std.141.rh.curv.gii.dset")).unwrap();
        assert_eq!(paths.left_path, PathBuf::from("std.141.lh.curv.gii.dset"));
        assert_eq!(paths.right_path, PathBuf::from("std.141.rh.curv.gii.dset"));
        assert_eq!(paths.display_name, "std.141.?h.curv.gii.dset");
    }

    #[test]
    fn paired_overlay_path_lookup_uses_exact_side_file() {
        let path = PathBuf::from("std.141.ISC_lh_alpha_neg.niml.dset");

        assert_eq!(
            paired_overlay_path_for_side(&path, &SurfaceSide::Left).unwrap(),
            PathBuf::from("std.141.ISC_lh_alpha_neg.niml.dset")
        );
        assert_eq!(
            paired_overlay_path_for_side(&path, &SurfaceSide::Right).unwrap(),
            PathBuf::from("std.141.ISC_rh_alpha_neg.niml.dset")
        );
        assert!(paired_overlay_path_for_side(&path, &SurfaceSide::Both).is_none());
    }

    #[test]
    fn paired_overlay_dataset_offsets_right_hemisphere_nodes() {
        let left_domain = SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap();
        let right_domain = SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap();
        let composite_domain =
            SurfaceDomain::from_triangles(6, vec![[0, 1, 2], [3, 4, 5]]).unwrap();
        let left = scalar_dataset(&left_domain, vec![1.0, 2.0, 3.0]);
        let right = scalar_dataset(&right_domain, vec![4.0, 5.0, 6.0]);

        let paired = paired_overlay_dataset(left, right, &composite_domain, 3).unwrap();

        assert_eq!(paired.row_count, 6);
        assert_eq!(paired.node_indices.as_deref(), None);
        let ColumnData::Float32(values) = &paired.columns[0].values else {
            panic!("expected float values");
        };
        assert_eq!(values, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    fn both_spec<const N: usize>(states: [&str; N]) -> SpecFile {
        SpecFile {
            path: PathBuf::from("both.spec"),
            group: None,
            states: states.into_iter().map(str::to_string).collect(),
            hemisphere: SpecHemisphere::Both,
            surfaces: Vec::new(),
        }
    }

    fn component(state: &str, side: SurfaceSide, x_offset: f32) -> SceneSurfaceComponent {
        let mut mesh = SurfaceMesh::new(
            vec![
                [x_offset, 0.0, 0.0],
                [x_offset + 1.0, 0.0, 0.0],
                [x_offset, 1.0, 0.0],
            ],
            vec![[0, 1, 2]],
        )
        .unwrap();
        mesh.metadata.side = side.clone();
        mesh.metadata.state_name = Some(state.to_string());
        let path = PathBuf::from(format!("{side:?}.{state}.gii"));

        SceneSurfaceComponent {
            name: format!("{side:?}.{state}"),
            state: Some(state.to_string()),
            path: path.clone(),
            side,
            spec_surface: SpecSurface {
                name: state.to_string(),
                path,
                surface_name: format!("{state}.gii"),
                surface_format: None,
                surface_type: None,
                state: Some(state.to_string()),
                raw_state: Some(state.to_string()),
                anatomical: None,
                side: mesh.metadata.side.clone(),
                local_domain_parent: None,
                local_curvature_parent: None,
                label_dataset: None,
                embed_dimension: None,
            },
            mesh: Some(mesh),
            normal_cache: None,
        }
    }

    fn component_x_gap(mesh: &SurfaceMesh) -> f32 {
        let left_max = mesh.vertices[0..3]
            .iter()
            .map(|vertex| vertex[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let right_min = mesh.vertices[3..6]
            .iter()
            .map(|vertex| vertex[0])
            .fold(f32::INFINITY, f32::min);

        right_min - left_max
    }

    fn scalar_dataset(domain: &SurfaceDomain, values: Vec<f32>) -> Dataset {
        Dataset::dense(
            DatasetKind::SurfaceScalar,
            domain,
            vec![
                DataColumn::new(
                    "scalar",
                    ColumnRole::Intensity,
                    None,
                    ColumnData::Float32(values),
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }
}
