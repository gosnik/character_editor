//! Explicit thin-wall correspondence for source clothing with an inner lining.
use crate::{
    native_character::NativePart,
    wardrobe_assets::{CustomGarment, GarmentSlot},
};
use bevy::prelude::Vec3;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShellPair {
    pub(crate) vertex: usize,
    pub(crate) vertices: [usize; 3],
    pub(crate) weights: [f32; 3],
}

pub(crate) fn parse_shell_pairs(garment: &CustomGarment) -> Result<Vec<ShellPair>, String> {
    let Some(value) = garment.material.get("fitShellDisplacementPairs") else {
        return Ok(Vec::new());
    };
    if !matches!(garment.slot, GarmentSlot::Top | GarmentSlot::Jacket) {
        return Err("fitShellDisplacementPairs is only valid for top or jacket garments".into());
    }
    let entries = value
        .as_array()
        .ok_or("fitShellDisplacementPairs must be an array")?;
    if entries.len() > garment.positions_dm.len() || entries.len() > 100_000 {
        return Err("fitShellDisplacementPairs exceeds the source vertex budget".into());
    }
    let mut pairs: Vec<ShellPair> = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid fitShellDisplacementPairs: {error}"))?;
    let mut controlled = HashSet::with_capacity(pairs.len());
    for pair in &mut pairs {
        if pair.vertex >= garment.positions_dm.len()
            || pair
                .vertices
                .iter()
                .any(|&index| index >= garment.positions_dm.len())
            || !controlled.insert(pair.vertex)
        {
            return Err("shell pair has an invalid or duplicate lining vertex".into());
        }
        if pair.vertices[0] == pair.vertices[1]
            || pair.vertices[1] == pair.vertices[2]
            || pair.vertices[0] == pair.vertices[2]
            || pair
                .weights
                .iter()
                .any(|weight| !weight.is_finite() || *weight < 0.)
        {
            return Err("shell pair has invalid outer triangle or weights".into());
        }
        let sum: f32 = pair.weights.iter().sum();
        if !sum.is_finite() || (sum - 1.).abs() > 0.0001 {
            return Err("shell pair weights must sum to one".into());
        }
        for weight in &mut pair.weights {
            *weight /= sum;
        }
    }
    let triangles: HashSet<[usize; 3]> = garment
        .indices
        .chunks_exact(3)
        .map(|face| {
            let mut key = [face[0] as usize, face[1] as usize, face[2] as usize];
            key.sort_unstable();
            key
        })
        .collect();
    for pair in &pairs {
        if pair.vertices.iter().any(|index| controlled.contains(index)) {
            return Err(
                "shell pair outer controls must be disjoint from all lining targets".into(),
            );
        }
        let mut triangle = pair.vertices;
        triangle.sort_unstable();
        if !triangles.contains(&triangle) {
            return Err("shell pair controls must form a saved source triangle".into());
        }
        let [a, b, c] = pair
            .vertices
            .map(|index| Vec3::from(garment.positions_dm[index]));
        let anchor = a * pair.weights[0] + b * pair.weights[1] + c * pair.weights[2];
        let offset = Vec3::from(garment.positions_dm[pair.vertex]) - anchor;
        if !offset.is_finite()
            || offset.length() <= 0.000001
            || offset.length() > 0.3
            || (b - a).cross(c - a).length_squared() <= 1e-16
        {
            return Err(
                "shell pair requires a finite nondegenerate wall at most 30 mm thick".into(),
            );
        }
    }
    // Keep the source-weld alias check in ordinary garment validation. A
    // lining target welded to an outer control would make the recurrence
    // self-referential even before runtime clearance is requested.
    let weld_key = |index: usize| -> Result<[i64; 3], String> {
        let point = garment.positions_dm[index];
        if !point.iter().all(|v| v.is_finite() && v.abs() <= 100.) {
            return Err("shell pair has non-finite or out-of-range source positions".into());
        }
        Ok(point.map(|v| (v * 1_000_000.).round() as i64))
    };
    let mut outer_welds = HashSet::new();
    for pair in &pairs {
        for control in pair.vertices {
            outer_welds.insert(weld_key(control)?);
        }
    }
    for pair in &pairs {
        if outer_welds.contains(&weld_key(pair.vertex)?) {
            return Err(
                "shell pair source weld group aliases lining target and outer control".into(),
            );
        }
    }
    Ok(pairs)
}

/// Result of the bounded lining clearance pass.  Counts are measured only
/// after the final source-pair recoupling, so callers never report a transient
/// independently moved lining as clear.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ShellClearanceStats {
    pub(crate) corrected_controls: usize,
    pub(crate) max_total_displacement: f32,
    pub(crate) remaining_violations: usize,
}

const PASSES: usize = 4;
const DEFAULT_TOTAL_DISPLACEMENT: f32 = 0.4; // DM: includes the earlier cloth pass.
const WELD_EPSILON: f32 = 1e-6;

type WeldKey = [i64; 3];

fn source_weld_groups(
    garment: &CustomGarment,
    pairs: &[ShellPair],
) -> Result<(Vec<usize>, Vec<Vec<usize>>), String> {
    let mut lookup = BTreeMap::<WeldKey, usize>::new();
    let mut owner = Vec::with_capacity(garment.positions_dm.len());
    let mut members = Vec::<Vec<usize>>::new();
    for (index, position) in garment.positions_dm.iter().enumerate() {
        if !position.iter().all(|v| v.is_finite() && v.abs() <= 100.) {
            return Err("shell coupling has non-finite or out-of-range source positions".into());
        }
        let key = position.map(|v| (v / WELD_EPSILON).round() as i64);
        let group = if let Some(&group) = lookup.get(&key) {
            group
        } else {
            let group = members.len();
            lookup.insert(key, group);
            members.push(Vec::new());
            group
        };
        owner.push(group);
        members[group].push(index);
    }
    let controls = pairs
        .iter()
        .flat_map(|pair| pair.vertices)
        .map(|index| owner[index])
        .collect::<HashSet<_>>();
    if pairs
        .iter()
        .any(|pair| controls.contains(&owner[pair.vertex]))
    {
        return Err("shell pair source weld group aliases lining target and outer control".into());
    }
    Ok((owner, members))
}

fn check_streams(garment: &CustomGarment, part: &NativePart) -> Result<(), String> {
    let n = garment.positions_dm.len();
    if part.positions.len() != n || part.refs.len() != n || part.ref_weights.len() != n {
        return Err("shell coupling has inconsistent runtime streams".into());
    }
    if part.positions.iter().any(|p| !Vec3::from(*p).is_finite()) {
        return Err("shell coupling has non-finite runtime positions".into());
    }
    Ok(())
}

fn recouple_positions(garment: &CustomGarment, part: &mut NativePart, pairs: &[ShellPair]) {
    for pair in pairs {
        let delta = pair.vertices.into_iter().zip(pair.weights).fold(
            Vec3::ZERO,
            |delta, (index, weight)| {
                delta
                    + (Vec3::from(part.positions[index]) - Vec3::from(garment.positions_dm[index]))
                        * weight
            },
        );
        part.positions[pair.vertex] =
            (Vec3::from(garment.positions_dm[pair.vertex]) + delta).to_array();
    }
}

/// Copy final outer rig streams only after all geometry movement has settled.
/// The source JSON remains untouched; this makes the inner wall follow the
/// dominant outer skin stream during animation without a second geometric
/// rebinding step that could break the wall equation.
pub(crate) fn couple_shell_bindings(
    garment: &CustomGarment,
    part: &mut NativePart,
) -> Result<usize, String> {
    let pairs = parse_shell_pairs(garment)?;
    if pairs.is_empty() {
        return Ok(0);
    }
    check_streams(garment, part)?;
    for pair in &pairs {
        let dominant = (1..3).fold(0, |best, index| {
            (pair.weights[index] > pair.weights[best])
                .then_some(index)
                .unwrap_or(best)
        });
        let control = pair.vertices[dominant];
        part.refs[pair.vertex] = part.refs[control];
        part.ref_weights[pair.vertex] = part.ref_weights[control];
    }
    Ok(pairs.len())
}

/// Recouple a lining from immutable authored source positions, propose body
/// corrections through only its outer controls, and apply one configured
/// aggregate displacement ball relative to `pre_clearance`.  Exact authored-position
/// seam copies share each outer-control movement even when their skin streams
/// differ.
pub(crate) fn resolve_shell_clearance(
    garment: &CustomGarment,
    part: &mut NativePart,
    body: &NativePart,
    pre_clearance: &[[f32; 3]],
) -> Result<Option<ShellClearanceStats>, String> {
    let pairs = parse_shell_pairs(garment)?;
    if pairs.is_empty() {
        return Ok(None);
    }
    check_streams(garment, part)?;
    if pre_clearance.len() != part.positions.len()
        || pre_clearance.iter().any(|p| !Vec3::from(*p).is_finite())
    {
        return Err("shell clearance has an invalid pre-clearance snapshot".into());
    }
    let Some(clearance) = crate::native_cloth::clearance_request(part) else {
        recouple_positions(garment, part, &pairs);
        return Ok(Some(ShellClearanceStats::default()));
    };
    let correction_limit = crate::native_cloth::correction_limit(part)?;
    let collider = if correction_limit == DEFAULT_TOTAL_DISPLACEMENT {
        // Preserve the default 40 mm collider and movement path exactly.
        crate::native_cloth::clearance_collider(body)?
    } else {
        crate::native_cloth::clearance_collider_with_limit(body, correction_limit, pre_clearance)?
    };
    let (owner, members) = source_weld_groups(garment, &pairs)?;
    let control_groups = pairs
        .iter()
        .flat_map(|pair| pair.vertices)
        .map(|index| owner[index])
        .collect::<HashSet<_>>();

    for _ in 0..PASSES {
        recouple_positions(garment, part, &pairs);
        let mut proposals = BTreeMap::<usize, (Vec3, f32)>::new();
        for pair in &pairs {
            let point = Vec3::from(part.positions[pair.vertex]);
            let hit = if correction_limit == DEFAULT_TOTAL_DISPLACEMENT {
                crate::native_cloth::clearance_violation_at(&collider, point, clearance)
            } else {
                crate::native_cloth::clearance_violation_at_with_limit(
                    &collider,
                    point,
                    clearance,
                    correction_limit,
                )
            };
            let Some((outward, signed)) = hit else {
                continue;
            };
            let delta = outward * (clearance - signed).min(clearance);
            for (control, weight) in pair.vertices.into_iter().zip(pair.weights) {
                let entry = proposals.entry(owner[control]).or_insert((Vec3::ZERO, 0.));
                entry.0 += delta * weight;
                entry.1 += weight;
            }
        }
        for (group, (sum, weight)) in proposals {
            if weight <= 0. || !control_groups.contains(&group) {
                continue;
            }
            let addition = sum / weight;
            for &vertex in &members[group] {
                let baseline = Vec3::from(pre_clearance[vertex]);
                let proposed = Vec3::from(part.positions[vertex]) + addition;
                part.positions[vertex] = (baseline
                    + (proposed - baseline).clamp_length_max(correction_limit))
                .to_array();
            }
        }
    }
    recouple_positions(garment, part, &pairs);
    if correction_limit != DEFAULT_TOTAL_DISPLACEMENT {
        // A lining target is recomputed from source-space outer deltas after
        // the aliases settle. Both sides of an expanded-budget wall must obey
        // the same authority; do not silently clamp a pair equation into a
        // different wall shape. The legacy 40 mm path stays byte-for-byte
        // behavior-compatible.
        for &group in &control_groups {
            for &vertex in &members[group] {
                if (Vec3::from(part.positions[vertex]) - Vec3::from(pre_clearance[vertex])).length()
                    > correction_limit + 0.000_01
                {
                    return Err("shell outer control exceeds fitMaxCorrectionMm total cap".into());
                }
            }
        }
        for pair in &pairs {
            if (Vec3::from(part.positions[pair.vertex]) - Vec3::from(pre_clearance[pair.vertex]))
                .length()
                > correction_limit + 0.000_01
            {
                return Err(
                    "shell lining recoupling is incompatible with fitMaxCorrectionMm total cap"
                        .into(),
                );
            }
        }
    }
    let mut stats = ShellClearanceStats::default();
    for &group in &control_groups {
        for &vertex in &members[group] {
            let displacement =
                Vec3::from(part.positions[vertex]) - Vec3::from(pre_clearance[vertex]);
            if displacement.length_squared() > 0. {
                stats.corrected_controls += 1;
                stats.max_total_displacement =
                    stats.max_total_displacement.max(displacement.length());
            }
        }
    }
    for pair in &pairs {
        let point = Vec3::from(part.positions[pair.vertex]);
        let remains = if correction_limit == DEFAULT_TOTAL_DISPLACEMENT {
            crate::native_cloth::clearance_violation_at(&collider, point, clearance)
        } else {
            crate::native_cloth::clearance_violation_at_with_limit(
                &collider,
                point,
                clearance,
                correction_limit,
            )
        };
        if remains.is_some() {
            stats.remaining_violations += 1;
        }
    }
    Ok(Some(stats))
}

/// Compatibility helper for the old one-shot callers. New runtime assembly
/// uses `resolve_shell_clearance` followed by `couple_shell_bindings`.
pub(crate) fn couple_shell_pairs(
    garment: &CustomGarment,
    part: &mut NativePart,
) -> Result<usize, String> {
    let pairs = parse_shell_pairs(garment)?;
    if pairs.is_empty() {
        return Ok(0);
    }
    check_streams(garment, part)?;
    recouple_positions(garment, part, &pairs);
    couple_shell_bindings(garment, part)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (CustomGarment, NativePart) {
        let g = CustomGarment {
            id: "shell-test".into(),
            name: "shell test".into(),
            slot: GarmentSlot::Top,
            positions_dm: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0.3, 0.5, -0.06]],
            indices: vec![0, 1, 2, 3, 2, 1],
            uvs: vec![[0., 0.]; 4],
            refs: vec![[0; 3], [1; 3], [2; 3], [3; 3]],
            weights: vec![[1., 0., 0.]; 4],
            reference_points_dm: vec![[0.; 3]; 4],
            hidden_faces: vec![],
            material: json!({"fitShellDisplacementPairs":[{"vertex":3,"vertices":[0,1,2],"weights":[0.2,0.3,0.5]}]}),
        };
        let p = NativePart {
            name: "custom:shell-test".into(),
            positions: g.positions_dm.clone(),
            indices: g.indices.clone(),
            uvs: g.uvs.clone(),
            refs: g.refs.clone(),
            ref_weights: g.weights.clone(),
            rigid_joint: None,
            skin_weight_source_topology: None,
            material: g.material.clone(),
        };
        (g, p)
    }
    #[test]
    fn shell_coupling_preserves_neutral_geometry_and_saved_source() {
        let (g, mut p) = fixture();
        let source = serde_json::to_value(&g).unwrap();
        let before = p.positions.clone();
        couple_shell_pairs(&g, &mut p).unwrap();
        assert_eq!(before, p.positions);
        assert_eq!(source, serde_json::to_value(&g).unwrap());
        assert_eq!(p.refs[3], p.refs[2]);
        assert_eq!(p.uvs, g.uvs);
        assert_eq!(p.indices, g.indices);
    }
    #[test]
    fn shell_coupling_repairs_inverted_lining_with_continuous_outer_delta() {
        let (g, mut p) = fixture();
        p.positions[0][2] = 0.1;
        p.positions[1][2] = 0.2;
        p.positions[2][2] = 0.3;
        p.positions[3][2] = 0.5; // Independent fitting wrongly crossed the exterior.
        p.refs[2] = [8, 9, 10];
        p.ref_weights[2] = [0.2, 0.3, 0.5];
        couple_shell_pairs(&g, &mut p).unwrap();
        let outer = Vec3::from(p.positions[0]) * 0.2
            + Vec3::from(p.positions[1]) * 0.3
            + Vec3::from(p.positions[2]) * 0.5;
        assert!((Vec3::from(p.positions[3]) - outer - Vec3::new(0., 0., -0.06)).length() < 1e-6);
        assert_eq!(p.refs[3], [8, 9, 10]);
        assert_eq!(p.ref_weights[3], [0.2, 0.3, 0.5]);
    }
    #[test]
    fn shell_coupling_is_default_off_and_rejects_ambiguous_controls() {
        let (mut g, mut p) = fixture();
        g.material = json!({});
        let before = p.clone();
        assert_eq!(couple_shell_pairs(&g, &mut p).unwrap(), 0);
        assert_eq!(p.positions, before.positions);
        assert_eq!(p.refs, before.refs);
        let (g, _) = fixture();
        let mut bad = g.clone();
        bad.material["fitShellDisplacementPairs"][0]["weights"] = json!([0., 0., 0.]);
        assert!(parse_shell_pairs(&bad).is_err());
        let mut bad = g.clone();
        bad.material["fitShellDisplacementPairs"][0]["vertices"] = json!([0, 1, 3]);
        assert!(parse_shell_pairs(&bad).is_err());
        let mut bad = g.clone();
        let entry = bad.material["fitShellDisplacementPairs"][0].clone();
        bad.material["fitShellDisplacementPairs"]
            .as_array_mut()
            .unwrap()
            .push(entry);
        assert!(parse_shell_pairs(&bad).is_err());
        let mut bad = g;
        bad.slot = GarmentSlot::Hair;
        assert!(parse_shell_pairs(&bad).is_err());
    }

    fn visible_plane(z: f32) -> NativePart {
        NativePart {
            name: "Body".into(),
            positions: vec![[-2., -2., z], [4., -2., z], [-2., 4., z]],
            indices: vec![0, 1, 2],
            uvs: vec![[0., 0.]; 3],
            refs: vec![[0; 3]; 3],
            ref_weights: vec![[1., 0., 0.]; 3],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material: json!({}),
        }
    }

    #[test]
    fn shell_clearance_moves_source_welded_control_copies_together_and_recouples() {
        let (mut g, mut p) = fixture();
        // Vertex four is a UV seam copy of control zero, but deliberately has
        // a different animation binding. It is not a pair control itself.
        g.positions_dm.push(g.positions_dm[0]);
        g.uvs.push([1., 0.]);
        g.refs.push([9, 9, 9]);
        g.weights.push([1., 0., 0.]);
        g.reference_points_dm.push([0.; 3]);
        p.positions.push(g.positions_dm[0]);
        p.uvs.push([1., 0.]);
        p.refs.push([9, 9, 9]);
        p.ref_weights.push([1., 0., 0.]);
        g.material["fitClearanceMm"] = json!(1.0);
        p.material = g.material.clone();
        let pre = p.positions.clone();
        let stats = resolve_shell_clearance(&g, &mut p, &visible_plane(-0.05), &pre)
            .unwrap()
            .unwrap();
        let delta0 = Vec3::from(p.positions[0]) - Vec3::from(pre[0]);
        let delta4 = Vec3::from(p.positions[4]) - Vec3::from(pre[4]);
        assert!(delta0.z > 0.);
        assert!((delta0 - delta4).length() < 1e-6);
        let outer = Vec3::from(p.positions[0]) * 0.2
            + Vec3::from(p.positions[1]) * 0.3
            + Vec3::from(p.positions[2]) * 0.5;
        assert!((Vec3::from(p.positions[3]) - outer - Vec3::new(0., 0., -0.06)).length() < 1e-6);
        assert_eq!(stats.remaining_violations, 0);
        // Geometry changes do not rewrite saved per-vertex rig streams.
        assert_eq!(p.refs[4], [9, 9, 9]);
    }

    #[test]
    fn shell_clearance_caps_total_motion_from_pre_clearance_and_reports_residual() {
        let (mut g, mut p) = fixture();
        g.material["fitClearanceMm"] = json!(10.0);
        p.material = g.material.clone();
        let pre = p.positions.clone();
        // Model an earlier ordinary cloth pass that already spent 39mm.
        for index in [0, 1, 2] {
            p.positions[index][2] += 0.39;
        }
        let stats = resolve_shell_clearance(&g, &mut p, &visible_plane(0.5), &pre)
            .unwrap()
            .unwrap();
        for index in [0, 1, 2] {
            assert!((Vec3::from(p.positions[index]) - Vec3::from(pre[index])).length() <= 0.400001);
        }
        assert!(stats.max_total_displacement <= 0.400001);
        assert!(stats.remaining_violations > 0);
    }

    #[test]
    fn shell_clearance_explicit_default_40mm_matches_omitted_limit() {
        let (mut omitted_garment, mut omitted_part) = fixture();
        omitted_garment.material["fitClearanceMm"] = json!(10.0);
        omitted_part.material = omitted_garment.material.clone();
        let mut explicit_garment = omitted_garment.clone();
        let mut explicit_part = omitted_part.clone();
        explicit_garment.material["fitMaxCorrectionMm"] = json!(40.0);
        explicit_part.material = explicit_garment.material.clone();
        let omitted_pre = omitted_part.positions.clone();
        let explicit_pre = explicit_part.positions.clone();
        let omitted = resolve_shell_clearance(
            &omitted_garment,
            &mut omitted_part,
            &visible_plane(-0.05),
            &omitted_pre,
        )
        .unwrap();
        let explicit = resolve_shell_clearance(
            &explicit_garment,
            &mut explicit_part,
            &visible_plane(-0.05),
            &explicit_pre,
        )
        .unwrap();
        assert_eq!(explicit_part.positions, omitted_part.positions);
        assert_eq!(explicit, omitted);
    }

    #[test]
    fn shell_clearance_nondefault_65mm_caps_combined_motion_from_snapshot() {
        let (mut garment, mut part) = fixture();
        garment.material["fitClearanceMm"] = json!(10.0);
        garment.material["fitMaxCorrectionMm"] = json!(65.0);
        part.material = garment.material.clone();
        let pre = part.positions.clone();
        // Model an ordinary pass that already consumed 64 mm.
        for index in [0, 1, 2] {
            part.positions[index][2] += 0.64;
        }
        let stats = resolve_shell_clearance(&garment, &mut part, &visible_plane(0.5), &pre)
            .unwrap()
            .unwrap();
        for index in [0, 1, 2, 3] {
            assert!(
                (Vec3::from(part.positions[index]) - Vec3::from(pre[index])).length() <= 0.650_01
            );
        }
        assert!(stats.max_total_displacement <= 0.650_01);
    }

    #[test]
    fn shell_clearance_nondefault_cap_rejects_incompatible_recoupled_lining() {
        let (mut garment, mut part) = fixture();
        garment.material["fitClearanceMm"] = json!(1.0);
        garment.material["fitMaxCorrectionMm"] = json!(65.0);
        part.material = garment.material.clone();
        let mut pre = part.positions.clone();
        for index in [0, 1, 2] {
            part.positions[index][2] += 0.64;
        }
        // An earlier pass moved the lining in the opposite direction. Its
        // source-pair equation cannot be restored within one 65 mm authority.
        pre[3][2] -= 0.64;
        assert!(
            resolve_shell_clearance(&garment, &mut part, &visible_plane(-2.0), &pre)
                .err()
                .expect("incompatible lining cap must reject")
                .contains("lining recoupling")
        );
    }

    #[test]
    fn shell_clearance_default_40mm_keeps_legacy_incompatible_lining_behavior() {
        let (mut garment, mut part) = fixture();
        garment.material["fitClearanceMm"] = json!(1.0);
        part.material = garment.material.clone();
        let mut pre = part.positions.clone();
        for index in [0, 1, 2] {
            part.positions[index][2] += 0.39;
        }
        // The legacy path allowed recoupling from a separately moved lining;
        // retaining that behavior keeps absent and explicit 40 mm metadata
        // identical. The nondefault case above rejects it under its declared
        // aggregate authority.
        pre[3][2] -= 0.64;
        assert!(
            resolve_shell_clearance(&garment, &mut part, &visible_plane(-2.0), &pre)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn shell_pairs_accept_valid_nondefault_limit_and_reject_malformed_limit() {
        let (mut garment, _) = fixture();
        garment.material["fitClearanceMm"] = json!(3.0);
        garment.material["fitMaxCorrectionMm"] = json!(65.0);
        assert!(garment.validate().is_ok());
        garment.material["fitMaxCorrectionMm"] = json!("65");
        assert!(garment.validate().is_err());
    }

    #[test]
    fn shell_clearance_rejects_aliasing_lining_and_control_source_geometry() {
        let (mut g, _) = fixture();
        g.positions_dm[3] = g.positions_dm[0];
        // The source-pair parser rejects this before the runtime group pass:
        // an aliased target has a zero wall offset and cannot form a feedback
        // loop with a control group.
        assert!(parse_shell_pairs(&g).is_err());
    }
}
