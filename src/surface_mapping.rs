// Developed by P. Molfese, National Institutes of Health (NIH).
// See LICENSE for the public-domain notice and disclaimer.
//! Explicit target-to-source weighted maps. Geometry never implies vertex identity.
use crate::{
    dataset::{ColumnData, ColumnRole, DataColumn, Dataset, DatasetKind},
    surface::SurfaceMesh,
};
use anyhow::{Context, Result, ensure};
use glam::{DMat4, DVec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapRow {
    pub source_nodes: Vec<u32>,
    pub weights: Vec<f64>,
    pub distance: Option<f64>,
    pub signed_distance: Option<f64>,
    pub triangle: Option<u32>,
    pub fallback: bool,
}
impl MapRow {
    fn missing() -> Self {
        Self {
            source_nodes: vec![],
            weights: vec![],
            distance: None,
            signed_distance: None,
            triangle: None,
            fallback: false,
        }
    }
    fn node(node: u32, distance: f64) -> Self {
        Self {
            source_nodes: vec![node],
            weights: vec![1.],
            distance: Some(distance),
            ..Self::missing()
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometricMap {
    pub schema: String,
    pub source_fingerprint: String,
    pub target_fingerprint: String,
    pub source_nodes: usize,
    pub target_nodes: usize,
    pub method: String,
    /// Row-major affine taking source XYZ into target XYZ, in the loaded mesh basis.
    pub source_to_target: [[f64; 4]; 4],
    pub distance_units: String,
    pub rows: Vec<MapRow>,
    pub notes: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingMethod {
    NearestNode,
    ClosestTriangle,
    NormalProjection,
    AreaAverage,
}
#[derive(Debug, Clone)]
pub struct MappingOptions {
    pub method: MappingMethod,
    pub transform: [[f64; 4]; 4],
    pub maximum_distance: Option<f64>,
    pub nearest_fallback: bool,
}
impl Default for MappingOptions {
    fn default() -> Self {
        Self {
            method: MappingMethod::ClosestTriangle,
            transform: identity(),
            maximum_distance: None,
            nearest_fallback: false,
        }
    }
}
pub fn identity() -> [[f64; 4]; 4] {
    [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ]
}
pub fn affine(matrix: [[f64; 4]; 4]) -> Result<DMat4> {
    // afni-core owns affine validation; the matrix is already in the loaded
    // coordinate basis. No convention conversion is performed implicitly.
    afni_core::affine::AffineTransform::new(matrix, afni_core::affine::CoordinateConvention::Ras)?;
    let result = DMat4::from_cols_array(&std::array::from_fn::<_, 16, _>(|i| matrix[i % 4][i / 4]));
    ensure!(result.determinant().abs() > 1e-15, "singular affine");
    Ok(result)
}
pub fn centroid_translation(source: &SurfaceMesh, target: &SurfaceMesh) -> [[f64; 4]; 4] {
    let center = |mesh: &SurfaceMesh| {
        mesh.vertices.iter().map(|p| point(*p)).sum::<DVec3>() / mesh.vertices.len() as f64
    };
    let delta = center(target) - center(source);
    let mut m = identity();
    for i in 0..3 {
        m[i][3] = (delta[i] * 1e6).round() / 1e6;
    }
    m
}
pub fn read_affine(path: &std::path::Path) -> Result<[[f64; 4]; 4]> {
    let table = afni_io::onedee::OneD::read(path)?;
    if table.rows == 1 && table.cols == 12 {
        return Ok(*afni_io::aff12::Aff12File::from_onedee(table)?
            .single_transform()?
            .matrix());
    }
    ensure!(
        table.cols == 4 && (table.rows == 3 || table.rows == 4),
        "affine requires one aff12 row, a 3x4 or 4x4 matrix"
    );
    let mut matrix = identity();
    for (i, value) in table.values.iter().enumerate() {
        matrix[i / 4][i % 4] = *value;
    }
    affine(matrix)?;
    Ok(matrix)
}
pub fn mesh_fingerprint(mesh: &SurfaceMesh) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    let mut feed = |b: &[u8]| {
        for x in b {
            hash ^= *x as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    feed(&(mesh.vertices.len() as u64).to_le_bytes());
    feed(&(mesh.triangles.len() as u64).to_le_bytes());
    for p in &mesh.vertices {
        for x in p {
            feed(&x.to_le_bytes());
        }
    }
    for t in &mesh.triangles {
        for x in t {
            feed(&x.to_le_bytes());
        }
    }
    format!("mesh-fnv1a64-v1:{hash:016x}")
}
fn point(p: [f32; 3]) -> DVec3 {
    DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)
}

// One deterministic AABB hierarchy serves point, closest-triangle and ray queries.
struct Bvh {
    lo: DVec3,
    hi: DVec3,
    ids: Vec<usize>,
    children: Option<Box<[Bvh; 2]>>,
}
impl Bvh {
    fn build(mut ids: Vec<usize>, bounds: &[(DVec3, DVec3)]) -> Self {
        let lo = ids
            .iter()
            .fold(DVec3::splat(f64::INFINITY), |a, &i| a.min(bounds[i].0));
        let hi = ids
            .iter()
            .fold(DVec3::splat(f64::NEG_INFINITY), |a, &i| a.max(bounds[i].1));
        let children = if ids.len() > 8 {
            let extent = hi - lo;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            ids.sort_unstable_by(|&a, &b| {
                (bounds[a].0[axis] + bounds[a].1[axis])
                    .total_cmp(&(bounds[b].0[axis] + bounds[b].1[axis]))
                    .then(a.cmp(&b))
            });
            let right = ids.split_off(ids.len() / 2);
            let left = std::mem::take(&mut ids);
            Some(Box::new([
                Self::build(left, bounds),
                Self::build(right, bounds),
            ]))
        } else {
            None
        };
        Self {
            lo,
            hi,
            ids,
            children,
        }
    }
    fn distance2(&self, p: DVec3) -> f64 {
        (p - self.lo).min(DVec3::ZERO).length_squared()
            + (p - self.hi).max(DVec3::ZERO).length_squared()
    }
    fn nearest(&self, p: DVec3, best: &mut (f64, usize), evaluate: &impl Fn(usize) -> f64) {
        if self.distance2(p) > best.0 {
            return;
        }
        if let Some(c) = &self.children {
            let (a, b) = if c[0].distance2(p) <= c[1].distance2(p) {
                (&c[0], &c[1])
            } else {
                (&c[1], &c[0])
            };
            a.nearest(p, best, evaluate);
            b.nearest(p, best, evaluate);
        } else {
            for &i in &self.ids {
                let distance = evaluate(i);
                if distance < best.0 || (distance == best.0 && i < best.1) {
                    *best = (distance, i);
                }
            }
        }
    }
    fn ray_intersects(&self, p: DVec3, d: DVec3, max: f64) -> bool {
        let mut low: f64 = 0.;
        let mut high = max;
        for i in 0..3 {
            if d[i].abs() < 1e-15 {
                if p[i] < self.lo[i] - 1e-12 || p[i] > self.hi[i] + 1e-12 {
                    return false;
                }
            } else {
                let a = (self.lo[i] - p[i]) / d[i];
                let b = (self.hi[i] - p[i]) / d[i];
                low = low.max(a.min(b));
                high = high.min(a.max(b));
            }
        }
        high >= low
    }
    fn ray(
        &self,
        p: DVec3,
        d: DVec3,
        best: &mut (f64, usize, [f64; 3]),
        evaluate: &impl Fn(usize) -> Option<(f64, [f64; 3])>,
    ) {
        if !self.ray_intersects(p, d, best.0) {
            return;
        }
        if let Some(c) = &self.children {
            c[0].ray(p, d, best, evaluate);
            c[1].ray(p, d, best, evaluate);
        } else {
            for &i in &self.ids {
                if let Some((t, w)) = evaluate(i)
                    && (t < best.0 || (t == best.0 && i < best.1))
                {
                    *best = (t, i, w);
                }
            }
        }
    }
}
/// Exact constrained closest point, including triangle edges and degenerate faces.
pub fn closest_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> (DVec3, [f64; 3]) {
    let ab = b - a;
    let ac = c - a;
    let n = ab.cross(ac);
    if n.length_squared() > 1e-28 {
        let q = p - n * ((p - a).dot(n) / n.length_squared());
        let v = q - a;
        let aa = ab.dot(ab);
        let bb = ab.dot(ac);
        let cc = ac.dot(ac);
        let den = aa * cc - bb * bb;
        if den > 0. {
            let u = (v.dot(ab) * cc - v.dot(ac) * bb) / den;
            let w = (v.dot(ac) * aa - v.dot(ab) * bb) / den;
            if u >= 0. && w >= 0. && u + w <= 1. {
                return (q, [1. - u - w, u, w]);
            }
        }
    }
    let mut best = (f64::INFINITY, a, [1., 0., 0.]);
    for (x, y, i, j) in [(a, b, 0, 1), (b, c, 1, 2), (c, a, 2, 0)] {
        let edge = y - x;
        let t = if edge.length_squared() > 0. {
            ((p - x).dot(edge) / edge.length_squared()).clamp(0., 1.)
        } else {
            0.
        };
        let q = x + edge * t;
        let dist = q.distance_squared(p);
        if dist < best.0 {
            let mut w = [0.; 3];
            w[i] = 1. - t;
            w[j] = t;
            best = (dist, q, w);
        }
    }
    (best.1, best.2)
}
fn ray_triangle(p: DVec3, d: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<(f64, [f64; 3])> {
    let e1 = b - a;
    let e2 = c - a;
    let h = d.cross(e2);
    let det = e1.dot(h);
    if det.abs() <= 1e-14 * e1.length() * e2.length() {
        return None;
    }
    let q = p - a;
    let u = q.dot(h) / det;
    let v = d.dot(q.cross(e1)) / det;
    let t = e2.dot(q.cross(e1)) / det;
    if u >= -1e-10 && v >= -1e-10 && u + v <= 1. + 1e-10 && t >= -1e-10 {
        let w = [(1. - u - v).max(0.), u.max(0.), v.max(0.)];
        let sum = w.iter().sum::<f64>();
        Some((t.max(0.), w.map(|v| v / sum)))
    } else {
        None
    }
}
struct Search {
    points: Vec<DVec3>,
    triangles: Vec<[u32; 3]>,
    nodes: Bvh,
    faces: Bvh,
}
impl Search {
    fn new(mesh: &SurfaceMesh, transform: DMat4) -> Self {
        let points: Vec<_> = mesh
            .vertices
            .iter()
            .map(|p| transform.transform_point3(point(*p)))
            .collect();
        let nb: Vec<_> = points.iter().map(|p| (*p, *p)).collect();
        let fb: Vec<_> = mesh
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| points[i as usize]);
                (a.min(b).min(c), a.max(b).max(c))
            })
            .collect();
        Self {
            nodes: Bvh::build((0..points.len()).collect(), &nb),
            faces: Bvh::build((0..fb.len()).collect(), &fb),
            points,
            triangles: mesh.triangles.clone(),
        }
    }
    fn nearest(&self, p: DVec3) -> MapRow {
        let mut best = (f64::INFINITY, usize::MAX);
        self.nodes
            .nearest(p, &mut best, &|i| p.distance_squared(self.points[i]));
        MapRow::node(best.1 as u32, best.0.sqrt())
    }
    fn closest(&self, p: DVec3) -> MapRow {
        let mut best = (f64::INFINITY, usize::MAX);
        self.faces.nearest(p, &mut best, &|i| {
            let [a, b, c] = self.triangles[i].map(|n| self.points[n as usize]);
            closest_triangle(p, a, b, c).0.distance_squared(p)
        });
        if best.1 == usize::MAX {
            return MapRow::missing();
        }
        let t = self.triangles[best.1];
        let [a, b, c] = t.map(|n| self.points[n as usize]);
        let (_, w) = closest_triangle(p, a, b, c);
        MapRow {
            source_nodes: t.to_vec(),
            weights: w.to_vec(),
            distance: Some(best.0.sqrt()),
            triangle: Some(best.1 as u32),
            ..MapRow::missing()
        }
    }
    fn normal(&self, p: DVec3, n: DVec3) -> MapRow {
        if n.length_squared() < 1e-20 {
            return MapRow::missing();
        }
        let n = n.normalize();
        let mut winner = MapRow::missing();
        for sign in [1., -1.] {
            let d = n * sign;
            let mut best = (
                winner.distance.unwrap_or(f64::INFINITY),
                usize::MAX,
                [0.; 3],
            );
            self.faces.ray(p, d, &mut best, &|i| {
                let [a, b, c] = self.triangles[i].map(|j| self.points[j as usize]);
                ray_triangle(p, d, a, b, c)
            });
            if best.1 != usize::MAX {
                winner = MapRow {
                    source_nodes: self.triangles[best.1].to_vec(),
                    weights: best.2.to_vec(),
                    distance: Some(best.0),
                    signed_distance: Some(sign * best.0),
                    triangle: Some(best.1 as u32),
                    ..MapRow::missing()
                };
            }
        }
        winner
    }
}
impl GeometricMap {
    pub fn build(
        source: &SurfaceMesh,
        target: &SurfaceMesh,
        options: &MappingOptions,
    ) -> Result<Self> {
        ensure!(
            !source.vertices.is_empty() && !target.vertices.is_empty(),
            "mapping requires nonempty meshes"
        );
        ensure!(
            source
                .vertices
                .iter()
                .chain(&target.vertices)
                .flatten()
                .all(|v| v.is_finite()),
            "nonfinite mesh coordinates"
        );
        if let Some(d) = options.maximum_distance {
            ensure!(
                d.is_finite() && d >= 0.,
                "maximum distance must be finite and nonnegative"
            );
        }
        ensure!(
            !options.nearest_fallback || options.method == MappingMethod::NormalProjection,
            "fallback applies only to normal projection"
        );
        let transform = affine(options.transform)?;
        let search = Search::new(source, transform);
        let core_target = afni_core::mesh::SurfaceMesh::from_triangles(
            target.vertices.clone(),
            target.triangles.clone(),
        )?;
        let normals = core_target.vertex_normals();
        let mut rows = Vec::with_capacity(target.vertices.len());
        if options.method == MappingMethod::AreaAverage {
            ensure!(
                !options.nearest_fallback,
                "area averaging has no projection fallback"
            );
            let targets = Search::new(target, DMat4::IDENTITY);
            rows = vec![MapRow::missing(); target.vertices.len()];
            let core_source = afni_core::mesh::SurfaceMesh::from_triangles(
                search
                    .points
                    .iter()
                    .map(|p| p.as_vec3().to_array())
                    .collect(),
                source.triangles.clone(),
            )?;
            let areas = core_source.node_areas();
            for (i, &p) in search.points.iter().enumerate() {
                if areas[i] <= 0. {
                    continue;
                }
                let r = targets.nearest(p);
                if options
                    .maximum_distance
                    .is_some_and(|limit| r.distance.unwrap() > limit)
                {
                    continue;
                }
                let row = &mut rows[r.source_nodes[0] as usize];
                row.source_nodes.push(i as u32);
                row.weights.push(areas[i]);
                row.distance = Some(row.distance.unwrap_or(0.).max(r.distance.unwrap()));
            }
            for row in &mut rows {
                let sum = row.weights.iter().sum::<f64>();
                if sum > 0. {
                    for w in &mut row.weights {
                        *w /= sum;
                    }
                }
            }
        } else {
            for (i, p) in target.vertices.iter().enumerate() {
                let p = point(*p);
                let mut row = match options.method {
                    MappingMethod::NearestNode => search.nearest(p),
                    MappingMethod::ClosestTriangle => search.closest(p),
                    MappingMethod::NormalProjection => {
                        search.normal(p, DVec3::from_array(normals[i]))
                    }
                    _ => unreachable!(),
                };
                if row.source_nodes.is_empty() && options.nearest_fallback {
                    row = search.nearest(p);
                    row.fallback = true;
                }
                if options
                    .maximum_distance
                    .is_some_and(|limit| row.distance.is_some_and(|d| d > limit))
                {
                    row = MapRow::missing();
                }
                rows.push(row);
            }
        }
        compact_rows(&mut rows);
        let mut map=Self{schema:"sumaru-weighted-map-1".into(),source_fingerprint:mesh_fingerprint(source),target_fingerprint:mesh_fingerprint(target),source_nodes:source.vertices.len(),target_nodes:target.vertices.len(),method:format!("{:?}",options.method),source_to_target:options.transform,distance_units:"loaded surface coordinate units (normally mm)".into(),rows,notes:vec!["Rows gather source values onto target vertices; empty rows are missing, never zero.".into()]};
        map.notes.push(format!(
            "Maximum distance: {:?}; nearest fallback requested: {}",
            options.maximum_distance, options.nearest_fallback
        ));
        if options.method == MappingMethod::AreaAverage {
            map.notes.push("Lumped source vertex areas, assigned to nearest target vertex and normalized per target bucket; not conservative triangle-overlap integration. Row distance is the farthest contributing source.".into());
        }
        map.validate(source, target)?;
        Ok(map)
    }
    pub fn validate(&self, source: &SurfaceMesh, target: &SurfaceMesh) -> Result<()> {
        ensure!(
            self.schema == "sumaru-weighted-map-1",
            "unsupported map schema"
        );
        affine(self.source_to_target)?;
        ensure!(
            self.source_nodes == source.vertices.len()
                && self.target_nodes == target.vertices.len()
                && self.rows.len() == target.vertices.len(),
            "map dimensions disagree with meshes"
        );
        ensure!(
            self.source_fingerprint == mesh_fingerprint(source)
                && self.target_fingerprint == mesh_fingerprint(target),
            "map geometry/vertex-order fingerprint mismatch; equal counts are not correspondence"
        );
        for row in &self.rows {
            ensure!(
                row.source_nodes.len() == row.weights.len(),
                "weight count mismatch"
            );
            let mut seen = std::collections::HashSet::new();
            for (&n, &w) in row.source_nodes.iter().zip(&row.weights) {
                ensure!(
                    (n as usize) < self.source_nodes && w.is_finite() && w >= 0. && seen.insert(n),
                    "invalid source ID, weight, or duplicate in map row"
                );
            }
            if !row.weights.is_empty() {
                ensure!(
                    (row.weights.iter().sum::<f64>() - 1.).abs() < 1e-6,
                    "weights must sum to one"
                );
            }
            ensure!(
                row.distance.is_none_or(|v| v.is_finite() && v >= 0.)
                    && row.signed_distance.is_none_or(f64::is_finite),
                "invalid mapping distance"
            );
            ensure!(
                row.triangle
                    .is_none_or(|t| (t as usize) < source.triangles.len()),
                "triangle out of range"
            );
        }
        Ok(())
    }
    pub fn apply(&self, source: &Dataset, target: &SurfaceMesh) -> Result<Dataset> {
        ensure!(
            self.rows.len() == target.vertices.len()
                && self.target_fingerprint == mesh_fingerprint(target),
            "target mismatch"
        );
        for row in &self.rows {
            ensure!(
                row.source_nodes.len() == row.weights.len()
                    && row
                        .source_nodes
                        .iter()
                        .all(|&n| (n as usize) < self.source_nodes)
                    && row.weights.iter().all(|w| w.is_finite() && *w >= 0.),
                "invalid map row"
            );
            ensure!(
                row.weights.is_empty() || (row.weights.iter().sum::<f64>() - 1.).abs() < 1e-6,
                "weights must sum to one"
            );
        }
        ensure!(
            source.columns.iter().all(|c| !matches!(
                c.role,
                ColumnRole::Label | ColumnRole::Mask | ColumnRole::NodeIndex
            )),
            "categorical data require a dedicated label-preserving route"
        );
        if let Some(ids) = &source.node_indices {
            afni_core::mapping::SampleMap::indexed(ids.clone(), self.source_nodes)?;
        } else {
            ensure!(
                source.row_count == self.source_nodes,
                "dense source node count mismatch"
            );
        }
        let mut columns = Vec::new();
        for c in &source.columns {
            let mut values = vec![f64::NAN; self.source_nodes];
            for row in 0..source.row_count {
                let node = source
                    .node_for_row(row)
                    .context("invalid source addressing")? as usize;
                ensure!(node < values.len(), "source data outside map");
                values[node] = match &c.values {
                    ColumnData::Float32(v) => v[row] as f64,
                    ColumnData::Float64(v) => v[row],
                    ColumnData::Int32(v) => v[row] as f64,
                    ColumnData::UInt32(v) => v[row] as f64,
                    ColumnData::Text(_) => anyhow::bail!("text data cannot be interpolated"),
                };
            }
            let output = self
                .rows
                .iter()
                .map(|row| {
                    if row.weights.is_empty() {
                        return f32::NAN;
                    }
                    let mut sum = 0.;
                    for (&id, &w) in row.source_nodes.iter().zip(&row.weights) {
                        if w > 0. {
                            let v = values[id as usize];
                            if !v.is_finite() {
                                return f32::NAN;
                            }
                            sum += w * v;
                        }
                    }
                    sum as f32
                })
                .collect();
            columns.push(DataColumn::new(
                c.label.clone(),
                c.role.clone(),
                c.units.clone(),
                ColumnData::Float32(output),
            )?);
        }
        let mut result = Dataset::dense(
            if source.kind == DatasetKind::SurfaceTimeSeries {
                DatasetKind::SurfaceTimeSeries
            } else {
                DatasetKind::SurfaceScalar
            },
            &target.domain,
            columns,
        )?;
        result.time_step_seconds = source.time_step_seconds;
        result.time_start_seconds = source.time_start_seconds;
        Ok(result)
    }
    pub fn recover_parent_ids(
        child: &SurfaceMesh,
        parent: &SurfaceMesh,
        transform: [[f64; 4]; 4],
        tolerance: f64,
    ) -> Result<Vec<u32>> {
        ensure!(
            tolerance.is_finite() && tolerance > 0.,
            "recovery tolerance must be positive and finite"
        );
        let transform = affine(transform)?;
        let search = Search::new(parent, DMat4::IDENTITY);
        let mut ids = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Check both uniqueness among children and ambiguity among parent candidates.
        for p in &child.vertices {
            let p = transform.transform_point3(point(*p));
            let nearest = search.nearest(p);
            let id = nearest.source_nodes[0];
            ensure!(
                nearest.distance.unwrap() <= tolerance,
                "no essentially exact parent vertex within tolerance"
            );
            let mut alternate = (tolerance * tolerance, usize::MAX);
            search.nodes.nearest(p, &mut alternate, &|i| {
                if i == id as usize {
                    f64::INFINITY
                } else {
                    search.points[i].distance_squared(p)
                }
            });
            ensure!(
                alternate.1 == usize::MAX && seen.insert(id),
                "ambiguous or non-unique parent-ID recovery"
            );
            ids.push(id);
        }
        Ok(ids)
    }
}

impl GeometricMap {
    /// Registered spheres must already share a registration frame; this is not
    /// a spherical registration algorithm. Physical meshes supply row identity.
    pub fn registered_spheres(
        source: &SurfaceMesh,
        target: &SurfaceMesh,
        source_sphere: &SurfaceMesh,
        target_sphere: &SurfaceMesh,
    ) -> Result<Self> {
        ensure!(
            source.vertices.len() == source_sphere.vertices.len()
                && source.triangles == source_sphere.triangles,
            "source sphere topology/order does not match source mesh"
        );
        ensure!(
            target.vertices.len() == target_sphere.vertices.len()
                && target.triangles == target_sphere.triangles,
            "target sphere topology/order does not match target mesh"
        );
        let normalize = |mesh: &SurfaceMesh| -> Result<SurfaceMesh> {
            let p = mesh
                .vertices
                .iter()
                .map(|p| {
                    let p = point(*p);
                    ensure!(
                        p.is_finite() && p.length() > 1e-12,
                        "sphere contains invalid or origin vertex"
                    );
                    Ok(p.normalize().as_vec3().to_array())
                })
                .collect::<Result<Vec<_>>>()?;
            SurfaceMesh::new(p, mesh.triangles.clone())
        };
        let ss = normalize(source_sphere)?;
        let ts = normalize(target_sphere)?;
        let search = Search::new(&ss, DMat4::IDENTITY);
        let rows = ts
            .vertices
            .iter()
            .map(|p| {
                let d = point(*p).normalize();
                let mut hit = (f64::INFINITY, usize::MAX, [0.; 3]);
                search.faces.ray(DVec3::ZERO, d, &mut hit, &|i| {
                    let [a, b, c] = search.triangles[i].map(|n| search.points[n as usize]);
                    ray_triangle(DVec3::ZERO, d, a, b, c)
                });
                if hit.1 == usize::MAX {
                    MapRow::missing()
                } else {
                    MapRow {
                        source_nodes: search.triangles[hit.1].to_vec(),
                        weights: hit.2.to_vec(),
                        distance: Some((1. - hit.0).abs()),
                        triangle: Some(hit.1 as u32),
                        ..MapRow::missing()
                    }
                }
            })
            .collect();
        let mut map=Self{schema:"sumaru-weighted-map-1".into(),source_fingerprint:mesh_fingerprint(source),target_fingerprint:mesh_fingerprint(target),source_nodes:source.vertices.len(),target_nodes:target.vertices.len(),method:"RegisteredSphereBarycentric".into(),source_to_target:identity(),distance_units:"unit-sphere radial distance (not anatomical mm)".into(),rows,notes:vec![format!("Source registration sphere: {}",mesh_fingerprint(source_sphere)),format!("Target registration sphere: {}",mesh_fingerprint(target_sphere)),"Spheres were supplied as registered, origin-centered coordinates; no registration inferred.".into()]};
        compact_rows(&mut map.rows);
        map.validate(source, target)?;
        Ok(map)
    }
    pub fn summary(&self) -> serde_json::Value {
        let mut distances: Vec<f64> = self.rows.iter().filter_map(|r| r.distance).collect();
        distances.sort_by(f64::total_cmp);
        serde_json::json!({"schema":self.schema,"method":self.method,"source_vertices":self.source_nodes,"target_vertices":self.target_nodes,"mapped_vertices":self.rows.iter().filter(|r|!r.weights.is_empty()).count(),"missing_vertices":self.rows.iter().filter(|r|r.weights.is_empty()).count(),"fallback_vertices":self.rows.iter().filter(|r|r.fallback).count(),"maximum_distance":distances.last(),"median_distance":distances.get(distances.len()/2),"distance_units":self.distance_units,"source_fingerprint":self.source_fingerprint,"target_fingerprint":self.target_fingerprint,"source_to_target":self.source_to_target,"notes":self.notes})
    }
}
fn compact_rows(rows: &mut [MapRow]) {
    for row in rows {
        let mut weights = std::collections::BTreeMap::new();
        for (&id, &weight) in row.source_nodes.iter().zip(&row.weights) {
            if weight > 0. {
                *weights.entry(id).or_insert(0.) += weight;
            }
        }
        row.source_nodes = weights.keys().copied().collect();
        row.weights = weights.values().copied().collect();
    }
}

/// Import AFNI's explicit M2->M1 gather rows. The caller deliberately binds
/// the supplied meshes: AFNI IDcodes cannot verify our geometry fingerprints.
pub fn import_afni_m2m(
    path: &std::path::Path,
    source: &SurfaceMesh,
    target: &SurfaceMesh,
) -> Result<GeometricMap> {
    use afni_io::niml::NimlData;
    let elements = afni_io::niml::read(path)?;
    let root = elements
        .iter()
        .find(|e| e.name == "M2M")
        .context("no M2M group")?;
    let NimlData::Group(children) = &root.data else {
        anyhow::bail!("M2M is not a group")
    };
    let count = |key: &str| -> Result<usize> {
        Ok(root
            .attrs
            .get(key)
            .with_context(|| format!("missing M2M {key}"))?
            .parse()?)
    };
    ensure!(
        count("M1_N_Nodes")? == target.vertices.len()
            && count("M2_N_Nodes")? == source.vertices.len(),
        "M2M source/target node count mismatch (M2=source, M1=target)"
    );
    let column = |name: &str| -> Result<Vec<f64>> {
        let element = children
            .iter()
            .find(|e| e.name == name)
            .with_context(|| format!("missing M2M {name}"))?;
        let NimlData::Numeric(m) = &element.data else {
            anyhow::bail!("M2M {name} is not numeric")
        };
        ensure!(m.column_count() == 1, "M2M vector shape mismatch");
        (0..m.rows)
            .map(|i| m.get(i, 0).context("truncated M2M numeric vector"))
            .collect()
    };
    let integer = |v: f64| -> Result<usize> {
        ensure!(
            v.is_finite() && v >= 0. && v.fract() == 0. && v <= usize::MAX as f64,
            "invalid M2M index/count"
        );
        Ok(v as usize)
    };
    let targets = column("M1n")?;
    let counts = column("M2Nne_M1n")?;
    let ids = column("M2ne_M1n")?;
    let weights = column("M2we_M1n")?;
    let triangles = column("M2t_M1n")?;
    let distances = column("PD")?;
    ensure!(
        targets.len() == count("M1Nn")?
            && counts.len() == targets.len()
            && triangles.len() == targets.len()
            && distances.len() == targets.len()
            && ids.len() == weights.len(),
        "M2M vector length mismatch"
    );
    ensure!(
        triangles
            .iter()
            .all(|t| t.is_finite() && t.fract() == 0. && *t >= -1.),
        "invalid M2M triangle sentinel/index"
    );
    ensure!(
        distances.iter().all(|d| d.is_finite()),
        "nonfinite M2M distance"
    );
    let mut rows = vec![MapRow::missing(); target.vertices.len()];
    let mut seen = std::collections::HashSet::new();
    let mut offset = 0usize;
    for i in 0..targets.len() {
        let target = integer(targets[i])?;
        ensure!(
            target < rows.len() && seen.insert(target),
            "duplicate/out-of-range M2M target"
        );
        let n = integer(counts[i])?;
        let end = offset.checked_add(n).context("M2M length overflow")?;
        ensure!(end <= ids.len(), "truncated M2M weights");
        // AFNI uses negative node IDs and a negative face for failed projections.
        if n > 0 {
            ensure!(
                triangles[i] >= 0. || n == 1,
                "invalid AFNI M2M face for weighted row"
            );
            let nodes = ids[offset..end]
                .iter()
                .map(|&v| u32::try_from(integer(v)?).map_err(Into::into))
                .collect::<Result<Vec<_>>>()?;
            rows[target] = MapRow {
                source_nodes: nodes,
                weights: weights[offset..end].to_vec(),
                distance: Some(distances[i].abs()),
                signed_distance: Some(distances[i]),
                triangle: if triangles[i] >= 0. {
                    Some(u32::try_from(integer(triangles[i])?)?)
                } else {
                    None
                },
                fallback: triangles[i] < 0.,
            };
        }
        offset = end;
    }
    ensure!(
        rows.iter()
            .flat_map(|r| &r.source_nodes)
            .all(|n| (*n as usize) < source.vertices.len()),
        "M2M source index out of range"
    );
    ensure!(
        weights.iter().all(|w| w.is_finite() && *w >= 0.),
        "invalid M2M weight"
    );
    ensure!(offset == ids.len(), "unused M2M weights");
    compact_rows(&mut rows);
    let map=GeometricMap{schema:"sumaru-weighted-map-1".into(),source_fingerprint:mesh_fingerprint(source),target_fingerprint:mesh_fingerprint(target),source_nodes:source.vertices.len(),target_nodes:target.vertices.len(),method:"AfniM2M".into(),source_to_target:identity(),distance_units:"AFNI mapping-coordinate units; transform is not stored in M2M".into(),rows,notes:vec!["Explicit user binding of M2=source and M1=target. Counts/AFNI IDcodes do not prove geometric correspondence; fingerprints bind subsequent reuse.".into(),format!("AFNI M1_IDcode={:?}; M2_IDcode={:?}",root.attrs.get("M1_IDcode"),root.attrs.get("M2_IDcode"))]};
    map.validate(source, target)?;
    Ok(map)
}
