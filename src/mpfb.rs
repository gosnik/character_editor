//! Interoperability with documented MakeHuman/MPFB text formats.
//!
//! This is an independent reader for the published file format, not an import
//! of MPFB's GPL Blender implementation.
use crate::ara::AraRecipe;
use flate2::read::GzDecoder;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, io::Read, path::Path};

fn finite(s: &str, what: &str) -> Result<f64, String> {
    s.parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .ok_or_else(|| format!("invalid {what}: {s}"))
}
fn norm(s: &str) -> String {
    s.rsplit('/')
        .next()
        .unwrap_or(s)
        .split('.')
        .next()
        .unwrap_or(s)
        .to_ascii_lowercase()
        .replace([' ', '-'], "_")
}
fn asset_words(words: &[&str]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let end = if words
        .last()
        .is_some_and(|s| s.len() == 36 && s.bytes().filter(|&c| c == b'-').count() == 4)
    {
        words.len() - 1
    } else {
        words.len()
    };
    if end == 0 {
        None
    } else {
        Some(words[..end].join(" "))
    }
}
fn set_number(recipe: &mut AraRecipe, key: &str, v: f64) -> Result<(), String> {
    if !(0.0..=1.0).contains(&v) {
        return Err(format!("macro {key} outside 0..1"));
    }
    recipe.params[key] = json!(v);
    Ok(())
}
fn macro_key(s: &str) -> Option<&'static str> {
    match s.rsplit('/').next()?.to_ascii_lowercase().as_str() {
        "gender" => Some("gender"),
        "age" => Some("age"),
        "muscle" => Some("muscle"),
        "weight" => Some("weight"),
        "height" => Some("height"),
        "bodyproportions" | "proportions" => Some("proportions"),
        "african" => Some("african"),
        "asian" => Some("asian"),
        "caucasian" => Some("caucasian"),
        "breastsize" | "cupsize" => Some("breastSize"),
        "breastfirmness" | "firmness" => Some("breastFirmness"),
        _ => None,
    }
}
fn slider_target(catalog: &Value, target: &str) -> Option<(String, f64)> {
    let target = target.trim_start_matches('/');
    for g in catalog.get("sliders")?.as_array()? {
        for s in g.get("sliders")?.as_array()? {
            let id = s.get("id")?.as_str()?;
            for pair in s.get("targets")?.as_array()? {
                let p = pair.as_array()?;
                if p.get(0)?.as_str() == Some(target) {
                    return Some((id.into(), -1.));
                }
                if p.get(1)?.as_str() == Some(target) {
                    return Some((id.into(), 1.));
                }
            }
        }
    }
    None
}
fn slider_id(catalog: &Value, id: &str) -> bool {
    catalog
        .get("sliders")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|g| {
            g.get("sliders")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .any(|s| s.get("id").and_then(Value::as_str) == Some(id))
}
fn asset_id(catalog: &Value, field: &str, raw: &str) -> Option<String> {
    let wanted = norm(raw);
    let groups: &[&str] = match field {
        "eyes" => &["eyes"],
        "eyebrows" => &["eyebrows"],
        "eyelashes" => &["eyelashes"],
        "hair" => &["hair"],
        "teeth" => &["teeth"],
        "skin" => &["skins"],
        "outfit" => &["outfits"],
        "shoes" => &["shoes"],
        "hat" => &["hats"],
        _ => &[],
    };
    for group in groups {
        let Some(items) = catalog.get(*group).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            if norm(id) == wanted
                || norm(item.get("name").and_then(Value::as_str).unwrap_or("")) == wanted
            {
                return Some(id.into());
            }
        }
    }
    // The editor catalog stores selectable assets as fields/choices rather
    // than the Ara public catalog's top-level collections.
    if let Some(fields) = catalog.get("fields").and_then(Value::as_array) {
        for f in fields {
            if f.get("id").and_then(Value::as_str) != Some(field) {
                continue;
            }
            let Some(choices) = f.get("choices").and_then(Value::as_array) else {
                continue;
            };
            for item in choices {
                let Some(id) = item.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if norm(id) == wanted
                    || norm(item.get("name").and_then(Value::as_str).unwrap_or("")) == wanted
                {
                    return Some(id.into());
                }
            }
        }
    }
    None
}
/// Imports the documented MHM directives that Ara can represent.  Unsupported
/// geometry-changing directives return an error instead of losing shape data.
pub fn import_mhm(text: &str, base: &AraRecipe, catalog: &Value) -> Result<AraRecipe, String> {
    // This snapshot is authoritative for a Human Studio round-trip.  It is
    // recognized first, because standard MHM cannot name arbitrary native
    // custom-target modifiers that the extension legitimately preserves.
    let extensions: Vec<_> = text
        .lines()
        .enumerate()
        .filter_map(|(n, raw)| {
            raw.trim()
                .strip_prefix("# human-studio ")
                .map(|json| (n + 1, json))
        })
        .collect();
    if extensions.len() > 1 {
        return Err("MHM has duplicate human-studio extensions".into());
    }
    if let Some((line, json)) = extensions.first() {
        let recipe: AraRecipe = serde_json::from_str(json)
            .map_err(|e| format!("MHM line {line}: invalid human-studio extension: {e}"))?;
        recipe
            .validate()
            .map_err(|e| format!("MHM line {line}: invalid human-studio extension: {e}"))?;
        return Ok(recipe);
    }
    let mut out = base.clone();
    out.validate()?;
    let mut clothes = Vec::new();
    for (line_no, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let key = it.next().unwrap();
        let rest: Vec<_> = it.collect();
        let err = |s: String| format!("MHM line {}: {s}", line_no + 1);
        match key {
            "version" | "camera" | "clothesHideFaces" | "subdivide" => {}
            "name" => {
                let name = line.strip_prefix("name").unwrap().trim();
                if name.len() > 256 {
                    return Err(err("name too long".into()));
                }
                out.params["name"] = json!(name)
            }
            "modifier" => {
                if rest.len() != 2 {
                    return Err(err("modifier requires path and weight".into()));
                }
                let v = finite(rest[1], "modifier weight").map_err(err)?;
                if let Some(k) = macro_key(rest[0]) {
                    set_number(&mut out, k, v).map_err(err)?
                } else {
                    let path = rest[0].trim_start_matches('/');
                    if slider_id(catalog, path) {
                        out.params["modifiers"][path] = json!(v.clamp(-1., 1.))
                    } else if let Some((id, sign)) = slider_target(catalog, path) {
                        let w = (v * sign).clamp(-1., 1.);
                        out.params["modifiers"][id] = json!(w)
                    } else if path.contains('|') {
                        let choices: Vec<_> = path.split('|').collect();
                        let chosen = if v < 0. {
                            choices.first()
                        } else {
                            choices.last()
                        }
                        .ok_or_else(|| err("empty opposing modifier".into()))?;
                        let Some((id, sign)) = slider_target(catalog, chosen) else {
                            return Err(err(format!("unsupported geometry modifier {path}")));
                        };
                        out.params["modifiers"][id] = json!((v.abs() * sign).clamp(-1., 1.));
                    } else {
                        return Err(err(format!("unsupported geometry modifier {path}")));
                    }
                }
            }
            "eyes" => {
                let raw = asset_words(&rest).ok_or_else(|| err("asset missing".into()))?;
                // Ara has a fixed eye mesh; its selectable `eyes` field is an
                // iris material, whereas MHM's `eyes` directive is a MHCLO.
                if norm(&raw) != "high_poly" {
                    return Err(err(format!("unsupported eyes mesh {raw}")));
                }
            }
            "eyebrows" | "eyelashes" | "hair" | "teeth" => {
                let raw = asset_words(&rest).ok_or_else(|| err("asset missing".into()))?;
                let Some(id) = asset_id(catalog, key, &raw) else {
                    return Err(err(format!("unsupported {key} asset {raw}")));
                };
                out.params[key] = json!(id)
            }
            "tongue" => {
                let raw = asset_words(&rest).ok_or_else(|| err("asset missing".into()))?;
                out.params["tongue"] = json!(norm(&raw) != "none")
            }
            "skinMaterial" => {
                let raw = asset_words(&rest).ok_or_else(|| err("skin material missing".into()))?;
                if let Some(id) = asset_id(catalog, "skin", &raw) {
                    out.params["skin"] = json!(id)
                } else {
                    return Err(err(format!("unsupported skin material {raw}")));
                }
            }
            "skeleton" => {
                let s = rest
                    .first()
                    .ok_or_else(|| err("skeleton missing".into()))?
                    .to_ascii_lowercase();
                out.params["rig"] = json!(if s.contains("game") {
                    "game"
                } else if s.contains("cmu") || s.contains("toes") {
                    "full"
                } else {
                    "game_lite"
                })
            }
            "clothes" => {
                let raw = asset_words(&rest).ok_or_else(|| err("clothing asset missing".into()))?;
                clothes.push(raw)
            }
            "proxy" => return Err(err("proxy topology cannot be represented by Ara".into())),
            _ => {}
        }
    }
    for raw in clothes {
        if let Some(id) = asset_id(catalog, "outfit", &raw) {
            out.params["outfit"] = json!(id)
        } else if let Some(id) = asset_id(catalog, "shoes", &raw) {
            out.params["shoes"] = json!(id)
        } else if let Some(id) = asset_id(catalog, "hat", &raw) {
            out.params["hat"] = json!(id)
        } else {
            return Err(format!("unsupported clothing geometry {raw}"));
        }
    }
    out.validate()?;
    Ok(out)
}
/// Writes standard MHM directives plus a comment extension preserving the full
/// Ara recipe.  MakeHuman/MPFB ignore the comment and consume the standard subset.
pub fn export_mhm(recipe: &AraRecipe) -> String {
    let p = &recipe.params;
    let n = |k: &str| p.get(k).and_then(Value::as_f64).unwrap_or(0.);
    let mut out = format!(
        "version v1.2.0\nname {}\n",
        p.get("name").and_then(Value::as_str).unwrap_or("character")
    );
    for (mh, k) in [
        ("macrodetails/Gender", "gender"),
        ("macrodetails/Age", "age"),
        ("macrodetails-universal/Muscle", "muscle"),
        ("macrodetails-universal/Weight", "weight"),
        ("macrodetails-height/Height", "height"),
        ("macrodetails-proportions/BodyProportions", "proportions"),
        ("macrodetails/African", "african"),
        ("macrodetails/Asian", "asian"),
        ("macrodetails/Caucasian", "caucasian"),
        ("breast/BreastSize", "breastSize"),
        ("breast/BreastFirmness", "breastFirmness"),
    ] {
        out.push_str(&format!("modifier {mh} {:0.6}\n", n(k)))
    }
    if let Some(m) = p.get("modifiers").and_then(Value::as_object) {
        for (id, v) in m {
            if let Some(w) = v.as_f64().filter(|x| *x != 0.) {
                out.push_str(&format!("modifier {id} {:0.6}\n", w))
            }
        }
    }
    // MHM `eyes` is a mesh asset, not Ara's iris-material choice.  The base
    // high-poly eye mesh is the compatible standard representation.
    out.push_str("eyes High-poly\n");
    for (field, key) in [
        ("eyebrows", "eyebrows"),
        ("eyelashes", "eyelashes"),
        ("hair", "hair"),
        ("teeth", "teeth"),
    ] {
        if let Some(x) = p.get(key).and_then(Value::as_str).filter(|x| *x != "none") {
            out.push_str(&format!("{field} {x}\n"))
        }
    }
    let skin = p.get("skin").and_then(Value::as_str).unwrap_or("auto");
    if skin != "auto" && skin != "none" {
        out.push_str(&format!("skinMaterial skins/{skin}/{skin}.mhmat\n"));
    }
    let rig = match p.get("rig").and_then(Value::as_str).unwrap_or("game") {
        "game" => "game.mhskel",
        "full" => "cmu_mb.mhskel",
        _ => "default.mhskel",
    };
    out.push_str(&format!("skeleton {rig}\n"));
    out.push_str(
        if p.get("tongue").and_then(Value::as_bool).unwrap_or(true) {
            "tongue tongue\n"
        } else {
            "tongue none\n"
        },
    );
    if let Some(x) = p
        .get("outfit")
        .and_then(Value::as_str)
        .filter(|x| *x != "none")
    {
        out.push_str(&format!("clothes {x}\n"))
    }
    for key in ["shoes", "hat"] {
        if let Some(x) = p.get(key).and_then(Value::as_str).filter(|x| *x != "none") {
            out.push_str(&format!("clothes {x}\n"));
        }
    }
    // No MakeHuman directive represents procedural garments, colors, animation
    // settings, expression options, custom targets, or Ara export settings.
    out.push_str("# human-studio ");
    out.push_str(&serde_json::to_string(recipe).expect("AraRecipe serializes"));
    out.push('\n');
    out
}

/// Parses target data into MakeHuman-space `[x, y, z]` decimetre deltas.
///
/// MPFB labels its fields `x z neg_y` because Blender is Z-up. The source
/// target bytes are already MakeHuman model-axis values; this Y-up native
/// pipeline preserves them without an axis or sign conversion.
pub fn parse_custom_target(
    text: &str,
    vertex_count: usize,
) -> Result<Vec<(usize, [f32; 3])>, String> {
    let mut out = BTreeMap::new();
    for (line_no, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('"') {
            continue;
        }
        if line.contains('\t') || line.contains("  ") {
            return Err(format!(
                "target line {}: fields must use one space",
                line_no + 1
            ));
        }
        let f: Vec<_> = line.split(' ').collect();
        if f.len() < 4 {
            return Err(format!("target line {}: expected four fields", line_no + 1));
        }
        let i = f[0]
            .parse::<usize>()
            .map_err(|_| format!("target line {}: invalid vertex", line_no + 1))?;
        if i >= vertex_count {
            return Err(format!("target line {}: vertex out of bounds", line_no + 1));
        }
        let x = finite(f[1], "x").map_err(|e| format!("target line {}: {e}", line_no + 1))? as f32;
        let y = finite(f[2], "y").map_err(|e| format!("target line {}: {e}", line_no + 1))? as f32;
        let z = finite(f[3], "z").map_err(|e| format!("target line {}: {e}", line_no + 1))? as f32;
        if !x.is_finite() || !y.is_finite() || !z.is_finite() {
            return Err(format!(
                "target line {}: delta exceeds finite f32 range",
                line_no + 1
            ));
        }
        if out.insert(i, [x, y, z]).is_some() {
            return Err(format!("target line {}: duplicate vertex {i}", line_no + 1));
        }
    }
    Ok(out.into_iter().collect())
}
pub fn load_target(path: impl AsRef<Path>, count: usize) -> Result<Vec<(usize, [f32; 3])>, String> {
    let path = path.as_ref();
    const LIMIT: u64 = 64 * 1024 * 1024;
    let file = fs::File::open(path).map_err(|e| format!("target: {e}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > LIMIT {
        return Err("target exceeds 64 MiB input limit".into());
    }
    let mut raw = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut raw)
        .map_err(|e| format!("target: {e}"))?;
    if raw.len() as u64 > LIMIT {
        return Err("target exceeds 64 MiB input limit".into());
    }
    let text = if path.extension().and_then(|x| x.to_str()) == Some("gz") {
        let mut d = GzDecoder::new(&raw[..]);
        let mut o = Vec::new();
        d.by_ref()
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut o)
            .map_err(|e| format!("target gzip: {e}"))?;
        if o.len() > 64 * 1024 * 1024 {
            return Err("target gzip exceeds limit".into());
        }
        String::from_utf8(o).map_err(|_| "target is not UTF-8".to_string())?
    } else {
        String::from_utf8(raw).map_err(|_| "target is not UTF-8".to_string())?
    };
    parse_custom_target(&text, count)
}
/// Mirrors sparse deltas across X by nearest reflected base vertex within tolerance.
pub fn mirror_target(
    deltas: &[(usize, [f32; 3])],
    basecoords: &[[f32; 3]],
    tolerance: f32,
) -> Result<Vec<(usize, [f32; 3])>, String> {
    if !tolerance.is_finite() || tolerance < 0. {
        return Err("invalid mirror tolerance".into());
    }
    let mut out = BTreeMap::new();
    for &(i, d) in deltas {
        let p = *basecoords.get(i).ok_or("delta vertex out of bounds")?;
        let mut best = None;
        for (j, q) in basecoords.iter().enumerate() {
            let dx = q[0] + p[0];
            let dy = q[1] - p[1];
            let dz = q[2] - p[2];
            let dist = dx * dx + dy * dy + dz * dz;
            if best.is_none_or(|(_, b): (usize, f32)| dist < b) {
                best = Some((j, dist))
            }
        }
        let (j, dist) = best.ok_or("empty base mesh")?;
        if dist > tolerance * tolerance {
            return Err(format!("no mirror vertex within tolerance for {i}"));
        }
        out.insert(j, [-d[0], d[1], d[2]]);
    }
    Ok(out.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_axes_and_comments_parse() {
        assert_eq!(
            parse_custom_target("# h\n1 0.5 0.25 -0.75 trailing", 3).unwrap(),
            vec![(1, [0.5, 0.25, -0.75])]
        );
        assert!(parse_custom_target("1  0.2 0.3 0.4", 3).is_err());
        assert!(parse_custom_target("1 0.2 0.3 0.4\n1 0.1 0.2 0.3", 3).is_err())
    }
    #[test]
    fn mirror_reflects_positions_and_x_delta() {
        let d = mirror_target(&[(0, [1., 2., 3.])], &[[-1., 0., 0.], [1., 0., 0.]], 0.001).unwrap();
        assert_eq!(d, vec![(1, [-1., 2., 3.])]);
    }
    #[test]
    fn full_recipe_roundtrips_through_native_comment_extension() {
        let mut r = AraRecipe::default();
        r.params["skin"] = json!("young_caucasian_female");
        r.params["rig"] = json!("full");
        r.params["tongue"] = json!(false);
        r.params["outfit"] = json!("male_casualsuit01");
        r.params["shoes"] = json!("shoes02");
        r.params["hat"] = json!("fedora01");
        // Native-only/procedural fields are deliberately retained by the
        // comment extension rather than falsely represented as MHM geometry.
        r.params["top"] = json!("tank_top");
        r.params["hairColor"] = json!("#2a1e16");
        r.params["accessories"] = json!(["watch_gold"]);
        r.params["modifiers"]["custom/user_target"] = json!(0.42);
        r.preview_clip = "walk".into();
        r.export.format = "obj".into();
        r.validate().unwrap();
        let text = export_mhm(&r);
        assert!(
            text.contains("skinMaterial skins/young_caucasian_female/young_caucasian_female.mhmat")
        );
        assert!(text.contains("skeleton cmu_mb.mhskel"));
        assert!(text.contains("tongue none"));
        assert!(text.contains("eyes High-poly"));
        assert!(text.contains("clothes shoes02"));
        assert!(text.contains("clothes fedora01"));
        let x = import_mhm(&text, &AraRecipe::default(), &crate::ara::native_catalog()).unwrap();
        assert_eq!(x, r);
    }
    #[test]
    fn documented_mesh_directives_resolve_against_editor_catalog() {
        let text = "eyes High-poly\neyebrows Eyebrow001\neyelashes Eyelashes01\nhair Short02\nteeth Teeth base\nclothes shoes02\nclothes fedora01\n";
        let r = import_mhm(text, &AraRecipe::default(), &crate::ara::native_catalog()).unwrap();
        assert_eq!(r.params["eyebrows"], "eyebrow001");
        assert_eq!(r.params["eyelashes"], "eyelashes01");
        assert_eq!(r.params["hair"], "short02");
        assert_eq!(r.params["teeth"], "teeth_base");
        assert_eq!(r.params["shoes"], "shoes02");
        assert_eq!(r.params["hat"], "fedora01");
    }
    #[test]
    fn invalid_native_extension_is_rejected() {
        let text = "# human-studio {\"params\":{},\"preview_clip\":\"\",\"export\":{\"mode\":\"static\",\"format\":\"obj\",\"clips\":[],\"root_motion\":false}}\n";
        assert!(import_mhm(text, &AraRecipe::default(), &crate::ara::native_catalog()).is_err());
    }
}
