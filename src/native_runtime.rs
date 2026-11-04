//! Native Bevy character generation and playback; host controls camera, UI and physics.
use crate::{
    CharacterBuildError, CharacterShape,
    ara::AraRecipe,
    native_assets::NativeLibrary,
    native_character::{NativeCharacter, NativePart},
    native_texture::fabric_image,
    wardrobe_assets::{CustomGarmentCatalog, native_texture_path},
};
use avian3d::prelude::*;
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::NoFrustumCulling,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};

#[derive(Resource, Clone)]
pub struct NativeAssets(pub Arc<NativeLibrary>, pub Arc<CustomGarmentCatalog>);
impl NativeAssets {
    pub fn load(root: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let library = Arc::new(NativeLibrary::load(root)?);
        let garments = Arc::new(CustomGarmentCatalog::discover_for_library(&library));
        Ok(Self(library, garments))
    }
}
#[derive(Component)]
pub struct NativeInstance {
    pub character: Arc<NativeCharacter>,
    pub clip: String,
    pub playing: bool,
    pub time: f32,
    pub breast_motion: crate::native_breast::BreastMotion,
    pub(crate) children: Vec<Entity>,
}
#[derive(Component)]
pub struct NativeVisual;
#[derive(Component)]
struct NativeMesh {
    part: usize,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
}
struct Running {
    entity: Entity,
    recipe: AraRecipe,
    receiver: Mutex<mpsc::Receiver<Result<NativeCharacter, String>>>,
}
#[derive(Resource, Default)]
pub struct NativeBuildQueue {
    pending: HashMap<Entity, (AraRecipe, Instant)>,
    running: Option<Running>,
    pub status: String,
    failed: HashMap<Entity, AraRecipe>,
}
pub struct NativeCharacterPlugin;
impl Plugin for NativeCharacterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NativeBuildQueue>().add_systems(
            Update,
            (queue_builds, finish_builds, animate_native)
                .chain()
                .run_if(resource_exists::<NativeAssets>),
        );
    }
}
fn queue_builds(
    mut shapes: Query<(Entity, Ref<CharacterShape>, Option<&mut NativeInstance>)>,
    assets: Option<Res<NativeAssets>>,
    mut queue: ResMut<NativeBuildQueue>,
) {
    let Some(assets) = assets else {
        return;
    };
    if assets.is_changed() {
        queue.failed.clear();
    }
    queue.failed.retain(|e, _| shapes.get(*e).is_ok());
    queue.pending.retain(|e, _| shapes.get(*e).is_ok());
    for (entity, shape, instance) in &mut shapes {
        if let Some(recipe) = &shape.0.customization {
            if queue.failed.get(&entity) == Some(recipe) {
                continue;
            }
            if let Some(mut instance) = instance {
                if instance.character.recipe.params == recipe.params
                    && instance.character.recipe.custom_targets == recipe.custom_targets
                    && instance.character.recipe.custom_garments == recipe.custom_garments
                    && instance.character.recipe.breasts == recipe.breasts
                    && instance.character.recipe.cartoon == recipe.cartoon
                {
                    if instance.clip != recipe.preview_clip {
                        instance.clip = recipe.preview_clip.clone();
                        instance.time = 0.;
                        instance.breast_motion.reset();
                    }
                    queue.pending.remove(&entity);
                    continue;
                }
            }

            if shape.is_changed()
                || (!queue.pending.contains_key(&entity)
                    && queue.running.as_ref().is_none_or(|r| r.entity != entity))
            {
                queue
                    .pending
                    .insert(entity, (recipe.clone(), Instant::now()));
            }
        }
    }
    if queue.running.is_some() {
        return;
    }
    let next = queue
        .pending
        .iter()
        .find(|(_, (_, t))| t.elapsed().as_millis() > 180)
        .map(|(&e, (r, _))| (e, r.clone()));
    if let Some((entity, recipe)) = next {
        queue.pending.remove(&entity);
        let lib = assets.0.clone();
        let job = recipe.clone();
        let (sender, receiver) = mpsc::channel();
        queue.status = "Building character in native Rust…".into();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                NativeCharacter::build(&lib, &job)
            }))
            .unwrap_or_else(|_| Err("native build rejected inconsistent geometry".into()));
            let _ = sender.send(result);
        });
        queue.running = Some(Running {
            entity,
            recipe,
            receiver: Mutex::new(receiver),
        });
    }
}
fn finish_builds(
    mut commands: Commands,
    mut queue: ResMut<NativeBuildQueue>,
    shapes: Query<(&CharacterShape, Option<&NativeInstance>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,

    assets: Option<Res<NativeAssets>>,
) {
    let result = queue
        .running
        .as_ref()
        .and_then(|r| r.receiver.lock().ok()?.try_recv().ok());
    let Some(result) = result else {
        return;
    };
    let Some(running) = queue.running.take() else {
        return;
    };
    let Ok((shape, previous)) = shapes.get(running.entity) else {
        return;
    };
    if shape.0.customization.as_ref() != Some(&running.recipe) {
        queue.status = "Discarded superseded build".into();
        return;
    }
    match result {
        Err(error) => {
            queue.failed.insert(running.entity, running.recipe.clone());
            queue.status = error.clone();
            commands
                .entity(running.entity)
                .insert(CharacterBuildError(error));
        }
        Ok(character) => {
            if let Some(previous) = previous {
                for child in &previous.children {
                    if let Ok(mut child) = commands.get_entity(*child) {
                        child.despawn();
                    }
                }
            }
            let character = Arc::new(character);
            let mut children = vec![];
            for (part_index, part) in character.parts.iter().enumerate() {
                if part.indices.is_empty() {
                    continue;
                }
                let mut material = standard_material(&part.material);
                let map = part.material["diffuse"]
                    .as_str()
                    .or_else(|| part.material["map"].as_str());
                if let (Some(path), Some(assets)) = (map, assets.as_ref()) {
                    if let Some(texture_path) = native_texture_path(&assets.0.root, path)
                        .filter(|texture_path| texture_path.is_file())
                    {
                        material.base_color_texture =
                            load_image(&texture_path, true, &part.material)
                        .map(|image| images.add(image));
                    }
                }
                let explicit_pattern = part.material["pattern"].as_str();
                if explicit_pattern.is_some()
                    || (material.base_color_texture.is_none()
                        && part.name != "Body"
                        && part.name != "Eyes")
                {
                    if part.name.contains("hair")
                        || part.name.contains("Hair")
                        || part.name.contains("beard")
                        || part.name.contains("earring")
                        || part.name.contains("weapon")
                        || part.name.contains("Bicycle")
                        || part.name.contains("Tattoo")
                    {
                    } else {
                        material.base_color_texture = Some(
                            images.add(
                                assets
                                    .as_ref()
                                    .and_then(|a| {
                                        crate::native_texture::recolor_pattern(
                                            &a.0.root,
                                            &part.material,
                                        )
                                    })
                                    .unwrap_or_else(|| fabric_image(&part.material)),
                            ),
                        );
                        material.base_color = Color::WHITE;
                    }
                }
                if let Some(normal) = part.material["normal"]
                    .as_str()
                    .or_else(|| part.material["normalMap"].as_str())
                {
                    if let Some(texture_path) = assets
                        .as_ref()
                        .and_then(|assets| native_texture_path(&assets.0.root, normal))
                        .filter(|texture_path| texture_path.is_file())
                    {
                        material.normal_map_texture =
                            load_image(&texture_path, false, &serde_json::Value::Null).map(|image| images.add(image));
                    }
                }
                let (joints, weights) = character
                    .rig
                    .as_ref()
                    .map(|r| r.skin_part(part))
                    .unwrap_or_default();
                let visual = commands
                    .spawn((
                        NativeVisual,
                        NativeMesh {
                            part: part_index,
                            joints,
                            weights,
                        },
                        Mesh3d(meshes.add(part_mesh(part, &part.positions))),
                        MeshMaterial3d(materials.add(material)),
                        Transform::default(),
                        NoFrustumCulling,
                    ))
                    .id();
                commands.entity(running.entity).add_child(visual);
                children.push(visual);
            }
            let height = character.height.max(0.2);
            let radius = (height * 0.13).clamp(0.05, height * 0.25);
            commands
                .entity(running.entity)
                .insert((
                    NativeInstance {
                        character: character.clone(),
                        clip: running.recipe.preview_clip.clone(),
                        playing: true,
                        time: 0.,
                        breast_motion: Default::default(),
                        children,
                    },
                    Collider::compound(vec![(
                        Vec3::Y * height * 0.5,
                        Quat::IDENTITY,
                        Collider::capsule(radius, height - radius * 2.),
                    )]),
                ))
                .remove::<CharacterBuildError>();
            queue.status = format!(
                "Native Rust · {} triangles · {} parts",
                character.triangle_count(),
                character.parts.len()
            );
        }
    }
}
fn animate_native(
    time: Res<Time>,
    mut instances: Query<(&mut NativeInstance, &GlobalTransform)>,
    visuals: Query<(&NativeMesh, &Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    for (mut instance, root_transform) in &mut instances {
        if instance.playing {
            instance.time += time.delta_secs();
        }
        let character = instance.character.clone();
        let Some(rig) = &character.rig else {
            continue;
        };
        let mut matrices = rig.pose(&instance.clip, instance.time);
        let dt = if instance.playing {
            time.delta_secs()
        } else {
            0.
        };
        instance.breast_motion.apply(
            rig,
            &mut matrices,
            root_transform.translation(),
            dt,
            &character.recipe.breasts,
            character.recipe.params["gender"].as_f64().unwrap_or(1.) as f32,
        );
        // Animation preview stays in place; the game controller owns Avian root motion.
        if let Some(root) = rig.bones.first() {
            let moved = matrices[0].transform_point3(root.head) - root.head;
            let correction = Mat4::from_translation(Vec3::new(-moved.x, 0., -moved.z));
            for matrix in &mut matrices {
                *matrix = correction * (*matrix);
            }
        }
        let mut facial = BTreeMap::new();
        let phase = instance.time % 4.1;
        let blink = if phase < 0.18 {
            (phase / 0.18 * std::f32::consts::PI).sin()
        } else {
            0.
        };
        facial.insert("eyeBlinkLeft".into(), blink);
        facial.insert("eyeBlinkRight".into(), blink);
        if instance.clip == "talk" {
            facial.insert("viseme_aa".into(), (instance.time * 10.).sin().abs() * 0.65);
        }
        for child in &instance.children {
            let Ok((binding, handle)) = visuals.get(*child) else {
                continue;
            };
            let part = &instance.character.parts[binding.part];
            if part.name == "Bicycle" {
                continue;
            }
            let offsets = instance.character.face_offsets(part, &facial);
            let source: Vec<[f32; 3]> = part
                .positions
                .iter()
                .zip(offsets)
                .map(|(p, d)| [p[0] + d[0], p[1] + d[1], p[2] + d[2]])
                .collect();
            let positions =
                rig.skin_positions(&source, &binding.joints, &binding.weights, &matrices);
            if let Some(mut mesh) = meshes.get_mut(&handle.0) {
                mesh.insert_attribute(
                    Mesh::ATTRIBUTE_NORMAL,
                    crate::native_normals::surface_normals(part, &positions),
                );
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
                if part.material["normal"].as_str().is_some()
                    || part.material["normalMap"].as_str().is_some()
                {
                    if let Err(error) = mesh.generate_tangents() {
                        mesh.remove_attribute(Mesh::ATTRIBUTE_TANGENT);
                        warn_once!(
                            "Cannot update normal-map tangents for {}: {error:?}",
                            part.name
                        );
                    }
                }
            }
        }
    }
}
pub fn part_mesh(part: &NativePart, positions: &[[f32; 3]]) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.to_vec());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, part.uvs.clone());
    mesh.insert_indices(Indices::U32(part.indices.clone()));
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        crate::native_normals::surface_normals(part, positions),
    );
    if part.material["normal"].as_str().is_some() || part.material["normalMap"].as_str().is_some() {
        if let Err(error) = mesh.generate_tangents() {
            warn!("Cannot generate mesh tangents for {}: {error:?}", part.name);
        }
    }
    mesh
}
fn standard_material(m: &serde_json::Value) -> StandardMaterial {
    let color = crate::ara::rgb(m["color"].as_str().unwrap_or("#ffffff"));
    let glow = crate::ara::rgb(m["emissive"].as_str().unwrap_or("#000000"));
    StandardMaterial {
        emissive: Color::srgb(glow[0], glow[1], glow[2]).to_linear(),
        base_color: Color::srgba(
            color[0],
            color[1],
            color[2],
            m["opacity"].as_f64().unwrap_or(1.) as f32,
        ),
        perceptual_roughness: m["roughness"].as_f64().unwrap_or(0.7) as f32,
        metallic: m["metalness"].as_f64().unwrap_or(0.) as f32,
        alpha_mode: match m["alpha"].as_str() {
            Some("blend") => AlphaMode::Blend,
            // Match the source Ara GLB exporter and MakeHuman's cutout hair:
            // a 0.3 cutoff keeps the semi-transparent fringe pixels as dark
            // rectangular card bands around short styles.
            Some("mask") => AlphaMode::Mask(0.5),
            _ => AlphaMode::Opaque,
        },
        cull_mode: if m["doubleSided"] == true {
            None
        } else {
            Some(bevy::render::render_resource::Face::Back)
        },
        ..default()
    }
}

fn load_image(path: &std::path::Path, srgb: bool, material: &serde_json::Value) -> Option<Image> {
    let mut rgba = image::open(path).ok()?.into_rgba8();
    crate::native_texture::cartoon_texture(&mut rgba, material);
    let (width, height) = rgba.dimensions();
    let mut output = Image::new(
        bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba.into_raw(),
        if srgb {
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb
        } else {
            bevy::render::render_resource::TextureFormat::Rgba8Unorm
        },
        RenderAssetUsages::default(),
    );
    output.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::Repeat,
        address_mode_v: bevy::image::ImageAddressMode::Repeat,
        ..default()
    });
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{asset::AssetApp, mesh::MeshPlugin};

    #[test]
    fn cutout_materials_match_the_source_half_alpha_threshold() {
        let material = standard_material(&serde_json::json!({"alpha":"mask"}));
        assert_eq!(material.alpha_mode, AlphaMode::Mask(0.5));
    }

    #[test]
    fn permanent_failure_is_not_retried_and_despawn_prunes_queue() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), MeshPlugin))
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .insert_resource(
                NativeAssets::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap(),
            )
            .add_plugins(NativeCharacterPlugin);
        let mut recipe = AraRecipe::default();
        recipe.params["height"] = serde_json::json!(10.);
        let entity = app
            .world_mut()
            .spawn((
                CharacterShape(crate::CharacterRecipe {
                    customization: Some(recipe),
                    ..default()
                }),
                Transform::default(),
            ))
            .id();
        app.finish();
        app.cleanup();
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(200));
        for _ in 0..20 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.world().get::<CharacterBuildError>(entity).is_some());
        for _ in 0..20 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(15));
            let queue = app.world().resource::<NativeBuildQueue>();
            assert_eq!(queue.failed.len(), 1);
            assert!(queue.running.is_none());
            assert!(queue.pending.is_empty());
        }
        app.world_mut().despawn(entity);
        app.update();
        assert!(app.world().resource::<NativeBuildQueue>().failed.is_empty());
    }
}
