use human_editor::{
    ara::{AraRecipe, CustomTarget, native_catalog},
    native_assets::NativeLibrary,
    native_character::NativeCharacter,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, OnceLock};

#[derive(Deserialize)]
struct MotionReference {
    cases: Vec<MotionReferenceCase>,
    t_cases: Vec<MotionReferenceCase>,
}
#[derive(Deserialize)]
struct MotionReferenceCase {
    clip: String,
    time: f32,
    joints: Vec<MotionReferenceJoint>,
}
#[derive(Deserialize)]
struct MotionReferenceJoint {
    name: String,
    position: [f32; 3],
    rotation: [f32; 4],
}
fn library() -> Arc<NativeLibrary> {
    static LIB: OnceLock<Arc<NativeLibrary>> = OnceLock::new();
    LIB.get_or_init(|| {
        Arc::new(
            NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap(),
        )
    })
    .clone()
}
fn valid(character: &NativeCharacter) {
    assert!(
        (0.8..2.5).contains(&character.height),
        "height {}",
        character.height
    );
    assert!(character.triangle_count() > 3000);
    for part in &character.parts {
        assert_eq!(part.positions.len(), part.refs.len());
        assert_eq!(part.positions.len(), part.uvs.len());
        assert!(part.positions.iter().flatten().all(|v| v.is_finite()));
        assert!(
            part.indices
                .iter()
                .all(|&i| (i as usize) < part.positions.len())
        );
    }
    if let Some(rig) = &character.rig {
        for b in &rig.bones {
            assert!(b.head.is_finite());
            assert!(
                b.head.y >= -0.1 && b.head.y <= character.height + 0.2,
                "bone {} at {:?} outside character",
                b.name,
                b.head
            );
        }
        for clip in native_catalog()["clips"].as_array().unwrap() {
            let name = clip["id"].as_str().unwrap();
            assert!(rig.clip_duration(name).is_some(), "missing motion {name}");
            assert!(rig.pose(name, 0.3).iter().all(|m| m.is_finite()));
        }
        for m in rig.pose("invalid", 0.) {
            assert!(m.abs_diff_eq(bevy::prelude::Mat4::IDENTITY, 0.001));
        }
    }
}
#[test]
fn complete_native_default_and_mhm_roundtrip() {
    let lib = library();
    let recipe = AraRecipe::default();
    let character = NativeCharacter::build(&lib, &recipe).unwrap();
    valid(&character);
    assert!(character.parts.iter().any(|p| p.name == "Eyes"));
    assert!(character.parts.iter().any(|p| p.name == "Outfit"));
    assert!(character.parts.iter().any(|p| p.name == "Teeth"));
    let mhm = human_editor::mpfb::export_mhm(&recipe);
    let restored = human_editor::mpfb::import_mhm(&mhm, &recipe, &native_catalog()).unwrap();
    assert_eq!(recipe.params["gender"], restored.params["gender"]);
    assert_eq!(recipe.params["height"], restored.params["height"]);
}
#[test]
fn wardrobe_looks_props_and_native_motion_change_actual_geometry() {
    let lib = library();
    let mut recipe = AraRecipe::default();
    recipe.params["gender"] = json!(0.);
    recipe.params["african"] = json!(0.9);
    recipe.params["asian"] = json!(0.05);
    recipe.params["caucasian"] = json!(0.05);
    recipe.params["outfit"] = json!("none");
    recipe.params["top"] = json!("tank_top");
    recipe.params["bottom"] = json!("cargo_shorts");
    recipe.params["hair"] = json!("cornrows");
    recipe.params["facialHair"] = json!("stubble");
    recipe.params["earrings"] = json!("small_hoops");
    recipe.params["tattoos"] = json!(["armband_l", "face_marks"]);
    recipe.params["weapon"] = json!("pistol");
    recipe.params["carry"] = json!("handbag");
    recipe.params["garmentStyles"] =
        json!({"tank_top":{"pattern":"camo","color":"#3f6b5a","color2":"#191a1c"}});
    let character = NativeCharacter::build(&lib, &recipe).unwrap();
    valid(&character);
    let names: Vec<_> = character.parts.iter().map(|p| p.name.as_str()).collect();
    for expected in [
        "tank_top",
        "cargo_shorts",
        "hair:cornrows",
        "earrings:small_hoops",
        "weapon:pistol",
        "carry:handbag",
        "tattoo_stubble",
        "tattoo_armband_l",
    ] {
        assert!(
            names.iter().any(|n| n.starts_with(expected)),
            "missing {expected} in {names:?}"
        );
    }
    let garment = character
        .parts
        .iter()
        .find(|p| p.name.starts_with("tank_top"))
        .unwrap();
    assert_eq!(garment.material["pattern"], "camo");
    let rig = character.rig.as_ref().unwrap();
    // Held props are rigidly owned by their attachment bones.  Sampling nearby
    // body vertices would otherwise blend wrist/forearm weights and shear them.
    for (prefix, joint) in [("weapon:pistol", "WeaponR"), ("carry:handbag", "LeftHand")] {
        let part = character
            .parts
            .iter()
            .find(|part| part.name.starts_with(prefix))
            .unwrap();
        assert_eq!(part.rigid_joint.as_deref(), Some(joint));
        let bone = rig
            .bones
            .iter()
            .position(|bone| bone.name == joint)
            .unwrap() as u16;
        let (joints, weights) = rig.skin_part(part);
        assert!(joints.iter().all(|j| *j == [bone, 0, 0, 0]));
        assert!(weights.iter().all(|w| *w == [1., 0., 0., 0.]));
    }
    let (j, w) = rig.skin_part(&character.parts[0]);
    let moved = rig.skin_positions(
        &character.parts[0].positions,
        &j,
        &w,
        &rig.pose("walk", 0.3),
    );
    assert_ne!(moved, character.parts[0].positions);

    // A hand-sized weapon is rigid in its socket frame even when the body,
    // forearm, and fingers receive deliberately extreme target values.
    let pistol = character
        .parts
        .iter()
        .find(|part| part.name == "weapon:pistol_0")
        .unwrap();
    let edge = |part: &human_editor::native_character::NativePart| {
        let a = bevy::prelude::Vec3::from(part.positions[0]);
        let b = bevy::prelude::Vec3::from(part.positions[part.positions.len() / 2]);
        a.distance(b)
    };
    let mut extreme = recipe.clone();
    extreme.params["height"] = json!(1.0);
    extreme.params["weight"] = json!(1.0);
    extreme.params["muscle"] = json!(1.0);
    for key in [
        "armslegs/upperarm-scale-horiz",
        "armslegs/lowerarm-scale-horiz",
        "armslegs/hand-scale",
        "armslegs/hand-fingers-length",
    ] {
        extreme.params[key] = json!(1.0);
    }
    let extreme = NativeCharacter::build(&lib, &extreme).unwrap();
    let extreme_pistol = extreme
        .parts
        .iter()
        .find(|part| part.name == "weapon:pistol_0")
        .unwrap();
    assert!((edge(pistol) - edge(extreme_pistol)).abs() < 1e-5);

    // Cartoon deformation must not bend a rigid prop. Its socket follows the
    // stylized hand, but distances within the pistol remain authored.
    let mut cartoon = recipe.clone();
    cartoon.params["rig"] = json!("none");
    cartoon.cartoon.enabled = true;
    let cartoon = NativeCharacter::build(&lib, &cartoon).unwrap();
    let cartoon_pistol = cartoon
        .parts
        .iter()
        .find(|part| part.name == "weapon:pistol_0")
        .unwrap();
    assert!((edge(pistol) - edge(cartoon_pistol)).abs() < 1e-5);

    let mut worn = recipe.clone();
    worn.params["weapon"] = json!("none");
    worn.params["carry"] = json!("backpack");
    worn.params["outfit"] = json!("set_suit_tie");
    let worn = NativeCharacter::build(&lib, &worn).unwrap();
    assert!(
        worn.parts
            .iter()
            .any(|part| part.name.starts_with("suit_trousers"))
    );
    let backpack = worn
        .parts
        .iter()
        .find(|part| part.name.starts_with("backpack"))
        .unwrap();
    assert_eq!(backpack.rigid_joint.as_deref(), Some("Spine2"));
}

/// Ara's TypeScript evaluator is the offline oracle.  These selected frames
/// cover locomotion, airborne motion, both turn signs, aiming, and sitting;
/// compare the native skin matrices after restoring their bind transforms so
/// this catches local-frame multiplication and root-motion regressions.
#[test]
fn native_motion_matches_ara_reference_poses() {
    let lib = library();
    let mut recipe = AraRecipe::default();
    for key in ["outfit", "top", "bottom", "jacket", "shoes", "hat"] {
        recipe.params[key] = json!("none");
    }
    let character = NativeCharacter::build(&lib, &recipe).unwrap();
    let reference: MotionReference =
        serde_json::from_str(include_str!("../assets/native/motion_reference.json")).unwrap();
    let check = |rig: &human_editor::native_rig::NativeRig, cases: &[MotionReferenceCase]| {
        for case in cases {
            let skin = rig.pose(&case.clip, case.time);
            for joint in &case.joints {
                let index = rig
                    .bones
                    .iter()
                    .position(|bone| bone.name == joint.name)
                    .unwrap();
                let world = skin[index] * rig.bind_world_matrices()[index];
                let (_, rotation, position) = world.to_scale_rotation_translation();
                let expected = bevy::prelude::Vec3::from(joint.position);
                assert!(
                    position.distance(expected) < 0.02,
                    "{} at {}: {} position {:?}, expected {:?}",
                    case.clip,
                    case.time,
                    joint.name,
                    position,
                    expected
                );
                let expected = bevy::prelude::Quat::from_xyzw(
                    joint.rotation[0],
                    joint.rotation[1],
                    joint.rotation[2],
                    joint.rotation[3],
                )
                .normalize();
                assert!(
                    rotation.dot(expected).abs() > 0.9999,
                    "{} at {}: {} rotation {:?}, expected {:?}",
                    case.clip,
                    case.time,
                    joint.name,
                    rotation,
                    expected
                );
            }
        }
    };
    check(character.rig.as_ref().unwrap(), &reference.cases);
    recipe.params["restPose"] = json!("T");
    let t_character = NativeCharacter::build(&lib, &recipe).unwrap();
    check(t_character.rig.as_ref().unwrap(), &reference.t_cases);
}
#[test]
fn detail_levels_and_full_t_pose_are_native() {
    let lib = library();
    let mut recipe = AraRecipe::default();
    recipe.params["rig"] = json!("full");
    recipe.params["restPose"] = json!("T");
    recipe.params["detail"] = json!("high");
    let high = NativeCharacter::build(&lib, &recipe).unwrap();
    valid(&high);
    recipe.params["detail"] = json!("low");
    let low = NativeCharacter::build(&lib, &recipe).unwrap();
    valid(&low);
    assert!(high.triangle_count() > low.triangle_count());
    assert!(high.rig.as_ref().unwrap().bones.len() > 100);
}
#[test]
fn custom_target_axis_and_recipe_persistence() {
    let lib = library();
    let mut recipe = AraRecipe::default();
    recipe.params["rig"] = json!("none");
    recipe.params["outfit"] = json!("none");
    recipe.params["top"] = json!("none");
    recipe.params["bottom"] = json!("none");
    recipe.params["shoes"] = json!("none");
    let base = NativeCharacter::build(&lib, &recipe).unwrap();
    let v = base.parts[0].refs[0][0] as usize;
    recipe.custom_targets.push(CustomTarget {
        name: "test x".into(),
        weight: 1.,
        deltas: vec![(v, [0.1, 0., 0.])],
    });
    let changed = NativeCharacter::build(&lib, &recipe).unwrap();
    let index = changed.parts[0]
        .refs
        .iter()
        .position(|r| r[0] as usize == v)
        .unwrap();
    assert!(
        (changed.parts[0].positions[index][0] - base.parts[0].positions[index][0] - 0.01).abs()
            < 1e-5
    );
    let restored: AraRecipe =
        serde_json::from_str(&serde_json::to_string(&recipe).unwrap()).unwrap();
    assert_eq!(recipe, restored);
}

#[test]
fn breast_projection_moves_the_nipple_with_its_areola() {
    let lib = library();
    let base: Vec<[f32; 3]> = lib.base.arrays["positions"].f32().unwrap().chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect();
    let faces = lib.base.arrays["faceVerts"].u32().unwrap();
    let tip = lib.base.meta["joints"]["breast.L____tail"][0].as_u64().unwrap() as u32;
    let mut ring = std::collections::BTreeSet::new();
    for q in faces.chunks_exact(4).filter(|q| q.contains(&tip)) {
        ring.extend(q.iter().copied().filter(|&v| v != tip));
    }
    for projection in [1.25, 2.0] {
        let mut coords = base.clone();
        let settings = human_editor::native_breast::BreastSettings { projection, ..Default::default() };
        human_editor::native_breast::reshape(&lib, &mut coords, 0.0, &settings).unwrap();
        let moved = |v: u32| {
            let (a, b) = (base[v as usize], coords[v as usize]);
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let neighbours = ring.iter().map(|&v| moved(v)).fold(0., f32::max);
        // The tip rides its areola: no lone spike ahead of the surrounding skin.
        assert!(moved(tip) > 0. && moved(tip) <= neighbours * 1.15, "projection {projection}: tip {} vs ring {neighbours}", moved(tip));
    }
}

#[test]
fn cartoon_muscle_follows_the_superhuman_references() {
    let lib = library();
    let info = &lib.cartoon_offsets().unwrap().meta["cartoon"];
    for key in ["female_muscle", "male_muscle", "female_muscle_chest", "male_muscle_chest"] {
        assert!(info[key].is_string(), "{key} correction missing from cartoon.pack");
    }
    let body = |gender: f64, muscle: f64, cartoon: bool| {
        let mut r = AraRecipe::default();
        r.params["gender"] = json!(gender);
        r.params["muscle"] = json!(muscle);
        r.params["breastSize"] = json!(1.0);
        r.cartoon.enabled = cartoon;
        let c = NativeCharacter::build(&lib, &r).unwrap();
        c.parts.iter().find(|p| p.name == "Body").unwrap().positions.clone()
    };
    let moved = |a: &[[f32; 3]], b: &[[f32; 3]]| {
        a.iter().zip(b).map(|(x, y)| (0..3).map(|k| (x[k] - y[k]).powi(2)).sum::<f32>().sqrt()).sum::<f32>() / a.len() as f32
    };
    for gender in [0.0, 1.0] {
        // Muscle moves the cartoon body further than the realistic one: the Superhuman
        // correction adds to MakeHuman's own muscle targets.
        let real = moved(&body(gender, 0.5, false), &body(gender, 1.0, false));
        let cartoon = moved(&body(gender, 0.5, true), &body(gender, 1.0, true));
        assert!(cartoon > real * 1.3, "gender {gender}: cartoon muscle {cartoon} vs realistic {real}");
    }
}

#[test]
fn cartoon_lashes_are_a_lid_line_that_blinks() {
    let lib = library();
    let mut recipe = AraRecipe::default();
    recipe.params["gender"] = json!(0.0);
    recipe.params["faceShapes"] = json!(true);
    recipe.cartoon.enabled = true;
    let c = NativeCharacter::build(&lib, &recipe).unwrap();
    let lashes = c.parts.iter().find(|p| p.name == "Eyelashes").expect("lashes");
    assert_eq!(lashes.material["alpha"], json!("opaque"), "solid lash line, not strand cards");
    let blink = c.face_names.iter().position(|n| n == "eyeBlinkLeft").expect("blink shape");
    // Every lash vertex rides one lid-margin vertex: the left ribbon's middle (not its
    // corners) closes with the left lid.
    let moving = lashes.refs.iter().zip(&lashes.ref_weights).filter(|(r, w)| {
        w[0] == 1. && c.face_deltas[blink].get(r[0] as usize).is_some_and(|d| d.iter().any(|x| x.abs() > 1e-4))
    });
    let moving = moving.count();
    assert!(moving * 5 >= lashes.refs.len(), "lashes must follow the blinking lid: {moving}/{}", lashes.refs.len());
}

#[test]
fn cartoon_arms_spread_clear_of_the_wider_torso() {
    let lib = library();
    let mut recipe = AraRecipe::default();
    recipe.params["gender"] = json!(0.0);
    let spread = |r: &AraRecipe| NativeCharacter::build(&lib, r).unwrap().rig.unwrap().arm_spread();
    assert_eq!(spread(&recipe), 0., "realistic bodies keep the clips' arm angles");
    recipe.cartoon.enabled = true;
    let cartoon = spread(&recipe);
    assert!(cartoon > 0.05 && cartoon < 0.35, "cartoon hips need a moderate spread, got {cartoon}");
}

#[test]
fn cartoon_hm08_offsets_keep_makehuman_controls() {
    let lib = library();
    let mut recipe = AraRecipe::default();
    let natural = NativeCharacter::build(&lib, &recipe).unwrap();
    recipe.cartoon.enabled = true;
    let cartoon = NativeCharacter::build(&lib, &recipe).unwrap();
    let names = |c: &NativeCharacter| c.parts.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&natural), names(&cartoon), "cartoon keeps every MakeHuman part");
    for (a, b) in natural.parts.iter().zip(&cartoon.parts) {
        if a.name == "Hair" {
            // Cartoon mode swaps the strand cards for the solid shell (tools/cartoon_hair).
            assert_eq!(b.material["cartoonShell"], json!(true), "cartoon hair uses the shell");
            assert_ne!(a.indices, b.indices);
            continue;
        }
        if a.name == "Eyelashes" {
            // ...and the lash cards for the solid lid line (tools/cartoon_hm08/lashes.py).
            assert_ne!(a.indices, b.indices);
            continue;
        }
        assert_eq!(a.indices, b.indices, "{} topology changed", a.name);
        assert_ne!(a.positions, b.positions, "{} was not reshaped", a.name);
    }
    assert_eq!(natural.face_names, cartoon.face_names);
    assert_ne!(natural.face_deltas, cartoon.face_deltas, "expressions must be resized");
    let bones = |c: &NativeCharacter| c.rig.as_ref().unwrap().bones.len();
    assert_eq!(bones(&natural), bones(&cartoon));
    // All surfaces: default clothing hides parts of the body mesh.
    let built = |r: &AraRecipe| {
        NativeCharacter::build(&lib, r).unwrap().parts.iter().flat_map(|p| p.positions.clone()).collect::<Vec<_>>()
    };
    // Amount 0 is exactly MakeHuman.
    recipe.cartoon.strength = 0.;
    let all = |c: &NativeCharacter| c.parts.iter().flat_map(|p| p.positions.clone()).collect::<Vec<_>>();
    assert_eq!(built(&recipe), all(&natural));
    recipe.cartoon.strength = 1.;
    // MakeHuman macros keep their range on top of the cartoon shape.
    for (field, lo, hi) in [("gender", 0.0, 1.0), ("breastSize", 0.0, 1.0), ("age", 0.21, 1.0)] {
        let mut a = recipe.clone();
        a.params["gender"] = json!(0.0);
        let mut b = a.clone();
        a.params[field] = json!(lo);
        b.params[field] = json!(hi);
        let (pa, pb) = (built(&a), built(&b));
        let moved = pa.iter().zip(&pb).map(|(x, y)| (0..3).map(|k| (x[k] - y[k]).abs()).fold(0., f32::max)).fold(0., f32::max);
        assert!(moved > 0.01, "{field} has no effect on the cartoon body");
    }
    // A cartoon child stays a sane offset from the realistic child (no runaway deltas).
    let mut child = recipe.clone();
    child.params["age"] = json!(0.21);
    // The cartoon hair shell and lash line have their own vertices.
    child.params["hair"] = json!("none");
    child.params["eyelashes"] = json!("none");
    let cartoon_child = built(&child);
    child.cartoon.enabled = false;
    let real_child = built(&child);
    let worst = cartoon_child.iter().zip(&real_child).map(|(a, b)| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }).fold(0., f32::max);
    assert!(worst.is_finite() && worst < 0.5, "child offset {worst} m");
}
