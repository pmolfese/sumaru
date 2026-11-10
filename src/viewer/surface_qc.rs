// Developed by P. Molfese, National Institutes of Health (NIH).
// See LICENSE for the public-domain notice and disclaimer.
//! Native parent/child QC overlay. Both surfaces share the parent's display transform.
use super::*;
use crate::surface_qc::SurfaceCorrespondence;

pub(super) struct SurfaceQcState {
    pub correspondence: SurfaceCorrespondence,
    pub parent_domain: crate::surface::SurfaceDomainId,
    pub parent_surface_id: SurfaceId,
    pub parent_visible: bool,
    pub child_visible: bool,
    pub mapped_positions: bool,
    pub pick_child: bool,
    pub xray: bool,
    pub child_style: SurfaceRenderStyle,
    pub instance: Option<SurfaceRenderInstance>,
    pub export_provenance: Option<serde_json::Value>,
    pub raw_source: Option<Dataset>,
    pub overlay_path: Option<PathBuf>,
    pub measured_ids: Vec<u32>,
    pub show_measured: bool,
    pub measured_instance: Option<SurfaceRenderInstance>,
}

impl ViewerState {
    pub(super) fn load_surface_qc(&mut self) -> Result<()> {
        let Some(path) = self.parent_load.qc_child.as_ref() else {
            return Ok(());
        };
        ensure!(
            !self.has_component_surface(),
            "QC currently takes one full parent hemisphere; open each hemisphere separately"
        );
        let parent = self
            .mesh
            .as_ref()
            .context("load the full parent before --qc-child")?;
        let child = SurfaceMesh::from_path(path)?;
        let correspondence = SurfaceCorrespondence::new(
            child,
            parent,
            self.parent_load
                .policy_override
                .unwrap_or(self.preferences.parent_mismatch_policy),
        )?;
        for warning in &correspondence.assessment.warnings {
            eprintln!("Warning: QC: {warning}");
        }
        let export_provenance = self
            .overlay
            .source
            .path
            .as_ref()
            .map(|path| crate::resample::read_provenance(path))
            .transpose()?
            .flatten();
        let raw_source = export_provenance.as_ref().and_then(|report| {
            match crate::resample::load_raw_export_source(report, parent) {
                Ok(source) => Some(source),
                Err(error) => {
                    eprintln!("Warning: QC original sampled traces unavailable: {error}");
                    None
                }
            }
        });
        self.surface_qc = Some(SurfaceQcState {
            correspondence,
            parent_domain: parent.domain.id.clone(),
            parent_surface_id: parent.metadata.id.clone(),
            parent_visible: true,
            child_visible: true,
            mapped_positions: true,
            pick_child: true,
            xray: true,
            child_style: SurfaceRenderStyle::Triangles,
            instance: None,
            export_provenance,
            raw_source,
            overlay_path: self.overlay.source.path.clone(),
            measured_ids: vec![],
            show_measured: false,
            measured_instance: None,
        });
        self.refresh_qc_measurements();
        self.upload_qc_surface();
        Ok(())
    }

    pub(super) fn refresh_qc_source(&mut self) -> Result<()> {
        if !self
            .surface_qc
            .as_ref()
            .is_some_and(|qc| qc.overlay_path != self.overlay.source.path)
        {
            return Ok(());
        }
        let provenance = match self
            .overlay
            .source
            .path
            .as_ref()
            .map(|path| crate::resample::read_provenance(path))
            .transpose()
        {
            Ok(value) => value.flatten(),
            Err(error) => {
                self.surface_qc = None;
                return Err(error);
            }
        };
        let source = provenance.as_ref().and_then(|report| {
            self.mesh
                .as_ref()
                .and_then(|mesh| crate::resample::load_raw_export_source(report, mesh).ok())
        });
        let qc = self.surface_qc.as_mut().unwrap();
        qc.export_provenance = provenance;
        qc.raw_source = source;
        qc.overlay_path = self.overlay.source.path.clone();
        Ok(())
    }

    pub(super) fn refresh_qc_measurements(&mut self) {
        let Some(qc) = self.surface_qc.as_ref() else {
            return;
        };
        let dataset = self
            .timecourse
            .as_ref()
            .map(|s| &s.source_dataset)
            .or_else(|| self.overlay.data.dataset());
        let ids = qc
            .export_provenance
            .as_ref()
            .and_then(|p| p["ordered_source_vertex_ids"].as_array())
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_u64().and_then(|id| u32::try_from(id).ok()))
                    .collect::<Vec<_>>()
            })
            .or_else(|| {
                dataset.map(|d| {
                    d.node_indices
                        .clone()
                        .unwrap_or_else(|| (0..d.row_count as u32).collect())
                })
            })
            .unwrap_or_default();
        if ids != qc.measured_ids {
            self.surface_qc.as_mut().unwrap().measured_ids = ids;
            self.upload_qc_surface();
        }
    }

    pub(super) fn upload_qc_surface(&mut self) {
        let (Some(qc), Some(parent)) = (&self.surface_qc, &self.mesh) else {
            return;
        };
        let mesh = if qc.mapped_positions {
            &qc.correspondence.mapped
        } else {
            &qc.correspondence.original
        };
        let geometry = prepared_geometry_from_raw_component(
            &mesh.vertices,
            &mesh.vertex_normals(),
            &mesh.triangles,
        );
        let colors = vec![[1.0, 0.72, 0.10, 1.0]; mesh.vertices.len()];
        let prepared = PreparedSurface::from_geometry_color_slices(
            &geometry,
            Some(&colors),
            None,
            1.0,
            None,
            None,
        );
        let radius = parent.bounds.radius.max(f32::EPSILON);
        let model = Mat4::from_scale(Vec3::splat(1.0 / radius))
            * Mat4::from_translation(-Vec3::from_array(parent.bounds.center));
        let instance = self.surface_render_instance_from_prepared(
            &prepared,
            SurfaceSide::Unknown,
            model,
            CHUNKED_SURFACE_BUFFER_LABELS,
        );
        let normals = parent.vertex_normals();
        let sample_geometry = PreparedGeometry {
            vertices: qc
                .measured_ids
                .iter()
                .filter_map(|id| {
                    Some(PreparedGeometryVertex {
                        position: *parent.vertices.get(*id as usize)?,
                        normal: normals[*id as usize],
                    })
                })
                .collect(),
            indices: vec![],
        };
        let measured_instance = if sample_geometry.vertices.is_empty() {
            None
        } else {
            let colors = vec![[0.0, 1.0, 1.0, 1.0]; sample_geometry.vertices.len()];
            let prepared = PreparedSurface::from_geometry_color_slices(
                &sample_geometry,
                Some(&colors),
                None,
                1.0,
                None,
                None,
            );
            Some(self.surface_render_instance_from_prepared(
                &prepared,
                SurfaceSide::Unknown,
                model,
                CHUNKED_SURFACE_BUFFER_LABELS,
            ))
        };
        let qc = self.surface_qc.as_mut().unwrap();
        qc.instance = Some(instance);
        qc.measured_instance = measured_instance;
    }

    pub(super) fn pick_qc_child(&self, cursor: (f64, f64)) -> Option<SurfacePick> {
        let qc = self.surface_qc.as_ref()?;
        if !qc.child_visible || !qc.pick_child {
            return None;
        }
        let mesh = if qc.mapped_positions {
            &qc.correspondence.mapped
        } else {
            &qc.correspondence.original
        };
        let instance = qc.instance.as_ref()?;
        let (pick, _) = pick_surface_with_model(
            mesh,
            None,
            &self.camera,
            self.scene_viewport_size(),
            cursor,
            instance.model_matrix,
            0,
            0,
        )?;
        let parent_id = *qc
            .correspondence
            .mapping
            .indices
            .get(pick.node_index as usize)?;
        surface_pick_for_mesh_node(
            self.mesh.as_ref()?,
            self.overlay.data.node_values(),
            parent_id,
        )
    }

    pub(super) fn qc_report(&self) -> Option<serde_json::Value> {
        if self.geometric_qc.is_some() {
            return self.geometric_qc_report();
        }
        let qc = self.surface_qc.as_ref()?;
        let selected = self.controller.interaction.pick;
        Some(
            serde_json::json!({"schema":"sumaru-surface-qc-1","sumaru_version":env!("CARGO_PKG_VERSION"),
            "parent_surface":self.surface_path,"child_surface":self.parent_load.qc_child,
            "parent_identity":crate::resample::identity_json(&crate::resample::parent_identity(self.mesh.as_ref()?)),
            "verification":format!("{:?}",qc.correspondence.assessment.verification),"warnings":qc.correspondence.assessment.warnings,
            "coordinates":if qc.mapped_positions {"child connectivity at mapped parent positions; display only, no registration"} else {"original child coordinates in parent display transform; no registration"},
            "camera_view_projection":self.camera.view_projection_matrix(self.scene_viewport_aspect()).to_cols_array(),
            "parent_style":format!("{:?}",self.controller.display.surface_render_style),"parent_visible":qc.parent_visible,"child_visible":qc.child_visible,"child_style":format!("{:?}",qc.child_style),"show_through_parent":qc.xray,
            "method":crate::resample::method_name(self.sparse_timecourse_display.method()),"smoothing_steps":self.sparse_smoothing_steps,
            "selected_parent_id":selected.map(|s|s.node_index),"selected_child_id":selected.and_then(|s|qc.correspondence.child_for_parent(s.node_index)),
            "measured_parent_ids":qc.measured_ids,"show_measured_nodes":qc.show_measured,"ordered_child_parent_ids":qc.correspondence.mapping.indices,"export_provenance":qc.export_provenance}),
        )
    }

    pub(super) fn check_qc_capture_paths(&self, path: &Path) -> Result<()> {
        if self.surface_qc.is_some() || self.geometric_qc.is_some() {
            let sidecar = PathBuf::from(format!("{}.qc.json", path.display()));
            ensure!(
                !path.exists() && !sidecar.exists(),
                "QC screenshot and sidecar paths must be new; choose another filename"
            );
        }
        Ok(())
    }

    pub(super) fn save_qc_report(&self, path: &Path) -> Result<()> {
        if let Some(report) = self.qc_report() {
            let path = PathBuf::from(format!("{}.qc.json", path.display()));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .with_context(|| {
                    format!(
                        "QC sidecar already exists or cannot be created: {}",
                        path.display()
                    )
                })?;
            use std::io::Write;
            file.write_all(&serde_json::to_vec_pretty(&report)?)?;
        }
        Ok(())
    }

    pub(super) fn draw_surface_qc_controls(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<ViewerCommand>,
    ) {
        self.draw_geometric_qc_controls(ui);
        if self.surface_qc.is_none() {
            return;
        }
        let mut geometry_changed = false;
        controller_section(ui, "PARENT / CHILD QC", true, |ui| {
            let qc = self.surface_qc.as_mut().unwrap();
            ui.label(format!(
                "Parent: {:?}",
                qc.correspondence.assessment.verification
            ));
            if let Some(parent) = &self.mesh {
                let identity = crate::resample::parent_identity(parent);
                ui.label(format!(
                    "{} · {:?} · {} nodes",
                    identity.subject.as_deref().unwrap_or("unknown subject"),
                    identity.hemisphere,
                    parent.vertices.len()
                ));
            }
            ui.label(format!(
                "Display fill: {} · {} smoothing steps",
                crate::resample::method_name(self.sparse_timecourse_display.method()),
                self.sparse_smoothing_steps
            ));
            ui.checkbox(
                &mut qc.show_measured,
                "Measured nodes (cyan, show through parent)",
            );
            for warning in &qc.correspondence.assessment.warnings {
                ui.colored_label(egui::Color32::YELLOW, warning);
            }
            ui.horizontal(|ui| {
                ui.checkbox(&mut qc.parent_visible, "Parent");
                ui.checkbox(&mut qc.child_visible, "Child");
            });
            ui.horizontal(|ui| {
                ui.label("Child style");
                for (style, label) in [
                    (SurfaceRenderStyle::Filled, "Filled"),
                    (SurfaceRenderStyle::Triangles, "Wire"),
                    (SurfaceRenderStyle::Vertices, "Nodes"),
                ] {
                    ui.selectable_value(&mut qc.child_style, style, label);
                }
            });
            if ui.button("Swap parent / child styles").clicked() {
                std::mem::swap(
                    &mut qc.child_style,
                    &mut self.controller.display.surface_render_style,
                );
            }
            ui.checkbox(
                &mut qc.xray,
                "Show child wire through parent (display only)",
            );
            ui.checkbox(
                &mut qc.pick_child,
                "Pick child vertices → linked parent trace",
            );
            geometry_changed |= ui
                .checkbox(
                    &mut qc.mapped_positions,
                    "Child connectivity at mapped parent positions",
                )
                .changed();
            ui.small(if qc.mapped_positions {"Mapped display: child positions follow parent IDs. This is not geometric registration."} else {"Original-coordinate view: both meshes use the parent's camera transform. No alignment applied."});
            let dataset = self
                .timecourse
                .as_ref()
                .map(|s| &s.source_dataset)
                .or_else(|| self.overlay.data.dataset());
            if let Some(dataset) = dataset {
                let samples = qc
                    .export_provenance
                    .as_ref()
                    .and_then(|p| p["source_rows"].as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(dataset.row_count);
                let valid = self
                    .overlay
                    .data
                    .node_values()
                    .map_or(0, |v| v.values.iter().filter(|v| v.is_finite()).count());
                ui.label(format!(
                    "{samples} sampled vertices; {valid} finite display vertices"
                ));
            }
            if let Some(provenance) = &qc.export_provenance {
                ui.label(format!("Exported method: {}", provenance["method"]));
                ui.label(format!(
                    "Export parent assessment: {}",
                    provenance["parent_verification"]
                ));
                if let Some(warnings) = provenance["warnings"].as_array() {
                    for warning in warnings {
                        if let Some(warning) = warning.as_str() {
                            ui.colored_label(egui::Color32::YELLOW, warning);
                        }
                    }
                }
                if qc.raw_source.is_none() {
                    ui.small("Original source unavailable; traces show exported values only.");
                }
            }
            if let Some(pick) = self.controller.interaction.pick {
                let local = qc.correspondence.child_for_parent(pick.node_index);
                ui.label(format!(
                    "Parent ID {} · child local ID {}",
                    pick.node_index,
                    local.map_or("none".into(), |v| v.to_string())
                ));
                ui.small(format!("Parent XYZ: {:?}", pick.surface_position));
                if let Some(local) = local {
                    ui.small(format!(
                        "Original child XYZ: {:?}",
                        qc.correspondence.original.vertices[local]
                    ));
                }
                let measured = qc
                    .export_provenance
                    .as_ref()
                    .and_then(|p| p["ordered_source_vertex_ids"].as_array())
                    .map(|ids| {
                        ids.iter()
                            .any(|v| v.as_u64() == Some(pick.node_index as u64))
                    })
                    .or_else(|| {
                        dataset.map(|d| dataset_row_for_node(d, pick.node_index).is_some())
                    });
                ui.label(if measured == Some(true) {
                    "Measured vertex (smoothing may alter its display value)"
                } else {
                    "Unsampled vertex: display value is inferred or missing"
                });
            }
            ui.horizontal(|ui| {
                if ui.button("Save QC screenshot").clicked() {
                    actions.push(ViewerCommand::SaveScreenshot);
                }
                if ui.button("Save QC montage").clicked() {
                    actions.push(ViewerCommand::SaveMontage);
                }
            });
        });
        if geometry_changed {
            self.upload_qc_surface();
        }
    }
}
