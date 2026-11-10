// Developed by P. Molfese, National Institutes of Health (NIH).
// See LICENSE for the public-domain notice and disclaimer.
//! QC geometry uses exact IDs only. Changing display coordinates is not registration.
use crate::{
    surface::SurfaceMesh,
    surface_align::{ParentAssessment, ParentMismatchPolicy, ParentVertexMapping},
};
use anyhow::{Context, Result, ensure};

pub struct SurfaceCorrespondence {
    pub original: SurfaceMesh,
    pub mapped: SurfaceMesh,
    pub mapping: ParentVertexMapping,
    pub assessment: ParentAssessment,
}
impl SurfaceCorrespondence {
    pub fn new(
        child: SurfaceMesh,
        parent: &SurfaceMesh,
        policy: ParentMismatchPolicy,
    ) -> Result<Self> {
        ensure!(
            crate::resample::is_original_parent(parent),
            "QC requires a full parent target with its original vertex numbering"
        );
        let mapping =
            child.metadata.parent_mapping.clone().context(
                "QC child has no parent vertex IDs; geometric alignment is not inferred",
            )?;
        mapping.validate(child.vertices.len())?;
        let assessment = mapping.assess_parent(
            &crate::resample::parent_identity(parent),
            parent.vertices.len(),
            policy,
        )?;
        let positions = mapping
            .indices
            .iter()
            .map(|&id| parent.vertices[id as usize])
            .collect();
        let mapped = SurfaceMesh::new(positions, child.triangles.clone())?;
        Ok(Self {
            original: child,
            mapped,
            mapping,
            assessment,
        })
    }
    pub fn child_for_parent(&self, parent: u32) -> Option<usize> {
        self.mapping.indices.iter().position(|&id| id == parent)
    }
}
