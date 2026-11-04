//! Portable outfit definitions and sampled wardrobe validation.
use crate::{
    ara::{AraRecipe, native_catalog},
    cli_io,
    model::atomic_write,
    native_assets::NativeLibrary,
    native_character::{NativeCharacter, NativePart},
    wardrobe_assets::CustomGarment,
};
use bevy::prelude::Vec3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WardrobeProfile {
    pub version: u32,
    pub name: String,
    pub params: Value,
    #[serde(default)]
    pub custom_garments: Vec<CustomGarment>,
}
fn wardrobe_keys(include_hair: bool, include_beard: bool) -> BTreeSet<String> {
    let catalog = native_catalog();
    let mut keys: BTreeSet<_> = catalog["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|field| {
            matches!(field["category"].as_str(), Some("Clothing" | "Props"))
                || (field["category"] == "Hair" &&
                    if matches!(field["id"].as_str(), Some("facialHair" | "beardColor")) {
                        include_beard
                    } else {
                        include_hair
                    })
        })
        .filter_map(|field| field["id"].as_str().map(str::to_owned))
        .collect();
    keys.insert("garmentStyles".into());
    keys
}
impl WardrobeProfile {
    pub fn capture(recipe: &AraRecipe, name: &str, include_hair: bool) -> Result<Self, String> {
        // Preserve the original API's complete Hair-category capture.
        Self::capture_with_beard(recipe, name, include_hair, include_hair)
    }
    pub fn capture_with_beard(recipe: &AraRecipe, name: &str, include_hair: bool, include_beard: bool) -> Result<Self, String> {
        let keys = wardrobe_keys(include_hair, include_beard);
        let profile = Self {
            version: 1,
            name: name.into(),
            params: Value::Object(
                recipe
                    .params
                    .as_object()
                    .ok_or("invalid recipe parameters")?
                    .iter()
                    .filter(|(key, _)| keys.contains(*key))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            custom_garments: recipe
                .custom_garments
                .iter()
                .filter(|g| match g.slot {
                    crate::wardrobe_assets::GarmentSlot::Hair => include_hair,
                    crate::wardrobe_assets::GarmentSlot::Beard => include_beard,
                    _ => true,
                })
                .cloned()
                .collect(),
        };
        profile.validate()?;
        Ok(profile)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.name.trim().is_empty() || self.name.len() > 256 {
            return Err(
                "wardrobe requires version 1 and a nonempty name of at most 256 bytes".into(),
            );
        }
        let params = self
            .params
            .as_object()
            .ok_or("wardrobe params must be an object")?;
        let allowed = wardrobe_keys(true, true);
        if let Some(key) = params.keys().find(|key| !allowed.contains(*key)) {
            return Err(format!(
                "wardrobe cannot change body/unknown parameter {key:?}"
            ));
        }
        let mut recipe = AraRecipe::default();
        for (key, value) in params {
            recipe.params[key] = value.clone();
        }
        recipe.custom_garments = self.custom_garments.clone();
        cli_io::validate(&recipe)
    }
    pub fn apply(&self, recipe: &mut AraRecipe) -> Result<(), String> {
        self.validate()?;
        let mut changed = recipe.clone();
        for (key, value) in self.params.as_object().unwrap() {
            changed.params[key] = value.clone();
        }
        for (parameter, slot) in [("hair", crate::wardrobe_assets::GarmentSlot::Hair),
                                  ("facialHair", crate::wardrobe_assets::GarmentSlot::Beard)] {
            if self.params.get(parameter).is_some() && !self.custom_garments.iter().any(|g| g.slot == slot) {
                changed.remove_custom_garment_slot(&slot);
            }
        }
        for garment in &self.custom_garments {
            if matches!(garment.slot, crate::wardrobe_assets::GarmentSlot::Hair | crate::wardrobe_assets::GarmentSlot::Beard) {
                changed.select_custom_garment(garment)?;
                continue;
            }
            if let Some(old) = changed
                .custom_garments
                .iter_mut()
                .find(|g| g.id == garment.id)
            {
                *old = garment.clone();
            } else {
                changed.custom_garments.push(garment.clone());
            }
        }
        cli_io::validate(&changed)?;
        *recipe = changed;
        Ok(())
    }
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let file = fs::File::open(path.as_ref()).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err("wardrobe exceeds 64 MiB".into());
        }
        let profile: Self = serde_json::from_slice(&bytes).map_err(|e| format!("wardrobe: {e}"))?;
        profile.validate()?;
        Ok(profile)
    }
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err("wardrobe exceeds 64 MiB".into());
        }
        atomic_write(path, &bytes).map_err(|e| e.to_string())
    }
}
pub fn is_garment(name: &str) -> bool {
    name == "Outfit"
        || name == "Shoes"
        || name == "Hat"
        || name.starts_with("custom:")
        || native_catalog()["garments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| {
                g["id"]
                    .as_str()
                    .is_some_and(|id| name.starts_with(&format!("{id}_")))
            })
}
/// Tests finite geometry/skinning and reports visible body/cloth intersections
/// at sampled poses. Intersections are warnings: intentional layered contacts
/// are possible, and sampling is not a cloth-simulation guarantee.
pub fn test_profile(
    lib: &NativeLibrary,
    profile: &WardrobeProfile,
    bases: &[(String, AraRecipe)],
    clips: &[String],
    samples: usize,
) -> Result<Value, String> {
    profile.validate()?;
    if bases.is_empty() || bases.len() > 64 || !(1..=120).contains(&samples) || clips.len() > 100 {
        return Err(
            "wardrobe tests require 1..64 bodies, 1..120 samples and at most 100 clips".into(),
        );
    }
    let known = crate::ara::native_catalog_with_assets(&lib.root);
    for clip in clips {
        if !known["clips"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == *clip)
        {
            return Err(format!("unknown wardrobe test clip {clip:?}"));
        }
    }
    let mut cases = vec![];
    let mut failures = 0;
    let mut warnings = 0;
    for (name, base) in bases {
        let result = (|| {
            let mut recipe = base.clone();
            profile.apply(&mut recipe)?;
            let character = NativeCharacter::build(lib, &recipe)?;
            let mut degenerate = 0;
            for part in &character.parts {
                if part.positions.is_empty()
                    || part.positions.iter().flatten().any(|v| !v.is_finite())
                    || part
                        .indices
                        .iter()
                        .any(|&i| i as usize >= part.positions.len())
                {
                    return Err(format!("invalid geometry in {}", part.name));
                }
                degenerate += part
                    .indices
                    .chunks_exact(3)
                    .filter(|t| {
                        let a = Vec3::from(part.positions[t[0] as usize]);
                        let b = Vec3::from(part.positions[t[1] as usize]);
                        let c = Vec3::from(part.positions[t[2] as usize]);
                        (b - a).cross(c - a).length_squared() < 1e-16
                    })
                    .count();
            }
            let body = character
                .parts
                .iter()
                .position(|p| p.name == "Body")
                .ok_or("character has no body")?;
            let garments: Vec<usize> = character
                .parts
                .iter()
                .enumerate()
                .filter(|(_, p)| is_garment(&p.name))
                .map(|(i, _)| i)
                .collect();
            let bindings = character.rig.as_ref().map(|rig| {
                character
                    .parts
                    .iter()
                    .map(|p| rig.skin_part(p))
                    .collect::<Vec<_>>()
            });
            let mut poses = vec![];
            let mut intersections = 0;
            let mut check_pose = |clip: &str, time: f32| -> Result<(), String> {
                let positions = if let Some(rig) = &character.rig {
                    let matrices = if clip == "rest" {
                        vec![bevy::prelude::Mat4::IDENTITY; rig.bones.len()]
                    } else {
                        crate::native_breast::bake_secondary(
                            rig,
                            clip,
                            time,
                            &recipe.breasts,
                            recipe.params["gender"].as_f64().unwrap_or(1.) as f32,
                        )
                    };
                    if matrices.iter().any(|m| !m.is_finite()) {
                        return Err(format!("nonfinite pose {clip} at {time}"));
                    }
                    character
                        .parts
                        .iter()
                        .zip(bindings.as_ref().unwrap())
                        .map(|(p, (j, w))| rig.skin_positions(&p.positions, j, w, &matrices))
                        .collect::<Vec<_>>()
                } else {
                    character
                        .parts
                        .iter()
                        .map(|p| p.positions.clone())
                        .collect()
                };
                if positions.iter().flatten().flatten().any(|v| !v.is_finite()) {
                    return Err(format!("nonfinite skinned geometry {clip} at {time}"));
                }
                let count = body_cloth_intersections(
                    &character.parts[body],
                    &positions[body],
                    &garments
                        .iter()
                        .map(|&i| (&character.parts[i], positions[i].as_slice()))
                        .collect::<Vec<_>>(),
                );
                intersections += count;
                poses.push(json!({"clip":clip,"time_s":time,"body_cloth_intersections":count}));
                Ok(())
            };
            check_pose("rest", 0.)?;
            if let Some(rig) = &character.rig {
                for clip in clips {
                    let duration = rig
                        .clip_duration(clip)
                        .ok_or_else(|| format!("missing clip {clip}"))?;
                    for i in 0..samples {
                        check_pose(clip, duration * i as f32 / samples as f32)?;
                    }
                }
            }
            let warning_count = usize::from(intersections > 0)
                + usize::from(degenerate > 0)
                + usize::from(character.hair_fit.remaining_violations > 0)
                + usize::from(character.rig.is_none() && !clips.is_empty());
            warnings += warning_count;
            Ok(
                json!({"name":name,"ok":true,"warning_count":warning_count,"height_m":character.height,"triangles":character.triangle_count(),"garments":garments.iter().map(|&i|character.parts[i].name.clone()).collect::<Vec<_>>(),"degenerate_triangles":degenerate,"hair_clearance_warnings":character.hair_fit.remaining_violations,"body_cloth_intersections":intersections,"poses":poses}),
            )
        })();
        match result {
            Ok(case) => cases.push(case),
            Err(error) => {
                failures += 1;
                cases.push(json!({"name":name,"ok":false,"error":error}));
            }
        }
    }
    Ok(
        json!({"version":1,"wardrobe":profile.name,"ok":failures==0,"failed":failures,"warning_count":warnings,"cases":cases,"method":"Sampled triangle intersections against visible body geometry; coplanar contacts ignored. Hair clearance warnings are bounded-fit diagnostics. Review warnings visually; this is not continuous collision detection or cloth simulation."}),
    )
}
#[derive(Clone, Copy)]
struct Tri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
}
impl Tri {
    fn min(self) -> Vec3 {
        self.a.min(self.b).min(self.c)
    }
    fn max(self) -> Vec3 {
        self.a.max(self.b).max(self.c)
    }
}
const GRID: f32 = 0.08;
fn key(p: Vec3) -> [i32; 3] {
    [
        (p.x / GRID).floor() as i32,
        (p.y / GRID).floor() as i32,
        (p.z / GRID).floor() as i32,
    ]
}
fn cells(tri: Tri) -> Vec<[i32; 3]> {
    let lo = key(tri.min());
    let hi = key(tri.max());
    let count = (0..3)
        .map(|i| hi[i] as i64 - lo[i] as i64 + 1)
        .fold(1i64, i64::saturating_mul);
    if !(1..=4096).contains(&count) {
        return vec![];
    }
    let mut out = vec![];
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                out.push([x, y, z]);
            }
        }
    }
    out
}
fn triangles(part: &NativePart, positions: &[[f32; 3]]) -> Vec<Tri> {
    part.indices
        .chunks_exact(3)
        .map(|t| Tri {
            a: positions[t[0] as usize].into(),
            b: positions[t[1] as usize].into(),
            c: positions[t[2] as usize].into(),
        })
        .collect()
}
fn segment_hits(a: Vec3, b: Vec3, tri: Tri) -> bool {
    let direction = b - a;
    let edge1 = tri.b - tri.a;
    let edge2 = tri.c - tri.a;
    let p = direction.cross(edge2);
    let det = edge1.dot(p);
    if det.abs() < 1e-10 {
        return false;
    }
    let inv = 1. / det;
    let delta = a - tri.a;
    let u = delta.dot(p) * inv;
    if !(1e-5..=1. - 1e-5).contains(&u) {
        return false;
    }
    let q = delta.cross(edge1);
    let v = direction.dot(q) * inv;
    if v < 1e-5 || u + v > 1. - 1e-5 {
        return false;
    }
    let t = edge2.dot(q) * inv;
    (1e-5..=1. - 1e-5).contains(&t)
}
fn intersect(a: Tri, b: Tri) -> bool {
    if a.min().cmpgt(b.max()).any() || b.min().cmpgt(a.max()).any() {
        return false;
    }
    [(a.a, a.b), (a.b, a.c), (a.c, a.a)]
        .iter()
        .any(|&(p, q)| segment_hits(p, q, b))
        || [(b.a, b.b), (b.b, b.c), (b.c, b.a)]
            .iter()
            .any(|&(p, q)| segment_hits(p, q, a))
}
fn body_cloth_intersections(
    body: &NativePart,
    body_positions: &[[f32; 3]],
    clothes: &[(&NativePart, &[[f32; 3]])],
) -> usize {
    let body = triangles(body, body_positions);
    let mut grid = BTreeMap::<[i32; 3], Vec<usize>>::new();
    for (index, tri) in body.iter().enumerate() {
        for cell in cells(*tri) {
            grid.entry(cell).or_default().push(index);
        }
    }
    let mut count = 0;
    for (part, positions) in clothes {
        for tri in triangles(part, positions) {
            let mut candidates = BTreeSet::new();
            for cell in cells(tri) {
                if let Some(indices) = grid.get(&cell) {
                    candidates.extend(indices.iter().copied());
                }
            }
            if candidates.into_iter().any(|i| intersect(tri, body[i])) {
                count += 1;
            }
        }
    }
    count
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wardrobe_does_not_replace_body_or_breast_settings() {
        let mut original = AraRecipe::default();
        original.params["top"] = json!("tank_top");
        let profile = WardrobeProfile::capture(&original, "Training", false).unwrap();
        let mut other = AraRecipe::default();
        other.params["height"] = json!(0.9);
        other.params["gender"] = json!(0.0);
        other.breasts.volume = 1.7;
        profile.apply(&mut other).unwrap();
        assert_eq!(other.params["height"], 0.9);
        assert_eq!(other.params["gender"], 0.0);
        assert_eq!(other.breasts.volume, 1.7);
        assert_eq!(other.params["top"], "tank_top");
    }
    #[test]
    fn wardrobe_rejects_body_keys() {
        let mut profile = WardrobeProfile::capture(&AraRecipe::default(), "Test", false).unwrap();
        profile.params["height"] = json!(0.5);
        assert!(profile.validate().is_err());
    }
    #[test]
    fn wardrobe_without_include_hair_does_not_capture_custom_hair() {
        let mut source = AraRecipe::default();
        source.params["hair"] = json!("long01");
        source.custom_garments.push(CustomGarment {
            id: "test-hair".into(),
            name: "Test hair".into(),
            slot: crate::wardrobe_assets::GarmentSlot::Hair,
            positions_dm: vec![[0., 0., 0.]],
            indices: vec![0, 0, 0],
            uvs: vec![[0., 0.]],
            refs: vec![[0, 0, 0]],
            weights: vec![[1., 0., 0.]],
            reference_points_dm: vec![[0., 0., 0.]],
            material: json!({}),
            hidden_faces: vec![],
        });

        let profile = WardrobeProfile::capture(&source, "Outfit", false).unwrap();
        assert!(profile.params.get("hair").is_none());
        assert!(profile.custom_garments.is_empty());

        let mut destination = AraRecipe::default();
        destination.params["hair"] = json!("short02");
        profile.apply(&mut destination).unwrap();
        assert_eq!(destination.params["hair"], "short02");
        assert!(destination.custom_garments.is_empty());
    }
    #[test]
    fn intersection_check_distinguishes_crossing_from_offset_triangles() {
        let a = Tri {
            a: Vec3::new(-1., 0., -1.),
            b: Vec3::new(1., 0., -1.),
            c: Vec3::new(0., 0., 1.),
        };
        let b = Tri {
            a: Vec3::new(0., -1., 0.),
            b: Vec3::new(0., 1., 0.),
            c: Vec3::new(0.5, 1., 0.),
        };
        assert!(intersect(a, b));
        assert!(!intersect(
            a,
            Tri {
                a: a.a + Vec3::Y,
                b: a.b + Vec3::Y,
                c: a.c + Vec3::Y
            }
        ));
    }
}
