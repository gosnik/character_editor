use human_editor::{
    ara::AraRecipe, native_assets::NativeLibrary, native_character::NativeCharacter,
};

#[test]
fn zero_strength_matches_natural_geometry_materials_and_rig() {
    let lib = NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap();
    let mut recipe = AraRecipe::default();
    let natural = NativeCharacter::build(&lib, &recipe).unwrap();
    recipe.cartoon.enabled = true;
    recipe.cartoon.strength = 0.;
    let zero = NativeCharacter::build(&lib, &recipe).unwrap();
    assert_eq!(natural.parts.len(), zero.parts.len());
    for (a, b) in natural.parts.iter().zip(&zero.parts) {
        assert_eq!(a.name, b.name);
        assert_eq!(
            a.positions, b.positions,
            "{} moved at zero strength",
            a.name
        );
        assert_eq!(a.indices, b.indices);
        assert_eq!(a.uvs, b.uvs);
        assert_eq!(a.material, b.material);
    }
    assert_eq!(natural.face_deltas, zero.face_deltas);
    for (a, b) in natural
        .rig
        .unwrap()
        .bones
        .iter()
        .zip(zero.rig.unwrap().bones.iter())
    {
        assert_eq!(a.name, b.name);
        assert_eq!(a.head, b.head);
    }
}
