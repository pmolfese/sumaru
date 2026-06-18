use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use gifti_rs::{ArrayData, DataArray, GiftiImage, Meta};

use crate::color::{LabelEntry, LabelTable, LabelTableSource, Rgba};
use crate::dataset::{
    AfniFdrCurve, ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind, DatasetParentIds,
};
use crate::roi::{Roi, RoiBrushAction, RoiDatum, RoiDrawingType, RoiElementKind, RoiSource};
use crate::surface::{SurfaceDomain, SurfaceSide};

mod gifti;
mod niml;
mod roi;

// The public face of `io`: read/write entry points and the NIML data model,
// re-exported from the topical submodules so callers keep using `crate::io::*`.
pub use gifti::*;
pub use niml::*;
pub use roi::*;

const MINIMAL_DSET_SAMPLE: &str = r#"
<AFNI_dataset
  ni_form="ni_group"
  dset_type="Node_Bucket"
  self_idcode="XYZ_TEST"
  filename="toy.niml.dset"
  label="toy.niml.dset" >
<SPARSE_DATA
  ni_type="float,float"
  ni_dimen="2"
  data_type="Node_Bucket_data" >
1.5 2.5
3.5 4.5
</SPARSE_DATA>
<INDEX_LIST
  ni_type="int"
  ni_dimen="2"
  data_type="Node_Bucket_node_indices" >
10
12
</INDEX_LIST>
<AFNI_atr
  ni_type="String"
  ni_dimen="1"
  atr_name="COLMS_LABS" >
effect;stat;
</AFNI_atr>
</AFNI_dataset>
"#;

const MINIMAL_ROI_SAMPLE: &str = r#"
# <Node_ROI
#  ni_type = "SUMA_NIML_ROI_DATUM"
#  ni_dimen = "2"
#  self_idcode = "XYZ_ROI"
#  domain_parent_idcode = "DOMAIN"
#  Parent_side = "L"
#  Label = "V1"
#  iLabel = "7"
#  Type = "4"
#  ColPlaneName = "ROI.L.iS_0"
#  FillColor = "0.5 0.1 0.9 1.0"
#  EdgeColor = "0.0 0.0 1.0 1.0"
#  EdgeThickness = "2"
# >
 1 4 3 1 2 3
 4 1 2 8 9
# </Node_ROI>
"#;

#[cfg(test)]
mod tests {
    use super::{
        NimlData, NimlDatasetPayload, NimlElement, NimlMixedTable, NimlNumericMatrix, NimlValue,
        NimlValueType, expand_niml_type, niml_reference_checks, parse_niml_bytes, parse_niml_str,
        read_niml_dset_str, read_niml_roi_str, serialize_niml_ascii,
    };
    use crate::color::Rgba;
    use crate::dataset::{ColumnData, ColumnRole, DatasetKind};
    use crate::roi::{Roi, RoiBrushAction, RoiDatum, RoiDrawingType, RoiElementKind, RoiSource};
    use crate::surface::{SurfaceDomain, SurfaceSide};
    use std::collections::BTreeMap;

    #[test]
    fn niml_type_expansion_matches_reference_repeat_syntax() {
        let types = expand_niml_type("int,3*float,String,SUMA_NIML_ROI_DATUM").unwrap();

        assert_eq!(
            types,
            vec![
                NimlValueType::Int32,
                NimlValueType::Float32,
                NimlValueType::Float32,
                NimlValueType::Float32,
                NimlValueType::String,
                NimlValueType::SumaRoiDatum,
            ]
        );
    }

    #[test]
    fn ascii_niml_parser_reads_dataset_group_contract() {
        let elements = parse_niml_str(super::MINIMAL_DSET_SAMPLE).unwrap();

        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].name, "AFNI_dataset");
        let NimlData::Group(children) = &elements[0].data else {
            panic!("expected group");
        };
        assert_eq!(children[0].name, "SPARSE_DATA");
        let NimlData::Numeric(matrix) = &children[0].data else {
            panic!("expected numeric sparse data");
        };
        assert_eq!(matrix.rows, 2);
        assert_eq!(matrix.column_count(), 2);
        assert_eq!(matrix.get(1, 1), Some(4.5));
    }

    #[test]
    fn dataset_payload_roundtrips_sparse_data_and_attributes() {
        let payload = read_niml_dset_str(super::MINIMAL_DSET_SAMPLE).unwrap();

        assert_eq!(payload.dset_type, "Node_Bucket");
        assert_eq!(payload.node_indices, Some(vec![10, 12]));
        assert_eq!(payload.column_labels, vec!["effect", "stat"]);

        let element = payload.to_element().unwrap();
        let serialized = serialize_niml_ascii(&[element]);
        let reparsed = read_niml_dset_str(&serialized).unwrap();

        assert_eq!(reparsed.node_indices, Some(vec![10, 12]));
        assert_eq!(
            reparsed.sparse_data.unwrap().values,
            vec![1.5, 2.5, 3.5, 4.5]
        );
    }

    #[test]
    fn binary_little_endian_numeric_payload_reads_fixed_width_values() {
        let mut bytes =
            b"<SPARSE_DATA ni_type=\"float,int\" ni_dimen=\"2\" ni_form=\"binary.lsbfirst\" >"
                .to_vec();
        bytes.extend_from_slice(&1.5_f32.to_le_bytes());
        bytes.extend_from_slice(&10_i32.to_le_bytes());
        bytes.extend_from_slice(&2.5_f32.to_le_bytes());
        bytes.extend_from_slice(&12_i32.to_le_bytes());
        bytes.extend_from_slice(b"</SPARSE_DATA>");

        let elements = parse_niml_bytes(&bytes).unwrap();
        let NimlData::Numeric(matrix) = &elements[0].data else {
            panic!("expected binary numeric matrix");
        };

        assert_eq!(matrix.rows, 2);
        assert_eq!(
            matrix.column_types,
            vec![NimlValueType::Float32, NimlValueType::Int32]
        );
        assert_eq!(matrix.get(0, 0), Some(1.5));
        assert_eq!(matrix.get(1, 1), Some(12.0));
    }

    #[test]
    fn bare_niml_attributes_are_preserved_as_empty_values() {
        let text = r#"<AFNI_dataset
            dset_type="Node_Bucket"
            domain_parent_idcode
            geometry_parent_idcode
            ni_form="ni_group">
            </AFNI_dataset>"#;

        let elements = parse_niml_str(text).unwrap();

        assert_eq!(
            elements[0].attrs.get("domain_parent_idcode"),
            Some(&String::new())
        );
        assert_eq!(
            elements[0].attrs.get("geometry_parent_idcode"),
            Some(&String::new())
        );
        assert_eq!(
            elements[0].attrs.get("dset_type").map(String::as_str),
            Some("Node_Bucket")
        );
    }

    #[test]
    fn binary_big_endian_double_payload_reads_values() {
        let mut bytes =
            b"<SPARSE_DATA ni_type=\"double\" ni_dimen=\"2\" ni_form=\"binary.msbfirst\" >"
                .to_vec();
        bytes.extend_from_slice(&1.25_f64.to_be_bytes());
        bytes.extend_from_slice(&2.75_f64.to_be_bytes());
        bytes.extend_from_slice(b"</SPARSE_DATA>");

        let elements = parse_niml_bytes(&bytes).unwrap();
        let NimlData::Numeric(matrix) = &elements[0].data else {
            panic!("expected binary numeric matrix");
        };

        assert_eq!(matrix.column_types, vec![NimlValueType::Float64]);
        assert_eq!(matrix.values, vec![1.25, 2.75]);
    }

    #[test]
    fn mixed_ascii_rows_preserve_string_and_numeric_columns() {
        let elements = parse_niml_str(
            r#"<MIXED ni_type="int,String,float" ni_dimen="2" >
1 "alpha beta" 2.5
2 gamma 3.5
</MIXED>"#,
        )
        .unwrap();
        let NimlData::Mixed(table) = &elements[0].data else {
            panic!("expected mixed table");
        };

        assert_eq!(table.rows, 2);
        assert_eq!(table.get(0, 0), Some(&NimlValue::Integer(1)));
        assert_eq!(
            table.get(0, 1),
            Some(&NimlValue::Text("alpha beta".to_string()))
        );
        assert_eq!(table.get(1, 2), Some(&NimlValue::Float(3.5)));

        let serialized = serialize_niml_ascii(&elements);
        let reparsed = parse_niml_str(&serialized).unwrap();
        assert_eq!(reparsed, elements);
    }

    #[test]
    fn canonical_dataset_conversion_builds_sparse_dataset() {
        let domain = SurfaceDomain::from_triangles(20, vec![[0, 1, 2]]).unwrap();
        let payload = read_niml_dset_str(
            r#"
<AFNI_dataset ni_form="ni_group" dset_type="Node_Bucket" self_idcode="XYZ_DATA" filename="toy.niml.dset" >
<SPARSE_DATA ni_type="float,float" ni_dimen="2" data_type="Node_Bucket_data" >
1.5 2.5
3.5 4.5
</SPARSE_DATA>
<INDEX_LIST ni_type="int" ni_dimen="2" data_type="Node_Bucket_node_indices" >
10
12
</INDEX_LIST>
<AFNI_atr ni_type="String" ni_dimen="1" atr_name="COLMS_LABS" >effect;Tstat;</AFNI_atr>
<AFNI_atr ni_type="String" ni_dimen="1" atr_name="COLMS_TYPE" >Generic_Float;Generic_Float;</AFNI_atr>
<AFNI_atr ni_type="String" ni_dimen="1" atr_name="COLMS_STATSYM" >none;Ttest(10);</AFNI_atr>
<AFNI_atr ni_type="float" ni_dimen="5" atr_name="FDRCURVE_000001" >
0 1 2 1 0.5
</AFNI_atr>
</AFNI_dataset>
"#,
        )
        .unwrap();

        let dataset = payload.to_dataset(&domain).unwrap();

        assert_eq!(dataset.kind, DatasetKind::SurfaceScalar);
        assert_eq!(dataset.node_indices, Some(vec![10, 12]));
        assert_eq!(
            dataset.parent_ids.source_dataset_id.as_deref(),
            Some("XYZ_DATA")
        );
        assert_eq!(dataset.columns[0].label, "effect");
        assert_eq!(dataset.columns[0].role, ColumnRole::Intensity);
        assert_eq!(dataset.columns[1].role, ColumnRole::Statistic);
        assert_eq!(dataset.columns[1].stat.as_deref(), Some("Ttest(10)"));
        assert!(payload.fdr_curves.contains_key(&1));
        assert!(dataset.columns[0].fdr_curve.is_none());
        assert_eq!(
            dataset.columns[1].fdr_curve.as_ref().unwrap().z_value(1.0),
            Some(1.0)
        );
        assert!(
            dataset.columns[1]
                .fdr_curve
                .as_ref()
                .unwrap()
                .q_value(1.0)
                .unwrap()
                < 0.32
        );
        match &dataset.columns[0].values {
            ColumnData::Float32(values) => assert_eq!(values, &vec![1.5, 3.5]),
            values => panic!("unexpected column data: {values:?}"),
        }
    }

    #[test]
    fn canonical_dataset_conversion_builds_dense_dataset_without_index_list() {
        let domain = SurfaceDomain::from_triangles(2, vec![[0, 1, 0]]).unwrap();
        let payload = NimlDatasetPayload {
            dset_type: "Node_Bucket".to_string(),
            self_idcode: Some("XYZ_DENSE".to_string()),
            filename: None,
            label: None,
            sparse_data: Some(
                NimlNumericMatrix::from_rows(
                    vec![NimlValueType::Int32],
                    vec![vec![1.0], vec![2.0]],
                )
                .unwrap(),
            ),
            node_indices: None,
            column_ranges: Vec::new(),
            column_labels: vec!["roi".to_string()],
            column_types: vec!["ROI_Label".to_string()],
            column_stats: vec!["none".to_string()],
            fdr_curves: BTreeMap::new(),
            history: None,
        };

        let dataset = payload.to_dataset(&domain).unwrap();

        assert_eq!(dataset.kind, DatasetKind::SurfaceLabel);
        assert!(!dataset.is_sparse());
        assert_eq!(dataset.columns[0].role, ColumnRole::Label);
        match &dataset.columns[0].values {
            ColumnData::Int32(values) => assert_eq!(values, &vec![1, 2]),
            values => panic!("unexpected column data: {values:?}"),
        }
    }

    #[test]
    fn embedded_afni_labeltable_maps_integer_keys_to_names() {
        let domain = SurfaceDomain::from_triangles(2, vec![[0, 1, 0]]).unwrap();
        let elements = parse_niml_str(
            r#"
<AFNI_dataset ni_form="ni_group" dset_type="Node_Label" >
<SPARSE_DATA ni_type="int" ni_dimen="2" data_type="Node_Label_data" >
42
0
</SPARSE_DATA>
<AFNI_labeltable ni_form="ni_group" dset_type="LabelTableObject" Name="FreeSurferColorLUT-test" >
<SPARSE_DATA ni_type="4*float,int,String" ni_dimen="2" data_type="LabelTableObject_data" >
0.1 0.2 0.3 1 42 "ctx-lh-test-region"
0 0 0 1 0 "Unknown"
</SPARSE_DATA>
</AFNI_labeltable>
<AFNI_atr ni_type="String" ni_dimen="1" atr_name="COLMS_LABS" >node label;</AFNI_atr>
<AFNI_atr ni_type="String" ni_dimen="1" atr_name="COLMS_TYPE" >Node_Index_Label;</AFNI_atr>
</AFNI_dataset>
"#,
        )
        .unwrap();
        let payload = NimlDatasetPayload::from_element(&elements[0]).unwrap();
        let dataset = payload.to_dataset(&domain).unwrap();
        let label_table = super::label_table_from_niml_dataset_element(&elements[0])
            .unwrap()
            .unwrap();

        assert_eq!(dataset.kind, DatasetKind::SurfaceLabel);
        assert_eq!(dataset.columns[0].role, ColumnRole::Label);
        assert_eq!(
            label_table.label(42).map(|entry| entry.label.as_str()),
            Some("ctx-lh-test-region")
        );
        assert_eq!(
            label_table.source,
            crate::color::LabelTableSource::FreeSurfer
        );
    }

    #[test]
    fn embedded_afni_labeltable_tolerates_duplicate_keys() {
        let elements = parse_niml_str(
            r#"
<AFNI_dataset ni_form="ni_group" dset_type="Node_Label" >
<AFNI_labeltable ni_form="ni_group" dset_type="LabelTableObject" Name="FreeSurferColorLUT-test" >
<SPARSE_DATA ni_type="4*float,int,String" ni_dimen="2" data_type="LabelTableObject_data" >
0.1 0.2 0.3 1 42 "old-name"
0.4 0.5 0.6 1 42 "new-name"
</SPARSE_DATA>
</AFNI_labeltable>
</AFNI_dataset>
"#,
        )
        .unwrap();

        let label_table = super::label_table_from_niml_dataset_element(&elements[0])
            .unwrap()
            .unwrap();

        assert_eq!(label_table.labels.len(), 1);
        assert_eq!(
            label_table.label(42).map(|entry| entry.label.as_str()),
            Some("new-name")
        );
    }

    #[test]
    fn mixed_table_constructor_validates_value_count() {
        let error = NimlMixedTable::new(
            vec![NimlValueType::Int32, NimlValueType::String],
            2,
            vec![NimlValue::Integer(1)],
        )
        .unwrap_err();

        assert!(error.to_string().contains("expected 4"));
    }

    #[test]
    fn commented_node_roi_reads_suma_datum_records() {
        let rois = read_niml_roi_str(super::MINIMAL_ROI_SAMPLE).unwrap();

        assert_eq!(rois.len(), 1);
        assert_eq!(rois[0].self_idcode.as_deref(), Some("XYZ_ROI"));
        assert_eq!(rois[0].parent_side, SurfaceSide::Left);
        assert_eq!(rois[0].label, "V1");
        assert_eq!(rois[0].integer_label, 7);
        assert_eq!(
            rois[0].fill_color,
            Some(Rgba::new_unchecked(0.5, 0.1, 0.9, 1.0))
        );
        assert_eq!(rois[0].records[0].action_code, 1);
        assert_eq!(rois[0].records[0].element_type_code, 4);
        assert_eq!(rois[0].records[0].node_path, vec![1, 2, 3]);
    }

    #[test]
    fn roi_payload_converts_to_roi_model() {
        let payload = read_niml_roi_str(super::MINIMAL_ROI_SAMPLE)
            .unwrap()
            .remove(0);
        let roi = payload.to_roi().unwrap();

        assert_eq!(roi.id.as_str(), "XYZ_ROI");
        assert_eq!(roi.label, "V1");
        assert_eq!(roi.parent_side, SurfaceSide::Left);
        assert_eq!(roi.drawing_type, RoiDrawingType::Collection);
        assert_eq!(roi.data[0].kind, RoiElementKind::NodeSegment);
        assert_eq!(roi.data[0].action, RoiBrushAction::AppendStroke);
        assert_eq!(roi.unique_nodes(), vec![1, 2, 3, 8, 9]);
    }

    #[test]
    fn roi_payload_roundtrips_through_ascii_niml_element() {
        let payload = read_niml_roi_str(super::MINIMAL_ROI_SAMPLE)
            .unwrap()
            .remove(0);
        let serialized = serialize_niml_ascii(&[payload.to_element()]);
        let reparsed = read_niml_roi_str(&serialized).unwrap();

        assert_eq!(reparsed[0].label, "V1");
        assert_eq!(reparsed[0].records, payload.records);
    }

    #[test]
    fn drawn_roi_exports_suma_edge_join_and_fill_records() {
        let roi = Roi::new("drawn", 3)
            .unwrap()
            .with_source(RoiSource::Drawn, None)
            .unwrap()
            .with_drawing_type(RoiDrawingType::FilledArea)
            .with_data(vec![
                RoiDatum::node_segment(vec![1, 2, 3], RoiBrushAction::AppendStroke).unwrap(),
                RoiDatum::node_segment(vec![3, 4, 1], RoiBrushAction::JoinEnds).unwrap(),
                RoiDatum::new(
                    RoiElementKind::NodeGroup,
                    RoiBrushAction::FillArea,
                    vec![1, 2, 3, 4, 5],
                    Vec::new(),
                )
                .unwrap(),
            ])
            .unwrap();
        let payload = super::NimlRoiPayload::from_roi(&roi);

        assert_eq!(payload.records.len(), 3);
        assert_eq!(
            payload
                .records
                .iter()
                .map(|record| (record.action_code, record.element_type_code))
                .collect::<Vec<_>>(),
            vec![(1, 4), (3, 4), (4, 1)]
        );

        let serialized = serialize_niml_ascii(&[payload.to_element()]);
        let reparsed = read_niml_roi_str(&serialized).unwrap();
        assert_eq!(reparsed[0].label, "drawn");
        assert_eq!(reparsed[0].records[1].action_code, 3);
        assert_eq!(reparsed[0].records[2].node_path, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn reference_contract_checks_cover_c_matlab_python_alignment() {
        let checks = niml_reference_checks();

        assert_eq!(checks.len(), 3);
        assert!(checks.iter().all(|check| check.passed), "{checks:?}");
        assert!(checks.iter().all(|check| !check.references.is_empty()));
    }

    #[test]
    fn numeric_matrix_rejects_wrong_value_count() {
        let error = NimlNumericMatrix::new(vec![NimlValueType::Float32], 2, vec![1.0]).unwrap_err();

        assert!(error.to_string().contains("expected 2"));
    }

    #[test]
    fn dataset_payload_requires_sparse_data_before_writing() {
        let payload = NimlDatasetPayload {
            dset_type: "Node_Bucket".to_string(),
            self_idcode: None,
            filename: None,
            label: None,
            sparse_data: None,
            node_indices: None,
            column_ranges: Vec::new(),
            column_labels: Vec::new(),
            column_types: Vec::new(),
            column_stats: Vec::new(),
            fdr_curves: BTreeMap::new(),
            history: None,
        };

        assert!(
            payload
                .to_element()
                .unwrap_err()
                .to_string()
                .contains("SPARSE_DATA")
        );
    }

    #[test]
    fn parser_preserves_unknown_text_elements() {
        let elements = parse_niml_str(
            r#"<AFNI_atr ni_type="String" ni_dimen="1" atr_name="NOTE" >hello &lt;sumaru&gt;</AFNI_atr>"#,
        )
        .unwrap();

        let NimlData::Text(text) = &elements[0].data else {
            panic!("expected text");
        };
        assert_eq!(text, "hello <sumaru>");
    }

    #[test]
    fn explicit_element_constructors_make_serializable_groups() {
        let matrix =
            NimlNumericMatrix::from_rows(vec![NimlValueType::Int32], vec![vec![1.0]]).unwrap();
        let child = NimlElement::numeric("INDEX_LIST", Default::default(), matrix);
        let root = NimlElement::group("AFNI_dataset", Default::default(), vec![child]);
        let text = serialize_niml_ascii(&[root]);

        assert!(text.contains("ni_form=\"ni_group\""));
        assert!(text.contains("ni_type=\"int\""));
    }
}
