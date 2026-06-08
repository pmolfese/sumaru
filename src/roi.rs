use std::collections::BTreeSet;

use anyhow::{Result, ensure};

use crate::color::{LabelEntry, Rgba};
use crate::dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind, DatasetParentIds};
use crate::surface::{SurfaceDomain, SurfaceDomainId, SurfaceId, SurfaceSide};

#[derive(Debug, Clone, PartialEq)]
pub struct Roi {
    pub id: RoiId,
    pub parent_surface_id: Option<SurfaceId>,
    pub parent_domain_id: Option<SurfaceDomainId>,
    pub parent_side: SurfaceSide,
    pub label: String,
    pub integer_label: i32,
    pub fill_color: Rgba,
    pub edge_color: Rgba,
    pub edge_thickness: u32,
    pub color_by_label: bool,
    pub draw_status: RoiDrawStatus,
    pub drawing_type: RoiDrawingType,
    pub source: RoiSource,
    pub source_id: Option<String>,
    pub data: Vec<RoiDatum>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoiId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoiSource {
    Manual,
    Drawn,
    NimlRoi,
    Dataset,
    ThresholdedOverlay,
    Imported,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoiDrawStatus {
    InCreation,
    Finished,
    InEdit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoiDrawingType {
    OpenPath,
    ClosedPath,
    FilledArea,
    Collection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoiElementKind {
    Unknown,
    NodeGroup,
    EdgeGroup,
    FaceGroup,
    NodeSegment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoiBrushAction {
    Unknown,
    AppendStroke,
    AppendStrokeOrFill,
    JoinEnds,
    FillArea,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoiDatum {
    pub kind: RoiElementKind,
    pub action: RoiBrushAction,
    pub node_path: Vec<u32>,
    pub triangle_path: Vec<u32>,
    pub node_distance: Option<f32>,
    pub surface_distance: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiNodeRange {
    pub min: u32,
    pub max: u32,
}

impl Roi {
    pub fn new(label: impl Into<String>, integer_label: i32) -> Result<Self> {
        let label = label.into();
        validate_label(&label, "ROI label")?;

        let mut roi = Self {
            id: RoiId::temporary(),
            parent_surface_id: None,
            parent_domain_id: None,
            parent_side: SurfaceSide::Unknown,
            label,
            integer_label,
            fill_color: Rgba::from_u8(255, 0, 0, 180),
            edge_color: Rgba::from_u8(0, 0, 0, 255),
            edge_thickness: 1,
            color_by_label: false,
            draw_status: RoiDrawStatus::Finished,
            drawing_type: RoiDrawingType::Collection,
            source: RoiSource::Manual,
            source_id: None,
            data: Vec::new(),
        };
        roi.id = RoiId::from_roi_content(&roi);

        Ok(roi)
    }

    pub fn from_nodes(
        label: impl Into<String>,
        integer_label: i32,
        nodes: Vec<u32>,
    ) -> Result<Self> {
        let datum = RoiDatum::node_group(nodes)?;
        let mut roi = Self::new(label, integer_label)?.with_data(vec![datum])?;
        roi.drawing_type = RoiDrawingType::Collection;
        roi.id = RoiId::from_roi_content(&roi);

        Ok(roi)
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Result<Self> {
        self.id = RoiId::new(id)?;
        Ok(self)
    }

    pub fn with_parent_surface(
        mut self,
        surface_id: SurfaceId,
        domain_id: SurfaceDomainId,
        side: SurfaceSide,
    ) -> Self {
        self.parent_surface_id = Some(surface_id);
        self.parent_domain_id = Some(domain_id);
        self.parent_side = side;
        self
    }

    pub fn with_parent_domain(mut self, domain_id: SurfaceDomainId, side: SurfaceSide) -> Self {
        self.parent_domain_id = Some(domain_id);
        self.parent_side = side;
        self
    }

    pub fn with_source(mut self, source: RoiSource, source_id: Option<String>) -> Result<Self> {
        if let Some(source_id) = &source_id {
            validate_label(source_id, "ROI source id")?;
        }
        self.source = source;
        self.source_id = source_id;
        Ok(self)
    }

    pub fn with_style(
        mut self,
        fill_color: Rgba,
        edge_color: Rgba,
        edge_thickness: u32,
    ) -> Result<Self> {
        ensure!(edge_thickness > 0, "ROI edge thickness must be positive");
        self.fill_color = fill_color;
        self.edge_color = edge_color;
        self.edge_thickness = edge_thickness;
        Ok(self)
    }

    pub fn with_color_by_label(mut self, color_by_label: bool) -> Self {
        self.color_by_label = color_by_label;
        self
    }

    pub fn with_draw_status(mut self, draw_status: RoiDrawStatus) -> Self {
        self.draw_status = draw_status;
        self
    }

    pub fn with_drawing_type(mut self, drawing_type: RoiDrawingType) -> Self {
        self.drawing_type = drawing_type;
        self
    }

    pub fn with_data(mut self, data: Vec<RoiDatum>) -> Result<Self> {
        for datum in &data {
            datum.validate()?;
        }
        self.data = data;
        Ok(self)
    }

    pub fn add_datum(&mut self, datum: RoiDatum) -> Result<()> {
        datum.validate()?;
        self.data.push(datum);
        Ok(())
    }

    pub fn label_entry(&self) -> Result<LabelEntry> {
        LabelEntry::new(self.integer_label, self.label.clone(), self.fill_color)
    }

    pub fn nodes(&self) -> Vec<u32> {
        self.data
            .iter()
            .flat_map(|datum| datum.node_path.iter().copied())
            .collect()
    }

    pub fn unique_nodes(&self) -> Vec<u32> {
        self.nodes()
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn contains_node(&self, node: u32) -> bool {
        self.data
            .iter()
            .any(|datum| datum.node_path.contains(&node))
    }

    pub fn node_range(&self) -> Option<RoiNodeRange> {
        self.nodes()
            .into_iter()
            .fold(None, |range: Option<RoiNodeRange>, node| {
                Some(match range {
                    Some(range) => RoiNodeRange {
                        min: range.min.min(node),
                        max: range.max.max(node),
                    },
                    None => RoiNodeRange {
                        min: node,
                        max: node,
                    },
                })
            })
    }

    pub fn validate_for_domain(&self, domain: &SurfaceDomain) -> Result<()> {
        if let Some(parent_domain_id) = &self.parent_domain_id {
            ensure!(
                parent_domain_id == &domain.id,
                "ROI parent domain does not match target surface domain"
            );
        }

        for datum in &self.data {
            datum.validate_for_domain(domain)?;
        }

        Ok(())
    }

    pub fn to_dataset(&self, domain: &SurfaceDomain) -> Result<Dataset> {
        self.validate_for_domain(domain)?;
        let nodes = self.unique_nodes();
        ensure!(
            !nodes.is_empty(),
            "ROI has no node path values to convert into a dataset"
        );
        let values = vec![self.integer_label; nodes.len()];
        let mut dataset = Dataset::sparse(
            DatasetKind::Roi,
            domain,
            nodes,
            vec![
                DataColumn::new(
                    self.label.clone(),
                    ColumnRole::Label,
                    None,
                    ColumnData::Int32(values),
                )
                .expect("ROI label column should be valid"),
            ],
        )?;

        dataset.parent_ids = DatasetParentIds {
            source_dataset_id: Some(self.id.as_str().to_string()),
            domain_parent_id: Some(domain.id.as_str().to_string()),
            surface_parent_id: self
                .parent_surface_id
                .as_ref()
                .map(|id| id.as_str().to_string()),
            volume_parent_id: None,
            originator_id: self.source_id.clone(),
        };

        Ok(dataset)
    }
}

impl RoiDatum {
    pub fn new(
        kind: RoiElementKind,
        action: RoiBrushAction,
        node_path: Vec<u32>,
        triangle_path: Vec<u32>,
    ) -> Result<Self> {
        let datum = Self {
            kind,
            action,
            node_path,
            triangle_path,
            node_distance: None,
            surface_distance: None,
        };
        datum.validate()?;

        Ok(datum)
    }

    pub fn node_group(nodes: Vec<u32>) -> Result<Self> {
        Self::new(
            RoiElementKind::NodeGroup,
            RoiBrushAction::Unknown,
            nodes,
            Vec::new(),
        )
    }

    pub fn node_segment(nodes: Vec<u32>, action: RoiBrushAction) -> Result<Self> {
        Self::new(RoiElementKind::NodeSegment, action, nodes, Vec::new())
    }

    pub fn face_group(triangles: Vec<u32>) -> Result<Self> {
        Self::new(
            RoiElementKind::FaceGroup,
            RoiBrushAction::Unknown,
            Vec::new(),
            triangles,
        )
    }

    pub fn with_triangle_path(mut self, triangle_path: Vec<u32>) -> Result<Self> {
        self.triangle_path = triangle_path;
        self.validate()?;
        Ok(self)
    }

    pub fn with_distances(
        mut self,
        node_distance: Option<f32>,
        surface_distance: Option<f32>,
    ) -> Result<Self> {
        validate_distance(node_distance, "node distance")?;
        validate_distance(surface_distance, "surface distance")?;
        self.node_distance = node_distance;
        self.surface_distance = surface_distance;
        Ok(self)
    }

    pub fn validate(&self) -> Result<()> {
        match self.kind {
            RoiElementKind::NodeGroup | RoiElementKind::EdgeGroup | RoiElementKind::NodeSegment => {
                ensure!(!self.node_path.is_empty(), "ROI datum has no node path");
            }
            RoiElementKind::FaceGroup => {
                ensure!(
                    !self.triangle_path.is_empty(),
                    "ROI face datum has no triangle path"
                );
            }
            RoiElementKind::Unknown => {
                ensure!(
                    !self.node_path.is_empty() || !self.triangle_path.is_empty(),
                    "ROI datum has no node or triangle path"
                );
            }
        }

        validate_distance(self.node_distance, "node distance")?;
        validate_distance(self.surface_distance, "surface distance")?;

        Ok(())
    }

    pub fn validate_for_domain(&self, domain: &SurfaceDomain) -> Result<()> {
        self.validate()?;
        for node in &self.node_path {
            ensure!(
                (*node as usize) < domain.node_count,
                "ROI datum references node {} outside domain node count {}",
                node,
                domain.node_count
            );
        }
        for triangle in &self.triangle_path {
            ensure!(
                (*triangle as usize) < domain.triangles.len(),
                "ROI datum references triangle {} outside domain triangle count {}",
                triangle,
                domain.triangles.len()
            );
        }

        Ok(())
    }
}

impl RoiId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_label(&value, "ROI id")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn temporary() -> Self {
        Self("roi-pending".to_string())
    }

    fn from_roi_content(roi: &Roi) -> Self {
        let mut hash = FNV_OFFSET;
        hash_bytes(&mut hash, b"sumaru.roi.v1");
        hash_option_str(
            &mut hash,
            roi.parent_surface_id.as_ref().map(SurfaceId::as_str),
        );
        hash_option_str(
            &mut hash,
            roi.parent_domain_id.as_ref().map(SurfaceDomainId::as_str),
        );
        hash_u8(&mut hash, surface_side_tag(&roi.parent_side));
        hash_str(&mut hash, &roi.label);
        hash_i32(&mut hash, roi.integer_label);
        hash_u8(&mut hash, roi_draw_status_tag(roi.draw_status));
        hash_u8(&mut hash, roi_drawing_type_tag(roi.drawing_type));
        hash_roi_source(&mut hash, &roi.source);
        hash_option_str(&mut hash, roi.source_id.as_deref());
        hash_color(&mut hash, roi.fill_color);
        hash_color(&mut hash, roi.edge_color);
        hash_u32(&mut hash, roi.edge_thickness);
        hash_bool(&mut hash, roi.color_by_label);
        hash_usize(&mut hash, roi.data.len());

        for datum in &roi.data {
            hash_u8(&mut hash, roi_element_kind_tag(datum.kind));
            hash_u8(&mut hash, roi_brush_action_tag(datum.action));
            hash_u32_slice(&mut hash, &datum.node_path);
            hash_u32_slice(&mut hash, &datum.triangle_path);
            hash_option_f32(&mut hash, datum.node_distance);
            hash_option_f32(&mut hash, datum.surface_distance);
        }

        Self(format!("roi-{hash:016x}"))
    }
}

fn validate_label(value: &str, label: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{label} is empty");
    Ok(())
}

fn validate_distance(value: Option<f32>, label: &str) -> Result<()> {
    if let Some(value) = value {
        ensure!(value.is_finite(), "ROI {label} must be finite");
        ensure!(value >= 0.0, "ROI {label} must be non-negative");
    }
    Ok(())
}

fn surface_side_tag(side: &SurfaceSide) -> u8 {
    match side {
        SurfaceSide::Left => 1,
        SurfaceSide::Right => 2,
        SurfaceSide::Both => 3,
        SurfaceSide::Unknown => 4,
        SurfaceSide::Other(_) => 5,
    }
}

fn roi_draw_status_tag(value: RoiDrawStatus) -> u8 {
    match value {
        RoiDrawStatus::InCreation => 1,
        RoiDrawStatus::Finished => 2,
        RoiDrawStatus::InEdit => 3,
    }
}

fn roi_drawing_type_tag(value: RoiDrawingType) -> u8 {
    match value {
        RoiDrawingType::OpenPath => 1,
        RoiDrawingType::ClosedPath => 2,
        RoiDrawingType::FilledArea => 3,
        RoiDrawingType::Collection => 4,
    }
}

fn roi_element_kind_tag(value: RoiElementKind) -> u8 {
    match value {
        RoiElementKind::Unknown => 0,
        RoiElementKind::NodeGroup => 1,
        RoiElementKind::EdgeGroup => 2,
        RoiElementKind::FaceGroup => 3,
        RoiElementKind::NodeSegment => 4,
    }
}

fn roi_brush_action_tag(value: RoiBrushAction) -> u8 {
    match value {
        RoiBrushAction::Unknown => 0,
        RoiBrushAction::AppendStroke => 1,
        RoiBrushAction::AppendStrokeOrFill => 2,
        RoiBrushAction::JoinEnds => 3,
        RoiBrushAction::FillArea => 4,
    }
}

fn hash_roi_source(hash: &mut u64, source: &RoiSource) {
    match source {
        RoiSource::Manual => hash_u8(hash, 1),
        RoiSource::Drawn => hash_u8(hash, 2),
        RoiSource::NimlRoi => hash_u8(hash, 3),
        RoiSource::Dataset => hash_u8(hash, 4),
        RoiSource::ThresholdedOverlay => hash_u8(hash, 5),
        RoiSource::Imported => hash_u8(hash, 6),
        RoiSource::Other(value) => {
            hash_u8(hash, 7);
            hash_str(hash, value);
        }
    }
}

fn hash_color(hash: &mut u64, color: Rgba) {
    hash_u32(hash, color.red.to_bits());
    hash_u32(hash, color.green.to_bits());
    hash_u32(hash, color.blue.to_bits());
    hash_u32(hash, color.alpha.to_bits());
}

fn hash_option_str(hash: &mut u64, value: Option<&str>) {
    match value {
        Some(value) => {
            hash_bool(hash, true);
            hash_str(hash, value);
        }
        None => hash_bool(hash, false),
    }
}

fn hash_option_f32(hash: &mut u64, value: Option<f32>) {
    match value {
        Some(value) => {
            hash_bool(hash, true);
            hash_u32(hash, value.to_bits());
        }
        None => hash_bool(hash, false),
    }
}

fn hash_u32_slice(hash: &mut u64, values: &[u32]) {
    hash_usize(hash, values.len());
    for value in values {
        hash_u32(hash, *value);
    }
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= *byte as u64;
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn hash_str(hash: &mut u64, value: &str) {
    hash_usize(hash, value.len());
    hash_bytes(hash, value.as_bytes());
}

fn hash_bool(hash: &mut u64, value: bool) {
    hash_u8(hash, u8::from(value));
}

fn hash_u8(hash: &mut u64, value: u8) {
    hash_bytes(hash, &[value]);
}

fn hash_i32(hash: &mut u64, value: i32) {
    hash_bytes(hash, &value.to_ne_bytes());
}

fn hash_u32(hash: &mut u64, value: u32) {
    hash_bytes(hash, &value.to_ne_bytes());
}

fn hash_usize(hash: &mut u64, value: usize) {
    hash_bytes(hash, &value.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::{
        Roi, RoiBrushAction, RoiDatum, RoiDrawStatus, RoiDrawingType, RoiElementKind, RoiSource,
    };
    use crate::color::Rgba;
    use crate::dataset::{ColumnData, DatasetKind};
    use crate::surface::{SurfaceDomain, SurfaceSide};

    #[test]
    fn roi_from_nodes_keeps_unique_sorted_node_view() {
        let roi = Roi::from_nodes("V1", 7, vec![4, 1, 1]).unwrap();

        assert_eq!(roi.label, "V1");
        assert_eq!(roi.integer_label, 7);
        assert_eq!(roi.unique_nodes(), vec![1, 4]);
        assert!(roi.contains_node(4));
        assert_eq!(roi.node_range().unwrap().min, 1);
        assert_eq!(roi.node_range().unwrap().max, 4);
        assert!(roi.id.as_str().starts_with("roi-"));
    }

    #[test]
    fn roi_rejects_empty_labels_and_ids() {
        let label_error = Roi::new(" ", 1).unwrap_err();
        let id_error = Roi::new("V1", 1).unwrap().with_id(" ").unwrap_err();

        assert!(label_error.to_string().contains("ROI label"));
        assert!(id_error.to_string().contains("ROI id"));
    }

    #[test]
    fn roi_tracks_style_status_source_and_label_entry() {
        let roi = Roi::from_nodes("auditory", 12, vec![0, 2])
            .unwrap()
            .with_style(
                Rgba::from_u8(10, 20, 30, 200),
                Rgba::from_u8(255, 255, 255, 255),
                3,
            )
            .unwrap()
            .with_source(RoiSource::NimlRoi, Some("roi-file-id".to_string()))
            .unwrap()
            .with_color_by_label(true)
            .with_draw_status(RoiDrawStatus::InEdit)
            .with_drawing_type(RoiDrawingType::FilledArea);

        assert_eq!(roi.fill_color, Rgba::from_u8(10, 20, 30, 200));
        assert_eq!(roi.edge_thickness, 3);
        assert_eq!(roi.source, RoiSource::NimlRoi);
        assert_eq!(roi.draw_status, RoiDrawStatus::InEdit);
        assert_eq!(roi.drawing_type, RoiDrawingType::FilledArea);
        assert_eq!(roi.label_entry().unwrap().key, 12);
    }

    #[test]
    fn roi_rejects_zero_edge_thickness() {
        let error = Roi::new("V1", 1)
            .unwrap()
            .with_style(Rgba::OPAQUE_BLACK, Rgba::OPAQUE_BLACK, 0)
            .unwrap_err();

        assert!(error.to_string().contains("edge thickness"));
    }

    #[test]
    fn roi_datum_requires_a_path_for_its_kind() {
        let node_error = RoiDatum::new(
            RoiElementKind::NodeGroup,
            RoiBrushAction::Unknown,
            vec![],
            vec![],
        )
        .unwrap_err();
        let face_error = RoiDatum::face_group(vec![]).unwrap_err();

        assert!(node_error.to_string().contains("node path"));
        assert!(face_error.to_string().contains("triangle path"));
    }

    #[test]
    fn roi_datum_tracks_stroke_action_triangle_path_and_distances() {
        let datum = RoiDatum::node_segment(vec![0, 1, 2], RoiBrushAction::AppendStroke)
            .unwrap()
            .with_triangle_path(vec![0])
            .unwrap()
            .with_distances(Some(2.0), Some(2.4))
            .unwrap();

        assert_eq!(datum.kind, RoiElementKind::NodeSegment);
        assert_eq!(datum.action, RoiBrushAction::AppendStroke);
        assert_eq!(datum.triangle_path, vec![0]);
        assert_eq!(datum.node_distance, Some(2.0));
        assert_eq!(datum.surface_distance, Some(2.4));
    }

    #[test]
    fn roi_datum_rejects_invalid_distances() {
        let error = RoiDatum::node_group(vec![0])
            .unwrap()
            .with_distances(Some(-1.0), None)
            .unwrap_err();

        assert!(error.to_string().contains("non-negative"));
    }

    #[test]
    fn roi_validates_paths_against_surface_domain() {
        let domain = SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap();
        let node_error = Roi::from_nodes("bad-node", 1, vec![3])
            .unwrap()
            .validate_for_domain(&domain)
            .unwrap_err();
        let triangle_error = Roi::new("bad-face", 2)
            .unwrap()
            .with_data(vec![RoiDatum::face_group(vec![1]).unwrap()])
            .unwrap()
            .validate_for_domain(&domain)
            .unwrap_err();

        assert!(node_error.to_string().contains("outside domain node count"));
        assert!(
            triangle_error
                .to_string()
                .contains("outside domain triangle count")
        );
    }

    #[test]
    fn roi_validates_parent_domain_match() {
        let first = SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap();
        let second = SurfaceDomain::from_triangles(3, vec![[0, 2, 1]]).unwrap();
        let error = Roi::from_nodes("V1", 1, vec![0])
            .unwrap()
            .with_parent_domain(first.id.clone(), SurfaceSide::Left)
            .validate_for_domain(&second)
            .unwrap_err();

        assert!(error.to_string().contains("parent domain"));
    }

    #[test]
    fn roi_converts_node_paths_to_sparse_label_dataset() {
        let domain = SurfaceDomain::from_triangles(5, vec![[0, 1, 2]]).unwrap();
        let roi = Roi::from_nodes("V1", 7, vec![4, 1, 1])
            .unwrap()
            .with_parent_domain(domain.id.clone(), SurfaceSide::Left);
        let dataset = roi.to_dataset(&domain).unwrap();

        assert_eq!(dataset.kind, DatasetKind::Roi);
        assert!(dataset.is_sparse());
        assert_eq!(dataset.row_count, 2);
        assert_eq!(dataset.node_indices, Some(vec![1, 4]));
        assert_eq!(
            dataset.parent_ids.domain_parent_id.as_deref(),
            Some(domain.id.as_str())
        );
        match &dataset.columns[0].values {
            ColumnData::Int32(values) => assert_eq!(values, &vec![7, 7]),
            values => panic!("unexpected ROI label values: {values:?}"),
        }
    }

    #[test]
    fn roi_without_nodes_cannot_be_converted_to_node_dataset_yet() {
        let domain = SurfaceDomain::from_triangles(3, vec![[0, 1, 2]]).unwrap();
        let roi = Roi::new("face-only", 9)
            .unwrap()
            .with_data(vec![RoiDatum::face_group(vec![0]).unwrap()])
            .unwrap();
        let error = roi.to_dataset(&domain).unwrap_err();

        assert!(error.to_string().contains("no node path"));
    }
}
