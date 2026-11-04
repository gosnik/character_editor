use human_editor::{
    ara::AraRecipe, native_assets::NativeLibrary, native_character::NativeCharacter,
    native_morph::apply_targets, native_wardrobe::build_wardrobe, wardrobe_assets::import_obj,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(serde::Deserialize)]
struct SourceCoverage {
    hidden_faces: BTreeMap<String, Vec<u32>>,
}
fn source_coverage() -> SourceCoverage {
    let file = std::fs::File::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/native/wardrobe.json.gz"
    ))
    .unwrap();
    serde_json::from_reader(flate2::read::GzDecoder::new(file)).unwrap()
}

#[test]
fn underwear_preserves_body_faces_adjacent_to_its_coverage() {
    let lib = NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap();
    let mut recipe = AraRecipe::default();
    recipe.params["gender"] = json!(0.0);
    for key in [
        "outfit", "top", "bottom", "jacket", "shoes", "hat", "carry", "weapon",
    ] {
        recipe.params[key] = json!("none");
    }
    recipe.params["accessories"] = json!([]);
    recipe.params["detail"] = json!("medium");
    recipe.params["underwear"] = json!(false);
    let bare = NativeCharacter::build(&lib, &recipe).unwrap();
    recipe.params["underwear"] = json!(true);
    let coords = apply_targets(&lib, &recipe.params).unwrap();
    let wardrobe = build_wardrobe(&lib, &coords, &recipe.params).unwrap();
    let source = source_coverage();
    let source_mask: BTreeSet<_> = source.hidden_faces["briefs"].iter().copied().collect();
    // Fixture from the pinned Ara shell builder, not a mask inferred by Rust.
    assert_eq!(source_mask.len(), 204);
    assert_eq!(source_mask.first(), Some(&3923));
    assert_eq!(source_mask.last(), Some(&13218));
    let hidden: BTreeSet<_> = wardrobe.hidden_faces.iter().copied().collect();
    assert!(hidden.is_subset(&source_mask));
    assert!(
        hidden.len() < source_mask.len(),
        "skin beneath garment hems must be retained"
    );
    assert!(!hidden.is_empty(), "fixture must exercise body occlusion");
    let dressed = NativeCharacter::build(&lib, &recipe).unwrap();
    let bare = bare.parts.iter().find(|p| p.name == "Body").unwrap();
    let body = dressed.parts.iter().find(|p| p.name == "Body").unwrap();
    assert_eq!(
        bare.indices.len() - body.indices.len(),
        hidden.len() * 6,
        "underwear must remove exactly its covered quads, without erasing neighboring skin"
    );
    let visible_triangles: BTreeSet<_> = body
        .indices
        .chunks_exact(3)
        .map(|tri| {
            let mut refs = std::array::from_fn::<_, 3, _>(|i| body.refs[tri[i] as usize][0]);
            refs.sort_unstable();
            refs
        })
        .collect();
    let faces = lib.base.arrays["faceVerts"].u32().unwrap();
    let covered_vertices: BTreeSet<_> = hidden
        .iter()
        .flat_map(|&f| faces[f as usize * 4..f as usize * 4 + 4].iter().copied())
        .collect();
    let mut adjacent = 0;
    for (index, face) in faces.chunks_exact(4).enumerate() {
        if !hidden.contains(&(index as u32)) && face.iter().any(|v| covered_vertices.contains(v)) {
            adjacent += 1;
            for mut tri in [[face[0], face[1], face[2]], [face[0], face[2], face[3]]] {
                tri.sort_unstable();
                assert!(
                    visible_triangles.contains(&tri),
                    "uncovered waist/thigh face {index} was removed"
                );
            }
        }
    }
    assert!(adjacent > 0);
}

#[test]
fn custom_garment_mask_hides_its_face_but_preserves_an_adjacent_face() {
    let lib = NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap();
    let faces = lib.base.arrays["faceVerts"].u32().unwrap();
    let (masked_face, adjacent_face) = faces
        .chunks_exact(4)
        .enumerate()
        .find_map(|(masked_index, masked)| {
            faces
                .chunks_exact(4)
                .enumerate()
                .skip(masked_index + 1)
                .find(|(_, neighbor)| {
                    masked
                        .iter()
                        .filter(|vertex| neighbor.contains(vertex))
                        .count()
                        >= 2
                })
                .map(|(adjacent_index, _)| (masked_index as u32, adjacent_index as u32))
        })
        .expect("native body must contain adjacent quad faces");
    let mut recipe = AraRecipe::default();
    for key in [
        "outfit", "top", "bottom", "jacket", "shoes", "hat", "carry", "weapon",
    ] {
        recipe.params[key] = json!("none");
    }
    recipe.params["accessories"] = json!([]);
    recipe.params["underwear"] = json!(false);
    recipe.params["detail"] = json!("medium");
    let bare = NativeCharacter::build(&lib, &recipe).unwrap();
    let mut garment = import_obj(
        &lib,
        "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "custom-mask",
        "top",
        "dm",
    )
    .unwrap();
    garment.hidden_faces = vec![masked_face];
    recipe.custom_garments.push(garment);
    let masked = NativeCharacter::build(&lib, &recipe).unwrap();
    let bare_body = bare.parts.iter().find(|part| part.name == "Body").unwrap();
    let masked_body = masked
        .parts
        .iter()
        .find(|part| part.name == "Body")
        .unwrap();
    assert_eq!(
        bare_body.indices.len() - masked_body.indices.len(),
        6,
        "a custom hiddenFaces entry must remove exactly one quad"
    );
    let visible_triangles: BTreeSet<_> = masked_body
        .indices
        .chunks_exact(3)
        .map(|triangle| {
            let mut refs =
                std::array::from_fn::<_, 3, _>(|i| masked_body.refs[triangle[i] as usize][0]);
            refs.sort_unstable();
            refs
        })
        .collect();
    let adjacent = &faces[adjacent_face as usize * 4..adjacent_face as usize * 4 + 4];
    for mut triangle in [
        [adjacent[0], adjacent[1], adjacent[2]],
        [adjacent[0], adjacent[2], adjacent[3]],
    ] {
        triangle.sort_unstable();
        assert!(
            visible_triangles.contains(&triangle),
            "adjacent face {adjacent_face} must remain visible"
        );
    }
}

#[test]
fn custom_hair_replaces_built_in_hair_without_changing_hat_selection() {
    let lib = NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap();
    let mut recipe = AraRecipe::default();
    recipe.params["hair"] = json!("cornrows");
    recipe.params["hat"] = json!("baseball_cap");
    let hair = import_obj(
        &lib,
        "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "custom-crown",
        "hair",
        "dm",
    )
    .unwrap();
    recipe.select_custom_garment(&hair).unwrap();
    assert_eq!(recipe.params["hair"], "none");
    assert_eq!(recipe.params["hat"], "baseball_cap");
    // A hat also hides native hair, so remove it before checking that the
    // custom Hair slot itself is what suppresses the selected built-in style.
    recipe.params["hat"] = json!("none");
    recipe.params["hair"] = json!("cornrows");
    let character = NativeCharacter::build(&lib, &recipe).unwrap();
    let names: Vec<_> = character
        .parts
        .iter()
        .map(|part| part.name.as_str())
        .collect();
    assert!(names.contains(&"custom:custom-crown"));
    assert!(
        names
            .iter()
            .all(|name| *name != "Hair" && !name.starts_with("hair:"))
    );
}

#[test]
fn boots_occlusion_is_preserved_at_low_detail() {
    let lib = NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap();
    let mut recipe = AraRecipe::default();
    for key in [
        "outfit", "top", "bottom", "jacket", "hat", "carry", "weapon",
    ] {
        recipe.params[key] = json!("none");
    }
    recipe.params["accessories"] = json!([]);
    recipe.params["underwear"] = json!(false);
    recipe.params["shoes"] = json!("combat_boots");
    recipe.params["detail"] = json!("medium");
    let coords = apply_targets(&lib, &recipe.params).unwrap();
    let wardrobe = build_wardrobe(&lib, &coords, &recipe.params).unwrap();
    assert!(
        !wardrobe.hidden_faces.is_empty(),
        "boot shafts must occlude the underlying legs"
    );
    let source = source_coverage();
    assert_eq!(source.hidden_faces["boots"].len(), 160);
    assert_eq!(source.hidden_faces["boots"].first(), Some(&4361));
    assert_eq!(source.hidden_faces["boots"].last(), Some(&13188));
    assert_ne!(
        source.hidden_faces["boots"], source.hidden_faces["briefs"],
        "a boot bake must not contain auto-added underwear"
    );
    let medium = NativeCharacter::build(&lib, &recipe).unwrap();
    recipe.params["detail"] = json!("low");
    let low = NativeCharacter::build(&lib, &recipe).unwrap();
    let body = |c: &NativeCharacter| {
        c.parts
            .iter()
            .find(|p| p.name == "Body")
            .unwrap()
            .indices
            .len()
    };
    assert_eq!(
        body(&medium),
        body(&low),
        "low detail must preserve the canonical visibility mask"
    );
    assert_eq!(
        body(&low),
        (lib.base.arrays["faceVerts"].u32().unwrap().len() / 4 - wardrobe.hidden_faces.len()) * 6
    );
}
