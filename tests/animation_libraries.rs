use human_editor::{
    ara::AraRecipe, native_assets::NativeLibrary, native_character::NativeCharacter,
};
use std::sync::Arc;
#[test]
fn ual_retarget_stays_finite_and_grounded_on_rig_variants() {
    let lib = Arc::new(
        NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap(),
    );
    for (kind, pose) in [
        ("game", "A"),
        ("game", "T"),
        ("game_lite", "A"),
        ("full", "A"),
        ("full", "T"),
    ] {
        let mut r = AraRecipe::default();
        r.params["rig"] = kind.into();
        r.params["restPose"] = pose.into();
        r.preview_clip = "ual1:Walk_Loop".into();
        let c = NativeCharacter::build(&lib, &r).unwrap();
        let rig = c.rig.unwrap();
        for clip in ["ual1:A_TPose", "ual1:Walk_Loop", "ual2:Bandage_Loop"] {
            for t in [0., 0.3] {
                assert!(rig.clip_duration(clip).is_some(), "missing {clip}");
                let skin = rig.pose(clip, t);
                assert!(skin.iter().all(|m| m.is_finite()), "{kind}/{pose} {clip}");
                let point = |n: &str| {
                    let i = rig.bones.iter().position(|b| b.name == n).unwrap();
                    (skin[i] * rig.bind_world_matrices()[i])
                        .transform_point3(bevy::prelude::Vec3::ZERO)
                };
                let head = point(if kind == "full" { "head" } else { "Head" });
                assert!(
                    head.y > 1.1 && head.y < 2.2,
                    "{kind}/{pose} {clip} head {head:?}"
                );
                let feet = [
                    point(if kind == "full" { "foot.L" } else { "LeftFoot" }),
                    point(if kind == "full" {
                        "foot.R"
                    } else {
                        "RightFoot"
                    }),
                ];
                assert!(
                    feet.iter().all(|p| p.y > -0.15 && p.y < 0.7),
                    "{kind}/{pose} {clip} feet {feet:?}"
                );
                if clip == "ual1:A_TPose" {
                    let left = point(if kind == "full" {
                        "wrist.L"
                    } else {
                        "LeftHand"
                    });
                    let right = point(if kind == "full" {
                        "wrist.R"
                    } else {
                        "RightHand"
                    });
                    let shoulder = point(if kind == "full" {
                        "upperarm01.L"
                    } else {
                        "LeftArm"
                    });
                    assert!(
                        (left.y - shoulder.y).abs() < 0.10,
                        "T-pose arm is not horizontal: {left:?}/{shoulder:?}"
                    );
                    assert!(
                        (left.x - right.x).abs() > 0.8 && (left.y - right.y).abs() < 0.15,
                        "{kind}/{pose} T-pose hands {left:?} {right:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn ual_root_motion_keeps_source_distance_and_export_preserves_vertical_motion() {
    use bevy::prelude::Vec3;
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native"));
    let lib = NativeLibrary::load(root).unwrap();
    let mut recipe = AraRecipe::default();
    recipe.export.clips = vec!["ual1:Walk_Loop".into()];
    recipe.export.root_motion = false;
    let character = NativeCharacter::build(&lib, &recipe).unwrap();
    let rig = character.rig.as_ref().unwrap();
    for (clip, distance) in [("ual1:Walk_Loop", 1.3), ("ual2:Walk_Fwd_Loop", 1.35)] {
        let duration = rig.clip_duration(clip).unwrap();
        let start = rig.pose_clamped(clip, 0.)[0].transform_point3(rig.bones[0].head);
        let end = rig.pose_clamped(clip, duration)[0].transform_point3(rig.bones[0].head);
        assert!(
            (end.z - start.z - distance * character.height / 1.8).abs() < 0.025,
            "wrong root displacement {clip}: {:?}",
            end - start
        );
    }
    let output = std::env::temp_dir().join(format!("ual-inplace-{}.glb", std::process::id()));
    human_editor::native_export::export_character(&character, &recipe, root, &output).unwrap();
    let document = gltf::Gltf::open(&output).unwrap();
    let blob = document.blob.as_deref().unwrap();
    let animation = document.animations().next().unwrap();
    let channel = animation
        .channels()
        .find(|c| {
            c.target().node().name() == Some("Hips")
                && c.target().property() == gltf::animation::Property::Translation
        })
        .expect("in-place export must retain pelvis bob");
    let reader = channel.reader(|_| Some(blob));
    let gltf::animation::util::ReadOutputs::Translations(values) = reader.read_outputs().unwrap()
    else {
        panic!("not translation")
    };
    let values: Vec<Vec3> = values.map(Vec3::from).collect();
    assert!(
        values
            .iter()
            .all(|p| (p.x - values[0].x).abs() < 1e-5 && (p.z - values[0].z).abs() < 1e-5)
    );
    assert!(
        values.iter().any(|p| (p.y - values[0].y).abs() > 0.005),
        "pelvis bob removed"
    );
    std::fs::remove_file(output).unwrap();
}
