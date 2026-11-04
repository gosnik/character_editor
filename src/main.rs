use avian3d::prelude::*;
use bevy::prelude::*;
use human_editor::{CharacterLibrary, CharacterPlugin, editor::CharacterEditorPlugin};

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--headless" || a == "cli") {
        let cli_args = std::iter::once(std::ffi::OsString::from("human-editor --headless"))
            .chain(args.into_iter().skip(2));
        std::process::exit(human_editor::cli::entry(cli_args));
    }
    if args.get(1).is_some_and(|a| a == "--help" || a == "-h") {
        println!(
            "Human Studio\n\nUsage: human-editor [BASIC_ASSET_DIRECTORY]\n       human-editor --headless <COMMAND> [OPTIONS]\n\nRun human-editor --headless --help for the complete headless interface.\nFor servers, build human-cli with --no-default-features."
        );
        return;
    }
    if args.get(1).is_some_and(|a| a == "--version" || a == "-V") {
        println!("human-editor {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let root = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/makehuman")
        });
    let library = CharacterLibrary::load(&root).unwrap_or_else(|error| {
        eprintln!(
            "Cannot load MakeHuman assets from {}: {error}",
            root.display()
        );
        std::process::exit(1)
    });
    let native = human_editor::native_runtime::NativeAssets::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/native"
    ))
    .unwrap_or_else(|e| {
        eprintln!("Native assets: {e}");
        std::process::exit(1)
    });
    App::new()
        .insert_resource(native)
        .insert_resource(library)
        .insert_resource(ClearColor(Color::srgb(0.035, 0.046, 0.065)))
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets").into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Human Studio • Character Editor".into(),
                        resolution: (1440, 960).into(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins((
            PhysicsPlugins::default(),
            CharacterPlugin,
            CharacterEditorPlugin,
        ))
        .add_systems(Update, smoke_capture)
        .run();
}

// Opt-in native rendering smoke test; normal editor sessions have no frame limit.
fn smoke_capture(
    errors: Query<&human_editor::CharacterBuildError>,
    mut instances: Query<&mut human_editor::native_runtime::NativeInstance>,
    mut frames: Local<u32>,
    mut configured: Local<bool>,
    mut shapes: Query<&mut human_editor::CharacterShape>,
    mut view: ResMut<human_editor::editor::EditorState>,
    visuals: Query<
        &MeshMaterial3d<StandardMaterial>,
        With<human_editor::native_runtime::NativeVisual>,
    >,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(path) = std::env::var("HUMAN_EDITOR_SMOKE") else {
        return;
    };
    if !*configured {
        let preset = std::env::var("HUMAN_EDITOR_SMOKE_PRESET").ok();
        let clip = std::env::var("HUMAN_EDITOR_SMOKE_CLIP").ok();
        if let Some(mut shape) = shapes.iter_mut().next() {
            let recipe = if let Ok(recipe_path) = std::env::var("HUMAN_EDITOR_SMOKE_RECIPE") {
                human_editor::cli_io::load(
                    std::path::Path::new(&recipe_path),
                    std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")),
                )
                .expect("smoke recipe must load")
            } else if let Some(name) = preset {
                let catalog = human_editor::ara::native_catalog();
                let source = catalog["presets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|p| p["name"].as_str() == Some(name.as_str()))
                    .expect("smoke preset exists");
                human_editor::ara::AraRecipe::from_reference(source["params"].clone()).unwrap()
            } else {
                shape.0.customization.clone().unwrap_or_default()
            };
            let mut recipe = recipe;
            if std::env::var_os("HUMAN_EDITOR_SMOKE_CARTOON").is_some() {
                recipe.cartoon.enabled = true;
            }
            if let Some(clip) = clip {
                recipe.preview_clip = clip;
            }
            shape.0.customization = Some(recipe);
            let yaw = match std::env::var("HUMAN_EDITOR_SMOKE_VIEW").as_deref() {
                Ok("back") => std::f32::consts::PI,
                Ok("side") => std::f32::consts::FRAC_PI_2,
                _ => 0.0,
            };
            let distance = std::env::var("HUMAN_EDITOR_SMOKE_DISTANCE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3.5);
            let focus = std::env::var("HUMAN_EDITOR_SMOKE_FOCUS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.9);
            view.set_view(yaw, 0.05, distance, focus);
            *configured = true;
        }
        return;
    }
    if instances.is_empty()
        || shapes.iter().any(|shape| {
            instances.iter().all(|i| {
                shape.0.customization.as_ref().is_some_and(|r| {
                    r.params != i.character.recipe.params
                        || r.cartoon != i.character.recipe.cartoon
                        || r.preview_clip != i.clip
                })
            })
        })
    {
        if let Some(error) = errors.iter().next() {
            panic!("smoke character build failed: {}", error.0);
        }
        return;
    }
    *frames += 1;
    if std::env::var_os("HUMAN_EDITOR_SMOKE_CLAY").is_some() {
        for handle in &visuals {
            if let Some(mut material) = materials.get_mut(&handle.0) {
                if material.alpha_mode == AlphaMode::Opaque && material.perceptual_roughness > 0.3 {
                    material.base_color_texture = None;
                    material.normal_map_texture = None;
                    material.base_color = Color::srgb(0.55, 0.53, 0.51);
                    material.perceptual_roughness = 0.85;
                }
            }
        }
    }
    if let Some(time) = std::env::var("HUMAN_EDITOR_SMOKE_TIME")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        for mut instance in &mut instances {
            instance.playing = false;
            instance.time = time;
        }
    }
    if *frames == 120 {
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
    }
    if *frames == 180 {
        exit.write(AppExit::Success);
    }
}
