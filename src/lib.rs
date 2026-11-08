//! Runtime API: add `CharacterPlugin`, insert a `CharacterLibrary`, and spawn
//! `CharacterRecipe` through `CharacterShape`. The host owns cameras and physics.
pub mod ara;
#[cfg(feature = "editor")]
pub mod ara_editor;
pub mod cli;
pub mod cli_io;
#[cfg(feature = "editor")]
pub mod editor;
pub mod model;
pub mod mpfb;
pub mod native_anatomy;
pub mod native_animation;
pub mod native_assets;
pub mod native_breast;
pub mod native_cartoon;
pub mod native_character;
pub mod native_cloth;
pub mod native_export;
pub mod native_fit;
pub mod native_hair;
pub mod native_morph;
pub mod native_normals;
pub mod native_occlusion;
pub mod native_rig;
pub mod native_runtime;
pub mod native_shell;
pub mod native_sleeve_cage;
pub mod native_texture;
pub mod native_torso_cage;
pub mod native_wardrobe;
pub mod wardrobe;
pub mod wardrobe_assets;
use avian3d::prelude::*;
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
pub use model::{CharacterLibrary, CharacterRecipe, ModelError};

#[derive(Component, Clone, Debug, Default)]
pub struct CharacterShape(pub CharacterRecipe);
#[derive(Component, Clone, Debug)]
pub struct CharacterBuildError(pub String);
#[derive(Component)]
pub struct CharacterVisual;
#[derive(Component)]
struct BuiltCharacter {
    visual: Entity,
}

/// Does not install physics, UI, cameras, lights, or load files. Hosts control lifecycle.
pub struct CharacterPlugin;
impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(native_runtime::NativeCharacterPlugin)
            .add_systems(Update, rebuild_characters);
    }
}

pub fn character_mesh(
    library: &CharacterLibrary,
    recipe: &CharacterRecipe,
) -> Result<Mesh, ModelError> {
    let positions = library.deform(recipe)?;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(Indices::U32(library.triangles().to_vec()));
    // Angle-weighted normals in Bevy 0.17 discard tiny triangles at meter scale.
    // Area weighting retains fine facial/finger geometry after decimeter conversion.
    mesh.compute_area_weighted_normals();
    Ok(mesh)
}

/// Capsule approximates the body for locomotion; it does not wrap extended arms.
pub fn character_collider(
    library: &CharacterLibrary,
    recipe: &CharacterRecipe,
) -> Result<Collider, ModelError> {
    let positions = library.deform(recipe)?;
    let height = library
        .triangles()
        .iter()
        .map(|&i| positions[i as usize][1])
        .fold(0.0_f32, f32::max)
        .max(0.2);
    let radius = (height * 0.13).clamp(0.05, height * 0.25);
    Ok(Collider::compound(vec![(
        Vec3::Y * height * 0.5,
        Quat::IDENTITY,
        Collider::capsule(radius, height - radius * 2.0),
    )]))
}

fn rebuild_characters(
    mut commands: Commands,
    library: Option<Res<CharacterLibrary>>,
    shapes: Query<(
        Entity,
        Ref<CharacterShape>,
        Option<&BuiltCharacter>,
        Option<&native_runtime::NativeInstance>,
    )>,
    visuals: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>), With<CharacterVisual>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(library) = library else {
        return;
    };
    for (entity, shape, built, native) in &shapes {
        if shape.0.customization.is_some() {
            if native.is_some() {
                if let Some(built) = built {
                    if let Ok(mut visual) = commands.get_entity(built.visual) {
                        visual.despawn();
                    }
                    commands.entity(entity).remove::<BuiltCharacter>();
                }
            }
            continue;
        }
        if let Some(native) = native {
            for child in &native.children {
                if let Ok(mut child) = commands.get_entity(*child) {
                    child.despawn();
                }
            }
            commands
                .entity(entity)
                .remove::<native_runtime::NativeInstance>();
        }

        if !shape.is_changed()
            && !library.is_changed()
            && built.is_some_and(|b| visuals.get(b.visual).is_ok())
        {
            continue;
        }
        let result = character_mesh(&library, &shape.0)
            .and_then(|mesh| Ok((mesh, character_collider(&library, &shape.0)?)));
        match result {
            Ok((mesh, collider)) => {
                let color = shape.0.skin_color;
                let material = StandardMaterial {
                    base_color: Color::srgb(color[0], color[1], color[2]),
                    perceptual_roughness: 0.78,
                    ..default()
                };
                if let Some((built, (mesh_handle, material_handle))) =
                    built.and_then(|b| visuals.get(b.visual).ok().map(|v| (b, v)))
                {
                    // Replace the asset behind the handle, avoiding GPU asset growth per edit.
                    if let Some(mut asset) = meshes.get_mut(&mesh_handle.0) {
                        *asset = mesh;
                    }
                    if let Some(mut asset) = materials.get_mut(&material_handle.0) {
                        *asset = material;
                    }
                    // Mesh bounds may have changed; Bevy will recompute them next frame.
                    commands
                        .entity(built.visual)
                        .remove::<bevy::camera::primitives::Aabb>();
                } else {
                    if let Some(built) = built {
                        if let Ok(mut old) = commands.get_entity(built.visual) {
                            old.despawn();
                        }
                    }
                    let visual = commands
                        .spawn((
                            CharacterVisual,
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(materials.add(material)),
                            Transform::default(),
                        ))
                        .id();
                    commands
                        .entity(entity)
                        .add_child(visual)
                        .insert(BuiltCharacter { visual });
                }
                commands
                    .entity(entity)
                    .insert(collider)
                    .remove::<CharacterBuildError>();
            }
            Err(error) => {
                commands
                    .entity(entity)
                    .insert(CharacterBuildError(error.to_string()));
            }
        }
    }
}
