//! Portable, body-bound garments authored against the exported native body OBJ.
//!
//! `CustomGarment` deliberately stores only geometry and a small JSON material.
//! OBJ material names and texture files are not imported: callers choose the
//! material JSON (and native-library-relative texture paths) explicitly.
use crate::{
    native_assets::{Array, NativeLibrary},
    native_character::NativePart,
    native_fit::BodyDeformation,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

const MAX_VERTICES: usize = 100_000;
const MAX_INDICES: usize = 1_200_000;
/// Current MakeHuman base topology has this many vertices.  Keeping the bound
/// in the portable format prevents malicious or accidentally incompatible refs.
const MAX_BODY_VERTEX: u32 = 19_158;
const MAX_GARMENT_FILE: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GarmentSlot {
    Hair,
    Beard,
    Top,
    Bottom,
    Jacket,
    Feet,
    Hat,
    Accessory,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomGarment {
    pub id: String,
    pub name: String,
    pub slot: GarmentSlot,
    #[serde(rename = "positionsDM", alias = "positions_dm")]
    pub positions_dm: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub uvs: Vec<[f32; 2]>,
    #[serde(rename = "refs3", alias = "refs")]
    pub refs: Vec<[u32; 3]>,
    #[serde(rename = "weights3", alias = "weights")]
    pub weights: Vec<[f32; 3]>,
    #[serde(rename = "referencePointsDM", alias = "reference_points_dm")]
    pub reference_points_dm: Vec<[f32; 3]>,
    pub material: Value,
    #[serde(rename = "hiddenFaces", alias = "hidden_faces", default)]
    pub hidden_faces: Vec<u32>,
}

/// One entry discovered in `NativeAssets/custom_garments`.  Invalid entries
/// remain visible to editor hosts along with their load error, but cannot be
/// selected.
#[derive(Clone, Debug)]
pub struct CustomGarmentCatalogEntry {
    pub id: String,
    pub name: String,
    pub slot: Option<GarmentSlot>,
    pub path: PathBuf,
    pub error: Option<String>,
    garment: Option<Arc<CustomGarment>>,
}

impl CustomGarmentCatalogEntry {
    pub fn garment(&self) -> Option<&Arc<CustomGarment>> {
        self.garment.as_ref()
    }
}

/// Validated custom garments available to a native asset library.  Definitions
/// are parsed once at startup and shared by the UI; selecting an item is the
/// only point at which its geometry is copied into a saved recipe.
#[derive(Clone, Debug, Default)]
pub struct CustomGarmentCatalog {
    pub entries: Vec<CustomGarmentCatalogEntry>,
}

impl CustomGarmentCatalog {
    pub fn discover(native_root: impl AsRef<Path>) -> Self {
        Self::discover_with_body_face_count(native_root.as_ref(), None)
    }

    /// Discovers garments against a loaded native library so invalid body-face
    /// masks are reported before an editor can select the garment.
    pub fn discover_for_library(library: &NativeLibrary) -> Self {
        let body_face_count = array(library, "faceVerts")
            .ok()
            .and_then(|faces| faces.u32().ok())
            .map(|faces| faces.len() / 4);
        Self::discover_with_body_face_count(&library.root, body_face_count)
    }

    fn discover_with_body_face_count(native_root: &Path, body_face_count: Option<usize>) -> Self {
        let directory = native_root.join("custom_garments");
        let Ok(read_dir) = fs::read_dir(&directory) else {
            return Self::default();
        };
        let mut paths: Vec<_> = read_dir
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        paths.sort();

        let mut entries: Vec<CustomGarmentCatalogEntry> = Vec::with_capacity(paths.len());
        let mut ids = BTreeMap::new();
        for path in paths {
            let loaded = load_catalog_garment(&path, native_root, body_face_count);
            let mut entry = match loaded {
                Ok(garment) => CustomGarmentCatalogEntry {
                    id: garment.id.clone(),
                    name: garment.name.clone(),
                    slot: Some(garment.slot.clone()),
                    path: path.clone(),
                    error: None,
                    garment: Some(Arc::new(garment)),
                },
                Err(error) => CustomGarmentCatalogEntry {
                    id: String::new(),
                    name: path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Invalid garment")
                        .to_owned(),
                    slot: None,
                    path: path.clone(),
                    error: Some(error),
                    garment: None,
                },
            };
            if let Some(previous) = ids.insert(entry.id.clone(), entries.len()) {
                if !entry.id.is_empty() {
                    let error = format!("duplicate garment id {:?}", entry.id);
                    entries[previous].error = Some(error.clone());
                    entries[previous].garment = None;
                    entry.error = Some(error);
                    entry.garment = None;
                }
            }
            entries.push(entry);
        }
        Self { entries }
    }
}

fn load_catalog_garment(
    path: &Path,
    native_root: &Path,
    body_face_count: Option<usize>,
) -> Result<CustomGarment, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_GARMENT_FILE {
        return Err("garment definition exceeds 64 MiB".into());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_GARMENT_FILE {
        return Err("garment definition exceeds 64 MiB".into());
    }
    let garment: CustomGarment =
        serde_json::from_slice(&bytes).map_err(|error| format!("garment JSON: {error}"))?;
    garment.validate()?;
    if let Some(face_count) = body_face_count {
        if garment
            .hidden_faces
            .iter()
            .any(|&face| face as usize >= face_count)
        {
            return Err("garment has an invalid hidden body face".into());
        }
    }
    for texture in material_texture_paths(&garment.material) {
        if !native_texture_path(native_root, texture).is_some_and(|path| path.is_file()) {
            return Err(format!("missing garment texture {texture:?}"));
        }
    }
    Ok(garment)
}

fn material_texture_paths(material: &Value) -> Vec<&str> {
    material
        .as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .filter_map(|(key, value)| {
            matches!(key.as_str(), "map" | "diffuse" | "normal" | "normalMap")
                .then(|| value.as_str())
                .flatten()
        })
        .collect()
}

fn array<'a>(lib: &'a NativeLibrary, name: &str) -> Result<&'a Array, String> {
    lib.base
        .arrays
        .get(name)
        .ok_or_else(|| format!("base asset is missing {name}"))
}

fn base_positions(lib: &NativeLibrary) -> Result<Vec<[f32; 3]>, String> {
    let raw = array(lib, "positions")?.f32()?;
    if raw.len() % 3 != 0 {
        return Err("base positions are not xyz triples".into());
    }
    Ok(raw.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect())
}

fn safe_asset_path(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty()
        && !p.is_absolute()
        && !path.contains('\\')
        && !p.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

/// Resolves a material texture exactly as native preview and export do.
/// Native catalog textures are conventionally bare names under `textures/`,
/// while custom garments may also include one leading `textures/` prefix.
pub(crate) fn native_texture_path(native_root: &Path, texture: &str) -> Option<PathBuf> {
    let relative = texture.strip_prefix("textures/").unwrap_or(texture);
    safe_asset_path(relative).then(|| native_root.join("textures").join(relative))
}

fn validate_material(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("garment material must be an object")?;
    for (key, value) in object {
        if matches!(key.as_str(), "map" | "diffuse" | "normal" | "normalMap") {
            let path = value
                .as_str()
                .ok_or_else(|| format!("material {key} must be a path string"))?;
            if !safe_asset_path(path) {
                return Err(format!(
                    "material {key} is not a native-library-relative path"
                ));
            }
        }
        validate_json_finite(value)?;
    }
    Ok(())
}

fn validate_json_finite(value: &Value) -> Result<(), String> {
    match value {
        Value::Number(n) if n.as_f64().is_some_and(|v| !v.is_finite()) => {
            return Err("material contains a non-finite number".into());
        }
        Value::Array(values) => {
            for value in values {
                validate_json_finite(value)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_json_finite(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl CustomGarment {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        {
            return Err("garment id must be 1-64 ASCII letters, digits, _ or -".into());
        }
        if self.name.is_empty() || self.name.len() > 160 {
            return Err("garment name must be 1-160 bytes".into());
        }
        let n = self.positions_dm.len();
        if n == 0 || n > MAX_VERTICES {
            return Err("garment has an invalid vertex count".into());
        }
        if self.uvs.len() != n
            || self.refs.len() != n
            || self.weights.len() != n
            || self.reference_points_dm.len() != n
        {
            return Err("garment vertex attributes have unequal lengths".into());
        }
        if self.indices.is_empty()
            || self.indices.len() > MAX_INDICES
            || self.indices.len() % 3 != 0
        {
            return Err("garment indices must be non-empty triangles".into());
        }
        if self.indices.iter().any(|&i| i as usize >= n) {
            return Err("garment index is out of bounds".into());
        }
        for (i, ((position, uv), (refs, weights))) in self
            .positions_dm
            .iter()
            .zip(&self.uvs)
            .zip(self.refs.iter().zip(&self.weights))
            .enumerate()
        {
            if !position.iter().chain(uv).all(|v| v.is_finite())
                || !self.reference_points_dm[i].iter().all(|v| v.is_finite())
            {
                return Err(format!("garment vertex {i} is non-finite"));
            }
            if refs.iter().any(|&r| r >= MAX_BODY_VERTEX) {
                return Err(format!(
                    "garment vertex {i} has an unsupported body reference"
                ));
            }
            if !weights.iter().all(|v| v.is_finite() && *v >= 0.)
                || (weights[0] + weights[1] + weights[2] - 1.).abs() > 0.001
            {
                return Err(format!("garment vertex {i} has invalid reference weights"));
            }
        }
        validate_material(&self.material)
    }

    /// Fits a garment in MakeHuman decimetres.  The native character builder
    /// performs its usual floor-origin/metre conversion after assembling parts.
    pub fn fit(&self, coords_dm: &[[f32; 3]]) -> Result<NativePart, String> {
        self.validate()?;
        let mut positions = Vec::with_capacity(self.positions_dm.len());
        for i in 0..self.positions_dm.len() {
            let mut anchor = [0.; 3];
            for k in 0..3 {
                let point = coords_dm
                    .get(self.refs[i][k] as usize)
                    .ok_or("garment reference is outside this body")?;
                for axis in 0..3 {
                    anchor[axis] += point[axis] * self.weights[i][k];
                }
            }
            positions.push(std::array::from_fn(|axis| {
                anchor[axis] + self.positions_dm[i][axis] - self.reference_points_dm[i][axis]
            }));
        }
        Ok(NativePart {
            name: format!("custom:{}", self.id),
            positions,
            indices: self.indices.clone(),
            uvs: self.uvs.clone(),
            refs: self.refs.clone(),
            ref_weights: self.weights.clone(),
            rigid_joint: None,
            material: self.material.clone(),
        })
    }

    /// Fits through body anchors and transports the authored normal/tangent
    /// offset through the supplied current body deformation field.
    pub fn fit_adaptive(
        &self,
        coords_dm: &[[f32; 3]],
        field: &BodyDeformation,
    ) -> Result<NativePart, String> {
        self.validate()?;
        let mut part = self.fit(coords_dm)?;
        for index in 0..part.positions.len() {
            let offset = bevy::prelude::Vec3::from(self.positions_dm[index])
                - bevy::prelude::Vec3::from(self.reference_points_dm[index]);
            let adaptive = field.transform_offset(self.refs[index], self.weights[index], offset)?;
            let mut anchor = [0.; 3];
            for k in 0..3 {
                let point = coords_dm
                    .get(self.refs[index][k] as usize)
                    .ok_or("garment reference is outside this body")?;
                for axis in 0..3 {
                    anchor[axis] += point[axis] * self.weights[index][k];
                }
            }
            part.positions[index] = (bevy::prelude::Vec3::from(anchor) + adaptive).to_array();
        }
        Ok(part)
    }
}

fn parse_index(raw: &str, len: usize, what: &str) -> Result<usize, String> {
    let index: i64 = raw
        .parse()
        .map_err(|_| format!("OBJ has invalid {what} index"))?;
    let index = if index < 0 {
        len as i64 + index
    } else {
        index - 1
    };
    if index < 0 || index as usize >= len {
        return Err(format!("OBJ {what} index is out of bounds"));
    }
    Ok(index as usize)
}

/// Imports a convex-polygon OBJ.  It uses `v/vt` face corners, so UV seams are
/// preserved even when one OBJ position appears with several texture vertices.
pub fn import_obj(
    lib: &NativeLibrary,
    text: &str,
    id: &str,
    slot: &str,
    units: &str,
) -> Result<CustomGarment, String> {
    let scale = match units {
        "dm" => 1.,
        "m" => 10.,
        _ => return Err("OBJ units must be m or dm".into()),
    };
    let slot: GarmentSlot =
        serde_json::from_str(&format!("\"{slot}\"")).map_err(|_| "unknown garment slot")?;
    let mut raw_positions = Vec::<[f32; 3]>::new();
    let mut raw_uvs = Vec::<[f32; 2]>::new();
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    let mut corners = std::collections::BTreeMap::<(usize, usize), u32>::new();
    for line in text.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.as_slice() {
            ["v", x, y, z, ..] => {
                let p = [
                    x.parse::<f32>().map_err(|_| "bad OBJ vertex")?,
                    y.parse::<f32>().map_err(|_| "bad OBJ vertex")?,
                    z.parse::<f32>().map_err(|_| "bad OBJ vertex")?,
                ];
                if !p.iter().all(|v| v.is_finite()) {
                    return Err("OBJ vertex is non-finite".into());
                }
                raw_positions.push([p[0] * scale, p[1] * scale, p[2] * scale]);
            }
            ["vt", u, v, ..] => {
                let uv = [
                    u.parse::<f32>().map_err(|_| "bad OBJ texture coordinate")?,
                    v.parse::<f32>().map_err(|_| "bad OBJ texture coordinate")?,
                ];
                if !uv.iter().all(|v| v.is_finite()) {
                    return Err("OBJ texture coordinate is non-finite".into());
                }
                raw_uvs.push(uv);
            }
            ["f", rest @ ..] => {
                if rest.len() < 3 {
                    return Err("OBJ face has fewer than three corners".into());
                }
                let mut face = Vec::with_capacity(rest.len());
                for corner in rest {
                    let mut split = corner.split('/');
                    let v = parse_index(split.next().unwrap_or(""), raw_positions.len(), "vertex")?;
                    let vt = parse_index(
                        split
                            .next()
                            .filter(|x| !x.is_empty())
                            .ok_or("OBJ faces require v/vt corners")?,
                        raw_uvs.len(),
                        "texture",
                    )?;
                    let key = (v, vt);
                    let out = if let Some(&out) = corners.get(&key) {
                        out
                    } else {
                        let out = positions.len() as u32;
                        corners.insert(key, out);
                        positions.push(raw_positions[v]);
                        // OBJ measures V from the bottom; NativePart/glTF
                        // texture coordinates measure V from the top.
                        uvs.push([raw_uvs[vt][0], 1. - raw_uvs[vt][1]]);
                        out
                    };
                    face.push(out);
                }
                // A fan is exact for the documented convex OBJ polygon input.
                for i in 1..face.len() - 1 {
                    indices.extend([face[0], face[i], face[i + 1]]);
                }
            }
            _ => {}
        }
    }
    if positions.is_empty() || indices.is_empty() {
        return Err("OBJ contains no textured faces".into());
    }
    let body = base_positions(lib)?;
    let regions = array(lib, "regions")?.u8()?;
    if regions.len() != body.len() {
        return Err("base regions do not match base positions".into());
    }
    // Beards must follow the face/jaw, rather than the nearby neck.  Cartoon
    // body shaping moves the neck independently enough that a generic nearest
    // surface binding turns a lower beard into a hanging curtain.  Region 0 is
    // the native base's head/jaw domain; other garment slots retain the broad
    // non-hidden binding domain used historically.
    let eligible: Vec<_> = regions
        .iter()
        .enumerate()
        .filter_map(|(i, &r)| {
            let bindable = if matches!(slot, GarmentSlot::Beard) {
                r == 0
            } else {
                r != 255
            };
            bindable.then_some(i)
        })
        .collect();
    if eligible.len() < 3 {
        return Err("base body has fewer than three bindable vertices".into());
    }
    let mut refs = Vec::with_capacity(positions.len());
    let mut weights = Vec::with_capacity(positions.len());
    let mut reference_points_dm = Vec::with_capacity(positions.len());
    for point in &positions {
        let mut nearest: Vec<_> = eligible
            .iter()
            .map(|&i| {
                let d = (0..3).map(|a| (body[i][a] - point[a]).powi(2)).sum::<f32>();
                (d, i)
            })
            .collect();
        nearest.sort_by(|a, b| a.0.total_cmp(&b.0));
        let chosen = [nearest[0], nearest[1], nearest[2]];
        let inverse = chosen.map(|(d, _)| 1. / d.max(0.000_001));
        let sum = inverse.iter().sum::<f32>();
        let w = inverse.map(|v| v / sum);
        let r = chosen.map(|(_, i)| i as u32);
        let anchor =
            std::array::from_fn(|axis| (0..3).map(|k| body[r[k] as usize][axis] * w[k]).sum());
        refs.push(r);
        weights.push(w);
        reference_points_dm.push(anchor);
    }
    let garment = CustomGarment {
        id: id.into(),
        name: id.into(),
        slot,
        positions_dm: positions,
        indices,
        uvs,
        refs,
        weights,
        reference_points_dm,
        material: serde_json::json!({"color":"#ffffff","roughness":0.8}),
        hidden_faces: vec![],
    };
    garment.validate()?;
    Ok(garment)
}

/// Exports the unmodified native base mesh for external binding.  `units` is
/// `dm` (the native MakeHuman coordinate frame) or `m`; it does no axis swap.
pub fn body_template_obj(lib: &NativeLibrary, units: &str) -> Result<String, String> {
    let scale = match units {
        "dm" => 1.,
        "m" => 0.1,
        _ => return Err("OBJ units must be m or dm".into()),
    };
    let positions = base_positions(lib)?;
    let uvs = array(lib, "uvs")?.f32()?;
    let verts = array(lib, "faceVerts")?.u32()?;
    let face_uvs = array(lib, "faceUVs")?.u32()?;
    if uvs.len() % 2 != 0 || verts.len() != face_uvs.len() {
        return Err("base OBJ arrays are malformed".into());
    }
    // The canonical native body stores fixed quads and omits `faceSizes`.
    // Imported proxy meshes retain variable polygon sizes, so support both.
    let sizes = match lib.base.arrays.get("faceSizes") {
        Some(sizes) => sizes.u8()?.to_vec(),
        None if verts.len() % 4 == 0 => vec![4; verts.len() / 4],
        None => return Err("base body faces are not quads and have no face sizes".into()),
    };
    let mut output = String::from(
        "# Native body template: Y-up, hip-origin; bind custom garments with this exact mesh.\n",
    );
    for p in &positions {
        output.push_str(&format!(
            "v {} {} {}\n",
            p[0] * scale,
            p[1] * scale,
            p[2] * scale
        ));
    }
    for uv in uvs.chunks_exact(2) {
        output.push_str(&format!("vt {} {}\n", uv[0], uv[1]));
    }
    let mut offset = 0usize;
    for size in sizes {
        let end = offset
            .checked_add(size as usize)
            .filter(|&n| n <= verts.len())
            .ok_or("base face data is truncated")?;
        output.push_str("f");
        for i in offset..end {
            if verts[i] as usize >= positions.len() || face_uvs[i] as usize >= uvs.len() / 2 {
                return Err("base face index is out of bounds".into());
            }
            output.push_str(&format!(" {}/{}", verts[i] + 1, face_uvs[i] + 1));
        }
        output.push('\n');
        offset = end;
    }
    if offset != verts.len() {
        return Err("base face sizes do not consume face data".into());
    }
    Ok(output)
}
