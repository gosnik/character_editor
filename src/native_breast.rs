//! Localized MakeHuman breast shaping and inertial secondary motion.
//!
//! Geometry is adjusted before proxy fitting, so fitted garments inherit the
//! same silhouette.  Secondary motion is deliberately separate from the rig:
//! it only changes the two breast skin matrices after the primary pose is
//! evaluated.

use crate::{
    native_assets::{Array, NativeLibrary, Pack},
    native_rig::NativeRig,
};
use bevy::prelude::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Controls the additional breast shape and its optional secondary motion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BreastSettings {
    pub volume: f32,
    pub projection: f32,
    pub lift: f32,
    pub separation: f32,
    pub dynamics: bool,
    pub stiffness: f32,
    pub damping: f32,
    pub motion_strength: f32,
}

impl Default for BreastSettings {
    fn default() -> Self {
        Self {
            volume: 1.0,
            projection: 1.0,
            lift: 0.0,
            separation: 0.0,
            dynamics: true,
            stiffness: 22.0,
            damping: 0.65,
            motion_strength: 0.65,
        }
    }
}

impl BreastSettings {
    pub fn validate(&self) -> Result<(), String> {
        range("volume", self.volume, 0.0, 2.5)?;
        range("projection", self.projection, 0.2, 2.5)?;
        range("lift", self.lift, -1.0, 1.0)?;
        range("separation", self.separation, -0.5, 0.5)?;
        range("stiffness", self.stiffness, 2.0, 80.0)?;
        range("damping", self.damping, 0.1, 2.0)?;
        range("motion_strength", self.motion_strength, 0.0, 2.0)
    }

    fn neutral_shape(&self) -> bool {
        self.volume == 1.0 && self.projection == 1.0 && self.lift == 0.0 && self.separation == 0.0
    }
}

fn range(name: &str, value: f32, min: f32, max: f32) -> Result<(), String> {
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(format!(
            "breast {name} must be finite and within {min}..={max}"
        ));
    }
    Ok(())
}

fn array<'a>(pack: &'a Pack, name: &str) -> Result<&'a Array, String> {
    pack.arrays
        .get(name)
        .ok_or_else(|| format!("base pack missing {name}"))
}

fn joint(meta: &Value, name: &str, coords: &[[f32; 3]]) -> Result<Vec3, String> {
    let vertices = joint_vertices(meta, name)?;
    let mut point = Vec3::ZERO;
    for &index in &vertices {
        point += Vec3::from(*coords.get(index).ok_or_else(|| {
            format!("breast joint {name:?} references vertex {index} outside coordinates")
        })?);
    }
    Ok(point / vertices.len() as f32)
}

fn joint_vertices(meta: &Value, name: &str) -> Result<Vec<usize>, String> {
    let vertices = meta
        .get("joints")
        .and_then(Value::as_object)
        .and_then(|joints| joints.get(name))
        .and_then(Value::as_array)
        .ok_or_else(|| format!("breast joint {name:?} is missing"))?;
    if vertices.is_empty() {
        return Err(format!("breast joint {name:?} has no vertices"));
    }
    vertices
        .iter()
        .map(|vertex| {
            vertex
                .as_u64()
                .ok_or_else(|| format!("breast joint {name:?} has an invalid vertex"))?
                .try_into()
                .map_err(|_| format!("breast joint {name:?} vertex exceeds usize"))
        })
        .collect()
}

#[derive(Clone)]
struct SourceBreast {
    index: u8,
    head: Vec3,
    tail: Vec3,
    side: f32,
    tail_name: String,
}

fn source_breasts(lib: &NativeLibrary, coords: &[[f32; 3]]) -> Result<[SourceBreast; 2], String> {
    let bones = lib
        .base
        .meta
        .get("bones")
        .and_then(Value::as_array)
        .ok_or("base metadata has no bones")?;
    let breast = |name: &str, side: f32| -> Result<SourceBreast, String> {
        let (index, bone) = bones
            .iter()
            .enumerate()
            .find(|(_, bone)| bone.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| format!("base metadata has no {name} bone"))?;
        let head_name = bone
            .get("head")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name} has no head anchor"))?;
        let tail_name = bone
            .get("tail")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name} has no nipple anchor"))?;
        let index = u8::try_from(index).map_err(|_| format!("{name} source index exceeds u8"))?;
        Ok(SourceBreast {
            index,
            head: joint(&lib.base.meta, head_name, coords)?,
            tail: joint(&lib.base.meta, tail_name, coords)?,
            side,
            tail_name: tail_name.to_owned(),
        })
    };
    Ok([breast("breast.L", 1.0)?, breast("breast.R", -1.0)?])
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Reshapes vertices which are explicitly weighted to MakeHuman's breast
/// source bones. Coordinates remain in MakeHuman decimetres.
///
/// The exact neutral shape is a no-op, including for a male character. This
/// matters because the normal target stack is authoritative at its defaults.
/// Areola dome radius for projection, as a share of the breast bone length.
const AREOLA_RADIUS: f32 = 0.18;
/// Skin-weight band of the breast's rim, which sets its chest-wall level.
const RIM_WEIGHT: (f32, f32) = (0.03, 0.15);
/// Areola and nipple relief, in decimetres (hm08 units; 0.01 = 1 mm): heights at
/// projection 1 (the cartoon base, which has no relief of its own) and their gain per
/// unit of projection. At projection 2 the areola stands 5 mm above the breast and
/// the nipple 12 mm above the areola.
const AREOLA_HEIGHT: (f32, f32) = (0.01, 0.04);
const NIPPLE_HEIGHT: (f32, f32) = (0.03, 0.09);
/// Below projection 1 the nipple sinks by this much per unit (dm): through flat to
/// 2 mm inverted at the slider minimum (0.2). The areola keeps its height there and
/// only rounds off (its flat top shrinks to nothing).
const NIPPLE_SINK: f32 = 0.0625;
/// Flat-top and outer radii (dm) of the areola and nipple mounds around the tip.
const AREOLA_RADII: (f32, f32) = (0.12, 0.20);
const NIPPLE_RADII: (f32, f32) = (0.045, 0.09);
/// Skin weight above which a vertex scales fully with projection.
const MEMBER_WEIGHT: f32 = 0.25;

pub fn reshape(
    lib: &NativeLibrary,
    coords: &mut [[f32; 3]],
    gender: f32,
    settings: &BreastSettings,
) -> Result<(), String> {
    settings.validate()?;
    if !gender.is_finite() {
        return Err("gender must be finite".into());
    }
    if settings.neutral_shape() {
        return Ok(());
    }
    let female = (1.0 - gender).clamp(0.0, 1.0);
    if female == 0.0 {
        return Ok(());
    }
    let offsets = array(&lib.base, "wOff")?.u32()?;
    let bones = array(&lib.base, "wBone")?.u8()?;
    let values = array(&lib.base, "wVal")?.f32()?;
    if offsets.len() != coords.len() + 1
        || offsets.last().copied().unwrap_or_default() as usize != bones.len()
        || bones.len() != values.len()
    {
        return Err("base breast skin arrays do not match coordinates".into());
    }
    let breasts = source_breasts(lib, coords)?;
    let source = coords.to_vec();
    let mut reshaped = vec![false; coords.len()];
    // Chest-wall level of each breast along its axis: projection scales how far each
    // point stands out from it, so the mound keeps its rounded shape.
    // The wall is the mean level of the breast's rim, where its skin weight fades out.
    let (mut wall, mut rim) = ([0f32; 2], [0f32; 2]);
    for (vertex, point) in source.iter().enumerate() {
        let (start, end) = (offsets[vertex] as usize, offsets[vertex + 1] as usize);
        for (b, breast) in breasts.iter().enumerate() {
            let weight: f32 = (start..end.min(bones.len()))
                .filter(|&entry| bones[entry] == breast.index)
                .map(|entry| values[entry].max(0.0))
                .sum();
            let axis = (breast.tail - breast.head).normalize_or_zero();
            let along = (Vec3::from(*point) - breast.head).dot(axis);
            if weight > RIM_WEIGHT.0 && weight < RIM_WEIGHT.1 && along > 0.0 {
                wall[b] += along;
                rim[b] += 1.0;
            }
        }
    }
    for b in 0..2 {
        wall[b] = if rim[b] > 0.0 { wall[b] / rim[b] } else { 0.0 };
    }
    for (vertex, point) in source.iter().enumerate() {
        let start = offsets[vertex] as usize;
        let end = offsets[vertex + 1] as usize;
        if start > end || end > bones.len() {
            return Err("base breast skin offset is out of bounds".into());
        }
        for (b, breast) in breasts.iter().enumerate() {
            let weight: f32 = (start..end)
                .filter(|&entry| bones[entry] == breast.index)
                .map(|entry| values[entry].max(0.0))
                .sum();
            if weight <= 0.0 {
                continue;
            }
            let axis = breast.tail - breast.head;
            let length = axis.length();
            if length <= 1e-5 {
                return Err("breast anchors are degenerate".into());
            }
            let forward = axis / length;
            let center = breast.head.lerp(breast.tail, 0.62);
            let from_center = Vec3::from(*point) - center;
            let along = from_center.dot(forward);
            let radial = from_center - forward * along;
            // The source weight establishes anatomical membership; this second
            // smooth envelope prevents a hard seam at its outer edge.
            let local = 1.0 - smoothstep(length * 1.05, length * 1.9, from_center.length());
            // Projection scales each point's stand-out from the chest wall, and carries
            // the nipple and areola fully (a cosine dome around the tip on top of the
            // skin weight), so the tip moves with its neighbours, not as a spike or cone.
            let tip = Vec3::from(*point).distance(breast.tail) / (length * AREOLA_RADIUS);
            let areola = if tip < 1.0 {
                0.5 * (1.0 + (std::f32::consts::PI * tip).cos())
            } else {
                0.0
            };
            let influence = weight * local * female;
            // Membership saturates quickly so the whole mound scales, not just the
            // heavily weighted tip; the standout itself falls to zero at the rim.
            let member = (weight / MEMBER_WEIGHT).min(1.0) * local;
            let projected = member.max(areola) * female;
            if projected <= 1e-6 {
                continue;
            }
            reshaped[vertex] = true;
            let volume = (radial * ((settings.volume - 1.0) * 0.72)
                + forward * ((settings.volume - 1.0) * length * 0.32))
                * influence;
            let standout = ((Vec3::from(*point) - breast.head).dot(forward) - wall[b]).max(0.0);
            let projection = forward * ((settings.projection - 1.0) * standout * projected);
            let lift = Vec3::Y * (settings.lift * length * 0.72);
            let separation = Vec3::X * (breast.side * settings.separation * length * 0.70);
            let changed =
                Vec3::from(*point) + volume + projection + (lift + separation) * influence;
            coords[vertex] = changed.to_array();
        }
    }
    // Breast-tail anchor vertices that carry no breast weight (helpers) are moved
    // explicitly so the rig's anchors follow an extreme projection too, without
    // touching the shared sternum head or unrelated chest vertices. Surface
    // vertices were already reshaped smoothly above; moving them again by the
    // full delta pulled the nipple out as a spike.
    for (b, breast) in breasts.iter().enumerate() {
        let axis = breast.tail - breast.head;
        let length = axis.length();
        let forward = axis / length;
        let standout = (length - wall[b]).max(0.0);
        let delta = forward
            * ((settings.volume - 1.0) * length * 0.32 + (settings.projection - 1.0) * standout)
            + Vec3::Y * (settings.lift * length * 0.72)
            + Vec3::X * (breast.side * settings.separation * length * 0.70);
        for vertex in joint_vertices(&lib.base.meta, &breast.tail_name)? {
            if reshaped.get(vertex).copied().unwrap_or(false) {
                continue;
            }
            *coords
                .get_mut(vertex)
                .ok_or_else(|| format!("breast tail vertex {vertex} outside coordinates"))? =
                (Vec3::from(source[vertex]) + delta * female).to_array();
        }
    }
    Ok(())
}

/// Integrator state for one breast. Values are in world metres.
#[derive(Clone, Debug)]
pub struct SecondaryMotion {
    pub offset: Vec3,
    pub velocity: Vec3,
}

impl Default for SecondaryMotion {
    fn default() -> Self {
        Self {
            offset: Vec3::ZERO,
            velocity: Vec3::ZERO,
        }
    }
}

/// Resettable two-breast secondary-motion state. Call [`Self::reset`] on a
/// clip change or seek; constructing `Default` is also a clean reset.
#[derive(Clone, Debug, Default)]
pub struct BreastMotion {
    pub left: SecondaryMotion,
    pub right: SecondaryMotion,
    previous_driver: Option<Vec3>,
    root_velocity: Vec3,
}

impl BreastMotion {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Advances the spring from the supplied primary root position and rotates
    /// only `LeftBreast`/`RightBreast` (or full-rig `breast.L`/`breast.R`).
    /// `matrices` contains regular skin matrices from [`NativeRig::pose`].
    pub fn apply(
        &mut self,
        rig: &NativeRig,
        matrices: &mut [Mat4],
        root_world: Vec3,
        dt: f32,
        settings: &BreastSettings,
        gender: f32,
    ) {
        if settings.validate().is_err()
            || !root_world.is_finite()
            || !dt.is_finite()
            || dt < 0.0
            || !gender.is_finite()
            || !settings.dynamics
            || gender >= 1.0
        {
            if !settings.dynamics || !dt.is_finite() || dt < 0.0 {
                self.reset();
            }
            return;
        }
        if dt > 0.0 {
            let female = (1.0 - gender).clamp(0.0, 1.0);
            // The root is often static in in-place locomotion clips. The average
            // of the primary breast pivots carries torso bob and rotation into the
            // driver, while `root_world` adds an entity's external locomotion.
            let primary_driver =
                primary_chest_driver(rig, matrices).unwrap_or(Vec3::ZERO) + root_world;
            let previous = self.previous_driver.replace(primary_driver);
            let elapsed = dt.min(0.1);
            let acceleration = if let Some(previous) = previous {
                let velocity =
                    ((primary_driver - previous) / dt.max(1.0 / 240.0)).clamp_length_max(12.0);
                let acceleration =
                    ((velocity - self.root_velocity) / dt.max(1.0 / 240.0)).clamp_length_max(40.0);
                self.root_velocity = velocity;
                acceleration
            } else {
                self.root_velocity = Vec3::ZERO;
                Vec3::ZERO
            };
            let steps = (elapsed / (1.0 / 120.0)).ceil().max(1.0) as usize;
            let h = elapsed / steps as f32;
            let drive = -acceleration * (settings.motion_strength * female * 0.0045);
            spring(&mut self.left, drive, h, steps, settings);
            spring(&mut self.right, drive, h, steps, settings);
        }
        for (names, state) in [
            (["LeftBreast", "breast.L"], &self.left),
            (["RightBreast", "breast.R"], &self.right),
        ] {
            let Some(index) = rig
                .bones
                .iter()
                .position(|bone| names.iter().any(|name| bone.name == *name))
            else {
                continue;
            };
            let bind_world = rig.bind_world_matrices()[index];
            let Some(current) = matrices.get(index).copied() else {
                continue;
            };
            let world = current * bind_world;
            // Reconstructing through the parent keeps the pivot valid even
            // when the primary animation has rotated the torso.
            let pivot = rig.bones[index]
                .parent
                .and_then(|parent| {
                    matrices
                        .get(parent)
                        .zip(rig.bind_world_matrices().get(parent))
                })
                .map(|(parent_skin, parent_bind)| {
                    let parent_world = *parent_skin * *parent_bind;
                    let local = parent_world.inverse() * world;
                    parent_world.transform_point3(local.transform_point3(Vec3::ZERO))
                })
                .unwrap_or_else(|| world.transform_point3(Vec3::ZERO));
            let axis = world.transform_vector3(Vec3::Y).normalize_or_zero();
            let deflection = state.offset - axis * state.offset.dot(axis);
            let angle = (deflection.length()
                / (rig.bones[index].tail - rig.bones[index].head)
                    .length()
                    .max(0.025))
            .clamp(0.0, 0.35);
            if angle <= 1e-6 {
                continue;
            }
            let turn_axis = axis.cross(deflection).normalize_or_zero();
            if turn_axis.length_squared() <= 1e-8 {
                continue;
            }
            let rotate = Mat4::from_translation(pivot)
                * Mat4::from_quat(Quat::from_axis_angle(turn_axis, angle))
                * Mat4::from_translation(-pivot);
            matrices[index] = (rotate * world) * rig.inverse_bind_matrices()[index];
        }
    }
}

fn spring(
    state: &mut SecondaryMotion,
    target: Vec3,
    h: f32,
    steps: usize,
    settings: &BreastSettings,
) {
    let omega = settings.stiffness.sqrt();
    let drag = 2.0 * settings.damping * omega;
    for _ in 0..steps {
        let acceleration = (target - state.offset) * (omega * omega) - state.velocity * drag;
        state.velocity = (state.velocity + acceleration * h).clamp_length_max(2.5);
        state.offset = (state.offset + state.velocity * h).clamp_length_max(0.12);
    }
}

fn primary_chest_driver(rig: &NativeRig, matrices: &[Mat4]) -> Option<Vec3> {
    let breast_pivots: Vec<Vec3> = rig
        .bones
        .iter()
        .enumerate()
        .filter(|(_, bone)| {
            matches!(
                bone.name.as_str(),
                "LeftBreast" | "RightBreast" | "breast.L" | "breast.R"
            )
        })
        .filter_map(|(index, _)| {
            matrices
                .get(index)
                .zip(rig.bind_world_matrices().get(index))
                .map(|(skin, bind)| (*skin * *bind).transform_point3(Vec3::ZERO))
        })
        .collect();
    if !breast_pivots.is_empty() {
        let count = breast_pivots.len() as f32;
        return Some(breast_pivots.into_iter().sum::<Vec3>() / count);
    }
    rig.bones
        .iter()
        .position(|bone| matches!(bone.name.as_str(), "Spine2" | "spine02"))
        .and_then(|index| {
            matrices
                .get(index)
                .zip(rig.bind_world_matrices().get(index))
                .map(|(skin, bind)| (*skin * *bind).transform_point3(Vec3::ZERO))
        })
}

/// Deterministically simulates secondary motion from the primary clip start
/// through `time`. Exporters may cache this per clip/sample instead of keeping
/// runtime state. The driver is the actual animated torso, including root movement.
pub fn bake_secondary(
    rig: &NativeRig,
    clip: &str,
    time: f32,
    settings: &BreastSettings,
    gender: f32,
) -> Vec<Mat4> {
    let end = time.max(0.0).min(600.0);
    let mut motion = BreastMotion::default();
    let mut sample = 0.0;
    while sample < end {
        let dt = (end - sample).min(1.0 / 120.0);
        sample += dt;
        let mut matrices = rig.pose(clip, sample);
        motion.apply(rig, &mut matrices, Vec3::ZERO, dt, settings, gender);
    }
    let mut matrices = rig.pose(clip, end);
    motion.apply(rig, &mut matrices, Vec3::ZERO, 0.0, settings, gender);
    matrices
}

/// Raises the areola and nipple as two rounded mounds along each breast's axis
/// (below projection 1 the areola rounds off and the nipple inverts).
/// `base` (0..1) adds the relief a smooth surface lacks at projection 1 (cartoon mode:
/// the cartoon fit has no nipples); realistic MakeHuman already models them, so it
/// gets only the change with projection. Heights are `AREOLA_HEIGHT`/`NIPPLE_HEIGHT`.
pub fn nipple_relief(
    lib: &NativeLibrary,
    coords: &mut [[f32; 3]],
    gender: f32,
    projection: f32,
    base: f32,
) -> Result<(), String> {
    let female = if gender.is_finite() { (1.0 - gender).clamp(0.0, 1.0) } else { 0.0 };
    if female == 0.0 {
        return Ok(());
    }
    let above = (projection - 1.0).max(0.0);
    let below = ((1.0 - projection) / 0.8).clamp(0.0, 1.0);
    let areola = AREOLA_HEIGHT.0 * base + AREOLA_HEIGHT.1 * above;
    let nipple = NIPPLE_HEIGHT.0 * base + NIPPLE_HEIGHT.1 * above - NIPPLE_SINK * 0.8 * below;
    if areola == 0.0 && nipple == 0.0 {
        return Ok(());
    }
    let areola_top = AREOLA_RADII.0 * (1.0 - below);
    let breasts = source_breasts(lib, coords)?;
    for breast in &breasts {
        let forward = (breast.tail - breast.head).normalize_or_zero();
        for point in coords.iter_mut() {
            let d = Vec3::from(*point).distance(breast.tail);
            if d >= AREOLA_RADII.1 {
                continue;
            }
            let lift = areola * (1.0 - smoothstep(areola_top, AREOLA_RADII.1, d))
                + nipple * (1.0 - smoothstep(NIPPLE_RADII.0, NIPPLE_RADII.1, d));
            *point = (Vec3::from(*point) + forward * lift * female).to_array();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ara::AraRecipe, native_character::NativeCharacter, native_morph::apply_targets};
    use serde_json::json;

    fn library() -> NativeLibrary {
        NativeLibrary::load(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native")).unwrap()
    }
    fn coords(lib: &NativeLibrary) -> Vec<[f32; 3]> {
        apply_targets(lib, &json!({"gender": 0.0})).unwrap()
    }

    #[test]
    fn neutral_shape_is_byte_identical() {
        let lib = library();
        let mut actual = coords(&lib);
        let expected = actual.clone();
        reshape(&lib, &mut actual, 0.0, &BreastSettings::default()).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn shape_is_local_and_fades_for_male() {
        let lib = library();
        let original = coords(&lib);
        let mut changed = original.clone();
        let settings = BreastSettings {
            volume: 2.2,
            projection: 1.8,
            lift: 0.65,
            separation: 0.35,
            ..BreastSettings::default()
        };
        reshape(&lib, &mut changed, 0.0, &settings).unwrap();
        let changed_count = changed
            .iter()
            .zip(&original)
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed_count > 50 && changed_count < original.len() / 3);
        let mut male = original.clone();
        reshape(&lib, &mut male, 1.0, &settings).unwrap();
        assert_eq!(male, original);
    }

    #[test]
    fn spring_settles_and_stays_bounded_at_variable_dt() {
        let settings = BreastSettings::default();
        let mut state = SecondaryMotion {
            offset: Vec3::new(0.1, -0.06, 0.03),
            velocity: Vec3::new(0.5, -0.2, 0.1),
        };
        for dt in [0.016_f32, 0.007, 0.032, 0.004, 0.02]
            .into_iter()
            .cycle()
            .take(400)
        {
            spring(
                &mut state,
                Vec3::ZERO,
                dt.min(1.0 / 120.0),
                (dt / (1.0 / 120.0)).ceil() as usize,
                &settings,
            );
            assert!(state.offset.is_finite() && state.velocity.is_finite());
            assert!(state.offset.length() <= 0.12001);
        }
        assert!(state.offset.length() < 1e-4);
    }

    #[test]
    fn primary_in_place_motion_drives_only_game_breasts() {
        let lib = library();
        let mut recipe = AraRecipe::default();
        for key in ["outfit", "top", "bottom", "jacket", "shoes", "hat"] {
            recipe.params[key] = json!("none");
        }
        recipe.params["gender"] = json!(0.0);
        let character = NativeCharacter::build(&lib, &recipe).unwrap();
        let rig = character.rig.as_ref().unwrap();
        let left = rig
            .bones
            .iter()
            .position(|bone| bone.name == "LeftBreast")
            .unwrap();
        let right = rig
            .bones
            .iter()
            .position(|bone| bone.name == "RightBreast")
            .unwrap();
        let mut motion = BreastMotion::default();
        let mut changed = false;
        for frame in 0..120 {
            let time = frame as f32 / 60.0;
            let primary = rig.pose("walk", time);
            let mut secondary = primary.clone();
            motion.apply(
                rig,
                &mut secondary,
                Vec3::ZERO,
                1.0 / 60.0,
                &BreastSettings::default(),
                0.0,
            );
            changed |= secondary[left] != primary[left] || secondary[right] != primary[right];
            for (index, matrix) in secondary.iter().enumerate() {
                if index != left && index != right {
                    assert_eq!(*matrix, primary[index]);
                }
            }
        }
        assert!(
            changed,
            "in-place walk never produced a breast secondary delta"
        );
        let primary_pause = rig.pose("walk", 119.0 / 60.0);
        let mut frozen = primary_pause.clone();
        let offset = motion.left.offset;
        motion.apply(
            rig,
            &mut frozen,
            Vec3::ZERO,
            0.0,
            &BreastSettings::default(),
            0.0,
        );
        let mut frozen_again = primary_pause.clone();
        motion.apply(
            rig,
            &mut frozen_again,
            Vec3::ZERO,
            0.0,
            &BreastSettings::default(),
            0.0,
        );
        assert_eq!(frozen, frozen_again);
        assert_eq!(motion.left.offset, offset);
        assert_ne!(frozen[left], primary_pause[left]);
        motion.apply(
            rig,
            &mut frozen_again,
            Vec3::ZERO,
            1.0 / 60.0,
            &BreastSettings::default(),
            0.0,
        );
        assert!(frozen_again.iter().all(|m| m.is_finite()));
        let primary = rig.pose("walk", 0.5);
        let mut disabled = primary.clone();
        let settings = BreastSettings {
            dynamics: false,
            ..BreastSettings::default()
        };
        motion.apply(rig, &mut disabled, Vec3::ZERO, 1.0 / 60.0, &settings, 0.0);
        assert_eq!(disabled, primary);
    }

    #[test]
    fn full_rig_breast_names_are_supported() {
        let lib = library();
        let mut recipe = AraRecipe::default();
        recipe.params["rig"] = json!("full");
        for key in ["outfit", "top", "bottom", "jacket", "shoes", "hat"] {
            recipe.params[key] = json!("none");
        }
        let character = NativeCharacter::build(&lib, &recipe).unwrap();
        let rig = character.rig.as_ref().unwrap();
        assert!(rig.bones.iter().any(|bone| bone.name == "breast.L"));
        assert!(rig.bones.iter().any(|bone| bone.name == "breast.R"));
        let mut matrices = rig.pose("walk", 0.2);
        BreastMotion::default().apply(
            rig,
            &mut matrices,
            Vec3::ZERO,
            1.0 / 60.0,
            &BreastSettings::default(),
            0.0,
        );
        assert!(matrices.iter().all(|matrix| matrix.is_finite()));
    }
}
