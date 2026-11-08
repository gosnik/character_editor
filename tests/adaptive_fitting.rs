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

fn face_index(lib: &NativeLibrary, refs: [u32; 3]) -> u32 {
    lib.base.arrays["faceVerts"]
        .u32()
        .unwrap()
        .chunks_exact(4)
        .position(|face| face[..3] == refs)
        .unwrap_or_else(|| panic!("native base has no face for {refs:?}")) as u32
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

fn body_part(character: &NativeCharacter) -> &human_editor::native_character::NativePart {
    character
        .parts
        .iter()
        .find(|part| part.name == "Body")
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
fn custom_beard_does_not_repel_scalp_hair_but_a_hat_still_does() {
    let lib = library();
    for gender in [0., 1.] {
        let mut recipe = AraRecipe::default();
        recipe.cartoon.enabled = true;
        recipe.cartoon.strength = 1.;
        for key in [
            "hair",
            "facialHair",
            "hat",
            "top",
            "bottom",
            "jacket",
            "shoes",
            "outfit",
            "rig",
        ] {
            recipe.params[key] = json!("none");
        }
        recipe.params["gender"] = json!(gender);
        let hair = tiny_garment(&lib, "scalp-blend", GarmentSlot::Hair, 0);
        recipe.custom_garments = vec![hair.clone()];
        let bare = NativeCharacter::build(&lib, &recipe).unwrap();
        let original = custom_part(&bare, "scalp-blend").positions.clone();

        // A coincident source surface exercises the collider path rather
        // than relying on an incidental gap between a beard and hairstyle.
        let mut companion = hair.clone();
        companion.id = "overlapping-companion".into();
        companion.slot = GarmentSlot::Beard;
        recipe.custom_garments.push(companion.clone());
        let bearded = NativeCharacter::build(&lib, &recipe).unwrap();
        assert_eq!(custom_part(&bearded, "scalp-blend").positions, original);
        assert!(
            !custom_part(&bearded, "overlapping-companion")
                .indices
                .is_empty()
        );

        // Solid headwear must retain clearance, so excluding all custom
        // companions would not satisfy this regression.
        companion.slot = GarmentSlot::Hat;
        recipe.custom_garments[1] = companion;
        let hatted = NativeCharacter::build(&lib, &recipe).unwrap();
        assert_ne!(custom_part(&hatted, "scalp-blend").positions, original);
    }
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

#[test]
fn empty_adaptive_coverage_keeps_the_saved_body_mask() {
    let lib = library();
    let (refs, _) = face_for(&lib, 2, false);
    let hidden_face = face_index(&lib, refs);
    let mut saved = tiny_garment(&lib, "saved-mask", GarmentSlot::Top, 2);
    saved.hidden_faces = vec![hidden_face];

    let mut baseline_recipe = AraRecipe::default();
    baseline_recipe.params["hair"] = json!("none");
    baseline_recipe.params["rig"] = json!("none");
    baseline_recipe.custom_garments.push(saved.clone());
    let baseline = NativeCharacter::build(&lib, &baseline_recipe).unwrap();

    saved.material["fitBodyMaskCandidates"] = json!([hidden_face]);
    let mut adaptive_recipe = baseline_recipe;
    adaptive_recipe.custom_garments = vec![saved];
    let adaptive = NativeCharacter::build(&lib, &adaptive_recipe).unwrap();

    // A one-triangle garment cannot cover a canonical body quad. Its empty
    // runtime subset must not expose the source-reviewed saved mask.
    assert_eq!(body_part(&adaptive).indices, body_part(&baseline).indices);
    assert_eq!(body_part(&adaptive).refs, body_part(&baseline).refs);
}

#[test]
fn outer_jacket_culls_adaptive_top_without_exposing_saved_mask() {
    let lib = library();
    let (refs, _) = face_for(&lib, 2, true);
    let hidden_face = face_index(&lib, refs);
    let mut lower = tiny_garment(&lib, "culled-mask", GarmentSlot::Top, 2);
    lower.hidden_faces = vec![hidden_face];
    let baseline_lower = lower.clone();
    lower.material["fitBodyMaskCandidates"] = json!([hidden_face]);
    let mut outer = tiny_garment(&lib, "outer-mask", GarmentSlot::Jacket, 2);
    let centroid = scale(
        add(
            add(lower.positions_dm[0], lower.positions_dm[1]),
            lower.positions_dm[2],
        ),
        1. / 3.,
    );
    let source_normal = cross(
        sub(lower.positions_dm[1], lower.positions_dm[0]),
        sub(lower.positions_dm[2], lower.positions_dm[0]),
    );
    let normal = scale(source_normal, 1. / length(source_normal));
    // Expand the jacket around the lower triangle and move it outward. Every
    // lower sample is then safely inside a same-facing front layer rather than
    // relying on a ray hit exactly at a coplanar triangle edge.
    outer.positions_dm = lower
        .positions_dm
        .iter()
        .map(|&point| {
            add(
                add(centroid, scale(sub(point, centroid), 2.5)),
                scale(normal, 0.03),
            )
        })
        .collect();
    outer.refs = lower.refs.clone();
    outer.weights = lower.weights.clone();
    outer.reference_points_dm = lower.reference_points_dm.clone();
    outer.material["fitOccludesGarments"] = json!(true);

    let mut baseline_recipe = AraRecipe::default();
    baseline_recipe.params["hair"] = json!("none");
    baseline_recipe.params["rig"] = json!("none");
    baseline_recipe.custom_garments.push(baseline_lower);
    let baseline = NativeCharacter::build(&lib, &baseline_recipe).unwrap();

    let mut adaptive_recipe = baseline_recipe;
    adaptive_recipe.custom_garments = vec![lower, outer];
    let adaptive = NativeCharacter::build(&lib, &adaptive_recipe).unwrap();

    assert_eq!(body_part(&adaptive).indices, body_part(&baseline).indices);
    assert_eq!(body_part(&adaptive).refs, body_part(&baseline).refs);
    assert!(
        adaptive
            .parts
            .iter()
            .all(|part| part.name != "custom:culled-mask")
    );
    assert!(adaptive.parts.iter().all(|part| !part.indices.is_empty()));
    assert!(!custom_part(&adaptive, "outer-mask").indices.is_empty());
}

#[test]
fn adaptive_candidates_make_low_detail_use_canonical_body() {
    let lib = library();
    let mut low = AraRecipe::default();
    low.params["hair"] = json!("none");
    low.params["rig"] = json!("none");
    low.params["detail"] = json!("low");
    let low_character = NativeCharacter::build(&lib, &low).unwrap();

    let mut canonical = low.clone();
    canonical.params["detail"] = json!("medium");
    let canonical_character = NativeCharacter::build(&lib, &canonical).unwrap();

    let mut adaptive = low;
    let mut garment = tiny_garment(&lib, "empty-candidates", GarmentSlot::Top, 2);
    garment.material["fitBodyMaskCandidates"] = json!([]);
    adaptive.custom_garments.push(garment);
    let adaptive_character = NativeCharacter::build(&lib, &adaptive).unwrap();

    assert_eq!(
        body_part(&low_character).indices.len(),
        body_part(&canonical_character).indices.len()
    );
    assert_eq!(
        body_part(&adaptive_character).indices,
        body_part(&canonical_character).indices
    );
    assert_eq!(
        body_part(&adaptive_character).refs,
        body_part(&canonical_character).refs
    );
}

#[test]
fn bottom_layer_clearance_includes_feet_and_is_recipe_order_independent() {
    let lib = library();
    let (refs, points) = face_for(&lib, 2, false);
    let anchor = scale(add(add(points[0], points[1]), points[2]), 1. / 3.);
    let raw_normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
    let normal = scale(raw_normal, 1. / length(raw_normal));
    // Synthetic planar patches isolate slot ordering on a real morphed body.
    // The feet patch is deliberately larger, so all bottom rays hit it.
    let patch = |id: &str, slot, extent, offset| {
        let mut garment = tiny_garment(&lib, id, slot, 2);
        garment.positions_dm = points
            .iter()
            .map(|point| {
                add(
                    add(anchor, scale(sub(*point, anchor), extent)),
                    scale(normal, offset),
                )
            })
            .collect();
        garment.refs = vec![refs; 3];
        garment.weights = vec![[1. / 3.; 3]; 3];
        garment.reference_points_dm = vec![anchor; 3];
        garment
    };
    let bottom = patch("layer-bottom", GarmentSlot::Bottom, 0.15, 0.03);
    let feet = patch("layer-feet", GarmentSlot::Feet, 5.0, 0.12);
    let mut recipe = AraRecipe::default();
    recipe.cartoon.enabled = true;
    for key in [
        "hair",
        "facialHair",
        "outfit",
        "top",
        "bottom",
        "jacket",
        "shoes",
        "rig",
    ] {
        recipe.params[key] = json!("none");
    }
    recipe.custom_garments = vec![bottom, feet];
    let baseline = NativeCharacter::build(&lib, &recipe).unwrap();
    recipe.custom_garments[0].material["fitLayerClearanceMm"] = json!(2.);
    let fitted = NativeCharacter::build(&lib, &recipe).unwrap();
    let outward = body_face_normal(&baseline, refs);
    for (after, before) in custom_part(&fitted, "layer-bottom")
        .positions
        .iter()
        .zip(&custom_part(&baseline, "layer-bottom").positions)
    {
        let delta = sub(*after, *before);
        assert!(
            delta.iter().zip(outward).map(|(d, n)| d * n).sum::<f32>() > 0.001,
            "delta={delta:?} normal={outward:?} bottom={:?} feet={:?}",
            custom_part(&baseline, "layer-bottom").positions,
            custom_part(&baseline, "layer-feet").positions
        );
    }
    assert_eq!(
        custom_part(&fitted, "layer-feet").positions,
        custom_part(&baseline, "layer-feet").positions
    );
    recipe.custom_garments.reverse();
    let reversed = NativeCharacter::build(&lib, &recipe).unwrap();
    assert_eq!(
        custom_part(&fitted, "layer-bottom").positions,
        custom_part(&reversed, "layer-bottom").positions
    );
}

#[test]
fn covered_waist_clearance_uses_opaque_runtime_top_coverage_independent_of_recipe_order() {
    let lib = library();
    let (triangle_refs, triangle_points) = face_for(&lib, 2, false);
    let face = face_index(&lib, triangle_refs) as usize;
    let canonical_face: [u32; 4] = lib.base.arrays["faceVerts"].u32().unwrap()
        [face * 4..face * 4 + 4]
        .try_into()
        .unwrap();
    let point_for = |reference: u32| {
        let positions = lib.base.arrays["positions"].f32().unwrap();
        let offset = reference as usize * 3;
        [
            positions[offset],
            positions[offset + 1],
            positions[offset + 2],
        ]
    };
    let face_points = canonical_face.map(point_for);
    let normal = scale(
        cross(
            sub(triangle_points[1], triangle_points[0]),
            sub(triangle_points[2], triangle_points[0]),
        ),
        1. / length(cross(
            sub(triangle_points[1], triangle_points[0]),
            sub(triangle_points[2], triangle_points[0]),
        )),
    );
    let centre = scale(
        face_points.into_iter().fold([0.; 3], add),
        1. / canonical_face.len() as f32,
    );
    let mut pants = tiny_garment(&lib, "covered-waist-pants", GarmentSlot::Bottom, 2);
    // Keep the upper two vertices directly under the body face so clearance
    // has a real 10 mm versus 2 mm difference. Extend only the low corner
    // beyond the top; its canonical anchors remain covered, while that exposed
    // physical hem prevents later triangle culling from removing the part.
    pants.positions_dm = triangle_points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let point = if index == 0 {
                add(centre, scale(sub(*point, centre), 2.5))
            } else {
                *point
            };
            add(point, scale(normal, -0.01))
        })
        .collect();
    pants.refs = triangle_refs.map(|reference| [reference; 3]).to_vec();
    pants.weights = vec![[1., 0., 0.]; 3];
    pants.reference_points_dm = triangle_points.to_vec();
    pants.material["fitClearanceMm"] = json!(10.0);
    pants.material["fitClearanceUpperBandMm"] = json!([0.0, 0.01, 2.0]);
    pants.material["fitClearanceUpperBandCoveredOnly"] = json!(true);

    let mut top = tiny_garment(&lib, "covered-waist-top", GarmentSlot::Top, 2);
    top.positions_dm = face_points
        .iter()
        .map(|point| {
            add(
                add(centre, scale(sub(*point, centre), 1.15)),
                scale(normal, 0.03),
            )
        })
        .collect();
    top.indices = vec![0, 1, 2, 0, 2, 3];
    top.uvs = vec![[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    top.refs = canonical_face.map(|reference| [reference; 3]).to_vec();
    top.weights = vec![[1., 0., 0.]; 4];
    top.reference_points_dm = face_points.to_vec();
    top.material["fitOccludesGarments"] = json!(true);
    top.material["fitBodyMaskCandidates"] = json!([face]);

    let recipe_for = |garments: Vec<CustomGarment>| {
        let mut recipe = AraRecipe::default();
        for key in [
            "hair",
            "facialHair",
            "outfit",
            "top",
            "bottom",
            "jacket",
            "shoes",
            "rig",
        ] {
            recipe.params[key] = json!("none");
        }
        recipe.custom_garments = garments;
        recipe
    };
    let alone = NativeCharacter::build(&lib, &recipe_for(vec![pants.clone()])).unwrap();
    let mut legacy_pants = pants.clone();
    legacy_pants
        .material
        .as_object_mut()
        .unwrap()
        .remove("fitClearanceUpperBandMm");
    legacy_pants
        .material
        .as_object_mut()
        .unwrap()
        .remove("fitClearanceUpperBandCoveredOnly");
    let legacy = NativeCharacter::build(&lib, &recipe_for(vec![legacy_pants])).unwrap();
    assert_eq!(
        custom_part(&alone, "covered-waist-pants").positions,
        custom_part(&legacy, "covered-waist-pants").positions,
        "pants alone retain their base 10 mm fitting"
    );

    let mut unconditional_pants = pants.clone();
    unconditional_pants.material["fitClearanceUpperBandCoveredOnly"] = json!(false);
    let unconditional =
        NativeCharacter::build(&lib, &recipe_for(vec![unconditional_pants])).unwrap();
    assert_ne!(
        custom_part(&unconditional, "covered-waist-pants").positions,
        custom_part(&legacy, "covered-waist-pants").positions,
        "the fixture must put its upper vertices inside the body-clearance band"
    );

    let covered =
        NativeCharacter::build(&lib, &recipe_for(vec![pants.clone(), top.clone()])).unwrap();
    let mut cropped = top.clone();
    cropped.indices = vec![0, 1, 2];
    let cropped = NativeCharacter::build(&lib, &recipe_for(vec![pants.clone(), cropped])).unwrap();
    assert_eq!(
        custom_part(&cropped, "covered-waist-pants").positions,
        custom_part(&alone, "covered-waist-pants").positions,
        "a cropped top must not reduce the exposed waistband"
    );
    let covered_pants = custom_part(&covered, "covered-waist-pants");
    assert_eq!(
        covered_pants.positions,
        custom_part(&unconditional, "covered-waist-pants").positions,
        "confirmed runtime coverage must select the same upper band as an explicit unconditional request"
    );

    let reversed = NativeCharacter::build(&lib, &recipe_for(vec![top, pants])).unwrap();
    assert_eq!(
        custom_part(&reversed, "covered-waist-pants").positions,
        covered_pants.positions,
        "covered fitting must not depend on recipe order"
    );
}

#[test]
fn covered_waist_clearance_rejects_same_garment_layer_clearance() {
    let lib = library();
    let mut pants = tiny_garment(&lib, "covered-layer-conflict", GarmentSlot::Bottom, 2);
    pants.material["fitClearanceMm"] = json!(10.0);
    pants.material["fitClearanceUpperBandMm"] = json!([0.0, 0.5, 2.0]);
    pants.material["fitClearanceUpperBandCoveredOnly"] = json!(true);
    pants.material["fitLayerClearanceMm"] = json!(2.0);
    let mut recipe = AraRecipe::default();
    for key in [
        "hair",
        "facialHair",
        "outfit",
        "top",
        "bottom",
        "jacket",
        "shoes",
        "rig",
    ] {
        recipe.params[key] = json!("none");
    }
    recipe.custom_garments = vec![pants];
    let error = NativeCharacter::build(&lib, &recipe).err().unwrap();
    assert!(
        error.contains("cannot combine fitClearanceUpperBandCoveredOnly with fitLayerClearanceMm")
    );
}
