//! Native MakeHuman character assembly. Geometry and customization run entirely in Rust.
pub use crate::native_assets::NativeLibrary;
use crate::{
    ara::{AraRecipe, multiply_colors, native_catalog},
    native_assets::{Array, Pack},
    native_morph::{apply_targets, apply_weighted_targets, fit_proxy},
    native_rig::NativeRig,
};
use bevy::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct NativePart {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub uvs: Vec<[f32; 2]>,
    pub refs: Vec<[u32; 3]>,
    pub ref_weights: Vec<[f32; 3]>,
    /// A named bone that owns every vertex in this part.  Rigid props retain
    /// their authored shape instead of inheriting blended surface weights.
    pub rigid_joint: Option<String>,
    pub material: Value,
}
#[derive(Clone, Debug)]
pub struct NativeCharacter {
    pub parts: Vec<NativePart>,
    pub height: f32,
    pub rig: Option<NativeRig>,
    pub face_names: Vec<String>,
    /// Full MakeHuman base-vertex deltas in meters, for native animation and export.
    pub face_deltas: Vec<Vec<[f32; 3]>>,
    pub recipe: AraRecipe,
    pub hair_fit: crate::native_hair::HairFitStats,
}
fn array<'a>(p: &'a Pack, key: &str) -> Result<&'a Array, String> {
    p.arrays
        .get(key)
        .ok_or_else(|| format!("missing pack array {key}"))
}
fn s<'a>(p: &'a Value, key: &str) -> &'a str {
    p[key].as_str().unwrap_or("none")
}
fn find<'a>(cat: &'a Value, key: &str, id: &str) -> Option<&'a Value> {
    cat[key].as_array()?.iter().find(|a| a["id"] == id)
}
fn tint_material(item: &Value, tint: &str) -> Value {
    let mut m = item["material"].clone();
    if !m.is_object() {
        m = json!({"color":"#ffffff","roughness":0.8,"alpha":"opaque"});
    }
    m["color"] = json!(multiply_colors(
        m["color"].as_str().unwrap_or("#ffffff"),
        tint
    ));
    m
}
fn pick_skin<'a>(p: &Value, skins: &'a [Value]) -> Result<&'a Value, String> {
    if s(p, "skin") != "auto" {
        return skins
            .iter()
            .find(|a| a["id"] == p["skin"])
            .ok_or_else(|| "unknown skin".into());
    }
    let heritage = [
        ("african", p["african"].as_f64().unwrap_or(0.)),
        ("asian", p["asian"].as_f64().unwrap_or(0.)),
        ("caucasian", p["caucasian"].as_f64().unwrap_or(0.)),
    ];
    let race = heritage
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
        .0;
    let sex = if p["gender"].as_f64().unwrap_or(1.) >= 0.5 {
        "male"
    } else {
        "female"
    };
    let age = p["age"].as_f64().unwrap_or(0.5);
    let years = if age < 0.5 {
        11. + (age - 0.1875) / 0.3125 * 14.
    } else {
        25. + (age - 0.5) * 130.
    };
    let target_age: i32 = if years < 38. {
        0
    } else if years < 60. {
        1
    } else {
        2
    };
    skins
        .iter()
        .max_by_key(|a| {
            let id = a["id"].as_str().unwrap_or("");
            let a = if id.starts_with("young") {
                0
            } else if id.starts_with("middleage") {
                1
            } else {
                2
            };
            (if id.contains(race) { 8 } else { 0 })
                + (if id.split('_').any(|part| part == sex) {
                    4
                } else {
                    0
                })
                - (a - target_age).abs()
        })
        .ok_or_else(|| "no skin assets".into())
}

/// Split polygon corners at UV seams while preserving MakeHuman fitting/skin references.
fn polygon_part(
    name: &str,
    coords: &[[f32; 3]],
    sizes: &[u8],
    vertices: &[u32],
    face_uvs: &[u32],
    uvs: &[f32],
    refs: Option<(&[u32], &[f32])>,
    material: Value,
    hidden: &[bool],
    hidden_faces: &[bool],
) -> Result<NativePart, String> {
    let mut out = NativePart {
        name: name.into(),
        positions: vec![],
        indices: vec![],
        uvs: vec![],
        refs: vec![],
        ref_weights: vec![],
        rigid_joint: None,
        material,
    };
    let mut keys = BTreeMap::new();
    let mut offset = 0;
    for (face_index, &size) in sizes.iter().enumerate() {
        let size = size as usize;
        let corners = vertices
            .get(offset..offset + size)
            .ok_or("truncated polygon vertices")?;
        let texture = face_uvs
            .get(offset..offset + size)
            .ok_or("truncated polygon UV indices")?;
        offset += size;
        if hidden_faces.get(face_index).copied().unwrap_or(false)
            || corners
                .iter()
                .any(|&v| hidden.get(v as usize).copied().unwrap_or(false))
        {
            continue;
        }
        let mut face = Vec::with_capacity(size);
        for (&v, &t) in corners.iter().zip(texture) {
            let vertex = *coords.get(v as usize).ok_or("invalid polygon index")?;
            let index = if let Some(&i) = keys.get(&(v, t)) {
                i
            } else {
                let i = out.positions.len() as u32;
                keys.insert((v, t), i);
                out.positions.push(vertex);
                let uv = uvs
                    .get(t as usize * 2..t as usize * 2 + 2)
                    .ok_or("invalid UV index")?;
                out.uvs.push([uv[0], 1. - uv[1]]);
                if let Some((r, w)) = refs {
                    let r = r
                        .get(v as usize * 3..v as usize * 3 + 3)
                        .ok_or("invalid proxy refs")?;
                    let w = w
                        .get(v as usize * 3..v as usize * 3 + 3)
                        .ok_or("invalid proxy weights")?;
                    out.refs.push([r[0], r[1], r[2]]);
                    out.ref_weights.push([w[0], w[1], w[2]]);
                } else {
                    out.refs.push([v, v, v]);
                    out.ref_weights.push([1., 0., 0.]);
                }
                i
            };
            face.push(index);
        }
        for k in 1..face.len().saturating_sub(1) {
            out.indices.extend([face[0], face[k], face[k + 1]]);
        }
    }
    Ok(out)
}
fn fitted_part(
    lib: &NativeLibrary,
    item: &Value,
    coords: &[[f32; 3]],
    name: &str,
    material: Value,
) -> Result<(NativePart, Vec<u32>), String> {
    let path = item["file"].as_str().ok_or("asset has no proxy file")?;
    let proxy = lib.proxy(path)?;
    let positions = fit_proxy(&proxy, coords)?;
    let part = polygon_part(
        name,
        &positions,
        array(&proxy, "faceSizes")?.u8()?,
        array(&proxy, "faceVerts")?.u32()?,
        array(&proxy, "faceUVs")?.u32()?,
        array(&proxy, "uvs")?.f32()?,
        Some((
            array(&proxy, "refs")?.u32()?,
            array(&proxy, "weights")?.f32()?,
        )),
        material,
        &[],
        &[],
    )?;
    let delete = proxy
        .arrays
        .get("deleteVerts")
        .map(|a| a.u32().map(|a| a.to_vec()))
        .transpose()?
        .unwrap_or_default();
    Ok((part, delete))
}
fn facial_shapes(
    lib: &NativeLibrary,
    p: &Value,
    cartoon: f32,
) -> Result<(Vec<String>, Vec<Vec<[f32; 3]>>), String> {
    let schema = native_catalog();
    let base = array(&lib.base, "positions")?.f32()?;
    let total = ["african", "asian", "caucasian"]
        .iter()
        .map(|k| p[k].as_f64().unwrap_or(0.) as f32)
        .sum::<f32>();
    let mut names = vec![];
    let mut deltas = vec![];
    for shape in schema["face_shapes"].as_array().unwrap() {
        let name = shape["name"].as_str().unwrap();
        if name.starts_with("viseme_") && p["speechShapes"] == false {
            continue;
        }
        let mut weights = BTreeMap::new();
        for unit in shape["units"].as_array().unwrap() {
            for race in ["african", "asian", "caucasian"] {
                let k = if total > 0. {
                    p[race].as_f64().unwrap_or(0.) as f32 / total
                } else {
                    1. / 3.
                };
                *weights
                    .entry(format!(
                        "expression/units/{race}/{}",
                        unit[0].as_str().unwrap()
                    ))
                    .or_default() += k * unit[1].as_f64().unwrap() as f32;
            }
        }
        let mut moved = apply_weighted_targets(lib, &weights)?;
        if cartoon > 0. {
            // Expression units resized to the cartoon eyes and mouth.
            let scaled = weights
                .iter()
                .map(|(k, w)| (k.clone(), w * cartoon))
                .collect();
            crate::native_morph::add_targets(lib.cartoon_offsets()?, &mut moved, &scaled)?;
        }
        for (i, v) in moved.iter_mut().enumerate() {
            for axis in 0..3 {
                v[axis] -= base[i * 3 + axis];
            }
        }
        // Typical closure calibration avoids MakeHuman's folded eyelid at full weight.
        if name.starts_with("eyeBlink") {
            for v in &mut moved {
                for x in v {
                    *x *= 0.55;
                }
            }
        }
        names.push(name.to_owned());
        deltas.push(moved);
    }
    Ok((names, deltas))
}
/// The Cartoon amount for the hm08 cartoon offsets (0 when the mode is off).
/// Share of the measured cartoon torso overhang turned into arm spread.
const ARM_SPREAD_GAIN: f32 = 1.0;

fn cartoon_amount(recipe: &AraRecipe) -> f32 {
    if recipe.cartoon.enabled {
        recipe.cartoon.strength
    } else {
        0.
    }
}

/// Adds the cartoon hm08 offsets on top of the regular MakeHuman targets, so every
/// macro and detail target keeps its authored range.  Female/male offsets blend by
/// gender and scale with body height relative to the adult default, so children and
/// short characters get proportionate offsets.  Children keep the full cartoon head,
/// a lighter body offset and no adult chest; both blend in between ages 12 and 18.
/// Muscle above 0.5 (and, for the female chest, breast size too) blends towards the
/// Superhuman references.
fn add_cartoon_offsets(
    lib: &NativeLibrary,
    recipe: &AraRecipe,
    coords: &mut [[f32; 3]],
) -> Result<(), String> {
    let amount = cartoon_amount(recipe);
    if amount <= 0. {
        return Ok(());
    }
    let pack = lib.cartoon_offsets()?;
    let info = &pack.meta["cartoon"];
    let gender = recipe.params["gender"]
        .as_f64()
        .unwrap_or(0.5)
        .clamp(0., 1.) as f32;
    let reference = |sex: &str| info["reference_height_dm"][sex].as_f64().unwrap_or(16.) as f32;
    let reference = reference("female") * (1. - gender) + reference("male") * gender;
    let faces = array(&lib.base, "faceVerts")?.u32()?;
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &v in faces {
        let y = coords.get(v as usize).ok_or("face vertex out of range")?[1];
        lo = lo.min(y);
        hi = hi.max(y);
    }
    let size = ((hi - lo) / reference).clamp(0.5, 1.25);
    // Adult bust/pectoral shape fades in between MakeHuman ages 12 (0.21) and 18 (0.36).
    let age = recipe.params["age"].as_f64().unwrap_or(0.5) as f32;
    let t = ((age - 0.21) / 0.15).clamp(0., 1.);
    let adult = t * t * (3. - 2. * t);
    let name = |key: &str| info[key].as_str().unwrap_or_default().to_owned();
    let (female, male) = (amount * (1. - gender) * size, amount * gender * size);
    // Children keep the full cartoon head but a lighter, straighter body.
    let body = 0.35 + 0.65 * adult;
    // Superhuman corrections: muscle 0.5 -> 1 fades in the max-muscle cartoon body; the
    // female chest part needs full muscle and full bust together (one reference has both).
    let above_half =
        |key: &str| ((recipe.params[key].as_f64().unwrap_or(0.5) as f32 - 0.5) * 2.).clamp(0., 1.);
    let (muscle, bust) = (above_half("muscle"), above_half("breastSize"));
    let mut weights = BTreeMap::from([
        (name("female_head"), female),
        (name("male_head"), male),
        (name("female"), female * body),
        (name("male"), male * body),
        (name("female_chest"), female * adult),
        (name("male_chest"), male * adult),
    ]);
    if !info["female_muscle"].is_null() {
        weights.extend([
            (name("female_muscle"), female * body * muscle),
            (name("male_muscle"), male * body * muscle),
            (name("female_muscle_chest"), female * adult * muscle * bust),
            (name("male_muscle_chest"), male * adult * muscle),
        ]);
    }
    crate::native_morph::add_targets(pack, coords, &weights)
}

impl NativeCharacter {
    pub fn build(lib: &NativeLibrary, recipe: &AraRecipe) -> Result<Self, String> {
        recipe.validate()?;
        let p = &recipe.params;
        let mut coords = apply_targets(lib, p)?;
        // Clips are retargeted against the realistic body; a wider cartoon torso
        // needs the arms spread by however much further it reaches out.
        let realistic_clearance = if cartoon_amount(recipe) > 0. {
            Some(crate::native_rig::arm_clearance(lib, &coords)?)
        } else {
            None
        };
        add_cartoon_offsets(lib, recipe, &mut coords)?;
        for target in &recipe.custom_targets {
            for &(i, d) in &target.deltas {
                for a in 0..3 {
                    let coord = coords
                        .get_mut(i)
                        .ok_or("custom target index exceeds loaded base topology")?;
                    coord[a] += d[a] * target.weight;
                }
            }
        }
        crate::native_breast::reshape(
            lib,
            &mut coords,
            p["gender"].as_f64().unwrap_or(1.) as f32,
            &recipe.breasts,
        )?;
        crate::native_breast::nipple_relief(
            lib,
            &mut coords,
            p["gender"].as_f64().unwrap_or(1.) as f32,
            recipe.breasts.projection,
            cartoon_amount(recipe),
        )?;
        if s(p, "shoes") == "heels" {
            crate::native_anatomy::arch_feet(lib, &mut coords)?;
        }
        let (mut face_names, mut face_deltas) = facial_shapes(lib, p, cartoon_amount(recipe))?;
        if let Some(teeth) = find(&lib.catalog, "teeth", s(p, "teeth")) {
            if let Some(index) = face_names.iter().position(|n| n == "mouthUpperUp") {
                let proxy = lib.proxy(teeth["file"].as_str().ok_or("teeth has no proxy")?)?;
                crate::native_anatomy::seal_lips(lib, &mut coords, &face_deltas[index], &proxy)?;
            }
        }
        // Runtime expression deltas are relative to this neutral, lip-sealed mesh.
        let schema = native_catalog();
        let expression = &schema["expression_weights"][s(p, "expression")];
        for (name, delta) in face_names.iter().zip(&face_deltas) {
            let w = expression[name].as_f64().unwrap_or(0.) as f32;
            if w != 0. {
                for (v, d) in coords.iter_mut().zip(delta) {
                    for axis in 0..3 {
                        v[axis] += d[axis] * w;
                    }
                }
            }
        }
        if p["faceShapes"] == false {
            face_names.clear();
            face_deltas.clear();
        }
        let mut hidden = vec![false; coords.len()];
        let mut parts = vec![];
        let mut add_proxy =
            |item: &Value, name: &str, tint: &str, hide: bool| -> Result<(), String> {
                let (part, delete) =
                    fitted_part(lib, item, &coords, name, tint_material(item, tint))?;
                if hide {
                    for v in delete {
                        if let Some(h) = hidden.get_mut(v as usize) {
                            *h = true;
                        }
                    }
                }
                parts.push(part);
                Ok(())
            };
        let outfit = find(&lib.catalog, "outfits", s(p, "outfit"));
        if let Some(item) = outfit {
            add_proxy(item, "Outfit", s(p, "outfitTint"), true)?;
        }
        for (category, key, label, tint, hide) in [
            ("shoes", "shoes", "Shoes", "#ffffff", true),
            ("hats", "hat", "Hat", "#ffffff", true),
            ("eyebrows", "eyebrows", "Eyebrows", s(p, "hairTint"), false),
            (
                "eyelashes",
                "eyelashes",
                "Eyelashes",
                s(p, "hairTint"),
                false,
            ),
            ("teeth", "teeth", "Teeth", "#ffffff", false),
        ] {
            if let Some(item) = find(&lib.catalog, category, s(p, key)) {
                // Cartoon mode replaces strand-card lashes with the solid upper-lid
                // lash line built by tools/cartoon_hm08/lashes.py.
                let sex = if p["gender"].as_f64().unwrap_or(1.) >= 0.5 {
                    "male"
                } else {
                    "female"
                };
                let ribbon = format!("proxies/cartoon_eyelashes_{sex}.pack");
                if label == "Eyelashes"
                    && cartoon_amount(recipe) >= 0.5
                    && lib.root.join(&ribbon).is_file()
                {
                    let mut swapped = item.clone();
                    swapped["file"] = json!(ribbon);
                    swapped["material"] = lib.proxy(&ribbon)?.meta["material"].clone();
                    add_proxy(&swapped, label, tint, hide)?;
                } else {
                    add_proxy(item, label, tint, hide)?;
                }
            }
        }
        if p["tongue"] != false && s(p, "teeth") != "none" {
            add_proxy(&lib.catalog["tongue"], "Tongue", "#ffffff", false)?;
        }
        if let Some(eye) = find(&lib.catalog, "eyes", s(p, "eyes")) {
            let mut item = eye.clone();
            item["file"] = lib.catalog["eyeMesh"].clone();
            add_proxy(&item, "Eyes", "#ffffff", false)?;
            // Match Ara's eye assembly: the outer cornea uses the transparent
            // atlas corner. Rendering it with the opaque iris material hides
            // both pupils behind flat blue discs. UV V is already flipped.
            let eyes = parts.last_mut().expect("eye proxy was just added");
            eyes.indices = eyes
                .indices
                .chunks_exact(3)
                .filter(|triangle| {
                    !triangle.iter().all(|&i| {
                        let uv = eyes.uvs[i as usize];
                        uv[0] >= 0.8 && uv[1] >= 0.8
                    })
                })
                .flatten()
                .copied()
                .collect();
            eyes.material["roughness"] = json!(0.12);
        }
        let has_custom_beard = recipe
            .custom_garments
            .iter()
            .any(|garment| garment.slot == crate::wardrobe_assets::GarmentSlot::Beard);
        // Resolve the conflict before native wardrobe construction so both card
        // beards and painted cartoon stubble are suppressed, including recipes
        // loaded directly rather than selected through the UI.
        let mut wardrobe_params = p.clone();
        if has_custom_beard {
            wardrobe_params["facialHair"] = json!("none");
        }
        let mut wardrobe = crate::native_wardrobe::build_wardrobe(lib, &coords, &wardrobe_params)?;
        // Cartoon beards: short and full card beards read as patchy clumps at toy
        // scale, so they become the painted stubble region in the beard colour, like
        // the Meshy cast's painted beards. Mustaches and goatees keep their cards.
        if !has_custom_beard
            && cartoon_amount(recipe) >= 0.5
            && matches!(s(p, "facialHair"), "short_beard" | "full_beard")
        {
            wardrobe
                .parts
                .retain(|part| !part.name.starts_with("facialHair:"));
            let ink = p["beardColor"].as_str().unwrap_or("#241a13");
            if let Some(part) = crate::native_wardrobe::tattoo_part(lib, &coords, "stubble", ink)? {
                wardrobe.parts.push(part);
            }
        }
        let hides_hair = wardrobe.hides_hair
            || recipe
                .custom_garments
                .iter()
                .any(|g| g.slot == crate::wardrobe_assets::GarmentSlot::Hat);
        let has_custom_hair = recipe
            .custom_garments
            .iter()
            .any(|g| g.slot == crate::wardrobe_assets::GarmentSlot::Hair);
        if hides_hair || has_custom_hair {
            wardrobe
                .parts
                .retain(|part| !is_hair(&part.name) && part.name != "tattoo_buzz_scalp");
        }
        let mut hidden_faces = vec![false; array(&lib.base, "faceVerts")?.u32()?.len() / 4];
        for face in wardrobe.hidden_faces {
            if let Some(h) = hidden_faces.get_mut(face as usize) {
                *h = true;
            }
        }
        if !hides_hair && !has_custom_hair {
            if let Some(item) = find(&lib.catalog, "hair", s(p, "hair")) {
                // Cartoon mode swaps the strand cards for the solid shell built by
                // tools/cartoon_hair (same binding, flat colour in the proxy meta).
                let shell = format!("proxies/cartoon_hair_{}.pack", s(p, "hair"));
                let item = if cartoon_amount(recipe) >= 0.5 && lib.root.join(&shell).is_file() {
                    let mut swapped = item.clone();
                    swapped["file"] = json!(shell);
                    swapped["material"] = lib.proxy(&shell)?.meta["material"].clone();
                    swapped["material"]["cartoonShell"] = json!(true);
                    swapped
                } else {
                    item.clone()
                };
                let (part, _) = fitted_part(
                    lib,
                    &item,
                    &coords,
                    "Hair",
                    tint_material(&item, s(p, "hairTint")),
                )?;
                parts.push(part);
            }
        }
        let mut hair_fit = crate::native_hair::HairFitStats::default();
        if !recipe.custom_garments.is_empty() {
            // Recompute once from the final body, including cartoon, face,
            // custom-target and breast morphs. Transport each item's authored
            // offsets as well as its anchors; keep saved source meshes intact.
            let neutral: Vec<[f32; 3]> = array(&lib.base, "positions")?
                .f32()?
                .chunks_exact(3)
                .map(|p| [p[0], p[1], p[2]])
                .collect();
            let deformation = crate::native_fit::BodyDeformation::new(
                &neutral,
                &coords,
                array(&lib.base, "faceVerts")?.u32()?,
                array(&lib.base, "regions")?.u8()?,
            )?;
            for garment in &recipe.custom_garments {
                let mut part = garment.fit_adaptive(&coords, &deformation)?;
                if garment.slot == crate::wardrobe_assets::GarmentSlot::Hair {
                    let fit = crate::native_hair::fit_custom_head_clearance(
                        garment,
                        &mut part,
                        &neutral,
                        &coords,
                        array(&lib.base, "faceVerts")?.u32()?,
                        array(&lib.base, "regions")?.u8()?,
                    )?;
                    hair_fit.corrected_vertices += fit.corrected_vertices;
                    hair_fit.max_displacement = hair_fit.max_displacement.max(fit.max_displacement);
                    hair_fit.remaining_violations += fit.remaining_violations;
                }
                parts.push(part);
                if garment
                    .hidden_faces
                    .iter()
                    .any(|&index| index as usize >= hidden_faces.len())
                {
                    return Err(format!("{} has an invalid hidden body face", garment.id));
                }
                for &index in &garment.hidden_faces {
                    hidden_faces[index as usize] = true;
                }
            }
        }
        parts.extend(wardrobe.parts);
        let skin = pick_skin(p, lib.catalog["skins"].as_array().ok_or("no skins")?)?;
        let mut material = tint_material(skin, s(p, "skinTint"));
        if material["color"] == "#808080" {
            material["color"] = p["skinTint"].clone();
        }
        material["roughness"] = json!(0.66);
        let face_verts = array(&lib.base, "faceVerts")?.u32()?;
        let sizes = vec![4; face_verts.len() / 4];
        // The low proxy has different face indices. Use the canonical mesh when
        // occlusion is active rather than applying hm08 masks to another topology.
        let body = if s(p, "detail") == "low"
            && !hidden.iter().any(|&v| v)
            && !hidden_faces.iter().any(|&v| v)
        {
            let id = if p["gender"].as_f64().unwrap_or(1.) >= 0.5 {
                "male1591"
            } else {
                "female1605"
            };
            if let Some(lod) = find(&lib.catalog, "bodies", id) {
                fitted_part(lib, lod, &coords, "Body", material.clone())?.0
            } else {
                polygon_part(
                    "Body",
                    &coords,
                    &sizes,
                    face_verts,
                    array(&lib.base, "faceUVs")?.u32()?,
                    array(&lib.base, "uvs")?.f32()?,
                    None,
                    material.clone(),
                    &hidden,
                    &hidden_faces,
                )?
            }
        } else {
            polygon_part(
                "Body",
                &coords,
                &sizes,
                face_verts,
                array(&lib.base, "faceUVs")?.u32()?,
                array(&lib.base, "uvs")?.f32()?,
                None,
                material,
                &hidden,
                &hidden_faces,
            )?
        };
        let is_custom_hair_part = |part: &NativePart| {
            recipe.custom_garments.iter().any(|garment| {
                garment.slot == crate::wardrobe_assets::GarmentSlot::Hair
                    && part.name == format!("custom:{}", garment.id)
            })
        };
        if parts
            .iter()
            .any(|part| is_hair(&part.name) || is_custom_hair_part(part))
        {
            let full_body = polygon_part(
                "Hair collider",
                &coords,
                &sizes,
                face_verts,
                array(&lib.base, "faceUVs")?.u32()?,
                array(&lib.base, "uvs")?.f32()?,
                None,
                json!({}),
                &[],
                &[],
            )?;
            let clothing: Vec<NativePart> = parts
                .iter()
                .filter(|part| {
                    part.name == "Outfit"
                        || part.name == "Hat"
                        || schema["garments"].as_array().unwrap().iter().any(|g| {
                            g["id"]
                                .as_str()
                                .is_some_and(|id| part.name.starts_with(&format!("{id}_")))
                        })
                        || recipe.custom_garments.iter().any(|garment| {
                            garment.slot != crate::wardrobe_assets::GarmentSlot::Hair
                                && part.name == format!("custom:{}", garment.id)
                        })
                })
                .cloned()
                .collect();
            for part in parts
                .iter_mut()
                .filter(|part| is_hair(&part.name) || is_custom_hair_part(part))
                // Built-in cartoon shells have authored clearance. Imported
                // hair still needs bounded neck/collar fitting; this pass
                // excludes the scalp, including a closed cap's inner surface.
                .filter(|part| is_custom_hair_part(part) || part.material["cartoonShell"] != true)
            {
                let fit = crate::native_hair::fit_hair_clearance(part, &full_body, &clothing)?;
                hair_fit.corrected_vertices += fit.corrected_vertices;
                hair_fit.max_displacement = hair_fit.max_displacement.max(fit.max_displacement);
                hair_fit.remaining_violations += fit.remaining_violations;
            }
        }
        let mut floor = face_verts
            .iter()
            .map(|&i| coords[i as usize][1])
            .fold(f32::INFINITY, f32::min);
        let top = face_verts
            .iter()
            .map(|&i| coords[i as usize][1])
            .fold(f32::NEG_INFINITY, f32::max);
        for part in &parts {
            let name = part.name.to_lowercase();
            if [
                "shoe", "boot", "sandal", "heels", "loafer", "sneaker", "oxford", "slide",
                "flipflop",
            ]
            .iter()
            .any(|k| name.contains(k))
            {
                for p in &part.positions {
                    floor = floor.min(p[1]);
                }
            }
        }
        parts.insert(0, body);
        if recipe.cartoon.enabled && recipe.cartoon.strength > 0. {
            for part in &mut parts {
                crate::native_cartoon::stylise_part(
                    &part.name,
                    &mut part.material,
                    recipe.cartoon.strength,
                );
            }
        }
        for part in &mut parts {
            for v in &mut part.positions {
                v[0] *= 0.1;
                v[1] = (v[1] - floor) * 0.1;
                v[2] *= 0.1;
            }
        }
        for delta in &mut face_deltas {
            for d in delta {
                for v in d {
                    *v *= 0.1;
                }
            }
        }
        let rig = if s(p, "rig") != "none" {
            let mut rig = NativeRig::build(lib, &coords, floor, p)?;
            if let Some(realistic) = realistic_clearance {
                let cartoon = crate::native_rig::arm_clearance(lib, &coords)?;
                rig.set_arm_spread((cartoon - realistic) * ARM_SPREAD_GAIN);
            }
            Some(rig)
        } else {
            None
        };
        if let Some(rig) = &rig {
            // Imported animation ids use a library prefix. Recipe validation
            // permits those ids before the corresponding library has loaded,
            // so verify the completed rig where its skeleton is known.
            for id in std::iter::once(&recipe.preview_clip).chain(recipe.export.clips.iter()) {
                if id.contains(':') && rig.clip_duration(id).is_none() {
                    return Err(format!(
                        "missing imported animation clip {id:?} for {} rig",
                        rig.kind
                    ));
                }
            }
            parts = parts
                .iter()
                .map(|part| {
                    if part.name == "Bicycle" {
                        part.clone()
                    } else {
                        rig.to_rest_pose(part)
                    }
                })
                .collect();
        }
        let mut result = Self {
            parts,
            height: (top - floor) * 0.1,
            rig,
            face_names,
            face_deltas,
            recipe: recipe.clone(),
            hair_fit,
        };
        if s(p, "detail") == "high" {
            for part in &mut result.parts {
                if part.name == "Body" || part.name == "Outfit" {
                    subdivide(part);
                }
            }
        }
        Ok(result)
    }
    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.indices.len() / 3).sum()
    }
    pub fn face_offsets(
        &self,
        part: &NativePart,
        weights: &BTreeMap<String, f32>,
    ) -> Vec<[f32; 3]> {
        let mut output = vec![[0.; 3]; part.positions.len()];
        for (name, delta) in self.face_names.iter().zip(&self.face_deltas) {
            let weight = weights.get(name).copied().unwrap_or(0.);
            if weight == 0. {
                continue;
            }
            for (i, refs) in part.refs.iter().enumerate() {
                for k in 0..3 {
                    if let Some(d) = delta.get(refs[k] as usize) {
                        for axis in 0..3 {
                            output[i][axis] += d[axis] * part.ref_weights[i][k] * weight;
                        }
                    }
                }
            }
        }
        output
    }
}
fn subdivide(part: &mut NativePart) {
    // Native midpoint refinement retains all material/UV/reference boundaries.
    let mut edges = BTreeMap::new();
    let mut indices = vec![];
    for tri in part.indices.clone().chunks_exact(3) {
        let mut mid = [0; 3];
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let key = (a.min(b), a.max(b));
            mid[k] = *edges.entry(key).or_insert_with(|| {
                let index = part.positions.len() as u32;
                let (a, b) = (a as usize, b as usize);
                part.positions.push(std::array::from_fn(|k| {
                    (part.positions[a][k] + part.positions[b][k]) * 0.5
                }));
                part.uvs.push(std::array::from_fn(|k| {
                    (part.uvs[a][k] + part.uvs[b][k]) * 0.5
                }));
                let mut weights = BTreeMap::<u32, f32>::new();
                for vertex in [a, b] {
                    for k in 0..3 {
                        *weights.entry(part.refs[vertex][k]).or_default() +=
                            part.ref_weights[vertex][k] * 0.5;
                    }
                }
                let mut weights: Vec<_> = weights.into_iter().filter(|(_, w)| *w > 0.).collect();
                weights.sort_by(|a, b| b.1.total_cmp(&a.1));
                weights.truncate(3);
                let sum = weights.iter().map(|(_, w)| w).sum::<f32>();
                let mut refs = [0; 3];
                let mut blend = [0.; 3];
                for (k, (i, w)) in weights.into_iter().enumerate() {
                    refs[k] = i;
                    blend[k] = w / sum;
                }
                part.refs.push(refs);
                part.ref_weights.push(blend);
                index
            });
        }
        indices.extend([
            tri[0], mid[0], mid[2], tri[1], mid[1], mid[0], tri[2], mid[2], mid[1], mid[0], mid[1],
            mid[2],
        ]);
    }
    part.indices = indices;
}

fn is_hair(name: &str) -> bool {
    name == "Hair" || name.starts_with("hair:")
}
