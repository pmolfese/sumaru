use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;

use crate::afni::{AfniConnection, AfniNimlSession, AfniPortConfig, AfniRouteAction};
use crate::command::{ControllerState, SurfacePick};
use crate::io::{
    NimlData, NimlElement, NimlNumericMatrix, NimlValueType, parse_niml_bytes, serialize_niml_ascii,
};

const RECORD_PREFIX: &str = "SUMARU_NIML_RECORD_V1";
const RECORD_HEADER: &str = "# sumaru NIML record v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NimlDirection {
    Tx,
    Rx,
    File,
}

impl NimlDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tx => "tx",
            Self::Rx => "rx",
            Self::File => "file",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "tx" => Some(Self::Tx),
            "rx" => Some(Self::Rx),
            "file" => Some(Self::File),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NimlRecorder {
    writer: Arc<Mutex<BufWriter<File>>>,
}

impl NimlRecorder {
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if has_gzip_extension(path) {
            bail!(
                "recording directly to gzip is disabled for live AFNI talk speed; \
                 record to .nimlrec, then gzip the file afterward"
            );
        }
        let mut writer = BufWriter::new(
            File::create(path)
                .with_context(|| format!("failed to create NIML record {}", path.display()))?,
        );
        writeln!(writer, "{RECORD_HEADER}")?;
        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
        })
    }

    pub fn record_elements(
        &self,
        direction: NimlDirection,
        elements: &[NimlElement],
    ) -> Result<()> {
        let payload = serialize_niml_ascii(elements);
        self.record_payload(direction, payload.as_bytes())
    }

    pub fn record_payload(&self, direction: NimlDirection, payload: &[u8]) -> Result<()> {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("NIML recorder lock was poisoned"))?;
        writeln!(
            writer,
            "{RECORD_PREFIX}\t{timestamp_ms}\t{}\t{}",
            direction.as_str(),
            hex_encode(payload)
        )?;
        writer.flush()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NimlDebugRecord {
    pub source_line: Option<usize>,
    pub timestamp_ms: Option<u128>,
    pub direction: NimlDirection,
    pub payload: Vec<u8>,
    pub elements: Vec<NimlElement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NimlReplayReport {
    pub records: usize,
    pub elements: usize,
    pub routed: usize,
    pub ignored: usize,
    pub viewer_commands: usize,
    pub load_dataset: usize,
    pub rgba_overlays: usize,
    pub surface_crosshairs: usize,
    pub roi_updates: usize,
    pub status_events: usize,
}

impl NimlReplayReport {
    pub fn to_text(&self) -> String {
        format!(
            "Replayed {} record(s), {} element(s): {} routed, {} ignored.\n\
             Actions: {} viewer command(s), {} dataset load(s), {} RGBA overlay(s), \
             {} surface crosshair update(s), {} ROI update(s).\n\
             Controller status events: {}.",
            self.records,
            self.elements,
            self.routed,
            self.ignored,
            self.viewer_commands,
            self.load_dataset,
            self.rgba_overlays,
            self.surface_crosshairs,
            self.roi_updates,
            self.status_events
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum NimlSendCommand {
    Raw(PathBuf),
    Crosshair {
        surface_idcode: String,
        domain_parent_idcode: Option<String>,
        node_index: u32,
        xyz: [f32; 3],
    },
    ViewerCommand(String),
}

pub fn read_debug_records(path: impl AsRef<Path>) -> Result<Vec<NimlDebugRecord>> {
    let path = path.as_ref();
    let bytes = read_debug_input_bytes(path)?;
    if looks_like_record_file(&bytes) {
        read_record_bytes(&bytes)
    } else {
        let elements = parse_niml_bytes(&bytes)
            .with_context(|| format!("failed to parse NIML input {}", path.display()))?;
        Ok(vec![NimlDebugRecord {
            source_line: None,
            timestamp_ms: None,
            direction: NimlDirection::File,
            payload: bytes,
            elements,
        }])
    }
}

fn read_debug_input_bytes(path: &Path) -> Result<Vec<u8>> {
    if has_gzip_extension(path) {
        let file = File::open(path)
            .with_context(|| format!("failed to open gzip NIML input {}", path.display()))?;
        let mut decoder = GzDecoder::new(file);
        let mut bytes = Vec::new();
        decoder
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to decompress gzip NIML input {}", path.display()))?;
        Ok(bytes)
    } else {
        fs::read(path).with_context(|| format!("failed to read NIML input {}", path.display()))
    }
}

fn has_gzip_extension(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".gz"))
}

pub fn inspect_debug_path(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    let records = read_debug_records(path)?;
    let mut out = String::new();
    writeln!(out, "NIML debug input: {}", path.display()).unwrap();
    writeln!(
        out,
        "{} record(s), {} element(s)",
        records.len(),
        records
            .iter()
            .map(|record| record.elements.len())
            .sum::<usize>()
    )
    .unwrap();

    for (record_index, record) in records.iter().enumerate() {
        writeln!(
            out,
            "\n[{}] {}{} bytes={} element(s)={}",
            record_index + 1,
            record.direction.as_str(),
            record
                .timestamp_ms
                .map(|timestamp| format!(" @{timestamp}ms"))
                .unwrap_or_default(),
            record.payload.len(),
            record.elements.len()
        )
        .unwrap();
        for element in &record.elements {
            write_element_summary(&mut out, element, 1);
        }
    }

    Ok(out)
}

pub fn replay_debug_path(path: impl AsRef<Path>) -> Result<NimlReplayReport> {
    let records = read_debug_records(path)?;
    replay_records(&records)
}

pub fn replay_records(records: &[NimlDebugRecord]) -> Result<NimlReplayReport> {
    let mut session = AfniNimlSession::new();
    let mut controller = ControllerState::default();
    let mut report = NimlReplayReport {
        records: records.len(),
        ..Default::default()
    };

    for record in records {
        for element in &record.elements {
            report.elements += 1;
            match session.receive_element(&mut controller, element)? {
                Some(outcome) => {
                    report.routed += 1;
                    for action in outcome.actions {
                        match action {
                            AfniRouteAction::ViewerCommand(_) => report.viewer_commands += 1,
                            AfniRouteAction::LoadDataset(_) => report.load_dataset += 1,
                            AfniRouteAction::RgbaOverlay(_) => report.rgba_overlays += 1,
                            AfniRouteAction::SurfaceCrosshair(_) => report.surface_crosshairs += 1,
                            AfniRouteAction::RoiUpdate(_) => report.roi_updates += 1,
                        }
                    }
                }
                None => report.ignored += 1,
            }
        }
    }
    report.status_events = controller.status_entries().len();
    Ok(report)
}

pub fn send_debug_command(
    config: &AfniPortConfig,
    verbose: bool,
    command: NimlSendCommand,
) -> Result<usize> {
    let elements = elements_for_send_command(command)?;
    let count = elements.len();
    let mut connection = AfniConnection::connect(config, verbose, None, || {})?;
    connection.send_elements(&elements)?;
    Ok(count)
}

pub fn elements_for_send_command(command: NimlSendCommand) -> Result<Vec<NimlElement>> {
    match command {
        NimlSendCommand::Raw(path) => {
            let bytes = fs::read(&path)
                .with_context(|| format!("failed to read raw NIML file {}", path.display()))?;
            parse_niml_bytes(&bytes)
                .with_context(|| format!("failed to parse raw NIML file {}", path.display()))
        }
        NimlSendCommand::Crosshair {
            surface_idcode,
            domain_parent_idcode,
            node_index,
            xyz,
        } => Ok(vec![crosshair_xyz_element(
            surface_idcode,
            domain_parent_idcode,
            node_index,
            xyz,
        )?]),
        NimlSendCommand::ViewerCommand(command) => {
            let mut attrs = BTreeMap::new();
            attrs.insert("command".to_string(), command);
            Ok(vec![NimlElement::text("SUMARU_viewer_command", attrs, "")])
        }
    }
}

fn crosshair_xyz_element(
    surface_idcode: String,
    domain_parent_idcode: Option<String>,
    node_index: u32,
    xyz: [f32; 3],
) -> Result<NimlElement> {
    let mut attrs = BTreeMap::new();
    attrs.insert("surface_nodeid".to_string(), node_index.to_string());
    attrs.insert("surface_idcode".to_string(), surface_idcode);
    if let Some(parent) = domain_parent_idcode {
        attrs.insert("domain_parent_idcode".to_string(), parent);
    }
    NimlNumericMatrix::from_rows(
        vec![NimlValueType::Float32],
        vec![
            vec![xyz[0] as f64],
            vec![xyz[1] as f64],
            vec![xyz[2] as f64],
        ],
    )
    .map(|matrix| NimlElement::numeric("SUMA_crosshair_xyz", attrs, matrix))
}

fn looks_like_record_file(bytes: &[u8]) -> bool {
    let prefix = RECORD_PREFIX.as_bytes();
    let header = RECORD_HEADER.as_bytes();
    bytes.starts_with(prefix)
        || bytes.starts_with(header)
        || bytes
            .split(|byte| *byte == b'\n')
            .find(|line| !line.trim_ascii().is_empty() && !line.starts_with(b"#"))
            .is_some_and(|line| line.starts_with(prefix))
}

fn read_record_bytes(bytes: &[u8]) -> Result<Vec<NimlDebugRecord>> {
    let text = std::str::from_utf8(bytes).context("NIML record file is not UTF-8")?;
    let mut records = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts = line.split('\t').collect::<Vec<_>>();
        ensure!(
            parts.len() == 4 && parts[0] == RECORD_PREFIX,
            "malformed NIML debug record at line {line_number}"
        );
        let timestamp_ms = parts[1]
            .parse::<u128>()
            .with_context(|| format!("invalid timestamp at line {line_number}"))?;
        let direction = NimlDirection::parse(parts[2])
            .ok_or_else(|| anyhow::anyhow!("invalid direction at line {line_number}"))?;
        let payload = hex_decode(parts[3])
            .with_context(|| format!("invalid payload hex at line {line_number}"))?;
        let elements = parse_recorded_payload(&payload)
            .with_context(|| format!("failed to parse recorded NIML at line {line_number}"))?;
        records.push(NimlDebugRecord {
            source_line: Some(line_number),
            timestamp_ms: Some(timestamp_ms),
            direction,
            payload,
            elements,
        });
    }
    Ok(records)
}

fn parse_recorded_payload(payload: &[u8]) -> Result<Vec<NimlElement>> {
    match parse_niml_bytes(payload) {
        Ok(elements) => Ok(elements),
        Err(error) => parse_legacy_reserialized_binary_payload(payload).with_context(|| {
            format!(
                "primary parse failed ({error:#}); also failed legacy reserialized-binary fallback"
            )
        }),
    }
}

fn parse_legacy_reserialized_binary_payload(payload: &[u8]) -> Result<Vec<NimlElement>> {
    let text = std::str::from_utf8(payload)
        .context("legacy reserialized binary fallback requires UTF-8 payload")?;
    ensure!(
        text.contains("ni_form=\"binary.") || text.contains("ni_form='binary."),
        "payload is not a legacy reserialized binary NIML element"
    );
    let normalized = text
        .replace("ni_form=\"binary.lsbfirst\"", "ni_form=\"ascii\"")
        .replace("ni_form=\"binary.msbfirst\"", "ni_form=\"ascii\"")
        .replace("ni_form='binary.lsbfirst'", "ni_form='ascii'")
        .replace("ni_form='binary.msbfirst'", "ni_form='ascii'");
    parse_niml_bytes(normalized.as_bytes())
}

fn write_element_summary(out: &mut String, element: &NimlElement, depth: usize) {
    let indent = "  ".repeat(depth);
    writeln!(
        out,
        "{indent}- {} {} attrs={}",
        element.name,
        data_summary(&element.data),
        attr_summary(&element.attrs)
    )
    .unwrap();
    if let NimlData::Group(children) = &element.data {
        for child in children {
            write_element_summary(out, child, depth + 1);
        }
    }
}

fn attr_summary(attrs: &BTreeMap<String, String>) -> String {
    if attrs.is_empty() {
        return "{}".to_string();
    }
    let pairs = attrs
        .iter()
        .map(|(key, value)| format!("{key}={}", truncate(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{pairs}}}")
}

fn data_summary(data: &NimlData) -> String {
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
        NimlData::Group(children) => format!("group children={}", children.len()),
    }
}

fn truncate(value: &str) -> String {
    const LIMIT: usize = 120;
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(LIMIT).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(value: &str) -> Result<Vec<u8>> {
    ensure!(value.len().is_multiple_of(2), "hex payload has odd length");
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let chars = value.as_bytes();
    for chunk in chars.chunks_exact(2) {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_value(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => bail!("invalid hex digit {}", value as char),
    }
}

#[allow(dead_code)]
fn _surface_pick_example(node_index: u32, xyz: [f32; 3]) -> SurfacePick {
    SurfacePick {
        node_index,
        face_index: 0,
        surface_position: xyz,
        normalized_position: [0.0, 0.0, 0.0],
        overlay_value: None,
        threshold_value: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::parse_niml_str;

    #[test]
    fn record_roundtrip_preserves_direction_and_payload() {
        let element = NimlElement::text("SUMARU_viewer_command", BTreeMap::new(), "");
        let payload = serialize_niml_ascii(std::slice::from_ref(&element));
        let line = format!(
            "{RECORD_PREFIX}\t123\trx\t{}",
            super::hex_encode(payload.as_bytes())
        );

        let records = super::read_record_bytes(line.as_bytes()).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].timestamp_ms, Some(123));
        assert_eq!(records[0].direction, NimlDirection::Rx);
        assert_eq!(records[0].elements[0].name, element.name);
    }

    #[test]
    fn raw_niml_inspection_input_is_wrapped_as_file_record() {
        let text = r#"<SUMARU_viewer_command command="reset_camera"></SUMARU_viewer_command>"#;
        let path = std::env::temp_dir().join("sumaru_raw_niml_inspect_test.niml");
        fs::write(&path, text).unwrap();

        let records = read_debug_records(&path).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].direction, NimlDirection::File);
        assert_eq!(records[0].elements[0].name, "SUMARU_viewer_command");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn gzip_record_input_reads_compressed_nimlrec() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let path = std::env::temp_dir().join(format!(
            "sumaru_gzip_record_input_{}.nimlrec.gz",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let element = NimlElement::text("SUMARU_viewer_command", BTreeMap::new(), "");
        let payload = serialize_niml_ascii(std::slice::from_ref(&element));
        let text = format!(
            "{RECORD_HEADER}\n{RECORD_PREFIX}\t123\trx\t{}\n",
            super::hex_encode(payload.as_bytes())
        );
        let file = File::create(&path).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap();

        let records = read_debug_records(&path).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].direction, NimlDirection::Rx);
        assert_eq!(records[0].elements[0].name, "SUMARU_viewer_command");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recorder_rejects_gzip_output_for_live_speed() {
        let path = std::env::temp_dir().join("sumaru_recording_should_not_write_gzip.nimlrec.gz");

        let error = NimlRecorder::create(&path).unwrap_err();

        assert!(error.to_string().contains("recording directly to gzip"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn replay_counts_routed_and_ignored_elements() {
        let routed = parse_niml_str(
            r#"<SUMARU_viewer_command command="reset_camera"></SUMARU_viewer_command>"#,
        )
        .unwrap()
        .remove(0);
        let ignored = NimlElement::text("UNKNOWN", BTreeMap::new(), "");
        let records = vec![NimlDebugRecord {
            source_line: None,
            timestamp_ms: None,
            direction: NimlDirection::File,
            payload: serialize_niml_ascii(&[routed.clone(), ignored.clone()]).into_bytes(),
            elements: vec![routed, ignored],
        }];

        let report = replay_records(&records).unwrap();

        assert_eq!(report.elements, 2);
        assert_eq!(report.routed, 1);
        assert_eq!(report.ignored, 1);
        assert_eq!(report.viewer_commands, 1);
    }

    #[test]
    fn send_crosshair_builds_suma_xyz_element() {
        let elements = elements_for_send_command(NimlSendCommand::Crosshair {
            surface_idcode: "surf".to_string(),
            domain_parent_idcode: Some("domain".to_string()),
            node_index: 42,
            xyz: [1.0, 2.0, 3.0],
        })
        .unwrap();

        assert_eq!(elements[0].name, "SUMA_crosshair_xyz");
        assert_eq!(
            elements[0].attrs.get("surface_nodeid"),
            Some(&"42".to_string())
        );
        assert_eq!(
            elements[0].attrs.get("domain_parent_idcode"),
            Some(&"domain".to_string())
        );
    }

    #[test]
    fn legacy_reserialized_binary_records_parse_as_ascii() {
        let payload = br#"<SUMA_irgba
  ni_form="binary.lsbfirst"
  ni_type="int,byte,byte,byte,byte"
  ni_dimen="2"
  surface_idcode="surf">
0 255 0 0 255
1 0 255 0 128
</SUMA_irgba>"#;

        let elements = parse_recorded_payload(payload).unwrap();

        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].name, "SUMA_irgba");
        let NimlData::Numeric(matrix) = &elements[0].data else {
            panic!("expected numeric recovered payload");
        };
        assert_eq!(matrix.rows, 2);
        assert_eq!(matrix.column_count(), 5);
    }
}
