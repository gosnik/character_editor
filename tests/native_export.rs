use bevy::scene::ScenePlugin;
use bevy::{
    asset::{AssetApp, LoadState},
    gltf::{Gltf, GltfPlugin},
    image::ImagePlugin,
    mesh::MeshPlugin,
    prelude::*,
};
use human_editor::{
    ara::AraRecipe, native_assets::NativeLibrary, native_character::NativeCharacter,
    native_export::export_character,
};
#[test]
fn native_rigged_glb_loads_in_bevy_with_textures_skin_and_animation() {
    let assets = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native"));
    let lib = NativeLibrary::load(&assets).unwrap();
    let mut recipe = AraRecipe::default();
    recipe.params["gender"] = serde_json::json!(0.0);
    recipe.export.clips = vec!["idle".into(), "walk".into()];
    let character = NativeCharacter::build(&lib, &recipe).unwrap();
    let root = std::env::temp_dir().join(format!("native-character-export-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    export_character(&character, &recipe, &assets, &root.join("character.glb")).unwrap();
    // Verify independent motion survives serialization as breast-local channels.
    let glb = std::fs::read(root.join("character.glb")).unwrap();
    let json_length = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let document: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_length]).unwrap();
    let binary = &glb[28 + json_length..];
    let walk = document["animations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "walk")
        .unwrap();
    for name in ["LeftBreast", "RightBreast"] {
        let node = document["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .position(|n| n["name"] == name)
            .unwrap();
        let channel = walk["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["target"]["node"] == node && c["target"]["path"] == "rotation")
            .unwrap();
        let sampler = &walk["samplers"][channel["sampler"].as_u64().unwrap() as usize];
        let accessor = &document["accessors"][sampler["output"].as_u64().unwrap() as usize];
        let view = &document["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
        let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize
            + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
        let count = accessor["count"].as_u64().unwrap() as usize;
        let samples: Vec<_> = binary[offset..offset + count * 16]
            .chunks_exact(16)
            .map(|q| {
                Quat::from_array(std::array::from_fn(|i| {
                    f32::from_le_bytes(q[i * 4..i * 4 + 4].try_into().unwrap())
                }))
            })
            .collect();
        assert!(
            samples
                .iter()
                .any(|q| q.angle_between(samples[0]).abs() > 0.001),
            "{name} lost its animation in GLB export"
        );
    }
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: root.to_string_lossy().into_owned(),
            ..default()
        },
        TransformPlugin,
        ScenePlugin,
        bevy::world_serialization::WorldSerializationPlugin,
        ImagePlugin::default(),
        MeshPlugin,
        AnimationPlugin,
        GltfPlugin::default(),
    ))
    .init_asset::<StandardMaterial>();
    app.finish();
    app.cleanup();
    let handle: Handle<Gltf> = app.world().resource::<AssetServer>().load("character.glb");
    let started = std::time::Instant::now();
    loop {
        app.update();
        let server = app.world().resource::<AssetServer>();
        if let LoadState::Failed(error) = server.load_state(handle.id()) {
            panic!("native GLB rejected by Bevy: {error}");
        }
        if server.is_loaded_with_dependencies(handle.id()) {
            break;
        }
        assert!(
            started.elapsed().as_secs() < 45,
            "native GLB import timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let data = app.world().resource::<Assets<Gltf>>().get(&handle).unwrap();
    assert_eq!(data.skins.len(), 1);
    assert_eq!(data.animations.len(), 2);
    assert_eq!(data.materials.len(), character.parts.len());
    assert!(data.default_scene.is_some());
    assert!(data.named_nodes.contains_key("WeaponR"));
    std::fs::remove_dir_all(root).unwrap();
}
