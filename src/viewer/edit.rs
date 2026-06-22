//! "Edit" menu actions: copy the picked vertex's index or coordinates to the
//! clipboard, and jump the selection to a typed/pasted coordinate or vertex.
//!
//! Coordinates round-trip with AFNI. Raw surface coordinates are treated as
//! AFNI's RAI/DICOM mm (the order AFNI's "Jump to (xyz)" expects by default);
//! the RAS convention papers/MNI use negates the x and y axes. If a round-trip
//! against a real AFNI session comes out mirror-flipped, the single place to fix
//! the assumption is `convert_from_raw`.

use super::*;

/// Coordinate convention for copy/jump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoordConvention {
    /// AFNI/DICOM order — what AFNI's jump field expects, and the convention the
    /// raw surface coordinates are assumed to already be in.
    Rai,
    /// RAS order (x = +right, y = +anterior) — the convention papers/MNI use.
    Ras,
}

impl CoordConvention {
    pub(super) fn label(self) -> &'static str {
        match self {
            CoordConvention::Rai => "RAI (AFNI)",
            CoordConvention::Ras => "RAS",
        }
    }
}

/// Convert a raw surface coordinate (assumed RAI/DICOM mm) into the chosen
/// convention. RAS negates the x and y axes; the mapping is its own inverse, so
/// the same function converts back.
fn convert_from_raw(coord: [f32; 3], convention: CoordConvention) -> [f32; 3] {
    match convention {
        CoordConvention::Rai => coord,
        CoordConvention::Ras => [-coord[0], -coord[1], coord[2]],
    }
}

/// State for the "Go to Location" popup window.
pub(super) struct GoToLocationState {
    pub(super) open: bool,
    pub(super) convention: CoordConvention,
    pub(super) xyz_text: String,
    pub(super) vertex_text: String,
}

impl Default for GoToLocationState {
    fn default() -> Self {
        Self {
            open: false,
            convention: CoordConvention::Rai,
            xyz_text: String::new(),
            vertex_text: String::new(),
        }
    }
}

impl ViewerState {
    /// Copy the picked vertex index to the clipboard.
    pub(super) fn copy_vertex_index(&mut self) {
        let Some(pick) = self.controller.interaction.pick else {
            self.log_status("Right-click a vertex first, then copy.");
            return;
        };
        self.set_clipboard_text(
            pick.node_index.to_string(),
            format!("vertex {}", pick.node_index),
        );
    }

    /// Copy the picked vertex's coordinate in the given convention.
    pub(super) fn copy_picked_xyz(&mut self, convention: CoordConvention) {
        let Some(pick) = self.controller.interaction.pick else {
            self.log_status("Right-click a vertex first, then copy.");
            return;
        };
        // `surface_position` is the anatomical coordinate, untouched by any
        // paired-hemisphere display layout — exactly what should be copied.
        let text = format_xyz(convert_from_raw(pick.surface_position, convention));
        self.set_clipboard_text(text.clone(), format!("{} {text}", convention.label()));
    }

    fn set_clipboard_text(&mut self, text: String, description: String) {
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
            Ok(()) => self.log_status(format!("Copied {description} to clipboard.")),
            Err(error) => self.set_error(anyhow::anyhow!("clipboard copy failed: {error}")),
        }
    }

    /// Read a coordinate from the clipboard and jump to the nearest vertex,
    /// interpreting the text as RAI/DICOM (AFNI's convention).
    pub(super) fn paste_location(&mut self) {
        let text = match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
            Ok(text) => text,
            Err(error) => {
                self.set_error(anyhow::anyhow!("clipboard read failed: {error}"));
                return;
            }
        };
        let Some(coord) = parse_xyz(&text) else {
            self.log_status(format!(
                "Clipboard does not contain an x y z coordinate: {:?}",
                text.trim()
            ));
            return;
        };
        self.jump_to_world(coord, CoordConvention::Rai);
    }

    /// Jump using the popup's XYZ field and selected convention.
    pub(super) fn submit_go_to_xyz(&mut self) {
        let convention = self.go_to_location.convention;
        let Some(coord) = parse_xyz(&self.go_to_location.xyz_text) else {
            self.log_status("Enter a coordinate as `x y z`.");
            return;
        };
        self.jump_to_world(coord, convention);
    }

    /// Jump using the popup's vertex-index field.
    pub(super) fn submit_go_to_vertex(&mut self) {
        let Ok(node) = self.go_to_location.vertex_text.trim().parse::<u32>() else {
            self.log_status("Enter a whole-number vertex index.");
            return;
        };
        self.jump_to_vertex(node);
    }

    /// Move the selection to the nearest vertex to a coordinate given in
    /// `convention`.
    fn jump_to_world(&mut self, coord: [f32; 3], convention: CoordConvention) {
        let raw = convert_from_raw(coord, convention);
        let Some(mesh) = self.mesh.as_ref() else {
            self.log_status("Load a surface before jumping to a location.");
            return;
        };
        let Some((node, distance)) = mesh.nearest_node_to_world(raw) else {
            self.log_status("No surface vertex found near that location.");
            return;
        };
        self.apply_jump_to_node(node);
        let note = if distance > 5.0 {
            format!(" (nearest vertex is {distance:.1} mm away)")
        } else {
            String::new()
        };
        self.log_status(format!(
            "Jumped to {} {} → vertex {node}{note}.",
            convention.label(),
            format_xyz(coord)
        ));
    }

    /// Move the selection to a specific vertex index.
    fn jump_to_vertex(&mut self, node: u32) {
        let Some(mesh) = self.mesh.as_ref() else {
            self.log_status("Load a surface before jumping to a vertex.");
            return;
        };
        if node as usize >= mesh.vertices.len() {
            self.log_status(format!(
                "Vertex {node} is out of range (0..{}).",
                mesh.vertices.len()
            ));
            return;
        }
        self.apply_jump_to_node(node);
        self.log_status(format!("Jumped to vertex {node}."));
    }

    /// Synthesize a pick at `node` and apply it like a right-click pick (updates
    /// the crosshair, AFNI talk, graph, and render).
    fn apply_jump_to_node(&mut self, node: u32) {
        let Some(mesh) = self.mesh.as_ref() else {
            return;
        };
        let Some(pick) = surface_pick_for_mesh_node(mesh, self.overlay.data.node_values(), node)
        else {
            return;
        };
        self.log_status(pick.status_text());
        self.controller.interaction.set_pick(Some(pick));
        if let Err(error) = self.send_afni_crosshair_for_pick(pick) {
            self.set_error(error);
        }
        self.upload_surface_buffers();
        self.refresh_graph_snapshot_if_open();
        self.view_window().request_redraw();
    }

    /// Draw the "Go to Location" popup window when open.
    pub(super) fn draw_go_to_location(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<ViewerCommand>,
    ) {
        if !self.go_to_location.open {
            return;
        }
        let mut open = true;
        egui::Window::new("Go to Location")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Coordinate (x y z):");
                let xyz = ui.text_edit_singleline(&mut self.go_to_location.xyz_text);
                ui.horizontal(|ui| {
                    ui.radio_value(
                        &mut self.go_to_location.convention,
                        CoordConvention::Rai,
                        "RAI (AFNI)",
                    );
                    ui.radio_value(
                        &mut self.go_to_location.convention,
                        CoordConvention::Ras,
                        "RAS",
                    );
                });
                let xyz_submit = xyz.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Jump to XYZ").clicked() || xyz_submit {
                    actions.push(ViewerCommand::SubmitGoToXyz);
                }
                ui.separator();
                ui.label("Vertex index:");
                let vertex = ui.text_edit_singleline(&mut self.go_to_location.vertex_text);
                let vertex_submit =
                    vertex.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Jump to Vertex").clicked() || vertex_submit {
                    actions.push(ViewerCommand::SubmitGoToVertex);
                }
            });
        if !open {
            actions.push(ViewerCommand::SetGoToLocationOpen(false));
        }
    }
}

/// Format an xyz coordinate as space-separated mm with 2 decimals
/// (AFNI-jump-field friendly).
fn format_xyz(xyz: [f32; 3]) -> String {
    format!("{:.2} {:.2} {:.2}", xyz[0], xyz[1], xyz[2])
}

/// Parse the first three finite floats out of arbitrary clipboard/typed text,
/// tolerating commas, tabs, parens/brackets, and surrounding labels. Tokens that
/// are not *entirely* numeric are skipped, so `"MNI152 -20 30 10"` parses as
/// `(-20, 30, 10)` rather than grabbing `152`.
fn parse_xyz(text: &str) -> Option<[f32; 3]> {
    let values: Vec<f32> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+'))
        .filter_map(|token| token.parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .take(3)
        .collect();
    match values.as_slice() {
        [x, y, z] => Some([*x, *y, *z]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_varied_coordinate_formats() {
        assert_eq!(parse_xyz("-20 30 10"), Some([-20.0, 30.0, 10.0]));
        assert_eq!(parse_xyz("(-20, 30, 10)"), Some([-20.0, 30.0, 10.0]));
        assert_eq!(parse_xyz("x=-20.5  y=30  z=10"), Some([-20.5, 30.0, 10.0]));
        assert_eq!(parse_xyz("MNI152: -20 30 10"), Some([-20.0, 30.0, 10.0]));
        assert_eq!(parse_xyz("nope"), None);
        assert_eq!(parse_xyz("1 2"), None);
    }

    #[test]
    fn ras_negates_x_and_y_and_is_its_own_inverse() {
        let raw = [12.0, -8.0, 5.0];
        let ras = convert_from_raw(raw, CoordConvention::Ras);
        assert_eq!(ras, [-12.0, 8.0, 5.0]);
        assert_eq!(convert_from_raw(ras, CoordConvention::Ras), raw);
        assert_eq!(convert_from_raw(raw, CoordConvention::Rai), raw);
    }
}
