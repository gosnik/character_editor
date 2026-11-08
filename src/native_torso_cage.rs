//! Opt-in source-space torso cage, derived from matched anatomical cross sections.
use bevy::prelude::{Vec2, Vec3};
use serde_json::Value;

pub struct TorsoCage {
    levels: Vec<f32>,
    half_band: f32,
    current_height_sections: bool,
    convex_sections: bool,
}

struct Slice {
    neutral: Vec3,
    current: Vec3,
    scale_x: f32,
    scale_z: f32,
    profiles: Option<(Vec<Vec2>, Vec<Vec2>)>,
}

impl TorsoCage {
    pub fn parse(material: &Value, allowed_slot: bool) -> Result<Option<Self>, String> {
        let Some(value) = material.get("fitTorsoCage") else {
            return Ok(None);
        };
        if !allowed_slot {
            return Err("fitTorsoCage is only valid for top or jacket garments".into());
        }
        if material.get("fitCoherentBodyAnchor").is_some()
            || material.get("fitDisplacementSmoothingDM").is_some()
        {
            return Err("fitTorsoCage cannot be combined with coherent body anchors or displacement smoothing".into());
        }
        let object = value.as_object().ok_or("fitTorsoCage must be an object")?;
        let levels = object
            .get("levelsYDM")
            .and_then(Value::as_array)
            .filter(|v| (2..=16).contains(&v.len()))
            .ok_or("fitTorsoCage needs 2..16 levelsYDM")?
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|v| v as f32)
                    .filter(|v| v.is_finite() && v.abs() <= 100.)
                    .ok_or("fitTorsoCage has invalid levelsYDM")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if levels.windows(2).any(|v| v[1] <= v[0]) {
            return Err("fitTorsoCage levelsYDM must be strictly increasing".into());
        }
        let half_band = object
            .get("halfBandDM")
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .filter(|v| v.is_finite() && (0.05..=1.).contains(v))
            .ok_or("fitTorsoCage halfBandDM must be within 0.05..1")?;
        let current_height_sections = object
            .get("currentHeightSections")
            .map(|value| {
                value
                    .as_bool()
                    .ok_or("currentHeightSections must be a boolean")
            })
            .transpose()?
            .unwrap_or(false);
        let convex_sections = object
            .get("convexSections")
            .map(|value| value.as_bool().ok_or("convexSections must be a boolean"))
            .transpose()?
            .unwrap_or(false);
        if convex_sections && !current_height_sections {
            return Err("convexSections requires currentHeightSections".into());
        }
        Ok(Some(Self {
            levels,
            half_band,
            current_height_sections,
            convex_sections,
        }))
    }

    pub fn fit(
        &self,
        source: &[[f32; 3]],
        neutral: &[Vec3],
        current: &[[f32; 3]],
        regions: &[u8],
    ) -> Result<Vec<[f32; 3]>, String> {
        if neutral.is_empty()
            || neutral.len() != current.len()
            || neutral.len() != regions.len()
            || neutral.len() > 100_000
            || source.len() > 100_000
        {
            return Err("torso cage has inconsistent or excessive input streams".into());
        }
        let valid = |p: Vec3| p.is_finite() && p.abs().max_element() <= 100.;
        if neutral.iter().any(|p| !valid(*p)) || current.iter().any(|p| !valid(Vec3::from(*p))) {
            return Err("torso cage body coordinates are outside the finite domain".into());
        }
        let current_points: Vec<Vec3> = current.iter().copied().map(Vec3::from).collect();
        let section_scale = if self.current_height_sections {
            let (from_low, from_high) = torso_bounds(neutral, regions)?;
            let (to_low, to_high) = torso_bounds(&current_points, regions)?;
            let scale = (to_high - to_low) / (from_high - from_low);
            if !scale.is_finite()
                || !(0.125..=8.).contains(&scale.x)
                || !(0.125..=8.).contains(&scale.y)
            {
                return Err("torso cage section sampling exceeds the positive scale domain".into());
            }
            scale
        } else {
            Vec3::ONE
        };
        let mut slices = Vec::with_capacity(self.levels.len());
        for &level in &self.levels {
            let mut count = 0;
            let (mut from, mut to) = (Vec3::ZERO, Vec3::ZERO);
            let (mut from_low, mut to_low) =
                (Vec3::splat(f32::INFINITY), Vec3::splat(f32::INFINITY));
            let (mut from_high, mut to_high) = (
                Vec3::splat(f32::NEG_INFINITY),
                Vec3::splat(f32::NEG_INFINITY),
            );
            // Start with the torso regions. Region 4 also contains the arms, but
            // its vertices immediately outside this section's torso width are the
            // shoulder roots and must contribute to the shoulder silhouette.
            let torso_half_width = neutral
                .iter()
                .enumerate()
                .filter(|(i, p)| {
                    (1..=3).contains(&regions[*i]) && (p.y - level).abs() <= self.half_band
                })
                .map(|(_, p)| p.x.abs())
                .reduce(f32::max);
            for (i, &p) in neutral.iter().enumerate() {
                if (p.y - level).abs() > self.half_band {
                    continue;
                }
                let torso = (1..=3).contains(&regions[i]);
                let shoulder_root = regions[i] == 4
                    && torso_half_width.is_some_and(|width| p.x.abs() <= width + 0.25);
                if !torso && !shoulder_root {
                    continue;
                }
                let q = Vec3::from(current[i]);
                from += p;
                to += q;
                count += 1;
                from_low = from_low.min(p);
                from_high = from_high.max(p);
                to_low = to_low.min(q);
                to_high = to_high.max(q);
            }
            if count < 8 {
                return Err(format!(
                    "torso cage level {level} has fewer than eight anatomical samples"
                ));
            }
            let mut section_points = None;
            if self.current_height_sections {
                // A breast/chest morph can move vertices between height bands.
                // Sample the final surface at the mapped height instead of
                // assuming the original band's vertices still bound it there.
                // Reselect both sections: neutral/translation/axis-affine input
                // must still reproduce the authored shape exactly.
                let from_points = anatomical_section(
                    neutral,
                    regions,
                    from.y / count as f32,
                    self.half_band,
                    0.25,
                )?;
                let to_points = anatomical_section(
                    &current_points,
                    regions,
                    to.y / count as f32,
                    self.half_band * section_scale.y,
                    0.25 * section_scale.x,
                )?;
                (from_low, from_high) = point_bounds(&from_points);
                (to_low, to_high) = point_bounds(&to_points);
                section_points = Some((from_points, to_points));
            }
            let from_extent = from_high - from_low;
            let to_extent = to_high - to_low;
            if from_extent.x < 1e-4 || from_extent.z < 1e-4 {
                return Err("torso cage has a degenerate neutral section".into());
            }
            let (scale_x, scale_z) = (to_extent.x / from_extent.x, to_extent.z / from_extent.z);
            if ![scale_x, scale_z]
                .iter()
                .all(|s| s.is_finite() && (0.125..=8.).contains(s))
            {
                return Err("torso cage section exceeds the positive scale domain".into());
            }
            // Match the actual section extrema, rather than a vertex-density
            // weighted centroid: asymmetric chest morphs must not shift the
            // fitted back surface or leave the new front extent uncovered.
            let from_mean = from / count as f32;
            let to_mean = to / count as f32;
            let from_centre = (from_low + from_high) * 0.5;
            let to_centre = (to_low + to_high) * 0.5;
            let profiles = if self.convex_sections {
                let (from_points, to_points) =
                    section_points.ok_or("convex sections require anatomical section samples")?;
                Some((
                    convex_profile(
                        from_points
                            .iter()
                            .map(|p| {
                                Vec2::new(
                                    (p.x - from_centre.x) * scale_x,
                                    (p.z - from_centre.z) * scale_z,
                                )
                            })
                            .collect(),
                    )?,
                    convex_profile(
                        to_points
                            .iter()
                            .map(|p| Vec2::new(p.x - to_centre.x, p.z - to_centre.z))
                            .collect(),
                    )?,
                ))
            } else {
                None
            };
            slices.push(Slice {
                neutral: Vec3::new(from_centre.x, from_mean.y, from_centre.z),
                current: Vec3::new(to_centre.x, to_mean.y, to_centre.z),
                scale_x,
                scale_z,
                profiles,
            });
        }
        for pair in slices.windows(2) {
            let span = pair[1].neutral.y - pair[0].neutral.y;
            let mapped_span = pair[1].current.y - pair[0].current.y;
            if span < 1e-4 || !(0.125..=8.).contains(&(mapped_span / span)) {
                return Err(
                    "torso cage section heights overlap, reverse, or exceed the scale domain"
                        .into(),
                );
            }
        }
        source
            .iter()
            .map(|&p| {
                let p = Vec3::from(p);
                if !valid(p) {
                    return Err("torso cage source point is outside the finite domain".into());
                }
                let i = slices
                    .partition_point(|s| s.neutral.y <= p.y)
                    .saturating_sub(1)
                    .min(slices.len() - 2);
                let (a, b) = (&slices[i], &slices[i + 1]);
                let t = (p.y - a.neutral.y) / (b.neutral.y - a.neutral.y);
                let blend = t.clamp(0., 1.);
                let blend = blend * blend * (3. - 2. * blend);
                let map = |s: &Slice| -> Result<Vec2, String> {
                    let mut offset = Vec2::new(
                        (p.x - s.neutral.x) * s.scale_x,
                        (p.z - s.neutral.z) * s.scale_z,
                    );
                    if let Some((neutral_profile, current_profile)) = &s.profiles {
                        let length = offset.length();
                        if length > 1e-6 {
                            let direction = offset / length;
                            let ratio = profile_radius(current_profile, direction)?
                                / profile_radius(neutral_profile, direction)?;
                            if !ratio.is_finite() || !(0.125..=8.).contains(&ratio) {
                                return Err(
                                    "torso cage radial scale exceeds the positive domain".into()
                                );
                            }
                            offset *= ratio;
                        }
                    }
                    Ok(Vec2::new(s.current.x, s.current.z) + offset)
                };
                let pa = map(a)?;
                let pb = map(b)?;
                let result = Vec3::new(
                    pa.x + (pb.x - pa.x) * blend,
                    a.current.y + (b.current.y - a.current.y) * t,
                    pa.y + (pb.y - pa.y) * blend,
                );
                if !valid(result) {
                    return Err("torso cage produced an invalid point".into());
                }
                Ok(result.to_array())
            })
            .collect()
    }
}

fn torso_bounds(points: &[Vec3], regions: &[u8]) -> Result<(Vec3, Vec3), String> {
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for (&point, &region) in points.iter().zip(regions) {
        if (1..=3).contains(&region) {
            low = low.min(point);
            high = high.max(point);
        }
    }
    if !low.is_finite() || !high.is_finite() || (high - low).min_element() < 1e-4 {
        return Err("torso cage has degenerate anatomical bounds".into());
    }
    Ok((low, high))
}

fn anatomical_section(
    points: &[Vec3],
    regions: &[u8],
    height: f32,
    half_band: f32,
    shoulder_padding: f32,
) -> Result<Vec<Vec3>, String> {
    let mut torso_low = f32::INFINITY;
    let mut torso_high = f32::NEG_INFINITY;
    for (&point, &region) in points.iter().zip(regions) {
        if (1..=3).contains(&region) && (point.y - height).abs() <= half_band {
            torso_low = torso_low.min(point.x);
            torso_high = torso_high.max(point.x);
        }
    }
    if !torso_low.is_finite() || !torso_high.is_finite() {
        return Err("torso cage has no anatomical samples at the mapped height".into());
    }
    let centre = (torso_low + torso_high) * 0.5;
    let half_width = (torso_high - torso_low) * 0.5;
    let mut samples = Vec::new();
    for (&point, &region) in points.iter().zip(regions) {
        if (point.y - height).abs() <= half_band
            && ((1..=3).contains(&region)
                || (region == 4 && (point.x - centre).abs() <= half_width + shoulder_padding))
        {
            samples.push(point);
        }
    }
    if samples.len() < 8 {
        return Err("torso cage mapped section has fewer than eight anatomical samples".into());
    }
    Ok(samples)
}

fn point_bounds(points: &[Vec3]) -> (Vec3, Vec3) {
    points.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(low, high), &p| (low.min(p), high.max(p)),
    )
}

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

fn convex_profile(mut points: Vec<Vec2>) -> Result<Vec<Vec2>, String> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    points.dedup();
    let mut lower: Vec<Vec2> = Vec::new();
    let mut upper: Vec<Vec2> = Vec::new();
    for &p in &points {
        while lower.len() >= 2
            && cross(
                lower[lower.len() - 1] - lower[lower.len() - 2],
                p - lower[lower.len() - 1],
            ) <= 0.
        {
            lower.pop();
        }
        lower.push(p);
    }
    for &p in points.iter().rev() {
        while upper.len() >= 2
            && cross(
                upper[upper.len() - 1] - upper[upper.len() - 2],
                p - upper[upper.len() - 1],
            ) <= 0.
        {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    if !(3..=256).contains(&lower.len())
        || lower
            .iter()
            .zip(lower.iter().cycle().skip(1))
            .any(|(&a, &b)| cross(b - a, -a) <= 1e-6)
    {
        return Err("torso cage radial profile is degenerate or excludes its centre".into());
    }
    Ok(lower)
}

fn profile_radius(profile: &[Vec2], direction: Vec2) -> Result<f32, String> {
    let mut radius = f32::INFINITY;
    for (&a, &b) in profile.iter().zip(profile.iter().cycle().skip(1)) {
        let edge = b - a;
        let denominator = cross(direction, edge);
        if denominator.abs() <= 1e-8 {
            continue;
        }
        let t = cross(a, edge) / denominator;
        let u = cross(a, direction) / denominator;
        if t > 1e-5 && (-1e-5..=1.00001).contains(&u) {
            radius = radius.min(t);
        }
    }
    if !radius.is_finite() {
        return Err("torso cage radial profile has no positive intersection".into());
    }
    Ok(radius)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body() -> Vec<Vec3> {
        [0., 1., 2.]
            .into_iter()
            .flat_map(|y| {
                (0..8).map(move |i| {
                    let angle = i as f32 * std::f32::consts::TAU / 8.;
                    Vec3::new(angle.cos(), y, angle.sin())
                })
            })
            .collect()
    }
    fn cage() -> TorsoCage {
        TorsoCage::parse(
            &json!({"fitTorsoCage":{"levelsYDM":[0,1,2],"halfBandDM":0.1}}),
            true,
        )
        .unwrap()
        .unwrap()
    }
    fn current_height_cage() -> TorsoCage {
        TorsoCage::parse(
            &json!({"fitTorsoCage":{"levelsYDM":[0,1,2],"halfBandDM":0.2,
                "currentHeightSections":true}}),
            true,
        )
        .unwrap()
        .unwrap()
    }
    fn convex_cage() -> TorsoCage {
        TorsoCage::parse(
            &json!({"fitTorsoCage":{
                "levelsYDM":[0,1,2],"halfBandDM":0.2,
                "currentHeightSections":true,"convexSections":true
            }}),
            true,
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn convex_sections_preserve_axis_affine_and_translation() {
        let neutral = body();
        let scale = Vec3::new(1.5, 0.8, 2.0);
        let shift = Vec3::new(2.0, -3.0, 4.0);
        let current: Vec<_> = neutral
            .iter()
            .map(|p| (*p * scale + shift).to_array())
            .collect();
        let source = [[0.6, 0.5, 1.1], [-0.4, -0.2, -0.7], [0.1, 2.2, 0.4]];
        let fitted = convex_cage()
            .fit(&source, &neutral, &current, &[2; 24])
            .unwrap();
        for (p, q) in source.iter().zip(fitted) {
            assert!((Vec3::from(q) - (Vec3::from(*p) * scale + shift)).length() < 1e-5);
        }
    }
    #[test]
    fn convex_sections_follow_changed_contour_with_unchanged_axis_bounds() {
        let diamond = [
            [1., 0.],
            [0.5, 0.5],
            [0., 1.],
            [-0.5, 0.5],
            [-1., 0.],
            [-0.5, -0.5],
            [0., -1.],
            [0.5, -0.5],
        ];
        let square = [
            [1., 0.],
            [1., 1.],
            [0., 1.],
            [-1., 1.],
            [-1., 0.],
            [-1., -1.],
            [0., -1.],
            [1., -1.],
        ];
        let neutral: Vec<_> = [0., 1., 2.]
            .iter()
            .flat_map(|&y| diamond.iter().map(move |p| Vec3::new(p[0], y, p[1])))
            .collect();
        let current: Vec<_> = [0., 1., 2.]
            .iter()
            .flat_map(|&y| square.iter().map(move |p| [p[0], y, p[1]]))
            .collect();
        let source = [[0.55, 1., 0.55]];
        let axis = current_height_cage()
            .fit(&source, &neutral, &current, &[2; 24])
            .unwrap();
        let fitted = convex_cage()
            .fit(&source, &neutral, &current, &[2; 24])
            .unwrap();
        assert!((axis[0][0] - 0.55).abs() < 1e-5);
        assert!((fitted[0][0] - 1.1).abs() < 1e-5 && (fitted[0][2] - 1.1).abs() < 1e-5);
        assert!(
            TorsoCage::parse(
                &json!({"fitTorsoCage":{
                    "levelsYDM":[0,1],"halfBandDM":0.2,"convexSections":true
                }}),
                true
            )
            .is_err()
        );
        assert!(
            convex_profile(vec![
                Vec2::new(1., 1.),
                Vec2::new(2., 1.),
                Vec2::new(2., 2.)
            ])
            .is_err()
        );
    }
    #[test]
    fn current_height_sections_preserve_neutral_and_axis_affine_with_translation() {
        let neutral = body();
        let source = [[0.3, -0.2, 0.7], [-0.6, 0.6, 1.2], [0.2, 2.3, -0.2]];
        for (scale, offset) in [
            (Vec3::ONE, Vec3::ZERO),
            (Vec3::new(1.5, 0.8, 2.0), Vec3::new(2.0, -3.0, 4.0)),
        ] {
            let current: Vec<_> = neutral
                .iter()
                .map(|p| (*p * scale + offset).to_array())
                .collect();
            let fitted = current_height_cage()
                .fit(&source, &neutral, &current, &[2; 24])
                .unwrap();
            for (p, q) in source.iter().zip(fitted) {
                assert!((Vec3::from(q) - (Vec3::from(*p) * scale + offset)).length() < 1e-5);
            }
        }
    }
    #[test]
    fn current_height_sections_include_chest_vertices_that_move_between_bands() {
        let mut neutral = body();
        neutral.push(neutral[16]);
        let mut current: Vec<_> = neutral.iter().map(|p| p.to_array()).collect();
        // A chest vertex originally above the middle section moves down and
        // projects forwards. It must bound the surface at its final height.
        current[18] = [0.0, 1.0, 2.0];
        let source = [[0.0, 1.0, 1.1]];
        let old = cage().fit(&source, &neutral, &current, &[2; 25]).unwrap();
        let fitted = current_height_cage()
            .fit(&source, &neutral, &current, &[2; 25])
            .unwrap();
        assert!(old[0][2] < 2.0);
        assert!(fitted[0][2] > 2.0);
        assert!((fitted[0][1] - 1.0).abs() < 1e-5);
    }
    #[test]
    fn current_height_sections_reject_non_boolean_and_missing_final_section() {
        assert!(
            TorsoCage::parse(
                &json!({"fitTorsoCage":{
                    "levelsYDM":[0,1,2],"halfBandDM":0.1,"currentHeightSections":"true"
                }}),
                true
            )
            .is_err()
        );
        let neutral = body();
        let mut current: Vec<_> = neutral.iter().map(|p| p.to_array()).collect();
        // A separated middle band has an empty mean-height section.
        for (i, point) in current[8..16].iter_mut().enumerate() {
            point[1] = if i % 2 == 0 { 0.7 } else { 1.3 };
        }
        assert!(
            current_height_cage()
                .fit(&[[0.0, 1.0, 1.1]], &neutral, &current, &[2; 24])
                .is_err()
        );
    }
    #[test]
    fn reproduces_neutral_translation_and_axis_affine_including_extrapolation() {
        let body = body();
        let source = [
            [0.3, -0.2, 0.7],
            [-0.6, 0.6, 1.2],
            [0.2, 1.4, -0.8],
            [0.4, 2.3, -0.2],
        ];
        for (scale, offset) in [
            (Vec3::ONE, Vec3::ZERO),
            (Vec3::ONE, Vec3::new(2., -3., 4.)),
            (Vec3::new(1.5, 0.8, 2.), Vec3::new(0.2, 0.3, -0.4)),
        ] {
            let current = body
                .iter()
                .map(|p| (*p * scale + offset).to_array())
                .collect::<Vec<_>>();
            let fitted = cage().fit(&source, &body, &current, &[2; 24]).unwrap();
            for (p, q) in source.iter().zip(fitted) {
                assert!((Vec3::from(q) - (Vec3::from(*p) * scale + offset)).length() < 1e-5);
            }
        }
    }
    #[test]
    fn nonlinear_sections_follow_anatomy_without_blending_authored_detail() {
        let body = body();
        let current = body
            .iter()
            .map(|p| {
                let mut q = *p;
                if p.y == 1. {
                    q.x *= 1.5;
                    q.z += 0.3;
                }
                q.to_array()
            })
            .collect::<Vec<_>>();
        let source = [
            [0.2, 1., 0.4],
            [0.3, 1., 0.4],
            [0.2, 0.99999, 0.4],
            [0.2, 1.00001, 0.4],
        ];
        let fitted = cage().fit(&source, &body, &current, &[2; 24]).unwrap();
        assert!((fitted[0][0] - 0.3).abs() < 1e-5);
        assert!((fitted[0][2] - 0.7).abs() < 1e-5);
        assert!((fitted[1][0] - fitted[0][0] - 0.15).abs() < 1e-5);
        assert!((Vec3::from(fitted[2]) - Vec3::from(fitted[3])).length() < 1e-4);
    }
    #[test]
    fn proximal_shoulder_roots_extend_the_section_but_far_arms_do_not() {
        let mut neutral = body();
        neutral.extend([Vec3::new(1.2, 1., 0.7), Vec3::new(3., 1., 0.)]);
        let mut current = neutral.clone();
        current[24] = Vec3::new(1.5, 1., 1.5);
        current[25] = Vec3::new(9., 1., 9.);
        let mut regions = vec![2; 24];
        regions.extend([4, 4]);

        let fitted = cage()
            .fit(
                &[[1.2, 1., 1.]],
                &neutral,
                &current.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
                &regions,
            )
            .unwrap();

        // The near shoulder root raises the middle ring's positive X and Z
        // extents. The far arm would instead produce an X/Z extent of 9.
        assert!((fitted[0][0] - 1.5).abs() < 1e-5);
        assert!((fitted[0][2] - 1.5).abs() < 1e-5);
    }

    #[test]
    fn asymmetric_depth_change_maps_both_section_extrema_exactly() {
        let body = body();
        let current = body
            .iter()
            .map(|p| {
                let mut q = *p;
                if p.y == 1. && p.z > 0.9 {
                    q.z += 0.5;
                }
                q.to_array()
            })
            .collect::<Vec<_>>();
        let result = cage()
            .fit(&[[0., 1., 1.], [0., 1., -1.]], &body, &current, &[2; 24])
            .unwrap();
        assert!((result[0][2] - 1.5).abs() < 1e-5);
        assert!((result[1][2] + 1.).abs() < 1e-5);
    }

    #[test]
    fn rejects_invalid_metadata_sparse_sections_and_reversed_height() {
        assert!(TorsoCage::parse(&json!({}), false).unwrap().is_none());
        for value in [
            json!(false),
            json!({"levelsYDM":[1,0],"halfBandDM":0.2}),
            json!({"levelsYDM":[0,1],"halfBandDM":0}),
            json!({"levelsYDM":[0,0.00000000001],"halfBandDM":0.2}),
        ] {
            let parsed = TorsoCage::parse(&json!({"fitTorsoCage":value}), true);
            if let Ok(Some(c)) = parsed {
                assert!(
                    c.fit(
                        &[[0.; 3]],
                        &body(),
                        &body().iter().map(|p| p.to_array()).collect::<Vec<_>>(),
                        &[2; 24]
                    )
                    .is_err()
                );
            }
        }
        let metadata = json!({"fitTorsoCage":{"levelsYDM":[0,1,2],"halfBandDM":0.1}});
        assert!(TorsoCage::parse(&metadata, false).is_err());
        let mut conflict = metadata;
        conflict["fitCoherentBodyAnchor"] = json!({});
        assert!(TorsoCage::parse(&conflict, true).is_err());
        let b = body();
        let reversed = b.iter().map(|p| [p.x, -p.y, p.z]).collect::<Vec<_>>();
        assert!(cage().fit(&[[0.; 3]], &b, &reversed, &[2; 24]).is_err());
        assert!(
            cage()
                .fit(
                    &[[0.; 3]],
                    &b,
                    &b.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
                    &[4; 24]
                )
                .is_err()
        );
    }
}
