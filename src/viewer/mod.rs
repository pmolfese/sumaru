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

use crate::surface::{NormalDirection, OverlayDataset, SurfaceMesh, ValueRange};
use camera::{Camera, CameraMode, PresetOrientation};
use gpu::{
    DEPTH_FORMAT, DepthBuffer, choose_alpha_mode, choose_present_mode, choose_surface_format,
};
use mesh::PreparedSurface;
use pick::pick_surface;

mod camera;
mod gpu;
mod mesh;
mod pick;

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
const VERTEX_STRIDE: wgpu::BufferAddress = 40;
const MODE_LABEL_DURATION: Duration = Duration::from_secs(2);
const CONTROL_CONTENT_WIDTH_POINTS: f32 = 380.0;
const CONTROL_MIN_INNER_WIDTH: u32 = 420;
const CONTROL_MIN_INNER_HEIGHT: u32 = 420;
const CONTROL_MAX_INNER_WIDTH: u32 = 640;
const CONTROL_RESIZE_THRESHOLD: u32 = 12;
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
    event_loop.set_control_flow(ControlFlow::Poll);

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
                    .with_inner_size(PhysicalSize::new(420, 720)),
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
                state.view_window().request_redraw();
                return;
            }

            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Resized(size) => state.resize_view(size),
                WindowEvent::RedrawRequested => {
                    state.update();

                    match state.render_view() {
                        RenderStatus::Rendered | RenderStatus::Skipped => {}
                        RenderStatus::Reconfigure => state.resize_view(state.view_size),
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
            WindowEvent::Resized(size) => state.resize_control(size),
            WindowEvent::RedrawRequested => match state.render_control() {
                RenderStatus::Rendered | RenderStatus::Skipped => {}
                RenderStatus::Reconfigure => state.resize_control(state.control_size),
                RenderStatus::ValidationError => eprintln!("control validation error"),
            },
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = self.state.as_ref() {
            state.view_window().request_redraw();
            state.control_window().request_redraw();
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
    render_pipeline: wgpu::RenderPipeline,
    surface_buffers: Option<SurfaceBuffers>,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    depth_buffer: DepthBuffer,
    mesh: Option<SurfaceMesh>,
    overlay: Option<OverlayDataset>,
    overlay_visible: bool,
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
            render_pipeline,
            surface_buffers: None,
            uniform_buffer,
            uniform_bind_group,
            depth_buffer,
            mesh: None,
            overlay: None,
            overlay_visible: true,
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

        Ok(state)
    }

    fn view_window(&self) -> &Window {
        &self.view_window
    }

    fn control_window(&self) -> &Window {
        &self.control_window
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
        self.egui_state
            .handle_platform_output(&self.control_window, full_output.platform_output);
        self.fit_control_window(desired_control_size_points, full_output.pixels_per_point);
        self.apply_ui_actions(ui_actions);
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
            self.egui_ctx.request_repaint();
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
            let panel_top = ui.cursor().top();
            ui.heading("sumaru");
            ui.separator();
            let content_top = ui.cursor().top();
            let scroll_output = egui::ScrollArea::vertical()
                .max_height(panel_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_min_width(CONTROL_CONTENT_WIDTH_POINTS);
                    ui.label("Surface");
                    let surface_response = ui.text_edit_singleline(&mut self.surface_path_input);
                    let surface_enter = surface_response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    let surface_path = trimmed_path(&self.surface_path_input);
                    ui.horizontal(|ui| {
                        if ui.button("Browse...").clicked() {
                            actions.push(UiAction::PickSurface);
                        }
                        if ui
                            .add_enabled(surface_path.is_some(), egui::Button::new("Load surface"))
                            .clicked()
                            || surface_enter
                        {
                            if let Some(path) = surface_path.clone() {
                                actions.push(UiAction::LoadSurface(path));
                            }
                        }
                    });

                    ui.add_space(8.0);
                    ui.label("Overlay");
                    let overlay_response = ui.text_edit_singleline(&mut self.overlay_path_input);
                    let overlay_enter = overlay_response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    let overlay_path = trimmed_path(&self.overlay_path_input);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(self.mesh.is_some(), egui::Button::new("Browse..."))
                            .clicked()
                        {
                            actions.push(UiAction::PickOverlay);
                        }
                        if ui
                            .add_enabled(
                                self.mesh.is_some() && overlay_path.is_some(),
                                egui::Button::new("Load overlay"),
                            )
                            .clicked()
                            || overlay_enter
                        {
                            if let Some(path) = overlay_path.clone() {
                                actions.push(UiAction::LoadOverlay(path));
                            }
                        }
                        if ui
                            .add_enabled(self.overlay.is_some(), egui::Button::new("Clear"))
                            .clicked()
                        {
                            actions.push(UiAction::ClearOverlay);
                        }
                        if ui
                            .add_enabled(
                                self.overlay.is_some(),
                                egui::Button::new(overlay_toggle_label(self.overlay_visible)),
                            )
                            .clicked()
                        {
                            actions.push(UiAction::ToggleOverlayVisibility);
                        }
                    });
                    if self.overlay.is_some() {
                        ui.label(format!(
                            "Overlay: {}",
                            if self.overlay_visible {
                                "visible"
                            } else {
                                "hidden"
                            }
                        ));
                    }

                    ui.separator();
                    ui.label(format!("Camera: {}", self.camera.mode().label()));
                    ui.horizontal(|ui| {
                        if ui.button("Reset").clicked() {
                            actions.push(UiAction::ResetCamera);
                        }
                        if ui.button("Switch").clicked() {
                            actions.push(UiAction::ToggleCameraMode);
                        }
                        if ui.button(self.background.next_label()).clicked() {
                            actions.push(UiAction::ToggleBackground);
                        }
                    });
                    ui.horizontal(|ui| {
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

                    if let Some(stats) = &self.scene_stats {
                        ui.separator();
                        ui.label("Scene");
                        egui::Grid::new("scene_stats")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                stat_row(ui, "Nodes", stats.node_count.to_string());
                                stat_row(ui, "Triangles", stats.face_count.to_string());
                                stat_row(ui, "Area", format!("{:.3}", stats.total_area));
                                stat_row(ui, "Boundary edges", stats.boundary_edges.to_string());
                                stat_row(
                                    ui,
                                    "Non-manifold edges",
                                    stats.non_manifold_edges.to_string(),
                                );
                                stat_row(
                                    ui,
                                    "Normals",
                                    normal_direction_label(stats.normal_direction),
                                );
                                if let Some(range) = stats.overlay_range {
                                    stat_row(
                                        ui,
                                        "Overlay range",
                                        format!("{:.4} to {:.4}", range.min, range.max),
                                    );
                                }
                            });
                    }

                    if let Some(pick) = &self.surface_pick {
                        ui.separator();
                        ui.label("Inspect");
                        egui::Grid::new("surface_pick")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                stat_row(ui, "Node", pick.node_index.to_string());
                                stat_row(ui, "Triangle", pick.face_index.to_string());
                                stat_row(ui, "Overlay", overlay_value_label(pick.overlay_value));
                            });
                    }

                    ui.separator();
                    let color = if self.status.is_error {
                        egui::Color32::from_rgb(255, 120, 120)
                    } else {
                        egui::Color32::from_rgb(145, 220, 145)
                    };
                    ui.colored_label(color, &self.status.text);
                });
            let header_height = content_top - panel_top;
            desired_control_size_points = egui::vec2(
                scroll_output
                    .content_size
                    .x
                    .max(CONTROL_CONTENT_WIDTH_POINTS)
                    + 32.0,
                header_height + scroll_output.content_size.y + 32.0,
            );
        });

        if let Some(text) = self.active_mode_label_text() {
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
            .map(|size| ((size.width as f32 * 0.45) as u32).min(CONTROL_MAX_INNER_WIDTH))
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
                UiAction::ToggleOverlayVisibility => self.toggle_overlay_visibility(),
                UiAction::ClearOverlay => self.clear_overlay(),
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
        self.overlay_visible = true;
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
        let overlay = OverlayDataset::from_gifti_path(&path, mesh.vertices.len())
            .with_context(|| format!("failed to load overlay {}", path.display()))?;
        let range = overlay.range;

        self.overlay = Some(overlay);
        self.overlay_visible = true;
        self.overlay_path = Some(path.clone());
        self.overlay_path_input = path.display().to_string();
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.status = StatusMessage::info(format!(
            "Loaded overlay range {:.4} to {:.4}.",
            range.min, range.max
        ));

        Ok(())
    }

    fn clear_overlay(&mut self) {
        self.overlay = None;
        self.overlay_visible = true;
        self.overlay_path = None;
        self.overlay_path_input.clear();
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.status = StatusMessage::info("Cleared overlay.");
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

        let prepared_surface = PreparedSurface::from_surface(mesh, self.visible_overlay());
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

    fn active_mode_label_text(&mut self) -> Option<&'static str> {
        let label = self.mode_label.as_ref()?;
        if Instant::now() > label.until {
            self.mode_label = None;
            return None;
        }

        Some(label.text)
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
    ToggleOverlayVisibility,
    ClearOverlay,
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
            .add_filter("GIFTI or SUMA dataset", &["gii", "dset"]),
        current_path,
    );

    dialog.pick_file()
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

fn overlay_toggle_label(overlay_visible: bool) -> &'static str {
    if overlay_visible {
        "Hide overlay"
    } else {
        "Show overlay"
    }
}

fn size_is_close(current: PhysicalSize<u32>, desired: PhysicalSize<u32>) -> bool {
    current.width.abs_diff(desired.width) <= CONTROL_RESIZE_THRESHOLD
        && current.height.abs_diff(desired.height) <= CONTROL_RESIZE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::{BackgroundMode, overlay_toggle_label};

    #[test]
    fn background_toggles_between_black_and_white() {
        let mut background = BackgroundMode::Black;

        background.toggle();
        assert_eq!(background, BackgroundMode::White);

        background.toggle();
        assert_eq!(background, BackgroundMode::Black);
    }

    #[test]
    fn overlay_toggle_label_describes_next_action() {
        assert_eq!(overlay_toggle_label(true), "Hide overlay");
        assert_eq!(overlay_toggle_label(false), "Show overlay");
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
