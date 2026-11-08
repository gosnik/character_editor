//! Opt-in two-segment source-space sleeve cage for cartoon garments.
use std::collections::HashSet;

use bevy::prelude::{Quat, Vec3};
use serde_json::Value;

use crate::native_fit::BodyDeformation;

const MAX_VERTICES: usize = 100_000;
const BODY_VERTICES: usize = 19_158;
const MIN_SCALE: f32 = 0.125;
const MAX_SCALE: f32 = 8.0;

#[derive(Clone, Debug)]
pub struct SleeveCage {
    arms: Vec<Arm>,
}
#[derive(Clone, Debug)]
struct Arm {
    vertices: Vec<usize>,
    weights: Vec<f32>,
    shoulder: Vec<usize>,
    elbow: Vec<usize>,
    wrist: Vec<usize>,
    root_blend: f32,
    elbow_blend: f32,
    radial_mode: RadialMode,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RadialMode {
    /// The original multiplicative body-radius transport.
    BodyRatio,
    /// Preserve authored loose-sleeve ease while the body radius changes.
    PreserveEase,
}
#[derive(Clone, Copy)]
struct ArmMap<'a> {
    arm: &'a Arm,
    upper: Segment,
    lower: Segment,
}
#[derive(Clone, Copy)]
struct Segment {
    from: Vec3,
    axis: Vec3,
    to: Vec3,
    rotation: Quat,
    longitudinal_scale: f32,
    radial_scale: f32,
    neutral_radius: f32,
    current_radius: f32,
    radial_mode: RadialMode,
}

impl SleeveCage {
    pub fn parse(
        material: &Value,
        allowed_slot: bool,
        source_len: usize,
    ) -> Result<Option<Self>, String> {
        let Some(value) = material.get("fitSleeveCage") else {
            return Ok(None);
        };
        if !allowed_slot {
            return Err("fitSleeveCage is only valid for top or jacket garments".into());
        }
        for key in ["fitTorsoCage", "fitCoherentBodyAnchor"] {
            if material.get(key).is_some() {
                return Err(format!("fitSleeveCage cannot be combined with {key}"));
            }
        }
        let object = value.as_object().ok_or("fitSleeveCage must be an object")?;
        let arms = object
            .get("arms")
            .and_then(Value::as_array)
            .filter(|a| (1..=2).contains(&a.len()))
            .ok_or("fitSleeveCage needs 1..2 arms")?;
        let mut used = HashSet::new();
        let mut parsed = Vec::with_capacity(arms.len());
        for arm in arms {
            let object = arm
                .as_object()
                .ok_or("fitSleeveCage arms must be objects")?;
            let vertices = parse_indices(
                object,
                "vertexIndices",
                1,
                MAX_VERTICES,
                source_len,
                "sleeve cage",
            )?;
            if vertices.iter().any(|v| !used.insert(*v)) {
                return Err("fitSleeveCage arm vertexIndices overlap".into());
            }
            let weights = parse_weights(object, vertices.len())?;
            parsed.push(Arm {
                vertices,
                weights,
                shoulder: parse_ring(object, "shoulderRefs")?,
                elbow: parse_ring(object, "elbowRefs")?,
                wrist: parse_ring(object, "wristRefs")?,
                root_blend: parse_blend(object, "rootBlendDM")?,
                elbow_blend: parse_blend(object, "elbowBlendDM")?,
                radial_mode: parse_radial_mode(object)?,
            });
        }
        if parsed.iter().map(|arm| arm.vertices.len()).sum::<usize>() > MAX_VERTICES {
            return Err("fitSleeveCage selected vertex budget exceeds 100000".into());
        }
        Ok(Some(Self { arms: parsed }))
    }

    pub fn fit(
        &self,
        source: &[[f32; 3]],
        mapped: &mut [[f32; 3]],
        current_coords: &[[f32; 3]],
        field: &BodyDeformation,
    ) -> Result<(), String> {
        if source.len() != mapped.len()
            || source.len() > MAX_VERTICES
            || current_coords.len() != BODY_VERTICES
        {
            return Err("sleeve cage has inconsistent or excessive garment streams".into());
        }
        if source
            .iter()
            .chain(mapped.iter())
            .any(|p| !Vec3::from(*p).is_finite())
        {
            return Err("sleeve cage garment coordinates are non-finite".into());
        }
        let neutral = field.neutral_positions();
        let regions = field.regions();
        if neutral.len() != BODY_VERTICES || regions.len() != BODY_VERTICES {
            return Err("sleeve cage requires the canonical native body".into());
        }
        let mut selected = vec![None; source.len()];
        for (arm_id, arm) in self.arms.iter().enumerate() {
            for &i in &arm.vertices {
                selected[i] = Some(arm_id);
            }
        }
        // A source-position field cannot give two results to coincident UV aliases.
        let mut selected_weights = vec![0.; source.len()];
        for arm in &self.arms {
            for (&index, &weight) in arm.vertices.iter().zip(&arm.weights) {
                selected_weights[index] = weight;
            }
        }
        let mut aliases = std::collections::HashMap::<[u32; 3], (Option<usize>, u32)>::new();
        for (i, p) in source.iter().enumerate() {
            let key = p.map(|v| if v == 0. { 0 } else { v.to_bits() });
            let value = (selected[i], selected_weights[i].to_bits());
            match aliases.insert(key, value) {
                Some(previous) if previous != value => {
                    return Err(
                        "fitSleeveCage aliases must have identical selection and vertexWeights"
                            .into(),
                    );
                }
                _ => {}
            }
        }
        let current: Vec<Vec3> = current_coords.iter().copied().map(Vec3::from).collect();
        if current
            .iter()
            .any(|p| !p.is_finite() || p.abs().max_element() > 100.)
        {
            return Err(
                "sleeve cage current body coordinates are outside the finite domain".into(),
            );
        }
        let mut maps = Vec::with_capacity(self.arms.len());
        for arm in &self.arms {
            let neutral_centres = [
                ring_center(neutral, regions, &arm.shoulder)?,
                ring_center(neutral, regions, &arm.elbow)?,
                ring_center(neutral, regions, &arm.wrist)?,
            ];
            if neutral_centres
                .iter()
                .any(|center| center.x.signum() != neutral_centres[0].x.signum())
            {
                return Err("fitSleeveCage arm anchor rings must lie on one body side".into());
            }
            if !(neutral_centres[0].x.abs() < neutral_centres[1].x.abs()
                && neutral_centres[1].x.abs() < neutral_centres[2].x.abs())
            {
                return Err(
                    "fitSleeveCage neutral rings must progress shoulder, elbow, wrist".into(),
                );
            }
            let current_centres = [
                ring_center(&current, regions, &arm.shoulder)?,
                ring_center(&current, regions, &arm.elbow)?,
                ring_center(&current, regions, &arm.wrist)?,
            ];
            let map = ArmMap {
                arm,
                upper: segment(
                    neutral_centres[0],
                    neutral_centres[1],
                    current_centres[0],
                    current_centres[1],
                    &arm.shoulder,
                    &arm.elbow,
                    neutral,
                    &current,
                    arm.radial_mode,
                )?,
                lower: segment(
                    neutral_centres[1],
                    neutral_centres[2],
                    current_centres[1],
                    current_centres[2],
                    &arm.elbow,
                    &arm.wrist,
                    neutral,
                    &current,
                    arm.radial_mode,
                )?,
            };
            if map.upper.axis.dot(map.lower.axis) <= -0.95 {
                return Err("fitSleeveCage neutral arm segments are near-antiparallel".into());
            }
            maps.push(map);
        }
        // Stage every output before mutation, including validation of both arms.
        let mut updates = Vec::new();
        for map in maps {
            let bisector = (map.upper.axis + map.lower.axis).normalize();
            for (&index, &vertex_weight) in map.arm.vertices.iter().zip(&map.arm.weights) {
                if vertex_weight == 0. {
                    continue;
                }
                let p = Vec3::from(source[index]);
                let legacy = Vec3::from(mapped[index]);
                if !p.is_finite() || !legacy.is_finite() {
                    return Err("sleeve cage has non-finite garment coordinates".into());
                }
                let along = (p - map.upper.from).dot(map.upper.axis);
                let root = smoothstep(along / map.arm.root_blend) * vertex_weight;
                if root == 0. {
                    continue;
                }
                let pu = map_segment(p, map.upper);
                let pl = map_segment(p, map.lower);
                // One plane gives a continuous field throughout the puff's
                // volume. Choosing the nearest segment jumps off the axis.
                let elbow = smoothstep(
                    ((p - map.lower.from).dot(bisector) + map.arm.elbow_blend * 0.5)
                        / map.arm.elbow_blend,
                );
                let cage = pu.lerp(pl, elbow);
                let output = legacy.lerp(cage, root);
                if !output.is_finite() || output.abs().max_element() > 100. {
                    return Err(
                        "sleeve cage produced a garment coordinate outside its domain".into(),
                    );
                }
                updates.push((index, output.to_array()));
            }
        }
        for (index, output) in updates {
            mapped[index] = output;
        }
        Ok(())
    }
}

fn parse_indices(
    object: &serde_json::Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
    bound: usize,
    label: &str,
) -> Result<Vec<usize>, String> {
    let values = object
        .get(key)
        .and_then(Value::as_array)
        .filter(|v| (min..=max).contains(&v.len()))
        .ok_or_else(|| format!("{label} {key} has invalid length"))?;
    let mut out = Vec::with_capacity(values.len());
    let mut set = HashSet::new();
    for value in values {
        let n = value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n < bound)
            .ok_or_else(|| format!("{label} {key} has invalid index"))?;
        if !set.insert(n) {
            return Err(format!("{label} {key} repeats an index"));
        };
        out.push(n);
    }
    Ok(out)
}
fn parse_weights(
    object: &serde_json::Map<String, Value>,
    count: usize,
) -> Result<Vec<f32>, String> {
    let Some(values) = object.get("vertexWeights") else {
        return Ok(vec![1.; count]);
    };
    let values = values
        .as_array()
        .filter(|v| v.len() == count)
        .ok_or("fitSleeveCage vertexWeights must match vertexIndices")?;
    values
        .iter()
        .map(|value| {
            value
                .as_f64()
                .map(|v| v as f32)
                .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
                .map(|v| if v == 0. { 0. } else { v })
                .ok_or_else(|| "fitSleeveCage vertexWeights must be finite within 0..1".to_owned())
        })
        .collect()
}
fn parse_ring(object: &serde_json::Map<String, Value>, key: &str) -> Result<Vec<usize>, String> {
    parse_indices(object, key, 3, 64, BODY_VERTICES, "fitSleeveCage")
}
fn parse_blend(object: &serde_json::Map<String, Value>, key: &str) -> Result<f32, String> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .filter(|v| v.is_finite() && (0.05..=1.).contains(v))
        .ok_or_else(|| format!("fitSleeveCage {key} must be within 0.05..1"))
}
fn parse_radial_mode(object: &serde_json::Map<String, Value>) -> Result<RadialMode, String> {
    match object.get("radialMode") {
        None => Ok(RadialMode::BodyRatio),
        Some(Value::String(mode)) if mode == "preserve-ease" => Ok(RadialMode::PreserveEase),
        _ => Err("fitSleeveCage radialMode must be \"preserve-ease\" when present".into()),
    }
}
fn ring_center(points: &[Vec3], regions: &[u8], ring: &[usize]) -> Result<Vec3, String> {
    if ring.iter().any(|&i| !matches!(regions[i], 4 | 5 | 6)) {
        return Err("fitSleeveCage anchors must use native arm regions 4, 5, or 6".into());
    }
    let sign = points[ring[0]].x.signum();
    if sign == 0. || ring.iter().any(|&i| points[i].x.signum() != sign) {
        return Err("fitSleeveCage anchor ring crosses body sides".into());
    }
    let c = ring.iter().map(|&i| points[i]).sum::<Vec3>() / ring.len() as f32;
    if !c.is_finite() {
        Err("fitSleeveCage anchor ring is non-finite".into())
    } else {
        Ok(c)
    }
}
fn segment(
    from: Vec3,
    to: Vec3,
    current_from: Vec3,
    current_to: Vec3,
    a: &[usize],
    b: &[usize],
    neutral: &[Vec3],
    current: &[Vec3],
    radial_mode: RadialMode,
) -> Result<Segment, String> {
    let d = to - from;
    let length = d.length();
    let cd = current_to - current_from;
    let clength = cd.length();
    if length < 1e-4 || clength < 1e-4 {
        return Err("fitSleeveCage has a zero-length arm segment".into());
    };
    let axis = d / length;
    let caxis = cd / clength;
    let radius = |points: &[Vec3], center: Vec3, axis: Vec3, ring: &[usize]| {
        ring.iter()
            .map(|&i| {
                let x = points[i] - center;
                (x - axis * x.dot(axis)).length()
            })
            .sum::<f32>()
            / ring.len() as f32
    };
    let r = (radius(neutral, from, axis, a) + radius(neutral, to, axis, b)) * 0.5;
    let cr =
        (radius(current, current_from, caxis, a) + radius(current, current_to, caxis, b)) * 0.5;
    let radial = cr / r;
    let longitudinal = clength / length;
    if !radial.is_finite()
        || !longitudinal.is_finite()
        || !(MIN_SCALE..=MAX_SCALE).contains(&radial)
        || !(MIN_SCALE..=MAX_SCALE).contains(&longitudinal)
    {
        return Err("fitSleeveCage segment scale is outside 0.125..8".into());
    }
    Ok(Segment {
        from,
        axis,
        to: current_from,
        rotation: Quat::from_rotation_arc(axis, caxis),
        longitudinal_scale: longitudinal,
        radial_scale: radial,
        neutral_radius: r,
        current_radius: cr,
        radial_mode,
    })
}
fn map_segment(p: Vec3, s: Segment) -> Vec3 {
    let o = p - s.from;
    let longitudinal = s.axis * o.dot(s.axis) * s.longitudinal_scale;
    let radial = o - s.axis * o.dot(s.axis);
    let radial = match s.radial_mode {
        RadialMode::BodyRatio => radial * s.radial_scale,
        RadialMode::PreserveEase => {
            let length = radial.length();
            if length == 0. {
                radial
            } else {
                // Close-fitting cloth follows the local body ratio. Past the
                // neutral body radius, retain the authored clearance in DM.
                let mapped_length = if length <= s.neutral_radius {
                    length * s.radial_scale
                } else {
                    length + s.current_radius - s.neutral_radius
                };
                radial * (mapped_length.max(0.) / length)
            }
        }
    };
    s.to + s.rotation * (longitudinal + radial)
}
fn smoothstep(v: f32) -> f32 {
    let t = v.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn arm(indices: &[usize]) -> Value {
        json!({"vertexIndices":indices,"shoulderRefs":[0,1,2],"elbowRefs":[3,4,5],"wristRefs":[6,7,8],"rootBlendDM":0.1,"elbowBlendDM":0.2})
    }
    fn metadata(indices: &[usize]) -> Value {
        json!({"fitSleeveCage":{"arms":[arm(indices)]}})
    }
    fn body(translate: Vec3) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u8>) {
        let mut n = vec![[0.; 3]; BODY_VERTICES];
        let mut r = vec![0; BODY_VERTICES];
        for (base, center, region) in [
            (0, Vec3::new(1., 4., 0.), 4),
            (3, Vec3::new(2., 3., 0.), 5),
            (6, Vec3::new(3., 2., 0.), 6),
        ] {
            for (o, delta) in [
                Vec3::new(0., 0.1, 0.),
                Vec3::new(0., -0.05, 0.0866),
                Vec3::new(0., -0.05, -0.0866),
            ]
            .into_iter()
            .enumerate()
            {
                n[base + o] = (center + delta).to_array();
                r[base + o] = region;
            }
        }
        let c = n
            .iter()
            .map(|p| (Vec3::from(*p) + translate).to_array())
            .collect();
        (n, c, r)
    }
    #[test]
    fn parser_rejects_overlap_and_bad_metadata() {
        assert!(
            SleeveCage::parse(&metadata(&[0]), true, 2)
                .unwrap()
                .is_some()
        );
        let mut overlap = metadata(&[0]);
        overlap["fitSleeveCage"]["arms"]
            .as_array_mut()
            .unwrap()
            .push(arm(&[0]));
        assert!(SleeveCage::parse(&overlap, true, 2).is_err());
        let mut bad = metadata(&[0]);
        bad["fitSleeveCage"]["arms"][0]["shoulderRefs"] = json!([0, 0, 1]);
        assert!(SleeveCage::parse(&bad, true, 2).is_err());
        assert!(SleeveCage::parse(&metadata(&[0]), false, 2).is_err());
        let mut wrong_weights = metadata(&[0]);
        wrong_weights["fitSleeveCage"]["arms"][0]["vertexWeights"] = json!([0.5, 0.5]);
        assert!(SleeveCage::parse(&wrong_weights, true, 2).is_err());
        let mut invalid_mode = metadata(&[0]);
        invalid_mode["fitSleeveCage"]["arms"][0]["radialMode"] = json!("ratio");
        assert!(SleeveCage::parse(&invalid_mode, true, 2).is_err());
    }
    #[test]
    fn cage_translates_selected_aliases_and_leaves_unselected_exact() {
        let cage = SleeveCage::parse(&metadata(&[0, 1]), true, 3)
            .unwrap()
            .unwrap();
        let (neutral, current, regions) = body(Vec3::Y);
        let field = BodyDeformation::new(&neutral, &current, &[], &regions).unwrap();
        let source = [[2., 3., 0.1], [2., 3., 0.1], [0., 0., 0.]];
        let mut mapped = source;
        cage.fit(&source, &mut mapped, &current, &field).unwrap();
        assert!((Vec3::from(mapped[0]) - Vec3::new(2., 4., 0.1)).length() < 1e-4);
        assert_eq!(mapped[0], mapped[1]);
        assert_eq!(mapped[2], source[2]);
        let mut zero = metadata(&[0, 1]);
        zero["fitSleeveCage"]["arms"][0]["vertexWeights"] = json!([0.0, 0.0]);
        let zero = SleeveCage::parse(&zero, true, 3).unwrap().unwrap();
        let mut unchanged = source;
        zero.fit(&source, &mut unchanged, &current, &field).unwrap();
        assert_eq!(unchanged, source);
    }
    #[test]
    fn cage_rejects_incomplete_aliases_invalid_regions_and_zero_axes() {
        let cage = SleeveCage::parse(&metadata(&[0]), true, 2)
            .unwrap()
            .unwrap();
        let (neutral, current, mut regions) = body(Vec3::ZERO);
        let field = BodyDeformation::new(&neutral, &current, &[], &regions).unwrap();
        let source = [[2., 3., 0.1], [2., 3., 0.1]];
        let mut mapped = source;
        assert!(cage.fit(&source, &mut mapped, &current, &field).is_err());
        regions[0] = 1;
        let bad_regions = BodyDeformation::new(&neutral, &current, &[], &regions).unwrap();
        let cage = SleeveCage::parse(&metadata(&[0, 1]), true, 2)
            .unwrap()
            .unwrap();
        assert!(
            cage.fit(&source, &mut mapped, &current, &bad_regions)
                .is_err()
        );
        let (neutral, mut collapsed, regions) = body(Vec3::ZERO);
        for i in 3..9 {
            collapsed[i] = collapsed[0];
        }
        let field = BodyDeformation::new(&neutral, &collapsed, &[], &regions).unwrap();
        assert!(cage.fit(&source, &mut mapped, &collapsed, &field).is_err());
    }

    #[test]
    fn off_axis_elbow_bisector_blend_is_continuous() {
        let (mut neutral, _, regions) = body(Vec3::ZERO);
        for p in &mut neutral[6..9] {
            *p = (Vec3::from(*p) + Vec3::new(0., 0.7, 0.7)).to_array();
        }
        let mut current = neutral.clone();
        for p in &mut current[6..9] {
            *p = (Vec3::from(*p) + Vec3::new(0., 0.3, 0.8)).to_array();
        }
        let field = BodyDeformation::new(&neutral, &current, &[], &regions).unwrap();
        let upper = Vec3::new(1., -1., 0.).normalize();
        let lower = Vec3::new(1., -0.3, 0.7).normalize();
        let bisector = (upper + lower).normalize();
        let on_plane = Vec3::new(2., 3., 0.) + bisector.cross(Vec3::Y).normalize() * 0.5;
        let source = [
            (on_plane - bisector * 1e-5).to_array(),
            (on_plane + bisector * 1e-5).to_array(),
        ];
        let cage = SleeveCage::parse(&metadata(&[0, 1]), true, 2)
            .unwrap()
            .unwrap();
        let mut mapped = source;
        cage.fit(&source, &mut mapped, &current, &field).unwrap();
        assert!((Vec3::from(mapped[0]) - Vec3::from(mapped[1])).length() < 1e-3);
        assert!((Vec3::from(mapped[0]) - Vec3::from(source[0])).length() > 0.01);
    }

    #[test]
    fn aliases_cannot_cross_arm_maps_or_hide_behind_signed_zero() {
        let (neutral, current, regions) = body(Vec3::Y);
        let field = BodyDeformation::new(&neutral, &current, &[], &regions).unwrap();
        let source = [[2., 3., 0.], [2., 3., -0.]];
        let single = SleeveCage::parse(&metadata(&[0]), true, 2)
            .unwrap()
            .unwrap();
        let mut mapped = source;
        assert!(single.fit(&source, &mut mapped, &current, &field).is_err());
        assert_eq!(mapped, source);
        let mut split = metadata(&[0]);
        let mut other = arm(&[1]);
        other["shoulderRefs"] = json!([3, 4, 5]);
        split["fitSleeveCage"]["arms"]
            .as_array_mut()
            .unwrap()
            .push(other);
        let split = SleeveCage::parse(&split, true, 2).unwrap().unwrap();
        assert!(split.fit(&source, &mut mapped, &current, &field).is_err());
        assert_eq!(mapped, source);
    }

    #[test]
    fn zero_weight_preserves_finite_legacy_points_outside_cage_domain() {
        let (neutral, current, regions) = body(Vec3::Y);
        let field = BodyDeformation::new(&neutral, &current, &[], &regions).unwrap();
        let mut meta = metadata(&[0]);
        meta["fitSleeveCage"]["arms"][0]["vertexWeights"] = json!([0.]);
        let cage = SleeveCage::parse(&meta, true, 1).unwrap().unwrap();
        let source = [[200., 300., 0.]];
        let mut mapped = [[201., 400., 2.]];
        let before = mapped;
        cage.fit(&source, &mut mapped, &current, &field).unwrap();
        assert_eq!(mapped, before);
    }

    #[test]
    fn segment_preserves_rotation_and_radial_scale() {
        let n = [
            Vec3::new(0., 0.1, 0.),
            Vec3::new(0., -0.05, 0.0866),
            Vec3::new(0., -0.05, -0.0866),
            Vec3::new(1., 0.1, 0.),
            Vec3::new(1., -0.05, 0.0866),
            Vec3::new(1., -0.05, -0.0866),
        ];
        let c = n.map(|p| Vec3::new(-p.y * 2., p.x * 2., p.z * 2.) + Vec3::Y);
        let seg = segment(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec3::Y + Vec3::Y * 2.,
            &[0, 1, 2],
            &[3, 4, 5],
            &n,
            &c,
            RadialMode::BodyRatio,
        )
        .unwrap();
        let out = map_segment(Vec3::new(0.5, 0., 0.1), seg);
        assert!((out - Vec3::new(0., 2., 0.2)).length() < 1e-3);
    }

    #[test]
    fn preserve_ease_follows_tight_cloth_and_keeps_loose_air_gap() {
        let n = [
            Vec3::new(0., 0.1, 0.),
            Vec3::new(0., -0.05, 0.0866),
            Vec3::new(0., -0.05, -0.0866),
            Vec3::new(1., 0.1, 0.),
            Vec3::new(1., -0.05, 0.0866),
            Vec3::new(1., -0.05, -0.0866),
        ];
        let c = n.map(|p| Vec3::new(p.x, p.y * 2., p.z * 2.));
        let preserve = segment(
            Vec3::ZERO,
            Vec3::X,
            Vec3::ZERO,
            Vec3::X,
            &[0, 1, 2],
            &[3, 4, 5],
            &n,
            &c,
            RadialMode::PreserveEase,
        )
        .unwrap();
        let ratio = segment(
            Vec3::ZERO,
            Vec3::X,
            Vec3::ZERO,
            Vec3::X,
            &[0, 1, 2],
            &[3, 4, 5],
            &n,
            &c,
            RadialMode::BodyRatio,
        )
        .unwrap();
        let tight = map_segment(Vec3::new(0.5, 0., 0.05), preserve);
        let loose = map_segment(Vec3::new(0.5, 0., 0.4), preserve);
        let legacy_loose = map_segment(Vec3::new(0.5, 0., 0.4), ratio);
        assert!((tight.z - 0.1).abs() < 1e-4, "tight cloth uses body ratio");
        assert!(
            (loose.z - 0.5).abs() < 1e-4,
            "loose cloth retains 0.3 DM ease"
        );
        assert!(
            (legacy_loose.z - 0.8).abs() < 1e-4,
            "absent mode remains ratio"
        );
        let at_radius = map_segment(Vec3::new(0.5, 0., 0.1), preserve);
        assert!(
            (at_radius.z - 0.2).abs() < 1e-4,
            "piecewise map is continuous"
        );
        assert!(map_segment(Vec3::new(0.5, 0., 0.), preserve).is_finite());
    }

    #[test]
    fn parsed_preserve_ease_maps_tight_and_loose_sleeve_points() {
        let (mut neutral, mut current, regions) = body(Vec3::ZERO);
        let radial_u = Vec3::new(1., 1., 0.).normalize();
        let radial_v = Vec3::Z;
        let offsets = [
            radial_u * 0.1,
            -radial_u * 0.05 + radial_v * 0.0075_f32.sqrt(),
            -radial_u * 0.05 - radial_v * 0.0075_f32.sqrt(),
        ];
        for (base, center) in [
            Vec3::new(1., 4., 0.),
            Vec3::new(2., 3., 0.),
            Vec3::new(3., 2., 0.),
        ]
        .into_iter()
        .enumerate()
        {
            for (offset, point) in offsets.into_iter().enumerate() {
                neutral[base * 3 + offset] = (center + point).to_array();
                current[base * 3 + offset] = (center + point * 2.).to_array();
            }
        }
        let field = BodyDeformation::new(&neutral, &current, &[], &regions).unwrap();
        let mut material = metadata(&[0, 1]);
        material["fitSleeveCage"]["arms"][0]["radialMode"] = json!("preserve-ease");
        let cage = SleeveCage::parse(&material, true, 2).unwrap().unwrap();
        let source = [[1.5, 3.5, 0.05], [1.5, 3.5, 0.4]];
        let mut mapped = source;
        cage.fit(&source, &mut mapped, &current, &field).unwrap();
        assert!(
            (mapped[0][2] - 0.1).abs() < 1e-4,
            "tight point follows radius"
        );
        assert!(
            (mapped[1][2] - 0.5).abs() < 1e-4,
            "loose point retains 0.3 DM air gap"
        );
    }
}
