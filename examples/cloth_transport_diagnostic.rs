//! Offline comparison of garment LBS with transport through its body anchors.
//! This records evidence only; it does not alter fitting, rendering, or export.
use bevy::prelude::Vec3;
use human_editor::{
    ara::AraRecipe, native_assets::NativeLibrary, native_character::NativeCharacter,
};
use serde_json::json;
use std::{collections::BTreeMap, env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 5 {
        return Err("usage: cloth_transport_diagnostic ASSETS RECIPE PART OUTPUT.json".into());
    }
    let recipe: AraRecipe = serde_json::from_slice(&fs::read(&args[2])?)?;
    if recipe.params["detail"] != "medium" || recipe.params["restPose"] != "A" {
        return Err("diagnostic requires medium canonical body and A rest pose".into());
    }
    let library = NativeLibrary::load(&args[1])?;
    let character = NativeCharacter::build(&library, &recipe)?;
    let part = character
        .parts
        .iter()
        .find(|p| p.name == args[3])
        .ok_or("missing target part")?;
    let mut rig = character.rig.clone().ok_or("missing rig")?;
    let extra_arm_spread: f32 = env::var("CLOTH_DIAGNOSTIC_EXTRA_ARM_RADIANS")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    if !extra_arm_spread.is_finite() || !(0.0..=0.2).contains(&extra_arm_spread) {
        return Err("diagnostic arm offset must be finite and within 0..0.2 radians".into());
    }
    rig.set_arm_spread(rig.arm_spread() + extra_arm_spread);
    // Remove occluding garments while retaining the exact shape and skeleton.
    let mut bare_recipe = recipe.clone();
    bare_recipe.custom_garments.clear();
    for key in [
        "top",
        "bottom",
        "jacket",
        "feet",
        "hair",
        "beard",
        "hat",
        "accessory",
    ] {
        if bare_recipe.params[key].is_string() {
            bare_recipe.params[key] = json!("none");
        }
    }
    bare_recipe.params["underwear"] = json!(false);
    let bare = NativeCharacter::build(&library, &bare_recipe)?;
    let mut body = bare
        .parts
        .iter()
        .find(|p| p.name == "Body")
        .ok_or("missing bare body")?
        .clone();
    // Footwear can lower the assembled character's grounding plane. Removing
    // it for a bare-body diagnostic changes that origin, even with identical
    // body morphs. Align through shared canonical vertices before comparing
    // body/cloth positions or applying the clothed character's rig matrices.
    let visible_body = character
        .parts
        .iter()
        .find(|p| p.name == "Body")
        .ok_or("missing clothed body")?;
    let visible: BTreeMap<u32, Vec3> = visible_body
        .refs
        .iter()
        .zip(&visible_body.positions)
        .filter(|(refs, _)| refs[0] == refs[1] && refs[0] == refs[2])
        .map(|(refs, p)| (refs[0], Vec3::from(*p)))
        .collect();
    let differences: Vec<Vec3> = body
        .refs
        .iter()
        .zip(&body.positions)
        .filter_map(|(refs, p)| visible.get(&refs[0]).map(|q| *q - Vec3::from(*p)))
        .collect();
    let grounding_translation = *differences
        .first()
        .ok_or("no shared canonical body anchors")?;
    if differences
        .iter()
        .any(|d| d.distance(grounding_translation) > 1e-5)
    {
        return Err("bare/clothed body shapes differ beyond a grounding translation".into());
    }
    for p in &mut body.positions {
        *p = (Vec3::from(*p) + grounding_translation).to_array();
    }
    let mut canonical = BTreeMap::new();
    for (i, refs) in body.refs.iter().enumerate() {
        if refs[0] != refs[1] || refs[0] != refs[2] {
            return Err("body is a proxy, not canonical".into());
        }
        canonical.entry(refs[0]).or_insert(i);
    }
    let (joints, weights) = rig.skin_part(part);
    let (body_joints, body_weights) = rig.skin_part(&body);
    let mut poses = Vec::new();
    for (clip, time) in [
        ("idle", 0.0),
        ("idle", 0.5),
        ("walk", 0.215),
        ("walk", 0.645),
    ] {
        let matrices = human_editor::native_breast::bake_secondary(
            &rig,
            clip,
            time,
            &recipe.breasts,
            recipe.params["gender"].as_f64().unwrap_or(1.0) as f32,
        );
        let current = rig.skin_positions(&part.positions, &joints, &weights, &matrices);
        let posed_body =
            rig.skin_positions(&body.positions, &body_joints, &body_weights, &matrices);
        let mut transported = Vec::new();
        let mut missing = Vec::new();
        for (i, p) in part.positions.iter().enumerate() {
            let refs = part.refs[i];
            let factors = part.ref_weights[i];
            let mut anchor = Vec3::ZERO;
            let mut posed_anchor = Vec3::ZERO;
            let mut valid = true;
            for (base, factor) in refs.into_iter().zip(factors) {
                if factor == 0.0 {
                    continue;
                }
                if let Some(&v) = canonical.get(&base) {
                    anchor += Vec3::from(body.positions[v]) * factor;
                    posed_anchor += Vec3::from(posed_body[v]) * factor;
                } else {
                    valid = false;
                }
            }
            if !valid {
                missing.push(i);
                transported.push(current[i]);
                continue;
            }
            let offset = Vec3::from(*p) - anchor;
            let mut transformed_offset = Vec3::ZERO;
            for k in 0..4 {
                transformed_offset +=
                    matrices[joints[i][k] as usize].transform_vector3(offset) * weights[i][k];
            }
            transported.push((posed_anchor + transformed_offset).to_array());
        }
        let differences: Vec<f32> = current
            .iter()
            .zip(&transported)
            .map(|(a, b)| Vec3::from(*a).distance(Vec3::from(*b)) * 1000.0)
            .collect();
        if differences.iter().any(|v| !v.is_finite()) {
            return Err("nonfinite comparison".into());
        }
        let mut sorted = differences.clone();
        sorted.sort_by(f32::total_cmp);
        poses.push(
            json!({"clip":clip,"time_s":time,"current_positions_m":current,
            "anchor_transport_positions_m":transported,"body_positions_m":posed_body,
            "difference_mm":differences,"max_difference_mm":sorted.last(),
            "p95_difference_mm":sorted.get(sorted.len()*95/100),
            "missing_anchor_vertices":missing}),
        );
    }
    fs::write(
        &args[4],
        serde_json::to_vec(&json!({
            "schema":"cloth-anchor-transport-diagnostic/v1", "recipe":args[2], "part":args[3],
        "note":"Comparison only. Anchor transport is not enabled in runtime or GLTF export.",
        "diagnostic_extra_arm_spread_radians":extra_arm_spread,"arm_spread_radians":rig.arm_spread(),
        "bare_body_grounding_translation_m":grounding_translation.to_array(),
            "positions_m":part.positions,"indices":part.indices,"refs":part.refs,"ref_weights":part.ref_weights,
            "skin_joints":joints,"skin_weights":weights,
            "body_positions_m":body.positions,"body_indices":body.indices,"body_refs":body.refs,"poses":poses,
        }))?,
    )?;
    Ok(())
}
