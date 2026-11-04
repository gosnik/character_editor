use human_editor::{
    CharacterLibrary, CharacterRecipe,
    model::{parse_obj, parse_target},
};
fn library() -> CharacterLibrary {
    CharacterLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/makehuman")).unwrap()
}
#[test]
fn shipped_assets_evaluate_every_extreme_without_drift() {
    let library = library();
    assert_eq!(library.base_positions().len(), 19158);
    let neutral = library.deform(&CharacterRecipe::default()).unwrap();
    for target in &library.targets {
        for weight in [-1., 1.] {
            let mut recipe = CharacterRecipe::default();
            recipe.weights.insert(target.definition.id.clone(), weight);
            let modified = library.deform(&recipe).unwrap();
            assert!(modified.iter().flatten().all(|x| x.is_finite()));
            assert_ne!(
                neutral, modified,
                "target {} has no effect",
                target.definition.id
            );
            assert_eq!(modified, library.deform(&recipe).unwrap());
        }
    }
    assert_eq!(
        neutral,
        library.deform(&CharacterRecipe::default()).unwrap()
    );
    let floor = library
        .triangles()
        .iter()
        .map(|&i| neutral[i as usize][1])
        .fold(f32::INFINITY, f32::min);
    assert_eq!(floor, 0.);
}
#[test]
fn target_parser_rejects_corruption_instead_of_panicking() {
    assert!(parse_target("9 0 0 0", 9).is_err());
    assert!(parse_target("0 NaN 0 0", 9).is_err());
    assert!(parse_target("0 1 0 0\n0 2 0 0", 9).is_err());
    assert!(parse_target("0 1 2", 9).is_err());
    assert_eq!(
        parse_target("# hm08\n\n0 .1 -.2 0", 1).unwrap(),
        vec![(0, [0.1, -0.2, 0.])]
    );
}
#[test]
fn obj_keeps_original_target_indices_but_omits_helpers() {
    let obj = "v 0 0 0\nv 10 0 0\nv 0 10 0\nv 0 0 10\ng body\nf 1/1 2/2 3/3\ng helper-tights\nf 1 3 4\ng joint-ground\nf 2 3 4";
    let (positions, triangles) = parse_obj(obj).unwrap();
    assert_eq!(positions.len(), 4);
    assert_eq!(positions[1], [1., 0., 0.]);
    assert_eq!(triangles, vec![0, 1, 2]);
    assert!(parse_obj("v 0 0 0\nf 1 2 3").is_err());
}
#[test]
fn recipes_and_export_roundtrip_and_validate() {
    let library = library();
    let directory = std::env::temp_dir().join(format!("human-editor-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut recipe = CharacterRecipe::default();
    recipe.weights.insert("height".into(), 0.4);
    recipe
        .save(directory.join("character.json"), &library)
        .unwrap();
    assert_eq!(
        recipe,
        CharacterRecipe::load(directory.join("character.json"), &library).unwrap()
    );
    library
        .export_obj(&recipe, directory.join("character.obj"))
        .unwrap();
    let (positions, triangles) =
        parse_obj(&std::fs::read_to_string(directory.join("character.obj")).unwrap()).unwrap();
    assert!(
        positions.len() < library.base_positions().len(),
        "export includes helper vertices"
    );
    assert_eq!(triangles.len(), library.triangles().len());
    recipe.weights.insert("unknown".into(), 0.);
    assert!(library.deform(&recipe).is_err());
    recipe.weights.remove("unknown");
    recipe.weights.insert("height".into(), f32::NAN);
    assert!(library.deform(&recipe).is_err());
    recipe.weights.clear();
    recipe.version = 2;
    assert!(library.validate(&recipe).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn concurrent_writes_with_same_stem_keep_destinations_separate() {
    use human_editor::model::atomic_write;
    let root = std::env::temp_dir().join(format!("human-editor-writes-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let path = root.join(format!("character.{n}"));
            std::thread::spawn(move || {
                let payload = vec![b'A' + n; 100_000];
                for _ in 0..10 {
                    atomic_write(&path, &payload).unwrap();
                }
                assert_eq!(std::fs::read(path).unwrap(), payload);
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 8);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn editor_mode_switch_preserves_native_recipe_and_saved_draft() {
    let mut recipe = CharacterRecipe::default();
    recipe.enter_full_editor();
    recipe.customization.as_mut().unwrap().params["height"] = serde_json::json!(0.8);
    recipe
        .customization
        .as_mut()
        .unwrap()
        .custom_targets
        .push(human_editor::ara::CustomTarget {
            name: "custom nose".into(),
            weight: 0.3,
            deltas: vec![(10, [0.1, 0.2, 0.3])],
        });
    let original = recipe.clone();
    recipe.enter_basic_editor();
    assert!(recipe.customization.is_none());
    assert_eq!(recipe.native_draft, original.customization);
    let saved = serde_json::to_string(&recipe).unwrap();
    let mut restored: CharacterRecipe = serde_json::from_str(&saved).unwrap();
    restored.enter_full_editor();
    assert_eq!(restored, original);
}
