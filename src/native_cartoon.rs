//! Cartoon material softening (Cartoon hm08 mode). The cartoon shape itself is the
//! `cartoon_hm08/cartoon.pack` offsets applied in `native_character`.
use serde_json::{Value, json};

/// Cartoon look for one built part: which texture pass it gets (see
/// `native_texture::cartoon_texture`) and its softened material.
pub fn stylise_part(name: &str, material: &mut Value, strength: f32) {
    let kind = match name {
        "Body" => "skin",
        "Eyes" => "eyes",
        "Eyebrows" => "brows",
        "Eyelashes" => "lashes",
        "tattoo_stubble" => "painted",
        "Teeth" | "Tongue" => return,
        _ => "",
    };
    if kind != "eyes" {
        soften_material(material, strength);
    }
    // Strand-card beards read as speckles at toy scale; their cards are made solid
    // like brows (a painted stubble layer underneath is added in `native_character`).
    let kind = if material["cartoonBeard"] == true {
        "brows"
    } else {
        kind
    };
    if !kind.is_empty() {
        material["cartoonDetail"] = json!(strength);
        material["cartoonKind"] = json!(kind);
    }
}

/// Gives skin, cloth and hair the low-specular response of the reference
/// sheets without changing eye and teeth materials, where a small highlight is
/// useful for readability.
pub fn soften_material(material: &mut Value, strength: f32) {
    let roughness = material["roughness"].as_f64().unwrap_or(0.7) as f32;
    let metallic = material["metalness"].as_f64().unwrap_or(0.) as f32;
    material["roughness"] = json!(roughness + (0.84 - roughness) * strength);
    material["metalness"] = json!(metallic * (1. - strength));
    if material["alpha"].as_str().unwrap_or("opaque") == "opaque" {
        material["cartoonDetail"] = json!(strength);
    }
    // Cartoon shading relies on broad lighting gradients rather than the
    // high-frequency photographic normal maps supplied by realistic outfits.
    material.as_object_mut().map(|object| {
        object.remove("normal");
        object.remove("normalMap");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softening_drops_normal_maps_and_scales_with_strength() {
        let mut m = json!({"roughness": 0.4, "metalness": 0.5, "normal": "n.jpg"});
        soften_material(&mut m, 1.);
        assert_eq!(m["roughness"], json!(0.84f32));
        assert_eq!(m["metalness"], json!(0.0f32));
        assert!(m.get("normal").is_none());
        let mut half = json!({"roughness": 0.4});
        soften_material(&mut half, 0.5);
        assert!((half["roughness"].as_f64().unwrap() - 0.62).abs() < 1e-5);
    }
}
