use human_editor::{
    ara::AraRecipe,
    native_assets::NativeLibrary,
    native_character::NativeCharacter,
    native_export::export_character,
    wardrobe::WardrobeProfile,
    wardrobe_assets::{CustomGarment, GarmentSlot, import_obj},
};
use serde_json::json;

fn library() -> NativeLibrary {
    NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap()
}

fn beard(lib: &NativeLibrary, id: &str) -> CustomGarment {
    // A small UV-mapped jaw patch tests the actual importer and body bindings.
    import_obj(lib,
        "v -.035 .63 .10\nv .035 .63 .10\nv 0 .58 .11\nvt 0 0\nvt 1 0\nvt .5 1\nf 1/1 2/2 3/3\n",
        id, "beard", "m").unwrap()
}

#[test]
fn imported_beard_binds_only_to_head_region_even_below_the_head_floor() {
    let lib = library();
    // The final point is below the head region's canonical lower Y boundary;
    // generic nearest-surface binding would otherwise be free to choose neck
    // vertices.  A beard must still remain bound to head/jaw region 0.
    let garment = beard(&lib, "head-only-bind");
    assert!(garment.positions_dm.iter().any(|point| point[1] < 6.16));
    let regions = lib.base.arrays["regions"].u8().unwrap();
    assert!(garment
        .refs
        .iter()
        .flatten()
        .all(|&reference| regions[reference as usize] == 0));
}

#[test]
fn non_beard_imports_keep_the_existing_non_hidden_binding_domain() {
    let lib = library();
    let garment = import_obj(&lib,
        "v -.035 .30 .10\nv .035 .30 .10\nv 0 .25 .11\nvt 0 0\nvt 1 0\nvt .5 1\nf 1/1 2/2 3/3\n",
        "non-beard-bind", "top", "m").unwrap();
    let regions = lib.base.arrays["regions"].u8().unwrap();
    assert!(garment.refs.iter().flatten().any(|&reference| regions[reference as usize] != 0));
    assert!(garment.refs.iter().flatten().all(|&reference| regions[reference as usize] != 255));
}

#[test]
fn beard_selection_replaces_only_the_beard_and_roundtrips() {
    let lib = library();
    let first = beard(&lib, "first-beard");
    let second = beard(&lib, "second-beard");
    let mut hair = first.clone();
    hair.id = "independent-hair".into();
    hair.slot = GarmentSlot::Hair;
    let mut recipe = AraRecipe::default();
    recipe.select_custom_garment(&hair).unwrap();
    recipe.params["outfit"] = json!("male_casualsuit01");
    recipe.params["hat"] = json!("baseball_cap");
    recipe.params["facialHair"] = json!("full_beard");
    recipe.select_custom_garment(&first).unwrap();
    recipe.select_custom_garment(&second).unwrap();
    assert_eq!(recipe.params["facialHair"], "none");
    assert_eq!(recipe.params["outfit"], "male_casualsuit01");
    assert_eq!(recipe.params["hat"], "baseball_cap");
    assert_eq!(recipe.custom_garments.len(), 2);
    assert!(recipe.custom_garments.iter().any(|g| g.id == hair.id));
    assert!(!recipe.custom_garments.iter().any(|g| g.id == first.id));
    let restored: AraRecipe = serde_json::from_value(serde_json::to_value(&recipe).unwrap()).unwrap();
    assert_eq!(restored.custom_garments, recipe.custom_garments);
}

#[test]
fn beard_profiles_mix_without_replacing_hair_or_body() {
    let lib = library();
    let mut source = AraRecipe::default();
    source.select_custom_garment(&beard(&lib, "source-beard")).unwrap();
    let profile = WardrobeProfile::capture_with_beard(&source, "Beard", false, true).unwrap();
    assert!(profile.params.get("hair").is_none());
    assert!(profile.params.get("facialHair").is_some());
    let clothing = WardrobeProfile::capture_with_beard(&source, "Clothing", false, false).unwrap();
    assert!(clothing.custom_garments.is_empty());
    let mut destination = AraRecipe::default();
    let mut hair = beard(&lib, "keep-hair");
    hair.slot = GarmentSlot::Hair;
    destination.select_custom_garment(&hair).unwrap();
    destination.select_custom_garment(&beard(&lib, "old-beard")).unwrap();
    destination.params["gender"] = json!(0.);
    destination.params["height"] = json!(0.8);
    destination.cartoon.enabled = true;
    profile.apply(&mut destination).unwrap();
    assert_eq!(destination.params["gender"], 0.);
    assert_eq!(destination.params["height"], 0.8);
    assert!(destination.cartoon.enabled);
    assert_eq!(destination.custom_garments.len(), 2);
    assert!(destination.custom_garments.iter().any(|g| g.id == "keep-hair"));
    assert!(destination.custom_garments.iter().any(|g| g.id == "source-beard"));
}

#[test]
fn custom_beard_follows_cartoon_body_and_exports_skinned_animation() {
    let lib = library();
    let custom = beard(&lib, "animated-beard");
    let mut previous = None;
    for gender in [0., 1.] {
        for strength in [0., 1.] {
            let mut recipe = AraRecipe::default();
            recipe.params["gender"] = json!(gender);
            recipe.params["hair"] = json!("short01");
            // Directly loaded recipes can contain conflicting builtin hair.
            recipe.params["facialHair"] = json!("full_beard");
            recipe.custom_garments.push(custom.clone());
            recipe.cartoon.enabled = true;
            recipe.cartoon.strength = strength;
            recipe.export.mode = "animated".into();
            recipe.export.clips = vec!["idle".into(), "walk".into()];
            let character = NativeCharacter::build(&lib, &recipe).unwrap();
            assert!(!character.parts.iter().any(|p| p.name.starts_with("facialHair:") || p.name == "tattoo_stubble"));
            assert!(character.parts.iter().any(|p| p.name == "Hair"));
            let part = character.parts.iter().find(|p| p.name == "custom:animated-beard").unwrap();
            assert_eq!(part.positions.len(), 3);
            assert_eq!(part.positions.len(), part.refs.len());
            assert!(part.positions.iter().flatten().all(|v| v.is_finite()));
            if let Some(positions) = &previous {
                if strength == 1. { assert_ne!(positions, &part.positions); }
            }
            previous = Some(part.positions.clone());
            if gender == 1. && strength == 1. {
                let path = std::env::temp_dir().join(format!("custom-beard-test-{}.glb", std::process::id()));
                export_character(&character, &recipe, &lib.root, &path).unwrap();
                let bytes = std::fs::read(&path).unwrap();
                let glb = gltf::Gltf::from_slice(&bytes).unwrap();
                let node = glb.nodes().find(|n| n.name() == Some("custom:animated-beard")).unwrap();
                assert!(node.skin().is_some());
                let primitive = node.mesh().unwrap().primitives().next().unwrap();
                assert!(primitive.get(&gltf::Semantic::Joints(0)).is_some());
                assert!(primitive.morph_targets().count() > 0, "Beard needs expression targets");
                let reader = primitive.reader(|buffer| match buffer.source() {
                    gltf::buffer::Source::Bin => glb.blob.as_deref(),
                    _ => None,
                });
                assert!(reader.read_morph_targets().any(|(positions, _, _)|
                    positions.is_some_and(|mut points| points.any(|p| p.iter().any(|v| v.abs() > 1e-7)))),
                    "At least one expression must move the beard's face bindings");
                for clip in ["idle", "walk"] { assert!(glb.animations().any(|a| a.name() == Some(clip))); }
                std::fs::remove_file(path).unwrap();
            }
        }
    }
}
