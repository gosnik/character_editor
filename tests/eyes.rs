use human_editor::{
    ara::AraRecipe,
    native_assets::NativeLibrary,
    native_character::{NativeCharacter, NativePart},
};
use serde_json::json;

fn library() -> NativeLibrary {
    NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap()
}

fn eyes(character: &NativeCharacter) -> &NativePart {
    character
        .parts
        .iter()
        .find(|part| part.name == "Eyes")
        .expect("character has fitted eyes")
}

fn iris_triangle(part: &NativePart, center: [f32; 2]) -> bool {
    part.indices.chunks_exact(3).any(|triangle| {
        let uv = [
            part.uvs[triangle[0] as usize],
            part.uvs[triangle[1] as usize],
            part.uvs[triangle[2] as usize],
        ];
        let centroid = [
            (uv[0][0] + uv[1][0] + uv[2][0]) / 3.,
            (uv[0][1] + uv[1][1] + uv[2][1]) / 3.,
        ];
        (centroid[0] - center[0]).hypot(centroid[1] - center[1]) < 0.13
    })
}

#[test]
fn eye_mesh_omits_transparent_cornea_swatch_and_keeps_blue_irises() {
    let lib = library();
    for detail in ["low", "high"] {
        let mut recipe = AraRecipe::default();
        // Cartoon characters use the source Meshy surface, which already
        // contains its eyes rather than adding the canonical fitted proxy.
        recipe.params["detail"] = json!(detail);
        recipe.params["eyes"] = json!("blue");
        let character = NativeCharacter::build(&lib, &recipe).unwrap();
        let eyes = eyes(&character);

        assert_eq!(eyes.material["diffuse"], "eyes_blue.png");
        assert!(eyes.indices.chunks_exact(3).all(|triangle| {
            triangle.iter().any(|&index| {
                let uv = eyes.uvs[index as usize];
                uv[0] < 0.8 || uv[1] < 0.8
            })
        }));
        assert!(eyes.uvs.iter().all(|uv| {
            uv.iter()
                .all(|value| value.is_finite() && (0. ..=1.).contains(value))
        }));
        // polygon_part flips V, so the two diagonal atlas irises are at
        // these render-space coordinates rather than their PNG-space ones.
        assert!(iris_triangle(eyes, [0.65, 0.75]));
        assert!(iris_triangle(eyes, [0.28, 0.35]));
    }
}
