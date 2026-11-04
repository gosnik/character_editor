use avian3d::prelude::*;
use bevy::{asset::AssetApp, mesh::MeshPlugin, prelude::*, time::TimeUpdateStrategy};
use human_editor::{
    CharacterBuildError, CharacterLibrary, CharacterPlugin, CharacterShape, CharacterVisual,
};
fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        TransformPlugin,
        MeshPlugin,
    ))
    .init_asset::<StandardMaterial>()
    .insert_resource(
        CharacterLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/makehuman")).unwrap(),
    )
    .insert_resource(TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1. / 60.),
    ))
    .add_plugins((PhysicsPlugins::default(), CharacterPlugin));
    app.finish();
    app.cleanup();
    app
}
#[test]
fn multiple_characters_rebuild_independently_without_leaking_assets() {
    let mut app = app();
    let first = app
        .world_mut()
        .spawn((
            CharacterShape::default(),
            Transform::default(),
            RigidBody::Kinematic,
        ))
        .id();
    let second = app
        .world_mut()
        .spawn((
            CharacterShape::default(),
            Transform::from_xyz(2., 0., 0.),
            RigidBody::Kinematic,
        ))
        .id();
    app.update();
    assert!(app.world().get::<Collider>(first).is_some());
    assert!(app.world().get::<Collider>(second).is_some());
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<CharacterVisual>>()
            .iter(app.world())
            .count(),
        2
    );
    for weight in [0.1, 0.3, 0.8, -0.4] {
        app.world_mut()
            .get_mut::<CharacterShape>(first)
            .unwrap()
            .0
            .weights
            .insert("height".into(), weight);
        app.update();
    }
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 2);
    assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 2);
    assert!(
        app.world()
            .get::<CharacterShape>(second)
            .unwrap()
            .0
            .weights
            .is_empty()
    );
    app.world_mut()
        .get_mut::<CharacterShape>(first)
        .unwrap()
        .0
        .weights
        .insert("bad-target".into(), 1.);
    app.update();
    assert!(app.world().get::<CharacterBuildError>(first).is_some());
    assert!(app.world().get::<Collider>(first).is_some());
}
#[test]
fn avian_character_falls_and_settles_on_floor() {
    let mut app = app();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20., 0.1, 20.),
        Transform::from_xyz(0., -0.05, 0.),
    ));
    let human = app
        .world_mut()
        .spawn((
            CharacterShape::default(),
            RigidBody::Dynamic,
            LockedAxes::ROTATION_LOCKED,
            Transform::from_xyz(0., 1., 0.),
        ))
        .id();
    for _ in 0..240 {
        app.update();
    }
    let position = app.world().get::<Transform>(human).unwrap().translation;
    assert!(
        position.y.abs() < 0.05,
        "character failed to settle at ground: {position:?}"
    );
}

#[test]
fn characters_can_wait_for_library_loading() {
    let mut app = app();
    let library = app
        .world_mut()
        .remove_resource::<CharacterLibrary>()
        .unwrap();
    let human = app
        .world_mut()
        .spawn((
            CharacterShape::default(),
            Transform::default(),
            RigidBody::Kinematic,
        ))
        .id();
    app.update();
    assert!(app.world().get::<Collider>(human).is_none());
    app.insert_resource(library);
    app.update();
    assert!(app.world().get::<Collider>(human).is_some());
}

#[test]
fn removed_visual_is_recovered_and_render_normals_are_finite() {
    let mut app = app();
    let human = app
        .world_mut()
        .spawn((
            CharacterShape::default(),
            Transform::default(),
            RigidBody::Kinematic,
        ))
        .id();
    app.update();
    let visual = app
        .world_mut()
        .query_filtered::<Entity, With<CharacterVisual>>()
        .single(app.world())
        .unwrap();
    app.world_mut().despawn(visual);
    app.update();
    let visual = app
        .world_mut()
        .query_filtered::<&Mesh3d, With<CharacterVisual>>()
        .single(app.world())
        .unwrap();
    let mesh = app
        .world()
        .resource::<Assets<Mesh>>()
        .get(&visual.0)
        .unwrap();
    let bevy::mesh::VertexAttributeValues::Float32x3(normals) =
        mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap()
    else {
        panic!("unexpected normals");
    };
    for &index in app.world().resource::<CharacterLibrary>().triangles() {
        assert!(
            normals[index as usize].iter().all(|n| n.is_finite()),
            "nonfinite normal at {index}"
        );
        assert!(
            Vec3::from(normals[index as usize]).length() > 0.9,
            "zero normal at {index}"
        );
    }
    assert!(app.world().get::<Collider>(human).is_some());
}
