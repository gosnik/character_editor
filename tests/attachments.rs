use bevy::prelude::Vec3;
use human_editor::{
    ara::AraRecipe, native_assets::NativeLibrary, native_character::NativeCharacter,
};
use serde_json::json;
use std::sync::{Arc, OnceLock};

fn library() -> Arc<NativeLibrary> {
    static LIB: OnceLock<Arc<NativeLibrary>> = OnceLock::new();
    LIB.get_or_init(|| {
        Arc::new(
            NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap(),
        )
    })
    .clone()
}

fn part<'a>(
    character: &'a NativeCharacter,
    prefix: &str,
) -> &'a human_editor::native_character::NativePart {
    character
        .parts
        .iter()
        .find(|part| part.name.starts_with(prefix))
        .unwrap_or_else(|| panic!("missing {prefix}"))
}

#[test]
fn held_props_keep_the_source_socket_and_grip_frames_through_animation() {
    let mut recipe = AraRecipe::default();
    recipe.params["outfit"] = json!("none");
    recipe.params["weapon"] = json!("rifle");
    recipe.params["carry"] = json!("handbag");
    let character = NativeCharacter::build(&library(), &recipe).unwrap();
    let rig = character.rig.as_ref().unwrap();

    let rifle = part(&character, "weapon:rifle_2");
    assert_eq!(rifle.rigid_joint.as_deref(), Some("WeaponR"));
    // The first two rings of Ara's rifle barrel are its near and far ends.
    // Its fitted +Z barrel axis must point along the character's forward axis.
    let centre = |range: std::ops::Range<usize>| {
        rifle.positions[range]
            .iter()
            .fold(Vec3::ZERO, |sum, point| sum + Vec3::from(*point))
            / 12.
    };
    let barrel = (centre(12..24) - centre(0..12)).normalize();
    assert!(
        barrel.z > 0.95,
        "rifle barrel should face forward: {barrel:?}"
    );

    let bag = part(&character, "carry:handbag");
    assert_eq!(bag.rigid_joint.as_deref(), Some("LeftHand"));
    let left = rig
        .bones
        .iter()
        .position(|bone| bone.name == "LeftHand")
        .unwrap();
    let lowest = bag
        .positions
        .iter()
        .map(|point| point[1])
        .fold(f32::INFINITY, f32::min);
    assert!(
        lowest < rig.bones[left].head.y - 0.12,
        "bag must hang below the relaxed grip"
    );
    let closest = bag
        .positions
        .iter()
        .map(|point| Vec3::from(*point).distance(rig.bones[left].head))
        .fold(f32::INFINITY, f32::min);
    // The source grip is just inside the closed fist; the nearest handle
    // vertex sits about 10 cm from the wrist-bone origin in the bind frame.
    assert!(
        (0.09..0.115).contains(&closest),
        "bag must use Ara's relaxed left-hand grip: {closest}"
    );

    let (joints, weights) = rig.skin_part(bag);
    assert!(joints.iter().all(|joint| joint[0] as usize == left));
    assert!(weights.iter().all(|weight| weight[0] == 1.0));
    let posed = rig.skin_positions(&bag.positions, &joints, &weights, &rig.pose("walk", 0.3));
    let expected = rig.pose("walk", 0.3)[left];
    for (bind, posed) in bag.positions.iter().zip(&posed) {
        assert!(
            Vec3::from(*posed).distance(expected.transform_point3(Vec3::from(*bind))) < 1e-5,
            "bag vertex did not follow LeftHand rigidly"
        );
    }
}

#[test]
fn sword_blade_uses_the_source_socket_up_axis() {
    let mut recipe = AraRecipe::default();
    recipe.params["outfit"] = json!("none");
    recipe.params["weapon"] = json!("sword");
    let character = NativeCharacter::build(&library(), &recipe).unwrap();
    let rig = character.rig.as_ref().unwrap();
    let sword = part(&character, "weapon:sword");
    let socket = rig
        .bones
        .iter()
        .position(|bone| bone.name == "WeaponR")
        .unwrap();
    let socket_inverse = rig.bind_world_matrices()[socket].inverse();
    let local: Vec<_> = sword
        .positions
        .iter()
        .map(|point| socket_inverse.transform_point3(Vec3::from(*point)))
        .collect();
    let y_min = local
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min);
    let y_max = local
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max);
    // Ara blades originate at the grip and run along socket +Y.  This catches
    // a row/column swap when baking the WeaponR frame.
    assert!(
        (0.08..0.09).contains(&y_min) && y_max > 0.8,
        "sword blade local Y range {y_min}..{y_max}"
    );
}
