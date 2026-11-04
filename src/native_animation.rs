//! Importer for external animation-library GLBs.
//!
//! This deliberately stores *local bind-relative* rotations.  A UAL mannequin
//! and an Ara character have different bone axes, so copying the local GLB
//! quaternions would twist limbs. The source hierarchy and reference pose are
//! retained so `NativeRig` can transfer world-space motion into its own frames.
use bevy::prelude::Quat;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use gltf::animation::util::ReadOutputs;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const CATALOG: &str = "animation_libraries.json.gz";
const MOTIONS: &str = "animation_libraries.motion.json.gz";
const MESHY_JOINTS: [&str; 24] = [
    "Hips",
    "Spine02",
    "Spine01",
    "Spine",
    "neck",
    "Head",
    "head_end",
    "headfront",
    "LeftShoulder",
    "LeftArm",
    "LeftForeArm",
    "LeftHand",
    "RightShoulder",
    "RightArm",
    "RightForeArm",
    "RightHand",
    "LeftUpLeg",
    "LeftLeg",
    "LeftFoot",
    "LeftToeBase",
    "RightUpLeg",
    "RightLeg",
    "RightFoot",
    "RightToeBase",
];

#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub id: String,
    pub source: PathBuf,
    pub output_root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AnimationClipMetadata {
    pub id: String,
    pub name: String,
    pub library: String,
    pub duration: f32,
    pub looped: bool,
    pub frames: usize,
    pub root_motion: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnimationLibraryCatalog {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default)]
    pub clips: Vec<AnimationClipMetadata>,
}
fn version() -> u32 {
    1
}
impl AnimationLibraryCatalog {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, String> {
        let p = root.as_ref().join(CATALOG);
        let f = fs::File::open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut bytes = Vec::new();
        GzDecoder::new(f)
            .take(16 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", p.display()))
    }
}

#[derive(Serialize, Deserialize)]
struct MotionFile {
    #[serde(default)]
    clips: BTreeMap<String, Motion>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Motion {
    looped: bool,
    duration: f32,
    bones: Vec<String>,
    times: Vec<f32>,
    rotations: Vec<Vec<[f32; 4]>>,
    #[serde(default)]
    roots: Vec<[f32; 3]>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    parents: Vec<Option<usize>>,
    #[serde(default)]
    bind_rotations: Vec<[f32; 4]>,
    #[serde(default)]
    translations: Vec<Vec<[f32; 3]>>,
    #[serde(default)]
    bind_translations: Vec<[f32; 3]>,
}

/// Imports a supported external animation library. UAL sources require both
/// the regular and root-motion GLBs. Meshy sources are the downloaded
/// `*-idle-casual-walk-run-02.glb` service assets.
pub fn import_library(options: &ImportOptions) -> Result<AnimationLibraryCatalog, String> {
    if matches!(options.id.as_str(), "meshy-female" | "meshy-male") {
        return import_meshy_service(options);
    }
    if !matches!(options.id.as_str(), "ual1" | "ual2") {
        return Err("animation library id must be ual1, ual2, meshy-female, or meshy-male".into());
    }
    let (normal, rooted) = if options.source.is_file() {
        let stem = options
            .source
            .file_stem()
            .and_then(|x| x.to_str())
            .ok_or("animation source needs a UTF-8 file name")?;
        let base = stem.strip_suffix("_RM").unwrap_or(stem);
        let dir = options
            .source
            .parent()
            .ok_or("animation source has no parent directory")?;
        (
            dir.join(format!("{base}.glb")),
            dir.join(format!("{base}_RM.glb")),
        )
    } else {
        let dir = if options.source.join("Unreal-Godot").is_dir() {
            options.source.join("Unreal-Godot")
        } else {
            options.source.clone()
        };
        let stem = options.id.to_ascii_uppercase();
        (
            dir.join(format!("{stem}.glb")),
            dir.join(format!("{stem}_RM.glb")),
        )
    };
    if !normal.is_file() || !rooted.is_file() {
        return Err(format!(
            "{} needs {} and {}",
            options.id,
            normal.display(),
            rooted.display()
        ));
    }
    let standard = read_glb(&normal, false)?;
    let rm = read_glb(&rooted, true)?;
    let mut prior = load_motions(&options.output_root)?;
    let mut catalog = AnimationLibraryCatalog::load(&options.output_root).unwrap_or_default();
    prior
        .clips
        .retain(|key, _| !key.starts_with(&(options.id.clone() + ":")));
    catalog.clips.retain(|x| x.library != options.id);
    for (name, mut clip) in standard {
        if let Some(root) = rm.get(&name) {
            clip.roots = root.roots.clone();
        }
        let id = format!("{}:{name}", options.id);
        let duration = clip.duration;
        let frames = clip.times.len();
        let looped = clip.looped;
        prior.clips.insert(id.clone(), clip);
        catalog.clips.push(AnimationClipMetadata {
            id,
            name,
            library: options.id.clone(),
            duration,
            looped,
            frames,
            root_motion: true,
        });
    }
    catalog.clips.sort_by(|a, b| a.id.cmp(&b.id));
    fs::create_dir_all(&options.output_root).map_err(|e| e.to_string())?;
    write_gzip(options.output_root.join(MOTIONS), &prior)?;
    write_gzip(options.output_root.join(CATALOG), &catalog)?;
    crate::native_rig::invalidate_motion_cache(&options.output_root);
    Ok(catalog)
}

fn import_meshy_service(options: &ImportOptions) -> Result<AnimationLibraryCatalog, String> {
    let sex = options
        .id
        .strip_prefix("meshy-")
        .ok_or("Meshy animation library id must start with meshy-")?;
    let file = format!("{sex}-idle-casual-walk-run-02.glb");
    let source = if options.source.is_file() {
        options.source.clone()
    } else {
        options.source.join(file)
    };
    let clips = read_meshy_glb(&source)?;
    let mut prior = load_motions(&options.output_root)?;
    let mut catalog = AnimationLibraryCatalog::load(&options.output_root).unwrap_or_default();
    prior
        .clips
        .retain(|key, _| !key.starts_with(&(options.id.clone() + ":")));
    catalog.clips.retain(|x| x.library != options.id);
    for (name, clip) in clips {
        let id = format!("{}:{name}", options.id);
        catalog.clips.push(AnimationClipMetadata {
            id: id.clone(),
            name,
            library: options.id.clone(),
            duration: clip.duration,
            looped: clip.looped,
            frames: clip.times.len(),
            root_motion: false,
        });
        prior.clips.insert(id, clip);
    }
    catalog.clips.sort_by(|a, b| a.id.cmp(&b.id));
    fs::create_dir_all(&options.output_root).map_err(|e| e.to_string())?;
    write_gzip(options.output_root.join(MOTIONS), &prior)?;
    write_gzip(options.output_root.join(CATALOG), &catalog)?;
    crate::native_rig::invalidate_motion_cache(&options.output_root);
    Ok(catalog)
}
fn load_motions(root: &Path) -> Result<MotionFile, String> {
    let p = root.join(MOTIONS);
    if !p.exists() {
        return Ok(MotionFile {
            clips: BTreeMap::new(),
        });
    }
    let f = fs::File::open(&p).map_err(|e| e.to_string())?;
    let mut b = Vec::new();
    GzDecoder::new(f)
        .take(512 * 1024 * 1024)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}
fn write_gzip<T: Serialize>(path: PathBuf, value: &T) -> Result<(), String> {
    let data = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let mut z = GzEncoder::new(Vec::new(), Compression::best());
    z.write_all(&data).map_err(|e| e.to_string())?;
    crate::model::atomic_write(&path, &z.finish().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
fn read_glb(path: &Path, roots: bool) -> Result<BTreeMap<String, Motion>, String> {
    let (doc, buffers, _) = gltf::import(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if doc.nodes().count() != 67 {
        return Err(format!(
            "{}: expected the UAL 67-node humanoid",
            path.display()
        ));
    }
    let nodes: Vec<_> = doc.nodes().collect();
    let names: Vec<String> = nodes
        .iter()
        .map(|n| n.name().unwrap_or("").to_owned())
        .collect();
    if !names.iter().any(|n| n == "pelvis") || !names.iter().any(|n| n == "spine_03") {
        return Err(format!("{}: unsupported skeleton", path.display()));
    }
    let mut bind: Vec<Quat> = nodes
        .iter()
        .map(|n| {
            let (_, r, _) = n.transform().decomposed();
            Quat::from_array(r).normalize()
        })
        .collect();
    let mut parents = vec![None; nodes.len()];
    for p in &nodes {
        for c in p.children() {
            parents[c.index()] = Some(p.index());
        }
    }
    let root_index = names
        .iter()
        .position(|n| n == "root")
        .ok_or("UAL root missing")?;
    let mut bind_translations: Vec<_> =
        nodes.iter().map(|n| n.transform().decomposed().0).collect();
    // UAL stores an explicit T-pose animation. It is the authored reference
    // for every clip; node transforms alone are not that pose on all exports.
    if let Some(reference) = doc.animations().find(|a| a.name() == Some("A_TPose")) {
        for channel in reference.channels() {
            let index = channel.target().node().index();
            let reader = channel.reader(|b| Some(&buffers[b.index()]));
            match reader.read_outputs() {
                Some(ReadOutputs::Rotations(values)) => {
                    if let Some(value) = values.into_f32().next() {
                        bind[index] = Quat::from_array(value).normalize();
                    }
                }
                Some(ReadOutputs::Translations(mut values)) => {
                    if let Some(value) = values.next() {
                        bind_translations[index] = value;
                    }
                }
                _ => {}
            }
        }
    }
    let root_bind_translation = bind_translations[root_index];
    let mut out = BTreeMap::new();
    for a in doc.animations() {
        let name = a.name().ok_or("UAL animation without name")?.to_owned();
        let mut tracks: BTreeMap<usize, (Vec<f32>, Vec<[f32; 4]>)> = BTreeMap::new();
        let mut translations: BTreeMap<usize, (Vec<f32>, Vec<[f32; 3]>)> = BTreeMap::new();
        for c in a.channels() {
            let node = c.target().node().index();
            let reader = c.reader(|b| Some(&buffers[b.index()]));
            let input = reader
                .read_inputs()
                .ok_or("animation missing times")?
                .collect::<Vec<_>>();
            match reader.read_outputs().ok_or("animation missing output")? {
                ReadOutputs::Rotations(v) => {
                    tracks.insert(node, (input, v.into_f32().collect()));
                }
                ReadOutputs::Translations(v) => {
                    translations.insert(node, (input, v.collect()));
                }
                _ => {}
            }
        }
        let duration = tracks
            .values()
            .filter_map(|x| x.0.last().copied())
            .chain(translations.values().filter_map(|x| x.0.last().copied()))
            .fold(0f32, f32::max);
        let frames = ((duration * 30.).ceil() as usize + 1).max(2);
        let times = (0..frames)
            .map(|i| duration * i as f32 / (frames - 1) as f32)
            .collect::<Vec<_>>();
        let rotations = times
            .iter()
            .map(|&t| {
                (0..nodes.len())
                    .map(|i| {
                        tracks
                            .get(&i)
                            .map(|x| relative(bind[i], sample_quat(&x.0, &x.1, t)))
                            .unwrap_or([0., 0., 0., 1.])
                    })
                    .collect()
            })
            .collect();
        let sampled_translations: Vec<Vec<[f32; 3]>> = times
            .iter()
            .map(|&t| {
                (0..nodes.len())
                    .map(|i| {
                        translations
                            .get(&i)
                            .map(|x| sample_vec3(&x.0, &x.1, t))
                            .unwrap_or(bind_translations[i])
                    })
                    .collect()
            })
            .collect();
        // Some UAL clips carry the armature's placement in every root key
        // (not just a displacement). Root motion starts at zero for each
        // clip; the pelvis track below retains the authored vertical bob.
        let root_reference = sampled_translations
            .first()
            .map(|x| x[root_index])
            .unwrap_or(root_bind_translation);
        let root_values = sampled_translations
            .iter()
            .map(|x| {
                (bevy::prelude::Vec3::from(x[root_index])
                    - bevy::prelude::Vec3::from(root_reference))
                .to_array()
            })
            .collect();
        let looped = name.to_ascii_lowercase().contains("loop")
            || name.to_ascii_lowercase().contains("idle");
        out.insert(
            name,
            Motion {
                looped,
                duration,
                bones: names.clone(),
                times,
                rotations,
                roots: if roots { root_values } else { vec![] },
                source: Some("ual".into()),
                parents: parents.clone(),
                bind_rotations: bind.iter().map(|q| q.to_array()).collect(),
                translations: sampled_translations,
                bind_translations: bind_translations.clone(),
            },
        );
    }
    Ok(out)
}

/// Reads the exact 24-joint skeleton emitted by Meshy's rigging service. The
/// armature applies a centimetre-to-metre scale at its root; bake that scale
/// into translations because retargeting stores only joint-local tracks.
fn read_meshy_glb(path: &Path) -> Result<BTreeMap<String, Motion>, String> {
    let (doc, buffers, _) = gltf::import(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let nodes: Vec<_> = doc.nodes().collect();
    if nodes.len() != 26 || doc.skins().count() != 1 || doc.meshes().count() != 1 {
        return Err(format!(
            "{}: expected Meshy's 26-node, one-skin service rig",
            path.display()
        ));
    }
    let names: Vec<String> = nodes
        .iter()
        .map(|n| n.name().unwrap_or("").to_owned())
        .collect();
    let skin = doc.skins().next().expect("checked skin count");
    let joints: Vec<_> = skin
        .joints()
        .map(|node| node.name().unwrap_or(""))
        .collect();
    if joints != MESHY_JOINTS {
        return Err(format!("{}: unsupported Meshy joint order", path.display()));
    }
    let armature = nodes
        .iter()
        .find(|node| node.name() == Some("Armature"))
        .ok_or_else(|| format!("{}: Meshy Armature node missing", path.display()))?;
    let (_, _, scale) = armature.transform().decomposed();
    if !scale.iter().all(|v| v.is_finite() && *v > 0.)
        || (scale[0] - scale[1]).abs() > 1e-5
        || (scale[0] - scale[2]).abs() > 1e-5
    {
        return Err(format!(
            "{}: Meshy Armature must have uniform positive scale",
            path.display()
        ));
    }
    let unit = scale[0];
    let bind: Vec<Quat> = nodes
        .iter()
        .map(|node| {
            let (_, rotation, _) = node.transform().decomposed();
            Quat::from_array(rotation).normalize()
        })
        .collect();
    let mut parents = vec![None; nodes.len()];
    for parent in &nodes {
        for child in parent.children() {
            parents[child.index()] = Some(parent.index());
        }
    }
    let bind_translations: Vec<[f32; 3]> = nodes
        .iter()
        .map(|node| {
            let (translation, _, _) = node.transform().decomposed();
            (bevy::prelude::Vec3::from(translation) * unit).to_array()
        })
        .collect();
    let mut out = BTreeMap::new();
    for animation in doc.animations() {
        let name = animation
            .name()
            .ok_or_else(|| format!("{}: Meshy animation without name", path.display()))?
            .to_owned();
        if !matches!(name.as_str(), "Idle" | "Casual_Walk" | "Run_02") {
            return Err(format!(
                "{}: unsupported Meshy animation {name:?}",
                path.display()
            ));
        }
        let mut tracks: BTreeMap<usize, (Vec<f32>, Vec<[f32; 4]>)> = BTreeMap::new();
        let mut translations: BTreeMap<usize, (Vec<f32>, Vec<[f32; 3]>)> = BTreeMap::new();
        for channel in animation.channels() {
            let node = channel.target().node().index();
            if !MESHY_JOINTS.contains(&names[node].as_str()) {
                return Err(format!(
                    "{}: Meshy animation targets non-joint node",
                    path.display()
                ));
            }
            let reader = channel.reader(|buffer| Some(&buffers[buffer.index()]));
            let input = reader
                .read_inputs()
                .ok_or("Meshy animation missing times")?
                .collect::<Vec<_>>();
            match reader
                .read_outputs()
                .ok_or("Meshy animation missing output")?
            {
                ReadOutputs::Rotations(values) => {
                    tracks.insert(node, (input, values.into_f32().collect()));
                }
                ReadOutputs::Translations(values) => {
                    translations.insert(
                        node,
                        (
                            input,
                            values
                                .map(|value| (bevy::prelude::Vec3::from(value) * unit).to_array())
                                .collect(),
                        ),
                    );
                }
                ReadOutputs::Scales(_) => {}
                _ => {
                    return Err(format!(
                        "{}: unsupported Meshy animation output",
                        path.display()
                    ));
                }
            }
        }
        for joint in MESHY_JOINTS {
            let node = names
                .iter()
                .position(|value| value == joint)
                .expect("checked joint order");
            if !tracks.contains_key(&node) || !translations.contains_key(&node) {
                return Err(format!(
                    "{}: Meshy {name} is missing {joint} rotation or translation",
                    path.display()
                ));
            }
        }
        let duration = tracks
            .values()
            .filter_map(|track| track.0.last().copied())
            .chain(
                translations
                    .values()
                    .filter_map(|track| track.0.last().copied()),
            )
            .fold(0f32, f32::max);
        if !duration.is_finite() || duration <= 0. {
            return Err(format!(
                "{}: Meshy {name} has invalid duration",
                path.display()
            ));
        }
        let frames = ((duration * 30.).ceil() as usize + 1).max(2);
        let times = (0..frames)
            .map(|index| duration * index as f32 / (frames - 1) as f32)
            .collect::<Vec<_>>();
        let rotations = times
            .iter()
            .map(|&time| {
                (0..nodes.len())
                    .map(|index| {
                        tracks
                            .get(&index)
                            .map(|track| {
                                relative(bind[index], sample_quat(&track.0, &track.1, time))
                            })
                            .unwrap_or([0., 0., 0., 1.])
                    })
                    .collect()
            })
            .collect();
        let sampled_translations = times
            .iter()
            .map(|&time| {
                (0..nodes.len())
                    .map(|index| {
                        translations
                            .get(&index)
                            .map(|track| sample_vec3(&track.0, &track.1, time))
                            .unwrap_or(bind_translations[index])
                    })
                    .collect()
            })
            .collect();
        out.insert(
            name,
            Motion {
                looped: true,
                duration,
                bones: names.clone(),
                times,
                rotations,
                roots: vec![],
                source: Some("meshy".into()),
                parents: parents.clone(),
                bind_rotations: bind.iter().map(|rotation| rotation.to_array()).collect(),
                translations: sampled_translations,
                bind_translations: bind_translations.clone(),
            },
        );
    }
    if out.len() != 3 {
        return Err(format!(
            "{}: expected Idle, Casual_Walk, and Run_02",
            path.display()
        ));
    }
    Ok(out)
}
fn relative(bind: Quat, value: [f32; 4]) -> [f32; 4] {
    (bind.inverse() * Quat::from_array(value).normalize())
        .normalize()
        .to_array()
}
fn sample_quat(t: &[f32], v: &[[f32; 4]], x: f32) -> [f32; 4] {
    let i = t
        .partition_point(|q| *q <= x)
        .min(t.len().saturating_sub(1));
    let j = i.saturating_sub(1);
    if i == j {
        return v.get(i).copied().unwrap_or([0., 0., 0., 1.]);
    }
    let f = (x - t[j]) / (t[i] - t[j]).max(f32::EPSILON);
    Quat::from_array(v[j])
        .slerp(Quat::from_array(v[i]), f)
        .normalize()
        .to_array()
}
fn sample_vec3(t: &[f32], v: &[[f32; 3]], x: f32) -> [f32; 3] {
    let i = t
        .partition_point(|q| *q <= x)
        .min(t.len().saturating_sub(1));
    let j = i.saturating_sub(1);
    if i == j {
        return v.get(i).copied().unwrap_or([0.; 3]);
    }
    let f = (x - t[j]) / (t[i] - t[j]).max(f32::EPSILON);
    let a = bevy::prelude::Vec3::from(v[j]);
    a.lerp(bevy::prelude::Vec3::from(v[i]), f).to_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ual_imports_are_source_backed_and_catalog_is_reloadable() {
        let root = std::env::temp_dir().join(format!("human-editor-ual-{}", std::process::id()));
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Universal Animation Library");
        // The Quaternius source packs are not part of this repository; the imported
        // clips are (assets/native/animation_libraries*). Skip when they are absent.
        if !source.is_dir() {
            eprintln!("skipping: {} not found", source.display());
            return;
        }
        let result = import_library(&ImportOptions {
            id: "ual1".into(),
            source,
            output_root: root.clone(),
        })
        .unwrap();
        assert_eq!(result.clips.len(), 120);
        assert!(
            result
                .clips
                .iter()
                .any(|x| x.id == "ual1:BackFlip" && x.duration.is_finite() && x.frames > 1)
        );
        assert_eq!(
            AnimationLibraryCatalog::load(&root).unwrap().clips.len(),
            120
        );
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../Universal Animation Library 2/Unreal-Godot/UAL2.glb");
        let result = import_library(&ImportOptions {
            id: "ual2".into(),
            source,
            output_root: root.clone(),
        })
        .unwrap();
        assert_eq!(result.clips.len(), 254);
        assert!(
            result
                .clips
                .iter()
                .any(|x| x.id == "ual2:Bandage_Loop" && x.looped)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn meshy_service_clips_keep_their_metric_bind_translation() {
        let root = std::env::temp_dir().join(format!("human-editor-meshy-{}", std::process::id()));
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/native/cartoon_meshy/rigging");
        for id in ["meshy-female", "meshy-male"] {
            let catalog = import_library(&ImportOptions {
                id: id.into(),
                source: source.clone(),
                output_root: root.clone(),
            })
            .unwrap();
            assert!(
                catalog
                    .clips
                    .iter()
                    .any(|clip| clip.id == format!("{id}:Casual_Walk") && clip.looped)
            );
        }
        let motions = load_motions(&root).unwrap();
        assert_eq!(
            motions
                .clips
                .keys()
                .filter(|id| id.starts_with("meshy-"))
                .count(),
            6
        );
        let walk = &motions.clips["meshy-female:Casual_Walk"];
        let hips = walk.bones.iter().position(|bone| bone == "Hips").unwrap();
        assert_eq!(walk.source.as_deref(), Some("meshy"));
        // Meshy's Armature node scales centimeter joint translations by .01.
        // A value near 1m proves we preserved that parent scale instead of
        // accidentally treating its [1, 1, 1] scale as the Hips translation.
        assert!((0.9..1.1).contains(&walk.bind_translations[hips][1]));
        assert!((walk.translations[0][hips][1] - walk.bind_translations[hips][1]).abs() < 0.1);
        let _ = fs::remove_dir_all(root);
    }
}
