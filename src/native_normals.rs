//! Area-weighted normals with canonical body vertices shared across UV seams.
use crate::native_character::NativePart;
use bevy::prelude::Vec3;
use std::collections::HashMap;

pub fn surface_normals(part: &NativePart, positions: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let mut sums = vec![Vec3::ZERO; positions.len()];
    let mut groups = Vec::with_capacity(positions.len());
    let mut canonical = HashMap::new();
    let smooth_custom_uv_seams =
        part.name.starts_with("custom:") && part.material["smoothUvSeams"].as_bool() == Some(true);
    for index in 0..positions.len() {
        // Body UV copies share reference weights and their original position.
        // Explicitly opted-in custom garments use the same exact key to smooth
        // imported UV-island copies. Do not weld cards or hard-edge assets by
        // default: their material must opt in with smoothUvSeams: true.
        let group = if (part.name == "Body" || smooth_custom_uv_seams)
            && index < part.refs.len()
            && index < part.ref_weights.len()
        {
            let key = (
                part.refs[index],
                part.ref_weights[index].map(f32::to_bits),
                part.positions[index].map(f32::to_bits),
            );
            *canonical.entry(key).or_insert(index)
        } else {
            index
        };
        groups.push(group);
    }
    for triangle in part.indices.chunks_exact(3) {
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let normal = (Vec3::from(positions[b]) - Vec3::from(positions[a]))
            .cross(Vec3::from(positions[c]) - Vec3::from(positions[a]));
        for index in [a, b, c] {
            sums[groups[index]] += normal;
        }
    }
    groups
        .into_iter()
        .map(|group| sums[group].normalize_or_zero().to_array())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uv_seam_part(name: &str, material: serde_json::Value) -> NativePart {
        NativePart {
            name: name.into(),
            positions: vec![
                [0., 0., 0.],
                [1., 0., 0.],
                [0., 1., 0.],
                [0., 0., 0.],
                [0., 0., 1.],
                [1., 0., 0.],
            ],
            // The duplicated origin belongs to two UV islands whose faces
            // meet at a right angle, so separate and welded normals differ.
            indices: vec![0, 1, 2, 3, 4, 5],
            uvs: vec![[0., 0.]; 6],
            refs: vec![[0; 3], [1; 3], [2; 3], [0; 3], [3; 3], [1; 3]],
            ref_weights: vec![[1., 0., 0.]; 6],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material,
        }
    }

    #[test]
    fn uv_seams_share_normals_without_welding_distinct_surfaces() {
        let mut part = uv_seam_part("Body", serde_json::json!({}));
        let normals = surface_normals(&part, &part.positions);
        assert_eq!(normals[0], normals[3]);
        assert_eq!(normals[1], normals[5]);
        assert!((Vec3::from(normals[0]) - Vec3::new(0., 1., 1.).normalize()).length() < 1e-6);
        part.refs[3] = [9; 3];
        let separate = surface_normals(&part, &part.positions);
        assert_ne!(separate[0], separate[3]);
    }

    #[test]
    fn opted_in_custom_garment_smooths_exactly_bound_uv_seams_without_mesh_changes() {
        let part = uv_seam_part("custom:boots", serde_json::json!({"smoothUvSeams": true}));
        let original_positions = part.positions.clone();
        let original_uvs = part.uvs.clone();
        let original_indices = part.indices.clone();

        let normals = surface_normals(&part, &part.positions);

        assert_eq!(normals[0], normals[3]);
        assert!(normals.iter().flatten().all(|value| value.is_finite()));
        assert_eq!(part.positions, original_positions);
        assert_eq!(part.uvs, original_uvs);
        assert_eq!(part.indices, original_indices);
    }

    #[test]
    fn opted_in_custom_garment_keeps_differently_bound_uv_seams_separate() {
        let mut part = uv_seam_part("custom:boots", serde_json::json!({"smoothUvSeams": true}));
        part.refs[3] = [9; 3];

        let normals = surface_normals(&part, &part.positions);

        assert_ne!(normals[0], normals[3]);
    }

    #[test]
    fn custom_uv_seam_smoothing_requires_a_literal_true_opt_in() {
        for material in [
            serde_json::json!({}),
            serde_json::json!({"smoothUvSeams": false}),
            serde_json::json!({"smoothUvSeams": "true"}),
        ] {
            let part = uv_seam_part("custom:boots", material);
            let normals = surface_normals(&part, &part.positions);
            assert_ne!(normals[0], normals[3]);
        }
        let part = uv_seam_part("boots", serde_json::json!({"smoothUvSeams": true}));
        let normals = surface_normals(&part, &part.positions);
        assert_ne!(normals[0], normals[3]);
    }
}
