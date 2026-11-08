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

const HEAD_GRID_REFERENCE_BUDGET: usize = 2_000_000;
// Detection must reach farther than the correction budget so deeply embedded
// roots are reported even when a 35 mm correction cannot clear them.
const HEAD_SURFACE_QUERY_REACH: f32 = 1.5;

/// Builds current region-0 head triangles, optionally including region-1
/// neck triangles for source hair extending below the scalp. Face winding is aligned to
/// local vertex normals computed from the canonical quad stream, rather than
/// a body-centre heuristic that is wrong at the jaw and neck.
fn current_head_collider(
    current: &[[f32; 3]],
    faces: &[u32],
    regions: &[u8],
    clearance: f32,
    include_neck: bool,
) -> Result<(Vec<Triangle>, BTreeMap<[i32; 3], Vec<usize>>), String> {
    if current.len() != regions.len() || faces.len() % 4 != 0 {
        return Err("custom head surface clearance has invalid body streams".into());
    }
    if current.iter().any(|point| {
        let point = Vec3::from(*point);
        !point.is_finite() || point.abs().max_element() > 100.
    }) {
        return Err("custom head surface clearance body coordinates are invalid".into());
    }
    let normals = quad_normals(current, faces)?;
    let mut triangles = Vec::new();
    let mut grid: BTreeMap<[i32; 3], Vec<usize>> = BTreeMap::new();
    let mut references = 0usize;
    let mut head_triangles = 0usize;
    let reach = HEAD_SURFACE_QUERY_REACH;
    for face in faces.chunks_exact(4) {
        if face.iter().any(|&index| index as usize >= current.len()) {
            return Err("custom head surface clearance face index is out of bounds".into());
        }
        let is_head = face.iter().all(|&index| regions[index as usize] == 0);
        if !is_head && (!include_neck || face.iter().any(|&index| regions[index as usize] > 1)) {
            continue;
        }
        for tri in [[face[0], face[1], face[2]], [face[0], face[2], face[3]]] {
            let [a, b, c] = tri.map(|index| Vec3::from(current[index as usize]));
            let mut outward = (b - a).cross(c - a).normalize_or_zero();
            if outward == Vec3::ZERO {
                continue;
            }
            if is_head {
                head_triangles += 1;
            }
            let local = tri
                .into_iter()
                .map(|index| normals[index as usize])
                .sum::<Vec3>()
                .normalize_or_zero();
            if local != Vec3::ZERO && outward.dot(local) < 0. {
                outward = -outward;
            }
            let index = triangles.len();
            triangles.push(Triangle {
                a,
                b,
                c,
                outward,
                clearance,
            });
            let low = cell(a.min(b).min(c) - Vec3::splat(reach));
            let high = cell(a.max(b).max(c) + Vec3::splat(reach));
            let cells = (0..3)
                .map(|axis| high[axis] as i64 - low[axis] as i64 + 1)
                .fold(1i64, i64::saturating_mul);
            if cells <= 0
                || cells > 4096
                || references.saturating_add(cells as usize) > HEAD_GRID_REFERENCE_BUDGET
            {
                return Err(
                    "custom head surface clearance collider exceeds bounded grid budget".into(),
                );
            }
            references += cells as usize;
            for x in low[0]..=high[0] {
                for y in low[1]..=high[1] {
                    for z in low[2]..=high[2] {
                        grid.entry([x, y, z]).or_default().push(index);
                    }
                }
            }
        }
    }
    if head_triangles == 0 {
        return Err("custom head surface clearance has no region-0 triangles".into());
    }
    Ok((triangles, grid))
}

/// Select the nearest valid surface first.  A farther inward-facing triangle
/// must never override a clear nearest scalp triangle.
fn nearest_surface(
    point: Vec3,
    candidates: &[usize],
    triangles: &[Triangle],
) -> Option<(Vec3, f32)> {
    candidates
        .iter()
        .filter_map(|&index| {
            let triangle = triangles[index];
            let closest = triangle.closest(point);
            let distance_squared = point.distance_squared(closest);
            (distance_squared <= HEAD_SURFACE_QUERY_REACH.powi(2)).then_some((
                triangle.outward,
                (point - closest).dot(triangle.outward),
                distance_squared,
            ))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(outward, signed, _)| (outward, signed))
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
fn fit_custom_head_surface_clearance(
    garment: &CustomGarment,
    hair: &mut NativePart,
    neutral: &[[f32; 3]],
    current: &[[f32; 3]],
    faces: &[u32],
    regions: &[u8],
    clearance_mm: f32,
) -> Result<HairFitStats, String> {
    // Native coordinates are decimetres: 100 mm, not 10 mm, per unit.
    let clearance = clearance_mm / 100.;
    let (triangles, grid) = current_head_collider(
        current,
        faces,
        regions,
        clearance,
        garment.head_neck_clearance()?,
    )?;
    let neutral_normals = quad_normals(neutral, faces)?;
    let authored_normals = garment_normals(garment)?;
    let mut eligible = vec![false; hair.positions.len()];
    for index in 0..hair.positions.len() {
        let refs = garment.refs[index];
        let weights = garment.weights[index];
        if refs
            .iter()
            .any(|&reference| reference as usize >= neutral.len())
        {
            return Err("garment reference is outside this body".into());
        }
        if refs
            .iter()
            .any(|&reference| regions[reference as usize] != 0)
        {
            continue;
        }
        let normal = refs
            .iter()
            .zip(weights)
            .map(|(&reference, weight)| neutral_normals[reference as usize] * weight)
            .sum::<Vec3>()
            .normalize_or_zero();
        eligible[index] = normal != Vec3::ZERO && authored_normals[index].dot(normal) > 0.2;
    }
    let mut stats = HairFitStats::default();
    for (index, position) in hair.positions.iter_mut().enumerate() {
        if !eligible[index] {
            continue;
        }
        let original = Vec3::from(*position);
        if !original.is_finite() {
            return Err(format!("{} has a non-finite custom hair vertex", hair.name));
        }
        let mut point = original;
        for _ in 0..PASSES {
            let Some(candidates) = grid.get(&cell(point)) else {
                break;
            };
            let Some((outward, signed)) = nearest_surface(point, candidates, &triangles) else {
                break;
            };
            if signed >= clearance {
                break;
            }
            let proposal = point + outward * (clearance - signed);
            let delta = proposal - original;
            point = original + delta.clamp_length_max(MAX_HAIR_CORRECTION);
        }
        let displacement = point.distance(original);
        if displacement > 0. {
            stats.corrected_vertices += 1;
            stats.max_displacement = stats.max_displacement.max(displacement);
            *position = point.to_array();
        }
    }
    for (index, position) in hair.positions.iter().enumerate() {
        if !eligible[index] {
            continue;
        }
        let point = Vec3::from(*position);
        let Some(candidates) = grid.get(&cell(point)) else {
            continue;
        };
        if let Some((_, signed)) = nearest_surface(point, candidates, &triangles) {
            if signed < clearance - 0.000_1 {
                stats.remaining_violations += 1;
            }
        }
    }
    Ok(stats)
}

/// A source explicitly classified as a single exterior cap may share its
/// clearance displacement through a continuous trilinear field. Nearby source
/// vertices then cannot jump independently to opposing nearest scalp surfaces.
fn fit_coherent_outer_cap(
    garment: &CustomGarment,
    hair: &mut NativePart,
    current: &[[f32; 3]],
    faces: &[u32],
    regions: &[u8],
    clearance: f32,
    spacing: f32,
) -> Result<HairFitStats, String> {
    use std::collections::BTreeMap;
    const MAX_NODES: usize = 20_000;
    type Key = [i32; 3];
    struct Node {
        point: Vec3,
        displacement: Vec3,
    }
    let (triangles, grid) = current_head_collider(
        current,
        faces,
        regions,
        clearance,
        garment.head_neck_clearance()?,
    )?;
    let originals: Vec<Vec3> = hair.positions.iter().copied().map(Vec3::from).collect();
    let mut totals = BTreeMap::<Key, (Vec3, f32)>::new();
    let mut keys = Vec::<[(Key, f32); 8]>::with_capacity(originals.len());
    for (index, &point) in originals.iter().enumerate() {
        if !point.is_finite() || point.abs().max_element() > 100. {
            return Err("outer hair cap position is outside the finite domain".into());
        }
        if garment.refs[index]
            .iter()
            .any(|&reference| regions.get(reference as usize).copied() != Some(0))
        {
            return Err("outer hair cap references must all belong to the head".into());
        }
        // Immutable coordinates keep UV seam copies on the same field even
        // when distinct valid body bindings separated them during transport.
        // The oracle centroid below still uses the actual current positions.
        let source = Vec3::from(garment.positions_dm[index]);
        if !source.is_finite() || source.abs().max_element() > 100. {
            return Err("outer hair cap authored position is outside the finite domain".into());
        }
        let voxel = source / spacing;
        let base = voxel.floor();
        let fraction = voxel - base;
        let base = base.to_array().map(|v| v as i32);
        let mut row = [([0; 3], 0.); 8];
        for dx in 0..=1 {
            for dy in 0..=1 {
                for dz in 0..=1 {
                    let axis = |f: f32, corner: i32| if corner == 0 { 1. - f } else { f };
                    let weight = axis(fraction.x, dx) * axis(fraction.y, dy) * axis(fraction.z, dz);
                    let key = [base[0] + dx, base[1] + dy, base[2] + dz];
                    row[(dx * 4 + dy * 2 + dz) as usize] = (key, weight);
                    if weight > 0. {
                        if !totals.contains_key(&key) && totals.len() >= MAX_NODES {
                            return Err("outer hair cap exceeds the coherent node budget".into());
                        }
                        let total = totals.entry(key).or_insert((Vec3::ZERO, 0.));
                        total.0 += point * weight;
                        total.1 += weight;
                    }
                }
            }
        }
        keys.push(row);
    }
    let mut indices = BTreeMap::<Key, usize>::new();
    let mut nodes = Vec::<Node>::with_capacity(totals.len());
    for (key, (sum, weight)) in totals {
        indices.insert(key, nodes.len());
        nodes.push(Node {
            point: sum / weight,
            displacement: Vec3::ZERO,
        });
    }
    let controls: Vec<Vec<(usize, f32)>> = keys
        .iter()
        .map(|row| {
            row.iter()
                .filter(|(_, weight)| *weight > 0.)
                .map(|(key, weight)| (indices[key], *weight))
                .collect()
        })
        .collect();
    let sample = |point: Vec3| -> Option<(Vec3, f32)> {
        nearest_surface(point, grid.get(&cell(point))?, &triangles)
    };
    // Start with only a few thousand shared control points, rather than
    // projecting every dense mesh vertex independently.
    for _ in 0..4 {
        for node in &mut nodes {
            let point = node.point + node.displacement;
            if let Some((normal, signed)) = sample(point) {
                if signed < clearance {
                    node.displacement = (node.displacement + normal * (clearance - signed))
                        .clamp_length_max(MAX_HAIR_CORRECTION);
                }
            }
        }
    }
    let displacement = |row: &[(usize, f32)], nodes: &[Node]| -> Vec3 {
        row.iter().map(|&(i, w)| nodes[i].displacement * w).sum()
    };
    // Enforce actual surface samples through the shared field. Clear nearby
    // vertices receive the same smoothly interpolated displacement; applying
    // this only to violating vertices would recreate a discontinuous edge.
    for _ in 0..3 {
        let mut requests = vec![(Vec3::ZERO, 0.); nodes.len()];
        let mut remaining = 0;
        for (&original, row) in originals.iter().zip(&controls) {
            let point = original + displacement(row, &nodes);
            if let Some((normal, signed)) = sample(point) {
                if signed < clearance - 0.000_1 {
                    remaining += 1;
                    let correction = normal * (clearance - signed);
                    for &(index, weight) in row {
                        requests[index].0 += correction * weight;
                        requests[index].1 += weight;
                    }
                }
            }
        }
        if remaining == 0 {
            break;
        }
        for (node, (sum, weight)) in nodes.iter_mut().zip(requests) {
            if weight > 0. {
                // Shared constraints dilute a request when nearby vertices
                // need different outward normals. Over-relax within the same
                // total correction ball; residuals are still measured below.
                node.displacement =
                    (node.displacement + 2. * sum / weight).clamp_length_max(MAX_HAIR_CORRECTION);
            }
        }
    }
    let mut stats = HairFitStats::default();
    for ((position, &original), row) in hair.positions.iter_mut().zip(&originals).zip(&controls) {
        // Trilinear weights form a convex combination of capped node vectors.
        // The final clamp also protects the cap from floating-point roundoff.
        let delta = displacement(row, &nodes).clamp_length_max(MAX_HAIR_CORRECTION);
        let point = original + delta;
        if delta.length_squared() > 0. {
            stats.corrected_vertices += 1;
            stats.max_displacement = stats.max_displacement.max(delta.length());
        }
        *position = point.to_array();
        if let Some((_, signed)) = sample(point) {
            if signed < clearance - 0.000_1 {
                stats.remaining_violations += 1;
            }
        }
    }
    Ok(stats)
}

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
    if let Some(clearance_mm) = garment.head_surface_clearance_mm()? {
        if let Some(spacing) = garment.head_outer_cap_grid_dm()? {
            return fit_coherent_outer_cap(
                garment,
                hair,
                current,
                faces,
                regions,
                clearance_mm / 100.,
                spacing,
            );
        }
        return fit_custom_head_surface_clearance(
            garment,
            hair,
            neutral,
            current,
            faces,
            regions,
            clearance_mm,
        );
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
            skin_weight_source_topology: None,
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
    fn coherent_outer_cap_limits_nearest_branch_edge_tearing() {
        let body = vec![
            [0., -1., -1.],
            [0., 1., -1.],
            [0., 1., 1.],
            [0., -1., 1.],
            [-1., -1., 0.],
            [1., -1., 0.],
            [1., 1., 0.],
            [-1., 1., 0.],
        ];
        let faces = vec![0, 1, 2, 3, 4, 5, 6, 7];
        let points = vec![[-0.12, 0., -0.10], [-0.10, 0., -0.12], [-0.11, 0.05, -0.11]];
        let mut garment = custom(
            points.clone(),
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"fitHeadSurfaceClearanceMm":4.0}),
        );
        let mut independent = part("custom:independent", points.clone(), vec![0, 1, 2]);
        fit_custom_head_clearance(&garment, &mut independent, &body, &body, &faces, &[0; 8])
            .unwrap();
        garment.material["fitHeadOuterCapGridDM"] = json!(0.2);
        let mut coherent = part("custom:coherent", points, vec![0, 1, 2]);
        let stats =
            fit_custom_head_clearance(&garment, &mut coherent, &body, &body, &faces, &[0; 8])
                .unwrap();
        let length =
            |p: &NativePart| Vec3::from(p.positions[0]).distance(Vec3::from(p.positions[1]));
        assert!(length(&coherent) < length(&independent) * 0.5);
        assert!(length(&coherent) < 0.08);
        assert!(stats.max_displacement <= MAX_HAIR_CORRECTION + 1e-6);
        assert_eq!(stats.remaining_violations, 0);
    }

    #[test]
    fn coherent_outer_cap_shares_weld_corrections_and_reports_cap_residuals() {
        let (body, faces, regions) = head_plane();
        let points = vec![
            [0.2, 0.2, -0.8],
            [0.25, 0.2, -0.8],
            [0.2, 0.25, -0.8],
            [0.2, 0.2, -0.8],
        ];
        let garment = custom(
            points.clone(),
            vec![0, 1, 2, 3, 2, 1],
            vec![[0, 0, 0], [0, 1, 2], [0, 1, 2], [1, 1, 1]],
            json!({"fitHeadSurfaceClearanceMm":4.0,"fitHeadOuterCapGridDM":0.2}),
        );
        let mut hair = part("custom:coherent", points, garment.indices.clone());
        let stats =
            fit_custom_head_clearance(&garment, &mut hair, &body, &body, &faces, &regions).unwrap();
        assert_eq!(hair.positions[0], hair.positions[3]);
        assert!((stats.max_displacement - MAX_HAIR_CORRECTION).abs() < 1e-6);
        assert_eq!(stats.remaining_violations, 4);
    }

    #[test]
    fn coherent_outer_cap_preserves_weld_delta_after_divergent_transport() {
        let body = vec![
            [0., -1., -1.],
            [0., 1., -1.],
            [0., 1., 1.],
            [0., -1., 1.],
            [-1., -1., 0.],
            [1., -1., 0.],
            [1., 1., 0.],
            [-1., 1., 0.],
        ];
        let faces = vec![0, 1, 2, 3, 4, 5, 6, 7];
        let points = vec![
            [-0.12, 0., -0.10],
            [-0.10, 0., -0.12],
            [-0.11, 0.05, -0.11],
            [-0.12, 0., -0.10],
        ];
        let garment = custom(
            points.clone(),
            vec![0, 1, 2, 3, 2, 1],
            vec![[0, 1, 2], [0, 1, 2], [0, 1, 2], [4, 5, 6]],
            json!({"fitHeadSurfaceClearanceMm":4.0,"fitHeadOuterCapGridDM":0.2}),
        );
        let mut hair = part("custom:divergent", points, garment.indices.clone());
        hair.positions[3] = [-0.10, 0., -0.12];
        let before = hair.positions.clone();
        fit_custom_head_clearance(&garment, &mut hair, &body, &body, &faces, &[0; 8]).unwrap();
        let a = Vec3::from(hair.positions[0]) - Vec3::from(before[0]);
        let b = Vec3::from(hair.positions[3]) - Vec3::from(before[3]);
        assert!(a.distance(b) < 1e-6);
        assert!(a.length() > 0.01);
    }

    #[test]
    fn coherent_outer_cap_rejects_authored_grid_overflow() {
        let (body, faces, regions) = head_plane();
        for invalid in [1e30, f32::INFINITY, f32::NAN] {
            let mut garment = custom(
                vec![[0.2, 0.2, 0.1], [0.25, 0.2, 0.1], [0.2, 0.25, 0.1]],
                vec![0, 1, 2],
                vec![[0, 1, 2]; 3],
                json!({"fitHeadSurfaceClearanceMm":4.0,"fitHeadOuterCapGridDM":0.2}),
            );
            let mut hair = part(
                "custom:bounded",
                garment.positions_dm.clone(),
                garment.indices.clone(),
            );
            garment.positions_dm[0][0] = invalid;
            let error =
                fit_custom_head_clearance(&garment, &mut hair, &body, &body, &faces, &regions)
                    .unwrap_err();
            if invalid.is_finite() {
                assert!(error.contains("authored position"));
            } else {
                assert!(error.contains("non-finite"));
            }
        }
    }

    #[test]
    fn coherent_outer_cap_requires_explicit_valid_head_metadata() {
        let mut garment = custom(
            vec![[0.2, 0.2, 0.1], [0.25, 0.2, 0.1], [0.2, 0.25, 0.1]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"fitHeadOuterCapGridDM":0.2}),
        );
        assert!(garment.validate().is_err());
        garment.material["fitHeadSurfaceClearanceMm"] = json!(4.0);
        assert!(garment.validate().is_ok());
        for invalid in [
            json!(0.0),
            json!(0.05),
            json!(0.6),
            json!(true),
            json!("0.2"),
        ] {
            garment.material["fitHeadOuterCapGridDM"] = invalid;
            assert!(garment.validate().is_err());
        }
        garment.material["fitHeadOuterCapGridDM"] = json!(0.2);
        let (body, faces, _) = head_plane();
        let mut hair = part(
            "custom:coherent",
            garment.positions_dm.clone(),
            garment.indices.clone(),
        );
        assert!(
            fit_custom_head_clearance(&garment, &mut hair, &body, &body, &faces, &[0, 1, 0])
                .is_err()
        );
        garment.slot = GarmentSlot::Top;
        assert!(garment.validate().is_err());
    }

    fn shifted_head_surfaces() -> (Vec<[f32; 3]>, Vec<u32>, Vec<u8>) {
        // Authored refs stay on the rear plane while the nearer current scalp
        // is a separate surface. This models a cartoon-morphed cranial fold.
        (
            vec![
                [0., 0., -0.2],
                [1., 0., -0.2],
                [0., 1., -0.2],
                [0., 0., 0.1],
                [1., 0., 0.1],
                [0., 1., 0.1],
            ],
            vec![0, 1, 2, 2, 3, 4, 5, 5],
            vec![0; 6],
        )
    }

    #[test]
    fn opt_in_nape_clearance_includes_neck_and_preserves_default_collider() {
        let (mut body, mut faces, mut regions) = head_plane();
        body.extend([[0., 0., 0.2], [1., 0., 0.2], [0., 1., 0.2]]);
        faces.extend([3, 4, 5, 5]);
        regions.extend([1; 3]);
        // A torso surface above the neck must not become a hair collider.
        body.extend([[0., 0., 0.3], [1., 0., 0.3], [0., 1., 0.3]]);
        faces.extend([6, 7, 8, 8]);
        regions.extend([2; 3]);
        for coherent in [false, true] {
            let mut garment = custom(
                vec![[0.2, 0.2, 0.15], [0.25, 0.2, 0.15], [0.2, 0.25, 0.15]],
                vec![0, 1, 2],
                vec![[0, 1, 2]; 3],
                json!({"fitHeadSurfaceClearanceMm": 1.0}),
            );
            if coherent {
                garment.material["fitHeadOuterCapGridDM"] = json!(0.1);
            }
            let make_part = || part("custom:nape", garment.positions_dm.clone(), vec![0, 1, 2]);
            let mut omitted = make_part();
            let omitted_stats =
                fit_custom_head_clearance(&garment, &mut omitted, &body, &body, &faces, &regions)
                    .unwrap();
            garment.material["fitHeadNeckClearance"] = json!(false);
            let mut disabled = omitted.clone();
            let disabled_stats =
                fit_custom_head_clearance(&garment, &mut disabled, &body, &body, &faces, &regions)
                    .unwrap();
            assert_eq!(omitted.positions, garment.positions_dm);
            assert_eq!(omitted.positions, disabled.positions);
            assert_eq!(omitted_stats, disabled_stats);
            garment.material["fitHeadNeckClearance"] = json!(true);
            let mut enabled = omitted;
            let stats =
                fit_custom_head_clearance(&garment, &mut enabled, &body, &body, &faces, &regions)
                    .unwrap();
            assert_eq!(stats.remaining_violations, 0);
            assert!(stats.corrected_vertices > 0);
            for (point, original) in enabled.positions.iter().zip(&garment.positions_dm) {
                assert!(point[2] >= 0.209_9 && point[2] < 0.25);
                assert!(Vec3::from(*point).distance(Vec3::from(*original)) <= MAX_HAIR_CORRECTION);
            }
            assert_eq!(enabled.indices, garment.indices);
            assert_eq!(enabled.uvs, vec![[0.; 2]; 3]);
        }
    }

    #[test]
    fn neck_collider_includes_mixed_head_neck_seam_quads_only_when_enabled() {
        let (mut body, mut faces, mut regions) = head_plane();
        body.extend([[0., 0., 0.2], [1., 0., 0.2], [1., 1., 0.2], [0., 1., 0.2]]);
        faces.extend([3, 4, 5, 6]);
        regions.extend([0, 0, 1, 1]);
        let (default_triangles, _) =
            current_head_collider(&body, &faces, &regions, 0.01, false).unwrap();
        let (neck_triangles, _) =
            current_head_collider(&body, &faces, &regions, 0.01, true).unwrap();
        assert_eq!(default_triangles.len(), 1);
        assert_eq!(neck_triangles.len(), 3);
        let point = Vec3::new(0.2, 0.2, 0.15);
        let (_, default_signed) = nearest_surface(point, &[0], &default_triangles).unwrap();
        let (_, neck_signed) = nearest_surface(point, &[0, 1, 2], &neck_triangles).unwrap();
        assert!(default_signed > 0.1);
        assert!(neck_signed < -0.04);
    }

    #[test]
    fn nape_clearance_requires_explicit_valid_hair_surface_metadata() {
        let mut garment = custom(
            vec![[0., 0., 0.1], [1., 0., 0.1], [0., 1., 0.1]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"fitHeadNeckClearance": true}),
        );
        assert!(garment.validate().is_err());
        garment.material["fitHeadSurfaceClearanceMm"] = json!(3.0);
        assert!(garment.validate().is_ok());
        for invalid in [json!(1), json!("true"), json!(null)] {
            garment.material["fitHeadNeckClearance"] = invalid;
            assert!(garment.validate().unwrap_err().contains("must be boolean"));
        }
        garment.material["fitHeadNeckClearance"] = json!(true);
        garment.slot = GarmentSlot::Beard;
        assert!(garment.validate().is_err());
    }

    #[test]
    fn opt_in_surface_clearance_uses_nearest_current_head_not_authored_anchor_plane() {
        let (body, faces, regions) = shifted_head_surfaces();
        let garment = custom(
            vec![[0., 0., 0.05], [1., 0., 0.05], [0., 1., 0.05]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"fitHeadSurfaceClearanceMm": 1.0}),
        );
        let mut hair = part(
            "custom:surface",
            garment.positions_dm.clone(),
            vec![0, 1, 2],
        );
        let stats =
            fit_custom_head_clearance(&garment, &mut hair, &body, &body, &faces, &regions).unwrap();
        assert_eq!(stats.corrected_vertices, 3);
        assert!((hair.positions[0][2] - 0.11).abs() < 0.000_1);
        // The omitted flag retains the old anchor-plane behavior exactly.
        let legacy = custom(
            garment.positions_dm.clone(),
            garment.indices.clone(),
            garment.refs.clone(),
            json!({}),
        );
        let mut legacy_part = part("custom:legacy", legacy.positions_dm.clone(), vec![0, 1, 2]);
        assert_eq!(
            fit_custom_head_clearance(&legacy, &mut legacy_part, &body, &body, &faces, &regions)
                .unwrap(),
            HairFitStats::default()
        );
        assert_eq!(legacy_part.positions, legacy.positions_dm);
    }

    #[test]
    fn opt_in_surface_clearance_prefers_near_clear_surface_over_far_violation() {
        let (mut body, mut faces, mut regions) = shifted_head_surfaces();
        // A farther surface is inward, but the immediately nearest scalp plane
        // is already clear. Selecting a "violating" plane first would wrongly
        // inflate the hair through the near scalp.
        body.extend([[0., 0., 0.3], [1., 0., 0.3], [0., 1., 0.3]]);
        faces.extend([6, 7, 8, 8]);
        regions.extend([0; 3]);
        let garment = custom(
            vec![[0., 0., 0.15], [1., 0., 0.15], [0., 1., 0.15]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"fitHeadSurfaceClearanceMm": 1.0}),
        );
        let mut hair = part(
            "custom:nearest",
            garment.positions_dm.clone(),
            vec![0, 1, 2],
        );
        assert_eq!(
            fit_custom_head_clearance(&garment, &mut hair, &body, &body, &faces, &regions).unwrap(),
            HairFitStats::default()
        );
        assert_eq!(hair.positions, garment.positions_dm);
    }

    #[test]
    fn opt_in_surface_clearance_rejects_mismatched_hair_stream_before_access() {
        let (body, faces, regions) = shifted_head_surfaces();
        let garment = custom(
            vec![[0., 0., 0.05], [1., 0., 0.05], [0., 1., 0.05]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"fitHeadSurfaceClearanceMm": 1.0}),
        );
        let mut hair = part("custom:short", vec![[0., 0., 0.05]], vec![]);
        assert!(
            fit_custom_head_clearance(&garment, &mut hair, &body, &body, &faces, &regions)
                .unwrap_err()
                .contains("inconsistent body or garment lengths")
        );
    }

    #[test]
    fn opt_in_surface_clearance_preserves_inner_shell_and_reports_capped_depth() {
        let (body, faces, regions) = shifted_head_surfaces();
        let inner = custom(
            vec![[0., 0., 0.05], [0., 1., 0.05], [1., 0., 0.05]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"fitHeadSurfaceClearanceMm": 1.0}),
        );
        let mut inner_part = part("custom:inner", inner.positions_dm.clone(), vec![0, 1, 2]);
        assert_eq!(
            fit_custom_head_clearance(&inner, &mut inner_part, &body, &body, &faces, &regions)
                .unwrap(),
            HairFitStats::default()
        );
        assert_eq!(inner_part.positions, inner.positions_dm);
        let deep = custom(
            vec![[0., 0., -1.], [1., 0., -1.], [0., 1., -1.]],
            vec![0, 1, 2],
            vec![[0, 1, 2]; 3],
            json!({"fitHeadSurfaceClearanceMm": 1.0}),
        );
        let mut deep_part = part("custom:deep", deep.positions_dm.clone(), vec![0, 1, 2]);
        let stats =
            fit_custom_head_clearance(&deep, &mut deep_part, &body, &body, &faces, &regions)
                .unwrap();
        assert!((stats.max_displacement - MAX_HAIR_CORRECTION).abs() < 0.000_1);
        assert_eq!(stats.remaining_violations, 3);
    }

    #[test]
    fn opt_in_surface_clearance_rejects_invalid_flag_type() {
        let (body, faces, regions) = shifted_head_surfaces();
        let garment = custom(
            vec![[0., 0., 0.]],
            vec![0, 0, 0],
            vec![[0, 1, 2]],
            json!({"fitHeadSurfaceClearanceMm": true}),
        );
        let mut hair = part(
            "custom:invalid",
            garment.positions_dm.clone(),
            vec![0, 0, 0],
        );
        assert!(
            fit_custom_head_clearance(&garment, &mut hair, &body, &body, &faces, &regions)
                .unwrap_err()
                .contains("fitHeadSurfaceClearanceMm")
        );
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
