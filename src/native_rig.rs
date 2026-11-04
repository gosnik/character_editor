//! Native Ara skeleton construction, CPU skinning, and sampled-motion playback.
//!
//! The motion file is an *offline* artefact.  It contains normalized local
//! rotation deltas for the canonical game skeleton, so it is safe to apply to
//! the proportioned skeleton built from an individual character's joints.

use crate::{
    native_assets::{Array, NativeLibrary},
    native_character::NativePart,
};
use bevy::prelude::{Mat4, Quat, Vec3};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Clone, Debug)]
pub struct NativeBone {
    pub name: String,
    pub parent: Option<usize>,
    pub head: Vec3,
    pub tail: Vec3,
    /// World-space rest orientation.  Local Y follows the bone.
    pub rotation: Quat,
}

/// A character-specific rig. Positions passed to [`NativeRig::skin_positions`]
/// are already in metres and grounded, as are the bone heads and tails.
#[derive(Clone, Debug)]
pub struct NativeRig {
    pub kind: String,
    pub rest_pose: String,
    pub bones: Vec<NativeBone>,
    base_joints: Vec<[u16; 4]>,
    base_weights: Vec<[f32; 4]>,
    bind_world: Vec<Mat4>,
    inverse_bind: Vec<Mat4>,
    bind_local_rotation: Vec<Quat>,
    bind_local_offset: Vec<Vec3>,
    /// Canonical T-pose world frames used as the UAL retarget reference.
    animation_reference_world: Vec<Quat>,
    /// A-pose to selected-rest-pose transforms, retained for parts assembled
    /// after the rig (garments and accessories included).
    rest_pose_transforms: Vec<Mat4>,
    /// Body-height factor for normalized root-motion samples.
    root_motion_scale: f32,
    /// Extra shoulder abduction (radians) added to every clip; see `arm_clearance`.
    arm_spread: f32,
    motions: Arc<MotionBank>,
}

#[derive(Clone, Debug, Default)]
struct MotionBank {
    clips: BTreeMap<String, MotionClip>,
    t_clips: BTreeMap<String, MotionClip>,
    full_clips: BTreeMap<String, MotionClip>,
    full_t_clips: BTreeMap<String, MotionClip>,
    /// Imported libraries use their own 67-node naming convention and are
    /// retargeted at pose time through `animation_channel`.
    external_clips: BTreeMap<String, MotionClip>,
}
static MOTION_CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<MotionBank>>>> = OnceLock::new();

/// Importing a library can replace its cache while an embedded host remains
/// alive. Subsequent rig builds must not retain the old decoded bank.
pub(crate) fn invalidate_motion_cache(root: &std::path::Path) {
    if let Some(cache) = MOTION_CACHE.get() {
        cache.lock().unwrap().remove(root);
    }
}

#[derive(Clone, Debug)]
struct MotionClip {
    looped: bool,
    duration: f32,
    bones: Vec<String>,
    times: Vec<f32>,
    /// Frame-major normalized local rotation deltas, xyzw.
    rotations: Vec<Vec<[f32; 4]>>,
    /// Optional frame-major root translations in canonical metres.
    roots: Vec<[f32; 3]>,
    source: Option<String>,
    parents: Vec<Option<usize>>,
    bind_rotations: Vec<[f32; 4]>,
    translations: Vec<Vec<[f32; 3]>>,
    bind_translations: Vec<[f32; 3]>,
}

#[derive(Deserialize)]
struct MotionFile {
    #[serde(default)]
    clips: BTreeMap<String, MotionFileClip>,
    #[serde(default, alias = "tClips")]
    t_clips: BTreeMap<String, MotionFileClip>,
    #[serde(default, alias = "fullClips")]
    full_clips: BTreeMap<String, MotionFileClip>,
    #[serde(default, alias = "fullTClips")]
    full_t_clips: BTreeMap<String, MotionFileClip>,
}

#[derive(Deserialize)]
struct MotionFileClip {
    #[serde(default, alias = "loop")]
    looped: bool,
    duration: f32,
    bones: Vec<String>,
    times: Vec<f32>,
    #[serde(alias = "rot")]
    rotations: Vec<Vec<[f32; 4]>>,
    #[serde(default, alias = "root")]
    roots: Vec<[f32; 3]>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    parents: Vec<Option<usize>>,
    #[serde(default, alias = "bindRotations")]
    bind_rotations: Vec<[f32; 4]>,
    #[serde(default)]
    translations: Vec<Vec<[f32; 3]>>,
    #[serde(default, alias = "bindTranslations")]
    bind_translations: Vec<[f32; 3]>,
}

impl From<MotionFileClip> for MotionClip {
    fn from(value: MotionFileClip) -> Self {
        Self {
            looped: value.looped,
            duration: value.duration.max(0.0),
            bones: value.bones,
            times: value.times,
            rotations: value.rotations,
            roots: value.roots,
            source: value.source,
            parents: value.parents,
            bind_rotations: value.bind_rotations,
            translations: value.translations,
            bind_translations: value.bind_translations,
        }
    }
}

#[derive(Clone)]
struct BoneDef {
    name: String,
    parent: Option<String>,
    head: String,
    tail: Option<String>,
    mh: Vec<String>,
}

impl NativeRig {
    /// Builds a game, game-lite, or complete MakeHuman skeleton from the
    /// morphed base vertices. `coords` and `floor` use Ara's decimetre space.
    pub fn build(
        lib: &NativeLibrary,
        coords: &[[f32; 3]],
        floor: f32,
        params: &Value,
    ) -> Result<Self, String> {
        let kind = string_param(params, "rig").unwrap_or("game");
        if kind == "none" {
            return Err("cannot build a NativeRig for rig=none".into());
        }
        let face = bool_param(params, "faceBones").unwrap_or(false);
        let rest_pose = string_param(params, "restPose").unwrap_or("A").to_owned();
        let source = source_bones(&lib.base.meta)?;
        let defs = if kind == "full" {
            full_defs(&source)
        } else {
            game_defs(&source, kind == "game", face)
        };
        if defs.is_empty() {
            return Err("selected rig has no bones".into());
        }

        let joint_indices = lib
            .base
            .meta
            .get("joints")
            .and_then(Value::as_object)
            .ok_or("base metadata has no joints map")?;
        let joint = |name: &str| -> Result<Vec3, String> {
            let indices = joint_indices
                .get(name)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("joint {name:?} is missing"))?;
            if indices.is_empty() {
                return Err(format!("joint {name:?} has no vertices"));
            }
            let mut p = Vec3::ZERO;
            for index in indices {
                let index = index
                    .as_u64()
                    .ok_or_else(|| format!("joint {name:?} has a non-index"))?
                    as usize;
                p += Vec3::from(
                    *coords
                        .get(index)
                        .ok_or_else(|| format!("joint {name:?} references vertex {index}"))?,
                );
            }
            // Ara geometry is decimetres. The character root is grounded in metres.
            Ok(Vec3::new(
                p.x / indices.len() as f32,
                p.y / indices.len() as f32 - floor,
                p.z / indices.len() as f32,
            ) * 0.1)
        };

        let mut name_index = HashMap::new();
        for (index, def) in defs.iter().enumerate() {
            name_index.insert(def.name.as_str(), index);
        }
        let heads: Vec<Vec3> = defs
            .iter()
            .map(|d| joint(&d.head))
            .collect::<Result<_, _>>()?;
        let mut first_child = HashMap::<usize, usize>::new();
        for (index, def) in defs.iter().enumerate() {
            if let Some(parent) = def
                .parent
                .as_deref()
                .and_then(|p| name_index.get(p))
                .copied()
            {
                first_child.entry(parent).or_insert(index);
            }
        }
        // The reference uses these main branches for stable spine/hand frames.
        for (parent, child) in [
            ("Hips", "Spine"),
            ("Spine2", "Neck"),
            ("LeftHand", "LeftHandMiddle1"),
            ("RightHand", "RightHandMiddle1"),
            ("Head", "HeadTop_End"),
        ] {
            if let (Some(&p), Some(&c)) = (name_index.get(parent), name_index.get(child)) {
                first_child.insert(p, c);
            }
        }
        let mut bones: Vec<NativeBone> = Vec::with_capacity(defs.len());
        for (index, def) in defs.iter().enumerate() {
            let parent = def
                .parent
                .as_deref()
                .and_then(|p| name_index.get(p))
                .copied();
            let mut tail = if let Some(tail) = def.tail.as_deref() {
                joint(tail)?
            } else if let Some(&child) = first_child.get(&index) {
                heads[child]
            } else if let Some(parent) = parent {
                let direction = (heads[index] - heads[parent]).normalize_or_zero();
                heads[index] + direction * 0.03
            } else {
                heads[index] + Vec3::Y * 0.03
            };
            let mut direction = tail - heads[index];
            if direction.length_squared() < 1e-10 {
                direction = parent
                    .map(|p| bones[p].tail - bones[p].head)
                    .unwrap_or(Vec3::Y);
                tail = heads[index] + direction.normalize_or_zero() * 0.03;
            }
            bones.push(NativeBone {
                name: def.name.clone(),
                parent,
                head: heads[index],
                tail,
                rotation: bone_frame(direction),
            });
        }

        // Use the same MakeHuman landmark frame as the wardrobe baker, even
        // when the selected skeleton omits finger bones.
        if let Some(hand) = bones
            .iter()
            .position(|b| b.name == "RightHand" || b.name == "wrist.R")
        {
            let wrist = bones[hand].head;
            let index = joint("finger2-1.R____head").ok();
            let middle = joint("finger3-1.R____head").ok();
            let pinky = joint("finger5-1.R____head").ok();
            let thumb = joint("finger1-3.R____tail").ok();
            let (head, tail, rotation) =
                if let (Some(index), Some(middle), Some(pinky), Some(thumb)) =
                    (index, middle, pinky, thumb)
                {
                    let mut palm = (index - wrist).cross(pinky - wrist).normalize_or_zero();
                    if (thumb - wrist).dot(palm) < 0.0 {
                        palm = -palm;
                    }
                    let y = (index - pinky).normalize_or_zero();
                    let forward = (middle - wrist).normalize_or_zero();
                    let z = (forward - y * forward.dot(y)).normalize_or_zero();
                    let x = y.cross(z);
                    let mid = (index + pinky) * 0.5;
                    let head = mid + palm * 0.024 - forward * 0.012;
                    (
                        head,
                        head + y * 0.06,
                        Quat::from_mat3(&bevy::math::Mat3::from_cols(x, y, z)),
                    )
                } else {
                    let head = bones[hand].head + (bones[hand].tail - bones[hand].head) * 0.65;
                    (head, head + Vec3::Y * 0.03, bones[hand].rotation)
                };
            bones.push(NativeBone {
                name: "WeaponR".into(),
                parent: Some(hand),
                head,
                tail,
                rotation,
            });
        }
        let mut animation_reference_bones = bones.clone();
        apply_t_pose(&mut animation_reference_bones);
        let (animation_reference_bind, _, _, _) = bind_matrices(&animation_reference_bones);
        let animation_reference_world = animation_reference_bind
            .iter()
            .map(Quat::from_mat4)
            .collect();
        let rest_pose_transforms = if rest_pose.eq_ignore_ascii_case("T") {
            apply_t_pose(&mut bones)
        } else {
            vec![Mat4::IDENTITY; bones.len()]
        };
        let (bind_world, inverse_bind, bind_local_rotation, bind_local_offset) =
            bind_matrices(&bones);
        // Match Animator's `character.height / 1.8` scale.  `coords` are the
        // morphed, ungrounded base mesh in decimetres, while `floor` is in the
        // same space.
        let root_motion_scale =
            (coords.iter().map(|p| p[1]).fold(floor, f32::max) - floor) * 0.1 / 1.8;
        let (base_joints, base_weights) = base_skin(lib, &source, &defs)?;
        let motions = MotionBank::load(&lib.root)?;
        Ok(Self {
            kind: kind.to_owned(),
            rest_pose,
            bones,
            base_joints,
            base_weights,
            bind_world,
            inverse_bind,
            bind_local_rotation,
            bind_local_offset,
            animation_reference_world,
            rest_pose_transforms,
            root_motion_scale,
            arm_spread: 0.,
            motions,
        })
    }

    /// Spreads the arms outward by `radians` in every sampled clip pose, for bodies
    /// wider than the realistic hm08 body the clips were retargeted against.
    pub fn set_arm_spread(&mut self, radians: f32) {
        self.arm_spread = radians.max(0.);
    }
    pub fn arm_spread(&self) -> f32 {
        self.arm_spread
    }

    /// Abducts each upper arm about the chest's forward axis, in place on local rotations.
    fn spread_arms(&self, locals: &mut [Quat]) {
        if self.arm_spread <= 0. {
            return;
        }
        let parents: Vec<_> = self.bones.iter().map(|b| b.parent).collect();
        let world = world_rotations(locals, &parents);
        for (i, bone) in self.bones.iter().enumerate() {
            // Forearms take part of the spread too, so bent-elbow swings (run,
            // cheer) carry the hands clear of a deeper belly.
            let sign = match bone.name.as_str() {
                "LeftArm" | "upperarm01.L" => 1.,
                "RightArm" | "upperarm01.R" => -1.,
                "LeftForeArm" | "lowerarm01.L" => FOREARM_SPREAD,
                "RightForeArm" | "lowerarm01.R" => -FOREARM_SPREAD,
                _ => continue,
            };
            let Some(parent) = bone.parent else { continue };
            // The body's forward axis (+Z at rest), carried along by the torso.
            let torso = world[parent] * Quat::from_mat4(&self.bind_world[parent]).inverse();
            let spread = Quat::from_axis_angle(torso * Vec3::Z, sign * self.arm_spread);
            locals[i] = (world[parent].inverse() * spread * world[parent] * locals[i]).normalize();
        }
    }

    /// Transfers base-mesh weights to a fitted part through its three
    /// barycentric base references. Parts with no references receive the root.
    pub fn skin_part(&self, part: &NativePart) -> (Vec<[u16; 4]>, Vec<[f32; 4]>) {
        if let Some(joint) = part.rigid_joint.as_deref() {
            let joint = self.rigid_joint_index(joint).unwrap_or(0) as u16;
            return (
                vec![[joint, 0, 0, 0]; part.positions.len()],
                vec![[1.0, 0.0, 0.0, 0.0]; part.positions.len()],
            );
        }
        let mut joints = vec![[0; 4]; part.positions.len()];
        let mut weights = vec![[0.0; 4]; part.positions.len()];
        for vertex in 0..part.positions.len() {
            let refs = part.refs.get(vertex).copied();
            let factors = part
                .ref_weights
                .get(vertex)
                .copied()
                .unwrap_or([1.0, 0.0, 0.0]);
            let mut accumulated = HashMap::<u16, f32>::new();
            if let Some(refs) = refs {
                for (base, factor) in refs.into_iter().zip(factors) {
                    if let (Some(js), Some(ws)) = (
                        self.base_joints.get(base as usize),
                        self.base_weights.get(base as usize),
                    ) {
                        for influence in 0..4 {
                            *accumulated.entry(js[influence]).or_default() +=
                                ws[influence] * factor.max(0.0);
                        }
                    }
                }
            }
            (joints[vertex], weights[vertex]) = top_four(accumulated);
        }
        (joints, weights)
    }

    /// Applies the selected bind-pose conversion to a generated part. Call
    /// this before uploading the part; it is a no-op for the default A-pose.
    pub fn to_rest_pose(&self, part: &NativePart) -> NativePart {
        if self
            .rest_pose_transforms
            .iter()
            .all(|m| *m == Mat4::IDENTITY)
        {
            return part.clone();
        }
        let (joints, weights) = self.skin_part(part);
        let mut posed = part.clone();
        for (vertex, position) in posed.positions.iter_mut().enumerate() {
            let mut output = Vec3::ZERO;
            for influence in 0..4 {
                if let Some(transform) = self
                    .rest_pose_transforms
                    .get(joints[vertex][influence] as usize)
                {
                    output += transform.transform_point3(Vec3::from(*position))
                        * weights[vertex][influence];
                }
            }
            *position = output.to_array();
        }
        posed
    }

    pub fn inverse_bind_matrices(&self) -> &[Mat4] {
        &self.inverse_bind
    }
    pub fn bind_world_matrices(&self) -> &[Mat4] {
        &self.bind_world
    }
    pub fn clip_duration(&self, id: &str) -> Option<f32> {
        self.motions
            .external_clips
            .get(id)
            .or_else(|| self.motions.clips.get(id))
            .map(|clip| clip.duration)
    }
    pub fn clip_looped(&self, id: &str) -> Option<bool> {
        self.motions
            .external_clips
            .get(id)
            .or_else(|| self.motions.clips.get(id))
            .map(|clip| clip.looped)
    }
    /// IDs actually available for this asset root, including imported UAL clips.
    pub fn clip_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.motions.clips.keys().cloned().collect();
        ids.extend(self.motions.external_clips.keys().cloned());
        ids.sort();
        ids.dedup();
        ids
    }

    /// Evaluates the named sampled clip and returns mesh skin matrices. Missing
    /// clips deliberately return the bind pose, so a bad animation selection
    /// never invalidates generated character geometry.
    pub fn pose(&self, clip: &str, time: f32) -> Vec<Mat4> {
        self.sample_pose(clip, time, true)
    }

    /// Sample a single cycle including its final root-motion key for export.
    pub fn pose_clamped(&self, clip: &str, time: f32) -> Vec<Mat4> {
        self.sample_pose(clip, time, false)
    }

    fn sample_pose(&self, clip: &str, time: f32, wrap: bool) -> Vec<Mat4> {
        let bank = if self.kind == "full" && self.rest_pose.eq_ignore_ascii_case("T") {
            &self.motions.full_t_clips
        } else if self.kind == "full" {
            &self.motions.full_clips
        } else if self.rest_pose.eq_ignore_ascii_case("T") {
            &self.motions.t_clips
        } else {
            &self.motions.clips
        };
        let Some(clip) = self
            .motions
            .external_clips
            .get(clip)
            .or_else(|| bank.get(clip))
        else {
            return self.bind_pose();
        };
        if clip.times.is_empty() || clip.rotations.len() != clip.times.len() {
            return self.bind_pose();
        }
        let duration = clip.duration.max(*clip.times.last().unwrap_or(&0.0));
        let sample_time = if wrap && clip.looped && duration > 0.0 {
            time.rem_euclid(duration)
        } else {
            time.clamp(0.0, duration)
        };
        let upper = clip
            .times
            .partition_point(|t| *t <= sample_time)
            .min(clip.times.len() - 1);
        let lower = upper.saturating_sub(1);
        let span = (clip.times[upper] - clip.times[lower]).max(f32::EPSILON);
        let blend = if upper == lower {
            0.0
        } else {
            (sample_time - clip.times[lower]) / span
        };
        let by_name: HashMap<&str, usize> = clip
            .bones
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let root_offset =
            interpolate_vec3(&clip.roots, lower, upper, blend) * self.root_motion_scale;
        let mut local_rotation = self.bind_local_rotation.clone();
        if matches!(clip.source.as_deref(), Some("ual" | "meshy")) {
            return self.pose_calibrated(
                clip,
                lower,
                upper,
                blend,
                root_offset,
                clip.source.as_deref().expect("checked source"),
            );
        }
        for (index, bone) in self.bones.iter().enumerate() {
            let Some(&channel) = animation_channel(&self.kind, &bone.name, clip.source.as_deref())
                .and_then(|name| by_name.get(name.as_str()))
            else {
                continue;
            };
            let a = clip.rotations[lower]
                .get(channel)
                .copied()
                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let b = clip.rotations[upper].get(channel).copied().unwrap_or(a);
            let delta = quat(a).slerp(quat(b), blend);
            local_rotation[index] = self.bind_local_rotation[index] * delta;
        }
        self.spread_arms(&mut local_rotation);
        self.skin_matrices(&local_rotation, root_offset)
    }

    pub fn skin_positions(
        &self,
        positions: &[[f32; 3]],
        joints: &[[u16; 4]],
        weights: &[[f32; 4]],
        matrices: &[Mat4],
    ) -> Vec<[f32; 3]> {
        positions
            .iter()
            .enumerate()
            .map(|(vertex, position)| {
                let joints = joints.get(vertex).copied().unwrap_or([0; 4]);
                let weights = weights.get(vertex).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]);
                let mut output = Vec3::ZERO;
                for influence in 0..4 {
                    if let Some(matrix) = matrices.get(joints[influence] as usize) {
                        output +=
                            matrix.transform_point3(Vec3::from(*position)) * weights[influence];
                    }
                }
                output.to_array()
            })
            .collect()
    }

    fn bind_pose(&self) -> Vec<Mat4> {
        self.bind_world
            .iter()
            .zip(&self.inverse_bind)
            .map(|(world, inverse)| *world * *inverse)
            .collect()
    }
    fn rigid_joint_index(&self, requested: &str) -> Option<usize> {
        let aliases: &[&str] = match requested {
            "WeaponR" => &["WeaponR"],
            "RightHand" | "wrist.R" => &["RightHand", "wrist.R"],
            "LeftHand" | "wrist.L" => &["LeftHand", "wrist.L"],
            "Spine2" | "chest" | "Chest" | "spine02" => {
                &["Spine2", "spine02", "spine03", "spine04"]
            }
            "Spine" | "spine04" => &["Spine", "spine04", "spine03", "spine02"],
            other => &[other],
        };
        aliases
            .iter()
            .find_map(|name| self.bones.iter().position(|bone| bone.name == *name))
    }
    fn skin_matrices(&self, rotations: &[Quat], root_offset: Vec3) -> Vec<Mat4> {
        self.skin_matrices_with_offsets(rotations, &self.bind_local_offset, root_offset)
    }
    fn skin_matrices_with_offsets(
        &self,
        rotations: &[Quat],
        offsets: &[Vec3],
        root_offset: Vec3,
    ) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.bones.len()];
        for (index, bone) in self.bones.iter().enumerate() {
            let local = Mat4::from_rotation_translation(rotations[index], offsets[index]);
            world[index] = bone
                .parent
                .map(|parent| world[parent] * local)
                .unwrap_or_else(|| Mat4::from_translation(root_offset) * local);
        }
        world
            .into_iter()
            .zip(&self.inverse_bind)
            .map(|(world, inverse)| world * *inverse)
            .collect()
    }

    /// Transfers a source armature's world-space rest-relative rotation onto the canonical
    /// T-pose reference, then resolves it back into this rig's A/T local
    /// frames. This is the calibration step that prevents axis-flipped limbs.
    fn pose_calibrated(
        &self,
        clip: &MotionClip,
        lower: usize,
        upper: usize,
        blend: f32,
        root_offset: Vec3,
        source_kind: &str,
    ) -> Vec<Mat4> {
        if clip.parents.len() != clip.bones.len() || clip.bind_rotations.len() != clip.bones.len() {
            return self.bind_pose();
        }
        let mut source_local = Vec::with_capacity(clip.bones.len());
        for i in 0..clip.bones.len() {
            let a = clip.rotations[lower]
                .get(i)
                .copied()
                .unwrap_or([0., 0., 0., 1.]);
            let b = clip.rotations[upper].get(i).copied().unwrap_or(a);
            source_local.push(quat(clip.bind_rotations[i]) * quat(a).slerp(quat(b), blend));
        }
        let source_rest: Vec<_> = clip.bind_rotations.iter().copied().map(quat).collect();
        let source_world = world_rotations(&source_local, &clip.parents);
        let source_rest_world = world_rotations(&source_rest, &clip.parents);
        let by_name: HashMap<&str, usize> = clip
            .bones
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let mut offsets = self.bind_local_offset.clone();
        // A target bone with no UAL counterpart must follow its calibrated
        // parent with its own bind-local frame (eyes, breast secondary bones,
        // twist/end bones). Leaving it in bind *world* space counter-rotates
        // it whenever an ancestor moves.
        let mut desired = vec![Quat::IDENTITY; self.bones.len()];
        for i in 0..self.bones.len() {
            let inherited = self.bones[i]
                .parent
                .map(|p| desired[p] * self.bind_local_rotation[i])
                .unwrap_or(self.bind_local_rotation[i]);
            desired[i] = inherited;
            if let Some(source) =
                animation_channel(&self.kind, &self.bones[i].name, Some(source_kind))
                    .and_then(|n| by_name.get(n.as_str()).copied())
            {
                let delta = source_world[source] * source_rest_world[source].inverse();
                // UAL's clips are authored against its T-pose reference,
                // whereas Meshy's service skeleton is already in the A-pose
                // expected by the selected Ara rig. Applying Meshy deltas to
                // the T reference raises its arms before any animation key.
                let reference = if source_kind == "meshy" {
                    Quat::from_mat4(&self.bind_world[i])
                } else {
                    self.animation_reference_world[i]
                };
                desired[i] = (delta * reference).normalize();
                if clip.translations.len() == clip.times.len()
                    && clip.bind_translations.len() == clip.bones.len()
                {
                    // Translate only the channel's local deviation. Full
                    // source world-position deltas already include every
                    // ancestor's rotational sweep and would be added again
                    // by target hierarchy evaluation.
                    let a = Vec3::from(
                        *clip.translations[lower]
                            .get(source)
                            .unwrap_or(&clip.bind_translations[source]),
                    );
                    let b = Vec3::from(
                        *clip.translations[upper]
                            .get(source)
                            .unwrap_or(&clip.bind_translations[source]),
                    );
                    let source_parent = clip.parents[source]
                        .map(|p| source_world[p])
                        .unwrap_or(Quat::IDENTITY);
                    let world_delta = source_parent
                        * (a.lerp(b, blend) - Vec3::from(clip.bind_translations[source]))
                        * self.root_motion_scale;
                    let local_delta = self.bones[i]
                        .parent
                        .map(|p| desired[p].inverse() * world_delta)
                        .unwrap_or(world_delta);
                    offsets[i] += local_delta;
                }
            }
        }
        let mut locals = desired
            .iter()
            .enumerate()
            .map(|(i, &world)| {
                self.bones[i]
                    .parent
                    .map(|p| desired[p].inverse() * world)
                    .unwrap_or(world)
            })
            .collect::<Vec<_>>();
        self.spread_arms(&mut locals);
        self.skin_matrices_with_offsets(&locals, &offsets, root_offset)
    }
}

/// Share of the shoulder spread also applied at each elbow.
const FOREARM_SPREAD: f32 = 0.6;

/// How far (radians) the torso and hips reach outside a hanging arm: the largest
/// angle, seen from the shoulder joint, of any non-arm body vertex between upper
/// chest and hand height. Comparing it for a reshaped and the realistic body gives
/// the extra abduction the clips need to keep the arms clear (`set_arm_spread`).
pub fn arm_clearance(lib: &NativeLibrary, coords: &[[f32; 3]]) -> Result<f32, String> {
    let joints = lib.base.meta["joints"]
        .as_object()
        .ok_or("base metadata has no joints map")?;
    let joint = |name: &str| -> Result<Vec3, String> {
        let ids = joints
            .get(name)
            .and_then(Value::as_array)
            .filter(|ids| !ids.is_empty())
            .ok_or_else(|| format!("joint {name:?} is missing"))?;
        let mut p = Vec3::ZERO;
        for id in ids {
            let v = id.as_u64().ok_or("joint has a non-index")? as usize;
            p += Vec3::from(*coords.get(v).ok_or("joint index out of range")?);
        }
        Ok(p / ids.len() as f32)
    };
    let source = source_bones(&lib.base.meta)?;
    let arm = |name: &str| {
        let stem = name.split('.').next().unwrap_or("");
        [
            "clavicle",
            "shoulder",
            "upperarm",
            "lowerarm",
            "wrist",
            "metacarpal",
            "finger",
        ]
        .iter()
        .any(|p| stem.starts_with(p))
    };
    let offsets = array(&lib.base, "wOff")?.u32()?;
    let bones = array(&lib.base, "wBone")?.u8()?;
    let values = array(&lib.base, "wVal")?.f32()?;
    let surface: std::collections::BTreeSet<u32> = array(&lib.base, "faceVerts")?
        .u32()?
        .iter()
        .copied()
        .collect();
    let mut widest = f32::MIN;
    for side in ["L", "R"] {
        let shoulder = joint(&format!("upperarm01.{side}____head"))?;
        let length = shoulder.distance(joint(&format!("wrist.{side}____head"))?);
        for &v in &surface {
            let v = v as usize;
            let Some(&p) = coords.get(v) else { continue };
            let (from, to) = (offsets[v] as usize, offsets[v + 1] as usize);
            let arm_weight: f32 = (from..to)
                .filter(|&k| source.get(bones[k] as usize).is_some_and(|b| arm(&b.name)))
                .map(|k| values[k])
                .sum();
            if arm_weight > 0.05 || p[0] * shoulder.x <= 0. {
                continue;
            }
            let down = shoulder.y - p[1];
            if down < 0.25 * length || down > length {
                continue;
            }
            widest = widest.max((p[0].abs() - shoulder.x.abs()).atan2(down));
        }
    }
    Ok(if widest == f32::MIN { 0. } else { widest })
}

fn world_rotations(local: &[Quat], parents: &[Option<usize>]) -> Vec<Quat> {
    let mut world = vec![Quat::IDENTITY; local.len()];
    let mut done = vec![false; local.len()];
    for _ in 0..local.len() {
        for i in 0..local.len() {
            if done[i] {
                continue;
            }
            if let Some(p) = parents[i] {
                if p >= local.len() || !done[p] {
                    continue;
                }
                world[i] = world[p] * local[i];
            } else {
                world[i] = local[i];
            }
            done[i] = true;
        }
    }
    world
}

impl MotionBank {
    /// Decoding all game/full × A/T banks is substantial, so native rebuilds
    /// share one immutable bank per asset root.  Rigs only clone the Arc.
    fn load(root: &std::path::Path) -> Result<Arc<Self>, String> {
        let cache = MOTION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(bank) = cache.lock().unwrap().get(root).cloned() {
            return Ok(bank);
        }
        let bank = Arc::new(Self::load_uncached(root)?);
        cache
            .lock()
            .unwrap()
            .insert(root.to_path_buf(), bank.clone());
        Ok(bank)
    }

    fn load_uncached(root: &std::path::Path) -> Result<Self, String> {
        let plain = root.join("motion.json");
        let compressed = root.join("motion.json.gz");
        let bytes = if plain.exists() {
            fs::read(plain).map_err(|e| format!("motion.json: {e}"))?
        } else if compressed.exists() {
            let mut d = flate2::read::GzDecoder::new(
                fs::File::open(compressed).map_err(|e| format!("motion.json.gz: {e}"))?,
            );
            let mut b = Vec::new();
            d.by_ref()
                // Four canonical banks (game/full × A/T) share this file.
                .take(512 * 1024 * 1024 + 1)
                .read_to_end(&mut b)
                .map_err(|e| format!("motion.json.gz: {e}"))?;
            b
        } else {
            return Err("native motion library is missing motion.json(.gz)".into());
        };
        if bytes.len() > 512 * 1024 * 1024 {
            return Err("motion data exceeds decompression limit".into());
        }
        let file: MotionFile =
            serde_json::from_slice(&bytes).map_err(|e| format!("native motion data: {e}"))?;
        Ok(Self {
            clips: file
                .clips
                .into_iter()
                .map(|(name, clip)| (name, clip.into()))
                .collect(),
            t_clips: file
                .t_clips
                .into_iter()
                .map(|(name, clip)| (name, clip.into()))
                .collect(),
            full_clips: file
                .full_clips
                .into_iter()
                .map(|(name, clip)| (name, clip.into()))
                .collect(),
            full_t_clips: file
                .full_t_clips
                .into_iter()
                .map(|(name, clip)| (name, clip.into()))
                .collect(),
            external_clips: load_external_clips(root)?,
        })
    }
}

fn bind_matrices(bones: &[NativeBone]) -> (Vec<Mat4>, Vec<Mat4>, Vec<Quat>, Vec<Vec3>) {
    let mut world = Vec::with_capacity(bones.len());
    let mut local_rot = Vec::with_capacity(bones.len());
    let mut local_offset = Vec::with_capacity(bones.len());
    for bone in bones {
        let (rotation, offset) = if let Some(parent) = bone.parent {
            (
                bones[parent].rotation.inverse() * bone.rotation,
                bones[parent].rotation.inverse() * (bone.head - bones[parent].head),
            )
        } else {
            (bone.rotation, bone.head)
        };
        local_rot.push(rotation);
        local_offset.push(offset);
        world.push(
            bone.parent
                .map(|p| world[p] * Mat4::from_rotation_translation(rotation, offset))
                .unwrap_or_else(|| Mat4::from_rotation_translation(rotation, offset)),
        );
    }
    let inverse = world.iter().map(|m| m.inverse()).collect();
    (world, inverse, local_rot, local_offset)
}

/// Mirrors Ara's A-pose to T-pose conversion. Each transform is world-space
/// and includes rotations accumulated from its ancestors, which is why skinning
/// a garment with the same transforms keeps it attached to the body.
fn apply_t_pose(bones: &mut [NativeBone]) -> Vec<Mat4> {
    let mut transforms = vec![Mat4::IDENTITY; bones.len()];
    for index in 0..bones.len() {
        let inherited = bones[index]
            .parent
            .map(|parent| transforms[parent])
            .unwrap_or(Mat4::IDENTITY);
        let side = t_pose_side(&bones[index].name);
        let transform = if side == 0.0 {
            inherited
        } else {
            let head = inherited.transform_point3(bones[index].head);
            let direction = inherited
                .transform_vector3(bones[index].tail - bones[index].head)
                .normalize_or_zero();
            let turn = Quat::from_rotation_arc(direction, Vec3::X * side);
            Mat4::from_translation(head)
                * Mat4::from_quat(turn)
                * Mat4::from_translation(-head)
                * inherited
        };
        transforms[index] = transform;
    }
    for (bone, transform) in bones.iter_mut().zip(&transforms) {
        bone.head = transform.transform_point3(bone.head);
        bone.tail = transform.transform_point3(bone.tail);
        bone.rotation = bone_frame(bone.tail - bone.head);
    }
    transforms
}

fn t_pose_side(name: &str) -> f32 {
    if matches!(name, "LeftArm" | "LeftForeArm" | "LeftHand")
        || matches!(
            name,
            "upperarm01.L" | "upperarm02.L" | "lowerarm01.L" | "lowerarm02.L" | "wrist.L"
        )
    {
        1.0
    } else if matches!(name, "RightArm" | "RightForeArm" | "RightHand")
        || matches!(
            name,
            "upperarm01.R" | "upperarm02.R" | "lowerarm01.R" | "lowerarm02.R" | "wrist.R"
        )
    {
        -1.0
    } else {
        0.0
    }
}

/// Each bank uses its skeleton's native channel names: Mixamo-style for game
/// rigs and MakeHuman names for full rigs.
fn animation_channel(_kind: &str, name: &str, source: Option<&str>) -> Option<String> {
    if source == Some("meshy") {
        return meshy_animation_channel(name);
    }
    if source != Some("ual") {
        return Some(name.into());
    }
    let left_side = name.starts_with("Left") || name.ends_with(".L");
    // Full MakeHuman finger chains map one-for-one; collapsing every segment
    // to UAL's first phalanx applies that rotation three times.
    if let Some(rest) = name.strip_prefix("finger") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 3
            && bytes[1] == b'-'
            && (b'1'..=b'5').contains(&bytes[0])
            && (b'1'..=b'3').contains(&bytes[2])
        {
            let digit = match bytes[0] {
                b'1' => "thumb",
                b'2' => "index",
                b'3' => "middle",
                b'4' => "ring",
                _ => "pinky",
            };
            let side = if name.ends_with(".L") { "l" } else { "r" };
            return Some(format!("{digit}_0{}_{}", bytes[2] as char, side));
        }
    }
    if (name.starts_with("LeftHand") || name.starts_with("RightHand"))
        && ["Thumb", "Index", "Middle", "Ring", "Pinky"]
            .iter()
            .any(|part| name.contains(part))
    {
        let (digit, index) = if name.contains("Thumb") {
            ("thumb", 1)
        } else if name.contains("Index") {
            ("index", 1)
        } else if name.contains("Middle") {
            ("middle", 1)
        } else if name.contains("Ring") {
            ("ring", 1)
        } else if name.contains("Pinky") {
            ("pinky", 1)
        } else {
            return None;
        };
        let suffix = name
            .chars()
            .last()
            .and_then(|x| x.to_digit(10))
            .unwrap_or(index)
            .min(3);
        return Some(format!(
            "{digit}_0{}_{}",
            suffix,
            if left_side { "l" } else { "r" }
        ));
    }
    let mapped = match name {
        "Hips" | "spine05" => "pelvis",
        "root" => "root",
        "Spine" | "spine04" => "spine_01",
        "Spine1" | "spine03" => "spine_02",
        "Spine2" | "spine02" => "spine_03",
        "Neck" | "neck01" => "neck_01",
        "Head" | "head" => "Head",
        "LeftShoulder" | "clavicle.L" => "clavicle_l",
        "RightShoulder" | "clavicle.R" => "clavicle_r",
        "LeftArm" | "upperarm01.L" => "upperarm_l",
        "RightArm" | "upperarm01.R" => "upperarm_r",
        "LeftForeArm" | "lowerarm01.L" => "lowerarm_l",
        "RightForeArm" | "lowerarm01.R" => "lowerarm_r",
        "LeftHand" | "wrist.L" => "hand_l",
        "RightHand" | "wrist.R" => "hand_r",
        "LeftUpLeg" | "upperleg01.L" => "thigh_l",
        "RightUpLeg" | "upperleg01.R" => "thigh_r",
        "LeftLeg" | "lowerleg01.L" => "calf_l",
        "RightLeg" | "lowerleg01.R" => "calf_r",
        "LeftFoot" | "foot.L" => "foot_l",
        "RightFoot" | "foot.R" => "foot_r",
        "LeftToeBase" | "toe1-1.L" | "toe2-1.L" | "toe3-1.L" => "ball_l",
        "RightToeBase" | "toe1-1.R" | "toe2-1.R" | "toe3-1.R" => "ball_r",
        _ => return None,
    };
    Some(mapped.into())
}

/// Meshy's service rig intentionally has no finger chains. Finger targets use
/// the hand's single calibrated transform so they move with the source hand
/// once, rather than attempting to invent phalanx channels.
fn meshy_animation_channel(name: &str) -> Option<String> {
    if name.starts_with("finger") {
        return Some(
            if name.ends_with(".R") {
                "RightHand"
            } else {
                "LeftHand"
            }
            .into(),
        );
    }
    if name.starts_with("LeftHand") && name != "LeftHand" {
        return Some("LeftHand".into());
    }
    if name.starts_with("RightHand") && name != "RightHand" {
        return Some("RightHand".into());
    }
    let mapped = match name {
        "Hips" | "spine05" => "Hips",
        "Spine" | "spine04" => "Spine02",
        "Spine1" | "spine03" => "Spine01",
        "Spine2" | "spine02" => "Spine",
        "Neck" | "neck01" => "neck",
        "Head" | "head" => "Head",
        "LeftShoulder" | "clavicle.L" => "LeftShoulder",
        "RightShoulder" | "clavicle.R" => "RightShoulder",
        "LeftArm" | "upperarm01.L" => "LeftArm",
        "RightArm" | "upperarm01.R" => "RightArm",
        "LeftForeArm" | "lowerarm01.L" => "LeftForeArm",
        "RightForeArm" | "lowerarm01.R" => "RightForeArm",
        "LeftHand" | "wrist.L" => "LeftHand",
        "RightHand" | "wrist.R" => "RightHand",
        "LeftUpLeg" | "upperleg01.L" => "LeftUpLeg",
        "RightUpLeg" | "upperleg01.R" => "RightUpLeg",
        "LeftLeg" | "lowerleg01.L" => "LeftLeg",
        "RightLeg" | "lowerleg01.R" => "RightLeg",
        "LeftFoot" | "foot.L" => "LeftFoot",
        "RightFoot" | "foot.R" => "RightFoot",
        "LeftToeBase" | "toe1-1.L" => "LeftToeBase",
        "RightToeBase" | "toe1-1.R" => "RightToeBase",
        _ => return None,
    };
    Some(mapped.into())
}

fn load_external_clips(root: &std::path::Path) -> Result<BTreeMap<String, MotionClip>, String> {
    let path = root.join("animation_libraries.motion.json.gz");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let mut b = Vec::new();
    flate2::read::GzDecoder::new(
        fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?,
    )
    .take(512 * 1024 * 1024 + 1)
    .read_to_end(&mut b)
    .map_err(|e| e.to_string())?;
    if b.len() > 512 * 1024 * 1024 {
        return Err("external animation data exceeds decompression limit".into());
    }
    let f: MotionFile =
        serde_json::from_slice(&b).map_err(|e| format!("external animation data: {e}"))?;
    Ok(f.clips.into_iter().map(|(n, c)| (n, c.into())).collect())
}

fn base_skin(
    lib: &NativeLibrary,
    source: &[SourceBone],
    defs: &[BoneDef],
) -> Result<(Vec<[u16; 4]>, Vec<[f32; 4]>), String> {
    let offsets = array(&lib.base, "wOff")?.u32()?;
    let source_bones = array(&lib.base, "wBone")?.u8()?;
    let values = array(&lib.base, "wVal")?.f32()?;
    if offsets.is_empty()
        || offsets.last().copied().unwrap() as usize != source_bones.len()
        || source_bones.len() != values.len()
    {
        return Err("base skin arrays have inconsistent lengths".into());
    }
    let target: HashMap<&str, usize> = defs
        .iter()
        .enumerate()
        .flat_map(|(i, d)| d.mh.iter().map(move |name| (name.as_str(), i)))
        .collect();
    let source_by_name: HashMap<&str, &SourceBone> = source
        .iter()
        .map(|bone| (bone.name.as_str(), bone))
        .collect();
    let mut mapped = Vec::with_capacity(source.len());
    for bone in source {
        let mut name = Some(bone.name.as_str());
        let mut index = 0;
        while let Some(current) = name {
            if let Some(&target) = target.get(current) {
                index = target;
                break;
            }
            name = source_by_name
                .get(current)
                .and_then(|bone| bone.parent.as_deref());
        }
        mapped.push(index as u16);
    }
    let mut joints = Vec::with_capacity(offsets.len() - 1);
    let mut weights = Vec::with_capacity(offsets.len() - 1);
    for vertex in 0..offsets.len() - 1 {
        let mut all = HashMap::new();
        for entry in offsets[vertex] as usize..offsets[vertex + 1] as usize {
            let source = *source_bones
                .get(entry)
                .ok_or("base skin index out of bounds")? as usize;
            *all.entry(*mapped.get(source).ok_or("base skin bone out of bounds")?)
                .or_insert(0.0) += values[entry];
        }
        let (j, w) = top_four(all);
        joints.push(j);
        weights.push(w);
    }
    Ok((joints, weights))
}

fn array<'a>(pack: &'a crate::native_assets::Pack, name: &str) -> Result<&'a Array, String> {
    pack.arrays
        .get(name)
        .ok_or_else(|| format!("base pack missing {name}"))
}

fn top_four(weights: HashMap<u16, f32>) -> ([u16; 4], [f32; 4]) {
    let mut entries: Vec<_> = weights.into_iter().filter(|(_, w)| *w > 0.0).collect();
    entries.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut joints = [0; 4];
    let mut values = [0.0; 4];
    for (i, (joint, weight)) in entries.into_iter().take(4).enumerate() {
        joints[i] = joint;
        values[i] = weight;
    }
    let sum: f32 = values.iter().sum();
    if sum > 0.0 {
        for weight in &mut values {
            *weight /= sum;
        }
    } else {
        values[0] = 1.0;
    }
    (joints, values)
}
fn quat(v: [f32; 4]) -> Quat {
    Quat::from_xyzw(v[0], v[1], v[2], v[3]).normalize()
}
fn interpolate_vec3(values: &[[f32; 3]], a: usize, b: usize, t: f32) -> Vec3 {
    let a = Vec3::from(values.get(a).copied().unwrap_or([0.0; 3]));
    let b = Vec3::from(values.get(b).copied().unwrap_or(a.to_array()));
    a.lerp(b, t)
}
fn bone_frame(direction: Vec3) -> Quat {
    let y = direction.normalize_or_zero();
    let reference = if y.z.abs() > 0.85 { Vec3::Y } else { Vec3::Z };
    let x = y.cross(reference).normalize_or_zero();
    let z = x.cross(y);
    Quat::from_mat3(&bevy::math::Mat3::from_cols(x, y, z))
}

#[derive(Clone)]
struct SourceBone {
    name: String,
    parent: Option<String>,
    head: String,
    tail: String,
}
fn source_bones(meta: &Value) -> Result<Vec<SourceBone>, String> {
    meta.get("bones")
        .and_then(Value::as_array)
        .ok_or("base metadata has no bones")?
        .iter()
        .map(|v| {
            Ok(SourceBone {
                name: v
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("bone name missing")?
                    .to_owned(),
                parent: v.get("parent").and_then(Value::as_str).map(str::to_owned),
                head: v
                    .get("head")
                    .and_then(Value::as_str)
                    .ok_or("bone head missing")?
                    .to_owned(),
                tail: v
                    .get("tail")
                    .and_then(Value::as_str)
                    .ok_or("bone tail missing")?
                    .to_owned(),
            })
        })
        .collect()
}
fn full_defs(source: &[SourceBone]) -> Vec<BoneDef> {
    let mut remaining: HashMap<_, _> = source.iter().map(|b| (b.name.as_str(), b)).collect();
    let mut out = Vec::new();
    while !remaining.is_empty() {
        let ready: Vec<_> = remaining
            .values()
            .filter(|b| {
                b.parent
                    .as_deref()
                    .is_none_or(|p| !remaining.contains_key(p))
            })
            .map(|b| b.name.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        for name in ready {
            let b = remaining.remove(name.as_str()).unwrap();
            out.push(BoneDef {
                name: b.name.clone(),
                parent: b.parent.clone(),
                head: b.head.clone(),
                tail: Some(b.tail.clone()),
                mh: vec![b.name.clone()],
            });
        }
    }
    out
}
fn game_defs(source: &[SourceBone], fingers: bool, face: bool) -> Vec<BoneDef> {
    let all: Vec<_> = source.iter().map(|b| b.name.as_str()).collect();
    let mut out = Vec::new();
    let mut add =
        |name: &str, parent: Option<&str>, head: String, tail: Option<String>, mh: Vec<String>| {
            out.push(BoneDef {
                name: name.into(),
                parent: parent.map(str::to_owned),
                head,
                tail,
                mh,
            });
        };
    let h = |n: &str| format!("{n}____head");
    let t = |n: &str| format!("{n}____tail");
    add(
        "Hips",
        None,
        h("spine05"),
        None,
        vec!["root", "pelvis.L", "pelvis.R", "spine05"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    );
    add(
        "Spine",
        Some("Hips"),
        h("spine04"),
        None,
        vec!["spine04".into()],
    );
    add(
        "Spine1",
        Some("Spine"),
        h("spine03"),
        None,
        vec!["spine03".into()],
    );
    add(
        "Spine2",
        Some("Spine1"),
        h("spine02"),
        None,
        vec!["spine02", "spine01"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    );
    // Keep breast influences separate from the chest so the native secondary
    // motion layer can move them without disturbing the spine skinning.
    for (side, label) in [("L", "Left"), ("R", "Right")] {
        add(
            &format!("{label}Breast"),
            Some("Spine2"),
            h(&format!("breast.{side}")),
            Some(t(&format!("breast.{side}"))),
            vec![format!("breast.{side}")],
        );
    }
    add(
        "Neck",
        Some("Spine2"),
        h("neck01"),
        None,
        vec!["neck01", "neck02", "neck03"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    );
    add(
        "Head",
        Some("Neck"),
        h("head"),
        None,
        all.iter()
            .filter(|n| {
                n.starts_with("head")
                    || n.starts_with("special")
                    || n.starts_with("oris")
                    || n.starts_with("levator")
                    || n.starts_with("risorius")
                    || n.starts_with("orbicularis")
                    || n.starts_with("oculi")
                    || n.starts_with("temporalis")
            })
            .map(|n| (*n).to_owned())
            .collect(),
    );
    add("HeadTop_End", Some("Head"), t("head"), None, vec![]);
    if face {
        add(
            "Jaw",
            Some("Head"),
            h("jaw"),
            Some(t("jaw")),
            all.iter()
                .filter(|n| n.starts_with("jaw") || n.starts_with("tongue"))
                .map(|n| (*n).to_owned())
                .collect(),
        );
        for (s, side) in [("L", "Left"), ("R", "Right")] {
            add(
                &format!("{side}Eye"),
                Some("Head"),
                h(&format!("eye.{s}")),
                Some(t(&format!("eye.{s}"))),
                vec![format!("eye.{s}")],
            );
        }
    }
    for (s, side) in [("L", "Left"), ("R", "Right")] {
        add(
            &format!("{side}Shoulder"),
            Some("Spine2"),
            h(&format!("clavicle.{s}")),
            None,
            vec![format!("clavicle.{s}")],
        );
        add(
            &format!("{side}Arm"),
            Some(&format!("{side}Shoulder")),
            h(&format!("upperarm01.{s}")),
            None,
            vec![
                format!("shoulder01.{s}"),
                format!("upperarm01.{s}"),
                format!("upperarm02.{s}"),
            ],
        );
        add(
            &format!("{side}ForeArm"),
            Some(&format!("{side}Arm")),
            h(&format!("lowerarm01.{s}")),
            None,
            vec![format!("lowerarm01.{s}"), format!("lowerarm02.{s}")],
        );
        add(
            &format!("{side}Hand"),
            Some(&format!("{side}ForeArm")),
            h(&format!("wrist.{s}")),
            if fingers {
                None
            } else {
                Some(t(&format!("finger3-1.{s}")))
            },
            vec![format!("wrist.{s}")],
        );
        if fingers {
            for (f, i) in [
                ("Thumb", 1),
                ("Index", 2),
                ("Middle", 3),
                ("Ring", 4),
                ("Pinky", 5),
            ] {
                let mut parent = format!("{side}Hand");
                for k in 1..=3 {
                    let name = format!("{side}Hand{f}{k}");
                    add(
                        &name,
                        Some(&parent),
                        h(&format!("finger{i}-{k}.{s}")),
                        None,
                        vec![format!("finger{i}-{k}.{s}")],
                    );
                    parent = name;
                }
                add(
                    &format!("{side}Hand{f}4"),
                    Some(&parent),
                    t(&format!("finger{i}-3.{s}")),
                    None,
                    vec![],
                );
            }
        }
        add(
            &format!("{side}UpLeg"),
            Some("Hips"),
            h(&format!("upperleg01.{s}")),
            None,
            vec![format!("upperleg01.{s}"), format!("upperleg02.{s}")],
        );
        add(
            &format!("{side}Leg"),
            Some(&format!("{side}UpLeg")),
            h(&format!("lowerleg01.{s}")),
            None,
            vec![format!("lowerleg01.{s}"), format!("lowerleg02.{s}")],
        );
        add(
            &format!("{side}Foot"),
            Some(&format!("{side}Leg")),
            h(&format!("foot.{s}")),
            None,
            vec![format!("foot.{s}")],
        );
        add(
            &format!("{side}ToeBase"),
            Some(&format!("{side}Foot")),
            t(&format!("foot.{s}")),
            None,
            all.iter()
                .filter(|n| n.starts_with("toe") && n.ends_with(s))
                .map(|n| (*n).to_owned())
                .collect(),
        );
        add(
            &format!("{side}Toe_End"),
            Some(&format!("{side}ToeBase")),
            t(&format!("toe3-3.{s}")),
            None,
            vec![],
        );
    }
    out
}
fn string_param<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).or_else(|| {
        value
            .get("rig")
            .and_then(|r| r.get(key))
            .and_then(Value::as_str)
    })
}
fn bool_param(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool).or_else(|| {
        value
            .get("rig")
            .and_then(|r| r.get(key))
            .and_then(Value::as_bool)
    })
}

#[cfg(test)]
mod ual_tests {
    use super::animation_channel;
    #[test]
    fn full_rig_does_not_apply_a_ual_segment_twice() {
        assert_eq!(
            animation_channel("full", "upperarm01.L", Some("ual")).as_deref(),
            Some("upperarm_l")
        );
        assert_eq!(animation_channel("full", "upperarm02.L", Some("ual")), None);
        assert_eq!(
            animation_channel("full", "finger2-1.R", Some("ual")).as_deref(),
            Some("index_01_r")
        );
        assert_eq!(
            animation_channel("full", "finger2-3.R", Some("ual")).as_deref(),
            Some("index_03_r")
        );
    }

    #[test]
    fn meshy_spine_order_and_missing_fingers_follow_the_service_hierarchy() {
        assert_eq!(
            animation_channel("game", "Spine", Some("meshy")).as_deref(),
            Some("Spine02")
        );
        assert_eq!(
            animation_channel("full", "spine02", Some("meshy")).as_deref(),
            Some("Spine")
        );
        assert_eq!(
            animation_channel("full", "finger2-1.R", Some("meshy")).as_deref(),
            Some("RightHand")
        );
    }
}
