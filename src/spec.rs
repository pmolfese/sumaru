use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};

use crate::surface::SurfaceSide;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecFile {
    pub path: PathBuf,
    pub group: Option<String>,
    pub states: Vec<String>,
    pub hemisphere: SpecHemisphere,
    pub surfaces: Vec<SpecSurface>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecSurface {
    pub name: String,
    pub path: PathBuf,
    pub surface_name: String,
    pub surface_format: Option<String>,
    pub surface_type: Option<String>,
    pub state: Option<String>,
    pub raw_state: Option<String>,
    pub anatomical: Option<bool>,
    pub side: SurfaceSide,
    pub local_domain_parent: Option<String>,
    pub local_curvature_parent: Option<String>,
    pub label_dataset: Option<PathBuf>,
    pub embed_dimension: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecHemisphere {
    Left,
    Right,
    Both,
    Unknown,
}

pub fn read_spec(path: impl AsRef<Path>) -> Result<SpecFile> {
    let path = path.as_ref();
    ensure!(path.exists(), "{} does not exist", path.display());

    let source_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let source_dir = source_path
        .parent()
        .map_or_else(PathBuf::new, Path::to_path_buf);
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read SUMA spec {}", path.display()))?;
    let mut group = None;
    let mut states = Vec::new();
    let mut blocks = Vec::new();
    let mut current_block: Option<Vec<(String, String)>> = None;

    for raw_line in text.lines() {
        let Some(line) = spec_line(raw_line) else {
            continue;
        };

        if line == "NewSurface" {
            if let Some(block) = current_block.take() {
                blocks.push(block);
            }
            current_block = Some(Vec::new());
            continue;
        }

        let Some((key, value)) = key_value(line) else {
            continue;
        };

        if let Some(block) = current_block.as_mut() {
            block.push((key.to_string(), value.to_string()));
            continue;
        }

        match key {
            "Group" => group = Some(value.to_string()),
            "StateDef" => states.push(normalize_state(value)),
            _ => {}
        }
    }

    if let Some(block) = current_block {
        blocks.push(block);
    }

    let mut surfaces = Vec::new();
    let mut seen_paths = HashSet::new();
    for block in blocks {
        let Some(surface) = surface_from_block(&source_dir, &block)? else {
            continue;
        };
        if seen_paths.insert(surface.path.clone()) {
            surfaces.push(surface);
        }
    }

    ensure!(
        !surfaces.is_empty(),
        "SUMA spec {} did not contain any NewSurface entries with SurfaceName",
        path.display()
    );

    let hemisphere = infer_spec_hemisphere(&source_path, &surfaces);

    Ok(SpecFile {
        path: source_path,
        group,
        states,
        hemisphere,
        surfaces,
    })
}

fn surface_from_block(
    source_dir: &Path,
    block: &[(String, String)],
) -> Result<Option<SpecSurface>> {
    let Some(surface_name) = block_value(block, "SurfaceName") else {
        return Ok(None);
    };
    let name = derive_spec_layer_name(surface_name);
    let path = resolve_spec_path(source_dir, surface_name);
    let raw_state = block_value(block, "SurfaceState").map(str::to_string);
    let state = raw_state.as_deref().map(normalize_state);
    let local_domain_parent =
        block_value(block, "LocalDomainParent").map(|parent| normalize_parent_name(parent, &name));
    let local_curvature_parent = block_value(block, "LocalCurvatureParent")
        .map(|parent| normalize_parent_name(parent, &name))
        .or_else(|| local_domain_parent.clone());
    let label_dataset =
        block_value(block, "LabelDset").map(|path| resolve_spec_path(source_dir, path));
    let embed_dimension = block_value(block, "EmbedDimension")
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("invalid EmbedDimension value {value:?}"))
        })
        .transpose()?;

    Ok(Some(SpecSurface {
        name,
        path,
        surface_name: surface_name.to_string(),
        surface_format: block_value(block, "SurfaceFormat").map(str::to_string),
        surface_type: block_value(block, "SurfaceType").map(str::to_string),
        state,
        raw_state,
        anatomical: block_value(block, "Anatomical").and_then(parse_spec_bool),
        side: infer_surface_side(surface_name),
        local_domain_parent,
        local_curvature_parent,
        label_dataset,
        embed_dimension,
    }))
}

fn block_value<'a>(block: &'a [(String, String)], key: &str) -> Option<&'a str> {
    block
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

fn spec_line(raw_line: &str) -> Option<&str> {
    let line = raw_line
        .split_once('#')
        .map_or(raw_line, |(before, _)| before)
        .trim();
    (!line.is_empty()).then_some(line)
}

fn key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

fn resolve_spec_path(source_dir: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        source_dir.join(path)
    };

    normalize_path(joined)
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(value) => normalized.push(value),
        }
    }

    normalized
}

fn normalize_state(state: &str) -> String {
    let state = state.trim();
    let lowered = state.to_ascii_lowercase();
    for suffix in ["_lh", "_rh"] {
        if lowered.ends_with(suffix) {
            return state[..state.len() - suffix.len()].to_string();
        }
    }

    state.to_string()
}

fn normalize_parent_name(parent: &str, own_name: &str) -> String {
    let parent_name = derive_spec_layer_name(parent);
    if parent_name.eq_ignore_ascii_case("same") {
        own_name.to_string()
    } else {
        parent_name
    }
}

fn derive_spec_layer_name(value: &str) -> String {
    let mut name = Path::new(value).file_name().map_or_else(
        || value.to_string(),
        |name| name.to_string_lossy().to_string(),
    );
    for suffix in [
        ".niml.dset",
        ".niml.roi",
        ".gii.dset",
        ".surf.gii",
        ".shape.gii",
        ".func.gii",
        ".label.gii",
        ".gii",
    ] {
        if name.to_ascii_lowercase().ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }

    name
}

fn parse_spec_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" | "true" | "t" | "1" => Some(true),
        "n" | "no" | "false" | "f" | "0" => Some(false),
        _ => None,
    }
}

fn infer_surface_side(value: &str) -> SurfaceSide {
    let lower = value.to_ascii_lowercase();
    let tokens = lower
        .split(|c: char| !(c.is_ascii_alphanumeric()))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    if tokens
        .iter()
        .any(|token| *token == "lh" || *token == "left")
    {
        SurfaceSide::Left
    } else if tokens
        .iter()
        .any(|token| *token == "rh" || *token == "right")
    {
        SurfaceSide::Right
    } else if tokens
        .iter()
        .any(|token| *token == "both" || *token == "bilateral" || *token == "lr")
    {
        SurfaceSide::Both
    } else {
        SurfaceSide::Unknown
    }
}

fn infer_spec_hemisphere(path: &Path, surfaces: &[SpecSurface]) -> SpecHemisphere {
    let stem = path.file_stem().map_or_else(String::new, |stem| {
        stem.to_string_lossy().to_ascii_lowercase()
    });
    if stem.ends_with("_both") || stem.ends_with("-both") || stem.contains("_both.") {
        return SpecHemisphere::Both;
    }
    if stem.ends_with("_lh") || stem.ends_with("-lh") || stem.contains("_lh.") {
        return SpecHemisphere::Left;
    }
    if stem.ends_with("_rh") || stem.ends_with("-rh") || stem.contains("_rh.") {
        return SpecHemisphere::Right;
    }

    let has_left = surfaces
        .iter()
        .any(|surface| surface.side == SurfaceSide::Left);
    let has_right = surfaces
        .iter()
        .any(|surface| surface.side == SurfaceSide::Right);
    match (has_left, has_right) {
        (true, true) => SpecHemisphere::Both,
        (true, false) => SpecHemisphere::Left,
        (false, true) => SpecHemisphere::Right,
        (false, false) => SpecHemisphere::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        SpecHemisphere, derive_spec_layer_name, normalize_parent_name, normalize_state, read_spec,
    };
    use crate::surface::SurfaceSide;

    #[test]
    fn spec_parser_reads_surface_blocks_and_global_state() {
        let spec = read_spec(Path::new("testing/sub-3_rh.spec")).unwrap();

        assert_eq!(spec.group.as_deref(), Some("sub-3"));
        assert_eq!(spec.states.len(), 7);
        assert_eq!(spec.hemisphere, SpecHemisphere::Right);
        assert_eq!(spec.surfaces.len(), 7);
        assert_eq!(spec.surfaces[0].name, "rh.smoothwm");
        assert_eq!(spec.surfaces[0].state.as_deref(), Some("smoothwm"));
        assert_eq!(spec.surfaces[0].side, SurfaceSide::Right);
        assert_eq!(
            spec.surfaces[0].local_domain_parent.as_deref(),
            Some("rh.smoothwm")
        );
        assert_eq!(
            spec.surfaces[1].local_domain_parent.as_deref(),
            Some("rh.smoothwm")
        );
        assert_eq!(spec.surfaces[1].anatomical, Some(true));
    }

    #[test]
    fn both_spec_is_detected_without_loading_bilateral_scene() {
        let spec = read_spec(Path::new("testing/std.141.sub-3_both.spec")).unwrap();

        assert_eq!(spec.hemisphere, SpecHemisphere::Both);
        assert!(
            spec.surfaces
                .iter()
                .any(|surface| surface.side == SurfaceSide::Left)
        );
        assert!(
            spec.surfaces
                .iter()
                .any(|surface| surface.side == SurfaceSide::Right)
        );
        assert!(spec.states.iter().any(|state| state == "std.inflated"));
    }

    #[test]
    fn state_and_parent_names_match_pysuma_normalization() {
        assert_eq!(normalize_state("std.inflated_lh"), "std.inflated");
        assert_eq!(normalize_state("std.sphere.reg_rh"), "std.sphere.reg");
        assert_eq!(
            normalize_parent_name("././SAME", "std.141.lh.smoothwm"),
            "std.141.lh.smoothwm"
        );
        assert_eq!(
            normalize_parent_name("././std.141.lh.smoothwm.gii", "std.141.lh.pial"),
            "std.141.lh.smoothwm"
        );
        assert_eq!(derive_spec_layer_name("rh.white.gii"), "rh.white");
    }
}
