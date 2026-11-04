//! Portable, dependency-free character export (GLB 2.0 and OBJ).
use crate::{
    ara::AraRecipe,
    model::atomic_write,
    native_character::{NativeCharacter, NativePart},
    native_texture::fabric_image,
    wardrobe_assets::native_texture_path,
};
use bevy::prelude::{Quat, Vec3};
use image::{ImageBuffer, ImageEncoder, Rgba};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub fn export_character(
    character: &NativeCharacter,
    recipe: &AraRecipe,
    assets_root: &Path,
    path: &Path,
) -> Result<(), String> {
    recipe.validate()?;
    if character.parts.is_empty() {
        return Err("character has no mesh parts".into());
    }
    if path.file_name().is_none() {
        return Err("export path has no filename".into());
    }
    match recipe.export.format.as_str() {
        "glb" => {
            atomic_write(path, &glb(character, recipe, assets_root)?).map_err(|e| e.to_string())
        }
        "obj" => obj(character, recipe, assets_root, path),
        _ => Err("unsupported export format".into()),
    }
}

fn valid_part(p: &NativePart) -> Result<(), String> {
    if p.positions.len() != p.uvs.len() || p.positions.is_empty() || p.indices.len() % 3 != 0 {
        return Err(format!("malformed mesh {}", p.name));
    }
    if p.positions
        .iter()
        .flatten()
        .chain(p.uvs.iter().flatten())
        .any(|v| !v.is_finite())
        || p.indices.iter().any(|&i| i as usize >= p.positions.len())
    {
        return Err(format!("invalid mesh data {}", p.name));
    }
    Ok(())
}
fn normals(p: &NativePart) -> Vec<[f32; 3]> {
    crate::native_normals::surface_normals(p, &p.positions)
}

fn color(m: &Value) -> [f32; 4] {
    let s = m["color"]
        .as_str()
        .unwrap_or("#ffffff")
        .trim_start_matches('#');
    let x = |i| {
        let value =
            u8::from_str_radix(s.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255) as f32 / 255.;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    [
        x(0),
        x(2),
        x(4),
        m["alpha"]
            .as_str()
            .map(|a| {
                if a == "opaque" {
                    1.
                } else {
                    m["opacity"].as_f64().unwrap_or(1.) as f32
                }
            })
            .unwrap_or(1.),
    ]
}
fn texture(m: &Value, root: &Path) -> Result<Vec<u8>, String> {
    if m["pattern"].is_null() {
        if let Some(s) = m["diffuse"].as_str().or_else(|| m["map"].as_str()) {
            if let Some(p) = native_texture_path(root, s).filter(|path| path.is_file()) {
                let mut image = image::load_from_memory(&fs::read(p).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("invalid texture {s}: {e}"))?
                    .to_rgba8();
                crate::native_texture::cartoon_texture(&mut image, m);
                let mut out = Vec::new();
                image::codecs::png::PngEncoder::new(&mut out)
                    .write_image(
                        image.as_raw(),
                        image.width(),
                        image.height(),
                        image::ExtendedColorType::Rgba8,
                    )
                    .map_err(|e| e.to_string())?;
                return Ok(out);
            }
        }
    }
    let i = if m["pattern"].is_string() {
        crate::native_texture::recolor_pattern(root, m).unwrap_or_else(|| fabric_image(m))
    } else {
        fabric_image(&json!({"pattern":"solid","color":"#ffffff","color2":"#ffffff"}))
    };
    let data = i.data.as_ref().ok_or("fabric image has no pixels")?;
    let w = i.texture_descriptor.size.width;
    let h = i.texture_descriptor.size.height;
    let img: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_raw(w, h, data.clone()).ok_or("invalid fabric pixels")?;
    let mut b = Vec::new();
    image::codecs::png::PngEncoder::new(&mut b)
        .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(b)
}
fn align(b: &mut Vec<u8>) {
    while b.len() % 4 != 0 {
        b.push(0)
    }
}
fn accessor(
    a: &mut Vec<Value>,
    views: &mut Vec<Value>,
    bin: &mut Vec<u8>,
    data: &[u8],
    component: u32,
    typ: &str,
    count: usize,
    minmax: Option<([f32; 3], [f32; 3])>,
) -> usize {
    align(bin);
    let off = bin.len();
    bin.extend_from_slice(data);
    let vi = views.len();
    views.push(json!({"buffer":0,"byteOffset":off,"byteLength":data.len()}));
    let mut v = json!({"bufferView":vi,"componentType":component,"count":count,"type":typ});
    if let Some((min, max)) = minmax {
        v["min"] = json!(min);
        v["max"] = json!(max);
    }
    a.push(v);
    a.len() - 1
}
fn bytes<T: Copy>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn glb(c: &NativeCharacter, r: &AraRecipe, root: &Path) -> Result<Vec<u8>, String> {
    let rig = c.rig.as_ref().filter(|_| r.export.mode != "static");
    let mut bin = Vec::new();
    let mut views = vec![];
    let mut acc = vec![];
    let mut images = vec![];
    let mut textures = vec![];
    let mut mats = vec![];
    let mut meshes = vec![];
    let mut skinned_meshes = vec![];
    for p in &c.parts {
        valid_part(p)?;
        let n = normals(p);
        let (min, max) = p.positions.iter().fold(
            ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]),
            |(mut lo, mut hi), q| {
                for k in 0..3 {
                    lo[k] = lo[k].min(q[k]);
                    hi[k] = hi[k].max(q[k]);
                }
                (lo, hi)
            },
        );
        let pos = accessor(
            &mut acc,
            &mut views,
            &mut bin,
            bytes(&p.positions),
            5126,
            "VEC3",
            p.positions.len(),
            Some((min, max)),
        );
        let nor = accessor(
            &mut acc,
            &mut views,
            &mut bin,
            bytes(&n),
            5126,
            "VEC3",
            n.len(),
            None,
        );
        let uv = accessor(
            &mut acc,
            &mut views,
            &mut bin,
            bytes(&p.uvs),
            5126,
            "VEC2",
            p.uvs.len(),
            None,
        );
        let ind = accessor(
            &mut acc,
            &mut views,
            &mut bin,
            bytes(&p.indices),
            5125,
            "SCALAR",
            p.indices.len(),
            None,
        );
        let tex = texture(&p.material, root)?;
        align(&mut bin);
        let o = bin.len();
        bin.extend(&tex);
        let vi = views.len();
        views.push(json!({"buffer":0,"byteOffset":o,"byteLength":tex.len()}));
        let im = images.len();
        images.push(json!({"bufferView":vi,"mimeType":"image/png"}));
        textures.push(json!({"source":im}));
        let mi = mats.len();
        let mut col = color(&p.material);
        if p.material["pattern"].is_string() {
            col[0] = 1.;
            col[1] = 1.;
            col[2] = 1.;
        }
        let metal = p.material["metallic"]
            .as_f64()
            .or_else(|| p.material["metalness"].as_f64())
            .unwrap_or(0.) as f32;
        mats.push(json!({"pbrMetallicRoughness":{"baseColorFactor":col,"metallicFactor":metal,"roughnessFactor":p.material["roughness"].as_f64().unwrap_or(0.8)as f32,"baseColorTexture":{"index":im}},"alphaMode":if p.material["alpha"].as_str()==Some("mask"){"MASK"}else if p.material["alpha"]=="blend"||col[3]<1.{"BLEND"}else{"OPAQUE"},"doubleSided":p.material["doubleSided"].as_bool().unwrap_or(false)}));
        if let Some(normal) = p.material["normal"]
            .as_str()
            .or_else(|| p.material["normalMap"].as_str())
        {
            if native_texture_path(root, normal).is_some_and(|path| path.is_file()) {
                let data = texture(&json!({"map":normal}), root)?;
                align(&mut bin);
                let offset = bin.len();
                bin.extend(&data);
                let view = views.len();
                views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":data.len()}));
                let image = images.len();
                images.push(json!({"bufferView":view,"mimeType":"image/png"}));
                let tex = textures.len();
                textures.push(json!({"source":image}));
                mats[mi]["normalTexture"] = json!({"index":tex});
            }
        }
        if let Some(glow) = p.material["emissive"].as_str() {
            let converted = color(&json!({"color":glow}));
            mats[mi]["emissiveFactor"] = json!([converted[0], converted[1], converted[2]]);
        }
        let mut attrs = json!({"POSITION":pos,"NORMAL":nor,"TEXCOORD_0":uv});
        let skinned = rig.is_some() && p.name != "Bicycle";
        if let Some(rig) = rig.filter(|_| skinned) {
            let (j, w) = rig.skin_part(p);
            if j.len() != p.positions.len() {
                return Err("invalid skin weights".into());
            }
            let ja = accessor(
                &mut acc,
                &mut views,
                &mut bin,
                bytes(&j),
                5123,
                "VEC4",
                j.len(),
                None,
            );
            let wa = accessor(
                &mut acc,
                &mut views,
                &mut bin,
                bytes(&w),
                5126,
                "VEC4",
                w.len(),
                None,
            );
            attrs["JOINTS_0"] = json!(ja);
            attrs["WEIGHTS_0"] = json!(wa);
        }
        let mut prim = json!({"attributes":attrs,"indices":ind,"material":mi});
        if skinned
            && r.params["faceShapes"] == true
            && !c.face_deltas.is_empty()
            && c.face_deltas.len() == c.face_names.len()
        {
            let mut targets = vec![];
            for d in &c.face_deltas {
                let mut x = vec![[0.; 3]; p.positions.len()];
                for (i, rr) in p.refs.iter().enumerate() {
                    let ww = p.ref_weights.get(i).copied().unwrap_or([1., 0., 0.]);
                    for k in 0..3 {
                        if let Some(q) = d.get(rr[k] as usize) {
                            for z in 0..3 {
                                x[i][z] += q[z] * ww[k]
                            }
                        }
                    }
                }
                let a = accessor(
                    &mut acc,
                    &mut views,
                    &mut bin,
                    bytes(&x),
                    5126,
                    "VEC3",
                    x.len(),
                    None,
                );
                targets.push(json!({"POSITION":a}));
            }
            prim["targets"] = json!(targets);
        }
        meshes.push(json!({"name":p.name,"primitives":[prim],"weights":vec![0.;c.face_names.len()],"extras":{"targetNames":c.face_names}}));
        skinned_meshes.push(skinned);
    }
    let mut nodes: Vec<Value> = meshes
        .iter()
        .enumerate()
        .map(|(i, m)| json!({"name":m["name"],"mesh":i}))
        .collect();
    let mut skins = vec![];
    let mut animations = vec![];
    if let Some(rig) = rig {
        let base = nodes.len();
        for (i, b) in rig.bones.iter().enumerate() {
            let world = rig.bind_world_matrices()[i];
            let local = b
                .parent
                .map(|p| rig.bind_world_matrices()[p].inverse() * world)
                .unwrap_or(world);
            let (_, rotation, translation) = local.to_scale_rotation_translation();
            nodes.push(json!({"name":b.name,"translation":translation.to_array(),"rotation":rotation.to_array()}));
        }
        for (i, b) in rig.bones.iter().enumerate() {
            if let Some(parent) = b.parent {
                if !nodes[parent + base]["children"].is_array() {
                    nodes[parent + base]["children"] = json!([])
                }
                nodes[parent + base]["children"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(base + i));
            }
        }
        let ib: Vec<[f32; 16]> = rig
            .inverse_bind_matrices()
            .iter()
            .map(|m| m.to_cols_array())
            .collect();
        let iba = accessor(
            &mut acc,
            &mut views,
            &mut bin,
            bytes(&ib),
            5126,
            "MAT4",
            ib.len(),
            None,
        );
        skins.push(json!({"joints":(0..rig.bones.len()).map(|i|base+i).collect::<Vec<_>>(),"inverseBindMatrices":iba,"skeleton":base}));
        for (i, skinned) in skinned_meshes.iter().enumerate() {
            if *skinned {
                nodes[i]["skin"] = json!(0)
            }
        }
        if r.export.mode == "animated" && r.params["animations"] != false {
            for id in &r.export.clips {
                let dur = rig
                    .clip_duration(id)
                    .ok_or_else(|| format!("unknown clip {id}"))?;
                let steps = (dur * 30.).ceil().max(1.) as usize + 1;
                let times: Vec<f32> = (0..steps)
                    .map(|i| dur * i as f32 / (steps - 1) as f32)
                    .collect();
                let ta = accessor(
                    &mut acc,
                    &mut views,
                    &mut bin,
                    bytes(&times),
                    5126,
                    "SCALAR",
                    times.len(),
                    None,
                );
                let mut secondary = crate::native_breast::BreastMotion::default();
                let frames: Vec<_> = times
                    .iter()
                    .enumerate()
                    .map(|(i, &t)| {
                        let mut skin = rig.pose_clamped(id, t);
                        let dt = if i == 0 {
                            1.0 / 120.0
                        } else {
                            t - times[i - 1]
                        };
                        secondary.apply(
                            rig,
                            &mut skin,
                            Vec3::ZERO,
                            dt,
                            &r.breasts,
                            r.params["gender"].as_f64().unwrap_or(1.) as f32,
                        );
                        skin
                    })
                    .collect();
                let mut samplers = vec![];
                let mut channels = vec![];
                for bi in 0..rig.bones.len() {
                    let mut rots = Vec::new();
                    let mut translations = Vec::new();
                    for skin in &frames {
                        let world = skin[bi] * rig.bind_world_matrices()[bi];
                        let local = rig.bones[bi]
                            .parent
                            .map(|p| (skin[p] * rig.bind_world_matrices()[p]).inverse() * world)
                            .unwrap_or(world);
                        rots.push(Quat::from_mat4(&local).normalize().to_array());
                        let mut translation = local.to_scale_rotation_translation().2;
                        if !r.export.root_motion && rig.bones[bi].parent.is_none() {
                            // In-place clips still need pelvis bob and jumps.
                            // Only horizontal controller travel is removed.
                            translation.x = rig.bones[bi].head.x;
                            translation.z = rig.bones[bi].head.z;
                        }
                        translations.push(translation.to_array());
                    }
                    let bind_local = rig.bones[bi]
                        .parent
                        .map(|p| {
                            rig.bind_world_matrices()[p].inverse() * rig.bind_world_matrices()[bi]
                        })
                        .unwrap_or(rig.bind_world_matrices()[bi]);
                    let bind_translation = bind_local.to_scale_rotation_translation().2;
                    if translations
                        .iter()
                        .all(|t| (Vec3::from(*t) - bind_translation).length_squared() < 1e-10)
                    {
                        translations.clear();
                    }
                    let oa = accessor(
                        &mut acc,
                        &mut views,
                        &mut bin,
                        bytes(&rots),
                        5126,
                        "VEC4",
                        rots.len(),
                        None,
                    );
                    let si = samplers.len();
                    samplers.push(json!({"input":ta,"output":oa,"interpolation":"LINEAR"}));
                    channels
                        .push(json!({"sampler":si,"target":{"node":base+bi,"path":"rotation"}}));
                    if !translations.is_empty() {
                        let oa = accessor(
                            &mut acc,
                            &mut views,
                            &mut bin,
                            bytes(&translations),
                            5126,
                            "VEC3",
                            translations.len(),
                            None,
                        );
                        let si = samplers.len();
                        samplers.push(json!({"input":ta,"output":oa,"interpolation":"LINEAR"}));
                        channels.push(
                            json!({"sampler":si,"target":{"node":base+bi,"path":"translation"}}),
                        );
                    }
                }
                animations.push(json!({"name":id,"samplers":samplers,"channels":channels,"extras":{"looped":rig.clip_looped(id).unwrap_or(false)}}));
            }
        }
    }
    let roots: Vec<usize> = if rig.is_some() {
        (0..meshes.len())
            .chain(std::iter::once(meshes.len()))
            .collect()
    } else {
        (0..meshes.len()).collect()
    };
    let mut doc = json!({"asset":{"version":"2.0","generator":"human_editor native exporter"},"scene":0,"scenes":[{"nodes":roots}],"nodes":nodes,"meshes":meshes,"materials":mats,"textures":textures,"images":images,"buffers":[{"byteLength":bin.len()}],"bufferViews":views,"accessors":acc});
    if !skins.is_empty() {
        doc["skins"] = json!(skins)
    }
    if !animations.is_empty() {
        doc["animations"] = json!(animations)
    }
    let mut j = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    while j.len() % 4 != 0 {
        // GLB permits whitespace padding in JSON; zero bytes are not JSON whitespace.
        j.push(b' ');
    }
    align(&mut bin);
    let total = 12 + 8 + j.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x46546c67u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4e4f534au32.to_le_bytes());
    out.extend(j);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004e4942u32.to_le_bytes());
    out.extend(bin);
    Ok(out)
}
fn obj(c: &NativeCharacter, _r: &AraRecipe, root: &Path, path: &Path) -> Result<(), String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .ok_or("invalid output filename")?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mtl = dir.join(format!("{stem}.mtl"));
    let mut o = format!("mtllib {stem}.mtl\n");
    let mut m = String::new();
    let mut vo = 1usize;
    for (i, p) in c.parts.iter().enumerate() {
        valid_part(p)?;
        let name = format!("material_{i}");
        o += &format!("o {}\nusemtl {name}\n", p.name);
        for v in &p.positions {
            o += &format!("v {} {} {}\n", v[0], v[1], v[2])
        }
        for u in &p.uvs {
            o += &format!("vt {} {}\n", u[0], u[1])
        }
        for t in p.indices.chunks_exact(3) {
            o += &format!(
                "f {0}/{0} {1}/{1} {2}/{2}\n",
                t[0] as usize + vo,
                t[1] as usize + vo,
                t[2] as usize + vo
            )
        }
        let mut col = color(&p.material);
        if p.material["pattern"].is_string() {
            col[0] = 1.;
            col[1] = 1.;
            col[2] = 1.;
        }
        let tex = format!("{stem}_{i}.png");
        m += &format!(
            "newmtl {name}\nKd {} {} {}\nd {}\nmap_Kd {tex}\n\n",
            col[0], col[1], col[2], col[3]
        );
        atomic_write(&dir.join(&tex), &texture(&p.material, root)?).map_err(|e| e.to_string())?;
        vo += p.positions.len();
    }
    atomic_write(&mtl, m.as_bytes()).map_err(|e| e.to_string())?;
    atomic_write(path, o.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn static_glb_has_valid_header_and_mesh_attributes() {
        let mut recipe = AraRecipe::default();
        recipe.export.mode = "static".into();
        recipe.export.format = "glb".into();
        let character = NativeCharacter {
            parts: vec![NativePart {
                name: "triangle".into(),
                positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                indices: vec![0, 1, 2],
                uvs: vec![[0., 0.], [1., 0.], [0., 1.]],
                refs: vec![],
                ref_weights: vec![],
                rigid_joint: None,
                material: json!({"color":"#ffffff"}),
            }],
            height: 1.,
            hair_fit: Default::default(),
            rig: None,
            face_names: vec![],
            face_deltas: vec![],
            recipe: recipe.clone(),
        };
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("native-export-{nonce}.glb"));
        export_character(&character, &recipe, Path::new("."), &path).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"glTF");
        let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let doc: Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
        let attrs = &doc["meshes"][0]["primitives"][0]["attributes"];
        assert!(
            attrs["POSITION"].is_number()
                && attrs["NORMAL"].is_number()
                && attrs["TEXCOORD_0"].is_number()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn static_glb_retains_imported_hair_as_a_mesh() {
        let mut recipe = AraRecipe::default();
        recipe.export.mode = "static".into();
        recipe.export.format = "glb".into();
        let character = NativeCharacter {
            parts: vec![NativePart {
                name: "custom:imported-hair".into(),
                positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                indices: vec![0, 1, 2],
                uvs: vec![[0., 0.], [1., 0.], [0., 1.]],
                refs: vec![],
                ref_weights: vec![],
                rigid_joint: None,
                material: json!({"color":"#ffffff"}),
            }],
            height: 1.,
            hair_fit: Default::default(),
            rig: None,
            face_names: vec![],
            face_deltas: vec![],
            recipe: recipe.clone(),
        };
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("native-hair-export-{nonce}.glb"));
        export_character(&character, &recipe, Path::new("."), &path).unwrap();
        let bytes = fs::read(&path).unwrap();
        let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let doc: Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
        assert!(
            doc["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["name"] == "custom:imported-hair")
        );
        fs::remove_file(path).unwrap();
    }
}
