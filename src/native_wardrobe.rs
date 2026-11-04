//! Native runtime for the offline-baked Ara procedural wardrobe.
//!
//! The baker records each procedural mesh in its neutral body pose plus three
//! body references per vertex.  Reconstructing from those references keeps
//! garments attached to every MakeHuman morph without invoking Node or Ara.
use crate::{native_assets::NativeLibrary, native_character::NativePart};
use bevy::prelude::{Mat3, Vec3};
use flate2::read::GzDecoder;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Default, Debug)]
pub struct Wardrobe {
    pub parts: Vec<NativePart>,
    /// Occluded quad indices in the original body topology, with an intact skin rim.
    pub hidden_faces: Vec<u32>,
    pub hides_hair: bool,
}

#[derive(Deserialize)]
struct Bake {
    parts: std::collections::BTreeMap<String, Vec<Template>>,
    #[serde(default)]
    #[serde(rename = "sets")]
    _sets: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    hidden_faces: std::collections::BTreeMap<String, Vec<u32>>,
}
#[derive(Clone, Deserialize)]
struct Template {
    name: String,
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    uvs: Vec<[f32; 2]>,
    refs: Vec<[u32; 3]>,
    weights: Vec<[f32; 3]>,
    #[serde(default)]
    rigid_joint: Option<String>,
    material: Value,
}

/// Maps an authored neutral attachment frame into its morphed counterpart.
/// Rigid props must use one transform for the whole mesh: moving each vertex
/// through its nearest body references bends a weapon or bag before skinning.
struct RigidTransform {
    neutral_head: Vec3,
    current_head: Vec3,
    neutral_axes: Mat3,
    current_axes: Mat3,
}
impl RigidTransform {
    fn point(&self, point: [f32; 3]) -> [f32; 3] {
        (self.current_head
            + self.current_axes
                * self.neutral_axes.transpose()
                * (Vec3::from(point) - self.neutral_head))
            .to_array()
    }
}
fn joint(meta: &Value, coords: &[[f32; 3]], name: &str) -> Option<Vec3> {
    let indices = meta.get("joints")?.get(name)?.as_array()?;
    if indices.is_empty() {
        return None;
    }
    let mut sum = Vec3::ZERO;
    for index in indices {
        sum += Vec3::from(*coords.get(index.as_u64()? as usize)?);
    }
    Some(sum / indices.len() as f32)
}
fn weapon_frame(meta: &Value, coords: &[[f32; 3]], side: &str) -> Option<(Vec3, Mat3)> {
    let wrist = joint(meta, coords, &format!("wrist.{side}____head"))?;
    let index = joint(meta, coords, &format!("finger2-1.{side}____head"))?;
    let middle = joint(meta, coords, &format!("finger3-1.{side}____head"))?;
    let pinky = joint(meta, coords, &format!("finger5-1.{side}____head"))?;
    let thumb = joint(meta, coords, &format!("finger1-3.{side}____tail"))?;
    let mut palm = (index - wrist).cross(pinky - wrist).normalize_or_zero();
    if (thumb - wrist).dot(palm) < 0. {
        palm = -palm;
    }
    let y = (index - pinky).normalize_or_zero();
    let forward = (middle - wrist).normalize_or_zero();
    let z = (forward - y * forward.dot(y)).normalize_or_zero();
    let x = y.cross(z);
    let mid = (index + pinky) * 0.5;
    Some((mid + palm * 0.24 - forward * 0.12, Mat3::from_cols(x, y, z)))
}
fn rigid_transform(
    meta: &Value,
    neutral: &[[f32; 3]],
    coords: &[[f32; 3]],
    joint_name: Option<&str>,
) -> Option<RigidTransform> {
    let (neutral_head, neutral_axes, current_head, current_axes) = match joint_name? {
        "WeaponR" => {
            let (nh, na) = weapon_frame(meta, neutral, "R")?;
            let (ch, ca) = weapon_frame(meta, coords, "R")?;
            (nh, na, ch, ca)
        }
        "LeftHand" => {
            let (nh, na) = weapon_frame(meta, neutral, "L")?;
            let (ch, ca) = weapon_frame(meta, coords, "L")?;
            (nh, na, ch, ca)
        }
        "Spine2" => {
            let nh = joint(meta, neutral, "spine02")?;
            let ch = joint(meta, coords, "spine02")?;
            (nh, Mat3::IDENTITY, ch, Mat3::IDENTITY)
        }
        _ => return None,
    };
    Some(RigidTransform {
        neutral_head,
        current_head,
        neutral_axes,
        current_axes,
    })
}

fn baked(lib: &NativeLibrary) -> Result<Arc<Bake>, String> {
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Bake>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(found) = cache
        .lock()
        .map_err(|_| "wardrobe cache poisoned")?
        .get(&lib.root)
        .cloned()
    {
        return Ok(found);
    }
    let bytes = fs::read(lib.root.join("wardrobe.json.gz"))
        .map_err(|e| format!("wardrobe.json.gz: {e}"))?;
    let mut gz = GzDecoder::new(&bytes[..]);
    let mut text = Vec::new();
    gz.by_ref()
        .take(128 * 1024 * 1024 + 1)
        .read_to_end(&mut text)
        .map_err(|e| format!("wardrobe gzip: {e}"))?;
    if text.len() > 128 * 1024 * 1024 {
        return Err("wardrobe exceeds decompression limit".into());
    }
    let parsed: Arc<Bake> =
        Arc::new(serde_json::from_slice(&text).map_err(|e| format!("wardrobe JSON: {e}"))?);
    let mut cache = cache.lock().map_err(|_| "wardrobe cache poisoned")?;
    // Bound retained template libraries when a host swaps asset roots.
    if cache.len() >= 2 && !cache.contains_key(&lib.root) {
        if let Some(key) = cache.keys().next().cloned() {
            cache.remove(&key);
        }
    }
    cache.insert(lib.root.clone(), parsed.clone());
    Ok(parsed)
}
fn s<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty() && *x != "none")
}
fn strings<'a>(p: &'a Value, key: &str) -> impl Iterator<Item = &'a str> {
    p.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|x| !x.is_empty() && *x != "none")
}
fn style(p: &Value, id: &str, mut material: Value) -> Value {
    let schema = crate::ara::native_catalog();
    let defaults = schema["garments"]
        .as_array()
        .and_then(|a| a.iter().find(|g| g["id"] == id));
    let mut overrides = serde_json::Map::new();
    if let Some(def) = defaults {
        if def["patterns"].as_array().is_some_and(|a| !a.is_empty()) {
            for key in ["color", "color2"] {
                if def[key].is_string() {
                    overrides.insert(key.into(), def[key].clone());
                }
            }
            overrides.insert("pattern".into(), def["patterns"][0].clone());
        }
    }
    if let Some(user) = p["garmentStyles"][id].as_object() {
        for (k, v) in user {
            overrides.insert(k.clone(), v.clone());
        }
    }
    let owned = Value::Object(overrides);
    let o = &owned;
    let Some(m) = material.as_object_mut() else {
        return material;
    };
    if let Some(c) = o.get("color").and_then(Value::as_str) {
        m.insert("color".into(), json!(c));
    }
    if let Some(c) = o.get("color2").and_then(Value::as_str) {
        m.insert("color2".into(), json!(c));
    }
    // The renderer consumes this explicit selector to pick a baked reusable
    // fabric mask; it is never merely a saved UI value.
    if let Some(pattern) = o.get("pattern").and_then(Value::as_str) {
        m.insert("pattern".into(), json!(pattern));
        m.insert(
            "map".into(),
            json!(format!("textures/wardrobe_pattern_{pattern}.png")),
        );
        m.insert(
            "normalMap".into(),
            json!(format!("textures/wardrobe_pattern_{pattern}_n.png")),
        );
    }
    material
}

/// Smooth-enough body normals for moving the tiny clearance on fitted cloth.
/// The garment bake's direct references identify vertices copied from the body
/// surface; their authored displacement is chiefly an outward normal offset.
fn body_normals(coords: &[[f32; 3]], faces: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; coords.len()];
    for face in faces.chunks_exact(4) {
        let Some((&a, &b, &c, &d)) = face
            .first()
            .zip(face.get(1))
            .zip(face.get(2))
            .zip(face.get(3))
            .map(|(((a, b), c), d)| (a, b, c, d))
        else {
            continue;
        };
        let (Some(a), Some(b), Some(c), Some(d)) = (
            coords.get(a as usize),
            coords.get(b as usize),
            coords.get(c as usize),
            coords.get(d as usize),
        ) else {
            continue;
        };
        // Both triangles contribute so warped quads retain the local surface
        // orientation rather than borrowing one arbitrary diagonal.
        let a = Vec3::from(*a);
        let b = Vec3::from(*b);
        let c = Vec3::from(*c);
        let d = Vec3::from(*d);
        let n = (b - a).cross(c - a) + (c - a).cross(d - a);
        for &vertex in face {
            if let Some(total) = normals.get_mut(vertex as usize) {
                *total += n;
            }
        }
    }
    normals.into_iter().map(Vec3::normalize_or_zero).collect()
}

fn is_shell_vertex(t: &Template, id: &str, vertex: usize) -> bool {
    // This correction is for the body-derived briefs shell. Proxy-based
    // clothing also has direct refs, but its offsets encode authored fitting
    // geometry and must retain their existing deformation behavior.
    if id != "briefs" {
        return false;
    }
    let Some([reference, duplicate_a, duplicate_b]) = t.refs.get(vertex).copied() else {
        return false;
    };
    let exact_ref = reference == duplicate_a
        && reference == duplicate_b
        && t.weights.get(vertex).copied() == Some([1., 0., 0.]);
    exact_ref
}
fn instantiate(
    t: Template,
    coords: &[[f32; 3]],
    neutral: &[[f32; 3]],
    current_normals: &[Vec3],
    params: &Value,
    id: &str,
    rigid_transform: Option<&RigidTransform>,
) -> NativePart {
    // `buildShell` emits each body reference first for the outer brief surface,
    // then again for its hem/inner ring.  Rebuild these two surfaces directly
    // from the morphed body instead of carrying the baked boundary smoothing.
    let mut seen_brief_refs = BTreeSet::new();
    let positions = t
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if let Some(transform) = rigid_transform {
                return transform.point(*p);
            }
            let r = t.refs.get(i).copied().unwrap_or([0; 3]);
            let w = t.weights.get(i).copied().unwrap_or([1., 0., 0.]);
            let mut now = [0.; 3];
            let mut old = [0.; 3];
            for k in 0..3 {
                if let (Some(a), Some(b)) = (coords.get(r[k] as usize), neutral.get(r[k] as usize))
                {
                    for d in 0..3 {
                        now[d] += a[d] * w[k];
                        old[d] += b[d] * w[k];
                    }
                }
            }
            let reference = r[0] as usize;
            // Rebuild the body-bound brief shell directly on the morphed body.
            // Its neutral boundary smoothing can point through wide female
            // hips, whereas the current surface normal supplies a stable fit.
            if is_shell_vertex(&t, id, i) {
                let current_normal = current_normals
                    .get(reference)
                    .copied()
                    .unwrap_or(Vec3::ZERO);
                let clearance = if seen_brief_refs.insert(r[0]) {
                    0.03
                } else {
                    0.0045
                };
                return (Vec3::from(now) + current_normal * clearance).to_array();
            }
            [
                now[0] + p[0] - old[0],
                now[1] + p[1] - old[1],
                now[2] + p[2] - old[2],
            ]
        })
        .collect();
    let mut material = t.material;
    if id.starts_with("hair:") {
        material["color"] = params["hairColor"].clone();
        let texture = if id.ends_with("buzz") {
            "hair_card"
        } else if id.ends_with("mohawk") {
            "hair_spike"
        } else {
            "hair_braid"
        };
        material["map"] = json!(format!("textures/look_{texture}.png"));
        material["alpha"] = json!("mask");
    }
    if id.starts_with("facialHair:") {
        material["color"] = params["beardColor"].clone();
        material["map"] = json!("textures/look_hair_card.png");
        material["alpha"] = json!("mask");
        material["cartoonBeard"] = json!(true);
    }
    if id.starts_with("earrings:") {
        material["color"] = json!(if params["earringMetal"] == "silver" {
            "#d9dbe0"
        } else {
            "#e0b45a"
        });
    }
    NativePart {
        name: t.name,
        positions,
        indices: t.indices,
        uvs: t.uvs,
        refs: t.refs,
        ref_weights: t.weights,
        rigid_joint: t.rigid_joint,
        material: style(params, id, material),
    }
}

/// Clearance (decimetres) an outer clothing layer keeps above the layer beneath.
const LAYER_GAP: f32 = 0.02;

/// Keeps tops outside bottoms and jackets outside both. Each garment carries its
/// authored offset from the neutral body unchanged, so where a morph widens the body
/// a lot (cartoon hips, heavy weight) an inner layer can end up outside the outer one.
/// Height above the body is measured along the body normal at each vertex's first
/// reference; an outer vertex is lifted to clear the highest inner vertex on the same
/// or a neighbouring body vertex.
fn separate_layers(
    parts: &mut [NativePart],
    layers: &[u8],
    coords: &[[f32; 3]],
    normals: &[Vec3],
    faces: &[u32],
) {
    let mut neighbours = vec![Vec::new(); coords.len()];
    for face in faces.chunks_exact(4) {
        for k in 0..4 {
            let (a, b) = (face[k] as usize, face[(k + 1) % 4] as usize);
            neighbours[a].push(b);
            neighbours[b].push(a);
        }
    }
    let height = |part: &NativePart, i: usize| -> Option<(usize, Vec3, f32)> {
        let r = part.refs.get(i)?;
        let w = part.ref_weights.get(i).copied().unwrap_or([1., 0., 0.]);
        let (mut base, mut normal) = (Vec3::ZERO, Vec3::ZERO);
        for k in 0..3 {
            base += Vec3::from(*coords.get(r[k] as usize)?) * w[k];
            normal += *normals.get(r[k] as usize)? * w[k];
        }
        let normal = normal.try_normalize()?;
        Some((r[0] as usize, normal, (Vec3::from(part.positions[i]) - base).dot(normal)))
    };
    for outer in 2..=3u8 {
        let mut inner = vec![f32::NEG_INFINITY; coords.len()];
        for (part, _) in parts.iter().zip(layers).filter(|(_, l)| (1..outer).contains(*l)) {
            for i in 0..part.positions.len() {
                if let Some((v, _, h)) = height(part, i) {
                    inner[v] = inner[v].max(h);
                }
            }
        }
        for _ in 0..2 {
            let grown: Vec<f32> = (0..coords.len())
                .map(|v| neighbours[v].iter().fold(inner[v], |m, &n| m.max(inner[n])))
                .collect();
            inner = grown;
        }
        for (part, _) in parts.iter_mut().zip(layers).filter(|(_, l)| **l == outer) {
            for i in 0..part.positions.len() {
                if let Some((v, n, h)) = height(part, i) {
                    let need = inner[v] + LAYER_GAP;
                    if need.is_finite() && h < need {
                        part.positions[i] = (Vec3::from(part.positions[i]) + n * (need - h)).to_array();
                    }
                }
            }
        }
    }
}

/// Tattoos are independent, body-bound surface meshes.  The original uses a
/// texture painter; retaining the selected skin faces here makes the overlay
/// follow every morph and lets the host shade it with the requested ink.
pub(crate) fn tattoo_part(
    lib: &NativeLibrary,
    coords: &[[f32; 3]],
    id: &str,
    ink: &str,
) -> Result<Option<NativePart>, String> {
    let masks: Value = serde_json::from_slice(
        &fs::read(lib.root.join("look_masks.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let Some(selected) = masks[id].as_array() else {
        return Ok(None);
    };
    let faces = lib
        .base
        .arrays
        .get("faceVerts")
        .ok_or("missing faces")?
        .u32()?;
    let face_uv = lib
        .base
        .arrays
        .get("faceUVs")
        .ok_or("missing UV faces")?
        .u32()?;
    let uvs = lib.base.arrays.get("uvs").ok_or("missing UVs")?.f32()?;
    let mut normals = vec![bevy::prelude::Vec3::ZERO; coords.len()];
    for q in faces.chunks_exact(4) {
        let a = bevy::prelude::Vec3::from(coords[q[0] as usize]);
        let b = bevy::prelude::Vec3::from(coords[q[1] as usize]);
        let c = bevy::prelude::Vec3::from(coords[q[2] as usize]);
        let n = (b - a).cross(c - a);
        for v in q {
            normals[*v as usize] += n;
        }
    }
    let mut part = NativePart {
        name: format!("tattoo_{id}"),
        positions: vec![],
        indices: vec![],
        uvs: vec![],
        refs: vec![],
        ref_weights: vec![],
        rigid_joint: None,
        material: json!({"name":id,"color":ink,"roughness":0.75,"alpha":"blend","doubleSided":false,"map":format!("textures/look_{id}.png")}),
    };
    for f in selected {
        let f = f.as_u64().ok_or("invalid look mask face")? as usize;
        let q = faces
            .get(f * 4..f * 4 + 4)
            .ok_or("look face out of bounds")?;
        let start = part.positions.len() as u32;
        for k in 0..4 {
            let v = q[k] as usize;
            let p = bevy::prelude::Vec3::from(coords[v]) + normals[v].normalize_or_zero() * 0.004;
            part.positions.push(p.to_array());
            let uv = face_uv[f * 4 + k] as usize;
            part.uvs.push([uvs[uv * 2], 1. - uvs[uv * 2 + 1]]);
            part.refs.push([v as u32; 3]);
            part.ref_weights.push([1., 0., 0.]);
        }
        part.indices
            .extend([start, start + 1, start + 2, start, start + 2, start + 3]);
    }
    Ok(Some(part))
}

/// Compact native bicycle for the riding selection.  Its frame is deliberately
/// separate from body-bound wardrobe data: it is a scene prop, not clothing.
fn bicycle(coords: &[[f32; 3]]) -> NativePart {
    use bevy::prelude::Vec3;
    let floor = coords.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let top = coords
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max);
    let scale = (top - floor) / 17.5;
    let mut part = NativePart {
        name: "Bicycle".into(),
        positions: vec![],
        indices: vec![],
        uvs: vec![],
        refs: vec![],
        ref_weights: vec![],
        rigid_joint: None,
        material: json!({"name":"Bicycle","color":"#242b35","roughness":0.5,"metalness":0.65,"doubleSided":true}),
    };
    for center_z in [-4.6, 5.1] {
        let start = part.positions.len() as u32;
        for i in 0..32 {
            let a = i as f32 * std::f32::consts::TAU / 32.;
            for j in 0..8 {
                let b = j as f32 * std::f32::consts::TAU / 8.;
                let r = 3.3 + 0.18 * b.cos();
                part.positions.push([
                    0.18 * b.sin() * scale,
                    floor + (3.5 + r * a.cos()) * scale,
                    (center_z + r * a.sin()) * scale,
                ]);
            }
        }
        for i in 0..32 {
            for j in 0..8 {
                let a = start + i * 8 + j;
                let b = start + ((i + 1) % 32) * 8 + j;
                let c = start + ((i + 1) % 32) * 8 + (j + 1) % 8;
                let d = start + i * 8 + (j + 1) % 8;
                part.indices.extend([a, b, c, a, c, d]);
            }
        }
    }
    let point = |y: f32, z: f32| Vec3::new(0., floor + y * scale, z * scale);
    let rear = point(3.5, -4.6);
    let front = point(3.5, 5.1);
    let crank = point(3.3, 0.);
    let seat = point(8.4, -1.);
    let stem = point(8., 3.7);
    let mut tube = |a: Vec3, b: Vec3, r: f32| {
        let axis = (b - a).normalize_or_zero();
        let x = axis.any_orthonormal_vector();
        let z = axis.cross(x);
        let start = part.positions.len() as u32;
        for p in [a, b] {
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::TAU / 8.;
                part.positions
                    .push((p + (x * angle.cos() + z * angle.sin()) * r * scale).to_array());
            }
        }
        for i in 0..8 {
            let n = (i + 1) % 8;
            part.indices.extend([
                start + i,
                start + n,
                start + 8 + n,
                start + i,
                start + 8 + n,
                start + 8 + i,
            ]);
        }
    };
    for (a, b) in [
        (rear, crank),
        (rear, seat),
        (crank, seat),
        (seat, stem),
        (crank, stem),
        (stem, front),
    ] {
        tube(a, b, 0.10);
    }
    tube(
        seat - Vec3::Z * 0.6 * scale,
        seat + Vec3::Z * 0.8 * scale,
        0.35,
    );
    tube(
        stem + Vec3::Y * 0.4 * scale - Vec3::X * 2.2 * scale,
        stem + Vec3::Y * 0.4 * scale + Vec3::X * 2.2 * scale,
        0.12,
    );
    tube(
        crank - Vec3::X * 1.2 * scale,
        crank + Vec3::X * 1.2 * scale,
        0.1,
    );
    let n = part.positions.len();
    part.uvs = vec![[0., 0.]; n];
    part.refs = vec![[0; 3]; n];
    part.ref_weights = vec![[1., 0., 0.]; n];
    part
}

pub fn build_wardrobe(
    lib: &NativeLibrary,
    coords: &[[f32; 3]],
    params: &Value,
) -> Result<Wardrobe, String> {
    let bake = baked(lib)?;
    let raw = lib
        .base
        .arrays
        .get("positions")
        .ok_or("base positions missing")?
        .f32()?;
    if raw.len() % 3 != 0 {
        return Err("base positions malformed".into());
    }
    let neutral: Vec<[f32; 3]> = raw.chunks_exact(3).map(|x| [x[0], x[1], x[2]]).collect();
    if coords.len() != neutral.len() {
        return Err(format!(
            "wardrobe body has {} vertices; expected {}",
            coords.len(),
            neutral.len()
        ));
    }
    let faces = lib
        .base
        .arrays
        .get("faceVerts")
        .ok_or("base faces missing")?
        .u32()?;
    let current_normals = body_normals(coords, faces);
    let schema = crate::ara::native_catalog();
    let mut effective = params.clone();
    let set = schema["outfit_sets"]
        .as_array()
        .and_then(|a| a.iter().find(|v| v["id"] == params["outfit"]));
    let mut chosen: BTreeSet<String> = BTreeSet::new();
    if let Some(set) = set {
        for key in ["top", "jacket", "bottom"] {
            if let Some(id) = set[key].as_str().filter(|s| *s != "none") {
                chosen.insert(id.into());
            }
        }
        if let Some(style) = params["garmentStyles"][set["id"].as_str().unwrap()].as_object() {
            for key in [
                if set["jacket"].is_string() {
                    "jacket"
                } else {
                    "top"
                },
                "bottom",
            ] {
                if let Some(id) = set[key].as_str() {
                    let mut overrides = Value::Object(style.clone());
                    if let Some(local) = params["garmentStyles"][id].as_object() {
                        for (k, v) in local {
                            overrides[k] = v.clone();
                        }
                    }
                    effective["garmentStyles"][id] = overrides;
                }
            }
        }
    } else if s(params, "outfit").is_none() {
        for key in ["top", "bottom", "jacket"] {
            if let Some(x) = s(params, key) {
                chosen.insert(x.into());
            }
        }
    }
    for key in ["shoes", "hat"] {
        if let Some(x) = s(params, key) {
            chosen.insert(if x == "combat_boots" {
                "boots".into()
            } else {
                x.into()
            });
        }
    }
    chosen.extend(strings(params, "accessories").map(str::to_owned));
    let years = if params["age"].as_f64().unwrap_or(0.5) < 0.5 {
        11. + (params["age"].as_f64().unwrap_or(0.5) - 0.1875) / 0.3125 * 14.
    } else {
        25. + (params["age"].as_f64().unwrap_or(0.5) - 0.5) * 130.
    };
    if params["underwear"] == true || years < 18. {
        chosen.insert("briefs".into());
    }
    if years < 18.
        && params["gender"].as_f64().unwrap_or(1.) < 0.5
        && s(params, "outfit").is_none()
        && s(params, "top").is_none()
    {
        chosen.insert("tank_top".into());
    }
    if let Some(x) = s(params, "hair") {
        chosen.insert(format!("hair:{x}"));
    }
    if let Some(x) = s(params, "facialHair") {
        chosen.insert(format!("facialHair:{x}"));
    }
    if let Some(x) = s(params, "earrings") {
        chosen.insert(format!("earrings:{x}"));
    }
    if let Some(x) = s(params, "weapon") {
        chosen.insert(format!("weapon:{x}"));
    }
    if let Some(x) = s(params, "carry") {
        // Ara's backpack is a worn garment, not a prop held in the left hand.
        // `buildCarry("backpack")` is intentionally empty.
        chosen.insert(if x == "backpack" {
            "backpack".into()
        } else {
            format!("carry:{x}")
        });
    }
    let mut out = Wardrobe::default();
    let mut hidden = BTreeSet::new();
    let slots: BTreeMap<&str, &str> = schema["garments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| Some((g["id"].as_str()?, g["slot"].as_str()?)))
        .collect();
    // Clothing layer of each part, for `separate_layers`.
    let mut layers = vec![];
    for id in chosen {
        if let Some(parts) = bake.parts.get(&id) {
            let layer = match slots.get(id.as_str()).copied() {
                Some("bottom") => 1,
                Some("top") => 2,
                Some("jacket") => 3,
                _ => 0,
            };
            layers.extend(std::iter::repeat_n(layer, parts.len()));
            out.parts.extend(parts.iter().cloned().map(|t| {
                let transform =
                    rigid_transform(&lib.base.meta, &neutral, coords, t.rigid_joint.as_deref());
                instantiate(
                    t,
                    coords,
                    &neutral,
                    &current_normals,
                    &effective,
                    &id,
                    transform.as_ref(),
                )
            }));
        }
        if let Some(v) = bake.hidden_faces.get(&id) {
            hidden.extend(v.iter().copied())
        }
        out.hides_hair |= matches!(
            id.as_str(),
            "keffiyeh" | "gele" | "helmet" | "beret" | "cowboy_hat" | "baseball_cap"
        );
    }
    separate_layers(&mut out.parts, &layers, coords, &current_normals, faces);
    if s(params, "facialHair").is_some_and(|s| s == "stubble" || s == "mustache_stubble") {
        if let Some(part) = tattoo_part(
            lib,
            coords,
            "stubble",
            s(params, "beardColor").unwrap_or("#241a13"),
        )? {
            out.parts.push(part);
        }
    }
    if s(params, "hair") == Some("buzz") {
        if let Some(part) = tattoo_part(
            lib,
            coords,
            "buzz_scalp",
            s(params, "hairColor").unwrap_or("#2a1e16"),
        )? {
            out.parts.push(part);
        }
    }
    let ink = s(params, "tattooInk").unwrap_or("#1c2128");
    for id in strings(params, "tattoos").filter(|_| years >= 18.) {
        if let Some(part) = tattoo_part(lib, coords, id, ink)? {
            out.parts.push(part)
        }
    }
    if s(params, "vehicle") == Some("bicycle") {
        out.parts.push(bicycle(coords));
    }
    // Keep a one-face rim of skin beneath garment edges. Garments are offset
    // from the body surface, so deleting skin right up to an open hem leaves
    // a visible slit between the two meshes, even with an exact coverage mask.
    let mut exposed_vertices = BTreeSet::new();
    for (index, face) in faces.chunks_exact(4).enumerate() {
        if !hidden.contains(&(index as u32)) {
            exposed_vertices.extend(face.iter().copied());
        }
    }
    hidden.retain(|&index| {
        faces
            .get(index as usize * 4..index as usize * 4 + 4)
            .is_some_and(|face| face.iter().all(|vertex| !exposed_vertices.contains(vertex)))
    });
    out.hidden_faces = hidden.into_iter().collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ara::AraRecipe, native_morph::apply_targets};

    #[test]
    fn briefs_stay_outside_the_female_body_at_their_direct_references() {
        let lib =
            NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap();
        let mut recipe = AraRecipe::default();
        recipe.params["gender"] = json!(0.0);
        for key in [
            "outfit", "top", "bottom", "jacket", "shoes", "hat", "carry", "weapon",
        ] {
            recipe.params[key] = json!("none");
        }
        recipe.params["accessories"] = json!([]);
        recipe.params["underwear"] = json!(true);
        let coords = apply_targets(&lib, &recipe.params).unwrap();
        let wardrobe = build_wardrobe(&lib, &coords, &recipe.params).unwrap();
        let briefs = wardrobe
            .parts
            .iter()
            .find(|part| part.name == "briefs_0")
            .expect("female wardrobe includes briefs");
        let normals = body_normals(&coords, lib.base.arrays["faceVerts"].u32().unwrap());
        let mut seen_references = BTreeSet::new();
        for (position, reference) in briefs.positions.iter().zip(&briefs.refs) {
            assert_eq!(reference[0], reference[1]);
            assert_eq!(reference[1], reference[2]);
            let body = Vec3::from(coords[reference[0] as usize]);
            let clearance = (Vec3::from(*position) - body).dot(normals[reference[0] as usize]);
            let required = if seen_references.insert(reference[0]) {
                0.03
            } else {
                0.0045
            };
            assert!(
                clearance >= required - 0.000_1,
                "brief vertex {} must remain outside its female body reference ({clearance}, expected {required})",
                reference[0]
            );
        }
    }
    #[test]
    fn proxy_garment_keeps_authored_fitting_displacements() {
        let lib =
            NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap();
        let mut recipe = AraRecipe::default();
        recipe.params["gender"] = json!(0.0);
        recipe.params["weight"] = json!(0.85);
        let coords = apply_targets(&lib, &recipe.params).unwrap();
        let neutral: Vec<[f32; 3]> = lib.base.arrays["positions"]
            .f32()
            .unwrap()
            .chunks_exact(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect();
        let faces = lib.base.arrays["faceVerts"].u32().unwrap();
        let normals = body_normals(&coords, faces);
        let templates = baked(&lib).unwrap();
        let mut direct_refs = 0;
        for template in &templates.parts["suit_jacket"] {
            let part = instantiate(
                template.clone(),
                &coords,
                &neutral,
                &normals,
                &recipe.params,
                "suit_jacket",
                None,
            );
            for (index, position) in part.positions.iter().enumerate() {
                let refs = template.refs[index];
                let weights = template.weights[index];
                direct_refs += usize::from(refs[0] == refs[1] && refs[1] == refs[2]);
                let mut now = Vec3::ZERO;
                let mut old = Vec3::ZERO;
                for k in 0..3 {
                    now += Vec3::from(coords[refs[k] as usize]) * weights[k];
                    old += Vec3::from(neutral[refs[k] as usize]) * weights[k];
                }
                let expected = now + Vec3::from(template.positions[index]) - old;
                assert!(
                    Vec3::from(*position).abs_diff_eq(expected, 1e-5),
                    "proxy fitting offset changed at {index}"
                );
            }
        }
        assert!(
            direct_refs > 100,
            "fixture must exercise proxy direct references"
        );
    }
}
