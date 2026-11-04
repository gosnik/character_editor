use human_editor::{
    ara::AraRecipe,
    native_assets::NativeLibrary,
    native_character::NativeCharacter,
    wardrobe_assets::{CustomGarment, GarmentSlot},
};
use serde_json::json;

fn library() -> NativeLibrary {
    NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap()
}

fn face_for(lib: &NativeLibrary, region: u8, crown: bool) -> ([u32; 3], [[f32; 3]; 3]) {
    let positions = lib.base.arrays["positions"].f32().unwrap();
    let regions = lib.base.arrays["regions"].u8().unwrap();
    let faces = lib.base.arrays["faceVerts"].u32().unwrap();
    let mut best = None;
    for face in faces.chunks_exact(4) {
        let refs = [face[0], face[1], face[2]];
        if refs.iter().all(|&index| regions[index as usize] == region) {
            let points = refs.map(|index| {
                let start = index as usize * 3;
                [positions[start], positions[start + 1], positions[start + 2]]
            });
            let ab = sub(points[1], points[0]);
            let ac = sub(points[2], points[0]);
            if length(cross(ab, ac)) > 0.001 {
                if !crown {
                    return (refs, points);
                }
                let height = (points[0][1] + points[1][1] + points[2][1]) / 3.;
                if best.is_none_or(|(_, best_height)| height > best_height) {
                    best = Some(((refs, points), height));
                }
            }
        }
    }
    best.map(|(face, _)| face)
        .unwrap_or_else(|| panic!("native base has no non-degenerate region-{region} triangle"))
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn tiny_garment(lib: &NativeLibrary, id: &str, slot: GarmentSlot, region: u8) -> CustomGarment {
    let (refs, points) = face_for(lib, region, slot != GarmentSlot::Beard);
    let anchor = scale(add(add(points[0], points[1]), points[2]), 1. / 3.);
    let mut normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
    normal = scale(normal, 1. / length(normal));
    let tangent = scale(
        sub(points[1], points[0]),
        1. / length(sub(points[1], points[0])),
    );
    // The first vertex is the moving body anchor.  The other two are small
    // outward residuals, so a frame-aware fit must alter their local vectors
    // when the underlying head/torso is reshaped.
    let positions = vec![
        anchor,
        add(add(anchor, scale(normal, 0.08)), scale(tangent, 0.015)),
        add(add(anchor, scale(normal, 0.065)), scale(tangent, -0.015)),
    ];
    CustomGarment {
        id: id.into(),
        name: id.into(),
        slot,
        positions_dm: positions,
        indices: vec![0, 1, 2],
        uvs: vec![[0., 0.], [1., 0.], [0., 1.]],
        refs: vec![refs; 3],
        weights: vec![[1. / 3.; 3]; 3],
        reference_points_dm: vec![anchor; 3],
        material: json!({"color":"#ffffff", "roughness":0.8}),
        hidden_faces: vec![],
    }
}

fn custom_part<'a>(
    character: &'a NativeCharacter,
    id: &str,
) -> &'a human_editor::native_character::NativePart {
    character
        .parts
        .iter()
        .find(|part| part.name == format!("custom:{id}"))
        .unwrap()
}

fn body_vertex(body: &human_editor::native_character::NativePart, reference: u32) -> [f32; 3] {
    body.refs
        .iter()
        .zip(&body.positions)
        .find_map(|(refs, position)| (*refs == [reference; 3]).then_some(*position))
        .unwrap()
}

fn body_face_normal(character: &NativeCharacter, refs: [u32; 3]) -> [f32; 3] {
    let body = character
        .parts
        .iter()
        .find(|part| part.name == "Body")
        .unwrap();
    let points = refs.map(|reference| body_vertex(body, reference));
    let mut normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
    normal = scale(normal, 1. / length(normal));
    normal
}

#[test]
fn custom_hair_beard_and_top_adapt_to_cartoon_head_and_body_morphs() {
    let lib = library();
    let mut reference_recipe = AraRecipe::default();
    reference_recipe.params["hair"] = json!("none");
    reference_recipe.params["facialHair"] = json!("none");
    reference_recipe.params["rig"] = json!("none");
    let mut hair = tiny_garment(&lib, "adaptive-hair", GarmentSlot::Hair, 0);
    hair.material["cartoonShell"] = json!(true);
    let beard = tiny_garment(&lib, "adaptive-beard", GarmentSlot::Beard, 0);
    // Region 2 is torso/body rather than the head-only domain used above.
    let top = tiny_garment(&lib, "adaptive-top", GarmentSlot::Top, 2);
    let authored = vec![hair.clone(), beard.clone(), top.clone()];

    let mut neutral_recipe = AraRecipe::default();
    neutral_recipe.params["hair"] = json!("none");
    neutral_recipe.params["facialHair"] = json!("none");
    neutral_recipe.params["rig"] = json!("none");
    neutral_recipe.custom_garments = authored.clone();
    let neutral = NativeCharacter::build(&lib, &neutral_recipe).unwrap();

    let mut recipe = neutral_recipe.clone();
    recipe.cartoon.enabled = true;
    recipe.cartoon.strength = 1.;
    recipe.params["modifiers"] = json!({
        "head/head-angle": 0.85,
        "head/head-scale-horiz": 0.9,
        "head/head-scale-depth": -0.8,
        "head/head-scale-vert": 0.7,
        "torso/torso-scale-horiz": 0.8,
        "torso/torso-scale-depth": -0.75,
        "torso/torso-scale-vert": 0.7
    });
    let adapted = NativeCharacter::build(&lib, &recipe).unwrap();

    // Building must not alter the serialized authored garment contract.
    assert_eq!(recipe.custom_garments, authored);
    for id in ["adaptive-hair", "adaptive-beard", "adaptive-top"] {
        let before = custom_part(&neutral, id);
        let after = custom_part(&adapted, id);
        assert_eq!(after.positions.len(), 3);
        assert!(after.positions.iter().flatten().all(|v| v.is_finite()));
        // The anchor remains attached, while its outward residual is still
        // present and has changed under the non-uniform local deformation.
        let old_offset = sub(before.positions[1], before.positions[0]);
        let new_offset = sub(after.positions[1], after.positions[0]);
        let normal = body_face_normal(&adapted, after.refs[0]);
        assert!(
            normal[0] * new_offset[0] + normal[1] * new_offset[1] + normal[2] * new_offset[2]
                > 1e-5,
            "{id} no longer has an outward offset: normal={normal:?}, old={old_offset:?}, offset={new_offset:?}"
        );
        assert!(
            length(sub(new_offset, old_offset)) > 0.0001,
            "{id} retained a world-space residual"
        );
    }
}

#[test]
fn imported_cartoon_shell_hair_keeps_neck_clearance_while_builtin_shell_is_exempt() {
    let lib = library();
    let mut reference_recipe = AraRecipe::default();
    reference_recipe.params["hair"] = json!("none");
    reference_recipe.params["rig"] = json!("none");
    // Bind the imported hair to the upper torso and put its vertices on the
    // body surface.  This lies outside the excluded scalp volume, so the
    // runtime clearance pass must push the custom shell outward.
    let mut imported = tiny_garment(&lib, "colliding-cartoon-shell", GarmentSlot::Hair, 2);
    imported.positions_dm = imported.reference_points_dm.clone();
    imported.material["cartoonShell"] = json!(true);
    let mut recipe = reference_recipe.clone();
    recipe.custom_garments.push(imported);
    let fitted = NativeCharacter::build(&lib, &recipe).unwrap();
    assert!(fitted.hair_fit.corrected_vertices > 0);
    assert!(fitted.hair_fit.max_displacement <= human_editor::native_hair::MAX_HAIR_CORRECTION);

    let mut builtin_recipe = AraRecipe::default();
    builtin_recipe.params["hair"] = json!("short01");
    builtin_recipe.params["rig"] = json!("none");
    builtin_recipe.cartoon.enabled = true;
    builtin_recipe.cartoon.strength = 1.;
    let builtin = NativeCharacter::build(&lib, &builtin_recipe).unwrap();
    assert!(
        builtin
            .parts
            .iter()
            .any(|part| part.name == "Hair" && part.material["cartoonShell"] == true)
    );
    assert_eq!(builtin.hair_fit.corrected_vertices, 0);
}
