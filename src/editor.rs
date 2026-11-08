//! Optional editor shell. Add after Egui, runtime, assets, and Avian are available.
use crate::{CharacterBuildError, CharacterLibrary, CharacterRecipe, CharacterShape};
use avian3d::prelude::*;
use bevy::{
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    prelude::*,
};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use std::path::PathBuf;

/// Standalone scene plus editor UI. Games should use CharacterEditorUiPlugin.
pub struct CharacterEditorPlugin;
impl Plugin for CharacterEditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(CharacterEditorUiPlugin)
            .add_systems(Startup, setup);
    }
}
/// UI-only integration. Set EditorState.selected to a host-owned CharacterShape.
/// Mark an existing camera with EditorCamera to opt into orbit controls.
pub struct CharacterEditorUiPlugin;
impl Plugin for CharacterEditorUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        app.add_plugins(crate::ara_editor::NativeEditorUiPlugin)
            .init_resource::<EditorState>()
            .add_systems(EguiPrimaryContextPass, (editor_ui, orbit_camera).chain())
            .add_systems(Update, (sync_selection, collider_gizmo).chain());
    }
}
#[derive(Resource)]
pub struct EditorState {
    pub selected: Option<Entity>,
    tracked: Option<Entity>,
    anchor: Transform,
    original_body: RigidBody,
    original_axes: Option<LockedAxes>,
    path: String,
    status: String,
    category: String,
    history: Vec<CharacterRecipe>,
    future: Vec<CharacterRecipe>,
    gesture_start: Option<CharacterRecipe>,
    saved: CharacterRecipe,
    pub(crate) physics: bool,
    pub(crate) collider: bool,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) distance: f32,
    pub(crate) focus: f32,
    pub(crate) pointer_captured: bool,
}
impl EditorState {
    /// Set the editor orbit camera; angles are radians and distances are metres.
    pub fn set_view(&mut self, yaw: f32, pitch: f32, distance: f32, focus: f32) {
        if [yaw, pitch, distance, focus].iter().all(|v| v.is_finite()) {
            self.yaw = yaw;
            self.pitch = pitch.clamp(-1.2, 1.2);
            self.distance = distance.clamp(0.4, 8.0);
            self.focus = focus;
        }
    }
}
impl Default for EditorState {
    fn default() -> Self {
        Self {
            selected: None,
            tracked: None,
            anchor: Transform::default(),
            original_body: RigidBody::Kinematic,
            original_axes: None,
            path: "exports/character.json".into(),
            status: "Ready • MakeHuman CC0 base loaded".into(),
            category: "Body".into(),
            history: vec![],
            future: vec![],
            gesture_start: None,
            saved: CharacterRecipe::default(),
            physics: false,
            collider: false,
            yaw: 0.0,
            pitch: 0.05,
            distance: 3.5,
            focus: 0.9,
            pointer_captured: false,
        }
    }
}
#[derive(Component)]
pub struct EditorCamera;
fn setup(
    mut commands: Commands,
    mut state: ResMut<EditorState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    state.selected = Some(
        commands
            .spawn((
                Name::new("Editable human"),
                CharacterShape(CharacterRecipe {
                    name: "character".into(),
                    customization: Some(crate::ara::AraRecipe::default()),
                    ..default()
                }),
                RigidBody::Kinematic,
                Transform::default(),
            ))
            .id(),
    );
    commands.spawn((
        Camera3d::default(),
        EditorCamera,
        Transform::from_xyz(0., 1.1, 3.5).looking_at(Vec3::Y * 0.9, Vec3::Y),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(200., 200.))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.065, 0.08, 0.10),
            perceptual_roughness: 1.,
            ..default()
        })),
        RigidBody::Static,
        // The plane depicts the collider's top surface, not its centre.
        Collider::compound(vec![(
            Vec3::new(0., -0.05, 0.),
            Quat::IDENTITY,
            Collider::cuboid(200., 0.1, 200.),
        )]),
        Transform::default(),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 6500.,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-3., 6., 4.).looking_at(Vec3::Y, Vec3::Y),
    ));
    commands.spawn((
        PointLight {
            intensity: 180_000.,
            color: Color::srgb(0.55, 0.72, 1.),
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(3., 2.5, -2.),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 130.,
        affects_lightmapped_meshes: true,
    });
}

pub(crate) fn editor_ui(
    mut contexts: EguiContexts,
    mut ara: ResMut<crate::ara_editor::AraEditorState>,
    mut state: ResMut<EditorState>,
    library: Res<CharacterLibrary>,
    mut characters: Query<(
        &mut CharacterShape,
        &mut Transform,
        &RigidBody,
        Option<&LockedAxes>,
        Option<&CharacterBuildError>,
    )>,
    mut commands: Commands,
) -> Result {
    if ara.enabled {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    ctx.set_visuals(egui::Visuals::dark());
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(10., 10.);
    style.visuals.panel_fill = egui::Color32::from_rgb(21, 27, 37);
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(35, 103, 113);
    ctx.set_style_of(egui::Theme::Dark, style);
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "editor-viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    let Some(entity) = state.selected else {
        return Ok(());
    };
    let Ok((mut shape, mut transform, _body, axes, error)) = characters.get_mut(entity) else {
        return Ok(());
    };
    let before = shape.0.clone();
    let mut recipe = before.clone();
    let mut history_action = false;
    let mut save = false;
    let mut load = false;
    let mut export = false;
    let mut reset_physics = false;
    egui::Panel::top("toolbar").show(&mut viewport_ui, |ui| {
        ui.add_space(7.);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("HUMAN STUDIO")
                    .strong()
                    .size(21.)
                    .color(egui::Color32::from_rgb(127, 222, 208)),
            );
            ui.separator();
            ui.label("CHARACTER LAB");
            if ui.button("Full customization").clicked() {
                ara.enabled = true;
                recipe.enter_full_editor();
            }
            ui.separator();
            if ui
                .add_enabled(
                    !state.physics && !state.history.is_empty(),
                    egui::Button::new("Undo"),
                )
                .clicked()
            {
                if let Some(previous) = state.history.pop() {
                    state.future.push(recipe.clone());
                    recipe = previous;
                    history_action = true;
                }
            }
            if ui
                .add_enabled(
                    !state.physics && !state.future.is_empty(),
                    egui::Button::new("Redo"),
                )
                .clicked()
            {
                if let Some(next) = state.future.pop() {
                    state.history.push(recipe.clone());
                    recipe = next;
                    history_action = true;
                }
            }
            if ui.button("Save preset").clicked() {
                save = true;
            }
            if ui
                .add_enabled(!state.physics, egui::Button::new("Load preset"))
                .clicked()
            {
                load = true;
            }
            if ui.button("Export OBJ").clicked() {
                export = true;
            }
            if recipe != state.saved {
                ui.label(
                    egui::RichText::new("● Unsaved").color(egui::Color32::from_rgb(238, 192, 106)),
                );
            }
        });
        ui.add_space(7.);
    });
    egui::Panel::bottom("status").show(&mut viewport_ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(&state.status);
            if let Some(error) = error {
                ui.colored_label(egui::Color32::LIGHT_RED, &error.0);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label("Y-UP  /  METERS  /  AVIAN 3D");
            });
        });
    });
    egui::Panel::left("morphs")
        .default_size(280.)
        .show(&mut viewport_ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(12.);
                ui.heading("Shape your character");
                ui.label(egui::RichText::new("MakeHuman · hm08 topology").weak());
                ui.separator();
                ui.add_enabled_ui(!state.physics, |ui| {
                    ui.label("CHARACTER NAME");
                    ui.add(
                        egui::TextEdit::singleline(&mut recipe.name)
                            .char_limit(64)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(10.);
                    ui.horizontal_wrapped(|ui| {
                        for category in ["Body", "Face", "Torso", "Limbs"] {
                            ui.selectable_value(&mut state.category, category.into(), category);
                        }
                    });
                    ui.separator();
                    for target in &library.targets {
                        let definition = &target.definition;
                        if definition.category != state.category {
                            continue;
                        }
                        ui.label(&definition.label);
                        let mut weight = recipe.weights.get(&definition.id).copied().unwrap_or(0.);
                        ui.horizontal(|ui| {
                            ui.add(egui::Slider::new(&mut weight, -1.0..=1.0).fixed_decimals(2));
                            if ui
                                .small_button("↺")
                                .on_hover_text("Reset this morph")
                                .clicked()
                            {
                                weight = 0.;
                            }
                        });
                        if weight == 0. {
                            recipe.weights.remove(&definition.id);
                        } else {
                            recipe.weights.insert(definition.id.clone(), weight);
                        }
                    }
                    ui.add_space(20.);
                    ui.separator();
                    ui.label("SKIN MATERIAL");
                    let mut color = recipe.skin_color.map(|v| (v * 255.).round() as u8);
                    if ui.color_edit_button_srgb(&mut color).changed() {
                        recipe.skin_color = color.map(|v| v as f32 / 255.);
                    }
                    ui.horizontal(|ui| {
                        for color in [[0.88, 0.70, 0.56], [0.67, 0.43, 0.29], [0.35, 0.20, 0.14]] {
                            if ui
                                .add(egui::Button::new("  ").fill(egui::Color32::from_rgb(
                                    (color[0] * 255.) as u8,
                                    (color[1] * 255.) as u8,
                                    (color[2] * 255.) as u8,
                                )))
                                .clicked()
                            {
                                recipe.skin_color = color;
                            }
                        }
                    });
                    ui.add_space(12.);
                    if ui.button("Reset all shapes").clicked() {
                        recipe.weights.clear();
                    }
                });
                if state.physics {
                    ui.label("Stop physics preview to edit shape.");
                }
                ui.add_space(20.);
                ui.label(
                    egui::RichText::new(format!(
                        "{} paired morphs\nDeterministic, sparse target blending",
                        library.targets.len()
                    ))
                    .weak(),
                );
            });
        });
    egui::Panel::right("inspector").default_size(260.).show(&mut viewport_ui, |ui| {
        ui.add_space(12.); ui.heading("Scene inspector"); ui.separator();
        ui.label("VIEW");
        ui.horizontal(|ui| {
            if ui.button("Front").clicked() { state.yaw = 0.; state.pitch = 0.05; }
            if ui.button("Side").clicked() { state.yaw = std::f32::consts::FRAC_PI_2; }
            if ui.button("Back").clicked() { state.yaw = std::f32::consts::PI; }
        });
        if ui.button("Frame body").clicked() { state.distance = 3.5; state.focus = 0.9; state.pitch = 0.05; }
        if ui.button("Frame face").clicked() { state.distance = 0.8; state.focus = 1.55; state.pitch = 0.; }
        ui.add(egui::Slider::new(&mut state.distance, 0.4..=8.).text("Zoom"));
        ui.add(egui::Slider::new(&mut state.focus, 0.0..=2.5).text("Focus height"));
        ui.add_space(12.); ui.separator(); ui.label("PHYSICS");
        ui.checkbox(&mut state.collider, "Show locomotion capsule");
        let was_physics = state.physics;
        ui.checkbox(&mut state.physics, "Drop test");
        if was_physics != state.physics {
            if state.physics { state.anchor = *transform; state.original_body = *_body;
        state.original_axes = axes.copied(); }
            reset_physics = true;
        }
        if ui.button("Reset position").clicked() { reset_physics = true; }
        ui.label(egui::RichText::new("A dynamic Avian body tests ground contact. The capsule updates with body height.").weak());
        ui.add_space(12.); ui.separator(); ui.label("ASSET");
        ui.label(format!("{} source vertices", library.base_positions().len()));
        ui.label(format!("{} visible triangles", library.triangles().len()/3));
        if let Ok(positions) = library.deform(&recipe) {
            let height = library.triangles().iter().map(|&i| positions[i as usize][1]).fold(0.0_f32, f32::max);
            ui.label(format!("Height: {height:.2} m"));
        }
        ui.add_space(12.); ui.separator(); ui.label("PRESET PATH");
        ui.add(egui::TextEdit::singleline(&mut state.path).desired_width(f32::INFINITY));
        ui.label(egui::RichText::new("OBJ exports beside the preset.\nRecipes keep editable target weights.").weak());
    });
    let viewport = viewport_ui.available_rect_before_wrap();
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(&mut viewport_ui, |ui| {
            ui.label(egui::RichText::new("LIVE PREVIEW").color(egui::Color32::from_gray(150)));
            ui.label(
                egui::RichText::new("Right-drag to orbit · Scroll to zoom")
                    .small()
                    .weak(),
            );
        });
    if load {
        match CharacterRecipe::load(&state.path, &library) {
            Ok(loaded) => {
                recipe = loaded.clone();
                state.saved = loaded;
                state.status = "Preset loaded".into();
            }
            Err(error) => state.status = format!("Load failed: {error}"),
        }
    }
    // Store one history entry per pointer gesture, not one per frame of a slider drag.
    if recipe != before && !history_action {
        if state.gesture_start.is_none() {
            state.gesture_start = Some(before.clone());
            state.future.clear();
        }
    }
    if !ctx.input(|input| input.pointer.any_down()) {
        if let Some(start) = state.gesture_start.take() {
            if start != recipe {
                state.history.push(start);
                if state.history.len() > 100 {
                    state.history.remove(0);
                }
            }
        }
    }
    if recipe != before {
        shape.0 = recipe.clone();
    }
    if save || export {
        let path = PathBuf::from(&state.path);
        let result = (|| -> std::result::Result<(), crate::ModelError> {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            if save {
                recipe.save(&path, &library)?;
            } else {
                library.export_obj(&recipe, path.with_extension("obj"))?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                if save {
                    state.saved = recipe;
                }
                state.status = if save {
                    format!("Saved {}", path.display())
                } else {
                    format!("Exported {}", path.with_extension("obj").display())
                };
            }
            Err(error) => state.status = format!("Write failed: {error}"),
        }
    }
    if reset_physics {
        commands.entity(entity).insert(if state.physics {
            RigidBody::Dynamic
        } else {
            state.original_body
        });
        *transform = state.anchor;
        if state.physics {
            transform.translation.y += 0.8;
        }
        commands
            .entity(entity)
            .insert((LinearVelocity::ZERO, AngularVelocity::ZERO));
        if state.physics {
            commands.entity(entity).insert(LockedAxes::ROTATION_LOCKED);
        } else if let Some(axes) = state.original_axes {
            commands.entity(entity).insert(axes);
        } else {
            commands.entity(entity).remove::<LockedAxes>();
        }
        commands.entity(entity).remove::<Sleeping>();
    }
    state.pointer_captured = ctx.egui_is_using_pointer()
        || ctx
            .pointer_hover_pos()
            .is_none_or(|p| !viewport.contains(p));
    Ok(())
}

pub(crate) fn orbit_camera(
    mut state: ResMut<EditorState>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mut camera: Query<&mut Transform, With<EditorCamera>>,
) {
    if !state.pointer_captured {
        if buttons.pressed(MouseButton::Right) {
            state.yaw -= motion.delta.x * 0.007;
            state.pitch = (state.pitch + motion.delta.y * 0.007).clamp(-1.2, 1.2);
        }
        state.distance = (state.distance * (1. - scroll.delta.y * 0.08)).clamp(0.4, 8.);
    }
    let target = state.anchor.translation + Vec3::Y * state.focus;
    let offset = Quat::from_rotation_y(state.yaw)
        * Quat::from_rotation_x(-state.pitch)
        * Vec3::Z
        * state.distance;
    for mut transform in &mut camera {
        *transform = Transform::from_translation(target + offset).looking_at(target, Vec3::Y);
    }
}
fn collider_gizmo(
    state: Res<EditorState>,
    library: Res<CharacterLibrary>,
    shapes: Query<(&CharacterShape, &GlobalTransform)>,
    mut gizmos: Gizmos,
) {
    // Use the same dimensions as the runtime collider. Rings make its approximation visible.
    if !state.collider {
        return;
    }
    let Some(entity) = state.selected else {
        return;
    };
    let Ok((shape, transform)) = shapes.get(entity) else {
        return;
    };
    let Ok(positions) = library.deform(&shape.0) else {
        return;
    };
    let height = library
        .triangles()
        .iter()
        .map(|&i| positions[i as usize][1])
        .fold(0.0_f32, f32::max)
        .max(0.2);
    let radius = (height * 0.13).clamp(0.05, height * 0.25);
    let origin = transform.translation();
    let color = Color::srgb(0.3, 0.95, 0.8);
    for y in [radius, height - radius] {
        gizmos.circle(
            Isometry3d::new(
                origin + Vec3::Y * y,
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ),
            radius,
            color,
        );
    }
    for direction in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
        gizmos.line(
            origin + Vec3::Y * radius + direction * radius,
            origin + Vec3::Y * (height - radius) + direction * radius,
            color,
        );
    }
}

fn sync_selection(
    mut commands: Commands,
    mut state: ResMut<EditorState>,
    mut characters: Query<(
        &CharacterShape,
        &mut Transform,
        &RigidBody,
        Option<&LockedAxes>,
    )>,
) {
    if state.tracked == state.selected {
        return;
    }
    if state.physics {
        if let Some(previous) = state.tracked {
            if let Ok((_, mut transform, _, _)) = characters.get_mut(previous) {
                *transform = state.anchor;
                let mut entity = commands.entity(previous);
                entity.insert((
                    state.original_body,
                    LinearVelocity::ZERO,
                    AngularVelocity::ZERO,
                ));
                if let Some(axes) = state.original_axes {
                    entity.insert(axes);
                } else {
                    entity.remove::<LockedAxes>();
                }
                entity.remove::<Sleeping>();
            }
        }
    }
    state.physics = false;
    state.tracked = None;
    state.history.clear();
    state.future.clear();
    state.gesture_start = None;
    if let Some(selected) = state.selected {
        if let Ok((shape, transform, body, axes)) = characters.get_mut(selected) {
            state.tracked = Some(selected);
            state.anchor = *transform;
            state.original_body = *body;
            state.original_axes = axes.copied();
            state.saved = shape.0.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switching_or_clearing_selection_restores_active_drop_preview() {
        for clear in [false, true] {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .init_resource::<EditorState>()
                .add_systems(Update, sync_selection);
            let origin = Transform::from_xyz(2., 0.4, 3.);
            let first = app
                .world_mut()
                .spawn((CharacterShape::default(), origin, RigidBody::Kinematic))
                .id();
            let second = app
                .world_mut()
                .spawn((
                    CharacterShape::default(),
                    Transform::default(),
                    RigidBody::Kinematic,
                ))
                .id();
            app.world_mut().resource_mut::<EditorState>().selected = Some(first);
            app.update();
            app.world_mut().resource_mut::<EditorState>().physics = true;
            app.world_mut().entity_mut(first).insert((
                RigidBody::Dynamic,
                Transform::from_xyz(2., 1.2, 3.),
                LockedAxes::ROTATION_LOCKED,
            ));
            app.world_mut().resource_mut::<EditorState>().selected =
                if clear { None } else { Some(second) };
            app.update();
            assert_eq!(*app.world().get::<Transform>(first).unwrap(), origin);
            assert_eq!(
                *app.world().get::<RigidBody>(first).unwrap(),
                RigidBody::Kinematic
            );
            assert!(app.world().get::<LockedAxes>(first).is_none());
            assert!(!app.world().resource::<EditorState>().physics);
        }
    }
}
