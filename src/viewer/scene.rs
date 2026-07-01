//! Scene model: GPU surface buffers, the `.spec`-derived surface scene and
//! its per-hemisphere components, display-mesh caching, and scene-geometry
//! statistics. Pure data types plus the scene-construction helpers that build
//! them from spec components. Moved out of `viewer/mod.rs`; all items are
//! `pub(super)` so the parent viewer module and its sibling submodules keep
//! using them unchanged.

use super::*;

pub(super) struct SurfaceBuffers {
    pub(super) surface_id: SurfaceId,
    pub(super) vertex_buffer: wgpu::Buffer,
    pub(super) vertex_bytes_len: usize,
    pub(super) triangle_index_buffer: wgpu::Buffer,
    pub(super) triangle_index_bytes_len: usize,
    pub(super) triangle_index_count: u32,
    pub(super) line_index_buffer: wgpu::Buffer,
    pub(super) line_index_bytes_len: usize,
    pub(super) line_index_count: u32,
    pub(super) point_index_buffer: wgpu::Buffer,
    pub(super) point_index_bytes_len: usize,
    pub(super) point_index_count: u32,
}

/// Per-hemisphere resident geometry for both-spec scenes. Positions and normals
/// remain in each source surface's mesh space; the model matrix moves them into
/// the active paired layout.
pub(super) struct SurfaceRenderSet {
    pub(super) instances: Vec<SurfaceRenderInstance>,
}

pub(super) struct SurfaceRenderInstance {
    pub(super) side: SurfaceSide,
    pub(super) vertex_buffer: wgpu::Buffer,
    pub(super) triangle_index_buffer: wgpu::Buffer,
    pub(super) triangle_index_count: u32,
    pub(super) line_index_buffer: wgpu::Buffer,
    pub(super) line_index_count: u32,
    pub(super) point_index_buffer: wgpu::Buffer,
    pub(super) point_index_count: u32,
    pub(super) uniform_buffer: wgpu::Buffer,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) model_matrix: Mat4,
}

impl SurfaceBuffers {
    pub(super) fn index_buffer(&self, style: SurfaceRenderStyle) -> &wgpu::Buffer {
        match style {
            SurfaceRenderStyle::Filled => &self.triangle_index_buffer,
            SurfaceRenderStyle::Triangles => &self.line_index_buffer,
            SurfaceRenderStyle::Vertices => &self.point_index_buffer,
        }
    }

    pub(super) fn index_count(&self, style: SurfaceRenderStyle) -> u32 {
        match style {
            SurfaceRenderStyle::Filled => self.triangle_index_count,
            SurfaceRenderStyle::Triangles => self.line_index_count,
            SurfaceRenderStyle::Vertices => self.point_index_count,
        }
    }
}

impl SurfaceRenderInstance {
    pub(super) fn index_buffer(&self, style: SurfaceRenderStyle) -> &wgpu::Buffer {
        match style {
            SurfaceRenderStyle::Filled => &self.triangle_index_buffer,
            SurfaceRenderStyle::Triangles => &self.line_index_buffer,
            SurfaceRenderStyle::Vertices => &self.point_index_buffer,
        }
    }

    pub(super) fn index_count(&self, style: SurfaceRenderStyle) -> u32 {
        match style {
            SurfaceRenderStyle::Filled => self.triangle_index_count,
            SurfaceRenderStyle::Triangles => self.line_index_count,
            SurfaceRenderStyle::Vertices => self.point_index_count,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SceneSurfaceLayout {
    Identity,
    PairedHemisphere,
}

#[derive(Clone)]
pub(super) struct PreparedGeometryCache {
    pub(super) surface_id: SurfaceId,
    pub(super) vertex_count: usize,
    pub(super) face_count: usize,
    pub(super) geometry: Arc<PreparedGeometry>,
}

impl PreparedGeometryCache {
    pub(super) fn matches(&self, mesh: &SurfaceMesh) -> bool {
        self.surface_id == mesh.metadata.id
            && self.vertex_count == mesh.vertices.len()
            && self.face_count == mesh.triangles.len()
    }
}

#[derive(Clone)]
pub(super) struct AnatomicalShadingCache {
    pub(super) surface_id: SurfaceId,
    pub(super) vertex_count: usize,
    pub(super) face_count: usize,
    pub(super) colors: Arc<Vec<[f32; 4]>>,
}

impl AnatomicalShadingCache {
    pub(super) fn matches(&self, mesh: &SurfaceMesh) -> bool {
        self.surface_id == mesh.metadata.id
            && self.vertex_count == mesh.vertices.len()
            && self.face_count == mesh.triangles.len()
    }
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceScene {
    pub(super) spec: SpecFile,
    pub(super) spec_path: PathBuf,
    pub(super) surface_volume_path: Option<PathBuf>,
    pub(super) surface_volume_idcode: Option<String>,
    pub(super) hemisphere: SpecHemisphere,
    pub(super) surfaces: Vec<SceneSurface>,
    pub(super) active_index: usize,
    pub(super) skipped_surfaces: usize,
    pub(super) skipped_states: usize,
}

#[derive(Debug, Clone)]
pub(super) struct SceneSurface {
    pub(super) name: String,
    pub(super) state: Option<String>,
    pub(super) path: PathBuf,
    pub(super) layout: SceneSurfaceLayout,
    pub(super) components: Vec<SceneSurfaceComponent>,
    pub(super) display_cache: Option<DisplayMeshCache>,
}

#[derive(Debug, Clone)]
pub(super) struct DisplayMeshCache {
    pub(super) layout: HemisphereLayoutState,
    pub(super) mesh: SurfaceMesh,
    pub(super) prepared_geometry: Option<Arc<PreparedGeometry>>,
}

pub(super) struct DisplayMeshSnapshot {
    pub(super) mesh: SurfaceMesh,
    pub(super) prepared_geometry: Option<Arc<PreparedGeometry>>,
}

#[derive(Debug, Clone)]
pub(super) struct SceneSurfaceComponent {
    pub(super) name: String,
    pub(super) state: Option<String>,
    pub(super) path: PathBuf,
    pub(super) side: SurfaceSide,
    pub(super) spec_surface: SpecSurface,
    pub(super) mesh: Option<SurfaceMesh>,
    pub(super) label_lookup: Option<SurfaceLabelLookup>,
    pub(super) normal_cache: Option<Arc<Vec<[f32; 3]>>>,
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceLabelLookup {
    pub(super) label_table: Option<LabelTable>,
    pub(super) node_keys: Vec<Option<i32>>,
}

impl SurfaceLabelLookup {
    pub(super) fn from_dataset(
        dataset: Dataset,
        label_table: Option<LabelTable>,
        node_count: usize,
    ) -> Result<Self> {
        let column =
            preferred_label_column(&dataset).context("label dataset has no label column")?;
        let mut node_keys = vec![None; node_count];

        for row in 0..dataset.row_count {
            let Some(node) = dataset.node_for_row(row) else {
                continue;
            };
            let Some(slot) = node_keys.get_mut(node as usize) else {
                continue;
            };
            *slot = label_value_for_row(column, row);
        }

        Ok(Self {
            label_table,
            node_keys,
        })
    }

    pub(super) fn region_for_node(&self, node_index: u32) -> Option<String> {
        let key = *self.node_keys.get(node_index as usize)?.as_ref()?;
        self.label_table
            .as_ref()
            .and_then(|table| table.label(key))
            .map(|entry| entry.label.clone())
            .or_else(|| Some(format!("label {key}")))
    }
}

impl SceneSurface {
    pub(super) fn single(component: SceneSurfaceComponent) -> Self {
        Self {
            name: component.name.clone(),
            state: component.state.clone(),
            path: component.path.clone(),
            layout: SceneSurfaceLayout::Identity,
            components: vec![component],
            display_cache: None,
        }
    }

    pub(super) fn paired(
        state: String,
        spec_path: PathBuf,
        left: SceneSurfaceComponent,
        right: SceneSurfaceComponent,
    ) -> Self {
        Self {
            name: state.clone(),
            state: Some(state),
            path: spec_path,
            layout: SceneSurfaceLayout::PairedHemisphere,
            components: vec![left, right],
            display_cache: None,
        }
    }

    pub(super) fn grouped(
        state: String,
        spec_path: PathBuf,
        components: Vec<SceneSurfaceComponent>,
    ) -> Self {
        Self {
            name: state.clone(),
            state: Some(state),
            path: spec_path,
            layout: SceneSurfaceLayout::Identity,
            components,
            display_cache: None,
        }
    }

    pub(super) fn display_mesh(
        &mut self,
        layout: HemisphereLayoutState,
    ) -> Result<DisplayMeshSnapshot> {
        ensure!(
            !self.components.is_empty(),
            "scene surface {} has no components",
            self.name
        );
        if self.components.len() == 1 {
            if let Some(cache) = self.display_cache.as_ref() {
                return Ok(DisplayMeshSnapshot {
                    mesh: cache.mesh.clone(),
                    prepared_geometry: cache.prepared_geometry.clone(),
                });
            }
            let mesh = self.components[0]
                .mesh
                .clone()
                .with_context(|| format!("surface {} is still loading", self.name))?;
            let prepared_geometry = Arc::new(PreparedGeometry::from_surface(&mesh));
            self.display_cache = Some(DisplayMeshCache {
                layout,
                mesh: mesh.clone(),
                prepared_geometry: Some(prepared_geometry.clone()),
            });
            return Ok(DisplayMeshSnapshot {
                mesh,
                prepared_geometry: Some(prepared_geometry),
            });
        }

        if let Some(cache) = self.display_cache.as_ref()
            && cache.layout == layout
        {
            return Ok(DisplayMeshSnapshot {
                mesh: cache.mesh.clone(),
                prepared_geometry: cache.prepared_geometry.clone(),
            });
        }

        let mut mesh = composite_component_mesh(&self.components, layout, self.layout)?;
        mesh.metadata.label = Some(self.name.clone());
        mesh.metadata.source_file = Some(self.path.clone());
        mesh.metadata.side = SurfaceSide::Both;
        mesh.metadata.state_name = self.state.clone();
        if let Some(first) = self.components.first() {
            let first_mesh = first
                .mesh
                .as_ref()
                .with_context(|| format!("surface component {} is still loading", first.name))?;
            mesh.metadata.group_label = first_mesh.metadata.group_label.clone();
            mesh.metadata.subject_label = first_mesh.metadata.subject_label.clone();
            mesh.metadata.surface_kind = first_mesh.metadata.surface_kind.clone();
            mesh.metadata.lineage.parent_volume_id =
                first_mesh.metadata.lineage.parent_volume_id.clone();
        }

        self.display_cache = Some(DisplayMeshCache {
            layout,
            mesh: mesh.clone(),
            prepared_geometry: None,
        });

        Ok(DisplayMeshSnapshot {
            mesh,
            prepared_geometry: None,
        })
    }

    pub(super) fn warm_display_cache(&mut self, layout: HemisphereLayoutState) -> Result<bool> {
        if self
            .display_cache
            .as_ref()
            .is_some_and(|cache| self.components.len() == 1 || cache.layout == layout)
        {
            return Ok(false);
        }

        self.display_mesh(layout)?;
        Ok(true)
    }
}

#[derive(Default)]
pub(super) struct StatePair {
    pub(super) left: Option<SceneSurfaceComponent>,
    pub(super) right: Option<SceneSurfaceComponent>,
}

pub(super) fn scene_surfaces_from_components(
    spec: &SpecFile,
    components: Vec<SceneSurfaceComponent>,
) -> (Vec<SceneSurface>, usize, Vec<String>) {
    if spec.hemisphere != SpecHemisphere::Both {
        return (
            components.into_iter().map(SceneSurface::single).collect(),
            0,
            Vec::new(),
        );
    }

    paired_scene_surfaces(spec, components)
}

pub(super) fn scene_surfaces_grouped_by_state(
    spec: &SpecFile,
    components: Vec<SceneSurfaceComponent>,
) -> (Vec<SceneSurface>, usize, Vec<String>) {
    let mut by_state = BTreeMap::<String, Vec<SceneSurfaceComponent>>::new();
    let mut skipped_states = 0;
    let mut messages = Vec::new();

    for component in components {
        let Some(state) = component.state.clone() else {
            skipped_states += 1;
            messages.push(format!(
                "Skipping command-line surface {} because it has no state.",
                component.path.display()
            ));
            continue;
        };
        by_state.entry(state).or_default().push(component);
    }

    let mut ordered_states = Vec::new();
    let mut seen = HashSet::new();
    for state in &spec.states {
        if by_state.contains_key(state) && seen.insert(state.clone()) {
            ordered_states.push(state.clone());
        }
    }
    for state in by_state.keys() {
        if seen.insert(state.clone()) {
            ordered_states.push(state.clone());
        }
    }

    let surfaces = ordered_states
        .into_iter()
        .filter_map(|state| {
            let components = by_state.remove(&state)?;
            if components.len() == 1 {
                components.into_iter().next().map(SceneSurface::single)
            } else {
                Some(SceneSurface::grouped(state, spec.path.clone(), components))
            }
        })
        .collect();

    (surfaces, skipped_states, messages)
}

pub(super) fn paired_scene_surfaces(
    spec: &SpecFile,
    components: Vec<SceneSurfaceComponent>,
) -> (Vec<SceneSurface>, usize, Vec<String>) {
    let mut by_state = BTreeMap::<String, StatePair>::new();
    let mut skipped_states = 0;
    let mut messages = Vec::new();

    for component in components {
        let Some(state) = component.state.clone() else {
            skipped_states += 1;
            messages.push(format!(
                "Skipping both-spec surface {} because it has no SurfaceState.",
                component.path.display()
            ));
            continue;
        };

        let side = component.side.clone();
        let component_name = component.name.clone();
        let pair = by_state.entry(state.clone()).or_default();
        let slot = match side {
            SurfaceSide::Left => &mut pair.left,
            SurfaceSide::Right => &mut pair.right,
            _ => {
                skipped_states += 1;
                messages.push(format!(
                    "Skipping both-spec surface {} for state {state}: side is not left or right.",
                    component.path.display()
                ));
                continue;
            }
        };

        if slot.is_some() {
            skipped_states += 1;
            messages.push(format!(
                "Skipping duplicate both-spec surface {component_name} for state {state}."
            ));
            continue;
        }

        *slot = Some(component);
    }

    let mut ordered_states = Vec::new();
    let mut seen = HashSet::new();
    for state in &spec.states {
        if by_state.contains_key(state) && seen.insert(state.clone()) {
            ordered_states.push(state.clone());
        }
    }
    for state in by_state.keys() {
        if seen.insert(state.clone()) {
            ordered_states.push(state.clone());
        }
    }

    let mut surfaces = Vec::new();
    for state in ordered_states {
        let Some(pair) = by_state.remove(&state) else {
            continue;
        };
        match (pair.left, pair.right) {
            (Some(left), Some(right)) => {
                surfaces.push(SceneSurface::paired(state, spec.path.clone(), left, right));
            }
            (left, right) => {
                skipped_states += 1;
                let missing = match (left.is_some(), right.is_some()) {
                    (true, false) => "right",
                    (false, true) => "left",
                    _ => "left and right",
                };
                messages.push(format!(
                    "Skipping both-spec state {state}: missing {missing} hemisphere surface."
                ));
            }
        }
    }

    (surfaces, skipped_states, messages)
}

pub(super) fn composite_component_mesh(
    components: &[SceneSurfaceComponent],
    layout: HemisphereLayoutState,
    component_layout: SceneSurfaceLayout,
) -> Result<SurfaceMesh> {
    let transforms = match component_layout {
        SceneSurfaceLayout::Identity => vec![ComponentTransform::default(); components.len()],
        SceneSurfaceLayout::PairedHemisphere => component_transforms(components, layout),
    };
    let vertex_count: usize = components
        .iter()
        .filter_map(|component| component.mesh.as_ref())
        .map(|mesh| mesh.vertices.len())
        .sum();
    let face_count: usize = components
        .iter()
        .filter_map(|component| component.mesh.as_ref())
        .map(|mesh| mesh.triangles.len())
        .sum();
    let mut vertices = Vec::with_capacity(vertex_count);
    let mut triangles = Vec::with_capacity(face_count);
    let mut node_offset = 0u32;

    for (component, transform) in components.iter().zip(transforms) {
        let mesh = component
            .mesh
            .as_ref()
            .with_context(|| format!("surface component {} is still loading", component.name))?;
        vertices.extend(mesh.vertices.iter().map(|position| {
            transform_point(mesh, transform, Vec3::from_array(*position)).to_array()
        }));
        triangles.extend(mesh.triangles.iter().map(|triangle| {
            [
                triangle[0] + node_offset,
                triangle[1] + node_offset,
                triangle[2] + node_offset,
            ]
        }));
        node_offset += u32::try_from(mesh.vertices.len())
            .context("paired surface has too many vertices for u32 indices")?;
    }

    SurfaceMesh::new(vertices, triangles)
}

#[derive(Debug, Clone)]
pub(super) struct SceneStats {
    pub(super) geometry: SceneGeometryStats,
    pub(super) overlay_range: Option<ValueRange>,
}

/// Geometry-derived scene statistics. Computing these runs `winding_report`,
/// which builds topology and is O(n) with heavy allocation, so the viewer
/// caches them per surface id and only recomputes when the mesh changes.
#[derive(Debug, Clone, Copy)]
pub(super) struct SceneGeometryStats {
    pub(super) node_count: usize,
    pub(super) face_count: usize,
    pub(super) total_area: f32,
    pub(super) boundary_edges: usize,
    pub(super) non_manifold_edges: usize,
    pub(super) normal_direction: NormalDirection,
}

impl SceneGeometryStats {
    pub(super) fn from_mesh(mesh: &SurfaceMesh) -> Self {
        let winding = mesh.winding_report();

        Self {
            node_count: mesh.vertices.len(),
            face_count: mesh.triangles.len(),
            total_area: mesh.total_area(),
            boundary_edges: winding.boundary_edges,
            non_manifold_edges: winding.non_manifold_edges,
            normal_direction: winding.normal_direction,
        }
    }
}
