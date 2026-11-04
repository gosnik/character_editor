//! Attach editor controls to a host-owned character and camera without a second scene.
use avian3d::prelude::*;
use bevy::prelude::*;
use human_editor::editor::{CharacterEditorUiPlugin, EditorCamera, EditorState};
use human_editor::{CharacterLibrary, CharacterPlugin, CharacterRecipe, CharacterShape};
fn main() {
    let library =
        CharacterLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/makehuman")).unwrap();
    App::new()
        .insert_resource(library)
        .insert_resource(
            human_editor::native_runtime::NativeAssets::load(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/native"
            ))
            .unwrap(),
        )
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets").into(),
            ..default()
        }))
        .add_plugins((
            PhysicsPlugins::default(),
            CharacterPlugin,
            CharacterEditorUiPlugin,
        ))
        .add_systems(Startup, setup)
        .run();
}
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut editor: ResMut<EditorState>,
) {
    for x in [-0.7, 0.7] {
        let entity = commands
            .spawn((
                CharacterShape(CharacterRecipe {
                    customization: Some(human_editor::ara::AraRecipe::default()),
                    ..default()
                }),
                RigidBody::Kinematic,
                Transform::from_xyz(x, 0., 0.),
            ))
            .id();
        if x < 0. {
            editor.selected = Some(entity);
        }
    }
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(10., 10.))),
        MeshMaterial3d(materials.add(Color::srgb(0.12, 0.15, 0.18))),
        RigidBody::Static,
        Collider::cuboid(10., 0.1, 10.),
        Transform::from_xyz(0., -0.05, 0.),
    ));
    commands.spawn((
        Camera3d::default(),
        EditorCamera,
        Transform::from_xyz(0., 1.5, 4.).looking_at(Vec3::Y, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 8000.,
            ..default()
        },
        Transform::from_xyz(2., 4., 3.).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
