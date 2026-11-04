//! Serialization and mutation helpers used by the headless CLI.

use crate::{
    ara::{AraRecipe, native_catalog, valid_color},
    model::{CharacterRecipe, atomic_write},
    mpfb::{export_mhm, import_mhm},
};
use serde_json::{Map, Value};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read, Write},
    path::Path,
};

const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;

/// Loads a project, native recipe, reference parameter object, or MHM file.
pub fn load(path: &Path, assets: &Path) -> Result<AraRecipe, String> {
    let text = read_bounded(path)?;
    if path
        .extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| x.eq_ignore_ascii_case("mhm"))
    {
        return import_mhm(&text, &AraRecipe::default(), &catalog(assets)?)
            .and_then(strict_validate);
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("invalid JSON: {e}"))?;
    let recipe = if value.get("version").is_some()
        || value.get("customization").is_some()
        || value.get("native_draft").is_some()
    {
        let project: CharacterRecipe =
            serde_json::from_value(value).map_err(|e| format!("invalid project: {e}"))?;
        validate_project_metadata(&project)?;
        project
            .customization
            .or(project.native_draft)
            .ok_or("project has no native customization")?
    } else if value.get("params").is_some() {
        serde_json::from_value(value).map_err(|e| format!("invalid native recipe: {e}"))?
    } else {
        reference_recipe(value)?
    };
    strict_validate(recipe)
}

/// Saves a recipe in native, project, or MHM form. `-` writes to stdout.
pub fn save(recipe: &AraRecipe, path: &Path, format: &str) -> Result<(), String> {
    validate(recipe)?;
    let bytes = match format.to_ascii_lowercase().as_str() {
        "native" => serde_json::to_vec_pretty(recipe).map_err(|e| e.to_string())?,
        "project" => {
            let mut project = CharacterRecipe {
                name: recipe
                    .params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("New character")
                    .into(),
                customization: Some(recipe.clone()),
                ..Default::default()
            };
            // Project construction above is intentionally explicit: do not retain a basic-editor draft.
            project.native_draft = None;
            serde_json::to_vec_pretty(&project).map_err(|e| e.to_string())?
        }
        "mhm" => export_mhm(recipe).into_bytes(),
        _ => return Err(format!("unsupported save format: {format}")),
    };
    if path == Path::new("-") {
        let mut stdout = io::stdout().lock();
        stdout.write_all(&bytes).map_err(|e| e.to_string())?;
        if !bytes.ends_with(b"\n") {
            stdout.write_all(b"\n").map_err(|e| e.to_string())?;
        }
        stdout.flush().map_err(|e| e.to_string())
    } else {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        atomic_write(path, &bytes).map_err(|e| e.to_string())
    }
}

/// Applies one `NAME=JSON_OR_STRING` assignment.
pub fn set(recipe: &mut AraRecipe, assignment: &str) -> Result<(), String> {
    let (name, raw) = assignment
        .split_once('=')
        .ok_or("assignment must be NAME=VALUE")?;
    if name.is_empty() {
        return Err("assignment name is empty".into());
    }
    let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
    let mut root = serde_json::to_value(&*recipe).map_err(|e| e.to_string())?;
    let parts = assignment_path(name)?;
    set_path(&mut root, &parts, value)?;
    let changed: AraRecipe =
        serde_json::from_value(root).map_err(|e| format!("invalid assignment {name}: {e}"))?;
    validate(&changed)?;
    *recipe = changed;
    Ok(())
}

/// Validates the native recipe plus the strict CLI schema guards.
pub fn validate(recipe: &AraRecipe) -> Result<(), String> {
    strict_validate(recipe.clone()).map(|_| ())
}

fn strict_validate(recipe: AraRecipe) -> Result<AraRecipe, String> {
    recipe.validate()?;
    let schema = native_catalog();
    strict_against_catalog(&recipe, &schema)?;
    Ok(recipe)
}

fn reference_recipe(value: Value) -> Result<AraRecipe, String> {
    let supplied = value
        .as_object()
        .ok_or("reference parameters must be an object")?;
    let mut recipe = AraRecipe::default();
    let params = recipe
        .params
        .as_object_mut()
        .expect("default params are object");
    for (key, value) in supplied {
        if !params.contains_key(key) {
            return Err(format!("unknown parameter: {key}"));
        }
        params.insert(key.clone(), value.clone());
    }
    Ok(recipe)
}

fn validate_project_metadata(project: &CharacterRecipe) -> Result<(), String> {
    if project.version != 1 {
        return Err(format!("unsupported project version: {}", project.version));
    }
    if project.name.len() > 256 {
        return Err("project name is too long".into());
    }
    if project
        .skin_color
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err("invalid project skin color".into());
    }
    if project
        .weights
        .values()
        .any(|v| !v.is_finite() || !(-1.0..=1.0).contains(v))
    {
        return Err("invalid basic-editor weight".into());
    }
    // Legacy basic-editor weights require the MakeHuman target library to
    // validate their IDs. They are intentionally not retained by this native
    // loader; their finite values and ranges have already been checked above.
    Ok(())
}

fn catalog(assets: &Path) -> Result<Value, String> {
    let candidates = [
        assets.join("editor_catalog.json"),
        assets.join("native/editor_catalog.json"),
    ];
    let path = candidates
        .iter()
        .find(|p| p.is_file())
        .ok_or_else(|| format!("native editor catalog not found below {}", assets.display()))?;
    let text = read_bounded(path)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid catalog {}: {e}", path.display()))?;
    if !value.get("defaults").is_some_and(Value::is_object)
        || !value.get("sliders").is_some_and(Value::is_array)
    {
        return Err(format!("invalid native editor catalog: {}", path.display()));
    }
    Ok(value)
}

fn read_bounded(path: &Path) -> Result<String, String> {
    let mut bytes = Vec::new();
    if path == Path::new("-") {
        io::stdin()
            .lock()
            .take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
    } else {
        let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
        if metadata.len() > MAX_INPUT_BYTES {
            return Err("input exceeds 64 MiB limit".into());
        }
        bytes = fs::read(path).map_err(|e| e.to_string())?;
    }
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("input exceeds 64 MiB limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "input is not UTF-8".into())
}

fn strict_against_catalog(recipe: &AraRecipe, schema: &Value) -> Result<(), String> {
    let defaults = schema["defaults"]
        .as_object()
        .ok_or("invalid schema defaults")?;
    let params = recipe
        .params
        .as_object()
        .ok_or("customization parameters must be an object")?;
    if let Some(key) = params.keys().find(|key| !defaults.contains_key(*key)) {
        return Err(format!("unknown parameter: {key}"));
    }
    let modifier_ids = slider_ids(schema)?;
    let modifiers = params
        .get("modifiers")
        .and_then(Value::as_object)
        .ok_or("invalid detail modifiers")?;
    if let Some(id) = modifiers.keys().find(|id| !modifier_ids.contains(*id)) {
        return Err(format!("unknown modifier: {id}"));
    }
    let styles = params
        .get("garmentStyles")
        .and_then(Value::as_object)
        .ok_or("invalid garment styles")?;
    let garments = garment_ids(schema);
    for (id, style) in styles {
        if !garments.contains(id) {
            return Err(format!("unknown garment style: {id}"));
        }
        let style = style
            .as_object()
            .ok_or_else(|| format!("invalid garment style: {id}"))?;
        if let Some(field) = style
            .keys()
            .find(|field| !matches!(field.as_str(), "color" | "color2" | "pattern"))
        {
            return Err(format!("unknown garment style field: {id}.{field}"));
        }
        for color in ["color", "color2"] {
            if let Some(value) = style.get(color) {
                if !value.as_str().is_some_and(valid_color) {
                    return Err(format!("invalid garment style color: {id}.{color}"));
                }
            }
        }
        if let Some(pattern) = style.get("pattern") {
            let pattern = pattern
                .as_str()
                .ok_or_else(|| format!("invalid garment style pattern: {id}.pattern"))?;
            let allowed = ["garments", "outfit_sets"]
                .into_iter()
                .flat_map(|key| schema[key].as_array().into_iter().flatten())
                .find(|item| item["id"].as_str() == Some(id.as_str()))
                .and_then(|item| item["patterns"].as_array())
                .is_some_and(|patterns| patterns.iter().any(|item| item.as_str() == Some(pattern)))
                || schema["presets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|preset| {
                        preset["params"]["garmentStyles"][id]["pattern"].as_str() == Some(pattern)
                    });
            if !allowed {
                return Err(format!("invalid garment style pattern: {id}.pattern"));
            }
        }
    }
    Ok(())
}

fn slider_ids(schema: &Value) -> Result<BTreeSet<String>, String> {
    let mut ids = BTreeSet::new();
    for group in schema["sliders"]
        .as_array()
        .ok_or("invalid schema sliders")?
    {
        for slider in group["sliders"]
            .as_array()
            .ok_or("invalid schema slider group")?
        {
            let id = slider["id"].as_str().ok_or("invalid schema slider")?;
            ids.insert(id.into());
        }
    }
    Ok(ids)
}

fn garment_ids(schema: &Value) -> BTreeSet<String> {
    let mut ids: BTreeSet<String> = ["garments", "outfit_sets"]
        .into_iter()
        .flat_map(|key| schema[key].as_array().into_iter().flatten())
        .filter_map(|item| item["id"].as_str())
        .map(str::to_owned)
        .collect();
    for preset in schema["presets"].as_array().into_iter().flatten() {
        if let Some(styles) = preset["params"]["garmentStyles"].as_object() {
            ids.extend(styles.keys().cloned());
        }
    }
    ids
}

fn assignment_path(name: &str) -> Result<Vec<String>, String> {
    if name == "preview_clip" {
        return Ok(vec![name.into()]);
    }
    if name.starts_with('/') {
        let parts: Vec<String> = name
            .split('/')
            .skip(1)
            .map(|p| p.replace("~1", "/").replace("~0", "~"))
            .collect();
        if parts.is_empty()
            || !matches!(
                parts.first().map(String::as_str),
                Some("params" | "breasts" | "cartoon" | "export" | "preview_clip")
            )
        {
            return Err(format!("unsupported assignment path: {name}"));
        }
        return Ok(parts);
    }
    if let Some(field) = name.strip_prefix("cartoon.") {
        return one_field("cartoon", field);
    }
    if let Some(field) = name.strip_prefix("breasts.") {
        return one_field("breasts", field);
    }
    if let Some(field) = name.strip_prefix("export.") {
        return one_field("export", field);
    }
    if let Some(field) = name.strip_prefix("params.") {
        let parts: Vec<_> = field.split('.').collect();
        if parts.is_empty() || parts.iter().any(|x| x.is_empty()) {
            return Err("invalid parameter assignment".into());
        }
        let mut out = vec!["params".into()];
        out.extend(parts.into_iter().map(str::to_owned));
        return Ok(out);
    }
    // `--style` uses this compact spelling while JSON pointers retain the
    // serialized `params/garmentStyles` path.
    if let Some(field) = name.strip_prefix("garmentStyles.") {
        let parts: Vec<_> = field.split('.').collect();
        if parts.iter().any(|x| x.is_empty()) {
            return Err("invalid garment style assignment".into());
        }
        let mut out = vec!["params".into(), "garmentStyles".into()];
        out.extend(parts.into_iter().map(str::to_owned));
        return Ok(out);
    }
    if native_catalog()["defaults"].get(name).is_some() {
        return Ok(vec!["params".into(), name.into()]);
    }
    Err(format!("unknown assignment field: {name}"))
}

fn one_field(parent: &str, field: &str) -> Result<Vec<String>, String> {
    if field.is_empty() || field.contains('.') {
        Err(format!("invalid {parent} field"))
    } else {
        Ok(vec![parent.into(), field.into()])
    }
}

fn set_path(root: &mut Value, parts: &[String], value: Value) -> Result<(), String> {
    let schema = native_catalog();
    match parts {
        [parent, field] if parent == "params" => {
            if schema["defaults"].get(field).is_none() {
                return Err(format!("unknown parameter: {field}"));
            }
        }
        [parent, field] if parent == "cartoon" => {
            if !["enabled", "strength"].contains(&field.as_str()) {
                return Err(format!("unknown cartoon field: {field}"));
            }
        }
        [parent, field] if parent == "breasts" => {
            if ![
                "volume",
                "projection",
                "lift",
                "separation",
                "dynamics",
                "stiffness",
                "damping",
                "motion_strength",
            ]
            .contains(&field.as_str())
            {
                return Err(format!("unknown breasts field: {field}"));
            }
        }
        [parent, field] if parent == "export" => {
            if !["mode", "format", "clips", "root_motion"].contains(&field.as_str()) {
                return Err(format!("unknown export field: {field}"));
            }
        }
        [field] if field == "preview_clip" => {}
        [parent, map, id] if parent == "params" && map == "modifiers" => {
            if !slider_ids(&schema)?.contains(id) {
                return Err(format!("unknown modifier: {id}"));
            }
        }
        [parent, map, id, field] if parent == "params" && map == "garmentStyles" => {
            if !garment_ids(&schema).contains(id) {
                return Err(format!("unknown garment style: {id}"));
            }
            if !["color", "color2", "pattern"].contains(&field.as_str()) {
                return Err(format!("unknown garment style field: {id}.{field}"));
            }
        }
        _ => {
            return Err(
                "assignment may only address known fields, modifiers, or garment styles".into(),
            );
        }
    }
    let mut cursor = root;
    for key in &parts[..parts.len() - 1] {
        cursor = cursor
            .as_object_mut()
            .ok_or("assignment target is not an object")?
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    cursor
        .as_object_mut()
        .ok_or("assignment target is not an object")?
        .insert(parts.last().expect("nonempty path").clone(), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_catalog_presets_pass_strict_validation() {
        let catalog = native_catalog();
        for preset in catalog["presets"].as_array().expect("catalog presets") {
            let recipe = reference_recipe(preset["params"].clone()).expect("reference recipe");
            validate(&recipe).unwrap_or_else(|error| {
                panic!("preset {:?}: {error}", preset["name"]);
            });
        }
    }
}
