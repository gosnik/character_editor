//! Small MakeHuman-space anatomy adjustments used before proxy fitting and rigging.
use crate::{
    native_assets::{Array, NativeLibrary, Pack},
    native_morph::fit_proxy,
};
use serde_json::Value;

const FOOT: u8 = 9;
const LOWER_LEG: u8 = 8;
const ARCH: f32 = 24.0 * std::f32::consts::PI / 180.0;

fn array<'a>(p: &'a Pack, name: &str) -> Result<&'a Array, String> {
    p.arrays
        .get(name)
        .ok_or_else(|| format!("array {name} missing"))
}
fn joints(meta: &Value, name: &str, coords: &[[f32; 3]]) -> Result<[f32; 3], String> {
    let a = meta
        .get("joints")
        .and_then(Value::as_object)
        .and_then(|j| j.get(name))
        .or_else(|| {
            meta.get("joints")
                .and_then(Value::as_object)
                .and_then(|j| j.get(&format!("{name}____head")))
        })
        .and_then(Value::as_array)
        .ok_or_else(|| format!("joint {name} missing"))?;
    if a.is_empty() {
        return Err(format!("joint {name} has no vertices"));
    }
    let mut out = [0.; 3];
    for x in a {
        let i = x.as_u64().ok_or("invalid joint vertex")? as usize;
        let p = coords.get(i).ok_or("joint vertex out of bounds")?;
        for d in 0..3 {
            out[d] += p[d]
        }
    }
    for x in &mut out {
        *x /= a.len() as f32
    }
    Ok(out)
}
fn rot_x(p: [f32; 3], o: [f32; 3], a: f32) -> [f32; 3] {
    let (y, z) = (p[1] - o[1], p[2] - o[2]);
    [
        p[0],
        o[1] + y * a.cos() - z * a.sin(),
        o[2] + y * a.sin() + z * a.cos(),
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Pitches each foot about its ankle and blends the lower shin, matching Ara's heel pose.
pub fn arch_feet(lib: &NativeLibrary, coords: &mut [[f32; 3]]) -> Result<(), String> {
    let regions = array(&lib.base, "regions")?.u8()?;
    if regions.len() != coords.len() {
        return Err("regions length does not match coordinates".into());
    }
    let src = coords.to_vec();
    let mut out = src.clone();
    for &(side, suffix) in &[(1.0f32, ".L"), (-1.0, ".R")] {
        let ankle = joints(&lib.base.meta, &format!("foot{suffix}____head"), &src)?;
        let toe = joints(&lib.base.meta, &format!("toe3-1{suffix}____head"), &src)?;
        let ball = rot_x(toe, ankle, ARCH);
        let leg_a = joints(&lib.base.meta, &format!("lowerleg01{suffix}____head"), &src)?;
        let leg_b = ankle;
        let axis = [
            leg_b[0] - leg_a[0],
            leg_b[1] - leg_a[1],
            leg_b[2] - leg_a[2],
        ];
        let den = dot(axis, axis);
        if den <= f32::EPSILON {
            return Err("lower leg joint axis is degenerate".into());
        }
        let foot_axis = [toe[0] - ankle[0], toe[1] - ankle[1], toe[2] - ankle[2]];
        let foot_den = dot(foot_axis, foot_axis);
        if foot_den <= f32::EPSILON {
            return Err("foot joint axis is degenerate".into());
        }
        for (v, &r) in regions.iter().enumerate() {
            if (src[v][0] >= 0.) != (side > 0.) {
                continue;
            }
            if r == FOOT {
                let t = dot(
                    [
                        src[v][0] - ankle[0],
                        src[v][1] - ankle[1],
                        src[v][2] - ankle[2],
                    ],
                    foot_axis,
                ) / foot_den;
                let mut p = rot_x(src[v], ankle, ARCH);
                let w2 = ((t - 0.9) / 0.15).clamp(0., 1.);
                if w2 > 0. {
                    p = rot_x(p, ball, -ARCH * w2)
                }
                out[v] = p
            } else if r == LOWER_LEG {
                let t = dot(
                    [
                        src[v][0] - leg_a[0],
                        src[v][1] - leg_a[1],
                        src[v][2] - leg_a[2],
                    ],
                    axis,
                ) / den;
                if t <= 0.86 {
                    continue;
                }
                out[v] = rot_x(
                    src[v],
                    ankle,
                    ARCH * ((t - 0.86) / 0.14).min(1.).powi(2) * 0.7,
                )
            }
        }
    }
    coords.copy_from_slice(&out);
    Ok(())
}
fn front_hit(x: f32, y: f32, tris: &[[usize; 3]], p: &[[f32; 3]], facing: bool) -> f32 {
    let mut best = f32::NEG_INFINITY;
    for &[ia, ib, ic] in tris {
        let (Some(a), Some(b), Some(c)) = (p.get(ia), p.get(ib), p.get(ic)) else {
            continue;
        };
        if x < a[0].min(b[0]).min(c[0])
            || x > a[0].max(b[0]).max(c[0])
            || y < a[1].min(b[1]).min(c[1])
            || y > a[1].max(b[1]).max(c[1])
        {
            continue;
        }
        if facing && (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) <= 0. {
            continue;
        }
        let den = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if den.abs() < 1e-12 {
            continue;
        }
        let u = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / den;
        let v = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / den;
        if u >= 0. && v >= 0. && u + v <= 1. {
            best = best.max(u * a[2] + v * b[2] + (1. - u - v) * c[2])
        }
    }
    best
}
fn proxy_tris(p: &Pack) -> Result<Vec<[usize; 3]>, String> {
    let sizes = array(p, "faceSizes")?.u8()?;
    let verts = array(p, "faceVerts")?.u32()?;
    let mut out = Vec::new();
    let mut off = 0usize;
    for &n in sizes {
        let n = n as usize;
        let f = verts
            .get(off..off.checked_add(n).ok_or("proxy face overflow")?)
            .ok_or("proxy face vertices truncated")?;
        if n >= 3 {
            for k in 1..n - 1 {
                out.push([f[0] as usize, f[k] as usize, f[k + 1] as usize])
            }
        }
        off += n
    }
    if off != verts.len() {
        return Err("proxy face vertices have trailing entries".into());
    }
    Ok(out)
}
fn body_tris(
    lib: &NativeLibrary,
    coords: &[[f32; 3]],
    teeth: &[[f32; 3]],
    teeth_tris: &[[usize; 3]],
) -> Result<Vec<[usize; 3]>, String> {
    let q = array(&lib.base, "faceVerts")?.u32()?;
    if q.len() % 4 != 0 {
        return Err("body faceVerts must be quads".into());
    }
    let (mut x0, mut x1, mut y0, mut y1, mut z0) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
    );
    for &[a, b, c] in teeth_tris {
        for i in [a, b, c] {
            let p = teeth.get(i).ok_or("tooth triangle out of bounds")?;
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
            z0 = z0.min(p[2])
        }
    }
    let mut out = Vec::new();
    for f in q.chunks_exact(4) {
        let mut near = false;
        for &i in f {
            let p = coords
                .get(i as usize)
                .ok_or("body face vertex out of bounds")?;
            near |= p[0] > x0 - 0.3
                && p[0] < x1 + 0.3
                && p[1] > y0 - 0.3
                && p[1] < y1 + 0.3
                && p[2] > z0 - 0.2
        }
        if near {
            out.push([f[0] as usize, f[1] as usize, f[2] as usize]);
            out.push([f[0] as usize, f[2] as usize, f[3] as usize])
        }
    }
    Ok(out)
}
fn showing(lib: &NativeLibrary, coords: &[[f32; 3]], proxy: &Pack) -> Result<bool, String> {
    let teeth = fit_proxy(proxy, coords)?;
    let tt = proxy_tris(proxy)?;
    if tt.is_empty() {
        return Ok(false);
    }
    let skin = body_tris(lib, coords, &teeth, &tt)?;
    let (mut x0, mut x1, mut y0, mut y1) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for &[a, b, c] in &tt {
        for i in [a, b, c] {
            let p = teeth.get(i).ok_or("tooth triangle out of bounds")?;
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1])
        }
    }
    for xi in 0..=((x1 - x0).max(0.) / 0.01).ceil() as usize {
        let x = x0 + xi as f32 * 0.01;
        for yi in 0..=((y1 - y0).max(0.) / 0.002).ceil() as usize {
            let y = y0 + yi as f32 * 0.002;
            let t = front_hit(x, y, &tt, &teeth, false);
            if t > f32::NEG_INFINITY && t > front_hit(x, y, &skin, coords, true) + 0.001 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
/// Lowers the upper lip shape just enough to hide neutral teeth; returns its applied weight.
pub fn seal_lips(
    lib: &NativeLibrary,
    coords: &mut [[f32; 3]],
    upper_up: &[[f32; 3]],
    teeth_proxy: &Pack,
) -> Result<f32, String> {
    if upper_up.len() != coords.len() {
        return Err("upper lip offset length does not match coordinates".into());
    }
    if !showing(lib, coords, teeth_proxy)? {
        return Ok(0.);
    }
    let at = |w: f32| {
        coords
            .iter()
            .zip(upper_up)
            .map(|(p, d)| [p[0] - d[0] * w, p[1] - d[1] * w, p[2] - d[2] * w])
            .collect::<Vec<_>>()
    };
    let mut lo = 0.;
    let mut hi = 0.4;
    if !showing(lib, &at(hi), teeth_proxy)? {
        for _ in 0..6 {
            let m = (lo + hi) * 0.5;
            if showing(lib, &at(m), teeth_proxy)? {
                lo = m
            } else {
                hi = m
            }
        }
    }
    let w = (hi + 0.06).min(0.45);
    for (p, d) in coords.iter_mut().zip(upper_up) {
        for k in 0..3 {
            p[k] -= d[k] * w
        }
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{collections::BTreeMap, path::PathBuf};
    fn lib(coords: &[[f32; 3]], faces: Vec<u32>, regions: Vec<u8>, meta: Value) -> NativeLibrary {
        NativeLibrary::from_packs(
            Pack {
                meta,
                arrays: BTreeMap::from([
                    (
                        "positions".into(),
                        Array::F32(coords.iter().flatten().copied().collect()),
                    ),
                    ("faceVerts".into(), Array::U32(faces)),
                    ("regions".into(), Array::U8(regions)),
                ]),
            },
            Pack {
                meta: json!({"targets":{}}),
                arrays: BTreeMap::new(),
            },
            json!({}),
            PathBuf::new(),
        )
    }
    #[test]
    fn heels_move_feet_but_not_torso() {
        let mut p = vec![
            [1., 0., 0.],
            [1., 0., 1.],
            [1., 0.1, 0.5],
            [1., 1., 0.],
            [-1., 0., 0.],
            [-1., 0., 1.],
            [-1., 0.1, 0.5],
            [-1., 1., 0.],
            [0., 2., 0.],
        ];
        let before = p.clone();
        let m = json!({"joints":{"foot.L____head":[0],"toe3-1.L____head":[1],"lowerleg01.L____head":[3],"foot.R____head":[4],"toe3-1.R____head":[5],"lowerleg01.R____head":[7]}});
        let l = lib(
            &p,
            vec![],
            vec![FOOT, FOOT, FOOT, LOWER_LEG, FOOT, FOOT, FOOT, LOWER_LEG, 2],
            m,
        );
        arch_feet(&l, &mut p).unwrap();
        assert_ne!(p[2], before[2]);
        assert_eq!(p[8], before[8]);
    }
    #[test]
    fn sealing_lips_reduces_visible_teeth() {
        let mut c = vec![
            [0., 0., 1.],
            [1., 0., 1.],
            [1., 1., 1.],
            [0., 1., 1.],
            [0., 0., 2.],
            [1., 0., 2.],
            [0., 1., 2.],
        ];
        let l = lib(&c, vec![0, 1, 2, 3], vec![0; 7], json!({"joints":{}}));
        let px = Pack {
            meta: json!({"vertexCount":3,"scale":[null,null,null]}),
            arrays: BTreeMap::from([
                ("refs".into(), Array::U32(vec![4, 4, 4, 5, 5, 5, 6, 6, 6])),
                (
                    "weights".into(),
                    Array::F32(vec![1., 0., 0., 1., 0., 0., 1., 0., 0.]),
                ),
                ("offsets".into(), Array::F32(vec![0.; 9])),
                ("faceSizes".into(), Array::U8(vec![3])),
                ("faceVerts".into(), Array::U32(vec![0, 1, 2])),
            ]),
        };
        let mut up = vec![[0.; 3]; 7];
        for d in up.iter_mut().take(4) {
            d[2] = -3.
        }
        let w = seal_lips(&l, &mut c, &up, &px).unwrap();
        assert!(w > 0.);
        assert!(!showing(&l, &c, &px).unwrap());
    }
}
