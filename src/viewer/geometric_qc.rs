// Developed by P. Molfese, National Institutes of Health (NIH).
// See LICENSE for the public-domain notice and disclaimer.
//! Explicit geometric maps never invent an exact child-to-parent identity.
use super::*;
use crate::surface_mapping::GeometricMap;

pub(super) struct GeometricQcState {
    pub map: GeometricMap,
    source: SurfaceMesh,
    pub visible: bool,
    transformed: bool,
    pub show_distances: bool,
    pub instance: Option<SurfaceRenderInstance>,
    pub distance_instance: Option<SurfaceRenderInstance>,
}
impl ViewerState {
    pub(super) fn load_geometric_qc(&mut self) -> Result<()> {
        let Some(path) = &self.parent_load.qc_map else {
            return Ok(());
        };
        ensure!(
            self.surface_qc.is_none() && !self.has_component_surface(),
            "geometric QC requires one target hemisphere and cannot use --qc-child"
        );
        let source = SurfaceMesh::from_path(
            self.parent_load
                .qc_source
                .as_ref()
                .context("--qc-source required")?,
        )?;
        let target = self
            .mesh
            .as_ref()
            .context("load target geometry for --qc-map")?;
        let map = crate::surface_mapping_cli::read_map(path, &source, target)?;
        self.geometric_qc = Some(GeometricQcState {
            map,
            source,
            visible: true,
            transformed: true,
            show_distances: false,
            instance: None,
            distance_instance: None,
        });
        self.upload_geometric_qc()?;
        Ok(())
    }
    fn upload_geometric_qc(&mut self) -> Result<()> {
        let (Some(qc), Some(target)) = (&self.geometric_qc, &self.mesh) else {
            return Ok(());
        };
        let transform = crate::surface_mapping::affine(qc.map.source_to_target)?;
        let vertices = qc
            .source
            .vertices
            .iter()
            .map(|&p| {
                if qc.transformed {
                    transform
                        .transform_point3(glam::DVec3::from_array(p.map(f64::from)))
                        .as_vec3()
                        .to_array()
                } else {
                    p
                }
            })
            .collect();
        let source = SurfaceMesh::new(vertices, qc.source.triangles.clone())?;
        let radius = target.bounds.radius.max(f32::EPSILON);
        let model = Mat4::from_scale(Vec3::splat(1.0 / radius))
            * Mat4::from_translation(-Vec3::from_array(target.bounds.center));
        let geometry = prepared_geometry_from_raw_component(
            &source.vertices,
            &source.vertex_normals(),
            &source.triangles,
        );
        let colors = vec![[1.0, 0.72, 0.10, 1.0]; source.vertices.len()];
        let prepared = PreparedSurface::from_geometry_color_slices(
            &geometry,
            Some(&colors),
            None,
            1.0,
            None,
            None,
        );
        let instance = self.surface_render_instance_from_prepared(
            &prepared,
            SurfaceSide::Unknown,
            model,
            CHUNKED_SURFACE_BUFFER_LABELS,
        );
        let maximum = qc
            .map
            .rows
            .iter()
            .filter_map(|r| r.distance)
            .fold(0.0_f64, f64::max)
            .max(f64::EPSILON);
        let colors = qc
            .map
            .rows
            .iter()
            .map(|r| {
                if r.weights.is_empty() {
                    [1.0, 0.0, 1.0, 1.0]
                } else if let Some(d) = r.distance {
                    let t = (d / maximum).clamp(0.0, 1.0) as f32;
                    [t, 0.2, 1.0 - t, 1.0]
                } else {
                    [0.5, 0.5, 0.5, 1.0]
                }
            })
            .collect::<Vec<_>>();
        let geometry = prepared_geometry_from_raw_component(
            &target.vertices,
            &target.vertex_normals(),
            &target.triangles,
        );
        let prepared = PreparedSurface::from_geometry_color_slices(
            &geometry,
            Some(&colors),
            None,
            1.0,
            None,
            None,
        );
        let distance_instance = self.surface_render_instance_from_prepared(
            &prepared,
            SurfaceSide::Unknown,
            model,
            CHUNKED_SURFACE_BUFFER_LABELS,
        );
        let qc = self.geometric_qc.as_mut().unwrap();
        qc.instance = Some(instance);
        qc.distance_instance = Some(distance_instance);
        Ok(())
    }
    pub(super) fn geometric_qc_report(&self) -> Option<serde_json::Value> {
        let qc = self.geometric_qc.as_ref()?;
        Some(
            serde_json::json!({"schema":"sumaru-geometric-qc-1", "map_path":self.parent_load.qc_map,"source":self.parent_load.qc_source,"target":self.surface_path,"mapping":qc.map.summary(),"source_visible":qc.visible,"source_transform_applied":qc.transformed,"show_distances":qc.show_distances,"selected_target":self.controller.interaction.pick.map(|p|p.node_index),"selected_row":self.controller.interaction.pick.and_then(|p|qc.map.rows.get(p.node_index as usize)),"camera_view_projection":self.camera.view_projection_matrix(self.scene_viewport_aspect()).to_cols_array(),"overlay":self.overlay.source.path}),
        )
    }
    pub(super) fn draw_geometric_qc_controls(&mut self, ui: &mut egui::Ui) {
        let Some(qc) = self.geometric_qc.as_mut() else {
            return;
        };
        let mut changed = false;
        controller_section(ui, "GEOMETRIC MAP QC", true, |ui| {
            ui.label(format!(
                "{} · {} → {} vertices",
                qc.map.method, qc.map.source_nodes, qc.map.target_nodes
            ));
            ui.checkbox(&mut qc.visible, "Source wireframe (show through target)");
            changed = ui
                .checkbox(
                    &mut qc.transformed,
                    "Apply declared source-to-target affine",
                )
                .changed();
            ui.small("Shared target camera. This switch changes display only; exported values stay fixed.");
            ui.checkbox(&mut qc.show_distances, "Mapping distances at target nodes");
            ui.small("Blue → red: zero → maximum distance. Magenta: missing. Gray: distance unavailable. Points show through the target.");
            ui.small(&qc.map.distance_units);
            ui.label(format!(
                "{} missing · {} fallback rows",
                qc.map.rows.iter().filter(|r| r.weights.is_empty()).count(),
                qc.map.rows.iter().filter(|r| r.fallback).count()
            ));
            if let Some(row) = self
                .controller
                .interaction
                .pick
                .and_then(|p| qc.map.rows.get(p.node_index as usize))
            {
                ui.label(format!(
                    "Distance {:?}; signed {:?}; fallback {}",
                    row.distance, row.signed_distance, row.fallback
                ));
                ui.small(format!("Source local IDs: {:?}", row.source_nodes));
                ui.small(format!("Gather weights: {:?}", row.weights));
            }
        });
        if changed && let Err(error) = self.upload_geometric_qc() {
            self.set_error(error);
        }
    }
}
