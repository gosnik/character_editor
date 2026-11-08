//! Candidate-limited runtime body masking for fitted custom garments.
use crate::native_character::NativePart;
use bevy::prelude::Vec3;
use std::cmp::Ordering;

const MAX_COORD: f32 = 100.;
const MAX_TRIANGLES: usize = 200_000;
const LEAF: usize = 16;
const INWARD: f32 = 0.4;
const OUTWARD: f32 = 1.0;

#[derive(Clone, Copy)]
struct Bounds {
    low: Vec3,
    high: Vec3,
}
impl Bounds {
    fn triangle(a: Vec3, b: Vec3, c: Vec3) -> Self {
        Self {
            low: a.min(b).min(c),
            high: a.max(b).max(c),
        }
    }
    fn join(self, other: Self) -> Self {
        Self {
            low: self.low.min(other.low),
            high: self.high.max(other.high),
        }
    }
    fn overlaps(self, other: Self) -> bool {
        (0..3).all(|i| self.low[i] <= other.high[i] && self.high[i] >= other.low[i])
    }
}
#[derive(Clone, Copy)]
struct ClothTriangle {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    normal: Vec3,
    normal_length: f32,
    bounds: Bounds,
}
struct Node {
    bounds: Bounds,
    left: Option<usize>,
    right: Option<usize>,
    triangles: Vec<usize>,
}

fn candidates(part: &NativePart, face_count: usize) -> Result<Option<Vec<usize>>, String> {
    if part.material["fitBodyMaskCandidates"].is_null() {
        return Ok(None);
    }
    let values = part.material["fitBodyMaskCandidates"]
        .as_array()
        .ok_or_else(|| format!("{} has invalid fitBodyMaskCandidates", part.name))?;
    if values.len() > face_count {
        return Err(format!("{} has too many fitBodyMaskCandidates", part.name));
    }
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let Some(value) = value.as_u64().filter(|value| *value < face_count as u64) else {
            return Err(format!(
                "{} has an invalid fitBodyMaskCandidates face",
                part.name
            ));
        };
        result.push(value as usize);
    }
    result.sort_unstable();
    result.dedup();
    Ok(Some(result))
}

fn forward_only(part: &NativePart) -> Result<bool, String> {
    let value = &part.material["fitBodyMaskForwardOnly"];
    if value.is_null() {
        Ok(false)
    } else {
        value
            .as_bool()
            .ok_or_else(|| format!("{} has invalid fitBodyMaskForwardOnly", part.name))
    }
}

fn build(nodes: &mut Vec<Node>, triangles: &[ClothTriangle], mut indices: Vec<usize>) -> usize {
    let bounds = indices
        .iter()
        .map(|&i| triangles[i].bounds)
        .reduce(Bounds::join)
        .unwrap();
    let index = nodes.len();
    nodes.push(Node {
        bounds,
        left: None,
        right: None,
        triangles: Vec::new(),
    });
    if indices.len() <= LEAF {
        nodes[index].triangles = indices;
        return index;
    }
    let extent = bounds.high - bounds.low;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    indices.sort_unstable_by(|&a, &b| {
        triangles[a].bounds.low[axis]
            .partial_cmp(&triangles[b].bounds.low[axis])
            .unwrap_or(Ordering::Equal)
    });
    let right = indices.split_off(indices.len() / 2);
    let left = build(nodes, triangles, indices);
    let right = build(nodes, triangles, right);
    nodes[index].left = Some(left);
    nodes[index].right = Some(right);
    index
}

fn hit_from(point: Vec3, direction: Vec3, triangle: ClothTriangle, minimum: f32) -> bool {
    let p = direction.cross(triangle.c - triangle.a);
    let det = (triangle.b - triangle.a).dot(p);
    if det.abs() <= 1e-12 {
        return false;
    }
    let inverse = det.recip();
    let delta = point - triangle.a;
    let u = delta.dot(p) * inverse;
    if u < -1e-8 || u > 1. + 1e-8 {
        return false;
    }
    let q = delta.cross(triangle.b - triangle.a);
    let v = direction.dot(q) * inverse;
    if v < -1e-8 || u + v > 1. + 1e-8 {
        return false;
    }
    let distance = (triangle.c - triangle.a).dot(q) * inverse;
    (minimum..=OUTWARD).contains(&distance)
}

fn covered(point: Vec3, normal: Vec3, nodes: &[Node], triangles: &[ClothTriangle]) -> bool {
    let segment = Bounds {
        low: (point - normal * INWARD).min(point + normal * OUTWARD),
        high: (point - normal * INWARD).max(point + normal * OUTWARD),
    };
    let mut stack = vec![0usize];
    while let Some(index) = stack.pop() {
        let node = &nodes[index];
        if !node.bounds.overlaps(segment) {
            continue;
        }
        if let (Some(left), Some(right)) = (node.left, node.right) {
            stack.push(left);
            stack.push(right);
            continue;
        }
        for &index in &node.triangles {
            let triangle = triangles[index];
            if triangle.normal.dot(normal) > triangle.normal_length * 0.05
                && hit_from(point, normal, triangle, -INWARD)
            {
                return true;
            }
        }
    }
    false
}

/// Layer culling must only see an outer garment in front of the lower surface.
/// Body masking intentionally uses `covered` above, which accepts a short
/// inward segment for authored body/cloth offset differences.
fn covered_forward(point: Vec3, normal: Vec3, nodes: &[Node], triangles: &[ClothTriangle]) -> bool {
    covered_layer(point, normal, nodes, triangles, 1e-5)
}

fn covered_layer(
    point: Vec3,
    normal: Vec3,
    nodes: &[Node],
    triangles: &[ClothTriangle],
    inward: f32,
) -> bool {
    // Preserve the original forward-only broad-phase interval when no actual
    // overlap was requested, including its tiny hit-test epsilon.
    let start = if inward > 1e-5 {
        point - normal * inward
    } else {
        point
    };
    let segment = Bounds {
        low: start.min(point + normal * OUTWARD),
        high: start.max(point + normal * OUTWARD),
    };
    let mut stack = vec![0usize];
    while let Some(index) = stack.pop() {
        let node = &nodes[index];
        if !node.bounds.overlaps(segment) {
            continue;
        }
        if let (Some(left), Some(right)) = (node.left, node.right) {
            stack.push(left);
            stack.push(right);
            continue;
        }
        for &index in &node.triangles {
            let triangle = triangles[index];
            if triangle.normal.dot(normal) > triangle.normal_length * 0.05
                && hit_from(point, normal, triangle, -inward)
            {
                return true;
            }
        }
    }
    false
}

fn triangles(part: &NativePart) -> Result<Vec<ClothTriangle>, String> {
    if part
        .positions
        .iter()
        .flatten()
        .any(|v| !v.is_finite() || v.abs() > MAX_COORD)
    {
        return Err(format!("{} has invalid cloth coordinates", part.name));
    }
    if part.indices.len() % 3 != 0 || part.indices.len() / 3 > MAX_TRIANGLES {
        return Err(format!("{} exceeds cloth triangle limits", part.name));
    }
    let mut triangles = Vec::with_capacity(part.indices.len() / 3);
    for indices in part.indices.chunks_exact(3) {
        let point = |i: u32| {
            part.positions
                .get(i as usize)
                .copied()
                .map(Vec3::from)
                .ok_or_else(|| format!("{} has an invalid cloth triangle", part.name))
        };
        let (a, b, c) = (point(indices[0])?, point(indices[1])?, point(indices[2])?);
        let normal = (b - a).cross(c - a);
        let normal_length = normal.length();
        if normal_length > 1e-12 {
            triangles.push(ClothTriangle {
                a,
                b,
                c,
                normal,
                normal_length,
                bounds: Bounds::triangle(a, b, c),
            });
        }
    }
    Ok(triangles)
}

fn collider(part: &NativePart) -> Result<(Vec<ClothTriangle>, Vec<Node>), String> {
    let triangles = triangles(part)?;
    if triangles.is_empty() {
        return Err(format!("{} has no finite cloth triangles", part.name));
    }
    let mut nodes = Vec::new();
    build(&mut nodes, &triangles, (0..triangles.len()).collect());
    Ok((triangles, nodes))
}

/// Removes only lower-layer triangles covered by every point of a bounded 5x5
/// barycentric lattice. Tiny subcell openings can still be missed; callers use
/// this only for an explicit author opt-in on the outer garment.
pub fn cull_fully_occluded_triangles(
    outer: &NativePart,
    lower: &mut NativePart,
) -> Result<usize, String> {
    // Source garments can cross slightly after independently following a body
    // morph. An explicit outer-layer opt-in gives that garment priority within
    // a bounded overlap, without extending coverage across its openings/hem.
    // NativePart coordinates are decimetres; the authored setting is mm.
    let inward = match &outer.material["fitOccludesGarmentsInwardMm"] {
        serde_json::Value::Null => 1e-5,
        value => {
            let mm = value
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0. && *v <= 40.)
                .ok_or_else(|| format!("{} has invalid fitOccludesGarmentsInwardMm", outer.name))?;
            let dm = (mm / 100.) as f32;
            if mm > 0. && dm == 0. {
                return Err(format!(
                    "{} has underflowing fitOccludesGarmentsInwardMm",
                    outer.name
                ));
            }
            dm.max(1e-5)
        }
    };
    let (outer_triangles, nodes) = collider(outer)?;
    // Validate all lower geometry before changing its index stream.
    let _ = triangles(lower)?;
    let mut retained = Vec::with_capacity(lower.indices.len());
    let mut removed = 0;
    for indices in lower.indices.chunks_exact(3) {
        let point = |i: u32| Vec3::from(lower.positions[i as usize]);
        let (a, b, c) = (point(indices[0]), point(indices[1]), point(indices[2]));
        let normal = (b - a).cross(c - a);
        let covered_triangle = normal.length_squared() > 1e-12
            && (0..=4).all(|row| {
                (0..=4 - row).all(|column| {
                    let b_weight = row as f32 / 4.;
                    let c_weight = column as f32 / 4.;
                    let point = a * (1. - b_weight - c_weight) + b * b_weight + c * c_weight;
                    covered_layer(point, normal.normalize(), &nodes, &outer_triangles, inward)
                })
            });
        if covered_triangle {
            removed += 1;
        } else {
            retained.extend_from_slice(indices);
        }
    }
    lower.indices = retained;
    Ok(removed)
}

/// Returns the candidate subset covered by final fitted cloth, or `None` when
/// this custom part has no explicit candidate opt-in.
pub fn runtime_hidden_faces(
    part: &NativePart,
    coords: &[[f32; 3]],
    face_verts: &[u32],
) -> Result<Option<Vec<u32>>, String> {
    if face_verts.len() % 4 != 0 {
        return Err("canonical body face stream must contain quads".into());
    }
    let Some(candidates) = candidates(part, face_verts.len() / 4)? else {
        return Ok(None);
    };
    let forward_only = forward_only(part)?;
    let (triangles, nodes) = collider(part)?;
    let mut selected = Vec::new();
    for face in candidates {
        let indices: [u32; 4] = face_verts[face * 4..face * 4 + 4]
            .try_into()
            .map_err(|_| "canonical body face is truncated")?;
        let vertex = |index: u32| {
            coords
                .get(index as usize)
                .copied()
                .map(Vec3::from)
                .ok_or("canonical body face index is out of bounds")
        };
        let (a, b, c, d) = (
            vertex(indices[0])?,
            vertex(indices[1])?,
            vertex(indices[2])?,
            vertex(indices[3])?,
        );
        if ![a, b, c, d]
            .iter()
            .all(|p| p.is_finite() && p.abs().max_element() <= MAX_COORD)
        {
            return Err("canonical body has invalid coordinates".into());
        }
        let normal = (b - a).cross(c - a);
        if normal.length_squared() <= 1e-12 {
            continue;
        }
        let normal = normal.normalize();
        // A 5x5 lattice includes edges and off-centre interior samples.
        // This remains a bounded coverage heuristic: openings smaller than
        // one quarter of a body quad may lie between samples. Callers retain
        // the reviewed baseline and restrict candidates to the garment family.
        let fully_sampled = (0..=4).all(|row| {
            let v = row as f32 / 4.;
            (0..=4).all(|column| {
                let u = column as f32 / 4.;
                let point = a.lerp(b, u).lerp(d.lerp(c, u), v);
                if forward_only {
                    covered_forward(point, normal, &nodes, &triangles)
                } else {
                    covered(point, normal, &nodes, &triangles)
                }
            })
        });
        if fully_sampled {
            selected.push(face as u32);
        }
    }
    Ok(Some(selected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn part_with_indices(
        points: Vec<[f32; 3]>,
        indices: Vec<u32>,
        material: serde_json::Value,
    ) -> NativePart {
        let count = points.len();
        NativePart {
            name: "custom:test".into(),
            positions: points,
            indices: indices,
            uvs: vec![[0.; 2]; count],
            refs: vec![[0; 3]; count],
            ref_weights: vec![[1., 0., 0.]; count],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material,
        }
    }
    fn part(points: Vec<[f32; 3]>, material: serde_json::Value) -> NativePart {
        part_with_indices(points, vec![0, 1, 2, 0, 2, 3], material)
    }
    fn coords() -> Vec<[f32; 3]> {
        vec![[-1., -1., 0.], [1., -1., 0.], [1., 1., 0.], [-1., 1., 0.]]
    }
    #[test]
    fn full_quad_is_covered_but_centre_only_and_backside_are_not() {
        let faces = [0, 1, 2, 3];
        let m = json!({"fitBodyMaskCandidates":[0]});
        let front = part(
            vec![
                [-1., -1., 0.03],
                [1., -1., 0.03],
                [1., 1., 0.03],
                [-1., 1., 0.03],
            ],
            m.clone(),
        );
        assert_eq!(
            runtime_hidden_faces(&front, &coords(), &faces).unwrap(),
            Some(vec![0])
        );
        let centre = part(
            vec![
                [-0.2, -0.2, 0.03],
                [0.2, -0.2, 0.03],
                [0.2, 0.2, 0.03],
                [-0.2, 0.2, 0.03],
            ],
            m.clone(),
        );
        assert_eq!(
            runtime_hidden_faces(&centre, &coords(), &faces).unwrap(),
            Some(vec![])
        );
        let back = part(
            vec![
                [-1., -1., 0.03],
                [-1., 1., 0.03],
                [1., 1., 0.03],
                [1., -1., 0.03],
            ],
            m,
        );
        assert_eq!(
            runtime_hidden_faces(&back, &coords(), &faces).unwrap(),
            Some(vec![])
        );
    }
    #[test]
    fn forward_only_requires_cloth_outside_the_body() {
        let faces = [0, 1, 2, 3];
        let legacy = part(
            vec![
                [-1., -1., -0.1],
                [1., -1., -0.1],
                [1., 1., -0.1],
                [-1., 1., -0.1],
            ],
            json!({"fitBodyMaskCandidates":[0]}),
        );
        assert_eq!(
            runtime_hidden_faces(&legacy, &coords(), &faces).unwrap(),
            Some(vec![0])
        );
        let inward = part(
            vec![
                [-1., -1., -0.1],
                [1., -1., -0.1],
                [1., 1., -0.1],
                [-1., 1., -0.1],
            ],
            json!({"fitBodyMaskCandidates":[0], "fitBodyMaskForwardOnly":true}),
        );
        assert_eq!(
            runtime_hidden_faces(&inward, &coords(), &faces).unwrap(),
            Some(vec![])
        );
        let outward = part(
            vec![
                [-1., -1., 0.1],
                [1., -1., 0.1],
                [1., 1., 0.1],
                [-1., 1., 0.1],
            ],
            json!({"fitBodyMaskCandidates":[0], "fitBodyMaskForwardOnly":true}),
        );
        assert_eq!(
            runtime_hidden_faces(&outward, &coords(), &faces).unwrap(),
            Some(vec![0])
        );
        let invalid = part(
            vec![[0., 0., 0.1]; 4],
            json!({"fitBodyMaskCandidates":[0], "fitBodyMaskForwardOnly":1}),
        );
        assert!(runtime_hidden_faces(&invalid, &coords(), &faces).is_err());
    }

    #[test]
    fn openings_and_invalid_candidates_or_geometry_are_rejected() {
        let faces = [0, 1, 2, 3];
        let open = part(
            vec![
                [-1., -1., 0.03],
                [0., -1., 0.03],
                [0., 1., 0.03],
                [-1., 1., 0.03],
            ],
            json!({"fitBodyMaskCandidates":[0]}),
        );
        assert_eq!(
            runtime_hidden_faces(&open, &coords(), &faces).unwrap(),
            Some(vec![])
        );
        let invalid = part(vec![[0., 0., 0.]; 4], json!({"fitBodyMaskCandidates":[1]}));
        assert!(runtime_hidden_faces(&invalid, &coords(), &faces).is_err());
        let mut bad = part(
            vec![[0., 0., 0.03]; 4],
            json!({"fitBodyMaskCandidates":[0]}),
        );
        bad.positions[0][0] = f32::NAN;
        assert!(runtime_hidden_faces(&bad, &coords(), &faces).is_err());
    }
    #[test]
    fn off_centre_opening_is_not_missed_by_lattice_sampling() {
        let faces = [0, 1, 2, 3];
        // This ring covers the four corners and centre used by the former
        // five-point heuristic, but has a hole at its (-.5, -.5) lattice
        // point.  A 5x5 test must retain the body quad.
        let ring = part_with_indices(
            vec![
                [-1., -1., 0.03],
                [1., -1., 0.03],
                [1., 1., 0.03],
                [-1., 1., 0.03],
                [-0.65, -0.65, 0.03],
                [-0.35, -0.65, 0.03],
                [-0.35, -0.35, 0.03],
                [-0.65, -0.35, 0.03],
            ],
            vec![
                0, 1, 5, 0, 5, 4, 1, 2, 6, 1, 6, 5, 2, 3, 7, 2, 7, 6, 3, 0, 4, 3, 4, 7,
            ],
            json!({"fitBodyMaskCandidates":[0]}),
        );
        assert_eq!(
            runtime_hidden_faces(&ring, &coords(), &faces).unwrap(),
            Some(vec![])
        );
    }

    #[test]
    fn layer_culling_removes_only_fully_covered_lower_triangles() {
        let outer = part_with_indices(
            vec![
                [-1., -1., 0.03],
                [1., -1., 0.03],
                [1., 1., 0.03],
                [-1., 1., 0.03],
            ],
            vec![0, 1, 2, 0, 2, 3],
            json!({}),
        );
        let mut covered_lower = part_with_indices(
            vec![[-0.5, -0.5, 0.], [0.5, -0.5, 0.], [0., 0.5, 0.]],
            vec![0, 1, 2],
            json!({}),
        );
        assert_eq!(
            cull_fully_occluded_triangles(&outer, &mut covered_lower).unwrap(),
            1
        );
        assert!(covered_lower.indices.is_empty());

        let open_outer = part_with_indices(
            vec![
                [-1., -1., 0.03],
                [0., -1., 0.03],
                [0., 1., 0.03],
                [-1., 1., 0.03],
            ],
            vec![0, 1, 2, 0, 2, 3],
            json!({}),
        );
        let mut exposed_lower = part_with_indices(
            vec![[-0.5, -0.5, 0.], [0.5, -0.5, 0.], [0., 0.5, 0.]],
            vec![0, 1, 2],
            json!({}),
        );
        assert_eq!(
            cull_fully_occluded_triangles(&open_outer, &mut exposed_lower).unwrap(),
            0
        );
        assert_eq!(exposed_lower.indices, vec![0, 1, 2]);
    }

    #[test]
    fn layer_culling_requires_outer_surface_in_front_of_lower_surface() {
        let mut lower = part_with_indices(
            vec![[-0.5, -0.5, 0.], [0.5, -0.5, 0.], [0., 0.5, 0.]],
            vec![0, 1, 2],
            json!({}),
        );
        let behind = part_with_indices(
            vec![
                [-1., -1., -0.03],
                [1., -1., -0.03],
                [1., 1., -0.03],
                [-1., 1., -0.03],
            ],
            vec![0, 1, 2, 0, 2, 3],
            json!({}),
        );
        assert_eq!(
            cull_fully_occluded_triangles(&behind, &mut lower).unwrap(),
            0
        );
        assert_eq!(lower.indices, vec![0, 1, 2]);

        let in_front = part_with_indices(
            vec![
                [-1., -1., 0.03],
                [1., -1., 0.03],
                [1., 1., 0.03],
                [-1., 1., 0.03],
            ],
            vec![0, 1, 2, 0, 2, 3],
            json!({}),
        );
        assert_eq!(
            cull_fully_occluded_triangles(&in_front, &mut lower).unwrap(),
            1
        );
        assert!(lower.indices.is_empty());
    }

    #[test]
    fn layer_overlap_opt_in_is_bounded_and_retains_exposed_hem() {
        let lower = part_with_indices(
            vec![[-0.5, -0.5, 0.], [0.5, -0.5, 0.], [0., 0.5, 0.]],
            vec![0, 1, 2],
            json!({}),
        );
        let mut outer = part(
            vec![
                [-1., -1., -0.2],
                [1., -1., -0.2],
                [1., 1., -0.2],
                [-1., 1., -0.2],
            ],
            json!({"fitOccludesGarmentsInwardMm": 25.}),
        );
        let mut overlapping = lower.clone();
        assert_eq!(
            cull_fully_occluded_triangles(&outer, &mut overlapping).unwrap(),
            1
        );
        // Only topology is culled; source shape, UVs and deformation anchors stay exact.
        assert_eq!(overlapping.positions, lower.positions);
        assert_eq!(overlapping.uvs, lower.uvs);
        assert_eq!(overlapping.refs, lower.refs);
        assert_eq!(overlapping.ref_weights, lower.ref_weights);

        outer.material["fitOccludesGarmentsInwardMm"] = json!(15.);
        let mut beyond_budget = lower.clone();
        assert_eq!(
            cull_fully_occluded_triangles(&outer, &mut beyond_budget).unwrap(),
            0
        );
        assert_eq!(beyond_budget.indices, lower.indices);

        outer.material["fitOccludesGarmentsInwardMm"] = json!(25.);
        outer.positions[1][0] = -0.1;
        outer.positions[2][0] = -0.1;
        let mut exposed_hem = lower.clone();
        assert_eq!(
            cull_fully_occluded_triangles(&outer, &mut exposed_hem).unwrap(),
            0
        );
        assert_eq!(exposed_hem.indices, lower.indices);
    }

    #[test]
    fn invalid_layer_overlap_does_not_mutate_lower_geometry() {
        let mut outer = part(coords(), json!({}));
        let mut lower = part(coords(), json!({}));
        let before = lower.indices.clone();
        for value in [
            json!(-1.),
            json!(40.01),
            json!(true),
            json!("5"),
            json!([]),
            json!(1e-100),
        ] {
            outer.material["fitOccludesGarmentsInwardMm"] = value;
            assert!(cull_fully_occluded_triangles(&outer, &mut lower).is_err());
            assert_eq!(lower.indices, before);
        }
    }

    #[test]
    fn omitted_and_zero_layer_overlap_keep_legacy_forward_bounds() {
        for (z, expected) in [(-1e-6, 0), (0., 1), (1e-6, 1)] {
            let mut outer = part(
                vec![[-1., -1., z], [1., -1., z], [1., 1., z], [-1., 1., z]],
                json!({}),
            );
            let lower = part_with_indices(
                vec![[-0.5, -0.5, 0.], [0.5, -0.5, 0.], [0., 0.5, 0.]],
                vec![0, 1, 2],
                json!({}),
            );
            let mut omitted = lower.clone();
            assert_eq!(
                cull_fully_occluded_triangles(&outer, &mut omitted).unwrap(),
                expected
            );
            outer.material["fitOccludesGarmentsInwardMm"] = json!(0.);
            let mut zero = lower.clone();
            assert_eq!(
                cull_fully_occluded_triangles(&outer, &mut zero).unwrap(),
                expected
            );
            assert_eq!(omitted.indices, zero.indices);
        }
    }
}
