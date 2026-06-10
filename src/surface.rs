use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use gifti_rs::{ArrayData, DataArray, GiftiImage, Meta};
use glam::{Mat4, Vec3};

#[derive(Debug, Clone)]
pub struct SurfaceMesh {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub domain: SurfaceDomain,
    pub bounds: Bounds,
    pub metadata: SurfaceMetadata,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMetadata {
    pub id: SurfaceId,
    pub label: Option<String>,
    pub source_file: Option<PathBuf>,
    pub node_count: usize,
    pub node_dimension: usize,
    pub embedding_dimension: usize,
    pub face_count: usize,
    pub face_dimension: usize,
    pub side: SurfaceSide,
    pub group_label: Option<String>,
    pub subject_label: Option<String>,
    pub state_name: Option<String>,
    pub surface_kind: SurfaceKind,
    pub anatomically_correct: AnatomicalCorrectness,
    pub sphere: Option<SphereMetadata>,
    pub lineage: SurfaceLineage,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SurfaceId(String);

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceLineage {
    pub local_domain_parent: Option<String>,
    pub local_curvature_parent: Option<String>,
    pub domain_grandparent: Option<String>,
    pub node_parent: Option<String>,
    pub parent_volume_id: Option<String>,
    pub originator_id: Option<String>,
    pub domain: SurfaceDomainIdentity,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceDomainIdentity {
    pub id: SurfaceDomainId,
    pub kind: SurfaceDomainKind,
    pub standard_space: Option<String>,
    pub node_count: usize,
    pub topology_hash: String,
    pub geometry_hash: String,
    pub allow_node_count_match: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SurfaceDomainId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDomainKind {
    NativeSubject,
    StandardTemplate,
    DerivedFromStandard,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKinship {
    SameSurface,
    SameGeometry,
    SameTopology,
    SameStandardNodeCount,
    NeedsMapping,
    Incompatible,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceDomain {
    pub id: SurfaceDomainId,
    pub node_count: usize,
    pub node_ids: Option<Vec<u32>>,
    pub row_to_node: RowToNodeMapping,
    pub sorted_nodes: SortedNodeMetadata,
    pub triangles: Vec<[u32; 3]>,
    pub topology_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowToNodeMapping {
    Dense,
    Indexed(Vec<u32>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortedNodeMetadata {
    pub is_sorted: bool,
    pub has_duplicates: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceSide {
    Left,
    Right,
    Both,
    Unknown,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceKind {
    Pial,
    WhiteMatter,
    SmoothWhiteMatter,
    Inflated,
    VeryInflated,
    Sphere,
    Flat,
    Fiducial,
    Midthickness,
    Original,
    Unknown,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnatomicalCorrectness {
    Correct,
    Incorrect,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphereMetadata {
    pub center: [f32; 3],
    pub radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub center: [f32; 3],
    pub radius: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceGeometryMetrics {
    pub face_normals: Vec<[f32; 3]>,
    pub face_areas: Vec<f32>,
    pub node_areas: Vec<f32>,
    pub total_area: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindingReport {
    pub components: usize,
    pub faces_to_flip_for_consistency: usize,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub inconsistent_edges: usize,
    pub globally_orientable: bool,
    pub normal_direction: NormalDirection,
    pub signed_volume: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalDirection {
    Outward,
    Inward,
    Mixed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceTopology {
    pub member_faces: Vec<Vec<usize>>,
    pub face_neighbors: Vec<Vec<usize>>,
    pub node_neighbors: Vec<Vec<u32>>,
    pub neighbor_distances: Vec<Vec<(u32, f32)>>,
    pub edges: Vec<EdgeRecord>,
    pub boundary_edges: Vec<EdgeRecord>,
    pub edge_to_faces: Vec<EdgeFaces>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeRecord {
    pub a: u32,
    pub b: u32,
    pub length: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeFaces {
    pub edge: EdgeRecord,
    pub faces: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeshValidationReport {
    pub issues: Vec<MeshValidationIssue>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MeshValidationIssue {
    EmptyVertices,
    EmptyTriangles,
    TriangleIndexOutOfBounds {
        face: usize,
        index: u32,
        vertex_count: usize,
    },
    DuplicateTriangle {
        first: usize,
        duplicate: usize,
    },
    DegenerateTriangle {
        face: usize,
    },
    NonManifoldEdge {
        edge: (u32, u32),
        face_count: usize,
    },
    BoundaryEdges {
        count: usize,
    },
    DisconnectedComponents {
        count: usize,
    },
    InconsistentWinding {
        inconsistent_edges: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMask(BitMask);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceMask(BitMask);

/// Shared bit-set backing both [`NodeMask`] and [`FaceMask`]. Indices are
/// `usize`; the wrappers convert their public index types to and from it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BitMask {
    selected: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceMaskMode {
    AnyNode,
    AllNodes,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfacePatch {
    pub nodes: Vec<u32>,
    pub faces: Vec<usize>,
    pub bounds: Bounds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfacePath {
    pub nodes: Vec<u32>,
    pub length: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceShapeMetrics {
    pub parent_surface_id: Option<SurfaceId>,
    pub convexity: Vec<f32>,
    pub mean_curvature_like: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmoothingWeights {
    Uniform,
    InverseDistance,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceTransform {
    mat: Mat4,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VolumeSpace {
    pub dimensions: [usize; 3],
    pub voxel_to_world: SurfaceTransform,
    pub world_to_voxel: SurfaceTransform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VoxelIndex {
    pub i: usize,
    pub j: usize,
    pub k: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeSamplePoint {
    pub node: u32,
    pub world: [f32; 3],
    pub voxel: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipPlane {
    pub normal: [f32; 3],
    pub offset: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineSegment {
    pub start: [f32; 3],
    pub end: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceToSurfaceMap {
    pub source_domain_id: SurfaceDomainId,
    pub target_domain_id: SurfaceDomainId,
    pub kind: SurfaceMappingKind,
    pub target_weights: Vec<NodeWeights>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceMappingKind {
    SameTopology,
    SameStandardNodeCount,
    NearestNode,
    BarycentricTriangle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeWeights {
    pub weights: Vec<(u32, f32)>,
}

#[derive(Debug, Clone)]
pub struct OverlayDataset {
    pub values: Vec<f32>,
    pub range: ValueRange,
    pub threshold_values: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Copy)]
pub struct ValueRange {
    pub min: f32,
    pub max: f32,
}

impl SurfaceMesh {
    pub fn new(vertices: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> Result<Self> {
        let domain = SurfaceDomain::from_triangles(vertices.len(), triangles.clone())?;
        let bounds = Bounds::from_vertices(&vertices)?;
        let metadata = SurfaceMetadata::from_geometry(None, None, &vertices, &domain, bounds, 3, 3);

        Ok(Self {
            vertices,
            triangles,
            domain,
            bounds,
            metadata,
        })
    }

    pub fn from_gifti_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            bail!("{} does not exist", path.display());
        }

        let image = gifti_rs::read(path)
            .with_context(|| format!("failed to read GIFTI surface {}", path.display()))?;

        let pointset = image
            .find_array(gifti_rs::intent::POINTSET)
            .context("GIFTI file does not contain a POINTSET data array")?;
        let triangle = image
            .find_array(gifti_rs::intent::TRIANGLE)
            .context("GIFTI file does not contain a TRIANGLE data array")?;

        let vertices = vertices_from_array(pointset)?;
        let triangles = triangles_from_array(triangle)?;
        let domain = SurfaceDomain::from_triangles(vertices.len(), triangles.clone())?;
        let bounds = Bounds::from_vertices(&vertices)?;
        let source_file = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let metadata = SurfaceMetadata::from_gifti(
            &source_file,
            &image,
            pointset,
            triangle,
            &vertices,
            &domain,
            bounds,
        );

        Ok(Self {
            vertices,
            triangles,
            domain,
            bounds,
            metadata,
        })
    }

    pub fn vertex_normals(&self) -> Vec<[f32; 3]> {
        let mut normals = vec![Vec3::ZERO; self.vertices.len()];

        for triangle in &self.domain.triangles {
            let [a, b, c] = *triangle;
            let a = a as usize;
            let b = b as usize;
            let c = c as usize;

            let pa = Vec3::from_array(self.vertices[a]);
            let pb = Vec3::from_array(self.vertices[b]);
            let pc = Vec3::from_array(self.vertices[c]);
            let face_normal = (pb - pa).cross(pc - pa);

            if face_normal.length_squared() > f32::EPSILON {
                normals[a] += face_normal;
                normals[b] += face_normal;
                normals[c] += face_normal;
            }
        }

        normals
            .into_iter()
            .map(|normal| {
                if normal.length_squared() > f32::EPSILON {
                    normal.normalize().to_array()
                } else {
                    Vec3::Z.to_array()
                }
            })
            .collect()
    }

    pub fn face_normals(&self) -> Vec<[f32; 3]> {
        self.domain
            .triangles
            .iter()
            .map(|triangle| face_normal_and_area(&self.vertices, *triangle).0)
            .collect()
    }

    pub fn face_areas(&self) -> Vec<f32> {
        self.domain
            .triangles
            .iter()
            .map(|triangle| face_normal_and_area(&self.vertices, *triangle).1)
            .collect()
    }

    pub fn node_areas(&self) -> Vec<f32> {
        self.geometry_metrics().node_areas
    }

    pub fn total_area(&self) -> f32 {
        self.domain
            .triangles
            .iter()
            .map(|triangle| face_normal_and_area(&self.vertices, *triangle).1)
            .sum()
    }

    pub fn geometry_metrics(&self) -> SurfaceGeometryMetrics {
        let mut face_normals = Vec::with_capacity(self.domain.triangles.len());
        let mut face_areas = Vec::with_capacity(self.domain.triangles.len());
        let mut node_areas = vec![0.0; self.vertices.len()];
        let mut total_area = 0.0;

        for triangle in &self.domain.triangles {
            let (normal, area) = face_normal_and_area(&self.vertices, *triangle);
            face_normals.push(normal);
            face_areas.push(area);
            total_area += area;

            let per_node_area = area / 3.0;
            for node in triangle {
                node_areas[*node as usize] += per_node_area;
            }
        }

        SurfaceGeometryMetrics {
            face_normals,
            face_areas,
            node_areas,
            total_area,
        }
    }

    pub fn winding_report(&self) -> WindingReport {
        WindingAnalysis::new(&self.vertices, &self.domain.triangles).report
    }

    pub fn flipped_winding(&self) -> Vec<[u32; 3]> {
        flip_triangles(&self.domain.triangles)
    }

    pub fn triangles_with_consistent_winding(&self) -> Vec<[u32; 3]> {
        let analysis = WindingAnalysis::new(&self.vertices, &self.domain.triangles);
        apply_face_flips(&self.domain.triangles, &analysis.face_flips)
    }

    pub fn triangles_oriented_outward(&self) -> Option<Vec<[u32; 3]>> {
        let analysis = WindingAnalysis::new(&self.vertices, &self.domain.triangles);
        if !analysis.report.globally_orientable
            || analysis.report.boundary_edges > 0
            || analysis.component_count == 0
        {
            return None;
        }

        let mut triangles = apply_face_flips(&self.domain.triangles, &analysis.face_flips);
        let volumes = component_signed_volumes(&self.vertices, &triangles, &analysis.component_ids);

        for component in 0..analysis.component_count {
            let volume = volumes.get(component).copied().unwrap_or(0.0);
            if volume.abs() <= f32::EPSILON {
                return None;
            }

            if volume < 0.0 {
                for (triangle, component_id) in
                    triangles.iter_mut().zip(analysis.component_ids.iter())
                {
                    if *component_id == component {
                        *triangle = flip_triangle(*triangle);
                    }
                }
            }
        }

        Some(triangles)
    }

    pub fn topology(&self) -> SurfaceTopology {
        SurfaceTopology::from_geometry(&self.vertices, &self.domain.triangles)
    }

    pub fn validation_report(&self) -> MeshValidationReport {
        MeshValidationReport::from_mesh(self)
    }

    pub fn face_mask_from_node_mask(
        &self,
        node_mask: &NodeMask,
        mode: FaceMaskMode,
    ) -> Result<FaceMask> {
        ensure!(
            node_mask.len() == self.vertices.len(),
            "node mask has {} nodes but surface has {}",
            node_mask.len(),
            self.vertices.len()
        );

        let selected = self
            .domain
            .triangles
            .iter()
            .map(|triangle| {
                let selected_nodes = triangle
                    .iter()
                    .filter(|node| node_mask.contains(**node))
                    .count();
                match mode {
                    FaceMaskMode::AnyNode => selected_nodes > 0,
                    FaceMaskMode::AllNodes => selected_nodes == 3,
                }
            })
            .collect();

        Ok(FaceMask(BitMask { selected }))
    }

    pub fn patch_from_node_mask(
        &self,
        node_mask: &NodeMask,
        mode: FaceMaskMode,
    ) -> Result<SurfacePatch> {
        let face_mask = self.face_mask_from_node_mask(node_mask, mode)?;
        self.patch_from_face_mask(&face_mask)
    }

    pub fn patch_from_face_mask(&self, face_mask: &FaceMask) -> Result<SurfacePatch> {
        ensure!(
            face_mask.len() == self.domain.triangles.len(),
            "face mask has {} faces but surface has {}",
            face_mask.len(),
            self.domain.triangles.len()
        );

        let faces = face_mask.faces();
        ensure!(!faces.is_empty(), "surface patch has no faces");

        let mut nodes = BTreeSet::new();
        for face in &faces {
            for node in self.domain.triangles[*face] {
                nodes.insert(node);
            }
        }
        let nodes = nodes.into_iter().collect::<Vec<_>>();
        let vertices = nodes
            .iter()
            .map(|node| self.vertices[*node as usize])
            .collect::<Vec<_>>();
        let bounds = Bounds::from_vertices(&vertices)?;

        Ok(SurfacePatch {
            nodes,
            faces,
            bounds,
        })
    }

    pub fn edge_path_from_nodes(&self, nodes: &[u32]) -> Result<Vec<EdgeRecord>> {
        ensure!(!nodes.is_empty(), "node path is empty");
        for node in nodes {
            ensure!(
                self.domain.contains_node(*node),
                "node path references node {} outside node count {}",
                node,
                self.domain.node_count
            );
        }

        let topology = self.topology();
        nodes
            .windows(2)
            .map(|window| {
                edge_record_between(&topology, window[0], window[1]).with_context(|| {
                    format!(
                        "node path segment {} -> {} is not a mesh edge",
                        window[0], window[1]
                    )
                })
            })
            .collect()
    }

    pub fn node_path_length(&self, nodes: &[u32]) -> Result<f32> {
        Ok(self
            .edge_path_from_nodes(nodes)?
            .iter()
            .map(|edge| edge.length)
            .sum())
    }

    pub fn triangle_path_from_node_path(&self, nodes: &[u32]) -> Result<Vec<usize>> {
        ensure!(!nodes.is_empty(), "node path is empty");
        let topology = self.topology();
        nodes
            .windows(2)
            .map(|window| {
                let (a, b) = edge_key(window[0], window[1]);
                topology
                    .edge_to_faces
                    .iter()
                    .find(|entry| entry.edge.a == a && entry.edge.b == b)
                    .and_then(|entry| entry.faces.first().copied())
                    .with_context(|| {
                        format!(
                            "node path segment {} -> {} is not a mesh edge",
                            window[0], window[1]
                        )
                    })
            })
            .collect()
    }

    pub fn faces_touching_nodes(&self, nodes: &[u32]) -> Result<Vec<usize>> {
        let topology = self.topology();
        let mut faces = BTreeSet::new();
        for node in nodes {
            ensure!(
                self.domain.contains_node(*node),
                "node {} is outside node count {}",
                node,
                self.domain.node_count
            );
            for face in &topology.member_faces[*node as usize] {
                faces.insert(*face);
            }
        }

        Ok(faces.into_iter().collect())
    }

    pub fn contour_edges_for_node_mask(&self, node_mask: &NodeMask) -> Result<Vec<EdgeRecord>> {
        ensure!(
            node_mask.len() == self.vertices.len(),
            "node mask has {} nodes but surface has {}",
            node_mask.len(),
            self.vertices.len()
        );

        Ok(self
            .topology()
            .edges
            .into_iter()
            .filter(|edge| node_mask.contains(edge.a) ^ node_mask.contains(edge.b))
            .collect())
    }

    pub fn node_mask_from_face_mask(&self, face_mask: &FaceMask) -> Result<NodeMask> {
        ensure!(
            face_mask.len() == self.domain.triangles.len(),
            "face mask has {} faces but surface has {}",
            face_mask.len(),
            self.domain.triangles.len()
        );

        let mut nodes = BTreeSet::new();
        for face in face_mask.faces() {
            for node in self.domain.triangles[face] {
                nodes.insert(node);
            }
        }

        NodeMask::from_nodes(self.vertices.len(), nodes)
    }

    pub fn shortest_node_path(&self, start: u32, goal: u32) -> Result<Option<SurfacePath>> {
        ensure!(
            self.domain.contains_node(start),
            "start node {} is outside node count {}",
            start,
            self.domain.node_count
        );
        ensure!(
            self.domain.contains_node(goal),
            "goal node {} is outside node count {}",
            goal,
            self.domain.node_count
        );

        if start == goal {
            return Ok(Some(SurfacePath {
                nodes: vec![start],
                length: 0.0,
            }));
        }

        let topology = self.topology();
        let (distances, previous) = shortest_path_tree(&topology, start);
        let goal_distance = distances[goal as usize];
        if !goal_distance.is_finite() {
            return Ok(None);
        }

        let mut nodes = Vec::new();
        let mut current = Some(goal);
        while let Some(node) = current {
            nodes.push(node);
            if node == start {
                break;
            }
            current = previous[node as usize];
        }
        nodes.reverse();

        Ok(Some(SurfacePath {
            nodes,
            length: goal_distance,
        }))
    }

    pub fn k_ring_neighborhood(&self, seed: u32, rings: usize) -> Result<NodeMask> {
        ensure!(
            self.domain.contains_node(seed),
            "seed node {} is outside node count {}",
            seed,
            self.domain.node_count
        );

        let topology = self.topology();
        let mut selected = BTreeSet::from([seed]);
        let mut frontier = BTreeSet::from([seed]);

        for _ in 0..rings {
            let mut next_frontier = BTreeSet::new();
            for node in &frontier {
                for neighbor in &topology.node_neighbors[*node as usize] {
                    if selected.insert(*neighbor) {
                        next_frontier.insert(*neighbor);
                    }
                }
            }
            frontier = next_frontier;
            if frontier.is_empty() {
                break;
            }
        }

        NodeMask::from_nodes(self.vertices.len(), selected)
    }

    pub fn distance_limited_neighborhood(
        &self,
        seed: u32,
        max_distance: f32,
    ) -> Result<Vec<(u32, f32)>> {
        ensure!(
            max_distance.is_finite() && max_distance >= 0.0,
            "distance limit must be finite and non-negative"
        );
        ensure!(
            self.domain.contains_node(seed),
            "seed node {} is outside node count {}",
            seed,
            self.domain.node_count
        );

        let topology = self.topology();
        let (distances, _) = shortest_path_tree(&topology, seed);
        Ok(distances
            .into_iter()
            .enumerate()
            .filter_map(|(node, distance)| {
                (distance.is_finite() && distance <= max_distance)
                    .then_some((node as u32, distance))
            })
            .collect())
    }

    pub fn spherical_neighborhood(&self, seed: u32, radius: f32) -> Result<NodeMask> {
        ensure!(
            radius.is_finite() && radius >= 0.0,
            "spherical neighborhood radius must be finite and non-negative"
        );
        ensure!(
            self.domain.contains_node(seed),
            "seed node {} is outside node count {}",
            seed,
            self.domain.node_count
        );

        let center = self.vertices[seed as usize];
        let nodes = self
            .vertices
            .iter()
            .enumerate()
            .filter_map(|(node, vertex)| {
                (vertex_distance(center, *vertex) <= radius).then_some(node as u32)
            })
            .collect::<Vec<_>>();

        NodeMask::from_nodes(self.vertices.len(), nodes)
    }

    pub fn shape_metrics(&self) -> SurfaceShapeMetrics {
        self.shape_metrics_with_parent(None)
    }

    pub fn suma_convexity(&self) -> Vec<f32> {
        let topology = self.topology();
        let normals = self.vertex_normals();
        let mut convexity = Vec::with_capacity(self.vertices.len());

        for (node, vertex) in self.vertices.iter().copied().enumerate() {
            let position = Vec3::from_array(vertex);
            let normal = Vec3::from_array(normals[node]);
            let plane_offset = -normal.dot(position);
            let mut node_convexity = 0.0;

            for neighbor in &topology.node_neighbors[node] {
                let neighbor_position = Vec3::from_array(self.vertices[*neighbor as usize]);
                let edge_length = position.distance(neighbor_position);
                if edge_length <= f32::EPSILON {
                    continue;
                }

                let signed_distance = normal.dot(neighbor_position) + plane_offset;
                node_convexity -= signed_distance / edge_length;
            }

            convexity.push(node_convexity);
        }

        convexity
    }

    pub fn shape_metrics_with_parent(
        &self,
        parent_surface_id: Option<SurfaceId>,
    ) -> SurfaceShapeMetrics {
        let topology = self.topology();
        let normals = self.vertex_normals();
        let center = Vec3::from_array(self.bounds.center);
        let mut convexity = Vec::with_capacity(self.vertices.len());
        let mut mean_curvature_like = Vec::with_capacity(self.vertices.len());

        for (node, vertex) in self.vertices.iter().copied().enumerate() {
            let position = Vec3::from_array(vertex);
            let normal = Vec3::from_array(normals[node]);
            convexity.push((position - center).dot(normal));

            let neighbors = &topology.node_neighbors[node];
            if neighbors.is_empty() {
                mean_curvature_like.push(0.0);
                continue;
            }

            let average_neighbor = neighbors
                .iter()
                .map(|neighbor| Vec3::from_array(self.vertices[*neighbor as usize]))
                .fold(Vec3::ZERO, |sum, value| sum + value)
                / neighbors.len() as f32;
            mean_curvature_like.push((average_neighbor - position).dot(normal));
        }

        SurfaceShapeMetrics {
            parent_surface_id,
            convexity,
            mean_curvature_like,
        }
    }

    pub fn smooth_scalar_values(
        &self,
        values: &[f32],
        iterations: usize,
        weights: SmoothingWeights,
        mask: Option<&NodeMask>,
    ) -> Result<Vec<f32>> {
        ensure!(
            values.len() == self.vertices.len(),
            "scalar values have {} rows but surface has {} nodes",
            values.len(),
            self.vertices.len()
        );
        if let Some(mask) = mask {
            ensure!(
                mask.len() == self.vertices.len(),
                "node mask has {} nodes but surface has {}",
                mask.len(),
                self.vertices.len()
            );
        }

        let topology = self.topology();
        let mut current = values.to_vec();
        for _ in 0..iterations {
            let mut next = current.clone();
            for node in 0..self.vertices.len() {
                if mask.is_some_and(|mask| !mask.contains(node as u32)) {
                    continue;
                }

                let mut weighted_sum = current[node];
                let mut total_weight = 1.0;
                for (neighbor, distance) in &topology.neighbor_distances[node] {
                    if mask.is_some_and(|mask| !mask.contains(*neighbor)) {
                        continue;
                    }

                    let weight = match weights {
                        SmoothingWeights::Uniform => 1.0,
                        SmoothingWeights::InverseDistance => {
                            if *distance > f32::EPSILON {
                                1.0 / *distance
                            } else {
                                1.0
                            }
                        }
                    };
                    weighted_sum += current[*neighbor as usize] * weight;
                    total_weight += weight;
                }
                next[node] = weighted_sum / total_weight;
            }
            current = next;
        }

        Ok(current)
    }

    pub fn smooth_vertices(
        &self,
        iterations: usize,
        lambda: f32,
        mask: Option<&NodeMask>,
    ) -> Result<Vec<[f32; 3]>> {
        ensure!(
            lambda.is_finite() && (0.0..=1.0).contains(&lambda),
            "vertex smoothing lambda must be in [0, 1]"
        );
        if let Some(mask) = mask {
            ensure!(
                mask.len() == self.vertices.len(),
                "node mask has {} nodes but surface has {}",
                mask.len(),
                self.vertices.len()
            );
        }

        let topology = self.topology();
        let mut current = self.vertices.clone();
        for _ in 0..iterations {
            let mut next = current.clone();
            for node in 0..self.vertices.len() {
                if mask.is_some_and(|mask| !mask.contains(node as u32)) {
                    continue;
                }

                let neighbors = topology.node_neighbors[node]
                    .iter()
                    .copied()
                    .filter(|neighbor| mask.is_none_or(|mask| mask.contains(*neighbor)))
                    .collect::<Vec<_>>();
                if neighbors.is_empty() {
                    continue;
                }

                let average = neighbors
                    .iter()
                    .map(|neighbor| Vec3::from_array(current[*neighbor as usize]))
                    .fold(Vec3::ZERO, |sum, value| sum + value)
                    / neighbors.len() as f32;
                let original = Vec3::from_array(current[node]);
                next[node] = original.lerp(average, lambda).to_array();
            }
            current = next;
        }

        Ok(current)
    }

    pub fn transformed_vertices(&self, transform: SurfaceTransform) -> Vec<[f32; 3]> {
        self.vertices
            .iter()
            .copied()
            .map(|vertex| transform.transform_point(vertex))
            .collect()
    }

    pub fn nearest_node_to_world(&self, world: [f32; 3]) -> Option<(u32, f32)> {
        self.vertices
            .iter()
            .copied()
            .enumerate()
            .map(|(node, vertex)| (node as u32, vertex_distance(vertex, world)))
            .min_by(|(_, left), (_, right)| left.partial_cmp(right).unwrap_or(Ordering::Equal))
    }

    pub fn nearest_node_to_voxel(
        &self,
        volume: &VolumeSpace,
        voxel: [f32; 3],
    ) -> Option<(u32, f32)> {
        self.nearest_node_to_world(volume.voxel_to_world(voxel))
    }

    pub fn voxel_distance(&self, volume: &VolumeSpace, voxel: [f32; 3]) -> Option<f32> {
        self.nearest_node_to_voxel(volume, voxel)
            .map(|(_, distance)| distance)
    }

    pub fn surface_voxels(&self, volume: &VolumeSpace) -> BTreeSet<VoxelIndex> {
        self.vertices
            .iter()
            .copied()
            .filter_map(|vertex| volume.world_to_voxel_index(vertex))
            .collect()
    }

    pub fn volume_sample_points(&self, volume: &VolumeSpace) -> Vec<VolumeSamplePoint> {
        self.vertices
            .iter()
            .copied()
            .enumerate()
            .map(|(node, world)| VolumeSamplePoint {
                node: node as u32,
                world,
                voxel: volume.world_to_voxel(world),
            })
            .collect()
    }

    pub fn node_mask_for_clip_plane(
        &self,
        plane: ClipPlane,
        keep_positive: bool,
    ) -> Result<NodeMask> {
        let nodes = self
            .vertices
            .iter()
            .enumerate()
            .filter_map(|(node, vertex)| {
                let distance = plane.signed_distance(*vertex);
                ((keep_positive && distance >= 0.0) || (!keep_positive && distance <= 0.0))
                    .then_some(node as u32)
            })
            .collect::<Vec<_>>();

        NodeMask::from_nodes(self.vertices.len(), nodes)
    }

    pub fn face_mask_for_clip_plane(
        &self,
        plane: ClipPlane,
        keep_positive: bool,
        mode: FaceMaskMode,
    ) -> Result<FaceMask> {
        let node_mask = self.node_mask_for_clip_plane(plane, keep_positive)?;
        self.face_mask_from_node_mask(&node_mask, mode)
    }

    pub fn plane_intersections(&self, plane: ClipPlane) -> Vec<LineSegment> {
        self.domain
            .triangles
            .iter()
            .filter_map(|triangle| triangle_plane_intersection(&self.vertices, *triangle, plane))
            .collect()
    }

    pub fn nodewise_mapping_to(&self, target: &Self) -> SurfaceToSurfaceMap {
        if self.domain.node_count == target.domain.node_count
            && self.metadata.can_share_nodewise_data_with(&target.metadata)
        {
            let kind = if self.domain.shares_topology_with(&target.domain) {
                SurfaceMappingKind::SameTopology
            } else {
                SurfaceMappingKind::SameStandardNodeCount
            };
            return SurfaceToSurfaceMap::identity_like(&self.domain, &target.domain, kind);
        }

        self.nearest_node_mapping_to(target)
    }

    pub fn nearest_node_mapping_to(&self, target: &Self) -> SurfaceToSurfaceMap {
        let target_weights = target
            .vertices
            .iter()
            .copied()
            .map(|vertex| {
                let source_node = self
                    .nearest_node_to_world(vertex)
                    .map(|(node, _)| node)
                    .unwrap_or(0);
                NodeWeights {
                    weights: vec![(source_node, 1.0)],
                }
            })
            .collect();

        SurfaceToSurfaceMap {
            source_domain_id: self.domain.id.clone(),
            target_domain_id: target.domain.id.clone(),
            kind: SurfaceMappingKind::NearestNode,
            target_weights,
        }
    }

    pub fn barycentric_triangle_mapping_to(&self, target: &Self) -> SurfaceToSurfaceMap {
        let target_weights = target
            .vertices
            .iter()
            .copied()
            .map(|vertex| closest_triangle_weights(&self.vertices, &self.domain.triangles, vertex))
            .collect();

        SurfaceToSurfaceMap {
            source_domain_id: self.domain.id.clone(),
            target_domain_id: target.domain.id.clone(),
            kind: SurfaceMappingKind::BarycentricTriangle,
            target_weights,
        }
    }
}

impl MeshValidationReport {
    pub fn is_valid(&self) -> bool {
        self.issues.is_empty()
    }
}

impl BitMask {
    fn filled(len: usize, value: bool) -> Self {
        Self {
            selected: vec![value; len],
        }
    }

    fn from_indices(
        len: usize,
        indices: impl IntoIterator<Item = usize>,
        what: &str,
    ) -> Result<Self> {
        let mut mask = Self::filled(len, false);
        for index in indices {
            ensure!(
                index < len,
                "{what} mask references {what} {} outside {what} count {}",
                index,
                len
            );
            mask.selected[index] = true;
        }
        Ok(mask)
    }

    fn len(&self) -> usize {
        self.selected.len()
    }

    fn is_empty(&self) -> bool {
        !self.selected.iter().any(|selected| *selected)
    }

    fn contains(&self, index: usize) -> bool {
        self.selected.get(index).copied().unwrap_or(false)
    }

    fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected
            .iter()
            .enumerate()
            .filter_map(|(index, selected)| selected.then_some(index))
    }

    fn invert(&self) -> Self {
        Self {
            selected: self.selected.iter().map(|selected| !selected).collect(),
        }
    }

    fn combine(&self, other: &Self, what: &str, f: impl Fn(bool, bool) -> bool) -> Result<Self> {
        ensure!(
            self.len() == other.len(),
            "{what} masks have different lengths: {} and {}",
            self.len(),
            other.len()
        );

        Ok(Self {
            selected: self
                .selected
                .iter()
                .copied()
                .zip(other.selected.iter().copied())
                .map(|(left, right)| f(left, right))
                .collect(),
        })
    }
}

impl NodeMask {
    pub fn empty(node_count: usize) -> Self {
        Self(BitMask::filled(node_count, false))
    }

    pub fn all(node_count: usize) -> Self {
        Self(BitMask::filled(node_count, true))
    }

    pub fn from_nodes(node_count: usize, nodes: impl IntoIterator<Item = u32>) -> Result<Self> {
        Ok(Self(BitMask::from_indices(
            node_count,
            nodes.into_iter().map(|node| node as usize),
            "node",
        )?))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, node: u32) -> bool {
        self.0.contains(node as usize)
    }

    pub fn nodes(&self) -> Vec<u32> {
        self.0.indices().map(|node| node as u32).collect()
    }

    pub fn union(&self, other: &Self) -> Result<Self> {
        Ok(Self(self.0.combine(&other.0, "node", |l, r| l || r)?))
    }

    pub fn intersection(&self, other: &Self) -> Result<Self> {
        Ok(Self(self.0.combine(&other.0, "node", |l, r| l && r)?))
    }

    pub fn difference(&self, other: &Self) -> Result<Self> {
        Ok(Self(self.0.combine(&other.0, "node", |l, r| l && !r)?))
    }

    pub fn invert(&self) -> Self {
        Self(self.0.invert())
    }
}

impl FaceMask {
    pub fn empty(face_count: usize) -> Self {
        Self(BitMask::filled(face_count, false))
    }

    pub fn all(face_count: usize) -> Self {
        Self(BitMask::filled(face_count, true))
    }

    pub fn from_faces(face_count: usize, faces: impl IntoIterator<Item = usize>) -> Result<Self> {
        Ok(Self(BitMask::from_indices(face_count, faces, "face")?))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, face: usize) -> bool {
        self.0.contains(face)
    }

    pub fn faces(&self) -> Vec<usize> {
        self.0.indices().collect()
    }

    pub fn union(&self, other: &Self) -> Result<Self> {
        Ok(Self(self.0.combine(&other.0, "face", |l, r| l || r)?))
    }

    pub fn intersection(&self, other: &Self) -> Result<Self> {
        Ok(Self(self.0.combine(&other.0, "face", |l, r| l && r)?))
    }

    pub fn difference(&self, other: &Self) -> Result<Self> {
        Ok(Self(self.0.combine(&other.0, "face", |l, r| l && !r)?))
    }

    pub fn invert(&self) -> Self {
        Self(self.0.invert())
    }
}

impl SurfaceTransform {
    pub fn identity() -> Self {
        Self {
            mat: Mat4::IDENTITY,
        }
    }

    pub fn from_matrix(matrix: [[f32; 4]; 4]) -> Self {
        Self {
            mat: Mat4::from_cols_array_2d(&matrix),
        }
    }

    pub fn to_matrix(self) -> [[f32; 4]; 4] {
        self.mat.to_cols_array_2d()
    }

    pub fn translation(offset: [f32; 3]) -> Self {
        Self {
            mat: Mat4::from_translation(Vec3::from_array(offset)),
        }
    }

    pub fn scale(scale: [f32; 3]) -> Self {
        Self {
            mat: Mat4::from_scale(Vec3::from_array(scale)),
        }
    }

    pub fn then(self, next: Self) -> Self {
        Self {
            mat: next.mat * self.mat,
        }
    }

    pub fn inverse(self) -> Self {
        Self {
            mat: self.mat.inverse(),
        }
    }

    pub fn transform_point(self, point: [f32; 3]) -> [f32; 3] {
        self.mat
            .transform_point3(Vec3::from_array(point))
            .to_array()
    }

    pub fn transform_vector(self, vector: [f32; 3]) -> [f32; 3] {
        self.mat
            .transform_vector3(Vec3::from_array(vector))
            .to_array()
    }
}

impl VolumeSpace {
    pub fn new(dimensions: [usize; 3], voxel_to_world: SurfaceTransform) -> Result<Self> {
        ensure!(
            dimensions.iter().all(|dimension| *dimension > 0),
            "volume dimensions must all be positive"
        );
        let world_to_voxel = voxel_to_world.inverse();

        Ok(Self {
            dimensions,
            voxel_to_world,
            world_to_voxel,
        })
    }

    pub fn voxel_to_world(&self, voxel: [f32; 3]) -> [f32; 3] {
        self.voxel_to_world.transform_point(voxel)
    }

    pub fn world_to_voxel(&self, world: [f32; 3]) -> [f32; 3] {
        self.world_to_voxel.transform_point(world)
    }

    pub fn world_to_voxel_index(&self, world: [f32; 3]) -> Option<VoxelIndex> {
        let voxel = self.world_to_voxel(world);
        self.voxel_index_from_float(voxel)
    }

    pub fn voxel_index_from_float(&self, voxel: [f32; 3]) -> Option<VoxelIndex> {
        if !voxel.iter().all(|value| value.is_finite()) {
            return None;
        }

        let i = voxel[0].round() as isize;
        let j = voxel[1].round() as isize;
        let k = voxel[2].round() as isize;
        if i < 0
            || j < 0
            || k < 0
            || i as usize >= self.dimensions[0]
            || j as usize >= self.dimensions[1]
            || k as usize >= self.dimensions[2]
        {
            return None;
        }

        Some(VoxelIndex {
            i: i as usize,
            j: j as usize,
            k: k as usize,
        })
    }
}

impl ClipPlane {
    pub fn from_point_normal(point: [f32; 3], normal: [f32; 3]) -> Result<Self> {
        let normal = Vec3::from_array(normal);
        ensure!(
            normal.length_squared() > f32::EPSILON,
            "clip plane normal must be non-zero"
        );
        let normal = normal.normalize();
        let point = Vec3::from_array(point);

        Ok(Self {
            normal: normal.to_array(),
            offset: -normal.dot(point),
        })
    }

    pub fn signed_distance(&self, point: [f32; 3]) -> f32 {
        Vec3::from_array(self.normal).dot(Vec3::from_array(point)) + self.offset
    }
}

impl SurfaceToSurfaceMap {
    fn identity_like(
        source_domain: &SurfaceDomain,
        target_domain: &SurfaceDomain,
        kind: SurfaceMappingKind,
    ) -> Self {
        let count = source_domain.node_count.min(target_domain.node_count);
        let target_weights = (0..count)
            .map(|node| NodeWeights {
                weights: vec![(node as u32, 1.0)],
            })
            .collect();

        Self {
            source_domain_id: source_domain.id.clone(),
            target_domain_id: target_domain.id.clone(),
            kind,
            target_weights,
        }
    }

    pub fn transfer_f32(&self, source_values: &[f32]) -> Result<Vec<f32>> {
        self.target_weights
            .iter()
            .map(|weights| weights.apply_f32(source_values))
            .collect()
    }
}

impl NodeWeights {
    fn apply_f32(&self, source_values: &[f32]) -> Result<f32> {
        let mut value = 0.0;
        for (node, weight) in &self.weights {
            let source = source_values
                .get(*node as usize)
                .with_context(|| format!("mapping references missing source node {node}"))?;
            value += source * weight;
        }
        Ok(value)
    }
}

impl SurfaceMetadata {
    fn from_geometry(
        source_file: Option<PathBuf>,
        label: Option<String>,
        vertices: &[[f32; 3]],
        domain: &SurfaceDomain,
        bounds: Bounds,
        node_dimension: usize,
        face_dimension: usize,
    ) -> Self {
        let surface_kind = source_file
            .as_deref()
            .and_then(infer_surface_kind_from_path)
            .unwrap_or(SurfaceKind::Unknown);
        let side = source_file
            .as_deref()
            .and_then(infer_side_from_path)
            .unwrap_or(SurfaceSide::Unknown);
        let subject_label = source_file.as_deref().and_then(infer_subject_from_path);
        let sphere = sphere_from_kind_and_bounds(&surface_kind, bounds);
        let lineage = SurfaceLineage::from_surface(
            source_file.as_deref(),
            vertices,
            domain,
            subject_label.as_deref(),
        );

        Self {
            id: SurfaceId::from_surface_data(vertices, &domain.triangles),
            label,
            source_file,
            node_count: vertices.len(),
            node_dimension,
            embedding_dimension: 3,
            face_count: domain.triangles.len(),
            face_dimension,
            side,
            group_label: None,
            subject_label,
            state_name: surface_kind.state_name(),
            surface_kind,
            anatomically_correct: AnatomicalCorrectness::Unknown,
            sphere,
            lineage,
        }
    }

    fn from_gifti(
        source_file: &Path,
        image: &GiftiImage,
        pointset: &DataArray,
        triangle: &DataArray,
        vertices: &[[f32; 3]],
        domain: &SurfaceDomain,
        bounds: Bounds,
    ) -> Self {
        let metas = [&image.meta, &pointset.meta, &triangle.meta];
        let label = meta_value(
            &metas,
            &[
                "Name",
                "SurfaceName",
                "SurfaceLabel",
                "Label",
                "ObjectLabel",
            ],
        )
        .or_else(|| label_from_path(source_file));
        let mut metadata = Self::from_geometry(
            Some(source_file.to_path_buf()),
            label,
            vertices,
            domain,
            bounds,
            component_dimension(pointset, 3),
            component_dimension(triangle, 3),
        );

        metadata.side = meta_value(
            &metas,
            &[
                "AnatomicalStructurePrimary",
                "AnatomicalStructure",
                "Hemisphere",
                "Side",
            ],
        )
        .and_then(|value| SurfaceSide::from_text(&value))
        .or_else(|| infer_side_from_path(source_file))
        .unwrap_or(SurfaceSide::Unknown);

        metadata.surface_kind = meta_value(
            &metas,
            &[
                "GeometricType",
                "SurfaceType",
                "SurfaceKind",
                "SurfaceState",
                "StateName",
            ],
        )
        .and_then(|value| SurfaceKind::from_text(&value))
        .or_else(|| infer_surface_kind_from_path(source_file))
        .unwrap_or(SurfaceKind::Unknown);

        metadata.group_label = meta_value(
            &metas,
            &["Group", "GroupLabel", "SubjectGroup", "SurfaceGroup"],
        );
        metadata.subject_label = meta_value(
            &metas,
            &[
                "Subject",
                "SubjectID",
                "SubjectId",
                "SubjectLabel",
                "SubjectName",
            ],
        )
        .or_else(|| infer_subject_from_path(source_file));
        metadata.state_name = meta_value(&metas, &["StateName", "State", "SurfaceState"])
            .or_else(|| metadata.surface_kind.state_name());
        metadata.anatomically_correct = meta_value(
            &metas,
            &[
                "AnatomicallyCorrect",
                "AnatomicalCorrect",
                "AnatomicalCorrectness",
                "AnatomicalCorrectFlag",
            ],
        )
        .and_then(|value| AnatomicalCorrectness::from_text(&value))
        .unwrap_or(AnatomicalCorrectness::Unknown);
        metadata.sphere = sphere_from_kind_and_bounds(&metadata.surface_kind, bounds);
        metadata.lineage.update_from_gifti_metadata(
            &metas,
            source_file,
            vertices,
            domain,
            metadata.subject_label.as_deref(),
        );

        metadata
    }

    pub fn kinship_with(&self, other: &Self) -> SurfaceKinship {
        if self.id == other.id {
            SurfaceKinship::SameSurface
        } else {
            self.lineage.domain.kinship_with(&other.lineage.domain)
        }
    }

    pub fn can_share_nodewise_data_with(&self, other: &Self) -> bool {
        matches!(
            self.kinship_with(other),
            SurfaceKinship::SameSurface
                | SurfaceKinship::SameGeometry
                | SurfaceKinship::SameTopology
                | SurfaceKinship::SameStandardNodeCount
        )
    }
}

impl SurfaceLineage {
    fn from_surface(
        source_file: Option<&Path>,
        vertices: &[[f32; 3]],
        domain: &SurfaceDomain,
        subject_label: Option<&str>,
    ) -> Self {
        Self {
            local_domain_parent: None,
            local_curvature_parent: None,
            domain_grandparent: None,
            node_parent: None,
            parent_volume_id: None,
            originator_id: None,
            domain: SurfaceDomainIdentity::from_surface(
                source_file,
                vertices,
                domain,
                subject_label,
            ),
        }
    }

    fn update_from_gifti_metadata(
        &mut self,
        metas: &[&Meta],
        source_file: &Path,
        vertices: &[[f32; 3]],
        domain: &SurfaceDomain,
        subject_label: Option<&str>,
    ) {
        self.local_domain_parent = meta_value(
            metas,
            &[
                "LocalDomainParent",
                "LocalDomainParentID",
                "LocalDomainParentId",
                "DomainParent",
            ],
        );
        self.local_curvature_parent = meta_value(
            metas,
            &[
                "LocalCurvatureParent",
                "LocalCurvatureParentID",
                "LocalCurvatureParentId",
                "CurvatureParent",
            ],
        );
        self.domain_grandparent = meta_value(
            metas,
            &[
                "DomainGrandParent",
                "DomainGrandparent",
                "DomainGrandParentID",
                "DomainGrandparentID",
            ],
        );
        self.node_parent = meta_value(
            metas,
            &[
                "NodeParent",
                "NodeParentID",
                "NodeParentId",
                "ParentNodeSet",
            ],
        );
        self.parent_volume_id = meta_value(
            metas,
            &[
                "ParentVolumeID",
                "ParentVolumeId",
                "ParentVolume",
                "VolPar",
                "VolumeParent",
            ],
        );
        self.originator_id = meta_value(
            metas,
            &["OriginatorID", "OriginatorId", "Originator", "CreatorID"],
        );
        self.domain = SurfaceDomainIdentity::from_gifti_metadata(
            source_file,
            metas,
            vertices,
            domain,
            subject_label,
        );
    }
}

impl SurfaceDomain {
    pub fn from_triangles(node_count: usize, triangles: Vec<[u32; 3]>) -> Result<Self> {
        Self::from_parts(node_count, None, RowToNodeMapping::Dense, triangles)
    }

    pub fn with_node_ids(node_ids: Vec<u32>, triangles: Vec<[u32; 3]>) -> Result<Self> {
        Self::from_parts(
            node_ids.len(),
            Some(node_ids),
            RowToNodeMapping::Dense,
            triangles,
        )
    }

    pub fn from_indexed_rows(
        node_count: usize,
        row_to_node: Vec<u32>,
        triangles: Vec<[u32; 3]>,
    ) -> Result<Self> {
        Self::from_parts(
            node_count,
            None,
            RowToNodeMapping::Indexed(row_to_node),
            triangles,
        )
    }

    fn from_parts(
        node_count: usize,
        node_ids: Option<Vec<u32>>,
        row_to_node: RowToNodeMapping,
        triangles: Vec<[u32; 3]>,
    ) -> Result<Self> {
        validate_triangle_indices(node_count, &triangles)?;

        if let Some(node_ids) = &node_ids {
            ensure!(
                node_ids.len() == node_count,
                "node ID count {} does not match node count {}",
                node_ids.len(),
                node_count
            );
        }

        row_to_node.validate(node_count)?;
        let sorted_nodes = SortedNodeMetadata::from_node_sequence(
            &row_to_node.node_sequence(node_count),
            node_ids.as_deref(),
        );
        let topology_hash = topology_hash(node_count, &triangles);

        Ok(Self {
            id: SurfaceDomainId::from_topology(&topology_hash),
            node_count,
            node_ids,
            row_to_node,
            sorted_nodes,
            triangles,
            topology_hash,
        })
    }

    pub fn row_count(&self) -> usize {
        self.row_to_node.row_count(self.node_count)
    }

    pub fn node_for_row(&self, row: usize) -> Option<u32> {
        self.row_to_node.node_for_row(row, self.node_count)
    }

    pub fn row_for_node(&self, node: u32) -> Option<usize> {
        self.row_to_node.row_for_node(node, self.node_count)
    }

    pub fn contains_node(&self, node: u32) -> bool {
        (node as usize) < self.node_count
    }

    pub fn nodes_for_rows(&self, rows: &[usize]) -> Result<Vec<u32>> {
        rows.iter()
            .map(|row| {
                self.node_for_row(*row).with_context(|| {
                    format!("row {row} is outside domain row count {}", self.row_count())
                })
            })
            .collect()
    }

    pub fn rows_for_nodes(&self, nodes: &[u32]) -> Result<Vec<usize>> {
        nodes
            .iter()
            .map(|node| {
                self.row_for_node(*node).with_context(|| {
                    format!("node {node} is not present in this domain row mapping")
                })
            })
            .collect()
    }

    pub fn node_id_for_node(&self, node: u32) -> Option<u32> {
        if !self.contains_node(node) {
            return None;
        }

        self.node_ids
            .as_ref()
            .map_or(Some(node), |node_ids| node_ids.get(node as usize).copied())
    }

    pub fn node_for_node_id(&self, node_id: u32) -> Option<u32> {
        self.node_ids.as_ref().map_or_else(
            || self.contains_node(node_id).then_some(node_id),
            |node_ids| {
                node_ids
                    .iter()
                    .position(|candidate| *candidate == node_id)
                    .map(|node| node as u32)
            },
        )
    }

    pub fn node_id_for_row(&self, row: usize) -> Option<u32> {
        self.node_for_row(row)
            .and_then(|node| self.node_id_for_node(node))
    }

    pub fn shares_topology_with(&self, other: &Self) -> bool {
        self.topology_hash == other.topology_hash
    }
}

impl RowToNodeMapping {
    fn validate(&self, node_count: usize) -> Result<()> {
        for node in self.node_sequence(node_count) {
            ensure!(
                (node as usize) < node_count,
                "row maps to node {} outside node count {}",
                node,
                node_count
            );
        }

        Ok(())
    }

    fn row_count(&self, node_count: usize) -> usize {
        match self {
            Self::Dense => node_count,
            Self::Indexed(nodes) => nodes.len(),
        }
    }

    fn node_for_row(&self, row: usize, node_count: usize) -> Option<u32> {
        match self {
            Self::Dense => (row < node_count).then_some(row as u32),
            Self::Indexed(nodes) => nodes.get(row).copied(),
        }
    }

    fn row_for_node(&self, node: u32, node_count: usize) -> Option<usize> {
        match self {
            Self::Dense => ((node as usize) < node_count).then_some(node as usize),
            Self::Indexed(nodes) => nodes.iter().position(|candidate| *candidate == node),
        }
    }

    fn node_sequence(&self, node_count: usize) -> Vec<u32> {
        match self {
            Self::Dense => (0..node_count).map(|node| node as u32).collect(),
            Self::Indexed(nodes) => nodes.clone(),
        }
    }
}

impl SortedNodeMetadata {
    fn from_node_sequence(row_nodes: &[u32], node_ids: Option<&[u32]>) -> Self {
        let nodes = node_ids.unwrap_or(row_nodes);
        let mut sorted_nodes = nodes.to_vec();
        sorted_nodes.sort_unstable();

        Self {
            is_sorted: nodes.windows(2).all(|window| window[0] <= window[1]),
            has_duplicates: sorted_nodes.windows(2).any(|window| window[0] == window[1]),
        }
    }
}

impl SurfaceTopology {
    fn from_geometry(vertices: &[[f32; 3]], triangles: &[[u32; 3]]) -> Self {
        let mut member_faces = vec![Vec::new(); vertices.len()];
        let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();

        for (face, triangle) in triangles.iter().copied().enumerate() {
            for node in triangle {
                if let Some(faces) = member_faces.get_mut(node as usize) {
                    faces.push(face);
                }
            }

            for (a, b) in directed_triangle_edges(triangle) {
                edge_faces.entry(edge_key(a, b)).or_default().push(face);
            }
        }

        let mut face_neighbor_sets = vec![BTreeSet::new(); triangles.len()];
        let mut node_neighbor_sets = vec![BTreeSet::new(); vertices.len()];

        for ((a, b), faces) in &edge_faces {
            if (*a as usize) < vertices.len() && (*b as usize) < vertices.len() {
                node_neighbor_sets[*a as usize].insert(*b);
                node_neighbor_sets[*b as usize].insert(*a);
            }

            for (index, face) in faces.iter().copied().enumerate() {
                for neighbor in faces.iter().copied().skip(index + 1) {
                    face_neighbor_sets[face].insert(neighbor);
                    face_neighbor_sets[neighbor].insert(face);
                }
            }
        }

        let face_neighbors = face_neighbor_sets
            .into_iter()
            .map(|neighbors| neighbors.into_iter().collect())
            .collect::<Vec<_>>();
        let node_neighbors = node_neighbor_sets
            .into_iter()
            .map(|neighbors| neighbors.into_iter().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let neighbor_distances = node_neighbors
            .iter()
            .enumerate()
            .map(|(node, neighbors)| {
                neighbors
                    .iter()
                    .copied()
                    .map(|neighbor| {
                        (
                            neighbor,
                            vertex_distance(vertices[node], vertices[neighbor as usize]),
                        )
                    })
                    .collect()
            })
            .collect::<Vec<_>>();

        let mut edge_to_faces = edge_faces
            .into_iter()
            .map(|((a, b), mut faces)| {
                faces.sort_unstable();
                EdgeFaces {
                    edge: EdgeRecord {
                        a,
                        b,
                        length: edge_length(vertices, a, b),
                    },
                    faces,
                }
            })
            .collect::<Vec<_>>();
        edge_to_faces.sort_by_key(|entry| (entry.edge.a, entry.edge.b));

        let edges = edge_to_faces
            .iter()
            .map(|entry| entry.edge)
            .collect::<Vec<_>>();
        let boundary_edges = edge_to_faces
            .iter()
            .filter(|entry| entry.faces.len() == 1)
            .map(|entry| entry.edge)
            .collect();

        Self {
            member_faces,
            face_neighbors,
            node_neighbors,
            neighbor_distances,
            edges,
            boundary_edges,
            edge_to_faces,
        }
    }
}

impl MeshValidationReport {
    fn from_mesh(mesh: &SurfaceMesh) -> Self {
        let mut issues = Vec::new();

        if mesh.vertices.is_empty() {
            issues.push(MeshValidationIssue::EmptyVertices);
        }
        if mesh.domain.triangles.is_empty() {
            issues.push(MeshValidationIssue::EmptyTriangles);
        }

        for (face, triangle) in mesh.domain.triangles.iter().copied().enumerate() {
            for index in triangle {
                if (index as usize) >= mesh.vertices.len() {
                    issues.push(MeshValidationIssue::TriangleIndexOutOfBounds {
                        face,
                        index,
                        vertex_count: mesh.vertices.len(),
                    });
                }
            }
        }

        let mut seen_triangles = HashMap::new();
        for (face, triangle) in mesh.domain.triangles.iter().copied().enumerate() {
            let mut sorted = triangle;
            sorted.sort_unstable();
            if let Some(first) = seen_triangles.insert(sorted, face) {
                issues.push(MeshValidationIssue::DuplicateTriangle {
                    first,
                    duplicate: face,
                });
            }

            let has_repeated_node = triangle[0] == triangle[1]
                || triangle[1] == triangle[2]
                || triangle[0] == triangle[2];
            let has_valid_indices = triangle
                .iter()
                .all(|node| (*node as usize) < mesh.vertices.len());
            let has_zero_area = has_valid_indices
                && face_normal_and_area(&mesh.vertices, triangle).1 <= f32::EPSILON;

            if has_repeated_node || has_zero_area {
                issues.push(MeshValidationIssue::DegenerateTriangle { face });
            }
        }

        let topology = mesh.topology();
        let non_manifold = topology
            .edge_to_faces
            .iter()
            .filter(|entry| entry.faces.len() > 2)
            .collect::<Vec<_>>();
        for entry in non_manifold {
            issues.push(MeshValidationIssue::NonManifoldEdge {
                edge: (entry.edge.a, entry.edge.b),
                face_count: entry.faces.len(),
            });
        }

        if !topology.boundary_edges.is_empty() {
            issues.push(MeshValidationIssue::BoundaryEdges {
                count: topology.boundary_edges.len(),
            });
        }

        let components = connected_component_count(&topology.face_neighbors);
        if components > 1 {
            issues.push(MeshValidationIssue::DisconnectedComponents { count: components });
        }

        let winding = mesh.winding_report();
        if winding.inconsistent_edges > 0 {
            issues.push(MeshValidationIssue::InconsistentWinding {
                inconsistent_edges: winding.inconsistent_edges,
            });
        }

        Self { issues }
    }
}

impl SurfaceDomainIdentity {
    fn from_surface(
        source_file: Option<&Path>,
        vertices: &[[f32; 3]],
        domain: &SurfaceDomain,
        subject_label: Option<&str>,
    ) -> Self {
        let standard_space = source_file.and_then(infer_standard_space_from_path);
        let kind = infer_domain_kind(source_file, standard_space.as_deref(), subject_label);
        let topology_hash = domain.topology_hash.clone();
        let geometry_hash = geometry_hash(vertices);
        let allow_node_count_match = kind.allows_node_count_match();

        Self::new(
            kind,
            standard_space,
            domain.node_count,
            topology_hash,
            geometry_hash,
            allow_node_count_match,
        )
    }

    fn from_gifti_metadata(
        source_file: &Path,
        metas: &[&Meta],
        vertices: &[[f32; 3]],
        domain: &SurfaceDomain,
        subject_label: Option<&str>,
    ) -> Self {
        let standard_space = meta_value(
            metas,
            &[
                "StandardSpace",
                "TemplateSpace",
                "SurfaceSpace",
                "DomainSpace",
                "Space",
            ],
        )
        .map(|space| normalize_standard_space(&space))
        .or_else(|| infer_standard_space_from_path(source_file));
        let kind = meta_value(
            metas,
            &[
                "SurfaceDomainKind",
                "DomainKind",
                "SurfaceDomain",
                "DomainType",
            ],
        )
        .and_then(|value| SurfaceDomainKind::from_text(&value))
        .unwrap_or_else(|| {
            infer_domain_kind(Some(source_file), standard_space.as_deref(), subject_label)
        });
        let allow_node_count_match = meta_value(
            metas,
            &[
                "AllowNodeCountMatch",
                "NodeCountCompatible",
                "StandardNodeCountCompatible",
            ],
        )
        .and_then(|value| parse_bool(&value))
        .unwrap_or_else(|| kind.allows_node_count_match());

        Self::new(
            kind,
            standard_space,
            domain.node_count,
            domain.topology_hash.clone(),
            geometry_hash(vertices),
            allow_node_count_match,
        )
    }

    fn new(
        kind: SurfaceDomainKind,
        standard_space: Option<String>,
        node_count: usize,
        topology_hash: String,
        geometry_hash: String,
        allow_node_count_match: bool,
    ) -> Self {
        Self {
            id: SurfaceDomainId::from_identity(
                kind,
                standard_space.as_deref(),
                node_count,
                &topology_hash,
                allow_node_count_match,
            ),
            kind,
            standard_space,
            node_count,
            topology_hash,
            geometry_hash,
            allow_node_count_match,
        }
    }

    fn kinship_with(&self, other: &Self) -> SurfaceKinship {
        if self.node_count != other.node_count {
            return if self.is_standard_like() || other.is_standard_like() {
                SurfaceKinship::NeedsMapping
            } else {
                SurfaceKinship::Incompatible
            };
        }

        if self.topology_hash == other.topology_hash && self.geometry_hash == other.geometry_hash {
            SurfaceKinship::SameGeometry
        } else if self.topology_hash == other.topology_hash {
            SurfaceKinship::SameTopology
        } else if self.can_match_by_standard_node_count(other) {
            SurfaceKinship::SameStandardNodeCount
        } else if self.kind == SurfaceDomainKind::Unknown
            && other.kind == SurfaceDomainKind::Unknown
        {
            SurfaceKinship::Unknown
        } else {
            SurfaceKinship::NeedsMapping
        }
    }

    fn can_match_by_standard_node_count(&self, other: &Self) -> bool {
        self.node_count == other.node_count
            && (self.allow_node_count_match || other.allow_node_count_match)
            && standard_spaces_are_compatible(
                self.standard_space.as_deref(),
                other.standard_space.as_deref(),
            )
    }

    fn is_standard_like(&self) -> bool {
        self.allow_node_count_match
            || matches!(
                self.kind,
                SurfaceDomainKind::StandardTemplate | SurfaceDomainKind::DerivedFromStandard
            )
    }
}

impl SurfaceDomainId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_topology(topology_hash: &str) -> Self {
        Self(format!("topology:{topology_hash}"))
    }

    fn from_identity(
        kind: SurfaceDomainKind,
        standard_space: Option<&str>,
        node_count: usize,
        topology_hash: &str,
        allow_node_count_match: bool,
    ) -> Self {
        if allow_node_count_match || kind.allows_node_count_match() {
            let space = standard_space.unwrap_or("standard-node-count");
            Self(format!("standard:{space}:{node_count}"))
        } else {
            Self(format!("topology:{topology_hash}"))
        }
    }
}

impl SurfaceDomainKind {
    fn from_text(value: &str) -> Option<Self> {
        match compact_lower(value).as_str() {
            "native" | "nativesubject" | "subjectnative" => Some(Self::NativeSubject),
            "standard" | "template" | "standardtemplate" => Some(Self::StandardTemplate),
            "derivedfromstandard" | "standardderived" | "standardized" => {
                Some(Self::DerivedFromStandard)
            }
            "" | "unknown" => None,
            _ => None,
        }
    }

    fn allows_node_count_match(self) -> bool {
        matches!(self, Self::StandardTemplate | Self::DerivedFromStandard)
    }
}

impl SurfaceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_surface_data(vertices: &[[f32; 3]], triangles: &[[u32; 3]]) -> Self {
        let mut hash = FNV_OFFSET;
        hash_bytes(&mut hash, b"sumaru.surface.v1");

        hash_usize(&mut hash, vertices.len());
        hash_usize(&mut hash, triangles.len());

        for vertex in vertices {
            for value in vertex {
                hash_bytes(&mut hash, &value.to_bits().to_ne_bytes());
            }
        }

        for triangle in triangles {
            for index in triangle {
                hash_bytes(&mut hash, &index.to_ne_bytes());
            }
        }

        Self(format!("surface-{hash:016x}"))
    }
}

impl SurfaceSide {
    fn from_text(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }

        let compact = compact_lower(trimmed);
        if compact.contains("left") || compact == "lh" || compact == "cortexleft" {
            Some(Self::Left)
        } else if compact.contains("right") || compact == "rh" || compact == "cortexright" {
            Some(Self::Right)
        } else if compact.contains("both") || compact.contains("bilateral") || compact == "lr" {
            Some(Self::Both)
        } else {
            Some(Self::Other(trimmed.to_string()))
        }
    }
}

impl SurfaceKind {
    fn from_text(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }

        match compact_lower(trimmed).as_str() {
            "pial" => Some(Self::Pial),
            "white" | "wm" | "whitematter" => Some(Self::WhiteMatter),
            "smoothwm" | "smoothwhite" | "smoothwhitematter" => Some(Self::SmoothWhiteMatter),
            "inflated" => Some(Self::Inflated),
            "veryinflated" => Some(Self::VeryInflated),
            "sphere" | "spherical" => Some(Self::Sphere),
            "flat" | "flattened" => Some(Self::Flat),
            "fiducial" | "fid" => Some(Self::Fiducial),
            "midthickness" | "midgray" => Some(Self::Midthickness),
            "orig" | "original" => Some(Self::Original),
            "anatomical" | "surface" | "unknown" | "unk" | "none" | "n/a" | "na" => None,
            _ => Some(Self::Other(trimmed.to_string())),
        }
    }

    fn state_name(&self) -> Option<String> {
        let name = match self {
            Self::Pial => "pial",
            Self::WhiteMatter => "white",
            Self::SmoothWhiteMatter => "smoothwm",
            Self::Inflated => "inflated",
            Self::VeryInflated => "veryinflated",
            Self::Sphere => "sphere",
            Self::Flat => "flat",
            Self::Fiducial => "fiducial",
            Self::Midthickness => "midthickness",
            Self::Original => "orig",
            Self::Other(value) => value,
            Self::Unknown => return None,
        };

        Some(name.to_string())
    }
}

impl AnatomicalCorrectness {
    fn from_text(value: &str) -> Option<Self> {
        match compact_lower(value).as_str() {
            "true" | "t" | "yes" | "y" | "1" | "correct" | "anatomicallycorrect" => {
                Some(Self::Correct)
            }
            "false" | "f" | "no" | "n" | "0" | "incorrect" | "notcorrect" => Some(Self::Incorrect),
            "" | "unknown" => None,
            _ => None,
        }
    }
}

impl OverlayDataset {
    pub fn from_gifti_path(path: impl AsRef<Path>, vertex_count: usize) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            bail!("{} does not exist", path.display());
        }

        let image = gifti_rs::read(path)
            .with_context(|| format!("failed to read GIFTI overlay {}", path.display()))?;
        let mut values = None;
        for array in image.data_arrays.iter().filter(|array| {
            array.intent != gifti_rs::intent::POINTSET && array.intent != gifti_rs::intent::TRIANGLE
        }) {
            if let Some(candidate) = overlay_values_from_array(array, vertex_count) {
                values = Some(candidate);
                break;
            }
        }

        let values = values.with_context(|| {
            format!(
                "GIFTI overlay must contain one numeric data array with exactly {vertex_count} values"
            )
        })?;
        let range = ValueRange::from_values(&values)?;

        Ok(Self {
            values,
            range,
            threshold_values: None,
        })
    }
}

impl ValueRange {
    pub fn from_values(values: &[f32]) -> Result<Self> {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;

        for value in values.iter().copied().filter(|value| value.is_finite()) {
            min = min.min(value);
            max = max.max(value);
        }

        ensure!(
            min.is_finite() && max.is_finite(),
            "overlay has no finite values"
        );

        Ok(Self { min, max })
    }
}

impl Bounds {
    pub fn from_vertices(vertices: &[[f32; 3]]) -> Result<Self> {
        ensure!(!vertices.is_empty(), "surface has no vertices");

        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);

        for vertex in vertices {
            let vertex = Vec3::from_array(*vertex);
            min = min.min(vertex);
            max = max.max(vertex);
        }

        let center = (min + max) * 0.5;
        let radius = vertices
            .iter()
            .map(|vertex| Vec3::from_array(*vertex).distance(center))
            .fold(0.0, f32::max);

        Ok(Self {
            min: min.to_array(),
            max: max.to_array(),
            center: center.to_array(),
            radius,
        })
    }
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001b3;

fn component_dimension(array: &DataArray, default_dimension: usize) -> usize {
    array.dims.get(1).copied().unwrap_or(default_dimension)
}

fn meta_value(metas: &[&Meta], keys: &[&str]) -> Option<String> {
    for key in keys {
        for meta in metas {
            for (name, value) in *meta {
                if name.eq_ignore_ascii_case(key) {
                    let value = value.trim();
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
        }
    }

    None
}

fn label_from_path(path: &Path) -> Option<String> {
    file_label(path).filter(|label| !label.is_empty())
}

fn infer_side_from_path(path: &Path) -> Option<SurfaceSide> {
    let tokens = path_tokens(path);

    if has_token(&tokens, &["lh", "left"]) {
        Some(SurfaceSide::Left)
    } else if has_token(&tokens, &["rh", "right"]) {
        Some(SurfaceSide::Right)
    } else if has_token(&tokens, &["both", "bilateral", "lr"]) {
        Some(SurfaceSide::Both)
    } else {
        None
    }
}

fn infer_surface_kind_from_path(path: &Path) -> Option<SurfaceKind> {
    let label = file_label(path)?;
    let compact = compact_lower(&label);
    let tokens = path_tokens(path);

    if compact.contains("smoothwm") || token_pair_exists(&tokens, "smooth", "wm") {
        Some(SurfaceKind::SmoothWhiteMatter)
    } else if compact.contains("veryinflated") || token_pair_exists(&tokens, "very", "inflated") {
        Some(SurfaceKind::VeryInflated)
    } else if has_token(&tokens, &["pial"]) {
        Some(SurfaceKind::Pial)
    } else if has_token(&tokens, &["white", "wm"]) {
        Some(SurfaceKind::WhiteMatter)
    } else if has_token(&tokens, &["inflated"]) {
        Some(SurfaceKind::Inflated)
    } else if has_token(&tokens, &["sphere", "spherical"]) {
        Some(SurfaceKind::Sphere)
    } else if has_token(&tokens, &["flat", "flattened"]) {
        Some(SurfaceKind::Flat)
    } else if has_token(&tokens, &["fiducial", "fid"]) {
        Some(SurfaceKind::Fiducial)
    } else if compact.contains("midthickness") || token_pair_exists(&tokens, "mid", "gray") {
        Some(SurfaceKind::Midthickness)
    } else if has_token(&tokens, &["orig", "original"]) {
        Some(SurfaceKind::Original)
    } else {
        None
    }
}

fn infer_subject_from_path(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let lower = text.to_ascii_lowercase();
    let start = lower.find("sub-")?;
    let subject = lower[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>();

    if subject.len() > "sub-".len() {
        Some(subject)
    } else {
        None
    }
}

fn infer_standard_space_from_path(path: &Path) -> Option<String> {
    let tokens = full_path_tokens(path);
    let compact = compact_lower(&path.to_string_lossy());

    for window in tokens.windows(2) {
        if window[0] == "std" && window[1].chars().all(|ch| ch.is_ascii_digit()) {
            return Some(format!("std.{}", window[1]));
        }
    }

    for token in &tokens {
        if token.starts_with("fsaverage") {
            return Some(token.to_string());
        }
    }

    for window in tokens.windows(3) {
        if window[0] == "fs" && window[1] == "lr" && window[2].ends_with('k') {
            return Some(format!("fs_LR_{}", window[2]));
        }
    }

    if compact.contains("fslr32k") {
        Some("fs_LR_32k".to_string())
    } else if compact.contains("fslr59k") {
        Some("fs_LR_59k".to_string())
    } else {
        None
    }
}

fn infer_domain_kind(
    source_file: Option<&Path>,
    standard_space: Option<&str>,
    subject_label: Option<&str>,
) -> SurfaceDomainKind {
    if standard_space.is_some() {
        if subject_label.is_some() {
            SurfaceDomainKind::DerivedFromStandard
        } else {
            SurfaceDomainKind::StandardTemplate
        }
    } else if subject_label.is_some() {
        SurfaceDomainKind::NativeSubject
    } else if source_file.and_then(infer_subject_from_path).is_some() {
        SurfaceDomainKind::NativeSubject
    } else {
        SurfaceDomainKind::Unknown
    }
}

fn normalize_standard_space(space: &str) -> String {
    let trimmed = space.trim();
    let compact = compact_lower(trimmed);

    if let Some(rest) = compact.strip_prefix("std") {
        if !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()) {
            return format!("std.{rest}");
        }
    }

    if compact.starts_with("fslr") {
        let density = compact.trim_start_matches("fslr");
        if !density.is_empty() {
            return format!("fs_LR_{density}");
        }
    }

    trimmed.to_string()
}

fn standard_spaces_are_compatible(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            normalize_standard_space(left) == normalize_standard_space(right)
        }
        _ => true,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match compact_lower(value).as_str() {
        "true" | "t" | "yes" | "y" | "1" => Some(true),
        "false" | "f" | "no" | "n" | "0" => Some(false),
        _ => None,
    }
}

fn sphere_from_kind_and_bounds(kind: &SurfaceKind, bounds: Bounds) -> Option<SphereMetadata> {
    matches!(kind, SurfaceKind::Sphere).then_some(SphereMetadata {
        center: bounds.center,
        radius: bounds.radius,
    })
}

fn file_label(path: &Path) -> Option<String> {
    let mut label = path.file_name()?.to_string_lossy().to_string();
    let lower = label.to_ascii_lowercase();

    for suffix in [
        ".surf.gii.gz",
        ".shape.gii.gz",
        ".func.gii.gz",
        ".label.gii.gz",
        ".gii.gz",
        ".surf.gii",
        ".shape.gii",
        ".func.gii",
        ".label.gii",
        ".gii",
    ] {
        if lower.ends_with(suffix) {
            label.truncate(label.len() - suffix.len());
            break;
        }
    }

    Some(label)
}

fn path_tokens(path: &Path) -> Vec<String> {
    file_label(path)
        .unwrap_or_default()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn full_path_tokens(path: &Path) -> Vec<String> {
    path.to_string_lossy()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn has_token(tokens: &[String], candidates: &[&str]) -> bool {
    tokens
        .iter()
        .any(|token| candidates.iter().any(|candidate| token == candidate))
}

fn token_pair_exists(tokens: &[String], first: &str, second: &str) -> bool {
    tokens
        .windows(2)
        .any(|window| window[0] == first && window[1] == second)
}

fn compact_lower(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn hash_usize(hash: &mut u64, value: usize) {
    hash_bytes(hash, &value.to_ne_bytes());
}

fn topology_hash(vertex_count: usize, triangles: &[[u32; 3]]) -> String {
    let mut hash = FNV_OFFSET;
    hash_bytes(&mut hash, b"sumaru.topology.v1");
    hash_usize(&mut hash, vertex_count);
    hash_usize(&mut hash, triangles.len());

    for triangle in triangles {
        for index in triangle {
            hash_bytes(&mut hash, &index.to_ne_bytes());
        }
    }

    format!("{hash:016x}")
}

fn geometry_hash(vertices: &[[f32; 3]]) -> String {
    let mut hash = FNV_OFFSET;
    hash_bytes(&mut hash, b"sumaru.geometry.v1");
    hash_usize(&mut hash, vertices.len());

    for vertex in vertices {
        for value in vertex {
            hash_bytes(&mut hash, &value.to_bits().to_ne_bytes());
        }
    }

    format!("{hash:016x}")
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn vertices_from_array(array: &DataArray) -> Result<Vec<[f32; 3]>> {
    ensure!(
        array.data.len() % 3 == 0,
        "POINTSET array length is not divisible by 3"
    );

    let vertices = match &array.data {
        ArrayData::Float32(values) => values
            .chunks_exact(3)
            .map(|chunk| [chunk[0], chunk[1], chunk[2]])
            .collect(),
        ArrayData::Float64(values) => values
            .chunks_exact(3)
            .map(|chunk| [chunk[0] as f32, chunk[1] as f32, chunk[2] as f32])
            .collect(),
        _ => bail!("POINTSET array must be Float32 or Float64"),
    };

    Ok(vertices)
}

fn overlay_values_from_array(array: &DataArray, vertex_count: usize) -> Option<Vec<f32>> {
    if array.data.len() != vertex_count {
        return None;
    }

    Some(numeric_values_from_array(array))
}

fn numeric_values_from_array(array: &DataArray) -> Vec<f32> {
    match &array.data {
        ArrayData::UInt8(values) => values.iter().map(|value| *value as f32).collect(),
        ArrayData::Int8(values) => values.iter().map(|value| *value as f32).collect(),
        ArrayData::UInt16(values) => values.iter().map(|value| *value as f32).collect(),
        ArrayData::Int16(values) => values.iter().map(|value| *value as f32).collect(),
        ArrayData::UInt32(values) => values.iter().map(|value| *value as f32).collect(),
        ArrayData::Int32(values) => values.iter().map(|value| *value as f32).collect(),
        ArrayData::UInt64(values) => values.iter().map(|value| *value as f32).collect(),
        ArrayData::Int64(values) => values.iter().map(|value| *value as f32).collect(),
        ArrayData::Float32(values) => values.clone(),
        ArrayData::Float64(values) => values.iter().map(|value| *value as f32).collect(),
    }
}

fn triangles_from_array(array: &DataArray) -> Result<Vec<[u32; 3]>> {
    ensure!(
        array.data.len() % 3 == 0,
        "TRIANGLE array length is not divisible by 3"
    );

    match &array.data {
        ArrayData::UInt8(values) => triplets_from_indices(values.iter().copied()),
        ArrayData::UInt16(values) => triplets_from_indices(values.iter().copied()),
        ArrayData::UInt32(values) => triplets_from_indices(values.iter().copied()),
        ArrayData::UInt64(values) => triplets_from_indices(values.iter().copied()),
        ArrayData::Int8(values) => triplets_from_indices(values.iter().copied()),
        ArrayData::Int16(values) => triplets_from_indices(values.iter().copied()),
        ArrayData::Int32(values) => triplets_from_indices(values.iter().copied()),
        ArrayData::Int64(values) => triplets_from_indices(values.iter().copied()),
        _ => bail!("TRIANGLE array must contain integer indices"),
    }
}

fn triplets_from_indices<T>(values: impl Iterator<Item = T>) -> Result<Vec<[u32; 3]>>
where
    T: TryInto<u32>,
{
    let indices = values
        .map(|value| {
            value
                .try_into()
                .map_err(|_| anyhow::anyhow!("triangle index out of range"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(indices
        .chunks_exact(3)
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect())
}

fn face_normal_and_area(vertices: &[[f32; 3]], triangle: [u32; 3]) -> ([f32; 3], f32) {
    let [a, b, c] = triangle;
    let a = Vec3::from_array(vertices[a as usize]);
    let b = Vec3::from_array(vertices[b as usize]);
    let c = Vec3::from_array(vertices[c as usize]);
    let cross = (b - a).cross(c - a);
    let cross_length = cross.length();

    if cross_length <= f32::EPSILON {
        ([0.0, 0.0, 0.0], 0.0)
    } else {
        ((cross / cross_length).to_array(), cross_length * 0.5)
    }
}

fn edge_length(vertices: &[[f32; 3]], a: u32, b: u32) -> f32 {
    if (a as usize) >= vertices.len() || (b as usize) >= vertices.len() {
        return f32::NAN;
    }

    vertex_distance(vertices[a as usize], vertices[b as usize])
}

fn vertex_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    Vec3::from_array(a).distance(Vec3::from_array(b))
}

fn connected_component_count(adjacency: &[Vec<usize>]) -> usize {
    let mut seen = vec![false; adjacency.len()];
    let mut components = 0;

    for seed in 0..adjacency.len() {
        if seen[seed] {
            continue;
        }

        components += 1;
        seen[seed] = true;
        let mut queue = VecDeque::from([seed]);
        while let Some(face) = queue.pop_front() {
            for neighbor in &adjacency[face] {
                if !seen[*neighbor] {
                    seen[*neighbor] = true;
                    queue.push_back(*neighbor);
                }
            }
        }
    }

    components
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct QueueState {
    node: u32,
    cost: f32,
}

impl Eq for QueueState {}

impl Ord for QueueState {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
            .then_with(|| self.node.cmp(&other.node))
    }
}

impl PartialOrd for QueueState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn shortest_path_tree(topology: &SurfaceTopology, seed: u32) -> (Vec<f32>, Vec<Option<u32>>) {
    let mut distances = vec![f32::INFINITY; topology.node_neighbors.len()];
    let mut previous = vec![None; topology.node_neighbors.len()];
    let mut queue = BinaryHeap::new();

    distances[seed as usize] = 0.0;
    queue.push(QueueState {
        node: seed,
        cost: 0.0,
    });

    while let Some(QueueState { node, cost }) = queue.pop() {
        if cost > distances[node as usize] {
            continue;
        }

        for (neighbor, edge_length) in &topology.neighbor_distances[node as usize] {
            let next_cost = cost + *edge_length;
            if next_cost < distances[*neighbor as usize] {
                distances[*neighbor as usize] = next_cost;
                previous[*neighbor as usize] = Some(node);
                queue.push(QueueState {
                    node: *neighbor,
                    cost: next_cost,
                });
            }
        }
    }

    (distances, previous)
}

fn edge_record_between(topology: &SurfaceTopology, a: u32, b: u32) -> Option<EdgeRecord> {
    let (a, b) = edge_key(a, b);
    topology
        .edges
        .iter()
        .find(|edge| edge.a == a && edge.b == b)
        .copied()
}

fn triangle_plane_intersection(
    vertices: &[[f32; 3]],
    triangle: [u32; 3],
    plane: ClipPlane,
) -> Option<LineSegment> {
    let points = triangle.map(|node| vertices[node as usize]);
    let distances = points.map(|point| plane.signed_distance(point));
    let mut intersections = Vec::new();

    for edge in [(0, 1), (1, 2), (2, 0)] {
        let a = edge.0;
        let b = edge.1;
        let da = distances[a];
        let db = distances[b];

        if da.abs() <= f32::EPSILON {
            push_unique_point(&mut intersections, points[a]);
        }
        if da * db < 0.0 {
            let t = da / (da - db);
            let pa = Vec3::from_array(points[a]);
            let pb = Vec3::from_array(points[b]);
            push_unique_point(&mut intersections, pa.lerp(pb, t).to_array());
        }
    }

    (intersections.len() >= 2).then_some(LineSegment {
        start: intersections[0],
        end: intersections[1],
    })
}

fn push_unique_point(points: &mut Vec<[f32; 3]>, point: [f32; 3]) {
    if !points
        .iter()
        .any(|existing| vertex_distance(*existing, point) <= 1e-5)
    {
        points.push(point);
    }
}

fn closest_triangle_weights(
    vertices: &[[f32; 3]],
    triangles: &[[u32; 3]],
    target: [f32; 3],
) -> NodeWeights {
    let target_vec = Vec3::from_array(target);
    let triangle = triangles
        .iter()
        .copied()
        .min_by(|left, right| {
            let left_distance = triangle_centroid(vertices, *left).distance_squared(target_vec);
            let right_distance = triangle_centroid(vertices, *right).distance_squared(target_vec);
            left_distance
                .partial_cmp(&right_distance)
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or([0, 0, 0]);
    let [a, b, c] = triangle;
    let weights = barycentric_weights(
        Vec3::from_array(vertices[a as usize]),
        Vec3::from_array(vertices[b as usize]),
        Vec3::from_array(vertices[c as usize]),
        target_vec,
    );

    NodeWeights {
        weights: vec![(a, weights[0]), (b, weights[1]), (c, weights[2])],
    }
}

fn triangle_centroid(vertices: &[[f32; 3]], triangle: [u32; 3]) -> Vec3 {
    let [a, b, c] = triangle;
    (Vec3::from_array(vertices[a as usize])
        + Vec3::from_array(vertices[b as usize])
        + Vec3::from_array(vertices[c as usize]))
        / 3.0
}

fn barycentric_weights(a: Vec3, b: Vec3, c: Vec3, p: Vec3) -> [f32; 3] {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let d00 = v0.dot(v0);
    let d01 = v0.dot(v1);
    let d11 = v1.dot(v1);
    let d20 = v2.dot(v0);
    let d21 = v2.dot(v1);
    let denominator = d00 * d11 - d01 * d01;

    if denominator.abs() <= f32::EPSILON {
        return [1.0, 0.0, 0.0];
    }

    let v = (d11 * d20 - d01 * d21) / denominator;
    let w = (d00 * d21 - d01 * d20) / denominator;
    let u = 1.0 - v - w;
    normalize_weights([u.max(0.0), v.max(0.0), w.max(0.0)])
}

fn normalize_weights(weights: [f32; 3]) -> [f32; 3] {
    let sum = weights[0] + weights[1] + weights[2];
    if sum <= f32::EPSILON {
        [1.0, 0.0, 0.0]
    } else {
        [weights[0] / sum, weights[1] / sum, weights[2] / sum]
    }
}

#[derive(Debug, Clone)]
struct WindingAnalysis {
    report: WindingReport,
    face_flips: Vec<bool>,
    component_ids: Vec<usize>,
    component_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct EdgeUse {
    face: usize,
    start: u32,
    end: u32,
}

type EdgeKey = (u32, u32);

impl WindingAnalysis {
    fn new(vertices: &[[f32; 3]], triangles: &[[u32; 3]]) -> Self {
        let edge_map = build_edge_map(triangles);
        let adjacency = build_winding_adjacency(triangles.len(), &edge_map);
        let (face_flips, component_ids, component_count) =
            assign_consistent_face_flips(triangles.len(), &adjacency);

        let boundary_edges = edge_map.values().filter(|uses| uses.len() == 1).count();
        let non_manifold_edges = edge_map.values().filter(|uses| uses.len() > 2).count();
        let inconsistent_edges = count_inconsistent_edges(&edge_map, &face_flips);
        let faces_to_flip_for_consistency = face_flips.iter().filter(|flip| **flip).count();
        let globally_orientable = inconsistent_edges == 0 && non_manifold_edges == 0;

        let (normal_direction, signed_volume) =
            if globally_orientable && boundary_edges == 0 && component_count > 0 {
                normal_direction_from_component_volumes(&component_signed_volumes(
                    vertices,
                    triangles,
                    &component_ids,
                ))
            } else {
                (NormalDirection::Unknown, None)
            };

        let report = WindingReport {
            components: component_count,
            faces_to_flip_for_consistency,
            boundary_edges,
            non_manifold_edges,
            inconsistent_edges,
            globally_orientable,
            normal_direction,
            signed_volume,
        };

        Self {
            report,
            face_flips,
            component_ids,
            component_count,
        }
    }
}

fn build_edge_map(triangles: &[[u32; 3]]) -> HashMap<EdgeKey, Vec<EdgeUse>> {
    let mut edge_map: HashMap<EdgeKey, Vec<EdgeUse>> = HashMap::new();

    for (face, triangle) in triangles.iter().copied().enumerate() {
        for (start, end) in directed_triangle_edges(triangle) {
            edge_map
                .entry(edge_key(start, end))
                .or_default()
                .push(EdgeUse { face, start, end });
        }
    }

    edge_map
}

fn build_winding_adjacency(
    face_count: usize,
    edge_map: &HashMap<EdgeKey, Vec<EdgeUse>>,
) -> Vec<Vec<(usize, bool)>> {
    let mut adjacency = vec![Vec::new(); face_count];

    for uses in edge_map.values().filter(|uses| uses.len() == 2) {
        let first = uses[0];
        let second = uses[1];
        let same_direction = first.start == second.start && first.end == second.end;
        adjacency[first.face].push((second.face, same_direction));
        adjacency[second.face].push((first.face, same_direction));
    }

    adjacency
}

fn assign_consistent_face_flips(
    face_count: usize,
    adjacency: &[Vec<(usize, bool)>],
) -> (Vec<bool>, Vec<usize>, usize) {
    let mut face_flips = vec![None; face_count];
    let mut component_ids = vec![usize::MAX; face_count];
    let mut component_count = 0;

    for seed in 0..face_count {
        if face_flips[seed].is_some() {
            continue;
        }

        face_flips[seed] = Some(false);
        component_ids[seed] = component_count;

        let mut queue = VecDeque::from([seed]);
        while let Some(face) = queue.pop_front() {
            let face_flip = face_flips[face].unwrap_or(false);
            for (neighbor, same_direction) in &adjacency[face] {
                let required_flip = face_flip ^ *same_direction;
                if face_flips[*neighbor].is_none() {
                    face_flips[*neighbor] = Some(required_flip);
                    component_ids[*neighbor] = component_count;
                    queue.push_back(*neighbor);
                }
            }
        }

        component_count += 1;
    }

    (
        face_flips
            .into_iter()
            .map(|flip| flip.unwrap_or(false))
            .collect(),
        component_ids,
        component_count,
    )
}

fn count_inconsistent_edges(
    edge_map: &HashMap<EdgeKey, Vec<EdgeUse>>,
    face_flips: &[bool],
) -> usize {
    edge_map
        .values()
        .filter(|uses| uses.len() == 2)
        .filter(|uses| {
            let first = uses[0];
            let second = uses[1];
            let same_direction = first.start == second.start && first.end == second.end;
            same_direction ^ face_flips[first.face] ^ face_flips[second.face]
        })
        .count()
}

fn component_signed_volumes(
    vertices: &[[f32; 3]],
    triangles: &[[u32; 3]],
    component_ids: &[usize],
) -> Vec<f32> {
    let component_count = component_ids
        .iter()
        .copied()
        .filter(|component| *component != usize::MAX)
        .max()
        .map_or(0, |component| component + 1);
    let mut volumes = vec![0.0; component_count];

    for (triangle, component) in triangles.iter().copied().zip(component_ids.iter().copied()) {
        if component != usize::MAX {
            volumes[component] += triangle_signed_volume(vertices, triangle);
        }
    }

    volumes
}

fn normal_direction_from_component_volumes(volumes: &[f32]) -> (NormalDirection, Option<f32>) {
    if volumes.is_empty() {
        return (NormalDirection::Unknown, None);
    }

    let signed_volume: f32 = volumes.iter().sum();
    if volumes.iter().any(|volume| volume.abs() <= f32::EPSILON) {
        return (NormalDirection::Unknown, Some(signed_volume));
    }

    let has_positive = volumes.iter().any(|volume| *volume > 0.0);
    let has_negative = volumes.iter().any(|volume| *volume < 0.0);
    let direction = match (has_positive, has_negative) {
        (true, false) => NormalDirection::Outward,
        (false, true) => NormalDirection::Inward,
        (true, true) => NormalDirection::Mixed,
        (false, false) => NormalDirection::Unknown,
    };

    (direction, Some(signed_volume))
}

fn triangle_signed_volume(vertices: &[[f32; 3]], triangle: [u32; 3]) -> f32 {
    let [a, b, c] = triangle;
    let a = Vec3::from_array(vertices[a as usize]);
    let b = Vec3::from_array(vertices[b as usize]);
    let c = Vec3::from_array(vertices[c as usize]);

    a.dot(b.cross(c)) / 6.0
}

fn apply_face_flips(triangles: &[[u32; 3]], face_flips: &[bool]) -> Vec<[u32; 3]> {
    triangles
        .iter()
        .copied()
        .zip(face_flips.iter().copied())
        .map(|(triangle, flip)| {
            if flip {
                flip_triangle(triangle)
            } else {
                triangle
            }
        })
        .collect()
}

fn flip_triangles(triangles: &[[u32; 3]]) -> Vec<[u32; 3]> {
    triangles.iter().copied().map(flip_triangle).collect()
}

fn flip_triangle([a, b, c]: [u32; 3]) -> [u32; 3] {
    [a, c, b]
}

fn directed_triangle_edges([a, b, c]: [u32; 3]) -> [(u32, u32); 3] {
    [(a, b), (b, c), (c, a)]
}

fn edge_key(a: u32, b: u32) -> EdgeKey {
    if a <= b { (a, b) } else { (b, a) }
}

fn validate_triangle_indices(vertex_count: usize, triangles: &[[u32; 3]]) -> Result<()> {
    ensure!(!triangles.is_empty(), "surface has no triangles");

    for triangle in triangles {
        for index in triangle {
            ensure!(
                (*index as usize) < vertex_count,
                "triangle index {} is outside vertex count {}",
                index,
                vertex_count
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        AnatomicalCorrectness, Bounds, ClipPlane, FaceMask, FaceMaskMode, MeshValidationIssue,
        NodeMask, NormalDirection, RowToNodeMapping, SmoothingWeights, SurfaceDomain,
        SurfaceDomainKind, SurfaceKind, SurfaceKinship, SurfaceMappingKind, SurfaceMesh,
        SurfaceSide, SurfaceTransform, ValueRange, VolumeSpace, VoxelIndex,
        infer_surface_kind_from_path,
    };

    #[test]
    fn bounds_capture_center_and_radius() {
        let bounds = Bounds::from_vertices(&[[-1.0, -2.0, 0.0], [3.0, 2.0, 0.0]]).unwrap();

        assert_eq!(bounds.min, [-1.0, -2.0, 0.0]);
        assert_eq!(bounds.max, [3.0, 2.0, 0.0]);
        assert_eq!(bounds.center, [1.0, 0.0, 0.0]);
        assert!(bounds.radius > 2.82 && bounds.radius < 2.83);
    }

    #[test]
    fn vertex_normals_average_adjacent_face_normals() {
        let vertices = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = SurfaceMesh::new(vertices, vec![[0, 1, 2]]).unwrap();

        let normals = mesh.vertex_normals();

        assert_eq!(normals.len(), 3);
        for normal in normals {
            assert_eq!(normal, [0.0, 0.0, 1.0]);
        }
    }

    #[test]
    fn geometry_metrics_measure_single_right_triangle() {
        let mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();

        let metrics = mesh.geometry_metrics();

        assert_eq!(metrics.face_normals.len(), 1);
        assert_vec3_close(metrics.face_normals[0], [0.0, 0.0, 1.0]);
        assert_close(metrics.face_areas[0], 0.5);
        assert_close(metrics.total_area, 0.5);
        for area in metrics.node_areas {
            assert_close(area, 1.0 / 6.0);
        }
        assert_vec3_close(mesh.face_normals()[0], [0.0, 0.0, 1.0]);
        assert_close(mesh.face_areas()[0], 0.5);
        assert_close(mesh.total_area(), 0.5);
    }

    #[test]
    fn geometry_metrics_distribute_square_area_to_nodes() {
        let mesh = SurfaceMesh::new(four_vertices(), vec![[0, 1, 2], [0, 2, 3]]).unwrap();

        let metrics = mesh.geometry_metrics();

        assert_eq!(metrics.face_normals.len(), 2);
        assert_eq!(metrics.face_areas.len(), 2);
        assert_close(metrics.face_areas[0], 0.5);
        assert_close(metrics.face_areas[1], 0.5);
        assert_close(metrics.total_area, 1.0);
        assert_close(metrics.node_areas.iter().sum(), metrics.total_area);
        assert_close(metrics.node_areas[0], 1.0 / 3.0);
        assert_close(metrics.node_areas[1], 1.0 / 6.0);
        assert_close(metrics.node_areas[2], 1.0 / 3.0);
        assert_close(metrics.node_areas[3], 1.0 / 6.0);
        assert_eq!(mesh.node_areas(), metrics.node_areas);
    }

    #[test]
    fn geometry_metrics_treat_degenerate_triangles_as_zero_area() {
        let mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();

        let metrics = mesh.geometry_metrics();

        assert_eq!(metrics.face_normals[0], [0.0, 0.0, 0.0]);
        assert_eq!(metrics.face_areas[0], 0.0);
        assert_eq!(metrics.node_areas, vec![0.0, 0.0, 0.0]);
        assert_eq!(metrics.total_area, 0.0);
    }

    #[test]
    fn winding_report_recognizes_consistent_open_patch() {
        let mesh = SurfaceMesh::new(four_vertices(), vec![[0, 1, 2], [0, 2, 3]]).unwrap();

        let report = mesh.winding_report();

        assert_eq!(report.components, 1);
        assert_eq!(report.faces_to_flip_for_consistency, 0);
        assert_eq!(report.boundary_edges, 4);
        assert_eq!(report.non_manifold_edges, 0);
        assert_eq!(report.inconsistent_edges, 0);
        assert!(report.globally_orientable);
        assert_eq!(report.normal_direction, NormalDirection::Unknown);
        assert_eq!(report.signed_volume, None);
        assert_eq!(
            mesh.triangles_with_consistent_winding(),
            vec![[0, 1, 2], [0, 2, 3]]
        );
        assert_eq!(mesh.triangles_oriented_outward(), None);
    }

    #[test]
    fn winding_utilities_flip_neighbor_faces_for_local_consistency() {
        let mesh = SurfaceMesh::new(four_vertices(), vec![[0, 1, 2], [0, 3, 2]]).unwrap();

        let report = mesh.winding_report();

        assert_eq!(report.components, 1);
        assert_eq!(report.faces_to_flip_for_consistency, 1);
        assert_eq!(report.inconsistent_edges, 0);
        assert_eq!(
            mesh.triangles_with_consistent_winding(),
            vec![[0, 1, 2], [0, 2, 3]]
        );
        assert_eq!(mesh.flipped_winding(), vec![[0, 2, 1], [0, 2, 3]]);
    }

    #[test]
    fn winding_report_detects_closed_outward_and_inward_direction() {
        let outward = SurfaceMesh::new(tetra_vertices(), outward_tetra_triangles()).unwrap();
        let inward = SurfaceMesh::new(tetra_vertices(), outward.flipped_winding()).unwrap();

        let outward_report = outward.winding_report();
        assert_eq!(outward_report.boundary_edges, 0);
        assert_eq!(outward_report.non_manifold_edges, 0);
        assert_eq!(outward_report.normal_direction, NormalDirection::Outward);
        assert!(outward_report.signed_volume.unwrap() > 0.0);
        assert_eq!(
            outward.triangles_oriented_outward().unwrap(),
            outward_tetra_triangles()
        );

        let inward_report = inward.winding_report();
        assert_eq!(inward_report.normal_direction, NormalDirection::Inward);
        assert!(inward_report.signed_volume.unwrap() < 0.0);
        assert_eq!(
            inward.triangles_oriented_outward().unwrap(),
            outward_tetra_triangles()
        );
    }

    #[test]
    fn winding_report_marks_non_manifold_edges_as_not_globally_orientable() {
        let mesh = SurfaceMesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 0.0],
            ],
            vec![[0, 1, 2], [1, 0, 3], [0, 1, 4]],
        )
        .unwrap();

        let report = mesh.winding_report();

        assert_eq!(report.non_manifold_edges, 1);
        assert!(!report.globally_orientable);
        assert_eq!(report.normal_direction, NormalDirection::Unknown);
        assert_eq!(mesh.triangles_oriented_outward(), None);
    }

    #[test]
    fn topology_cache_tracks_member_faces_neighbors_edges_and_distances() {
        let mesh = SurfaceMesh::new(four_vertices(), vec![[0, 1, 2], [0, 2, 3]]).unwrap();

        let topology = mesh.topology();

        assert_eq!(topology.member_faces[0], vec![0, 1]);
        assert_eq!(topology.member_faces[1], vec![0]);
        assert_eq!(topology.member_faces[2], vec![0, 1]);
        assert_eq!(topology.member_faces[3], vec![1]);
        assert_eq!(topology.face_neighbors, vec![vec![1], vec![0]]);
        assert_eq!(topology.node_neighbors[0], vec![1, 2, 3]);
        assert_eq!(topology.node_neighbors[1], vec![0, 2]);
        assert_eq!(topology.edges.len(), 5);
        assert_eq!(topology.boundary_edges.len(), 4);

        let shared_edge = topology
            .edge_to_faces
            .iter()
            .find(|entry| (entry.edge.a, entry.edge.b) == (0, 2))
            .unwrap();
        assert_eq!(shared_edge.faces, vec![0, 1]);

        let distance_to_2 = topology.neighbor_distances[0]
            .iter()
            .find(|(node, _)| *node == 2)
            .map(|(_, distance)| *distance)
            .unwrap();
        assert_close(distance_to_2, 2.0_f32.sqrt());
    }

    #[test]
    fn mesh_validation_reports_duplicate_degenerate_boundary_and_nonmanifold_issues() {
        let mesh = SurfaceMesh::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 0.0],
            ],
            vec![[0, 1, 2], [1, 0, 3], [0, 1, 4], [0, 1, 2], [0, 0, 1]],
        )
        .unwrap();

        let report = mesh.validation_report();

        assert!(!report.is_valid());
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, MeshValidationIssue::DuplicateTriangle { .. }))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, MeshValidationIssue::DegenerateTriangle { .. }))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, MeshValidationIssue::BoundaryEdges { .. }))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, MeshValidationIssue::NonManifoldEdge { .. }))
        );
    }

    #[test]
    fn surface_domain_lookup_maps_rows_nodes_and_external_node_ids() {
        let domain = SurfaceDomain::from_indexed_rows(5, vec![4, 1, 3], vec![[0, 1, 2]]).unwrap();

        assert_eq!(domain.nodes_for_rows(&[0, 2]).unwrap(), vec![4, 3]);
        assert_eq!(domain.rows_for_nodes(&[1, 4]).unwrap(), vec![1, 0]);
        assert_eq!(domain.node_id_for_node(3), Some(3));
        assert_eq!(domain.node_for_node_id(4), Some(4));
        assert!(domain.rows_for_nodes(&[2]).is_err());

        let external = SurfaceDomain::with_node_ids(vec![10, 20, 30], vec![[0, 1, 2]]).unwrap();
        assert_eq!(external.node_id_for_node(1), Some(20));
        assert_eq!(external.node_for_node_id(30), Some(2));
        assert_eq!(external.node_id_for_row(2), Some(30));
        assert_eq!(external.node_for_node_id(2), None);
    }

    #[test]
    fn node_and_face_masks_compose_and_extract_surface_patches() {
        let mesh = SurfaceMesh::new(four_vertices(), vec![[0, 1, 2], [0, 2, 3]]).unwrap();
        let first = NodeMask::from_nodes(4, [0, 1, 2]).unwrap();
        let second = NodeMask::from_nodes(4, [2, 3]).unwrap();

        assert_eq!(first.union(&second).unwrap().nodes(), vec![0, 1, 2, 3]);
        assert_eq!(first.intersection(&second).unwrap().nodes(), vec![2]);
        assert_eq!(first.difference(&second).unwrap().nodes(), vec![0, 1]);
        assert_eq!(first.invert().nodes(), vec![3]);

        let all_node_faces = mesh
            .face_mask_from_node_mask(&first, FaceMaskMode::AllNodes)
            .unwrap();
        assert_eq!(all_node_faces.faces(), vec![0]);

        let any_node_faces = mesh
            .face_mask_from_node_mask(&second, FaceMaskMode::AnyNode)
            .unwrap();
        assert_eq!(any_node_faces.faces(), vec![0, 1]);

        let patch = mesh.patch_from_face_mask(&all_node_faces).unwrap();
        assert_eq!(patch.faces, vec![0]);
        assert_eq!(patch.nodes, vec![0, 1, 2]);
        assert_eq!(patch.bounds.min, [0.0, 0.0, 0.0]);
        assert_eq!(patch.bounds.max, [1.0, 1.0, 0.0]);

        let first_face = FaceMask::from_faces(2, [0]).unwrap();
        let second_face = FaceMask::from_faces(2, [1]).unwrap();
        assert_eq!(first_face.union(&second_face).unwrap().faces(), vec![0, 1]);
        assert_eq!(
            FaceMask::all(2).difference(&first_face).unwrap().faces(),
            vec![1]
        );
    }

    #[test]
    fn surface_paths_contours_fill_and_neighborhoods_follow_mesh_topology() {
        let mesh = SurfaceMesh::new(four_vertices(), vec![[0, 1, 2], [0, 2, 3]]).unwrap();

        let edge_path = mesh.edge_path_from_nodes(&[1, 0, 3]).unwrap();
        assert_eq!(
            edge_path
                .iter()
                .map(|edge| (edge.a, edge.b))
                .collect::<Vec<_>>(),
            vec![(0, 1), (0, 3)]
        );
        assert_close(mesh.node_path_length(&[1, 0, 3]).unwrap(), 2.0);
        assert!(mesh.node_path_length(&[1, 3]).is_err());
        assert_eq!(
            mesh.triangle_path_from_node_path(&[1, 0, 3]).unwrap(),
            vec![0, 1]
        );

        assert_eq!(mesh.faces_touching_nodes(&[1]).unwrap(), vec![0]);

        let node_mask = NodeMask::from_nodes(4, [0, 1, 2]).unwrap();
        let contours = mesh.contour_edges_for_node_mask(&node_mask).unwrap();
        assert_eq!(
            contours
                .iter()
                .map(|edge| (edge.a, edge.b))
                .collect::<Vec<_>>(),
            vec![(0, 3), (2, 3)]
        );

        let face_mask = FaceMask::from_faces(2, [0]).unwrap();
        assert_eq!(
            mesh.node_mask_from_face_mask(&face_mask).unwrap().nodes(),
            vec![0, 1, 2]
        );

        let shortest = mesh.shortest_node_path(1, 3).unwrap().unwrap();
        assert_eq!(shortest.nodes.first().copied(), Some(1));
        assert_eq!(shortest.nodes.last().copied(), Some(3));
        assert_close(shortest.length, 2.0);
        assert_eq!(
            mesh.shortest_node_path(2, 2).unwrap().unwrap().nodes,
            vec![2]
        );

        assert_eq!(mesh.k_ring_neighborhood(0, 0).unwrap().nodes(), vec![0]);
        assert_eq!(
            mesh.k_ring_neighborhood(0, 1).unwrap().nodes(),
            vec![0, 1, 2, 3]
        );

        assert_eq!(
            mesh.distance_limited_neighborhood(0, 1.0)
                .unwrap()
                .into_iter()
                .map(|(node, _)| node)
                .collect::<Vec<_>>(),
            vec![0, 1, 3]
        );
        assert_eq!(
            mesh.spherical_neighborhood(0, 1.0).unwrap().nodes(),
            vec![0, 1, 3]
        );
    }

    #[test]
    fn shape_metrics_smoothing_and_transforms_are_surface_primitives() {
        let mesh = SurfaceMesh::new(four_vertices(), vec![[0, 1, 2], [0, 2, 3]]).unwrap();

        let shape = mesh.shape_metrics_with_parent(Some(mesh.metadata.id.clone()));
        assert_eq!(shape.parent_surface_id, Some(mesh.metadata.id.clone()));
        assert_eq!(shape.convexity.len(), 4);
        assert_eq!(shape.mean_curvature_like.len(), 4);
        assert!(shape.convexity.iter().all(|value| value.is_finite()));
        assert!(
            shape
                .mean_curvature_like
                .iter()
                .all(|value| value.is_finite())
        );

        let smoothed = mesh
            .smooth_scalar_values(&[0.0, 3.0, 0.0, 0.0], 1, SmoothingWeights::Uniform, None)
            .unwrap();
        assert_close(smoothed[0], 0.75);
        assert_close(smoothed[1], 1.0);

        let mask = NodeMask::from_nodes(4, [0, 1]).unwrap();
        let masked = mesh
            .smooth_scalar_values(
                &[0.0, 3.0, 0.0, 0.0],
                1,
                SmoothingWeights::Uniform,
                Some(&mask),
            )
            .unwrap();
        assert_close(masked[0], 1.5);
        assert_close(masked[1], 1.5);
        assert_close(masked[2], 0.0);
        assert_close(masked[3], 0.0);

        let smoothed_vertices = mesh.smooth_vertices(1, 1.0, None).unwrap();
        assert_close(smoothed_vertices[0][0], 2.0 / 3.0);
        assert_close(smoothed_vertices[0][1], 2.0 / 3.0);

        let transform = SurfaceTransform::translation([1.0, 0.0, 0.0])
            .then(SurfaceTransform::scale([2.0, 2.0, 2.0]));
        assert_eq!(transform.transform_point([1.0, 1.0, 1.0]), [4.0, 2.0, 2.0]);
        assert_eq!(transform.transform_vector([1.0, 1.0, 1.0]), [2.0, 2.0, 2.0]);
        assert_eq!(
            mesh.transformed_vertices(SurfaceTransform::translation([1.0, 0.0, 0.0]))[0],
            [1.0, 0.0, 0.0]
        );
    }

    #[test]
    fn suma_convexity_is_zero_on_a_flat_surface() {
        let mesh = SurfaceMesh::new(four_vertices(), vec![[0, 1, 2], [0, 2, 3]]).unwrap();
        let convexity = mesh.suma_convexity();

        assert_eq!(convexity.len(), 4);
        for value in convexity {
            assert_close(value, 0.0);
        }
    }

    #[test]
    fn volume_clipping_and_mapping_primitives_cover_phase_two_hooks() {
        let mesh = SurfaceMesh::new(four_vertices(), vec![[0, 1, 2], [0, 2, 3]]).unwrap();
        let volume = VolumeSpace::new([4, 4, 4], SurfaceTransform::identity()).unwrap();

        assert_eq!(volume.voxel_to_world([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
        assert_eq!(volume.world_to_voxel([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
        assert_eq!(
            volume.world_to_voxel_index([1.2, 2.1, 0.0]),
            Some(VoxelIndex { i: 1, j: 2, k: 0 })
        );
        assert_eq!(mesh.nearest_node_to_world([0.1, 0.0, 0.0]).unwrap().0, 0);
        assert_eq!(
            mesh.nearest_node_to_voxel(&volume, [1.0, 1.0, 0.0])
                .unwrap()
                .0,
            2
        );
        assert_close(mesh.voxel_distance(&volume, [0.0, 0.0, 0.0]).unwrap(), 0.0);
        assert_eq!(mesh.surface_voxels(&volume).len(), 4);
        assert_eq!(mesh.volume_sample_points(&volume).len(), 4);

        let plane = ClipPlane::from_point_normal([0.5, 0.0, 0.0], [1.0, 0.0, 0.0]).unwrap();
        assert_eq!(
            mesh.node_mask_for_clip_plane(plane, true).unwrap().nodes(),
            vec![1, 2]
        );
        assert_eq!(
            mesh.face_mask_for_clip_plane(plane, true, FaceMaskMode::AnyNode)
                .unwrap()
                .faces(),
            vec![0, 1]
        );
        assert_eq!(mesh.plane_intersections(plane).len(), 2);

        let same = SurfaceMesh::new(
            vec![
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
        .unwrap();
        let same_map = mesh.nodewise_mapping_to(&same);
        assert_eq!(same_map.kind, SurfaceMappingKind::SameTopology);
        assert_eq!(
            same_map.transfer_f32(&[1.0, 2.0, 3.0, 4.0]).unwrap(),
            vec![1.0, 2.0, 3.0, 4.0]
        );

        let target = SurfaceMesh::new(
            vec![[0.1, 0.0, 0.0], [0.9, 0.1, 0.0], [0.1, 0.9, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let nearest = mesh.nearest_node_mapping_to(&target);
        assert_eq!(nearest.kind, SurfaceMappingKind::NearestNode);
        assert_eq!(
            nearest.transfer_f32(&[10.0, 20.0, 30.0, 40.0]).unwrap()[0],
            10.0
        );

        let barycentric = mesh.barycentric_triangle_mapping_to(&target);
        assert_eq!(barycentric.kind, SurfaceMappingKind::BarycentricTriangle);
        let transferred = barycentric.transfer_f32(&[1.0, 1.0, 1.0, 1.0]).unwrap();
        assert_eq!(transferred, vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn value_range_ignores_non_finite_values() {
        let range = ValueRange::from_values(&[f32::NAN, -2.0, f32::INFINITY, 3.0]).unwrap();

        assert_eq!(range.min, -2.0);
        assert_eq!(range.max, 3.0);
    }

    #[test]
    fn surface_mesh_new_populates_basic_metadata() {
        let mesh = SurfaceMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )
        .unwrap();

        assert_eq!(mesh.metadata.node_count, 3);
        assert_eq!(mesh.metadata.node_dimension, 3);
        assert_eq!(mesh.metadata.embedding_dimension, 3);
        assert_eq!(mesh.metadata.face_count, 1);
        assert_eq!(mesh.metadata.face_dimension, 3);
        assert_eq!(mesh.metadata.source_file, None);
        assert_eq!(mesh.metadata.side, SurfaceSide::Unknown);
        assert_eq!(mesh.metadata.surface_kind, SurfaceKind::Unknown);
        assert_eq!(
            mesh.metadata.lineage.domain.kind,
            SurfaceDomainKind::Unknown
        );
        assert_eq!(mesh.domain.node_count, 3);
        assert_eq!(mesh.domain.row_to_node, RowToNodeMapping::Dense);
        assert_eq!(mesh.domain.node_for_row(1), Some(1));
        assert_eq!(mesh.domain.row_for_node(2), Some(2));
        assert_eq!(mesh.domain.triangles, vec![[0, 1, 2]]);
        assert!(!mesh.metadata.lineage.domain.allow_node_count_match);
        assert!(mesh.metadata.id.as_str().starts_with("surface-"));
    }

    #[test]
    fn surface_domain_tracks_indexed_row_mapping() {
        let domain = SurfaceDomain::from_indexed_rows(10, vec![2, 5, 9], vec![[0, 1, 2]]).unwrap();

        assert_eq!(domain.node_count, 10);
        assert_eq!(domain.row_count(), 3);
        assert_eq!(domain.node_for_row(0), Some(2));
        assert_eq!(domain.node_for_row(2), Some(9));
        assert_eq!(domain.row_for_node(5), Some(1));
        assert_eq!(domain.row_for_node(4), None);
        assert!(domain.sorted_nodes.is_sorted);
        assert!(!domain.sorted_nodes.has_duplicates);
    }

    #[test]
    fn surface_domain_tracks_external_node_ids_and_duplicates() {
        let domain = SurfaceDomain::with_node_ids(vec![10, 3, 10], vec![[0, 1, 2]]).unwrap();

        assert_eq!(domain.node_ids.as_deref(), Some([10, 3, 10].as_slice()));
        assert_eq!(domain.row_to_node, RowToNodeMapping::Dense);
        assert!(!domain.sorted_nodes.is_sorted);
        assert!(domain.sorted_nodes.has_duplicates);
    }

    #[test]
    fn surface_domain_topology_ignores_coordinates() {
        let first =
            SurfaceDomain::from_triangles(four_vertices().len(), vec![[0, 1, 2], [0, 2, 3]])
                .unwrap();
        let second =
            SurfaceDomain::from_triangles(four_vertices().len(), vec![[0, 1, 2], [0, 2, 3]])
                .unwrap();
        let different =
            SurfaceDomain::from_triangles(four_vertices().len(), vec![[0, 1, 3], [1, 2, 3]])
                .unwrap();

        assert!(first.shares_topology_with(&second));
        assert!(!first.shares_topology_with(&different));
    }

    #[test]
    fn surface_id_is_independent_of_source_path() {
        let vertices = four_vertices();
        let triangles = vec![[0, 1, 2], [0, 2, 3]];
        let first = metadata_for_path(
            Some(Path::new("/first/location/lh.pial.surf.gii")),
            vertices.clone(),
            triangles.clone(),
        );
        let copied = metadata_for_path(
            Some(Path::new("/copied/location/lh.pial.surf.gii")),
            vertices,
            triangles,
        );

        assert_eq!(first.id, copied.id);
        assert_ne!(first.source_file, copied.source_file);
    }

    #[test]
    fn surface_id_changes_when_triangle_indices_change() {
        let vertices = four_vertices();
        let first = metadata_for_path(None, vertices.clone(), vec![[0, 1, 2], [0, 2, 3]]);
        let changed = metadata_for_path(None, vertices, vec![[0, 1, 3], [1, 2, 3]]);

        assert_ne!(first.id, changed.id);
    }

    #[test]
    fn filename_inference_detects_side_kind_and_subject() {
        let path = Path::new("/data/sub-01/surf/lh.pial.surf.gii");
        let vertices = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let domain = SurfaceDomain::from_triangles(vertices.len(), vec![[0, 1, 2]]).unwrap();
        let metadata = super::SurfaceMetadata::from_geometry(
            Some(path.to_path_buf()),
            super::label_from_path(path),
            &vertices,
            &domain,
            Bounds::from_vertices(&vertices).unwrap(),
            3,
            3,
        );

        assert_eq!(metadata.label.as_deref(), Some("lh.pial"));
        assert_eq!(metadata.subject_label.as_deref(), Some("sub-01"));
        assert_eq!(metadata.side, SurfaceSide::Left);
        assert_eq!(metadata.surface_kind, SurfaceKind::Pial);
        assert_eq!(metadata.state_name.as_deref(), Some("pial"));
        assert_eq!(
            metadata.lineage.domain.kind,
            SurfaceDomainKind::NativeSubject
        );
    }

    #[test]
    fn surface_kind_inference_handles_common_suma_names() {
        assert_eq!(
            infer_surface_kind_from_path(Path::new("rh.smoothwm.surf.gii")),
            Some(SurfaceKind::SmoothWhiteMatter)
        );
        assert_eq!(
            infer_surface_kind_from_path(Path::new("std.141.lh.sphere.gii")),
            Some(SurfaceKind::Sphere)
        );
        assert_eq!(
            infer_surface_kind_from_path(Path::new("lh.very_inflated.surf.gii")),
            Some(SurfaceKind::VeryInflated)
        );
    }

    #[test]
    fn sphere_kind_adds_sphere_metadata() {
        let vertices = [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let bounds = Bounds::from_vertices(&vertices).unwrap();
        let domain = SurfaceDomain::from_triangles(vertices.len(), vec![[0, 1, 2]]).unwrap();
        let metadata = super::SurfaceMetadata::from_geometry(
            Some(Path::new("lh.sphere.gii").to_path_buf()),
            None,
            &vertices,
            &domain,
            bounds,
            3,
            3,
        );

        let sphere = metadata.sphere.unwrap();
        assert_eq!(sphere.center, bounds.center);
        assert_eq!(sphere.radius, bounds.radius);
    }

    #[test]
    fn anatomical_correctness_parser_keeps_unknown_explicit() {
        assert_eq!(
            AnatomicalCorrectness::from_text("YES"),
            Some(AnatomicalCorrectness::Correct)
        );
        assert_eq!(
            AnatomicalCorrectness::from_text("false"),
            Some(AnatomicalCorrectness::Incorrect)
        );
        assert_eq!(AnatomicalCorrectness::from_text("maybe"), None);
    }

    #[test]
    fn standard_surface_can_match_unknown_surface_by_node_count() {
        let standard = metadata_for_path(
            Some(Path::new("/data/sub-01/std.141/lh.pial.surf.gii")),
            four_vertices(),
            vec![[0, 1, 2], [0, 2, 3]],
        );
        let unknown = metadata_for_path(None, four_vertices(), vec![[0, 1, 3], [1, 2, 3]]);

        assert_eq!(
            standard.lineage.domain.kind,
            SurfaceDomainKind::DerivedFromStandard
        );
        assert_eq!(
            standard.lineage.domain.standard_space.as_deref(),
            Some("std.141")
        );
        assert!(standard.lineage.domain.allow_node_count_match);
        assert_eq!(
            standard.kinship_with(&unknown),
            SurfaceKinship::SameStandardNodeCount
        );
        assert!(standard.can_share_nodewise_data_with(&unknown));
    }

    #[test]
    fn native_surfaces_do_not_match_by_node_count_alone() {
        let native_a = metadata_for_path(
            Some(Path::new("/data/sub-01/surf/lh.pial.surf.gii")),
            four_vertices(),
            vec![[0, 1, 2], [0, 2, 3]],
        );
        let native_b = metadata_for_path(
            Some(Path::new("/data/sub-02/surf/lh.pial.surf.gii")),
            four_vertices(),
            vec![[0, 1, 3], [1, 2, 3]],
        );

        assert_eq!(
            native_a.kinship_with(&native_b),
            SurfaceKinship::NeedsMapping
        );
        assert!(!native_a.can_share_nodewise_data_with(&native_b));
    }

    #[test]
    fn same_topology_surfaces_can_share_nodewise_data() {
        let pial = metadata_for_path(
            Some(Path::new("/data/sub-01/surf/lh.pial.surf.gii")),
            four_vertices(),
            vec![[0, 1, 2], [0, 2, 3]],
        );
        let inflated = metadata_for_path(
            Some(Path::new("/data/sub-01/surf/lh.inflated.surf.gii")),
            vec![
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        );

        assert_eq!(pial.kinship_with(&inflated), SurfaceKinship::SameTopology);
        assert!(pial.can_share_nodewise_data_with(&inflated));
    }

    #[test]
    fn different_standard_spaces_need_mapping() {
        let std_141 = metadata_for_path(
            Some(Path::new("/data/sub-01/std.141/lh.pial.surf.gii")),
            four_vertices(),
            vec![[0, 1, 2], [0, 2, 3]],
        );
        let fs_lr = metadata_for_path(
            Some(Path::new("/data/sub-01/fs_LR_32k/lh.pial.surf.gii")),
            four_vertices(),
            vec![[0, 1, 3], [1, 2, 3]],
        );

        assert_eq!(
            fs_lr.lineage.domain.standard_space.as_deref(),
            Some("fs_LR_32k")
        );
        assert_eq!(std_141.kinship_with(&fs_lr), SurfaceKinship::NeedsMapping);
        assert!(!std_141.can_share_nodewise_data_with(&fs_lr));
    }

    fn metadata_for_path(
        path: Option<&Path>,
        vertices: Vec<[f32; 3]>,
        triangles: Vec<[u32; 3]>,
    ) -> super::SurfaceMetadata {
        let bounds = Bounds::from_vertices(&vertices).unwrap();
        let domain = SurfaceDomain::from_triangles(vertices.len(), triangles).unwrap();
        super::SurfaceMetadata::from_geometry(
            path.map(Path::to_path_buf),
            path.and_then(super::label_from_path),
            &vertices,
            &domain,
            bounds,
            3,
            3,
        )
    }

    fn four_vertices() -> Vec<[f32; 3]> {
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ]
    }

    fn tetra_vertices() -> Vec<[f32; 3]> {
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ]
    }

    fn outward_tetra_triangles() -> Vec<[u32; 3]> {
        vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]]
    }

    fn assert_close(left: f32, right: f32) {
        assert!(
            (left - right).abs() < 1e-6,
            "expected {left} to be close to {right}"
        );
    }

    fn assert_vec3_close(left: [f32; 3], right: [f32; 3]) {
        for (left, right) in left.into_iter().zip(right) {
            assert_close(left, right);
        }
    }
}
