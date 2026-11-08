//! Stable local deformation frames for body-bound custom garments.
//!
//! Garments store a neutral barycentric body anchor plus an authored offset.
//! This field transports that offset through the current body's local affine
//! deformation without changing the portable garment format.
use bevy::prelude::{Mat3, Vec3};

const MAX_OFFSET_SCALE: f32 = 8.0;
#[derive(Clone, Debug)]
pub struct BodyDeformation {
    frames: Vec<Mat3>,
    regional_frames: Vec<Mat3>,
    /// Regional fits split by mesh connectivity.  The two feet share a native
    /// region id but must not estimate a single affine scale from their gap.
    coherent_frames: Vec<Mat3>,
    coherent_components: Vec<usize>,
    edge_scales: Vec<f32>,
    regions: Vec<u8>,
    neutral_points: Vec<Vec3>,
    translation_only: bool,
}

impl BodyDeformation {
    pub fn new(
        neutral: &[[f32; 3]],
        current: &[[f32; 3]],
        faces: &[u32],
        regions: &[u8],
    ) -> Result<Self, String> {
        if neutral.is_empty() || neutral.len() != current.len() || regions.len() != neutral.len() {
            return Err("body deformation arrays have inconsistent lengths".into());
        }
        let neutral: Vec<Vec3> = neutral.iter().copied().map(Vec3::from).collect();
        let current: Vec<Vec3> = current.iter().copied().map(Vec3::from).collect();
        if neutral
            .iter()
            .chain(&current)
            .any(|point| !point.is_finite())
        {
            return Err("body deformation contains non-finite coordinates".into());
        }
        // Native `faceVerts` is a quad stream; triangular faces repeat their
        // last vertex.  Do not infer a stride: twelve entries are otherwise
        // ambiguous between four triangles and three quads.
        if faces.len() % 4 != 0 {
            return Err("body deformation face stream must contain native quads".into());
        }
        let mut neighbours = vec![Vec::new(); neutral.len()];
        for face in faces.chunks_exact(4) {
            let mut vertices: Vec<usize> = face
                .iter()
                .map(|&index| {
                    let index = index as usize;
                    if index >= neutral.len() {
                        Err("body deformation face index is out of bounds".to_owned())
                    } else {
                        Ok(index)
                    }
                })
                .collect::<Result<_, _>>()?;
            vertices.sort_unstable();
            vertices.dedup();
            for (offset, &a) in vertices.iter().enumerate() {
                for &b in vertices.iter().skip(offset + 1) {
                    if a != b && regions[a] == regions[b] {
                        neighbours[a].push(b);
                        neighbours[b].push(a);
                    }
                }
            }
        }
        // `neighbours` only joins vertices within a native region. Label each
        // connected component before the translation shortcut so coherent
        // bindings retain the same validation semantics in every pose.
        let mut coherent_components = vec![usize::MAX; neutral.len()];
        let mut components = Vec::new();
        for start in 0..neutral.len() {
            if coherent_components[start] != usize::MAX {
                continue;
            }
            let component_id = components.len();
            let mut component = vec![start];
            coherent_components[start] = component_id;
            let mut cursor = 0;
            while cursor < component.len() {
                let vertex = component[cursor];
                cursor += 1;
                for &other in &neighbours[vertex] {
                    if coherent_components[other] == usize::MAX {
                        coherent_components[other] = component_id;
                        component.push(other);
                    }
                }
            }
            components.push(component);
        }
        let translated = current
            .iter()
            .zip(&neutral)
            .map(|(to, from)| *to - *from)
            .collect::<Vec<_>>();
        let exact_translation = translated.windows(2).all(|window| window[0] == window[1]);
        if exact_translation {
            return Ok(Self {
                frames: vec![Mat3::IDENTITY; neutral.len()],
                regional_frames: vec![Mat3::IDENTITY; neutral.len()],
                coherent_frames: vec![Mat3::IDENTITY; neutral.len()],
                coherent_components,
                edge_scales: vec![0.; neutral.len()],
                regions: regions.to_vec(),
                neutral_points: neutral,
                translation_only: true,
            });
        }

        let mut global = [Mat3::IDENTITY; 256];
        for region in 0..=u8::MAX {
            let indices: Vec<_> = regions
                .iter()
                .enumerate()
                .filter_map(|(i, &r)| (r == region).then_some(i))
                .collect();
            if indices.len() >= 2 {
                global[region as usize] =
                    affine_from_points(&neutral, &current, &indices, Mat3::IDENTITY);
            }
        }
        let mut frames = Vec::with_capacity(neutral.len());
        let mut regional_frames = Vec::with_capacity(neutral.len());
        let mut edge_scales = Vec::with_capacity(neutral.len());
        for vertex in 0..neutral.len() {
            let region = regions[vertex] as usize;
            let local = &neighbours[vertex];
            frames.push(affine_from_edges(
                vertex,
                local,
                &neutral,
                &current,
                global[region],
            ));
            regional_frames.push(global[region]);
            edge_scales.push(
                local
                    .iter()
                    .map(|&other| (neutral[other] - neutral[vertex]).length())
                    .sum::<f32>()
                    / local.len().max(1) as f32,
            );
        }
        let mut coherent_frames = vec![Mat3::IDENTITY; neutral.len()];
        for component in components {
            let fallback = global[regions[component[0]] as usize];
            let frame = affine_from_points(&neutral, &current, &component, fallback);
            for vertex in component {
                coherent_frames[vertex] = frame;
            }
        }
        Ok(Self {
            frames,
            regional_frames,
            coherent_frames,
            coherent_components,
            edge_scales,
            regions: regions.to_vec(),
            neutral_points: neutral,
            translation_only: false,
        })
    }

    /// Verifies an explicit shared clothing anchor against the immutable
    /// neutral body. This is provenance validation for new author metadata;
    /// legacy footwear anchors intentionally retain their existing semantics.
    pub fn validate_neutral_anchor(
        &self,
        refs: [u32; 3],
        weights: [f32; 3],
        reference_point: [f32; 3],
    ) -> Result<(), String> {
        let reference_point = Vec3::from(reference_point);
        if !reference_point.is_finite()
            || reference_point.abs().max_element() > 100.
            || weights
                .iter()
                .any(|weight| !weight.is_finite() || *weight < 0.)
        {
            return Err("coherent body anchor is non-finite or outside the body domain".into());
        }
        let weight_sum: f32 = weights.iter().sum();
        if !weight_sum.is_finite() || weight_sum <= 0. {
            return Err("coherent body anchor weights have no finite positive sum".into());
        }
        let mut expected = Vec3::ZERO;
        for (&reference, &weight) in refs.iter().zip(weights.iter()) {
            expected += *self
                .neutral_points
                .get(reference as usize)
                .ok_or("coherent body anchor reference is outside body deformation")?
                * weight;
        }
        if (expected - reference_point).length() > 1e-4 {
            return Err(
                "coherent body anchor referencePointDM does not match the neutral body".into(),
            );
        }
        Ok(())
    }

    pub fn neutral_positions(&self) -> &[Vec3] {
        &self.neutral_points
    }

    pub fn regions(&self) -> &[u8] {
        &self.regions
    }

    pub fn transform_offset(
        &self,
        refs: [u32; 3],
        weights: [f32; 3],
        offset: Vec3,
    ) -> Result<Vec3, String> {
        if !offset.is_finite()
            || weights
                .iter()
                .any(|weight| !weight.is_finite() || *weight < 0.)
        {
            return Err("garment offset or weights are non-finite".into());
        }
        let mut baseline = Vec3::ZERO;
        let mut local = Vec3::ZERO;
        let mut edge_scale = 0.;
        let weight_sum: f32 = weights.iter().sum();
        if weight_sum <= 0. || !weight_sum.is_finite() {
            return Err("garment reference weights have no finite positive sum".into());
        }
        for (&reference, &weight) in refs.iter().zip(weights.iter()) {
            let index = reference as usize;
            let frame = self
                .frames
                .get(index)
                .ok_or("garment reference is outside body deformation")?;
            let regional = self
                .regional_frames
                .get(index)
                .ok_or("garment reference is outside body deformation")?;
            baseline += *regional * offset * weight;
            local += *frame * offset * weight;
            edge_scale += self
                .edge_scales
                .get(index)
                .copied()
                .ok_or("garment reference is outside body deformation")?
                * weight;
        }
        // Accepted saved garments may have sums within 0.001 of one. Their
        // authored residual is applied once, independently of anchor weights.
        if self.translation_only {
            return Ok(offset);
        }
        baseline /= weight_sum;
        local /= weight_sum;
        if !baseline.is_finite() || !local.is_finite() {
            return Err("body deformation produced a non-finite garment offset".into());
        }
        edge_scale /= weight_sum;
        let distance = offset.length();
        // A scalp one-ring is small while imported strand offsets can be many
        // decimetres.  The stable regional frame owns distant hair; local
        // detail is a smooth, bounded near-surface correction only.
        let locality = if edge_scale > 1e-6 {
            (edge_scale * 2. / (edge_scale * 2. + distance)).powi(2)
        } else {
            0.
        };
        let correction = local - baseline;
        let limit = baseline.length() * 0.25;
        let correction = if correction.length() > limit {
            correction.normalize_or_zero() * limit
        } else {
            correction
        };
        let transformed = baseline + correction * locality;
        if !transformed.is_finite() {
            return Err("body deformation produced a non-finite garment offset".into());
        }
        // Regional fits are normally head/body scale (about 1..2).  This is a
        // malformed-global-frame circuit breaker, never the local-hair policy.
        if distance > 1e-6 && transformed.length() > distance * MAX_OFFSET_SCALE {
            return Err("body deformation amplified garment offset beyond safe bound".into());
        }
        Ok(transformed)
    }

    /// Transport an offset with connected-component affine frames only.
    /// This is intentionally separate from `transform_offset`: it is for an
    /// explicit footwear opt-in and omits the normal local-frame correction.
    pub fn transform_offset_coherent(
        &self,
        refs: [u32; 3],
        weights: [f32; 3],
        offset: Vec3,
    ) -> Result<Vec3, String> {
        if !offset.is_finite()
            || weights
                .iter()
                .any(|weight| !weight.is_finite() || *weight < 0.)
        {
            return Err("garment offset or weights are non-finite".into());
        }
        let weight_sum: f32 = weights.iter().sum();
        if weight_sum <= 0. || !weight_sum.is_finite() {
            return Err("garment reference weights have no finite positive sum".into());
        }
        // Match the regular path: validate every referenced corner even when
        // a zero weight or an exact translation would otherwise hide it.
        let mut component = None;
        for &reference in &refs {
            if reference as usize >= self.coherent_frames.len() {
                return Err("garment reference is outside body deformation".into());
            }
            let candidate = *self
                .coherent_components
                .get(reference as usize)
                .ok_or("garment reference is outside body deformation")?;
            if let Some(component) = component {
                if component != candidate {
                    return Err("coherent footwear references span body components".into());
                }
            } else {
                component = Some(candidate);
            }
        }
        if self.translation_only {
            return Ok(offset);
        }
        let mut transformed = Vec3::ZERO;
        for (&reference, &weight) in refs.iter().zip(weights.iter()) {
            let frame = self
                .coherent_frames
                .get(reference as usize)
                .ok_or("garment reference is outside body deformation")?;
            transformed += *frame * offset * weight;
        }
        transformed /= weight_sum;
        if !transformed.is_finite() {
            return Err("body deformation produced a non-finite garment offset".into());
        }
        if offset.length_squared() > 1e-12
            && transformed.length() > offset.length() * MAX_OFFSET_SCALE
        {
            return Err("body deformation amplified garment offset beyond safe bound".into());
        }
        Ok(transformed)
    }

    /// Transport an offset through the local frames at one reviewed anchor.
    ///
    /// This keeps a tall-footwear shaft in one shared local deformation frame:
    /// every source vertex uses the same anchor refs and weights.  Validate
    /// those refs with the coherent path first so an authored anchor cannot
    /// silently span disconnected limbs, even where one weight is zero.
    pub fn transform_offset_anchor_local(
        &self,
        refs: [u32; 3],
        weights: [f32; 3],
        offset: Vec3,
    ) -> Result<Vec3, String> {
        // This validates finite input, positive weights, every index, and the
        // connected-component invariant before the translation early return.
        self.transform_offset_coherent(refs, weights, Vec3::ZERO)?;
        if !offset.is_finite() {
            return Err("garment offset or weights are non-finite".into());
        }
        if self.translation_only {
            return Ok(offset);
        }
        let weight_sum: f32 = weights.iter().sum();
        let mut transformed = Vec3::ZERO;
        for (&reference, &weight) in refs.iter().zip(weights.iter()) {
            let frame = self
                .frames
                .get(reference as usize)
                .ok_or("garment reference is outside body deformation")?;
            transformed += *frame * offset * weight;
        }
        transformed /= weight_sum;
        if !transformed.is_finite() {
            return Err("body deformation produced a non-finite garment offset".into());
        }
        if offset.length_squared() > 1e-12
            && transformed.length() > offset.length() * MAX_OFFSET_SCALE
        {
            return Err("body deformation amplified garment offset beyond safe bound".into());
        }
        Ok(transformed)
    }

    pub fn region(&self, vertex: usize) -> Option<u8> {
        self.regions.get(vertex).copied()
    }
}

fn outer(a: Vec3, b: Vec3) -> Mat3 {
    Mat3::from_cols(a * b.x, a * b.y, a * b.z)
}

fn stable_solve(covariance: Mat3, cross: Mat3, fallback: Mat3) -> Mat3 {
    // Normalize first so a tiny native edge does not turn a fixed absolute
    // determinant threshold into a false singularity.  The ridge belongs in
    // both sides of the least-squares system: it explicitly prefers the
    // region's global frame in a locally planar/rank-deficient neighborhood.
    let trace = covariance.x_axis.x + covariance.y_axis.y + covariance.z_axis.z;
    if !trace.is_finite() || trace <= 1e-12 {
        return fallback;
    }
    let covariance = covariance * trace.recip();
    let cross = cross * trace.recip();
    let ridge = 1e-3;
    let system = covariance + Mat3::IDENTITY * ridge;
    let determinant = system.determinant();
    if !determinant.is_finite() || determinant.abs() < f32::EPSILON {
        return fallback;
    }
    let result = (cross + fallback * ridge) * system.inverse();
    result.is_finite().then_some(result).unwrap_or(fallback)
}

fn affine_from_points(
    neutral: &[Vec3],
    current: &[Vec3],
    indices: &[usize],
    fallback: Mat3,
) -> Mat3 {
    let count = indices.len() as f32;
    let from = indices.iter().map(|&i| neutral[i]).sum::<Vec3>() / count;
    let to = indices.iter().map(|&i| current[i]).sum::<Vec3>() / count;
    let mut covariance = Mat3::ZERO;
    let mut cross = Mat3::ZERO;
    for &index in indices {
        let a = neutral[index] - from;
        let b = current[index] - to;
        covariance += outer(a, a);
        cross += outer(b, a);
    }
    // A well-conditioned region identifies the full 3-D affine map. Avoid
    // biasing that reliable baseline toward identity, especially for distant
    // strands where the local correction intentionally fades out.
    let trace = covariance.x_axis.x + covariance.y_axis.y + covariance.z_axis.z;
    if trace.is_finite() && trace > 1e-12 {
        let normalized = covariance * trace.recip();
        let determinant = normalized.determinant();
        if determinant.is_finite() && determinant.abs() > 1e-9 {
            let inverse = normalized.inverse();
            let frobenius = |m: Mat3| {
                (m.x_axis.length_squared() + m.y_axis.length_squared() + m.z_axis.length_squared())
                    .sqrt()
            };
            if inverse.is_finite() && frobenius(normalized) * frobenius(inverse) < 1e4 {
                let frame = cross * trace.recip() * inverse;
                if frame.is_finite() {
                    return frame;
                }
            }
        }
    }
    stable_solve(covariance, cross, fallback)
}

fn affine_from_edges(
    vertex: usize,
    neighbours: &[usize],
    neutral: &[Vec3],
    current: &[Vec3],
    fallback: Mat3,
) -> Mat3 {
    if neighbours.is_empty() {
        return fallback;
    }
    let mut covariance = Mat3::ZERO;
    let mut cross = Mat3::ZERO;
    for &other in neighbours {
        let a = neutral[other] - neutral[vertex];
        let b = current[other] - current[vertex];
        covariance += outer(a, a);
        cross += outer(b, a);
    }
    stable_solve(covariance, cross, fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body() -> (Vec<[f32; 3]>, Vec<u32>, Vec<u8>) {
        (
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            vec![0, 1, 2, 2, 0, 3, 1, 1, 0, 2, 3, 3, 1, 3, 2, 2],
            vec![0; 4],
        )
    }
    #[test]
    fn identity_and_translation_preserve_offsets_exactly() {
        let (neutral, faces, regions) = body();
        let field = BodyDeformation::new(&neutral, &neutral, &faces, &regions).unwrap();
        assert_eq!(
            field
                .transform_offset([0, 0, 0], [1., 0., 0.], Vec3::new(1., 2., 3.))
                .unwrap(),
            Vec3::new(1., 2., 3.)
        );
        let moved: Vec<_> = neutral
            .iter()
            .map(|p| [p[0] + 5., p[1] - 2., p[2] + 3.])
            .collect();
        let field = BodyDeformation::new(&neutral, &moved, &faces, &regions).unwrap();
        assert_eq!(
            field
                .transform_offset([0, 1, 2], [0.2, 0.3, 0.5], Vec3::new(1., 2., 3.))
                .unwrap(),
            Vec3::new(1., 2., 3.)
        );
        assert!(
            field
                .transform_offset_coherent([0, 1, 99], [1., 0., 0.], Vec3::ONE)
                .is_err()
        );
    }
    #[test]
    fn affine_rotation_and_scale_transport_offsets() {
        let (neutral, faces, regions) = body();
        let transform = Mat3::from_cols(
            Vec3::new(0., 2., 0.),
            Vec3::new(-3., 0., 0.),
            Vec3::new(0., 0., 0.5),
        );
        let current: Vec<_> = neutral
            .iter()
            .map(|p| (transform * Vec3::from(*p)).to_array())
            .collect();
        let field = BodyDeformation::new(&neutral, &current, &faces, &regions).unwrap();
        let offset = Vec3::new(0.2, 0.3, 0.4);
        assert!(
            (field
                .transform_offset([0, 1, 2], [0.2, 0.3, 0.5], offset)
                .unwrap()
                - transform * offset)
                .length()
                < 2e-3
        );
    }
    #[test]
    fn repeated_triangle_corner_does_not_bias_local_non_affine_fit() {
        let (neutral, _, regions) = body();
        let mut current = neutral.clone();
        current[2][2] += 0.3;
        current[3][1] += 0.2;
        let a =
            BodyDeformation::new(&neutral, &current, &[0, 1, 2, 2, 0, 3, 1, 1], &regions).unwrap();
        let b =
            BodyDeformation::new(&neutral, &current, &[0, 1, 2, 0, 0, 3, 1, 0], &regions).unwrap();
        for vertex in 0..4 {
            assert_eq!(
                a.transform_offset([vertex; 3], [1., 0., 0.], Vec3::ONE)
                    .unwrap(),
                b.transform_offset([vertex; 3], [1., 0., 0.], Vec3::ONE)
                    .unwrap()
            );
        }
    }

    #[test]
    fn degenerate_faces_stay_finite() {
        let neutral = vec![[0., 0., 0.], [1., 0., 0.]];
        let current = vec![[0., 0., 0.], [2., 0., 0.]];
        let field = BodyDeformation::new(&neutral, &current, &[0, 0, 0, 0], &[0, 0]).unwrap();
        assert!(
            field
                .transform_offset([0, 0, 0], [1., 0., 0.], Vec3::X)
                .unwrap()
                .is_finite()
        );
    }
    #[test]
    fn planar_local_neighborhood_uses_global_normal_frame() {
        let neutral = vec![
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 1., 0.],
            [-1., 0., 0.],
            [0., -1., 0.],
            [0., 0., 1.],
            [1., 0., 1.],
            [0., 1., 1.],
        ];
        let scale = Mat3::from_diagonal(Vec3::new(2., 3., 4.));
        let current: Vec<_> = neutral
            .iter()
            .map(|p| (scale * Vec3::from(*p)).to_array())
            .collect();
        let faces = vec![0, 1, 2, 3, 0, 3, 4, 1, 5, 6, 7, 7, 1, 6, 7, 2];
        let field = BodyDeformation::new(&neutral, &current, &faces, &[0; 8]).unwrap();
        let moved = field
            .transform_offset([0, 0, 0], [1., 0., 0.], Vec3::Z)
            .unwrap();
        assert!((moved - Vec3::new(0., 0., 4.)).length() < 0.02);
    }
    #[test]
    fn coherent_frames_do_not_mix_disconnected_same_region_components() {
        // Two tetrahedral feet intentionally share region 9.  Their separate
        // width scales must remain independent; a region-wide fit is biased by
        // the distance between them.
        let left = [
            Vec3::new(-4., 0., 0.),
            Vec3::new(-3., 0., 0.),
            Vec3::new(-4., 1., 0.),
            Vec3::new(-4., 0., 1.),
        ];
        let right = [
            Vec3::new(4., 0., 0.),
            Vec3::new(5., 0., 0.),
            Vec3::new(4., 1., 0.),
            Vec3::new(4., 0., 1.),
        ];
        let neutral: Vec<[f32; 3]> = left
            .into_iter()
            .chain(right)
            .map(|point| point.to_array())
            .collect();
        let current: Vec<[f32; 3]> = neutral
            .iter()
            .enumerate()
            .map(|(index, point)| {
                let scale = if index < 4 { 2. } else { 4. };
                (Vec3::from(*point) * scale).to_array()
            })
            .collect();
        let faces = vec![
            0, 1, 2, 2, 0, 1, 3, 3, 0, 2, 3, 3, 1, 2, 3, 3, 4, 5, 6, 6, 4, 5, 7, 7, 4, 6, 7, 7, 5,
            6, 7, 7,
        ];
        let field = BodyDeformation::new(&neutral, &current, &faces, &[9; 8]).unwrap();
        assert!(
            (field
                .transform_offset_coherent([0, 1, 2], [1., 0., 0.], Vec3::X)
                .unwrap()
                - Vec3::X * 2.)
                .length()
                < 1e-3
        );
        assert!(
            (field
                .transform_offset_coherent([4, 5, 6], [1., 0., 0.], Vec3::X)
                .unwrap()
                - Vec3::X * 4.)
                .length()
                < 1e-3
        );
        // A coherent binding may not blend frames from the disconnected feet,
        // even when the foreign reference has zero barycentric weight.
        assert!(
            field
                .transform_offset_coherent([0, 1, 4], [1., 0., 0.], Vec3::X)
                .is_err()
        );
        let translated: Vec<_> = neutral
            .iter()
            .map(|point| (Vec3::from(*point) + Vec3::new(1., 2., 3.)).to_array())
            .collect();
        let translation_field =
            BodyDeformation::new(&neutral, &translated, &faces, &[9; 8]).unwrap();
        assert!(
            translation_field
                .transform_offset_coherent([0, 1, 4], [1., 0., 0.], Vec3::X)
                .is_err()
        );
    }

    #[test]
    fn anchor_local_transport_uses_local_frame_but_keeps_component_validation() {
        let field = BodyDeformation {
            frames: vec![Mat3::from_diagonal(Vec3::splat(2.)); 3],
            regional_frames: vec![Mat3::IDENTITY; 3],
            // Deliberately different from the anchor-local frame: a long
            // connected calf component can make this component fit unsuitable
            // for a small boot-shaft patch.
            coherent_frames: vec![Mat3::from_diagonal(Vec3::splat(4.)); 3],
            coherent_components: vec![0; 3],
            edge_scales: vec![1.; 3],
            regions: vec![8; 3],
            neutral_points: vec![Vec3::ZERO; 3],
            translation_only: false,
        };
        assert_eq!(
            field
                .transform_offset_anchor_local([0, 1, 2], [1., 0., 0.], Vec3::X)
                .unwrap(),
            Vec3::X * 2.
        );
        let mut split_component = field;
        split_component.coherent_components[2] = 1;
        assert!(
            split_component
                .transform_offset_anchor_local([0, 1, 2], [1., 0., 0.], Vec3::X)
                .is_err()
        );
    }

    #[test]
    fn collapsed_regional_direction_cannot_inherit_local_stretch() {
        let field = BodyDeformation {
            frames: vec![Mat3::IDENTITY * 4.],
            regional_frames: vec![Mat3::ZERO],
            coherent_frames: vec![Mat3::ZERO],
            coherent_components: vec![0],
            edge_scales: vec![1.],
            regions: vec![0],
            neutral_points: vec![Vec3::ZERO],
            translation_only: false,
        };
        assert_eq!(
            field
                .transform_offset([0; 3], [1., 0., 0.], Vec3::X)
                .unwrap(),
            Vec3::ZERO
        );
    }

    #[test]
    fn long_authored_offset_rejects_unstable_local_stretch() {
        let field = BodyDeformation {
            frames: vec![Mat3::from_diagonal(Vec3::splat(100.))],
            regional_frames: vec![Mat3::IDENTITY],
            coherent_frames: vec![Mat3::IDENTITY],
            coherent_components: vec![0],
            edge_scales: vec![0.1],
            regions: vec![0],
            neutral_points: vec![Vec3::ZERO],
            translation_only: false,
        };
        let offset = Vec3::new(2., 0., 0.);
        let transformed = field
            .transform_offset([0, 0, 0], [1., 0., 0.], offset)
            .unwrap();
        assert!(
            transformed.length() < 2.01,
            "long hair must use the regional frame"
        );
    }
}
