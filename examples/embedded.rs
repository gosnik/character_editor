//! Host-owned game world with two independently editable runtime characters, no editor UI.
use avian3d::prelude::*;
use bevy::prelude::*;
use human_editor::{CharacterLibrary, CharacterPlugin, CharacterRecipe, CharacterShape};
fn main() {
    let library =
        CharacterLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/makehuman")).unwrap();
    App::new()
        .insert_resource(library)
        .add_plugins(DefaultPlugins)
        .add_plugins((PhysicsPlugins::default(), CharacterPlugin))
        .add_systems(Startup, setup)
        .run();
}
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for x in [-0.7, 0.7] {
        commands.spawn((
            CharacterShape(CharacterRecipe::default()),
            RigidBody::Kinematic,
            Transform::from_xyz(x, 0., 0.),
        ));
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
