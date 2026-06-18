use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;

use crate::surface::{SurfaceId, SurfaceSide};

const STATUS_LOG_LIMIT: usize = 256;
const DEFAULT_PAIR_MAX_OPEN_DEGREES: f32 = 85.0;
const DEFAULT_PAIR_ACORN_EXTRA_GAP: f32 = 50.0;

#[derive(Debug, Clone, Default)]
pub struct ControllerState {
    pub camera: CameraCommandState,
    pub display: DisplayCommandState,
    pub overlay: OverlayCommandState,
    pub roi: RoiCommandState,
    pub surface: SurfaceCommandState,
    pub interaction: InteractionState,
    pub panels: PanelState,
    status_log: StatusLog,
}

impl ControllerState {
    pub fn record_status(&self, message: impl Into<String>) {
        self.status_log.push(message.into());
    }

    pub fn status_entries(&self) -> Vec<StatusEvent> {
        self.status_log.entries()
    }

    pub fn clear_status_entries(&self) {
        self.status_log.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraCommandState {
    pub mode: CameraControlMode,
    pub last_preset: Option<ViewPreset>,
    pub reset_generation: u64,
}

impl Default for CameraCommandState {
    fn default() -> Self {
        Self {
            mode: CameraControlMode::Orbit,
            last_preset: None,
            reset_generation: 0,
        }
    }
}

impl CameraCommandState {
    pub fn toggle_mode(&mut self) -> CameraControlMode {
        self.mode = self.mode.toggled();
        self.mode
    }

    pub fn set_preset(&mut self, preset: ViewPreset) {
        self.last_preset = Some(preset);
    }

    pub fn note_reset(&mut self) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.last_preset = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraControlMode {
    Orbit,
    Turntable,
}

impl CameraControlMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Orbit => "orbit",
            Self::Turntable => "turntable",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Orbit => Self::Turntable,
            Self::Turntable => Self::Orbit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewPreset {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayCommandState {
    pub background: BackgroundMode,
    pub anatomical_shading_visible: bool,
    pub pair_layout: HemisphereLayout,
    pub pair_state: HemisphereLayoutState,
    pub pair_visibility: PairVisibility,
}

impl Default for DisplayCommandState {
    fn default() -> Self {
        Self {
            background: BackgroundMode::Black,
            anatomical_shading_visible: false,
            pair_layout: HemisphereLayout::Closed,
            pair_state: HemisphereLayoutState::closed(),
            pair_visibility: PairVisibility::both(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundMode {
    Black,
    White,
}

impl BackgroundMode {
    pub fn toggle(&mut self) {
        *self = match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        };
    }

    pub fn next_label(self) -> &'static str {
        match self {
            Self::Black => "White background",
            Self::White => "Black background",
        }
    }

    pub fn rgba8(self) -> [u8; 4] {
        match self {
            Self::Black => [0, 0, 0, 255],
            Self::White => [255, 255, 255, 255],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HemisphereLayout {
    Closed,
    Open,
}

impl HemisphereLayout {
    pub fn label(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HemisphereLayoutState {
    pub open_angle_degrees: f32,
    pub separation_distance: f32,
}

impl HemisphereLayoutState {
    pub fn closed() -> Self {
        Self {
            open_angle_degrees: 0.0,
            separation_distance: 0.0,
        }
    }

    pub fn acorn() -> Self {
        Self::acorn_signed(1.0)
    }

    pub fn acorn_signed(sign: f32) -> Self {
        Self {
            open_angle_degrees: DEFAULT_PAIR_MAX_OPEN_DEGREES * sign.signum(),
            separation_distance: DEFAULT_PAIR_ACORN_EXTRA_GAP,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairVisibility {
    pub left: bool,
    pub right: bool,
}

impl PairVisibility {
    pub fn both() -> Self {
        Self {
            left: true,
            right: true,
        }
    }

    pub fn is_visible(self, side: &SurfaceSide) -> bool {
        match side {
            SurfaceSide::Left => self.left,
            SurfaceSide::Right => self.right,
            _ => true,
        }
    }

    pub fn toggled(self, side: SurfaceSide) -> Option<Self> {
        let mut next = self;
        match side {
            SurfaceSide::Left => next.left = !next.left,
            SurfaceSide::Right => next.right = !next.right,
            _ => return None,
        }
        (next.left || next.right).then_some(next)
    }

    pub fn label(self) -> &'static str {
        match (self.left, self.right) {
            (true, true) => "left+right",
            (true, false) => "left only",
            (false, true) => "right only",
            (false, false) => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayCommandState {
    pub visible: bool,
}

impl Default for OverlayCommandState {
    fn default() -> Self {
        Self { visible: true }
    }
}

/// Overlay threshold display state shared by the render appearance and the AFNI
/// wire protocol. `enabled` carries the on/off state; in the partial-update
/// `AfniOverlayState` this type is wrapped in `Option` to mean "present in the
/// message" instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayThreshold {
    pub enabled: bool,
    pub absolute: bool,
    pub value: f32,
    pub hide_failed: bool,
}

impl Default for OverlayThreshold {
    fn default() -> Self {
        Self {
            enabled: false,
            absolute: true,
            value: 0.0,
            hide_failed: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiCommandState {
    pub visible: bool,
    pub active_slot: usize,
}

impl Default for RoiCommandState {
    fn default() -> Self {
        Self {
            visible: true,
            active_slot: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SurfaceCommandState {
    pub current_surface_id: Option<SurfaceId>,
    pub current_surface_path: Option<PathBuf>,
    pub current_surface_volume_path: Option<PathBuf>,
    pub current_overlay_path: Option<PathBuf>,
    pub current_roi_path: Option<PathBuf>,
    pub current_scene_surface_index: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct InteractionState {
    pub pick: Option<SurfacePick>,
    pub crosshair: Option<CrosshairState>,
}

impl InteractionState {
    pub fn set_pick(&mut self, pick: Option<SurfacePick>) {
        self.crosshair = pick.map(CrosshairState::from);
        self.pick = pick;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfacePick {
    pub node_index: u32,
    pub face_index: usize,
    pub surface_position: [f32; 3],
    pub normalized_position: [f32; 3],
    pub overlay_value: Option<f32>,
    pub threshold_value: Option<f32>,
}

impl SurfacePick {
    pub fn status_text(self) -> String {
        format!(
            "Inspected node {}, triangle {}, I {}; T {}.",
            self.node_index,
            self.face_index,
            value_label(self.overlay_value),
            value_label(self.threshold_value)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrosshairState {
    pub node_index: u32,
    pub face_index: usize,
    pub surface_position: [f32; 3],
}

impl From<SurfacePick> for CrosshairState {
    fn from(pick: SurfacePick) -> Self {
        Self {
            node_index: pick.node_index,
            face_index: pick.face_index,
            surface_position: pick.surface_position,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelState {
    pub surface_controller_visible: bool,
    pub roi_controller_open: bool,
    pub graph_window_open: bool,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            surface_controller_visible: true,
            roi_controller_open: false,
            graph_window_open: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StatusEvent {
    pub message: String,
}

#[derive(Debug, Default)]
struct StatusLog {
    entries: RefCell<VecDeque<StatusEvent>>,
}

impl Clone for StatusLog {
    fn clone(&self) -> Self {
        Self {
            entries: RefCell::new(self.entries.borrow().clone()),
        }
    }
}

impl StatusLog {
    fn push(&self, message: String) {
        let mut entries = self.entries.borrow_mut();
        entries.push_back(StatusEvent { message });
        while entries.len() > STATUS_LOG_LIMIT {
            entries.pop_front();
        }
    }

    fn entries(&self) -> Vec<StatusEvent> {
        self.entries.borrow().iter().cloned().collect()
    }

    fn clear(&self) {
        self.entries.borrow_mut().clear();
    }
}

#[derive(Debug, Clone)]
pub enum ViewerCommand {
    PickSurface,
    PickOverlay,
    PickRoi,
    PickSpec,
    PickSurfaceVolume,
    RefreshOverlayColumns,
    RefreshOverlayAppearance,
    ResetCamera,
    ToggleCameraMode,
    ToggleCameraMomentum,
    ToggleBackground,
    SetAnatomicalShadingVisible(bool),
    SetOverlayVisible(bool),
    SetRoiVisible(bool),
    SetRoiSlotVisible(usize, bool),
    ClearRoi,
    ToggleRoiDraw(usize, bool),
    JoinRoiDraft(usize),
    ArmRoiFill(usize),
    UndoRoiDraft(usize),
    RedoRoiDraft(usize),
    FinalizeRoiSlot(usize),
    EditRoiSlot(usize),
    DeleteRoiSlot(usize),
    SaveRoiSlot(usize),
    SaveAllRois,
    SetSurfaceControllerVisible(bool),
    SetRoiControllerOpen(bool),
    OpenGraphForPick,
    SetGraphWindowOpen(bool),
    Preset(ViewPreset),
    HemisphereLayout(HemisphereLayout),
    SelectSceneSurface(usize),
    SaveScreenshot,
    SaveMontage,
    /// Spawn a fresh, empty sumaru window — no surface, overlay, or session
    /// context carried over.
    LaunchNewInstance,
    /// Spawn a new sumaru window preloaded with the current surface/spec (and
    /// surface volume), but no overlay or ROI, as a clean second analysis view.
    LaunchDuplicateInstance,
    /// Add an axial slice plane in `--volume` mode.
    AddVolumeAxial,
    /// Add a coronal slice plane in `--volume` mode.
    AddVolumeCoronal,
    /// Add a sagittal slice plane in `--volume` mode.
    AddVolumeSagittal,
    /// Remove the currently selected slice in `--volume` mode.
    RemoveSelectedVolumeSlice,
}

fn value_label(value: Option<f32>) -> String {
    value.map_or_else(|| "not loaded".to_string(), |value| format!("{value:.4}"))
}

#[cfg(test)]
mod tests {
    use super::{
        BackgroundMode, CameraControlMode, ControllerState, PairVisibility, SurfacePick, ViewPreset,
    };
    use crate::surface::SurfaceSide;

    #[test]
    fn background_toggles_between_black_and_white() {
        let mut background = BackgroundMode::Black;

        background.toggle();
        assert_eq!(background, BackgroundMode::White);

        background.toggle();
        assert_eq!(background, BackgroundMode::Black);
    }

    #[test]
    fn camera_command_state_tracks_mode_preset_and_reset() {
        let mut state = ControllerState::default();

        assert_eq!(state.camera.toggle_mode(), CameraControlMode::Turntable);
        state.camera.set_preset(ViewPreset::Top);
        assert_eq!(state.camera.last_preset, Some(ViewPreset::Top));
        state.camera.note_reset();

        assert_eq!(state.camera.reset_generation, 1);
        assert_eq!(state.camera.last_preset, None);
    }

    #[test]
    fn pair_visibility_keeps_at_least_one_hemisphere_visible() {
        let visibility = PairVisibility::both()
            .toggled(SurfaceSide::Left)
            .expect("left can be hidden");

        assert_eq!(visibility.label(), "right only");
        assert!(visibility.toggled(SurfaceSide::Right).is_none());
    }

    #[test]
    fn interaction_pick_updates_crosshair() {
        let mut state = ControllerState::default();
        let pick = SurfacePick {
            node_index: 7,
            face_index: 3,
            surface_position: [1.0, 2.0, 3.0],
            normalized_position: [0.1, 0.2, 0.3],
            overlay_value: Some(1.5),
            threshold_value: None,
        };

        state.interaction.set_pick(Some(pick));

        assert_eq!(state.interaction.pick, Some(pick));
        assert_eq!(state.interaction.crosshair.unwrap().node_index, 7);
    }

    #[test]
    fn status_log_is_bounded_and_replayable() {
        let state = ControllerState::default();
        state.record_status("one");
        state.record_status("two");

        let messages = state
            .status_entries()
            .into_iter()
            .map(|event| event.message)
            .collect::<Vec<_>>();

        assert_eq!(messages, vec!["one", "two"]);
    }
}
