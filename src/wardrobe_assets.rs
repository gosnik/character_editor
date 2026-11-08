//! Portable, body-bound garments authored against the exported native body OBJ.
//!
//! `CustomGarment` deliberately stores only geometry and a small JSON material.
//! OBJ material names and texture files are not imported: callers choose the
//! material JSON (and native-library-relative texture paths) explicitly.
use crate::{
    native_assets::{Array, NativeLibrary},
    native_character::{NativePart, SkinWeightSourceTopology},
    native_fit::BodyDeformation,
    native_sleeve_cage::SleeveCage,
    native_torso_cage::TorsoCage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
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
const MAX_DISPLACEMENT_SMOOTH_NODES: usize = 1_000_000;
/// Surface binding is only substituted when the nearest reviewed body face is
/// close enough to the authored point.  Far-away accessories retain the
/// historical vertex binding instead of acquiring an arbitrary body anchor.
const MAX_SURFACE_BIND_DISTANCE_DM: f32 = 0.5;

#[derive(Clone, Copy)]
struct FootwearMorphAnchor {
    refs: [u32; 3],
    weights: [f32; 3],
    reference_point: [f32; 3],
}

struct FootwearMorphAnchors {
    split_x: f32,
    negative_x: FootwearMorphAnchor,
    positive_x: FootwearMorphAnchor,
}

#[derive(Clone, Copy)]
struct CoherentBodyUpperGuide {
    anchor: FootwearMorphAnchor,
    start_y_dm: f32,
    end_y_dm: f32,
    coherent_offsets: bool,
}

#[derive(Clone, Copy)]
struct CoherentBodyLocalUpperBlend {
    start_y_dm: f32,
    end_y_dm: f32,
}

#[derive(Clone, Copy)]
struct CoherentBodyAnchor {
    anchor: FootwearMorphAnchor,
    upper_guide: Option<CoherentBodyUpperGuide>,
    local_upper_blend: Option<CoherentBodyLocalUpperBlend>,
}

#[derive(Clone, Copy)]
struct FootwearShaftAnchors {
    negative_x: FootwearMorphAnchor,
    positive_x: FootwearMorphAnchor,
}

/// An explicit source-space transition band for tall footwear.  Below the
/// band, the shoe uses one stable foot-component frame; above it, the shaft
/// returns to its reviewed per-vertex calf binding.  This is deliberately an
/// author opt-in because ordinary footwear benefits from one coherent frame.
#[derive(Clone, Copy)]
struct FootwearShaftBlend {
    start_y_dm: f32,
    end_y_dm: f32,
    span_dm: f32,
    calf_anchors: Option<FootwearShaftAnchors>,
}

fn parse_footwear_morph_anchor(
    object: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<FootwearMorphAnchor, String> {
    let anchor = object
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("coherent footwear missing {name} anchor"))?;
    parse_morph_anchor_object(anchor, &format!("coherent footwear {name}"))
}

fn parse_morph_anchor_object(
    anchor: &serde_json::Map<String, Value>,
    label: &str,
) -> Result<FootwearMorphAnchor, String> {
    let refs_value = anchor
        .get("refs3")
        .and_then(Value::as_array)
        .filter(|values| values.len() == 3)
        .ok_or_else(|| format!("{label} refs3 is invalid"))?;
    let refs = std::array::from_fn(|index| refs_value[index].as_u64().unwrap_or(u64::MAX) as u32);
    if refs_value.iter().any(|value| {
        value
            .as_u64()
            .is_none_or(|reference| reference > u32::MAX as u64)
    }) || refs.iter().any(|&reference| reference >= MAX_BODY_VERTEX)
    {
        return Err(format!("{label} refs3 is invalid"));
    }
    let parse_floats = |key: &str| -> Result<[f32; 3], String> {
        let values = anchor
            .get(key)
            .and_then(Value::as_array)
            .filter(|values| values.len() == 3)
            .ok_or_else(|| format!("{label} {key} is invalid"))?;
        let result = std::array::from_fn(|index| values[index].as_f64().unwrap_or(f64::NAN) as f32);
        result
            .iter()
            .all(|value| value.is_finite())
            .then_some(result)
            .ok_or_else(|| format!("{label} {key} is invalid"))
    };
    let weights = parse_floats("weights3")?;
    if weights.iter().any(|weight| *weight < 0.) || (weights.iter().sum::<f32>() - 1.).abs() > 0.001
    {
        return Err(format!("{label} weights3 is invalid"));
    }
    Ok(FootwearMorphAnchor {
        refs,
        weights,
        reference_point: parse_floats("referencePointDM")?,
    })
}

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
        validate_material(&self.material)?;
        TorsoCage::parse(
            &self.material,
            matches!(self.slot, GarmentSlot::Top | GarmentSlot::Jacket),
        )?;
        SleeveCage::parse(
            &self.material,
            matches!(self.slot, GarmentSlot::Top | GarmentSlot::Jacket),
            self.positions_dm.len(),
        )?;
        self.displacement_smoothing_spacing()?;
        let fit_max_correction = self.fit_max_correction_mm()?;
        self.head_surface_clearance_mm()?;
        self.head_outer_cap_grid_dm()?;
        self.head_neck_clearance()?;
        crate::native_shell::parse_shell_pairs(self)?;
        self.clearance_coherent_grid_dm()?;
        if fit_max_correction.is_some_and(|limit| (limit - 40.).abs() > f32::EPSILON) {
            if let Some(value) = self.material.get("fitClearanceUpperBandPhysicalCoverage") {
                let physical = value
                    .as_bool()
                    .ok_or("fitClearanceUpperBandPhysicalCoverage must be boolean")?;
                if physical {
                    return Err("fitClearanceUpperBandPhysicalCoverage does not support nondefault fitMaxCorrectionMm".into());
                }
            }
        }
        self.coherent_body_anchor()?;
        let coherent = self.coherent_footwear_anchors()?;
        self.coherent_footwear_shaft_blend(coherent.is_some())?;
        self.animation_surface_rebind()?;
        self.skin_weight_smoothing_iterations()?;
        self.skin_weight_repair_vertices()?;
        Ok(())
    }

    /// Optional source-space grid for coherent body-clearance displacement.
    /// The grid is deliberately limited to one exterior top or jacket: other
    /// clearance modes make an independently projected correction afterwards.
    pub(crate) fn clearance_coherent_grid_dm(&self) -> Result<Option<f32>, String> {
        let Some(value) = self.material.get("fitClearanceCoherentGridDM") else {
            return Ok(None);
        };
        if !matches!(self.slot, GarmentSlot::Top | GarmentSlot::Jacket) {
            return Err(
                "fitClearanceCoherentGridDM is only valid for top or jacket garments".into(),
            );
        }
        if self
            .material
            .get("fitClearanceMm")
            .and_then(Value::as_f64)
            .map(|value| value as f32)
            .filter(|value| value.is_finite() && *value > 0. && *value <= 10.)
            .is_none()
        {
            return Err("fitClearanceCoherentGridDM requires valid fitClearanceMm".into());
        }
        let spacing = value
            .as_f64()
            .ok_or("fitClearanceCoherentGridDM must be a finite number within 0.1..=0.5")?
            as f32;
        if !spacing.is_finite() || !(0.1..=0.5).contains(&spacing) {
            return Err(
                "fitClearanceCoherentGridDM must be a finite number within 0.1..=0.5".into(),
            );
        }
        let incompatible = [
            "fitTriangleClearance",
            "fitClearanceExcludedVertices",
            "fitClearanceUpperBandMm",
            "fitClearanceUpperBandCoveredOnly",
            "fitClearanceUpperBandCoveredTuckMm",
            "fitClearanceUpperBandPhysicalCoverage",
            "fitLayerClearanceMm",
        ];
        if incompatible
            .iter()
            .any(|key| self.material.get(*key).is_some())
        {
            return Err("fitClearanceCoherentGridDM cannot be combined with triangle, exclusion, upper-band, or layer clearance metadata".into());
        }
        if self.material.get("fitShellDisplacementPairs").is_some() {
            return Err(
                "fitClearanceCoherentGridDM cannot be combined with fitShellDisplacementPairs"
                    .into(),
            );
        }
        Ok(Some(spacing))
    }

    /// Optional, source-preserving animation smoothing for imported tops and
    /// jackets.  Geometry and the authored body references remain untouched;
    /// the rig consumes this metadata only while constructing GPU skin weights.
    pub(crate) fn skin_weight_smoothing_iterations(&self) -> Result<u8, String> {
        let Some(value) = self.material.get("fitSkinWeightSmoothingIterations") else {
            return Ok(0);
        };
        if !matches!(self.slot, GarmentSlot::Top | GarmentSlot::Jacket) {
            return Err(
                "fitSkinWeightSmoothingIterations is only valid for top or jacket garments".into(),
            );
        }
        let iterations = value
            .as_u64()
            .ok_or("fitSkinWeightSmoothingIterations must be an integer within 0..=8")?;
        if iterations > 8 {
            return Err("fitSkinWeightSmoothingIterations must be an integer within 0..=8".into());
        }
        Ok(iterations as u8)
    }

    /// Explicit, bounded source-vertex islands whose animation weights should
    /// be harmonically reconstructed from their unmarked mesh boundary.
    pub(crate) fn skin_weight_repair_vertices(&self) -> Result<Vec<usize>, String> {
        let Some(value) = self.material.get("fitSkinWeightRepairVertices") else {
            return Ok(Vec::new());
        };
        if !matches!(self.slot, GarmentSlot::Top | GarmentSlot::Jacket) {
            return Err(
                "fitSkinWeightRepairVertices is only valid for top or jacket garments".into(),
            );
        }
        if self.skin_weight_smoothing_iterations()? != 0 {
            return Err("fitSkinWeightRepairVertices cannot be combined with fitSkinWeightSmoothingIterations".into());
        }
        let values = value
            .as_array()
            .ok_or("fitSkinWeightRepairVertices must be an array of vertex indices")?;
        if values.len() > 4_096 {
            return Err("fitSkinWeightRepairVertices exceeds 4096 vertices".into());
        }
        let mut vertices = BTreeSet::new();
        for value in values {
            let vertex = value
                .as_u64()
                .ok_or("fitSkinWeightRepairVertices must contain integer vertex indices")?;
            let vertex = usize::try_from(vertex)
                .map_err(|_| "fitSkinWeightRepairVertices contains an out-of-range vertex index")?;
            if vertex >= self.positions_dm.len() {
                return Err(
                    "fitSkinWeightRepairVertices contains an out-of-range vertex index".into(),
                );
            }
            if !vertices.insert(vertex) {
                return Err("fitSkinWeightRepairVertices contains duplicate vertex indices".into());
            }
        }
        if vertices.is_empty() {
            return Ok(Vec::new());
        }
        let key = |position: [f32; 3]| {
            position.map(|value| if value == 0. { 0 } else { value.to_bits() })
        };
        let mut by_position = BTreeMap::<[u32; 3], usize>::new();
        let mut groups = Vec::with_capacity(self.positions_dm.len());
        let mut next_group = 0;
        for &position in &self.positions_dm {
            let group = *by_position.entry(key(position)).or_insert_with(|| {
                let group = next_group;
                next_group += 1;
                group
            });
            groups.push(group);
        }
        let mut marked = vec![false; next_group];
        for vertex in &vertices {
            marked[groups[*vertex]] = true;
        }
        let mut neighbours = vec![BTreeSet::new(); next_group];
        for triangle in self.indices.chunks_exact(3) {
            let groups = [
                groups[triangle[0] as usize],
                groups[triangle[1] as usize],
                groups[triangle[2] as usize],
            ];
            for (a, b) in [
                (groups[0], groups[1]),
                (groups[1], groups[2]),
                (groups[2], groups[0]),
            ] {
                if a != b {
                    neighbours[a].insert(b);
                    neighbours[b].insert(a);
                }
            }
        }
        let mut visited = vec![false; marked.len()];
        for start in 0..marked.len() {
            if !marked[start] || visited[start] {
                continue;
            }
            let mut stack = vec![start];
            visited[start] = true;
            let mut has_boundary = false;
            while let Some(group) = stack.pop() {
                for &neighbour in &neighbours[group] {
                    if marked[neighbour] {
                        if !visited[neighbour] {
                            visited[neighbour] = true;
                            stack.push(neighbour);
                        }
                    } else {
                        has_boundary = true;
                    }
                }
            }
            if !has_boundary {
                return Err(
                    "fitSkinWeightRepairVertices component has no unmarked boundary".into(),
                );
            }
        }
        Ok(vertices.into_iter().collect())
    }

    fn animation_surface_rebind(&self) -> Result<bool, String> {
        let Some(value) = self.material.get("fitAnimationSurfaceRebind") else {
            return Ok(false);
        };
        let enabled = value
            .as_bool()
            .ok_or("fitAnimationSurfaceRebind must be a boolean")?;
        if enabled && !matches!(self.slot, GarmentSlot::Top | GarmentSlot::Jacket) {
            return Err(
                "fitAnimationSurfaceRebind is only valid for top or jacket garments".into(),
            );
        }
        Ok(enabled)
    }

    /// Optional animation-only bindings against the final morphed body, after
    /// clearance. Saved source bindings and every fitted geometry stream stay
    /// intact. Points farther than 5 cm retain their authored animation refs.
    pub fn fit_animation_bindings(
        &self,
        lib: &NativeLibrary,
        coords_dm: &[[f32; 3]],
        part: &mut NativePart,
    ) -> Result<(), String> {
        if !self.animation_surface_rebind()? {
            return Ok(());
        }
        self.validate()?;
        let regions = array(lib, "regions")?.u8()?;
        let valid = |p: &[f32; 3]| p.iter().all(|v| v.is_finite() && v.abs() <= 100.);
        if coords_dm.len() != regions.len()
            || coords_dm.len() > MAX_BODY_VERTEX as usize
            || coords_dm.iter().any(|p| !valid(p))
            || part.positions.len() != self.positions_dm.len()
            || part.refs.len() != part.positions.len()
            || part.ref_weights.len() != part.positions.len()
            || part.positions.iter().any(|p| !valid(p))
        {
            return Err("animation surface rebinding has invalid fitted/body streams".into());
        }
        let eligible = regions
            .iter()
            .enumerate()
            .filter_map(|(i, r)| (1..=4).contains(r).then_some(i))
            .collect::<Vec<_>>();
        let surface = body_surface_candidates(lib, &regions, &eligible, coords_dm)?;
        if surface.root.is_none() {
            return Err("animation surface rebinding has no eligible torso/arm faces".into());
        }
        for (i, point) in part.positions.iter().enumerate() {
            if let Some((refs, weights)) = surface_binding(point, &surface, coords_dm) {
                part.refs[i] = refs;
                part.ref_weights[i] = weights;
            }
        }
        Ok(())
    }

    /// Opt-in current-head-surface clearance for custom hair.  Legacy hair
    /// retains its anchor-plane correction exactly when this is absent.
    pub fn head_surface_clearance_mm(&self) -> Result<Option<f32>, String> {
        let Some(value) = self.material.get("fitHeadSurfaceClearanceMm") else {
            return Ok(None);
        };
        if self.slot != GarmentSlot::Hair {
            return Err(
                "material fitHeadSurfaceClearanceMm is only valid for hair garments".into(),
            );
        }
        value
            .as_f64()
            .map(|value| value as f32)
            .filter(|value| value.is_finite() && *value > 0. && *value <= 10.)
            .ok_or_else(|| {
                "material fitHeadSurfaceClearanceMm must be finite and within (0,10]".to_owned()
            })
            .map(Some)
    }

    /// Include the adjacent neck in the current-head collision oracle. The
    /// authored hair bindings remain on region zero, including for nape hair.
    pub fn head_neck_clearance(&self) -> Result<bool, String> {
        let Some(value) = self.material.get("fitHeadNeckClearance") else {
            return Ok(false);
        };
        let enabled = value
            .as_bool()
            .ok_or("fitHeadNeckClearance must be boolean")?;
        if self.slot != GarmentSlot::Hair || self.head_surface_clearance_mm()?.is_none() {
            return Err("fitHeadNeckClearance requires Hair with fitHeadSurfaceClearanceMm".into());
        }
        Ok(enabled)
    }

    /// Explicit single exterior cap classification plus coherent clearance.
    /// Closed wigs with an inner wall must retain the normal-gated path.
    pub fn head_outer_cap_grid_dm(&self) -> Result<Option<f32>, String> {
        let Some(value) = self.material.get("fitHeadOuterCapGridDM") else {
            return Ok(None);
        };
        if self.slot != GarmentSlot::Hair || self.head_surface_clearance_mm()?.is_none() {
            return Err(
                "fitHeadOuterCapGridDM requires Hair with fitHeadSurfaceClearanceMm".into(),
            );
        }
        value
            .as_f64()
            .map(|v| v as f32)
            .filter(|v| v.is_finite() && (0.1..=0.5).contains(v))
            .ok_or_else(|| "fitHeadOuterCapGridDM must be finite within 0.1..0.5 DM".into())
            .map(Some)
    }

    fn displacement_smoothing_spacing(&self) -> Result<Option<f32>, String> {
        let Some(value) = self.material.get("fitDisplacementSmoothingDM") else {
            return Ok(None);
        };
        if !matches!(
            self.slot,
            GarmentSlot::Top | GarmentSlot::Bottom | GarmentSlot::Jacket | GarmentSlot::Hair
        ) {
            return Err(
                "material fitDisplacementSmoothingDM is only valid for top, bottom, jacket, or hair garments"
                    .into(),
            );
        }
        let spacing = value
            .as_f64()
            .map(|value| value as f32)
            .filter(|value| value.is_finite() && (0.05..=0.5).contains(value))
            .ok_or("material fitDisplacementSmoothingDM must be finite and within 0.05..=0.5")?;
        Ok(Some(spacing))
    }

    /// Optional clothing-local correction budget. The fitting implementation
    /// consumes this only for body and layer clearance; it never raises a
    /// global solver limit or changes garments that omit the field.
    fn fit_max_correction_mm(&self) -> Result<Option<f32>, String> {
        let Some(value) = self.material.get("fitMaxCorrectionMm") else {
            return Ok(None);
        };
        if !matches!(
            self.slot,
            GarmentSlot::Top | GarmentSlot::Bottom | GarmentSlot::Jacket | GarmentSlot::Feet
        ) {
            return Err(
                "material fitMaxCorrectionMm is only valid for top, bottom, jacket, or feet garments"
                    .into(),
            );
        }
        // Clearance is parsed by the runtime solver too, but catalog loading
        // must reject a malformed opt-in before an editor can save it.
        let clearance = self
            .material
            .get("fitClearanceMm")
            .and_then(Value::as_f64)
            .map(|value| value as f32)
            .filter(|value| value.is_finite() && *value > 0. && *value <= 10.);
        if clearance.is_none() {
            return Err("fitMaxCorrectionMm requires fitClearanceMm".into());
        }
        let limit = value
            .as_f64()
            .filter(|value| value.is_finite() && *value > 0. && *value <= 80.)
            .ok_or_else(|| {
                "material fitMaxCorrectionMm must be finite and within (0,80]".to_owned()
            })? as f32;
        if limit < clearance.expect("checked above") {
            return Err("fitMaxCorrectionMm must be at least fitClearanceMm".into());
        }
        Ok(Some(limit))
    }

    fn coherent_body_anchor(&self) -> Result<Option<CoherentBodyAnchor>, String> {
        let Some(value) = self.material.get("fitCoherentBodyAnchor") else {
            return Ok(None);
        };
        if !matches!(
            self.slot,
            GarmentSlot::Top | GarmentSlot::Bottom | GarmentSlot::Jacket
        ) {
            return Err(
                "material fitCoherentBodyAnchor is only valid for top, bottom, or jacket garments"
                    .into(),
            );
        }
        let object = value
            .as_object()
            .ok_or("material fitCoherentBodyAnchor must be an object")?;
        let mut anchor = parse_morph_anchor_object(object, "coherent body anchor")?;
        // New body-anchor metadata uses one normalized frame for both the
        // neutral provenance check and current-anchor translation. Keep legacy
        // footwear parsing unchanged.
        let sum = anchor.weights.iter().sum::<f32>();
        for weight in &mut anchor.weights {
            *weight /= sum;
        }
        let upper_guide = object
            .get("upperGuide")
            .map(|value| -> Result<CoherentBodyUpperGuide, String> {
                let object = value
                    .as_object()
                    .ok_or("coherent body anchor upperGuide must be an object")?;
                let mut anchor = parse_morph_anchor_object(object, "coherent body upperGuide")?;
                let sum = anchor.weights.iter().sum::<f32>();
                for weight in &mut anchor.weights {
                    *weight /= sum;
                }
                let number = |key: &str| {
                    object
                        .get(key)
                        .and_then(Value::as_f64)
                        .map(|value| value as f32)
                        .filter(|value| value.is_finite() && value.abs() <= 100.)
                        .ok_or_else(|| {
                            format!("coherent body upperGuide {key} must be finite and in range")
                        })
                };
                let start_y_dm = number("startYDM")?;
                let end_y_dm = number("endYDM")?;
                if end_y_dm <= start_y_dm {
                    return Err("coherent body upperGuide must have a positive span".into());
                }
                let coherent_offsets = object
                    .get("coherentOffsets")
                    .map(|value| {
                        value
                            .as_bool()
                            .ok_or("coherent body upperGuide coherentOffsets must be a boolean")
                    })
                    .transpose()?
                    .unwrap_or(false);
                Ok(CoherentBodyUpperGuide {
                    anchor,
                    start_y_dm,
                    end_y_dm,
                    coherent_offsets,
                })
            })
            .transpose()?;
        let local_upper_blend = object
            .get("localUpperBlend")
            .map(|value| -> Result<CoherentBodyLocalUpperBlend, String> {
                let object = value
                    .as_object()
                    .ok_or("coherent body anchor localUpperBlend must be an object")?;
                let number = |key: &str| {
                    object
                        .get(key)
                        .and_then(Value::as_f64)
                        .map(|value| value as f32)
                        .filter(|value| value.is_finite() && value.abs() <= 100.)
                        .ok_or_else(|| {
                            format!(
                                "coherent body localUpperBlend {key} must be finite and in range"
                            )
                        })
                };
                let start_y_dm = number("startYDM")?;
                let end_y_dm = number("endYDM")?;
                if end_y_dm <= start_y_dm {
                    return Err("coherent body localUpperBlend must have a positive span".into());
                }
                Ok(CoherentBodyLocalUpperBlend {
                    start_y_dm,
                    end_y_dm,
                })
            })
            .transpose()?;
        if upper_guide.is_some_and(|guide| !guide.coherent_offsets) && local_upper_blend.is_some() {
            return Err(
                "coherent body anchor upperGuide and localUpperBlend cannot be combined".into(),
            );
        }
        Ok(Some(CoherentBodyAnchor {
            anchor,
            upper_guide,
            local_upper_blend,
        }))
    }

    fn coherent_footwear_anchors(&self) -> Result<Option<FootwearMorphAnchors>, String> {
        let Some(value) = self.material.get("fitCoherentFootwear") else {
            return Ok(None);
        };
        let enabled = value
            .as_bool()
            .ok_or("material fitCoherentFootwear must be a boolean")?;
        if !enabled {
            return Ok(None);
        }
        if self.slot != GarmentSlot::Feet {
            return Err("material fitCoherentFootwear is only valid for feet garments".into());
        }
        let object = self.material["fitFootwearAnchors"]
            .as_object()
            .ok_or("coherent footwear requires fitFootwearAnchors")?;
        let split_x = object
            .get("splitXDM")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .ok_or("coherent footwear splitXDM must be finite")? as f32;
        if !split_x.is_finite() {
            return Err("coherent footwear splitXDM must be finite".into());
        }
        Ok(Some(FootwearMorphAnchors {
            split_x,
            negative_x: parse_footwear_morph_anchor(object, "negativeX")?,
            positive_x: parse_footwear_morph_anchor(object, "positiveX")?,
        }))
    }

    fn coherent_footwear_shaft_blend(
        &self,
        coherent_enabled: bool,
    ) -> Result<Option<FootwearShaftBlend>, String> {
        let Some(value) = self.material.get("fitFootwearShaftBlend") else {
            return Ok(None);
        };
        if !coherent_enabled {
            return Err("footwear shaft blend requires coherent footwear".into());
        }
        let object = value
            .as_object()
            .ok_or("footwear shaft blend must be an object")?;
        let number = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_f64)
                .map(|value| value as f32)
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("footwear shaft blend {key} must be finite"))
        };
        let calf_anchors = object
            .get("calfAnchors")
            .map(|value| -> Result<FootwearShaftAnchors, String> {
                let anchors = value
                    .as_object()
                    .ok_or("footwear shaft blend calfAnchors must be an object")?;
                Ok(FootwearShaftAnchors {
                    negative_x: parse_footwear_morph_anchor(anchors, "negativeX")?,
                    positive_x: parse_footwear_morph_anchor(anchors, "positiveX")?,
                })
            })
            .transpose()?;
        let blend = FootwearShaftBlend {
            start_y_dm: number("startYDM")?,
            end_y_dm: number("endYDM")?,
            span_dm: 0.,
            calf_anchors,
        };
        let span_dm = blend.end_y_dm - blend.start_y_dm;
        if !span_dm.is_finite() || span_dm <= 0. {
            return Err("footwear shaft blend must have a finite positive span".into());
        }
        Ok(Some(FootwearShaftBlend { span_dm, ..blend }))
    }

    /// Fits a garment in MakeHuman decimetres.  The native character builder
    /// performs its usual floor-origin/metre conversion after assembling parts.
    pub fn fit(&self, coords_dm: &[[f32; 3]]) -> Result<NativePart, String> {
        self.validate()?;
        let needs_source_topology = !self.skin_weight_repair_vertices()?.is_empty()
            || self.clearance_coherent_grid_dm()?.is_some();
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
            skin_weight_source_topology: if !needs_source_topology {
                None
            } else {
                Some(std::sync::Arc::new(SkinWeightSourceTopology {
                    positions: self.positions_dm.clone(),
                    indices: self.indices.clone(),
                }))
            },
            material: self.material.clone(),
        })
    }

    fn apply_displacement_smoothing(
        &self,
        part: &mut NativePart,
        spacing_dm: Option<f32>,
    ) -> Result<(), String> {
        let Some(spacing_dm) = spacing_dm else {
            return Ok(());
        };
        if part.positions.len() != self.positions_dm.len() {
            return Err("displacement smoothing position streams have unequal lengths".into());
        }
        let mut nodes = BTreeMap::<[i32; 3], (bevy::prelude::Vec3, f32)>::new();
        for (&source, &mapped) in self.positions_dm.iter().zip(&part.positions) {
            let source = bevy::prelude::Vec3::from(source);
            let delta = bevy::prelude::Vec3::from(mapped) - source;
            if !source.is_finite() || source.abs().max_element() > 100. || !delta.is_finite() {
                return Err(
                    "displacement smoothing input is non-finite or outside the source domain"
                        .into(),
                );
            }
            let voxel = source / spacing_dm;
            let base = voxel.floor();
            let fraction = voxel - base;
            let base = base.to_array().map(|value| value as i32);
            for dx in 0..=1_i32 {
                for dy in 0..=1_i32 {
                    for dz in 0..=1_i32 {
                        let weight = axis_weight(fraction.x, dx)
                            * axis_weight(fraction.y, dy)
                            * axis_weight(fraction.z, dz);
                        if weight <= 0. {
                            continue;
                        }
                        let key = [base[0] + dx, base[1] + dy, base[2] + dz];
                        if !nodes.contains_key(&key) && nodes.len() >= MAX_DISPLACEMENT_SMOOTH_NODES
                        {
                            return Err(
                                "displacement smoothing exceeds the voxel-node budget".into()
                            );
                        }
                        let entry = nodes.entry(key).or_insert((bevy::prelude::Vec3::ZERO, 0.));
                        entry.0 += delta * weight;
                        entry.1 += weight;
                    }
                }
            }
        }
        for (source, mapped) in self.positions_dm.iter().zip(&mut part.positions) {
            let source_vec = bevy::prelude::Vec3::from(*source);
            let voxel = source_vec / spacing_dm;
            let base = voxel.floor();
            let fraction = voxel - base;
            let base = base.to_array().map(|value| value as i32);
            let mut delta = bevy::prelude::Vec3::ZERO;
            let mut total = 0.;
            for dx in 0..=1_i32 {
                for dy in 0..=1_i32 {
                    for dz in 0..=1_i32 {
                        let weight = axis_weight(fraction.x, dx)
                            * axis_weight(fraction.y, dy)
                            * axis_weight(fraction.z, dz);
                        let key = [base[0] + dx, base[1] + dy, base[2] + dz];
                        if let Some((sum, count)) = nodes.get(&key) {
                            if *count > 0. {
                                delta += (*sum / *count) * weight;
                                total += weight;
                            }
                        }
                    }
                }
            }
            if !total.is_finite() || total <= 0. {
                return Err("displacement smoothing lost an authored source point".into());
            }
            let smoothed = source_vec + delta / total;
            if !smoothed.is_finite() {
                return Err("displacement smoothing produced a non-finite point".into());
            }
            *mapped = smoothed.to_array();
        }
        Ok(())
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
        let top_or_jacket = matches!(self.slot, GarmentSlot::Top | GarmentSlot::Jacket);
        let sleeve_cage =
            SleeveCage::parse(&self.material, top_or_jacket, self.positions_dm.len())?;
        let torso_cage = TorsoCage::parse(&self.material, top_or_jacket)?;
        if let Some(cage) = torso_cage {
            part.positions = cage.fit(
                &self.positions_dm,
                field.neutral_positions(),
                coords_dm,
                field.regions(),
            )?;
            return Ok(part);
        }
        let displacement_smoothing = self.displacement_smoothing_spacing()?;
        let coherent_body_anchor = self.coherent_body_anchor()?;
        let coherent_footwear = self.coherent_footwear_anchors()?;
        let coherent_split_x = coherent_footwear.as_ref().map(|anchors| anchors.split_x);
        let shaft_blend = self.coherent_footwear_shaft_blend(coherent_footwear.is_some())?;
        if let Some(body_anchor) = coherent_body_anchor {
            for anchor in std::iter::once(body_anchor.anchor)
                .chain(body_anchor.upper_guide.map(|guide| guide.anchor))
            {
                field.validate_neutral_anchor(
                    anchor.refs,
                    anchor.weights,
                    anchor.reference_point,
                )?;
                field.transform_offset_coherent(
                    anchor.refs,
                    anchor.weights,
                    bevy::prelude::Vec3::ZERO,
                )?;
            }
        }
        if let Some(anchors) = &coherent_footwear {
            // Validate both authored shoe anchors against the current topology
            // even if a malformed draft happens to have vertices on one side.
            for anchor in [anchors.negative_x, anchors.positive_x] {
                field.transform_offset_coherent(
                    anchor.refs,
                    anchor.weights,
                    bevy::prelude::Vec3::ZERO,
                )?;
            }
        }
        if let Some(anchors) = shaft_blend.and_then(|blend| blend.calf_anchors) {
            // Calf anchors use one shared local frame, but still must lie in
            // one connected native component. Validate before moving a part.
            for anchor in [anchors.negative_x, anchors.positive_x] {
                field.transform_offset_coherent(
                    anchor.refs,
                    anchor.weights,
                    bevy::prelude::Vec3::ZERO,
                )?;
            }
        }
        if let Some(body_anchor) = coherent_body_anchor {
            let anchor = body_anchor.anchor;
            let current_anchor = weighted_anchor(coords_dm, anchor.refs, anchor.weights)?;
            let upper_correction = if let Some(guide) = body_anchor
                .upper_guide
                .filter(|guide| !guide.coherent_offsets)
            {
                let projected_neutral_guide = current_anchor
                    + field.transform_offset_coherent(
                        anchor.refs,
                        anchor.weights,
                        bevy::prelude::Vec3::from(guide.anchor.reference_point)
                            - bevy::prelude::Vec3::from(anchor.reference_point),
                    )?;
                Some(
                    weighted_anchor(coords_dm, guide.anchor.refs, guide.anchor.weights)?
                        - projected_neutral_guide,
                )
            } else {
                None
            };
            let coherent_upper_anchor = body_anchor
                .upper_guide
                .filter(|guide| guide.coherent_offsets)
                .map(|guide| {
                    weighted_anchor(coords_dm, guide.anchor.refs, guide.anchor.weights)
                        .map(|current| (guide, current))
                })
                .transpose()?;
            for (index, position) in part.positions.iter_mut().enumerate() {
                let source = bevy::prelude::Vec3::from(self.positions_dm[index]);
                let offset = source - bevy::prelude::Vec3::from(anchor.reference_point);
                let mut mapped = current_anchor
                    + field.transform_offset_coherent(anchor.refs, anchor.weights, offset)?;
                if let (Some(guide), Some(correction)) = (body_anchor.upper_guide, upper_correction)
                {
                    let progress = ((source.y - guide.start_y_dm)
                        / (guide.end_y_dm - guide.start_y_dm))
                        .clamp(0., 1.);
                    let smooth = progress * progress * (3. - 2. * progress);
                    mapped += correction * smooth;
                }
                if let Some(blend) = body_anchor.local_upper_blend {
                    let progress = ((source.y - blend.start_y_dm)
                        / (blend.end_y_dm - blend.start_y_dm))
                        .clamp(0., 1.);
                    let smooth = progress * progress * (3. - 2. * progress);
                    if smooth > 0. {
                        let raw_offset =
                            source - bevy::prelude::Vec3::from(self.reference_points_dm[index]);
                        let raw =
                            weighted_anchor(coords_dm, self.refs[index], self.weights[index])?
                                + field.transform_offset(
                                    self.refs[index],
                                    self.weights[index],
                                    raw_offset,
                                )?;
                        mapped = mapped.lerp(raw, smooth);
                    }
                }
                if let Some((guide, guide_current)) = coherent_upper_anchor {
                    let progress = ((source.y - guide.start_y_dm)
                        / (guide.end_y_dm - guide.start_y_dm))
                        .clamp(0., 1.);
                    let smooth = progress * progress * (3. - 2. * progress);
                    if smooth > 0. {
                        let target = guide_current
                            + field.transform_offset_coherent(
                                guide.anchor.refs,
                                guide.anchor.weights,
                                source - bevy::prelude::Vec3::from(guide.anchor.reference_point),
                            )?;
                        mapped = mapped.lerp(target, smooth);
                    }
                }
                *position = mapped.to_array();
            }
            self.apply_displacement_smoothing(&mut part, displacement_smoothing)?;
            if let Some(cage) = &sleeve_cage {
                cage.fit(&self.positions_dm, &mut part.positions, coords_dm, field)?;
            }
            return Ok(part);
        }
        for index in 0..part.positions.len() {
            let (refs, weights, reference_point, coherent) =
                if let Some(anchors) = &coherent_footwear {
                    let anchor = if self.positions_dm[index][0] <= anchors.split_x {
                        anchors.negative_x
                    } else {
                        anchors.positive_x
                    };
                    (anchor.refs, anchor.weights, anchor.reference_point, true)
                } else {
                    (
                        self.refs[index],
                        self.weights[index],
                        self.reference_points_dm[index],
                        false,
                    )
                };
            let offset = bevy::prelude::Vec3::from(self.positions_dm[index])
                - bevy::prelude::Vec3::from(reference_point);
            let adaptive = if coherent {
                field.transform_offset_coherent(refs, weights, offset)?
            } else {
                field.transform_offset(refs, weights, offset)?
            };
            let anchor = weighted_anchor(coords_dm, refs, weights)?;
            let coherent_position = anchor + adaptive;
            if let Some(blend) = shaft_blend {
                let progress = ((self.positions_dm[index][1] - blend.start_y_dm) / blend.span_dm)
                    .clamp(0., 1.);
                // Smoothstep avoids a normal discontinuity at either authored
                // boundary while preserving each transport exactly outside it.
                let progress = progress * progress * (3. - 2. * progress);
                if progress > 0. {
                    let shaft_position = if let Some(calf_anchors) = blend.calf_anchors {
                        let anchor = if self.positions_dm[index][0]
                            <= coherent_split_x.expect("shaft blend requires coherent footwear")
                        {
                            calf_anchors.negative_x
                        } else {
                            calf_anchors.positive_x
                        };
                        let offset = bevy::prelude::Vec3::from(self.positions_dm[index])
                            - bevy::prelude::Vec3::from(anchor.reference_point);
                        weighted_anchor(coords_dm, anchor.refs, anchor.weights)?
                            + field.transform_offset_anchor_local(
                                anchor.refs,
                                anchor.weights,
                                offset,
                            )?
                    } else {
                        let regular_offset = bevy::prelude::Vec3::from(self.positions_dm[index])
                            - bevy::prelude::Vec3::from(self.reference_points_dm[index]);
                        weighted_anchor(coords_dm, self.refs[index], self.weights[index])?
                            + field.transform_offset(
                                self.refs[index],
                                self.weights[index],
                                regular_offset,
                            )?
                    };
                    part.positions[index] =
                        coherent_position.lerp(shaft_position, progress).to_array();
                    continue;
                }
            }
            part.positions[index] = coherent_position.to_array();
        }
        self.apply_displacement_smoothing(&mut part, displacement_smoothing)?;
        if let Some(cage) = &sleeve_cage {
            cage.fit(&self.positions_dm, &mut part.positions, coords_dm, field)?;
        }
        Ok(part)
    }
}

fn axis_weight(fraction: f32, upper: i32) -> f32 {
    if upper == 0 { 1. - fraction } else { fraction }
}

fn weighted_anchor(
    coords_dm: &[[f32; 3]],
    refs: [u32; 3],
    weights: [f32; 3],
) -> Result<bevy::prelude::Vec3, String> {
    let mut anchor = bevy::prelude::Vec3::ZERO;
    for (reference, weight) in refs.into_iter().zip(weights) {
        let point = coords_dm
            .get(reference as usize)
            .ok_or("garment reference is outside this body")?;
        anchor += bevy::prelude::Vec3::from(*point) * weight;
    }
    Ok(anchor)
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
    import_obj_with_binding(lib, text, id, slot, units, false)
}

/// Imports an OBJ with optional face-local body anchors.  Face-local binding
/// leaves the source positions and UVs untouched; it only replaces the three
/// body references used to transport each authored offset.
pub fn import_obj_with_binding(
    lib: &NativeLibrary,
    text: &str,
    id: &str,
    slot: &str,
    units: &str,
    surface_bind: bool,
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
    let surface = surface_bind
        .then(|| body_surface_candidates(lib, &regions, &eligible, &body))
        .transpose()?;
    let mut refs = Vec::with_capacity(positions.len());
    let mut weights = Vec::with_capacity(positions.len());
    let mut reference_points_dm = Vec::with_capacity(positions.len());
    for point in &positions {
        let (r, w) = surface
            .as_ref()
            .and_then(|surface| surface_binding(point, surface, &body))
            .unwrap_or_else(|| {
                // Surface-bound imports already have an exact BVH result.
                // Avoid sorting the entire body for every source vertex unless
                // that query actually needs the legacy nearest-three fallback.
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
                (chosen.map(|(_, i)| i as u32), inverse.map(|v| v / sum))
            });
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

struct SurfaceCandidates {
    triangles: Vec<SurfaceTriangle>,
    root: Option<Box<SurfaceNode>>,
}

struct SurfaceTriangle {
    face_id: usize,
    triangle_id: usize,
    vertices: [usize; 3],
    minimum: [f32; 3],
    maximum: [f32; 3],
}

const SURFACE_BVH_LEAF_SIZE: usize = 12;

enum SurfaceNodeKind {
    Leaf(Vec<usize>),
    Branch(Box<SurfaceNode>, Box<SurfaceNode>),
}

struct SurfaceNode {
    minimum: [f32; 3],
    maximum: [f32; 3],
    minimum_face_id: usize,
    kind: SurfaceNodeKind,
}

fn body_surface_candidates(
    lib: &NativeLibrary,
    regions: &[u8],
    eligible: &[usize],
    body: &[[f32; 3]],
) -> Result<SurfaceCandidates, String> {
    let vertices = array(lib, "faceVerts")?.u32()?;
    let sizes = match lib.base.arrays.get("faceSizes") {
        Some(sizes) => sizes.u8()?.to_vec(),
        None if vertices.len() % 4 == 0 => vec![4; vertices.len() / 4],
        None => return Err("base body faces are not quads and have no face sizes".into()),
    };
    if sizes.iter().map(|&size| size as usize).sum::<usize>() != vertices.len() {
        return Err("base body face sizes do not match face vertices".into());
    }
    let mut bindable = vec![false; regions.len()];
    for &index in eligible {
        bindable[index] = true;
    }
    let mut triangles = Vec::new();
    let mut offset = 0;
    for (face_id, size) in sizes.into_iter().enumerate() {
        let face = &vertices[offset..offset + size as usize];
        offset += size as usize;
        if face.len() < 3 || face.iter().any(|&v| v as usize >= regions.len()) {
            continue;
        }
        for corner in 1..face.len() - 1 {
            let triangle = [
                face[0] as usize,
                face[corner] as usize,
                face[corner + 1] as usize,
            ];
            if triangle.iter().all(|&v| bindable[v])
                && triangle.iter().all(|&v| regions[v] == regions[triangle[0]])
                && triangle[0] != triangle[1]
                && triangle[1] != triangle[2]
                && triangle[0] != triangle[2]
            {
                let points = triangle.map(|index| body[index]);
                let ab = subtract(points[1], points[0]);
                let ac = subtract(points[2], points[0]);
                let area2 = dot(cross(ab, ac), cross(ab, ac));
                if !points.iter().flatten().all(|value| value.is_finite())
                    || !area2.is_finite()
                    || area2 <= 1e-12
                {
                    continue;
                }
                triangles.push(SurfaceTriangle {
                    face_id,
                    triangle_id: triangles.len(),
                    vertices: triangle,
                    minimum: std::array::from_fn(|axis| {
                        points
                            .iter()
                            .map(|point| point[axis])
                            .fold(f32::INFINITY, f32::min)
                    }),
                    maximum: std::array::from_fn(|axis| {
                        points
                            .iter()
                            .map(|point| point[axis])
                            .fold(f32::NEG_INFINITY, f32::max)
                    }),
                });
            }
        }
    }
    let root = build_surface_bvh(&triangles, (0..triangles.len()).collect());
    Ok(SurfaceCandidates { triangles, root })
}

fn surface_binding(
    point: &[f32; 3],
    surface: &SurfaceCandidates,
    body: &[[f32; 3]],
) -> Option<([u32; 3], [f32; 3])> {
    surface_binding_search(point, surface, body).0
}

fn surface_binding_search(
    point: &[f32; 3],
    surface: &SurfaceCandidates,
    body: &[[f32; 3]],
) -> (Option<([u32; 3], [f32; 3])>, usize) {
    let Some(root) = surface.root.as_deref() else {
        return (None, 0);
    };
    let mut best: Option<(f32, usize, usize, [usize; 3], [f32; 3])> = None;
    let maximum_distance2 = MAX_SURFACE_BIND_DISTANCE_DM.powi(2);
    let mut visited = 0;
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        let limit = best
            .as_ref()
            .map_or(maximum_distance2, |(distance, _, _, _, _)| *distance);
        if point_aabb_distance2(*point, node.minimum, node.maximum) > limit {
            continue;
        }
        match &node.kind {
            SurfaceNodeKind::Leaf(indices) => {
                for &index in indices {
                    visited += 1;
                    let triangle = &surface.triangles[index];
                    let limit = best
                        .as_ref()
                        .map_or(maximum_distance2, |(distance, _, _, _, _)| *distance);
                    if point_aabb_distance2(*point, triangle.minimum, triangle.maximum) > limit {
                        continue;
                    }
                    let vertices = triangle.vertices;
                    let Some(weights) = closest_triangle_weights(
                        *point,
                        body[vertices[0]],
                        body[vertices[1]],
                        body[vertices[2]],
                    ) else {
                        continue;
                    };
                    let anchor: [f32; 3] = std::array::from_fn(|axis| {
                        body[vertices[0]][axis] * weights[0]
                            + body[vertices[1]][axis] * weights[1]
                            + body[vertices[2]][axis] * weights[2]
                    });
                    let distance2 = (0..3)
                        .map(|axis| (point[axis] - anchor[axis]).powi(2))
                        .sum::<f32>();
                    if !distance2.is_finite()
                        || !weights
                            .iter()
                            .all(|weight| weight.is_finite() && *weight >= 0.)
                    {
                        continue;
                    }
                    if distance2 <= maximum_distance2
                        && best.as_ref().is_none_or(
                            |(best_distance, face_id, triangle_id, _, _)| {
                                distance2 < *best_distance
                                    || (distance2 == *best_distance
                                        && (triangle.face_id, triangle.triangle_id)
                                            < (*face_id, *triangle_id))
                            },
                        )
                    {
                        best = Some((
                            distance2,
                            triangle.face_id,
                            triangle.triangle_id,
                            vertices,
                            weights,
                        ));
                    }
                }
            }
            SurfaceNodeKind::Branch(left, right) => {
                let left_distance = point_aabb_distance2(*point, left.minimum, left.maximum);
                let right_distance = point_aabb_distance2(*point, right.minimum, right.maximum);
                let left_first = left_distance < right_distance
                    || (left_distance == right_distance
                        && left.minimum_face_id <= right.minimum_face_id);
                if left_first {
                    pending.push(right);
                    pending.push(left);
                } else {
                    pending.push(left);
                    pending.push(right);
                }
            }
        }
    }
    (
        best.map(|(_, _, _, triangle, weights)| (triangle.map(|v| v as u32), weights)),
        visited,
    )
}

fn build_surface_bvh(
    triangles: &[SurfaceTriangle],
    mut indices: Vec<usize>,
) -> Option<Box<SurfaceNode>> {
    if indices.is_empty() {
        return None;
    }
    let (minimum, maximum, minimum_face_id) = surface_node_bounds(triangles, &indices);
    if indices.len() <= SURFACE_BVH_LEAF_SIZE {
        indices.sort_by_key(|&index| (triangles[index].face_id, triangles[index].triangle_id));
        return Some(Box::new(SurfaceNode {
            minimum,
            maximum,
            minimum_face_id,
            kind: SurfaceNodeKind::Leaf(indices),
        }));
    }
    let axis = (0..3)
        .max_by(|&left, &right| {
            (maximum[left] - minimum[left]).total_cmp(&(maximum[right] - minimum[right]))
        })
        .unwrap();
    indices.sort_by(|&left, &right| {
        let left_centre = (triangles[left].minimum[axis] + triangles[left].maximum[axis]) * 0.5;
        let right_centre = (triangles[right].minimum[axis] + triangles[right].maximum[axis]) * 0.5;
        left_centre
            .total_cmp(&right_centre)
            .then_with(|| triangles[left].face_id.cmp(&triangles[right].face_id))
    });
    let right = indices.split_off(indices.len() / 2);
    Some(Box::new(SurfaceNode {
        minimum,
        maximum,
        minimum_face_id,
        kind: SurfaceNodeKind::Branch(
            build_surface_bvh(triangles, indices)?,
            build_surface_bvh(triangles, right)?,
        ),
    }))
}

fn surface_node_bounds(
    triangles: &[SurfaceTriangle],
    indices: &[usize],
) -> ([f32; 3], [f32; 3], usize) {
    let minimum = std::array::from_fn(|axis| {
        indices
            .iter()
            .map(|&index| triangles[index].minimum[axis])
            .fold(f32::INFINITY, f32::min)
    });
    let maximum = std::array::from_fn(|axis| {
        indices
            .iter()
            .map(|&index| triangles[index].maximum[axis])
            .fold(f32::NEG_INFINITY, f32::max)
    });
    let minimum_face_id = indices
        .iter()
        .map(|&index| triangles[index].face_id)
        .min()
        .unwrap_or(usize::MAX);
    (minimum, maximum, minimum_face_id)
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|index| left[index] - right[index])
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    (0..3).map(|index| left[index] * right[index]).sum()
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn point_aabb_distance2(point: [f32; 3], minimum: [f32; 3], maximum: [f32; 3]) -> f32 {
    (0..3)
        .map(|axis| {
            if point[axis] < minimum[axis] {
                minimum[axis] - point[axis]
            } else if point[axis] > maximum[axis] {
                point[axis] - maximum[axis]
            } else {
                0.
            }
        })
        .map(|distance| distance * distance)
        .sum()
}

fn closest_triangle_weights(
    p: [f32; 3],
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
) -> Option<[f32; 3]> {
    let ab = subtract(b, a);
    let ac = subtract(c, a);
    let ap = subtract(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0. && d2 <= 0. {
        return Some([1., 0., 0.]);
    }
    let bp = subtract(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0. && d4 <= d3 {
        return Some([0., 1., 0.]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0. && d1 >= 0. && d3 <= 0. {
        let v = d1 / (d1 - d3);
        return normalise_weights([1. - v, v, 0.]);
    }
    let cp = subtract(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0. && d5 <= d6 {
        return Some([0., 0., 1.]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0. && d2 >= 0. && d6 <= 0. {
        let w = d2 / (d2 - d6);
        return normalise_weights([1. - w, 0., w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0. && (d4 - d3) >= 0. && (d5 - d6) >= 0. {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return normalise_weights([0., 1. - w, w]);
    }
    let denominator = (va + vb + vc).recip();
    let v = vb * denominator;
    let w = vc * denominator;
    normalise_weights([1. - v - w, v, w])
}

fn normalise_weights(weights: [f32; 3]) -> Option<[f32; 3]> {
    if !weights
        .iter()
        .all(|weight| weight.is_finite() && *weight >= -1e-6)
    {
        return None;
    }
    let weights = weights.map(|weight| weight.max(0.));
    let total: f32 = weights.iter().sum();
    (total.is_finite() && total > 0.).then(|| weights.map(|weight| weight / total))
}

#[cfg(test)]
mod surface_bvh_tests {
    use super::*;

    #[test]
    fn bvh_prunes_distributed_faces_before_testing_them() {
        let mut body = vec![[-1., -1., 0.], [1., -1., 0.], [-1., 1., 0.]];
        let mut triangles = vec![SurfaceTriangle {
            face_id: 0,
            triangle_id: 0,
            vertices: [0, 1, 2],
            minimum: [-1., -1., 0.],
            maximum: [1., 1., 0.],
        }];
        for face_id in 1..1025 {
            let x = face_id as f32 + 10.;
            let start = body.len();
            body.extend([[x, 0., 0.], [x + 0.5, 0., 0.], [x, 0.5, 0.]]);
            triangles.push(SurfaceTriangle {
                face_id,
                triangle_id: face_id,
                vertices: [start, start + 1, start + 2],
                minimum: [x, 0., 0.],
                maximum: [x + 0.5, 0.5, 0.],
            });
        }
        let root = build_surface_bvh(&triangles, (0..triangles.len()).collect());
        let surface = SurfaceCandidates { triangles, root };
        let (binding, visited) = surface_binding_search(&[0., 0., 0.01], &surface, &body);
        assert_eq!(binding.unwrap().0, [0, 1, 2]);
        assert!(
            visited <= SURFACE_BVH_LEAF_SIZE * 2,
            "visited {visited} distributed faces"
        );
    }
}

#[cfg(test)]
mod coherent_footwear_tests {
    use super::*;
    use serde_json::json;

    fn garment(slot: GarmentSlot, flag: Value) -> CustomGarment {
        CustomGarment {
            id: "test-feet".into(),
            name: "test feet".into(),
            slot,
            positions_dm: vec![[0.2, 0.3, 0.4], [0.3, 0.3, 0.4], [0.2, 0.4, 0.4]],
            indices: vec![0, 1, 2],
            uvs: vec![[0., 0.], [1., 0.], [0., 1.]],
            refs: vec![[0, 1, 2], [1, 0, 2], [2, 1, 0]],
            weights: vec![[1., 0., 0.]; 3],
            reference_points_dm: vec![[0., 0., 0.]; 3],
            material: serde_json::json!({"fitCoherentFootwear": flag,
                "fitFootwearAnchors": {"splitXDM": 0.0,
                    "negativeX": {"refs3": [0, 1, 2], "weights3": [1.0, 0.0, 0.0], "referencePointDM": [0.0, 0.0, 0.0]},
                    "positiveX": {"refs3": [0, 1, 2], "weights3": [1.0, 0.0, 0.0], "referencePointDM": [0.0, 0.0, 0.0]}}}),
            hidden_faces: vec![],
        }
    }

    #[test]
    fn coherent_footwear_is_feet_only_boolean_and_does_not_mutate_source_streams() {
        let mut custom = garment(GarmentSlot::Feet, Value::Bool(true));
        let before = custom.clone();
        let neutral = vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let current: Vec<_> = neutral
            .iter()
            .map(|point| [point[0] * 2., point[1] * 2., point[2] * 2.])
            .collect();
        let field = BodyDeformation::new(
            &neutral,
            &current,
            &[0, 1, 2, 2, 0, 3, 1, 1, 0, 2, 3, 3, 1, 3, 2, 2],
            &[9; 4],
        )
        .unwrap();
        let part = custom.fit_adaptive(&current, &field).unwrap();
        assert_eq!(custom, before);
        assert_eq!(part.indices, before.indices);
        assert_eq!(part.uvs, before.uvs);
        assert_eq!(
            part.refs, before.refs,
            "NativeRig keeps authored per-vertex refs"
        );
        assert_eq!(
            part.ref_weights, before.weights,
            "NativeRig keeps authored per-vertex weights"
        );
        assert_ne!(
            part.refs[0], part.refs[1],
            "authored rig bindings remain vertex-specific"
        );

        assert!(
            garment(GarmentSlot::Top, Value::Bool(true))
                .validate()
                .is_err()
        );
        assert!(
            garment(GarmentSlot::Top, Value::Bool(false))
                .validate()
                .is_ok()
        );
        assert!(
            garment(GarmentSlot::Feet, serde_json::json!(1))
                .validate()
                .is_err()
        );
        let mut huge_split = garment(GarmentSlot::Feet, Value::Bool(true));
        huge_split.material["fitFootwearAnchors"]["splitXDM"] = serde_json::json!(1e300_f64);
        assert!(huge_split.validate().is_err());
    }

    #[test]
    fn coherent_footwear_neutral_fit_matches_authored_rig_binding_fit() {
        let neutral = vec![[0., 0., 0.], [2., 0., 0.], [0., 2., 0.], [0., 0., 2.]];
        let field = BodyDeformation::new(
            &neutral,
            &neutral,
            &[0, 1, 2, 2, 0, 3, 1, 1, 0, 2, 3, 3, 1, 3, 2, 2],
            &[9; 4],
        )
        .unwrap();
        let mut custom = garment(GarmentSlot::Feet, Value::Bool(true));
        custom.positions_dm = vec![[0.25, 0.5, 0.5], [2.25, 0.5, 0.5], [0.25, 2.5, 0.5]];
        custom.refs = vec![[0, 1, 2], [1, 0, 2], [2, 1, 0]];
        custom.weights = vec![[1., 0., 0.]; 3];
        custom.reference_points_dm = vec![neutral[0], neutral[1], neutral[2]];
        let regular = custom.fit(&neutral).unwrap();
        let adaptive = custom.fit_adaptive(&neutral, &field).unwrap();
        for (expected, actual) in regular.positions.iter().zip(&adaptive.positions) {
            for axis in 0..3 {
                assert!((expected[axis] - actual[axis]).abs() <= 1e-6);
            }
        }
        custom.material["fitFootwearShaftBlend"] = json!({
            "startYDM": 0.0, "endYDM": 1.0,
            "calfAnchors": {
                "negativeX": {"refs3": [2, 1, 0], "weights3": [1.0, 0.0, 0.0], "referencePointDM": neutral[2]},
                "positiveX": {"refs3": [1, 0, 2], "weights3": [1.0, 0.0, 0.0], "referencePointDM": neutral[1]}
            }
        });
        let calf_blended = custom.fit_adaptive(&neutral, &field).unwrap();
        for (expected, actual) in regular.positions.iter().zip(&calf_blended.positions) {
            for axis in 0..3 {
                assert!((expected[axis] - actual[axis]).abs() <= 1e-6);
            }
        }
        assert_eq!(calf_blended.refs, custom.refs);
        assert_eq!(calf_blended.ref_weights, custom.weights);
    }

    #[test]
    fn shaft_blend_uses_foot_transport_below_band_and_calf_transport_above_it() {
        let neutral = vec![
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 1., 0.], // foot component
            [0., 0., 0.],
            [2., 0., 0.],
            [0., 2., 0.], // calf component
        ];
        let current = vec![
            [0., 0., 0.],
            [2., 0., 0.],
            [0., 2., 0.],
            [0., 0., 0.],
            [6., 0., 0.],
            [0., 6., 0.],
        ];
        let field = BodyDeformation::new(
            &neutral,
            &current,
            &[0, 1, 2, 2, 3, 4, 5, 5],
            &[9, 9, 9, 8, 8, 8],
        )
        .unwrap();
        let mut blended = garment(GarmentSlot::Feet, Value::Bool(true));
        blended.positions_dm = vec![[0.2, 0., 0.4], [2.2, 2., 0.4], [0.2, 2., 0.4]];
        blended.refs = vec![[3, 3, 3], [4, 4, 4], [5, 5, 5]];
        blended.weights = vec![[1., 0., 0.]; 3];
        blended.reference_points_dm = vec![neutral[3], neutral[4], neutral[5]];
        blended.material["fitFootwearShaftBlend"] = serde_json::json!({
            "startYDM": 0.5,
            "endYDM": 1.0,
            "calfAnchors": {
                "negativeX": {"refs3": [3, 4, 5], "weights3": [1.0, 0.0, 0.0], "referencePointDM": [0.0, 0.0, 0.0]},
                "positiveX": {"refs3": [3, 4, 5], "weights3": [1.0, 0.0, 0.0], "referencePointDM": [0.0, 0.0, 0.0]}
            }
        });
        let mut foot_only = blended.clone();
        foot_only
            .material
            .as_object_mut()
            .unwrap()
            .remove("fitFootwearShaftBlend");
        let result = blended.fit_adaptive(&current, &field).unwrap();
        let foot = foot_only.fit_adaptive(&current, &field).unwrap();
        let local_calf_position = |source: [f32; 3]| {
            (weighted_anchor(&current, [3, 4, 5], [1., 0., 0.]).unwrap()
                + field
                    .transform_offset_anchor_local(
                        [3, 4, 5],
                        [1., 0., 0.],
                        bevy::prelude::Vec3::from(source),
                    )
                    .unwrap())
            .to_array()
        };
        assert_eq!(result.positions[0], foot.positions[0]);
        assert_eq!(
            result.positions[1],
            local_calf_position(blended.positions_dm[1])
        );
        assert_eq!(
            result.positions[2],
            local_calf_position(blended.positions_dm[2])
        );
        let source_patch_edge = bevy::prelude::Vec3::from(blended.positions_dm[1])
            - bevy::prelude::Vec3::from(blended.positions_dm[2]);
        let expected_patch_edge = field
            .transform_offset_anchor_local([3, 4, 5], [1., 0., 0.], source_patch_edge)
            .unwrap();
        assert!(
            (bevy::prelude::Vec3::from(result.positions[1])
                - bevy::prelude::Vec3::from(result.positions[2])
                - expected_patch_edge)
                .length()
                < 1e-5,
            "upper-shaft neighbors must share the calf anchor's local frame"
        );
        assert_eq!(result.refs, blended.refs);
        assert_eq!(result.ref_weights, blended.weights);

        let mut malformed = blended.clone();
        malformed.material["fitFootwearShaftBlend"]["endYDM"] = serde_json::json!(0.5);
        assert!(malformed.validate().is_err());
        let mut midpoint = blended.clone();
        midpoint.positions_dm[0][1] = 0.75;
        let mut midpoint_foot = midpoint.clone();
        midpoint_foot
            .material
            .as_object_mut()
            .unwrap()
            .remove("fitFootwearShaftBlend");
        let mixed = midpoint.fit_adaptive(&current, &field).unwrap();
        let foot = midpoint_foot.fit_adaptive(&current, &field).unwrap();
        let calf = local_calf_position(midpoint.positions_dm[0]);
        for axis in 0..3 {
            assert!(
                (mixed.positions[0][axis] - (foot.positions[0][axis] + calf[axis]) * 0.5).abs()
                    < 1e-5
            );
        }
        assert_eq!(mixed.refs, midpoint.refs);
        assert_eq!(mixed.ref_weights, midpoint.weights);
        let mut overflow = blended.clone();
        overflow.material["fitFootwearShaftBlend"] = serde_json::json!({
            "startYDM": -(f32::MAX as f64), "endYDM": f32::MAX as f64,
        });
        assert!(overflow.validate().is_err());
        let mut without_coherent = blended;
        without_coherent.material["fitCoherentFootwear"] = Value::Bool(false);
        assert!(without_coherent.validate().is_err());

        let mut cross_component = foot_only;
        cross_component.material["fitFootwearShaftBlend"] = serde_json::json!({
            "startYDM": 0.5,
            "endYDM": 1.0,
            "calfAnchors": {
                "negativeX": {"refs3": [0, 3, 3], "weights3": [1.0, 0.0, 0.0], "referencePointDM": [0.0, 0.0, 0.0]},
                "positiveX": {"refs3": [3, 4, 5], "weights3": [1.0, 0.0, 0.0], "referencePointDM": [0.0, 0.0, 0.0]}
            }
        });
        assert!(cross_component.fit_adaptive(&current, &field).is_err());
    }
}

#[cfg(test)]
mod coherent_body_anchor_tests {
    use super::*;
    use serde_json::json;

    const FACES: [u32; 16] = [0, 1, 2, 2, 0, 1, 3, 3, 0, 2, 3, 3, 1, 2, 3, 3];

    fn garment(material: Value) -> CustomGarment {
        CustomGarment {
            id: "coherent-body-anchor-test".into(),
            name: "coherent body anchor test".into(),
            slot: GarmentSlot::Top,
            positions_dm: vec![[0.2, 0.3, 0.4], [0.7, 0.3, 0.4], [0.2, 0.9, 0.4]],
            indices: vec![0, 1, 2],
            uvs: vec![[0., 0.], [1., 0.], [0., 1.]],
            refs: vec![[0, 1, 2], [1, 0, 2], [2, 1, 0]],
            weights: vec![[1., 0., 0.]; 3],
            reference_points_dm: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            material,
            hidden_faces: vec![],
        }
    }

    fn anchor() -> Value {
        json!({"refs3": [0, 1, 2], "weights3": [1.0, 0.0, 0.0],
            "referencePointDM": [0.0, 0.0, 0.0]})
    }

    fn anchor_with_local_upper_blend() -> Value {
        json!({"refs3": [0, 1, 2], "weights3": [1.0, 0.0, 0.0],
            "referencePointDM": [0.0, 0.0, 0.0],
            "localUpperBlend": {"startYDM": 0.5, "endYDM": 0.8}})
    }

    fn anchor_with_upper_guide() -> Value {
        json!({"refs3": [0, 1, 2], "weights3": [1.0, 0.0, 0.0],
            "referencePointDM": [0.0, 0.0, 0.0],
            "upperGuide": {"refs3": [3, 3, 3], "weights3": [1.0, 0.0, 0.0],
                "referencePointDM": [0.0, 0.0, 1.0], "startYDM": 0.5, "endYDM": 0.8}})
    }

    #[test]
    fn coherent_body_anchor_preserves_neutral_patch_and_authored_rig_streams() {
        let neutral = vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let field = BodyDeformation::new(&neutral, &neutral, &FACES, &[2; 4]).unwrap();
        let custom = garment(json!({"fitCoherentBodyAnchor": anchor()}));
        let part = custom.fit_adaptive(&neutral, &field).unwrap();
        assert_eq!(part.positions, custom.positions_dm);
        assert_eq!(part.refs, custom.refs);
        assert_eq!(part.ref_weights, custom.weights);
        assert_eq!(part.indices, custom.indices);
        assert_eq!(part.uvs, custom.uvs);
    }

    #[test]
    fn coherent_body_anchor_applies_one_component_affine_to_every_patch_vertex() {
        let neutral = vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let current = vec![[0., 0., 0.], [2., 0., 0.], [0., 3., 0.], [0., 0., 4.]];
        let field = BodyDeformation::new(&neutral, &current, &FACES, &[2; 4]).unwrap();
        let custom = garment(json!({"fitCoherentBodyAnchor": anchor()}));
        let part = custom.fit_adaptive(&current, &field).unwrap();
        for (source, fitted) in custom.positions_dm.iter().zip(&part.positions) {
            let expected = field
                .transform_offset_coherent(
                    [0, 1, 2],
                    [1., 0., 0.],
                    bevy::prelude::Vec3::from(*source),
                )
                .unwrap();
            assert!((bevy::prelude::Vec3::from(*fitted) - expected).length() < 1e-5);
        }
    }

    #[test]
    fn coherent_body_upper_guide_adds_only_a_smooth_upper_translation() {
        let neutral = vec![
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [0., 2., 0.],
        ];
        // The neck is connected to the torso but belongs to another native
        // region, and moves one DM beyond the torso affine prediction.
        let current = vec![
            [0., 0., 0.],
            [2., 0., 0.],
            [0., 3., 0.],
            [0., 0., 4.],
            [0., 7., 0.],
        ];
        let faces = [FACES.as_slice(), &[0, 2, 4, 4]].concat();
        let regions = [2, 2, 2, 2, 1];
        let field = BodyDeformation::new(&neutral, &current, &faces, &regions).unwrap();
        let mut metadata = anchor_with_upper_guide();
        metadata["upperGuide"]["refs3"] = json!([4, 4, 4]);
        metadata["upperGuide"]["referencePointDM"] = json!([0., 2., 0.]);
        let custom = garment(json!({"fitCoherentBodyAnchor": metadata}));
        let without_guide = garment(json!({"fitCoherentBodyAnchor": anchor()}));
        let guided = custom.fit_adaptive(&current, &field).unwrap();
        let plain = without_guide.fit_adaptive(&current, &field).unwrap();
        // The first two source points sit below the band; the third is above.
        assert_eq!(guided.positions[0], plain.positions[0]);
        assert_eq!(guided.positions[1], plain.positions[1]);
        let correction = bevy::prelude::Vec3::from(current[4])
            - field
                .transform_offset_coherent([0, 1, 2], [1., 0., 0.], bevy::prelude::Vec3::Y * 2.)
                .unwrap();
        assert!((correction - bevy::prelude::Vec3::Y).length() < 1e-5);
        assert!(
            (bevy::prelude::Vec3::from(guided.positions[2])
                - bevy::prelude::Vec3::from(plain.positions[2])
                - correction)
                .length()
                < 1e-5
        );
        assert_eq!(guided.refs, custom.refs);
        assert_eq!(guided.ref_weights, custom.weights);
        let neutral_guided = custom
            .fit_adaptive(
                &neutral,
                &BodyDeformation::new(&neutral, &neutral, &faces, &regions).unwrap(),
            )
            .unwrap();
        assert_eq!(neutral_guided.positions, custom.positions_dm);
    }

    #[test]
    fn coherent_body_local_upper_blend_keeps_lower_frame_and_reaches_raw_upper() {
        let neutral = vec![
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [0., 2., 0.],
        ];
        let current = vec![
            [0., 0., 0.],
            [2., 0., 0.],
            [0., 3., 0.],
            [0., 0., 4.],
            [0., 7., 0.],
        ];
        let faces = [FACES.as_slice(), &[0, 2, 4, 4]].concat();
        let field = BodyDeformation::new(&neutral, &current, &faces, &[2, 2, 2, 2, 1]).unwrap();
        let mut blended =
            garment(json!({"fitCoherentBodyAnchor": anchor_with_local_upper_blend()}));
        blended.positions_dm[1][1] = 0.65;
        blended.refs = vec![[4, 4, 4]; 3];
        blended.reference_points_dm = vec![[0., 2., 0.]; 3];
        let mut coherent = blended.clone();
        coherent.material = json!({"fitCoherentBodyAnchor": anchor()});
        let mut raw = blended.clone();
        raw.material = json!({});
        let blended_part = blended.fit_adaptive(&current, &field).unwrap();
        let coherent_part = coherent.fit_adaptive(&current, &field).unwrap();
        let raw_part = raw.fit_adaptive(&current, &field).unwrap();
        assert_eq!(blended_part.positions[0], coherent_part.positions[0]);
        assert_eq!(blended_part.positions[2], raw_part.positions[2]);
        assert!(
            (bevy::prelude::Vec3::from(blended_part.positions[1])
                - bevy::prelude::Vec3::from(coherent_part.positions[1]))
            .length()
                > 1e-5
        );
        assert!(
            (bevy::prelude::Vec3::from(blended_part.positions[1])
                - bevy::prelude::Vec3::from(raw_part.positions[1]))
            .length()
                > 1e-5
        );
        assert_eq!(blended_part.refs, blended.refs);
        assert_eq!(blended_part.ref_weights, blended.weights);
        let mut invalid_span =
            garment(json!({"fitCoherentBodyAnchor": anchor_with_local_upper_blend()}));
        invalid_span.material["fitCoherentBodyAnchor"]["localUpperBlend"]["endYDM"] = json!(0.5);
        assert!(invalid_span.validate().is_err());
        assert!(garment(json!({"fitCoherentBodyAnchor": {
            "refs3": [0, 1, 2], "weights3": [1.0, 0.0, 0.0], "referencePointDM": [0.0, 0.0, 0.0],
            "upperGuide": anchor_with_upper_guide()["upperGuide"].clone(),
            "localUpperBlend": {"startYDM": 0.5, "endYDM": 0.8}
        }}))
        .validate()
        .is_err());
    }

    #[test]
    fn coherent_upper_guide_uses_its_component_frame_at_upper_endpoint() {
        let neutral = vec![
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [0., 2., 0.],
        ];
        let current = vec![
            [0., 0., 0.],
            [2., 0., 0.],
            [0., 3., 0.],
            [0., 0., 4.],
            [0., 8., 0.],
        ];
        let faces = [FACES.as_slice(), &[0, 2, 4, 4]].concat();
        let regions = [2, 2, 2, 2, 1];
        let field = BodyDeformation::new(&neutral, &current, &faces, &regions).unwrap();
        let mut metadata = anchor_with_upper_guide();
        metadata["upperGuide"]["refs3"] = json!([4, 4, 4]);
        metadata["upperGuide"]["referencePointDM"] = json!([0., 2., 0.]);
        metadata["upperGuide"]["coherentOffsets"] = json!(true);
        let custom = garment(json!({"fitCoherentBodyAnchor": metadata}));
        let base = garment(json!({"fitCoherentBodyAnchor": anchor()}));
        let fitted = custom.fit_adaptive(&current, &field).unwrap();
        let baseline = base.fit_adaptive(&current, &field).unwrap();
        assert_eq!(fitted.positions[0], baseline.positions[0]);
        let source = bevy::prelude::Vec3::from(custom.positions_dm[2]);
        let target = bevy::prelude::Vec3::from(current[4])
            + field
                .transform_offset_coherent(
                    [4, 4, 4],
                    [1., 0., 0.],
                    source - bevy::prelude::Vec3::new(0., 2., 0.),
                )
                .unwrap();
        assert!((bevy::prelude::Vec3::from(fitted.positions[2]) - target).length() < 1e-5);
        let neutral_part = custom
            .fit_adaptive(
                &neutral,
                &BodyDeformation::new(&neutral, &neutral, &faces, &regions).unwrap(),
            )
            .unwrap();
        assert_eq!(neutral_part.positions, custom.positions_dm);
        let mut invalid = custom.clone();
        invalid.material["fitCoherentBodyAnchor"]["upperGuide"]["coherentOffsets"] = json!(1);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn coherent_body_anchor_normalizes_tolerated_weight_sum_before_translation() {
        let neutral = vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let translation = bevy::prelude::Vec3::new(2., -3., 4.);
        let current = neutral
            .iter()
            .map(|point| (bevy::prelude::Vec3::from(*point) + translation).to_array())
            .collect::<Vec<_>>();
        let field = BodyDeformation::new(&neutral, &current, &FACES, &[2; 4]).unwrap();
        let custom = garment(json!({"fitCoherentBodyAnchor": {
            "refs3": [0, 1, 2], "weights3": [0.5, 0.4991, 0.0],
            "referencePointDM": [0.4991_f32 / 0.9991_f32, 0.0, 0.0]
        }}));
        let part = custom.fit_adaptive(&current, &field).unwrap();
        for (source, fitted) in custom.positions_dm.iter().zip(&part.positions) {
            let expected = bevy::prelude::Vec3::from(*source) + translation;
            assert!((bevy::prelude::Vec3::from(*fitted) - expected).length() < 1e-5);
        }
    }

    #[test]
    fn coherent_body_anchor_defaults_off_and_rejects_invalid_metadata() {
        let neutral = vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let current = vec![[0., 0., 0.], [2., 0., 0.], [0., 3., 0.], [0., 0., 4.]];
        let field = BodyDeformation::new(&neutral, &current, &FACES, &[2; 4]).unwrap();
        let plain = garment(json!({}));
        let no_metadata = plain.fit_adaptive(&current, &field).unwrap();
        let explicit_unrelated = garment(json!({"roughness": 0.5}));
        assert_eq!(
            no_metadata.positions,
            explicit_unrelated
                .fit_adaptive(&current, &field)
                .unwrap()
                .positions
        );
        let mut wrong_slot = garment(json!({"fitCoherentBodyAnchor": anchor()}));
        wrong_slot.slot = GarmentSlot::Feet;
        assert!(wrong_slot.validate().is_err());
        assert!(
            garment(json!({"fitCoherentBodyAnchor": true}))
                .validate()
                .is_err()
        );
        assert!(
            garment(
                json!({"fitCoherentBodyAnchor": {"refs3": [0, 1, 2], "weights3": [1.0, 0.0, 0.0]}})
            )
            .validate()
            .is_err()
        );
        let mut mismatched = garment(json!({"fitCoherentBodyAnchor": anchor()}));
        mismatched.material["fitCoherentBodyAnchor"]["referencePointDM"] = json!([0.01, 0.0, 0.0]);
        assert!(mismatched.fit_adaptive(&current, &field).is_err());
        let mut out_of_domain = garment(json!({"fitCoherentBodyAnchor": anchor()}));
        out_of_domain.material["fitCoherentBodyAnchor"]["referencePointDM"] =
            json!([101.0, 0.0, 0.0]);
        assert!(out_of_domain.fit_adaptive(&current, &field).is_err());
        let mut invalid_span = garment(json!({"fitCoherentBodyAnchor": anchor_with_upper_guide()}));
        invalid_span.material["fitCoherentBodyAnchor"]["upperGuide"]["endYDM"] = json!(0.5);
        assert!(invalid_span.validate().is_err());
        let mut invalid_guide =
            garment(json!({"fitCoherentBodyAnchor": anchor_with_upper_guide()}));
        invalid_guide.material["fitCoherentBodyAnchor"]["upperGuide"]["referencePointDM"] =
            json!([0.0, 0.0, 0.9]);
        assert!(invalid_guide.fit_adaptive(&current, &field).is_err());
    }
}

#[cfg(test)]
mod displacement_smoothing_tests {
    use super::*;
    use serde_json::json;

    fn garment(material: Value, positions_dm: Vec<[f32; 3]>) -> CustomGarment {
        CustomGarment {
            id: "displacement-smoothing-test".into(),
            name: "displacement smoothing test".into(),
            slot: GarmentSlot::Top,
            indices: vec![0, 1, 2],
            uvs: vec![[0., 0.], [1., 0.], [0., 1.]],
            refs: vec![[0, 0, 0]; 3],
            weights: vec![[1., 0., 0.]; 3],
            reference_points_dm: vec![[0., 0., 0.]; 3],
            positions_dm,
            material,
            hidden_faces: vec![],
        }
    }

    fn part(positions: Vec<[f32; 3]>) -> NativePart {
        NativePart {
            name: "test".into(),
            positions,
            indices: vec![0, 1, 2],
            uvs: vec![[0., 0.], [1., 0.], [0., 1.]],
            refs: vec![[0, 0, 0]; 3],
            ref_weights: vec![[1., 0., 0.]; 3],
            rigid_joint: None,
            skin_weight_source_topology: None,
            material: json!({}),
        }
    }

    #[test]
    fn fit_preserves_full_source_topology_for_skin_weight_repair() {
        let custom = garment(
            json!({"fitSkinWeightRepairVertices": [0]}),
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        );
        let fitted = custom.fit(&[[0., 0., 0.]]).unwrap();
        let source = fitted.skin_weight_source_topology.as_ref().unwrap();
        assert_eq!(source.positions, custom.positions_dm);
        assert_eq!(source.indices, custom.indices);
    }

    #[test]
    fn coherent_clearance_metadata_is_scoped_and_preserves_source_topology() {
        let mut enabled = garment(
            json!({"fitClearanceMm": 3.0, "fitClearanceCoherentGridDM": 0.3}),
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        );
        assert_eq!(enabled.clearance_coherent_grid_dm().unwrap(), Some(0.3));
        let fitted = enabled.fit(&[[0., 0., 0.]]).unwrap();
        let source = fitted.skin_weight_source_topology.as_ref().unwrap();
        assert_eq!(source.positions, enabled.positions_dm);
        assert_eq!(source.indices, enabled.indices);
        for invalid in [json!(0.09), json!(0.6), json!(true)] {
            enabled.material["fitClearanceCoherentGridDM"] = invalid;
            assert!(enabled.validate().is_err());
        }
        enabled.material = json!({"fitClearanceMm": 3.0, "fitClearanceCoherentGridDM": 0.3, "fitTriangleClearance": true});
        assert!(enabled.validate().is_err());
        enabled.material = json!({"fitClearanceMm": 3.0, "fitClearanceCoherentGridDM": 0.3, "fitShellDisplacementPairs": []});
        assert!(enabled.validate().is_err());
        enabled.material = json!({"fitClearanceMm": 3.0, "fitClearanceCoherentGridDM": 0.3});
        enabled.slot = GarmentSlot::Feet;
        assert!(enabled.validate().is_err());
    }

    #[test]
    fn displacement_smoothing_preserves_uniform_and_neutral_displacements() {
        let source = vec![[0.1, 0.1, 0.1], [0.2, 0.1, 0.1], [0.1, 0.2, 0.1]];
        let custom = garment(json!({"fitDisplacementSmoothingDM": 0.2}), source.clone());
        let mut translated = part(
            source
                .iter()
                .map(|point| [point[0] + 0.3, point[1] - 0.2, point[2] + 0.1])
                .collect(),
        );
        custom
            .apply_displacement_smoothing(&mut translated, Some(0.2))
            .unwrap();
        for (actual, expected) in
            translated
                .positions
                .iter()
                .zip([[0.4, -0.1, 0.2], [0.5, -0.1, 0.2], [0.4, 0.0, 0.2]])
        {
            assert!(
                (bevy::prelude::Vec3::from(*actual) - bevy::prelude::Vec3::from(expected)).length()
                    < 1e-6
            );
        }
        let mut neutral = part(source.clone());
        custom
            .apply_displacement_smoothing(&mut neutral, Some(0.2))
            .unwrap();
        assert_eq!(neutral.positions, source);
    }

    #[test]
    fn displacement_smoothing_shares_uv_duplicate_delta_and_preserves_authored_detail() {
        let source = vec![[0.1, 0.1, 0.1], [0.1, 0.1, 0.1], [0.3, 0.1, 0.1]];
        let custom = garment(json!({"fitDisplacementSmoothingDM": 0.5}), source.clone());
        let mut mapped = part(vec![[0.1, 0.1, 0.0], [0.1, 0.1, 0.5], [0.3, 0.1, 0.1]]);
        custom
            .apply_displacement_smoothing(&mut mapped, Some(0.5))
            .unwrap();
        assert!(
            (bevy::prelude::Vec3::from(mapped.positions[0])
                - bevy::prelude::Vec3::from(mapped.positions[1]))
            .length()
                < 1e-6
        );
        // The shared field changes displacement only: the source-space X edge
        // is retained even when its Z displacement has been averaged.
        assert!((mapped.positions[2][0] - mapped.positions[0][0] - 0.2).abs() < 1e-6);
        assert!(mapped.positions[0][2] > 0. && mapped.positions[0][2] < 0.5);
    }

    #[test]
    fn hair_displacement_smoothing_is_opt_in_and_keeps_source_curls() {
        let source = vec![[0.1, 8.0, 0.1], [0.1, 8.0, 0.1], [0.3, 8.1, 0.1]];
        let mut hair = garment(json!({}), source.clone());
        hair.slot = GarmentSlot::Hair;
        assert_eq!(hair.displacement_smoothing_spacing().unwrap(), None);
        hair.material["fitDisplacementSmoothingDM"] = json!(0.5);
        hair.validate().unwrap();

        let mut neutral = part(source.clone());
        hair.apply_displacement_smoothing(
            &mut neutral,
            hair.displacement_smoothing_spacing().unwrap(),
        )
        .unwrap();
        assert_eq!(neutral.positions, source);

        // Coincident UV copies must share the head-morph delta, while the
        // authored curl's rising X/Y edge stays intact.
        let mut mapped = part(vec![[0.1, 8.0, 0.0], [0.1, 8.0, 0.5], [0.3, 8.1, 0.1]]);
        hair.apply_displacement_smoothing(
            &mut mapped,
            hair.displacement_smoothing_spacing().unwrap(),
        )
        .unwrap();
        assert_eq!(mapped.positions[0], mapped.positions[1]);
        assert!((mapped.positions[2][0] - mapped.positions[0][0] - 0.2).abs() < 1e-6);
        assert!((mapped.positions[2][1] - mapped.positions[0][1] - 0.1).abs() < 1e-6);
        assert_eq!(hair.positions_dm, source);
    }

    #[test]
    fn displacement_smoothing_rejects_invalid_metadata_and_source_domain() {
        assert!(
            garment(
                json!({"fitDisplacementSmoothingDM": 0.04}),
                vec![[0., 0., 0.]; 3]
            )
            .validate()
            .is_err()
        );
        assert!(
            garment(
                json!({"fitDisplacementSmoothingDM": true}),
                vec![[0., 0., 0.]; 3]
            )
            .validate()
            .is_err()
        );
        let mut feet = garment(
            json!({"fitDisplacementSmoothingDM": 0.2}),
            vec![[0., 0., 0.]; 3],
        );
        feet.slot = GarmentSlot::Feet;
        assert!(feet.validate().is_err());
        let custom = garment(
            json!({"fitDisplacementSmoothingDM": 0.2}),
            vec![[101., 0., 0.]; 3],
        );
        let mut raw = part(vec![[101., 0., 0.]; 3]);
        assert!(
            custom
                .apply_displacement_smoothing(&mut raw, Some(0.2))
                .is_err()
        );
    }

    #[test]
    fn skin_weight_smoothing_metadata_is_strict_and_garment_scoped() {
        let plain = garment(json!({}), vec![[0., 0., 0.]; 3]);
        assert_eq!(plain.skin_weight_smoothing_iterations().unwrap(), 0);
        let enabled = garment(
            json!({"fitSkinWeightSmoothingIterations": 3}),
            vec![[0., 0., 0.]; 3],
        );
        assert_eq!(enabled.skin_weight_smoothing_iterations().unwrap(), 3);
        for value in [json!(true), json!("3"), json!(1.5), json!(9)] {
            let invalid = garment(
                json!({"fitSkinWeightSmoothingIterations": value}),
                vec![[0., 0., 0.]; 3],
            );
            assert!(invalid.validate().is_err());
        }
        let mut wrong_slot = enabled.clone();
        wrong_slot.slot = GarmentSlot::Bottom;
        assert!(wrong_slot.validate().is_err());
    }

    #[test]
    fn skin_weight_repair_metadata_is_strict_scoped_and_requires_a_boundary() {
        let plain = garment(json!({}), vec![[0., 0., 0.]; 3]);
        assert!(plain.skin_weight_repair_vertices().unwrap().is_empty());
        let enabled = garment(
            json!({"fitSkinWeightRepairVertices": [0]}),
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        );
        assert_eq!(enabled.skin_weight_repair_vertices().unwrap(), vec![0]);
        for value in [json!(true), json!([0, 0]), json!([3]), json!([1.5])] {
            let invalid = garment(
                json!({"fitSkinWeightRepairVertices": value}),
                vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            );
            assert!(invalid.validate().is_err());
        }
        let incompatible = garment(
            json!({"fitSkinWeightRepairVertices": [0], "fitSkinWeightSmoothingIterations": 1}),
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        );
        assert!(incompatible.validate().is_err());
        let closed = garment(
            json!({"fitSkinWeightRepairVertices": [0, 1, 2]}),
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        );
        assert!(closed.validate().is_err());
        let mut wrong_slot = enabled;
        wrong_slot.slot = GarmentSlot::Bottom;
        assert!(wrong_slot.validate().is_err());
    }
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
