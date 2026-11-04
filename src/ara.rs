//! Native customization schema and portable recipes imported from Ara's MIT reference.
//! No JavaScript interpreter, subprocess, web service, or authoring runtime is used.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Cartoon hm08: cartoon proportions added on top of the MakeHuman body
/// (`assets/native/cartoon_hm08/cartoon.pack`); `strength` is the Cartoon amount.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CartoonSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cartoon_strength")]
    pub strength: f32,
    /// Accepted and ignored: recipes from when a separate Meshy-surface base existed.
    #[serde(default, skip_serializing)]
    base: Option<String>,
}
fn default_cartoon_strength() -> f32 {
    1.
}
impl Default for CartoonSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: default_cartoon_strength(),
            base: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AraRecipe {
    #[serde(default)]
    pub cartoon: CartoonSettings,
    #[serde(default)]
    pub custom_garments: Vec<crate::wardrobe_assets::CustomGarment>,
    #[serde(default)]
    pub breasts: crate::native_breast::BreastSettings,
    #[serde(default)]
    pub custom_targets: Vec<CustomTarget>,
    pub params: Value,
    pub preview_clip: String,
    pub export: ExportOptions,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExportOptions {
    pub mode: String,
    pub format: String,
    pub clips: Vec<String>,
    pub root_motion: bool,
}
impl Default for AraRecipe {
    fn default() -> Self {
        let catalog = native_catalog();
        Self {
            cartoon: Default::default(),
            breasts: Default::default(),
            custom_garments: vec![],
            custom_targets: vec![],
            params: catalog["defaults"].clone(),
            preview_clip: "idle".into(),
            export: ExportOptions {
                mode: "animated".into(),
                format: "glb".into(),
                clips: catalog["clips"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|c| c["id"].as_str().map(str::to_string))
                    .collect(),
                root_motion: false,
            },
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CustomTarget {
    pub name: String,
    pub weight: f32,
    pub deltas: Vec<(usize, [f32; 3])>,
}
pub fn native_catalog() -> Value {
    serde_json::from_str(include_str!("../assets/native/editor_catalog.json"))
        .expect("bundled customization schema")
}
/// Control metadata plus animation libraries installed in this asset directory.
pub fn native_catalog_with_assets(root: &std::path::Path) -> Value {
    let mut catalog = native_catalog();
    if let Ok(imported) = crate::native_animation::AnimationLibraryCatalog::load(root) {
        let clips = catalog["clips"].as_array_mut().unwrap();
        for clip in imported.clips {
            clips.push(json!({"id":clip.id,"name":format!("{} · {}", clip.library.to_uppercase(), clip.name),
                "duration":clip.duration,"looped":clip.looped,"library":clip.library,"root_motion":clip.root_motion}));
        }
    }
    catalog
}
impl AraRecipe {
    fn clear_builtin_garment_conflict(&mut self, slot: &crate::wardrobe_assets::GarmentSlot) {
        if !matches!(slot, crate::wardrobe_assets::GarmentSlot::Hair | crate::wardrobe_assets::GarmentSlot::Beard) {
            self.params["outfit"] = json!("none");
        }
        match slot {
            crate::wardrobe_assets::GarmentSlot::Hair => self.params["hair"] = json!("none"),
            crate::wardrobe_assets::GarmentSlot::Beard => self.params["facialHair"] = json!("none"),
            crate::wardrobe_assets::GarmentSlot::Top => self.params["top"] = json!("none"),
            crate::wardrobe_assets::GarmentSlot::Bottom => self.params["bottom"] = json!("none"),
            crate::wardrobe_assets::GarmentSlot::Jacket => self.params["jacket"] = json!("none"),
            crate::wardrobe_assets::GarmentSlot::Feet => self.params["shoes"] = json!("none"),
            crate::wardrobe_assets::GarmentSlot::Hat => self.params["hat"] = json!("none"),
            crate::wardrobe_assets::GarmentSlot::Accessory => {
                self.params["accessories"] = json!([])
            }
        }
    }

    /// Toggles a garment as an additional recipe layer without replacing other
    /// custom garments in the same slot.
    pub fn toggle_layered_custom_garment(
        &mut self,
        garment: &crate::wardrobe_assets::CustomGarment,
    ) -> Result<(), String> {
        garment.validate()?;
        if self
            .custom_garments
            .iter()
            .any(|existing| existing.id == garment.id)
        {
            self.custom_garments
                .retain(|existing| existing.id != garment.id);
            return Ok(());
        }
        if self.custom_garments.len() >= 16 {
            return Err("too many custom garments (maximum 16)".into());
        }
        self.custom_garments.push(garment.clone());
        self.clear_builtin_garment_conflict(&garment.slot);
        Ok(())
    }

    /// Adds a catalog garment to this recipe, replacing the existing custom
    /// garment in its slot.  A native outfit and an independently selected
    /// custom garment cannot occupy the same slot.
    pub fn select_custom_garment(
        &mut self,
        garment: &crate::wardrobe_assets::CustomGarment,
    ) -> Result<(), String> {
        garment.validate()?;
        if garment.slot == crate::wardrobe_assets::GarmentSlot::Accessory {
            if self
                .custom_garments
                .iter()
                .any(|existing| existing.id == garment.id)
            {
                self.custom_garments
                    .retain(|existing| existing.id != garment.id);
                return Ok(());
            }
            if self.custom_garments.len() >= 16 {
                return Err("too many custom garments (maximum 16)".into());
            }
            self.custom_garments.push(garment.clone());
            self.clear_builtin_garment_conflict(&garment.slot);
            return Ok(());
        }
        self.custom_garments
            .retain(|existing| existing.slot != garment.slot && existing.id != garment.id);
        if self.custom_garments.len() >= 16 {
            return Err("too many custom garments (maximum 16)".into());
        }
        self.custom_garments.push(garment.clone());
        self.clear_builtin_garment_conflict(&garment.slot);
        Ok(())
    }

    pub fn remove_custom_garment_slot(&mut self, slot: &crate::wardrobe_assets::GarmentSlot) {
        self.custom_garments.retain(|garment| &garment.slot != slot);
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.cartoon.strength.is_finite() || !(0.0..=1.0).contains(&self.cartoon.strength) {
            return Err("cartoon strength must be finite and between 0 and 1".into());
        }
        self.breasts.validate()?;
        if self.custom_garments.len() > 16 {
            return Err("too many custom garments (maximum 16)".into());
        }
        let mut garment_ids = std::collections::BTreeSet::new();
        for garment in &self.custom_garments {
            garment.validate()?;
            if !garment_ids.insert(&garment.id) {
                return Err("custom garment IDs must be unique".into());
            }
        }
        if self.custom_targets.len() > 64 {
            return Err("too many custom targets".into());
        }
        let mut target_names = std::collections::BTreeSet::new();
        for t in &self.custom_targets {
            if t.name.trim().is_empty() || !target_names.insert(&t.name) {
                return Err("custom target names must be nonempty and unique".into());
            }
            let mut vertices = std::collections::BTreeSet::new();
            if t.deltas.iter().any(|(index, _)| !vertices.insert(*index)) {
                return Err("custom target contains duplicate vertex indices".into());
            }
            if !t.weight.is_finite()
                || !(-1.0..=1.0).contains(&t.weight)
                || t.name.len() > 256
                || t.deltas.len() > 19158
                || t.deltas
                    .iter()
                    .any(|(i, d)| *i >= 19158 || d.iter().any(|v| !v.is_finite()))
            {
                return Err("invalid custom target".into());
            }
        }
        let schema = native_catalog();
        let params = self
            .params
            .as_object()
            .ok_or("customization parameters must be an object")?;
        for field in schema["fields"].as_array().unwrap() {
            let id = field["id"].as_str().unwrap();
            let value = params
                .get(id)
                .ok_or_else(|| format!("missing customization: {id}"))?;
            let invalid = || format!("invalid customization: {id}");
            match field["kind"].as_str().unwrap_or("") {
                "number" | "integer" => {
                    let v = value
                        .as_f64()
                        .filter(|v| v.is_finite())
                        .ok_or_else(invalid)?;
                    if v < field["min"].as_f64().unwrap_or(0.)
                        || v > field["max"].as_f64().unwrap_or(1.)
                    {
                        return Err(invalid());
                    }
                    if field["kind"] == "integer" && v.fract() != 0. {
                        return Err(invalid());
                    }
                }
                "boolean" => {
                    if !value.is_boolean() {
                        return Err(invalid());
                    }
                }
                "text" => {
                    if value.as_str().is_none_or(|v| v.len() > 256) {
                        return Err(invalid());
                    }
                }
                "color" => {
                    if !valid_color(value.as_str().unwrap_or("")) {
                        return Err(invalid());
                    }
                }
                "choice" => {
                    if !field["choices"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|c| c["id"] == *value)
                    {
                        return Err(invalid());
                    }
                }
                "multi" => {
                    let entries = value
                        .as_array()
                        .filter(|a| a.len() <= 64)
                        .ok_or_else(invalid)?;
                    if entries.iter().any(|v| {
                        !field["choices"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|c| c["id"] == *v)
                    }) {
                        return Err(invalid());
                    }
                }
                _ => return Err(invalid()),
            }
        }
        let mods = params
            .get("modifiers")
            .and_then(Value::as_object)
            .ok_or("invalid detail modifiers")?;
        if mods.len() > 1500
            || mods.iter().any(|(k, v)| {
                k.len() > 160
                    || v.as_f64()
                        .is_none_or(|n| !n.is_finite() || !(-1.0..=1.0).contains(&n))
            })
        {
            return Err("invalid detail modifier weight".into());
        }
        if params
            .get("garmentStyles")
            .and_then(Value::as_object)
            .is_none()
        {
            return Err("invalid garment styles".into());
        }
        if !self.preview_clip.is_empty()
            && !external_clip_id(&self.preview_clip)
            && !schema["clips"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == self.preview_clip)
        {
            return Err("unknown preview clip".into());
        }
        if !["static", "rigged", "animated"].contains(&self.export.mode.as_str())
            || !["glb", "obj"].contains(&self.export.format.as_str())
        {
            return Err("unsupported export settings".into());
        }
        if self.export.clips.len() > 1024
            || self.export.clips.iter().any(|id| {
                !external_clip_id(id)
                    && !schema["clips"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|c| c["id"] == *id)
            })
        {
            return Err("unknown export clip".into());
        }
        Ok(())
    }
    pub fn from_reference(params: Value) -> Result<Self, String> {
        let mut out = Self::default();
        let defaults = out.params.as_object_mut().unwrap();
        for (key, value) in params.as_object().ok_or("expected parameter object")? {
            if defaults.contains_key(key) {
                defaults.insert(key.clone(), value.clone());
            }
        }
        out.validate()?;
        Ok(out)
    }
    /// Deterministic native randomization: presets provide coherent wardrobe combinations.
    pub fn random(seed: u64) -> Self {
        let schema = native_catalog();
        let mut state = seed.wrapping_add(0x9e3779b97f4a7c15);
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / ((1u64 << 53) as f64)
        };
        let presets = schema["presets"].as_array().unwrap();
        let preset = &presets[(next() * presets.len() as f64) as usize % presets.len()];
        let mut out = Self::from_reference(preset["params"].clone()).unwrap_or_default();
        for key in ["muscle", "weight", "height", "proportions"] {
            out.params[key] = json!(0.2 + next() * 0.65);
        }
        out.params["seed"] = json!(seed % 1_000_000_000);
        out.params["name"] = json!(format!("Character {seed}"));
        out
    }
}
pub fn valid_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|c| c.is_ascii_hexdigit())
}
/// Imported IDs are validated against the installed bank when building a rig.
fn external_clip_id(id: &str) -> bool {
    id.split_once(':').is_some_and(|(library, name)| {
        matches!(library, "ual1" | "ual2" | "meshy-female" | "meshy-male")
            && !name.is_empty()
            && name.len() <= 160
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '))
    })
}
pub fn rgb(s: &str) -> [f32; 3] {
    let n = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0xffffff);
    [
        ((n >> 16) & 255) as f32 / 255.,
        ((n >> 8) & 255) as f32 / 255.,
        (n & 255) as f32 / 255.,
    ]
}
pub fn multiply_colors(a: &str, b: &str) -> String {
    let a = rgb(a);
    let b = rgb(b);
    format!(
        "#{:02x}{:02x}{:02x}",
        (a[0] * b[0] * 255.).round() as u8,
        (a[1] * b[1] * 255.).round() as u8,
        (a[2] * b[2] * 255.).round() as u8
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_reference_presets_and_random_recipes_validate() {
        AraRecipe::default().validate().unwrap();
        for p in native_catalog()["presets"].as_array().unwrap() {
            AraRecipe::from_reference(p["params"].clone())
                .unwrap_or_else(|e| panic!("{}: {e}", p["name"]));
        }
        for seed in 0..20 {
            let a = AraRecipe::random(seed);
            a.validate().unwrap();
            assert_eq!(a, AraRecipe::random(seed));
        }
    }
    #[test]
    fn malformed_customizations_rejected() {
        let mut r = AraRecipe::default();
        r.params["height"] = json!(50);
        assert!(r.validate().is_err());
    }
    #[test]
    fn cartoon_settings_default_and_bounds_validate() {
        let recipe = AraRecipe::default();
        assert!(!recipe.cartoon.enabled);
        assert_eq!(recipe.cartoon.strength, 1.);
        let mut bad = recipe;
        bad.cartoon.strength = 1.01;
        assert!(bad.validate().is_err());
    }
}
