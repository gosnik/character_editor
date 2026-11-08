//! Validate a real catalog item through the editor's discovery/selection path.
use human_editor::{
    ara::AraRecipe, native_character::NativeCharacter, native_runtime::NativeAssets,
};
use serde_json::json;
use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        return Err("usage: catalog_garment_smoke NATIVE_ASSETS GARMENT_ID OUTPUT.json".into());
    }
    let assets = NativeAssets::load(&args[1])?;
    let entry = assets
        .1
        .entries
        .iter()
        .find(|e| e.id == args[2])
        .ok_or("catalog item absent")?;
    let garment = entry.garment().ok_or("catalog item invalid")?;
    let mut cases = Vec::new();
    for gender in [0.0, 1.0] {
        let mut recipe = AraRecipe::default();
        recipe.cartoon.enabled = true;
        recipe.params["gender"] = json!(gender);
        recipe.params["detail"] = json!("medium");
        recipe.select_custom_garment(garment)?;
        let character = NativeCharacter::build(&assets.0, &recipe)?;
        let part = character
            .parts
            .iter()
            .find(|p| p.name == format!("custom:{}", garment.id))
            .ok_or("selected garment missing from built character")?;
        if part.positions.iter().flatten().any(|v| !v.is_finite()) {
            return Err("selected garment has nonfinite positions".into());
        }
        cases.push(
            json!({"gender":gender,"vertices":part.positions.len(),"triangles":part.indices.len()/3,
            "selected_id":garment.id,"cartoon":recipe.cartoon.enabled}),
        );
    }
    fs::write(
        &args[3],
        serde_json::to_vec_pretty(&json!({
        "schema":"catalog-garment-smoke/v1","catalog_path":entry.path,"name":entry.name,
        "catalog_entries":assets.1.entries.len(),"cases":cases,"passed":true,
        "valid_catalog_entries":assets.1.entries.iter().filter(|e| e.garment().is_some()).count(),
        "catalog_errors":assets.1.entries.iter().filter_map(|e| e.error.as_ref().map(|error|
            json!({"path":e.path,"error":error}))).collect::<Vec<_>>(),
            "scope":"Actual editor asset discovery, selection and character assembly; not a visual or full pose review."
        }))?,
    )?;
    Ok(())
}
