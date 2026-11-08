//! Native MakeHuman character assembly. Geometry and customization run entirely in Rust.
pub use crate::native_assets::NativeLibrary;
use crate::{
    ara::{AraRecipe, multiply_colors, native_catalog},
    native_assets::{Array, Pack},
    native_morph::{apply_targets, apply_weighted_targets, fit_proxy},
    native_rig::NativeRig,
};
use bevy::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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
    /// Immutable authored topology for metadata whose topology must survive
    /// later fitting, morphing, and runtime triangle culling. Only opted-in
    /// custom garments populate this.
    pub skin_weight_source_topology: Option<Arc<SkinWeightSourceTopology>>,
    pub material: Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SkinWeightSourceTopology {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
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

/// Ordering for custom clothing-only occlusion.  Source geometry remains
/// intact; an outer garment must explicitly request runtime triangle culling.
fn custom_garment_layer(slot: crate::wardrobe_assets::GarmentSlot) -> Option<u8> {
    match slot {
        crate::wardrobe_assets::GarmentSlot::Feet => Some(1),
        crate::wardrobe_assets::GarmentSlot::Bottom => Some(2),
        crate::wardrobe_assets::GarmentSlot::Top => Some(3),
        crate::wardrobe_assets::GarmentSlot::Jacket => Some(4),
        _ => None,
    }
}

/// The covered tuck is intentionally a Bottom-only repair. Validate its
/// schema before any ordinary fitting mutates a sibling garment, including
/// malformed requests which otherwise would not opt into clearance at all.
fn covered_upper_tuck_request_for_garment(
    garment: &crate::wardrobe_assets::CustomGarment,
    part: &NativePart,
) -> Result<bool, String> {
    let physical = crate::native_cloth::physical_upper_coverage_request(part)?;
    if crate::native_cloth::covered_upper_tuck_request(part)?.is_none() {
        if physical {
            return Err(format!(
                "{} fitClearanceUpperBandPhysicalCoverage requires fitClearanceUpperBandCoveredTuckMm",
                garment.id
            ));
        }
        return Ok(false);
    }
    if garment.slot != crate::wardrobe_assets::GarmentSlot::Bottom {
        return Err(format!(
            "{} can use fitClearanceUpperBandCoveredTuckMm only on a Bottom",
            garment.id
        ));
    }
    if !crate::native_cloth::covered_only_upper_clearance_request(part)? {
        return Err(format!(
            "{} fitClearanceUpperBandCoveredTuckMm requires fitClearanceUpperBandCoveredOnly: true",
            garment.id
        ));
    }
    Ok(true)
}

fn garment_occludes_lower_layers(garment: &crate::wardrobe_assets::CustomGarment) -> bool {
    matches!(
        &garment.slot,
        crate::wardrobe_assets::GarmentSlot::Bottom
            | crate::wardrobe_assets::GarmentSlot::Top
            | crate::wardrobe_assets::GarmentSlot::Jacket
    ) && garment.material["fitOccludesGarments"].as_bool() == Some(true)
}

fn snapshot_outer_parts(
    parts: &[NativePart],
    garments: &[crate::wardrobe_assets::CustomGarment],
) -> Result<Vec<(NativePart, u8)>, String> {
    garments
        .iter()
        .filter(|garment| garment_occludes_lower_layers(garment))
        .map(|garment| {
            let layer = custom_garment_layer(garment.slot.clone())
                .expect("eligible custom outer garment has a layer");
            let outer = parts
                .iter()
                .find(|part| part.name == format!("custom:{}", garment.id))
                .cloned()
                .ok_or_else(|| format!("missing fitted custom garment {}", garment.id))?;
            Ok((outer, layer))
        })
        .collect()
}

/// Canonical anchors covered by higher, explicitly opaque custom layers after
/// their ordinary body-clearance pass.  Saved reviewed masks remain useful for
/// static coverage, while runtime candidates add the actual fitted coverage.
fn higher_opaque_covered_vertices(
    lower_layer: u8,
    parts: &[NativePart],
    garments: &[crate::wardrobe_assets::CustomGarment],
    coords: &[[f32; 3]],
    face_verts: &[u32],
    runtime_confirmation_required: &BTreeSet<String>,
) -> Result<BTreeSet<u32>, String> {
    let mut covered_faces = BTreeSet::new();
    for garment in garments.iter().filter(|garment| {
        garment_occludes_lower_layers(garment)
            && custom_garment_layer(garment.slot.clone()).is_some_and(|layer| layer > lower_layer)
    }) {
        // A shell or layer solve can move an outer garment after its reviewed
        // static mask was authored. Such a mask is not evidence of its final
        // coverage; require the final runtime sampler to confirm it instead.
        if !runtime_confirmation_required.contains(&garment.id) {
            covered_faces.extend(garment.hidden_faces.iter().copied());
        }
        let part = parts
            .iter()
            .find(|part| part.name == format!("custom:{}", garment.id))
            .ok_or_else(|| format!("missing fitted custom garment {}", garment.id))?;
        if let Some(runtime) =
            crate::native_occlusion::runtime_hidden_faces(part, coords, face_verts)?
        {
            covered_faces.extend(runtime);
        }
    }
    let mut covered_vertices = BTreeSet::new();
    for face in covered_faces {
        let vertices = face_verts
            .get(face as usize * 4..face as usize * 4 + 4)
            .ok_or("covered body face is out of bounds")?;
        covered_vertices.extend(vertices.iter().copied());
    }
    Ok(covered_vertices)
}

/// A cloth vertex can use reduced waist clearance only when every anchor that
/// materially contributes to it is covered.  This keeps seams that straddle a
/// hem at the legacy body clearance.
fn covered_anchor_mask(
    part: &NativePart,
    covered_vertices: &BTreeSet<u32>,
) -> Result<Vec<bool>, String> {
    if part.positions.len() != part.refs.len() || part.positions.len() != part.ref_weights.len() {
        return Err(format!(
            "{} has inconsistent custom clothing bindings",
            part.name
        ));
    }
    part.refs
        .iter()
        .zip(&part.ref_weights)
        .map(|(refs, weights)| {
            let mut meaningful = false;
            for (&reference, &weight) in refs.iter().zip(weights) {
                if !weight.is_finite() || weight < 0. {
                    return Err(format!(
                        "{} has invalid custom clothing binding weights",
                        part.name
                    ));
                }
                if weight > f32::EPSILON {
                    meaningful = true;
                    if !covered_vertices.contains(&reference) {
                        return Ok(false);
                    }
                }
            }
            Ok(meaningful)
        })
        .collect()
}

fn wardrobe_params_for_custom_garments(
    params: &Value,
    garments: &[crate::wardrobe_assets::CustomGarment],
) -> Value {
    let mut wardrobe_params = params.clone();
    if garments
        .iter()
        .any(|garment| garment.slot == crate::wardrobe_assets::GarmentSlot::Beard)
    {
        wardrobe_params["facialHair"] = json!("none");
    }
    // An opaque custom bottom can explicitly replace the stock brief layer.
    // Bottoms with cutouts retain it unless their author opts in.
    if garments.iter().any(|garment| {
        garment.slot == crate::wardrobe_assets::GarmentSlot::Bottom
            && garment.material["occludesUnderwear"].as_bool() == Some(true)
    }) {
        wardrobe_params["underwear"] = json!(false);
    }
    wardrobe_params
}

fn is_grounding_footwear(name: &str, garments: &[crate::wardrobe_assets::CustomGarment]) -> bool {
    if let Some(id) = name.strip_prefix("custom:") {
        return garments.iter().any(|garment| {
            garment.id == id && garment.slot == crate::wardrobe_assets::GarmentSlot::Feet
        });
    }
    let name = name.to_lowercase();
    [
        "shoe", "boot", "sandal", "heels", "loafer", "sneaker", "oxford", "slide", "flipflop",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// Layer culling only removes triangle indices so authored custom-asset data
/// remains available to the editor. Remove the resulting empty runtime parts
/// only after they have participated in assembly-dependent work such as floor
/// selection.
fn remove_fully_culled_custom_parts(parts: &mut Vec<NativePart>) {
    parts.retain(|part| !part.name.starts_with("custom:") || !part.indices.is_empty());
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
        skin_weight_source_topology: None,
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
        let wardrobe_params = wardrobe_params_for_custom_garments(p, &recipe.custom_garments);
        let cartoon_stock_clothing = recipe.cartoon.enabled && recipe.cartoon.strength > 0.;
        let needs_deformation = cartoon_stock_clothing || !recipe.custom_garments.is_empty();
        let neutral: Vec<[f32; 3]> = if needs_deformation {
            array(&lib.base, "positions")?
                .f32()?
                .chunks_exact(3)
                .map(|p| [p[0], p[1], p[2]])
                .collect()
        } else {
            vec![]
        };
        // Custom garments retain their existing strict error path. Stock
        // proxies fall back to their established fitting if a malformed body
        // field cannot be constructed for cartoon-only transport.
        let deformation = if !recipe.custom_garments.is_empty() {
            Some(crate::native_fit::BodyDeformation::new(
                &neutral,
                &coords,
                array(&lib.base, "faceVerts")?.u32()?,
                array(&lib.base, "regions")?.u8()?,
            )?)
        } else if cartoon_stock_clothing {
            crate::native_fit::BodyDeformation::new(
                &neutral,
                &coords,
                array(&lib.base, "faceVerts")?.u32()?,
                array(&lib.base, "regions")?.u8()?,
            )
            .ok()
        } else {
            None
        };
        let mut wardrobe = crate::native_wardrobe::build_wardrobe_with_deformation(
            lib,
            &coords,
            &wardrobe_params,
            cartoon_stock_clothing
                .then_some(deformation.as_ref())
                .flatten(),
        )?;
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
            let deformation = deformation
                .as_ref()
                .expect("custom garments require deformation");
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
                if !garment.material["fitBodyMaskCandidates"].is_null()
                    && !garment.material["fitBodyMaskCandidates"].is_array()
                {
                    return Err(format!("{} has invalid fitBodyMaskCandidates", garment.id));
                }
                // Clearance collides against the same visible body as legacy
                // fitting, including this garment's saved mask. Adaptive
                // replacement happens only after clearance below.
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
        let adaptive_occlusion = recipe.custom_garments.iter().any(|garment| {
            matches!(
                garment.slot,
                crate::wardrobe_assets::GarmentSlot::Top
                    | crate::wardrobe_assets::GarmentSlot::Bottom
                    | crate::wardrobe_assets::GarmentSlot::Jacket
                    | crate::wardrobe_assets::GarmentSlot::Feet
            ) && !garment.material["fitBodyMaskCandidates"].is_null()
        });
        let mut body = if s(p, "detail") == "low"
            && !adaptive_occlusion
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
        // Validate every explicit clothing-layer request before the ordinary
        // body pass changes any fitted geometry.  Shell linings are coupled to
        // their outer controls by a separate solver, so this new independent
        // layer pass fails closed for them rather than breaking recoupling.
        let mut layer_pre_clearance = BTreeMap::<String, Vec<[f32; 3]>>::new();
        // Covered-only upper bands depend on the final ordinary fit of an
        // explicitly higher opaque layer. Validate the new request before any
        // ordinary pass, then defer only those garments until their higher
        // dependencies have been fitted.
        let mut covered_only_clearance = BTreeSet::<String>::new();
        let mut physical_coverage_tuck = BTreeSet::<String>::new();
        for garment in &recipe.custom_garments {
            let part = parts
                .iter()
                .find(|part| part.name == format!("custom:{}", garment.id))
                .ok_or_else(|| format!("missing fitted custom garment {}", garment.id))?;
            if covered_upper_tuck_request_for_garment(garment, part)?
                && crate::native_cloth::physical_upper_coverage_request(part)?
            {
                physical_coverage_tuck.insert(garment.id.clone());
            }
        }
        for garment in &recipe.custom_garments {
            if !matches!(
                garment.slot,
                crate::wardrobe_assets::GarmentSlot::Top
                    | crate::wardrobe_assets::GarmentSlot::Bottom
                    | crate::wardrobe_assets::GarmentSlot::Jacket
                    | crate::wardrobe_assets::GarmentSlot::Feet
            ) {
                continue;
            }
            let part = parts
                .iter()
                .find(|part| part.name == format!("custom:{}", garment.id))
                .ok_or_else(|| format!("missing fitted custom garment {}", garment.id))?;
            if crate::native_cloth::covered_only_upper_clearance_request(part)? {
                if !crate::native_shell::parse_shell_pairs(garment)?.is_empty() {
                    return Err(format!(
                        "{} cannot use fitClearanceUpperBandCoveredOnly with fitShellDisplacementPairs",
                        garment.id
                    ));
                }
                covered_only_clearance.insert(garment.id.clone());
            }
        }
        let physical_tuck_source = physical_coverage_tuck
            .iter()
            .map(|id| {
                parts
                    .iter()
                    .find(|part| part.name == format!("custom:{id}"))
                    .map(|part| (id.clone(), part.positions.clone()))
                    .ok_or_else(|| format!("missing fitted custom garment {id}"))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        for garment in &recipe.custom_garments {
            if !matches!(
                garment.slot,
                crate::wardrobe_assets::GarmentSlot::Top
                    | crate::wardrobe_assets::GarmentSlot::Bottom
                    | crate::wardrobe_assets::GarmentSlot::Jacket
            ) {
                continue;
            }
            let part = parts
                .iter()
                .find(|part| part.name == format!("custom:{}", garment.id))
                .ok_or_else(|| format!("missing fitted custom garment {}", garment.id))?;
            if crate::native_cloth::layer_clearance_request(part)?.is_some() {
                if covered_only_clearance.contains(&garment.id) {
                    return Err(format!(
                        "{} cannot combine fitClearanceUpperBandCoveredOnly with fitLayerClearanceMm",
                        garment.id
                    ));
                }
                if !crate::native_shell::parse_shell_pairs(garment)?.is_empty() {
                    return Err(format!(
                        "{} cannot use fitLayerClearanceMm with fitShellDisplacementPairs",
                        garment.id
                    ));
                }
                layer_pre_clearance.insert(garment.id.clone(), part.positions.clone());
            }
        }
        // Snapshot shell garments before their ordinary clothing pass. The
        // subsequent lining solver caps its *total* control displacement from
        // this state, so the two clearance stages cannot silently add to 8cm.
        let mut shell_pre_clearance = BTreeMap::<String, Vec<[f32; 3]>>::new();
        for garment in &recipe.custom_garments {
            if !crate::native_shell::parse_shell_pairs(garment)?.is_empty() {
                let part = parts
                    .iter()
                    .find(|part| part.name == format!("custom:{}", garment.id))
                    .ok_or_else(|| format!("missing fitted custom garment {}", garment.id))?;
                shell_pre_clearance.insert(garment.id.clone(), part.positions.clone());
            }
        }
        // Imported clothing can explicitly request a small post-morph body
        // clearance. The collider is the visible body assembled above, so
        // hidden anatomy never pushes source clothing outward.
        for garment in &recipe.custom_garments {
            if !matches!(
                garment.slot,
                crate::wardrobe_assets::GarmentSlot::Top
                    | crate::wardrobe_assets::GarmentSlot::Bottom
                    | crate::wardrobe_assets::GarmentSlot::Jacket
                    | crate::wardrobe_assets::GarmentSlot::Feet
            ) {
                continue;
            }
            if let Some(part) = parts
                .iter_mut()
                .find(|part| part.name == format!("custom:{}", garment.id))
            {
                if !covered_only_clearance.contains(&garment.id) {
                    let _ = crate::native_cloth::fit_custom_clothing_clearance(part, &body)?;
                }
            }
        }
        // Thin source walls cannot be cleared independently: move only their
        // outer authored weld groups, recouple the lining after every pass,
        // then bind animation after final positions are known.
        for garment in &recipe.custom_garments {
            if let Some(pre_clearance) = shell_pre_clearance.get(&garment.id) {
                let part = parts
                    .iter_mut()
                    .find(|part| part.name == format!("custom:{}", garment.id))
                    .ok_or_else(|| format!("missing fitted custom garment {}", garment.id))?;
                let _ = crate::native_shell::resolve_shell_clearance(
                    garment,
                    part,
                    &body,
                    pre_clearance,
                )?;
            }
        }
        // Layer solvers must see the largest permitted lower geometry even
        // though covered-only parts wait for final outer coverage below. This
        // one-way conservative preview prevents a later base-clearance replay
        // from invalidating an already-settled outer layer.
        let mut conservative_covered_lower = BTreeMap::<String, NativePart>::new();
        for id in &covered_only_clearance {
            let mut preview = parts
                .iter()
                .find(|part| part.name == format!("custom:{id}"))
                .cloned()
                .ok_or_else(|| format!("missing fitted custom garment {id}"))?;
            let uncovered = vec![false; preview.positions.len()];
            let _ = crate::native_cloth::fit_custom_clothing_clearance_with_coverage(
                &mut preview,
                &body,
                Some(&uncovered),
            )?;
            conservative_covered_lower.insert(id.clone(), preview);
        }
        // Run layer clearance after body and shell fitting, while all custom
        // meshes still have their original triangle sets and before animation
        // bindings or coverage culling.  The complete current canonical body
        // supplies directions only; the lower-clothing collider below never
        // includes hidden anatomy.
        if !layer_pre_clearance.is_empty() {
            let full_body = polygon_part(
                "Layer clearance normals",
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
            let mut ranked: Vec<(u8, &str)> = recipe
                .custom_garments
                .iter()
                .filter_map(|garment| {
                    custom_garment_layer(garment.slot.clone())
                        .map(|rank| (rank, garment.id.as_str()))
                })
                .collect();
            // Stable ID order makes equal-rank assets deterministic without
            // making recipe order part of the collision policy.
            ranked.sort_unstable_by(|left, right| left.cmp(right));
            for (rank, id) in &ranked {
                let Some(source) = layer_pre_clearance.get(*id) else {
                    continue;
                };
                let lower: Vec<NativePart> = ranked
                    .iter()
                    .filter(|(lower_rank, _)| lower_rank < rank)
                    .filter_map(|(_, lower_id)| {
                        conservative_covered_lower.get(*lower_id).or_else(|| {
                            parts
                                .iter()
                                .find(|part| part.name == format!("custom:{lower_id}"))
                        })
                    })
                    .cloned()
                    .collect();
                let lower_refs: Vec<&NativePart> = lower.iter().collect();
                let part = parts
                    .iter_mut()
                    .find(|part| part.name == format!("custom:{id}"))
                    .ok_or_else(|| format!("missing fitted custom garment {id}"))?;
                let _ = crate::native_cloth::fit_custom_layer_clearance(
                    part,
                    &lower_refs,
                    &full_body,
                    source,
                )?;
            }
        }
        // Final outer geometry is now settled. A shell or layer-adjusted
        // occluder must prove coverage with its runtime mask rather than a
        // source-reviewed static mask. Fit deferred lower garments once from
        // their untouched source positions, in deterministic high-to-low
        // order, so no clearance/layer feedback cycle is introduced.
        let mut outer_geometry_changed: BTreeSet<String> = shell_pre_clearance
            .keys()
            .chain(layer_pre_clearance.keys())
            .chain(covered_only_clearance.iter())
            .cloned()
            .collect();
        // Ordinary body clearance also changes the outer surface used by a
        // saved face mask. Require its post-fit runtime sampler rather than
        // treating that source mask as final coverage.
        outer_geometry_changed.extend(
            recipe
                .custom_garments
                .iter()
                .filter(|garment| !garment.material["fitClearanceMm"].is_null())
                .map(|garment| garment.id.clone()),
        );
        let mut covered_only_ranked: Vec<(u8, &str)> = recipe
            .custom_garments
            .iter()
            .filter(|garment| covered_only_clearance.contains(&garment.id))
            .filter_map(|garment| {
                custom_garment_layer(garment.slot.clone()).map(|rank| (rank, garment.id.as_str()))
            })
            .collect();
        covered_only_ranked
            .sort_unstable_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(right.1)));
        for (lower_layer, id) in covered_only_ranked {
            let covered_vertices = higher_opaque_covered_vertices(
                lower_layer,
                &parts,
                &recipe.custom_garments,
                &coords,
                face_verts,
                &outer_geometry_changed,
            )?;
            let coverage = {
                let part = parts
                    .iter()
                    .find(|part| part.name == format!("custom:{id}"))
                    .ok_or_else(|| format!("missing fitted custom garment {id}"))?;
                covered_anchor_mask(part, &covered_vertices)?
            };
            let part = parts
                .iter_mut()
                .find(|part| part.name == format!("custom:{id}"))
                .ok_or_else(|| format!("missing fitted custom garment {id}"))?;
            let _ = crate::native_cloth::fit_custom_clothing_clearance_with_coverage(
                part,
                &body,
                Some(&coverage),
            )?;
        }
        // Canonical coverage is deliberately conservative around hems. An
        // explicit physical-waist request may additionally prove a final
        // opaque outer surface from the full canonical body. It tucks a Bottom
        // inward, or restores a slightly under-floor authored wall outward only
        // after the same outer-room proof; it is never an ordinary hidden-body
        // clearance pass.
        if !physical_coverage_tuck.is_empty() {
            let full_body = polygon_part(
                "Physical waist tuck normals",
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
            let outer = snapshot_outer_parts(&parts, &recipe.custom_garments)?;
            let mut requested: Vec<(u8, &str)> = recipe
                .custom_garments
                .iter()
                .filter(|garment| physical_coverage_tuck.contains(&garment.id))
                .filter_map(|garment| {
                    custom_garment_layer(garment.slot.clone())
                        .map(|rank| (rank, garment.id.as_str()))
                })
                .collect();
            requested.sort_unstable();
            for (rank, id) in requested {
                let higher: Vec<&NativePart> = outer
                    .iter()
                    .filter(|(_, outer_rank)| *outer_rank > rank)
                    .map(|(part, _)| part)
                    .collect();
                let source = physical_tuck_source
                    .get(id)
                    .ok_or_else(|| format!("missing physical-tuck source positions for {id}"))?;
                let part = parts
                    .iter_mut()
                    .find(|part| part.name == format!("custom:{id}"))
                    .ok_or_else(|| format!("missing fitted custom garment {id}"))?;
                let _ = crate::native_cloth::fit_custom_clothing_physical_upper_tuck(
                    part, &full_body, &higher, source,
                )?;
            }
        }
        // Animation rebinding happens only after shell controls settle. The
        // inner lining then inherits the dominant final outer rig stream;
        // never rebind it again after this copy.
        for garment in &recipe.custom_garments {
            if !matches!(
                garment.slot,
                crate::wardrobe_assets::GarmentSlot::Top
                    | crate::wardrobe_assets::GarmentSlot::Bottom
                    | crate::wardrobe_assets::GarmentSlot::Jacket
                    | crate::wardrobe_assets::GarmentSlot::Feet
            ) {
                continue;
            }
            if let Some(part) = parts
                .iter_mut()
                .find(|part| part.name == format!("custom:{}", garment.id))
            {
                garment.fit_animation_bindings(lib, &coords, part)?;
                crate::native_shell::couple_shell_bindings(garment, part)?;
            }
        }
        // An outer imported top or jacket may explicitly suppress fully
        // covered triangles of lower custom clothing. This affects assembled
        // runtime and exported meshes alike while leaving saved source assets
        // and all vertex/UV/reference streams unchanged.
        // Snapshot every outer mesh before culling any lower layer. A jacket
        // may subsequently cull a top, but the jacket must still test pants
        // against its original complete surface regardless of recipe order.
        let occluders = snapshot_outer_parts(&parts, &recipe.custom_garments)?;
        for (outer, outer_layer) in occluders {
            for lower_id in recipe
                .custom_garments
                .iter()
                .filter(|garment| {
                    custom_garment_layer(garment.slot.clone())
                        .is_some_and(|layer| layer < outer_layer)
                })
                .map(|garment| garment.id.as_str())
            {
                let lower = parts
                    .iter_mut()
                    .find(|part| part.name == format!("custom:{lower_id}"))
                    .ok_or_else(|| format!("missing fitted custom garment {lower_id}"))?;
                let _ = crate::native_occlusion::cull_fully_occluded_triangles(&outer, lower)?;
            }
        }
        let mut rebuilt_adaptive_mask = false;
        // Clearance used every saved source-reviewed mask above. Preserve that
        // baseline so the final mask cannot expose a body surface that the
        // collision pass deliberately excluded; runtime coverage only adds.
        let mut final_hidden_faces = hidden_faces.clone();
        for garment in &recipe.custom_garments {
            let adaptive_slot = matches!(
                garment.slot,
                crate::wardrobe_assets::GarmentSlot::Top
                    | crate::wardrobe_assets::GarmentSlot::Bottom
                    | crate::wardrobe_assets::GarmentSlot::Jacket
                    | crate::wardrobe_assets::GarmentSlot::Feet
            );
            if adaptive_slot && !garment.material["fitBodyMaskCandidates"].is_null() {
                let part = parts
                    .iter()
                    .find(|part| part.name == format!("custom:{}", garment.id))
                    .ok_or_else(|| format!("missing fitted custom garment {}", garment.id))?;
                // Another opted-in outer garment may have removed every
                // triangle from this lower layer. Its saved mask remains in
                // the baseline, while an empty final mesh has no runtime
                // coverage to add and cannot build a cloth collider.
                if !part.indices.is_empty() {
                    let selected =
                        crate::native_occlusion::runtime_hidden_faces(part, &coords, face_verts)?
                            .expect("candidate opt-in was checked above");
                    for face in selected {
                        final_hidden_faces[face as usize] = true;
                    }
                }
                rebuilt_adaptive_mask = true;
            } else {
                for &face in &garment.hidden_faces {
                    final_hidden_faces[face as usize] = true;
                }
            }
        }
        if rebuilt_adaptive_mask {
            hidden_faces = final_hidden_faces;
            // Candidate masks use canonical native face IDs, so rebuild the
            // visible Body from the final union without changing any garment.
            body = polygon_part(
                "Body",
                &coords,
                &sizes,
                face_verts,
                array(&lib.base, "faceUVs")?.u32()?,
                array(&lib.base, "uvs")?.f32()?,
                None,
                body.material.clone(),
                &hidden,
                &hidden_faces,
            )?;
        }
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
                            // Facial hair blends with scalp hair at the
                            // sideburns; it is not a solid collar obstacle.
                            !matches!(
                                garment.slot,
                                crate::wardrobe_assets::GarmentSlot::Hair
                                    | crate::wardrobe_assets::GarmentSlot::Beard
                            ) && part.name == format!("custom:{}", garment.id)
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
            if is_grounding_footwear(&part.name, &recipe.custom_garments) {
                for p in &part.positions {
                    floor = floor.min(p[1]);
                }
            }
        }
        remove_fully_culled_custom_parts(&mut parts);
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

#[cfg(test)]
mod custom_underwear_tests {
    use super::*;
    use crate::wardrobe_assets::{CustomGarment, GarmentSlot};

    fn garment(slot: GarmentSlot, material: Value) -> CustomGarment {
        CustomGarment {
            id: "test".into(),
            name: "Test".into(),
            slot,
            positions_dm: vec![],
            indices: vec![],
            uvs: vec![],
            refs: vec![],
            weights: vec![],
            reference_points_dm: vec![],
            material,
            hidden_faces: vec![],
        }
    }

    fn native_part(name: &str, indices: Vec<u32>) -> NativePart {
        NativePart {
            name: name.into(),
            positions: vec![],
            indices,
            uvs: vec![],
            refs: vec![],
            ref_weights: vec![],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material: json!({}),
        }
    }

    #[test]
    fn opted_in_custom_bottom_suppresses_underwear_only_in_build_params() {
        let mut recipe = AraRecipe::default();
        recipe.params["underwear"] = json!(true);
        let before = recipe.params.clone();
        let params = wardrobe_params_for_custom_garments(
            &recipe.params,
            &[garment(
                GarmentSlot::Bottom,
                json!({"occludesUnderwear": true}),
            )],
        );
        assert_eq!(params["underwear"], false);
        assert_eq!(
            recipe.params, before,
            "build policy must not mutate the saved recipe"
        );
    }

    #[test]
    fn custom_bottom_does_not_suppress_underwear_without_opt_in() {
        let mut params = AraRecipe::default().params;
        params["underwear"] = json!(true);
        let result = wardrobe_params_for_custom_garments(
            &params,
            &[garment(GarmentSlot::Bottom, json!({}))],
        );
        assert_eq!(result["underwear"], true);
    }

    #[test]
    fn occlusion_metadata_on_other_slots_is_ignored() {
        let mut params = AraRecipe::default().params;
        params["underwear"] = json!(true);
        for slot in [
            GarmentSlot::Top,
            GarmentSlot::Jacket,
            GarmentSlot::Accessory,
        ] {
            let result = wardrobe_params_for_custom_garments(
                &params,
                &[garment(slot, json!({"occludesUnderwear": true}))],
            );
            assert_eq!(result["underwear"], true);
        }
    }

    #[test]
    fn covered_upper_tuck_is_bottom_only_and_prevalidates_its_dependencies() {
        let mut top = garment(GarmentSlot::Top, json!({}));
        top.id = "top".into();
        let mut top_part = native_part("custom:top", vec![]);
        top_part.material = json!({"fitClearanceUpperBandCoveredTuckMm": 20.0});
        let error = covered_upper_tuck_request_for_garment(&top, &top_part).unwrap_err();
        assert!(error.contains("only on a Bottom"));

        let mut bottom = garment(GarmentSlot::Bottom, json!({}));
        bottom.id = "bottom".into();
        let mut bottom_part = native_part("custom:bottom", vec![]);
        bottom_part.material = json!({"fitClearanceUpperBandCoveredTuckMm": 20.0});
        let error = covered_upper_tuck_request_for_garment(&bottom, &bottom_part).unwrap_err();
        assert!(error.contains("fitClearanceUpperBandCoveredOnly"));

        bottom_part.material = json!({
            "fitClearanceMm": 10.0,
            "fitClearanceUpperBandMm": [0.6, 0.8, 5.0],
            "fitClearanceUpperBandCoveredOnly": true,
            "fitClearanceUpperBandCoveredTuckMm": 20.0,
        });
        assert!(covered_upper_tuck_request_for_garment(&bottom, &bottom_part).unwrap());

        top_part.material = bottom_part.material.clone();
        top_part.material["fitClearanceUpperBandPhysicalCoverage"] = json!(true);
        let error = covered_upper_tuck_request_for_garment(&top, &top_part).unwrap_err();
        assert!(error.contains("only on a Bottom"));
        bottom_part.material["fitClearanceUpperBandPhysicalCoverage"] = json!(true);
        assert!(covered_upper_tuck_request_for_garment(&bottom, &bottom_part).unwrap());
    }

    #[test]
    fn custom_garment_occlusion_uses_only_opted_outer_layers() {
        assert!(
            custom_garment_layer(GarmentSlot::Feet) < custom_garment_layer(GarmentSlot::Bottom)
        );
        assert!(custom_garment_layer(GarmentSlot::Bottom) < custom_garment_layer(GarmentSlot::Top));
        assert!(custom_garment_layer(GarmentSlot::Top) < custom_garment_layer(GarmentSlot::Jacket));
        assert!(garment_occludes_lower_layers(&garment(
            GarmentSlot::Top,
            json!({"fitOccludesGarments": true}),
        )));
        assert!(garment_occludes_lower_layers(&garment(
            GarmentSlot::Jacket,
            json!({"fitOccludesGarments": true}),
        )));
        assert!(garment_occludes_lower_layers(&garment(
            GarmentSlot::Bottom,
            json!({"fitOccludesGarments": true}),
        )));
        assert!(!garment_occludes_lower_layers(&garment(
            GarmentSlot::Top,
            json!({})
        )));
    }

    #[test]
    fn covered_upper_band_uses_final_runtime_top_coverage_but_not_a_cropped_top() {
        let coords = vec![[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]];
        let face_verts = vec![0, 1, 2, 3];
        let mut top = garment(
            GarmentSlot::Top,
            json!({"fitOccludesGarments": true, "fitBodyMaskCandidates": [0]}),
        );
        top.id = "top".into();
        let full_top = NativePart {
            name: "custom:top".into(),
            positions: vec![[0., 0., 0.1], [1., 0., 0.1], [1., 1., 0.1], [0., 1., 0.1]],
            indices: vec![0, 1, 2, 0, 2, 3],
            uvs: vec![],
            refs: vec![],
            ref_weights: vec![],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material: top.material.clone(),
        };
        let lower = NativePart {
            name: "custom:pants".into(),
            positions: vec![[0.; 3]],
            indices: vec![],
            uvs: vec![],
            refs: vec![[0, 1, 2]],
            ref_weights: vec![[1. / 3.; 3]],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material: json!({}),
        };
        let covered = higher_opaque_covered_vertices(
            custom_garment_layer(GarmentSlot::Bottom).unwrap(),
            &[lower.clone(), full_top.clone()],
            &[top.clone()],
            &coords,
            &face_verts,
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(covered_anchor_mask(&lower, &covered).unwrap(), vec![true]);

        let mut cropped = full_top;
        cropped.indices = vec![0, 1, 2];
        let covered = higher_opaque_covered_vertices(
            custom_garment_layer(GarmentSlot::Bottom).unwrap(),
            &[lower.clone(), cropped],
            &[top],
            &coords,
            &face_verts,
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(covered_anchor_mask(&lower, &covered).unwrap(), vec![false]);
    }

    #[test]
    fn opted_bottom_culls_covered_feet_but_preserves_source_holes_and_default_feet() {
        let mut bottom = garment(GarmentSlot::Bottom, json!({"fitOccludesGarments": true}));
        bottom.id = "bottom".into();
        let mut feet = garment(GarmentSlot::Feet, json!({}));
        feet.id = "feet".into();
        let outer = NativePart {
            name: "custom:bottom".into(),
            positions: vec![
                [-1., -1., 0.03],
                [1., -1., 0.03],
                [1., 1., 0.03],
                [-1., 1., 0.03],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            uvs: vec![],
            refs: vec![],
            ref_weights: vec![],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material: json!({}),
        };
        let feet_part = NativePart {
            name: "custom:feet".into(),
            positions: vec![[-0.5, -0.5, 0.], [0.5, -0.5, 0.], [0., 0.5, 0.]],
            indices: vec![0, 1, 2],
            uvs: vec![],
            refs: vec![],
            ref_weights: vec![],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material: json!({}),
        };
        let snapshots = snapshot_outer_parts(
            &[outer.clone(), feet_part.clone()],
            &[bottom.clone(), feet.clone()],
        )
        .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].1,
            custom_garment_layer(GarmentSlot::Bottom).unwrap()
        );
        let mut covered_feet = feet_part.clone();
        assert_eq!(
            crate::native_occlusion::cull_fully_occluded_triangles(
                &snapshots[0].0,
                &mut covered_feet
            )
            .unwrap(),
            1
        );
        assert!(covered_feet.indices.is_empty());

        // A source opening has no covering outer triangle, so the boot collar stays visible.
        let mut through_hole = feet_part.clone();
        let mut open_bottom = outer.clone();
        open_bottom.positions[1][0] = -0.1;
        open_bottom.positions[2][0] = -0.1;
        assert_eq!(
            crate::native_occlusion::cull_fully_occluded_triangles(&open_bottom, &mut through_hole)
                .unwrap(),
            0
        );
        assert_eq!(through_hole.indices, vec![0, 1, 2]);

        bottom.material = json!({});
        assert!(
            snapshot_outer_parts(&[outer, feet_part], &[bottom, feet])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn outer_layer_snapshots_survive_an_earlier_layer_cull() {
        let mut top = garment(GarmentSlot::Top, json!({"fitOccludesGarments": true}));
        top.id = "top".into();
        let mut jacket = garment(GarmentSlot::Jacket, json!({"fitOccludesGarments": true}));
        jacket.id = "jacket".into();
        let mut parts = vec![
            NativePart {
                name: "custom:top".into(),
                positions: vec![],
                indices: vec![0, 1, 2],
                uvs: vec![],
                refs: vec![],
                ref_weights: vec![],
                rigid_joint: None,
                skin_weight_source_topology: None,
                material: json!({}),
            },
            NativePart {
                name: "custom:jacket".into(),
                positions: vec![],
                indices: vec![3, 4, 5],
                uvs: vec![],
                refs: vec![],
                ref_weights: vec![],
                rigid_joint: None,
                skin_weight_source_topology: None,
                material: json!({}),
            },
        ];
        let snapshots = snapshot_outer_parts(&parts, &[top, jacket]).unwrap();
        // Simulate jacket culling the top before it is evaluated against pants.
        parts[0].indices.clear();
        assert_eq!(snapshots[0].0.indices, vec![0, 1, 2]);
        assert_eq!(snapshots[1].0.indices, vec![3, 4, 5]);
    }

    #[test]
    fn fully_culled_custom_part_is_absent_from_assembled_character() {
        let mut character = NativeCharacter {
            parts: vec![
                native_part("Body", vec![]),
                native_part("custom:fully-occluded", vec![]),
                native_part("custom:visible", vec![0, 1, 2]),
                native_part("Hair", vec![]),
            ],
            height: 1.,
            rig: None,
            face_names: vec![],
            face_deltas: vec![],
            recipe: AraRecipe::default(),
            hair_fit: Default::default(),
        };

        remove_fully_culled_custom_parts(&mut character.parts);

        assert_eq!(
            character
                .parts
                .iter()
                .map(|part| part.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Body", "custom:visible", "Hair"]
        );
    }

    #[test]
    fn custom_feet_ground_by_slot_without_footwear_in_their_name() {
        let feet = garment(GarmentSlot::Feet, json!({}));
        assert!(is_grounding_footwear("custom:test", &[feet]));
        assert!(!is_grounding_footwear("custom:missing", &[]));
    }

    #[test]
    fn custom_non_feet_do_not_ground_even_with_footwear_in_their_id() {
        let mut hat = garment(GarmentSlot::Hat, json!({}));
        hat.id = "boot-hat".into();
        assert!(!is_grounding_footwear("custom:boot-hat", &[hat]));
    }

    #[test]
    fn stock_footwear_keeps_grounding_without_custom_assets() {
        for name in ["sneakers_0", "Boots_1", "sandals_2"] {
            assert!(is_grounding_footwear(name, &[]));
        }
        assert!(!is_grounding_footwear("Hair", &[]));
    }
}
