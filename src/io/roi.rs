//! Drawn-ROI NIML I/O: reading and writing `.niml.roi` payloads and the
//! small code<->enum mappers for sides, drawing types, element kinds, and
//! brush actions. Builds on the NIML element model in `super::niml`.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NimlRoiDatumRecord {
    pub action_code: i32,
    pub element_type_code: i32,
    pub node_path: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NimlRoiPayload {
    pub self_idcode: Option<String>,
    pub domain_parent_idcode: Option<String>,
    pub parent_side: SurfaceSide,
    pub label: String,
    pub integer_label: i32,
    pub roi_type_code: Option<i32>,
    pub drawing_type: RoiDrawingType,
    pub color_plane_name: Option<String>,
    pub fill_color: Option<Rgba>,
    pub edge_color: Option<Rgba>,
    pub edge_thickness: Option<u32>,
    pub records: Vec<NimlRoiDatumRecord>,
}

pub fn read_niml_roi_str(text: &str) -> Result<Vec<NimlRoiPayload>> {
    parse_niml_str(text)?
        .iter()
        .filter(|element| element.name == "Node_ROI")
        .map(NimlRoiPayload::from_element)
        .collect()
}

pub fn read_niml_roi(path: impl AsRef<Path>) -> Result<Vec<NimlRoiPayload>> {
    read_niml(path)?
        .iter()
        .filter(|element| element.name == "Node_ROI")
        .map(NimlRoiPayload::from_element)
        .collect()
}

pub fn write_niml_roi(path: impl AsRef<Path>, rois: &[Roi]) -> Result<()> {
    let elements = rois
        .iter()
        .map(|roi| NimlRoiPayload::from_roi(roi).to_element())
        .collect::<Vec<_>>();
    write_niml_ascii(path, &elements)
}

impl NimlRoiPayload {
    pub fn from_roi(roi: &Roi) -> Self {
        Self {
            self_idcode: Some(roi.id.as_str().to_string()),
            domain_parent_idcode: roi
                .parent_domain_id
                .as_ref()
                .map(|id| id.as_str().to_string()),
            parent_side: roi.parent_side.clone(),
            label: roi.label.clone(),
            integer_label: roi.integer_label,
            roi_type_code: Some(drawing_type_to_code(roi.drawing_type)),
            drawing_type: roi.drawing_type,
            color_plane_name: Some(roi_plane_name(&roi.parent_side)),
            fill_color: Some(roi.fill_color),
            edge_color: Some(roi.edge_color),
            edge_thickness: Some(roi.edge_thickness),
            records: roi
                .data
                .iter()
                .map(NimlRoiDatumRecord::from_roi_datum)
                .collect(),
        }
    }

    pub fn from_element(element: &NimlElement) -> Result<Self> {
        ensure!(
            element.name == "Node_ROI",
            "expected Node_ROI element, got {}",
            element.name
        );
        let NimlData::RoiDatums(records) = &element.data else {
            bail!("Node_ROI payload is not SUMA_NIML_ROI_DATUM data");
        };

        let label = element
            .attrs
            .get("Label")
            .cloned()
            .unwrap_or_else(|| "ROI".to_string());
        let integer_label = parse_i32_attr(&element.attrs, "iLabel")?.unwrap_or(0);
        let roi_type_code = parse_i32_attr(&element.attrs, "Type")?;
        let fill_color = element
            .attrs
            .get("FillColor")
            .map(|value| parse_rgba(value))
            .transpose()?;
        let edge_color = element
            .attrs
            .get("EdgeColor")
            .map(|value| parse_rgba(value))
            .transpose()?;
        let edge_thickness = parse_u32_attr(&element.attrs, "EdgeThickness")?;

        Ok(Self {
            self_idcode: element
                .attrs
                .get("self_idcode")
                .or_else(|| element.attrs.get("idcode_str"))
                .or_else(|| element.attrs.get("Object_ID"))
                .cloned(),
            domain_parent_idcode: element
                .attrs
                .get("domain_parent_idcode")
                .or_else(|| element.attrs.get("Parent_idcode_str"))
                .or_else(|| element.attrs.get("Parent_ID"))
                .cloned(),
            parent_side: element
                .attrs
                .get("Parent_side")
                .map_or(SurfaceSide::Unknown, |value| side_from_text(value)),
            label,
            integer_label,
            roi_type_code,
            drawing_type: roi_type_code
                .map(drawing_type_from_code)
                .unwrap_or(RoiDrawingType::Collection),
            color_plane_name: element.attrs.get("ColPlaneName").cloned(),
            fill_color,
            edge_color,
            edge_thickness,
            records: records.clone(),
        })
    }

    pub fn to_roi(&self) -> Result<Roi> {
        let mut roi = Roi::new(self.label.clone(), self.integer_label)?
            .with_source(RoiSource::NimlRoi, self.self_idcode.clone())?
            .with_drawing_type(self.drawing_type);

        if let Some(id) = &self.self_idcode {
            roi = roi.with_id(id.clone())?;
        }
        if let (Some(fill), Some(edge), Some(thickness)) =
            (self.fill_color, self.edge_color, self.edge_thickness)
        {
            roi = roi.with_style(fill, edge, thickness)?;
        }
        roi.parent_side = self.parent_side.clone();
        roi = roi.with_data(
            self.records
                .iter()
                .map(NimlRoiDatumRecord::to_roi_datum)
                .collect::<Result<Vec<_>>>()?,
        )?;

        Ok(roi)
    }

    pub fn to_element(&self) -> NimlElement {
        let mut attrs = BTreeMap::new();
        if let Some(value) = &self.self_idcode {
            attrs.insert("self_idcode".to_string(), value.clone());
        }
        if let Some(value) = &self.domain_parent_idcode {
            attrs.insert("domain_parent_idcode".to_string(), value.clone());
        }
        attrs.insert(
            "Parent_side".to_string(),
            side_to_niml(&self.parent_side).to_string(),
        );
        attrs.insert("Label".to_string(), self.label.clone());
        attrs.insert("iLabel".to_string(), self.integer_label.to_string());
        attrs.insert(
            "Type".to_string(),
            self.roi_type_code
                .unwrap_or_else(|| drawing_type_to_code(self.drawing_type))
                .to_string(),
        );
        if let Some(value) = &self.color_plane_name {
            attrs.insert("ColPlaneName".to_string(), value.clone());
        }
        if let Some(value) = self.fill_color {
            attrs.insert("FillColor".to_string(), rgba_to_string(value));
        }
        if let Some(value) = self.edge_color {
            attrs.insert("EdgeColor".to_string(), rgba_to_string(value));
        }
        if let Some(value) = self.edge_thickness {
            attrs.insert("EdgeThickness".to_string(), value.to_string());
        }

        NimlElement {
            name: "Node_ROI".to_string(),
            attrs,
            data: NimlData::RoiDatums(self.records.clone()),
        }
    }
}

impl NimlRoiDatumRecord {
    pub(crate) fn from_roi_datum(datum: &RoiDatum) -> Self {
        let node_path = if datum.kind == RoiElementKind::FaceGroup {
            datum.triangle_path.clone()
        } else {
            datum.node_path.clone()
        };

        Self {
            action_code: brush_action_to_code(datum.action),
            element_type_code: roi_element_kind_to_code(datum.kind),
            node_path,
        }
    }

    pub(crate) fn to_roi_datum(&self) -> Result<RoiDatum> {
        let action = brush_action_from_code(self.action_code);
        match roi_element_kind_from_code(self.element_type_code) {
            RoiElementKind::FaceGroup => RoiDatum::face_group(self.node_path.clone()),
            kind => RoiDatum::new(kind, action, self.node_path.clone(), Vec::new()),
        }
    }
}

pub(crate) fn parse_roi_datum_records(body: &str, rows: usize) -> Result<Vec<NimlRoiDatumRecord>> {
    let values = body
        .split_whitespace()
        .map(|token| {
            token
                .parse::<i32>()
                .with_context(|| format!("invalid SUMA_NIML_ROI_DATUM value {token:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut records = Vec::new();
    let mut pos = 0;

    while pos < values.len() {
        ensure!(
            pos + 3 <= values.len(),
            "malformed SUMA_NIML_ROI_DATUM record header"
        );
        let action_code = values[pos];
        let element_type_code = values[pos + 1];
        let node_count = values[pos + 2];
        ensure!(
            node_count >= 0,
            "SUMA_NIML_ROI_DATUM record has negative node count"
        );
        pos += 3;
        let node_count = node_count as usize;
        ensure!(
            pos + node_count <= values.len(),
            "malformed SUMA_NIML_ROI_DATUM node path"
        );
        let mut node_path = Vec::with_capacity(node_count);
        for value in &values[pos..pos + node_count] {
            ensure!(
                *value >= 0,
                "SUMA_NIML_ROI_DATUM node path contains negative node index"
            );
            node_path.push(*value as u32);
        }
        pos += node_count;
        records.push(NimlRoiDatumRecord {
            action_code,
            element_type_code,
            node_path,
        });
    }

    if rows > 0 {
        ensure!(
            records.len() == rows,
            "SUMA_NIML_ROI_DATUM contains {} records but ni_dimen says {}",
            records.len(),
            rows
        );
    }

    Ok(records)
}

pub(crate) fn parse_rgba(value: &str) -> Result<Rgba> {
    let components = value
        .split_whitespace()
        .map(|piece| {
            piece
                .parse::<f32>()
                .with_context(|| format!("invalid color component {piece:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        components.len() == 3 || components.len() == 4,
        "NIML color must have three or four components"
    );
    let alpha = components.get(3).copied().unwrap_or(1.0);
    Ok(Rgba::clamped(
        components[0],
        components[1],
        components[2],
        alpha,
    ))
}

pub(crate) fn rgba_to_string(value: Rgba) -> String {
    format!(
        "{} {} {} {}",
        format_float(value.red as f64),
        format_float(value.green as f64),
        format_float(value.blue as f64),
        format_float(value.alpha as f64)
    )
}

pub(crate) fn side_from_text(value: &str) -> SurfaceSide {
    match value.trim().to_ascii_lowercase().as_str() {
        "l" | "lh" | "left" => SurfaceSide::Left,
        "r" | "rh" | "right" => SurfaceSide::Right,
        "lr" | "both" | "bilateral" => SurfaceSide::Both,
        "" | "no_side" | "none" | "unknown" => SurfaceSide::Unknown,
        _ => SurfaceSide::Other(value.trim().to_string()),
    }
}

pub(crate) fn side_to_niml(side: &SurfaceSide) -> &str {
    match side {
        SurfaceSide::Left => "L",
        SurfaceSide::Right => "R",
        SurfaceSide::Both => "LR",
        SurfaceSide::Unknown => "no_side",
        SurfaceSide::Other(value) => value,
    }
}

pub(crate) fn drawing_type_from_code(value: i32) -> RoiDrawingType {
    match value {
        0 => RoiDrawingType::OpenPath,
        1 => RoiDrawingType::ClosedPath,
        2 => RoiDrawingType::FilledArea,
        _ => RoiDrawingType::Collection,
    }
}

pub(crate) fn drawing_type_to_code(value: RoiDrawingType) -> i32 {
    match value {
        RoiDrawingType::OpenPath => 0,
        RoiDrawingType::ClosedPath => 1,
        RoiDrawingType::FilledArea => 2,
        RoiDrawingType::Collection => 4,
    }
}

pub(crate) fn roi_element_kind_to_code(value: RoiElementKind) -> i32 {
    match value {
        RoiElementKind::NodeGroup => 1,
        RoiElementKind::EdgeGroup => 2,
        RoiElementKind::FaceGroup => 3,
        RoiElementKind::NodeSegment => 4,
        RoiElementKind::Unknown => 0,
    }
}

pub(crate) fn roi_element_kind_from_code(value: i32) -> RoiElementKind {
    match value {
        1 => RoiElementKind::NodeGroup,
        2 => RoiElementKind::EdgeGroup,
        3 => RoiElementKind::FaceGroup,
        4 => RoiElementKind::NodeSegment,
        _ => RoiElementKind::Unknown,
    }
}

pub(crate) fn brush_action_from_code(value: i32) -> RoiBrushAction {
    match value {
        1 => RoiBrushAction::AppendStroke,
        2 => RoiBrushAction::AppendStrokeOrFill,
        3 => RoiBrushAction::JoinEnds,
        4 => RoiBrushAction::FillArea,
        _ => RoiBrushAction::Unknown,
    }
}

pub(crate) fn brush_action_to_code(value: RoiBrushAction) -> i32 {
    match value {
        RoiBrushAction::AppendStroke => 1,
        RoiBrushAction::AppendStrokeOrFill => 2,
        RoiBrushAction::JoinEnds => 3,
        RoiBrushAction::FillArea => 4,
        RoiBrushAction::Unknown => 0,
    }
}

pub(crate) fn roi_plane_name(side: &SurfaceSide) -> String {
    match side {
        SurfaceSide::Left => "ROI.L.iS_0".to_string(),
        SurfaceSide::Right => "ROI.R.iS_0".to_string(),
        _ => "Sumaru_ROI".to_string(),
    }
}
