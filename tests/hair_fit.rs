use human_editor::{
    ara::AraRecipe, native_assets::NativeLibrary, native_character::NativeCharacter,
};
use serde_json::json;

#[test]
fn long_hair_with_coat_keeps_valid_bindings_and_bounded_clearance() {
    let lib = NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap();
    let mut recipe = AraRecipe::default();
    recipe.params["gender"] = json!(0.);
    recipe.params["hair"] = json!("long01");
    recipe.params["jacket"] = json!("overcoat");
    let character = NativeCharacter::build(&lib, &recipe).unwrap();
    let hair = character.parts.iter().find(|p| p.name == "Hair").unwrap();
    assert!(character.hair_fit.corrected_vertices > 0);
    assert!(
        character.hair_fit.max_displacement
            <= human_editor::native_hair::MAX_HAIR_CORRECTION + 1e-5
    );
    assert_eq!(hair.positions.len(), hair.refs.len());
    assert!(hair.positions.iter().flatten().all(|v| v.is_finite()));
    recipe.params["hair"] = json!("mohawk");
    recipe.params["hat"] = json!("baseball_cap");
    let hatted = NativeCharacter::build(&lib, &recipe).unwrap();
    assert!(
        !hatted
            .parts
            .iter()
            .any(|p| p.name == "Hair" || p.name.starts_with("hair:"))
    );
}
