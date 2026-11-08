use human_editor::{
    ara::AraRecipe,
    native_assets::{Array, NativeLibrary, Pack},
    native_fit::BodyDeformation,
    wardrobe_assets::{
        CustomGarmentCatalog, GarmentSlot, body_template_obj, import_obj, import_obj_with_binding,
    },
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn library() -> NativeLibrary {
    NativeLibrary::from_packs(
        Pack {
            meta: json!({}),
            arrays: BTreeMap::from([
                (
                    "positions".into(),
                    Array::F32(vec![0., 0., 0., 10., 0., 0., 0., 10., 0., 10., 10., 0.]),
                ),
                ("regions".into(), Array::U8(vec![1, 1, 1, 255])),
                ("uvs".into(), Array::F32(vec![0., 0., 1., 0., 0., 1.])),
                ("faceSizes".into(), Array::U8(vec![3])),
                ("faceVerts".into(), Array::U32(vec![0, 1, 2])),
                ("faceUVs".into(), Array::U32(vec![0, 1, 2])),
            ]),
        },
        Pack {
            meta: json!({}),
            arrays: BTreeMap::new(),
        },
        json!({}),
        PathBuf::new(),
    )
}

fn two_surface_library() -> NativeLibrary {
    NativeLibrary::from_packs(
        Pack {
            meta: json!({}),
            arrays: BTreeMap::from([
                (
                    "positions".into(),
                    Array::F32(vec![
                        0., 0., 0., 10., 0., 0., 0., 10., 0., 1., 1., 1., 11., 1., 1., 1., 11., 1.,
                    ]),
                ),
                ("regions".into(), Array::U8(vec![1; 6])),
                ("uvs".into(), Array::F32(vec![0., 0., 1., 0., 0., 1.])),
                ("faceSizes".into(), Array::U8(vec![3, 3])),
                ("faceVerts".into(), Array::U32(vec![0, 1, 2, 3, 4, 5])),
                ("faceUVs".into(), Array::U32(vec![0, 1, 2, 0, 1, 2])),
            ]),
        },
        Pack {
            meta: json!({}),
            arrays: BTreeMap::new(),
        },
        json!({}),
        PathBuf::new(),
    )
}

fn surface_library(
    positions: Vec<f32>,
    face_sizes: Vec<u8>,
    face_verts: Vec<u32>,
) -> NativeLibrary {
    let count = positions.len() / 3;
    NativeLibrary::from_packs(
        Pack {
            meta: json!({}),
            arrays: BTreeMap::from([
                ("positions".into(), Array::F32(positions)),
                ("regions".into(), Array::U8(vec![1; count])),
                ("faceSizes".into(), Array::U8(face_sizes)),
                ("faceVerts".into(), Array::U32(face_verts)),
            ]),
        },
        Pack {
            meta: json!({}),
            arrays: BTreeMap::new(),
        },
        json!({}),
        PathBuf::new(),
    )
}

#[test]
fn surface_binding_avoids_nearest_vertices_from_disconnected_faces() {
    let garment = import_obj_with_binding(
        &two_surface_library(),
        "v 0.1 0.1 0.01\nv 0.2 0.1 0.01\nv 0.1 0.2 0.01\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "surface-bound",
        "top",
        "dm",
        true,
    )
    .unwrap();
    for refs in &garment.refs {
        assert_eq!(
            *refs,
            [0, 1, 2],
            "anchor must remain on the closest body face"
        );
    }
    assert_eq!(
        garment.positions_dm[0],
        [0.1, 0.1, 0.01],
        "binding must not alter source positions"
    );
    assert_eq!(
        garment.uvs,
        vec![[0., 1.], [1., 1.], [0., 0.]],
        "binding must not alter UVs"
    );
}

#[test]
fn surface_binding_checks_a_broad_face_outside_the_nearest_vertex_neighbourhood() {
    let mut positions = vec![-10., -10., 0., 10., -10., 0., -10., 10., 0.];
    for offset in [0., 0.25, 0.5, 0.75] {
        positions.extend([
            0.1 + offset,
            0.1,
            0.2,
            0.2 + offset,
            0.1,
            0.2,
            0.1 + offset,
            0.2,
            0.2,
        ]);
    }
    let garment = import_obj_with_binding(
        &surface_library(positions, vec![3; 5], (0..15).collect()),
        "v 0 0 .01\nv .1 0 .01\nv 0 .1 .01\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "broad-face",
        "top",
        "dm",
        true,
    )
    .unwrap();
    assert!(garment.refs.iter().all(|refs| *refs == [0, 1, 2]));
}

#[test]
fn surface_binding_skips_degenerate_faces_before_a_valid_face() {
    let garment = import_obj_with_binding(
        &surface_library(
            vec![
                0., 0., 0., 1., 0., 0., 2., 0., 0., 0., 0., 0.1, 1., 0., 0.1, 0., 1., 0.1,
            ],
            vec![3, 3],
            vec![0, 1, 2, 3, 4, 5],
        ),
        "v .1 .1 .01\nv .2 .1 .01\nv .1 .2 .01\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "valid-face",
        "top",
        "dm",
        true,
    )
    .unwrap();
    assert!(garment.refs.iter().all(|refs| *refs == [3, 4, 5]));
    assert!(
        garment
            .weights
            .iter()
            .flatten()
            .all(|weight| weight.is_finite())
    );
}

#[test]
fn surface_binding_false_and_out_of_range_true_preserve_legacy_binding_exactly() {
    let obj = "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n";
    let legacy = import_obj(&library(), obj, "legacy", "top", "dm").unwrap();
    let disabled = import_obj_with_binding(&library(), obj, "legacy", "top", "dm", false).unwrap();
    let out_of_range =
        import_obj_with_binding(&library(), obj, "legacy", "top", "dm", true).unwrap();
    assert_eq!(disabled, legacy, "false must retain legacy binding exactly");
    assert_eq!(out_of_range.refs, legacy.refs);
    assert_eq!(out_of_range.weights, legacy.weights);
    assert_eq!(out_of_range.reference_points_dm, legacy.reference_points_dm);
}

#[test]
fn surface_binding_honours_the_five_centimetre_cutoff() {
    let make_obj = |height: f32| {
        format!(
            "v 0 0 {height}\nv 1 0 {height}\nv 0 1 {height}\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n"
        )
    };
    let within = import_obj_with_binding(
        &library(),
        &make_obj(0.499),
        "within-cutoff",
        "top",
        "dm",
        true,
    )
    .unwrap();
    let beyond = import_obj_with_binding(
        &library(),
        &make_obj(0.501),
        "beyond-cutoff",
        "top",
        "dm",
        true,
    )
    .unwrap();
    let legacy_within =
        import_obj(&library(), &make_obj(0.499), "within-cutoff", "top", "dm").unwrap();
    let legacy = import_obj(&library(), &make_obj(0.501), "beyond-cutoff", "top", "dm").unwrap();
    assert_eq!(within.refs[0], [0, 1, 2]);
    assert_eq!(within.weights[0], [1., 0., 0.]);
    assert_ne!(
        within.weights, legacy_within.weights,
        "within-cutoff must use surface barycentrics"
    );
    assert_eq!(beyond.refs, legacy.refs);
    assert_eq!(beyond.weights, legacy.weights);
    assert_eq!(beyond.reference_points_dm, legacy.reference_points_dm);
}

#[test]
fn obj_negative_indices_uv_seams_and_template_roundtrip() {
    let lib = library();
    let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvt .5 1\nf -4/-5 -3/-4 -2/-3 -1/-2\nf 1/1 3/5 4/4\n";
    let garment = import_obj(&lib, obj, "seamed-top", "top", "m").unwrap();
    assert_eq!(garment.indices.len(), 9);
    assert_eq!(
        garment.positions_dm.len(),
        5,
        "same OBJ position splits at its UV seam"
    );
    assert_eq!(garment.positions_dm[1], [10., 0., 0.]);
    assert_eq!(garment.uvs[0], [0., 1.], "OBJ bottom edge becomes glTF V=1");
    assert_eq!(garment.uvs[2], [1., 0.], "OBJ top edge becomes glTF V=0");
    let template = body_template_obj(&lib, "m").unwrap();
    let round_trip = import_obj(&lib, &template, "body-copy", "accessory", "m").unwrap();
    assert_eq!(round_trip.indices.len(), 3);
    assert_eq!(
        round_trip.positions_dm,
        vec![[0., 0., 0.], [10., 0., 0.], [0., 10., 0.]]
    );
}

#[test]
fn same_body_identity_and_morphed_body_follow_anchors() {
    let lib = library();
    let garment = import_obj(
        &lib,
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "follow",
        "top",
        "m",
    )
    .unwrap();
    let body = vec![[0., 0., 0.], [10., 0., 0.], [0., 10., 0.], [10., 10., 0.]];
    let fit = garment.fit(&body).unwrap();
    assert_eq!(fit.positions, garment.positions_dm);
    let moved: Vec<_> = body
        .iter()
        .map(|p| [p[0] + 2., p[1] - 3., p[2] + 4.])
        .collect();
    let morphed = garment.fit(&moved).unwrap();
    for (old, new) in fit.positions.iter().zip(morphed.positions) {
        for axis in 0..3 {
            assert!(
                (new[axis] - (old[axis] + [2., -3., 4.][axis])).abs() < 1e-5,
                "vertex did not inherit the body translation"
            );
        }
    }
}

#[test]
fn adaptive_identity_preserves_valid_rounded_reference_weights() {
    let lib = library();
    let mut garment = import_obj(
        &lib,
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "rounded-weights",
        "top",
        "dm",
    )
    .unwrap();
    garment.refs = vec![[0; 3]; 3];
    garment.weights = vec![[1.0005, 0., 0.]; 3];
    garment.validate().unwrap();
    let neutral = vec![[0., 0., 0.], [10., 0., 0.], [0., 10., 0.], [10., 10., 0.]];
    for translation in [[0., 0., 0.], [2., -3., 4.]] {
        let current: Vec<_> = neutral
            .iter()
            .map(|p| std::array::from_fn(|i| p[i] + translation[i]))
            .collect();
        let field =
            BodyDeformation::new(&neutral, &current, &[0, 1, 2, 2], &[1, 1, 1, 255]).unwrap();
        assert_eq!(
            garment.fit(&current).unwrap().positions,
            garment.fit_adaptive(&current, &field).unwrap().positions
        );
    }
}

#[test]
fn adaptive_custom_garments_transport_offsets_for_every_slot() {
    let lib = library();
    let mut garment = import_obj(
        &lib,
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "adaptive",
        "top",
        "dm",
    )
    .unwrap();
    let neutral = vec![[0., 0., 0.], [10., 0., 0.], [0., 10., 0.], [10., 10., 0.]];
    let current = vec![[0., 0., 0.], [20., 0., 0.], [0., 30., 0.], [20., 30., 0.]];
    let field = BodyDeformation::new(&neutral, &current, &[0, 1, 2, 2], &[1, 1, 1, 255]).unwrap();
    garment.positions_dm = vec![[1., 0., 0.], [6., 0., 0.], [2.5, 5., 0.]];
    garment.reference_points_dm = vec![[0., 0., 0.], [5., 0., 0.], [0., 5., 0.]];
    garment.refs = vec![[0, 0, 0], [0, 1, 2], [0, 1, 2]];
    garment.weights = vec![[1., 0., 0.], [0.5, 0.5, 0.], [0.25, 0.25, 0.5]];
    for slot in [
        GarmentSlot::Hair,
        GarmentSlot::Beard,
        GarmentSlot::Top,
        GarmentSlot::Bottom,
        GarmentSlot::Jacket,
        GarmentSlot::Feet,
        GarmentSlot::Hat,
        GarmentSlot::Accessory,
    ] {
        garment.slot = slot;
        let fitted = garment.fit_adaptive(&current, &field).unwrap();
        assert!(
            (fitted.positions[0][0] - 2.).abs() < 0.01,
            "direct reference offset did not scale"
        );
        assert!(
            fitted
                .positions
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
    }
}

#[test]
fn imported_hair_uses_the_portable_catalog_and_recipe_format() {
    let root = std::env::temp_dir().join(format!(
        "human-editor-hair-catalog-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let garments = root.join("custom_garments");
    fs::create_dir_all(&garments).unwrap();
    let hair = import_obj(
        &library(),
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "braided-crown",
        "hair",
        "m",
    )
    .unwrap();
    let mut top = hair.clone();
    top.id = "independent-top".into();
    top.slot = GarmentSlot::Top;
    fs::write(
        garments.join("braided-crown.json"),
        serde_json::to_vec(&hair).unwrap(),
    )
    .unwrap();
    let catalog = CustomGarmentCatalog::discover(&root);
    let entry = &catalog.entries[0];
    assert_eq!(entry.slot, Some(GarmentSlot::Hair));
    let mut recipe = AraRecipe::default();
    recipe.select_custom_garment(&top).unwrap();
    recipe.params["outfit"] = json!("male_casualsuit01");
    recipe.params["hair"] = json!("cornrows");
    recipe.params["top"] = json!("tank_top");
    recipe.params["hat"] = json!("baseball_cap");
    recipe
        .select_custom_garment(entry.garment().unwrap())
        .unwrap();
    assert_eq!(recipe.params["hair"], "none");
    assert_eq!(recipe.params["outfit"], "male_casualsuit01");
    assert_eq!(recipe.params["top"], "tank_top");
    assert_eq!(recipe.params["hat"], "baseball_cap");
    assert!(
        recipe
            .custom_garments
            .iter()
            .any(|garment| garment.id == "independent-top")
    );
    assert!(
        recipe
            .custom_garments
            .iter()
            .any(|garment| garment.id == "braided-crown")
    );
    let restored: AraRecipe =
        serde_json::from_value(serde_json::to_value(&recipe).unwrap()).unwrap();
    assert_eq!(restored.custom_garments, recipe.custom_garments);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn canonical_quad_body_template_needs_no_face_sizes() {
    let mut lib = library();
    lib.base.arrays.remove("faceSizes");
    lib.base
        .arrays
        .insert("faceVerts".into(), Array::U32(vec![0, 1, 3, 2]));
    lib.base
        .arrays
        .insert("faceUVs".into(), Array::U32(vec![0, 1, 3, 2]));
    lib.base.arrays.insert(
        "uvs".into(),
        Array::F32(vec![0., 0., 1., 0., 0., 1., 1., 1.]),
    );
    let obj = body_template_obj(&lib, "m").unwrap();
    assert!(obj.contains("f 1/1 2/2 4/4 3/3\n"));
}

#[test]
fn catalog_reports_missing_texture_without_exposing_a_selectable_garment() {
    let root = std::env::temp_dir().join(format!(
        "human-editor-catalog-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let garments = root.join("custom_garments");
    fs::create_dir_all(&garments).unwrap();
    let mut garment = import_obj(
        &library(),
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "catalog-top",
        "top",
        "m",
    )
    .unwrap();
    garment.material["map"] = json!("textures/not-here.png");
    fs::write(
        garments.join("top.json"),
        serde_json::to_vec(&garment).unwrap(),
    )
    .unwrap();
    let catalog = CustomGarmentCatalog::discover(&root);
    assert_eq!(catalog.entries.len(), 1);
    assert!(
        catalog.entries[0]
            .error
            .as_deref()
            .unwrap()
            .contains("missing garment texture")
    );
    assert!(catalog.entries[0].garment().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn catalog_texture_paths_match_native_preview_and_export_resolution() {
    let root = std::env::temp_dir().join(format!(
        "human-editor-catalog-textures-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let garments = root.join("custom_garments");
    let textures = root.join("textures");
    fs::create_dir_all(textures.join("textures")).unwrap();
    fs::create_dir_all(&garments).unwrap();
    fs::write(textures.join("coat.png"), []).unwrap();
    fs::write(textures.join("textures/coat.png"), []).unwrap();
    fs::write(root.join("stray.png"), []).unwrap();

    let base = import_obj(
        &library(),
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "catalog-texture",
        "top",
        "m",
    )
    .unwrap();
    let mut bare = base.clone();
    bare.id = "bare".into();
    bare.material["map"] = json!("coat.png");
    let mut prefixed = base.clone();
    prefixed.id = "prefixed".into();
    prefixed.material["diffuse"] = json!("textures/coat.png");
    prefixed.material["normalMap"] = json!("textures/textures/coat.png");
    let mut root_relative = base;
    root_relative.id = "root-relative".into();
    root_relative.material["map"] = json!("stray.png");
    for garment in [bare, prefixed, root_relative] {
        fs::write(
            garments.join(format!("{}.json", garment.id)),
            serde_json::to_vec(&garment).unwrap(),
        )
        .unwrap();
    }

    let catalog = CustomGarmentCatalog::discover(&root);
    assert!(
        catalog
            .entries
            .iter()
            .find(|entry| entry.id == "bare")
            .unwrap()
            .garment()
            .is_some()
    );
    assert!(
        catalog
            .entries
            .iter()
            .find(|entry| entry.id == "prefixed")
            .unwrap()
            .garment()
            .is_some()
    );
    assert!(
        catalog
            .entries
            .iter()
            .find(|entry| entry.name == "root-relative")
            .unwrap()
            .garment()
            .is_none()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn library_catalog_rejects_hidden_faces_outside_the_native_body() {
    let root = std::env::temp_dir().join(format!(
        "human-editor-catalog-hidden-faces-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let garments = root.join("custom_garments");
    fs::create_dir_all(&garments).unwrap();
    let mut garment = import_obj(
        &library(),
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "bad-hidden-face",
        "top",
        "m",
    )
    .unwrap();
    garment.hidden_faces = vec![1];
    fs::write(
        garments.join("bad-hidden-face.json"),
        serde_json::to_vec(&garment).unwrap(),
    )
    .unwrap();
    let mut lib = library();
    lib.root = root.clone();
    lib.base
        .arrays
        .insert("faceVerts".into(), Array::U32(vec![0, 1, 2, 0]));
    let catalog = CustomGarmentCatalog::discover_for_library(&lib);
    assert!(catalog.entries[0].garment().is_none());
    assert!(
        catalog.entries[0]
            .error
            .as_deref()
            .unwrap()
            .contains("invalid hidden body face")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn selecting_catalog_garment_replaces_its_slot_and_clears_builtin_conflicts() {
    let lib = library();
    let top = import_obj(
        &lib,
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "top-a",
        "top",
        "m",
    )
    .unwrap();
    let mut next_top = top.clone();
    next_top.id = "top-b".into();
    next_top.name = "Second top".into();
    let jacket = import_obj(
        &lib,
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "jacket-a",
        "jacket",
        "m",
    )
    .unwrap();
    let mut recipe = AraRecipe::default();
    recipe.params["outfit"] = json!("male_casualsuit01");
    recipe.params["top"] = json!("tank_top");
    recipe.params["jacket"] = json!("jacket01");
    recipe.select_custom_garment(&top).unwrap();
    assert_eq!(recipe.params["outfit"], "none");
    assert_eq!(recipe.params["top"], "none");
    assert_eq!(recipe.params["jacket"], "jacket01");
    recipe.select_custom_garment(&jacket).unwrap();
    recipe.select_custom_garment(&next_top).unwrap();
    assert_eq!(recipe.custom_garments.len(), 2);
    assert!(
        recipe
            .custom_garments
            .iter()
            .any(|garment| garment.id == "top-b")
    );
    assert!(
        recipe
            .custom_garments
            .iter()
            .any(|garment| garment.id == "jacket-a")
    );
}

#[test]
fn accessory_catalog_garments_toggle_independently() {
    let lib = library();
    let goggles = import_obj(
        &lib,
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "goggles",
        "accessory",
        "m",
    )
    .unwrap();
    let mut belt = goggles.clone();
    belt.id = "belt".into();
    belt.name = "Tool belt".into();
    let mut recipe = AraRecipe::default();
    recipe.params["accessories"] = json!(["belt"]);
    recipe.select_custom_garment(&goggles).unwrap();
    recipe.select_custom_garment(&belt).unwrap();
    assert_eq!(recipe.params["accessories"], json!([]));
    assert_eq!(recipe.custom_garments.len(), 2);
    recipe.select_custom_garment(&goggles).unwrap();
    assert_eq!(recipe.custom_garments.len(), 1);
    assert_eq!(recipe.custom_garments[0].id, "belt");
}

#[test]
fn layered_garments_preserve_same_slot_base_mid_and_outer_layers() {
    let lib = library();
    let shirt = import_obj(
        &lib,
        "v 0 0 1\nv 1 0 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "blue-shirt",
        "top",
        "m",
    )
    .unwrap();
    let mut sweater = shirt.clone();
    sweater.id = "green-sweater".into();
    sweater.name = "Green sweater".into();
    let mut coat = shirt.clone();
    coat.id = "utility-coat".into();
    coat.name = "Utility coat".into();
    coat.slot = human_editor::wardrobe_assets::GarmentSlot::Jacket;
    let mut recipe = AraRecipe::default();
    recipe.select_custom_garment(&shirt).unwrap();
    recipe.toggle_layered_custom_garment(&sweater).unwrap();
    recipe.toggle_layered_custom_garment(&coat).unwrap();
    assert_eq!(recipe.custom_garments.len(), 3);
    assert_eq!(recipe.params["top"], "none");
    assert_eq!(recipe.params["jacket"], "none");
    recipe.toggle_layered_custom_garment(&sweater).unwrap();
    assert_eq!(recipe.custom_garments.len(), 2);
    assert!(
        recipe
            .custom_garments
            .iter()
            .any(|garment| garment.id == "blue-shirt")
    );
    assert!(
        recipe
            .custom_garments
            .iter()
            .any(|garment| garment.id == "utility-coat")
    );
}

#[test]
fn torso_cage_adaptive_path_preserves_portable_geometry_and_rig_streams() {
    use bevy::prelude::Vec3;
    use human_editor::wardrobe_assets::CustomGarment;
    let neutral = [0., 1., 2.]
        .into_iter()
        .flat_map(|y| {
            (0..8).map(move |i| {
                let a = i as f32 * std::f32::consts::TAU / 8.;
                [a.cos(), y, a.sin()]
            })
        })
        .collect::<Vec<_>>();
    let faces = (0..2)
        .flat_map(|band| {
            (0..8).flat_map(move |i| {
                let a = band * 8 + i;
                let b = band * 8 + (i + 1) % 8;
                [a, b, b + 8, a + 8]
            })
        })
        .collect::<Vec<u32>>();
    let scale = Vec3::new(1.2, 0.9, 1.1);
    let offset = Vec3::new(0.2, 0.1, -0.1);
    let current = neutral
        .iter()
        .map(|p| (Vec3::from(*p) * scale + offset).to_array())
        .collect::<Vec<_>>();
    let field = BodyDeformation::new(&neutral, &current, &faces, &[2; 24]).unwrap();
    let garment = CustomGarment {
        id: "cage-stream-test".into(),
        name: "cage stream test".into(),
        slot: GarmentSlot::Top,
        positions_dm: vec![[0.2, 0.3, 0.4], [0.7, 0.3, 0.4], [0.2, 0.9, 0.4]],
        indices: vec![0, 1, 2],
        uvs: vec![[0., 0.], [1., 0.], [0., 1.]],
        refs: vec![[0, 0, 0]; 3],
        weights: vec![[1., 0., 0.]; 3],
        reference_points_dm: vec![neutral[0]; 3],
        hidden_faces: vec![],
        material: json!({"fitTorsoCage":{"levelsYDM":[0,1,2],"halfBandDM":0.1}}),
    };
    let original = serde_json::to_value(&garment).unwrap();
    let part = garment.fit_adaptive(&current, &field).unwrap();
    for (source, result) in garment.positions_dm.iter().zip(&part.positions) {
        assert!((Vec3::from(*result) - (Vec3::from(*source) * scale + offset)).length() < 1e-5);
    }
    assert_eq!(part.refs, garment.refs);
    assert_eq!(part.ref_weights, garment.weights);
    assert_eq!(part.uvs, garment.uvs);
    assert_eq!(part.indices, garment.indices);
    assert_eq!(part.material, garment.material);
    assert_eq!(serde_json::to_value(&garment).unwrap(), original);
}

#[test]
fn animation_rebinding_uses_current_surface_without_changing_fitted_geometry() {
    let lib = two_surface_library();
    let mut garment = import_obj(
        &lib,
        "v 1 1 0.1\nv 2 1 0.1\nv 1 2 0.1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "animation-rebind",
        "top",
        "dm",
    )
    .unwrap();
    garment.material["fitAnimationSurfaceRebind"] = json!(true);
    let saved = serde_json::to_value(&garment).unwrap();
    let current = [
        [0., 0., 5.],
        [10., 0., 5.],
        [0., 10., 5.],
        [1., 1., 1.],
        [11., 1., 1.],
        [1., 11., 1.],
    ];
    let mut part = garment.fit(&current).unwrap();
    part.positions = vec![[2., 2., 1.1], [3., 2., 1.1], [2., 3., 1.1]];
    let geometry = (
        part.positions.clone(),
        part.indices.clone(),
        part.uvs.clone(),
        part.material.clone(),
    );
    garment
        .fit_animation_bindings(&lib, &current, &mut part)
        .unwrap();
    assert!(part.refs.iter().flatten().all(|r| (3..=5).contains(r)));
    assert!(
        part.ref_weights
            .iter()
            .all(|w| (w.iter().sum::<f32>() - 1.).abs() < 1e-5)
    );
    assert_eq!(
        geometry,
        (part.positions, part.indices, part.uvs, part.material)
    );
    assert_eq!(saved, serde_json::to_value(&garment).unwrap());
}

#[test]
fn animation_rebinding_is_default_off_and_retains_far_away_source_bindings() {
    let lib = library();
    let mut garment = import_obj(
        &lib,
        "v 1 1 0.1\nv 2 1 0.1\nv 1 2 0.1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "rebind-default",
        "top",
        "dm",
    )
    .unwrap();
    let current = [[0., 0., 0.], [10., 0., 0.], [0., 10., 0.], [10., 10., 0.]];
    let mut part = garment.fit(&current).unwrap();
    let original = (part.refs.clone(), part.ref_weights.clone());
    garment
        .fit_animation_bindings(&lib, &[], &mut part)
        .unwrap();
    assert_eq!(original, (part.refs.clone(), part.ref_weights.clone()));
    garment.material["fitAnimationSurfaceRebind"] = json!(true);
    for p in &mut part.positions {
        p[2] = 2.;
    }
    garment
        .fit_animation_bindings(&lib, &current, &mut part)
        .unwrap();
    assert_eq!(original, (part.refs, part.ref_weights));
    garment.material["fitAnimationSurfaceRebind"] = json!("yes");
    assert!(garment.validate().is_err());
    garment.material["fitAnimationSurfaceRebind"] = json!(true);
    garment.slot = GarmentSlot::Bottom;
    assert!(garment.validate().is_err());
}

#[test]
fn physical_upper_coverage_rejects_nondefault_correction_at_catalog_validation() {
    let lib = library();
    let mut garment = import_obj(
        &lib,
        "v 1 1 0.1\nv 2 1 0.1\nv 1 2 0.1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
        "physical-correction",
        "top",
        "dm",
    )
    .unwrap();
    garment.material["fitClearanceMm"] = json!(3.0);
    garment.material["fitClearanceUpperBandPhysicalCoverage"] = json!(true);
    assert!(garment.validate().is_ok());
    garment.material["fitMaxCorrectionMm"] = json!(40.0);
    assert!(garment.validate().is_ok());
    garment.material["fitMaxCorrectionMm"] = json!(60.0);
    assert!(garment.validate().unwrap_err().contains("PhysicalCoverage"));
    garment.material["fitClearanceUpperBandPhysicalCoverage"] = json!(false);
    assert!(garment.validate().is_ok());
    garment.material["fitClearanceUpperBandPhysicalCoverage"] = json!("yes");
    assert!(garment.validate().is_err());
}
