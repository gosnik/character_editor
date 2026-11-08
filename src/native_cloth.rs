//! Small, opt-in bind-pose clearance for imported custom clothing.
//!
//! This is a collision correction, not cloth simulation. It operates on the
//! final visible body before metre conversion, preserving the saved garment's
//! topology, UVs, bindings, and material.
use crate::native_character::NativePart;
use bevy::prelude::Vec3;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const CELL: f32 = 0.5;
const MAX_BODY_COORDINATE: f32 = 100.;
const MAX_GRID_CELLS: usize = 1_000_000;
const MAX_GRID_REFERENCES: usize = 2_000_000;
const MAX_REPORT_CACHE_CELLS: usize = 4_096;
const MAX_REPORT_CACHE_CANDIDATES: usize = 2_000_000;
const MAX_CORRECTION: f32 = 0.4; // 4 cm; imported chest/hip offsets can exceed 2 cm.
const REPORT_DISTANCE: f32 = 1.0;
const PASSES: usize = 4;
const SMOOTHING: f32 = 0.35;
const TRIANGLE_CONTACT_PASSES: usize = 2;
const MAX_CONTACT_TRIANGLES: usize = 200_000;
const MAX_CONTACT_GRID_CELLS: i64 = 64;
const MAX_CONTACT_CANDIDATES: usize = 4_000_000;
const MAX_LAYER_CANDIDATES: usize = 4_000_000;
// Physical coverage discovers surfaces at twice the layer solver's reach.
// Bound its larger search volume explicitly, while keeping each collider and
// each contact-repair pass under their existing geometry/allocation limits.
const MAX_PHYSICAL_TUCK_CANDIDATES: usize = MAX_LAYER_CANDIDATES * 8;
const LAYER_RAY_REACH: f32 = 0.4;
// An inward tuck may start outside the 4 cm movement budget while still ending
// near the visible body. Keep discovery bounded independently of that budget.
const TUCK_QUERY_REACH: f32 = MAX_CORRECTION * 2.0;
const CLEARANCE_FLOOR_EPSILON: f32 = 1e-5;
const MAX_COHERENT_CLEARANCE_NODES: usize = 20_000;
const MAX_COHERENT_CLEARANCE_VERTICES: usize = 100_000;
const MAX_COHERENT_CLEARANCE_INDICES: usize = 1_200_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClothFitStats {
    pub corrected_vertices: usize,
    pub max_displacement: f32,
    pub remaining_violations: usize,
}

#[derive(Clone, Copy)]
struct Triangle {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    outward: Vec3,
}

#[derive(Clone, Copy)]
struct GridBudget {
    cells: usize,
    references: usize,
}

/// Conservative region in which an opted-in garment can query the body.
/// The correction limit bounds every fitted point's movement from its source;
/// reporting can subsequently query one decimetre plus at most 10 mm of clearance.
#[derive(Clone, Copy)]
struct ColliderDomain {
    low: Vec3,
    high: Vec3,
}

impl Triangle {
    fn closest(self, point: Vec3) -> Vec3 {
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
        if vb <= 0. && d2 >= 0. && d5 <= d6 {
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

fn collider_with_budget(
    body: &NativePart,
    budget: GridBudget,
) -> Result<(Vec<Triangle>, BTreeMap<[i32; 3], Vec<usize>>), String> {
    collider_with_margin(body, budget, MAX_CORRECTION + 0.1)
}

fn collider_with_limit(
    body: &NativePart,
    budget: GridBudget,
    correction_limit: f32,
    part_positions: &[Vec3],
) -> Result<(Vec<Triangle>, BTreeMap<[i32; 3], Vec<usize>>), String> {
    // The omitted and explicit 40 mm requests intentionally preserve the
    // legacy full-body collider bit-for-bit.
    if correction_limit == MAX_CORRECTION {
        return collider_with_margin(body, budget, correction_limit + 0.1);
    }
    let Some(domain) = collider_domain(part_positions, correction_limit)? else {
        return collider_with_margin(body, budget, correction_limit + 0.1);
    };
    collider_with_optional_domain(body, budget, correction_limit + 0.1, Some(domain))
}

fn collider_with_margin(
    body: &NativePart,
    budget: GridBudget,
    margin: f32,
) -> Result<(Vec<Triangle>, BTreeMap<[i32; 3], Vec<usize>>), String> {
    collider_with_optional_domain(body, budget, margin, None)
}

fn collider_domain(
    part_positions: &[Vec3],
    correction_limit: f32,
) -> Result<Option<ColliderDomain>, String> {
    if part_positions
        .iter()
        .any(|point| !point.is_finite() || point.abs().max_element() > MAX_BODY_COORDINATE)
    {
        return Err("custom clothing has non-finite or out-of-range positions".into());
    }
    let mut positions = part_positions.iter().copied();
    let Some(first) = positions.next() else {
        return Ok(None);
    };
    let (low, high) = positions.fold((first, first), |(low, high), point| {
        (low.min(point), high.max(point))
    });
    // Every fitted point remains within `correction_limit` of its source.
    // The largest later body query is the one-decimetre report plus 10 mm of
    // requested clearance, so this domain contains every reachable triangle.
    let expansion = correction_limit + REPORT_DISTANCE + 0.1 + CLEARANCE_FLOOR_EPSILON;
    Ok(Some(ColliderDomain {
        low: low - Vec3::splat(expansion),
        high: high + Vec3::splat(expansion),
    }))
}

fn triangle_intersects_domain(a: Vec3, b: Vec3, c: Vec3, domain: ColliderDomain) -> bool {
    let low = a.min(b).min(c);
    let high = a.max(b).max(c);
    (0..3).all(|axis| low[axis] <= domain.high[axis] && high[axis] >= domain.low[axis])
}

fn collider_with_optional_domain(
    body: &NativePart,
    budget: GridBudget,
    margin: f32,
    domain: Option<ColliderDomain>,
) -> Result<(Vec<Triangle>, BTreeMap<[i32; 3], Vec<usize>>), String> {
    if body.indices.len() % 3 != 0 {
        return Err("visible body triangle index stream is incomplete".into());
    }
    if body.positions.is_empty()
        || body
            .positions
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || value.abs() > MAX_BODY_COORDINATE)
    {
        return Err("visible body has non-finite or out-of-range positions".into());
    }
    let mut triangles = Vec::new();
    let mut grid = BTreeMap::<[i32; 3], Vec<usize>>::new();
    let mut grid_cells = 0usize;
    let mut grid_references = 0usize;
    for indices in body.indices.chunks_exact(3) {
        let vertex = |index: u32| {
            body.positions
                .get(index as usize)
                .copied()
                .map(Vec3::from)
                .ok_or_else(|| "visible body has an invalid triangle index".to_owned())
        };
        let (a, b, c) = (
            vertex(indices[0])?,
            vertex(indices[1])?,
            vertex(indices[2])?,
        );
        debug_assert!(a.is_finite() && b.is_finite() && c.is_finite());
        // Fetch and validate every body triangle before filtering. A malformed
        // face outside this garment's local domain must still reject the body.
        if domain.is_some_and(|domain| !triangle_intersects_domain(a, b, c, domain)) {
            continue;
        }
        let outward = (b - a).cross(c - a).normalize_or_zero();
        if outward == Vec3::ZERO {
            continue;
        }
        // Native body winding is canonical. A body-centre heuristic flips
        // legitimate inward-facing concave regions such as inner legs.
        let index = triangles.len();
        triangles.push(Triangle { a, b, c, outward });
        let low = cell(a.min(b).min(c) - Vec3::splat(margin));
        let high = cell(a.max(b).max(c) + Vec3::splat(margin));
        if (0..3)
            .map(|axis| high[axis] as i64 - low[axis] as i64 + 1)
            .fold(1i64, i64::saturating_mul)
            > 4096
        {
            return Err("visible body triangle spans an unreasonable clearance grid".into());
        }
        let references = (high[0] as i64 - low[0] as i64 + 1)
            .saturating_mul(high[1] as i64 - low[1] as i64 + 1)
            .saturating_mul(high[2] as i64 - low[2] as i64 + 1);
        let mut new_cells = 0usize;
        for x in low[0]..=high[0] {
            for y in low[1]..=high[1] {
                for z in low[2]..=high[2] {
                    new_cells += (!grid.contains_key(&[x, y, z])) as usize;
                }
            }
        }
        let references = references as usize;
        if grid_cells.saturating_add(new_cells) > budget.cells
            || grid_references.saturating_add(references) > budget.references
        {
            return Err("visible body exceeds clearance grid budget".into());
        }
        for x in low[0]..=high[0] {
            for y in low[1]..=high[1] {
                for z in low[2]..=high[2] {
                    grid.entry([x, y, z]).or_default().push(index);
                }
            }
        }
        grid_cells += new_cells;
        grid_references += references;
    }
    Ok((triangles, grid))
}

fn collider(body: &NativePart) -> Result<(Vec<Triangle>, BTreeMap<[i32; 3], Vec<usize>>), String> {
    collider_with_budget(
        body,
        GridBudget {
            cells: MAX_GRID_CELLS,
            references: MAX_GRID_REFERENCES,
        },
    )
}

/// Read the layer-clearance opt-in strictly.  Unlike the older body-clearance
/// key, a malformed explicit layer request is an authoring error: silently
/// ignoring it could leave an outer garment inside a layer it intends to hide.
pub(crate) fn layer_clearance_request(part: &NativePart) -> Result<Option<f32>, String> {
    let Some(value) = part.material.get("fitLayerClearanceMm") else {
        return Ok(None);
    };
    let millimetres = value
        .as_f64()
        .ok_or("fitLayerClearanceMm must be numeric")?;
    if !millimetres.is_finite() || !(0.0..=10.0).contains(&millimetres) || millimetres == 0.0 {
        return Err("fitLayerClearanceMm must be greater than zero and at most 10".into());
    }
    let clearance = millimetres as f32 * 0.01;
    if clearance <= 0.0 {
        return Err("fitLayerClearanceMm is too small for fitting precision".into());
    }
    Ok(Some(clearance))
}

fn collider_parts(
    parts: &[&NativePart],
    margin: f32,
) -> Result<(Vec<Triangle>, BTreeMap<[i32; 3], Vec<usize>>), String> {
    let mut joined = NativePart {
        name: "lower clothing collider".into(),
        positions: Vec::new(),
        indices: Vec::new(),
        uvs: Vec::new(),
        refs: Vec::new(),
        ref_weights: Vec::new(),
        rigid_joint: None,
        skin_weight_source_topology: None,
        material: serde_json::Value::Null,
    };
    for part in parts {
        if part
            .indices
            .iter()
            .any(|&index| index as usize >= part.positions.len())
        {
            return Err(format!("{} has invalid lower clothing indices", part.name));
        }
        if joined.positions.len().saturating_add(part.positions.len()) > 600_000
            || joined.indices.len().saturating_add(part.indices.len()) / 3 > MAX_CONTACT_TRIANGLES
        {
            return Err("lower clothing exceeds layer collider geometry budget".into());
        }
        let base = u32::try_from(joined.positions.len())
            .map_err(|_| "lower clothing exceeds vertex index budget")?;
        if part.indices.len() % 3 != 0 {
            return Err(format!(
                "{} has incomplete lower clothing triangles",
                part.name
            ));
        }
        joined.positions.extend_from_slice(&part.positions);
        joined.indices.extend(
            part.indices
                .iter()
                .map(|&index| {
                    index
                        .checked_add(base)
                        .ok_or("lower clothing exceeds vertex index budget")
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    collider_with_margin(
        &joined,
        GridBudget {
            cells: MAX_GRID_CELLS,
            references: MAX_GRID_REFERENCES,
        },
        margin,
    )
}

fn ray_triangle_distance(origin: Vec3, direction: Vec3, triangle: Triangle) -> Option<f32> {
    let edge_a = triangle.b - triangle.a;
    let edge_b = triangle.c - triangle.a;
    let cross = direction.cross(edge_b);
    let determinant = edge_a.dot(cross);
    if determinant.abs() <= 1e-8 {
        return None;
    }
    let inverse = determinant.recip();
    let offset = origin - triangle.a;
    let u = offset.dot(cross) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = offset.cross(edge_a);
    let v = direction.dot(q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(edge_b.dot(q) * inverse)
}

fn layer_target(
    point: Vec3,
    normal: Vec3,
    candidates: &[usize],
    triangles: &[Triangle],
    clearance: f32,
) -> Option<Vec3> {
    let start = point - normal * LAYER_RAY_REACH;
    let furthest = candidates
        .iter()
        .copied()
        .filter_map(|index| {
            ray_triangle_distance(start, normal, triangles[index])
                .filter(|distance| *distance >= 0.0 && *distance <= LAYER_RAY_REACH * 2.0)
        })
        .fold(None::<f32>, |best, distance| {
            Some(best.map_or(distance, |old| old.max(distance)))
        });
    furthest.map(|distance| start + normal * (distance + clearance))
}

/// Separates an opted-in ranked garment from lower ranked custom clothing.
/// The full body is used only to obtain the current outward direction; it is
/// never included in the lower-layer collision set.
pub(crate) fn fit_custom_layer_clearance(
    part: &mut NativePart,
    lower: &[&NativePart],
    full_body: &NativePart,
    source_positions: &[[f32; 3]],
) -> Result<Option<ClothFitStats>, String> {
    // Validate the independent correction opt-in before the layer opt-in can
    // take its no-op path. A malformed limit must not be silently ignored.
    let correction_limit = correction_limit(part)?;
    let Some(clearance) = layer_clearance_request(part)? else {
        return Ok(None);
    };
    if source_positions.len() != part.positions.len() {
        return Err(format!(
            "{} has inconsistent pre-clearance positions",
            part.name
        ));
    }
    if part.indices.len() % 3 != 0
        || part
            .indices
            .iter()
            .any(|&index| index as usize >= part.positions.len())
    {
        return Err(format!(
            "{} has invalid layer-clearance triangle indices",
            part.name
        ));
    }
    let (owner, members, _) = groups(part)?;
    if source_positions
        .iter()
        .flatten()
        .any(|value| !value.is_finite() || value.abs() > MAX_BODY_COORDINATE)
    {
        return Err(format!("{} has invalid pre-clearance positions", part.name));
    }
    if lower.is_empty() {
        return Ok(Some(ClothFitStats::default()));
    }
    let current: Vec<Vec3> = part.positions.iter().copied().map(Vec3::from).collect();
    // Layer clearance starts after body clearance. Include both its original
    // source and current positions: the source-relative correction cap makes
    // their union a conservative local body-query domain.
    let mut domain_positions = current.clone();
    domain_positions.extend(source_positions.iter().copied().map(Vec3::from));
    let (body_triangles, body_grid) = collider_with_limit(
        full_body,
        GridBudget {
            cells: MAX_GRID_CELLS,
            references: MAX_GRID_REFERENCES,
        },
        correction_limit,
        &domain_positions,
    )?;
    let (lower_triangles, lower_grid) = collider_parts(lower, LAYER_RAY_REACH)?;
    let mut proposed = current.clone();
    let mut candidate_count = 0usize;
    let mut remaining_violations = 0usize;
    for vertices in &members {
        let vertex = vertices[0];
        let point = current[vertex];
        // Collider AABBs already include the full ray reach. Querying the
        // point's cell therefore retains every possible hit; another 27-cell
        // neighborhood needlessly repeats broad-phase work and budgets.
        let Some(lower_candidates) = lower_grid.get(&cell(point)) else {
            continue;
        };
        candidate_count = candidate_count.saturating_add(lower_candidates.len());
        let Some(candidates) = body_grid.get(&cell(point)) else {
            continue;
        };
        candidate_count = candidate_count.saturating_add(candidates.len());
        if candidate_count > MAX_LAYER_CANDIDATES {
            return Err("custom clothing exceeds layer-clearance candidate budget".into());
        }
        let Some((normal, _)) =
            nearest_surface(point, candidates, &body_triangles, 0.0, LAYER_RAY_REACH)
        else {
            continue;
        };
        let Some(target) =
            layer_target(point, normal, lower_candidates, &lower_triangles, clearance)
        else {
            continue;
        };
        let needed = (target - point).dot(normal);
        if needed <= 0.0 {
            continue;
        }
        // Keep the existing body correction intact. Clamping the combined
        // vector would shrink its tangential component and can re-enter skin.
        // Solve the remaining distance along this outward ray instead.
        let source = Vec3::from(source_positions[vertex]);
        let prior = point - source;
        if prior.length_squared() >= correction_limit * correction_limit {
            remaining_violations += vertices.len();
            continue;
        }
        let parallel = prior.dot(normal);
        let residual = (-parallel
            + (parallel * parallel + correction_limit * correction_limit - prior.length_squared())
                .max(0.0)
                .sqrt())
        .max(0.0);
        let allowed = needed.min(residual);
        if needed - allowed > 1e-5 {
            remaining_violations += vertices.len();
        }
        proposed[vertex] = point + normal * allowed;
    }
    let mut stats = ClothFitStats {
        remaining_violations,
        ..ClothFitStats::default()
    };
    for (group, vertices) in members.iter().enumerate() {
        let point = proposed[vertices[0]];
        let delta = point - current[vertices[0]];
        if delta.length_squared() > 0.0 {
            stats.corrected_vertices += vertices.len();
            stats.max_displacement = stats.max_displacement.max(delta.length());
        }
        for &vertex in vertices {
            part.positions[vertex] = point.to_array();
        }
        debug_assert!(owner[vertices[0]] == group);
    }
    Ok(Some(stats))
}

fn nearest_surface(
    point: Vec3,
    candidates: &[usize],
    triangles: &[Triangle],
    clearance: f32,
    reach: f32,
) -> Option<(Vec3, f32)> {
    let mut nearest = None::<(Vec3, f32, f32)>;
    for &index in candidates {
        let triangle = triangles[index];
        let closest = triangle.closest(point);
        let distance_squared = point.distance_squared(closest);
        let signed = (point - closest).dot(triangle.outward);
        if distance_squared <= (reach + clearance).powi(2)
            && nearest.is_none_or(|(_, best, _)| distance_squared < best)
        {
            nearest = Some((triangle.outward, distance_squared, signed));
        }
    }
    nearest.map(|(outward, _, signed)| (outward, signed))
}

fn violation(
    point: Vec3,
    candidates: &[usize],
    triangles: &[Triangle],
    clearance: f32,
    reach: f32,
) -> Option<(Vec3, f32)> {
    nearest_surface(point, candidates, triangles, clearance, reach)
        .filter(|(_, signed)| *signed < clearance)
}

fn physical_tuck_candidates(
    grid: &BTreeMap<[i32; 3], Vec<usize>>,
    point: Vec3,
    candidates_seen: &mut usize,
) -> Result<Vec<usize>, String> {
    let candidates = nearby_candidates(grid, point, TUCK_QUERY_REACH);
    *candidates_seen = candidates_seen.saturating_add(candidates.len());
    if *candidates_seen > MAX_PHYSICAL_TUCK_CANDIDATES {
        return Err(format!(
            "custom clothing exceeds physical-tuck candidate budget ({}/{MAX_PHYSICAL_TUCK_CANDIDATES})",
            *candidates_seen
        ));
    }
    Ok(candidates)
}

fn physical_tuck_ray_candidates(
    grid: &BTreeMap<[i32; 3], Vec<usize>>,
    origin: Vec3,
    normal: Vec3,
    candidates_seen: &mut usize,
) -> Result<Vec<usize>, String> {
    // Only a finite outward ray is tested against the outer garment. Its
    // padded AABB contains every admissible hit, without visiting the large
    // nearest-body search cube for every refined waistband vertex.
    let start = origin + normal * CLEARANCE_FLOOR_EPSILON;
    let end = start + normal * TUCK_QUERY_REACH;
    let margin = Vec3::splat(CLEARANCE_FLOOR_EPSILON);
    let low = start.min(end) - margin;
    let high = start.max(end) + margin;
    let candidates = triangle_candidates(low, high, low, grid)?;
    *candidates_seen = candidates_seen.saturating_add(candidates.len());
    if *candidates_seen > MAX_PHYSICAL_TUCK_CANDIDATES {
        return Err(format!(
            "custom clothing exceeds physical-tuck candidate budget ({}/{MAX_PHYSICAL_TUCK_CANDIDATES})",
            *candidates_seen
        ));
    }
    Ok(candidates)
}

const PHYSICAL_TRACE_ENV: &str = "HUMAN_CLOTH_TRACE_VERTICES";
const MAX_PHYSICAL_TRACE_VERTICES: usize = 64;
const MAX_PHYSICAL_TRACE_CHARS: usize = 1024;

#[derive(Default)]
struct PhysicalTraceGroup {
    requested_vertices: Vec<usize>,
    member_count: usize,
    source_indices: Vec<usize>,
    activation: Option<f32>,
    signed: Option<f32>,
    ceiling: Option<f32>,
    proposed_step: Option<f32>,
    rejection: Option<&'static str>,
    post_contact: Option<&'static str>,
    body_triangle_rollback: bool,
    outer_triangle_rollback: bool,
}

struct PhysicalTrace {
    groups: BTreeMap<usize, PhysicalTraceGroup>,
}

fn parse_physical_trace_vertices(raw: &str, vertex_count: usize) -> Result<Vec<usize>, String> {
    if raw.len() > MAX_PHYSICAL_TRACE_CHARS {
        return Err(format!(
            "{PHYSICAL_TRACE_ENV} exceeds {MAX_PHYSICAL_TRACE_CHARS} characters"
        ));
    }
    if raw.is_empty() {
        return Err(format!(
            "{PHYSICAL_TRACE_ENV} must be a comma-separated vertex list"
        ));
    }
    let mut vertices = BTreeSet::new();
    for token in raw.split(',') {
        let vertex = token
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("{PHYSICAL_TRACE_ENV} must contain integer vertex indices"))?;
        if vertex >= vertex_count {
            return Err(format!(
                "{PHYSICAL_TRACE_ENV} contains an out-of-range vertex index"
            ));
        }
        if !vertices.insert(vertex) {
            return Err(format!(
                "{PHYSICAL_TRACE_ENV} contains duplicate vertex indices"
            ));
        }
        if vertices.len() > MAX_PHYSICAL_TRACE_VERTICES {
            return Err(format!(
                "{PHYSICAL_TRACE_ENV} permits at most {MAX_PHYSICAL_TRACE_VERTICES} vertices"
            ));
        }
    }
    Ok(vertices.into_iter().collect())
}

fn physical_trace_request(
    owner: &[usize],
    members: &[Vec<usize>],
) -> Result<Option<PhysicalTrace>, String> {
    let Some(raw) = std::env::var_os(PHYSICAL_TRACE_ENV) else {
        return Ok(None);
    };
    let raw = raw
        .into_string()
        .map_err(|_| format!("{PHYSICAL_TRACE_ENV} must be valid UTF-8"))?;
    let vertices = parse_physical_trace_vertices(&raw, owner.len())?;
    let mut groups = BTreeMap::<usize, PhysicalTraceGroup>::new();
    for vertex in vertices {
        let group = owner[vertex];
        let entry = groups.entry(group).or_insert_with(|| PhysicalTraceGroup {
            member_count: members[group].len(),
            // Source and runtime arrays have the same vertex indexing. Keep
            // output bounded even for a malformed all-mesh weld.
            source_indices: members[group].iter().copied().take(64).collect(),
            ..Default::default()
        });
        entry.requested_vertices.push(vertex);
    }
    Ok(Some(PhysicalTrace { groups }))
}

fn emit_physical_trace(
    trace: &PhysicalTrace,
    source: &[Vec3],
    current: &[Vec3],
    members: &[Vec<usize>],
    displacement: &[Vec3],
) {
    for (&group, record) in &trace.groups {
        let representative = members[group][0];
        let final_point = source[representative] + displacement[group];
        let delta = final_point - current[representative];
        let line = serde_json::json!({
            "kind": "physical_upper_tuck",
            "units": "native DM",
            "requested_vertices": record.requested_vertices,
            "weld_group": group,
            "weld_member_count": record.member_count,
            "source_vertex_indices_first64": record.source_indices,
            "activation": record.activation,
            "signed_body_distance": record.signed,
            "upper_ceiling_signed_distance": record.ceiling,
            "proposed_normal_step": record.proposed_step,
            "rejection": record.rejection,
            "post_contact": record.post_contact,
            "body_triangle_rollback": record.body_triangle_rollback,
            "outer_triangle_rollback": record.outer_triangle_rollback,
            "final_delta": delta.to_array(),
            "final_delta_length": delta.length(),
        });
        let encoded = serde_json::to_string(&line).unwrap_or_else(|_| {
            "{\"kind\":\"physical_upper_tuck\",\"trace_error\":\"serialization\"}".into()
        });
        eprintln!("HUMAN_CLOTH_TRACE {encoded}");
    }
}

fn nearby_candidates(grid: &BTreeMap<[i32; 3], Vec<usize>>, point: Vec3, reach: f32) -> Vec<usize> {
    let centre = cell(point);
    let radius = (reach / CELL).ceil() as i32;
    let mut candidates = BTreeSet::new();
    for x in centre[0] - radius..=centre[0] + radius {
        for y in centre[1] - radius..=centre[1] + radius {
            for z in centre[2] - radius..=centre[2] + radius {
                if let Some(indices) = grid.get(&[x, y, z]) {
                    candidates.extend(indices.iter().copied());
                }
            }
        }
    }
    candidates.into_iter().collect()
}

fn clearance(part: &NativePart) -> Option<f32> {
    let millimetres = part.material["fitClearanceMm"].as_f64()? as f32;
    (millimetres.is_finite() && millimetres > 0. && millimetres <= 10.)
        .then_some(millimetres * 0.01)
}

/// Validated total displacement authority shared with thin-shell recoupling.
/// The default remains the legacy 40 mm constant.
pub(crate) fn correction_limit(part: &NativePart) -> Result<f32, String> {
    let Some(value) = part.material.get("fitMaxCorrectionMm") else {
        return Ok(MAX_CORRECTION);
    };
    if clearance(part).is_none() {
        return Err("fitMaxCorrectionMm requires fitClearanceMm".into());
    }
    let millimetres = value.as_f64().ok_or("fitMaxCorrectionMm must be numeric")?;
    if !millimetres.is_finite() || !(0.0..=80.0).contains(&millimetres) || millimetres == 0.0 {
        return Err("fitMaxCorrectionMm must be greater than zero and at most 80".into());
    }
    let limit = if millimetres == 40.0 {
        // Preserve the exact legacy constant and its downstream grid/cap
        // behavior for an explicit default request.
        MAX_CORRECTION
    } else {
        (millimetres / 100.0) as f32
    };
    let clearance = clearance(part).expect("checked above");
    if limit < clearance {
        return Err("fitMaxCorrectionMm must be at least fitClearanceMm".into());
    }
    Ok(limit)
}

/// Immutable visible-body collision oracle shared by the shell lining pass.
/// It deliberately preserves the nearest-surface-first signed test used by
/// ordinary clothing clearance.
pub(crate) struct ClearanceCollider {
    triangles: Vec<Triangle>,
    grid: BTreeMap<[i32; 3], Vec<usize>>,
}

pub(crate) fn clearance_collider(body: &NativePart) -> Result<ClearanceCollider, String> {
    let (triangles, grid) = collider(body)?;
    Ok(ClearanceCollider { triangles, grid })
}

/// Builds the ordinary clothing collider with the same bounded local domain
/// used by a configured correction limit. Shell callers pass their immutable
/// pre-clearance snapshot because their ordinary and recoupling passes share
/// one total displacement budget from that state.
pub(crate) fn clearance_collider_with_limit(
    body: &NativePart,
    correction_limit: f32,
    pre_clearance: &[[f32; 3]],
) -> Result<ClearanceCollider, String> {
    let positions = pre_clearance
        .iter()
        .copied()
        .map(Vec3::from)
        .collect::<Vec<_>>();
    let (triangles, grid) = collider_with_limit(
        body,
        GridBudget {
            cells: MAX_GRID_CELLS,
            references: MAX_GRID_REFERENCES,
        },
        correction_limit,
        &positions,
    )?;
    Ok(ClearanceCollider { triangles, grid })
}

pub(crate) fn clearance_request(part: &NativePart) -> Option<f32> {
    clearance(part)
}

pub(crate) fn clearance_violation_at(
    collider: &ClearanceCollider,
    point: Vec3,
    clearance: f32,
) -> Option<(Vec3, f32)> {
    collider.grid.get(&cell(point)).and_then(|candidates| {
        violation(
            point,
            candidates,
            &collider.triangles,
            clearance,
            MAX_CORRECTION,
        )
    })
}

pub(crate) fn clearance_violation_at_with_limit(
    collider: &ClearanceCollider,
    point: Vec3,
    clearance: f32,
    correction_limit: f32,
) -> Option<(Vec3, f32)> {
    collider.grid.get(&cell(point)).and_then(|candidates| {
        violation(
            point,
            candidates,
            &collider.triangles,
            clearance,
            correction_limit,
        )
    })
}

/// A second, explicit opt-in for the small set of imported surfaces whose
/// triangle can still cross the body after coherent vertex smoothing.  This
/// never changes a legacy garment or expands the global displacement ball.
fn triangle_clearance(part: &NativePart) -> bool {
    part.material["fitTriangleClearance"].as_bool() == Some(true)
}

fn coherent_clearance_spacing(part: &NativePart) -> Result<Option<f32>, String> {
    let Some(value) = part.material.get("fitClearanceCoherentGridDM") else {
        return Ok(None);
    };
    if clearance(part).is_none() {
        return Err("fitClearanceCoherentGridDM requires valid fitClearanceMm".into());
    }
    let spacing = value
        .as_f64()
        .ok_or("fitClearanceCoherentGridDM must be a finite number within 0.1..=0.5")?
        as f32;
    if !spacing.is_finite() || !(0.1..=0.5).contains(&spacing) {
        return Err("fitClearanceCoherentGridDM must be a finite number within 0.1..=0.5".into());
    }
    Ok(Some(spacing))
}

/// Applies body clearance through a shared field indexed by immutable authored
/// garment coordinates.  The collision oracle remains in current fitted space;
/// only the field basis is authored, which keeps seam copies coherent after
/// body morphing has separated their bindings.
fn fit_coherent_clearance(
    part: &mut NativePart,
    body: &NativePart,
    clearance: f32,
    spacing: f32,
    correction_limit: f32,
) -> Result<ClothFitStats, String> {
    type Key = [i32; 3];
    struct Node {
        displacement: Vec3,
    }
    let count = part.positions.len();
    if count == 0
        || count > MAX_COHERENT_CLEARANCE_VERTICES
        || part.indices.len() % 3 != 0
        || part.indices.len() > MAX_COHERENT_CLEARANCE_INDICES
        || part.uvs.len() != count
        || part.refs.len() != count
        || part.ref_weights.len() != count
        || part.indices.iter().any(|&index| index as usize >= count)
    {
        return Err("fitClearanceCoherentGridDM has invalid runtime streams".into());
    }
    let source = part
        .skin_weight_source_topology
        .as_ref()
        .ok_or("fitClearanceCoherentGridDM requires immutable authored source topology")?;
    if source.positions.len() != count
        || source.positions.len() > MAX_COHERENT_CLEARANCE_VERTICES
        || source.indices.len() % 3 != 0
        || source.indices.len() > MAX_COHERENT_CLEARANCE_INDICES
        || source.positions.iter().any(|p| {
            !Vec3::from(*p).is_finite() || Vec3::from(*p).abs().max_element() > MAX_BODY_COORDINATE
        })
        || source
            .indices
            .iter()
            .any(|&i| i as usize >= source.positions.len())
    {
        return Err("fitClearanceCoherentGridDM has invalid immutable source topology".into());
    }
    let originals = part
        .positions
        .iter()
        .copied()
        .map(Vec3::from)
        .collect::<Vec<_>>();
    if originals
        .iter()
        .any(|p| !p.is_finite() || p.abs().max_element() > MAX_BODY_COORDINATE)
    {
        return Err("fitClearanceCoherentGridDM has invalid fitted positions".into());
    }
    let (triangles, grid) = collider_with_limit(
        body,
        GridBudget {
            cells: MAX_GRID_CELLS,
            references: MAX_GRID_REFERENCES,
        },
        correction_limit,
        &originals,
    )?;
    let mut node_keys = BTreeSet::<Key>::new();
    let mut keys = Vec::<[(Key, f32); 8]>::with_capacity(originals.len());
    for &authored in &source.positions {
        let voxel = Vec3::from(authored) / spacing;
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
                        if !node_keys.contains(&key)
                            && node_keys.len() >= MAX_COHERENT_CLEARANCE_NODES
                        {
                            return Err(
                                "fitClearanceCoherentGridDM exceeds the coherent node budget"
                                    .into(),
                            );
                        }
                        node_keys.insert(key);
                    }
                }
            }
        }
        keys.push(row);
    }
    let mut node_indices = BTreeMap::<Key, usize>::new();
    let mut nodes = Vec::<Node>::with_capacity(node_keys.len());
    for key in node_keys {
        node_indices.insert(key, nodes.len());
        nodes.push(Node {
            displacement: Vec3::ZERO,
        });
    }
    let controls = keys
        .iter()
        .map(|row| {
            row.iter()
                .filter(|(_, w)| *w > 0.)
                .map(|(key, w)| (node_indices[key], *w))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let sample = |point: Vec3| {
        grid.get(&cell(point)).and_then(|candidates| {
            violation(point, candidates, &triangles, clearance, correction_limit)
        })
    };
    let field = |row: &[(usize, f32)], nodes: &[Node]| -> Vec3 {
        row.iter().map(|&(i, w)| nodes[i].displacement * w).sum()
    };
    // Controls are displacement degrees of freedom, not cloth samples. Their
    // averaged locations can lie inside the body even when every surface
    // point is clear. Only actual fitted cloth points may request corrections.
    for _ in 0..6 {
        let mut requests = vec![(Vec3::ZERO, 0.); nodes.len()];
        let mut remaining = 0;
        for (&original, row) in originals.iter().zip(&controls) {
            let point = original + field(row, &nodes);
            if let Some((outward, signed)) = sample(point) {
                remaining += 1;
                let correction = outward * (clearance - signed);
                for &(node, weight) in row {
                    requests[node].0 += correction * weight;
                    requests[node].1 += weight;
                }
            }
        }
        if remaining == 0 {
            break;
        }
        for (node, (sum, weight)) in nodes.iter_mut().zip(requests) {
            if weight > 0. {
                node.displacement =
                    (node.displacement + sum / weight).clamp_length_max(correction_limit);
            }
        }
    }
    let mut stats = ClothFitStats::default();
    for ((position, &original), row) in part.positions.iter_mut().zip(&originals).zip(&controls) {
        let delta = field(row, &nodes).clamp_length_max(correction_limit);
        let point = original + delta;
        if delta.length_squared() > 0. {
            stats.corrected_vertices += 1;
            stats.max_displacement = stats.max_displacement.max(delta.length());
        }
        *position = point.to_array();
        if sample(point).is_some() {
            stats.remaining_violations += 1;
        }
    }
    Ok(stats)
}

/// Optional transition beyond the requested clearance, in millimetres.
/// Stops neighbour smoothing from inflating an already-clear waistband while
/// retaining the ordinary collision projection and its displacement budget.
fn exterior_smoothing_guard(part: &NativePart) -> Result<Option<f32>, String> {
    let Some(value) = part.material.get("fitClearanceSmoothingGuardMm") else {
        return Ok(None);
    };
    let millimetres = value
        .as_f64()
        .ok_or("fitClearanceSmoothingGuardMm must be numeric")?;
    if !millimetres.is_finite() || !(0.0..=10.0).contains(&millimetres) || millimetres == 0.0 {
        return Err("fitClearanceSmoothingGuardMm must be greater than zero and at most 10".into());
    }
    let transition = millimetres as f32 * 0.01;
    if transition <= 0.0 {
        return Err("fitClearanceSmoothingGuardMm is too small for fitting precision".into());
    }
    Ok(Some(transition))
}

fn exterior_guard_upper_band(
    part: &NativePart,
    guard: Option<f32>,
) -> Result<Option<[f32; 2]>, String> {
    let Some(value) = part.material.get("fitClearanceSmoothingGuardUpperBand") else {
        return Ok(None);
    };
    let invalid = || {
        "fitClearanceSmoothingGuardUpperBand requires a guard and [start,end] within 0..1, start < end".to_string()
    };
    let values = value
        .as_array()
        .filter(|values| values.len() == 2)
        .ok_or_else(invalid)?;
    let start = values[0].as_f64().ok_or_else(invalid)?;
    let end = values[1].as_f64().ok_or_else(invalid)?;
    if guard.is_none()
        || !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end > 1.0
        || start >= end
    {
        return Err(invalid());
    }
    let band = [start as f32, end as f32];
    if band[0] >= band[1] {
        return Err(invalid());
    }
    Ok(Some(band))
}

fn upper_clearance_band(part: &NativePart, base: f32) -> Result<Option<[f32; 3]>, String> {
    let Some(value) = part.material.get("fitClearanceUpperBandMm") else {
        return Ok(None);
    };
    let invalid = || {
        "fitClearanceUpperBandMm requires [start,end,mm], increasing fractions within 0..1 and positive mm no greater than fitClearanceMm".to_string()
    };
    let values = value
        .as_array()
        .filter(|v| v.len() == 3)
        .ok_or_else(invalid)?;
    let start = values[0].as_f64().ok_or_else(invalid)?;
    let end = values[1].as_f64().ok_or_else(invalid)?;
    let mm = values[2].as_f64().ok_or_else(invalid)?;
    if !start.is_finite()
        || !end.is_finite()
        || !mm.is_finite()
        || start < 0.0
        || end > 1.0
        || start >= end
        || mm <= 0.0
        || mm > 10.0
    {
        return Err(invalid());
    }
    let band = [start as f32, end as f32, mm as f32 * 0.01];
    if band[0] >= band[1] || band[2] <= 0.0 || band[2] > base {
        return Err(invalid());
    }
    Ok(Some(band))
}

/// Restricts an upper clearance band to body anchors hidden by an explicitly
/// higher opaque custom garment.  This is deliberately a strict opt-in: a
/// typo must not quietly make a standalone garment fit differently.
pub(crate) fn upper_clearance_band_covered_only(part: &NativePart) -> Result<bool, String> {
    match part.material.get("fitClearanceUpperBandCoveredOnly") {
        None => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "fitClearanceUpperBandCoveredOnly must be boolean".into()),
    }
}

/// Validate the covered-only request independently of geometry mutation so
/// character assembly can reject malformed dependencies before fitting any
/// custom clothing.
pub(crate) fn covered_only_upper_clearance_request(part: &NativePart) -> Result<bool, String> {
    let covered_only = upper_clearance_band_covered_only(part)?;
    if !covered_only {
        return Ok(false);
    }
    let clearance =
        clearance(part).ok_or("fitClearanceUpperBandCoveredOnly requires fitClearanceMm")?;
    if upper_clearance_band(part, clearance)?.is_none() {
        return Err("fitClearanceUpperBandCoveredOnly requires fitClearanceUpperBandMm".into());
    }
    Ok(true)
}

/// Parse the tuck request independently of ordinary clearance so character
/// assembly can reject a malformed or unsupported request before fitting any
/// other custom garment.
pub(crate) fn covered_upper_tuck_request(part: &NativePart) -> Result<Option<f32>, String> {
    let Some(value) = part.material.get("fitClearanceUpperBandCoveredTuckMm") else {
        return Ok(None);
    };
    let millimetres = value
        .as_f64()
        .ok_or("fitClearanceUpperBandCoveredTuckMm must be numeric")?;
    if !millimetres.is_finite() || !(0.0..=40.0).contains(&millimetres) || millimetres == 0.0 {
        return Err(
            "fitClearanceUpperBandCoveredTuckMm must be greater than zero and at most 40".into(),
        );
    }
    let tuck = millimetres as f32 * 0.01;
    if tuck <= 0.0 {
        return Err("fitClearanceUpperBandCoveredTuckMm is too small for fitting precision".into());
    }
    Ok(Some(tuck))
}

/// Optional inward reduction of an already-clear, covered upper band. This is
/// deliberately separate from clearance: it may never make the visible body
/// clearance smaller than the band's requested floor.
fn covered_upper_tuck(part: &NativePart, covered_only: bool) -> Result<Option<f32>, String> {
    let tuck = covered_upper_tuck_request(part)?;
    if tuck.is_some() && !covered_only {
        return Err(
            "fitClearanceUpperBandCoveredTuckMm requires fitClearanceUpperBandCoveredOnly: true"
                .into(),
        );
    }
    Ok(tuck)
}

/// A stricter alternative to canonical-anchor coverage for an authored waist
/// wall. It is deliberately a bool rather than another distance: the same
/// covered-tuck limit remains the sole movement authority.
pub(crate) fn physical_upper_coverage_request(part: &NativePart) -> Result<bool, String> {
    match part.material.get("fitClearanceUpperBandPhysicalCoverage") {
        None => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "fitClearanceUpperBandPhysicalCoverage must be boolean".into()),
    }
}

fn height_band_activation(y: f32, low: f32, high: f32, start: f32, end: f32) -> f32 {
    let t = (((y - low) / (high - low) - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Project the complete proposed displacement into the global correction ball.
/// A scalar residual is wrong after a change of normal: an orthogonal addition
/// can fit even when its length exceeds `MAX - |current|`.
fn bounded_add(displacement: Vec3, outward: Vec3, amount: f32) -> Vec3 {
    bounded_add_with_limit(displacement, outward, amount, MAX_CORRECTION)
}

fn bounded_add_with_limit(displacement: Vec3, outward: Vec3, amount: f32, limit: f32) -> Vec3 {
    (displacement + outward * amount).clamp_length_max(limit)
}

fn segment_hits_triangle(start: Vec3, end: Vec3, triangle: Triangle) -> bool {
    let direction = end - start;
    let p = direction.cross(triangle.c - triangle.a);
    let determinant = (triangle.b - triangle.a).dot(p);
    if determinant.abs() <= 1e-8 {
        return false;
    }
    let inverse = determinant.recip();
    let offset = start - triangle.a;
    let u = offset.dot(p) * inverse;
    if !(-1e-6..=1. + 1e-6).contains(&u) {
        return false;
    }
    let q = offset.cross(triangle.b - triangle.a);
    let v = direction.dot(q) * inverse;
    if v < -1e-6 || u + v > 1. + 1e-6 {
        return false;
    }
    let t = (triangle.c - triangle.a).dot(q) * inverse;
    (-1e-6..=1. + 1e-6).contains(&t)
}

fn triangles_intersect(a: Vec3, b: Vec3, c: Vec3, body: Triangle) -> bool {
    let cloth_edges = [(a, b), (b, c), (c, a)];
    if cloth_edges
        .into_iter()
        .any(|(start, end)| segment_hits_triangle(start, end, body))
    {
        return true;
    }
    let cloth = Triangle {
        a,
        b,
        c,
        outward: (b - a).cross(c - a).normalize_or_zero(),
    };
    cloth.outward != Vec3::ZERO
        && [(body.a, body.b), (body.b, body.c), (body.c, body.a)]
            .into_iter()
            .any(|(start, end)| segment_hits_triangle(start, end, cloth))
}

fn triangle_candidates(
    a: Vec3,
    b: Vec3,
    c: Vec3,
    grid: &BTreeMap<[i32; 3], Vec<usize>>,
) -> Result<Vec<usize>, String> {
    // This pass tests only triangles that intersect at their current fitted
    // positions. The body collider already inserted each body triangle with
    // its vertex-clearance margin, so expanding the cloth AABB again only
    // repeats distant cells and can exhaust the candidate budget.
    let low = cell(a.min(b).min(c));
    let high = cell(a.max(b).max(c));
    let cells = (0..3)
        .map(|axis| high[axis] as i64 - low[axis] as i64 + 1)
        .fold(1i64, i64::saturating_mul);
    if cells > MAX_CONTACT_GRID_CELLS {
        return Err("custom clothing triangle spans an unreasonable contact grid".into());
    }
    let mut candidates = BTreeSet::new();
    for x in low[0]..=high[0] {
        for y in low[1]..=high[1] {
            for z in low[2]..=high[2] {
                if let Some(indices) = grid.get(&[x, y, z]) {
                    candidates.extend(indices.iter().copied());
                }
            }
        }
    }
    Ok(candidates.into_iter().collect())
}

/// Correct true, same-facing body/cloth triangle crossings after smoothing or
/// a covered tuck. Each pass is at most the requested clearance and still
/// projects the whole displacement back into the existing 4 cm ball.
fn resolve_triangle_contacts(
    part: &NativePart,
    original: &[Vec3],
    owner: &[usize],
    displacement: &mut [Vec3],
    triangles: &[Triangle],
    grid: &BTreeMap<[i32; 3], Vec<usize>>,
    clearances: &[f32],
    movable_groups: Option<&[bool]>,
    correction_limit: f32,
) -> Result<(), String> {
    if !triangle_clearance(part) && movable_groups.is_none() {
        return Ok(());
    }
    if part.indices.len() % 3 != 0 || part.indices.len() / 3 > MAX_CONTACT_TRIANGLES {
        return Err(format!("{} exceeds triangle contact limits", part.name));
    }
    for _ in 0..TRIANGLE_CONTACT_PASSES {
        let mut desired = displacement.to_vec();
        let mut candidate_count = 0usize;
        for indices in part.indices.chunks_exact(3) {
            let vertex = |index: u32| -> Result<(usize, usize, Vec3), String> {
                let index = index as usize;
                let group = *owner
                    .get(index)
                    .ok_or_else(|| format!("{} has an invalid triangle index", part.name))?;
                Ok((index, group, original[index] + displacement[group]))
            };
            let (_ia, ga, a) = vertex(indices[0])?;
            let (_ib, gb, b) = vertex(indices[1])?;
            let (_ic, gc, c) = vertex(indices[2])?;
            if movable_groups.is_some_and(|movable| !movable[ga] && !movable[gb] && !movable[gc]) {
                continue;
            }
            let normal = (b - a).cross(c - a).normalize_or_zero();
            if normal == Vec3::ZERO {
                continue;
            }
            let candidates = triangle_candidates(a, b, c, grid)?;
            candidate_count = candidate_count.saturating_add(candidates.len());
            if candidate_count > MAX_CONTACT_CANDIDATES {
                return Err("custom clothing exceeds triangle contact candidate budget".into());
            }
            for body_index in candidates {
                let body = triangles[body_index];
                if normal.dot(body.outward) <= 0.05 || !triangles_intersect(a, b, c, body) {
                    continue;
                }
                for (group, point) in [(ga, a), (gb, b), (gc, c)] {
                    if movable_groups.is_some_and(|movable| !movable[group]) {
                        continue;
                    }
                    let clearance = clearances[group];
                    let signed = (point - body.a).dot(body.outward);
                    // This pass treats only an inward endpoint of a real
                    // crossing. Inner linings and merely close outer layers
                    // retain the vertex solver's behavior.
                    if signed < 0. {
                        let correction = (clearance - signed).min(clearance);
                        let proposed = bounded_add_with_limit(
                            desired[group],
                            body.outward,
                            correction,
                            correction_limit,
                        );
                        // Several body triangles can meet at a seam. Their
                        // combined final-contact step still has the same
                        // per-pass bound as a single requested clearance.
                        let step = (proposed - displacement[group]).clamp_length_max(clearance);
                        desired[group] =
                            (displacement[group] + step).clamp_length_max(correction_limit);
                    }
                }
            }
        }
        displacement.copy_from_slice(&desired);
    }
    Ok(())
}

fn groups(
    part: &NativePart,
) -> Result<(Vec<usize>, Vec<Vec<usize>>, Vec<BTreeSet<usize>>), String> {
    if part.positions.len() != part.refs.len() || part.positions.len() != part.ref_weights.len() {
        return Err(format!(
            "{} has inconsistent custom clothing bindings",
            part.name
        ));
    }
    let mut canonical = HashMap::new();
    let mut owner = Vec::with_capacity(part.positions.len());
    let mut members = Vec::<Vec<usize>>::new();
    for (index, position) in part.positions.iter().enumerate() {
        if !position
            .iter()
            .all(|value| value.is_finite() && value.abs() <= MAX_BODY_COORDINATE)
        {
            return Err(format!(
                "{} has a non-finite or out-of-range custom clothing vertex",
                part.name
            ));
        }
        let key = (
            position.map(f32::to_bits),
            part.refs[index],
            part.ref_weights[index].map(f32::to_bits),
        );
        let group = if let Some(&group) = canonical.get(&key) {
            group
        } else {
            let group = members.len();
            canonical.insert(key, group);
            members.push(Vec::new());
            group
        };
        owner.push(group);
        members[group].push(index);
    }
    let mut adjacent = vec![BTreeSet::new(); members.len()];
    for triangle in part.indices.chunks_exact(3) {
        let vertices = triangle
            .iter()
            .map(|index| {
                owner
                    .get(*index as usize)
                    .copied()
                    .ok_or_else(|| format!("{} has an invalid triangle index", part.name))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for &left in &vertices {
            for &right in &vertices {
                if left != right {
                    adjacent[left].insert(right);
                }
            }
        }
    }
    Ok((owner, members, adjacent))
}

/// Applies an explicit, bounded clearance request to one custom clothing part.
/// `None` means no valid opt-in was present, preserving legacy garments exactly.
pub fn fit_custom_clothing_clearance(
    part: &mut NativePart,
    body: &NativePart,
) -> Result<Option<ClothFitStats>, String> {
    fit_custom_clothing_clearance_with_coverage(part, body, None)
}

/// Applies body clearance with an optional per-vertex canonical-body coverage
/// mask.  The mask is meaningful only for the explicit covered-only upper
/// band; all inputs are validated before the part is changed.
pub fn fit_custom_clothing_clearance_with_coverage(
    part: &mut NativePart,
    body: &NativePart,
    covered_anchors: Option<&[bool]>,
) -> Result<Option<ClothFitStats>, String> {
    // This property is independent from the ordinary clearance request, so it
    // must be checked before the legacy no-clearance return below.
    let correction_limit = correction_limit(part)?;
    let coherent_spacing = coherent_clearance_spacing(part)?;
    if coherent_spacing.is_some() {
        let incompatible = [
            "fitTriangleClearance",
            "fitClearanceExcludedVertices",
            "fitClearanceUpperBandMm",
            "fitClearanceUpperBandCoveredOnly",
            "fitClearanceUpperBandCoveredTuckMm",
            "fitClearanceUpperBandPhysicalCoverage",
            "fitLayerClearanceMm",
            "fitShellDisplacementPairs",
        ];
        if incompatible
            .iter()
            .any(|key| part.material.get(*key).is_some())
        {
            return Err(
                "fitClearanceCoherentGridDM cannot be combined with independent clearance metadata"
                    .into(),
            );
        }
        if covered_anchors.is_some() {
            return Err(
                "fitClearanceCoherentGridDM cannot be combined with coverage clearance".into(),
            );
        }
    }
    let covered_only = covered_only_upper_clearance_request(part)?;
    // Parse the independent opt-in before the legacy no-clearance early return:
    // an authored tuck must never be silently ignored because its base request
    // is absent or malformed.
    let tuck = covered_upper_tuck(part, covered_only)?;
    if physical_upper_coverage_request(part)? {
        if tuck.is_none() {
            return Err(
                "fitClearanceUpperBandPhysicalCoverage requires fitClearanceUpperBandCoveredTuckMm"
                    .into(),
            );
        }
        if part.material.get("fitMaxCorrectionMm").is_some()
            && (correction_limit - MAX_CORRECTION).abs() > f32::EPSILON
        {
            return Err(
                "fitClearanceUpperBandPhysicalCoverage does not support nondefault fitMaxCorrectionMm"
                    .into(),
            );
        }
    }
    if part.material.get("fitClearanceExcludedVertices").is_some() && clearance(part).is_none() {
        return Err("fitClearanceExcludedVertices requires fitClearanceMm".into());
    }
    let Some(clearance) = clearance(part) else {
        return Ok(None);
    };
    if let Some(spacing) = coherent_spacing {
        return fit_coherent_clearance(part, body, clearance, spacing, correction_limit).map(Some);
    }
    let upper_clearance = upper_clearance_band(part, clearance)?;
    let covered_anchors = if covered_only {
        let covered = covered_anchors
            .ok_or("fitClearanceUpperBandCoveredOnly requires canonical anchor coverage")?;
        if covered.len() != part.positions.len() {
            return Err(
                "canonical anchor coverage length does not match custom clothing vertices".into(),
            );
        }
        Some(covered)
    } else {
        None
    };
    let original: Vec<Vec3> = part.positions.iter().copied().map(Vec3::from).collect();
    let (triangles, grid) = collider_with_limit(
        body,
        GridBudget {
            cells: MAX_GRID_CELLS,
            references: MAX_GRID_REFERENCES,
        },
        correction_limit,
        &original,
    )?;
    let (owner, members, adjacent) = groups(part)?;
    let excluded_groups = if let Some(value) = part.material.get("fitClearanceExcludedVertices") {
        let indices = value
            .as_array()
            .ok_or("fitClearanceExcludedVertices must be an array")?;
        let mut selected = vec![false; part.positions.len()];
        for value in indices {
            let index = usize::try_from(
                value
                    .as_u64()
                    .ok_or("fitClearanceExcludedVertices must contain integer indices")?,
            )
            .map_err(|_| "fitClearanceExcludedVertices contains an out-of-range index")?;
            if index >= selected.len() || selected[index] {
                return Err(
                    "fitClearanceExcludedVertices must contain unique in-range indices".into(),
                );
            }
            selected[index] = true;
        }
        members.iter().map(|vertices| {
            let any = vertices.iter().any(|&vertex| selected[vertex]);
            if any && !vertices.iter().all(|&vertex| selected[vertex]) { return Err("fitClearanceExcludedVertices must include whole exact-position weld groups".into()); }
            Ok(any)
        }).collect::<Result<Vec<_>, String>>()?
    } else {
        vec![false; members.len()]
    };
    let guard = exterior_smoothing_guard(part)?;
    let upper_band = exterior_guard_upper_band(part, guard)?;
    let (low_y, high_y) = original
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), p| {
            (low.min(p.y), high.max(p.y))
        });
    if (upper_band.is_some() || upper_clearance.is_some()) && high_y - low_y <= f32::EPSILON {
        return Err("upper clearance bands require a nonzero garment height".into());
    }
    let clearances: Vec<f32> = members
        .iter()
        .enumerate()
        .map(|(group, vertices)| {
            if excluded_groups[group] {
                return 0.0;
            }
            upper_clearance.map_or(clearance, |[start, end, upper]| {
                // A source-weld group receives one coherent displacement. Do
                // not reduce its body floor unless every UV sibling's anchor
                // is hidden; choosing the representative could pull an
                // exposed sibling into a visible body surface.
                if covered_anchors
                    .is_some_and(|covered| !vertices.iter().all(|&vertex| covered[vertex]))
                {
                    return clearance;
                }
                let activation =
                    height_band_activation(original[vertices[0]].y, low_y, high_y, start, end);
                clearance * (1.0 - activation) + upper * activation
            })
        })
        .collect();
    let smoothing_weights: Vec<f32> = members
        .iter()
        .enumerate()
        .map(|(group, vertices)| {
            if excluded_groups[group] {
                return 0.0;
            }
            let Some(transition) = guard else {
                return SMOOTHING;
            };
            let point = original[vertices[0]];
            let clearance = clearances[group];
            let activation = upper_band.map_or(1.0, |[start, end]| {
                height_band_activation(point.y, low_y, high_y, start, end)
            });
            let nearest = grid.get(&cell(point)).and_then(|candidates| {
                nearest_surface(point, candidates, &triangles, clearance, correction_limit)
            });
            let fade = nearest.map_or(1.0, |(_, signed)| {
                let t = ((signed - clearance) / transition).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            });
            SMOOTHING * (1.0 - activation * fade)
        })
        .collect();
    let mut displacement = vec![Vec3::ZERO; members.len()];
    for _ in 0..PASSES {
        let mut desired = displacement.clone();
        for (group, vertices) in members.iter().enumerate() {
            if excluded_groups[group] {
                continue;
            }
            let clearance = clearances[group];
            let point = original[vertices[0]] + displacement[group];
            if let Some(candidates) = grid.get(&cell(point)) {
                if let Some((outward, signed)) =
                    violation(point, candidates, &triangles, clearance, correction_limit)
                {
                    desired[group] = bounded_add_with_limit(
                        desired[group],
                        outward,
                        clearance - signed,
                        correction_limit,
                    );
                }
            }
        }
        for group in 0..displacement.len() {
            if excluded_groups[group] {
                displacement[group] = Vec3::ZERO;
                continue;
            }
            let neighbour_average = if adjacent[group].is_empty() {
                desired[group]
            } else {
                adjacent[group]
                    .iter()
                    .map(|&other| desired[other])
                    .sum::<Vec3>()
                    / adjacent[group].len() as f32
            };
            let smoothed = desired[group].lerp(neighbour_average, smoothing_weights[group]);
            displacement[group] = smoothed.clamp_length_max(correction_limit);
        }
    }
    // Smoothing the final pass can pull a just-cleared contact back through
    // the requested clearance. Project once more without smoothing, while
    // limiting this last correction to one requested clearance (at most 1cm)
    // so it cannot reintroduce the old independent large projection noise.
    for (group, vertices) in members.iter().enumerate() {
        if excluded_groups[group] {
            continue;
        }
        let clearance = clearances[group];
        let point = original[vertices[0]] + displacement[group];
        if let Some(candidates) = grid.get(&cell(point)) {
            if let Some((outward, signed)) =
                violation(point, candidates, &triangles, clearance, correction_limit)
            {
                let correction = (clearance - signed).min(clearance);
                displacement[group] = bounded_add_with_limit(
                    displacement[group],
                    outward,
                    correction,
                    correction_limit,
                );
            }
        }
    }
    // A covered waistband can intentionally settle back toward the visible
    // body after ordinary clearance has removed intersections. Require every
    // member of a source-weld group to be covered: using a single representative
    // would pull an uncovered UV sibling inward as well. Record the groups so a
    // final, bounded triangle-contact pass can repair an inward crossing
    // without moving the uncovered or lower band. Allocate its bitmap only
    // after an actual tuck, so a pants-alone covered request has no extra
    // geometry work when final coverage proves nothing is hidden.
    let mut tucked_groups = None::<Vec<bool>>;
    if let (Some(tuck), Some(covered), Some([start, end, _])) =
        (tuck, covered_anchors, upper_clearance)
    {
        for (group, vertices) in members.iter().enumerate() {
            if excluded_groups[group] || !vertices.iter().all(|&vertex| covered[vertex]) {
                continue;
            }
            let activation =
                height_band_activation(original[vertices[0]].y, low_y, high_y, start, end);
            if activation <= 0. {
                continue;
            }
            let point = original[vertices[0]] + displacement[group];
            let candidates = nearby_candidates(&grid, point, TUCK_QUERY_REACH);
            let Some((outward, signed)) = nearest_surface(
                point,
                &candidates,
                &triangles,
                clearances[group],
                TUCK_QUERY_REACH,
            ) else {
                continue;
            };
            let amount = (signed - clearances[group]).max(0.).min(tuck * activation);
            if amount <= 0. {
                continue;
            }
            let proposed =
                (displacement[group] - outward * amount).clamp_length_max(correction_limit);
            let settled = original[vertices[0]] + proposed;
            let settled_candidates = nearby_candidates(&grid, settled, correction_limit);
            // Reject rather than partially applying a move if a seam changes
            // the nearest body surface or the clearance floor would be lost.
            if nearest_surface(
                settled,
                &settled_candidates,
                &triangles,
                clearances[group],
                correction_limit,
            )
            // The source and collider streams are f32. Allow only their
            // established 1e-5 contact tolerance at the physical floor.
            .is_some_and(|(_, signed)| signed >= clearances[group] - CLEARANCE_FLOOR_EPSILON)
            {
                displacement[group] = proposed;
                tucked_groups.get_or_insert_with(|| vec![false; members.len()])[group] = true;
            }
        }
    }
    let tuck_touched = tucked_groups
        .as_deref()
        .is_some_and(|groups| groups.iter().any(|&tucked| tucked));
    // Contact repair moves only outward, but at seams a different body face
    // can become nearest. Keep a local snapshot and restore a touched group if
    // that change would lose the floor established above.
    let pre_contact_tuck = tuck_touched.then(|| displacement.clone());
    if triangle_clearance(part) || tuck_touched {
        // True current intersections need only overlapping AABB cells. The
        // vertex-distance collider's 5 cm neighbourhood includes many distant
        // surfaces and exhausts the contact budget on dense source garments.
        // Rebuild only the broad phase with a tiny numerical boundary margin;
        // a nondefault limit retains the same local triangle order as above.
        let contact_domain = if correction_limit == MAX_CORRECTION {
            None
        } else {
            collider_domain(&original, correction_limit)?
        };
        let (_, contact_grid) = collider_with_optional_domain(
            body,
            GridBudget {
                cells: MAX_GRID_CELLS,
                references: MAX_GRID_REFERENCES,
            },
            1e-5,
            contact_domain,
        )?;
        let protected_contact_groups =
            excluded_groups.iter().any(|&excluded| excluded).then(|| {
                excluded_groups
                    .iter()
                    .enumerate()
                    .map(|(group, &excluded)| {
                        !excluded
                            && (triangle_clearance(part)
                                || tucked_groups.as_ref().is_some_and(|groups| groups[group]))
                    })
                    .collect::<Vec<_>>()
            });
        resolve_triangle_contacts(
            part,
            &original,
            &owner,
            &mut displacement,
            &triangles,
            &contact_grid,
            &clearances,
            if let Some(movable) = protected_contact_groups.as_deref() {
                Some(movable)
            } else if triangle_clearance(part) {
                None
            } else {
                tucked_groups.as_deref()
            },
            correction_limit,
        )?;
    }
    if let (Some(before), Some(tucked_groups)) = (pre_contact_tuck, tucked_groups.as_deref()) {
        for (group, &tucked) in tucked_groups.iter().enumerate() {
            if !tucked {
                continue;
            }
            let point = original[members[group][0]] + displacement[group];
            let candidates = nearby_candidates(&grid, point, correction_limit);
            if !nearest_surface(
                point,
                &candidates,
                &triangles,
                clearances[group],
                correction_limit,
            )
            .is_some_and(|(_, signed)| signed >= clearances[group] - CLEARANCE_FLOOR_EPSILON)
            {
                displacement[group] = before[group];
            }
        }
    }
    let mut stats = ClothFitStats::default();
    for (group, vertices) in members.iter().enumerate() {
        let delta = displacement[group];
        if delta.length_squared() > 0. {
            stats.corrected_vertices += vertices.len();
            stats.max_displacement = stats.max_displacement.max(delta.length());
        }
        for &vertex in vertices {
            part.positions[vertex] = (original[vertex] + delta).to_array();
        }
    }
    let mut report_cache = HashMap::<[i32; 3], Vec<usize>>::new();
    let mut report_cache_candidates = 0usize;
    for (group, vertices) in members.iter().enumerate() {
        if excluded_groups[group] {
            continue;
        }
        let point = original[vertices[0]] + displacement[group];
        let clearance = clearances[group];
        let query_cell = cell(point);
        let remains = if let Some(cached) = report_cache.get(&query_cell) {
            violation(point, cached, &triangles, clearance, REPORT_DISTANCE).is_some()
        } else {
            let candidates = nearby_candidates(&grid, point, REPORT_DISTANCE);
            let remains =
                violation(point, &candidates, &triangles, clearance, REPORT_DISTANCE).is_some();
            if report_cache.len() < MAX_REPORT_CACHE_CELLS
                && report_cache_candidates.saturating_add(candidates.len())
                    <= MAX_REPORT_CACHE_CANDIDATES
            {
                report_cache_candidates += candidates.len();
                report_cache.insert(query_cell, candidates);
            }
            remains
        };
        if remains {
            stats.remaining_violations += vertices.len();
        }
    }
    Ok(Some(stats))
}

/// Pulls a covered upper waistband beneath a final opaque outer mesh, using
/// the full morphed body as a bounded floor. An already-under-floor authored
/// wall may move outward only enough to restore that floor after the same
/// final-outer proof; this remains a local phase, not a body-clearance pass.
/// It is intentionally a second, explicit phase after canonical covered
/// fitting and never changes a garment without the opt-in.
pub(crate) fn fit_custom_clothing_physical_upper_tuck(
    part: &mut NativePart,
    full_body: &NativePart,
    outer: &[&NativePart],
    source_positions: &[[f32; 3]],
) -> Result<Option<ClothFitStats>, String> {
    let configured_limit = correction_limit(part)?;
    if part.material.get("fitMaxCorrectionMm").is_some()
        && (configured_limit - MAX_CORRECTION).abs() > f32::EPSILON
    {
        return Err(
            "fitClearanceUpperBandPhysicalCoverage does not support nondefault fitMaxCorrectionMm"
                .into(),
        );
    }
    if !physical_upper_coverage_request(part)? {
        return Ok(None);
    }
    let covered_only = covered_only_upper_clearance_request(part)?;
    let tuck = covered_upper_tuck(part, covered_only)?.ok_or(
        "fitClearanceUpperBandPhysicalCoverage requires fitClearanceUpperBandCoveredTuckMm",
    )?;
    let base =
        clearance(part).ok_or("fitClearanceUpperBandPhysicalCoverage requires fitClearanceMm")?;
    let [start, end, floor] = upper_clearance_band(part, base)?
        .ok_or("fitClearanceUpperBandPhysicalCoverage requires fitClearanceUpperBandMm")?;
    if source_positions.len() != part.positions.len()
        || source_positions
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > MAX_BODY_COORDINATE)
    {
        return Err(format!(
            "{} has inconsistent physical-tuck source positions",
            part.name
        ));
    }
    let (owner, members, _) = groups(part)?;
    // This environment-only diagnostic is parsed before any phase mutation.
    // With the variable absent it allocates no trace state and emits nothing.
    let mut trace = physical_trace_request(&owner, &members)?;
    let excluded_groups = if let Some(value) = part.material.get("fitClearanceExcludedVertices") {
        let values = value
            .as_array()
            .ok_or("fitClearanceExcludedVertices must be an array")?;
        let mut selected = vec![false; part.positions.len()];
        for value in values {
            let index = usize::try_from(
                value
                    .as_u64()
                    .ok_or("fitClearanceExcludedVertices must contain integer indices")?,
            )
            .map_err(|_| "fitClearanceExcludedVertices contains an out-of-range index")?;
            if index >= selected.len() || selected[index] {
                return Err(
                    "fitClearanceExcludedVertices must contain unique in-range indices".into(),
                );
            }
            selected[index] = true;
        }
        members.iter().map(|vertices| {
            let any = vertices.iter().any(|&v| selected[v]);
            if any && !vertices.iter().all(|&v| selected[v]) {
                return Err("fitClearanceExcludedVertices must include whole exact-position weld groups".into());
            }
            Ok(any)
        }).collect::<Result<Vec<_>, String>>()?
    } else {
        vec![false; members.len()]
    };
    if part.indices.len() % 3 != 0
        || part
            .indices
            .iter()
            .any(|&index| index as usize >= part.positions.len())
    {
        return Err(format!(
            "{} has invalid physical-tuck triangle indices",
            part.name
        ));
    }
    if outer.iter().all(|part| part.indices.is_empty()) {
        if let Some(trace) = trace.as_mut() {
            for record in trace.groups.values_mut() {
                record.rejection = Some("no_outer_triangles");
            }
            let source: Vec<Vec3> = source_positions.iter().copied().map(Vec3::from).collect();
            let current: Vec<Vec3> = part.positions.iter().copied().map(Vec3::from).collect();
            let displacement = members
                .iter()
                .map(|vertices| current[vertices[0]] - source[vertices[0]])
                .collect::<Vec<_>>();
            emit_physical_trace(trace, &source, &current, &members, &displacement);
        }
        return Ok(Some(ClothFitStats::default()));
    }
    // The query itself covers the complete 8 cm reach. Expanding every
    // triangle by that reach *and* walking its neighbouring cells multiplies
    // dense body/outer candidates, without finding any additional hit.
    let (body_triangles, body_grid) = collider_with_margin(
        full_body,
        GridBudget {
            cells: MAX_GRID_CELLS,
            references: MAX_GRID_REFERENCES,
        },
        0.0,
    )?;
    let (outer_triangles, outer_grid) = collider_parts(outer, 0.0)?;
    let source: Vec<Vec3> = source_positions.iter().copied().map(Vec3::from).collect();
    let current: Vec<Vec3> = part.positions.iter().copied().map(Vec3::from).collect();
    let (low_y, high_y) = source
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), p| {
            (low.min(p.y), high.max(p.y))
        });
    if high_y - low_y <= f32::EPSILON {
        return Err("upper clearance bands require a nonzero garment height".into());
    }
    // All later state is indexed by an exact weld group, never by a vertex.
    // A stale source snapshot that no longer maps coherently to a current weld
    // is rejected before any group can be moved through one sibling's state.
    let mut displacement = Vec::with_capacity(members.len());
    for vertices in &members {
        let representative = current[vertices[0]] - source[vertices[0]];
        if vertices.iter().any(|&vertex| {
            (current[vertex] - source[vertex] - representative).length() > CLEARANCE_FLOOR_EPSILON
        }) {
            return Err(format!(
                "{} has a physical-tuck source snapshot that splits an exact weld group",
                part.name
            ));
        }
        if representative.length() > MAX_CORRECTION + CLEARANCE_FLOOR_EPSILON {
            return Err(format!(
                "{} exceeds the physical-tuck source correction budget",
                part.name
            ));
        }
        displacement.push(representative);
    }
    let canonical_displacement = displacement.clone();
    let mut candidates_seen = 0usize;
    let mut touched = vec![false; members.len()];
    for (group, vertices) in members.iter().enumerate() {
        let activation = height_band_activation(source[vertices[0]].y, low_y, high_y, start, end);
        if let Some(record) = trace
            .as_mut()
            .and_then(|trace| trace.groups.get_mut(&group))
        {
            record.activation = Some(activation);
        }
        if excluded_groups[group] {
            if let Some(record) = trace
                .as_mut()
                .and_then(|trace| trace.groups.get_mut(&group))
            {
                record.rejection = Some("excluded_weld_group");
            }
            continue;
        }
        if activation <= 0. {
            if let Some(record) = trace
                .as_mut()
                .and_then(|trace| trace.groups.get_mut(&group))
            {
                record.rejection = Some("outside_upper_source_y_band");
            }
            continue;
        }
        let mut normal = None;
        // `lower..=upper` is the signed-normal interval that leaves every
        // sibling above the body floor and below the final outer mesh floor.
        let mut lower = f32::NEG_INFINITY;
        let mut upper = f32::INFINITY;
        let mut eligible = true;
        for &vertex in vertices {
            let point = source[vertex] + displacement[group];
            let candidates = physical_tuck_candidates(&body_grid, point, &mut candidates_seen)?;
            let Some((vertex_normal, signed, _body_point)) =
                nearest_surface_point(point, &candidates, &body_triangles, floor, TUCK_QUERY_REACH)
            else {
                if let Some(record) = trace
                    .as_mut()
                    .and_then(|trace| trace.groups.get_mut(&group))
                {
                    record.rejection = Some("missing_nearest_body_surface");
                }
                eligible = false;
                break;
            };
            if normal.is_some_and(|shared: Vec3| shared.dot(vertex_normal) < 0.999) {
                // A seam has no single displacement direction that can safely
                // represent every member of this exact source weld.
                if let Some(record) = trace
                    .as_mut()
                    .and_then(|trace| trace.groups.get_mut(&group))
                {
                    record.rejection = Some("weld_body_normals_diverge");
                }
                eligible = false;
                break;
            }
            normal.get_or_insert(vertex_normal);
            // Preserve the garment's tangential position at body-triangle
            // edges. The closest body point is only a signed-distance
            // reference, not the line along which this garment can tuck.
            let ray_origin = point - vertex_normal * signed;
            let outer_candidates = physical_tuck_ray_candidates(
                &outer_grid,
                ray_origin,
                vertex_normal,
                &mut candidates_seen,
            )?;
            let Some(outer_distance) = outer_candidates
                .iter()
                .copied()
                .filter_map(|index| {
                    let triangle = outer_triangles[index];
                    (triangle.outward.dot(vertex_normal) > 0.05)
                        .then(|| {
                            ray_triangle_distance(
                                ray_origin + vertex_normal * CLEARANCE_FLOOR_EPSILON,
                                vertex_normal,
                                triangle,
                            )
                        })
                        .flatten()
                        .filter(|distance| *distance >= 0.0 && *distance <= TUCK_QUERY_REACH)
                })
                .min_by(f32::total_cmp)
            else {
                if let Some(record) = trace
                    .as_mut()
                    .and_then(|trace| trace.groups.get_mut(&group))
                {
                    record.signed = Some(signed);
                    record.rejection = Some("missing_forward_outer_ray_hit");
                }
                eligible = false;
                break;
            };
            // Leave the upper-band floor on both surfaces. This rejects hems,
            // holes, and meshes whose first outward surface has no usable room.
            let ceiling = outer_distance + CLEARANCE_FLOOR_EPSILON - floor;
            if let Some(record) = trace
                .as_mut()
                .and_then(|trace| trace.groups.get_mut(&group))
            {
                record.signed = Some(signed);
                record.ceiling = Some(ceiling);
            }
            if ceiling < floor {
                if let Some(record) = trace
                    .as_mut()
                    .and_then(|trace| trace.groups.get_mut(&group))
                {
                    record.rejection = Some("outer_room_smaller_than_floor");
                }
                eligible = false;
                break;
            }
            lower = lower.max(floor - signed);
            upper = upper.min(ceiling - signed);
            if lower > upper {
                if let Some(record) = trace
                    .as_mut()
                    .and_then(|trace| trace.groups.get_mut(&group))
                {
                    record.rejection = Some("body_floor_and_outer_ceiling_interval_empty");
                }
                eligible = false;
                break;
            }
        }
        let Some(normal) = normal else {
            continue;
        };
        if !eligible {
            continue;
        }
        let prior = displacement[group];
        let perpendicular_sq = (prior - normal * prior.dot(normal)).length_squared();
        let residual = (MAX_CORRECTION * MAX_CORRECTION - perpendicular_sq)
            .max(0.0)
            .sqrt();
        let budget_lower = -prior.dot(normal) - residual;
        let budget_upper = -prior.dot(normal) + residual;
        lower = lower.max(budget_lower);
        upper = upper.min(budget_upper);
        let cap = tuck * activation;
        let step = if lower > 0. {
            // A small positive move is permitted only to restore an already
            // under-floor source wall while a final opaque outer surface proves
            // room. This is not a general hidden-body clearance pass.
            (lower <= upper && lower <= cap).then_some(lower)
        } else if upper < 0. {
            // Pull inward as far as the authored tuck request and source
            // budget permit; a partial inward step stays above the body floor.
            let proposed = upper.max(-cap).max(lower);
            (proposed <= upper).then_some(proposed)
        } else {
            None
        };
        if let Some(record) = trace
            .as_mut()
            .and_then(|trace| trace.groups.get_mut(&group))
        {
            record.proposed_step = step;
            if step.is_none() {
                record.rejection = Some(if lower > upper {
                    "source_relative_budget_interval_empty"
                } else if lower > 0. {
                    "floor_recovery_exceeds_tuck_cap"
                } else if upper < 0. {
                    "inward_tuck_exceeds_source_budget"
                } else {
                    "already_within_body_and_outer_interval"
                });
            }
        }
        if let Some(step) = step.filter(|step| step.abs() > 0.) {
            displacement[group] += normal * step;
            touched[group] = true;
        }
    }
    if touched.iter().any(|&value| value) {
        let (_, contact_grid) = collider_with_margin(
            full_body,
            GridBudget {
                cells: MAX_GRID_CELLS,
                references: MAX_GRID_REFERENCES,
            },
            1e-5,
        )?;
        let floors = vec![floor; members.len()];
        resolve_triangle_contacts(
            part,
            &source,
            &owner,
            &mut displacement,
            &body_triangles,
            &contact_grid,
            &floors,
            Some(&touched),
            MAX_CORRECTION,
        )?;
        for (group, vertices) in members.iter().enumerate() {
            if !touched[group] {
                continue;
            }
            let point = source[vertices[0]] + displacement[group];
            let candidates = physical_tuck_candidates(&body_grid, point, &mut candidates_seen)?;
            let settled =
                nearest_surface_point(point, &candidates, &body_triangles, floor, TUCK_QUERY_REACH);
            let room_remains = if let Some((normal, signed, _body_point)) = settled {
                let ray_origin = point - normal * signed;
                let outer_candidates = physical_tuck_ray_candidates(
                    &outer_grid,
                    ray_origin,
                    normal,
                    &mut candidates_seen,
                )?;
                outer_candidates
                    .iter()
                    .copied()
                    .filter_map(|index| {
                        let triangle = outer_triangles[index];
                        (triangle.outward.dot(normal) > 0.05)
                            .then(|| {
                                ray_triangle_distance(
                                    ray_origin + normal * CLEARANCE_FLOOR_EPSILON,
                                    normal,
                                    triangle,
                                )
                            })
                            .flatten()
                            .filter(|distance| *distance >= 0.0 && *distance <= TUCK_QUERY_REACH)
                    })
                    .min_by(f32::total_cmp)
                    .is_some_and(|distance| {
                        signed >= floor - CLEARANCE_FLOOR_EPSILON
                            && signed <= distance + CLEARANCE_FLOOR_EPSILON - floor
                    })
            } else {
                false
            };
            if let Some(record) = trace
                .as_mut()
                .and_then(|trace| trace.groups.get_mut(&group))
            {
                record.post_contact = Some(if room_remains {
                    "retained_body_and_outer_room"
                } else {
                    "room_rollback_to_canonical"
                });
            }
            if !room_remains {
                // A failed repair may not retain the tuck that caused it.
                // Restore this entire weld group to its settled canonical-fit
                // position, rather than its already-tucked pre-contact state.
                displacement[group] = canonical_displacement[group];
            }
        }
        // Vertex floors alone cannot prove a sloped outer triangle remains
        // separate after contact repair. Compare every touched cloth triangle
        // to its settled canonical position and roll back all implicated welds.
        let body_rollback = introduced_triangle_crossing_groups(
            part,
            &source,
            &owner,
            &canonical_displacement,
            &displacement,
            &touched,
            &body_triangles,
            &body_grid,
            &mut candidates_seen,
        )?;
        let outer_rollback = introduced_triangle_crossing_groups(
            part,
            &source,
            &owner,
            &canonical_displacement,
            &displacement,
            &touched,
            &outer_triangles,
            &outer_grid,
            &mut candidates_seen,
        )?;
        if let Some(trace) = trace.as_mut() {
            for group in &body_rollback {
                if let Some(record) = trace.groups.get_mut(group) {
                    record.body_triangle_rollback = true;
                }
            }
            for group in &outer_rollback {
                if let Some(record) = trace.groups.get_mut(group) {
                    record.outer_triangle_rollback = true;
                }
            }
        }
        for group in body_rollback.union(&outer_rollback) {
            displacement[*group] = canonical_displacement[*group];
        }
    }
    let mut stats = ClothFitStats::default();
    for (group, vertices) in members.iter().enumerate() {
        let point = source[vertices[0]] + displacement[group];
        let delta = point - current[vertices[0]];
        if delta.length_squared() > 0.0 {
            stats.corrected_vertices += vertices.len();
            stats.max_displacement = stats.max_displacement.max(delta.length());
        }
        for &vertex in vertices {
            part.positions[vertex] = point.to_array();
        }
    }
    if let Some(trace) = trace.as_ref() {
        emit_physical_trace(trace, &source, &current, &members, &displacement);
    }
    Ok(Some(stats))
}

/// Return touched groups whose new triangle intersects a collider triangle that
/// its canonical-fit position did not. Restoring every touched group in such a
/// triangle is monotone: it can only remove this phase's motion, never retain a
/// half-tucked crossing through an unchanged sibling.
fn introduced_triangle_crossing_groups(
    part: &NativePart,
    source: &[Vec3],
    owner: &[usize],
    canonical: &[Vec3],
    displacement: &[Vec3],
    touched: &[bool],
    collider_triangles: &[Triangle],
    collider_grid: &BTreeMap<[i32; 3], Vec<usize>>,
    candidates_seen: &mut usize,
) -> Result<BTreeSet<usize>, String> {
    let mut rollback = BTreeSet::new();
    for indices in part.indices.chunks_exact(3) {
        let vertex = |index: u32| {
            let vertex = index as usize;
            owner
                .get(vertex)
                .copied()
                .map(|group| (vertex, group))
                .ok_or_else(|| format!("{} has invalid physical-tuck triangle indices", part.name))
        };
        let (ia, ga) = vertex(indices[0])?;
        let (ib, gb) = vertex(indices[1])?;
        let (ic, gc) = vertex(indices[2])?;
        if !touched[ga] && !touched[gb] && !touched[gc] {
            continue;
        }
        let current = [
            source[ia] + displacement[ga],
            source[ib] + displacement[gb],
            source[ic] + displacement[gc],
        ];
        let baseline = [
            source[ia] + canonical[ga],
            source[ib] + canonical[gb],
            source[ic] + canonical[gc],
        ];
        let candidates = triangle_candidates(current[0], current[1], current[2], collider_grid)?;
        *candidates_seen = candidates_seen.saturating_add(candidates.len());
        if *candidates_seen > MAX_PHYSICAL_TUCK_CANDIDATES {
            return Err(format!(
                "custom clothing exceeds physical-tuck candidate budget ({}/{MAX_PHYSICAL_TUCK_CANDIDATES})",
                *candidates_seen
            ));
        }
        if candidates.into_iter().any(|index| {
            let triangle = collider_triangles[index];
            triangles_intersect(current[0], current[1], current[2], triangle)
                && !triangles_intersect(baseline[0], baseline[1], baseline[2], triangle)
        }) {
            for group in [ga, gb, gc] {
                if touched[group] {
                    rollback.insert(group);
                }
            }
        }
    }
    Ok(rollback)
}

fn nearest_surface_point(
    point: Vec3,
    candidates: &[usize],
    triangles: &[Triangle],
    clearance: f32,
    reach: f32,
) -> Option<(Vec3, f32, Vec3)> {
    candidates
        .iter()
        .copied()
        .filter_map(|index| {
            let triangle = triangles[index];
            let closest = triangle.closest(point);
            let distance_squared = point.distance_squared(closest);
            (distance_squared <= (reach + clearance).powi(2)).then_some((
                triangle.outward,
                (point - closest).dot(triangle.outward),
                closest,
                distance_squared,
            ))
        })
        .min_by(|left, right| left.3.total_cmp(&right.3))
        .map(|(normal, signed, closest, _)| (normal, signed, closest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_character::SkinWeightSourceTopology;
    use serde_json::json;
    use std::sync::Arc;

    fn part(
        name: &str,
        positions: Vec<[f32; 3]>,
        indices: Vec<u32>,
        material: serde_json::Value,
    ) -> NativePart {
        let count = positions.len();
        NativePart {
            name: name.into(),
            positions,
            indices,
            uvs: vec![[0.; 2]; count],
            refs: vec![[0; 3]; count],
            ref_weights: vec![[1., 0., 0.]; count],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material,
        }
    }
    fn body() -> NativePart {
        part(
            "Body",
            vec![[-2., -2., 0.], [2., -2., 0.], [0., 2., 0.]],
            vec![0, 1, 2],
            json!({}),
        )
    }
    fn cloth(points: Vec<[f32; 3]>) -> NativePart {
        part(
            "custom:top",
            points,
            vec![0, 1, 2],
            json!({"fitClearanceMm": 3.0}),
        )
    }

    fn coherent_cloth(current: Vec<[f32; 3]>, source: Vec<[f32; 3]>) -> NativePart {
        let mut part = cloth(current);
        part.material["fitClearanceCoherentGridDM"] = json!(0.1);
        part.skin_weight_source_topology = Some(Arc::new(SkinWeightSourceTopology {
            positions: source,
            indices: part.indices.clone(),
        }));
        part
    }

    fn layer_cloth(name: &str, points: Vec<[f32; 3]>, clearance: f32) -> NativePart {
        part(
            name,
            points,
            vec![0, 1, 2],
            json!({"fitLayerClearanceMm": clearance}),
        )
    }

    #[test]
    fn coherent_clearance_keeps_default_clearance_parity_when_absent() {
        let points = vec![[0., 0., -0.02], [0.1, 0., -0.02], [0., 0.1, -0.02]];
        let mut legacy = cloth(points.clone());
        let mut absent = cloth(points);
        absent.skin_weight_source_topology = Some(Arc::new(SkinWeightSourceTopology {
            positions: absent.positions.clone(),
            indices: absent.indices.clone(),
        }));
        let legacy_stats = fit_custom_clothing_clearance(&mut legacy, &body()).unwrap();
        let absent_stats = fit_custom_clothing_clearance(&mut absent, &body()).unwrap();
        assert_eq!(absent.positions, legacy.positions);
        assert_eq!(absent_stats, legacy_stats);
    }

    #[test]
    fn coherent_clearance_shares_authored_alias_delta_after_divergent_transport() {
        let current = vec![[0., 0., -0.03], [0., 0., -0.01], [0.1, 0., -0.02]];
        let source = vec![[0., 0., 0.], [0., 0., 0.], [0.1, 0., 0.]];
        let before = current.clone();
        let mut part = coherent_cloth(current, source);
        fit_custom_clothing_clearance(&mut part, &body()).unwrap();
        let a = Vec3::from(part.positions[0]) - Vec3::from(before[0]);
        let b = Vec3::from(part.positions[1]) - Vec3::from(before[1]);
        assert!((a - b).length() < 1e-6);
    }

    #[test]
    fn coherent_clearance_moves_a_crossing_patch_without_tearing_its_short_edge() {
        let current = vec![[0., 0., -0.02], [0.008, 0., -0.02], [0., 0.1, -0.02]];
        let before = current.clone();
        let mut part = coherent_cloth(current.clone(), current);
        let stats = fit_custom_clothing_clearance(&mut part, &body())
            .unwrap()
            .unwrap();
        let before_edge = Vec3::from(before[0]).distance(Vec3::from(before[1]));
        let after_edge = Vec3::from(part.positions[0]).distance(Vec3::from(part.positions[1]));
        assert!(stats.corrected_vertices > 0);
        assert!(part.positions.iter().all(|p| Vec3::from(*p).is_finite()));
        assert!((after_edge - before_edge).abs() < 0.002);
        assert!(part.positions.iter().all(|p| p[2] > before[0][2]));
    }

    #[test]
    fn coherent_clearance_keeps_clear_patches_when_shared_node_centroid_is_inside_body() {
        let h = 0.1;
        let cube = part(
            "Body",
            vec![
                [-h, -h, -h],
                [h, -h, -h],
                [h, h, -h],
                [-h, h, -h],
                [-h, -h, h],
                [h, -h, h],
                [h, h, h],
                [-h, h, h],
            ],
            vec![
                0, 3, 2, 0, 2, 1, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0,
                7, 3, 1, 2, 6, 1, 6, 5,
            ],
            json!({}),
        );
        // Two disconnected cloth patches are each 5 mm outside the body.
        // Their shared coarse-grid controls average inside the body, but
        // those averages are not points on either cloth surface.
        let points = vec![
            [-0.15, -0.02, -0.02],
            [-0.15, 0.02, -0.02],
            [-0.15, 0., 0.02],
            [0.15, -0.02, -0.02],
            [0.15, 0., 0.02],
            [0.15, 0.02, -0.02],
        ];
        let mut garment = part(
            "custom:top",
            points.clone(),
            vec![0, 1, 2, 3, 4, 5],
            json!({"fitClearanceMm":3., "fitClearanceCoherentGridDM":0.5}),
        );
        garment.skin_weight_source_topology = Some(Arc::new(SkinWeightSourceTopology {
            positions: points.clone(),
            indices: garment.indices.clone(),
        }));
        let stats = fit_custom_clothing_clearance(&mut garment, &cube)
            .unwrap()
            .unwrap();
        assert_eq!(garment.positions, points);
        assert_eq!(stats.corrected_vertices, 0);
        assert_eq!(stats.remaining_violations, 0);
    }

    #[test]
    fn coherent_clearance_rejects_invalid_or_overbudget_source_field() {
        let mut invalid = cloth(vec![[0., 0., -0.02], [0.1, 0., -0.02], [0., 0.1, -0.02]]);
        invalid.material["fitClearanceCoherentGridDM"] = json!(0.09);
        assert!(fit_custom_clothing_clearance(&mut invalid, &body()).is_err());

        let count = MAX_COHERENT_CLEARANCE_NODES + 1;
        let current = vec![[0., 0., -0.02]; count];
        let source = (0..count)
            .map(|i| {
                let x = (i % 100) as f32 * 0.2;
                let y = ((i / 100) % 100) as f32 * 0.2;
                let z = (i / 10_000) as f32 * 0.2;
                [x, y, z]
            })
            .collect::<Vec<_>>();
        let mut overbudget = coherent_cloth(current, source);
        assert!(fit_custom_clothing_clearance(&mut overbudget, &body()).is_err());
    }

    #[test]
    fn coherent_clearance_honours_the_configured_total_cap_without_mutating_bad_inputs() {
        let default_current = vec![[0., 0., -0.39], [0.1, 0., -0.39], [0., 0.1, -0.39]];
        let mut default_capped = coherent_cloth(default_current.clone(), default_current.clone());
        let default_stats = fit_custom_clothing_clearance(&mut default_capped, &body())
            .unwrap()
            .unwrap();
        assert!(default_stats.max_displacement <= 0.4 + 1e-6);
        assert!(
            default_capped
                .positions
                .iter()
                .zip(&default_current)
                .all(
                    |(&after, &before)| Vec3::from(after).distance(Vec3::from(before))
                        <= 0.4 + 1e-6
                )
        );
        assert!(default_stats.remaining_violations > 0);

        let current = vec![[0., 0., -0.64], [0.1, 0., -0.64], [0., 0.1, -0.64]];
        let mut capped = coherent_cloth(current.clone(), current.clone());
        capped.material["fitMaxCorrectionMm"] = json!(65.0);
        let stats = fit_custom_clothing_clearance(&mut capped, &body())
            .unwrap()
            .unwrap();
        assert!(stats.max_displacement <= 0.65 + 1e-6);
        assert!(
            capped
                .positions
                .iter()
                .zip(&current)
                .all(
                    |(&after, &before)| Vec3::from(after).distance(Vec3::from(before))
                        <= 0.65 + 1e-6
                )
        );
        assert!(stats.remaining_violations > 0);

        let mut malformed = coherent_cloth(current.clone(), current);
        malformed.refs.pop();
        let before = malformed.positions.clone();
        assert!(fit_custom_clothing_clearance(&mut malformed, &body()).is_err());
        assert_eq!(malformed.positions, before);

        let mut bad_source = coherent_cloth(
            vec![[0., 0., -0.02], [0.1, 0., -0.02], [0., 0.1, -0.02]],
            vec![[0., 0., 0.], [0.1, 0., 0.], [0., 0.1, 0.]],
        );
        Arc::make_mut(bad_source.skin_weight_source_topology.as_mut().unwrap()).indices =
            vec![9, 1, 2];
        let before = bad_source.positions.clone();
        assert!(fit_custom_clothing_clearance(&mut bad_source, &body()).is_err());
        assert_eq!(bad_source.positions, before);
    }

    #[test]
    fn clearance_exclusions_protect_welded_cuffs_from_adjacent_projection_and_contacts() {
        let points = vec![
            [0., 0., -0.02],
            [0.1, 0., -0.02],
            [0., 0.1, -0.02],
            [0., 0., -0.02],
        ];
        let mut garment = cloth(points.clone());
        garment.indices.extend([3, 1, 2]);
        garment.material["fitClearanceExcludedVertices"] = json!([0, 3]);
        garment.material["fitTriangleClearance"] = json!(true);
        fit_custom_clothing_clearance(&mut garment, &body()).unwrap();
        assert_eq!(garment.positions[0], points[0]);
        assert_eq!(garment.positions[3], points[3]);
        assert!(garment.positions[1][2] > 0.0);
        assert!(garment.positions[2][2] > 0.0);
    }

    #[test]
    fn clearance_exclusions_validate_dependencies_indices_and_complete_welds_atomically() {
        let points = vec![[0., 0., -0.02], [0.1, 0., -0.02], [0., 0.1, -0.02]];
        for bad in [
            json!(true),
            json!([-1]),
            json!([0.5]),
            json!([0, 0]),
            json!([3]),
        ] {
            let mut garment = cloth(points.clone());
            garment.material["fitClearanceExcludedVertices"] = bad;
            assert!(fit_custom_clothing_clearance(&mut garment, &body()).is_err());
            assert_eq!(garment.positions, points);
        }
        let mut missing = cloth(points.clone());
        missing
            .material
            .as_object_mut()
            .unwrap()
            .remove("fitClearanceMm");
        missing.material["fitClearanceExcludedVertices"] = json!([0]);
        assert!(fit_custom_clothing_clearance(&mut missing, &body()).is_err());
        assert_eq!(missing.positions, points);

        let mut partial = cloth(vec![points[0], points[1], points[2], points[0]]);
        partial.material["fitClearanceExcludedVertices"] = json!([0]);
        let before = partial.positions.clone();
        assert!(fit_custom_clothing_clearance(&mut partial, &body()).is_err());
        assert_eq!(partial.positions, before);
    }

    #[test]
    fn clearance_exclusions_keep_empty_mask_parity_and_override_covered_tuck() {
        let mut legacy = cloth(vec![[0., 0., -0.02], [0.1, 0., -0.02], [0., 0.1, -0.02]]);
        let mut empty = legacy.clone();
        empty.material["fitClearanceExcludedVertices"] = json!([]);
        fit_custom_clothing_clearance(&mut legacy, &body()).unwrap();
        fit_custom_clothing_clearance(&mut empty, &body()).unwrap();
        assert_eq!(empty.positions, legacy.positions);

        let points = vec![
            [0., 0., 0.1],
            [0.1, 0., 0.1],
            [0., 0.1, 0.1],
            [0., 0.9, 0.6],
            [0.1, 1., 0.6],
            [0., 1., 0.6],
        ];
        let mut protected = cloth(points.clone());
        protected.indices.extend([3, 4, 5]);
        protected.material["fitClearanceMm"] = json!(10.0);
        protected.material["fitClearanceUpperBandMm"] = json!([0.6, 0.8, 5.0]);
        protected.material["fitClearanceUpperBandCoveredOnly"] = json!(true);
        protected.material["fitClearanceUpperBandCoveredTuckMm"] = json!(40.0);
        protected.material["fitClearanceExcludedVertices"] = json!([3, 4, 5]);
        fit_custom_clothing_clearance_with_coverage(&mut protected, &body(), Some(&[true; 6]))
            .unwrap();
        assert_eq!(protected.positions, points);
    }

    #[test]
    fn layer_clearance_moves_outer_weld_groups_beyond_lower_surface() {
        let lower = part(
            "custom:bottom",
            vec![[-1., -1., 0.1], [1., -1., 0.1], [0., 1., 0.1]],
            vec![0, 1, 2],
            json!({}),
        );
        let mut outer = layer_cloth(
            "custom:top",
            vec![
                [0., 0., 0.05],
                [0.1, 0., 0.05],
                [0., 0.1, 0.05],
                [0.1, 0., 0.05],
            ],
            3.0,
        );
        outer.indices.extend([0, 3, 2]);
        let source = outer.positions.clone();
        let stats = fit_custom_layer_clearance(&mut outer, &[&lower], &body(), &source)
            .unwrap()
            .unwrap();
        assert_eq!(stats.corrected_vertices, 4);
        assert!(outer.positions.iter().all(|point| point[2] >= 0.13 - 1e-5));
        assert_eq!(outer.positions[1], outer.positions[3]);
    }

    #[test]
    fn layer_clearance_leaves_already_outer_geometry_unchanged() {
        let lower = part(
            "custom:bottom",
            vec![[-1., -1., 0.1], [1., -1., 0.1], [0., 1., 0.1]],
            vec![0, 1, 2],
            json!({}),
        );
        let mut outer = layer_cloth(
            "custom:top",
            vec![[0., 0., 0.2], [0.1, 0., 0.2], [0., 0.1, 0.2]],
            3.0,
        );
        let source = outer.positions.clone();
        let stats = fit_custom_layer_clearance(&mut outer, &[&lower], &body(), &source)
            .unwrap()
            .unwrap();
        assert_eq!(stats.corrected_vertices, 0);
        assert_eq!(outer.positions, source);
    }

    #[test]
    fn layer_clearance_caps_combined_source_displacement_and_is_atomic_on_metadata_error() {
        let lower = part(
            "custom:bottom",
            vec![[-1., -1., 0.1], [1., -1., 0.1], [0., 1., 0.1]],
            vec![0, 1, 2],
            json!({}),
        );
        let source = vec![[0., 0., -0.3], [0.1, 0., -0.3], [0., 0.1, -0.3]];
        let mut outer = layer_cloth(
            "custom:top",
            vec![[0., 0., 0.05], [0.1, 0., 0.05], [0., 0.1, 0.05]],
            3.0,
        );
        let stats = fit_custom_layer_clearance(&mut outer, &[&lower], &body(), &source)
            .unwrap()
            .unwrap();
        assert_eq!(stats.remaining_violations, 3);
        assert!(outer.positions.iter().zip(&source).all(|(after, before)| {
            Vec3::from(*after).distance(Vec3::from(*before)) <= MAX_CORRECTION + 1e-6
        }));
        let mut invalid = layer_cloth(
            "custom:top",
            vec![[0., 0., 0.05], [0.1, 0., 0.05], [0., 0.1, 0.05]],
            3.0,
        );
        invalid.material["fitLayerClearanceMm"] = json!(11.0);
        let before = invalid.positions.clone();
        assert!(fit_custom_layer_clearance(&mut invalid, &[&lower], &body(), &before).is_err());
        assert_eq!(invalid.positions, before);
    }

    #[test]
    fn layer_ray_budget_preserves_existing_nonparallel_body_correction() {
        let lower = part(
            "custom:bottom",
            vec![[-1., -1., 0.3], [1., -1., 0.3], [0., 1., 0.3]],
            vec![0, 1, 2],
            json!({}),
        );
        let mut outer = layer_cloth(
            "custom:top",
            vec![[0., 0., 0.05], [0.1, 0., 0.05], [0., 0.1, 0.05]],
            3.,
        );
        let before = outer.positions.clone();
        let source: Vec<[f32; 3]> = before.iter().map(|p| [p[0] - 0.39, p[1], p[2]]).collect();
        fit_custom_layer_clearance(&mut outer, &[&lower], &body(), &source).unwrap();
        for ((after, current), original) in outer.positions.iter().zip(before).zip(source) {
            assert_eq!(after[0], current[0]);
            assert_eq!(after[1], current[1]);
            assert!(after[2] > current[2]);
            assert!(Vec3::from(*after).distance(Vec3::from(original)) <= MAX_CORRECTION + 1e-6);
        }
    }

    #[test]
    fn invalid_lower_part_cannot_alias_the_next_parts_vertices() {
        let first = part(
            "custom:bad-bottom",
            vec![[-1., -1., 0.1], [1., -1., 0.1], [0., 1., 0.1]],
            vec![0, 1, 3],
            json!({}),
        );
        let next = part(
            "custom:feet",
            vec![[0., 1., 0.1], [1., 1., 0.1], [0., 2., 0.1]],
            vec![0, 1, 2],
            json!({}),
        );
        let mut outer = layer_cloth(
            "custom:top",
            vec![[0., 0., 0.05], [0.1, 0., 0.05], [0., 0.1, 0.05]],
            3.,
        );
        let before = outer.positions.clone();
        assert!(
            fit_custom_layer_clearance(&mut outer, &[&first, &next], &body(), &before).is_err()
        );
        assert_eq!(outer.positions, before);
    }

    #[test]
    fn clips_against_a_plane_with_coherent_neighbour_displacements() {
        let mut garment = cloth(vec![[0., 0., -0.01], [0.1, 0., -0.01], [0., 0.1, -0.01]]);
        let before = garment.positions.clone();
        let stats = fit_custom_clothing_clearance(&mut garment, &body())
            .unwrap()
            .unwrap();
        assert_eq!(stats.remaining_violations, 0);
        assert!(
            garment
                .positions
                .iter()
                .all(|point| point[2] >= 0.03 - 1e-4)
        );
        let deltas: Vec<_> = garment
            .positions
            .iter()
            .zip(before)
            .map(|(after, before)| Vec3::from(*after) - Vec3::from(before))
            .collect();
        assert!((deltas[0] - deltas[1]).length() < 0.02);
    }

    #[test]
    fn final_projection_restores_contact_lost_to_last_smoothing_pass() {
        // Only the first vertex begins inside the plane. Its two already-clear
        // neighbours pull it inward during every smoothing pass, including the
        // fourth; the final bounded projection must restore requested clearance.
        let mut garment = cloth(vec![[0., 0., -0.01], [0.1, 0., 0.1], [0., 0.1, 0.1]]);
        let before = garment.positions.clone();
        let stats = fit_custom_clothing_clearance(&mut garment, &body())
            .unwrap()
            .unwrap();
        assert!(garment.positions[0][2] >= 0.03 - 1e-5);
        assert_eq!(stats.remaining_violations, 0);
        assert!(garment.positions.iter().zip(before).all(|(after, before)| {
            (Vec3::from(*after) - Vec3::from(before)).length() <= MAX_CORRECTION + 1e-6
        }));
    }

    #[test]
    fn exterior_guard_preserves_clear_neighbours_and_still_resolves_contacts() {
        let points = vec![[0., 0., -0.01], [0.1, 0., 0.1], [0., 0.1, 0.1]];
        let mut legacy = cloth(points.clone());
        fit_custom_clothing_clearance(&mut legacy, &body()).unwrap();
        assert!(legacy.positions[1][2] > points[1][2]);
        let mut guarded = cloth(points.clone());
        guarded.material["fitClearanceSmoothingGuardMm"] = json!(3.0);
        let stats = fit_custom_clothing_clearance(&mut guarded, &body())
            .unwrap()
            .unwrap();
        assert_eq!(guarded.positions[1..], points[1..]);
        assert!(guarded.positions[0][2] >= 0.03 - 1e-5);
        assert_eq!(stats.remaining_violations, 0);

        // The guard follows the current morphed body, rather than freezing a
        // source-space vertex that a wider body now needs to push outward.
        let mut wider_body = body();
        for point in &mut wider_body.positions {
            point[2] = 0.12;
        }
        let mut remorphed = cloth(points.clone());
        remorphed.material["fitClearanceSmoothingGuardMm"] = json!(3.0);
        let stats = fit_custom_clothing_clearance(&mut remorphed, &wider_body)
            .unwrap()
            .unwrap();
        assert!(remorphed.positions.iter().all(|p| p[2] >= 0.15 - 1e-5));
        assert_eq!(stats.corrected_vertices, 3);
    }

    #[test]
    fn exterior_guard_rejects_invalid_values_and_keeps_uv_copies_coherent() {
        for value in [
            json!(0),
            json!(-1),
            json!(11),
            json!(1e-300),
            json!("3"),
            json!(null),
        ] {
            let mut garment = cloth(vec![[0., 0., -0.01], [0.1, 0., 0.1], [0., 0.1, 0.1]]);
            garment.material["fitClearanceSmoothingGuardMm"] = value;
            let before = garment.positions.clone();
            assert!(fit_custom_clothing_clearance(&mut garment, &body()).is_err());
            assert_eq!(garment.positions, before);
        }
        let mut garment = cloth(vec![
            [0., 0., -0.01],
            [0.1, 0., 0.1],
            [0., 0.1, 0.1],
            [0.1, 0., 0.1],
        ]);
        garment.indices.extend([0, 3, 2]);
        garment.material["fitClearanceSmoothingGuardMm"] = json!(3.0);
        fit_custom_clothing_clearance(&mut garment, &body()).unwrap();
        assert_eq!(garment.positions[1], garment.positions[3]);
    }

    #[test]
    fn upper_band_keeps_separate_lower_leg_clearance_exact() {
        let points = vec![
            [0., 0., -0.01],
            [0.1, 0.1, 0.1],
            [0., 0.1, 0.1],
            [0., 0.9, -0.01],
            [0.1, 1., 0.1],
            [0., 1., 0.1],
        ];
        let mut legacy = cloth(points.clone());
        legacy.indices.extend([3, 4, 5]);
        let mut guarded = legacy.clone();
        guarded.material["fitClearanceSmoothingGuardMm"] = json!(3.0);
        guarded.material["fitClearanceSmoothingGuardUpperBand"] = json!([0.6, 0.8]);
        fit_custom_clothing_clearance(&mut legacy, &body()).unwrap();
        fit_custom_clothing_clearance(&mut guarded, &body()).unwrap();
        assert_eq!(guarded.positions[..3], legacy.positions[..3]);
        assert_eq!(guarded.positions[4..], points[4..]);
        assert!(guarded.positions[3][2] >= 0.03 - 1e-5);
    }

    #[test]
    fn upper_band_rejects_invalid_ranges_without_mutating_geometry() {
        for band in [
            json!([0.9, 0.8]),
            json!([0.8, 0.8]),
            json!([-1e-300, 0.8]),
            json!([0.8, 1.00000001]),
            json!([0.5]),
            json!("upper"),
        ] {
            let mut garment = cloth(vec![[0., 0., -0.01], [0.1, 0., 0.1], [0., 0.1, 0.1]]);
            garment.material["fitClearanceSmoothingGuardMm"] = json!(3.0);
            garment.material["fitClearanceSmoothingGuardUpperBand"] = band;
            let before = garment.positions.clone();
            assert!(fit_custom_clothing_clearance(&mut garment, &body()).is_err());
            assert_eq!(garment.positions, before);
        }
        let mut garment = cloth(vec![[0., 0., -0.01], [0.1, 0., 0.1], [0., 0.1, 0.1]]);
        garment.material["fitClearanceSmoothingGuardUpperBand"] = json!([0.8, 0.9]);
        assert!(fit_custom_clothing_clearance(&mut garment, &body()).is_err());
    }

    #[test]
    fn upper_clearance_band_reduces_waist_padding_without_changing_lower_legs() {
        let points = vec![
            [0., 0., -0.01],
            [0.1, 0.1, -0.01],
            [0., 0.1, -0.01],
            [0., 0.9, -0.01],
            [0.1, 1., -0.01],
            [0., 1., -0.01],
        ];
        let mut legacy = cloth(points.clone());
        legacy.indices.extend([3, 4, 5]);
        legacy.material["fitClearanceMm"] = json!(10.0);
        let mut tapered = legacy.clone();
        tapered.material["fitClearanceUpperBandMm"] = json!([0.6, 0.8, 2.0]);
        fit_custom_clothing_clearance(&mut legacy, &body()).unwrap();
        fit_custom_clothing_clearance(&mut tapered, &body()).unwrap();
        assert_eq!(tapered.positions[..3], legacy.positions[..3]);
        assert!(
            tapered.positions[..3]
                .iter()
                .all(|p| (p[2] - 0.1).abs() < 1e-5)
        );
        assert!(
            tapered.positions[3..]
                .iter()
                .all(|p| (p[2] - 0.02).abs() < 1e-5)
        );
        assert!(
            tapered
                .positions
                .iter()
                .zip(&points)
                .all(|(p, before)| Vec3::from(*p).distance(Vec3::from(*before)) <= MAX_CORRECTION)
        );
    }

    #[test]
    fn upper_clearance_band_rejects_invalid_and_excessive_padding() {
        for value in [
            json!([0.8, 0.6, 2]),
            json!([0.8, 0.8, 2]),
            json!([0.6, 1.00000001, 2]),
            json!([0.6, 0.8, 0]),
            json!([0.6, 0.8, 1e-300]),
            json!([0.6, 0.8, 4]),
            json!([0.6, 0.8]),
            json!("2"),
        ] {
            let mut garment = cloth(vec![[0., 0., -0.01], [0.1, 0., 0.1], [0., 0.1, 0.1]]);
            garment.material["fitClearanceUpperBandMm"] = value;
            let before = garment.positions.clone();
            assert!(fit_custom_clothing_clearance(&mut garment, &body()).is_err());
            assert_eq!(garment.positions, before);
        }
    }

    #[test]
    fn covered_only_upper_band_preserves_absent_false_and_uncovered_clearance() {
        let points = vec![
            [0., 0., -0.01],
            [0.1, 0.1, -0.01],
            [0., 0.1, -0.01],
            [0., 0.9, -0.01],
            [0.1, 1., -0.01],
            [0., 1., -0.01],
        ];
        let mut legacy = cloth(points.clone());
        legacy.indices.extend([3, 4, 5]);
        legacy.material["fitClearanceMm"] = json!(10.0);
        let mut ordinary_band = legacy.clone();
        ordinary_band.material["fitClearanceUpperBandMm"] = json!([0.6, 0.8, 2.0]);
        let mut explicit_false = ordinary_band.clone();
        explicit_false.material["fitClearanceUpperBandCoveredOnly"] = json!(false);
        fit_custom_clothing_clearance(&mut ordinary_band, &body()).unwrap();
        fit_custom_clothing_clearance(&mut explicit_false, &body()).unwrap();
        assert_eq!(explicit_false.positions, ordinary_band.positions);

        fit_custom_clothing_clearance(&mut legacy, &body()).unwrap();
        let mut covered_only = explicit_false.clone();
        covered_only.positions = points.clone();
        covered_only.material["fitClearanceUpperBandCoveredOnly"] = json!(true);
        fit_custom_clothing_clearance_with_coverage(
            &mut covered_only,
            &body(),
            Some(&[false, false, false, false, false, false]),
        )
        .unwrap();
        assert_eq!(covered_only.positions, legacy.positions);
    }

    #[test]
    fn covered_only_upper_band_reduces_only_covered_vertices_and_triangle_contacts() {
        let points = vec![
            [0., 0., -0.01],
            [0.1, 0.1, -0.01],
            [0., 0.1, -0.01],
            [0., 0.9, -0.01],
            [0.1, 1., -0.01],
            [0., 1., -0.01],
        ];
        let mut garment = cloth(points.clone());
        garment.indices.extend([3, 4, 5]);
        garment.material["fitClearanceMm"] = json!(10.0);
        garment.material["fitClearanceUpperBandMm"] = json!([0.6, 0.8, 2.0]);
        garment.material["fitClearanceUpperBandCoveredOnly"] = json!(true);
        garment.material["fitTriangleClearance"] = json!(true);
        fit_custom_clothing_clearance_with_coverage(
            &mut garment,
            &body(),
            Some(&[false, false, false, true, true, true]),
        )
        .unwrap();
        assert!(
            garment.positions[..3]
                .iter()
                .all(|point| (point[2] - 0.1).abs() < 1e-5)
        );
        assert!(
            garment.positions[3..]
                .iter()
                .all(|point| (point[2] - 0.02).abs() < 1e-5)
        );
    }

    #[test]
    fn covered_only_upper_band_rejects_bad_flag_dependency_or_mask_without_mutating() {
        let mut garment = cloth(vec![[0., 0., -0.01], [0.1, 0., -0.01], [0., 0.1, -0.01]]);
        garment.material["fitClearanceMm"] = json!(10.0);
        garment.material["fitClearanceUpperBandMm"] = json!([0.6, 0.8, 2.0]);
        garment.material["fitClearanceUpperBandCoveredOnly"] = json!("true");
        let before = garment.positions.clone();
        assert!(
            fit_custom_clothing_clearance_with_coverage(&mut garment, &body(), Some(&[true; 3]))
                .is_err()
        );
        assert_eq!(garment.positions, before);

        garment.material["fitClearanceUpperBandCoveredOnly"] = json!(true);
        garment
            .material
            .as_object_mut()
            .unwrap()
            .remove("fitClearanceUpperBandMm");
        assert!(
            fit_custom_clothing_clearance_with_coverage(&mut garment, &body(), Some(&[true; 3]))
                .is_err()
        );
        assert_eq!(garment.positions, before);

        garment.material["fitClearanceUpperBandMm"] = json!([0.6, 0.8, 2.0]);
        assert!(
            fit_custom_clothing_clearance_with_coverage(&mut garment, &body(), Some(&[true; 2]))
                .is_err()
        );
        assert_eq!(garment.positions, before);
    }

    #[test]
    fn covered_upper_tuck_requires_covered_upper_band_dependencies_without_mutating() {
        for value in [
            json!(0),
            json!(-1),
            json!(40.0001),
            json!(1e-300),
            json!("20"),
            json!(null),
        ] {
            let mut garment = cloth(vec![[0., 0., 0.1], [0.1, 0., 0.1], [0., 1., 0.1]]);
            garment.material["fitClearanceUpperBandCoveredTuckMm"] = value;
            let before = garment.positions.clone();
            assert!(fit_custom_clothing_clearance(&mut garment, &body()).is_err());
            assert_eq!(garment.positions, before);
        }

        // Parsing precedes the legacy no-clearance early return, so a typo is
        // never silently ignored by a non-fitting imported part.
        let mut no_clearance = cloth(vec![[0., 0., 0.1], [0.1, 0., 0.1], [0., 1., 0.1]]);
        no_clearance.material = json!({"fitClearanceUpperBandCoveredTuckMm": "20"});
        let before = no_clearance.positions.clone();
        assert!(fit_custom_clothing_clearance(&mut no_clearance, &body()).is_err());
        assert_eq!(no_clearance.positions, before);

        let mut missing_band = cloth(vec![[0., 0., 0.1], [0.1, 0., 0.1], [0., 1., 0.1]]);
        missing_band.material["fitClearanceUpperBandCoveredOnly"] = json!(true);
        missing_band.material["fitClearanceUpperBandCoveredTuckMm"] = json!(20.0);
        let before = missing_band.positions.clone();
        assert!(
            fit_custom_clothing_clearance_with_coverage(
                &mut missing_band,
                &body(),
                Some(&[true; 3]),
            )
            .is_err()
        );
        assert_eq!(missing_band.positions, before);
    }

    #[test]
    fn covered_upper_tuck_respects_floor_budget_and_covered_welds() {
        // The high triangle begins 4.49 cm clear of the visible body. A 4 cm
        // tuck must stop on the 5 mm upper-band floor, while the low triangle
        // stays byte-for-byte at its source position.
        let points = vec![
            [0., 0., 0.1],
            [0.1, 0., 0.1],
            [0., 0.1, 0.1],
            [0., 0.9, 0.449],
            [0.1, 1., 0.449],
            [0., 1., 0.449],
        ];
        let mut garment = cloth(points.clone());
        garment.indices.extend([3, 4, 5]);
        garment.material["fitClearanceMm"] = json!(10.0);
        garment.material["fitClearanceUpperBandMm"] = json!([0.6, 0.8, 5.0]);
        garment.material["fitClearanceUpperBandCoveredOnly"] = json!(true);
        garment.material["fitClearanceUpperBandCoveredTuckMm"] = json!(40.0);
        fit_custom_clothing_clearance_with_coverage(
            &mut garment,
            &body(),
            Some(&[false, false, false, true, true, true]),
        )
        .unwrap();
        assert_eq!(garment.positions[..3], points[..3]);
        assert!(
            garment.positions[3..]
                .iter()
                .all(|point| (point[2] - 0.05).abs() < 1e-5)
        );
        assert!(
            garment
                .positions
                .iter()
                .zip(&points)
                .all(|(after, before)| {
                    (Vec3::from(*after) - Vec3::from(*before)).length() <= MAX_CORRECTION + 1e-6
                })
        );

        // Without a higher opaque layer, the same opt-in is coordinate-exact
        // with the authored pants: it never turns into unconditional shrink.
        let mut uncovered = cloth(points.clone());
        uncovered.indices.extend([3, 4, 5]);
        uncovered.material = garment.material.clone();
        fit_custom_clothing_clearance_with_coverage(
            &mut uncovered,
            &body(),
            Some(&[false, false, false, false, false, false]),
        )
        .unwrap();
        assert_eq!(uncovered.positions, points);

        // Discovery intentionally reaches beyond the 4 cm source-relative
        // movement cap: a 6 cm authored wall still tucks by exactly 4 cm.
        let mut far = cloth(vec![
            [0., 0., 0.1],
            [0.1, 0., 0.1],
            [0., 0.1, 0.1],
            [0., 0.9, 0.6],
            [0.1, 1., 0.6],
            [0., 1., 0.6],
        ]);
        far.indices.extend([3, 4, 5]);
        far.material["fitClearanceMm"] = json!(10.0);
        far.material["fitClearanceUpperBandMm"] = json!([0.6, 0.8, 5.0]);
        far.material["fitClearanceUpperBandCoveredOnly"] = json!(true);
        far.material["fitClearanceUpperBandCoveredTuckMm"] = json!(40.0);
        fit_custom_clothing_clearance_with_coverage(
            &mut far,
            &body(),
            Some(&[false, false, false, true, true, true]),
        )
        .unwrap();
        assert!(
            far.positions[3..]
                .iter()
                .all(|point| (point[2] - 0.2).abs() < 1e-5)
        );

        // A UV-welded source vertex must not tuck when one of its canonical
        // anchors remains visible. The two duplicate corners stay coherent at
        // the base floor rather than using the first corner's coverage as a
        // proxy for a reduced upper-band floor.
        let mut mixed_weld = cloth(vec![
            [0., 0., 0.1],
            [0.1, 0., 0.1],
            [0., 0.1, 0.1],
            [0., 0.9, 0.449],
            [0.1, 1., 0.01],
            [0., 1., 0.449],
            [0.1, 1., 0.01],
        ]);
        mixed_weld.indices.extend([3, 4, 5, 3, 6, 5]);
        mixed_weld.material["fitClearanceMm"] = json!(10.0);
        mixed_weld.material["fitClearanceUpperBandMm"] = json!([0.6, 0.8, 5.0]);
        mixed_weld.material["fitClearanceUpperBandCoveredOnly"] = json!(true);
        mixed_weld.material["fitClearanceUpperBandCoveredTuckMm"] = json!(40.0);
        fit_custom_clothing_clearance_with_coverage(
            &mut mixed_weld,
            &body(),
            Some(&[false, false, false, true, true, true, false]),
        )
        .unwrap();
        assert_eq!(mixed_weld.positions[4], mixed_weld.positions[6]);
        assert!((mixed_weld.positions[4][2] - 0.1).abs() < 1e-5);
    }

    #[test]
    fn physical_upper_tuck_moves_only_the_covered_upper_weld_and_keeps_group_state() {
        // Vertices 0 and 1 are an early low-band weld. Vertex 3 has an
        // unrelated canonical-fit displacement. The active weld first appears
        // at 4, but is group 3: indexing displacement by vertex would borrow
        // vertex 3's .02dm state and fail the exact .04dm tuck below.
        let source = vec![
            [0., 0., 0.1],
            [0., 0., 0.1],
            [0.1, 0., 0.1],
            [0., 0.1, 0.1],
            [0., 0.9, 0.35],
            [0.1, 1., 0.35],
            [0., 1., 0.35],
            [0., 0.9, 0.35],
        ];
        let mut garment = cloth(source.clone());
        garment.positions[3][2] += 0.02;
        garment.indices = vec![0, 1, 2, 4, 5, 6, 7, 5, 6];
        garment.material = json!({
            "fitClearanceMm": 10.0,
            "fitClearanceUpperBandMm": [0.6, 0.8, 5.0],
            "fitClearanceUpperBandCoveredOnly": true,
            "fitClearanceUpperBandCoveredTuckMm": 40.0,
            "fitClearanceUpperBandPhysicalCoverage": true,
        });
        let outer = part(
            "custom:top",
            vec![[-2., -2., 0.36], [2., -2., 0.36], [0., 2., 0.36]],
            vec![0, 1, 2],
            json!({}),
        );
        let before = garment.positions.clone();
        fit_custom_clothing_physical_upper_tuck(&mut garment, &body(), &[&outer], &source).unwrap();
        // Lower vertices preserve even their pre-existing canonical movement;
        // the active UV weld gets exactly its 4 cm authored cap and remains
        // coherent across duplicate source vertices.
        assert_eq!(garment.positions[..4], before[..4]);
        for vertex in [4, 5, 6, 7] {
            assert!((garment.positions[vertex][2] - 0.31).abs() < 1e-5);
        }
        assert_eq!(garment.positions[4], garment.positions[7]);

        let mut explicit_false = garment.clone();
        explicit_false.material["fitClearanceUpperBandPhysicalCoverage"] = json!(false);
        let before_false = explicit_false.positions.clone();
        assert_eq!(
            fit_custom_clothing_physical_upper_tuck(
                &mut explicit_false,
                &body(),
                &[&outer],
                &source,
            )
            .unwrap(),
            None
        );
        assert_eq!(explicit_false.positions, before_false);

        let mut no_outer = garment.clone();
        no_outer.positions = before.clone();
        fit_custom_clothing_physical_upper_tuck(&mut no_outer, &body(), &[], &source).unwrap();
        assert_eq!(no_outer.positions, before);

        let no_room = part(
            "custom:top",
            vec![[-2., -2., 0.09], [2., -2., 0.09], [0., 2., 0.09]],
            vec![0, 1, 2],
            json!({}),
        );
        let mut blocked = garment.clone();
        blocked.positions = before.clone();
        fit_custom_clothing_physical_upper_tuck(&mut blocked, &body(), &[&no_room], &source)
            .unwrap();
        assert_eq!(blocked.positions, before);

        let mut partial_weld = garment.clone();
        partial_weld.positions = before.clone();
        partial_weld.material["fitClearanceExcludedVertices"] = json!([4]);
        assert!(fit_custom_clothing_physical_upper_tuck(
            &mut partial_weld,
            &body(),
            &[&outer],
            &source,
        )
        .is_err());
        assert_eq!(partial_weld.positions, before);

        let mut over_budget = garment.clone();
        over_budget.positions = before.clone();
        over_budget.positions[4][2] = 0.76;
        over_budget.positions[7][2] = 0.76;
        let before_over_budget = over_budget.positions.clone();
        assert!(
            fit_custom_clothing_physical_upper_tuck(&mut over_budget, &body(), &[&outer], &source,)
                .is_err()
        );
        assert_eq!(over_budget.positions, before_over_budget);
    }

    #[test]
    fn physical_upper_tuck_can_restore_a_proven_under_floor_upper_weld() {
        let source = vec![[0., 0., 0.1], [0.1, 0., 0.1], [0., 1., 0.04]];
        let mut garment = cloth(source.clone());
        garment.material = json!({
            "fitClearanceMm": 10.0,
            "fitClearanceUpperBandMm": [0.6, 0.8, 5.0],
            "fitClearanceUpperBandCoveredOnly": true,
            "fitClearanceUpperBandCoveredTuckMm": 40.0,
            "fitClearanceUpperBandPhysicalCoverage": true,
        });
        let outer = part(
            "custom:top",
            vec![[-2., -2., 0.15], [2., -2., 0.15], [0., 2., 0.15]],
            vec![0, 1, 2],
            json!({}),
        );
        fit_custom_clothing_physical_upper_tuck(&mut garment, &body(), &[&outer], &source).unwrap();
        assert!((garment.positions[2][2] - 0.05).abs() < 1e-5);
        assert_eq!(garment.positions[..2], source[..2]);

        let no_room = part(
            "custom:top",
            vec![[-2., -2., 0.09], [2., -2., 0.09], [0., 2., 0.09]],
            vec![0, 1, 2],
            json!({}),
        );
        let mut blocked = cloth(source.clone());
        blocked.material = garment.material.clone();
        fit_custom_clothing_physical_upper_tuck(&mut blocked, &body(), &[&no_room], &source)
            .unwrap();
        assert_eq!(blocked.positions, source);
    }

    #[test]
    fn physical_upper_tuck_rejects_bad_dependency_atomically() {
        let source = vec![[0., 0., 0.1], [0.1, 0., 0.1], [0., 1., 0.1]];
        let mut garment = cloth(source.clone());
        garment.material["fitClearanceUpperBandPhysicalCoverage"] = json!(true);
        let before = garment.positions.clone();
        assert!(fit_custom_clothing_clearance(&mut garment, &body()).is_err());
        assert_eq!(garment.positions, before);
        garment.material["fitClearanceUpperBandPhysicalCoverage"] = json!("true");
        assert!(
            fit_custom_clothing_physical_upper_tuck(&mut garment, &body(), &[], &source).is_err()
        );
        assert_eq!(garment.positions, before);
    }

    #[test]
    fn physical_trace_vertex_parser_is_strict_and_bounded() {
        assert_eq!(
            parse_physical_trace_vertices("7, 2,19", 20).unwrap(),
            vec![2, 7, 19]
        );
        for input in ["", "2,2", "20", "wat"] {
            assert!(parse_physical_trace_vertices(input, 20).is_err(), "{input}");
        }
        let too_many = (0..=MAX_PHYSICAL_TRACE_VERTICES)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_physical_trace_vertices(&too_many, MAX_PHYSICAL_TRACE_VERTICES + 1).is_err());
        assert!(
            parse_physical_trace_vertices(&"1".repeat(MAX_PHYSICAL_TRACE_CHARS + 1), 2).is_err()
        );
    }

    #[test]
    fn physical_tuck_zero_margin_grid_keeps_the_full_query_reach() {
        let far_body = part(
            "Body",
            vec![[-2., -2., 0.499], [2., -2., 0.499], [0., 2., 0.499]],
            vec![0, 1, 2],
            json!({}),
        );
        let (triangles, grid) = collider_with_margin(
            &far_body,
            GridBudget {
                cells: MAX_GRID_CELLS,
                references: MAX_GRID_REFERENCES,
            },
            0.0,
        )
        .unwrap();
        // 0.85 dm is the nearest-surface reach (.8 dm) plus a 5 mm floor;
        // it crosses two .5 dm cells but must remain discoverable without
        // storing the same body triangle in every nearby query cell.
        let point = Vec3::new(0., 0., 1.3489);
        let mut candidates_seen = 0;
        let candidates = physical_tuck_candidates(&grid, point, &mut candidates_seen).unwrap();
        assert_eq!(candidates, vec![0]);
        assert!(
            nearest_surface_point(point, &candidates, &triangles, 0.05, TUCK_QUERY_REACH).is_some()
        );
    }

    #[test]
    fn physical_tuck_ray_grid_keeps_endpoint_hits_and_rejects_unrelated_cells() {
        for normal in [Vec3::Z, -Vec3::Z, Vec3::new(1., 1., 1.).normalize()] {
            let origin = Vec3::new(0.49999, -0.00001, 0.49999);
            let endpoint = origin + normal * (TUCK_QUERY_REACH + CLEARANCE_FLOOR_EPSILON);
            let mut grid = BTreeMap::new();
            grid.insert(cell(endpoint), vec![0]);
            grid.entry(cell(origin)).or_insert_with(Vec::new).push(1);
            grid.insert([12, 12, 12], vec![2]);
            let mut seen = 0;
            let candidates =
                physical_tuck_ray_candidates(&grid, origin, normal, &mut seen).unwrap();
            assert_eq!(candidates, vec![0, 1]);
            assert_eq!(seen, 2);
            let mut exhausted = MAX_PHYSICAL_TUCK_CANDIDATES;
            assert!(physical_tuck_ray_candidates(&grid, origin, normal, &mut exhausted).is_err());
        }
    }

    #[test]
    fn physical_tuck_ray_grid_preserves_actual_boundary_hits() {
        for normal in [Vec3::Z, -Vec3::Z, Vec3::new(1., 1., 1.).normalize()] {
            let origin = Vec3::new(0.5, 0., 0.) - normal * CLEARANCE_FLOOR_EPSILON;
            let start = origin + normal * CLEARANCE_FLOOR_EPSILON;
            let u = normal.cross(Vec3::X).normalize();
            let v = normal.cross(u);
            for distance in [0., TUCK_QUERY_REACH - 1e-6, TUCK_QUERY_REACH] {
                let centre = start + normal * distance;
                let triangle = [
                    centre - u * 0.125 - v * 0.125,
                    centre + u * 0.125 - v * 0.125,
                    centre + v * 0.125,
                ];
                let mut positions: Vec<_> = triangle.map(|p| p.to_array()).to_vec();
                // This triangle occupies nearby search cells but misses the
                // ray. Candidate membership alone must not count as a hit.
                positions.extend(triangle.map(|p| (p + u * 0.1).to_array()));
                let outer = part("outer", positions, vec![0, 1, 2, 3, 4, 5], json!({}));
                let (triangles, grid) = collider_with_margin(
                    &outer,
                    GridBudget {
                        cells: MAX_GRID_CELLS,
                        references: MAX_GRID_REFERENCES,
                    },
                    0.,
                )
                .unwrap();
                let legacy = nearby_candidates(&grid, origin, TUCK_QUERY_REACH);
                let mut seen = 0;
                let finite =
                    physical_tuck_ray_candidates(&grid, origin, normal, &mut seen).unwrap();
                let hits = |candidates: &[usize]| {
                    candidates
                        .iter()
                        .filter_map(|&i| {
                            ray_triangle_distance(start, normal, triangles[i])
                                .filter(|t| *t >= 0. && *t <= TUCK_QUERY_REACH)
                                .map(|t| (i, t))
                        })
                        .collect::<Vec<_>>()
                };
                assert_eq!(hits(&finite), hits(&legacy));
                // Axis-aligned start and end cases are exactly representable
                // and prove both boundaries, rather than vacuous parity.
                if normal == Vec3::Z || normal == -Vec3::Z {
                    assert_eq!(hits(&finite).len(), 1, "distance={distance}");
                }
            }
        }
    }

    #[test]
    fn bounded_projection_keeps_reachable_orthogonal_clearance_and_caps_turns() {
        // A .35dm x correction can accept a .1dm z correction: the combined
        // .364dm vector is inside the 4cm ball. Scalar remaining length (.05)
        // would incorrectly leave the z contact unresolved.
        let turned = bounded_add(Vec3::new(0.35, 0., 0.), Vec3::Z, 0.1);
        assert!((turned - Vec3::new(0.35, 0., 0.1)).length() < 1e-6);
        assert!(turned.length() < MAX_CORRECTION);

        // Once on the boundary, an opposite/turning proposal remains bounded.
        let capped = bounded_add(Vec3::X * MAX_CORRECTION, Vec3::Z, 0.1);
        assert!(capped.length() <= MAX_CORRECTION + 1e-6);
        assert!(capped.z > 0.);
    }

    #[test]
    fn leaves_clear_clothing_unchanged_and_ignores_invalid_opt_in() {
        let mut garment = cloth(vec![[0., 0., 0.1], [0.1, 0., 0.1], [0., 0.1, 0.1]]);
        let before = garment.positions.clone();
        assert_eq!(
            fit_custom_clothing_clearance(&mut garment, &body())
                .unwrap()
                .unwrap()
                .corrected_vertices,
            0
        );
        assert_eq!(garment.positions, before);
        garment.material = json!({"fitClearanceMm": 11.0});
        assert_eq!(
            fit_custom_clothing_clearance(&mut garment, &body()).unwrap(),
            None
        );
    }

    #[test]
    fn deep_clips_stay_bounded_and_are_reported() {
        let mut garment = cloth(vec![[0., 0., -1.], [0.1, 0., -1.], [0., 0.1, -1.]]);
        let before = garment.positions.clone();
        let stats = fit_custom_clothing_clearance(&mut garment, &body())
            .unwrap()
            .unwrap();
        assert!(stats.remaining_violations > 0);
        assert!(garment.positions.iter().zip(before).all(|(after, before)| {
            (Vec3::from(*after) - Vec3::from(before)).length() <= MAX_CORRECTION + 1e-6
        }));
    }

    #[test]
    fn correction_limit_omitted_and_explicit_40mm_are_exactly_legacy_parity() {
        let points = vec![[0., 0., -0.055], [0.1, 0., -0.055], [0., 0.1, -0.055]];
        let mut omitted = cloth(points.clone());
        let mut explicit = omitted.clone();
        explicit.material["fitMaxCorrectionMm"] = json!(40.0);
        let omitted_stats = fit_custom_clothing_clearance(&mut omitted, &body()).unwrap();
        let explicit_stats = fit_custom_clothing_clearance(&mut explicit, &body()).unwrap();
        assert_eq!(explicit.positions, omitted.positions);
        assert_eq!(explicit_stats, omitted_stats);
    }

    #[test]
    fn nondefault_limit_keeps_crossing_triangle_normal_and_report_parity() {
        // Every vertex is outside the local domain, but this long body face
        // crosses it and must remain available to both fitting and reporting.
        let body = part(
            "Body",
            vec![[-3., -3., 0.], [3., -3., 0.], [0., 3., 0.]],
            vec![0, 1, 2],
            json!({}),
        );
        let positions = vec![Vec3::new(0., 0., -0.65)];
        let budget = GridBudget {
            cells: MAX_GRID_CELLS,
            references: MAX_GRID_REFERENCES,
        };
        let (full_triangles, full_grid) = collider_with_margin(&body, budget, 0.75).unwrap();
        let (local_triangles, local_grid) =
            collider_with_limit(&body, budget, 0.65, &positions).unwrap();
        assert_eq!(local_triangles.len(), 1);

        for (point, reach) in [
            (positions[0], 0.65),
            (Vec3::new(0., 0., 0.95), REPORT_DISTANCE),
        ] {
            let full = nearest_surface(
                point,
                &nearby_candidates(&full_grid, point, reach),
                &full_triangles,
                0.1,
                reach,
            );
            let local = nearest_surface(
                point,
                &nearby_candidates(&local_grid, point, reach),
                &local_triangles,
                0.1,
                reach,
            );
            assert_eq!(local, full);
        }
    }

    #[test]
    fn nondefault_limit_keeps_face_touching_report_domain_boundary() {
        let limit = 0.65;
        let source = Vec3::ZERO;
        let boundary = limit + REPORT_DISTANCE + 0.1 + CLEARANCE_FLOOR_EPSILON;
        let body = part(
            "Body",
            vec![
                [-0.1, -0.1, boundary],
                [0.1, -0.1, boundary],
                [0., 0.1, boundary],
            ],
            vec![0, 1, 2],
            json!({}),
        );
        let budget = GridBudget {
            cells: MAX_GRID_CELLS,
            references: MAX_GRID_REFERENCES,
        };
        let (full_triangles, full_grid) = collider_with_margin(&body, budget, limit + 0.1).unwrap();
        let (local_triangles, local_grid) =
            collider_with_limit(&body, budget, limit, &[source]).unwrap();
        assert_eq!(local_triangles.len(), 1);
        let point = Vec3::new(0., 0., boundary - REPORT_DISTANCE);
        let full = nearest_surface(
            point,
            &nearby_candidates(&full_grid, point, REPORT_DISTANCE),
            &full_triangles,
            0.1,
            REPORT_DISTANCE,
        );
        let local = nearest_surface(
            point,
            &nearby_candidates(&local_grid, point, REPORT_DISTANCE),
            &local_triangles,
            0.1,
            REPORT_DISTANCE,
        );
        assert_eq!(local, full);
    }

    #[test]
    fn nondefault_limit_filters_grid_without_raising_its_budget() {
        let body = part(
            "Body",
            vec![
                [-0.1, -0.1, 0.],
                [0.1, -0.1, 0.],
                [0., 0.1, 0.],
                [20., -0.1, 0.],
                [20.2, -0.1, 0.],
                [20., 0.1, 0.],
            ],
            vec![0, 1, 2, 3, 4, 5],
            json!({}),
        );
        let budget = GridBudget {
            cells: 70,
            references: 70,
        };
        assert!(collider_with_margin(&body, budget, 0.75).is_err());
        let (triangles, grid) = collider_with_limit(&body, budget, 0.65, &[Vec3::ZERO]).unwrap();
        assert_eq!(triangles.len(), 1);
        assert!(grid.len() <= budget.cells);
        assert!(grid.values().map(Vec::len).sum::<usize>() <= budget.references);
    }

    #[test]
    fn nondefault_limit_rejects_incomplete_body_triangle_stream() {
        let mut malformed = body();
        malformed.indices.push(99);
        assert!(
            collider_with_limit(
                &malformed,
                GridBudget {
                    cells: MAX_GRID_CELLS,
                    references: MAX_GRID_REFERENCES
                },
                0.65,
                &[Vec3::ZERO],
            )
            .err()
            .expect("incomplete body triangles must reject")
            .contains("incomplete")
        );
    }

    #[test]
    fn nondefault_limit_rejects_invalid_body_face_outside_local_domain() {
        let body = part(
            "Body",
            vec![
                [-0.1, -0.1, 0.],
                [0.1, -0.1, 0.],
                [0., 0.1, 0.],
                [20., -0.1, 0.],
                [20.2, -0.1, 0.],
            ],
            vec![0, 1, 2, 3, 4, 99],
            json!({}),
        );
        assert!(
            collider_with_limit(
                &body,
                GridBudget {
                    cells: MAX_GRID_CELLS,
                    references: MAX_GRID_REFERENCES,
                },
                0.65,
                &[Vec3::ZERO],
            )
            .is_err()
        );
    }

    #[test]
    fn correction_limit_60mm_clears_a_55mm_clip_that_legacy_budget_cannot() {
        let points = vec![[0., 0., -0.55], [0.1, 0., -0.55], [0., 0.1, -0.55]];
        let mut legacy = cloth(points.clone());
        let _legacy_stats = fit_custom_clothing_clearance(&mut legacy, &body())
            .unwrap()
            .unwrap();
        assert!(legacy.positions.iter().all(|point| point[2] < 0.0));

        let mut expanded = cloth(points);
        expanded.material["fitMaxCorrectionMm"] = json!(60.0);
        let stats = fit_custom_clothing_clearance(&mut expanded, &body())
            .unwrap()
            .unwrap();
        assert_eq!(stats.corrected_vertices, 3);
        assert!(
            expanded
                .positions
                .iter()
                .all(|point| point[2] >= 0.03 - 1e-5)
        );
        assert!(expanded.positions.iter().all(|point| {
            Vec3::from(*point).distance(Vec3::new(point[0], point[1], -0.55)) <= 0.6 + 1e-6
        }));
    }

    #[test]
    fn correction_limit_metadata_is_atomic_when_invalid_or_missing_clearance() {
        let points = vec![[0., 0., -0.02], [0.1, 0., -0.02], [0., 0.1, -0.02]];
        for value in [json!("60"), json!(0.0), json!(80.1), json!(2.0)] {
            let mut garment = cloth(points.clone());
            garment.material["fitMaxCorrectionMm"] = value;
            assert!(fit_custom_clothing_clearance(&mut garment, &body()).is_err());
            assert_eq!(garment.positions, points);
        }
        let mut missing = cloth(points.clone());
        missing
            .material
            .as_object_mut()
            .unwrap()
            .remove("fitClearanceMm");
        missing.material["fitMaxCorrectionMm"] = json!(60.0);
        assert!(fit_custom_clothing_clearance(&mut missing, &body()).is_err());
        assert_eq!(missing.positions, points);
    }

    #[test]
    fn layer_clearance_uses_the_same_60mm_source_relative_budget() {
        let lower = part(
            "custom:bottom",
            vec![[-1., -1., 0.2], [1., -1., 0.2], [0., 1., 0.2]],
            vec![0, 1, 2],
            json!({}),
        );
        let source = vec![[0., 0., -0.55], [0.1, 0., -0.55], [0., 0.1, -0.55]];
        let mut outer = part(
            "custom:top",
            source.clone(),
            vec![0, 1, 2],
            json!({"fitClearanceMm": 3.0, "fitMaxCorrectionMm": 60.0, "fitLayerClearanceMm": 3.0}),
        );
        fit_custom_clothing_clearance(&mut outer, &body()).unwrap();
        fit_custom_layer_clearance(&mut outer, &[&lower], &body(), &source).unwrap();
        assert!(outer.positions.iter().zip(&source).all(|(after, before)| {
            (Vec3::from(*after) - Vec3::from(*before)).length() <= 0.6 + 1e-6
        }));
    }

    #[test]
    fn exact_uv_copies_receive_identical_displacement_and_nonfinite_is_rejected() {
        let mut garment = cloth(vec![
            [0., 0., -0.01],
            [0.1, 0., -0.01],
            [0., 0.1, -0.01],
            [0., 0., -0.01],
        ]);
        garment.indices.extend([3, 2, 1]);
        let before = garment.positions.clone();
        fit_custom_clothing_clearance(&mut garment, &body()).unwrap();
        assert_eq!(
            Vec3::from(garment.positions[0]) - Vec3::from(before[0]),
            Vec3::from(garment.positions[3]) - Vec3::from(before[3])
        );
        garment.positions[0][0] = f32::NAN;
        assert!(fit_custom_clothing_clearance(&mut garment, &body()).is_err());
        let mut visible_body = body();
        visible_body.positions[0][0] = f32::NAN;
        assert!(collider(&visible_body).is_err());
        let mut distant_garment = cloth(vec![[0., 0., 0.], [0.1, 0., 0.], [0., 0.1, 0.]]);
        distant_garment.positions[0][0] = MAX_BODY_COORDINATE + 1.;
        assert!(fit_custom_clothing_clearance(&mut distant_garment, &body()).is_err());
    }

    #[test]
    fn preserves_canonical_winding_for_a_concave_inner_body_surface() {
        let body = part(
            "Body",
            vec![
                [-0.5, -1., -1.],
                [-0.5, 1., -1.],
                [-0.5, 0., 1.],
                [1., -1., -1.],
                [1., 0., 1.],
                [1., 1., -1.],
            ],
            vec![0, 1, 2, 3, 4, 5],
            json!({}),
        );
        let (triangles, _) = collider(&body).unwrap();
        assert!(triangles[0].outward.dot(Vec3::X) > 0.999);
    }

    #[test]
    fn nearest_clear_surface_wins_over_farther_violating_surface() {
        let triangle = |z| Triangle {
            a: Vec3::new(-1., -1., z),
            b: Vec3::new(1., -1., z),
            c: Vec3::new(0., 1., z),
            outward: Vec3::Z,
        };
        let triangles = vec![triangle(0.), triangle(0.05)];
        assert_eq!(
            violation(Vec3::new(0., 0., 0.01), &[0, 1], &triangles, 0.003, 0.2),
            None
        );
    }

    #[test]
    fn opt_in_triangle_contact_pass_repairs_only_same_facing_crossings() {
        let body = body();
        let (triangles, grid) = collider(&body).unwrap();
        let mut garment = part(
            "custom:top",
            vec![[-0.1, -0.1, -0.01], [0.1, -0.1, 0.02], [-0.1, 0.1, 0.02]],
            vec![0, 1, 2],
            json!({"fitClearanceMm": 3.0, "fitTriangleClearance": true}),
        );
        let original: Vec<_> = garment.positions.iter().copied().map(Vec3::from).collect();
        let owner = vec![0, 1, 2];
        let mut displacement = vec![Vec3::ZERO; 3];
        resolve_triangle_contacts(
            &garment,
            &original,
            &owner,
            &mut displacement,
            &triangles,
            &grid,
            &[0.03; 3],
            None,
            MAX_CORRECTION,
        )
        .unwrap();
        assert!(displacement[0].z > 0.);
        assert!(
            displacement
                .iter()
                .all(|delta| delta.length() <= 0.03 + 1e-6)
        );
        let corrected = [
            original[0] + displacement[0],
            original[1] + displacement[1],
            original[2] + displacement[2],
        ];
        assert!(!triangles_intersect(
            corrected[0],
            corrected[1],
            corrected[2],
            triangles[0]
        ));

        garment.indices = vec![0, 2, 1]; // opposite-facing inner surface
        let mut opposite = vec![Vec3::ZERO; 3];
        resolve_triangle_contacts(
            &garment,
            &original,
            &owner,
            &mut opposite,
            &triangles,
            &grid,
            &[0.03; 3],
            None,
            MAX_CORRECTION,
        )
        .unwrap();
        assert_eq!(opposite, vec![Vec3::ZERO; 3]);

        garment.indices = vec![0, 1, 2];
        garment.material["fitTriangleClearance"] = json!(false);
        let mut disabled = vec![Vec3::ZERO; 3];
        resolve_triangle_contacts(
            &garment,
            &original,
            &owner,
            &mut disabled,
            &triangles,
            &grid,
            &[0.03; 3],
            None,
            MAX_CORRECTION,
        )
        .unwrap();
        assert_eq!(disabled, vec![Vec3::ZERO; 3]);
    }

    #[test]
    fn triangle_contact_at_cell_boundary_keeps_same_facing_candidate() {
        let body = body();
        let (triangles, grid) = collider(&body).unwrap();
        let candidates = triangle_candidates(
            Vec3::new(-CELL, -0.1, -0.01),
            Vec3::new(CELL, -0.1, 0.02),
            Vec3::new(0., 0.1, 0.02),
            &grid,
        )
        .unwrap();
        assert!(candidates.contains(&0));
        assert!(triangles[0].outward.dot(Vec3::Z) > 0.);
    }

    #[test]
    fn exact_contact_grid_skips_degenerate_faces_without_reordering_valid_triangles() {
        let body = part(
            "Body",
            vec![
                [0., 0., 0.],
                [0., 0., 0.],
                [1., 0., 0.],
                [-CELL, -0.1, 0.],
                [CELL, -0.1, 0.],
                [0., 0.1, 0.],
            ],
            vec![0, 1, 2, 3, 4, 5],
            json!({}),
        );
        let (triangles, grid) = collider_with_margin(
            &body,
            GridBudget {
                cells: MAX_GRID_CELLS,
                references: MAX_GRID_REFERENCES,
            },
            1e-5,
        )
        .unwrap();
        assert_eq!(triangles.len(), 1);
        assert!(triangles[0].outward.dot(Vec3::Z).abs() > 0.99);
        let candidates = triangle_candidates(
            Vec3::new(-CELL, -0.1, -0.01),
            Vec3::new(CELL, -0.1, 0.02),
            Vec3::new(0., 0.1, 0.02),
            &grid,
        )
        .unwrap();
        assert_eq!(candidates, vec![0]);
    }

    #[test]
    fn rejects_many_individually_legal_triangles_that_exceed_the_total_grid_budget() {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for triangle in 0..20u32 {
            let start = positions.len() as u32;
            let x = triangle as f32 * 3.;
            positions.extend([[x, 0., 0.], [x + 0.1, 0., 0.], [x, 0.1, 0.]]);
            indices.extend([start, start + 1, start + 2]);
        }
        let body = part("Body", positions, indices, json!({}));
        let budget = GridBudget {
            cells: 100,
            references: 1_000,
        };
        assert!(collider_with_budget(&body, budget).is_err());
    }

    #[test]
    fn rejects_finite_body_coordinates_outside_the_safe_grid_domain() {
        let mut visible_body = body();
        visible_body.positions[0][0] = MAX_BODY_COORDINATE + 1.;
        assert!(collider(&visible_body).is_err());
    }
}
