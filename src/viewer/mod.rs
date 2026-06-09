use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset};
use crate::io::{read_gifti_dataset, read_niml_dataset};
use crate::stats::AfniStatSpec;
use crate::surface::{NormalDirection, OverlayDataset, SurfaceMesh, ValueRange};
use camera::{Camera, CameraMode, PresetOrientation};
use gpu::{
    DEPTH_FORMAT, DepthBuffer, choose_alpha_mode, choose_present_mode, choose_surface_format,
};
use mesh::{OverlayAppearance, OverlayColorMap, PreparedSurface, sample_colormap};
use pick::pick_surface;

mod camera;
mod gpu;
mod mesh;
mod pick;

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
const VERTEX_STRIDE: wgpu::BufferAddress = 40;
const MODE_LABEL_DURATION: Duration = Duration::from_secs(2);
const STARTUP_REDRAW_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_REDRAW_RETRY_INTERVAL: Duration = Duration::from_millis(16);
const CONTROL_CONTENT_WIDTH_POINTS: f32 = 560.0;
const CONTROL_MIN_INNER_WIDTH: u32 = 620;
const CONTROL_MIN_INNER_HEIGHT: u32 = 420;
const CONTROL_MAX_INNER_WIDTH: u32 = 900;
const CONTROL_RESIZE_THRESHOLD: u32 = 12;
const OVERLAY_THRESHOLD_COLUMN_WIDTH_POINTS: f32 = 96.0;
const OVERLAY_THRESHOLD_RAIL_HEIGHT_POINTS: f32 = 390.0;
const OVERLAY_THRESHOLD_BAR_HEIGHT_POINTS: f32 = 320.0;
const OVERLAY_SELECTOR_WIDTH_POINTS: f32 = 250.0;
const DEFAULT_OVERLAY_RANGE: ValueRange = ValueRange {
    min: -1.0,
    max: 1.0,
};
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

pub fn run(surface_path: Option<PathBuf>, overlay_path: Option<PathBuf>) -> Result<()> {
    let event_loop = EventLoop::new()?;
    // Render on demand rather than spinning at max FPS: the loop sleeps until an
    // input event, a requested redraw, or a scheduled animation deadline.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = ViewerApp::new(surface_path, overlay_path);
    event_loop.run_app(&mut app)?;

    if let Some(error) = app.setup_error {
        return Err(error);
    }

    Ok(())
}

struct ViewerApp {
    initial_surface_path: Option<PathBuf>,
    initial_overlay_path: Option<PathBuf>,
    state: Option<ViewerState>,
    setup_error: Option<anyhow::Error>,
}

impl ViewerApp {
    fn new(initial_surface_path: Option<PathBuf>, initial_overlay_path: Option<PathBuf>) -> Self {
        Self {
            initial_surface_path,
            initial_overlay_path,
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
                    .with_inner_size(PhysicalSize::new(CONTROL_MIN_INNER_WIDTH, 720)),
            )?,
        );
        if let Ok(position) = view_window.outer_position() {
            control_window.set_outer_position(PhysicalPosition::new(position.x + 1320, position.y));
        }
        self.state = Some(pollster::block_on(ViewerState::new(
            view_window,
            control_window,
            self.initial_surface_path.take(),
            self.initial_overlay_path.take(),
        ))?);

        Ok(())
    }
}

impl ApplicationHandler for ViewerApp {
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
    depth_buffer: DepthBuffer,
    mesh: Option<SurfaceMesh>,
    overlay: Option<OverlayDataset>,
    overlay_dataset: Option<Dataset>,
    overlay_columns: OverlayColumnSelections,
    overlay_visible: bool,
    overlay_appearance: OverlayAppearance,
    overlay_symmetric_range: bool,
    surface_path: Option<PathBuf>,
    overlay_path: Option<PathBuf>,
    surface_path_input: String,
    overlay_path_input: String,
    scene_stats: Option<SceneStats>,
    surface_pick: Option<SurfacePick>,
    status: StatusMessage,
    camera: Camera,
    view_cursor_position: Option<(f64, f64)>,
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
        initial_overlay_path: Option<PathBuf>,
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
            depth_buffer,
            mesh: None,
            overlay: None,
            overlay_dataset: None,
            overlay_columns: OverlayColumnSelections::default(),
            overlay_visible: true,
            overlay_appearance: OverlayAppearance::from_range(DEFAULT_OVERLAY_RANGE),
            overlay_symmetric_range: true,
            surface_path: None,
            overlay_path: None,
            surface_path_input: initial_surface_path
                .as_ref()
                .map_or_else(String::new, |path| path.display().to_string()),
            overlay_path_input: initial_overlay_path
                .as_ref()
                .map_or_else(String::new, |path| path.display().to_string()),
            scene_stats: None,
            surface_pick: None,
            status: StatusMessage::info("Ready. Paste a GIFTI surface path and load it."),
            camera,
            view_cursor_position: None,
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
        }
        if let Some(path) = initial_overlay_path {
            state.load_overlay_path(path)?;
        }
        state.arm_startup_redraw_guard();

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
                false
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.view_cursor_position = Some((position.x, position.y));
                self.camera.pointer_input(event)
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
                    PhysicalKey::Code(KeyCode::KeyO) => {
                        self.toggle_overlay_visibility();
                        true
                    }
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

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("surface render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.background.color()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_buffer.view,
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

            if let Some(buffers) = &self.surface_buffers {
                render_pass.set_pipeline(&self.render_pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, buffers.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(buffers.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..buffers.index_count, 0, 0..1);
            }
        }

        self.queue.submit([encoder.finish()]);
        output.present();

        RenderStatus::Rendered
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
        self.fit_control_window(desired_control_size_points, full_output.pixels_per_point);
        self.apply_ui_actions(ui_actions);
        if actions_present {
            self.view_window.request_redraw();
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
                    self.draw_status_section(ui);
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
        controller_section(ui, "SURFACE / DATASET", |ui| {
            egui::Grid::new("surface_dataset_grid")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Surface");
                    ui.horizontal(|ui| {
                        let response = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.surface_path_input)
                                    .desired_width(270.0)
                                    .hint_text("surface.gii"),
                            )
                            .on_hover_text("Press Return to load pasted path");
                        if response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            && let Some(path) = trimmed_path(&self.surface_path_input)
                        {
                            actions.push(UiAction::LoadSurface(path));
                        }
                        if ui.button("...").on_hover_text("Browse surface").clicked() {
                            actions.push(UiAction::PickSurface);
                        }
                    });
                    ui.end_row();

                    ui.label("Overlay");
                    ui.horizontal(|ui| {
                        let response = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.overlay_path_input)
                                    .desired_width(270.0)
                                    .hint_text("overlay.gii"),
                            )
                            .on_hover_text("Press Return to load pasted path");
                        let can_load_overlay =
                            self.mesh.is_some() && trimmed_path(&self.overlay_path_input).is_some();
                        if response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            && can_load_overlay
                            && let Some(path) = trimmed_path(&self.overlay_path_input)
                        {
                            actions.push(UiAction::LoadOverlay(path));
                        }
                        if ui
                            .add_enabled(self.mesh.is_some(), egui::Button::new("..."))
                            .on_hover_text("Browse overlay")
                            .clicked()
                        {
                            actions.push(UiAction::PickOverlay);
                        }
                    });
                    ui.end_row();
                });

            ui.add_space(4.0);
            object_line(
                ui,
                "Surface object",
                file_display(self.surface_path.as_ref()),
            );
            object_line(
                ui,
                "Dataset object",
                file_display(self.overlay_path.as_ref()),
            );
        });
    }

    fn draw_overlay_workbench(&mut self, ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
        let overlay_loaded = self.overlay.is_some();
        let overlay_row_count = self.overlay_dataset.as_ref().map_or_else(
            || {
                self.overlay
                    .as_ref()
                    .map_or(0, |overlay| overlay.values.len())
            },
            |dataset| dataset.row_count,
        );
        let column_options = self
            .overlay_dataset
            .as_ref()
            .map(overlay_column_options)
            .unwrap_or_default();
        let mut columns_changed = false;
        let mut changed = false;

        controller_section(ui, "OVERLAY WORKBENCH", |ui| {
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
                            stat_row(ui, "Dset", file_display(self.overlay_path.as_ref()));
                            stat_row(ui, "Rows", overlay_row_count.to_string());
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
            .or_else(|| self.overlay.as_ref().map(|overlay| overlay.range))
            .unwrap_or(DEFAULT_OVERLAY_RANGE)
    }

    fn selected_threshold_p_value(&self) -> Option<f64> {
        self.selected_threshold_stat_spec()
            .and_then(|stat| stat.two_sided_p_value(self.overlay_appearance.threshold.value as f64))
    }

    fn draw_view_section(&mut self, ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
        controller_section(ui, "VIEW", |ui| {
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
            });
        });
    }

    fn draw_scene_section(&self, ui: &mut egui::Ui) {
        controller_section(ui, "SCENE", |ui| {
            if let Some(stats) = self.scene_stats.as_ref() {
                egui::Grid::new("scene_stats_grid")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .show(ui, |ui| {
                        stat_row(ui, "Nodes", stats.node_count.to_string());
                        stat_row(ui, "Triangles", stats.face_count.to_string());
                        stat_row(ui, "Area", format!("{:.4}", stats.total_area));
                        stat_row(
                            ui,
                            "Normals",
                            normal_direction_label(stats.normal_direction),
                        );
                        stat_row(ui, "Boundary edges", stats.boundary_edges.to_string());
                        stat_row(ui, "Non-manifold", stats.non_manifold_edges.to_string());
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
        controller_section(ui, "PICK", |ui| {
            if let Some(pick) = self.surface_pick {
                egui::Grid::new("pick_grid")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .show(ui, |ui| {
                        stat_row(ui, "Node", pick.node_index.to_string());
                        stat_row(ui, "Triangle", pick.face_index.to_string());
                        stat_row(ui, "Overlay", overlay_value_label(pick.overlay_value));
                    });
            } else {
                ui.label(egui::RichText::new("No pick").color(muted_color()));
            }
        });
    }

    fn draw_status_section(&self, ui: &mut egui::Ui) {
        controller_section(ui, "STATUS", |ui| {
            let color = if self.status.is_error {
                egui::Color32::from_rgb(255, 126, 104)
            } else {
                egui::Color32::from_rgb(215, 224, 232)
            };
            ui.add(egui::Label::new(egui::RichText::new(&self.status.text).color(color)).wrap());
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
                UiAction::LoadSurface(path) => {
                    if let Err(error) = self.load_surface_path(path) {
                        self.set_error(error);
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
                UiAction::LoadOverlay(path) => {
                    if let Err(error) = self.load_overlay_path(path) {
                        self.set_error(error);
                    }
                }
                UiAction::RefreshOverlayColumns => {
                    if let Err(error) = self.refresh_overlay_columns() {
                        self.set_error(error);
                    }
                }
                UiAction::RefreshOverlayAppearance => {
                    if self.overlay.is_some() {
                        self.upload_surface_buffers();
                    }
                }
                UiAction::ResetCamera => self.camera.reset(),
                UiAction::ToggleCameraMode => {
                    let mode = self.camera.toggle_mode();
                    self.show_mode_label(mode);
                }
                UiAction::ToggleBackground => self.background.toggle(),
                UiAction::Preset(preset) => self.camera.set_preset(preset),
            }
        }
    }

    fn load_surface_path(&mut self, path: PathBuf) -> Result<()> {
        let mesh = SurfaceMesh::from_gifti_path(&path)
            .with_context(|| format!("failed to load surface {}", path.display()))?;
        let node_count = mesh.vertices.len();
        let face_count = mesh.triangles.len();

        self.mesh = Some(mesh);
        self.surface_path = Some(path.clone());
        self.surface_path_input = path.display().to_string();
        self.overlay = None;
        self.overlay_dataset = None;
        self.overlay_columns = OverlayColumnSelections::default();
        self.overlay_visible = true;
        self.overlay_appearance = OverlayAppearance::from_range(DEFAULT_OVERLAY_RANGE);
        self.overlay_symmetric_range = true;
        self.overlay_path = None;
        self.overlay_path_input.clear();
        self.surface_pick = None;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.camera.reset();
        self.view_window
            .set_title(&window_title(self.surface_path.as_ref()));
        self.status = StatusMessage::info(format!(
            "Loaded surface with {node_count} nodes and {face_count} triangles."
        ));

        Ok(())
    }

    fn load_overlay_path(&mut self, path: PathBuf) -> Result<()> {
        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before loading an overlay")?;
        let loaded_overlay = load_overlay_from_path(&path, mesh)
            .with_context(|| format!("failed to load overlay {}", path.display()))?;
        let overlay = loaded_overlay.overlay;
        let range = overlay.range;

        self.overlay = Some(overlay);
        self.overlay_dataset = loaded_overlay.dataset;
        self.overlay_columns = loaded_overlay.columns;
        self.overlay_visible = true;
        self.overlay_appearance = OverlayAppearance::from_range(range);
        self.overlay_symmetric_range = range.min < 0.0 && range.max > 0.0;
        self.overlay_path = Some(path.clone());
        self.overlay_path_input = path.display().to_string();
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.status = StatusMessage::info(format!(
            "Loaded overlay range {:.4} to {:.4}.{}",
            range.min,
            range.max,
            loaded_overlay
                .column_summary
                .map_or_else(String::new, |summary| format!(" {summary}"))
        ));

        Ok(())
    }

    fn refresh_overlay_columns(&mut self) -> Result<()> {
        let dataset = self
            .overlay_dataset
            .as_ref()
            .context("no canonical overlay dataset is loaded")?;
        let node_count = self
            .mesh
            .as_ref()
            .map(|mesh| mesh.vertices.len())
            .context("load a surface before selecting overlay columns")?;
        let overlay =
            overlay_dataset_from_canonical_dataset(dataset, node_count, self.overlay_columns)?;
        let range = overlay.range;
        let status = format!(
            "Overlay columns: I {}, T {}, B {}.",
            column_selection_label(dataset, Some(self.overlay_columns.intensity)),
            column_selection_label(dataset, self.overlay_columns.threshold),
            column_selection_label(dataset, self.overlay_columns.brightness)
        );

        self.overlay = Some(overlay);
        self.overlay_appearance.range = if self.overlay_symmetric_range {
            symmetric_value_range(range)
        } else {
            range
        };
        self.sanitize_overlay_appearance();
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.status = StatusMessage::info(status);

        Ok(())
    }

    fn toggle_overlay_visibility(&mut self) {
        if self.overlay.is_none() {
            self.status = StatusMessage::info("No overlay is loaded.");
            return;
        }

        self.overlay_visible = !self.overlay_visible;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.status = StatusMessage::info(if self.overlay_visible {
            "Overlay visible."
        } else {
            "Overlay hidden."
        });
    }

    fn visible_overlay(&self) -> Option<&OverlayDataset> {
        self.overlay.as_ref().filter(|_| self.overlay_visible)
    }

    fn inspect_surface_at_cursor(&mut self) {
        let Some(cursor) = self.view_cursor_position else {
            self.status =
                StatusMessage::info("Move the cursor over the surface before inspecting.");
            return;
        };
        let Some(mesh) = self.mesh.as_ref() else {
            self.status = StatusMessage::info("Load a surface before inspecting nodes.");
            return;
        };

        match pick_surface(
            mesh,
            self.overlay.as_ref(),
            &self.camera,
            self.view_size,
            cursor,
        ) {
            Some(pick) => {
                self.status = StatusMessage::info(pick.status_text());
                self.surface_pick = Some(pick);
            }
            None => {
                self.surface_pick = None;
                self.status = StatusMessage::info("No surface under the cursor.");
            }
        }
    }

    fn refresh_pick_overlay_value(&mut self) {
        if let Some(pick) = &mut self.surface_pick {
            pick.overlay_value = self
                .overlay
                .as_ref()
                .and_then(|overlay| overlay.values.get(pick.node_index as usize))
                .copied();
        }
    }

    fn upload_surface_buffers(&mut self) {
        let Some(mesh) = self.mesh.as_ref() else {
            self.surface_buffers = None;
            return;
        };

        let overlay = self.visible_overlay();
        let overlay_appearance = overlay.map(|_| self.overlay_appearance);
        let prepared_surface = PreparedSurface::from_surface(mesh, overlay, overlay_appearance);
        let vertex_bytes = prepared_surface.vertex_bytes();
        let index_bytes = prepared_surface.index_bytes();
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("surface vertex buffer"),
                contents: &vertex_bytes,
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("surface index buffer"),
                contents: &index_bytes,
                usage: wgpu::BufferUsages::INDEX,
            });

        self.surface_buffers = Some(SurfaceBuffers {
            vertex_buffer,
            index_buffer,
            index_count: prepared_surface.index_count(),
        });
    }

    fn update_scene_stats(&mut self) {
        self.scene_stats = self
            .mesh
            .as_ref()
            .map(|mesh| SceneStats::from_scene(mesh, self.overlay.as_ref()));
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
        self.status = StatusMessage::error(format!("{error:#}"));
    }
}

struct SurfaceBuffers {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

#[derive(Debug, Clone)]
struct LoadedOverlay {
    overlay: OverlayDataset,
    dataset: Option<Dataset>,
    columns: OverlayColumnSelections,
    column_summary: Option<String>,
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
    overlay_value: Option<f32>,
}

impl SurfacePick {
    fn status_text(self) -> String {
        match self.overlay_value {
            Some(value) => format!(
                "Inspected node {}, triangle {}, overlay {:.4}.",
                self.node_index, self.face_index, value
            ),
            None => format!(
                "Inspected node {}, triangle {}.",
                self.node_index, self.face_index
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct SceneStats {
    node_count: usize,
    face_count: usize,
    total_area: f32,
    boundary_edges: usize,
    non_manifold_edges: usize,
    normal_direction: NormalDirection,
    overlay_range: Option<ValueRange>,
}

impl SceneStats {
    fn from_scene(mesh: &SurfaceMesh, overlay: Option<&OverlayDataset>) -> Self {
        let winding = mesh.winding_report();

        Self {
            node_count: mesh.vertices.len(),
            face_count: mesh.triangles.len(),
            total_area: mesh.total_area(),
            boundary_edges: winding.boundary_edges,
            non_manifold_edges: winding.non_manifold_edges,
            normal_direction: winding.normal_direction,
            overlay_range: overlay.map(|overlay| overlay.range),
        }
    }
}

#[derive(Debug, Clone)]
struct StatusMessage {
    text: String,
    is_error: bool,
}

impl StatusMessage {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
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

enum UiAction {
    PickSurface,
    LoadSurface(PathBuf),
    PickOverlay,
    LoadOverlay(PathBuf),
    RefreshOverlayColumns,
    RefreshOverlayAppearance,
    ResetCamera,
    ToggleCameraMode,
    ToggleBackground,
    Preset(PresetOrientation),
}

enum RenderStatus {
    Rendered,
    Skipped,
    Reconfigure,
    ValidationError,
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
}

fn window_title(surface_path: Option<&PathBuf>) -> String {
    surface_path.map_or_else(
        || "sumaru".to_string(),
        |path| format!("sumaru - {}", path.display()),
    )
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

fn load_overlay_from_path(path: &Path, mesh: &SurfaceMesh) -> Result<LoadedOverlay> {
    if is_niml_dset_path(path) {
        let dataset = read_niml_dataset(path, &mesh.domain)?;
        loaded_overlay_from_dataset(dataset, mesh.vertices.len(), "NIML")
    } else if is_gifti_path(path) {
        match read_gifti_dataset(path, &mesh.domain) {
            Ok(dataset) => loaded_overlay_from_dataset(dataset, mesh.vertices.len(), "GIFTI"),
            Err(dataset_error) => Ok(LoadedOverlay {
                overlay: OverlayDataset::from_gifti_path(path, mesh.vertices.len())
                    .with_context(|| {
                        format!(
                            "failed to load GIFTI as canonical dataset ({dataset_error:#}) or simple overlay"
                        )
                    })?,
                dataset: None,
                columns: OverlayColumnSelections::default(),
                column_summary: None,
            }),
        }
    } else {
        Ok(LoadedOverlay {
            overlay: OverlayDataset::from_gifti_path(path, mesh.vertices.len())?,
            dataset: None,
            columns: OverlayColumnSelections::default(),
            column_summary: None,
        })
    }
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
    let column_summary = Some(format!(
        "I {}, T {}, B {}.",
        column_selection_label(&dataset, Some(columns.intensity)),
        column_selection_label(&dataset, columns.threshold),
        column_selection_label(&dataset, columns.brightness)
    ));

    Ok(LoadedOverlay {
        overlay,
        dataset: Some(dataset),
        columns,
        column_summary,
    })
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

fn trimmed_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

fn controller_section(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(title)
            .size(11.0)
            .strong()
            .color(egui::Color32::from_rgb(123, 184, 226)),
    );
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(28, 32, 39))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 62, 74)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, add_contents);
}

fn object_line(ui: &mut egui::Ui, label: &str, value: String) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(muted_color()));
        ui.monospace(value);
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
    path.and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .map_or_else(|| "none".to_string(), ToString::to_string)
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

fn size_is_close(current: PhysicalSize<u32>, desired: PhysicalSize<u32>) -> bool {
    current.width.abs_diff(desired.width) <= CONTROL_RESIZE_THRESHOLD
        && current.height.abs_diff(desired.height) <= CONTROL_RESIZE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::BackgroundMode;

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
}
