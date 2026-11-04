//! Conservative collision clearance for generated hair.
//!
//! Hair proxies and the baked procedural styles are body-referenced, but their
//! authored offsets can pass through an unusually shaped neck, shoulder, or a
//! high-collared garment.  This is deliberately a small bind-pose correction,
//! rather than simulation: the retained body references give the adjusted hair
//! the same skinning influences as before.
use crate::{native_character::NativePart, wardrobe_assets::CustomGarment};
use bevy::prelude::Vec3;
use std::collections::BTreeMap;

/// Distance in MakeHuman decimetres.  Characters are scaled by 0.1 later, so
/// this becomes a 12 mm visible gap in native export/runtime coordinates.
pub const HAIR_CLEARANCE: f32 = 0.12;
/// Do not turn a bad proxy into a radically different hairstyle.
pub const MAX_HAIR_CORRECTION: f32 = 0.35;
const CELL: f32 = 0.75;
const PASSES: usize = 3;
/// The scalp begins in the upper third of a canonical MakeHuman body. Hair
/// proxies are authored against this region, which needs a lateral cranial
/// gate so shoulders at the same height remain useful collision targets.
const HEAD_COLLIDER_FRACTION: f32 = 0.68;
const CROWN_SAMPLE_FRACTION: f32 = 0.15;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HairFitStats {
    pub corrected_vertices: usize,
    pub max_displacement: f32,
    /// Vertices that remain on the wrong side of a nearby collider after the
    /// bounded correction.  Deeply invalid source geometry is never moved
    /// farther than `MAX_HAIR_CORRECTION` just to make this zero.
    pub remaining_violations: usize,
}

#[derive(Clone, Copy)]
struct Triangle {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    outward: Vec3,
    clearance: f32,
}

impl Triangle {
    fn closest(self, point: Vec3) -> Vec3 {
        // Real-Time Collision Detection, Christer Ericson, closest point on a
        // triangle.  Keeping this local avoids a mesh or physics dependency.
        let ab = self.b - self.a;
        let ac = self.c - self.a;
        let ap = point - self.a;
        let d1 = ab.dot(ap);
        let d2 = ac.dot(ap);
        if d1 <= 0. && d2 <= 0. {
            return self.a;
        }
        let bp = point - self.b;
        let d3 = ab.dot(bp);
        let d4 = ac.dot(bp);
        if d3 >= 0. && d4 <= d3 {
            return self.b;
        }
        let vc = d1 * d4 - d3 * d2;
        if vc <= 0. && d1 >= 0. && d3 <= 0. {
            return self.a + ab * (d1 / (d1 - d3));
        }
        let cp = point - self.c;
        let d5 = ab.dot(cp);
        let d6 = ac.dot(cp);
        if d6 >= 0. && d5 <= d6 {
            return self.c;
        }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0. && d2 >= 0. && d6 <= 0. {
            return self.a + ac * (d2 / (d2 - d6));
        }
        let va = d3 * d6 - d5 * d4;
        if va <= 0. && (d4 - d3) >= 0. && (d5 - d6) >= 0. {
            return self.b + (self.c - self.b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
        }
        let denom = 1. / (va + vb + vc);
        self.a + ab * (vb * denom) + ac * (vc * denom)
    }
}

fn cell(point: Vec3) -> [i32; 3] {
    [
        (point.x / CELL).floor() as i32,
        (point.y / CELL).floor() as i32,
        (point.z / CELL).floor() as i32,
    ]
}

fn add_part(
    part: &NativePart,
    clearance: f32,
    triangles: &mut Vec<Triangle>,
    grid: &mut BTreeMap<[i32; 3], Vec<usize>>,
) -> Result<(), String> {
    let mut centre = Vec3::ZERO;
    let mut count = 0usize;
    for &index in &part.indices {
        let Some(position) = part.positions.get(index as usize).copied().map(Vec3::from) else {
            continue;
        };
        if position.is_finite() {
            centre += position;
            count += 1;
        }
    }
    let centre = if count > 0 {
        centre / count as f32
    } else {
        Vec3::ZERO
    };
    for indices in part.indices.chunks_exact(3) {
        let vertices = [indices[0], indices[1], indices[2]].map(|index| {
            part.positions
                .get(index as usize)
                .copied()
                .map(Vec3::from)
                .ok_or_else(|| format!("{} has an invalid triangle index", part.name))
        });
        let [a, b, c] = vertices;
        let (a, b, c) = (a?, b?, c?);
        if !a.is_finite() || !b.is_finite() || !c.is_finite() {
            continue;
        }
        let mut outward = (b - a).cross(c - a).normalize_or_zero();
        if outward == Vec3::ZERO {
            continue;
        }
        // Bake meshes do not all use the same winding.  A closed body or
        // garment shell nevertheless has its visible side pointing away from
        // its centre, which gives a stable outward direction for a local
        // clearance correction without requiring a physics volume.
        if outward.dot((a + b + c) / 3. - centre) < 0. {
            outward = -outward;
        }
        let triangle = Triangle {
            a,
            b,
            c,
            outward,
            clearance,
        };
        let index = triangles.len();
        triangles.push(triangle);
        let reach = MAX_HAIR_CORRECTION + clearance;
        let low = cell(a.min(b).min(c) - Vec3::splat(reach));
        let high = cell(a.max(b).max(c) + Vec3::splat(reach));
        // A malformed collider must not allocate an unbounded uniform grid.
        if (0..3)
            .map(|i| high[i] as i64 - low[i] as i64 + 1)
            .fold(1i64, i64::saturating_mul)
            > 4096
        {
            continue;
        }
        for x in low[0]..=high[0] {
            for y in low[1]..=high[1] {
                for z in low[2]..=high[2] {
                    grid.entry([x, y, z]).or_default().push(index);
                }
            }
        }
    }
    Ok(())
}

/// Adds the body as a hair collider except inside its cranial scalp volume.
///
/// A body has a meaningful vertical span, unlike the small planar test
/// colliders used below.  In that degenerate case there is no trustworthy
/// head boundary, so preserving the authored root is safer than inventing
/// one.  The source proxy's fitted scalp coordinates are already correct.
fn add_body_except_scalp(
    body: &NativePart,
    triangles: &mut Vec<Triangle>,
    grid: &mut BTreeMap<[i32; 3], Vec<usize>>,
) -> Result<(), String> {
    let (min_y, max_y) = body
        .positions
        .iter()
        .filter_map(|p| p[1].is_finite().then_some(p[1]))
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), y| {
            (low.min(y), high.max(y))
        });
    if !min_y.is_finite() || max_y - min_y < CELL {
        return Ok(());
    }
    let span = max_y - min_y;
    let cutoff = min_y + span * HEAD_COLLIDER_FRACTION;
    // Sample the crown alone to establish the horizontal skull footprint.
    // Shoulders can be as high as the neck but are wider/deeper, so excluding
    // every triangle above `cutoff` would silently remove their protection.
    let crown_floor = max_y - span * CROWN_SAMPLE_FRACTION;
    let (min_x, max_x, min_z, max_z) = body
        .positions
        .iter()
        .filter(|position| position[1] >= crown_floor)
        .fold(
            (
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
            ),
            |(min_x, max_x, min_z, max_z), position| {
                (
                    min_x.min(position[0]),
                    max_x.max(position[0]),
                    min_z.min(position[2]),
                    max_z.max(position[2]),
                )
            },
        );
    if !min_x.is_finite() || !min_z.is_finite() {
        return add_part(body, HAIR_CLEARANCE, triangles, grid);
    }
    // 75 mm in source space covers the crown sample's narrower top while
    // staying well inside shoulder width on normal MakeHuman proportions.
    let pad = CELL;
    let mut torso = body.clone();
    torso.indices = body
        .indices
        .chunks_exact(3)
        .filter(|triangle| {
            let centroid = triangle
                .iter()
                .filter_map(|&index| body.positions.get(index as usize))
                .fold(Vec3::ZERO, |sum, position| sum + Vec3::from(*position))
                / 3.;
            let scalp = centroid.y >= cutoff
                && centroid.x >= min_x - pad
                && centroid.x <= max_x + pad
                && centroid.z >= min_z - pad
                && centroid.z <= max_z + pad;
            !scalp
        })
        .flatten()
        .copied()
        .collect();
    add_part(&torso, HAIR_CLEARANCE, triangles, grid)
}

fn nearest_violation(
    point: Vec3,
    candidates: &[usize],
    triangles: &[Triangle],
) -> Option<(Vec3, f32, f32)> {
    // Choose the actual nearest violating surface.  Ordering by signed plane
    // distance instead makes a farther triangle on the other side of a thin
    // head/collar win over the scalp directly under a root, which can pull
    // rear hair cards through their attachment.
    let mut nearest = None::<(Vec3, f32, f32, f32)>;
    for &index in candidates {
        let triangle = triangles[index];
        let closest = triangle.closest(point);
        let distance_squared = point.distance_squared(closest);
        let signed = (point - closest).dot(triangle.outward);
        if distance_squared <= (MAX_HAIR_CORRECTION + triangle.clearance).powi(2)
            && signed < triangle.clearance
            && nearest.is_none_or(|(_, best_distance_squared, _, _)| {
                distance_squared < best_distance_squared
            })
        {
            nearest = Some((
                triangle.outward,
                distance_squared,
                signed,
                triangle.clearance,
            ));
        }
    }
    nearest.map(|(outward, _, signed, clearance)| (outward, signed, clearance))
}

/// Pushes a hair mesh clear of the lower body and supplied wardrobe mesh.
///
/// Hair proxies already carry MakeHuman's authored scalp offsets. Treating
/// the head as a clearance collider moves almost every vertex of short card
/// styles onto an artificial shell, exposing a detached band below the rear
/// hairline. The head is therefore excluded while the neck and torso remain
/// collision targets for hanging hair. Call this only for hair parts;
/// eyebrows, eyelashes, eyes, and hats are intentionally outside its scope.
pub fn fit_hair_clearance(
    hair: &mut NativePart,
    body: &NativePart,
    clothing: &[NativePart],
) -> Result<HairFitStats, String> {
    let mut triangles = Vec::new();
    let mut grid = BTreeMap::new();
    add_body_except_scalp(body, &mut triangles, &mut grid)?;
    for part in clothing {
        add_part(part, HAIR_CLEARANCE, &mut triangles, &mut grid)?;
    }
    let mut stats = HairFitStats::default();
    for position in &mut hair.positions {
        let mut point = Vec3::from(*position);
        if !point.is_finite() {
            return Err(format!("{} has a non-finite hair vertex", hair.name));
        }
        let original = point;
        for _ in 0..PASSES {
            let Some(candidates) = grid.get(&cell(point)) else {
                break;
            };
            let Some((outward, signed, clearance)) =
                nearest_violation(point, candidates, &triangles)
            else {
                break;
            };
            let needed = clearance - signed;
            let remaining = (MAX_HAIR_CORRECTION - point.distance(original)).max(0.);
            if remaining <= 0. {
                break;
            }
            point += outward * needed.min(remaining);
        }
        let displacement = point.distance(original);
        if displacement > 0. {
            stats.corrected_vertices += 1;
            stats.max_displacement = stats.max_displacement.max(displacement);
            *position = point.to_array();
        }
    }
    for position in &hair.positions {
        let point = Vec3::from(*position);
        let Some(candidates) = grid.get(&cell(point)) else {
            continue;
        };
        if candidates.iter().any(|&index| {
            let triangle = triangles[index];
            let closest = triangle.closest(point);
            point.distance_squared(closest) <= (MAX_HAIR_CORRECTION + triangle.clearance).powi(2)
                && (point - closest).dot(triangle.outward) < triangle.clearance - 0.000_1
        }) {
            stats.remaining_violations += 1;
        }
    }
    Ok(stats)
}

fn quad_normals(points: &[[f32; 3]], faces: &[u32]) -> Result<Vec<Vec3>, String> {
    if faces.len() % 4 != 0 {
        return Err("body face stream must contain quads".into());
    }
    let mut normals = vec![Vec3::ZERO; points.len()];
    for face in faces.chunks_exact(4) {
        let vertex = |index: u32| {
            points
                .get(index as usize)
                .copied()
                .map(Vec3::from)
                .ok_or("body face index is out of bounds")
        };
        let [a, b, c, d] = [
            vertex(face[0])?,
            vertex(face[1])?,
            vertex(face[2])?,
            vertex(face[3])?,
        ];
        let first = (b - a).cross(c - a);
        let second = (c - a).cross(d - a);
        for &index in &[face[0], face[1], face[2]] {
            normals[index as usize] += first;
        }
        for &index in &[face[0], face[2], face[3]] {
            normals[index as usize] += second;
        }
    }
    Ok(normals
        .into_iter()
        .map(|normal| normal.normalize_or_zero())
        .collect())
}

fn garment_normals(garment: &CustomGarment) -> Result<Vec<Vec3>, String> {
    let mut normals = vec![Vec3::ZERO; garment.positions_dm.len()];
    for triangle in garment.indices.chunks_exact(3) {
        let point = |index: u32| {
            garment
                .positions_dm
                .get(index as usize)
                .copied()
                .map(Vec3::from)
                .ok_or("garment triangle index is out of bounds")
        };
        let [a, b, c] = [
            point(triangle[0])?,
            point(triangle[1])?,
            point(triangle[2])?,
        ];
        let normal = (b - a).cross(c - a);
        for &index in triangle {
            normals[index as usize] += normal;
        }
    }
    Ok(normals
        .into_iter()
        .map(|normal| normal.normalize_or_zero())
        .collect())
}

/// Clears only authored outward custom-hair surfaces from the current head.
/// Closed wigs deliberately retain their inward shell, while the source-facing
/// surface is held outside the morphed scalp after adaptive fitting.
pub fn fit_custom_head_clearance(
    garment: &CustomGarment,
    hair: &mut NativePart,
    neutral: &[[f32; 3]],
    current: &[[f32; 3]],
    faces: &[u32],
    regions: &[u8],
) -> Result<HairFitStats, String> {
    garment.validate()?;
    if neutral.len() != current.len()
        || regions.len() != neutral.len()
        || hair.positions.len() != garment.positions_dm.len()
    {
        return Err("custom head clearance has inconsistent body or garment lengths".into());
    }
    let neutral_normals = quad_normals(neutral, faces)?;
    let current_normals = quad_normals(current, faces)?;
    let authored_normals = garment_normals(garment)?;
    let shell = hair.material["cartoonShell"] == true;
    let mut stats = HairFitStats::default();
    for index in 0..hair.positions.len() {
        let refs = garment.refs[index];
        let weights = garment.weights[index];
        if refs
            .iter()
            .any(|&reference| reference as usize >= current.len())
        {
            return Err("garment reference is outside this body".into());
        }
        if refs
            .iter()
            .any(|&reference| regions[reference as usize] != 0)
        {
            continue;
        }
        let mut neutral_normal = Vec3::ZERO;
        let mut current_normal = Vec3::ZERO;
        let mut anchor = Vec3::ZERO;
        for (&reference, &weight) in refs.iter().zip(weights.iter()) {
            let reference = reference as usize;
            neutral_normal += neutral_normals[reference] * weight;
            current_normal += current_normals[reference] * weight;
            anchor += Vec3::from(current[reference]) * weight;
        }
        neutral_normal = neutral_normal.normalize_or_zero();
        current_normal = current_normal.normalize_or_zero();
        if neutral_normal == Vec3::ZERO
            || current_normal == Vec3::ZERO
            || authored_normals[index].dot(neutral_normal) <= 0.2
        {
            continue;
        }
        let direct = refs[0] == refs[1] && refs[1] == refs[2];
        let clearance = if direct {
            0.01
        } else if shell {
            HAIR_CLEARANCE
        } else {
            0.01
        };
        let original = Vec3::from(hair.positions[index]);
        if !original.is_finite() {
            return Err(format!("{} has a non-finite custom hair vertex", hair.name));
        }
        let signed = (original - anchor).dot(current_normal);
        if signed >= clearance {
            continue;
        }
        let correction = (clearance - signed).min(MAX_HAIR_CORRECTION);
        let corrected = original + current_normal * correction;
        hair.positions[index] = corrected.to_array();
        stats.corrected_vertices += 1;
        stats.max_displacement = stats.max_displacement.max(correction);
        if (corrected - anchor).dot(current_normal) < clearance - 0.000_1 {
            stats.remaining_violations += 1;
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wardrobe_assets::{CustomGarment, GarmentSlot};
    use serde_json::json;

    fn part(name: &str, positions: Vec<[f32; 3]>, indices: Vec<u32>) -> NativePart {
        let n = positions.len();
        NativePart {
            name: name.into(),
            positions,
            indices,
            uvs: vec![[0.; 2]; n],
            refs: vec![[0; 3]; n],
            ref_weights: vec![[1., 0., 0.]; n],
            rigid_joint: None,
            material: json!({}),
        }
    }

    fn custom(
        points: Vec<[f32; 3]>,
        indices: Vec<u32>,
        refs: Vec<[u32; 3]>,
        material: serde_json::Value,
    ) -> CustomGarment {
        let count = points.len();
        CustomGarment {
            id: "test-hair".into(),
            name: "Test hair".into(),
            slot: GarmentSlot::Hair,
            positions_dm: points.clone(),
            indices,
            uvs: vec![[0., 0.]; count],
            refs,
            weights: vec![[1., 0., 0.]; count],
            reference_points_dm: vec![[0., 0., 0.]; count],
            material,
            hidden_faces: vec![],
        }
    }

    fn head_plane() -> (Vec<[f32; 3]>, Vec<u32>, Vec<u8>) {
        (
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            vec![0, 1, 2, 2],
            vec![0, 0, 0],
        )
    }

    #[test]
    fn custom_head_clearance_moves_outward_surface_but_not_inner_shell() {
        let (body, faces, regions) = head_plane();
        let garment = custom(
            vec![[0., 0., -0.2], [1., 0., -0.2], [0., 1., -0.2]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"cartoonShell":true}),
        );
        let mut hair = part("custom:outer", garment.positions_dm.clone(), vec![0, 1, 2]);
        hair.refs = garment.refs.clone();
        hair.ref_weights = garment.weights.clone();
        hair.material = garment.material.clone();
        let stats =
            fit_custom_head_clearance(&garment, &mut hair, &body, &body, &faces, &regions).unwrap();
        assert_eq!(stats.corrected_vertices, 3);
        assert!((hair.positions[0][2] - HAIR_CLEARANCE).abs() < 0.000_1);
        let inner = custom(
            vec![[0., 0., 0.02], [0., 1., 0.02], [1., 0., 0.02]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"cartoonShell":true}),
        );
        let mut inner_part = part("custom:inner", inner.positions_dm.clone(), vec![0, 1, 2]);
        assert_eq!(
            fit_custom_head_clearance(&inner, &mut inner_part, &body, &body, &faces, &regions)
                .unwrap(),
            HairFitStats::default()
        );
        assert_eq!(inner_part.positions, inner.positions_dm);
    }

    #[test]
    fn custom_head_clearance_caps_deep_and_keeps_direct_lining_small() {
        let (body, faces, regions) = head_plane();
        let deep = custom(
            vec![[0., 0., -0.5], [1., 0., -0.5], [0., 1., -0.5]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"cartoonShell":true}),
        );
        let mut deep_part = part("custom:deep", deep.positions_dm.clone(), vec![0, 1, 2]);
        deep_part.material = deep.material.clone();
        let stats =
            fit_custom_head_clearance(&deep, &mut deep_part, &body, &body, &faces, &regions)
                .unwrap();
        assert!((stats.max_displacement - MAX_HAIR_CORRECTION).abs() < 0.000_1);
        assert_eq!(stats.remaining_violations, 3);
        let lining = custom(
            vec![[0., 0., 0.05], [1., 0., 0.05], [0., 1., 0.05]],
            vec![0, 1, 2],
            vec![[0, 0, 0], [1, 1, 1], [2, 2, 2]],
            json!({"cartoonShell":true}),
        );
        let mut lining_part = part("custom:lining", lining.positions_dm.clone(), vec![0, 1, 2]);
        lining_part.material = lining.material.clone();
        assert_eq!(
            fit_custom_head_clearance(&lining, &mut lining_part, &body, &body, &faces, &regions)
                .unwrap(),
            HairFitStats::default()
        );
        let outer = custom(
            vec![[0., 0., 0.08], [1., 0., 0.08], [0., 1., 0.08]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"cartoonShell":true}),
        );
        let mut outer_part = part("custom:outer", outer.positions_dm.clone(), vec![0, 1, 2]);
        outer_part.material = outer.material.clone();
        fit_custom_head_clearance(&outer, &mut outer_part, &body, &body, &faces, &regions).unwrap();
        assert!((outer_part.positions[0][2] - HAIR_CLEARANCE).abs() < 0.000_1);
    }

    #[test]
    fn pushes_hair_outside_body_and_clothing_with_bounded_clearance() {
        let body = part(
            "Body",
            vec![[-1., 0., 0.], [0., 0., 2.], [1., 0., 0.]],
            vec![0, 1, 2],
        );
        let collar = part(
            "high_collar",
            vec![[-1., 0.05, 0.], [0., 0.05, 2.], [1., 0.05, 0.]],
            vec![0, 1, 2],
        );
        let mut hair = part("Hair", vec![[0., 0.06, 0.8]], vec![]);
        let stats = fit_hair_clearance(&mut hair, &body, &[collar]).unwrap();
        assert_eq!(stats.corrected_vertices, 1);
        assert!(hair.positions[0][1] >= 0.05 + HAIR_CLEARANCE - 0.000_1);
        assert!(stats.max_displacement <= MAX_HAIR_CORRECTION + f32::EPSILON);
        assert_eq!(stats.remaining_violations, 0);
    }

    #[test]
    fn preserves_authored_scalp_roots_without_wardrobe_collision() {
        let body = part(
            "Body",
            vec![[-1., 0., 0.], [0., 0., 2.], [1., 0., 0.]],
            vec![0, 1, 2],
        );
        // This vertex is deliberately inside the scalp plane: source hair
        // cards may overlap their attachment surface, and that overlap must
        // not turn into a visible shell gap.
        let mut hair = part("Hair", vec![[0., -0.1, 0.8]], vec![]);
        assert_eq!(
            fit_hair_clearance(&mut hair, &body, &[]).unwrap(),
            HairFitStats::default()
        );
        assert_eq!(hair.positions, vec![[0., -0.1, 0.8]]);
    }

    #[test]
    fn caps_a_deep_wardrobe_penetration_and_reports_it() {
        let body = part(
            "Body",
            vec![[-1., 0., 0.], [0., 0., 2.], [1., 0., 0.]],
            vec![0, 1, 2],
        );
        let collar = part(
            "collar",
            vec![[-1., 0., 0.], [0., 0., 2.], [1., 0., 0.]],
            vec![0, 1, 2],
        );
        let mut hair = part("Hair", vec![[0., -0.34, 0.8]], vec![]);
        let stats = fit_hair_clearance(&mut hair, &body, &[collar]).unwrap();
        assert_eq!(stats.corrected_vertices, 1);
        assert!((stats.max_displacement - MAX_HAIR_CORRECTION).abs() < 0.000_1);
        assert_eq!(stats.remaining_violations, 1);
    }

    #[test]
    fn excludes_scalp_but_clears_hanging_hair_and_shoulders() {
        let body = part(
            "Body",
            vec![
                [-1., 0., 0.],
                [0., 0., 2.],
                [1., 0., 0.],
                [-1., 10., 0.],
                [0., 10., 2.],
                [1., 10., 0.],
                [3., 10., 0.],
                [4., 10., 2.],
                [5., 10., 0.],
                [-1., 12., 0.],
                [0., 12., 2.],
                [1., 12., 0.],
            ],
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
        );
        let mut hair = part(
            "Hair",
            vec![[0., -0.02, 0.8], [0., 10.02, 0.8], [4., 10.02, 0.8]],
            vec![],
        );

        let stats = fit_hair_clearance(&mut hair, &body, &[]).unwrap();

        assert_eq!(stats.corrected_vertices, 2);
        assert!(hair.positions[0][1] <= -HAIR_CLEARANCE + 0.000_1);
        assert_eq!(hair.positions[1], [0., 10.02, 0.8]);
        assert!(hair.positions[2][1] >= 10. + HAIR_CLEARANCE - 0.000_1);
    }
}
