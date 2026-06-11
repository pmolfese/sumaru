use std::collections::{BTreeMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Result, bail, ensure};

use crate::command::{
    BackgroundMode, ControllerState, CrosshairState, OverlayThresholdCommandState, ViewerCommand,
};
use crate::io::{
    NimlData, NimlElement, NimlNumericMatrix, NimlValueType, parse_niml_bytes, serialize_niml_ascii,
};
use crate::surface::SurfaceMesh;

pub const DEFAULT_AFNI_NIML_PORT: u16 = 53211;
pub const DEFAULT_PORT_OFFSET: u16 = 1024;
pub const AFNI_SUMA_NIML_PORT_NAME: &str = "AFNI_SUMA_NIML";
pub const DEFAULT_AFNI_HOST: &str = "127.0.0.1";

const AFNI_READ_TIMEOUT: Duration = Duration::from_millis(250);
const AFNI_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const AFNI_CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const AFNI_MAX_PENDING_BYTES: usize = 256 * 1024 * 1024;

const AFNI_PORT_NAMES: &[&str] = &[
    "AFNI_SUMA_NIML",
    "AFNI_DEFAULT_LISTEN_NIML",
    "AFNI_GroupInCorr_NIML",
    "SUMA_DEFAULT_LISTEN_NIML",
    "SUMA_GroupInCorr_NIML",
    "MATLAB_SUMA_NIML",
    "SUMA_GEOMCOMP_NIML",
    "SUMA_BRAINWRAP_NIML",
    "SUMA_DRIVESUMA_NIML",
    "AFNI_PLUGOUT_TCP_0",
    "AFNI_PLUGOUT_TCP_1",
    "AFNI_PLUGOUT_TCP_2",
    "AFNI_PLUGOUT_TCP_3",
    "AFNI_PLUGOUT_TCP_4",
    "AFNI_TCP_PORT",
    "AFNI_CONTROL_PORT",
    "PLUGOUT_DRIVE_PORT",
    "PLUGOUT_GRAPH_PORT",
    "PLUGOUT_IJK_PORT",
    "PLUGOUT_SURF_PORT",
    "PLUGOUT_TT_PORT",
    "PLUGOUT_TTA_PORT",
    "SUMA_HALLO_SUMA_NIML",
    "SUMA_INSTA_TRACT_NIML",
];

/// Minimal AFNI/SUMA NIML talk subset for Sumaru's first interop pass.
///
/// The concrete AFNI-compatible exchange, checked against AFNI's
/// `afni_niml.c` and PySuma's `afni_niml.py`, is:
///
/// - `SUMA_ixyz`: surface node index plus XYZ coordinates sent to AFNI.
/// - `SUMA_node_normals`: per-node normals sent to AFNI.
/// - `SUMA_ijk`: surface triangle indices sent to AFNI.
/// - `SUMA_irgba`: sparse node RGBA colors sent from AFNI back to a surface
///   viewer, including optional threshold/function/volume attributes.
///
/// The remaining first-pass control messages are deliberately Sumaru-prefixed
/// NIML elements. They define the non-`wgpu` contract we want before a real TCP
/// loop is wired in: active surface, crosshair/selected node, dataset loading,
/// overlay/threshold state, controller commands, and ROI updates.
/// Compatibility tests can later replace or augment these names with exact
/// AFNI/SUMA stream captures where AFNI already has a canonical message.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfniPortConfig {
    pub host: String,
    pub port: u16,
    pub port_offset: Option<u16>,
    pub port_bloc: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfniSurfaceInfo {
    pub surface_idcode: String,
    pub surface_label: String,
    pub local_domain_parent_id: String,
    pub local_domain_parent: String,
    pub specfile_name: Option<String>,
    pub specfile_path: Option<String>,
    pub volume_idcode: Option<String>,
    pub volume_headname: Option<String>,
    pub volume_filecode: Option<String>,
    pub volume_dirname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfniRgbaOverlay {
    pub surface_idcode: String,
    pub local_domain_parent_id: Option<String>,
    pub node_indices: Vec<u32>,
    pub rgba: Vec<[u8; 4]>,
    pub threshold: Option<String>,
    pub function_idcode: Option<String>,
    pub volume_idcode: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AfniIncomingMessage {
    RgbaOverlay(AfniRgbaOverlay),
    SurfaceSelection(AfniSurfaceSelection),
    Crosshair(CrosshairState),
    SurfaceCrosshair(AfniSurfaceCrosshair),
    DatasetLoad(PathBuf),
    OverlayState(AfniOverlayState),
    ControllerCommand(AfniControllerCommand),
    RoiUpdate(AfniRoiUpdate),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfniSurfaceSelection {
    pub surface_idcode: Option<String>,
    pub surface_label: Option<String>,
    pub scene_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AfniSurfaceCrosshair {
    pub surface_idcode: Option<String>,
    pub node_index: Option<u32>,
    pub surface_position: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct AfniOverlayState {
    pub visible: Option<bool>,
    pub symmetric_range: Option<bool>,
    pub threshold: Option<OverlayThresholdCommandState>,
    pub opacity: Option<f32>,
    pub overlay_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfniControllerCommand {
    ResetCamera,
    ToggleOverlay,
    SetBackground(BackgroundMode),
    OpenSurfaceController(bool),
    OpenRoiController(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfniRoiUpdate {
    pub path: Option<PathBuf>,
    pub visible: Option<bool>,
}

#[derive(Debug, Clone)]
pub enum AfniRouteAction {
    ViewerCommand(ViewerCommand),
    LoadDataset(PathBuf),
    RgbaOverlay(AfniRgbaOverlay),
    SurfaceCrosshair(AfniSurfaceCrosshair),
    RoiUpdate(AfniRoiUpdate),
}

#[derive(Debug, Default, Clone)]
pub struct AfniRouteOutcome {
    pub actions: Vec<AfniRouteAction>,
    pub applied_state: bool,
}

#[derive(Debug, Default, Clone)]
pub struct AfniNimlSession {
    registered_surface_ids: HashSet<String>,
}

#[derive(Debug)]
pub enum AfniConnectionEvent {
    Elements(Vec<NimlElement>),
    Error(String),
    Disconnected,
}

#[derive(Debug)]
pub struct AfniConnection {
    stream: TcpStream,
    receiver: Receiver<AfniConnectionEvent>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    verbose: bool,
}

impl AfniConnection {
    /// Opens the AFNI/SUMA NIML TCP stream and starts a background reader.
    ///
    /// SUMA and PySuma make the same conceptual move when the user presses `t`:
    /// connect to AFNI's `AFNI_SUMA_NIML` port, send surface geometry, and keep
    /// listening for `SUMA_irgba` color updates. The caller supplies `wake`
    /// because the reader thread must not mutate viewer state directly; it only
    /// queues parsed NIML elements and nudges the GUI event loop.
    pub fn connect(
        config: &AfniPortConfig,
        verbose: bool,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let address = (config.host.as_str(), config.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow::anyhow!("could not resolve AFNI host {}", config.host))?;
        let stream =
            TcpStream::connect_timeout(&address, AFNI_CONNECT_TIMEOUT).map_err(|error| {
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionRefused | ErrorKind::TimedOut | ErrorKind::NotFound
                ) {
                    anyhow::anyhow!(
                        "AFNI/SUMA NIML talk is not listening at {}:{} ({error}). \
                         Start AFNI with `afni -niml -yesplugouts` or press the `NIML+PO` \
                         button in AFNI, then press `T` in Sumaru to retry. If AFNI was \
                         launched with `-np` or `-npb`, launch Sumaru with the same value.",
                        config.host,
                        config.port
                    )
                } else {
                    anyhow::anyhow!(error)
                }
            })?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(AFNI_WRITE_TIMEOUT))?;

        let reader_stream = stream.try_clone()?;
        reader_stream.set_read_timeout(Some(AFNI_READ_TIMEOUT))?;
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let reader_stop = stop.clone();
        let reader = thread::spawn(move || {
            read_afni_stream(reader_stream, sender, reader_stop, verbose, wake);
        });

        Ok(Self {
            stream,
            receiver,
            stop,
            reader: Some(reader),
            verbose,
        })
    }

    pub fn send_elements(&mut self, elements: &[NimlElement]) -> Result<()> {
        let payload = serialize_niml_ascii(elements);
        log_niml_elements(self.verbose, "tx", elements, payload.len());
        self.stream
            .write_all(payload.as_bytes())
            .map_err(|error| afni_write_error(error, payload.len(), elements.len()))?;
        self.stream
            .flush()
            .map_err(|error| afni_write_error(error, payload.len(), elements.len()))?;
        Ok(())
    }

    pub fn try_recv(&self) -> Option<AfniConnectionEvent> {
        self.receiver.try_recv().ok()
    }

    pub fn disconnect(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.stream.shutdown(Shutdown::Both);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn afni_write_error(
    error: std::io::Error,
    byte_count: usize,
    element_count: usize,
) -> anyhow::Error {
    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
        anyhow::anyhow!(
            "timed out while sending {byte_count} bytes ({element_count} NIML elements) \
             to AFNI/SUMA NIML talk after {:.1}s. AFNI may be busy, paused, or crashed; \
             press `T` in Sumaru to disconnect/reconnect after AFNI is listening again.",
            AFNI_WRITE_TIMEOUT.as_secs_f32()
        )
    } else if matches!(
        error.kind(),
        ErrorKind::BrokenPipe
            | ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::NotConnected
    ) {
        anyhow::anyhow!(
            "AFNI/SUMA NIML talk disconnected while sending {byte_count} bytes \
             ({element_count} NIML elements): {error}. Restart AFNI NIML listening, \
             then press `T` in Sumaru to reconnect."
        )
    } else {
        anyhow::anyhow!(
            "failed to send {byte_count} bytes ({element_count} NIML elements) \
             to AFNI/SUMA NIML talk: {error}"
        )
    }
}

impl Drop for AfniConnection {
    fn drop(&mut self) {
        self.disconnect();
    }
}

fn log_niml_elements(verbose: bool, direction: &str, elements: &[NimlElement], byte_count: usize) {
    if !verbose {
        return;
    }

    for element in elements {
        eprintln!(
            "sumaru niml {direction}: {} bytes={} {} attrs={}",
            element.name,
            byte_count,
            niml_data_summary(&element.data),
            niml_attr_summary(&element.attrs)
        );
    }
}

fn niml_attr_summary(attrs: &BTreeMap<String, String>) -> String {
    if attrs.is_empty() {
        return "{}".to_string();
    }

    let pairs = attrs
        .iter()
        .map(|(key, value)| format!("{key}={}", truncate_log_value(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{pairs}}}")
}

fn niml_data_summary(data: &NimlData) -> String {
    match data {
        NimlData::None => "empty".to_string(),
        NimlData::Text(text) => format!("text chars={}", text.chars().count()),
        NimlData::Numeric(matrix) => format!(
            "numeric rows={} cols={} types={:?}",
            matrix.rows,
            matrix.column_count(),
            matrix.column_types
        ),
        NimlData::Mixed(table) => format!(
            "mixed rows={} cols={} types={:?}",
            table.rows,
            table.column_types.len(),
            table.column_types
        ),
        NimlData::RoiDatums(records) => format!("roi_datums count={}", records.len()),
        NimlData::Group(children) => {
            let names = children
                .iter()
                .map(|child| child.name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            format!("group children={} names=[{}]", children.len(), names)
        }
    }
}

fn truncate_log_value(value: &str) -> String {
    const MAX_LOG_ATTR_CHARS: usize = 120;
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(MAX_LOG_ATTR_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn read_afni_stream(
    mut stream: TcpStream,
    sender: mpsc::Sender<AfniConnectionEvent>,
    stop: Arc<AtomicBool>,
    verbose: bool,
    wake: impl Fn() + Send + 'static,
) {
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 65_536];

    while !stop.load(Ordering::Relaxed) {
        match stream.read(&mut chunk) {
            Ok(0) => {
                let _ = sender.send(AfniConnectionEvent::Disconnected);
                wake();
                return;
            }
            Ok(read) => {
                pending.extend_from_slice(&chunk[..read]);
                match parse_niml_bytes(&pending) {
                    Ok(elements) => {
                        if !elements.is_empty() {
                            log_niml_elements(verbose, "rx", &elements, pending.len());
                            let _ = sender.send(AfniConnectionEvent::Elements(elements));
                            wake();
                        }
                        pending.clear();
                    }
                    Err(error) if pending.len() > AFNI_MAX_PENDING_BYTES => {
                        let _ = sender.send(AfniConnectionEvent::Error(format!(
                            "AFNI NIML stream exceeded {} pending bytes before parsing: {error:#}",
                            AFNI_MAX_PENDING_BYTES
                        )));
                        wake();
                        pending.clear();
                    }
                    Err(_) => {
                        // A TCP read can split a NIML element anywhere, including
                        // inside binary payload bytes. Keep the bytes until the
                        // next read gives the parser a complete element.
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => {
                if !stop.load(Ordering::Relaxed) {
                    let _ = sender.send(AfniConnectionEvent::Error(format!(
                        "AFNI NIML read failed: {error}"
                    )));
                    wake();
                }
                return;
            }
        }
    }
}

pub fn resolve_afni_port_config(
    host: impl Into<String>,
    port: Option<u16>,
    port_offset: Option<u16>,
    port_bloc: Option<u16>,
    environ: &BTreeMap<String, String>,
) -> Result<AfniPortConfig> {
    if let Some(port) = port {
        return Ok(AfniPortConfig {
            host: host.into(),
            port,
            port_offset,
            port_bloc,
        });
    }

    let effective_offset = resolve_port_offset(port_offset, port_bloc, environ)?;
    if let Some(offset) = effective_offset {
        return Ok(AfniPortConfig {
            host: host.into(),
            port: offset + port_index(AFNI_SUMA_NIML_PORT_NAME)?,
            port_offset: Some(offset),
            port_bloc: port_offset_to_bloc(offset),
        });
    }

    if let Some(port) = env_u16(environ, "SUMA_AFNI_TCP_PORT").filter(|port| *port > 0) {
        return Ok(AfniPortConfig {
            host: host.into(),
            port,
            port_offset: None,
            port_bloc: None,
        });
    }

    Ok(AfniPortConfig {
        host: host.into(),
        port: DEFAULT_AFNI_NIML_PORT,
        port_offset: None,
        port_bloc: None,
    })
}

pub fn npb_to_np(port_bloc: u16) -> u16 {
    DEFAULT_PORT_OFFSET + port_bloc * AFNI_PORT_NAMES.len() as u16
}

pub fn port_offset_to_bloc(port_offset: u16) -> Option<u16> {
    (port_offset >= DEFAULT_PORT_OFFSET)
        .then_some((port_offset - DEFAULT_PORT_OFFSET) / AFNI_PORT_NAMES.len() as u16)
}

pub fn surface_registration_elements(
    mesh: &SurfaceMesh,
    info: &AfniSurfaceInfo,
) -> Result<Vec<NimlElement>> {
    Ok(vec![
        surface_ixyz_element(mesh, info)?,
        surface_normals_element(mesh, info)?,
        surface_ijk_element(mesh, info)?,
    ])
}

pub fn outgoing_state_elements(controller: &ControllerState) -> Result<Vec<NimlElement>> {
    let mut elements = Vec::new();
    elements.push(surface_state_element(controller));
    if let Some(crosshair) = controller.interaction.crosshair {
        elements.push(crosshair_element(crosshair)?);
    }
    elements.push(overlay_state_element(controller));
    if controller.surface.current_roi_path.is_some() {
        elements.push(roi_state_element(controller));
    }
    Ok(elements)
}

pub fn surface_crosshair_element(
    mesh: &SurfaceMesh,
    info: &AfniSurfaceInfo,
    node_index: u32,
    surface_position: [f32; 3],
) -> Result<NimlElement> {
    let mut attrs = BTreeMap::new();
    attrs.insert("surface_nodeid".to_string(), node_index.to_string());
    attrs.insert("surface_idcode".to_string(), info.surface_idcode.clone());
    attrs.insert(
        "domain_parent_idcode".to_string(),
        info.local_domain_parent_id.clone(),
    );
    attrs.insert("surface_label".to_string(), info.surface_label.clone());
    push_opt_attr(&mut attrs, "volume_idcode", info.volume_idcode.as_deref());

    let position = afni_xyz(surface_position, surface_uses_lpi_coordinates(mesh));
    NimlNumericMatrix::from_rows(
        vec![NimlValueType::Float32],
        vec![
            vec![position[0] as f64],
            vec![position[1] as f64],
            vec![position[2] as f64],
        ],
    )
    .map(|matrix| NimlElement::numeric("SUMA_crosshair_xyz", attrs, matrix))
}

pub fn parse_incoming_message(element: &NimlElement) -> Result<Option<AfniIncomingMessage>> {
    match element.name.as_str() {
        "SUMA_irgba" => Ok(Some(AfniIncomingMessage::RgbaOverlay(
            AfniRgbaOverlay::from_element(element)?,
        ))),
        "SUMARU_surface_select" => Ok(Some(AfniIncomingMessage::SurfaceSelection(
            surface_selection_from_element(element),
        ))),
        "SUMARU_crosshair" => Ok(Some(AfniIncomingMessage::Crosshair(
            crosshair_from_element(element)?,
        ))),
        "SUMA_crosshair" => Ok(Some(AfniIncomingMessage::SurfaceCrosshair(
            surface_crosshair_from_group(element)?,
        ))),
        "SUMARU_load_dataset" => Ok(path_attr(element, "path")
            .map(AfniIncomingMessage::DatasetLoad)
            .or_else(|| path_attr(element, "dataset_path").map(AfniIncomingMessage::DatasetLoad))),
        "SUMARU_overlay_state" => Ok(Some(AfniIncomingMessage::OverlayState(
            overlay_state_from_element(element),
        ))),
        "SUMARU_viewer_command" => Ok(
            controller_command_from_element(element).map(AfniIncomingMessage::ControllerCommand)
        ),
        "SUMARU_roi_state" => Ok(Some(AfniIncomingMessage::RoiUpdate(
            roi_update_from_element(element),
        ))),
        "EngineCommand" => {
            Ok(engine_command_from_element(element).map(AfniIncomingMessage::ControllerCommand))
        }
        _ => Ok(None),
    }
}

pub fn route_incoming_message(
    controller: &mut ControllerState,
    message: AfniIncomingMessage,
) -> AfniRouteOutcome {
    let mut outcome = AfniRouteOutcome::default();

    match message {
        AfniIncomingMessage::RgbaOverlay(overlay) => {
            controller.overlay.visible = true;
            if let Some(threshold) = overlay
                .threshold
                .as_deref()
                .and_then(|value| value.parse::<f32>().ok())
            {
                controller.overlay.threshold = Some(OverlayThresholdCommandState {
                    value: threshold,
                    absolute: true,
                    hide_failed: true,
                });
                outcome.applied_state = true;
            }
            outcome.actions.push(AfniRouteAction::RgbaOverlay(overlay));
        }
        AfniIncomingMessage::SurfaceSelection(selection) => {
            if let Some(index) = selection.scene_index {
                controller.surface.current_scene_surface_index = Some(index);
                outcome.actions.push(AfniRouteAction::ViewerCommand(
                    ViewerCommand::SelectSceneSurface(index),
                ));
                outcome.applied_state = true;
            }
            if let Some(label) = selection.surface_label {
                controller.record_status(format!("AFNI selected surface {label}."));
                outcome.applied_state = true;
            } else if let Some(idcode) = selection.surface_idcode {
                controller.record_status(format!("AFNI selected surface id {idcode}."));
                outcome.applied_state = true;
            }
        }
        AfniIncomingMessage::Crosshair(crosshair) => {
            controller.interaction.crosshair = Some(crosshair);
            outcome.applied_state = true;
        }
        AfniIncomingMessage::SurfaceCrosshair(crosshair) => {
            if let Some(node_index) = crosshair.node_index {
                controller.record_status(format!("AFNI crosshair selected node {node_index}."));
            } else {
                controller.record_status("AFNI crosshair did not include a surface node id.");
            }
            outcome
                .actions
                .push(AfniRouteAction::SurfaceCrosshair(crosshair));
            outcome.applied_state = true;
        }
        AfniIncomingMessage::DatasetLoad(path) => {
            controller.surface.current_overlay_path = Some(path.clone());
            controller.overlay.visible = true;
            outcome.actions.push(AfniRouteAction::LoadDataset(path));
            outcome.applied_state = true;
        }
        AfniIncomingMessage::OverlayState(state) => {
            if let Some(visible) = state.visible {
                controller.overlay.visible = visible;
                outcome.actions.push(AfniRouteAction::ViewerCommand(
                    ViewerCommand::SetOverlayVisible(visible),
                ));
                outcome.applied_state = true;
            }
            if let Some(symmetric) = state.symmetric_range {
                controller.overlay.symmetric_range = symmetric;
                outcome.applied_state = true;
            }
            if let Some(threshold) = state.threshold {
                controller.overlay.threshold = Some(threshold);
                outcome.applied_state = true;
            }
            if let Some(opacity) = state.opacity {
                controller.overlay.opacity = opacity.clamp(0.0, 1.0);
                outcome.applied_state = true;
            }
            if let Some(path) = state.overlay_path {
                controller.surface.current_overlay_path = Some(path);
                outcome.applied_state = true;
            }
        }
        AfniIncomingMessage::ControllerCommand(command) => {
            let viewer_command = match command {
                AfniControllerCommand::ResetCamera => Some(ViewerCommand::ResetCamera),
                AfniControllerCommand::ToggleOverlay => {
                    let visible = !controller.overlay.visible;
                    controller.overlay.visible = visible;
                    Some(ViewerCommand::SetOverlayVisible(visible))
                }
                AfniControllerCommand::SetBackground(background) => {
                    controller.display.background = background;
                    None
                }
                AfniControllerCommand::OpenSurfaceController(visible) => {
                    controller.panels.surface_controller_visible = visible;
                    Some(ViewerCommand::SetSurfaceControllerVisible(visible))
                }
                AfniControllerCommand::OpenRoiController(open) => {
                    controller.panels.roi_controller_open = open;
                    Some(ViewerCommand::SetRoiControllerOpen(open))
                }
            };
            if let Some(command) = viewer_command {
                outcome
                    .actions
                    .push(AfniRouteAction::ViewerCommand(command));
            }
            outcome.applied_state = true;
        }
        AfniIncomingMessage::RoiUpdate(update) => {
            if let Some(visible) = update.visible {
                controller.roi.visible = visible;
                outcome.actions.push(AfniRouteAction::ViewerCommand(
                    ViewerCommand::SetRoiVisible(visible),
                ));
                outcome.applied_state = true;
            }
            if let Some(path) = update.path.as_ref() {
                controller.surface.current_roi_path = Some(path.clone());
                outcome.applied_state = true;
            }
            outcome.actions.push(AfniRouteAction::RoiUpdate(update));
        }
    }

    outcome
}

impl AfniNimlSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_surface_once(
        &mut self,
        mesh: &SurfaceMesh,
        info: &AfniSurfaceInfo,
    ) -> Result<Option<Vec<NimlElement>>> {
        if !self
            .registered_surface_ids
            .insert(info.surface_idcode.clone())
        {
            return Ok(None);
        }
        surface_registration_elements(mesh, info).map(Some)
    }

    pub fn receive_element(
        &mut self,
        controller: &mut ControllerState,
        element: &NimlElement,
    ) -> Result<Option<AfniRouteOutcome>> {
        parse_incoming_message(element)
            .map(|message| message.map(|message| route_incoming_message(controller, message)))
    }
}

impl AfniSurfaceInfo {
    pub fn from_mesh(mesh: &SurfaceMesh) -> Self {
        let surface_idcode = mesh.metadata.id.as_str().to_string();
        let surface_label = mesh
            .metadata
            .label
            .clone()
            .or_else(|| {
                mesh.metadata
                    .source_file
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| surface_idcode.clone());
        let local_domain_parent_id = mesh
            .metadata
            .lineage
            .local_domain_parent
            .clone()
            .unwrap_or_else(|| mesh.metadata.lineage.domain.id.as_str().to_string());
        let local_domain_parent = mesh
            .metadata
            .lineage
            .local_domain_parent
            .clone()
            .unwrap_or_else(|| surface_label.clone());

        Self {
            surface_idcode,
            surface_label,
            local_domain_parent_id,
            local_domain_parent,
            specfile_name: None,
            specfile_path: None,
            volume_idcode: mesh.metadata.lineage.parent_volume_id.clone(),
            volume_headname: None,
            volume_filecode: None,
            volume_dirname: None,
        }
    }
}

impl AfniRgbaOverlay {
    pub fn from_element(element: &NimlElement) -> Result<Self> {
        ensure!(element.name == "SUMA_irgba", "expected SUMA_irgba element");
        let surface_idcode = attr(element, "surface_idcode")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("SUMA_irgba is missing surface_idcode"))?
            .to_string();
        let NimlData::Numeric(matrix) = &element.data else {
            bail!("SUMA_irgba must carry a numeric matrix");
        };
        ensure!(
            matrix.column_count() >= 5,
            "SUMA_irgba needs at least node,r,g,b,a columns"
        );

        let mut node_indices = Vec::with_capacity(matrix.rows);
        let mut rgba = Vec::with_capacity(matrix.rows);
        for row in 0..matrix.rows {
            let node = matrix.get(row, 0).unwrap_or(0.0).round() as i64;
            ensure!(node >= 0, "SUMA_irgba node index must be non-negative");
            node_indices.push(node as u32);
            rgba.push([
                clamp_u8(matrix.get(row, 1).unwrap_or(0.0)),
                clamp_u8(matrix.get(row, 2).unwrap_or(0.0)),
                clamp_u8(matrix.get(row, 3).unwrap_or(0.0)),
                clamp_u8(matrix.get(row, 4).unwrap_or(0.0)),
            ]);
        }

        Ok(Self {
            surface_idcode,
            local_domain_parent_id: attr(element, "local_domain_parent_ID").map(str::to_string),
            node_indices,
            rgba,
            threshold: attr(element, "threshold").map(str::to_string),
            function_idcode: attr(element, "function_idcode").map(str::to_string),
            volume_idcode: attr(element, "volume_idcode").map(str::to_string),
        })
    }

    pub fn to_element(&self) -> Result<NimlElement> {
        ensure!(
            self.node_indices.len() == self.rgba.len(),
            "RGBA overlay node/color length mismatch"
        );
        let mut attrs = BTreeMap::new();
        attrs.insert("surface_idcode".to_string(), self.surface_idcode.clone());
        push_opt_attr(
            &mut attrs,
            "local_domain_parent_ID",
            self.local_domain_parent_id.as_deref(),
        );
        push_opt_attr(&mut attrs, "threshold", self.threshold.as_deref());
        push_opt_attr(
            &mut attrs,
            "function_idcode",
            self.function_idcode.as_deref(),
        );
        push_opt_attr(&mut attrs, "volume_idcode", self.volume_idcode.as_deref());

        let rows = self
            .node_indices
            .iter()
            .zip(&self.rgba)
            .map(|(node, color)| {
                vec![
                    *node as f64,
                    color[0] as f64,
                    color[1] as f64,
                    color[2] as f64,
                    color[3] as f64,
                ]
            })
            .collect();
        Ok(NimlElement::numeric(
            "SUMA_irgba",
            attrs,
            NimlNumericMatrix::from_rows(
                vec![
                    NimlValueType::Int32,
                    NimlValueType::UInt8,
                    NimlValueType::UInt8,
                    NimlValueType::UInt8,
                    NimlValueType::UInt8,
                ],
                rows,
            )?,
        ))
    }
}

fn resolve_port_offset(
    port_offset: Option<u16>,
    port_bloc: Option<u16>,
    environ: &BTreeMap<String, String>,
) -> Result<Option<u16>> {
    ensure!(
        port_offset.is_none() || port_bloc.is_none(),
        "use either a port offset or a port bloc, not both"
    );
    if let Some(offset) = port_offset {
        ensure!(
            offset >= DEFAULT_PORT_OFFSET,
            "AFNI port offset is too small"
        );
        return Ok(Some(offset));
    }
    if let Some(bloc) = port_bloc {
        return Ok(Some(npb_to_np(bloc)));
    }
    if let Some(bloc) = env_u16(environ, "AFNI_PORT_BLOC") {
        return Ok(Some(npb_to_np(bloc)));
    }
    if let Some(offset) = env_u16(environ, "AFNI_PORT_OFFSET").filter(|v| *v >= DEFAULT_PORT_OFFSET)
    {
        return Ok(Some(offset));
    }
    env_u16(environ, "AFNI_NIML_FIRST_PORT")
        .filter(|first| *first > DEFAULT_PORT_OFFSET)
        .map(|first| first - 1)
        .map(Some)
        .map(Ok)
        .unwrap_or(Ok(None))
}

fn port_index(name: &str) -> Result<u16> {
    AFNI_PORT_NAMES
        .iter()
        .position(|candidate| *candidate == name)
        .map(|index| index as u16)
        .ok_or_else(|| anyhow::anyhow!("unknown AFNI NIML port name {name}"))
}

fn surface_ixyz_element(mesh: &SurfaceMesh, info: &AfniSurfaceInfo) -> Result<NimlElement> {
    let uses_lpi_coordinates = surface_uses_lpi_coordinates(mesh);
    let rows = mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(index, vertex)| {
            let vertex = afni_xyz(*vertex, uses_lpi_coordinates);
            vec![
                index as f64,
                vertex[0] as f64,
                vertex[1] as f64,
                vertex[2] as f64,
            ]
        })
        .collect();
    Ok(NimlElement::numeric(
        "SUMA_ixyz",
        surface_attrs(info),
        NimlNumericMatrix::from_rows(
            vec![
                NimlValueType::Int32,
                NimlValueType::Float32,
                NimlValueType::Float32,
                NimlValueType::Float32,
            ],
            rows,
        )?,
    ))
}

fn surface_normals_element(mesh: &SurfaceMesh, info: &AfniSurfaceInfo) -> Result<NimlElement> {
    let uses_lpi_coordinates = surface_uses_lpi_coordinates(mesh);
    let rows = mesh
        .vertex_normals()
        .into_iter()
        .map(|normal| {
            let normal = afni_xyz(normal, uses_lpi_coordinates);
            vec![normal[0] as f64, normal[1] as f64, normal[2] as f64]
        })
        .collect();
    Ok(NimlElement::numeric(
        "SUMA_node_normals",
        surface_attrs(info),
        NimlNumericMatrix::from_rows(
            vec![
                NimlValueType::Float32,
                NimlValueType::Float32,
                NimlValueType::Float32,
            ],
            rows,
        )?,
    ))
}

fn surface_uses_lpi_coordinates(mesh: &SurfaceMesh) -> bool {
    mesh.metadata
        .source_file
        .as_deref()
        .is_some_and(path_uses_lpi_coordinates)
}

fn path_uses_lpi_coordinates(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gii"))
}

fn afni_xyz(mut value: [f32; 3], uses_lpi_coordinates: bool) -> [f32; 3] {
    if uses_lpi_coordinates {
        value[0] *= -1.0;
        value[1] *= -1.0;
    }
    value
}

fn surface_ijk_element(mesh: &SurfaceMesh, info: &AfniSurfaceInfo) -> Result<NimlElement> {
    let rows = mesh
        .triangles
        .iter()
        .map(|triangle| vec![triangle[0] as f64, triangle[1] as f64, triangle[2] as f64])
        .collect();
    Ok(NimlElement::numeric(
        "SUMA_ijk",
        surface_attrs(info),
        NimlNumericMatrix::from_rows(
            vec![
                NimlValueType::Int32,
                NimlValueType::Int32,
                NimlValueType::Int32,
            ],
            rows,
        )?,
    ))
}

fn surface_attrs(info: &AfniSurfaceInfo) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    attrs.insert("surface_idcode".to_string(), info.surface_idcode.clone());
    attrs.insert("surface_label".to_string(), info.surface_label.clone());
    attrs.insert(
        "local_domain_parent_ID".to_string(),
        info.local_domain_parent_id.clone(),
    );
    attrs.insert(
        "local_domain_parent".to_string(),
        info.local_domain_parent.clone(),
    );
    push_opt_attr(
        &mut attrs,
        "surface_specfile_name",
        info.specfile_name.as_deref(),
    );
    push_opt_attr(
        &mut attrs,
        "surface_specfile_path",
        info.specfile_path.as_deref(),
    );
    push_opt_attr(&mut attrs, "volume_idcode", info.volume_idcode.as_deref());
    push_opt_attr(
        &mut attrs,
        "volume_headname",
        info.volume_headname.as_deref(),
    );
    push_opt_attr(
        &mut attrs,
        "volume_filecode",
        info.volume_filecode.as_deref(),
    );
    push_opt_attr(&mut attrs, "volume_dirname", info.volume_dirname.as_deref());
    attrs.extend(surface_control_attrs(&info.surface_label));
    attrs
}

fn surface_control_attrs(surface_label: &str) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    attrs.insert("afni_surface_controls_toggle".to_string(), "on".to_string());
    attrs.insert(
        "afni_surface_controls_nodes".to_string(),
        "none".to_string(),
    );
    attrs.insert(
        "afni_surface_controls_lines".to_string(),
        afni_surface_line_color(surface_label).to_string(),
    );
    attrs.insert(
        "afni_surface_controls_plusminus".to_string(),
        "none".to_string(),
    );
    attrs
}

fn afni_surface_line_color(surface_label: &str) -> &'static str {
    let label = surface_label.to_ascii_lowercase();
    let left = label.contains("lh") || label.contains("left");
    let right = label.contains("rh") || label.contains("right");
    if label.contains("smoothwm") || label.contains("white") {
        if right { "#ffff00" } else { "#00ff00" }
    } else if label.contains("pial") {
        if left { "#0000ff" } else { "#ff0000" }
    } else {
        "#ff69b4"
    }
}

fn surface_state_element(controller: &ControllerState) -> NimlElement {
    let mut attrs = BTreeMap::new();
    if let Some(id) = controller.surface.current_surface_id.as_ref() {
        attrs.insert("surface_idcode".to_string(), id.as_str().to_string());
    }
    push_opt_path_attr(
        &mut attrs,
        "surface_path",
        controller.surface.current_surface_path.as_ref(),
    );
    push_opt_path_attr(
        &mut attrs,
        "surface_volume_path",
        controller.surface.current_surface_volume_path.as_ref(),
    );
    if let Some(index) = controller.surface.current_scene_surface_index {
        attrs.insert("scene_index".to_string(), index.to_string());
    }
    NimlElement::text("SUMARU_surface_state", attrs, "")
}

fn crosshair_element(crosshair: CrosshairState) -> Result<NimlElement> {
    let matrix = NimlNumericMatrix::from_rows(
        vec![
            NimlValueType::Int32,
            NimlValueType::Int32,
            NimlValueType::Float32,
            NimlValueType::Float32,
            NimlValueType::Float32,
        ],
        vec![vec![
            crosshair.node_index as f64,
            crosshair.face_index as f64,
            crosshair.surface_position[0] as f64,
            crosshair.surface_position[1] as f64,
            crosshair.surface_position[2] as f64,
        ]],
    )?;
    Ok(NimlElement::numeric(
        "SUMARU_crosshair",
        BTreeMap::new(),
        matrix,
    ))
}

fn overlay_state_element(controller: &ControllerState) -> NimlElement {
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "visible".to_string(),
        bool_attr(controller.overlay.visible).to_string(),
    );
    attrs.insert(
        "symmetric_range".to_string(),
        bool_attr(controller.overlay.symmetric_range).to_string(),
    );
    attrs.insert(
        "opacity".to_string(),
        controller.overlay.opacity.to_string(),
    );
    if let Some(range) = controller.overlay.intensity_range {
        attrs.insert("range_min".to_string(), range[0].to_string());
        attrs.insert("range_max".to_string(), range[1].to_string());
    }
    if let Some(threshold) = controller.overlay.threshold {
        attrs.insert("threshold_enabled".to_string(), "yes".to_string());
        attrs.insert("threshold_value".to_string(), threshold.value.to_string());
        attrs.insert(
            "threshold_absolute".to_string(),
            bool_attr(threshold.absolute).to_string(),
        );
        attrs.insert(
            "threshold_hide_failed".to_string(),
            bool_attr(threshold.hide_failed).to_string(),
        );
    } else {
        attrs.insert("threshold_enabled".to_string(), "no".to_string());
    }
    push_opt_path_attr(
        &mut attrs,
        "overlay_path",
        controller.surface.current_overlay_path.as_ref(),
    );
    NimlElement::text("SUMARU_overlay_state", attrs, "")
}

fn roi_state_element(controller: &ControllerState) -> NimlElement {
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "visible".to_string(),
        bool_attr(controller.roi.visible).to_string(),
    );
    attrs.insert(
        "active_slot".to_string(),
        controller.roi.active_slot.to_string(),
    );
    push_opt_path_attr(
        &mut attrs,
        "path",
        controller.surface.current_roi_path.as_ref(),
    );
    NimlElement::text("SUMARU_roi_state", attrs, "")
}

fn surface_selection_from_element(element: &NimlElement) -> AfniSurfaceSelection {
    AfniSurfaceSelection {
        surface_idcode: attr(element, "surface_idcode").map(str::to_string),
        surface_label: attr(element, "surface_label")
            .or_else(|| attr(element, "SO_label"))
            .map(str::to_string),
        scene_index: attr(element, "scene_index").and_then(|value| value.parse().ok()),
    }
}

fn crosshair_from_element(element: &NimlElement) -> Result<CrosshairState> {
    if let NimlData::Numeric(matrix) = &element.data
        && matrix.rows > 0
        && matrix.column_count() >= 5
    {
        return Ok(CrosshairState {
            node_index: matrix.get(0, 0).unwrap_or(0.0).round() as u32,
            face_index: matrix.get(0, 1).unwrap_or(0.0).round() as usize,
            surface_position: [
                matrix.get(0, 2).unwrap_or(0.0) as f32,
                matrix.get(0, 3).unwrap_or(0.0) as f32,
                matrix.get(0, 4).unwrap_or(0.0) as f32,
            ],
        });
    }

    Ok(CrosshairState {
        node_index: parse_attr(element, "node_index").unwrap_or(0),
        face_index: parse_attr(element, "face_index").unwrap_or(0),
        surface_position: [
            parse_attr(element, "x").unwrap_or(0.0),
            parse_attr(element, "y").unwrap_or(0.0),
            parse_attr(element, "z").unwrap_or(0.0),
        ],
    })
}

fn surface_crosshair_from_group(element: &NimlElement) -> Result<AfniSurfaceCrosshair> {
    ensure!(
        element.name == "SUMA_crosshair",
        "expected SUMA_crosshair group"
    );
    let NimlData::Group(children) = &element.data else {
        bail!("SUMA_crosshair must be a NIML group");
    };
    let xyz = children
        .iter()
        .find(|child| child.name == "SUMA_crosshair_xyz")
        .ok_or_else(|| anyhow::anyhow!("SUMA_crosshair group has no SUMA_crosshair_xyz child"))?;

    let surface_idcode = attr(xyz, "surface_idcode")
        .or_else(|| attr(xyz, "domain_parent_idcode"))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let node_index = attr(xyz, "surface_nodeid")
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|value| (value >= 0).then_some(value as u32));

    Ok(AfniSurfaceCrosshair {
        surface_idcode,
        node_index,
        surface_position: numeric_xyz_from_element(xyz)?,
    })
}

fn numeric_xyz_from_element(element: &NimlElement) -> Result<[f32; 3]> {
    let NimlData::Numeric(matrix) = &element.data else {
        bail!("{} must carry numeric XYZ data", element.name);
    };
    ensure!(
        matrix.values.len() >= 3,
        "{} needs at least three numeric XYZ values",
        element.name
    );
    Ok([
        matrix.values[0] as f32,
        matrix.values[1] as f32,
        matrix.values[2] as f32,
    ])
}

fn overlay_state_from_element(element: &NimlElement) -> AfniOverlayState {
    let threshold = parse_attr::<bool>(element, "threshold_enabled")
        .unwrap_or(false)
        .then_some(OverlayThresholdCommandState {
            value: parse_attr(element, "threshold_value").unwrap_or(0.0),
            absolute: parse_attr(element, "threshold_absolute").unwrap_or(true),
            hide_failed: parse_attr(element, "threshold_hide_failed").unwrap_or(true),
        });

    AfniOverlayState {
        visible: parse_attr(element, "visible"),
        symmetric_range: parse_attr(element, "symmetric_range"),
        threshold,
        opacity: parse_attr(element, "opacity"),
        overlay_path: path_attr(element, "overlay_path"),
    }
}

fn controller_command_from_element(element: &NimlElement) -> Option<AfniControllerCommand> {
    match attr(element, "command")? {
        "reset_camera" => Some(AfniControllerCommand::ResetCamera),
        "toggle_overlay" => Some(AfniControllerCommand::ToggleOverlay),
        "background_black" => Some(AfniControllerCommand::SetBackground(BackgroundMode::Black)),
        "background_white" => Some(AfniControllerCommand::SetBackground(BackgroundMode::White)),
        "surface_controller_open" => Some(AfniControllerCommand::OpenSurfaceController(true)),
        "surface_controller_closed" => Some(AfniControllerCommand::OpenSurfaceController(false)),
        "roi_controller_open" => Some(AfniControllerCommand::OpenRoiController(true)),
        "roi_controller_closed" => Some(AfniControllerCommand::OpenRoiController(false)),
        _ => None,
    }
}

fn engine_command_from_element(element: &NimlElement) -> Option<AfniControllerCommand> {
    match attr(element, "Command")? {
        "viewer_cont" => {
            if let Some(value) = attr(element, "bkg_col") {
                let is_white = value
                    .split_whitespace()
                    .filter_map(|piece| piece.parse::<f32>().ok())
                    .take(3)
                    .sum::<f32>()
                    > 1.5;
                return Some(AfniControllerCommand::SetBackground(if is_white {
                    BackgroundMode::White
                } else {
                    BackgroundMode::Black
                }));
            }
            None
        }
        "surf_cont" => {
            if parse_attr::<bool>(element, "view_dset") == Some(false) {
                Some(AfniControllerCommand::ToggleOverlay)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn roi_update_from_element(element: &NimlElement) -> AfniRoiUpdate {
    AfniRoiUpdate {
        path: path_attr(element, "path"),
        visible: parse_attr(element, "visible"),
    }
}

fn attr<'a>(element: &'a NimlElement, key: &str) -> Option<&'a str> {
    element.attrs.get(key).map(String::as_str)
}

fn parse_attr<T: NimlAttrParse>(element: &NimlElement, key: &str) -> Option<T> {
    attr(element, key).and_then(T::parse_attr)
}

trait NimlAttrParse: Sized {
    fn parse_attr(value: &str) -> Option<Self>;
}

impl NimlAttrParse for bool {
    fn parse_attr(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "yes" | "y" | "on" | "1" | "true" => Some(true),
            "no" | "n" | "off" | "0" | "false" => Some(false),
            _ => None,
        }
    }
}

impl NimlAttrParse for f32 {
    fn parse_attr(value: &str) -> Option<Self> {
        value.parse().ok()
    }
}

impl NimlAttrParse for usize {
    fn parse_attr(value: &str) -> Option<Self> {
        value.parse().ok()
    }
}

impl NimlAttrParse for u32 {
    fn parse_attr(value: &str) -> Option<Self> {
        value.parse().ok()
    }
}

fn path_attr(element: &NimlElement, key: &str) -> Option<PathBuf> {
    attr(element, key)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

fn env_u16(environ: &BTreeMap<String, String>, key: &str) -> Option<u16> {
    environ.get(key).and_then(|value| value.parse().ok())
}

fn push_opt_attr(attrs: &mut BTreeMap<String, String>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        attrs.insert(key.to_string(), value.to_string());
    }
}

fn push_opt_path_attr(attrs: &mut BTreeMap<String, String>, key: &str, value: Option<&PathBuf>) {
    if let Some(value) = value {
        attrs.insert(key.to_string(), value.display().to_string());
    }
}

fn bool_attr(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn clamp_u8(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

impl std::str::FromStr for AfniControllerCommand {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let mut attrs = BTreeMap::new();
        attrs.insert("command".to_string(), value.to_string());
        controller_command_from_element(&NimlElement::text("SUMARU_viewer_command", attrs, ""))
            .ok_or_else(|| anyhow::anyhow!("unknown AFNI controller command {value}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::SurfacePick;
    use crate::io::{NimlData, parse_niml_str, serialize_niml_ascii};

    #[test]
    fn afni_port_config_matches_pysuma_bloc_logic() {
        let config =
            resolve_afni_port_config("127.0.0.1", None, None, Some(1), &BTreeMap::new()).unwrap();

        assert_eq!(config.port_offset, Some(1048));
        assert_eq!(config.port, 1048);
        assert_eq!(config.port_bloc, Some(1));
    }

    #[test]
    fn surface_registration_uses_afni_element_names_and_shapes() {
        let mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let info = AfniSurfaceInfo::from_mesh(&mesh);

        let elements = surface_registration_elements(&mesh, &info).unwrap();

        assert_eq!(
            elements
                .iter()
                .map(|element| element.name.as_str())
                .collect::<Vec<_>>(),
            vec!["SUMA_ixyz", "SUMA_node_normals", "SUMA_ijk"]
        );
        let NimlData::Numeric(ixyz) = &elements[0].data else {
            panic!("expected numeric ixyz");
        };
        assert_eq!(ixyz.rows, 3);
        assert_eq!(ixyz.column_count(), 4);
        assert_eq!(
            elements[0].attrs.get("surface_idcode"),
            Some(&info.surface_idcode)
        );
        assert_eq!(
            elements[0].attrs.get("afni_surface_controls_nodes"),
            Some(&"none".to_string())
        );
        assert!(
            elements[0]
                .attrs
                .contains_key("afni_surface_controls_lines")
        );
        assert_eq!(
            elements[0].attrs.get("afni_surface_controls_toggle"),
            Some(&"on".to_string())
        );
    }

    #[test]
    fn gifti_surface_registration_flips_lpi_xy_for_afni() {
        let mut mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [10.0, 20.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        mesh.metadata.source_file = Some(PathBuf::from("lh.smoothwm.gii"));
        let info = AfniSurfaceInfo::from_mesh(&mesh);

        let elements = surface_registration_elements(&mesh, &info).unwrap();
        let NimlData::Numeric(ixyz) = &elements[0].data else {
            panic!("expected numeric ixyz");
        };
        let NimlData::Numeric(normals) = &elements[1].data else {
            panic!("expected numeric normals");
        };
        let NimlData::Numeric(ijk) = &elements[2].data else {
            panic!("expected numeric triangles");
        };

        assert_eq!(ixyz.get(1, 1), Some(-10.0));
        assert_eq!(ixyz.get(1, 2), Some(-20.0));
        assert_eq!(ixyz.get(1, 3), Some(0.0));
        assert_eq!(normals.get(0, 0), Some(-0.0));
        assert_eq!(normals.get(0, 1), Some(-0.0));
        assert_eq!(normals.get(0, 2), Some(1.0));
        assert_eq!(ijk.get(0, 0), Some(0.0));
        assert_eq!(ijk.get(0, 1), Some(1.0));
        assert_eq!(ijk.get(0, 2), Some(2.0));
    }

    #[test]
    fn niml_log_summary_keeps_payloads_compact() {
        let matrix = NimlNumericMatrix::from_rows(
            vec![NimlValueType::Int32, NimlValueType::Float32],
            vec![vec![1.0, 2.0], vec![3.0, 4.0]],
        )
        .unwrap();
        let mut attrs = BTreeMap::new();
        attrs.insert("very_long".to_string(), "x".repeat(140));
        let element = NimlElement::numeric("demo", attrs, matrix);

        assert_eq!(
            niml_data_summary(&element.data),
            "numeric rows=2 cols=2 types=[Int32, Float32]"
        );
        let attr_summary = niml_attr_summary(&element.attrs);
        assert!(attr_summary.starts_with("{very_long="));
        assert!(attr_summary.ends_with("...}"));
        assert!(attr_summary.len() < 140);
    }

    #[test]
    fn suma_irgba_roundtrips_and_routes_to_overlay_action() {
        let overlay = AfniRgbaOverlay {
            surface_idcode: "surf-1".to_string(),
            local_domain_parent_id: Some("domain-1".to_string()),
            node_indices: vec![2, 5],
            rgba: vec![[255, 0, 0, 255], [0, 0, 255, 128]],
            threshold: Some("2.5".to_string()),
            function_idcode: Some("func".to_string()),
            volume_idcode: Some("vol".to_string()),
        };
        let serialized = serialize_niml_ascii(&[overlay.to_element().unwrap()]);
        let parsed = parse_niml_str(&serialized).unwrap();
        let message = parse_incoming_message(&parsed[0]).unwrap().unwrap();

        let mut controller = ControllerState::default();
        let outcome = route_incoming_message(&mut controller, message);

        assert!(outcome.applied_state);
        assert!(controller.overlay.visible);
        assert_eq!(controller.overlay.threshold.unwrap().value, 2.5);
        assert!(matches!(
            outcome.actions.as_slice(),
            [AfniRouteAction::RgbaOverlay(AfniRgbaOverlay { surface_idcode, .. })]
                if surface_idcode == "surf-1"
        ));
    }

    #[test]
    fn crosshair_message_updates_shared_interaction_state() {
        let element = NimlElement::numeric(
            "SUMARU_crosshair",
            BTreeMap::new(),
            NimlNumericMatrix::from_rows(
                vec![
                    NimlValueType::Int32,
                    NimlValueType::Int32,
                    NimlValueType::Float32,
                    NimlValueType::Float32,
                    NimlValueType::Float32,
                ],
                vec![vec![7.0, 3.0, 1.0, 2.0, 3.0]],
            )
            .unwrap(),
        );

        let message = parse_incoming_message(&element).unwrap().unwrap();
        let mut controller = ControllerState::default();
        let outcome = route_incoming_message(&mut controller, message);

        assert!(outcome.applied_state);
        assert_eq!(
            controller.interaction.crosshair,
            Some(CrosshairState {
                node_index: 7,
                face_index: 3,
                surface_position: [1.0, 2.0, 3.0]
            })
        );
    }

    #[test]
    fn afni_suma_crosshair_group_routes_surface_node_selection() {
        let mut xyz_attrs = BTreeMap::new();
        xyz_attrs.insert("surface_idcode".to_string(), "surf-1".to_string());
        xyz_attrs.insert("surface_nodeid".to_string(), "42".to_string());
        let xyz = NimlElement::numeric(
            "SUMA_crosshair_xyz",
            xyz_attrs,
            NimlNumericMatrix::from_rows(
                vec![
                    NimlValueType::Float32,
                    NimlValueType::Float32,
                    NimlValueType::Float32,
                ],
                vec![vec![1.0, 2.0, 3.0]],
            )
            .unwrap(),
        );
        let underlay = NimlElement::numeric(
            "underlay_array",
            BTreeMap::new(),
            NimlNumericMatrix::from_rows(vec![NimlValueType::Float32], vec![vec![0.0]]).unwrap(),
        );
        let v2s = NimlElement::numeric(
            "v2s_node_array",
            BTreeMap::new(),
            NimlNumericMatrix::from_rows(vec![NimlValueType::Float32], vec![vec![0.0]]).unwrap(),
        );
        let group = NimlElement::group("SUMA_crosshair", BTreeMap::new(), vec![xyz, underlay, v2s]);

        let message = parse_incoming_message(&group).unwrap().unwrap();
        let mut controller = ControllerState::default();
        let outcome = route_incoming_message(&mut controller, message);

        assert!(outcome.applied_state);
        assert!(matches!(
            outcome.actions.as_slice(),
            [AfniRouteAction::SurfaceCrosshair(AfniSurfaceCrosshair {
                surface_idcode,
                node_index: Some(42),
                surface_position,
            })] if surface_idcode.as_deref() == Some("surf-1")
                && *surface_position == [1.0, 2.0, 3.0]
        ));
    }

    #[test]
    fn surface_crosshair_element_uses_afni_shape_and_gifti_orientation() {
        let mut mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [10.0, 20.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        mesh.metadata.source_file = Some(PathBuf::from("lh.smoothwm.gii"));
        mesh.metadata.lineage.parent_volume_id = Some("AFN_test".to_string());
        let info = AfniSurfaceInfo::from_mesh(&mesh);

        let element = surface_crosshair_element(&mesh, &info, 1, [10.0, 20.0, 0.0]).unwrap();

        assert_eq!(element.name, "SUMA_crosshair_xyz");
        assert_eq!(element.attrs.get("surface_nodeid"), Some(&"1".to_string()));
        assert_eq!(
            element.attrs.get("surface_idcode"),
            Some(&info.surface_idcode)
        );
        assert_eq!(
            element.attrs.get("volume_idcode"),
            Some(&"AFN_test".to_string())
        );
        let NimlData::Numeric(matrix) = &element.data else {
            panic!("expected numeric crosshair data");
        };
        assert_eq!(matrix.rows, 3);
        assert_eq!(matrix.column_count(), 1);
        assert_eq!(matrix.get(0, 0), Some(-10.0));
        assert_eq!(matrix.get(1, 0), Some(-20.0));
        assert_eq!(matrix.get(2, 0), Some(0.0));
    }

    #[test]
    fn outgoing_state_reports_surface_crosshair_and_overlay() {
        let mut controller = ControllerState::default();
        controller.interaction.set_pick(Some(SurfacePick {
            node_index: 4,
            face_index: 2,
            surface_position: [4.0, 5.0, 6.0],
            normalized_position: [0.0, 0.0, 0.0],
            overlay_value: None,
            threshold_value: None,
        }));
        controller.surface.current_overlay_path = Some(PathBuf::from("stats.niml.dset"));
        controller.overlay.threshold = Some(OverlayThresholdCommandState {
            value: 3.1,
            absolute: true,
            hide_failed: true,
        });

        let elements = outgoing_state_elements(&controller).unwrap();

        assert!(
            elements
                .iter()
                .any(|element| element.name == "SUMARU_crosshair")
        );
        let overlay = elements
            .iter()
            .find(|element| element.name == "SUMARU_overlay_state")
            .unwrap();
        assert_eq!(
            overlay.attrs.get("overlay_path"),
            Some(&"stats.niml.dset".to_string())
        );
        assert_eq!(
            overlay.attrs.get("threshold_value"),
            Some(&"3.1".to_string())
        );
    }

    #[test]
    fn session_registers_surface_once_and_applies_received_elements() {
        let mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let info = AfniSurfaceInfo::from_mesh(&mesh);
        let mut session = AfniNimlSession::new();

        assert!(
            session
                .register_surface_once(&mesh, &info)
                .unwrap()
                .is_some()
        );
        assert!(
            session
                .register_surface_once(&mesh, &info)
                .unwrap()
                .is_none()
        );

        let mut attrs = BTreeMap::new();
        attrs.insert("command".to_string(), "background_white".to_string());
        let command = NimlElement::text("SUMARU_viewer_command", attrs, "");
        let mut controller = ControllerState::default();

        let outcome = session
            .receive_element(&mut controller, &command)
            .unwrap()
            .unwrap();

        assert!(outcome.applied_state);
        assert_eq!(controller.display.background, BackgroundMode::White);
    }
}
