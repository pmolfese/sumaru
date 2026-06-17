use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
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

use crate::afni::{
    AfniConnection, AfniConnectionEvent, AfniNimlSession, AfniPortConfig, AfniRgbaOverlay,
    AfniRouteAction, AfniSurfaceCrosshair, AfniSurfaceInfo, DEFAULT_AFNI_HOST,
    DEFAULT_AFNI_NIML_PORT, surface_crosshair_element,
};
use crate::color::Rgba;
use crate::command::{
    BackgroundMode, CameraControlMode, ControllerState, HemisphereLayout, HemisphereLayoutState,
    PairVisibility, SurfacePick, ViewPreset, ViewerCommand,
};
use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind};
use crate::io::{
    NimlElement, read_gifti_dataset, read_niml_dataset, read_niml_roi, write_niml_roi,
};
use crate::niml_debug::NimlRecorder;
use crate::overlay::{
    ColumnSelection, MaskMode, Overlay, OverlayColumns, OverlayRange, RangeSelection, Threshold,
};
use crate::roi::{
    Roi, RoiBrushAction, RoiDatum, RoiDrawStatus, RoiDrawingType, RoiElementKind, RoiSource,
};
use crate::spec::{SpecFile, SpecHemisphere, SpecSurface, read_spec};
use crate::stats::AfniStatSpec;
use crate::surface::{
    AnatomicalCorrectness, NodeMask, NormalDirection, OverlayDataset, SmoothingWeights,
    SurfaceDomain, SurfaceDomainId, SurfaceId, SurfaceKind, SurfaceMesh, SurfaceSide, ValueRange,
};
use camera::{Camera, CameraMode, PresetOrientation};
use gpu::{
    DEPTH_FORMAT, DepthBuffer, choose_alpha_mode, choose_present_mode, choose_surface_format,
};
use mesh::{
    OverlayAppearance, OverlayColorMap, PreparedGeometry, PreparedGeometryVertex, PreparedSurface,
    RoiAppearance, SelectionHighlight, sample_colormap,
};
use pick::{pick_surface, pick_surface_with_model};
use screenshot::ScreenshotImage;

mod camera;
mod gpu;
mod mesh;
mod pick;
mod screenshot;

impl From<CameraMode> for CameraControlMode {
    fn from(mode: CameraMode) -> Self {
        match mode {
            CameraMode::Orbit => Self::Orbit,
            CameraMode::Turntable => Self::Turntable,
        }
    }
}

impl From<ViewPreset> for PresetOrientation {
    fn from(preset: ViewPreset) -> Self {
        match preset {
            ViewPreset::Left => Self::Left,
            ViewPreset::Right => Self::Right,
            ViewPreset::Top => Self::Top,
            ViewPreset::Bottom => Self::Bottom,
        }
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
const VERTEX_STRIDE: wgpu::BufferAddress = 40;
const MODE_LABEL_DURATION: Duration = Duration::from_secs(2);
const STARTUP_REDRAW_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_REDRAW_RETRY_INTERVAL: Duration = Duration::from_millis(16);
const SUMA_CONVEXITY_SMOOTHING_ITERATIONS: usize = 5;
/// Overlay color for nodes AFNI did not colorize (absent from the `SUMA_irgba`
/// node list, or sent with zero alpha). Transparent so the anatomical underlay
/// shows through, matching SUMA.
const AFNI_TRANSPARENT_NODE_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
const SUMA_CONVEXITY_OPACITY: f32 = 0.85;
const CONTROL_CONTENT_WIDTH_POINTS: f32 = 560.0;
const CONTROL_MIN_INNER_WIDTH: u32 = 620;
const CONTROL_MIN_INNER_HEIGHT: u32 = 420;
const CONTROL_INITIAL_INNER_HEIGHT: u32 = 720;
const CONTROL_MAX_INNER_WIDTH: u32 = 900;
const CONTROL_RESIZE_THRESHOLD: u32 = 12;
const ROI_CONTROL_CONTENT_WIDTH_POINTS: f32 = 360.0;
const ROI_CONTROL_MIN_INNER_WIDTH: u32 = 430;
const ROI_CONTROL_INNER_WIDTH: u32 = 430;
const ROI_CONTROL_INNER_HEIGHT: u32 = 260;
const ROI_CONTROL_MAX_INNER_WIDTH: u32 = 1100;
const ROI_CONTROL_MIN_INNER_HEIGHT: u32 = 260;
const GRAPH_WINDOW_INNER_WIDTH: u32 = 600;
const GRAPH_WINDOW_INNER_HEIGHT: u32 = 400;
const GRAPH_MIN_INITIAL_INNER_WIDTH: u32 = 420;
const GRAPH_MIN_INITIAL_INNER_HEIGHT: u32 = 160;
const GRAPH_MIN_PLOT_WIDTH_POINTS: f32 = 320.0;
const GRAPH_MIN_PLOT_HEIGHT_POINTS: f32 = 96.0;
const GRAPH_DEFAULT_PLOT_HEIGHT_POINTS: f32 = 138.0;
const GRAPH_DOCK_DEFAULT_HEIGHT_POINTS: f32 = 360.0;
const GRAPH_DOCK_MIN_HEIGHT_POINTS: f32 = 180.0;
const GRAPH_MAX_VIEW_WIDTH_FRACTION: f32 = 0.75;
const GRAPH_MAX_VIEW_HEIGHT_FRACTION: f32 = 0.25;
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
    pub overlay_pair_paths: Option<ExplicitOverlayPair>,
    pub roi_path: Option<PathBuf>,
    pub overlay_subs: Option<Vec<String>>,
    pub overlay_p_value: Option<f64>,
    pub verbose: bool,
    pub preload: bool,
    pub afni: AfniViewerOptions,
    pub niml_record_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitOverlayPair {
    pub left_path: PathBuf,
    pub right_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AfniViewerOptions {
    pub connect_on_launch: bool,
    pub port_config: AfniPortConfig,
}

impl Default for AfniViewerOptions {
    fn default() -> Self {
        Self {
            connect_on_launch: false,
            port_config: AfniPortConfig {
                host: DEFAULT_AFNI_HOST.to_string(),
                port: DEFAULT_AFNI_NIML_PORT,
                port_offset: None,
                port_bloc: None,
            },
        }
    }
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
    initial_overlay_pair_paths: Option<ExplicitOverlayPair>,
    initial_roi_path: Option<PathBuf>,
    initial_overlay_subs: Option<Vec<String>>,
    initial_overlay_p_value: Option<f64>,
    verbose: bool,
    preload: bool,
    afni: AfniViewerOptions,
    niml_record_path: Option<PathBuf>,
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
            initial_overlay_pair_paths: options.overlay_pair_paths,
            initial_roi_path: options.roi_path,
            initial_overlay_subs: options.overlay_subs,
            initial_overlay_p_value: options.overlay_p_value,
            verbose: options.verbose,
            preload: options.preload,
            afni: options.afni,
            niml_record_path: options.niml_record_path,
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
        let roi_control_window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("sumaru ROI controls")
                    .with_inner_size(PhysicalSize::new(
                        ROI_CONTROL_INNER_WIDTH,
                        ROI_CONTROL_INNER_HEIGHT,
                    ))
                    .with_visible(false),
            )?,
        );
        let graph_window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("sumaru graph")
                    .with_inner_size(graph_initial_inner_size(view_window.inner_size()))
                    .with_visible(false),
            )?,
        );
        if let Ok(position) = view_window.outer_position() {
            let raised_y = position.y.saturating_sub(INITIAL_WINDOW_RAISE_PIXELS);
            view_window.set_outer_position(PhysicalPosition::new(position.x, raised_y));
            control_window.set_outer_position(PhysicalPosition::new(position.x + 1320, raised_y));
            roi_control_window
                .set_outer_position(PhysicalPosition::new(position.x + 1320, raised_y + 760));
            graph_window.set_outer_position(PhysicalPosition::new(position.x + 80, raised_y + 80));
        }
        self.state = Some(pollster::block_on(ViewerState::new(
            view_window,
            control_window,
            roi_control_window,
            graph_window,
            self.initial_surface_path.take(),
            self.initial_spec_path.take(),
            self.initial_surface_volume_path.take(),
            self.initial_overlay_path.take(),
            self.initial_overlay_pair_paths.take(),
            self.initial_roi_path.take(),
            self.initial_overlay_subs.take(),
            self.initial_overlay_p_value.take(),
            self.verbose,
            self.preload,
            self.afni.clone(),
            self.niml_record_path.clone(),
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

        if window_id == state.control_window().id() {
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
            return;
        }

        if window_id == state.roi_control_window().id() {
            let input = state.roi_control_input(&event);
            if input.repaint {
                state.roi_control_window().request_redraw();
            }
            if input.consumed {
                state.roi_control_window().request_redraw();
                return;
            }
            match event {
                WindowEvent::CloseRequested => {
                    state.apply_commands(vec![ViewerCommand::SetRoiControllerOpen(false)]);
                }
                WindowEvent::Resized(size) => {
                    state.resize_roi_control(size);
                    state.roi_control_window().request_redraw();
                }
                WindowEvent::RedrawRequested => match state.render_roi_control() {
                    RenderStatus::Rendered => {}
                    RenderStatus::Skipped => {}
                    RenderStatus::Reconfigure => {
                        state.resize_roi_control(state.roi_control_size);
                        state.roi_control_window().request_redraw();
                    }
                    RenderStatus::ValidationError => eprintln!("ROI control validation error"),
                },
                _ => {}
            }
            return;
        }

        if window_id == state.graph_window().id() {
            let input = state.graph_input(&event);
            if input.repaint {
                state.graph_window().request_redraw();
            }
            if input.consumed {
                state.graph_window().request_redraw();
                return;
            }
            match event {
                WindowEvent::CloseRequested => {
                    state.apply_commands(vec![ViewerCommand::SetGraphWindowOpen(false)]);
                }
                WindowEvent::Resized(size) => {
                    state.resize_graph(size);
                    state.graph_window().request_redraw();
                }
                WindowEvent::RedrawRequested => match state.render_graph() {
                    RenderStatus::Rendered => {}
                    RenderStatus::Skipped => {}
                    RenderStatus::Reconfigure => {
                        state.resize_graph(state.graph_size);
                        state.graph_window().request_redraw();
                    }
                    RenderStatus::ValidationError => eprintln!("graph validation error"),
                },
                _ => {}
            }
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
                    if state.controller.panels.roi_controller_open {
                        state.roi_control_window().request_redraw();
                    }
                    if state.controller.panels.graph_window_open {
                        state.view_window().request_redraw();
                    }
                    state.view_window().request_redraw();
                }
            }
            ViewerEvent::AfniMessagesReady => {
                if state.drain_afni_events() {
                    state.control_window().request_redraw();
                    if state.controller.panels.roi_controller_open {
                        state.roi_control_window().request_redraw();
                    }
                    if state.controller.panels.graph_window_open {
                        state.view_window().request_redraw();
                    }
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

        let next_view = state.view_repaint_at;
        let next_control = state.control_repaint_at;
        let next_roi_control = state
            .controller
            .panels
            .roi_controller_open
            .then_some(state.roi_control_repaint_at)
            .flatten();
        let view_due = next_view.is_some_and(|at| at <= now);
        let control_due = next_control.is_some_and(|at| at <= now);
        let roi_control_due = next_roi_control.is_some_and(|at| at <= now);
        if view_due {
            state.view_window().request_redraw();
        }
        if control_due {
            state.control_window().request_redraw();
        }
        if roi_control_due {
            state.roi_control_window().request_redraw();
        }

        let next_wake = [next_view, next_control, next_roi_control]
            .into_iter()
            .flatten()
            .filter(|at| *at > now)
            .min();
        match next_wake {
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None if view_due || control_due || roi_control_due => {
                event_loop.set_control_flow(ControlFlow::Wait)
            }
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

struct ViewerState {
    view_window: Arc<Window>,
    control_window: Arc<Window>,
    roi_control_window: Arc<Window>,
    graph_window: Arc<Window>,
    view_surface: wgpu::Surface<'static>,
    control_surface: wgpu::Surface<'static>,
    roi_control_surface: wgpu::Surface<'static>,
    graph_surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    view_config: wgpu::SurfaceConfiguration,
    control_config: wgpu::SurfaceConfiguration,
    roi_control_config: wgpu::SurfaceConfiguration,
    graph_config: wgpu::SurfaceConfiguration,
    view_size: PhysicalSize<u32>,
    control_size: PhysicalSize<u32>,
    roi_control_size: PhysicalSize<u32>,
    graph_size: PhysicalSize<u32>,
    last_requested_control_size: Option<PhysicalSize<u32>>,
    last_requested_roi_control_size: Option<PhysicalSize<u32>>,
    graph_dock_pre_open_size: Option<PhysicalSize<u32>>,
    /// When the view window should next repaint for menu animations, or `None`
    /// if its egui overlay is idle.
    view_repaint_at: Option<Instant>,
    /// When the control window should next repaint for an egui animation, or
    /// `None` if it is idle. Drives `ControlFlow::WaitUntil`.
    control_repaint_at: Option<Instant>,
    roi_control_repaint_at: Option<Instant>,
    graph_repaint_at: Option<Instant>,
    view_frame_rendered: bool,
    control_frame_rendered: bool,
    startup_redraw_until: Instant,
    render_pipeline: wgpu::RenderPipeline,
    surface_buffers: Option<SurfaceBuffers>,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    uniform_bind_group_layout: wgpu::BindGroupLayout,
    /// Per-hemisphere resident buffers for active both-spec scenes. The logical
    /// combined mesh still backs overlays, ROI node offsets, and picking, while
    /// drawing uses these instances with tiny model-matrix uniform updates.
    surface_render_set: Option<SurfaceRenderSet>,
    depth_buffer: DepthBuffer,
    mesh: Option<SurfaceMesh>,
    prepared_geometry_cache: Option<PreparedGeometryCache>,
    anatomical_shading_cache: Option<AnatomicalShadingCache>,
    surface_scene: Option<SurfaceScene>,
    scene_generation: u64,
    controller: ControllerState,
    overlay: Option<Overlay>,
    overlay_values: Option<OverlayDataset>,
    overlay_dataset: Option<Dataset>,
    overlay_columns: OverlayColumnSelections,
    overlay_appearance: OverlayAppearance,
    surface_path: Option<PathBuf>,
    overlay_path: Option<PathBuf>,
    overlay_pair_paths: Option<ExplicitOverlayPair>,
    roi_path: Option<PathBuf>,
    overlay_display_name: Option<String>,
    roi_layer: Option<RoiLayer>,
    roi_workspace: RoiWorkspace,
    graph_snapshot: Option<GraphSnapshot>,
    surface_volume_path: Option<PathBuf>,
    surface_volume_idcode: Option<String>,
    scene_stats: Option<SceneStats>,
    /// Cached geometry-derived stats (winding/area/counts) keyed by surface id,
    /// so recolors do not recompute the expensive `winding_report`.
    scene_geometry_stats: Option<(SurfaceId, SceneGeometryStats)>,
    verbose: bool,
    preload_enabled: bool,
    preload_sender: Sender<PreloadResult>,
    preload_receiver: Receiver<PreloadResult>,
    event_proxy: EventLoopProxy<ViewerEvent>,
    afni_options: AfniViewerOptions,
    afni_connection: Option<AfniConnection>,
    afni_session: AfniNimlSession,
    afni_recorder: Option<NimlRecorder>,
    afni_rgba_colors: Option<Vec<[f32; 4]>>,
    /// Last applied `SUMA_irgba` payload hash per source surface idcode. AFNI
    /// resends identical colorizations on every redraw; this lets us skip the
    /// recolor + re-upload when nothing changed.
    afni_rgba_signatures: HashMap<String, u64>,
    /// Combined-mesh node of the last crosshair we sent to AFNI.
    sent_crosshair_node: Option<u32>,
    /// Combined-mesh node of AFNI's most recently reported crosshair. Together
    /// with [`Self::sent_crosshair_node`] this lets us nudge AFNI into redrawing
    /// after a surface registration without moving its crosshair to an arbitrary
    /// location.
    afni_crosshair_node: Option<u32>,
    camera: Camera,
    view_cursor_position: Option<(f64, f64)>,
    pair_dragging: bool,
    pair_drag_last_cursor: Option<(f64, f64)>,
    pair_drag_changed: bool,
    modifiers: ModifiersState,
    mode_label: Option<ModeLabel>,
    view_egui_ctx: egui::Context,
    view_egui_state: egui_winit::State,
    view_egui_renderer: Renderer,
    view_pending_egui_textures: egui::TexturesDelta,
    view_allocated_egui_textures: HashSet<egui::TextureId>,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: Renderer,
    pending_egui_textures: egui::TexturesDelta,
    allocated_egui_textures: HashSet<egui::TextureId>,
    roi_egui_ctx: egui::Context,
    roi_egui_state: egui_winit::State,
    roi_egui_renderer: Renderer,
    roi_pending_egui_textures: egui::TexturesDelta,
    roi_allocated_egui_textures: HashSet<egui::TextureId>,
    graph_egui_ctx: egui::Context,
    graph_egui_state: egui_winit::State,
    graph_egui_renderer: Renderer,
    graph_pending_egui_textures: egui::TexturesDelta,
    graph_allocated_egui_textures: HashSet<egui::TextureId>,
}

impl ViewerState {
    async fn new(
        view_window: Arc<Window>,
        control_window: Arc<Window>,
        roi_control_window: Arc<Window>,
        graph_window: Arc<Window>,
        initial_surface_path: Option<PathBuf>,
        initial_spec_path: Option<PathBuf>,
        initial_surface_volume_path: Option<PathBuf>,
        initial_overlay_path: Option<PathBuf>,
        initial_overlay_pair_paths: Option<ExplicitOverlayPair>,
        initial_roi_path: Option<PathBuf>,
        initial_overlay_subs: Option<Vec<String>>,
        initial_overlay_p_value: Option<f64>,
        verbose: bool,
        preload_enabled: bool,
        afni_options: AfniViewerOptions,
        niml_record_path: Option<PathBuf>,
        event_proxy: EventLoopProxy<ViewerEvent>,
    ) -> Result<Self> {
        let view_size = view_window.inner_size();
        let control_size = control_window.inner_size();
        let roi_control_size = roi_control_window.inner_size();
        let graph_size = graph_window.inner_size();
        let instance = wgpu::Instance::default();
        let view_surface = instance.create_surface(view_window.clone())?;
        let control_surface = instance.create_surface(control_window.clone())?;
        let roi_control_surface = instance.create_surface(roi_control_window.clone())?;
        let graph_surface = instance.create_surface(graph_window.clone())?;
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
        let roi_control_caps = roi_control_surface.get_capabilities(&adapter);
        let graph_caps = graph_surface.get_capabilities(&adapter);
        let surface_format = choose_surface_format(&view_caps, &control_caps);
        let present_mode = choose_present_mode(&view_caps, &control_caps);
        let alpha_mode = choose_alpha_mode(&view_caps, &control_caps);
        ensure!(
            roi_control_caps.formats.contains(&surface_format),
            "ROI controller surface does not support selected format {surface_format:?}"
        );
        ensure!(
            roi_control_caps.present_modes.contains(&present_mode),
            "ROI controller surface does not support selected present mode {present_mode:?}"
        );
        ensure!(
            roi_control_caps.alpha_modes.contains(&alpha_mode),
            "ROI controller surface does not support selected alpha mode {alpha_mode:?}"
        );
        ensure!(
            graph_caps.formats.contains(&surface_format),
            "graph surface does not support selected format {surface_format:?}"
        );
        ensure!(
            graph_caps.present_modes.contains(&present_mode),
            "graph surface does not support selected present mode {present_mode:?}"
        );
        ensure!(
            graph_caps.alpha_modes.contains(&alpha_mode),
            "graph surface does not support selected alpha mode {alpha_mode:?}"
        );
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
        let roi_control_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: roi_control_size.width.max(1),
            height: roi_control_size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        let graph_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: graph_size.width.max(1),
            height: graph_size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        view_surface.configure(&device, &view_config);
        control_surface.configure(&device, &control_config);
        roi_control_surface.configure(&device, &roi_control_config);
        graph_surface.configure(&device, &graph_config);

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
        let view_egui_ctx = egui::Context::default();
        view_egui_ctx.set_visuals(egui::Visuals::dark());
        let mut view_egui_state = egui_winit::State::new(
            view_egui_ctx.clone(),
            egui::ViewportId::ROOT,
            view_window.as_ref(),
            None,
            None,
            None,
        );
        view_egui_state.set_max_texture_side(device.limits().max_texture_dimension_2d as usize);
        let view_egui_renderer = Renderer::new(&device, surface_format, RendererOptions::default());
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
        let roi_egui_ctx = egui::Context::default();
        roi_egui_ctx.set_visuals(egui::Visuals::dark());
        let mut roi_egui_state = egui_winit::State::new(
            roi_egui_ctx.clone(),
            egui::ViewportId::ROOT,
            roi_control_window.as_ref(),
            None,
            None,
            None,
        );
        roi_egui_state.set_max_texture_side(device.limits().max_texture_dimension_2d as usize);
        let roi_egui_renderer = Renderer::new(&device, surface_format, RendererOptions::default());
        let graph_egui_ctx = egui::Context::default();
        graph_egui_ctx.set_visuals(egui::Visuals::dark());
        let mut graph_egui_state = egui_winit::State::new(
            graph_egui_ctx.clone(),
            egui::ViewportId::ROOT,
            graph_window.as_ref(),
            None,
            None,
            None,
        );
        graph_egui_state.set_max_texture_side(device.limits().max_texture_dimension_2d as usize);
        let graph_egui_renderer =
            Renderer::new(&device, surface_format, RendererOptions::default());
        let initial_surface_volume_path =
            initial_surface_volume_path.map(canonical_or_original_path);
        let initial_surface_volume_idcode =
            query_afni_dataset_idcode_optional(initial_surface_volume_path.as_deref())?;
        let (preload_sender, preload_receiver) = mpsc::channel();
        let afni_recorder = niml_record_path.map(NimlRecorder::create).transpose()?;

        let mut state = Self {
            view_window,
            control_window,
            roi_control_window,
            graph_window,
            view_surface,
            control_surface,
            roi_control_surface,
            graph_surface,
            device,
            queue,
            view_config,
            control_config,
            roi_control_config,
            graph_config,
            view_size,
            control_size,
            roi_control_size,
            graph_size,
            last_requested_control_size: None,
            last_requested_roi_control_size: None,
            graph_dock_pre_open_size: None,
            view_repaint_at: None,
            control_repaint_at: None,
            roi_control_repaint_at: None,
            graph_repaint_at: None,
            view_frame_rendered: false,
            control_frame_rendered: false,
            startup_redraw_until: Instant::now(),
            render_pipeline,
            surface_buffers: None,
            uniform_buffer,
            uniform_bind_group,
            uniform_bind_group_layout,
            surface_render_set: None,
            depth_buffer,
            mesh: None,
            prepared_geometry_cache: None,
            anatomical_shading_cache: None,
            surface_scene: None,
            scene_generation: 0,
            controller: ControllerState::default(),
            overlay: None,
            overlay_values: None,
            overlay_dataset: None,
            overlay_columns: OverlayColumnSelections::default(),
            overlay_appearance: OverlayAppearance::from_range(DEFAULT_OVERLAY_RANGE),
            surface_path: None,
            overlay_path: None,
            overlay_pair_paths: None,
            roi_path: None,
            overlay_display_name: None,
            roi_layer: None,
            roi_workspace: RoiWorkspace::default(),
            graph_snapshot: None,
            surface_volume_path: initial_surface_volume_path.clone(),
            surface_volume_idcode: initial_surface_volume_idcode,
            scene_stats: None,
            scene_geometry_stats: None,
            verbose,
            preload_enabled,
            preload_sender,
            preload_receiver,
            event_proxy,
            afni_options,
            afni_connection: None,
            afni_session: AfniNimlSession::new(),
            afni_recorder,
            afni_rgba_colors: None,
            afni_rgba_signatures: HashMap::new(),
            sent_crosshair_node: None,
            afni_crosshair_node: None,
            camera,
            view_cursor_position: None,
            pair_dragging: false,
            pair_drag_last_cursor: None,
            pair_drag_changed: false,
            modifiers: ModifiersState::empty(),
            mode_label: None,
            view_egui_ctx,
            view_egui_state,
            view_egui_renderer,
            view_pending_egui_textures: egui::TexturesDelta::default(),
            view_allocated_egui_textures: HashSet::new(),
            egui_ctx,
            egui_state,
            egui_renderer,
            pending_egui_textures: egui::TexturesDelta::default(),
            allocated_egui_textures: HashSet::new(),
            roi_egui_ctx,
            roi_egui_state,
            roi_egui_renderer,
            roi_pending_egui_textures: egui::TexturesDelta::default(),
            roi_allocated_egui_textures: HashSet::new(),
            graph_egui_ctx,
            graph_egui_state,
            graph_egui_renderer,
            graph_pending_egui_textures: egui::TexturesDelta::default(),
            graph_allocated_egui_textures: HashSet::new(),
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
        } else if let Some(pair) = initial_overlay_pair_paths {
            state.load_overlay_pair_paths(pair)?;
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
        if state.afni_options.connect_on_launch
            && let Err(error) = state.connect_afni_talk()
        {
            state.set_error(error);
        }

        Ok(state)
    }

    fn view_window(&self) -> &Window {
        &self.view_window
    }

    fn control_window(&self) -> &Window {
        &self.control_window
    }

    fn roi_control_window(&self) -> &Window {
        &self.roi_control_window
    }

    fn graph_window(&self) -> &Window {
        &self.graph_window
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

    fn resize_roi_control(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        self.roi_control_size = size;
        self.last_requested_roi_control_size = None;
        self.roi_control_config.width = size.width;
        self.roi_control_config.height = size.height;
        self.roi_control_surface
            .configure(&self.device, &self.roi_control_config);
    }

    fn resize_graph(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        self.graph_size = size;
        self.graph_config.width = size.width;
        self.graph_config.height = size.height;
        self.graph_surface
            .configure(&self.device, &self.graph_config);
    }

    fn view_input(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::ModifiersChanged(modifiers) = event {
            self.modifiers = modifiers.state();
            if !self.modifiers.control_key() && self.pair_dragging {
                self.finish_pair_drag();
            }
        }

        let egui_response = self
            .view_egui_state
            .on_window_event(&self.view_window, event);
        if egui_response.repaint {
            self.view_window.request_redraw();
        }
        if egui_response.consumed {
            return true;
        }

        match event {
            WindowEvent::ModifiersChanged(_) => false,
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
                let roi_draw_active = self
                    .roi_workspace
                    .active_draft()
                    .is_some_and(|draft| draft.draw_enabled || draft.fill_pending);
                if roi_draw_active {
                    if let Err(error) = self.handle_roi_draw_click_at_cursor() {
                        self.set_error(error);
                    }
                } else {
                    self.inspect_surface_at_cursor();
                }
                true
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyR) if self.modifiers.control_key() => {
                        self.set_roi_controller_open(true);
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyS) if self.modifiers.control_key() => {
                        self.set_surface_controller_visible(
                            !self.controller.panels.surface_controller_visible,
                        );
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyT) if self.modifiers.control_key() => {
                        if let Err(error) = self.force_resend_afni_surfaces() {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyT) => {
                        if let Err(error) = self.toggle_afni_talk() {
                            self.set_error(error);
                        }
                        true
                    }
                    PhysicalKey::Code(KeyCode::KeyC) => {
                        let mode = self.camera.toggle_mode();
                        self.controller.camera.mode = mode.into();
                        self.show_mode_label(mode);
                        true
                    }
                    PhysicalKey::Code(KeyCode::Space) => {
                        self.camera.reset();
                        self.controller.camera.note_reset();
                        true
                    }
                    PhysicalKey::Code(KeyCode::F5) => {
                        self.controller.display.background.toggle();
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
                    PhysicalKey::Code(KeyCode::KeyG) => {
                        if let Err(error) = self.open_graph_for_current_pick() {
                            self.set_error(error);
                        }
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
                        self.controller.camera.set_preset(ViewPreset::Left);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowRight) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Right);
                        self.controller.camera.set_preset(ViewPreset::Right);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowUp) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Top);
                        self.controller.camera.set_preset(ViewPreset::Top);
                        true
                    }
                    PhysicalKey::Code(KeyCode::ArrowDown) if self.modifiers.alt_key() => {
                        self.camera.set_preset(PresetOrientation::Bottom);
                        self.controller.camera.set_preset(ViewPreset::Bottom);
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

    fn roi_control_input(&mut self, event: &WindowEvent) -> InputResponse {
        let egui_response = self
            .roi_egui_state
            .on_window_event(&self.roi_control_window, event);

        InputResponse {
            consumed: egui_response.consumed,
            repaint: egui_response.repaint,
        }
    }

    fn graph_input(&mut self, event: &WindowEvent) -> InputResponse {
        let egui_response = self
            .graph_egui_state
            .on_window_event(&self.graph_window, event);

        InputResponse {
            consumed: egui_response.consumed,
            repaint: egui_response.repaint,
        }
    }

    fn update(&mut self) {
        let camera = self.camera.clone();
        self.update_render_uniforms_for_camera(&camera);
    }

    fn has_renderable_surface(&self) -> bool {
        self.surface_render_set.is_some() || self.surface_buffers.is_some()
    }

    fn scene_viewport_size(&self) -> PhysicalSize<u32> {
        if self.controller.panels.graph_window_open
            && let Some(pre_open_size) = self.graph_dock_pre_open_size
        {
            return PhysicalSize::new(
                self.view_size.width.max(1),
                pre_open_size.height.min(self.view_size.height).max(1),
            );
        }

        PhysicalSize::new(self.view_size.width.max(1), self.view_size.height.max(1))
    }

    fn scene_viewport_aspect(&self) -> f32 {
        let size = self.scene_viewport_size();
        size.width.max(1) as f32 / size.height.max(1) as f32
    }

    fn update_render_uniforms_for_camera(&mut self, camera: &Camera) {
        let aspect = self.scene_viewport_aspect();
        if let Some(render_set) = self.surface_render_set.as_ref() {
            for instance in &render_set.instances {
                self.queue.write_buffer(
                    &instance.uniform_buffer,
                    0,
                    &camera.uniform_bytes_with_model(aspect, instance.model_matrix),
                );
            }
        } else {
            self.queue
                .write_buffer(&self.uniform_buffer, 0, &camera.uniform_bytes(aspect));
        }
    }

    fn render_view(&mut self) -> RenderStatus {
        egui_winit::update_viewport_info(
            self.view_egui_state
                .egui_input_mut()
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default(),
            &self.view_egui_ctx,
            &self.view_window,
            false,
        );
        let raw_input = self.view_egui_state.take_egui_input(&self.view_window);
        let egui_ctx = self.view_egui_ctx.clone();
        let mut ui_actions = Vec::new();
        #[allow(deprecated)]
        let full_output = egui_ctx.run(raw_input, |ctx| {
            ui_actions = self.draw_view_overlay_ui(ctx);
        });
        self.view_repaint_at = repaint_delay_to_instant(&full_output);
        let actions_present = !ui_actions.is_empty();
        self.view_egui_state
            .handle_platform_output(&self.view_window, full_output.platform_output);
        self.apply_commands(ui_actions);
        if actions_present {
            self.view_window.request_redraw();
            self.control_window.request_redraw();
            if self.controller.panels.roi_controller_open {
                self.roi_control_window.request_redraw();
            }
            if self.controller.panels.graph_window_open {
                self.view_window.request_redraw();
            }
        }
        let paint_jobs = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.view_config.width, self.view_config.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        self.view_pending_egui_textures
            .append(full_output.textures_delta);

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

        let (needs_texture_repaint, retained_textures) = upload_pending_egui_textures(
            &self.device,
            &self.queue,
            &mut self.view_egui_renderer,
            &self.view_pending_egui_textures,
            &mut self.view_allocated_egui_textures,
        );
        let mut command_buffers = self.view_egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let egui_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("view egui render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            self.view_egui_renderer.render(
                &mut egui_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }

        free_pending_egui_textures(
            &mut self.view_egui_renderer,
            &self.view_pending_egui_textures,
            &mut self.view_allocated_egui_textures,
        );
        self.view_pending_egui_textures = retained_textures;
        if needs_texture_repaint {
            self.view_repaint_at = Some(Instant::now());
        }

        command_buffers.push(encoder.finish());
        self.queue.submit(command_buffers);
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
                    load: wgpu::LoadOp::Clear(self.controller.display.background.color()),
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
        let viewport_size = self.scene_viewport_size();
        render_pass.set_viewport(
            0.0,
            0.0,
            viewport_size.width as f32,
            viewport_size.height as f32,
            0.0,
            1.0,
        );
        render_pass.set_scissor_rect(0, 0, viewport_size.width, viewport_size.height);

        if let Some(render_set) = &self.surface_render_set {
            render_pass.set_pipeline(&self.render_pipeline);
            for instance in &render_set.instances {
                if !self
                    .controller
                    .display
                    .pair_visibility
                    .is_visible(&instance.side)
                {
                    continue;
                }
                render_pass.set_bind_group(0, &instance.bind_group, &[]);
                render_pass.set_vertex_buffer(0, instance.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(instance.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..instance.index_count, 0, 0..1);
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

    fn save_preset_montage_screenshot(&mut self) -> Result<()> {
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
    fn save_colormap_companion(&self, base: &ScreenshotImage, path: &Path) -> Result<()> {
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
    fn has_thresholded_overlay(&self) -> bool {
        self.overlay.is_some() && self.overlay_appearance.threshold.enabled
    }

    /// Builds a right-side panel the same height as `base` containing a vertical
    /// colorbar for the active overlay colormap (max at top, min at bottom).
    fn colorbar_panel_image(&self, base: &ScreenshotImage, background: [u8; 4]) -> ScreenshotImage {
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
            let color = sample_colormap(self.overlay_appearance.colormap, t);
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

    fn capture_standard_montage(&mut self) -> Result<ScreenshotImage> {
        let shots = standard_montage_shots();
        self.capture_montage_shots(&shots)
    }

    fn capture_paired_spec_montage(&mut self) -> Result<ScreenshotImage> {
        let original_layout = self.controller.display.pair_layout;
        let original_state = self.controller.display.pair_state;
        let shots = paired_spec_montage_shots();
        let result = self.capture_paired_montage_shots(&shots);

        self.apply_hemisphere_layout_state(original_layout, original_state)?;
        result
    }

    fn capture_paired_montage_shots(&mut self, shots: &[MontageShot]) -> Result<ScreenshotImage> {
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

    fn fit_camera_to_current_geometry(&self, camera: &mut Camera, padding: f32) {
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

    fn fit_camera_to_active_pair(&self, camera: &mut Camera, padding: f32) -> bool {
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
        self.control_repaint_at = repaint_delay_to_instant(&full_output);
        let repaint_delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.repaint_delay)
            .unwrap_or(Duration::MAX);
        // A panel action (load, toggle, camera/background change) alters the
        // 3D scene, so the view window needs to repaint too.
        let actions_present = !ui_actions.is_empty();
        self.egui_state
            .handle_platform_output(&self.control_window, full_output.platform_output);
        if repaint_delay != Duration::ZERO {
            self.fit_control_window(desired_control_size_points, full_output.pixels_per_point);
        }
        self.apply_commands(ui_actions);
        if actions_present {
            self.view_window.request_redraw();
            self.control_window.request_redraw();
            if self.controller.panels.roi_controller_open {
                self.roi_control_window.request_redraw();
            }
            if self.controller.panels.graph_window_open {
                self.view_window.request_redraw();
            }
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

        let (needs_texture_repaint, retained_textures) = upload_pending_egui_textures(
            &self.device,
            &self.queue,
            &mut self.egui_renderer,
            &self.pending_egui_textures,
            &mut self.allocated_egui_textures,
        );
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

        free_pending_egui_textures(
            &mut self.egui_renderer,
            &self.pending_egui_textures,
            &mut self.allocated_egui_textures,
        );
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

    fn render_roi_control(&mut self) -> RenderStatus {
        egui_winit::update_viewport_info(
            self.roi_egui_state
                .egui_input_mut()
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default(),
            &self.roi_egui_ctx,
            &self.roi_control_window,
            false,
        );
        let raw_input = self
            .roi_egui_state
            .take_egui_input(&self.roi_control_window);
        let egui_ctx = self.roi_egui_ctx.clone();
        let mut ui_actions = Vec::new();
        let mut desired_roi_control_size_points = egui::Vec2::ZERO;
        #[allow(deprecated)]
        let full_output = egui_ctx.run(raw_input, |ctx| {
            let output = self.draw_roi_control_ui(ctx);
            ui_actions = output.actions;
            desired_roi_control_size_points = output.desired_control_size_points;
        });
        self.roi_control_repaint_at = repaint_delay_to_instant(&full_output);
        let repaint_delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.repaint_delay)
            .unwrap_or(Duration::MAX);

        let actions_present = !ui_actions.is_empty();
        self.roi_egui_state
            .handle_platform_output(&self.roi_control_window, full_output.platform_output);
        if repaint_delay != Duration::ZERO {
            self.fit_roi_control_window(
                desired_roi_control_size_points,
                full_output.pixels_per_point,
            );
        }
        self.apply_commands(ui_actions);
        if actions_present {
            self.view_window.request_redraw();
            self.control_window.request_redraw();
            if self.controller.panels.roi_controller_open {
                self.roi_control_window.request_redraw();
            }
            if self.controller.panels.graph_window_open {
                self.view_window.request_redraw();
            }
        }

        let paint_jobs = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [
                self.roi_control_config.width,
                self.roi_control_config.height,
            ],
            pixels_per_point: full_output.pixels_per_point,
        };
        self.roi_pending_egui_textures
            .append(full_output.textures_delta);

        let output = match self.roi_control_surface.get_current_texture() {
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
                label: Some("ROI control render encoder"),
            });

        let (needs_texture_repaint, retained_textures) = upload_pending_egui_textures(
            &self.device,
            &self.queue,
            &mut self.roi_egui_renderer,
            &self.roi_pending_egui_textures,
            &mut self.roi_allocated_egui_textures,
        );
        let mut command_buffers = self.roi_egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let egui_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ROI control egui render pass"),
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

            self.roi_egui_renderer.render(
                &mut egui_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }

        free_pending_egui_textures(
            &mut self.roi_egui_renderer,
            &self.roi_pending_egui_textures,
            &mut self.roi_allocated_egui_textures,
        );
        self.roi_pending_egui_textures = retained_textures;
        if needs_texture_repaint {
            self.roi_control_repaint_at = Some(Instant::now());
        }

        command_buffers.push(encoder.finish());
        self.queue.submit(command_buffers);
        output.present();

        RenderStatus::Rendered
    }

    fn render_graph(&mut self) -> RenderStatus {
        egui_winit::update_viewport_info(
            self.graph_egui_state
                .egui_input_mut()
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default(),
            &self.graph_egui_ctx,
            &self.graph_window,
            false,
        );
        let raw_input = self.graph_egui_state.take_egui_input(&self.graph_window);
        let egui_ctx = self.graph_egui_ctx.clone();
        #[allow(deprecated)]
        let full_output = egui_ctx.run(raw_input, |ctx| {
            self.draw_graph_ui(ctx);
        });
        self.graph_repaint_at = repaint_delay_to_instant(&full_output);
        self.graph_egui_state
            .handle_platform_output(&self.graph_window, full_output.platform_output);

        let paint_jobs = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.graph_config.width, self.graph_config.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        self.graph_pending_egui_textures
            .append(full_output.textures_delta);

        let output = match self.graph_surface.get_current_texture() {
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
                label: Some("graph render encoder"),
            });

        let (needs_texture_repaint, retained_textures) = upload_pending_egui_textures(
            &self.device,
            &self.queue,
            &mut self.graph_egui_renderer,
            &self.graph_pending_egui_textures,
            &mut self.graph_allocated_egui_textures,
        );
        let mut command_buffers = self.graph_egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let egui_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("graph egui render pass"),
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

            self.graph_egui_renderer.render(
                &mut egui_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }

        free_pending_egui_textures(
            &mut self.graph_egui_renderer,
            &self.graph_pending_egui_textures,
            &mut self.graph_allocated_egui_textures,
        );
        self.graph_pending_egui_textures = retained_textures;
        if needs_texture_repaint {
            self.graph_repaint_at = Some(Instant::now());
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

        ControlUiOutput {
            actions,
            desired_control_size_points,
        }
    }

    fn draw_view_overlay_ui(&mut self, ctx: &egui::Context) -> Vec<ViewerCommand> {
        let mut actions = Vec::new();

        #[allow(deprecated)]
        egui::TopBottomPanel::top("main_menu_bar")
            .resizable(false)
            .show(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Open Surface...").clicked() {
                            actions.push(ViewerCommand::PickSurface);
                            ui.close();
                        }
                        if ui.button("Open Spec...").clicked() {
                            actions.push(ViewerCommand::PickSpec);
                            ui.close();
                        }
                        if ui.button("Open Surface Volume...").clicked() {
                            actions.push(ViewerCommand::PickSurfaceVolume);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(self.mesh.is_some(), egui::Button::new("Open Overlay..."))
                            .clicked()
                        {
                            actions.push(ViewerCommand::PickOverlay);
                            ui.close();
                        }
                        if ui
                            .add_enabled(self.mesh.is_some(), egui::Button::new("Open ROI..."))
                            .clicked()
                        {
                            actions.push(ViewerCommand::PickRoi);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                self.surface_buffers.is_some(),
                                egui::Button::new("Save View..."),
                            )
                            .clicked()
                        {
                            actions.push(ViewerCommand::SaveScreenshot);
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                self.surface_buffers.is_some(),
                                egui::Button::new("Save Montage..."),
                            )
                            .clicked()
                        {
                            actions.push(ViewerCommand::SaveMontage);
                            ui.close();
                        }
                    });

                    ui.menu_button("View", |ui| {
                        ui.label(format!("Mode: {}", self.camera.mode().label()));
                        ui.separator();
                        if ui.button("Reset").clicked() {
                            actions.push(ViewerCommand::ResetCamera);
                            ui.close();
                        }
                        if ui.button("Cycle Camera").clicked() {
                            actions.push(ViewerCommand::ToggleCameraMode);
                            ui.close();
                        }
                        if ui
                            .button(self.controller.display.background.next_label())
                            .clicked()
                        {
                            actions.push(ViewerCommand::ToggleBackground);
                            ui.close();
                        }
                        let mut anatomical_shading_visible =
                            self.controller.display.anatomical_shading_visible;
                        if ui
                            .add_enabled_ui(self.mesh.is_some(), |ui| {
                                ui.checkbox(&mut anatomical_shading_visible, "Anatomical Shading")
                            })
                            .inner
                            .changed()
                        {
                            actions.push(ViewerCommand::SetAnatomicalShadingVisible(
                                anatomical_shading_visible,
                            ));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Left").clicked() {
                            actions.push(ViewerCommand::Preset(ViewPreset::Left));
                            ui.close();
                        }
                        if ui.button("Right").clicked() {
                            actions.push(ViewerCommand::Preset(ViewPreset::Right));
                            ui.close();
                        }
                        if ui.button("Top").clicked() {
                            actions.push(ViewerCommand::Preset(ViewPreset::Top));
                            ui.close();
                        }
                        if ui.button("Bottom").clicked() {
                            actions.push(ViewerCommand::Preset(ViewPreset::Bottom));
                            ui.close();
                        }
                        ui.separator();
                        let mut overlay_visible = self.controller.overlay.visible;
                        if ui
                            .add_enabled_ui(self.overlay.is_some(), |ui| {
                                ui.checkbox(&mut overlay_visible, "Overlay Visible")
                            })
                            .inner
                            .changed()
                        {
                            actions.push(ViewerCommand::SetOverlayVisible(overlay_visible));
                            ui.close();
                        }
                        let can_layout_hemispheres = self.has_both_scene();
                        if ui
                            .add_enabled(can_layout_hemispheres, egui::Button::new("Close Pair"))
                            .clicked()
                        {
                            actions.push(ViewerCommand::HemisphereLayout(HemisphereLayout::Closed));
                            ui.close();
                        }
                        if ui
                            .add_enabled(can_layout_hemispheres, egui::Button::new("Open Pair"))
                            .clicked()
                        {
                            actions.push(ViewerCommand::HemisphereLayout(HemisphereLayout::Open));
                            ui.close();
                        }
                    });

                    ui.menu_button("Controllers", |ui| {
                        let mut surface_visible = self.controller.panels.surface_controller_visible;
                        if ui
                            .checkbox(
                                &mut surface_visible,
                                "Surface / Overlay Controller    Ctrl+S",
                            )
                            .changed()
                        {
                            actions
                                .push(ViewerCommand::SetSurfaceControllerVisible(surface_visible));
                            ui.close();
                        }
                        let mut roi_open = self.controller.panels.roi_controller_open;
                        if ui
                            .checkbox(&mut roi_open, "ROI Drawing Controller    Ctrl+R")
                            .changed()
                        {
                            actions.push(ViewerCommand::SetRoiControllerOpen(roi_open));
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                self.controller.interaction.pick.is_some(),
                                egui::Button::new("Graph Pick    G"),
                            )
                            .clicked()
                        {
                            actions.push(ViewerCommand::OpenGraphForPick);
                            ui.close();
                        }
                    });
                });
            });

        if self.controller.panels.graph_window_open {
            self.draw_graph_dock_ui(ctx, &mut actions);
        }

        self.draw_view_transient_label(ctx);

        actions
    }

    fn draw_graph_dock_ui(&self, ctx: &egui::Context, actions: &mut Vec<ViewerCommand>) {
        #[allow(deprecated)]
        egui::TopBottomPanel::bottom("graph_dock")
            .resizable(true)
            .default_height(GRAPH_DOCK_DEFAULT_HEIGHT_POINTS)
            .min_height(GRAPH_DOCK_MIN_HEIGHT_POINTS)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Graph").strong().color(accent_color()));
                    ui.separator();
                    ui.label(
                        egui::RichText::new("picked node overlay values").color(muted_color()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            actions.push(ViewerCommand::SetGraphWindowOpen(false));
                        }
                    });
                });
                ui.separator();
                self.draw_graph_contents(ui);
            });
    }

    fn draw_view_transient_label(&mut self, ctx: &egui::Context) {
        if let Some((text, remaining)) = self.active_mode_label() {
            // Ensure the label is cleared on time even with no further input.
            ctx.request_repaint_after(remaining);
            egui::Area::new(egui::Id::new("view_transient_label"))
                .anchor(egui::Align2::CENTER_TOP, [0.0, 48.0])
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(180))
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_min_width(128.0);
                            ui.vertical_centered(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(text)
                                            .size(18.0)
                                            .strong()
                                            .color(egui::Color32::WHITE),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Extend),
                                );
                            });
                        });
                });
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn draw_roi_control_ui(&mut self, ctx: &egui::Context) -> ControlUiOutput {
        let mut actions = Vec::new();
        let panel_height = (self.roi_control_size.height as f32 - 24.0).max(160.0);
        let mut desired_control_size_points = egui::vec2(
            ROI_CONTROL_CONTENT_WIDTH_POINTS + 24.0,
            ROI_CONTROL_MIN_INNER_HEIGHT as f32,
        );

        #[allow(deprecated)]
        egui::CentralPanel::default().show(ctx, |ui| {
            let scroll_output = egui::ScrollArea::vertical()
                .max_height(panel_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_min_width(ROI_CONTROL_CONTENT_WIDTH_POINTS);
                    self.draw_roi_control_contents(ui, &mut actions);
                });
            desired_control_size_points = egui::vec2(
                scroll_output
                    .content_size
                    .x
                    .max(ROI_CONTROL_CONTENT_WIDTH_POINTS)
                    + 32.0,
                scroll_output.content_size.y + 32.0,
            );
        });

        ControlUiOutput {
            actions,
            desired_control_size_points,
        }
    }

    fn draw_graph_ui(&self, ctx: &egui::Context) {
        #[allow(deprecated)]
        egui::CentralPanel::default().show(ctx, |ui| {
            self.draw_graph_contents(ui);
        });
    }

    fn draw_graph_contents(&self, ui: &mut egui::Ui) {
        ui.set_min_width(GRAPH_MIN_PLOT_WIDTH_POINTS);
        let Some(snapshot) = self.graph_snapshot.as_ref() else {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.35).max(24.0));
                ui.label(
                    egui::RichText::new("Pick a node, then press G")
                        .size(18.0)
                        .color(muted_color()),
                );
            });
            return;
        };

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Node").color(accent_color()));
            ui.monospace(snapshot.node_index.to_string());
            ui.separator();
            ui.label(egui::RichText::new("Surf x,y,z").color(accent_color()));
            ui.monospace(coordinate_label(snapshot.surface_position));
        });
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Surface").color(accent_color()));
            ui.monospace(truncate_middle(&snapshot.surface_label, 44));
            ui.separator();
            ui.label(egui::RichText::new("Overlay").color(accent_color()));
            ui.monospace(truncate_middle(&snapshot.overlay_label, 44));
        });
        ui.add_space(6.0);

        if snapshot.points.is_empty() {
            ui.label(
                egui::RichText::new("No numeric overlay columns are available for this node.")
                    .color(muted_color()),
            );
            return;
        }

        draw_graph_snapshot(ui, snapshot, self.overlay_columns);
    }

    fn draw_roi_control_contents(&mut self, ui: &mut egui::Ui, actions: &mut Vec<ViewerCommand>) {
        controller_section(ui, "ROI", true, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.mesh.is_some(), egui::Button::new("Open ROI"))
                    .clicked()
                {
                    actions.push(ViewerCommand::PickRoi);
                }
                if ui
                    .add_enabled(
                        self.roi_layer.is_some() || self.roi_workspace.has_saveable_rois(),
                        egui::Button::new("Clear"),
                    )
                    .clicked()
                {
                    actions.push(ViewerCommand::ClearRoi);
                }
                if ui
                    .add_enabled(
                        self.roi_workspace.has_saveable_rois(),
                        egui::Button::new("Save All"),
                    )
                    .on_hover_text("Save every ROI object in one .niml.roi file")
                    .clicked()
                {
                    actions.push(ViewerCommand::SaveAllRois);
                }
                let mut visible = self.controller.roi.visible;
                if ui
                    .add_enabled_ui(self.roi_layer.is_some(), |ui| {
                        ui.checkbox(&mut visible, "Visible")
                    })
                    .inner
                    .changed()
                {
                    actions.push(ViewerCommand::SetRoiVisible(visible));
                }
            });

            ui.add_space(8.0);
            egui::Grid::new("roi_controller_summary_grid")
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    stat_row(ui, "ROI", self.roi_display_text());
                    stat_row(ui, "Slots", self.roi_workspace.slots.len().to_string());
                    if let Some(layer) = self.roi_layer.as_ref() {
                        stat_row(ui, "Objects", layer.rois.len().to_string());
                        stat_row(ui, "Nodes", layer.mapped_nodes.to_string());
                    }
                });
        });

        ui.add_space(10.0);
        controller_section(ui, "ROI OBJECTS", true, |ui| {
            let slot_count = self.roi_workspace.slots.len();
            for index in 0..slot_count {
                ui.push_id(("roi_slot", index), |ui| {
                    let is_active = self.roi_workspace.active_index == index;
                    let slot = &mut self.roi_workspace.slots[index];
                    egui::Frame::new()
                        .stroke(egui::Stroke::new(1.0, border_color()))
                        .fill(panel_fill_color())
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let title = format!("ROI {}", index + 1);
                                let title = if is_active {
                                    format!("{title}  editing")
                                } else if slot.editing {
                                    title
                                } else {
                                    format!("{title}  finalized")
                                };
                                ui.label(egui::RichText::new(title).color(accent_color()));
                                ui.add_space(8.0);
                                let mut visible = slot.visible;
                                if ui.checkbox(&mut visible, "Visible").changed() {
                                    actions.push(ViewerCommand::SetRoiSlotVisible(index, visible));
                                }
                            });

                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label("Label");
                                if slot.editing {
                                    ui.text_edit_singleline(&mut slot.draft.label);
                                } else {
                                    ui.monospace(slot.label());
                                }
                                ui.label("Value");
                                if slot.editing {
                                    ui.add(
                                        egui::DragValue::new(&mut slot.draft.integer_label)
                                            .speed(1),
                                    );
                                } else {
                                    ui.monospace(slot.integer_label().to_string());
                                }
                            });

                            ui.add_space(6.0);
                            egui::Grid::new("roi_slot_summary_grid")
                                .num_columns(2)
                                .spacing([10.0, 4.0])
                                .show(ui, |ui| {
                                    stat_row(ui, "State", roi_slot_state_text(slot));
                                    stat_row(ui, "Draft", roi_draft_status_text(&slot.draft));
                                });

                            ui.add_space(8.0);
                            ui.horizontal_wrapped(|ui| {
                                if slot.editing {
                                    let draw_clicked = ui
                                        .add_enabled(
                                            self.mesh.is_some(),
                                            egui::Button::new("Draw")
                                                .selected(is_active && slot.draft.draw_enabled),
                                        )
                                        .on_hover_text(
                                            "Right-click the surface to add ROI anchor points",
                                        )
                                        .clicked();
                                    if draw_clicked {
                                        actions.push(ViewerCommand::ToggleRoiDraw(
                                            index,
                                            !slot.draft.draw_enabled,
                                        ));
                                    }
                                    if ui
                                        .add_enabled(slot.draft.can_join(), egui::Button::new("Join"))
                                        .on_hover_text(
                                            "Close the ROI by joining the last point back to the first",
                                        )
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::JoinRoiDraft(index));
                                    }
                                    if ui
                                        .add_enabled(slot.draft.can_fill(), egui::Button::new("Fill"))
                                        .on_hover_text(
                                            "Right-click inside or outside the closed ROI to define the fill",
                                        )
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::ArmRoiFill(index));
                                    }
                                    if ui
                                        .add_enabled(slot.draft.can_undo(), egui::Button::new("Undo"))
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::UndoRoiDraft(index));
                                    }
                                    if ui
                                        .add_enabled(slot.draft.can_redo(), egui::Button::new("Redo"))
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::RedoRoiDraft(index));
                                    }
                                    if ui
                                        .add_enabled(!slot.draft.is_empty(), egui::Button::new("Finalize"))
                                        .on_hover_text("Finish this ROI and start a new one")
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::FinalizeRoiSlot(index));
                                    }
                                } else {
                                    if ui.button("Edit").clicked() {
                                        actions.push(ViewerCommand::EditRoiSlot(index));
                                    }
                                    if ui
                                        .add_enabled(slot.has_roi(), egui::Button::new("Delete"))
                                        .on_hover_text("Remove only this ROI object")
                                        .clicked()
                                    {
                                        actions.push(ViewerCommand::DeleteRoiSlot(index));
                                    }
                                }

                                if ui
                                    .add_enabled(slot.has_roi(), egui::Button::new("Save"))
                                    .on_hover_text("Save only this ROI object")
                                    .clicked()
                                {
                                    actions.push(ViewerCommand::SaveRoiSlot(index));
                                }
                            });
                        });
                    ui.add_space(8.0);
                });
            }
        });
    }

    fn draw_surface_dataset_section(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<ViewerCommand>,
    ) {
        controller_section(ui, "SURFACE / DATASET", true, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Open:");
                if ui
                    .button("Surf")
                    .on_hover_text("Open GIFTI surface")
                    .clicked()
                {
                    actions.push(ViewerCommand::PickSurface);
                }
                if ui
                    .add_enabled(self.mesh.is_some(), egui::Button::new("Olay"))
                    .on_hover_text("Open overlay dataset")
                    .clicked()
                {
                    actions.push(ViewerCommand::PickOverlay);
                }
                if ui
                    .add_enabled(self.mesh.is_some(), egui::Button::new("ROI"))
                    .on_hover_text("Open SUMA ROI")
                    .clicked()
                {
                    actions.push(ViewerCommand::PickRoi);
                }
                if ui.button("Spec").on_hover_text("Open SUMA spec").clicked() {
                    actions.push(ViewerCommand::PickSpec);
                }
                if ui
                    .button("SV")
                    .on_hover_text("Open surface volume")
                    .clicked()
                {
                    actions.push(ViewerCommand::PickSurfaceVolume);
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
                            actions.push(ViewerCommand::SelectSceneSurface(selected_index));
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

    fn draw_overlay_workbench(&mut self, ui: &mut egui::Ui, actions: &mut Vec<ViewerCommand>) {
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
                        if let Some(q_value) = self.selected_threshold_q_value() {
                            ui.label(
                                egui::RichText::new(threshold_q_value_display(q_value))
                                    .color(muted_color()),
                            );
                        }
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
            actions.push(ViewerCommand::RefreshOverlayColumns);
        }
        if changed {
            self.sanitize_overlay_appearance();
            actions.push(ViewerCommand::RefreshOverlayAppearance);
        }
    }

    fn draw_overlay_range_controls(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            changed |= ui
                .checkbox(&mut self.controller.overlay.symmetric_range, "Symmetric")
                .changed();

            if self.controller.overlay.symmetric_range {
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

    fn selected_threshold_q_value(&self) -> Option<f64> {
        let dataset = self.overlay_dataset.as_ref()?;
        let index = self.overlay_columns.threshold?;
        let column = dataset.columns.get(index)?;
        column
            .fdr_curve
            .as_ref()?
            .q_value(self.overlay_appearance.threshold.value as f64)
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
                    if let Some(pick) = self.controller.interaction.pick {
                        stat_row(ui, "Node", pick.node_index.to_string());
                        stat_row(ui, "Triangle", pick.face_index.to_string());
                        stat_row(ui, "Surf x,y,z", coordinate_label(pick.surface_position));
                        stat_row(ui, "Overlay Value", picked_overlay_value_label(pick));
                        stat_row(ui, "ROI", self.pick_roi_display_text(pick));
                    }
                });
            if self.controller.interaction.pick.is_none() {
                ui.label(egui::RichText::new("No pick").color(muted_color()));
            }
        });
    }

    fn sanitize_overlay_appearance(&mut self) {
        let range = &mut self.overlay_appearance.range;
        if !range.min.is_finite() || !range.max.is_finite() {
            *range = DEFAULT_OVERLAY_RANGE;
        }

        if self.controller.overlay.symmetric_range {
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
        let Some(desired_size) = desired_panel_size(
            &self.control_window,
            desired_control_size_points,
            pixels_per_point,
            CONTROL_MIN_INNER_WIDTH,
            CONTROL_MIN_INNER_HEIGHT,
            0.55,
            CONTROL_MAX_INNER_WIDTH,
            0.85,
            960,
        ) else {
            return;
        };
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

    fn fit_roi_control_window(
        &mut self,
        desired_control_size_points: egui::Vec2,
        pixels_per_point: f32,
    ) {
        let Some(desired_size) = desired_panel_size(
            &self.roi_control_window,
            desired_control_size_points,
            pixels_per_point,
            ROI_CONTROL_MIN_INNER_WIDTH,
            ROI_CONTROL_MIN_INNER_HEIGHT,
            0.65,
            ROI_CONTROL_MAX_INNER_WIDTH,
            0.45,
            520,
        ) else {
            return;
        };
        if size_is_close(self.roi_control_size, desired_size) {
            return;
        }
        if self.last_requested_roi_control_size == Some(desired_size) {
            return;
        }
        self.last_requested_roi_control_size = Some(desired_size);
        if let Some(actual_size) = self.roi_control_window.request_inner_size(desired_size) {
            self.resize_roi_control(actual_size);
        }
        self.roi_control_window.request_redraw();
    }

    fn apply_commands(&mut self, actions: Vec<ViewerCommand>) {
        for action in actions {
            match action {
                ViewerCommand::PickSurface => {
                    if let Some(path) = pick_surface_file(self.surface_path.as_ref()) {
                        if let Err(error) = self.load_surface_path(path) {
                            self.set_error(error);
                        }
                    }
                }
                ViewerCommand::PickOverlay => {
                    if let Some(path) =
                        pick_overlay_file(self.overlay_path.as_ref().or(self.surface_path.as_ref()))
                    {
                        if let Err(error) = self.load_overlay_path(path) {
                            self.set_error(error);
                        }
                    }
                }
                ViewerCommand::PickRoi => {
                    if let Some(path) =
                        pick_roi_file(self.roi_path.as_ref().or(self.surface_path.as_ref()))
                    {
                        if let Err(error) = self.load_roi_path(path) {
                            self.set_error(error);
                        }
                    }
                }
                ViewerCommand::PickSpec => {
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
                ViewerCommand::PickSurfaceVolume => {
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
                ViewerCommand::RefreshOverlayColumns => {
                    if let Err(error) = self.refresh_overlay_columns() {
                        self.set_error(error);
                    }
                }
                ViewerCommand::RefreshOverlayAppearance => {
                    if let Err(error) = self.refresh_overlay_appearance() {
                        self.set_error(error);
                    }
                }
                ViewerCommand::ResetCamera => {
                    self.camera.reset();
                    self.controller.camera.note_reset();
                }
                ViewerCommand::ToggleCameraMode => {
                    let mode = self.camera.toggle_mode();
                    self.controller.camera.mode = mode.into();
                    self.show_mode_label(mode);
                }
                ViewerCommand::ToggleBackground => self.controller.display.background.toggle(),
                ViewerCommand::SetAnatomicalShadingVisible(visible) => {
                    self.controller.display.anatomical_shading_visible = visible;
                    self.upload_surface_buffers();
                    self.update_scene_stats();
                    self.log_status(if visible {
                        "Anatomical shading visible."
                    } else {
                        "Anatomical shading hidden."
                    });
                }
                ViewerCommand::SetOverlayVisible(visible) => {
                    if self.overlay.is_some() {
                        self.controller.overlay.visible = visible;
                        self.upload_surface_buffers();
                        self.update_scene_stats();
                        self.log_status(if visible {
                            "Overlay visible."
                        } else {
                            "Overlay hidden."
                        });
                    }
                }
                ViewerCommand::SetRoiVisible(visible) => {
                    if self.roi_layer.is_some() {
                        self.controller.roi.visible = visible;
                        self.upload_surface_buffers();
                        self.update_scene_stats();
                        self.log_status(if visible {
                            "ROI visible."
                        } else {
                            "ROI hidden."
                        });
                    }
                }
                ViewerCommand::SetRoiSlotVisible(index, visible) => {
                    if let Some(slot) = self.roi_workspace.slots.get_mut(index) {
                        slot.visible = visible;
                        if let Err(error) = self.rebuild_roi_layer_from_state() {
                            self.set_error(error);
                        } else {
                            self.upload_surface_buffers();
                            self.update_scene_stats();
                        }
                    }
                }
                ViewerCommand::ClearRoi => {
                    if self.roi_layer.is_some()
                        || self.roi_path.is_some()
                        || self.roi_workspace.has_saveable_rois()
                    {
                        self.roi_layer = None;
                        self.roi_path = None;
                        self.controller.surface.current_roi_path = None;
                        self.roi_workspace.clear();
                        self.controller.roi.visible = true;
                        self.upload_surface_buffers();
                        self.update_scene_stats();
                        self.log_status("ROI cleared.");
                    }
                }
                ViewerCommand::ToggleRoiDraw(index, active) => {
                    if self.mesh.is_some() && self.roi_workspace.set_active(index) {
                        self.controller.roi.active_slot = index;
                        if let Some(draft) = self.roi_workspace.active_draft_mut() {
                            draft.draw_enabled = active;
                            draft.fill_pending = false;
                        }
                        if active {
                            self.log_status("ROI draw on. Right-click the surface to add points.");
                        } else {
                            self.log_status("ROI draw off.");
                        }
                    }
                }
                ViewerCommand::JoinRoiDraft(index) => {
                    self.roi_workspace.set_active(index);
                    self.controller.roi.active_slot = index;
                    if let Err(error) = self.join_roi_draft() {
                        self.set_error(error);
                    }
                }
                ViewerCommand::ArmRoiFill(index) => {
                    self.roi_workspace.set_active(index);
                    self.controller.roi.active_slot = index;
                    if let Some(draft) = self.roi_workspace.active_draft_mut()
                        && draft.can_fill()
                    {
                        draft.fill_pending = true;
                        draft.draw_enabled = true;
                        self.log_status(
                            "ROI fill armed. Right-click inside or outside the closed path.",
                        );
                    } else {
                        self.log_status("Join the ROI before filling it.");
                    }
                }
                ViewerCommand::UndoRoiDraft(index) => {
                    self.roi_workspace.set_active(index);
                    self.controller.roi.active_slot = index;
                    let changed = self
                        .roi_workspace
                        .active_draft_mut()
                        .is_some_and(RoiDraft::undo);
                    if changed {
                        if let Err(error) = self.rebuild_roi_layer_from_state() {
                            self.set_error(error);
                        } else {
                            self.sync_pick_to_roi_draft_anchor();
                            self.upload_surface_buffers();
                            self.update_scene_stats();
                            self.log_status("ROI undo.");
                        }
                    }
                }
                ViewerCommand::RedoRoiDraft(index) => {
                    self.roi_workspace.set_active(index);
                    self.controller.roi.active_slot = index;
                    let changed = self
                        .roi_workspace
                        .active_draft_mut()
                        .is_some_and(RoiDraft::redo);
                    if changed {
                        if let Err(error) = self.rebuild_roi_layer_from_state() {
                            self.set_error(error);
                        } else {
                            self.sync_pick_to_roi_draft_anchor();
                            self.upload_surface_buffers();
                            self.update_scene_stats();
                            self.log_status("ROI redo.");
                        }
                    }
                }
                ViewerCommand::FinalizeRoiSlot(index) => {
                    match self.roi_workspace.finalize_slot(index) {
                        Ok(true) => {
                            if let Err(error) = self.rebuild_roi_layer_from_state() {
                                self.set_error(error);
                            } else {
                                self.controller.interaction.set_pick(None);
                                self.upload_surface_buffers();
                                self.update_scene_stats();
                                self.log_status("ROI finalized. Started a new ROI slot.");
                            }
                        }
                        Ok(false) => self.log_status("No ROI draft is available to finalize."),
                        Err(error) => self.set_error(error),
                    }
                }
                ViewerCommand::EditRoiSlot(index) => match self.roi_workspace.edit_slot(index) {
                    Ok(true) => {
                        self.controller.roi.active_slot = index;
                        self.sync_pick_to_roi_draft_anchor();
                        self.log_status(format!("Editing ROI {}.", index + 1));
                    }
                    Ok(false) => {
                        self.log_status("This ROI cannot be edited as a Sumaru draft yet.");
                    }
                    Err(error) => self.set_error(error),
                },
                ViewerCommand::DeleteRoiSlot(index) => {
                    if self.roi_workspace.delete_slot(index) {
                        if let Err(error) = self.rebuild_roi_layer_from_state() {
                            self.set_error(error);
                        } else {
                            if !self.roi_workspace.has_saveable_rois() {
                                self.roi_path = None;
                                self.controller.surface.current_roi_path = None;
                            }
                            self.controller.interaction.set_pick(None);
                            self.upload_surface_buffers();
                            self.update_scene_stats();
                            self.log_status(format!("Deleted ROI {}.", index + 1));
                        }
                    }
                }
                ViewerCommand::SaveRoiSlot(index) => {
                    if let Err(error) = self.save_roi_slot(index) {
                        self.set_error(error);
                    }
                }
                ViewerCommand::SaveAllRois => {
                    if let Err(error) = self.save_all_rois() {
                        self.set_error(error);
                    }
                }
                ViewerCommand::SetSurfaceControllerVisible(visible) => {
                    self.set_surface_controller_visible(visible);
                }
                ViewerCommand::SetRoiControllerOpen(open) => {
                    self.set_roi_controller_open(open);
                }
                ViewerCommand::OpenGraphForPick => {
                    if let Err(error) = self.open_graph_for_current_pick() {
                        self.set_error(error);
                    }
                }
                ViewerCommand::SetGraphWindowOpen(open) => {
                    self.set_graph_window_open(open);
                }
                ViewerCommand::Preset(preset) => {
                    self.controller.camera.set_preset(preset);
                    self.camera.set_preset(preset.into());
                }
                ViewerCommand::HemisphereLayout(layout) => {
                    if let Err(error) = self.set_hemisphere_layout(layout) {
                        self.set_error(error);
                    }
                }
                ViewerCommand::SelectSceneSurface(index) => {
                    if let Err(error) = self.activate_scene_surface(index) {
                        self.set_error(error);
                    }
                }
                ViewerCommand::SaveScreenshot => {
                    if let Err(error) = self.save_current_view_screenshot() {
                        self.set_error(error);
                    }
                }
                ViewerCommand::SaveMontage => {
                    if let Err(error) = self.save_preset_montage_screenshot() {
                        self.set_error(error);
                    }
                }
            }
        }
    }

    fn reset_scene_state(&mut self) {
        self.overlay = None;
        self.overlay_values = None;
        self.overlay_dataset = None;
        self.overlay_columns = OverlayColumnSelections::default();
        self.controller.overlay.visible = true;
        self.overlay_appearance = OverlayAppearance::from_range(DEFAULT_OVERLAY_RANGE);
        self.controller.overlay.symmetric_range = true;
        self.controller.overlay.intensity_range = None;
        self.controller.overlay.threshold = None;
        self.controller.overlay.opacity = self.overlay_appearance.opacity;
        self.overlay_path = None;
        self.overlay_pair_paths = None;
        self.overlay_display_name = None;
        self.afni_rgba_colors = None;
        self.afni_rgba_signatures.clear();
        self.controller.surface.current_overlay_path = None;
        self.roi_path = None;
        self.controller.surface.current_roi_path = None;
        self.roi_layer = None;
        self.roi_workspace.clear();
        self.graph_snapshot = None;
        self.set_graph_window_open(false);
        self.controller.roi.visible = true;
        self.controller.interaction.set_pick(None);
        self.controller.display.pair_visibility = PairVisibility::both();
        self.surface_render_set = None;
    }

    fn load_surface_path(&mut self, path: PathBuf) -> Result<()> {
        let mut mesh = SurfaceMesh::from_gifti_path(&path)
            .with_context(|| format!("failed to load surface {}", path.display()))?;
        apply_surface_volume_parent(&mut mesh, self.surface_volume_idcode.as_deref());
        let node_count = mesh.vertices.len();
        let face_count = mesh.triangles.len();

        self.set_active_mesh(mesh, None);
        self.scene_generation = self.scene_generation.wrapping_add(1);
        self.surface_scene = None;
        self.surface_path = Some(path.clone());
        self.reset_scene_state();
        self.controller.surface.current_surface_id =
            self.mesh.as_ref().map(|mesh| mesh.metadata.id.clone());
        self.controller.surface.current_surface_path = Some(path.clone());
        self.controller.surface.current_scene_surface_index = None;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.camera.reset();
        self.controller.camera.note_reset();
        self.view_window
            .set_title(&window_title(self.surface_path.as_ref()));
        self.log_status(format!(
            "Loaded surface with {node_count} nodes and {face_count} triangles."
        ));
        if self.afni_connection.is_some()
            && let Err(error) = self.force_resend_afni_surfaces()
        {
            self.set_error(error);
        }

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
        let surface_volume_idcode =
            query_afni_dataset_idcode_optional(Some(surface_volume_path.as_path()))?;
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
        self.surface_volume_idcode = surface_volume_idcode.clone();
        self.controller.surface.current_surface_volume_path = Some(surface_volume_path.clone());
        self.surface_scene = Some(SurfaceScene {
            spec: spec.clone(),
            spec_path: spec.path.clone(),
            surface_volume_path: Some(surface_volume_path.clone()),
            surface_volume_idcode: surface_volume_idcode.clone(),
            hemisphere: spec.hemisphere,
            surfaces,
            active_index: 0,
            skipped_surfaces,
            skipped_states,
        });
        self.reset_scene_state();
        self.ensure_scene_surface_loaded(0)?;
        self.activate_scene_surface(0)?;
        self.start_scene_preload(generation);
        self.camera.reset();
        self.controller.camera.note_reset();
        self.log_status(format!(
            "Loaded {loaded_count} {loaded_label} from spec {} (skipped {skipped_surfaces} files, {skipped_states} states).",
            spec.path.display()
        ));

        Ok(())
    }

    fn set_surface_volume_path(&mut self, path: PathBuf) -> Result<()> {
        let path = canonical_or_original_path(path);
        let idcode = query_afni_dataset_idcode_optional(Some(path.as_path()))?;
        self.surface_volume_path = Some(path.clone());
        self.surface_volume_idcode = idcode.clone();
        self.controller.surface.current_surface_volume_path = Some(path.clone());

        if let Some(scene) = self.surface_scene.as_mut() {
            scene.surface_volume_path = Some(path.clone());
            scene.surface_volume_idcode = idcode.clone();
            for surface in &mut scene.surfaces {
                surface.display_cache = None;
                for component in &mut surface.components {
                    if let Some(mesh) = component.mesh.as_mut() {
                        apply_surface_volume_parent(mesh, idcode.as_deref());
                    }
                }
            }
        }

        if let Some(mesh) = self.mesh.as_mut() {
            apply_surface_volume_parent(mesh, idcode.as_deref());
        }

        self.log_status(format!(
            "Surface volume set to {}{}.",
            path.display(),
            idcode
                .as_deref()
                .map(|idcode| format!(" (AFNI idcode {idcode})"))
                .unwrap_or_else(|| "; AFNI idcode unavailable".to_string())
        ));
        if self.afni_connection.is_some()
            && let Err(error) = self.force_resend_afni_surfaces()
        {
            self.set_error(error);
        }

        Ok(())
    }

    fn toggle_afni_talk(&mut self) -> Result<()> {
        if self.afni_connection.is_some() {
            self.disconnect_afni_talk();
            return Ok(());
        }

        self.connect_afni_talk()
    }

    fn connect_afni_talk(&mut self) -> Result<()> {
        if self.afni_connection.is_some() {
            self.log_status("AFNI/SUMA NIML talk is already connected.");
            return Ok(());
        }

        let config = self.afni_options.port_config.clone();
        let event_proxy = self.event_proxy.clone();
        let connection = AfniConnection::connect(
            &config,
            self.verbose,
            self.afni_recorder.clone(),
            move || {
                let _ = event_proxy.send_event(ViewerEvent::AfniMessagesReady);
            },
        )
        .with_context(|| {
            format!(
                "failed to connect to AFNI/SUMA NIML talk at {}:{}",
                config.host, config.port
            )
        })?;
        self.afni_connection = Some(connection);
        self.afni_session = AfniNimlSession::new();
        self.log_status(format!(
            "Connected AFNI/SUMA NIML talk at {}:{}.",
            config.host, config.port
        ));
        self.send_afni_surfaces(false)
    }

    fn disconnect_afni_talk(&mut self) {
        if let Some(mut connection) = self.afni_connection.take() {
            connection.disconnect();
            self.afni_session = AfniNimlSession::new();
            self.log_status("Disconnected AFNI/SUMA NIML talk.");
        }
    }

    fn force_resend_afni_surfaces(&mut self) -> Result<()> {
        if self.afni_connection.is_none() {
            self.connect_afni_talk()?;
            return Ok(());
        }

        self.afni_session = AfniNimlSession::new();
        self.send_afni_surfaces(true)
    }

    fn send_afni_surfaces(&mut self, force: bool) -> Result<()> {
        if self.afni_connection.is_none() {
            return Ok(());
        }
        if self.mesh.is_none() {
            self.log_status("AFNI/SUMA NIML talk connected; no surface is loaded yet.");
            return Ok(());
        }

        if force {
            self.afni_session = AfniNimlSession::new();
        }

        self.ensure_scene_surfaces_loaded_for_afni()?;
        let exports = self.afni_surface_exports()?;
        let mut sent_count = 0usize;
        for (mesh, info) in exports {
            let Some(elements) = self.afni_session.register_surface_once(&mesh, &info)? else {
                continue;
            };
            let connection = self
                .afni_connection
                .as_mut()
                .context("AFNI/SUMA NIML talk is not connected")?;
            for element in elements {
                if let Err(error) = connection.send_elements(std::slice::from_ref(&element)) {
                    self.disconnect_afni_talk();
                    return Err(error.context("AFNI/SUMA NIML write failed"));
                }
            }
            sent_count += 1;
        }

        if sent_count > 0 {
            self.log_status(format!(
                "Sent {sent_count} surface registration{} to AFNI/SUMA.",
                if sent_count == 1 { "" } else { "s" }
            ));
        } else if force {
            self.log_status("No new surface geometry needed to be sent to AFNI/SUMA.");
        }

        // Nudge AFNI to redraw the overlay for the freshly registered surfaces
        // so the connection is visibly live without waiting for the user to
        // click. Prefer the current/last crosshair location to avoid moving
        // AFNI's focus on a plain surface switch.
        self.send_afni_redraw_crosshair();

        Ok(())
    }

    /// Send a `SUMA_crosshair_xyz` to prompt AFNI to resend its colorization for
    /// the active surfaces. Targets, in order: the current selection, the last
    /// crosshair we sent, AFNI's most recently reported crosshair, and finally
    /// the node nearest the brain's center to trigger an initial draw when none
    /// of those exist yet.
    fn send_afni_redraw_crosshair(&mut self) {
        if self.afni_connection.is_none() {
            return;
        }
        let Some(mesh) = self.mesh.as_ref() else {
            return;
        };
        if mesh.vertices.is_empty() {
            return;
        }
        let node = self
            .controller
            .interaction
            .pick
            .map(|pick| pick.node_index)
            .or(self.sent_crosshair_node)
            .or(self.afni_crosshair_node)
            .or_else(|| node_nearest_bounds_center(mesh))
            .unwrap_or(0);
        let Some(pick) = surface_pick_for_mesh_node(mesh, self.overlay_values.as_ref(), node)
        else {
            return;
        };
        if let Err(error) = self.send_afni_crosshair_for_pick(pick) {
            self.set_error(error);
        }
    }

    fn afni_surface_exports(&self) -> Result<Vec<(SurfaceMesh, AfniSurfaceInfo)>> {
        if let Some(scene) = self.surface_scene.as_ref() {
            let mut exports = Vec::new();
            for surface in &scene.surfaces {
                for component in &surface.components {
                    let Some(mesh) = component.mesh.as_ref() else {
                        continue;
                    };
                    if !afni_component_is_sendable(component, Some(mesh)) {
                        continue;
                    }
                    let mut info = AfniSurfaceInfo::from_mesh(mesh);
                    decorate_afni_surface_info(&mut info, Some(scene), Some(component));
                    exports.push((mesh.clone(), info));
                }
            }
            if !exports.is_empty() {
                return Ok(exports);
            }
        }

        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before connecting to AFNI/SUMA NIML talk")?;
        let mut info = AfniSurfaceInfo::from_mesh(mesh);
        decorate_afni_surface_info(&mut info, self.surface_scene.as_ref(), None);
        decorate_afni_surface_volume_info(
            &mut info,
            self.surface_volume_path.as_ref(),
            self.surface_volume_idcode.as_deref(),
        );
        Ok(vec![(mesh.clone(), info)])
    }

    fn drain_afni_events(&mut self) -> bool {
        let mut events = Vec::new();
        if let Some(connection) = self.afni_connection.as_ref() {
            while let Some(event) = connection.try_recv() {
                events.push(event);
            }
        }

        let mut changed = false;
        for event in events {
            match event {
                AfniConnectionEvent::Elements(elements) => {
                    match self.handle_afni_elements(elements) {
                        Ok(event_changed) => changed |= event_changed,
                        Err(error) => self.set_error(error),
                    }
                }
                AfniConnectionEvent::Error(message) => {
                    self.log_status(format!("AFNI/SUMA NIML talk error: {message}"));
                }
                AfniConnectionEvent::Disconnected => {
                    self.afni_connection = None;
                    self.afni_session = AfniNimlSession::new();
                    self.log_status("AFNI/SUMA NIML talk disconnected.");
                    changed = true;
                }
            }
        }

        changed
    }

    fn handle_afni_elements(&mut self, elements: Vec<NimlElement>) -> Result<bool> {
        let mut actions = Vec::new();
        let mut changed = false;
        for element in elements {
            let Some(outcome) = self
                .afni_session
                .receive_element(&mut self.controller, &element)?
            else {
                if self.verbose {
                    self.log_status(format!("Ignored AFNI/SUMA NIML element {}.", element.name));
                }
                continue;
            };
            changed |= outcome.applied_state;
            actions.extend(outcome.actions);
        }

        for action in actions {
            changed |= self.apply_afni_route_action(action)?;
        }

        Ok(changed)
    }

    fn apply_afni_route_action(&mut self, action: AfniRouteAction) -> Result<bool> {
        match action {
            AfniRouteAction::ViewerCommand(command) => {
                self.apply_commands(vec![command]);
                Ok(true)
            }
            AfniRouteAction::LoadDataset(path) => {
                self.load_overlay_path(path)?;
                Ok(true)
            }
            AfniRouteAction::RgbaOverlay(overlay) => self.apply_afni_rgba_overlay(overlay),
            AfniRouteAction::SurfaceCrosshair(crosshair) => {
                self.apply_afni_surface_crosshair(crosshair)
            }
            AfniRouteAction::RoiUpdate(update) => {
                if let Some(visible) = update.visible {
                    self.apply_commands(vec![ViewerCommand::SetRoiVisible(visible)]);
                }
                if let Some(path) = update.path {
                    self.load_roi_path(path)?;
                }
                Ok(true)
            }
        }
    }

    fn apply_afni_rgba_overlay(&mut self, overlay: AfniRgbaOverlay) -> Result<bool> {
        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before applying AFNI/SUMA RGBA overlay")?;
        let Some(target) = self.afni_surface_target_for_message(
            &overlay.surface_idcode,
            overlay.local_domain_parent_id.as_deref(),
        ) else {
            self.log_status(format!(
                "Ignored AFNI/SUMA RGBA overlay for unknown surface {}{}.",
                overlay.surface_idcode,
                overlay
                    .local_domain_parent_id
                    .as_deref()
                    .map(|parent| format!(" (domain parent {parent})"))
                    .unwrap_or_default()
            ));
            return Ok(false);
        };

        // AFNI resends an identical irgba colorization on every redraw. When the
        // payload for this surface matches the one we last applied, skip the
        // whole O(n) recolor + vertex re-upload and report "no change" so we
        // don't even trigger a redraw.
        let signature = afni_rgba_overlay_signature(&overlay);
        let previous_signature = self
            .afni_rgba_signatures
            .get(&overlay.surface_idcode)
            .copied();
        if self.afni_rgba_colors.is_some() && previous_signature == Some(signature) {
            if self.verbose {
                self.log_status(format!(
                    "Skipped unchanged AFNI/SUMA RGBA overlay for {}.",
                    overlay.surface_idcode
                ));
            }
            return Ok(false);
        }

        let (colors, applied, skipped) =
            apply_afni_rgba_to_color_cache(self.afni_rgba_colors.take(), mesh, target, &overlay);

        let dataset_id = overlay
            .function_idcode
            .clone()
            .or_else(|| Some("AFNI SUMA_irgba".to_string()));
        let overlay_model = Overlay::from_color_cache(&mesh.domain, colors.clone(), dataset_id)?;
        self.afni_rgba_colors = Some(colors);
        self.overlay = Some(overlay_model);
        self.overlay_values = None;
        self.overlay_dataset = None;
        self.overlay_columns = OverlayColumnSelections::default();
        self.overlay_path = None;
        self.overlay_pair_paths = None;
        self.controller.surface.current_overlay_path = None;
        self.overlay_display_name = Some("AFNI SUMA_irgba".to_string());
        self.controller.overlay.visible = true;
        // AFNI bakes its threshold into the colors it sends (sub-threshold nodes
        // are simply absent from the sparse list), so we do not re-apply a
        // scalar threshold to this already-resolved color cache.
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.afni_rgba_signatures
            .insert(overlay.surface_idcode.clone(), signature);
        self.log_status(format!(
            "Applied AFNI/SUMA RGBA overlay to {applied} nodes{}.",
            if skipped > 0 {
                format!(" ({skipped} skipped)")
            } else {
                String::new()
            }
        ));
        if self.verbose {
            self.log_status(match previous_signature {
                Some(_) => format!(
                    "AFNI/SUMA RGBA overlay for {} changed; re-applied.",
                    overlay.surface_idcode
                ),
                None => format!(
                    "AFNI/SUMA RGBA overlay for {} applied for the first time.",
                    overlay.surface_idcode
                ),
            });
        }

        Ok(true)
    }

    fn apply_afni_surface_crosshair(&mut self, crosshair: AfniSurfaceCrosshair) -> Result<bool> {
        let Some(local_node) = crosshair.node_index else {
            self.log_status(format!(
                "AFNI/SUMA crosshair at {} did not include a surface node id.",
                coordinate_label(crosshair.surface_position)
            ));
            return Ok(false);
        };
        let node_offset = crosshair
            .surface_idcode
            .as_deref()
            .and_then(|surface_idcode| self.afni_node_offset_for_surface(surface_idcode))
            .unwrap_or(0);
        let node_index = node_offset
            .checked_add(local_node as usize)
            .and_then(|node| u32::try_from(node).ok())
            .context("AFNI/SUMA crosshair node index is outside Sumaru node range")?;
        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before applying AFNI/SUMA crosshair")?;
        let pick = surface_pick_for_mesh_node(mesh, self.overlay_values.as_ref(), node_index)
            .with_context(|| {
                format!("AFNI/SUMA crosshair references unavailable node {node_index}")
            })?;

        self.controller.interaction.set_pick(Some(pick));
        self.afni_crosshair_node = Some(node_index);
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.control_window.request_redraw();
        if self.controller.panels.roi_controller_open {
            self.roi_control_window.request_redraw();
        }
        self.log_status(format!(
            "Applied AFNI/SUMA crosshair to node {}{}.",
            node_index,
            crosshair
                .surface_idcode
                .as_deref()
                .map(|idcode| format!(" on {idcode}"))
                .unwrap_or_default()
        ));

        Ok(true)
    }

    fn send_afni_crosshair_for_pick(&mut self, pick: SurfacePick) -> Result<()> {
        if self.afni_connection.is_none() {
            return Ok(());
        }
        let Some(element) = self.afni_crosshair_element_for_pick(pick)? else {
            if self.verbose {
                self.log_status(format!(
                    "Could not map node {} to an AFNI/SUMA surface crosshair.",
                    pick.node_index
                ));
            }
            return Ok(());
        };

        let connection = self
            .afni_connection
            .as_mut()
            .context("AFNI/SUMA NIML talk is not connected")?;
        if let Err(error) = connection.send_elements(std::slice::from_ref(&element)) {
            self.disconnect_afni_talk();
            return Err(error.context("AFNI/SUMA crosshair write failed"));
        }
        self.sent_crosshair_node = Some(pick.node_index);
        if self.verbose {
            self.log_status(format!(
                "Sent AFNI/SUMA crosshair for node {}.",
                pick.node_index
            ));
        }

        Ok(())
    }

    fn afni_crosshair_element_for_pick(&self, pick: SurfacePick) -> Result<Option<NimlElement>> {
        if let Some(scene) = self.surface_scene.as_ref() {
            let Some(surface) = scene.surfaces.get(scene.active_index) else {
                return Ok(None);
            };
            let mut node_offset = 0u32;
            for component in &surface.components {
                let Some(mesh) = component.mesh.as_ref() else {
                    continue;
                };
                let node_count = u32::try_from(mesh.vertices.len())
                    .context("surface has too many vertices for AFNI/SUMA node ids")?;
                let Some(node_limit) = node_offset.checked_add(node_count) else {
                    return Ok(None);
                };
                if (node_offset..node_limit).contains(&pick.node_index) {
                    let local_node = pick.node_index - node_offset;
                    let mut info = AfniSurfaceInfo::from_mesh(mesh);
                    decorate_afni_surface_info(&mut info, Some(scene), Some(component));
                    return surface_crosshair_element(
                        mesh,
                        &info,
                        local_node,
                        pick.surface_position,
                    )
                    .map(Some);
                }
                node_offset = node_limit;
            }
        }

        let Some(mesh) = self.mesh.as_ref() else {
            return Ok(None);
        };
        if (pick.node_index as usize) >= mesh.vertices.len() {
            return Ok(None);
        }
        let mut info = AfniSurfaceInfo::from_mesh(mesh);
        decorate_afni_surface_info(&mut info, self.surface_scene.as_ref(), None);
        decorate_afni_surface_volume_info(
            &mut info,
            self.surface_volume_path.as_ref(),
            self.surface_volume_idcode.as_deref(),
        );
        surface_crosshair_element(mesh, &info, pick.node_index, pick.surface_position).map(Some)
    }

    fn afni_node_offset_for_surface(&self, surface_idcode: &str) -> Option<usize> {
        self.afni_surface_target_for_message(surface_idcode, None)
            .map(|target| target.node_offset)
    }

    fn afni_surface_target_for_message(
        &self,
        surface_idcode: &str,
        local_domain_parent_id: Option<&str>,
    ) -> Option<AfniSurfaceTarget> {
        if let Some(scene) = self.surface_scene.as_ref() {
            if let Some(target) = afni_surface_target_in_scene_surface(
                scene,
                scene.active_index,
                |component, mesh| {
                    afni_component_matches_surface_id(component, mesh, surface_idcode)
                },
            ) {
                return Some(target);
            }

            if let Some(parent_id) = local_domain_parent_id
                && let Some(target) = afni_surface_target_in_scene_surface(
                    scene,
                    scene.active_index,
                    |component, mesh| {
                        afni_component_matches_domain_parent(component, mesh, parent_id)
                    },
                )
            {
                return Some(target);
            }
        }

        let mesh = self.mesh.as_ref()?;
        if mesh.metadata.id.as_str() == surface_idcode
            || local_domain_parent_id
                .is_some_and(|parent_id| afni_mesh_matches_domain_parent(mesh, parent_id))
        {
            return Some(AfniSurfaceTarget {
                node_offset: 0,
                node_count: mesh.vertices.len(),
            });
        }

        None
    }

    fn ensure_scene_surfaces_loaded_for_afni(&mut self) -> Result<()> {
        let Some(scene) = self.surface_scene.as_ref() else {
            return Ok(());
        };
        let mut tasks = Vec::new();
        for (surface_index, surface) in scene.surfaces.iter().enumerate() {
            for (component_index, component) in surface.components.iter().enumerate() {
                if !afni_component_is_sendable(component, component.mesh.as_ref()) {
                    continue;
                }
                if component.mesh.is_none() {
                    tasks.push((
                        surface_index,
                        component_index,
                        component.spec_surface.clone(),
                    ));
                }
            }
        }
        if tasks.is_empty() {
            return Ok(());
        }

        let spec = scene.spec.clone();
        let surface_volume_idcode = scene.surface_volume_idcode.clone();
        self.log_status(format!(
            "Loading {} spec surface component{} for AFNI/SUMA registration.",
            tasks.len(),
            if tasks.len() == 1 { "" } else { "s" }
        ));

        for (surface_index, component_index, surface) in tasks {
            let mesh = load_spec_component_mesh(&spec, &surface, surface_volume_idcode.as_deref())?;
            if let Some(scene) = self.surface_scene.as_mut()
                && let Some(component) = scene
                    .surfaces
                    .get_mut(surface_index)
                    .and_then(|surface| surface.components.get_mut(component_index))
                && component.mesh.is_none()
            {
                component.mesh = Some(mesh);
            }
        }

        Ok(())
    }

    fn ensure_scene_surface_loaded(&mut self, index: usize) -> Result<()> {
        let (spec, surface_volume_idcode, tasks) = {
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

            (
                scene.spec.clone(),
                scene.surface_volume_idcode.clone(),
                tasks,
            )
        };

        for (component_index, surface) in tasks {
            let mesh = load_spec_component_mesh(&spec, &surface, surface_volume_idcode.as_deref())?;
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
                        surface_volume_idcode: scene.surface_volume_idcode.clone(),
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
                    task.surface_volume_idcode.as_deref(),
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
                let layout = self.controller.display.pair_state;
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
        let layout = self.controller.display.pair_state;
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
        self.controller.interaction.set_pick(None);
        self.controller.surface.current_surface_id =
            self.mesh.as_ref().map(|mesh| mesh.metadata.id.clone());
        self.controller.surface.current_surface_path = Some(path.clone());
        self.controller.surface.current_scene_surface_index = Some(index);
        if self.roi_layer.is_some() {
            self.rebuild_roi_layer_from_state()?;
        }
        if self.has_both_scene()
            && self.controller.display.pair_visibility != PairVisibility::both()
        {
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
        // Switching the displayed state (e.g. smoothwm -> inflated) does not
        // change the surfaces AFNI already holds, so send only ones it has not
        // seen yet rather than force-resending and tripping its duplicate-
        // surface warning.
        if self.afni_connection.is_some()
            && let Err(error) = self.send_afni_surfaces(false)
        {
            self.set_error(error);
        }

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
        self.surface_buffers = None;
        self.surface_render_set = None;
        self.prepared_geometry_cache = prepared_geometry.map(|geometry| PreparedGeometryCache {
            surface_id: mesh.metadata.id.clone(),
            vertex_count: mesh.vertices.len(),
            face_count: mesh.triangles.len(),
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

    fn begin_pair_drag(&mut self) {
        self.pair_dragging = true;
        self.pair_drag_last_cursor = self.view_cursor_position;
        self.pair_drag_changed = false;
        if self.surface_render_set.is_none() {
            self.upload_surface_buffers();
        }
        if let Err(error) = self.refresh_active_pair_render_geometry() {
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
                "Hemisphere layout: open {}, angle {:.1} deg, gap {:.1}.",
                pair_open_percent_label(self.controller.display.pair_state.open_angle_degrees),
                self.controller.display.pair_state.open_angle_degrees,
                self.controller.display.pair_state.separation_distance
            ));
        }
        self.pair_drag_changed = false;
        if let Err(error) = self.refresh_active_pair_render_geometry() {
            self.set_error(error);
        }
    }

    fn adjust_pair_transform(&mut self, dx: f32, dy: f32) -> Result<()> {
        let Some(pair_width) = self.active_pair_reference_width() else {
            return Ok(());
        };
        let vertical_scale = (pair_width / 700.0).max(0.05);
        self.controller.display.pair_state.open_angle_degrees =
            (self.controller.display.pair_state.open_angle_degrees
                + dx * PAIR_OPEN_DEGREES_PER_PIXEL)
                .clamp(-PAIR_MAX_OPEN_DEGREES, PAIR_MAX_OPEN_DEGREES);
        self.controller.display.pair_state.separation_distance =
            (self.controller.display.pair_state.separation_distance + -dy * vertical_scale)
                .clamp(0.0, pair_width * PAIR_MAX_DRAG_GAP_FACTOR);
        self.controller.display.pair_layout =
            if self.controller.display.pair_state.open_angle_degrees.abs() <= f32::EPSILON
                && self.controller.display.pair_state.separation_distance <= f32::EPSILON
            {
                HemisphereLayout::Closed
            } else {
                HemisphereLayout::Open
            };
        self.pair_drag_changed = true;
        self.show_transient_label(format!(
            "Open {}",
            pair_open_percent_label(self.controller.display.pair_state.open_angle_degrees)
        ));
        self.preview_active_pair_transform()
    }

    fn preview_active_pair_transform(&mut self) -> Result<()> {
        self.refresh_active_pair_render_geometry()
    }

    fn refresh_active_pair_render_geometry(&mut self) -> Result<()> {
        if self.surface_render_set.is_none() {
            self.upload_surface_buffers();
        }
        self.refresh_surface_render_set_matrices();
        let camera = self.camera.clone();
        self.update_render_uniforms_for_camera(&camera);

        Ok(())
    }

    fn refresh_surface_render_set_matrices(&mut self) {
        let matrices = self.active_pair_matrices_for_layout(
            self.controller.display.pair_state,
            self.controller.display.pair_visibility,
        );
        let Some(render_set) = self.surface_render_set.as_mut() else {
            return;
        };

        for instance in &mut render_set.instances {
            if let Some((_, matrix)) = matrices.iter().find(|(side, _)| *side == instance.side) {
                instance.model_matrix = *matrix;
            }
        }
    }

    fn active_pair_matrices_for_layout(
        &self,
        layout: HemisphereLayoutState,
        visibility: PairVisibility,
    ) -> Vec<(SurfaceSide, Mat4)> {
        let Some(scene) = self.surface_scene.as_ref() else {
            return Vec::new();
        };
        let Some(surface) = scene.surfaces.get(scene.active_index) else {
            return Vec::new();
        };

        pair_hemisphere_matrices(&surface.components, layout, visibility)
    }

    fn toggle_pair_hemisphere_visibility(&mut self, side: SurfaceSide) -> Result<()> {
        if !self.has_both_scene() {
            self.log_status("Load a both-hemisphere spec before toggling hemisphere visibility.");
            return Ok(());
        }
        let Some(next) = self
            .controller
            .display
            .pair_visibility
            .toggled(side.clone())
        else {
            return Ok(());
        };
        if next == self.controller.display.pair_visibility {
            return Ok(());
        }

        self.controller.display.pair_visibility = next;
        self.refresh_active_pair_render_geometry()?;
        self.update_scene_stats();
        self.log_status(format!(
            "{} hemisphere toggled; visible hemispheres: {}.",
            surface_side_label(&side),
            self.controller.display.pair_visibility.label()
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
        if let Some(pick) = self.controller.interaction.pick
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

        if let Some(pick) = self.controller.interaction.pick
            && let Some(component) = self.picked_paired_component(pick)
            && let Some(pair) = self.overlay_pair_paths.as_ref()
        {
            return match component.side {
                SurfaceSide::Left => file_name_display(&pair.left_path),
                SurfaceSide::Right => file_name_display(&pair.right_path),
                _ => file_name_display(path),
            };
        }

        if let Some(pick) = self.controller.interaction.pick
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
        if self.controller.display.pair_layout == layout
            && self.controller.display.pair_state == target
        {
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
        self.controller.display.pair_layout = layout;
        self.controller.display.pair_state.open_angle_degrees = state.open_angle_degrees;
        self.controller.display.pair_state.separation_distance = state.separation_distance;
        if let Some(scene) = self.surface_scene.as_ref()
            && scene.hemisphere == SpecHemisphere::Both
        {
            self.refresh_active_pair_render_geometry()?;
        }

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
        self.afni_rgba_colors = None;
        self.afni_rgba_signatures.clear();
        self.overlay_values = Some(overlay_values);
        self.overlay_dataset = Some(loaded_overlay.dataset);
        self.overlay_columns = loaded_overlay.columns;
        self.controller.overlay.visible = true;
        self.overlay_appearance = OverlayAppearance::from_range(range);
        self.controller.overlay.symmetric_range = range.min < 0.0 && range.max > 0.0;
        self.overlay_path = Some(path.clone());
        self.overlay_pair_paths = self.explicit_overlay_pair_for_loaded_path(&path);
        self.controller.surface.current_overlay_path = Some(path.clone());
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

    fn load_overlay_pair_paths(&mut self, pair: ExplicitOverlayPair) -> Result<()> {
        let mesh = self
            .mesh
            .as_ref()
            .context("load a both-hemisphere spec before loading explicit paired overlays")?;
        let loaded_selection = self
            .load_explicit_paired_overlay_selection(&pair, mesh)
            .with_context(|| {
                format!(
                    "failed to load paired overlays {} and {}",
                    pair.left_path.display(),
                    pair.right_path.display()
                )
            })?;
        let loaded_overlay = loaded_selection.overlay;
        let column_summary =
            overlay_column_summary(&loaded_overlay.dataset, loaded_overlay.columns);
        let overlay_values = loaded_overlay.overlay_values;
        let range = overlay_values.range;

        self.overlay = None;
        self.afni_rgba_colors = None;
        self.afni_rgba_signatures.clear();
        self.overlay_values = Some(overlay_values);
        self.overlay_dataset = Some(loaded_overlay.dataset);
        self.overlay_columns = loaded_overlay.columns;
        self.controller.overlay.visible = true;
        self.overlay_appearance = OverlayAppearance::from_range(range);
        self.controller.overlay.symmetric_range = range.min < 0.0 && range.max > 0.0;
        self.overlay_path = Some(pair.left_path.clone());
        self.overlay_pair_paths = Some(pair.clone());
        self.controller.surface.current_overlay_path = Some(pair.left_path.clone());
        self.overlay_display_name = Some(loaded_selection.display_name);
        self.rebuild_overlay_model()?;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Loaded paired overlays range {:.4} to {:.4}. {column_summary}",
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
        let rois = rois
            .into_iter()
            .map(|roi| self.attach_roi_to_current_surface(roi))
            .collect::<Vec<_>>();
        self.roi_path = Some(path.clone());
        self.controller.surface.current_roi_path = Some(path.clone());
        self.roi_workspace = RoiWorkspace::from_rois(rois);
        self.rebuild_roi_layer_from_state()
            .with_context(|| format!("failed to map ROI {}", path.display()))?;
        let layer = self
            .roi_layer
            .as_ref()
            .context("loaded ROI did not produce a display layer")?;
        let roi_count = layer.rois.len();
        let mapped_nodes = layer.mapped_nodes;

        self.controller.roi.visible = true;
        self.refresh_pick_overlay_value();
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Loaded {roi_count} ROI object(s) from {} on {mapped_nodes} nodes.",
            path.display()
        ));

        Ok(())
    }

    fn save_roi_slot(&mut self, index: usize) -> Result<()> {
        let Some(roi) = self.roi_workspace.saveable_roi_at(index)? else {
            self.log_status("No ROI is available to save in this slot.");
            return Ok(());
        };

        let default_name = roi_save_default_name(&roi, self.surface_path.as_ref());
        let Some(path) = save_roi_file(
            "Save ROI",
            &default_name,
            self.roi_path.as_ref().or(self.surface_path.as_ref()),
        ) else {
            self.log_status("ROI save cancelled.");
            return Ok(());
        };

        write_niml_roi(&path, std::slice::from_ref(&roi))?;
        self.log_status(format!(
            "Saved ROI {} to {}.",
            roi_display_label(&roi),
            path.display()
        ));

        Ok(())
    }

    fn save_all_rois(&mut self) -> Result<()> {
        let rois = self.roi_workspace.saveable_rois()?;
        if rois.is_empty() {
            self.log_status("No ROI is available to save.");
            return Ok(());
        }

        let default_name = roi_save_all_default_name(self.roi_path.as_ref());
        let Some(path) = save_roi_file(
            "Save All ROIs",
            &default_name,
            self.roi_path.as_ref().or(self.surface_path.as_ref()),
        ) else {
            self.log_status("ROI save cancelled.");
            return Ok(());
        };

        write_niml_roi(&path, &rois)?;
        self.roi_path = Some(path.clone());
        self.controller.surface.current_roi_path = Some(path.clone());
        self.rebuild_roi_layer_from_state()?;
        self.controller.roi.visible = true;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(format!(
            "Saved {} ROI object(s) to {}.",
            rois.len(),
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
            display_name: file_name_display(&path),
            rois,
            appearance: build.appearance,
            node_labels: build.node_labels,
            mapped_nodes: build.mapped_nodes,
            skipped_nodes: build.skipped_nodes,
        })
    }

    fn attach_roi_to_current_surface(&self, roi: Roi) -> Roi {
        if let Some(target) = self.roi_draft_target_for_side(&roi.parent_side) {
            roi.with_parent_surface(target.surface_id, target.domain_id, target.side)
        } else {
            roi
        }
    }

    fn roi_draft_target_for_side(&self, side: &SurfaceSide) -> Option<RoiDraftTarget> {
        if let Some((left, right)) = self.active_paired_components() {
            let component = match side {
                SurfaceSide::Left => left,
                SurfaceSide::Right => right,
                _ => return None,
            };
            let mesh = component.mesh.as_ref()?;
            return Some(RoiDraftTarget {
                surface_id: mesh.metadata.id.clone(),
                domain_id: mesh.domain.id.clone(),
                side: component.side.clone(),
            });
        }

        let mesh = self.mesh.as_ref()?;
        let side = if matches!(side, SurfaceSide::Unknown) {
            mesh.metadata.side.clone()
        } else {
            side.clone()
        };
        Some(RoiDraftTarget {
            surface_id: mesh.metadata.id.clone(),
            domain_id: mesh.domain.id.clone(),
            side,
        })
    }

    fn rebuild_roi_layer_from_state(&mut self) -> Result<()> {
        let rois = self.roi_workspace.visible_rois()?;

        if rois.is_empty() {
            self.roi_layer = None;
            return Ok(());
        }

        let path = self
            .roi_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("sumaru_draft.niml.roi"));
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

    fn load_explicit_paired_overlay_selection(
        &self,
        pair: &ExplicitOverlayPair,
        mesh: &SurfaceMesh,
    ) -> Result<LoadedOverlaySelection> {
        let (left, right) = self
            .active_paired_components()
            .context("--overlay-lh/--overlay-rh require an active both-hemisphere spec")?;
        let left_mesh = left
            .mesh
            .as_ref()
            .context("left hemisphere surface is still loading")?;
        let right_mesh = right
            .mesh
            .as_ref()
            .context("right hemisphere surface is still loading")?;
        ensure!(
            pair.left_path.exists(),
            "left hemisphere overlay {} does not exist",
            pair.left_path.display()
        );
        ensure!(
            pair.right_path.exists(),
            "right hemisphere overlay {} does not exist",
            pair.right_path.display()
        );

        let left_dataset = load_dataset_from_path(&pair.left_path, left_mesh)
            .with_context(|| format!("failed to load {}", pair.left_path.display()))?;
        let right_dataset = load_dataset_from_path(&pair.right_path, right_mesh)
            .with_context(|| format!("failed to load {}", pair.right_path.display()))?;
        let dataset = paired_overlay_dataset(
            left_dataset,
            right_dataset,
            &mesh.domain,
            left_mesh.vertices.len() as u32,
        )?;
        let overlay = loaded_overlay_from_dataset(dataset, mesh.vertices.len(), "paired NIML")?;

        Ok(LoadedOverlaySelection {
            overlay,
            display_name: explicit_overlay_pair_display_name(pair),
        })
    }

    fn explicit_overlay_pair_for_loaded_path(&self, path: &Path) -> Option<ExplicitOverlayPair> {
        self.active_paired_components()?;
        let paths = paired_overlay_paths(path)?;
        Some(ExplicitOverlayPair {
            left_path: paths.left_path,
            right_path: paths.right_path,
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
        self.overlay_appearance.range = if self.controller.overlay.symmetric_range {
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
            .with_symmetric_range(self.controller.overlay.symmetric_range)
            .with_threshold(threshold, mask_mode)
            .with_opacity(self.overlay_appearance.opacity);

        overlay.rebuild_color_cache(dataset, domain)?;
        self.overlay = Some(overlay);
        self.sync_controller_overlay_display_state();

        Ok(())
    }

    fn sync_controller_overlay_display_state(&mut self) {
        self.controller.overlay.intensity_range = Some([
            self.overlay_appearance.range.min,
            self.overlay_appearance.range.max,
        ]);
        self.controller.overlay.threshold = self.overlay_appearance.threshold.enabled.then_some(
            crate::command::OverlayThresholdCommandState {
                value: self.overlay_appearance.threshold.value,
                absolute: self.overlay_appearance.threshold.absolute,
                hide_failed: self.overlay_appearance.threshold.hide_failed,
            },
        );
        self.controller.overlay.opacity = self.overlay_appearance.opacity;
    }

    fn toggle_overlay_visibility(&mut self) {
        if self.overlay.is_none() {
            self.log_status("No overlay is loaded.");
            return;
        }

        self.controller.overlay.visible = !self.controller.overlay.visible;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status(if self.controller.overlay.visible {
            "Overlay visible."
        } else {
            "Overlay hidden."
        });
    }

    fn set_surface_controller_visible(&mut self, visible: bool) {
        self.controller.panels.surface_controller_visible = visible;
        self.control_window.set_visible(visible);
        if visible {
            self.control_window.request_redraw();
        }
        self.view_window.request_redraw();
    }

    fn set_roi_controller_open(&mut self, open: bool) {
        self.controller.panels.roi_controller_open = open;
        self.roi_control_window.set_visible(open);
        if open {
            self.roi_control_window.request_redraw();
        }
        self.view_window.request_redraw();
    }

    fn visible_overlay(&self) -> Option<&Overlay> {
        self.overlay
            .as_ref()
            .filter(|_| self.controller.overlay.visible)
    }

    fn visible_roi_layer(&self) -> Option<&RoiLayer> {
        self.roi_layer
            .as_ref()
            .filter(|_| self.controller.roi.visible)
    }

    fn inspect_surface_at_cursor(&mut self) {
        match self.pick_surface_at_cursor() {
            Some(pick) => {
                self.log_status(pick.status_text());
                self.controller.interaction.set_pick(Some(pick));
                if let Err(error) = self.send_afni_crosshair_for_pick(pick) {
                    self.set_error(error);
                }
                self.upload_surface_buffers();
            }
            None => {
                self.controller.interaction.set_pick(None);
                self.upload_surface_buffers();
                self.log_status("No surface under the cursor.");
            }
        }
    }

    fn pick_surface_at_cursor(&self) -> Option<SurfacePick> {
        let cursor = self.view_cursor_position?;
        let scene_size = self.scene_viewport_size();
        if cursor.0 < 0.0
            || cursor.1 < 0.0
            || cursor.0 > f64::from(scene_size.width)
            || cursor.1 > f64::from(scene_size.height)
        {
            return None;
        }
        if let Some(pick) = self.pick_active_pair_surface_at_cursor(cursor) {
            return Some(pick);
        }

        let mesh = self.mesh.as_ref()?;
        pick_surface(
            mesh,
            self.overlay_values.as_ref(),
            &self.camera,
            scene_size,
            cursor,
        )
    }

    fn pick_active_pair_surface_at_cursor(&self, cursor: (f64, f64)) -> Option<SurfacePick> {
        if !self.has_both_scene() {
            return None;
        }
        let scene = self.surface_scene.as_ref()?;
        let surface = scene.surfaces.get(scene.active_index)?;
        let matrices = pair_hemisphere_matrices(
            &surface.components,
            self.controller.display.pair_state,
            self.controller.display.pair_visibility,
        );
        let mut best = None;
        let mut best_distance = f32::INFINITY;
        let mut node_offset = 0u32;
        let mut face_offset = 0usize;

        for component in &surface.components {
            let mesh = component.mesh.as_ref()?;
            if self
                .controller
                .display
                .pair_visibility
                .is_visible(&component.side)
                && let Some((_, matrix)) = matrices.iter().find(|(side, _)| *side == component.side)
                && let Some((pick, distance)) = pick_surface_with_model(
                    mesh,
                    self.overlay_values.as_ref(),
                    &self.camera,
                    self.scene_viewport_size(),
                    cursor,
                    *matrix,
                    node_offset,
                    face_offset,
                )
                && distance < best_distance
            {
                best_distance = distance;
                best = Some(pick);
            }
            node_offset = node_offset.saturating_add(mesh.vertices.len() as u32);
            face_offset += mesh.triangles.len();
        }

        best
    }

    fn handle_roi_draw_click_at_cursor(&mut self) -> Result<()> {
        let Some(pick) = self.pick_surface_at_cursor() else {
            self.log_status("No surface under the cursor for ROI drawing.");
            return Ok(());
        };
        let target = self.roi_pick_target(pick)?;
        self.controller.interaction.set_pick(Some(pick));
        self.send_afni_crosshair_for_pick(pick)?;

        let fill_pending = self
            .roi_workspace
            .active_draft()
            .is_some_and(|draft| draft.fill_pending);
        if fill_pending {
            self.fill_roi_draft_from_seed(&target)?;
        } else {
            self.add_roi_draft_point(&target)?;
        }

        self.upload_surface_buffers();
        self.update_scene_stats();
        self.control_window.request_redraw();
        if self.controller.panels.roi_controller_open {
            self.roi_control_window.request_redraw();
        }

        Ok(())
    }

    fn roi_pick_target(&self, pick: SurfacePick) -> Result<RoiPickTarget> {
        if let Some((left, right)) = self.active_paired_components() {
            let left_mesh = left
                .mesh
                .as_ref()
                .context("left hemisphere surface is still loading")?;
            let right_mesh = right
                .mesh
                .as_ref()
                .context("right hemisphere surface is still loading")?;
            let left_node_count = left_mesh.vertices.len() as u32;
            if pick.node_index < left_node_count {
                return Ok(RoiPickTarget {
                    mesh: left_mesh.clone(),
                    target: RoiDraftTarget {
                        surface_id: left_mesh.metadata.id.clone(),
                        domain_id: left_mesh.domain.id.clone(),
                        side: SurfaceSide::Left,
                    },
                    local_node: pick.node_index,
                });
            }
            let local_node = pick
                .node_index
                .checked_sub(left_node_count)
                .context("picked node index is outside the left hemisphere")?;
            ensure!(
                (local_node as usize) < right_mesh.vertices.len(),
                "picked node {} is outside the right hemisphere node count {}",
                local_node,
                right_mesh.vertices.len()
            );
            return Ok(RoiPickTarget {
                mesh: right_mesh.clone(),
                target: RoiDraftTarget {
                    surface_id: right_mesh.metadata.id.clone(),
                    domain_id: right_mesh.domain.id.clone(),
                    side: SurfaceSide::Right,
                },
                local_node,
            });
        }

        let mesh = self
            .mesh
            .as_ref()
            .context("load a surface before drawing an ROI")?;
        Ok(RoiPickTarget {
            mesh: mesh.clone(),
            target: RoiDraftTarget {
                surface_id: mesh.metadata.id.clone(),
                domain_id: mesh.domain.id.clone(),
                side: mesh.metadata.side.clone(),
            },
            local_node: pick.node_index,
        })
    }

    fn ensure_roi_draft_target(&self, target: &RoiDraftTarget) -> Result<()> {
        if let Some(existing) = self
            .roi_workspace
            .active_draft()
            .and_then(|draft| draft.target.as_ref())
        {
            ensure!(
                existing == target,
                "ROI draft is tied to the {:?} surface; clear or save it before drawing on {:?}",
                existing.side,
                target.side
            );
        }

        Ok(())
    }

    fn add_roi_draft_point(&mut self, target: &RoiPickTarget) -> Result<()> {
        self.ensure_roi_draft_target(&target.target)?;
        let Some(draft) = self.roi_workspace.active_draft() else {
            self.log_status("No editable ROI slot is active.");
            return Ok(());
        };
        let was_joined = draft.is_joined();

        let new_segment = if let Some(previous) = draft.anchor_nodes.last().copied() {
            Some(
                target
                    .mesh
                    .shortest_node_path(previous, target.local_node)?
                    .context("no mesh path connects the selected ROI points")?
                    .nodes,
            )
        } else {
            None
        };
        let draft = self
            .roi_workspace
            .active_draft_mut()
            .context("no editable ROI slot is active")?;
        if draft.target.is_none() {
            draft.target = Some(target.target.clone());
        }
        draft.push_history();
        if was_joined {
            draft.reopen_joined_path_for_append();
        }
        draft.target = Some(target.target.clone());
        if let Some(segment) = new_segment {
            draft.segments.push(segment);
        }
        draft.anchor_nodes.push(target.local_node);
        draft.fill_nodes = None;
        draft.fill_seed_node = None;
        draft.fill_pending = false;
        self.rebuild_roi_layer_from_state()?;
        if was_joined {
            self.log_status(format!(
                "ROI reopened and point added at node {}.",
                target.local_node
            ));
        } else {
            self.log_status(format!("ROI point added at node {}.", target.local_node));
        }

        Ok(())
    }

    fn join_roi_draft(&mut self) -> Result<()> {
        let Some(draft) = self.roi_workspace.active_draft() else {
            self.log_status("No editable ROI slot is active.");
            return Ok(());
        };
        if !draft.can_join() {
            self.log_status("Need at least three ROI points before joining.");
            return Ok(());
        }
        let mesh = self
            .roi_draft_mesh()
            .context("active surface for the ROI draft is not available")?;
        let first = draft.anchor_nodes[0];
        let last = *draft
            .anchor_nodes
            .last()
            .context("ROI draft has no last point")?;
        let closing = mesh
            .shortest_node_path(last, first)?
            .context("no mesh path connects the ROI endpoints")?;

        let draft = self
            .roi_workspace
            .active_draft_mut()
            .context("no editable ROI slot is active")?;
        draft.push_history();
        draft.segments.push(closing.nodes);
        draft.fill_nodes = None;
        draft.fill_seed_node = None;
        draft.fill_pending = false;
        self.rebuild_roi_layer_from_state()?;
        self.upload_surface_buffers();
        self.update_scene_stats();
        self.log_status("ROI loop joined. Press Fill, then right-click a seed point.");

        Ok(())
    }

    fn fill_roi_draft_from_seed(&mut self, target: &RoiPickTarget) -> Result<()> {
        self.ensure_roi_draft_target(&target.target)?;
        let draft = self
            .roi_workspace
            .active_draft()
            .context("no editable ROI slot is active")?;
        ensure!(draft.can_fill(), "join the ROI before filling it");
        let boundary = draft.boundary_nodes();
        let nodes = roi_fill_nodes_from_seed(&target.mesh, &boundary, target.local_node)?;
        let node_count = nodes.len();

        let draft = self
            .roi_workspace
            .active_draft_mut()
            .context("no editable ROI slot is active")?;
        draft.push_history();
        draft.fill_nodes = Some(nodes);
        draft.fill_seed_node = Some(target.local_node);
        draft.fill_pending = false;
        self.rebuild_roi_layer_from_state()?;
        self.log_status(format!(
            "ROI fill defined from node {} on {node_count} nodes.",
            target.local_node
        ));

        Ok(())
    }

    fn roi_draft_mesh(&self) -> Option<SurfaceMesh> {
        let target = self.roi_workspace.active_draft()?.target.as_ref()?;
        if let Some((left, right)) = self.active_paired_components() {
            return match &target.side {
                SurfaceSide::Left => left.mesh.clone(),
                SurfaceSide::Right => right.mesh.clone(),
                _ => None,
            };
        }

        self.mesh.as_ref().cloned()
    }

    fn sync_pick_to_roi_draft_anchor(&mut self) {
        let pick = self.roi_draft_anchor_pick();
        self.controller.interaction.set_pick(pick);
    }

    fn roi_draft_anchor_pick(&self) -> Option<SurfacePick> {
        let draft = self.roi_workspace.active_draft()?;
        let target = draft.target.as_ref()?;
        let local_node = draft.anchor_nodes.last().copied()?;
        let mesh = self.mesh.as_ref()?;
        let node_index = self.display_node_for_roi_anchor(target, local_node)?;

        surface_pick_for_mesh_node(mesh, self.overlay_values.as_ref(), node_index)
    }

    fn display_node_for_roi_anchor(&self, target: &RoiDraftTarget, local_node: u32) -> Option<u32> {
        if let Some((left, right)) = self.active_paired_components() {
            let left_nodes = left.mesh.as_ref()?.vertices.len() as u32;
            let right_nodes = right.mesh.as_ref()?.vertices.len() as u32;
            return match &target.side {
                SurfaceSide::Left => (local_node < left_nodes).then_some(local_node),
                SurfaceSide::Right => (local_node < right_nodes)
                    .then(|| left_nodes.checked_add(local_node))
                    .flatten(),
                _ => None,
            };
        }

        self.mesh
            .as_ref()
            .and_then(|mesh| ((local_node as usize) < mesh.vertices.len()).then_some(local_node))
    }

    fn refresh_pick_overlay_value(&mut self) {
        if let Some(pick) = &mut self.controller.interaction.pick {
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

    fn open_graph_for_current_pick(&mut self) -> Result<()> {
        let Some(pick) = self.controller.interaction.pick else {
            self.log_status("Pick a surface node before opening a graph.");
            return Ok(());
        };
        let snapshot = self
            .graph_snapshot_for_pick(pick)
            .context("no plottable overlay values are available for the picked node")?;
        self.graph_snapshot = Some(snapshot);
        self.set_graph_window_open(true);
        self.log_status(format!("Opened graph for node {}.", pick.node_index));
        Ok(())
    }

    fn set_graph_window_open(&mut self, open: bool) {
        let was_open = self.controller.panels.graph_window_open;
        if open && !was_open {
            self.grow_view_window_for_graph_dock();
        } else if !open && was_open {
            self.shrink_view_window_after_graph_dock();
        }
        self.controller.panels.graph_window_open = open;
        self.graph_window.set_visible(false);
        if open {
            self.view_window.request_redraw();
        }
        self.view_window.request_redraw();
    }

    fn grow_view_window_for_graph_dock(&mut self) {
        if self.graph_dock_pre_open_size.is_some() {
            return;
        }

        let growth = self.graph_dock_height_pixels();
        if growth == 0 {
            return;
        }

        let desired_size = PhysicalSize::new(
            self.view_size.width.max(1),
            self.view_size.height.saturating_add(growth).max(1),
        );
        self.graph_dock_pre_open_size = Some(self.view_size);
        if let Some(actual_size) = self.view_window.request_inner_size(desired_size) {
            self.resize_view(actual_size);
        }
    }

    fn shrink_view_window_after_graph_dock(&mut self) {
        let Some(previous_size) = self.graph_dock_pre_open_size.take() else {
            return;
        };
        let desired_height = previous_size.height.max(1);
        let desired_size = PhysicalSize::new(self.view_size.width.max(1), desired_height);
        if let Some(actual_size) = self.view_window.request_inner_size(desired_size) {
            self.resize_view(actual_size);
        }
    }

    fn graph_dock_height_pixels(&self) -> u32 {
        (GRAPH_DOCK_DEFAULT_HEIGHT_POINTS * self.view_egui_ctx.pixels_per_point())
            .round()
            .max(1.0) as u32
    }

    fn graph_snapshot_for_pick(&self, pick: SurfacePick) -> Option<GraphSnapshot> {
        let mut points = Vec::new();
        if let Some(dataset) = self.overlay_dataset.as_ref()
            && let Some(row) = dataset_row_for_node(dataset, pick.node_index)
        {
            for (column_index, column) in dataset.columns.iter().enumerate() {
                let Some(value) = numeric_column_value_as_f32(column, row) else {
                    continue;
                };
                points.push(GraphPoint {
                    column_index,
                    label: graph_column_label(column_index, column),
                    value,
                });
            }
        }

        if points.is_empty() {
            if let Some(value) = pick.overlay_value {
                points.push(GraphPoint {
                    column_index: self.overlay_columns.intensity,
                    label: "I".to_string(),
                    value,
                });
            }
            if let Some(value) = pick.threshold_value {
                points.push(GraphPoint {
                    column_index: self.overlay_columns.threshold.unwrap_or(1),
                    label: "T".to_string(),
                    value,
                });
            }
        }

        if points.is_empty() {
            return None;
        }

        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for point in &points {
            min = min.min(point.value);
            max = max.max(point.value);
        }
        if !min.is_finite() || !max.is_finite() {
            return None;
        }
        if (max - min).abs() <= f32::EPSILON {
            min -= 1.0;
            max += 1.0;
        } else {
            let padding = (max - min) * 0.08;
            min -= padding;
            max += padding;
        }

        Some(GraphSnapshot {
            node_index: pick.node_index,
            surface_position: pick.surface_position,
            surface_label: self.pick_surface_display_text(),
            overlay_label: self.pick_overlay_display_text(),
            points,
            y_range: ValueRange { min, max },
        })
    }

    fn upload_surface_buffers(&mut self) {
        let afni_surface_colors = (self.controller.overlay.visible)
            .then(|| self.afni_rgba_colors.clone())
            .flatten();
        let surface_colors = if let Some(afni) = afni_surface_colors {
            // AFNI colors replace the surface color, so resolve their alpha
            // against the anatomical underlay now: sub-threshold nodes show the
            // underlay instead of painting the surface black.
            let underlay = self.visible_anatomical_shading_colors();
            Some(Arc::new(afni_colors_over_underlay(
                &afni,
                underlay.as_deref().map(Vec::as_slice),
            )))
        } else {
            self.visible_anatomical_shading_colors()
        };
        if self.mesh.is_none() {
            self.surface_buffers = None;
            self.surface_render_set = None;
            self.prepared_geometry_cache = None;
            self.anatomical_shading_cache = None;
            return;
        }

        if self.has_both_scene()
            && self.upload_paired_surface_render_set(surface_colors.as_deref().map(Vec::as_slice))
        {
            return;
        }

        self.surface_render_set = None;
        let mesh = self
            .mesh
            .as_ref()
            .expect("mesh existence was checked above");
        if !self
            .prepared_geometry_cache
            .as_ref()
            .is_some_and(|cache| cache.matches(mesh))
        {
            self.prepared_geometry_cache = Some(PreparedGeometryCache {
                surface_id: mesh.metadata.id.clone(),
                vertex_count: mesh.vertices.len(),
                face_count: mesh.triangles.len(),
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
        let use_afni_cell_colors =
            self.afni_rgba_colors.is_some() && self.controller.overlay.visible;
        let visible_overlay = self
            .afni_rgba_colors
            .is_none()
            .then(|| self.visible_overlay())
            .flatten();
        let surface_color_slice = surface_colors.as_deref().map(Vec::as_slice);
        let roi = self.visible_roi_layer().map(|layer| &layer.appearance);
        let prepared_surface = if use_afni_cell_colors {
            PreparedSurface::from_geometry_cell_colors(
                &geometry,
                surface_color_slice,
                roi.map(|roi| roi.node_colors.as_slice()),
                selection,
            )
        } else {
            PreparedSurface::from_geometry_with_selection(
                &geometry,
                surface_color_slice,
                visible_overlay,
                self.overlay_appearance.dim,
                roi,
                selection,
            )
        };
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

    fn upload_paired_surface_render_set(&mut self, surface_colors: Option<&[[f32; 4]]>) -> bool {
        struct RawRenderComponent {
            side: SurfaceSide,
            node_offset: u32,
            face_offset: usize,
            positions: Vec<[f32; 3]>,
            normals: Vec<[f32; 3]>,
            triangles: Vec<[u32; 3]>,
        }

        let raw = {
            let Some(scene) = self.surface_scene.as_mut() else {
                return false;
            };
            if scene.hemisphere != SpecHemisphere::Both {
                return false;
            }
            let Some(surface) = scene.surfaces.get_mut(scene.active_index) else {
                return false;
            };

            let mut raw = Vec::with_capacity(surface.components.len());
            let mut node_offset = 0u32;
            let mut face_offset = 0usize;
            for component in &mut surface.components {
                let Ok(normals) = ensure_component_normals(component) else {
                    return false;
                };
                let Some(mesh) = component.mesh.as_ref() else {
                    return false;
                };
                raw.push(RawRenderComponent {
                    side: component.side.clone(),
                    node_offset,
                    face_offset,
                    positions: mesh.vertices.clone(),
                    normals: (*normals).clone(),
                    triangles: mesh.triangles.clone(),
                });
                let Ok(component_nodes) = u32::try_from(mesh.vertices.len()) else {
                    return false;
                };
                node_offset = node_offset.saturating_add(component_nodes);
                face_offset += mesh.triangles.len();
            }
            raw
        };
        if raw.len() != 2 {
            return false;
        }

        let visible_overlay = self
            .afni_rgba_colors
            .is_none()
            .then(|| self.visible_overlay())
            .flatten();
        let use_afni_cell_colors =
            self.afni_rgba_colors.is_some() && self.controller.overlay.visible;
        let overlay_colors = visible_overlay.map(|overlay| overlay.color_cache.colors.clone());
        let roi_colors = self
            .visible_roi_layer()
            .map(|layer| layer.appearance.node_colors.clone());
        let selection = self.controller.interaction.pick;
        let dim = self.overlay_appearance.dim;
        let layout = self.controller.display.pair_state;
        let visibility = self.controller.display.pair_visibility;
        let matrices = self.active_pair_matrices_for_layout(layout, visibility);
        let selection_scale = selection_scale_from_model_matrices(&matrices);
        let aspect = self.scene_viewport_aspect();

        let mut instances = Vec::with_capacity(raw.len());
        for component in raw {
            let node_start = component.node_offset as usize;
            let node_end = node_start + component.positions.len();
            let surface_color_slice =
                surface_colors.and_then(|colors| colors.get(node_start..node_end));
            let overlay_color_slice = overlay_colors
                .as_ref()
                .and_then(|colors| colors.get(node_start..node_end));
            let roi_color_slice = roi_colors
                .as_ref()
                .and_then(|colors| colors.get(node_start..node_end));
            let selection = selection_for_component(
                selection,
                component.node_offset,
                component.face_offset,
                &component.positions,
                selection_scale,
            );
            let geometry = prepared_geometry_from_raw_component(
                &component.positions,
                &component.normals,
                &component.triangles,
            );
            let prepared_surface = if use_afni_cell_colors {
                PreparedSurface::from_geometry_cell_colors(
                    &geometry,
                    surface_color_slice,
                    roi_color_slice,
                    selection,
                )
            } else {
                PreparedSurface::from_geometry_color_slices(
                    &geometry,
                    surface_color_slice,
                    overlay_color_slice,
                    dim,
                    roi_color_slice,
                    selection,
                )
            };
            let vertex_bytes = prepared_surface.vertex_bytes();
            let index_bytes = prepared_surface.index_bytes();
            let model_matrix = matrices
                .iter()
                .find(|(side, _)| *side == component.side)
                .map(|(_, matrix)| *matrix)
                .unwrap_or(Mat4::IDENTITY);
            let uniform_bytes = self.camera.uniform_bytes_with_model(aspect, model_matrix);
            let vertex_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("paired surface vertex buffer"),
                    contents: &vertex_bytes,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });
            let index_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("paired surface index buffer"),
                    contents: &index_bytes,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                });
            let uniform_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("paired surface uniform buffer"),
                        contents: &uniform_bytes,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    });
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("paired surface bind group"),
                layout: &self.uniform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                }],
            });
            instances.push(SurfaceRenderInstance {
                side: component.side,
                vertex_buffer,
                index_buffer,
                index_count: prepared_surface.index_count(),
                uniform_buffer,
                bind_group,
                model_matrix,
            });
        }

        self.surface_buffers = None;
        self.surface_render_set = Some(SurfaceRenderSet { instances });
        true
    }

    fn visible_anatomical_shading_colors(&mut self) -> Option<Arc<Vec<[f32; 4]>>> {
        if !self.controller.display.anatomical_shading_visible {
            return None;
        }

        if let Some(cache) = self.anatomical_shading_cache.as_ref()
            && self.mesh.as_ref().is_some_and(|mesh| cache.matches(mesh))
        {
            return Some(cache.colors.clone());
        }

        let (surface_id, vertex_count, face_count, colors) = {
            let mesh = self.mesh.as_ref()?;
            let colors = if let Some(scene) = self.surface_scene.as_ref() {
                scene
                    .surfaces
                    .get(scene.active_index)
                    .map(|surface| scene_anatomical_shading_colors(scene, surface, mesh))
                    .unwrap_or_else(|| direct_anatomical_shading_colors(mesh))
            } else {
                direct_anatomical_shading_colors(mesh)
            };
            (
                mesh.metadata.id.clone(),
                mesh.vertices.len(),
                mesh.triangles.len(),
                colors,
            )
        };
        let colors = Arc::new(colors);
        self.anatomical_shading_cache = Some(AnatomicalShadingCache {
            surface_id,
            vertex_count,
            face_count,
            colors: colors.clone(),
        });

        Some(colors)
    }

    fn selection_highlight(&self) -> Option<SelectionHighlight> {
        let pick = self.controller.interaction.pick?;
        Some(SelectionHighlight::normalized(
            pick.node_index,
            pick.face_index,
            pick.normalized_position,
        ))
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
        self.show_transient_label(mode.label());
    }

    fn show_transient_label(&mut self, text: impl Into<String>) {
        self.mode_label = Some(ModeLabel {
            text: text.into(),
            until: Instant::now() + MODE_LABEL_DURATION,
        });
    }

    /// Returns the active mode-label text and the time remaining before it
    /// expires, so the caller can schedule a repaint to clear it.
    fn active_mode_label(&mut self) -> Option<(String, Duration)> {
        let label = self.mode_label.as_ref()?;
        let now = Instant::now();
        if now >= label.until {
            self.mode_label = None;
            return None;
        }

        Some((label.text.clone(), label.until - now))
    }

    fn set_error(&mut self, error: anyhow::Error) {
        eprintln!("sumaru error: {error:#}");
    }

    fn log_status(&self, message: impl AsRef<str>) {
        self.controller.record_status(message.as_ref());
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

/// Per-hemisphere resident geometry for both-spec scenes. Positions and normals
/// remain in each source surface's mesh space; the model matrix moves them into
/// the active paired layout.
struct SurfaceRenderSet {
    instances: Vec<SurfaceRenderInstance>,
}

struct SurfaceRenderInstance {
    side: SurfaceSide,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    model_matrix: Mat4,
}

#[derive(Clone)]
struct PreparedGeometryCache {
    surface_id: SurfaceId,
    vertex_count: usize,
    face_count: usize,
    geometry: Arc<PreparedGeometry>,
}

impl PreparedGeometryCache {
    fn matches(&self, mesh: &SurfaceMesh) -> bool {
        self.surface_id == mesh.metadata.id
            && self.vertex_count == mesh.vertices.len()
            && self.face_count == mesh.triangles.len()
    }
}

#[derive(Clone)]
struct AnatomicalShadingCache {
    surface_id: SurfaceId,
    vertex_count: usize,
    face_count: usize,
    colors: Arc<Vec<[f32; 4]>>,
}

impl AnatomicalShadingCache {
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
    surface_volume_idcode: Option<String>,
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
    prepared_geometry: Option<Arc<PreparedGeometry>>,
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
                    prepared_geometry: cache.prepared_geometry.clone(),
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
                prepared_geometry: Some(prepared_geometry.clone()),
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
                prepared_geometry: cache.prepared_geometry.clone(),
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

        self.display_cache = Some(DisplayMeshCache {
            layout,
            mesh: mesh.clone(),
            prepared_geometry: None,
        });

        Ok(DisplayMeshSnapshot {
            mesh,
            prepared_geometry: None,
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
        vertices.extend(mesh.vertices.iter().map(|position| {
            transform_point(mesh, transform, Vec3::from_array(*position)).to_array()
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

/// Node closest to the surface's bounding-box center (½ x, ½ y, ½ z), used as
/// the default crosshair target when nothing has been picked yet.
fn node_nearest_bounds_center(mesh: &SurfaceMesh) -> Option<u32> {
    let center = Vec3::from_array(mesh.bounds.center);
    mesh.vertices
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            let left = Vec3::from_array(**left).distance_squared(center);
            let right = Vec3::from_array(**right).distance_squared(center);
            left.total_cmp(&right)
        })
        .and_then(|(index, _)| u32::try_from(index).ok())
}

fn surface_pick_for_mesh_node(
    mesh: &SurfaceMesh,
    overlay: Option<&OverlayDataset>,
    node_index: u32,
) -> Option<SurfacePick> {
    let surface_position = *mesh.vertices.get(node_index as usize)?;
    let face_index = first_face_containing_node(mesh, node_index)?;
    let center = Vec3::from_array(mesh.bounds.center);
    let scale = if mesh.bounds.radius > f32::EPSILON {
        1.0 / mesh.bounds.radius
    } else {
        1.0
    };
    let normalized_position = ((Vec3::from_array(surface_position) - center) * scale).to_array();
    let overlay_value = overlay
        .and_then(|overlay| overlay.values.get(node_index as usize))
        .copied();
    let threshold_value = overlay
        .and_then(|overlay| overlay.threshold_values.as_ref())
        .and_then(|values| values.get(node_index as usize))
        .copied();

    Some(SurfacePick {
        node_index,
        face_index,
        surface_position,
        normalized_position,
        overlay_value,
        threshold_value,
    })
}

fn first_face_containing_node(mesh: &SurfaceMesh, node_index: u32) -> Option<usize> {
    mesh.triangles
        .iter()
        .position(|triangle| triangle.contains(&node_index))
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
            Some((
                component.side.clone(),
                hemisphere_model_matrix(
                    transform
                        .rotation_pivot
                        .unwrap_or_else(|| Vec3::from_array(mesh.bounds.center)),
                    transform.rotation_z_degrees,
                    transform.offset,
                    center,
                    radius,
                ),
            ))
        })
        .collect()
}

fn prepared_geometry_from_raw_component(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    triangles: &[[u32; 3]],
) -> PreparedGeometry {
    let vertices = positions
        .iter()
        .zip(normals)
        .map(|(position, normal)| PreparedGeometryVertex {
            position: *position,
            normal: *normal,
        })
        .collect();
    let indices = triangles
        .iter()
        .flat_map(|triangle| triangle.iter().copied())
        .collect();

    PreparedGeometry { vertices, indices }
}

fn selection_for_component(
    selection: Option<SurfacePick>,
    node_offset: u32,
    face_offset: usize,
    positions: &[[f32; 3]],
    scale: f32,
) -> Option<SelectionHighlight> {
    let selection = selection?;
    let local_node = selection.node_index.checked_sub(node_offset)?;
    if local_node as usize >= positions.len() {
        return None;
    }
    let local_face = selection.face_index.checked_sub(face_offset)?;

    Some(SelectionHighlight::scaled(
        local_node,
        local_face,
        positions[local_node as usize],
        scale,
    ))
}

fn selection_scale_from_model_matrices(matrices: &[(SurfaceSide, Mat4)]) -> f32 {
    matrices
        .iter()
        .find_map(|(_, matrix)| {
            let inv_radius = matrix.transform_vector3(Vec3::X).length();
            (inv_radius.is_finite() && inv_radius > f32::EPSILON).then_some(1.0 / inv_radius)
        })
        .unwrap_or(1.0)
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
    transforms[left_index].rotation_pivot =
        Some(pair_medial_rotation_pivot(left_mesh, &SurfaceSide::Left));
    transforms[left_index].rotation_z_degrees = layout.open_angle_degrees;
    transforms[right_index].offset.x += half_shift;
    transforms[right_index].rotation_pivot =
        Some(pair_medial_rotation_pivot(right_mesh, &SurfaceSide::Right));
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

/// Builds the affine model matrix that moves one raw hemisphere into the active
/// paired layout, so the GPU can apply a small uniform update instead of the CPU
/// re-transforming and re-uploading every vertex on each drag frame.
///
/// The baked transform is `p -> (pivot + R*(p - pivot) + offset - center) /
/// radius`, where `pivot` is the hemisphere hinge point, `R` the Z rotation,
/// and `center`/`radius` the scene normalization. The uniform scale keeps
/// normals correct under the same matrix (the shader renormalizes them).
fn hemisphere_model_matrix(
    rotation_pivot: Vec3,
    rotation_z_degrees: f32,
    offset: Vec3,
    scene_center: Vec3,
    radius: f32,
) -> Mat4 {
    let inv_radius = 1.0 / radius;
    Mat4::from_scale(Vec3::splat(inv_radius))
        * Mat4::from_translation(rotation_pivot + offset - scene_center)
        * Mat4::from_rotation_z(rotation_z_degrees.to_radians())
        * Mat4::from_translation(-rotation_pivot)
}

#[cfg(test)]
mod acorn_matrix_tests {
    use super::hemisphere_model_matrix;
    use glam::{Quat, Vec3};

    /// Mirrors the old CPU-side per-vertex transform.
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

fn transform_point(mesh: &SurfaceMesh, transform: ComponentTransform, point: Vec3) -> Vec3 {
    let pivot = transform
        .rotation_pivot
        .unwrap_or_else(|| Vec3::from_array(mesh.bounds.center));
    let rotation = Quat::from_rotation_z(transform.rotation_z_degrees.to_radians());
    pivot + rotation * (point - pivot) + transform.offset
}

fn pair_medial_rotation_pivot(mesh: &SurfaceMesh, side: &SurfaceSide) -> Vec3 {
    let center = Vec3::from_array(mesh.bounds.center);
    let medial_x = match side {
        SurfaceSide::Left => mesh.bounds.max[0],
        SurfaceSide::Right => mesh.bounds.min[0],
        _ => center.x,
    };

    Vec3::new(medial_x, center.y, center.z)
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
    mean_half_width * open_angle_degrees.abs().to_radians().sin() * 0.9
}

fn pair_open_percent(open_angle_degrees: f32) -> f32 {
    (open_angle_degrees / PAIR_MAX_OPEN_DEGREES) * 100.0
}

fn pair_open_percent_label(open_angle_degrees: f32) -> String {
    let percent = pair_open_percent(open_angle_degrees);
    if percent.abs() < 0.5 {
        "0%".to_string()
    } else {
        format!("{percent:+.0}%")
    }
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
    display_name: String,
    rois: Vec<Roi>,
    appearance: RoiAppearance,
    node_labels: HashMap<u32, Vec<String>>,
    mapped_nodes: usize,
    skipped_nodes: usize,
}

#[derive(Debug, Clone)]
struct RoiWorkspace {
    slots: Vec<RoiSlot>,
    active_index: usize,
    next_integer_label: i32,
}

#[derive(Debug, Clone)]
struct RoiSlot {
    draft: RoiDraft,
    finalized_roi: Option<Roi>,
    editing: bool,
    visible: bool,
}

#[derive(Debug, Clone)]
struct RoiDraft {
    label: String,
    integer_label: i32,
    target: Option<RoiDraftTarget>,
    anchor_nodes: Vec<u32>,
    segments: Vec<Vec<u32>>,
    fill_nodes: Option<Vec<u32>>,
    fill_seed_node: Option<u32>,
    fill_pending: bool,
    draw_enabled: bool,
    history: Vec<RoiDraftSnapshot>,
    redo_history: Vec<RoiDraftSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoiDraftTarget {
    surface_id: SurfaceId,
    domain_id: SurfaceDomainId,
    side: SurfaceSide,
}

#[derive(Debug, Clone)]
struct RoiDraftSnapshot {
    target: Option<RoiDraftTarget>,
    anchor_nodes: Vec<u32>,
    segments: Vec<Vec<u32>>,
    fill_nodes: Option<Vec<u32>>,
    fill_seed_node: Option<u32>,
    fill_pending: bool,
    draw_enabled: bool,
}

#[derive(Debug, Clone)]
struct RoiPickTarget {
    mesh: SurfaceMesh,
    target: RoiDraftTarget,
    local_node: u32,
}

impl RoiLayer {
    fn labels_for_node(&self, node: u32) -> &[String] {
        self.node_labels
            .get(&node)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

impl Default for RoiWorkspace {
    fn default() -> Self {
        let mut workspace = Self {
            slots: Vec::new(),
            active_index: 0,
            next_integer_label: 1,
        };
        workspace.push_blank_slot();
        workspace
    }
}

impl RoiWorkspace {
    fn from_rois(rois: Vec<Roi>) -> Self {
        let next_integer_label = rois
            .iter()
            .map(|roi| roi.integer_label)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let mut workspace = Self {
            slots: rois.into_iter().map(RoiSlot::from_roi).collect(),
            active_index: 0,
            next_integer_label,
        };
        let active_index = workspace.push_blank_slot();
        workspace.active_index = active_index;
        workspace
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn has_saveable_rois(&self) -> bool {
        self.slots.iter().any(RoiSlot::has_roi)
    }

    fn active_draft(&self) -> Option<&RoiDraft> {
        self.slots
            .get(self.active_index)
            .filter(|slot| slot.editing)
            .map(|slot| &slot.draft)
    }

    fn active_draft_mut(&mut self) -> Option<&mut RoiDraft> {
        self.slots
            .get_mut(self.active_index)
            .filter(|slot| slot.editing)
            .map(|slot| &mut slot.draft)
    }

    fn set_active(&mut self, index: usize) -> bool {
        if index >= self.slots.len() {
            return false;
        }
        self.active_index = index;
        true
    }

    fn saveable_rois(&self) -> Result<Vec<Roi>> {
        self.slots
            .iter()
            .filter_map(RoiSlot::current_roi)
            .collect::<Result<Vec<_>>>()
    }

    fn saveable_roi_at(&self, index: usize) -> Result<Option<Roi>> {
        self.slots
            .get(index)
            .and_then(RoiSlot::current_roi)
            .transpose()
    }

    fn visible_rois(&self) -> Result<Vec<Roi>> {
        self.slots
            .iter()
            .filter(|slot| slot.visible)
            .filter_map(RoiSlot::current_roi)
            .collect::<Result<Vec<_>>>()
    }

    fn finalize_slot(&mut self, index: usize) -> Result<bool> {
        let Some(slot) = self.slots.get_mut(index) else {
            return Ok(false);
        };
        let Some(roi) = slot.draft.to_roi()? else {
            return Ok(false);
        };
        slot.finalized_roi = Some(roi);
        slot.editing = false;
        slot.draft.draw_enabled = false;
        slot.draft.fill_pending = false;
        slot.visible = true;
        let next_index = self.push_blank_slot();
        self.active_index = next_index;

        Ok(true)
    }

    fn edit_slot(&mut self, index: usize) -> Result<bool> {
        let Some(slot) = self.slots.get_mut(index) else {
            return Ok(false);
        };
        if slot.editing {
            self.active_index = index;
            return Ok(true);
        }
        let Some(roi) = slot.finalized_roi.as_ref() else {
            return Ok(false);
        };
        let Some(draft) = RoiDraft::from_roi(roi) else {
            return Ok(false);
        };
        slot.draft = draft;
        slot.finalized_roi = None;
        slot.editing = true;
        self.active_index = index;

        Ok(true)
    }

    fn delete_slot(&mut self, index: usize) -> bool {
        if index >= self.slots.len() {
            return false;
        }

        self.slots.remove(index);
        if self.slots.is_empty() {
            self.active_index = self.push_blank_slot();
            return true;
        }

        if self.active_index > index {
            self.active_index -= 1;
        } else if self.active_index >= self.slots.len() {
            self.active_index = self.slots.len() - 1;
        }

        if !self.slots.iter().any(|slot| slot.editing) {
            self.active_index = self.push_blank_slot();
        }

        true
    }

    fn push_blank_slot(&mut self) -> usize {
        let value = self.next_integer_label;
        self.next_integer_label = self.next_integer_label.saturating_add(1);
        self.slots
            .push(RoiSlot::blank(format!("roi_{value}"), value));
        self.slots.len() - 1
    }
}

impl RoiSlot {
    fn blank(label: String, integer_label: i32) -> Self {
        Self {
            draft: RoiDraft::new(label, integer_label),
            finalized_roi: None,
            editing: true,
            visible: true,
        }
    }

    fn from_roi(roi: Roi) -> Self {
        let draft = RoiDraft::from_roi(&roi)
            .unwrap_or_else(|| RoiDraft::new(roi.label.clone(), roi.integer_label));
        Self {
            draft,
            finalized_roi: Some(roi),
            editing: false,
            visible: true,
        }
    }

    fn has_roi(&self) -> bool {
        self.finalized_roi.is_some() || !self.draft.is_empty()
    }

    fn current_roi(&self) -> Option<Result<Roi>> {
        if self.editing {
            return self.draft.to_roi().transpose();
        }
        self.finalized_roi.clone().map(Ok)
    }

    fn label(&self) -> &str {
        if self.editing {
            &self.draft.label
        } else {
            self.finalized_roi
                .as_ref()
                .map(|roi| roi.label.as_str())
                .unwrap_or(self.draft.label.as_str())
        }
    }

    fn integer_label(&self) -> i32 {
        if self.editing {
            self.draft.integer_label
        } else {
            self.finalized_roi
                .as_ref()
                .map(|roi| roi.integer_label)
                .unwrap_or(self.draft.integer_label)
        }
    }
}

impl Default for RoiDraft {
    fn default() -> Self {
        Self::new("roi_1", 1)
    }
}

impl RoiDraft {
    fn new(label: impl Into<String>, integer_label: i32) -> Self {
        Self {
            label: label.into(),
            integer_label,
            target: None,
            anchor_nodes: Vec::new(),
            segments: Vec::new(),
            fill_nodes: None,
            fill_seed_node: None,
            fill_pending: false,
            draw_enabled: false,
            history: Vec::new(),
            redo_history: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.anchor_nodes.is_empty() && self.segments.is_empty() && self.fill_nodes.is_none()
    }

    fn is_joined(&self) -> bool {
        self.segments.last().is_some_and(|segment| {
            segment.len() >= 2
                && self.anchor_nodes.len() >= 3
                && segment.last().copied() == self.anchor_nodes.first().copied()
        })
    }

    fn can_join(&self) -> bool {
        self.anchor_nodes.len() >= 3 && !self.is_joined()
    }

    fn can_fill(&self) -> bool {
        self.is_joined()
    }

    fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    fn can_redo(&self) -> bool {
        !self.redo_history.is_empty()
    }

    fn reopen_joined_path_for_append(&mut self) {
        if self.is_joined() {
            self.segments.pop();
        }
        self.fill_nodes = None;
        self.fill_seed_node = None;
        self.fill_pending = false;
    }

    fn from_roi(roi: &Roi) -> Option<Self> {
        let mut draft = Self::new(roi.label.clone(), roi.integer_label);
        draft.target = match (&roi.parent_surface_id, &roi.parent_domain_id) {
            (Some(surface_id), Some(domain_id)) => Some(RoiDraftTarget {
                surface_id: surface_id.clone(),
                domain_id: domain_id.clone(),
                side: roi.parent_side.clone(),
            }),
            _ => None,
        };

        for datum in &roi.data {
            if datum.action == RoiBrushAction::FillArea {
                if !datum.node_path.is_empty() {
                    draft.fill_nodes = Some(datum.node_path.clone());
                }
                continue;
            }

            match datum.kind {
                RoiElementKind::NodeSegment | RoiElementKind::EdgeGroup => {
                    if datum.node_path.is_empty() {
                        continue;
                    }
                    if draft.anchor_nodes.is_empty()
                        && let Some(first) = datum.node_path.first().copied()
                    {
                        draft.anchor_nodes.push(first);
                    }
                    if datum.action != RoiBrushAction::JoinEnds
                        && let Some(last) = datum.node_path.last().copied()
                        && draft.anchor_nodes.last().copied() != Some(last)
                    {
                        draft.anchor_nodes.push(last);
                    }
                    draft.segments.push(datum.node_path.clone());
                }
                RoiElementKind::NodeGroup if !datum.node_path.is_empty() => {
                    if draft.segments.is_empty() && draft.anchor_nodes.is_empty() {
                        draft.anchor_nodes = datum.node_path.clone();
                    } else {
                        draft.fill_nodes = Some(datum.node_path.clone());
                    }
                }
                _ => {}
            }
        }

        (!draft.is_empty()).then_some(draft)
    }

    fn snapshot(&self) -> RoiDraftSnapshot {
        RoiDraftSnapshot {
            target: self.target.clone(),
            anchor_nodes: self.anchor_nodes.clone(),
            segments: self.segments.clone(),
            fill_nodes: self.fill_nodes.clone(),
            fill_seed_node: self.fill_seed_node,
            fill_pending: self.fill_pending,
            draw_enabled: self.draw_enabled,
        }
    }

    fn restore(&mut self, snapshot: RoiDraftSnapshot) {
        self.target = snapshot.target;
        self.anchor_nodes = snapshot.anchor_nodes;
        self.segments = snapshot.segments;
        self.fill_nodes = snapshot.fill_nodes;
        self.fill_seed_node = snapshot.fill_seed_node;
        self.fill_pending = snapshot.fill_pending;
        self.draw_enabled = snapshot.draw_enabled;
    }

    fn push_history(&mut self) {
        self.history.push(self.snapshot());
        self.redo_history.clear();
    }

    fn undo(&mut self) -> bool {
        let Some(snapshot) = self.history.pop() else {
            return false;
        };
        self.redo_history.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo_history.pop() else {
            return false;
        };
        self.history.push(self.snapshot());
        self.restore(snapshot);
        true
    }

    fn boundary_nodes(&self) -> Vec<u32> {
        let mut nodes = Vec::new();
        for segment in &self.segments {
            if segment.is_empty() {
                continue;
            }
            let start = usize::from(nodes.last().copied() == segment.first().copied());
            nodes.extend(segment.iter().skip(start).copied());
        }
        nodes
    }

    fn to_roi(&self) -> Result<Option<Roi>> {
        if self.is_empty() {
            return Ok(None);
        }

        let Some(target) = self.target.clone() else {
            return Ok(None);
        };
        let mut data = Vec::new();
        if self.segments.is_empty() && !self.anchor_nodes.is_empty() {
            data.push(RoiDatum::node_group(self.anchor_nodes.clone())?);
        }
        for (index, segment) in self.segments.iter().enumerate() {
            let action = if index == self.segments.len().saturating_sub(1) && self.is_joined() {
                RoiBrushAction::JoinEnds
            } else {
                RoiBrushAction::AppendStroke
            };
            data.push(RoiDatum::node_segment(segment.clone(), action)?);
        }
        if let Some(nodes) = &self.fill_nodes {
            data.push(RoiDatum::new(
                RoiElementKind::NodeGroup,
                RoiBrushAction::FillArea,
                nodes.clone(),
                Vec::new(),
            )?);
        }

        let drawing_type = if self.fill_nodes.is_some() {
            RoiDrawingType::FilledArea
        } else if self.is_joined() {
            RoiDrawingType::ClosedPath
        } else {
            RoiDrawingType::OpenPath
        };
        let roi = Roi::new(self.label.clone(), self.integer_label)?
            .with_parent_surface(target.surface_id, target.domain_id, target.side)
            .with_source(RoiSource::Drawn, None)?
            .with_style(
                roi_fill_color_for_label(self.integer_label),
                roi_edge_color_for_label(self.integer_label),
                2,
            )?
            .with_color_by_label(true)
            .with_draw_status(if self.is_joined() {
                RoiDrawStatus::Finished
            } else {
                RoiDrawStatus::InCreation
            })
            .with_drawing_type(drawing_type)
            .with_data(data)?;

        Ok(Some(roi))
    }
}

#[derive(Debug, Clone)]
struct RoiAppearanceBuild {
    appearance: RoiAppearance,
    node_labels: HashMap<u32, Vec<String>>,
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
    actions: Vec<ViewerCommand>,
    desired_control_size_points: egui::Vec2,
}

#[derive(Debug, Clone, Copy)]
enum ViewerEvent {
    SpecPreloadReady,
    AfniMessagesReady,
}

struct PreloadTask {
    generation: u64,
    surface_index: usize,
    component_index: usize,
    spec: SpecFile,
    surface: SpecSurface,
    surface_volume_idcode: Option<String>,
}

struct PreloadResult {
    generation: u64,
    surface_index: usize,
    component_index: usize,
    path: PathBuf,
    result: std::result::Result<SurfaceMesh, String>,
}

#[derive(Debug, Clone)]
struct GraphSnapshot {
    node_index: u32,
    surface_position: [f32; 3],
    surface_label: String,
    overlay_label: String,
    points: Vec<GraphPoint>,
    y_range: ValueRange,
}

#[derive(Debug, Clone)]
struct GraphPoint {
    column_index: usize,
    label: String,
    value: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AfniSurfaceTarget {
    node_offset: usize,
    node_count: usize,
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
    let open_medial = Some(MontageLayout {
        layout: HemisphereLayout::Open,
        state: HemisphereLayoutState::acorn_signed(1.0),
    });
    let open_lateral = Some(MontageLayout {
        layout: HemisphereLayout::Open,
        state: HemisphereLayoutState::acorn_signed(-1.0),
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
            layout: open_medial,
            camera: MontageCamera::Direction {
                eye_direction: Vec3::NEG_Y,
                up: Vec3::Z,
            },
            padding: MONTAGE_OPEN_PADDING,
        },
        MontageShot {
            layout: open_lateral,
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

#[derive(Debug, Clone, Copy)]
struct ComponentTransform {
    offset: Vec3,
    rotation_z_degrees: f32,
    rotation_pivot: Option<Vec3>,
}

impl Default for ComponentTransform {
    fn default() -> Self {
        Self {
            offset: Vec3::ZERO,
            rotation_z_degrees: 0.0,
            rotation_pivot: None,
        }
    }
}

struct ModeLabel {
    text: String,
    until: Instant,
}

impl BackgroundMode {
    fn color(self) -> wgpu::Color {
        match self {
            Self::Black => BLACK_BACKGROUND,
            Self::White => WHITE_BACKGROUND,
        }
    }
}

/// Picks black or white (whichever contrasts more) for framing a colorbar
/// against the given background.
fn contrasting_border(background: [u8; 4]) -> [u8; 4] {
    let luminance =
        0.299 * background[0] as f32 + 0.587 * background[1] as f32 + 0.114 * background[2] as f32;
    if luminance > 127.0 {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
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
    surface_volume_idcode: Option<&str>,
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
    apply_surface_volume_parent(mesh, surface_volume_idcode);
}

fn load_spec_component_mesh(
    spec: &SpecFile,
    surface: &SpecSurface,
    surface_volume_idcode: Option<&str>,
) -> Result<SurfaceMesh> {
    let mut mesh = SurfaceMesh::from_gifti_path(&surface.path)
        .with_context(|| format!("failed to load spec surface {}", surface.path.display()))?;
    apply_spec_surface_metadata(&mut mesh, spec, surface, surface_volume_idcode);

    Ok(mesh)
}

fn afni_component_is_sendable(
    component: &SceneSurfaceComponent,
    mesh: Option<&SurfaceMesh>,
) -> bool {
    if component.spec_surface.anatomical == Some(false) {
        return false;
    }

    if let Some(mesh) = mesh {
        if mesh.metadata.anatomically_correct == AnatomicalCorrectness::Incorrect {
            return false;
        }

        if component.spec_surface.anatomical.is_none()
            && mesh.metadata.anatomically_correct == AnatomicalCorrectness::Unknown
            && matches!(
                mesh.metadata.surface_kind,
                SurfaceKind::Inflated
                    | SurfaceKind::VeryInflated
                    | SurfaceKind::Sphere
                    | SurfaceKind::Flat
            )
        {
            return false;
        }
    }

    true
}

fn apply_surface_volume_parent(mesh: &mut SurfaceMesh, surface_volume_idcode: Option<&str>) {
    mesh.metadata.lineage.parent_volume_id = surface_volume_idcode.map(ToString::to_string);
}

fn decorate_afni_surface_info(
    info: &mut AfniSurfaceInfo,
    scene: Option<&SurfaceScene>,
    component: Option<&SceneSurfaceComponent>,
) {
    if let Some(scene) = scene {
        info.specfile_name = scene
            .spec_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToString::to_string);
        info.specfile_path = scene
            .spec_path
            .parent()
            .map(|path| path.display().to_string());
        decorate_afni_surface_volume_info(
            info,
            scene.surface_volume_path.as_ref(),
            scene.surface_volume_idcode.as_deref(),
        );
    }

    if let Some(component) = component {
        info.surface_label = component.name.clone();
        info.local_domain_parent = component
            .spec_surface
            .local_domain_parent
            .clone()
            .unwrap_or_else(|| info.local_domain_parent.clone());
    }
}

fn decorate_afni_surface_volume_info(
    info: &mut AfniSurfaceInfo,
    surface_volume_path: Option<&PathBuf>,
    surface_volume_idcode: Option<&str>,
) {
    if let Some(idcode) = surface_volume_idcode {
        info.volume_idcode = info
            .volume_idcode
            .clone()
            .or_else(|| Some(idcode.to_string()));
    }

    if let Some(path) = surface_volume_path {
        info.volume_headname = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToString::to_string);
        info.volume_filecode = Some(path.display().to_string());
        info.volume_dirname = path.parent().map(|parent| parent.display().to_string());
    }
}

/// Content hash of an incoming `SUMA_irgba` payload, used to drop AFNI's
/// redundant re-sends of an unchanged colorization for a given surface.
fn afni_rgba_overlay_signature(overlay: &AfniRgbaOverlay) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    overlay.function_idcode.hash(&mut hasher);
    overlay.threshold.hash(&mut hasher);
    overlay.node_indices.hash(&mut hasher);
    overlay.rgba.hash(&mut hasher);
    hasher.finish()
}

fn afni_rgba_to_suma_node_color(rgba: [u8; 4]) -> [f32; 4] {
    // Honor AFNI's alpha: a == 0 means "not colored, show the underlay", and
    // intermediate alphas blend the overlay over the underlay (see
    // [`afni_colors_over_underlay`]).
    [
        rgba[0] as f32 / 255.0,
        rgba[1] as f32 / 255.0,
        rgba[2] as f32 / 255.0,
        rgba[3] as f32 / 255.0,
    ]
}

/// Flatten AFNI's per-node RGBA color cache into opaque surface colors by
/// blending each node over the anatomical underlay using its alpha. AFNI colors
/// are rendered as the surface itself (not a separate overlay plane), so a
/// sub-threshold node (alpha 0) must resolve to the underlay here or it would
/// paint the surface black.
fn afni_colors_over_underlay(afni: &[[f32; 4]], underlay: Option<&[[f32; 4]]>) -> Vec<[f32; 4]> {
    afni.iter()
        .enumerate()
        .map(|(index, color)| {
            let base = underlay
                .and_then(|colors| colors.get(index))
                .copied()
                .unwrap_or(mesh::DEFAULT_SURFACE_COLOR);
            let alpha = color[3].clamp(0.0, 1.0);
            [
                base[0] * (1.0 - alpha) + color[0] * alpha,
                base[1] * (1.0 - alpha) + color[1] * alpha,
                base[2] * (1.0 - alpha) + color[2] * alpha,
                1.0,
            ]
        })
        .collect()
}

fn apply_afni_rgba_to_color_cache(
    existing: Option<Vec<[f32; 4]>>,
    mesh: &SurfaceMesh,
    target: AfniSurfaceTarget,
    overlay: &AfniRgbaOverlay,
) -> (Vec<[f32; 4]>, usize, usize) {
    let total_node_count = mesh.vertices.len();
    let mut colors = existing
        .filter(|colors| colors.len() == total_node_count)
        .unwrap_or_else(|| vec![AFNI_TRANSPARENT_NODE_COLOR; total_node_count]);
    let start = target.node_offset.min(total_node_count);
    let end = target
        .node_offset
        .saturating_add(target.node_count)
        .min(total_node_count);
    // Clear this surface's slice to transparent so any node AFNI does not send
    // falls back to the underlay (SUMA leaves un-sent nodes uncolored rather
    // than smearing nearby colors across them).
    for color in &mut colors[start..end] {
        *color = AFNI_TRANSPARENT_NODE_COLOR;
    }

    let mut applied = 0usize;
    let mut skipped = 0usize;
    for (node, rgba) in overlay.node_indices.iter().zip(&overlay.rgba) {
        let local_node = *node as usize;
        if local_node >= target.node_count {
            skipped += 1;
            continue;
        }
        let Some(index) = target.node_offset.checked_add(local_node) else {
            skipped += 1;
            continue;
        };
        if let Some(color) = colors.get_mut(index) {
            *color = afni_rgba_to_suma_node_color(*rgba);
            applied += 1;
        } else {
            skipped += 1;
        }
    }
    (colors, applied, skipped)
}

fn afni_surface_target_in_scene_surface(
    scene: &SurfaceScene,
    surface_index: usize,
    matches: impl Fn(&SceneSurfaceComponent, &SurfaceMesh) -> bool,
) -> Option<AfniSurfaceTarget> {
    let surface = scene.surfaces.get(surface_index)?;
    let mut node_offset = 0usize;
    for component in &surface.components {
        let mesh = component.mesh.as_ref()?;
        let node_count = mesh.vertices.len();
        if matches(component, mesh) {
            return Some(AfniSurfaceTarget {
                node_offset,
                node_count,
            });
        }
        node_offset = node_offset.checked_add(node_count)?;
    }

    None
}

fn afni_component_matches_surface_id(
    component: &SceneSurfaceComponent,
    mesh: &SurfaceMesh,
    surface_idcode: &str,
) -> bool {
    mesh.metadata.id.as_str() == surface_idcode || component.name == surface_idcode
}

fn afni_component_matches_domain_parent(
    component: &SceneSurfaceComponent,
    mesh: &SurfaceMesh,
    parent_id: &str,
) -> bool {
    afni_component_domain_parent_candidates(component, mesh)
        .into_iter()
        .any(|candidate| candidate == parent_id)
}

fn afni_mesh_matches_domain_parent(mesh: &SurfaceMesh, parent_id: &str) -> bool {
    mesh.metadata
        .lineage
        .local_domain_parent
        .as_deref()
        .into_iter()
        .chain(std::iter::once(mesh.metadata.lineage.domain.id.as_str()))
        .chain(std::iter::once(mesh.metadata.id.as_str()))
        .any(|candidate| candidate == parent_id)
}

fn afni_component_domain_parent_candidates<'a>(
    component: &'a SceneSurfaceComponent,
    mesh: &'a SurfaceMesh,
) -> Vec<&'a str> {
    let mut candidates = Vec::new();
    push_unique_candidate(&mut candidates, mesh.metadata.id.as_str());
    push_unique_candidate(&mut candidates, component.name.as_str());
    push_unique_candidate(&mut candidates, mesh.metadata.lineage.domain.id.as_str());
    if let Some(parent) = mesh.metadata.lineage.local_domain_parent.as_deref() {
        push_unique_candidate(&mut candidates, parent);
    }
    if let Some(parent) = component.spec_surface.local_domain_parent.as_deref() {
        push_unique_candidate(&mut candidates, parent);
    }
    candidates
}

fn push_unique_candidate<'a>(candidates: &mut Vec<&'a str>, value: &'a str) {
    if !value.is_empty() && !candidates.contains(&value) {
        candidates.push(value);
    }
}

fn query_afni_dataset_idcode_optional(path: Option<&Path>) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let output = match Command::new("3dinfo").arg("-id").arg(path).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to query AFNI idcode for {}", path.display()));
        }
    };
    if !output.status.success() {
        return Ok(None);
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() || value == "NO-DSET" {
        Ok(None)
    } else {
        Ok(Some(value))
    }
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

fn save_roi_file(
    title: &str,
    default_name: &str,
    current_path: Option<&PathBuf>,
) -> Option<PathBuf> {
    // macOS save panels hide the final extension when file-type filters are
    // applied. Keep the visible default as `*.niml.roi`, then normalize below.
    let dialog = dialog_with_start_directory(
        rfd::FileDialog::new()
            .set_title(title)
            .set_file_name(default_name),
        current_path,
    );

    dialog.save_file().map(append_niml_roi_extension)
}

fn append_niml_roi_extension(path: PathBuf) -> PathBuf {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return path.with_extension("niml.roi");
    };
    let name = name.to_string();
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".niml.roi") {
        path
    } else if lower.ends_with(".roi") {
        path.with_extension("niml.roi")
    } else {
        path.with_extension("niml.roi")
    }
}

fn roi_save_default_name(roi: &Roi, _surface_path: Option<&PathBuf>) -> String {
    let label = sanitize_file_stem(&roi.label);

    format!("{label}.niml.roi")
}

fn roi_save_all_default_name(current_path: Option<&PathBuf>) -> String {
    current_path
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .map(|name| append_niml_roi_extension(PathBuf::from(name)))
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "sumaru_rois.niml.roi".to_string())
}

fn sanitize_file_stem(value: &str) -> String {
    let mut out = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "roi".to_string()
    } else {
        out
    }
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

fn explicit_overlay_pair_display_name(pair: &ExplicitOverlayPair) -> String {
    format!(
        "LH {} / RH {}",
        file_name_display(&pair.left_path),
        file_name_display(&pair.right_path)
    )
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
        left: "lh.",
        right: "rh.",
        wildcard: "?h.",
    },
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
    let right_fdr_curve = right.fdr_curve;
    let left_fdr_curve = left.fdr_curve.clone();
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
    let fdr_curve = if left_fdr_curve == right_fdr_curve {
        left_fdr_curve
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
    .with_stat(stat)
    .with_fdr_curve(fdr_curve))
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
    let mut node_labels: HashMap<u32, Vec<String>> = HashMap::new();
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
    node_labels: &mut HashMap<u32, Vec<String>>,
    mapped: &mut BTreeSet<u32>,
) -> usize {
    let mut skipped = 0usize;
    let label = roi_display_label(roi);

    for node in roi_datum_nodes(roi, datum, mesh, ranges) {
        match node {
            Some(node) if appearance.set_node_color(node, color) => {
                mapped.insert(node);
                let labels = node_labels.entry(node).or_default();
                if !labels.contains(&label) {
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

fn roi_fill_nodes_from_seed(mesh: &SurfaceMesh, boundary: &[u32], seed: u32) -> Result<Vec<u32>> {
    ensure!(
        boundary.len() >= 3,
        "ROI fill requires a joined boundary with at least three nodes"
    );
    ensure!(
        mesh.domain.contains_node(seed),
        "ROI fill seed node {} is outside node count {}",
        seed,
        mesh.domain.node_count
    );

    let boundary_set = boundary.iter().copied().collect::<HashSet<_>>();
    ensure!(
        !boundary_set.contains(&seed),
        "ROI fill seed cannot be on the boundary"
    );

    let mut blocked_edges = HashSet::new();
    for pair in boundary.windows(2) {
        blocked_edges.insert(edge_pair_key(pair[0], pair[1]));
    }
    if boundary.first() != boundary.last()
        && let (Some(first), Some(last)) = (boundary.first(), boundary.last())
    {
        blocked_edges.insert(edge_pair_key(*first, *last));
    }

    let topology = mesh.topology();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::from([seed]);
    visited.insert(seed);

    while let Some(node) = queue.pop_front() {
        for neighbor in &topology.node_neighbors[node as usize] {
            if boundary_set.contains(neighbor) {
                continue;
            }
            if blocked_edges.contains(&edge_pair_key(node, *neighbor)) {
                continue;
            }
            if visited.insert(*neighbor) {
                queue.push_back(*neighbor);
            }
        }
    }

    visited.extend(boundary_set);
    Ok(NodeMask::from_nodes(mesh.vertices.len(), visited)?.nodes())
}

fn edge_pair_key(a: u32, b: u32) -> (u32, u32) {
    if a <= b { (a, b) } else { (b, a) }
}

fn roi_display_label(roi: &Roi) -> String {
    format!("{} ({})", roi.label, roi.integer_label)
}

fn direct_anatomical_shading_colors(mesh: &SurfaceMesh) -> Vec<[f32; 4]> {
    let Some(parent_path) = mesh_curvature_parent_path(mesh) else {
        return anatomical_shading_colors(mesh);
    };

    let source_path = mesh.metadata.source_file.as_ref();
    if source_path.is_some_and(|source_path| *source_path == parent_path) {
        return anatomical_shading_colors(mesh);
    }

    let Ok(parent_mesh) = SurfaceMesh::from_gifti_path(&parent_path) else {
        return anatomical_shading_colors(mesh);
    };

    if parent_mesh.vertices.len() == mesh.vertices.len() {
        anatomical_shading_colors(&parent_mesh)
    } else {
        anatomical_shading_colors(mesh)
    }
}

fn scene_anatomical_shading_colors(
    scene: &SurfaceScene,
    surface: &SceneSurface,
    display_mesh: &SurfaceMesh,
) -> Vec<[f32; 4]> {
    if surface.components.is_empty() {
        return anatomical_shading_colors(display_mesh);
    }

    let mut colors = Vec::with_capacity(display_mesh.vertices.len());
    for component in &surface.components {
        let Some(mesh) = component.mesh.as_ref() else {
            return anatomical_shading_colors(display_mesh);
        };
        let component_colors = component_anatomical_shading_colors(scene, component, mesh);
        colors.extend(component_colors);
    }

    if colors.len() == display_mesh.vertices.len() {
        colors
    } else {
        anatomical_shading_colors(display_mesh)
    }
}

fn component_anatomical_shading_colors(
    scene: &SurfaceScene,
    component: &SceneSurfaceComponent,
    mesh: &SurfaceMesh,
) -> Vec<[f32; 4]> {
    let Some(parent_path) = component_curvature_parent_path(scene, component) else {
        return anatomical_shading_colors(mesh);
    };

    if parent_path == component.path {
        return anatomical_shading_colors(mesh);
    }

    let Ok(parent_mesh) = SurfaceMesh::from_gifti_path(&parent_path) else {
        return anatomical_shading_colors(mesh);
    };

    if parent_mesh.vertices.len() == mesh.vertices.len() {
        anatomical_shading_colors(&parent_mesh)
    } else {
        anatomical_shading_colors(mesh)
    }
}

fn mesh_curvature_parent_path(mesh: &SurfaceMesh) -> Option<PathBuf> {
    let source_file = mesh.metadata.source_file.as_ref()?;
    let lineage = &mesh.metadata.lineage;
    if let Some(parent_name) = lineage
        .local_curvature_parent
        .as_deref()
        .or(lineage.local_domain_parent.as_deref())
        && let Some(path) = resolve_surface_parent_path(source_file, parent_name)
    {
        return Some(path);
    }

    infer_smoothwm_parent_path(mesh)
}

fn resolve_surface_parent_path(source_file: &Path, parent_name: &str) -> Option<PathBuf> {
    let parent_path = Path::new(parent_name);
    let path = if parent_path.is_absolute() {
        parent_path.to_path_buf()
    } else {
        source_file.parent()?.join(parent_path)
    };

    Some(canonical_or_original_path(path))
}

fn infer_smoothwm_parent_path(mesh: &SurfaceMesh) -> Option<PathBuf> {
    if !matches!(
        mesh.metadata.surface_kind,
        SurfaceKind::Inflated | SurfaceKind::VeryInflated
    ) {
        return None;
    }
    let source_file = mesh.metadata.source_file.as_ref()?;
    let directory = source_file.parent()?;
    let file_name = source_file.file_name()?.to_string_lossy();
    let candidates = [
        file_name.replace(".veryinflated.", ".smoothwm."),
        file_name.replace(".inflated.", ".smoothwm."),
        file_name.replace("veryinflated", "smoothwm"),
        file_name.replace("inflated", "smoothwm"),
    ];

    candidates
        .into_iter()
        .map(|name| canonical_or_original_path(directory.join(name)))
        .find(|path| path.exists())
}

fn component_curvature_parent_path(
    scene: &SurfaceScene,
    component: &SceneSurfaceComponent,
) -> Option<PathBuf> {
    let parent_name = component
        .spec_surface
        .local_curvature_parent
        .as_deref()
        .or(component.spec_surface.local_domain_parent.as_deref())?;
    if parent_name == component.name {
        return Some(component.path.clone());
    }

    scene
        .spec
        .surfaces
        .iter()
        .find(|surface| surface.name == parent_name)
        .map(|surface| surface.path.clone())
}

fn anatomical_shading_colors(mesh: &SurfaceMesh) -> Vec<[f32; 4]> {
    let convexity = mesh.suma_convexity();
    let convexity = mesh
        .smooth_scalar_values(
            &convexity,
            SUMA_CONVEXITY_SMOOTHING_ITERATIONS,
            SmoothingWeights::Uniform,
            None,
        )
        .unwrap_or(convexity);

    anatomical_shading_colors_from_values(&convexity)
}

fn anatomical_shading_colors_from_values(values: &[f32]) -> Vec<[f32; 4]> {
    let Some((low, high)) = robust_finite_range(values) else {
        return vec![mesh::DEFAULT_SURFACE_COLOR; values.len()];
    };
    let span = (high - low).abs();
    if span <= f32::EPSILON {
        return vec![mesh::DEFAULT_SURFACE_COLOR; values.len()];
    }

    let midpoint = low + span * 0.5;
    values
        .iter()
        .map(|value| {
            // SUMA's convexity defaults use gray02 with SUMA_NO_INTERP, so
            // convexity is displayed as two gray bins rather than a smooth ramp.
            let convexity_gray = if value.is_finite() && *value >= midpoint {
                0.70
            } else {
                0.40
            };
            [
                blend_color_channel(mesh::DEFAULT_SURFACE_COLOR[0], convexity_gray),
                blend_color_channel(mesh::DEFAULT_SURFACE_COLOR[1], convexity_gray),
                blend_color_channel(mesh::DEFAULT_SURFACE_COLOR[2], convexity_gray),
                1.0,
            ]
        })
        .collect()
}

fn blend_color_channel(base: f32, overlay: f32) -> f32 {
    base * (1.0 - SUMA_CONVEXITY_OPACITY) + overlay * SUMA_CONVEXITY_OPACITY
}

fn robust_finite_range(values: &[f32]) -> Option<(f32, f32)> {
    let mut finite = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if finite.is_empty() {
        return None;
    }
    finite.sort_by(f32::total_cmp);
    let last = finite.len().saturating_sub(1);
    let low_index = ((last as f32) * 0.05).round() as usize;
    let high_index = ((last as f32) * 0.95).round() as usize;
    let mut low = finite[low_index];
    let mut high = finite[high_index];
    if finite[0] < 0.0 && finite[last] > 0.0 {
        if low.abs() > high.abs() {
            high = -low;
        } else {
            low = -high;
        }
    }

    Some((low, high))
}

fn roi_fill_color_for_label(integer_label: i32) -> Rgba {
    const PALETTE: [[u8; 3]; 10] = [
        [239, 58, 49],
        [48, 166, 86],
        [48, 116, 230],
        [239, 181, 42],
        [205, 82, 206],
        [28, 175, 190],
        [241, 126, 40],
        [139, 93, 224],
        [142, 196, 58],
        [228, 77, 126],
    ];
    let label = integer_label.max(1);
    let index = (label - 1).rem_euclid(PALETTE.len() as i32) as usize;
    let [red, green, blue] = PALETTE[index];

    Rgba::from_u8(red, green, blue, 205)
}

fn roi_edge_color_for_label(integer_label: i32) -> Rgba {
    let fill = roi_fill_color_for_label(integer_label);

    Rgba::new_unchecked(fill.red * 0.28, fill.green * 0.28, fill.blue * 0.28, 1.0)
}

fn roi_slot_state_text(slot: &RoiSlot) -> String {
    if slot.editing {
        if slot.draft.is_empty() {
            "editing, empty".to_string()
        } else {
            "editing".to_string()
        }
    } else {
        "finalized".to_string()
    }
}

fn roi_draft_status_text(draft: &RoiDraft) -> String {
    if draft.is_empty() {
        if draft.draw_enabled {
            return "draw armed".to_string();
        }
        return "none".to_string();
    }

    let mut parts = vec![
        format!("{} anchors", draft.anchor_nodes.len()),
        format!("{} segments", draft.segments.len()),
    ];
    if draft.is_joined() {
        parts.push("joined".to_string());
    }
    if let Some(nodes) = &draft.fill_nodes {
        parts.push(format!("{} filled nodes", nodes.len()));
    } else if draft.fill_pending {
        parts.push("fill armed".to_string());
    }

    parts.join(", ")
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
    let mut values = vec![f32::NAN; node_count];
    let mut threshold_values = threshold_column.map(|_| vec![f32::NAN; node_count]);

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
            }
        }
    }

    let range = ValueRange::from_values(&values)?;
    Ok(OverlayDataset {
        values,
        range,
        threshold_values,
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

fn dataset_row_for_node(dataset: &Dataset, node: u32) -> Option<usize> {
    if let Some(indices) = dataset.node_indices.as_ref() {
        indices.iter().position(|candidate| *candidate == node)
    } else {
        let row = node as usize;
        (row < dataset.row_count).then_some(row)
    }
}

fn graph_column_label(index: usize, column: &DataColumn) -> String {
    column.stat.as_ref().map_or_else(
        || format!("#{index} {}", column.label),
        |stat| format!("#{index} {} [{}]", column.label, compact_stat_label(stat)),
    )
}

fn compact_stat_label(stat: &str) -> &str {
    stat.split_once('(').map_or(stat, |(label, _)| label).trim()
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

fn draw_graph_snapshot(
    ui: &mut egui::Ui,
    snapshot: &GraphSnapshot,
    columns: OverlayColumnSelections,
) {
    let available_height = ui.available_height();
    let plot_height = (available_height - 32.0).clamp(
        GRAPH_MIN_PLOT_HEIGHT_POINTS,
        GRAPH_DEFAULT_PLOT_HEIGHT_POINTS.max(available_height),
    );
    let plot_size = egui::vec2(
        ui.available_width().max(GRAPH_MIN_PLOT_WIDTH_POINTS),
        plot_height,
    );
    let (rect, _) = ui.allocate_exact_size(plot_size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let plot_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(54.0, 14.0),
        rect.max - egui::vec2(70.0, 56.0),
    );
    let axis_color = egui::Color32::from_rgb(92, 103, 122);
    let grid_color = egui::Color32::from_rgb(43, 50, 62);
    let line_color = egui::Color32::from_rgb(123, 184, 226);

    painter.rect_filled(rect, egui::CornerRadius::same(6), panel_fill_color());
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(1.0, border_color()),
        egui::StrokeKind::Outside,
    );

    for step in 0..=4 {
        let t = step as f32 / 4.0;
        let y = egui::lerp(plot_rect.bottom()..=plot_rect.top(), t);
        painter.line_segment(
            [
                egui::pos2(plot_rect.left(), y),
                egui::pos2(plot_rect.right(), y),
            ],
            egui::Stroke::new(1.0, grid_color),
        );
    }
    painter.line_segment(
        [
            egui::pos2(plot_rect.left(), plot_rect.top()),
            egui::pos2(plot_rect.left(), plot_rect.bottom()),
        ],
        egui::Stroke::new(1.0, axis_color),
    );
    painter.line_segment(
        [
            egui::pos2(plot_rect.left(), plot_rect.bottom()),
            egui::pos2(plot_rect.right(), plot_rect.bottom()),
        ],
        egui::Stroke::new(1.0, axis_color),
    );

    let y_min = snapshot.y_range.min;
    let y_max = snapshot.y_range.max;
    painter.text(
        egui::pos2(rect.left() + 8.0, plot_rect.top() - 6.0),
        egui::Align2::LEFT_TOP,
        format!("{y_max:.4}"),
        egui::FontId::monospace(12.0),
        muted_color(),
    );
    painter.text(
        egui::pos2(rect.left() + 8.0, plot_rect.bottom() - 12.0),
        egui::Align2::LEFT_TOP,
        format!("{y_min:.4}"),
        egui::FontId::monospace(12.0),
        muted_color(),
    );

    let points = graph_plot_positions(snapshot, plot_rect);
    for pair in points.windows(2) {
        painter.line_segment([pair[0].1, pair[1].1], egui::Stroke::new(2.0, line_color));
    }

    for (index, position) in &points {
        let point = &snapshot.points[*index];
        let (color, radius) = graph_point_style(columns, point.column_index);
        painter.circle_filled(*position, radius, color);
        painter.circle_stroke(
            *position,
            radius,
            egui::Stroke::new(1.0, egui::Color32::BLACK),
        );
    }

    for (index, position) in points.iter().step_by(graph_label_stride(points.len())) {
        let label = &snapshot.points[*index].label;
        draw_rotated_graph_label(
            &painter,
            egui::pos2(position.x, plot_rect.bottom() + 8.0),
            &truncate_middle(label, 18),
        );
    }

    ui.horizontal_wrapped(|ui| {
        graph_legend_chip(ui, "I", egui::Color32::from_rgb(123, 184, 226));
        graph_legend_chip(ui, "T", egui::Color32::from_rgb(246, 199, 94));
        graph_legend_chip(ui, "B", egui::Color32::from_rgb(170, 132, 255));
        ui.label(egui::RichText::new("other numeric sub-bricks").color(muted_color()));
        if let Some(current) = snapshot
            .points
            .iter()
            .find(|point| point.column_index == columns.intensity)
        {
            ui.separator();
            ui.label(format!(
                "I {} = {:.6}",
                truncate_middle(&current.label, 24),
                current.value
            ));
        }
        if let Some(current) = snapshot
            .points
            .iter()
            .find(|point| Some(point.column_index) == columns.threshold)
        {
            ui.separator();
            ui.label(format!(
                "T {} = {:.6}",
                truncate_middle(&current.label, 24),
                current.value
            ));
        }
        if let Some(current) = snapshot
            .points
            .iter()
            .find(|point| Some(point.column_index) == columns.brightness)
        {
            ui.separator();
            ui.label(format!(
                "B {} = {:.6}",
                truncate_middle(&current.label, 24),
                current.value
            ));
        }
    });
}

fn draw_rotated_graph_label(painter: &egui::Painter, anchor: egui::Pos2, label: &str) {
    let font_id = egui::FontId::monospace(10.0);
    let color = muted_color();
    let galley = painter.layout_no_wrap(label.to_string(), font_id, color);
    let rect = egui::Align2::CENTER_TOP.anchor_size(anchor, galley.size());
    let text_shape = egui::epaint::TextShape::new(rect.min, galley, color)
        .with_override_text_color(color)
        .with_angle_and_anchor(std::f32::consts::FRAC_PI_4, egui::Align2::CENTER_TOP);
    painter.add(egui::Shape::Text(text_shape));
}

fn graph_plot_positions(snapshot: &GraphSnapshot, rect: egui::Rect) -> Vec<(usize, egui::Pos2)> {
    let count = snapshot.points.len();
    let denominator = count.saturating_sub(1).max(1) as f32;
    snapshot
        .points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let x_t = index as f32 / denominator;
            let y_t = ((point.value - snapshot.y_range.min)
                / (snapshot.y_range.max - snapshot.y_range.min))
                .clamp(0.0, 1.0);
            let x = egui::lerp(rect.left()..=rect.right(), x_t);
            let y = egui::lerp(rect.bottom()..=rect.top(), y_t);
            (index, egui::pos2(x, y))
        })
        .collect()
}

fn graph_point_style(
    columns: OverlayColumnSelections,
    column_index: usize,
) -> (egui::Color32, f32) {
    if column_index == columns.intensity {
        (egui::Color32::from_rgb(123, 184, 226), 5.0)
    } else if Some(column_index) == columns.threshold {
        (egui::Color32::from_rgb(246, 199, 94), 5.0)
    } else if Some(column_index) == columns.brightness {
        (egui::Color32::from_rgb(170, 132, 255), 4.5)
    } else {
        (egui::Color32::from_rgb(210, 216, 224), 3.5)
    }
}

fn graph_label_stride(point_count: usize) -> usize {
    (point_count / 8).max(1)
}

fn graph_legend_chip(ui: &mut egui::Ui, label: &str, color: egui::Color32) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        ui.label(label);
    });
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

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars || max_chars < 5 {
        return value.to_string();
    }

    let marker = "...";
    let left_count = (max_chars - marker.len()) / 2;
    let right_count = max_chars - marker.len() - left_count;
    let left = value.chars().take(left_count).collect::<String>();
    let right = value
        .chars()
        .rev()
        .take(right_count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{left}{marker}{right}")
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

fn accent_color() -> egui::Color32 {
    egui::Color32::from_rgb(123, 184, 226)
}

fn panel_fill_color() -> egui::Color32 {
    egui::Color32::from_rgb(28, 32, 39)
}

fn border_color() -> egui::Color32 {
    egui::Color32::from_rgb(55, 62, 74)
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

fn repaint_delay_to_instant(full_output: &egui::FullOutput) -> Option<Instant> {
    let repaint_delay = full_output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|viewport| viewport.repaint_delay)
        .unwrap_or(Duration::MAX);
    if repaint_delay == Duration::ZERO {
        Some(Instant::now())
    } else if repaint_delay == Duration::MAX {
        None
    } else {
        Instant::now().checked_add(repaint_delay)
    }
}

fn upload_pending_egui_textures(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut Renderer,
    pending: &egui::TexturesDelta,
    allocated: &mut HashSet<egui::TextureId>,
) -> (bool, egui::TexturesDelta) {
    let mut retained = egui::TexturesDelta::default();
    let mut needs_repaint = false;
    for (id, image_delta) in &pending.set {
        if image_delta.pos.is_some() && !allocated.contains(id) {
            retained.set.push((*id, image_delta.clone()));
            needs_repaint = true;
            continue;
        }
        renderer.update_texture(device, queue, *id, image_delta);
        allocated.insert(*id);
    }
    (needs_repaint, retained)
}

fn free_pending_egui_textures(
    renderer: &mut Renderer,
    pending: &egui::TexturesDelta,
    allocated: &mut HashSet<egui::TextureId>,
) {
    for id in &pending.free {
        if allocated.remove(id) {
            renderer.free_texture(id);
        }
    }
}

fn graph_initial_inner_size(view_size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    let max_width = graph_max_inner_width(view_size);
    let max_height = graph_max_inner_height(view_size);
    PhysicalSize::new(
        bounded_initial_graph_dimension(
            GRAPH_WINDOW_INNER_WIDTH,
            GRAPH_MIN_INITIAL_INNER_WIDTH,
            max_width,
        ),
        bounded_initial_graph_dimension(
            GRAPH_WINDOW_INNER_HEIGHT,
            GRAPH_MIN_INITIAL_INNER_HEIGHT,
            max_height,
        ),
    )
}

fn bounded_initial_graph_dimension(preferred: u32, min: u32, max: u32) -> u32 {
    preferred.min(max).max(min.min(max))
}

fn graph_max_inner_width(view_size: PhysicalSize<u32>) -> u32 {
    ((view_size.width.max(1) as f32 * GRAPH_MAX_VIEW_WIDTH_FRACTION).round() as u32).max(1)
}

fn graph_max_inner_height(view_size: PhysicalSize<u32>) -> u32 {
    ((view_size.height.max(1) as f32 * GRAPH_MAX_VIEW_HEIGHT_FRACTION).round() as u32).max(1)
}

fn desired_panel_size(
    window: &Window,
    desired_points: egui::Vec2,
    pixels_per_point: f32,
    min_width: u32,
    min_height: u32,
    max_width_factor: f32,
    max_width_cap: u32,
    max_height_factor: f32,
    fallback_height: u32,
) -> Option<PhysicalSize<u32>> {
    if desired_points.x <= 0.0 || desired_points.y <= 0.0 {
        return None;
    }
    let monitor_size = window.current_monitor().map(|monitor| monitor.size());
    let max_width = monitor_size
        .map(|size| ((size.width as f32 * max_width_factor) as u32).min(max_width_cap))
        .unwrap_or(max_width_cap)
        .max(min_width);
    let max_height = monitor_size
        .map(|size| (size.height as f32 * max_height_factor) as u32)
        .unwrap_or(fallback_height)
        .max(min_height);
    Some(PhysicalSize::new(
        ((desired_points.x * pixels_per_point).ceil() as u32).clamp(min_width, max_width),
        ((desired_points.y * pixels_per_point).ceil() as u32).clamp(min_height, max_height),
    ))
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

pub(super) fn symmetric_value_range(range: ValueRange) -> ValueRange {
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

fn threshold_q_value_display(qvalue: f64) -> String {
    if qvalue < 0.001 {
        format!("q <= {qvalue:.2e}")
    } else {
        format!("q <= {qvalue:.4}")
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
        AfniSurfaceTarget, BackgroundMode, HemisphereLayout, HemisphereLayoutState, MontageCamera,
        OverlayAppearance, OverlayColumnSelections, PAIR_MAX_DRAG_GAP_FACTOR,
        PAIR_MAX_OPEN_DEGREES, PAIR_OPEN_DEGREES_PER_PIXEL, PairVisibility, PresetOrientation,
        RoiComponentRange, RoiDraftTarget, RoiWorkspace, SceneSurface, SceneSurfaceComponent,
        SurfacePick, afni_component_is_sendable, afni_rgba_overlay_signature,
        apply_afni_rgba_to_color_cache, canonical_overlay_columns, component_transforms,
        pair_hemisphere_matrices, paired_component_for_node, paired_overlay_dataset,
        paired_overlay_path_for_side, paired_overlay_paths, paired_spec_montage_shots,
        resolve_overlay_subs, roi_appearance_for_mesh, roi_fill_nodes_from_seed,
        scene_surface_display_label, scene_surfaces_from_components, selection_for_component,
        selection_scale_from_model_matrices, standard_montage_shots, surface_pick_for_mesh_node,
        threshold_and_mask_from_appearance, timestamped_png_name_from_unix_seconds,
    };
    use crate::afni::AfniRgbaOverlay;
    use crate::color::Rgba;
    use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind};
    use crate::overlay::{MaskMode, Threshold};
    use crate::roi::Roi;
    use crate::spec::{SpecFile, SpecHemisphere, SpecSurface};
    use crate::surface::{
        AnatomicalCorrectness, OverlayDataset, SurfaceDomain, SurfaceKind, SurfaceMesh,
        SurfaceSide, ValueRange,
    };
    use glam::{Mat4, Vec3};
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
    fn afni_rgba_overlay_signature_detects_payload_changes() {
        let base = AfniRgbaOverlay {
            surface_idcode: "lh".to_string(),
            local_domain_parent_id: Some("lh.smoothwm".to_string()),
            node_indices: vec![1, 2],
            rgba: vec![[255, 0, 0, 255], [0, 255, 0, 255]],
            threshold: Some("0.0001".to_string()),
            function_idcode: Some("func-a".to_string()),
            volume_idcode: Some("vol".to_string()),
        };

        // A resend of the identical colorization hashes the same.
        assert_eq!(
            afni_rgba_overlay_signature(&base),
            afni_rgba_overlay_signature(&base.clone())
        );

        // Differences in any colorization-relevant field change the hash.
        let mut recolored = base.clone();
        recolored.rgba[0] = [254, 0, 0, 255];
        assert_ne!(
            afni_rgba_overlay_signature(&base),
            afni_rgba_overlay_signature(&recolored)
        );

        let mut new_function = base.clone();
        new_function.function_idcode = Some("func-b".to_string());
        assert_ne!(
            afni_rgba_overlay_signature(&base),
            afni_rgba_overlay_signature(&new_function)
        );

        // The wire surface idcode keys the cache separately, so it is not part
        // of the payload hash itself.
        let mut renamed = base.clone();
        renamed.surface_idcode = "rh".to_string();
        assert_eq!(
            afni_rgba_overlay_signature(&base),
            afni_rgba_overlay_signature(&renamed)
        );
    }

    #[test]
    fn afni_rgba_sparse_packets_clear_only_the_target_surface_slice() {
        let mesh = SurfaceMesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
            ],
            vec![[0, 2, 3]],
        )
        .unwrap();
        let existing = vec![
            [0.1, 0.1, 0.1, 1.0],
            [0.2, 0.2, 0.2, 1.0],
            [0.3, 0.3, 0.3, 1.0],
            [0.4, 0.4, 0.4, 1.0],
        ];
        let overlay = AfniRgbaOverlay {
            surface_idcode: "lh".to_string(),
            local_domain_parent_id: Some("lh.smoothwm".to_string()),
            node_indices: vec![1],
            rgba: vec![[255, 0, 0, 255]],
            threshold: None,
            function_idcode: None,
            volume_idcode: None,
        };

        let (colors, applied, skipped) = apply_afni_rgba_to_color_cache(
            Some(existing),
            &mesh,
            AfniSurfaceTarget {
                node_offset: 0,
                node_count: 2,
            },
            &overlay,
        );

        assert_eq!(applied, 1);
        assert_eq!(skipped, 0);
        // Un-sent node inside the target slice clears to transparent...
        assert_eq!(colors[0], [0.0, 0.0, 0.0, 0.0]);
        // ...the sent node takes its color...
        assert_eq!(colors[1], [1.0, 0.0, 0.0, 1.0]);
        // ...and nodes outside the target slice are left untouched.
        assert_eq!(colors[2], [0.3, 0.3, 0.3, 1.0]);
        assert_eq!(colors[3], [0.4, 0.4, 0.4, 1.0]);
    }

    #[test]
    fn afni_rgba_sparse_packets_honor_alpha_and_leave_unsent_transparent() {
        let mesh = SurfaceMesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [100.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
            ],
            vec![[0, 1, 3], [1, 2, 3]],
        )
        .unwrap();
        let overlay = AfniRgbaOverlay {
            surface_idcode: "lh".to_string(),
            local_domain_parent_id: None,
            node_indices: vec![0, 2],
            rgba: vec![[64, 128, 255, 255], [200, 10, 10, 0]],
            threshold: None,
            function_idcode: None,
            volume_idcode: None,
        };

        let (colors, applied, skipped) = apply_afni_rgba_to_color_cache(
            None,
            &mesh,
            AfniSurfaceTarget {
                node_offset: 0,
                node_count: 4,
            },
            &overlay,
        );

        assert_eq!(applied, 2);
        assert_eq!(skipped, 0);
        // Sent node keeps its color at full alpha.
        assert_eq!(colors[0], [64.0 / 255.0, 128.0 / 255.0, 1.0, 1.0]);
        // Sent node with zero alpha is honored as transparent (shows underlay).
        assert_eq!(colors[2], [200.0 / 255.0, 10.0 / 255.0, 10.0 / 255.0, 0.0]);
        // Un-sent nodes are left transparent rather than filled by proximity.
        assert_eq!(colors[1], [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(colors[3], [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn afni_colors_resolve_alpha_against_the_underlay() {
        let afni = vec![
            [1.0, 0.0, 0.0, 1.0], // opaque: keeps its own color
            [0.0, 1.0, 0.0, 0.0], // transparent: falls back to underlay
            [0.0, 0.0, 1.0, 0.5], // half: blends with underlay
        ];
        let underlay = vec![
            [0.2, 0.2, 0.2, 1.0],
            [0.4, 0.4, 0.4, 1.0],
            [0.6, 0.6, 0.6, 1.0],
        ];

        let composed = super::afni_colors_over_underlay(&afni, Some(&underlay));

        assert_eq!(composed[0], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(composed[1], [0.4, 0.4, 0.4, 1.0]);
        assert_eq!(composed[2], [0.3, 0.3, 0.8, 1.0]);

        // With no underlay, transparent nodes resolve to the default surface so
        // the surface is never painted black.
        let without = super::afni_colors_over_underlay(&afni, None);
        assert_eq!(without[1], super::mesh::DEFAULT_SURFACE_COLOR);
        assert_eq!(without[0], [1.0, 0.0, 0.0, 1.0]);
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
    fn roi_default_save_name_uses_label_once() {
        let roi = Roi::new("roi_1", 1).unwrap();

        assert_eq!(super::roi_save_default_name(&roi, None), "roi_1.niml.roi");
        assert_eq!(
            super::roi_save_all_default_name(None),
            "sumaru_rois.niml.roi"
        );
        assert_eq!(
            super::roi_save_all_default_name(Some(&PathBuf::from("saved_set.roi"))),
            "saved_set.niml.roi"
        );
        assert_eq!(
            super::roi_save_all_default_name(Some(&PathBuf::from("saved_set.niml.roi"))),
            "saved_set.niml.roi"
        );
        assert_eq!(
            super::append_niml_roi_extension(PathBuf::from("roi_1.niml.roi")),
            PathBuf::from("roi_1.niml.roi")
        );
        assert_eq!(
            super::append_niml_roi_extension(PathBuf::from("roi_1.roi")),
            PathBuf::from("roi_1.niml.roi")
        );
        assert_eq!(
            super::append_niml_roi_extension(PathBuf::from("roi_1.niml")),
            PathBuf::from("roi_1.niml.roi")
        );
    }

    #[test]
    fn roi_seed_fill_respects_joined_boundary_edges() {
        let mesh = SurfaceMesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
        .unwrap();
        let boundary = vec![0, 2, 3, 0];

        let fill_from_one = roi_fill_nodes_from_seed(&mesh, &boundary, 1).unwrap();

        assert_eq!(fill_from_one, vec![0, 1, 2, 3]);
        assert!(roi_fill_nodes_from_seed(&mesh, &boundary, 2).is_err());
    }

    #[test]
    fn roi_workspace_save_is_non_destructive_and_finalize_adds_next_slot() {
        let mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let target = RoiDraftTarget {
            surface_id: mesh.metadata.id.clone(),
            domain_id: mesh.domain.id.clone(),
            side: SurfaceSide::Left,
        };
        let mut workspace = RoiWorkspace::default();
        {
            let draft = workspace.active_draft_mut().unwrap();
            draft.target = Some(target);
            draft.anchor_nodes = vec![0, 1, 2];
        }

        let rois = workspace.saveable_rois().unwrap();

        assert_eq!(rois.len(), 1);
        assert!(workspace.slots[0].editing);
        assert_eq!(workspace.slots[0].draft.anchor_nodes, vec![0, 1, 2]);

        assert!(workspace.finalize_slot(0).unwrap());
        assert_eq!(workspace.slots.len(), 2);
        assert!(!workspace.slots[0].editing);
        assert_eq!(workspace.active_index, 1);
        assert_eq!(workspace.slots[1].label(), "roi_2");
        assert_eq!(workspace.slots[1].integer_label(), 2);
        assert_eq!(workspace.saveable_rois().unwrap().len(), 1);
        assert_eq!(
            workspace.saveable_roi_at(0).unwrap().unwrap().label,
            "roi_1"
        );
        assert!(workspace.saveable_roi_at(1).unwrap().is_none());

        assert!(workspace.delete_slot(0));
        assert_eq!(workspace.saveable_rois().unwrap().len(), 0);
        assert_eq!(workspace.slots.len(), 1);
        assert!(workspace.slots[0].editing);
    }

    #[test]
    fn finalized_roi_slot_can_be_reopened_for_editing() {
        let mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let roi = Roi::from_nodes("roi_7", 7, vec![0, 2])
            .unwrap()
            .with_parent_surface(
                mesh.metadata.id.clone(),
                mesh.domain.id.clone(),
                SurfaceSide::Left,
            );
        let mut workspace = RoiWorkspace::from_rois(vec![roi]);

        assert_eq!(workspace.slots.len(), 2);
        assert!(!workspace.slots[0].editing);
        assert!(workspace.edit_slot(0).unwrap());
        assert_eq!(workspace.active_index, 0);
        assert!(workspace.slots[0].editing);
        assert_eq!(workspace.slots[0].draft.anchor_nodes, vec![0, 2]);
        assert_eq!(workspace.saveable_rois().unwrap().len(), 1);
    }

    #[test]
    fn joined_filled_roi_draft_reopens_for_appending_points() {
        let mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let target = RoiDraftTarget {
            surface_id: mesh.metadata.id.clone(),
            domain_id: mesh.domain.id.clone(),
            side: SurfaceSide::Left,
        };
        let mut draft = super::RoiDraft::new("roi_1", 1);
        draft.anchor_nodes = vec![0, 1, 2];
        draft.segments = vec![vec![0, 1], vec![1, 2], vec![2, 0]];
        draft.fill_nodes = Some(vec![0, 1, 2, 3]);
        draft.fill_seed_node = Some(3);
        draft.fill_pending = true;

        assert!(draft.is_joined());
        if draft.target.is_none() {
            draft.target = Some(target.clone());
        }
        draft.push_history();
        draft.reopen_joined_path_for_append();
        draft.segments.push(vec![2, 1]);
        draft.anchor_nodes.push(1);

        assert!(!draft.is_joined());
        assert_eq!(draft.segments, vec![vec![0, 1], vec![1, 2], vec![2, 1]]);
        assert_eq!(draft.anchor_nodes, vec![0, 1, 2, 1]);
        assert_eq!(draft.fill_nodes, None);
        assert_eq!(draft.fill_seed_node, None);
        assert!(!draft.fill_pending);

        assert!(draft.undo());
        assert!(draft.is_joined());
        assert_eq!(draft.target, Some(target));
        assert_eq!(draft.segments, vec![vec![0, 1], vec![1, 2], vec![2, 0]]);
        assert_eq!(draft.anchor_nodes, vec![0, 1, 2]);
        assert_eq!(draft.fill_nodes, Some(vec![0, 1, 2, 3]));
        assert_eq!(draft.fill_seed_node, Some(3));
    }

    #[test]
    fn roi_draft_style_uses_integer_label_color() {
        let mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let target = RoiDraftTarget {
            surface_id: mesh.metadata.id.clone(),
            domain_id: mesh.domain.id.clone(),
            side: SurfaceSide::Left,
        };
        let mut draft = super::RoiDraft::new("roi_2", 2);
        draft.target = Some(target);
        draft.anchor_nodes = vec![0, 1, 2];

        let roi = draft.to_roi().unwrap().unwrap();

        assert_eq!(roi.fill_color, super::roi_fill_color_for_label(2));
        assert_eq!(roi.edge_color, super::roi_edge_color_for_label(2));
        assert_ne!(
            super::roi_fill_color_for_label(1),
            super::roi_fill_color_for_label(2)
        );
        assert!(roi.color_by_label);
    }

    #[test]
    fn anatomical_shading_maps_low_values_dark_and_high_values_light() {
        let colors =
            super::anatomical_shading_colors_from_values(&[-2.0, -1.0, 0.0, 1.0, 2.0, f32::NAN]);

        assert_eq!(colors.len(), 6);
        assert!(colors[0][0] < colors[2][0]);
        assert_eq!(colors[4], colors[2]);
        assert!((colors[5][0] - 0.454).abs() < 0.0001);
        assert!((colors[5][1] - 0.457).abs() < 0.0001);
        assert!((colors[5][2] - 0.451).abs() < 0.0001);
        assert_eq!(colors[5][3], 1.0);
    }

    #[test]
    fn surface_pick_for_node_matches_vertex_and_overlay_values() {
        let mesh = SurfaceMesh::new(
            vec![[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let overlay = OverlayDataset {
            values: vec![10.0, 20.0, 30.0],
            range: ValueRange {
                min: 10.0,
                max: 30.0,
            },
            threshold_values: Some(vec![1.0, 2.0, 3.0]),
        };

        let pick = surface_pick_for_mesh_node(&mesh, Some(&overlay), 1).unwrap();

        assert_eq!(pick.node_index, 1);
        assert_eq!(pick.face_index, 0);
        assert_eq!(pick.surface_position, [1.0, 0.0, 0.0]);
        assert_eq!(pick.overlay_value, Some(20.0));
        assert_eq!(pick.threshold_value, Some(2.0));
        assert!(surface_pick_for_mesh_node(&mesh, Some(&overlay), 3).is_none());
    }

    #[test]
    fn paired_spec_montage_uses_closed_top_bottom_then_signed_acorn_views() {
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
            HemisphereLayoutState::acorn_signed(1.0)
        );
        assert_eq!(
            paired[2].camera,
            MontageCamera::Direction {
                eye_direction: Vec3::NEG_Y,
                up: Vec3::Z,
            }
        );
        assert_eq!(paired[3].layout.unwrap().layout, HemisphereLayout::Open);
        assert_eq!(
            paired[3].layout.unwrap().state,
            HemisphereLayoutState::acorn_signed(-1.0)
        );
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
        assert_eq!(build.node_labels.get(&1).unwrap(), &vec!["left-roi (1)"]);
        assert_eq!(build.node_labels.get(&4).unwrap(), &vec!["right-roi (2)"]);
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
    fn acorn_opening_pivots_from_medial_edges() {
        let components = vec![
            component("smoothwm", SurfaceSide::Left, -3.0),
            component("smoothwm", SurfaceSide::Right, 3.0),
        ];

        let transforms = component_transforms(&components, HemisphereLayoutState::acorn());
        let left_mesh = components[0].mesh.as_ref().unwrap();
        let right_mesh = components[1].mesh.as_ref().unwrap();

        assert_eq!(
            transforms[0].rotation_pivot.unwrap().x,
            left_mesh.bounds.max[0]
        );
        assert_eq!(
            transforms[1].rotation_pivot.unwrap().x,
            right_mesh.bounds.min[0]
        );
        assert_ne!(
            transforms[0].rotation_pivot.unwrap().x,
            left_mesh.bounds.center[0]
        );
        assert_ne!(
            transforms[1].rotation_pivot.unwrap().x,
            right_mesh.bounds.center[0]
        );
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
    fn pair_hemisphere_matrices_keep_resident_instances_when_visibility_changes() {
        let components = vec![
            component("smoothwm", SurfaceSide::Left, 0.0),
            component("smoothwm", SurfaceSide::Right, 3.0),
        ];

        let both = pair_hemisphere_matrices(
            &components,
            HemisphereLayoutState::closed(),
            PairVisibility::both(),
        );
        assert_eq!(
            both.iter().map(|(side, _)| side).collect::<Vec<_>>(),
            vec![&SurfaceSide::Left, &SurfaceSide::Right]
        );

        let left_only = pair_hemisphere_matrices(
            &components,
            HemisphereLayoutState::closed(),
            PairVisibility {
                left: true,
                right: false,
            },
        );
        assert_eq!(
            left_only.iter().map(|(side, _)| side).collect::<Vec<_>>(),
            vec![&SurfaceSide::Left, &SurfaceSide::Right]
        );

        let right_only = pair_hemisphere_matrices(
            &components,
            HemisphereLayoutState::closed(),
            PairVisibility {
                left: false,
                right: true,
            },
        );
        assert_eq!(
            right_only.iter().map(|(side, _)| side).collect::<Vec<_>>(),
            vec![&SurfaceSide::Left, &SurfaceSide::Right]
        );
        assert_ne!(both[0].1, left_only[0].1);
        assert_ne!(both[1].1, right_only[1].1);
    }

    #[test]
    fn paired_selection_highlight_uses_local_indices_and_model_scale() {
        let pick = SurfacePick {
            node_index: 4,
            face_index: 8,
            surface_position: [11.0, 2.0, 3.0],
            normalized_position: [0.0, 0.0, 0.0],
            overlay_value: Some(1.25),
            threshold_value: Some(2.5),
        };
        let positions = vec![[10.0, 0.0, 0.0], [11.0, 2.0, 3.0], [12.0, 0.0, 0.0]];
        let matrices = vec![(
            SurfaceSide::Right,
            Mat4::from_scale(Vec3::splat(1.0 / 100.0)),
        )];
        let scale = selection_scale_from_model_matrices(&matrices);

        let highlight = selection_for_component(Some(pick), 3, 7, &positions, scale).unwrap();

        assert_eq!(highlight.node_index, 1);
        assert_eq!(highlight.face_index, 1);
        assert_eq!(highlight.crosshair_position, positions[1]);
        assert!((highlight.marker_radius - 2.5).abs() < 1e-5);
        assert!((highlight.face_offset - 0.3).abs() < 1e-5);
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

        open_angle = (open_angle + 100.0 * PAIR_OPEN_DEGREES_PER_PIXEL)
            .clamp(-PAIR_MAX_OPEN_DEGREES, PAIR_MAX_OPEN_DEGREES);
        separation = (separation + -(-50.0) * (pair_width / 700.0).max(0.05))
            .clamp(0.0, pair_width * PAIR_MAX_DRAG_GAP_FACTOR);

        assert_eq!(open_angle, 18.0);
        assert_eq!(separation, 10.0);
        assert_eq!(super::pair_open_percent_label(open_angle), "+21%");

        open_angle = (open_angle + -200.0 * PAIR_OPEN_DEGREES_PER_PIXEL)
            .clamp(-PAIR_MAX_OPEN_DEGREES, PAIR_MAX_OPEN_DEGREES);

        assert_eq!(open_angle, -18.0);
        assert_eq!(super::pair_open_percent_label(open_angle), "-21%");
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

        let paths = paired_overlay_paths(&PathBuf::from("lh.aparc.a2009s.annot.niml.dset"))
            .expect("start-of-filename lh overlay should pair");
        assert_eq!(
            paths.left_path,
            PathBuf::from("lh.aparc.a2009s.annot.niml.dset")
        );
        assert_eq!(
            paths.right_path,
            PathBuf::from("rh.aparc.a2009s.annot.niml.dset")
        );
        assert_eq!(paths.display_name, "?h.aparc.a2009s.annot.niml.dset");

        let paths = paired_overlay_paths(&PathBuf::from("rh.aparc.a2009s.annot.niml.dset"))
            .expect("start-of-filename rh overlay should pair");
        assert_eq!(
            paths.left_path,
            PathBuf::from("lh.aparc.a2009s.annot.niml.dset")
        );
        assert_eq!(
            paths.right_path,
            PathBuf::from("rh.aparc.a2009s.annot.niml.dset")
        );
        assert_eq!(paths.display_name, "?h.aparc.a2009s.annot.niml.dset");
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

    #[test]
    fn afni_registration_skips_non_anatomical_spec_surfaces() {
        let mut smoothwm = component("smoothwm", SurfaceSide::Left, 0.0);
        smoothwm.spec_surface.anatomical = Some(true);
        assert!(afni_component_is_sendable(
            &smoothwm,
            smoothwm.mesh.as_ref()
        ));

        let mut inflated = component("inflated", SurfaceSide::Left, 0.0);
        inflated.spec_surface.anatomical = Some(false);
        assert!(!afni_component_is_sendable(
            &inflated,
            inflated.mesh.as_ref()
        ));

        let mut unknown_sphere = component("sphere", SurfaceSide::Left, 0.0);
        if let Some(mesh) = unknown_sphere.mesh.as_mut() {
            mesh.metadata.surface_kind = SurfaceKind::Sphere;
            mesh.metadata.anatomically_correct = AnatomicalCorrectness::Unknown;
        }
        assert!(!afni_component_is_sendable(
            &unknown_sphere,
            unknown_sphere.mesh.as_ref()
        ));
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
