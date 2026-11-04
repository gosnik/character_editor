//! Complete native customization UI; uses the same CharacterShape in games and standalone.
use crate::{
    CharacterBuildError, CharacterLibrary, CharacterRecipe, CharacterShape,
    ara::{AraRecipe, CustomTarget, native_catalog, native_catalog_with_assets},
    editor::EditorState,
    native_runtime::{NativeAssets, NativeBuildQueue, NativeInstance},
    wardrobe::{WardrobeProfile, test_profile},
    wardrobe_assets::{CustomGarment, CustomGarmentCatalog, GarmentSlot},
};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
#[derive(Resource)]
pub struct AraEditorState {
    pub enabled: bool,
    schema: Arc<Value>,
    category: String,
    search: String,
    clip_search: String,
    path: String,
    target_path: String,
    status: String,
    history: Vec<CharacterRecipe>,
    future: Vec<CharacterRecipe>,
    gesture: Option<CharacterRecipe>,
    saved: Option<CharacterRecipe>,
    tracked: Option<Entity>,
    seed: u64,
    exporting: Option<std::sync::Mutex<std::sync::mpsc::Receiver<Result<String, String>>>>,
    wardrobe_path: String,
    wardrobe_name: String,
    garment_path: String,
    wardrobe_testing: Option<std::sync::Mutex<std::sync::mpsc::Receiver<Result<String, String>>>>,
}
impl Default for AraEditorState {
    fn default() -> Self {
        Self {
            enabled: true,
            schema: Arc::new(native_catalog()),
            category: "Body".into(),
            search: String::new(),
            clip_search: String::new(),
            path: "exports/character.json".into(),
            target_path: String::new(),
            status: "Native MakeHuman customization".into(),
            history: vec![],
            future: vec![],
            gesture: None,
            saved: None,
            tracked: None,
            seed: 1,
            exporting: None,
            wardrobe_path: "exports/wardrobe.json".into(),
            wardrobe_name: "My wardrobe".into(),
            garment_path: String::new(),
            wardrobe_testing: None,
        }
    }
}
pub struct NativeEditorUiPlugin;
impl Plugin for NativeEditorUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AraEditorState>().add_systems(
            EguiPrimaryContextPass,
            native_ui
                .after(crate::editor::editor_ui)
                .before(crate::editor::orbit_camera),
        );
    }
}
pub(crate) fn native_ui(
    mut contexts: EguiContexts,
    mut state: ResMut<AraEditorState>,
    mut view: ResMut<EditorState>,
    library: Res<CharacterLibrary>,
    assets: Option<Res<NativeAssets>>,
    queue: Res<NativeBuildQueue>,
    mut shapes: Query<(
        &mut CharacterShape,
        Option<&mut NativeInstance>,
        Option<&CharacterBuildError>,
    )>,
) -> Result {
    if !state.enabled {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "native_editor_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    let Some(selected) = view.selected else {
        return Ok(());
    };
    let Ok((mut shape, mut instance, error)) = shapes.get_mut(selected) else {
        return Ok(());
    };
    if state.tracked != Some(selected) {
        state.tracked = Some(selected);
        if let Some(assets) = &assets {
            state.schema = Arc::new(native_catalog_with_assets(&assets.0.root));
        }
        state.history.clear();
        state.future.clear();
        state.gesture = None;
        state.saved = Some(shape.0.clone());
    }
    let before = shape.0.clone();
    let mut current = before.clone();
    current.customization.get_or_insert_with(AraRecipe::default);
    let schema = state.schema.clone();
    let mut save = false;
    let mut load = false;
    let mut export = false;
    let mut history = 0i8;
    let mut import_mhm = false;
    let mut export_mhm = false;
    if let Some(result) = state
        .exporting
        .as_ref()
        .and_then(|r| r.lock().ok()?.try_recv().ok())
    {
        state.status = match result {
            Ok(s) => s,
            Err(e) => format!("Export failed: {e}"),
        };
        state.exporting = None;
    }
    if let Some(result) = state
        .wardrobe_testing
        .as_ref()
        .and_then(|r| r.lock().ok()?.try_recv().ok())
    {
        state.status = match result {
            Ok(s) => s,
            Err(e) => format!("Wardrobe test failed: {e}"),
        };
        state.wardrobe_testing = None;
    }
    egui::Panel::top("native_toolbar").show(&mut viewport_ui, |ui| {
        ui.add_space(5.);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("HUMAN STUDIO")
                    .size(21.)
                    .strong()
                    .color(egui::Color32::from_rgb(126, 224, 208)),
            );
            ui.separator();
            ui.label("NATIVE CHARACTER LAB");
            if ui
                .add_enabled(!state.history.is_empty(), egui::Button::new("Undo"))
                .clicked()
            {
                history = -1;
            }
            if ui
                .add_enabled(!state.future.is_empty(), egui::Button::new("Redo"))
                .clicked()
            {
                history = 1;
            }
            if ui.button("Save preset").clicked() {
                save = true;
            }
            if ui.button("Load preset").clicked() {
                load = true;
            }
            if ui
                .add_enabled(
                    instance.is_some() && state.exporting.is_none(),
                    egui::Button::new("Export character"),
                )
                .clicked()
            {
                export = true;
            }
            if ui.button("Basic editor").clicked() {
                state.enabled = false;
                current.enter_basic_editor();
            }
        });
        ui.add_space(5.);
    });
    // A single recipe drives controls, generation, save files, and in-game instances.
    if current.customization.is_none() {
        shape.0 = current;
        return Ok(());
    }
    let recipe = current.customization.as_mut().unwrap();
    egui::Panel::bottom("native_status").show(&mut viewport_ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(&state.status);
            ui.separator();
            ui.label(&queue.status);
            if let Some(e) = error {
                ui.colored_label(egui::Color32::LIGHT_RED, &e.0);
            }
        });
    });
    egui::Panel::left("native_controls").default_size(340.).show(&mut viewport_ui,|ui|{
        egui::ScrollArea::vertical().show(ui,|ui|{
            ui.heading("Build your character");ui.label("Rust · Bevy · Avian3D");ui.separator();
            egui::ComboBox::from_id_salt("native_presets").selected_text("Choose a complete preset…").width(310.).show_ui(ui,|ui|{
                for p in schema["presets"].as_array().unwrap(){if ui.selectable_label(false,p["name"].as_str().unwrap()).clicked(){match AraRecipe::from_reference(p["params"].clone()){Ok(r)=>*recipe=r,Err(e)=>state.status=e}}}
            });
            ui.horizontal(|ui|{ui.add(egui::DragValue::new(&mut state.seed).prefix("Seed "));if ui.button("Surprise me").clicked(){*recipe=AraRecipe::random(state.seed);state.seed=state.seed.wrapping_add(1);}});
            ui.checkbox(&mut recipe.cartoon.enabled, "Cartoon mode")
                .on_hover_text("Cartoon proportions on top of the MakeHuman body.");
            if recipe.cartoon.enabled {
                ui.add(egui::Slider::new(&mut recipe.cartoon.strength, 0.0..=1.0).text("Cartoon"))
                    .on_hover_text("0 = realistic MakeHuman, 1 = full cartoon proportions.");
                ui.small("Gender, age, weight, breasts and every detail slider keep their full MakeHuman range on top of the cartoon shape.");
            }
            ui.separator();
            ui.horizontal_wrapped(|ui|{for c in ["Body","Skin","Face","Hair","Looks","Clothing","Props","Rig","Details","MPFB"]{ui.selectable_value(&mut state.category,c.into(),c);}});ui.separator();
            if state.category=="Details"{
                ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Search detail controls"));
                if ui.button("Reset detail morphs").clicked(){recipe.params["modifiers"]=json!({});}
                for group in schema["sliders"].as_array().unwrap(){let sliders=group["sliders"].as_array().unwrap();let query=state.search.to_lowercase();
                    let filtered:Vec<_>=sliders.iter().filter(|s|query.is_empty()||s["label"].as_str().unwrap_or("").to_lowercase().contains(&query)||s["id"].as_str().unwrap_or("").contains(&query)).collect();
                    if filtered.is_empty(){continue;}
                    egui::CollapsingHeader::new(format!("{} / {}",group["category"].as_str().unwrap_or("Shape"),group["group"].as_str().unwrap_or("Details"))).id_salt(format!("{}{}",group["category"],group["group"])).default_open(!query.is_empty()).show(ui,|ui|{
                        for slider in filtered{let id=slider["id"].as_str().unwrap();let mut value=recipe.params["modifiers"][id].as_f64().unwrap_or(0.) as f32;
                            ui.label(slider["label"].as_str().unwrap_or(id));ui.horizontal(|ui|{ui.add(egui::Slider::new(&mut value,if slider["bipolar"]==true{-1.}else{0.}..=1.).fixed_decimals(2));if ui.small_button("↺").clicked(){value=0.;}});
                            if value==0.{recipe.params["modifiers"].as_object_mut().unwrap().remove(id);}else{recipe.params["modifiers"][id]=json!(value);}
                        }
                    });
                }
            }else if state.category=="MPFB"{
                if recipe.cartoon.enabled { ui.small("Imported targets apply as authored, on top of the cartoon shape."); }
                ui.heading("MakeHuman / MPFB assets");ui.label("Import a compatible .target or .target.gz. Original hm08 vertex indices are preserved.");
                ui.add(egui::TextEdit::singleline(&mut state.target_path).hint_text("/path/to/custom.target.gz"));
                if ui.button("Import custom target").clicked(){match crate::mpfb::load_target(&state.target_path,19158){Ok(deltas)=>{let name=PathBuf::from(&state.target_path).file_name().unwrap_or_default().to_string_lossy().into_owned();let name=unique_target_name(recipe,&name);recipe.custom_targets.push(CustomTarget{name,weight:1.,deltas});state.status="Custom target imported".into();},Err(e)=>state.status=e}}
                let mut remove=None;let mut mirror=None;
                for (i,target) in recipe.custom_targets.iter_mut().enumerate(){ui.label(&target.name);ui.add(egui::Slider::new(&mut target.weight,-1.0..=1.0));ui.horizontal(|ui|{if ui.button("Mirror X").clicked(){mirror=Some(i);}if ui.button("Remove").clicked(){remove=Some(i);}});}
                if let Some(i)=remove{recipe.custom_targets.remove(i);}
                if let (Some(i),Some(assets))=(mirror,assets.as_ref()) {if let Some(target)=recipe.custom_targets.get(i){let base=assets.0.base.arrays["positions"].f32().unwrap().chunks_exact(3).map(|p|[p[0],p[1],p[2]]).collect::<Vec<_>>();match crate::mpfb::mirror_target(&target.deltas,&base,0.01){Ok(deltas)=>recipe.custom_targets.push(CustomTarget{name:unique_target_name(recipe,&format!("{} mirrored",target.name)),weight:target.weight,deltas}),Err(e)=>state.status=e}}}
                ui.separator();ui.label("MHM uses the preset path below with .mhm extension.");if ui.button("Import MHM").clicked(){import_mhm=true;}if ui.button("Export MHM").clicked(){export_mhm=true;}
            }else{
                if state.category=="Body" {
                    ui.collapsing("Breast shape and motion",|ui|{
                        let b=&mut recipe.breasts;
                        ui.add(egui::Slider::new(&mut b.volume,0.0..=2.5).text("Volume"));
                        ui.add(egui::Slider::new(&mut b.projection,0.2..=2.5).text("Projection"));
                        ui.add(egui::Slider::new(&mut b.lift,-1.0..=1.0).text("Lift"));
                        ui.add(egui::Slider::new(&mut b.separation,-0.5..=0.5).text("Separation"));
                        ui.checkbox(&mut b.dynamics,"Secondary motion");
                        ui.add(egui::Slider::new(&mut b.motion_strength,0.0..=2.0).text("Motion strength"));
                        ui.add(egui::Slider::new(&mut b.stiffness,2.0..=80.0).text("Stiffness"));
                        ui.add(egui::Slider::new(&mut b.damping,0.1..=2.0).text("Damping"));
                        if ui.button("Reset breast shape and motion").clicked(){*b=Default::default();}
                    });
                }
                if state.category=="Clothing" {
                    ui.heading("Wardrobe profiles");
                    ui.add(egui::TextEdit::singleline(&mut state.wardrobe_path).hint_text("exports/wardrobe.json"));
                    ui.add(egui::TextEdit::singleline(&mut state.wardrobe_name).hint_text("Wardrobe name"));
                    ui.horizontal(|ui| {
                        if ui.button("Save profile").clicked() {
                            match WardrobeProfile::capture(recipe, &state.wardrobe_name, true)
                                .and_then(|profile| profile.save(&state.wardrobe_path))
                            {
                                Ok(()) => state.status = format!("Saved wardrobe {}", state.wardrobe_path),
                                Err(error) => state.status = error,
                            }
                        }
                        if ui.button("Apply profile").clicked() {
                            match WardrobeProfile::load(&state.wardrobe_path)
                                .and_then(|profile| profile.apply(recipe))
                            {
                                Ok(()) => state.status = "Applied wardrobe; body settings were preserved".into(),
                                Err(error) => state.status = error,
                            }
                        }
                    });
                    if state.wardrobe_testing.is_some() {
                        ui.label("Testing current fit in the background…");
                    } else if ui.button("Test current fit").clicked() {
                        match WardrobeProfile::capture(recipe, &state.wardrobe_name, true) {
                            Err(error) => state.status = error,
                            Ok(profile) => {
                                let base = recipe.clone();
                                let clips = if base.preview_clip.is_empty() { vec![] } else { vec![base.preview_clip.clone()] };
                                let report_path = PathBuf::from(&state.wardrobe_path).with_extension("fit-report.json");
                                let library = assets.as_ref().map(|assets| assets.0.clone());
                                if let Some(library) = library {
                                    let (sender, receiver) = std::sync::mpsc::channel();
                                    state.wardrobe_testing = Some(std::sync::Mutex::new(receiver));
                                    state.status = "Testing wardrobe fit…".into();
                                    std::thread::spawn(move || {
                                        let result = (|| -> Result<String, String> {
                                            let report = test_profile(
                                                &library,
                                                &profile,
                                                &[("Current character".into(), base)],
                                                &clips,
                                                8,
                                            )?;
                                            if let Some(parent) = report_path.parent().filter(|path| !path.as_os_str().is_empty()) {
                                                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                                            }
                                            let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
                                            crate::model::atomic_write(&report_path, &bytes).map_err(|error| error.to_string())?;
                                            if report["ok"] == true {
                                                Ok(format!("Wardrobe fit passed: {}", report_path.display()))
                                            } else {
                                                Err(format!("fit warnings or failures saved to {}", report_path.display()))
                                            }
                                        })();
                                        let _ = sender.send(result);
                                    });
                                } else {
                                    state.status = "Native assets are required to test wardrobe fit".into();
                                }
                            }
                        }
                    }
                    ui.separator();
                    ui.heading("Custom garment JSON");
                    ui.add(egui::TextEdit::singleline(&mut state.garment_path).hint_text("/path/to/garment.json"));
                    if ui.button("Add or replace JSON garment").clicked() {
                        let result = (|| {
                            use std::io::Read;
                            let file = std::fs::File::open(&state.garment_path).map_err(|e| e.to_string())?;
                            let mut bytes = Vec::new();
                            file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                            Ok::<_, String>(bytes)
                        })()
                            .and_then(|bytes| {
                                if bytes.len() > 64 * 1024 * 1024 { Err("garment JSON exceeds 64 MiB".into()) }
                                else { serde_json::from_slice::<CustomGarment>(&bytes).map_err(|error| format!("garment JSON: {error}")) }
                            })
                            .and_then(|garment| {
                                let name = garment.name.clone();
                                recipe.select_custom_garment(&garment).map(|()| name)
                            });
                        match result {
                            Ok(name) => state.status = format!("Added custom garment {name}"),
                            Err(error) => state.status = error,
                        }
                    }
                    let mut remove_garment = None;
                    for garment in &recipe.custom_garments {
                        ui.horizontal(|ui| {
                            ui.label(format!("{} ({:?})", garment.name, garment.slot));
                            if ui.small_button("Remove").clicked() { remove_garment = Some(garment.id.clone()); }
                        });
                    }
                    if let Some(id) = remove_garment {
                        recipe.custom_garments.retain(|garment| garment.id != id);
                        state.status = format!("Removed custom garment {id}");
                    }
                    if let Some(assets) = assets.as_ref() {
                        draw_custom_garment_catalog(ui, recipe, &assets.1, &mut state.status, &[
                            (GarmentSlot::Top, "Top"),
                            (GarmentSlot::Bottom, "Bottom"),
                            (GarmentSlot::Jacket, "Jacket"),
                            (GarmentSlot::Feet, "Feet"),
                            (GarmentSlot::Hat, "Hat"),
                            (GarmentSlot::Accessory, "Accessory"),
                        ], "Custom wardrobe");
                    } else {
                        ui.colored_label(egui::Color32::LIGHT_RED, "Custom garments require native assets.");
                    }
                }
                if state.category=="Hair" {
                    if let Some(assets) = assets.as_ref() {
                        draw_custom_garment_catalog(ui, recipe, &assets.1, &mut state.status,
                            &[(GarmentSlot::Hair, "Imported hair"), (GarmentSlot::Beard, "Imported beard")], "Custom hair & beards");
                    } else {
                        ui.colored_label(egui::Color32::LIGHT_RED, "Custom hair requires native assets.");
                    }
                }
                let outfit_before=recipe.params["outfit"].clone();
                let hair_before=recipe.params["hair"].clone();
                let beard_before=recipe.params["facialHair"].clone();
                for field in schema["fields"].as_array().unwrap(){if field["category"].as_str()!=Some(&state.category){continue;}draw_field(ui,&mut recipe.params,field);}
                if state.category=="Hair"&&recipe.params["hair"]!=hair_before{recipe.remove_custom_garment_slot(&GarmentSlot::Hair);}
                if state.category=="Hair"&&recipe.params["facialHair"]!=beard_before{recipe.remove_custom_garment_slot(&GarmentSlot::Beard);}
                if recipe.params["outfit"]!=outfit_before{
                    if let Some(set)=schema["outfit_sets"].as_array().unwrap().iter().find(|s|s["id"]==recipe.params["outfit"]){
                        for key in ["shoes","carry","hat"]{if set[key].is_string(){recipe.params[key]=set[key].clone();}}
                        recipe.params["accessories"]=set["accessories"].clone();
                    }
                }
                if state.category=="Clothing"||state.category=="Props"{
                    ui.separator();ui.heading("Fabric & finish");
                    for garment in schema["garments"].as_array().unwrap(){let id=garment["id"].as_str().unwrap();
                        let worn=["outfit","top","jacket","bottom","shoes","hat","carry"].iter().any(|k|recipe.params[k]==id)||recipe.params["accessories"].as_array().is_some_and(|a|a.iter().any(|v|v==id));
                        let in_set=schema["outfit_sets"].as_array().unwrap().iter().find(|s|s["id"]==recipe.params["outfit"]).is_some_and(|s|["top","bottom","jacket"].iter().any(|k|s[k]==id));
                        if !(worn||in_set){continue;}
                        egui::CollapsingHeader::new(garment["label"].as_str().unwrap_or(id)).id_salt(id).show(ui,|ui|{
                            if recipe.params["garmentStyles"][id].is_null(){recipe.params["garmentStyles"][id]=json!({});}
                            for (key,label) in [("color","Color"),("color2","Accent")]{let default=garment[key].as_str().unwrap_or("#777777");let val=recipe.params["garmentStyles"][id][key].as_str().unwrap_or(default).to_owned();let mut value=crate::ara::rgb(&val).map(|v|(v*255.) as u8);ui.horizontal(|ui|{ui.label(label);if ui.color_edit_button_srgb(&mut value).changed(){recipe.params["garmentStyles"][id][key]=json!(format!("#{:02x}{:02x}{:02x}",value[0],value[1],value[2]));}});}
                            let mut pattern=recipe.params["garmentStyles"][id]["pattern"].as_str().or_else(||garment["patterns"][0].as_str()).unwrap_or("solid").to_owned();
                            egui::ComboBox::from_id_salt((id,"fabric")).selected_text(&pattern).show_ui(ui,|ui|{if let Some(patterns)=garment["patterns"].as_array(){for p in patterns{let v=p.as_str().unwrap();ui.selectable_value(&mut pattern,v.into(),v);}}});
                            recipe.params["garmentStyles"][id]["pattern"]=json!(pattern);
                        });
                    }
                }
            }
        });
    });
    egui::Panel::right("native_scene").default_size(270.).show(&mut viewport_ui,|ui|{
        egui::ScrollArea::vertical().show(ui,|ui|{ui.heading("Preview & export");ui.separator();
            ui.horizontal(|ui|{if ui.button("Front").clicked(){view.yaw=0.;}if ui.button("Side").clicked(){view.yaw=std::f32::consts::FRAC_PI_2;}if ui.button("Back").clicked(){view.yaw=std::f32::consts::PI;}});
            if ui.button("Frame body").clicked(){view.distance=3.7;view.focus=instance.as_ref().map(|i|i.character.height*0.52).unwrap_or(0.9);}
            if ui.button("Frame face").clicked(){view.distance=0.85;view.focus=instance.as_ref().map(|i|i.character.height*0.92).unwrap_or(1.6);}
            ui.add(egui::Slider::new(&mut view.distance,0.4..=8.).text("Zoom"));ui.add(egui::Slider::new(&mut view.focus,0.0..=2.5).text("Focus"));
            ui.separator();ui.label("ANIMATION");
            ui.horizontal(|ui| { ui.label("Find animation"); ui.text_edit_singleline(&mut state.clip_search); });
            let clip_filter = state.clip_search.to_lowercase();
            egui::ComboBox::from_id_salt("native_clip").selected_text(schema["clips"].as_array().unwrap().iter().find(|c|c["id"]==recipe.preview_clip).and_then(|c|c["name"].as_str()).unwrap_or("Rest pose")).width(240.).show_ui(ui,|ui|{
                ui.selectable_value(&mut recipe.preview_clip,String::new(),"Rest pose");for clip in schema["clips"].as_array().unwrap().iter().filter(|clip| clip["name"].as_str().unwrap_or("").to_lowercase().contains(&clip_filter) || clip["id"].as_str().unwrap_or("").to_lowercase().contains(&clip_filter)){ui.selectable_value(&mut recipe.preview_clip,clip["id"].as_str().unwrap().into(),clip["name"].as_str().unwrap());}
            });
            if let Some(instance)=instance.as_mut(){ui.checkbox(&mut instance.playing,"Play animation");if ui.button("Restart").clicked(){instance.time=0.;instance.breast_motion.reset();}ui.label(format!("{} parts · {} triangles",instance.character.parts.len(),instance.character.triangle_count()));ui.label(format!("{:.2} m · {} bones",instance.character.height,instance.character.rig.as_ref().map(|r|r.bones.len()).unwrap_or(0)));}
            if assets.is_none(){ui.colored_label(egui::Color32::LIGHT_RED,"Native assets were not loaded by the host.");}
            ui.separator();ui.label("OUTPUT");ui.add(egui::TextEdit::singleline(&mut state.path).hint_text("exports/character.json"));
            egui::ComboBox::from_id_salt("native_format").selected_text(&recipe.export.format).show_ui(ui,|ui|{for f in ["glb","obj"]{ui.selectable_value(&mut recipe.export.format,f.into(),f);}});
            egui::ComboBox::from_id_salt("native_export_mode").selected_text(&recipe.export.mode).show_ui(ui,|ui|{for m in ["static","rigged","animated"]{ui.selectable_value(&mut recipe.export.mode,m.into(),m);}});
            ui.label("GLB keeps materials, textures, rig and selected animation clips. OBJ bakes the current shape.");
            if ui.button(format!("Export all {} clips", schema["clips"].as_array().unwrap().len())).clicked(){recipe.export.clips=schema["clips"].as_array().unwrap().iter().map(|c|c["id"].as_str().unwrap().to_owned()).collect();}
            if ui.button("Export preview clip only").clicked(){recipe.export.clips=if recipe.preview_clip.is_empty(){vec![]}else{vec![recipe.preview_clip.clone()]};}
            ui.label(format!("{} export clips",recipe.export.clips.len()));
        });
    });
    let viewport = viewport_ui.available_rect_before_wrap();
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(&mut viewport_ui, |ui| {
            ui.label(egui::RichText::new("LIVE NATIVE PREVIEW").weak());
            ui.label("Right-drag to orbit · Scroll to zoom");
        });
    view.pointer_captured = ctx.egui_is_using_pointer()
        || ctx
            .pointer_hover_pos()
            .is_none_or(|p| !viewport.contains(p));
    // History buttons are outside the recipe borrows.
    if history != 0 {
        if history < 0 && !state.history.is_empty() {
            let old = state.history.pop().unwrap();
            state.future.push(before.clone());
            current = old;
        } else if history > 0 {
            if let Some(next) = state.future.pop() {
                state.history.push(before.clone());
                current = next;
            }
        }
    }
    if load {
        let path = PathBuf::from(&state.path);
        match CharacterRecipe::load(&path, &library) {
            Ok(r) => {
                current = r;
                state.saved = Some(current.clone());
                state.status = "Preset loaded".into();
            }
            Err(e) => state.status = e.to_string(),
        }
    }
    if import_mhm {
        let path = PathBuf::from(&state.path).with_extension("mhm");
        let result = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| {
                crate::mpfb::import_mhm(&t, current.customization.as_ref().unwrap(), &schema)
            });
        match result {
            Ok(r) => current.customization = Some(r),
            Err(e) => state.status = e,
        }
    }
    if current != before && history == 0 {
        if state.gesture.is_none() {
            state.gesture = Some(before.clone());
            state.future.clear();
        }
    }
    if !ctx.input(|i| i.pointer.any_down()) {
        if let Some(start) = state.gesture.take() {
            if start != current {
                state.history.push(start);
                if state.history.len() > 100 {
                    state.history.remove(0);
                }
            }
        }
    }
    current.name = current
        .customization
        .as_ref()
        .and_then(|r| r.params["name"].as_str())
        .unwrap_or(&current.name)
        .to_owned();
    if current != before {
        shape.0 = current.clone();
    }
    if save || export_mhm {
        let path = PathBuf::from(&state.path);
        let result = (|| -> Result<(), String> {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            if export_mhm {
                crate::model::atomic_write(
                    &path.with_extension("mhm"),
                    crate::mpfb::export_mhm(current.customization.as_ref().unwrap()).as_bytes(),
                )
                .map_err(|e| e.to_string())?;
            } else {
                current.save(&path, &library).map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                state.saved = Some(current.clone());
                state.status = "Preset saved".into();
            }
            Err(e) => state.status = e,
        }
    }
    if export {
        if let (Some(instance), Some(assets)) = (instance.as_ref(), assets.as_ref()) {
            if current.customization.as_ref().is_some_and(|r| {
                r.params != instance.character.recipe.params
                    || r.custom_targets != instance.character.recipe.custom_targets
                    || r.custom_garments != instance.character.recipe.custom_garments
                    || r.breasts != instance.character.recipe.breasts
                    || r.cartoon != instance.character.recipe.cartoon
            }) {
                state.status = "Wait for the latest native build before exporting".into();
            } else {
                let character = instance.character.clone();
                let root = assets.0.root.clone();
                let recipe = current.customization.as_ref().unwrap().clone();
                let path = PathBuf::from(&state.path).with_extension(&recipe.export.format);
                let (sender, receiver) = std::sync::mpsc::channel();
                state.exporting = Some(std::sync::Mutex::new(receiver));
                state.status = "Exporting in native Rust…".into();
                std::thread::spawn(move || {
                    let result =
                        crate::native_export::export_character(&character, &recipe, &root, &path)
                            .map(|()| format!("Exported {}", path.display()));
                    let _ = sender.send(result);
                });
            }
        }
    }
    // The host's marked camera uses the existing native orbit system.

    Ok(())
}
fn draw_custom_garment_catalog(
    ui: &mut egui::Ui,
    recipe: &mut AraRecipe,
    catalog: &CustomGarmentCatalog,
    status: &mut String,
    slots: &[(GarmentSlot, &str)],
    heading: &str,
) {
    ui.separator();
    ui.heading(heading);
    ui.label("Definitions in native/custom_garments are validated when native assets load.");
    for (slot, label) in slots {
        let selected = if *slot == GarmentSlot::Accessory {
            let count = recipe
                .custom_garments
                .iter()
                .filter(|garment| garment.slot == *slot)
                .count();
            if count == 0 {
                "None".into()
            } else {
                format!("{count} selected")
            }
        } else {
            recipe
                .custom_garments
                .iter()
                .find(|garment| garment.slot == *slot)
                .map(|garment| garment.name.clone())
                .unwrap_or_else(|| "None".into())
        };
        egui::CollapsingHeader::new(format!("{label}: {selected}"))
            .id_salt(("custom-garment", label))
            .show(ui, |ui| {
                if ui
                    .button(if *slot == GarmentSlot::Accessory {
                        "Clear accessories"
                    } else {
                        "None"
                    })
                    .clicked()
                {
                    recipe.remove_custom_garment_slot(slot);
                }
                for entry in catalog
                    .entries
                    .iter()
                    .filter(|entry| entry.slot.as_ref() == Some(slot))
                {
                    if let Some(error) = &entry.error {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            format!("{} ({})", entry.path.display(), error),
                        );
                        continue;
                    }
                    let is_selected = recipe
                        .custom_garments
                        .iter()
                        .any(|garment| garment.id == entry.id);
                    let replace = ui.selectable_label(is_selected, &entry.name).clicked();
                    let layer = ui.small_button("Layer").clicked();
                    if replace {
                        if let Some(garment) = entry.garment() {
                            match recipe.select_custom_garment(garment) {
                                Ok(()) => {
                                    *status = format!("Selected custom garment {}", entry.name)
                                }
                                Err(error) => *status = error,
                            }
                        }
                    }
                    if layer {
                        if let Some(garment) = entry.garment() {
                            match recipe.toggle_layered_custom_garment(garment) {
                                Ok(()) => {
                                    *status = format!("Toggled layered garment {}", entry.name)
                                }
                                Err(error) => *status = error,
                            }
                        }
                    }
                }
            });
    }
    for entry in catalog.entries.iter().filter(|entry| entry.slot.is_none()) {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            format!(
                "{}: {}",
                entry.path.display(),
                entry.error.as_deref().unwrap_or("invalid garment")
            ),
        );
    }
}
fn draw_field(ui: &mut egui::Ui, params: &mut Value, field: &Value) {
    let id = field["id"].as_str().unwrap();
    let label = field["label"].as_str().unwrap_or(id);
    ui.push_id(id, |ui| match field["kind"].as_str().unwrap_or("") {
        "number" | "integer" => {
            let mut value = params[id].as_f64().unwrap_or(0.);
            if id == "age" {
                let mut years = if value < 0.5 {
                    11. + (value - 0.1875) / 0.3125 * 14.
                } else {
                    25. + (value - 0.5) * 130.
                };
                ui.add(
                    egui::Slider::new(&mut years, 12.0..=90.)
                        .text("Age (years)")
                        .integer(),
                );
                value = if years < 25. {
                    0.1875 + (years - 11.) / 14. * 0.3125
                } else {
                    0.5 + (years - 25.) / 130.
                };
            } else {
                let slider = egui::Slider::new(
                    &mut value,
                    field["min"].as_f64().unwrap_or(0.)..=field["max"].as_f64().unwrap_or(1.),
                )
                .text(label);
                ui.add(if field["kind"] == "integer" {
                    slider.integer()
                } else {
                    slider.fixed_decimals(2)
                });
            }
            params[id] = json!(value);
        }
        "boolean" => {
            let mut value = params[id].as_bool().unwrap_or(false);
            ui.checkbox(&mut value, label);
            params[id] = json!(value);
        }
        "text" => {
            ui.label(label);
            let mut value = params[id].as_str().unwrap_or("").to_owned();
            ui.add(egui::TextEdit::singleline(&mut value).char_limit(64));
            params[id] = json!(value);
        }
        "color" => {
            let mut color = crate::ara::rgb(params[id].as_str().unwrap_or("#ffffff"))
                .map(|v| (v * 255.).round() as u8);
            ui.horizontal(|ui| {
                ui.label(label);
                if ui.color_edit_button_srgb(&mut color).changed() {
                    params[id] =
                        json!(format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2]));
                }
            });
        }
        "choice" => {
            ui.label(label);
            let mut value = params[id].as_str().unwrap_or("none").to_owned();
            let choices = field["choices"].as_array().unwrap();
            let name = choices
                .iter()
                .find(|c| c["id"] == value)
                .and_then(|c| c["name"].as_str())
                .unwrap_or(&value);
            egui::ComboBox::from_id_salt(id)
                .selected_text(name)
                .width(290.)
                .show_ui(ui, |ui| {
                    for choice in choices {
                        ui.selectable_value(
                            &mut value,
                            choice["id"].as_str().unwrap().into(),
                            choice["name"].as_str().unwrap(),
                        );
                    }
                });
            params[id] = json!(value);
        }
        "multi" => {
            egui::CollapsingHeader::new(label).show(ui, |ui| {
                let mut selected = params[id].as_array().cloned().unwrap_or_default();
                for choice in field["choices"].as_array().unwrap() {
                    let mut checked = selected.contains(&choice["id"]);
                    if ui
                        .checkbox(&mut checked, choice["name"].as_str().unwrap())
                        .changed()
                    {
                        if checked {
                            selected.push(choice["id"].clone());
                        } else {
                            selected.retain(|s| *s != choice["id"]);
                        }
                    }
                }
                params[id] = json!(selected);
            });
        }
        _ => {}
    });
}

fn unique_target_name(recipe: &AraRecipe, proposed: &str) -> String {
    let mut stem = proposed.trim().to_owned();
    while stem.len() > 240 {
        stem.pop();
    }
    if stem.is_empty() {
        stem = "Custom target".into();
    }
    let mut name = stem.clone();
    let mut index = 2;
    while recipe.custom_targets.iter().any(|t| t.name == name) {
        name = format!("{stem} ({index})");
        index += 1;
    }
    name
}
